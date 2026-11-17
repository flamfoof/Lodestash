#!/usr/bin/env node
// gate.mjs — the design-contract gate (spec §10.4, ADR-0023; plan P6 Tasks 6.4–6.5).
//
//   node web/e2e/gate.mjs --gallery
//
// Builds web/, serves the build on loopback, opens the gallery in headless Edge at the contract's
// viewport (1440×900, DPR 1) and measures every specimen marked `data-anchor` with the same
// in-page ruler the contract extractor used (scripts/design/lib/measure.mjs). Each anchor is
// compared with docs/design/contract.json state `library` by the groups anchors.mjs lists for it,
// exempting only the (state, anchor, property) triples docs/design/DEVIATIONS.md records.
//   - primitive specimens (gallery.html): positions are not compared — a specimen is not where the
//     prototype draws it;
//   - the shell specimen (gallery.html#shell): every frame anchor, positions included.
// Every contract anchor must be measured here or listed in DEFERRED with the plan task that will
// measure it; a missing specimen is a failure, never a skip. Writes test-reports/MM-DD-HH-MM.csv.
// Exit 1 on any failure. `LS_BROWSER_CHANNEL=""` uses Playwright's bundled Chromium.

import fs from "node:fs";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ANCHORS } from "../../scripts/design/anchors.mjs";
import { compare, describe, exemptionsFor, readDeviations } from "../../scripts/design/lib/compare.mjs";
import { measureInPage } from "../../scripts/design/lib/measure.mjs";
import { loadPlaywright, serveDir } from "../../scripts/design/lib/prototype.mjs";
import { createReport } from "./lib/report.mjs";

const WEB = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ROOT = path.resolve(WEB, "..");
const STATE = "library";

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

/** The shell specimen (gallery.html#shell) at the contract viewport: every frame anchor. */
const SHELL = [];

/** Anchors no specimen here can reproduce yet, and the task that will measure them. */
const DEFERRED = {
  topbar: "the top bar in place — the shell, plan P6 Task 6.5",
  "topbar.search": "the search well's width is what the top bar leaves it — the shell, Task 6.5",
  sidebar: "the rail in place — the shell, plan P6 Task 6.5",
  "library.banner": "spans the library's main column — LibraryScreen, plan P7",
  "library.toolbar": "the library toolbar in place — LibraryScreen, plan P7",
  "library.toolbar.count": "the result label's own text style — LibraryScreen, plan P7",
  "library.grid": "the asset grid — AssetGrid, plan P7",
  card: "the asset card — AssetCard, plan P7",
  "card.thumb": "the asset card — AssetCard, plan P7",
  "card.body": "the asset card — AssetCard, plan P7",
  "card.name": "the asset card — AssetCard, plan P7",
  "card.meta": "the asset card — AssetCard, plan P7",
};

// ---- run ----

const args = process.argv.slice(2).filter((a) => a !== "--");
if (!args.includes("--gallery")) {
  console.error("usage: node web/e2e/gate.mjs --gallery   (--app arrives with plan P6 Task 6.8)");
  process.exit(2);
}

const report = createReport({ origin: "web/e2e/gate.mjs", root: ROOT });
const contract = JSON.parse(fs.readFileSync(path.join(ROOT, "docs", "design", "contract.json"), "utf8"));
const expected = contract.states[STATE]?.anchors ?? {};
const defs = ANCHORS[STATE];

console.log(`\ndesign gate — gallery against contract state "${STATE}"\n`);

let deviations = [];
await report.check("DEVIATIONS.md parses", async () => {
  deviations = readDeviations(fs.readFileSync(path.join(ROOT, "docs", "design", "DEVIATIONS.md"), "utf8"));
  return `${deviations.length} row(s) exempt measured properties`;
});

await report.check("every contract anchor has a specimen or a deferral", async () => {
  const names = Object.keys(expected);
  if (names.length === 0) throw new Error(`contract.json has no anchors for state "${STATE}"`);
  const covered = new Set([...GALLERY, ...SHELL, ...Object.keys(DEFERRED)]);
  const uncovered = names.filter((n) => !covered.has(n));
  const unknown = [...GALLERY, ...SHELL].filter((n) => !(n in expected));
  if (uncovered.length) throw new Error(`no specimen and no deferral: ${uncovered.join(", ")}`);
  if (unknown.length) throw new Error(`not in the contract: ${unknown.join(", ")}`);
  return `${names.length} anchors: ${new Set([...GALLERY, ...SHELL]).size} measured here, ${Object.keys(DEFERRED).length} deferred`;
});

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

async function openPage(hash) {
  const page = await browser.newPage({ viewport: { width: contract.viewport.width, height: contract.viewport.height }, deviceScaleFactor: 1 });
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

async function measureAll(label, hash, names, skip) {
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
      const ignore = exemptionsFor(deviations, STATE, name);
      const diffs = compare(expected[name], m, defs[name].compare, { ignore: [...ignore], skip });
      if (diffs.length) throw new Error(describe(diffs));
      return ignore.size ? `exempt by DEVIATIONS: ${[...ignore].join(", ")}` : "";
    });
    if (r.ok) passed++;
  }
  await opened.page.close();
  return passed;
}

const galleryPassed = await measureAll("gallery", "", GALLERY, ["pos"]);
const shellPassed = SHELL.length ? await measureAll("shell", "#shell", SHELL, []) : 0;

if (browser) await browser.close();
site?.close();

const file = report.write();
const total = GALLERY.length + SHELL.length;
const failures = report.failures;
console.log(
  `\n${galleryPassed}/${GALLERY.length} gallery anchors, ${shellPassed}/${SHELL.length} shell anchors (with positions) match the contract; ` +
    `${Object.keys(DEFERRED).length} deferred.\n${report.rows.length} checks, ${failures.length} failed — ${path.relative(process.cwd(), file)}\n`,
);
process.exit(failures.length || report.rows.length === 0 || galleryPassed + shellPassed !== total ? 1 : 0);
