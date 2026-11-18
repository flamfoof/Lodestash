#!/usr/bin/env node
// gate.mjs — the design-contract gate (spec §10.4, ADR-0023; plan P6 Tasks 6.4–6.5).
//
//   node web/e2e/gate.mjs --gallery
//
// Builds web/, serves the build on loopback, opens each specimen page in headless Edge at the
// contract's viewport (1440×900, DPR 1) and measures every element marked `data-anchor` with the
// same in-page ruler the contract extractor used (scripts/design/lib/measure.mjs). Each anchor is
// compared with its page's state in docs/design/contract.json by the groups anchors.mjs lists for
// it, exempting only the (state, anchor, property) triples docs/design/DEVIATIONS.md records.
//   - primitive specimens (gallery.html): positions are not compared — a specimen is not where the
//     prototype draws it;
//   - screen specimens (gallery.html#shell, #library, …): the shell with fixture data, every
//     anchor of the state, positions included.
// Every contract anchor must be measured by a page of its state or listed in DEFERRED with the
// plan task that will measure it; a missing specimen is a failure, never a skip. Writes
// test-reports/MM-DD-HH-MM.csv.
// Exit 1 on any failure. `LS_BROWSER_CHANNEL=""` uses Playwright's bundled Chromium.

import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ANCHORS, FRAME_ANCHORS } from "../../scripts/design/anchors.mjs";
import { compare, describe, exemptionsFor, readDeviations } from "../../scripts/design/lib/compare.mjs";
import { measureInPage } from "../../scripts/design/lib/measure.mjs";
import { loadPlaywright, serveDir } from "../../scripts/design/lib/prototype.mjs";
import { createReport } from "./lib/report.mjs";

const WEB = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ROOT = path.resolve(WEB, "..");

// ---- which specimen measures which anchor ----

/** Reproduced by one primitive on its own (web/src/gallery/Gallery.svelte). */
const GALLERY = [
  "topbar.wordmark",
  "topbar.mode",
  "topbar.mode.on",
  "topbar.mode.off",
  "topbar.category",
  "topbar.category.on",
  "topbar.meter",
  "topbar.meter.label",
  "topbar.project",
  "topbar.hardware",
  "sidebar.nav",
  "sidebar.nav.on",
  "sidebar.nav.badge",
  "sidebar.heading",
  "sidebar.facet",
  "sidebar.addSource",
  "library.banner.action",
  "library.sort",
  "library.favorites",
  "library.filters",
  "library.density",
  "library.density.on",
  "card.chip.lead",
  "card.chip",
  "card.chip.more",
  "card.size",
  "card.stage",
  "card.favorite",
  "card.status",
];

/**
 * The pages measured, each against one contract state. `skip` lists compare groups a page cannot
 * answer for (a primitive on the specimen sheet is not where the prototype draws it).
 */
const PAGES = [
  { label: "gallery", hash: "", state: "library", anchors: GALLERY, skip: ["pos"] },
  // The shell alone: every frame anchor, with positions.
  { label: "shell", hash: "#shell", state: "library", anchors: [...FRAME_ANCHORS], skip: [] },
];

/** Anchors no page reproduces yet, per state, and the plan task that will measure them. */
const DEFERRED = {
  library: Object.fromEntries(
    Object.keys(ANCHORS.library)
      .filter((name) => !GALLERY.includes(name) && !FRAME_ANCHORS.includes(name))
      .map((name) => [name, "the Library screen — plan P7 Task 7.6"]),
  ),
  "library+inspector": Object.fromEntries(
    Object.keys(ANCHORS["library+inspector"]).map((name) => [name, "the Inspector — plan P7 Task 7.7"]),
  ),
};
const deferredCount = Object.values(DEFERRED).reduce((n, d) => n + Object.keys(d).length, 0);

// ---- run ----

const args = process.argv.slice(2).filter((a) => a !== "--");
if (!args.includes("--gallery")) {
  console.error("usage: node web/e2e/gate.mjs --gallery   (--app arrives with plan P6 Task 6.8)");
  process.exit(2);
}

const report = createReport({ origin: "web/e2e/gate.mjs", root: ROOT });
const contract = JSON.parse(fs.readFileSync(path.join(ROOT, "docs", "design", "contract.json"), "utf8"));
const STATES = Object.keys(ANCHORS);

console.log(`\ndesign gate — ${PAGES.map((p) => p.label).join(", ")} against contract states ${STATES.join(", ")}\n`);

let deviations = [];
await report.check("DEVIATIONS.md parses", async () => {
  deviations = readDeviations(fs.readFileSync(path.join(ROOT, "docs", "design", "DEVIATIONS.md"), "utf8"));
  return `${deviations.length} row(s) exempt measured properties`;
});

for (const state of STATES) {
  await report.check(`${state}: every contract anchor has a specimen or a deferral`, async () => {
    const names = Object.keys(contract.states[state]?.anchors ?? {});
    if (names.length === 0) throw new Error(`contract.json has no anchors for state "${state}"`);
    const measured = new Set(PAGES.filter((p) => p.state === state).flatMap((p) => p.anchors));
    const deferred = Object.keys(DEFERRED[state] ?? {});
    const uncovered = names.filter((n) => !measured.has(n) && !deferred.includes(n));
    const unknown = [...measured, ...deferred].filter((n) => !names.includes(n));
    if (uncovered.length) throw new Error(`no specimen and no deferral: ${uncovered.join(", ")}`);
    if (unknown.length) throw new Error(`not in the contract: ${unknown.join(", ")}`);
    return `${names.length} anchors: ${measured.size} measured here, ${deferred.filter((n) => !measured.has(n)).length} deferred`;
  });
}

// Building takes longer than a check's 5 s: vite compiles every component.
const build = await report.check(
  "build web/",
  () =>
    new Promise((resolve, reject) => {
      const vite = path.join(WEB, "node_modules", "vite", "bin", "vite.js");
      const child = spawn(process.execPath, [vite, "build", "--logLevel", "warn"], { cwd: WEB, stdio: ["ignore", "inherit", "inherit"] });
      child.on("error", reject);
      child.on("exit", (code) => (code === 0 ? resolve() : reject(new Error(`vite build exited ${code}`))));
    }),
  { budget: 90_000 },
);

let browser = null;
let site = null;
let setupError = build.ok ? null : "the build failed";

if (!setupError) {
  // Launching a browser is a cold start of Edge: allow it longer than a check.
  const launched = await report.check(
    "launch browser and serve the build",
    async () => {
      site = await serveDir(path.join(WEB, "dist"));
      const { chromium } = loadPlaywright();
      const channel = process.env.LS_BROWSER_CHANNEL ?? "msedge";
      browser = await chromium.launch({ headless: true, ...(channel ? { channel } : {}) });
      return channel || "bundled chromium";
    },
    { budget: 30_000 },
  );
  if (!launched.ok) setupError = "the browser did not start";
}

async function openPage(hash, viewport = contract.viewport) {
  const page = await browser.newPage({ viewport: { width: viewport.width, height: viewport.height }, deviceScaleFactor: 1 });
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()));
  page.on("requestfailed", (r) => errors.push(`request failed: ${r.url()}`));
  page.on("response", (r) => r.status() >= 400 && errors.push(`${r.status()} ${r.url()}`));
  await page.goto(`${site.base}gallery.html${hash}`, { waitUntil: "networkidle", timeout: 4000 });
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(150); // entry animations (none are longer)
  return { page, errors };
}

async function measureAll({ label, hash, state, anchors: names, skip }) {
  const expected = contract.states[state]?.anchors ?? {};
  const defs = ANCHORS[state];
  let opened = null;
  if (!setupError) {
    const r = await report.check(`${label}: page loads cleanly`, async () => {
      opened = await openPage(hash);
      // The sprite must parse and hold every icon the page uses, or icons render as empty boxes.
      const missing = await opened.page.evaluate(async () => {
        const text = await (await fetch("/icons.svg")).text();
        const doc = new DOMParser().parseFromString(text, "image/svg+xml");
        if (doc.querySelector("parsererror")) return ["icons.svg does not parse"];
        const ids = new Set([...doc.querySelectorAll("symbol")].map((s) => s.id));
        const used = [...document.querySelectorAll("use")].map((u) => (u.getAttribute("href") ?? "").split("#")[1]);
        return [...new Set(used.filter((id) => !ids.has(id)))].map((id) => `icon not in sprite: ${id}`);
      });
      const problems = [...opened.errors, ...missing];
      if (problems.length) throw new Error(problems.join("; "));
    });
    if (!r.ok && !opened) {
      for (const n of names) report.notRun(`${label}: ${n}`, "the page did not open");
      return 0;
    }
  } else {
    for (const n of names) report.notRun(`${label}: ${n}`, setupError);
    return 0;
  }

  let passed = 0;
  for (const name of names) {
    const r = await report.check(`${label}: ${name}`, async () => {
      const m = await opened.page.evaluate(measureInPage, { anchor: name });
      if (!m.ok) throw new Error(`specimen missing — ${m.reason}`);
      const ignore = exemptionsFor(deviations, state, name);
      const diffs = compare(expected[name], m, defs[name].compare, { ignore: [...ignore], skip });
      if (diffs.length) throw new Error(describe(diffs));
      return ignore.size ? `exempt by DEVIATIONS: ${[...ignore].join(", ")}` : "";
    });
    if (r.ok) passed++;
  }
  await opened.page.close();
  return passed;
}

const passed = [];
for (const page of PAGES) passed.push(await measureAll(page));

// The window floor (ADR-0009): at 900 × 600 nothing in the frame overflows and the rail is the
// 48 px icon rail; below the floor the frame stops shrinking and the page scrolls instead.
const FLOOR = [
  {
    name: "shell at 900×600: top bar and rail fit, rail is icons only",
    viewport: { width: 900, height: 600 },
    probe: () => {
      const bar = document.querySelector('[data-anchor="topbar"]');
      const rail = document.querySelector('[data-anchor="sidebar"]');
      const problems = [];
      if (!bar || !rail) return ["the shell did not render"];
      if (bar.scrollWidth > bar.clientWidth) problems.push(`top bar overflows by ${bar.scrollWidth - bar.clientWidth} px`);
      if (document.documentElement.scrollWidth > innerWidth) problems.push("the page scrolls sideways");
      const w = rail.getBoundingClientRect().width;
      if (Math.abs(w - 48) > 0.5) problems.push(`rail is ${w} px wide, not the 48 px icon rail`);
      const search = document.querySelector('[data-anchor="topbar.search"]').getBoundingClientRect().width;
      if (search < 120) problems.push(`search well is only ${search} px`);
      return problems;
    },
  },
  {
    name: "shell below the floor (800×500) scrolls instead of shrinking",
    viewport: { width: 800, height: 500 },
    probe: () => {
      const shell = document.querySelector('[data-anchor="topbar"]')?.closest(".shell") ?? document.body.firstElementChild;
      const r = shell.getBoundingClientRect();
      const problems = [];
      if (r.width < 900) problems.push(`frame shrank to ${r.width} px`);
      if (r.height < 600) problems.push(`frame shrank to ${r.height} px tall`);
      if (document.documentElement.scrollWidth <= innerWidth) problems.push("the page does not scroll sideways");
      return problems;
    },
  },
];
for (const f of FLOOR) {
  if (setupError) {
    report.notRun(f.name, setupError);
    continue;
  }
  await report.check(f.name, async () => {
    const { page, errors } = await openPage("#shell", f.viewport);
    const problems = [...errors, ...(await page.evaluate(f.probe))];
    await page.close();
    if (problems.length) throw new Error(problems.join("; "));
  });
}

if (browser) await browser.close();
site?.close();

const file = report.write();
// (floor checks are counted among report.rows / failures)
const failures = report.failures;
const tally = PAGES.map((p, i) => `${passed[i]}/${p.anchors.length} ${p.label}${p.skip.includes("pos") ? "" : " (with positions)"}`);
console.log(
  `\n${tally.join(", ")} match the contract; ${deferredCount} deferred.\n` +
    `${report.rows.length} checks, ${failures.length} failed — ${path.relative(process.cwd(), file)}\n`,
);
const allPassed = PAGES.every((p, i) => passed[i] === p.anchors.length);
process.exit(failures.length || report.rows.length === 0 || !allPassed ? 1 : 0);
