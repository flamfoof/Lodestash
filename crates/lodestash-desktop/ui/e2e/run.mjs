/**
 * End-to-end checks against the running desktop app.
 *
 * Scope rule: this suite only covers what fails **silently** — behaviour that is invisible to
 * `cargo test` because it lives in the browser, and invisible in a screenshot because it looks
 * like an empty state. Anything assertable in Rust stays in Rust; this is not a second home for
 * the paging or Range-parsing tests.
 *
 * No test framework, deliberately: plain asserts and one runner keep the dependency count at
 * `playwright-core` alone.
 */
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { launch, BIN, HANDSHAKE } from "./harness.mjs";

/** Repo root, so a run's report lands next to the code rather than inside the UI package. */
const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "..");

/**
 * The viewport every check is entitled to find the app in, captured once the app is up.
 *
 * One check resizes the window on purpose — the responsive ladder, because media queries cannot be
 * driven any other way — and it restores it afterwards. This is what makes "afterwards" enforceable
 * for *every* check rather than only that one: a resize left behind silently re-runs everything
 * after it at a width nobody chose, and the failures land far from the cause. That has happened
 * twice here (once from a leaked app, once from a restore that threw and was swallowed), and both
 * times the tell was a suite that failed everywhere at once and passed in isolation.
 */
let baseline = null;
let restoreViewport = async () => {};

const viewport = () => page.evaluate(() => [window.innerWidth, window.innerHeight]);

/**
 * Poll an async predicate until it holds. `page.waitForFunction` runs *inside* the page and so
 * cannot await a Tauri `invoke` from this side; several checks need exactly that — press a key,
 * then wait for the catalog to agree — and a fixed sleep would either be flaky or slow.
 */
async function waitFor(pred, message, timeout = 5000, interval = 50) {
  const until = Date.now() + timeout;
  for (;;) {
    if (await pred()) return;
    if (Date.now() > until) throw new Error(message);
    await new Promise((r) => setTimeout(r, interval));
  }
}

const results = [];

/**
 * How long a check may take before it counts as a failure rather than a slow pass.
 *
 * A check that hangs has stopped telling you anything: the suite sits there, the cause scrolls
 * away, and a wait that quietly grew from 200 ms to a minute reads as "e2e is slow" rather than
 * "this regressed". Five seconds is the ceiling for anything driving the UI — every `waitFor` in
 * this file already polls against a deadline well under it.
 *
 * A check that genuinely needs longer says so at its own call site, `check(name, { budget }, fn)`,
 * with the reason. The number is then a reviewable claim about that one check instead of a blanket
 * allowance every future check inherits. `E2E_BUDGET_MS` raises the default for a measuring run.
 */
const DEFAULT_BUDGET_MS = Number(process.env.E2E_BUDGET_MS) || 5000;

/**
 * Where a check was written, read off the call stack: the enclosing function and `file:line`.
 *
 * The report is read later by someone holding only a check's name, and names describe behaviour,
 * not location. This makes a row something you can open — and when a check comes from a helper
 * rather than the top level, it names the helper.
 */
function origin() {
  const frames = (new Error().stack || "").split("\n").slice(1);
  // Frame 0 is `origin` itself and frame 1 is `check`; the first frame below those is the caller.
  const frame = frames.find((f) => !/\bat (origin|check)\b/.test(f) && f.includes("run.mjs"));
  if (!frame) return ["", ""];
  const named = frame.match(/at\s+(?:async\s+)?([\w$.<>]+)\s+\(/);
  const at = frame.match(/run\.mjs:(\d+):\d+/);
  return [named ? named[1] : "top-level", at ? `e2e/run.mjs:${at[1]}` : ""];
}

// `check(name, fn)`, or `check(name, { budget }, fn)` — options go before the body so a declared
// budget sits next to the name instead of sixty lines below it.
async function check(name, ...args) {
  const fn = args.pop();
  let run = Promise.resolve();
  const { budget = DEFAULT_BUDGET_MS } = args[0] ?? {};
  const [fromFn, site] = origin();
  const started = performance.now();
  let timer;
  let timedOut = false;
  const record = (ok, error = "") =>
    results.push({ ok, name, seconds: (performance.now() - started) / 1000, fromFn, site, error });
  try {
    // A timed-out check keeps running — nothing here can cancel someone else's `await` — so it is
    // given a bounded moment to finish (below) before the next check starts. Otherwise its tail —
    // a toast still up, a selection still changing — lands inside the next check and fails that
    // one instead, which is the cascade a budget exists to prevent.
    run = fn();
    await Promise.race([
      run,
      new Promise((_, reject) => {
        timer = setTimeout(() => {
          timedOut = true;
          reject(
            new Error(
              `took longer than ${(budget / 1000).toFixed(1)}s — a check that really needs ` +
                `longer must say so: check(name, { budget }, fn)`,
            ),
          );
        }, budget);
      }),
    ]);
    clearTimeout(timer);
    if (baseline) {
      const got = await viewport();
      // ±1px, the same slack the restore below settles for — they disagreed until 2026-08-26,
      // and that disagreement is what a 30/87 run looked like. `Browser.setWindowBounds` reports
      // **frame** geometry and applies **client** geometry, so a restore lands within a pixel of
      // where it started and not reliably on it; the restore already waits on `<= 1` and then
      // returned happy, whereupon this compared exactly and failed the *next* eighty checks. The
      // leak this guard exists to catch is a check that forgot to restore at all — hundreds of
      // pixels, an entire responsive rung — never one.
      if (Math.abs(got[0] - baseline[0]) > 1 || Math.abs(got[1] - baseline[1]) > 1) {
        // Put it back before reporting, so one careless check cannot cascade into eighty
        // unrelated failures — the failure mode this guard exists to make legible, not to spread.
        await restoreViewport();
        throw new Error(
          `left the app at ${got.join("\u00d7")}, not the ${baseline.join("\u00d7")} it started at — ` +
            `a check that resizes the window must put it back`,
        );
      }
    }
    record(true);
    console.log(`  ok   ${name}`);
  } catch (e) {
    clearTimeout(timer);
    record(false, e.message.split("\n")[0]);
    if (timedOut) {
      await Promise.race([run.catch(() => {}), new Promise((r) => setTimeout(r, 10_000))]);
    }
    console.log(`  FAIL ${name}\n       ${e.message.split("\n")[0]}`);
  }
}

/**
 * One CSV per run under `test-reports/`, named `month-day-hour-minute` in local 24-hour time.
 *
 * CSV because the interesting question is asked *across* runs — which check is drifting towards
 * its budget, which one only fails on a busy machine — and that is a spreadsheet question rather
 * than a log-reading one.
 */
function writeReport() {
  const d = new Date();
  const p2 = (n) => String(n).padStart(2, "0");
  const stamp = `${p2(d.getMonth() + 1)}-${p2(d.getDate())}-${p2(d.getHours())}-${p2(d.getMinutes())}`;
  const dir = path.join(REPO_ROOT, "test-reports");
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, `${stamp}.csv`);
  const cell = (v) => (/[",\n]/.test(String(v)) ? `"${String(v).replace(/"/g, '""')}"` : String(v));
  const rows = [
    ["check", "result", "seconds", "origin_fn", "origin_site", "error"],
    ...results.map((r) => [
      r.name,
      r.ok ? "pass" : "fail",
      r.seconds.toFixed(3),
      r.fromFn,
      r.site,
      r.error,
    ]),
  ];
  fs.writeFileSync(file, rows.map((r) => r.map(cell).join(",")).join("\n") + "\n");
  return file;
}

const app = await launch();
const { page, invoke } = app;
// Playwright's own default is 30 s per step, which is how a selector that matched nothing sat
// silently for thirty seconds inside a check that then passed. A step that needs longer passes its
// own `{ timeout }`, where it can be read and questioned.
page.setDefaultTimeout(5000);
console.log(`\ndriving ${page.url()}\n`);

// Arm the per-check viewport guard against whatever size the app actually opened at — not a
// hard-coded number, because that is the user's window and it is theirs to size.
{
  const cdp = await page.context().newCDPSession(page);
  const { windowId } = await cdp.send("Browser.getWindowForTarget");
  const { bounds } = await cdp.send("Browser.getWindowBounds", { windowId });
  baseline = await viewport();
  restoreViewport = async () => {
    if (bounds.windowState && bounds.windowState !== "normal") {
      await cdp
        .send("Browser.setWindowBounds", { windowId, bounds: { windowState: bounds.windowState } })
        .catch(() => {});
    } else {
      const { left, top, width, height } = bounds;
      await cdp
        .send("Browser.setWindowBounds", { windowId, bounds: { left, top, width, height } })
        .catch(() => {});
    }
    await page
      .waitForFunction((w) => Math.abs(window.innerWidth - w) <= 1, baseline[0], { timeout: 4000 })
      .catch(() => {});
  };
  console.log(`  …    app viewport ${baseline.join("\u00d7")} — every check must leave it that way\n`);
}

try {
  // First-run onboarding (FEATURES §8) covers the window on a fresh config. Every DOM
  // interaction below needs it gone — and dismissing it is itself the behaviour under test:
  // the modal must never trap the library behind it. A returning user (skip_onboarding set)
  // never sees it, which this check also tolerates.
  await check("onboarding is dismissed, and the library stays reachable", async () => {
    // **This does not test onboarding**, and saying so is the point: `launch()` spends six
    // seconds clicking the panel away before this line runs, so the panel is never present and
    // the branch below never taken. It was written as "if present, assert things about it" and
    // has therefore asserted nothing since the day it landed — which is how a rewritten
    // onboarding screen nearly shipped unseen. What is checked here is the thing that matters to
    // the other ninety checks: the panel is *gone* and the library behind it is reachable.
    // The panel itself is a `keepOnboarding` launch — `node e2e/onboarding-shot.mjs` — plus the
    // command-level check below.
    const box = page.locator('[data-testid="onboarding"]');
    assert.equal(await box.count(), 0, "onboarding survived the harness — it will swallow every click");
    const s = await invoke("library_status");
    assert.equal(s.open, true, "no catalog open — run a scan in the egui build first");
  });

  /**
   * Onboarding §1's two reads, checked at the command layer because the panel itself only renders
   * on a machine that has not dismissed it — and the wiring is what breaks silently.
   *
   * `unity_cache_candidates` is the detect-without-importing half: onboarding has to *show* the
   * Asset Store cache before the user agrees to index it, and the importing version would crawl
   * the machine to answer the question.
   */
  await check("onboarding can list sources and detect the cache without indexing anything", async () => {
    const before = await invoke("library_status");

    // The source rows need `kind` (folder vs auto-detected pill) and `last_scanned_at`
    // ("last scanned …"). Both are on `SourceInfo` and were simply not carried to the frontend.
    for (const src of before.sources) {
      assert.equal(typeof src.kind, "string", `source ${src.name} has no kind`);
      assert.ok(
        src.last_scanned_at === null || typeof src.last_scanned_at === "number",
        `source ${src.name} has a malformed last_scanned_at`,
      );
    }

    const candidates = await invoke("unity_cache_candidates");
    assert.ok(Array.isArray(candidates), "unity_cache_candidates did not return a list");
    const roots = before.sources.map((s) => s.root_path.toLowerCase());
    for (const c of candidates) {
      assert.equal(typeof c.path, "string", "a candidate has no path");
      assert.equal(typeof c.packs, "number", "a candidate has no pack count");
      assert.ok(
        !roots.includes(c.path.toLowerCase()),
        `${c.path} is already a source and must not be offered again`,
      );
    }

    // Detection must not index. The whole point of the separate command is that asking is free.
    const after = await invoke("library_status");
    assert.equal(
      after.total_assets,
      before.total_assets,
      "detecting the cache changed the library — it crawled instead of looking",
    );
    assert.equal(after.sources.length, before.sources.length, "detecting the cache added a source");
  });

  await check("the shell opens a real catalog", async () => {
    const s = await invoke("library_status");
    assert.equal(s.open, true, "no catalog open — run a scan in the egui build first");
    assert.ok(s.total_assets > 0, "catalog has no assets");
    assert.ok(s.sources.length > 0, "catalog has no sources");
  });

  // The bug this exists for: `Library` loaded only `entries_for_source`, which filters
  // `package_id IS NULL`. That showed 1,728 of 7,691 assets — 21% of the library — with nothing
  // saying so. It reads as a small library, not a missing query, so it never gets reported.
  await check("in-pack assets are loaded, not just plain on-disk files", async () => {
    const s = await invoke("library_status");
    assert.ok(s.in_pack > 0, "no in-pack assets loaded — a hydrate query is missing");
    assert.ok(
      s.in_pack < s.total_assets,
      "every asset is in-pack, which means the on-disk query is missing instead",
    );
  });

  await check("the grid renders a card per returned item", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100 },
    });
    const cards = await page.locator(".card").count();
    assert.equal(cards, p.items.length, "DOM card count disagrees with grid_page");
  });

  await check("typing filters the grid", async () => {
    const before = await page.locator(".card").count();
    await page.fill('input[type="text"]', "Stairs");
    await page.waitForFunction((n) => document.querySelectorAll(".card").length !== n, before, {
      timeout: 5000,
    });
    const after = await page.locator(".card").count();
    assert.ok(after > 0 && after < before, `expected a narrower result, got ${after} of ${before}`);
  });

  // ── D1 + D2: one search box, and one bar that says what is filtered ──────────────────────────
  // The bar is the answer to "why am I seeing these?", so the check that matters is not that it
  // renders — it is that removing a chip actually drives the control the chip came from. A bar that
  // draws chips but clears nothing is worse than no bar: it claims a reset it did not perform.
  await check("a query renders a chip, and clearing the chip clears the query", async () => {
    await page.fill('input[type="text"]', "Stairs");
    await page.waitForSelector('[data-testid="filter-bar"]', { timeout: 5000 });
    await page.click('[data-testid="chip-x-q"]');
    await page.waitForFunction(
      () => !document.querySelector('[data-testid="filter-bar"]'),
      undefined,
      { timeout: 5000 },
    );
    const left = await page.inputValue('input[type="text"]');
    assert.equal(left, "", `the chip cleared the bar but not the search box (still "${left}")`);
  });

  await check("Clear all resets every axis at once", async () => {
    await page.fill('input[type="text"]', "Stairs");
    await page.click('[data-testid="tab-models"]');
    await page.click('[data-testid="status-identified"]');
    await page.waitForSelector('[data-testid="filter-bar"]', { timeout: 5000 });
    const chips = await page.locator('[data-testid="filter-bar"] .chip').count();
    assert.ok(chips >= 3, `expected a chip per active axis, got ${chips}`);

    await page.click('[data-testid="filter-clear"]');
    await page.waitForFunction(
      () => !document.querySelector('[data-testid="filter-bar"]'),
      undefined,
      { timeout: 5000 },
    );
    // Every axis, not just the one the bar was rendered from — this is the reset that used to be
    // "click the sidebar's Library row" and nothing told you about.
    const state = await page.evaluate(() => ({
      q: document.querySelector('input[type="text"]')?.value ?? "",
      tab: document.querySelector('[data-testid="tab-all"]')?.className ?? "",
      status: document.querySelector('[data-testid="status-all"]')?.getAttribute("aria-pressed"),
    }));
    assert.equal(state.q, "", "the query survived Clear all");
    assert.ok(state.tab.includes("on"), "the type tab did not go back to All");
    assert.equal(state.status, "true", "the status facet did not go back to All");
  });

  // ADR-0007: the AI is offered, never run for you. If typing ever started a semantic search this
  // check fails — and so would every keystroke, on a 7k-asset library.
  await check("the AI is offered inside the results, not run by typing", async () => {
    await page.fill('input[type="text"]', "Stairs");
    await page.waitForSelector('[data-testid="escalate"]', { timeout: 5000 });
    const ranked = await page.locator('[data-testid="ranked-header"]').count();
    assert.equal(ranked, 0, "typing produced a ranked AI result — the escalation ran itself");
    await page.fill('input[type="text"]', "");
  });

  // §2.6: the state the old copy dead-ended in. A description is not a filename, and the engine
  // that can answer it has to be the primary action here rather than a toggle nobody flipped.
  await check("an empty result offers the AI instead of dead-ending", async () => {
    await page.fill('input[type="text"]', "zzzqqqnotathinginanylibrary");
    await page.waitForSelector('[data-testid="empty"]', { timeout: 5000 });
    const offer = await page.locator('[data-testid="empty-ask-ai"]').count();
    assert.equal(offer, 1, "the empty state does not offer the AI escalation");
    await page.fill('input[type="text"]', "");
  });

  // T4's whole point: the grid used to filter with `contains()` over file names, so a query only
  // ever reached what was spelled in the name. FTS5 indexes name + description + tags, and a
  // tag-only hit is the proof the index is actually being consulted.
  await check("search reaches tags, not just file names", async () => {
    const q = (search) =>
      invoke("grid_page", {
        query: { tab: "all", search, status: "all", sort_by: "relevance", sort_asc: true,
                 favorites_only: false, page: 0, page_size: 200 },
      });
    // Any asset whose name does not contain the query text can only have matched through the
    // index. Probing a few terms keeps this from depending on one particular tagged asset.
    let proof = null;
    for (const term of ["haunted", "dark", "environment", "level", "dungeon"]) {
      const r = await q(term);
      const viaIndex = r.items.filter((i) => !i.name.toLowerCase().includes(term));
      if (viaIndex.length > 0) {
        proof = { term, name: viaIndex[0].name, total: r.total };
        break;
      }
    }
    assert.ok(
      proof,
      "no query matched an asset whose name lacks the term — FTS is not being consulted (or this catalog has no tags: run Identify in the egui build)",
    );
    console.log(`       "${proof.term}" matched ${proof.name} through the index`);
  });

  // Ranked order, and the guard on `grid::relevance_key`: `Option`'s ordering puts None first,
  // which would bury every real hit under the assets that merely matched as substrings.
  await check("relevance puts index hits above substring-only matches", async () => {
    const q = (sort_by) =>
      invoke("grid_page", {
        query: { tab: "all", search: "dungeon", status: "all", sort_by, sort_asc: true,
                 favorites_only: false, page: 0, page_size: 200 },
      });
    const rel = await q("relevance");
    const byName = await q("name");
    assert.ok(rel.total > 1, "need more than one match to have an order at all");
    assert.equal(rel.total, byName.total, "the sort must not change what matched");
    assert.notDeepEqual(
      rel.items.map((i) => i.asset_id),
      byName.items.map((i) => i.asset_id),
      "relevance produced name order — the ranks are not reaching the sort",
    );
  });

  // The check that would have caught the `lodestash://localhost` routing bug on the spot. A broken
  // image is indistinguishable from a styled placeholder in a screenshot, but its naturalWidth
  // is 0 — so the failure that cost a debugging session becomes one assertion.
  //
  // `complete` is not optional here: cards render `loading="lazy"`, so an image below the fold has
  // naturalWidth 0 while being perfectly healthy. Asserting on that measures the scroll position,
  // not the route. A 404 or a 503 still sets `complete`, so the bug this exists for is still caught.
  await check("every vector preview decodes (naturalWidth > 0)", async () => {
    const imgs = await page.locator(".thumb img").evaluateAll((els) =>
      els.map((e) => ({ src: e.getAttribute("src"), w: e.naturalWidth, done: e.complete })),
    );
    assert.ok(imgs.length > 0, "no preview images on screen — nothing was exercised");
    const loaded = imgs.filter((i) => i.done);
    assert.ok(loaded.length > 0, "no preview image finished loading — nothing was exercised");
    const broken = loaded.filter((i) => i.w === 0);
    assert.equal(broken.length, 0, `${broken.length} broken: ${broken.map((b) => b.src).join(", ")}`);
  });

  await check("clicking a card fills the inspector with that asset", async () => {
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(600);
    const first = page.locator(".card").first();
    const name = (await first.locator(".name").textContent())?.trim();
    await first.click();
    await page.waitForSelector('[data-testid="inspector-name"]', { timeout: 5000 });
    const shown = (await page.locator('[data-testid="inspector-name"]').textContent())?.trim();
    assert.equal(shown, name, "inspector shows a different asset than the one clicked");
  });

  // Descending must be the exact reverse, tie-breaks included. If it is not, page 2 of a
  // descending sort can repeat or drop rows page 1 already showed — invisible on page 1,
  // which is the only page anyone checks by hand. Sorted on `size`, where thousands of assets
  // tie, so a comparator that reverses only the primary key fails here and not on `name`.
  await check("descending sort is the exact reverse of ascending", async () => {
    const all = (asc) =>
      invoke("grid_page", {
        query: {
          tab: "all", search: "", status: "all",
          sort_by: "size", sort_asc: asc, page: 0, page_size: 100000,
        },
      }).then((p) => p.items.map((i) => i.asset_id));
    const [up, down] = await Promise.all([all(true), all(false)]);
    assert.ok(up.length > 100, `need a real library, got ${up.length} rows`);
    assert.deepEqual(down, up.slice().reverse());
  });

  await check("the status facet folds discovered and indexed into Unidentified", async () => {
    const count = async (status) =>
      (await invoke("grid_page", {
        query: { tab: "all", search: "", status, sort_by: "name", sort_asc: true, page: 0, page_size: 1 },
      })).total;
    const [all, un, id, conf] = await Promise.all(
      ["all", "unidentified", "identified", "confirmed"].map(count),
    );
    assert.equal(un + id + conf, all, "the three status buckets must partition the library");
  });

  // Favorites is a filter over the library, not its own dataset. Routing it at the same grid
  // without passing the flag shows the whole library under a "Favorites" heading — which reads
  // as "everything is starred" rather than as a bug, so nobody reports it.
  await check("the Favorites route narrows to starred assets", async () => {
    const s = await invoke("library_status");
    const all = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 1 },
    });
    const fav = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: true, page: 0, page_size: 500 },
    });
    assert.equal(fav.total, s.favorites, "the filter must agree with the catalog's own count");
    assert.ok(fav.total <= all.total, "favourites cannot exceed the library");
    assert.ok(fav.items.every((i) => i.favorite), "every returned item must be starred");
  });

  // The check above exercises the Rust filter but calls `invoke` directly, so it would pass even
  // with the routing bug intact. This one goes through the sidebar, which is where the defect
  // actually was: the route rendered the shared grid without passing the flag.
  await check("clicking Favorites in the nav shows only starred cards", async () => {
    await page.fill('input[type="text"]', "");
    const s = await invoke("library_status");
    // The toolbar's ★ Favorites toggle is also a "Favorites" button now — scope to the sidebar
    // row, which is the route this check exists to pin down.
    await page
      .getByRole("navigation", { name: "Main" })
      .getByRole("button", { name: "Favorites" })
      .click();
    await page.waitForFunction(
      (n) => document.querySelector("h1")?.textContent?.trim() === "Favorites" &&
             (document.querySelectorAll(".card").length === n ||
              document.querySelector('[data-testid="empty"]') !== null),
      s.favorites,
      { timeout: 5000 },
    );
    const cards = await page.locator(".card").count();
    assert.equal(cards, s.favorites, `Favorites rendered ${cards} cards for ${s.favorites} favourites`);
    const starred = await page
      .locator(".card")
      // `.star` is on *every* card now (it is the control that makes a favourite, not a
      // read-out), so counting it would pass for any grid at all. `.star.on` is the lit one.
      .evaluateAll((els) => els.filter((e) => e.querySelector(".star.on")).length);
    assert.equal(starred, cards, "every card on Favorites must carry a lit star");
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForTimeout(500);
  });

  // The card's ☆ became a control (UI-SPEC §2.5). Before this, the grid could only *report* a
  // favourite — you had to open the inspector to make one — and a star that renders but does
  // not write looks identical in a screenshot to one that does.
  //
  // Self-restoring: the second click is both the cleanup and the other half of the assertion,
  // so the developer's catalog ends where it started even if the check fails midway.
  await check("the card star writes, and writes back", async () => {
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(600);
    const before = (await invoke("library_status")).favorites;
    // Pick an *unstarred* card so the first click is always an add, whatever the catalog holds —
    // then hold on to **that card's own name**. Re-finding the star by `.star.on` would click
    // whichever lit star came first in the DOM, i.e. someone's pre-existing favourite: the
    // catalog count returns to `before` either way, so the assertion would stay green while
    // un-favouriting an asset the test never touched.
    const card = page.locator(".card").filter({ hasNot: page.locator(".star.on") }).first();
    assert.ok((await card.count()) > 0, "no unstarred card on screen to exercise");
    const name = (await card.locator(".name").textContent())?.trim();
    const mine = page.locator(".card", { has: page.getByText(name, { exact: true }) }).first();
    try {
      await card.getByTestId("card-star").click();
      // Scoped to this card: a global `.star.on` count is already satisfied by any pre-existing
      // favourite, so it would return on the first poll and race the async write.
      await mine.locator(".star.on").waitFor({ state: "attached", timeout: 5000 });
      assert.equal(
        (await invoke("library_status")).favorites,
        before + 1,
        "clicking the star did not reach the catalog",
      );
    } finally {
      await mine.getByTestId("card-star").click();
      await page.waitForTimeout(400);
    }
    assert.equal(
      (await invoke("library_status")).favorites,
      before,
      "un-starring did not undo the write — the developer's catalog was left dirty",
    );
  });

  // Double-click at a card's centre is how Quick Look opens. Shortening the thumb to the
  // spec's 104px pulled the hover-revealed action tray up toward that centre, and an
  // `opacity: 0` element still takes clicks — so the double-click hit a button nobody could
  // see and Quick Look never opened. It looked like the 3D viewer hanging, three checks later.
  //
  // Asserting on geometry rather than on the symptom: whatever sits at the centre of a card
  // must be the card.
  await check("a card's centre belongs to the card, not to a hover control", async () => {
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(600);
    const owner = await page.locator(".card").first().evaluate((card) => {
      const r = card.getBoundingClientRect();
      // Hover first: the tray is only clickable once revealed, which is exactly the state a
      // real double-click happens in.
      card.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
      const hit = document.elementFromPoint(r.x + r.width / 2, r.y + r.height / 2);
      return { tag: hit?.tagName, cls: hit?.className?.toString() ?? "", inCard: card.contains(hit) };
    });
    assert.ok(owner.inCard, "the card's centre is covered by something outside the card");
    assert.ok(
      !/\bact\b|\bid-btn\b|\bsel\b/.test(owner.cls),
      `a hover-only control sits under the card's centre (${owner.cls}) — double-click cannot reach the card`,
    );
  });

  // Most of a real Unity library's audio is still inside a `.unitypackage`, so `lodestash://thumb`
  // answers 503 and no waveform raster exists — which used to leave 1,841 sounds rendering as one
  // wall of identical speaker glyphs. That reads as an empty grid rather than as a bug, so nobody
  // reports it.
  //
  // Both paths are pinned here, because the interesting failure is picking the *wrong* one: the
  // client-side silhouette must not shadow the real envelope the backend can draw.
  await check("in-pack sounds draw a silhouette; rasterisable ones keep the real envelope", async () => {
    const sounds = (
      await invoke("grid_page", {
        query: { tab: "sounds", search: "", status: "all", sort_by: "name", sort_asc: true,
                 favorites_only: false, page: 0, page_size: 100000 },
      })
    ).items;
    assert.ok(sounds.length > 0, "no audio in the library — nothing was exercised");

    const show = async (item) => {
      await page.fill('input[type="text"]', item.name);
      await page.waitForFunction(
        (n) => document.querySelector(".card .name")?.textContent?.trim() === n,
        item.name,
        { timeout: 5000 },
      );
      // The <img> resolves (or 503s into the fallback) a tick after the card mounts.
      await page.waitForTimeout(700);
      return page.locator(".card").first();
    };

    const packed = sounds.find((i) => i.in_pack);
    if (packed) {
      const card = await show(packed);
      assert.equal(await card.locator(".wave").count(), 1, `no silhouette on in-pack ${packed.name}`);
      assert.equal(await card.locator(".wave .bar").count(), 5, "UI-SPEC §2.5 specifies 5 bars");
    }

    const loose = sounds.find((i) => !i.in_pack);
    if (loose) {
      await show(loose);
      // An uncached waveform is decoded from the audio on demand — seconds, not milliseconds, the
      // first time. Wait for the decode rather than for a fixed delay, or this asserts on a
      // half-loaded <img> and reads as the silhouette winning.
      await page.waitForFunction(
        () => {
          const img = document.querySelector(".card .thumb img");
          return (img && img.naturalWidth > 0) || !!document.querySelector(".card .wave");
        },
        null,
        { timeout: 20000 },
      );
      const w = await page.locator(".card").first().locator(".thumb img")
        .evaluateAll((e) => e.map((i) => i.naturalWidth));
      // A silhouette here means the *backend* could not decode this file (codec support is a
      // property of the machine, not of the code under test) — report it and move on. Only a
      // present-but-broken <img> is a real failure of the precedence rule.
      if (w.length === 0) {
        console.log(`       (skipped loose ${loose.name}: backend could not rasterise it)`);
      } else {
        assert.ok(
          w[0] > 0,
          `${loose.name} rasterised, so the drawn envelope must win over the silhouette (got [${w}])`,
        );
      }
    }

    // Heights are hashed off the asset id: stable per asset, different between neighbours. A
    // constant silhouette would still look like a waveform and carry no information at all.
    await page.fill('input[type="text"]', "");
    await page.getByTestId("tab-sounds").click();
    await page.waitForTimeout(900);
    const shapes = await page
      .locator(".card .wave")
      .evaluateAll((els) =>
        els.slice(0, 12).map((e) =>
          [...e.querySelectorAll(".bar")].map((b) => b.style.getPropertyValue("--h")).join(","),
        ),
      );
    if (shapes.length > 1) {
      assert.ok(new Set(shapes).size > 1, "every sound card drew the same waveform");
    }
    await page.getByTestId("tab-all").click();
    await page.waitForTimeout(500);
  });

  // B12b removed the toolbar's duplicate status chips, leaving the sidebar facet as the only
  // control. If that removal had orphaned `view.status`, the facet would light up and the grid
  // would not move — which looks like a slow query rather than a dead control.
  await check("the sidebar STATUS facet is the one control that filters the grid", async () => {
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForTimeout(400);
    const s = await invoke("library_status");
    await page.getByRole("button", { name: /^Unidentified/ }).click();
    await page.waitForFunction(
      (n) =>
        document.querySelector('[data-testid="result-count"]')?.textContent?.replace(/\D/g, "") ===
        String(n),
      s.unidentified,
      { timeout: 5000 },
    );
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForTimeout(400);
  });

  // The facet ids are a contract between two crates and a TypeScript popover. A chip whose id the
  // backend does not parse fails the *whole* `grid_page` command, so the grid stops responding
  // entirely — this asserts every published id survives the round trip it was minted for.
  await check("every facet id filter_facets publishes is one grid_page accepts", async () => {
    const f = await invoke("filter_facets");
    assert.ok(f.tri_buckets.length === 4, `expected 4 triangle buckets, got ${f.tri_buckets.length}`);
    assert.ok(f.sizes.length === 4, `expected 4 size tiers, got ${f.sizes.length}`);
    const base = { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
                   page: 0, page_size: 1 };
    const all = (await invoke("grid_page", { query: base })).total;

    let triSum = 0;
    for (const b of f.tri_buckets) {
      const q = { ...base, filter: { traits: [], tri_buckets: [b.id], sizes: [] } };
      triSum += (await invoke("grid_page", { query: q })).total;
    }
    const everyBucket = await invoke("grid_page", {
      query: { ...base, filter: { traits: [], tri_buckets: f.tri_buckets.map((b) => b.id), sizes: [] } },
    });
    assert.equal(triSum, everyBucket.total, "the buckets must partition the assets that have a mesh");
    assert.ok(everyBucket.total > 0, "this library has models — some bucket must match");
    assert.ok(everyBucket.total < all, "assets with no triangle count must be excluded");

    // Size tiers partition the *whole* library: every file has a size, so nothing falls out.
    let sizeSum = 0;
    for (const s of f.sizes) {
      const q = { ...base, filter: { traits: [], tri_buckets: [], sizes: [s.id] } };
      sizeSum += (await invoke("grid_page", { query: q })).total;
    }
    assert.equal(sizeSum, all, "S+M+L+XL must add up to the library");
  });

  // The check above talks to Rust directly, so it passes even if the popover never sends what it
  // collected. This one drives the chips: open ⚑, click a size tier, and require the grid to
  // actually narrow — then require Clear all to give it back.
  await check("the Filters popover narrows the grid and Clear all restores it", async () => {
    // Assert on the **result total**, not the card count: the grid pages at 100, so a filter that
    // cuts the library in half still renders a full page and card count proves nothing.
    const total = () =>
      page.getByTestId("result-count").textContent().then((t) => Number(t.replace(/\D/g, "")));
    try {
      await page.fill('input[type="text"]', "");
      await page.waitForTimeout(500);
      const before = await total();
      assert.ok(before > 100, `need a real library, got ${before}`);

      await page.getByTestId("filters-button").click();
      await page.waitForSelector('[data-testid="filters-popover"]', { timeout: 5000 });
      await page.locator('[data-testid^="facet-size-"]').first().click();
      await page.waitForFunction(
        (n) => Number(
          document.querySelector('[data-testid="result-count"]')?.textContent?.replace(/\D/g, ""),
        ) !== n,
        before,
        { timeout: 5000 },
      );
      const badge = await page.getByTestId("filters-badge").textContent();
      assert.equal(badge?.trim(), "1", "one chip on must show a count of 1");
      const filtered = await total();
      assert.ok(filtered > 0 && filtered < before, `filtered to ${filtered} of ${before}`);

      await page.getByTestId("filters-clear").click();
      await page.waitForFunction(
        (n) => Number(
          document.querySelector('[data-testid="result-count"]')?.textContent?.replace(/\D/g, ""),
        ) === n,
        before,
        { timeout: 5000 },
      );
      assert.equal(await page.getByTestId("filters-badge").count(), 0, "cleared shows no badge");
    } finally {
      // Leave the shell as it was found. A filter left on narrows every later check's grid, and
      // an open popover swallows their first click — which turns one failure into eight.
      if ((await page.getByTestId("filters-badge").count()) > 0) {
        if ((await page.getByTestId("filters-popover").count()) === 0) {
          await page.getByTestId("filters-button").click();
        }
        await page.getByTestId("filters-clear").click();
      }
      await page.evaluate(() => document.body.click());
      await page.waitForTimeout(400);
    }
  });

  // `add_source` is exercised **only** through its refusals here, and deliberately so: a
  // successful call writes a permanent source row into the developer's real catalog, and there is
  // no remove-source op to take it back. The refusals are the part worth guarding anyway — both
  // happen before any write, so a green run leaves the catalog exactly as it found it. The
  // success path is covered where it is safe: `lodestash_app::scan`, over a temp dir it owns.
  await check("add_source refuses what it cannot index, without touching the catalog", async () => {
    const before = await invoke("library_status");

    // A *file* is a different feature (import one asset); indexing its parent folder instead is
    // not what anyone meant by dropping a file on the window.
    await assert.rejects(
      () => invoke("add_source", { path: before.catalog_path }),
      /not a folder/,
      "dropping a file must be refused, not silently widened to its folder",
    );
    await assert.rejects(
      () => invoke("add_source", { path: "G:\\definitely\\not\\here" }),
      /not a folder/,
    );

    const after = await invoke("library_status");
    assert.equal(after.sources.length, before.sources.length, "a refusal must add no source");
    assert.equal(after.total_assets, before.total_assets, "…and index nothing");
  });

  // Not covered here: that the indexing status card is non-modal. The card only exists while
  // `add_source` is in flight, and this suite must not run a real one (see above) — and Svelte's
  // scoped styles mean a synthetic `.toast` div gets none of its CSS, so probing one asserts
  // nothing about the real element. It is a structural property of `DropZone.svelte`: the
  // full-window overlay renders on `over` only, the corner card on `busy`.

  // The vocabulary and the ⚑ trait cloud are two views of the same tags. If they disagree, one of
  // them is reading a different table — and the symptom is a trait row whose Browse lands on an
  // empty grid, which reads as "that tag has no assets" rather than as a bug.
  await check("the trait vocabulary agrees with the filter's trait cloud", async () => {
    const [vocab, facets] = await Promise.all([invoke("vocabulary"), invoke("filter_facets")]);
    const cloud = new Map(facets.traits.map((t) => [t.name, t.count]));

    // A tag **name is not unique** — this catalog holds `free` twice, once in the `free`
    // namespace and once in `mood`. The vocabulary lists a row per tag id; the ⚑ cloud and
    // `ContentFilter::passes` both key on the name, so those rows merge into one chip. Comparing
    // per-row would fail on exactly that, which is how this was found.
    const byName = new Map();
    for (const v of vocab) byName.set(v.value, (byName.get(v.value) ?? 0) + v.usage_count);

    for (const [name, usage] of [...byName].filter(([, u]) => u > 0).slice(0, 20)) {
      // The cloud counts live rows; the vocabulary counts every undeleted asset, trashed
      // included — so the cloud may be smaller, never larger.
      const c = cloud.get(name) ?? 0;
      assert.ok(c <= usage, `${name}: cloud says ${c}, vocabulary says ${usage}`);
    }
    assert.ok(
      vocab.every((v) => v.manual_count + v.ai_count <= v.usage_count),
      "the manual/AI split cannot exceed the usage it splits",
    );

    await page.getByRole("button", { name: "Traits" }).click();
    await page.waitForSelector(
      vocab.length > 0 ? '[data-testid="vocab"]' : '[data-testid="empty"]',
      { timeout: 5000 },
    );
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // Counts here come from the indexed rows, not from `package.asset_count` — which is populated
  // for 1 of 87 packs on this cache, so a screen that trusted it would be a wall of blanks next
  // to packs that plainly contain things.
  await check("Packages counts match what Browse → actually lands on", async () => {
    const packs = await invoke("packages");
    assert.ok(packs.length > 1, `expected a real pack list, got ${packs.length}`);
    assert.ok(
      packs.some((p) => p.count > 0),
      "no pack reports a single indexed asset — the count join is not matching",
    );
    // The count printed next to a pack must be the number of assets its Browse → shows.
    const sample = packs.filter((p) => p.count > 0).slice(0, 3);
    for (const p of sample) {
      const page = await invoke("grid_page", {
        query: {
          tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
          package: p.name, page: 0, page_size: 1,
        },
      });
      assert.equal(page.total, p.count, `${p.name}: screen says ${p.count}, grid says ${page.total}`);
    }
  });

  await check("Browse → filters the library, and the chip clears it", async () => {
    try {
      await page.getByRole("button", { name: "Packages" }).click();
      await page.waitForSelector('[data-testid="publishers"]', { timeout: 5000 });
      await page.getByTestId("pack-browse").first().click();
      await page.waitForSelector('[data-testid="pack-chip"]', { timeout: 5000 });

      const all = await invoke("library_status").then((s) => s.total_assets);
      // The chip renders from view state the instant it is set; the grid behind it is still
      // fetching, and `total` reads 0 until that lands. Waiting on the chip alone asserts nothing.
      await page.waitForFunction(
        () => {
          const t = document.querySelector('[data-testid="result-count"]')?.textContent;
          return !!t && Number(t.replace(/\D/g, "")) > 0;
        },
        undefined,
        { timeout: 5000 },
      );
      const filtered = await page
        .getByTestId("result-count")
        .textContent()
        .then((t) => Number(t.replace(/\D/g, "")));
      assert.ok(filtered > 0 && filtered < all, `pack view showed ${filtered} of ${all}`);

      await page.getByTestId("pack-chip").click();
      await page.waitForFunction(
        (n) => Number(
          document.querySelector('[data-testid="result-count"]')?.textContent?.replace(/\D/g, ""),
        ) === n,
        all,
        { timeout: 5000 },
      );
    } finally {
      // The sidebar's Library row is the reset-to-everything affordance, so this also proves it.
      await page.getByRole("button", { name: "Library", exact: true }).click();
      await page.waitForSelector(".card", { timeout: 5000 });
    }
  });

  // Stats derives everything from the rows the shell already holds rather than running its own
  // COUNT(*)s — so the one thing worth asserting is that it still agrees with the library it is
  // summarising. A summary screen that says 8,237 over a grid showing 8,236 is unactionable.
  await check("Stats agrees with the library it summarises", async () => {
    const [s, st] = await Promise.all([invoke("library_stats"), invoke("library_status")]);
    assert.equal(s.total_assets, st.total_assets, "stats and status must count the same library");
    assert.equal(s.sources, st.sources.length);
    const byType = s.by_type.reduce((a, t) => a + t.count, 0);
    assert.equal(byType, s.total_assets, "the type buckets must partition the library");
    const bytes = s.by_type.reduce((a, t) => a + t.bytes, 0);
    assert.equal(bytes, s.bytes_total, "…and so must their sizes");
    assert.ok(s.heaviest.length > 0 && s.heaviest[0].size_bytes >= s.heaviest.at(-1).size_bytes);

    await page.getByRole("button", { name: "Stats" }).click();
    await page.waitForSelector('[data-testid="stat-tiles"]', { timeout: 5000 });
    const shown = await page.locator('[data-testid="stat-tiles"] .value').first().textContent();
    assert.equal(
      Number(shown.replace(/\D/g, "")),
      s.total_assets,
      "the first tile must show the asset count the backend reported",
    );
  });

  // Exact duplicates only. The failure worth catching is the one that looks like a working
  // screen: grouping rows whose hash is *absent* collects the entire un-drained backlog into one
  // enormous "duplicate" group.
  await check("Duplicates groups only hashed assets, keeper first", async () => {
    const groups = await invoke("duplicate_groups");
    for (const g of groups) {
      assert.ok(g.hash.length > 0, "a group must be identified by a real hash");
      assert.ok(g.items.length > 1, "a group of one is not a duplicate");
      assert.equal(g.items[0].asset_id, g.keeper, "the keeper leads its group");
      assert.ok(
        new Set(g.items.map((i) => i.asset_id)).size === g.items.length,
        "an asset cannot duplicate itself",
      );
    }
    const s = await invoke("library_stats");
    assert.equal(s.duplicate_groups, groups.length, "Stats and the screen must agree on the count");

    await page.getByRole("button", { name: "Duplicates" }).click();
    await page.waitForSelector(
      groups.length > 0 ? '[data-testid="dup-list"]' : '[data-testid="empty"]',
      { timeout: 5000 },
    );
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // ── D11: Duplicates is a queue reviewed in the compare pane ──────────────────────────────────
  // "Keep both" is session-only — it decides nothing in the catalog — so this drives the real UI
  // without writing anything. Skips honestly when this library has no duplicate groups rather than
  // asserting on an empty screen and calling that a pass.
  await check("the duplicates queue keeps your place after a decision", async () => {
    const groups = await invoke("duplicate_groups");
    const near = await invoke("near_duplicate_groups").catch(() => []);
    // One group is enough to prove the part that matters — that a decision sticks to its row and
    // the row keeps its place. The cursor assertion only applies when there is somewhere to move.
    if (groups.length + near.length < 1) {
      console.log("       skipped — this library has no duplicate groups at all");
      return;
    }
    const many = groups.length + near.length > 1;
    await page.getByRole("button", { name: "Duplicates" }).click();
    await page.waitForSelector('[data-testid="dup-pane"]', { timeout: 5000 });

    const before = await page.evaluate(() =>
      document.querySelector('[data-testid="dup-row-0"]')?.className ?? "",
    );
    assert.ok(!before.includes("done"), "the first group is already marked resolved");

    await page.click('[data-testid="keep-both"]');
    await page.waitForFunction(
      () => document.querySelector('[data-testid="dup-row-0"]')?.className.includes("done"),
      undefined,
      { timeout: 5000 },
    );
    // The resolved row stays in place with its outcome — the queue must read as work done, not as
    // work vanishing — and the cursor has moved on to the next undecided group.
    const after = await page.evaluate(() => ({
      first: document.querySelector('[data-testid="dup-row-0"]')?.className ?? "",
      onIndex: [...document.querySelectorAll('[data-testid^="dup-row-"]')].findIndex((r) =>
        r.className.includes("on"),
      ),
      rows: document.querySelectorAll('[data-testid^="dup-row-"]').length,
    }));
    assert.ok(after.first.includes("done"), "the decision did not stick to its row");
    if (many) {
      assert.notEqual(after.onIndex, 0, "the cursor stayed on the group just decided");
    }
    assert.equal(after.rows, groups.length + near.length, "a resolved group vanished from the list");

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // ── D12: the responsive ladder, driven for real (ADR-0009) ───────────────────────────────────
  // `Browser.setWindowBounds` resizes what the page sees, so media queries follow — unlike
  // `setViewportSize`, which changes nothing here and leaves an emulation override that poisons
  // every later check. Worth the trouble: the first version of this ladder had two of its four
  // stops doing *nothing* at any width, because rules written in the global sheet lose to a
  // component's own scoped ones, and nothing said so.
  //
  // **What it actually resizes**, measured with Win32 `GetWindowRect` rather than assumed: only the
  // **WebView2 controller**, never the Tauri window. Held at 900×800, CDP and `window.outerWidth`
  // both report 900×800 while the real HWND is still `IsZoomed: True` at 2576×1408. So for a few
  // seconds this check makes the app look like a small page pinned inside a full-size frame, with
  // black around it — alarming, reported as a bug once, and entirely transient. It also means a
  // *failed* restore does not shrink the window, it strands the webview inside it, which is what
  // the original "the layout keeps getting smaller" report was actually looking at.
  //
  // Reachability is `offsetParent !== null`, not computed display: what decides whether a control
  // can be used is whether an ancestor hid it, and the first probe of this got that wrong.
  await check("each rung of the responsive ladder moves the chrome it promises", async () => {
    const cdp = await page.context().newCDPSession(page);
    const { windowId } = await cdp.send("Browser.getWindowForTarget");
    const { bounds: original } = await cdp.send("Browser.getWindowBounds", { windowId });
    // Say so, so that watching the app shrink for a few seconds reads as this check working rather
    // than as the app breaking.
    console.log("  …    resizing the app to walk the responsive ladder — it is put back after");
    const resizeStart = Date.now();

    /**
     * Put the window back exactly as it was found.
     *
     * Three traps here, all silent, all of which shipped:
     *
     * - `getWindowBounds` reports a maximized window as geometry **plus** `windowState:
     *   "maximized"`, and `setWindowBounds` rejects that combination outright — *"the
     *   ‘maximized’ state cannot be combined with ‘left’, ‘top’, ‘width’ or ‘height’"*. Handing the
     *   object straight back therefore throws, and the ladder’s last rung (900×800) is where the
     *   window stays. Restoring the **state alone** is what puts a maximized window back.
     * - Sending width/height without left/top lets the window re-centre instead of staying put.
     * - Round-tripping left/top **does not round-trip**: CDP reports window-frame coordinates and
     *   applies client coordinates, so writing back the `top` you just read moves the window down
     *   by one title bar (measured here: 23 → 46). Run after run it walks down the screen until the
     *   bottom is off it — which is what "the layout keeps getting smaller while the window stays
     *   the same" actually looks like.
     *
     * So the position is corrected rather than assumed: set it, read back, and if it landed
     * somewhere else, subtract the delta and set once more. Two passes converge because the offset
     * is a constant frame inset, and asking the window where it ended up beats hard-coding a
     * platform’s border metrics.
     *
     * This is not tidiness. The app under test shares the user’s config directory, so
     * `tauri-plugin-window-state` writes whatever geometry is left behind and every future launch
     * opens in it — which is exactly the report that found this.
     */
    const restoreWindow = async () => {
      if (original.windowState && original.windowState !== "normal") {
        await cdp.send("Browser.setWindowBounds", {
          windowId,
          bounds: { windowState: original.windowState },
        });
        return;
      }
      const { left, top, width, height } = original;
      const target = { left, top, width, height };
      let want = { ...target };
      // Correct **all four** axes, not just position. The frame-vs-client mismatch documented
      // above applies to size exactly as it does to position: writing back the `width` just read
      // lands a pixel wide, and because the result is persisted by `tauri-plugin-window-state`
      // the next run reads *that* as its original and adds another. It is a one-pixel bug that
      // grows without bound, and it is the tell behind "the window is not the size I left it".
      for (let pass = 0; pass < 3; pass++) {
        await cdp.send("Browser.setWindowBounds", { windowId, bounds: want });
        await page.waitForTimeout(250);
        const { bounds: got } = await cdp.send("Browser.getWindowBounds", { windowId });
        const off = Object.fromEntries(
          Object.keys(target).map((k) => [k, got[k] - target[k]]),
        );
        if (Object.values(off).every((d) => d === 0)) return;
        want = Object.fromEntries(Object.keys(target).map((k) => [k, want[k] - off[k]]));
      }
    };
    // Height is held at whatever it already was: only width drives this ladder, and changing both
    // makes the transient resize twice as jarring for nothing.
    const at = async (width) => {
      await cdp.send("Browser.setWindowBounds", {
        windowId,
        bounds: { width, height: original.height, windowState: "normal" },
      });
      // The icon rail widens on hover *by design*, and earlier checks leave the pointer wherever
      // they last clicked — often on a nav row. Park it over the grid before measuring, or step 3
      // reads 210px and looks like a broken media query.
      await page.mouse.move(width - 60, 400);
      // Earlier checks leave focus on whatever they last clicked; a nav row still holding focus
      // would expand the rail through `:has(:focus-visible)` and read as a broken media query.
      await page.evaluate(() => (document.activeElement instanceof HTMLElement ? document.activeElement.blur() : null));
      // Wait for the resize to *land*, not for a guess at how long it takes. The page knows: its
      // own `innerWidth` is the width the media queries are being evaluated at. A fixed 500ms per
      // rung was both slower and less certain — and five of them are most of the several seconds
      // this check spends visibly shrinking the app, which has been reported as a bug three times.
      await page
        .waitForFunction((w) => Math.abs(window.innerWidth - w) <= 1, width, { timeout: 4000 })
        .catch(() => {});
      // One frame for the relayout the new width triggers, which `innerWidth` does not wait for.
      await page.evaluate(
        () => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(null)))),
      );
      return page.evaluate(() => {
        const shown = (sel) => {
          const el = document.querySelector(sel);
          return el ? el.offsetParent !== null : false;
        };
        const rail = document.querySelector(".inspector, .edge");
        return {
          hint: shown(".toolbar .hint"),
          railOverlays: rail ? getComputedStyle(rail).position === "absolute" : false,
          sidebar: Math.round(document.querySelector(".sidebar").getBoundingClientRect().width),
          sort: shown(".toolbar .sort"),
          more: shown('[data-testid="toolbar-overflow"]'),
          view: shown('[aria-label="View mode"]'),
        };
      });
    };
    try {
      const wide = await at(1500);
      assert.ok(wide.hint, "the keyboard hint is already gone at 1500px");
      assert.ok(!wide.railOverlays, "the inspector already overlays at 1500px");
      assert.equal(wide.sidebar, 210, "the sidebar is not full width at 1500px");

      assert.ok(!(await at(1300)).hint, "step 1: the hint did not go by 1300px");
      assert.ok((await at(1200)).railOverlays, "step 2: the inspector still pushes the grid at 1200px");

      const rail = await at(1000);
      assert.equal(rail.sidebar, 48, `step 3: sidebar is ${rail.sidebar}px at 1000px, not the icon rail`);

      const narrow = await at(900);
      assert.ok(!narrow.sort, "step 4: sort is still inline at 900px");
      assert.ok(narrow.more, "step 4: sort folded away instead of into the ⋯ overflow");
      assert.ok(narrow.view, "the view toggle folded — it changes what you are looking at");
    } finally {
      await restoreWindow();
      await page
        .waitForFunction((w) => Math.abs(window.innerWidth - w) <= 1, original.width, { timeout: 4000 })
        .catch(() => {});
      // Verified, not assumed. The previous restore swallowed its own error with `.catch(() => {})`
      // and left the app permanently 900×800 — a check that resizes the user’s window has to prove
      // it gave it back.
      const { bounds: after } = await cdp.send("Browser.getWindowBounds", { windowId });
      console.log(`  …    the app was resized for ${((Date.now() - resizeStart) / 1000).toFixed(1)}s`);
      // Within a pixel per axis, not exactly — the corrective loop above converges to the pixel
      // and no further, because CDP reports frame geometry and applies client geometry and the
      // inset is not always an integer at fractional DPI. Demanding exact equality here failed a
      // restore that had in fact worked, and every check after it inherited the failure. The leak
      // worth failing over is a window left an entire responsive rung away, never one pixel.
      const drift = ["left", "top", "width", "height"].filter(
        (k) => Math.abs(after[k] - original[k]) > 1,
      );
      assert.deepEqual(
        [drift, after.windowState ?? "normal"],
        [[], original.windowState ?? "normal"],
        `the ladder did not put the window back — this state is persisted and the app reopens in it` +
          ` (was ${JSON.stringify(original)}, now ${JSON.stringify(after)})`,
      );
      // Give the relayout time to settle before the next check measures anything, and hand keyboard
      // focus back: this check blurs whatever held it, and a document with nothing focused swallows
      // the next check's Ctrl-K. Clicking the result count is inert — a span, no state.
      await page.waitForTimeout(800);
      await page.click(".toolbar .count", { timeout: 3000 }).catch(() => {});
    }
  });

  await check("Ctrl-K opens the palette, and Enter on an asset reveals it", async () => {
    try {
      await page.fill('input[type="text"]', "");
      await page.waitForTimeout(400);
      await page.keyboard.press("Control+k");
      await page.waitForSelector('[data-testid="palette-input"]', { timeout: 5000 });

      // Commands are reachable with no query at all — an empty palette that shows nothing is a
      // box you have to already know the contents of.
      const idle = await page.locator('[data-testid="palette-results"] .row').count();
      assert.ok(idle > 0, "the palette must offer commands before anything is typed");

      await page.getByTestId("palette-input").fill("Stairs");
      await page.waitForFunction(
        () =>
          [...document.querySelectorAll('[data-testid="palette-results"] .hint')].some(
            (e) => e.textContent?.trim() === "model_3d",
          ),
        undefined,
        { timeout: 5000 },
      );
      const name = await page
        .locator('[data-testid="palette-results"] .row')
        .first()
        .locator(".name")
        .textContent();
      await page.keyboard.press("Enter");

      // Revealing means the grid is showing it and the Inspector is describing it — not merely
      // that the palette closed.
      await page.waitForSelector('[data-testid="palette"]', { state: "detached", timeout: 5000 });
      await page.waitForFunction(
        (n) => document.querySelector('[data-testid="inspector-name"]')?.textContent?.trim() === n,
        name?.trim(),
        { timeout: 5000 },
      );
    } finally {
      await page.keyboard.press("Escape");
      await page.fill('input[type="text"]', "");
      await page.waitForTimeout(400);
    }
  });

  // The defect this exists to catch is invisible by inspection: every `.unitypackage` lays its
  // assets out under `Assets/…`, so browsing by `rel_path` alone merges every pack in a source
  // into one folder holding thousands of files from unrelated products. In-pack assets are the
  // majority of this library, so that failure looks like a working folder tree — just a useless one.
  await check("packs browse as their own folders instead of merging under Assets/", async () => {
    const s = await invoke("library_status");
    let widest = 0;
    for (const src of s.sources) {
      const level = await invoke("folder_page", {
        query: { tab: "all", search: "", status: "all", source: src.name, dir: "" },
      });
      if (level.total === 0) continue;
      const names = level.tiles.map((t) => t.name);
      // The merge bug's exact signature: a source's whole tree collapsed into one `Assets` tile.
      assert.ok(
        !(names.length === 1 && names[0] === "Assets"),
        `${src.name} shows a single Assets/ tile — that is every pack merged into one pile`,
      );
      // The level's own count must be everything it leads to: the files sitting directly in it
      // plus everything under each tile. Some sources really do keep loose files at their root,
      // so dropping `items` here under-counts by exactly those.
      const sum = level.tiles.reduce((a, t) => a + t.recursive, 0) + level.items.length;
      assert.equal(sum, level.total, `${src.name}: parts sum to ${sum}, level says ${level.total}`);
      widest = Math.max(widest, names.length);
    }
    // Sources here hold anywhere from one pack to eighty-six, so this asserts the shape of the
    // library rather than a per-source count: somewhere in it, packs are separate folders.
    assert.ok(widest > 1, `no source browses as more than one folder (widest was ${widest})`);
  });

  await check("the Folders view walks in, and Backspace walks back out", async () => {
    try {
      await page.fill('input[type="text"]', "");
      await page.getByRole("button", { name: "Folders view" }).click();
      await page.waitForSelector('[data-testid="folder-tiles"]', { timeout: 5000 });

      // A single-source library descends on its own, so the first crumb is already the source.
      const crumbsBefore = await page.getByTestId("folder-crumbs").textContent();
      const first = page.locator('[data-testid="folder-tiles"] .tile').first();
      const name = (await first.locator(".name").textContent())?.trim();
      await first.click();
      await page.waitForFunction(
        (n) => document.querySelector('[data-testid="folder-crumbs"]')?.textContent?.includes(n),
        name,
        { timeout: 5000 },
      );
      assert.notEqual(
        await page.getByTestId("folder-crumbs").textContent(),
        crumbsBefore,
        "entering a folder must move the breadcrumb",
      );

      await page.keyboard.press("Backspace");
      await page.waitForFunction(
        (c) => document.querySelector('[data-testid="folder-crumbs"]')?.textContent === c,
        crumbsBefore,
        { timeout: 5000 },
      );
    } finally {
      await page.getByRole("button", { name: "Grid view" }).click();
      // Walking into a source sets `view.source`, which the sidebar's LIBRARIES facet and the flat
      // grid *also* read — so leaving Folders in Synty leaves the grid scoped to Synty, and every
      // later check that searches for an asset in another library finds nothing. Reset the way a
      // user does: the Library nav row is the documented reset-to-everything (UI-SPEC §0.2).
      await page.getByRole("button", { name: "Library", exact: true }).click();
      await page.waitForSelector(".card", { timeout: 5000 });
    }
  });

  // Both view modes must render the same result set — a list that quietly shows a different
  // number of rows than the grid means one of them is filtering on its own.
  await check("list view shows the same assets as the grid", async () => {
    await page.fill('input[type="text"]', "Stairs");
    await page.waitForTimeout(700);
    const cards = await page.locator(".card").count();
    await page.getByRole("button", { name: "List view" }).click();
    await page.waitForSelector(".list .row", { timeout: 5000 });
    const rows = await page.locator(".list .row").count();
    assert.equal(rows, cards, `list showed ${rows} rows for ${cards} cards`);
    await page.getByRole("button", { name: "Grid view" }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(500);
  });

  // ── D6: rows get what cards already have (closes ALIGNMENT-AUDIT F6) ─────────────────────────
  // Switching to list used to silently cost you the star, Quick Look and staging. The check is
  // presence *and* reach: a control that renders but does nothing is the failure worth catching.
  await check("list rows carry the star, Quick Look and stage", async () => {
    await page.fill('input[type="text"]', "Stairs");
    await page.waitForTimeout(700);
    await page.getByRole("button", { name: "List view" }).click();
    await page.waitForSelector(".list .row", { timeout: 5000 });
    const row = page.locator(".list .row").first();
    for (const id of ["row-star", "row-open"]) {
      assert.equal(await row.locator(`[data-testid="${id}"]`).count(), 1, `no ${id} on a list row`);
    }
    // Stage is on identified rows only — an unidentified row leads with ✦ instead, since on a row
    // you cannot see the art and appraising is the fastest useful thing.
    const stages = await page.locator('.list [data-testid="row-stage"]').count();
    const ais = await page.locator('.list [data-testid="row-identify"]').count();
    const rows = await page.locator(".list .row").count();
    assert.equal(stages + ais, rows, `${rows} rows carried ${stages} stage + ${ais} identify actions`);
  });

  await check("⤢ on a row opens Quick Look for that row", async () => {
    const row = page.locator(".list .row").first();
    const name = (await row.locator(".label").textContent())?.trim();
    await row.locator('[data-testid="row-open"]').click();
    await page.waitForSelector('[data-testid="quicklook"]', { timeout: 5000 });
    await page.waitForFunction(
      () => document.querySelector('[data-testid="ql-name"]')?.textContent?.trim() !== "…",
      undefined,
      { timeout: 5000 },
    );
    const shown = (await page.locator('[data-testid="ql-name"]').textContent())?.trim();
    assert.equal(shown, name, "Quick Look opened a different asset than the row it was clicked on");
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Grid view" }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(500);
  });

  // ── D5: the rail earns its 300px or gets out of the way ──────────────────────────────────────
  await check("the inspector collapses to an edge tab with nothing selected", async () => {
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="inspector-edge"]', { timeout: 5000 });
    const wide = await page.locator("aside.inspector").count();
    assert.equal(wide, 0, "the 300px rail is still open with nothing selected");
  });

  // Compare is the pane §5 reviews duplicate groups in, so the rule that makes it readable —
  // agreement stays dim, difference goes gold — is the thing worth pinning.
  await check("picking two opens compare, and only differing rows are gold", async () => {
    const boxes = page.locator('[data-testid="card-select"]');
    await boxes.nth(0).click();
    await boxes.nth(1).click();
    await page.waitForSelector('[data-testid="inspector-compare"]', { timeout: 5000 });

    const rows = await page.locator('[data-testid="compare-rows"] .cmp-v').evaluateAll((els) =>
      els.map((e) => ({ text: e.textContent?.trim() ?? "", gold: e.classList.contains("differs") })),
    );
    assert.ok(rows.length >= 4, `expected a stat table, got ${rows.length} cells`);
    for (let i = 0; i < rows.length; i += 2) {
      const [a, b] = [rows[i], rows[i + 1]];
      assert.equal(
        a.gold,
        a.text !== b.text,
        `"${a.text}" vs "${b.text}" is marked ${a.gold ? "different" : "equal"} and is not`,
      );
    }
    // Leave the pick set as it was found — the bulk bar acts on it.
    await page.click('[data-testid="bulk-clear"]');
  });

  // ── D15: the grid is one tab stop, and arrows move inside it ─────────────────────────────────
  // The property that matters is that focus and the lead selection stay together: if they came
  // apart, Tab would return you to a card the inspector is not describing, and the focus ring would
  // be pointing at the wrong asset.
  await check("arrow keys move the lead, and focus goes with it", async () => {
    await page.fill('input[type="text"]', "");
    await page.waitForFunction(() => document.querySelectorAll(".card").length > 6, undefined, {
      timeout: 5000,
    });
    await page.locator(".card").first().click();
    await page.waitForSelector('[data-testid="inspector-name"]', { timeout: 5000 });
    const first = await page.evaluate(
      () => document.querySelector(".card.selected")?.getAttribute("data-asset-id"),
    );

    await page.keyboard.press("ArrowRight");
    await page.waitForFunction(
      (was) => document.querySelector(".card.selected")?.getAttribute("data-asset-id") !== was,
      first,
      { timeout: 5000 },
    );
    const moved = await page.evaluate(() => ({
      selected: document.querySelector(".card.selected")?.getAttribute("data-asset-id"),
      focused: document.activeElement?.getAttribute("data-asset-id"),
      tabbable: document.querySelectorAll('.card[tabindex="0"]').length,
    }));
    assert.notEqual(moved.selected, first, "ArrowRight did not move the lead");
    assert.equal(moved.focused, moved.selected, "focus and the lead selection came apart");
    assert.equal(moved.tabbable, 1, `the grid offers ${moved.tabbable} tab stops, not one`);

    // ArrowDown must move by a whole row, which is what makes this navigation rather than a list.
    const cols = await page.evaluate(() => {
      const el = document.querySelector('[data-testid="grid"]');
      return getComputedStyle(el).gridTemplateColumns.split(/\s+/).filter(Boolean).length;
    });
    if (cols > 1) {
      const ids = await page.evaluate(() =>
        [...document.querySelectorAll(".card")].map((c) => c.getAttribute("data-asset-id")),
      );
      const before = ids.indexOf(moved.selected);
      await page.keyboard.press("ArrowDown");
      await page.waitForFunction(
        (was) => document.querySelector(".card.selected")?.getAttribute("data-asset-id") !== was,
        moved.selected,
        { timeout: 5000 },
      );
      const after = ids.indexOf(
        await page.evaluate(
          () => document.querySelector(".card.selected")?.getAttribute("data-asset-id"),
        ),
      );
      assert.equal(after - before, cols, `ArrowDown moved ${after - before} places for ${cols} columns`);
    }
    await page.keyboard.press("Escape");
  });

  await check("double-click opens Quick Look on the right asset", async () => {
    await page.fill('input[type="text"]', "");
    // Wait for the grid to settle before pinning an element: a `locator` re-resolves on every
    // call, so reading the name and clicking can land on two different cards while the previous
    // screen's results are still being replaced.
    await page.waitForFunction(
      () => document.querySelectorAll(".card").length > 10,
      undefined,
      { timeout: 5000 },
    );
    await page.waitForTimeout(400);
    const first = await page.locator(".card").first().elementHandle();
    const name = (await first.$eval(".name", (e) => e.textContent))?.trim();
    await first.dblclick();
    await page.waitForSelector('[data-testid="quicklook"]', { timeout: 5000 });
    await page.waitForFunction(
      () => document.querySelector('[data-testid="ql-stage"]')?.getAttribute("data-kind") !== "loading",
      undefined,
      { timeout: 5000 },
    );
    const shown = (await page.locator('[data-testid="ql-name"]').textContent())?.trim();
    assert.equal(shown, name, `Quick Look showed "${shown}" for a double-click on "${name}"`);
  });

  await check("arrow keys step, Escape closes", async () => {
    const before = (await page.locator('[data-testid="ql-name"]').textContent())?.trim();
    await page.keyboard.press("ArrowRight");
    await page.waitForFunction(
      (prev) => document.querySelector('[data-testid="ql-name"]')?.textContent?.trim() !== prev,
      before,
      { timeout: 5000 },
    );
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="quicklook"]', { state: "detached", timeout: 5000 });
  });

  // An in-pack asset must say so rather than showing a broken viewer — the read-only
  // invariant only means something if the UI explains it instead of looking failed.
  await check("Quick Look on an in-pack asset renders its content (FEATURES §5.4)", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.find((i) => i.in_pack);
    assert.ok(packed, "no in-pack asset loaded");
    await page.fill('input[type="text"]', packed.name);
    await page.waitForFunction(
      (n) => document.querySelector(".card .name")?.textContent?.trim() === n,
      packed.name,
      { timeout: 5000 },
    );
    await page.locator(".card").first().dblclick();
    await page.waitForSelector('[data-testid="ql-stage"]', { timeout: 5000 });
    await page.waitForFunction(
      () => document.querySelector('[data-testid="ql-stage"]')?.getAttribute("data-kind") !== "loading",
      undefined,
      { timeout: 5000 },
    );
    const kind = await page.locator('[data-testid="ql-stage"]').getAttribute("data-kind");
    // FEATURES §5.4: an in-pack asset's Quick Look materializes the one payload (size-capped
    // scratch) — the stage renders the content, not the old 503 explanation. Whole-pack
    // extraction is still an explicit command; only a pull that actually fails (over the cap,
    // vanished pack) falls back to `not_local`.
    assert.notEqual(kind, "not_local", "an in-pack asset must render its content now");
    assert.ok(
      kind === "image" || kind === "audio" || kind === "model",
      `in-pack Quick Look rendered unexpected kind ${kind}`,
    );
    await page.keyboard.press("Escape");
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(400);
  });

  // The B2b hard gate. Note the range forms deliberately: WebView2 does **not** forward suffix
  // ranges (`bytes=-N`) to a custom protocol handler — verified by logging, the header simply
  // never arrives — so asserting on them would test the platform, not this code. Chromium's
  // media stack seeks with open-ended `bytes=N-`, which is what is checked here.
  await check("audio streams over lodestash://file and an <audio> element can seek it", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "sounds", search: "", status: "all", sort_by: "size", sort_asc: false,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const audio = p.items.find((i) => !i.in_pack);
    if (!audio) {
      console.log("       (skipped: no on-disk audio in this library)");
      return;
    }
    const r = await page.evaluate(async ([id, size]) => {
      const mid = Math.floor(size / 2);
      const head = await fetch(`http://lodestash.localhost/file/${id}`, { headers: { Range: "bytes=0-1023" } });
      const tail = await fetch(`http://lodestash.localhost/file/${id}`, { headers: { Range: `bytes=${mid}-` } });
      // Then the real thing: decode metadata and seek, which is what scrubbing does.
      const el = new Audio(`http://lodestash.localhost/file/${id}`);
      const meta = await new Promise((res) => {
        el.addEventListener("loadedmetadata", () => res(true), { once: true });
        el.addEventListener("error", () => res(false), { once: true });
        setTimeout(() => res(false), 10000);
      });
      let seeked = false;
      if (meta && Number.isFinite(el.duration)) {
        el.currentTime = el.duration / 2;
        seeked = await new Promise((res) => {
          el.addEventListener("seeked", () => res(true), { once: true });
          setTimeout(() => res(false), 10000);
        });
      }
      return {
        headStatus: head.status,
        headLen: (await head.arrayBuffer()).byteLength,
        tailStatus: tail.status,
        tailRange: tail.headers.get("content-range"),
        meta,
        duration: el.duration,
        seeked,
        at: el.currentTime,
      };
    }, [audio.asset_id, audio.size_bytes]);

    assert.equal(r.headStatus, 206, "leading range");
    assert.equal(r.headLen, 1024);
    assert.equal(r.tailStatus, 206, "open-ended range from the middle");
    assert.match(r.tailRange, /^bytes \d+-\d+\/\d+$/);
    assert.ok(r.meta, "<audio> could not decode metadata over lodestash://file");
    assert.ok(r.duration > 0, `expected a real duration, got ${r.duration}`);
    assert.ok(r.seeked, "seeking did not complete — Range streaming is not working for playback");
    assert.ok(r.at > 0, "playhead did not move");
  });

  // Wire/parser drift between `encode_meshes` and lsm1.ts would surface as garbled geometry, not
  // an error — so parse a real response rather than trusting the shape.
  await check("lodestash://mesh returns LSM1 the TS parser can read", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "models", search: "", status: "all", sort_by: "size", sort_asc: false,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const model = p.items.find((i) => !i.in_pack);
    assert.ok(model, "no on-disk model in the library");
    const r = await page.evaluate(async (id) => {
      const res = await fetch(`http://lodestash.localhost/mesh/${id}`);
      if (!res.ok) return { status: res.status };
      const buf = await res.arrayBuffer();
      const magic = new TextDecoder().decode(new Uint8Array(buf, 0, 4));
      const { parseLsm1, bounds } = await import("/src/lib/lsm1.ts");
      const meshes = parseLsm1(buf);
      const tris = meshes.reduce((n, m) => n + m.indices.length / 3, 0);
      return {
        status: res.status, magic, bytes: buf.byteLength,
        meshes: meshes.length, tris,
        hasUvs: meshes.some((m) => m.uvs !== null),
        radius: bounds(meshes).radius,
      };
    }, model.asset_id);
    assert.equal(r.status, 200);
    assert.equal(r.magic, "LSM1");
    assert.ok(r.meshes > 0 && r.tris > 0, `parsed ${r.meshes} meshes / ${r.tris} tris`);
    // Geometry is served undecimated so palette-atlas UVs stay exact; only pathological meshes
    // are cut down. The cap is on the response, not the triangle count.
    assert.ok(r.bytes < 20 * 1024 * 1024, `mesh response too large: ${r.bytes} bytes`);
    assert.ok(r.radius > 0, "degenerate bounds — the camera would have nothing to frame");
  });

  // The automated stand-in for looking at it: if shaders, buffers, camera framing or the mesh
  // fetch break, the canvas is a flat background and this fails.
  await check("a model renders non-blank pixels, and orbit + zoom change them", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "models", search: "", status: "all", sort_by: "size", sort_asc: false,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const model = p.items.find((i) => !i.in_pack);
    await page.fill('input[type="text"]', model.name);
    await page.waitForFunction(
      (n) => document.querySelector(".card .name")?.textContent?.trim() === n,
      model.name, { timeout: 5000 });
    await page.locator(".card").first().dblclick();
    await page.waitForSelector('[data-testid="viewer3d"][data-status="ready"]', { timeout: 20000 });
    await page.waitForTimeout(600);

    const sample = () =>
      page.evaluate(() => {
        const c = document.querySelector('[data-testid="viewer3d-canvas"]');
        const gl = c.getContext("webgl2", { preserveDrawingBuffer: true });
        const px = new Uint8Array(c.width * c.height * 4);
        gl.readPixels(0, 0, c.width, c.height, gl.RGBA, gl.UNSIGNED_BYTE, px);
        // Background is --ls-bg (14,17,23). Anything materially different is drawn geometry.
        let lit = 0, hash = 0;
        for (let i = 0; i < px.length; i += 4) {
          if (Math.abs(px[i] - 14) + Math.abs(px[i + 1] - 17) + Math.abs(px[i + 2] - 23) > 24) lit++;
          hash = (hash * 31 + px[i] + px[i + 1] * 3 + px[i + 2] * 7) | 0;
        }
        return { lit, total: px.length / 4, hash };
      });

    const first = await sample();
    assert.ok(
      first.lit > first.total * 0.01,
      `canvas is effectively blank: ${first.lit} of ${first.total} pixels differ from background`,
    );

    const box = await page.locator('[data-testid="viewer3d-canvas"]').boundingBox();
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.mouse.down();
    await page.mouse.move(box.x + box.width / 2 + 120, box.y + box.height / 2 + 40, { steps: 8 });
    await page.mouse.up();
    await page.waitForTimeout(300);
    const orbited = await sample();
    assert.notEqual(orbited.hash, first.hash, "dragging did not change the render — orbit is dead");

    await page.mouse.wheel(0, -400);
    await page.waitForTimeout(300);
    const zoomed = await sample();
    assert.notEqual(zoomed.hash, orbited.hash, "scrolling did not change the render — zoom is dead");

    await page.keyboard.press("Escape");
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(400);
  });

  // T3: a material has no geometry of its own, so the route generates the same shader ball the
  // grid thumbnail renders. It is generated rather than parsed, which is why an in-pack material
  // previews without pulling anything — assert that explicitly, since it is the interesting part.
  await check("a material's shader ball comes back as a unit sphere, in-pack included", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "materials", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.find((i) => i.in_pack) ?? p.items[0];
    assert.ok(packed, "no materials in the library");
    const r = await page.evaluate(async (id) => {
      const res = await fetch(`http://lodestash.localhost/mesh/${id}`);
      if (!res.ok) return { status: res.status };
      const buf = await res.arrayBuffer();
      const { parseLsm1, bounds } = await import("/src/lib/lsm1.ts");
      const meshes = parseLsm1(buf);
      // Every vertex of a unit sphere sits at radius 1 — the cheapest proof this is the ball
      // and not some other mesh that happens to parse.
      let minR = Infinity, maxR = 0;
      for (const m of meshes) {
        for (let i = 0; i < m.positions.length; i += 3) {
          const d = Math.hypot(m.positions[i], m.positions[i + 1], m.positions[i + 2]);
          if (d < minR) minR = d;
          if (d > maxR) maxR = d;
        }
      }
      return {
        status: res.status,
        magic: new TextDecoder().decode(new Uint8Array(buf, 0, 4)),
        tris: meshes.reduce((n, m) => n + m.indices.length / 3, 0),
        hasUvs: meshes.every((m) => m.uvs !== null),
        minR, maxR, radius: bounds(meshes).radius,
      };
    }, packed.asset_id);
    assert.equal(r.status, 200, `in-pack material must still get a ball, got ${r.status}`);
    assert.equal(r.magic, "LSM1");
    assert.ok(r.tris > 100, `too coarse to be the shader ball: ${r.tris} tris`);
    assert.ok(r.hasUvs, "no UVs — the albedo texture would have nowhere to land");
    assert.ok(r.maxR - r.minR < 0.01, `not a unit sphere: radii ${r.minR}..${r.maxR}`);
  });

  // A material is **one** hop from its texture; a model is two. Send a material down the model's
  // two-hop join and it resolves nothing, so the ball renders flat grey with no error anywhere —
  // which looks exactly like a material that genuinely has no texture.
  await check("a material resolves its albedo through the one-hop join", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "materials", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    // Walk the whole tab, not a prefix: only ~130 of the 668 materials have a resolved edge and
    // none of them sort near the front, so sampling the first page proves nothing either way.
    let found = null;
    for (const m of p.items) {
      const d = await invoke("asset_detail", { assetId: m.asset_id });
      if (d?.texture_asset_id) { found = { mat: m.name, tex: d.texture_asset_id }; break; }
    }
    assert.ok(found, "no material resolved a texture — the one-hop join is dead");
    console.log(`       ${found.mat} -> texture ${found.tex}`);
    // And the thing it resolved must be an image, not the material itself or another material.
    const tex = await invoke("asset_detail", { assetId: found.tex });
    assert.ok(
      ["image", "texture", "sprite"].includes(tex.media_type),
      `resolved a ${tex.media_type}, not an image — that is the two-hop bug in reverse`,
    );
  });

  // Warm and cold have to agree about what a model wears (v1 plan S08): the viewer binds one
  // texture per material slot from `lodestash://slottex`, which is the same resolution the CPU
  // thumbnail renders from. Before this the viewer painted one albedo over every slot, and a model
  // whose materials the graph resolved showed flat grey.
  await check("a model wears its textures in Quick Look, one per material slot", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "models", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    // Ask the route itself which models it can actually paint. A model whose textures are still
    // inside a pack resolves an id but no file, and answers 204 — that is the route working, not a
    // subject for the viewer check.
    let subject = null;
    let served = null;
    for (const item of p.items.filter((i) => !i.in_pack).slice(0, 120)) {
      const res = await page.evaluate(async (id) => {
        const r = await fetch(`http://lodestash.localhost/slottex/${id}/0.png`);
        const buf = new Uint8Array(await r.arrayBuffer());
        return { status: r.status, type: r.headers.get("content-type"), bytes: buf.length,
                 png: buf[0] === 0x89 && buf[1] === 0x50 };
      }, item.asset_id);
      if (res.status === 200) {
        subject = item;
        served = res;
        break;
      }
    }
    if (!subject) {
      console.log("       no on-disk model in this library resolves a texture file — nothing to bind");
      return;
    }
    assert.equal(served.type, "image/png");
    assert.ok(served.png && served.bytes > 0, `slottex served ${served.bytes} bytes, PNG=${served.png}`);
    console.log(`       ${subject.name} -> ${served.bytes} bytes of PNG`);

    try {
      // By id, not by name: three assets in this library are called `Characters`, and opening
      // whichever card sorted first asked the viewer about a different model than the one probed.
      await page.fill('input[type="text"]', subject.name);
      const card = `.card[data-asset-id="${subject.asset_id}"]`;
      await page.waitForSelector(card, { timeout: 5000 });
      await page.locator(card).first().dblclick();
      await page.waitForSelector('[data-testid="viewer3d"][data-status="ready"]', { timeout: 20000 });
      await waitFor(
        async () =>
          Number(await page.getAttribute('[data-testid="viewer3d"]', "data-textures")) > 0,
        `${subject.name} bound no texture, though slottex serves one`,
        15000,
        250,
      ).catch(async (e) => {
        // Say which half is missing: the trailer that tells the viewer about slots, or the images.
        const why = await page.evaluate(async (id) => {
          const r = await fetch(`http://lodestash.localhost/mesh/${id}`);
          const buf = new Uint8Array(await r.arrayBuffer());
          const ascii = new TextDecoder("latin1").decode(buf.slice(-4096));
          const tex = await fetch(`http://lodestash.localhost/slottex/${id}/0.png`);
          const img = await new Promise((done) => {
            const i = new Image();
            i.crossOrigin = "anonymous";
            i.onload = () => done(`${i.width}x${i.height}`);
            i.onerror = () => done("error");
            i.src = `http://lodestash.localhost/slottex/${id}/0.png`;
          });
          return {
            id,
            mesh: r.status,
            bytes: buf.length,
            trailer: ascii.lastIndexOf("SLT1"),
            slottex: tex.status,
            img,
            attr: document.querySelector('[data-testid="viewer3d"]')?.getAttribute("data-textures"),
            slots: document.querySelector('[data-testid="viewer3d"]')?.getAttribute("data-slots"),
            texerr: document.querySelector('[data-testid="viewer3d"]')?.getAttribute("data-texerr"),
          };
        }, subject.asset_id);
        throw new Error(`${e.message}; ${JSON.stringify(why)}`);
      });
    } finally {
      // Whatever happened, hand the next check a closed viewer and an empty search box.
      await page.keyboard.press("Escape").catch(() => {});
      await page.fill('input[type="text"]', "").catch(() => {});
      await page.waitForTimeout(400);
    }
  });

  await check("a material renders non-blank pixels in Quick Look", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "materials", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const mat = p.items[0];
    assert.ok(mat, "no materials in the library");
    await page.fill('input[type="text"]', mat.name);
    await page.waitForFunction(
      (n) => document.querySelector(".card .name")?.textContent?.trim() === n,
      mat.name, { timeout: 5000 });
    await page.locator(".card").first().dblclick();
    await page.waitForSelector('[data-testid="viewer3d"][data-status="ready"]', { timeout: 20000 });
    await page.waitForTimeout(600);
    const lit = await page.evaluate(() => {
      const c = document.querySelector('[data-testid="viewer3d-canvas"]');
      const gl = c.getContext("webgl2", { preserveDrawingBuffer: true });
      const px = new Uint8Array(c.width * c.height * 4);
      gl.readPixels(0, 0, c.width, c.height, gl.RGBA, gl.UNSIGNED_BYTE, px);
      let n = 0;
      for (let i = 0; i < px.length; i += 4) {
        if (Math.abs(px[i] - 14) + Math.abs(px[i + 1] - 17) + Math.abs(px[i + 2] - 23) > 24) n++;
      }
      return { n, total: px.length / 4 };
    });
    assert.ok(lit.n > lit.total * 0.01, `blank canvas: ${lit.n} of ${lit.total} pixels lit`);
    await page.keyboard.press("Escape");
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(400);
  });

  // Every asset with pixels should get a picture. The three generators already existed in
  // `lodestash_ingest::thumb`; none of them was reachable from the frontend until `lodestash://thumb`.
  await check("lodestash://thumb renders images, audio waveforms and material balls", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const pick = (mt) => p.items.find((i) => i.media_type === mt && !i.in_pack);
    for (const mt of ["image", "audio", "material"]) {
      const a = pick(mt);
      if (!a) { console.log(`       (skipped ${mt}: none on disk)`); continue; }
      const r = await page.evaluate(async (id) => {
        const res = await fetch(`http://lodestash.localhost/thumb/${id}.png`);
        if (!res.ok) return { status: res.status };
        const buf = new Uint8Array(await res.arrayBuffer());
        const png = buf[0] === 0x89 && buf[1] === 0x50 && buf[2] === 0x4e && buf[3] === 0x47;
        const decoded = await new Promise((ok) => {
          const i = new Image();
          i.onload = () => ok(i.naturalWidth);
          i.onerror = () => ok(0);
          i.src = `http://lodestash.localhost/thumb/${id}.png`;
          setTimeout(() => ok(0), 15000);
        });
        return { status: res.status, png, bytes: buf.length, decoded };
      }, a.asset_id);
      assert.equal(r.status, 200, `${mt}: ${r.status}`);
      assert.ok(r.png, `${mt}: not a PNG`);
      assert.ok(r.decoded > 0, `${mt}: PNG served but the browser could not decode it`);
    }
  });

  // A model thumbnail costs a full mesh parse; a grid page would trigger a hundred of them.
  // Models are served by the vector-preview pipeline instead, and the route refuses so a
  // hand-written URL cannot cause the stampede either.
  await check("lodestash://thumb refuses models — they use lodestash://preview", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "models", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const model = p.items.find((i) => !i.in_pack);
    const status = await page.evaluate(async (id) => {
      const res = await fetch(`http://lodestash.localhost/thumb/${id}.png`);
      return res.status;
    }, model.asset_id);
    assert.equal(status, 404, "a model must not be raster-thumbnailed on demand");
  });

  // The one route that may write: an in-pack image is pulled to the thumbnail cache rather than
  // left as a typed placeholder. Two outcomes are legitimate and nothing else is — a decodable PNG
  // (pack under the size cap) or 503 not_local (pack over it). A 404 would mean the route lost the
  // asset; a 500 would mean the pull half-succeeded and served a truncated file.
  await check("in-pack image thumbnails are pulled, or refused over the size cap (FEATURES §5.4)", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "images", search: "", status: "all", sort_by: "size", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.filter((i) => i.in_pack).slice(0, 6);
    assert.ok(packed.length > 0, "no in-pack images loaded — nothing was exercised");
    const seen = [];
    for (const a of packed) {
      const r = await page.evaluate(async (id) => {
        const res = await fetch(`http://lodestash.localhost/thumb/${id}.png`);
        if (res.status !== 200) return { status: res.status, body: await res.text() };
        const buf = new Uint8Array(await res.arrayBuffer());
        const png = buf[0] === 0x89 && buf[1] === 0x50 && buf[2] === 0x4e && buf[3] === 0x47;
        const decoded = await new Promise((ok) => {
          const i = new Image();
          i.onload = () => ok(i.naturalWidth);
          i.onerror = () => ok(0);
          i.src = `http://lodestash.localhost/thumb/${id}.png`;
          setTimeout(() => ok(0), 15000);
        });
        return { status: res.status, png, decoded };
      }, a.asset_id);
      if (r.status === 200) {
        assert.ok(r.png, `${a.name}: 200 but not a PNG`);
        assert.ok(r.decoded > 0, `${a.name}: PNG served but the browser could not decode it`);
      } else {
        assert.equal(r.status, 503, `${a.name}: expected 200 or 503, got ${r.status}`);
        // The refusal body distinguishes the two honest answers: over the size cap vs a pull
        // that failed for another reason. Either is fine — what matters is the status.
        assert.match(r.body, /not_local|pull cap/);
      }
      seen.push(r.status);
    }
    console.log(`       ${seen.filter((s) => s === 200).length}/${seen.length} pulled, rest over cap`);
    assert.ok(seen.includes(200), "every sampled pack was over the cap — no pull was exercised");
  });

  // The pull must not resurrect the model stampede: a model is refused before any gzip scan,
  // in-pack or not. If this ever answers 200, a grid page of in-pack models re-inflates the
  // archive once per card *and* parses a mesh each time.
  await check("an in-pack model is refused before any pull (FEATURES §5.4)", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "models", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const model = p.items.find((i) => i.in_pack);
    assert.ok(model, "no in-pack model loaded");
    const status = await page.evaluate(async (id) => {
      const res = await fetch(`http://lodestash.localhost/thumb/${id}.png`);
      return res.status;
    }, model.asset_id);
    assert.equal(status, 404, "an in-pack model must be refused, not pulled");
  });

  await check("lodestash:// serves a preview as SVG", async () => {
    // Ask for an asset that actually has a preview rather than whatever is on screen — which
    // page 1 shows depends on the sort, and most of this library is in-pack with no preview.
    const p = await invoke("grid_page", {
      query: { tab: "models", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const withPreview = p.items.find((i) => i.has_preview);
    assert.ok(withPreview, "no asset in the library has a stored preview");
    const r = await page.evaluate(async (id) => {
      const res = await fetch(`http://lodestash.localhost/preview/${id}.svg`);
      return { status: res.status, type: res.headers.get("content-type"), len: (await res.text()).length };
    }, withPreview.asset_id);
    assert.equal(r.status, 200);
    assert.equal(r.type, "image/svg+xml");
    assert.ok(r.len > 100, "preview body is implausibly small");
  });

  await check("lodestash://file honours Range with a 206 and the right window", async () => {
    // Must be an on-disk asset: most of this library is in-pack, and those correctly answer 503.
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const local = p.items.find((i) => !i.in_pack);
    assert.ok(local, "no on-disk asset in the library to range-request");
    const r = await page.evaluate(async (assetId) => {
      const res = await fetch(`http://lodestash.localhost/file/${assetId}`, {
        headers: { Range: "bytes=10-19" },
      });
      return {
        status: res.status,
        range: res.headers.get("content-range"),
        accept: res.headers.get("accept-ranges"),
        len: (await res.arrayBuffer()).byteLength,
      };
    }, local.asset_id);
    assert.equal(r.status, 206, `expected 206, got ${r.status}`);
    assert.equal(r.len, 10, "a 10-byte window must return 10 bytes");
    assert.match(r.range, /^bytes 10-19\/\d+$/);
    assert.equal(r.accept, "bytes");
  });

  // An in-pack asset exists but its bytes are in the archive. 503 not_local is the contract;
  // 404 would tell the client an asset it can see in the grid does not exist.
  await check("a known in-pack asset never 404s: served or not_local (FEATURES §5.4)", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.find((i) => i.in_pack);
    assert.ok(packed, "no in-pack asset loaded");
    const r = await page.evaluate(async (assetId) => {
      const res = await fetch(`http://lodestash.localhost/file/${assetId}`);
      return { status: res.status, body: await res.text() };
    }, packed.asset_id);
    // FEATURES §5.4: a known in-pack asset's payload is materialized on demand (size-capped
    // scratch) — the file route serves it instead of the old not_local 503. The distinction
    // that matters is known-vs-missing: a known asset must never answer 404.
    assert.notEqual(r.status, 404, "a known in-pack asset must not 404");
    assert.ok(r.status === 200 || r.status === 503, `unexpected status ${r.status}`);
    if (r.status === 200) {
      assert.ok(r.body.length > 0, "200 with an empty body");
    } else {
      assert.match(r.body, /not_local/);
    }
  });

  // ── B5: writes ────────────────────────────────────────────────────────────────────────────
  // Every check below reads the value back through a **separate call**, never from the write's
  // own response. A command that returns `{changed: 1}` while persisting nothing is the failure
  // mode here, and it is invisible if you trust the reply.
  //
  // **This suite never deletes a file, and no check here may be changed to.** It runs against the
  // developer's real library, so a test that deletes is a test that destroys someone's assets on
  // a green run. The delete path is exercised the only safe way: against an **in-pack** asset,
  // which the fence must refuse — so the assertion is that nothing happened. The fence's positive
  // case (a real file under a real root passes) lives in `writer.rs`, where the file is one the
  // test created in a temp dir and owns.
  //
  // Catalog writes are fine because they are reversible, and each one is reverted in a `finally`.
  // `remove_from_library` is deliberately **not** exercised here either: it writes an exclude into
  // the developer's real `AppConfig`, which nothing in this suite can take back.

  await check("favourite round-trips through the catalog", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, trashed_only: false, page: 0, page_size: 50 },
    });
    const a = p.items.find((i) => !i.favorite);
    assert.ok(a, "every sampled asset is already starred — nothing to toggle");
    try {
      await invoke("set_favorite", { assetIds: [a.asset_id], on: true });
      const after = await invoke("asset_detail", { assetId: a.asset_id });
      assert.equal(after.favorite, true, "the star did not persist");
      const starred = await invoke("grid_page", {
        query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
                 favorites_only: true, trashed_only: false, page: 0, page_size: 100000 },
      });
      assert.ok(
        starred.items.some((i) => i.asset_id === a.asset_id),
        "starred, but the Favorites filter does not show it",
      );
    } finally {
      // Leave the user's library exactly as it was found.
      await invoke("set_favorite", { assetIds: [a.asset_id], on: false });
    }
    const restored = await invoke("asset_detail", { assetId: a.asset_id });
    assert.equal(restored.favorite, false, "unstarring did not persist");
  });

  // Trash is the recoverable middle option: the row leaves every normal view, appears in exactly
  // one, and comes back intact. A trashed asset that no view can reach is a trapdoor.
  // ── D8: the undo lives where the action was announced ────────────────────────────────────────
  // The whole point is that the toast undoes *its own* op rather than the head of the log, so the
  // check drives it through the UI (right-click → Move to Trash → Undo) and then asks the catalog
  // whether the asset actually came back. Trash is catalog-only — no file is touched either way,
  // and the asset is restored through the backend in `finally` whatever the UI did.
  /**
   * Every `var(--token)` the app actually *renders* resolves to something.
   *
   * A misspelled custom property is the quietest bug CSS has: `var(--font-mono, monospace)` looks
   * like a token with a sensible fallback and is in fact a typo for `--ff-mono` that silently
   * caught itself, so four mono readouts were never JetBrains Mono. Where there is no fallback the
   * declaration is simply dropped — `background: var(--ls-hover)` gave the context menu no hover
   * at all, and `minmax(var(--card, 148px))` pinned the Identify grid to one density forever.
   * None of it warns, none of it fails a build, and none of it is visible unless you already knew.
   *
   * This walks the live DOM instead of the source, so it only judges what the app is really using.
   */
  await check("every CSS token the app renders is a token that exists", async () => {
    const unresolved = await page.evaluate(() => {
      const bad = [];
      const root = getComputedStyle(document.documentElement);
      const seen = new Set();
      for (const sheet of document.styleSheets) {
        let rules;
        try {
          rules = sheet.cssRules;
        } catch {
          continue; // cross-origin; nothing of ours lives there
        }
        const walk = (list) => {
          for (const r of list) {
            if (r.cssRules) walk(r.cssRules);
            if (!r.style) continue;
            // Anything the rule defines itself is defined for its own subtree.
            const local = new Set();
            for (const prop of r.style) if (prop.startsWith("--")) local.add(prop);
            for (const prop of r.style) {
              const value = r.style.getPropertyValue(prop);
              for (const m of value.matchAll(/var\(\s*(--[\w-]+)\s*([,)])/g)) {
                const [, name, next] = m;
                if (local.has(name)) continue;
                if (root.getPropertyValue(name).trim() !== "") continue;
                // A token with a fallback still *renders*, but if the name is wrong the fallback
                // is what you get forever — which is the bug, not the safety net.
                const key = `${name} in ${r.selectorText ?? "?"} { ${prop} }`;
                if (seen.has(key)) continue;
                seen.add(key);
                bad.push({ name, selector: r.selectorText ?? "?", prop, fallback: next === "," });
              }
            }
          }
        };
        walk(rules);
      }
      return bad;
    });
    // Element-scoped tokens (`style="--card: 205px"`, `--h` on a waveform bar, `--depth` on a
    // tree row) live on the element, never on `:root`, so the sweep above cannot see them.
    //
    // Deliberately **not** an allow-list of their names: a hand-kept list of exceptions is the
    // thing that rots, and this check exists precisely because nobody notices token names going
    // stale. The rule instead is *is this name ever assigned anywhere in the running app* —
    // in any stylesheet rule, or in any element's inline style. A name that is assigned nowhere
    // is the bug (`--font-mono`, `--ls-hover`); a name assigned somewhere is a real token whose
    // scope this sweep simply cannot reach.
    //
    // What that trade gives up is the third case: `--card` **was** assigned — on the Library
    // grid — and merely absent on the Identify one. No name-based rule can see that, which is
    // why the element check below names it directly.
    const assigned = await page.evaluate(() => {
      const names = new Set();
      for (const sheet of document.styleSheets) {
        let rules;
        try {
          rules = sheet.cssRules;
        } catch {
          continue;
        }
        const walk = (list) => {
          for (const r of list) {
            if (r.cssRules) walk(r.cssRules);
            if (!r.style) continue;
            for (const prop of r.style) if (prop.startsWith("--")) names.add(prop);
          }
        };
        walk(rules);
      }
      for (const el of document.querySelectorAll("[style]")) {
        for (const prop of el.style) if (prop.startsWith("--")) names.add(prop);
      }
      return [...names];
    });
    // …plus every name the *source* assigns. An inline token can be conditional —
    // `style={traitColor.has(v) ? `--chip: var(${…})` : ""}` — so whether the running app has one
    // set depends on the data in this library, not on whether the code is correct. Reading the
    // components closes that gap without an exceptions list: a name assigned in the source is a
    // real token, and a name assigned nowhere at all is the bug this check is for.
    const src = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "src");
    const files = [];
    (function walkDir(d) {
      for (const e of fs.readdirSync(d, { withFileTypes: true })) {
        const p = path.join(d, e.name);
        if (e.isDirectory()) walkDir(p);
        else if (/\.(svelte|css|ts)$/.test(e.name)) files.push(p);
      }
    })(src);
    const assignedSet = new Set(assigned);
    for (const f of files) {
      for (const m of fs.readFileSync(f, "utf8").matchAll(/(--[\w-]+)\s*:/g)) {
        assignedSet.add(m[1]);
      }
    }
    const real = unresolved.filter((u) => !assignedSet.has(u.name));
    assert.deepEqual(
      real,
      [],
      // One line: the runner prints only a failure's first line, and a list that scrolls off is
      // a list nobody reads.
      `these var() names resolve to nothing \u2014 ` +
        real
          .map((u) => `${u.name} in ${u.selector} { ${u.prop} }${u.fallback ? " [falls back, so it never looks broken]" : ""}`)
          .join(" \u00b7 "),
    );
    // …and the element-scoped ones must actually be set where they are used, or they are the
    // same bug wearing a fallback: the Identify grid read `--card` that nothing assigned.
    const cardSet = await page.evaluate(() => {
      const el = document.querySelector('[data-testid="grid"], [data-testid="backlog"]');
      return el ? el.style.getPropertyValue("--card").trim() : "";
    });
    assert.notEqual(cardSet, "", "the grid reads --card but nothing sets it on the element");
  });

  await check("the Inspector names the pack, and the name opens its stats", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const withPack = p.items.find((i) => i.package);
    if (!withPack) return; // a library of loose files only — nothing to name

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('[data-testid="search-input"]', withPack.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      withPack.asset_id,
      { timeout: 5000 },
    );
    await page.locator(`.card[data-asset-id="${withPack.asset_id}"]`).click();
    await page.waitForSelector('[data-testid="inspector-pack"]', { timeout: 5000 });

    // §13.9's provenance row. The panel named the library and stopped until 2026-08-27, so the
    // place you open to learn about an asset said less about where it came from than the card.
    const named = (await page.textContent('[data-testid="inspector-pack"]'))?.trim();
    assert.equal(named, withPack.package, `the Inspector calls the pack "${named}"`);

    await page.click('[data-testid="inspector-pack"]');
    await page.waitForSelector('[data-testid="pack-stats"]', { timeout: 5000 });
    const title = (await page.locator("#pack-stats-title").textContent())?.trim();
    assert.equal(title, withPack.package, `Pack stats opened on "${title}"`);
    // One modal, one owner: App renders it now, so the grid's copy cannot also be open.
    assert.equal(
      await page.locator('[data-testid="pack-stats"]').count(),
      1,
      "two Pack stats modals are mounted at once",
    );
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="pack-stats"]', { state: "detached", timeout: 3000 });
    await page.fill('[data-testid="search-input"]', "");
    await page.keyboard.press("Escape");
  });

  await check("an in-pack asset says where inside the archive it lives", async () => {
    // §13.9. An in-pack asset has no file to reveal in Explorer, so "where is this?" was
    // answerable nowhere in the app — for 5,963 of 7,691 assets here. Also covers the licence
    // link's colour indirectly: it sits in the same row group and is the only anchor in the UI.
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    let found = null;
    for (const item of p.items.slice(0, 60)) {
      const d = await invoke("asset_detail", { assetId: item.asset_id });
      if (d?.pack_entry_path) { found = { item, d }; break; }
      if (d && !d.in_pack) continue;
    }
    // Most of this library is in-pack, so finding none in sixty means the join is broken.
    assert.ok(found, "no in-pack asset in the first 60 reports where it lives inside its archive");
    assert.ok(
      found.d.pack_archive_path?.toLowerCase().endsWith(".unitypackage"),
      `archive row is not an archive: ${found.d.pack_archive_path}`,
    );

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('[data-testid="search-input"]', found.item.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      found.item.asset_id,
      { timeout: 5000 },
    );
    await page.locator(`.card[data-asset-id="${found.item.asset_id}"]`).click();
    await page.waitForSelector('[data-testid="inspector-pack-path"]', { timeout: 5000 });
    const shownPath = (await page.textContent('[data-testid="inspector-pack-path"]'))?.trim();
    assert.ok(
      shownPath?.includes(found.d.pack_entry_path.split("/").pop()),
      `the row does not name the file: ${shownPath}`,
    );
    await page.waitForSelector('[data-testid="inspector-pack-archive"]', { timeout: 3000 });

    // An on-disk asset must NOT grow these rows — it has an abs_path instead.
    for (const item of p.items.slice(0, 60)) {
      const d = await invoke("asset_detail", { assetId: item.asset_id });
      if (!d || d.in_pack) continue;
      assert.equal(d.pack_entry_path, null, "an on-disk asset claims an in-pack path");
      assert.equal(d.pack_archive_path, null, "an on-disk asset claims an archive");
      break;
    }
    await page.fill('[data-testid="search-input"]', "");
    await page.keyboard.press("Escape");
  });

  await check("SIMILAR shows neighbours, not the dependency list again", async () => {
    // §13.8. Until 2026-08-29 this panel drew the *dependency* edges — the same rows LINKED FILES
    // shows directly above it — under a SIMILAR heading. It is embeddings now (6,807 of 8,237
    // assets carry one here, against 24 carrying a tag), with the index cached in `Library`
    // because this fires on every selection change.
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    let found = null;
    for (const item of p.items.slice(0, 40)) {
      const sim = await invoke("similar_assets", { assetId: item.asset_id, limit: 3 });
      if (sim.length > 0) { found = { item, sim }; break; }
    }
    // Not an early return: most of this library is encoded, so finding nothing in forty assets
    // means the embedding read is broken, not that the library is unencoded.
    assert.ok(found, "no asset in the first 40 has any neighbour — is the index reading anything?");

    assert.ok(found.sim.length <= 3, "§13.8 asks for top 3");
    assert.ok(
      !found.sim.some((s) => s.asset_id === found.item.asset_id),
      "an asset is its own nearest neighbour — self must be dropped",
    );
    // The old bug in one assertion: neighbours are not the asset's dependency edges.
    const edges = await invoke("related_assets", { assetId: found.item.asset_id });
    const edgeIds = new Set(edges.map((e) => e.asset_id).filter((x) => x != null));
    if (edgeIds.size > 0) {
      assert.notDeepEqual(
        found.sim.map((s) => s.asset_id),
        [...edgeIds].slice(0, found.sim.length),
        "SIMILAR is repeating LINKED FILES",
      );
    }

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('[data-testid="search-input"]', found.item.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      found.item.asset_id,
      { timeout: 5000 },
    );
    await page.locator(`.card[data-asset-id="${found.item.asset_id}"]`).click();
    await page.waitForSelector('[data-testid="inspector-similar-0"]', { timeout: 5000 });
    const shown = await page.$$eval('[data-testid^="inspector-similar-"]', (els) =>
      els.map((e) => e.textContent.replace(/\s+/g, " ").trim()),
    );
    assert.equal(shown.length, found.sim.length, "the panel and the command disagree on how many");
    assert.ok(
      shown[0].includes(found.sim[0].name),
      `the panel's first row is not the nearest neighbour: ${shown[0]}`,
    );
    await page.fill('[data-testid="search-input"]', "");
    await page.keyboard.press("Escape");
  });

  await check("Date added and Status are real orderings, not labels", async () => {
    // DESIGN-GUIDELINES §7 asked for both and neither existed. They are cheap to fake — a select
    // option that changes nothing looks identical until you read the grid — so this asserts the
    // order actually changes and matches what the sort claims.
    const q = (sort_by) => ({
      tab: "all", search: "", status: "all", sort_by, sort_asc: true,
      // The whole library, not a page: page 0 of a name-ordered library is all one status
      // here (7,667 of 8,237 are Unidentified), which makes every ordering assertion below
      // trivially true and the check dead. Proven by mutation — an inert Status sort passed
      // the paged version.
      favorites_only: false, page: 0, page_size: 100000,
    });
    const added = await invoke("grid_page", { query: q("added") });
    assert.ok(added.items.length > 1, "not enough assets to order");
    const ids = added.items.map((i) => i.asset_id);
    assert.deepEqual(
      ids,
      [...ids].sort((a, b) => a - b),
      "Date added must be insert order — that is what asset id means here",
    );

    const byStatus = await invoke("grid_page", { query: q("status") });
    const rank = { discovered: 0, indexed: 1, identified: 2, confirmed: 3 };
    const ranks = byStatus.items.map((i) => rank[i.status] ?? 9);
    assert.deepEqual(
      ranks,
      [...ranks].sort((a, b) => a - b),
      "Status must sort by lifecycle, so the unidentified work comes first",
    );
    // …and it must be a different order from name, or it is not sorting at all.
    const byName = await invoke("grid_page", { query: q("name") });
    assert.ok(
      new Set(byStatus.items.map((i) => i.status)).size > 1,
      "every asset shares one status — this check cannot see whether the sort works",
    );
    {
      assert.notDeepEqual(
        byStatus.items.map((i) => i.asset_id),
        byName.items.map((i) => i.asset_id),
        "Status ordering is identical to Name — the sort is inert",
      );
    }
  });

  await check("Settings storage says what the workspace actually holds", async () => {
    // The three STORAGE CAPS controls were inert until 2026-08-29: loaded from config, written
    // back to config, read by nothing. The workspace grew to 3.55 GB across 1,179 folders that no
    // catalog row referenced, and the cap capped nothing.
    //
    // This **plants its own orphan** rather than asserting whatever the machine happens to hold.
    // A check shaped "if there are orphans, assert the callout" passes over an empty workspace
    // without asserting anything — the failure mode the onboarding check sat in for months — and
    // after the first reclaim, a real machine's workspace is exactly that.
    const workspace = path.join(process.env.LOCALAPPDATA, "Lodestash", "workspace");
    const planted = path.join(workspace, "e2e-orphan-" + Date.now());
    fs.mkdirSync(planted, { recursive: true });
    fs.writeFileSync(path.join(planted, "junk.bin"), Buffer.alloc(4096));

    // Away and back, so the screen is mounted fresh after the orphan is planted — clicking
    // Settings while already on Settings is a no-op and would read a stale snapshot.
    const open = async () => {
      await page.getByRole("button", { name: "Library", exact: true }).click();
      await page.getByRole("button", { name: "Settings" }).click();
      await page.waitForSelector('[data-testid="keep-extracted"]', { timeout: 5000 });
    };
    const callout = page.locator('[data-testid="workspace-orphans"]');
    const none = page.locator('[data-testid="workspace-orphans-none"]');

    try {
      const plan = await invoke("workspace_usage");
      assert.ok(
        plan.entries.some((e) => e.path === planted),
        "the planted orphan is not in the plan — the sweep cannot see unreferenced folders",
      );
      assert.ok(plan.bytes >= 4096, "the plan does not count the orphan's bytes");

      await open();
      await callout.waitFor({ timeout: 3000 });
      assert.equal(await none.count(), 0, "both storage states rendered at once");
      const text = (await callout.textContent()) ?? "";
      assert.ok(
        /is not referenced by the catalog/.test(text),
        `orphan callout does not say what it means: ${text}`,
      );
      assert.ok(
        text.includes(plan.entries.length.toLocaleString()),
        `callout does not name the ${plan.entries.length} folders it would remove`,
      );
      await page.waitForSelector('[data-testid="reclaim-workspace"]', { timeout: 3000 });
    } finally {
      fs.rmSync(planted, { recursive: true, force: true });
    }

    // …and with nothing unreferenced, no reclaim is offered. Removed by hand above rather than
    // through the button: this check proves what the screen says, not what the sweep deletes —
    // that is `plan_reclaim`'s own mutation-checked test.
    const after = await invoke("workspace_usage");
    if (after.entries.length === 0) {
      await open();
      await none.waitFor({ timeout: 3000 });
      assert.equal(
        await page.locator('[data-testid="reclaim-workspace"]').count(),
        0,
        "a reclaim button was offered with nothing to reclaim",
      );
    }
  });

  await check("a pack can be kept, and keeping it is not pinning it", async () => {
    // M23. `pinned` already means "Add to Library" (M7) — sharing one flag would make freeing
    // disk space cost the pack its place in the sidebar, so this checks they move independently.
    const packs = await invoke("packages");
    if (packs.length === 0) return;
    const target = packs[0];
    const was = target.keep_cached;
    const pinnedWas = target.pinned;

    const read = async () => (await invoke("packages")).find((p) => p.id === target.id);
    try {
      await invoke("set_pack_keep_cached", { packageId: target.id, keep: !was });
      const after = await read();
      assert.equal(after?.keep_cached, !was, "keep_cached did not flip");
      assert.equal(after?.pinned, pinnedWas, "keeping a pack changed whether it is in the Library");

      // …and the Packages screen shows it.
      await page.getByRole("button", { name: "Packages" }).click();
      const toggle = page.locator(`[data-testid="pack-keep-${target.id}"]`);
      await toggle.waitFor({ timeout: 5000 });
      assert.equal(
        await toggle.getAttribute("aria-pressed"),
        String(!was),
        "the row's toggle disagrees with the catalog",
      );
    } finally {
      await invoke("set_pack_keep_cached", { packageId: target.id, keep: was });
    }
    assert.equal((await read())?.keep_cached, was, "the pack was left as it was found");
  });

  await check("LINKED FILES says which side of the pack boundary a link is on", async () => {
    // §13.6's row state. Until 2026-08-29 every resolved edge rendered identically, so the
    // panel could not tell a dependency that travels with its pack from one that resolves
    // across packs — which is exactly what ENGINE-API §5.5 flags, because Unity reassigns
    // GUIDs on import and that reference does not survive the trip.
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    // Ask the catalog which asset has edges rather than sampling the grid. A name-sorted scan of
    // the first N items finds nothing in this library — the first asset with a dependency sits
    // well past any sane N — so the sampling version of this check asserted nothing at all while
    // reporting ok, which is the failure mode the onboarding check spent months in.
    const edges = await invoke("asset_edges");
    if (edges.length === 0) return; // a library with no harvested edges
    const byId = new Map(p.items.map((i) => [i.asset_id, i]));
    // Prefer an asset that owns a **same-pack** edge, so the ✓/⤷ distinction is actually
    // exercised. Both ends' packs are already on the grid rows, so this is a map lookup, not a
    // scan — and picking any old edge would leave the `in pack` branch unasserted in a library
    // where cross-pack edges outnumber it 1428 to 4.
    const samePack = edges.find((e) => {
      const f = byId.get(e.from);
      const t = byId.get(e.to);
      return f && t && f.package && f.package === t.package;
    });
    const chosen = samePack ?? edges[0];
    const item = byId.get(chosen.from);
    assert.ok(item, `asset ${chosen.from} has edges but is not in the grid`);
    const rows = await invoke("related_assets", { assetId: item.asset_id });
    assert.ok(rows.length > 0, "the edge list and related_assets disagree");
    if (samePack) {
      assert.ok(
        rows.some((r) => r.same_pack === true),
        "picked an asset for its same-pack edge but related_assets reports none",
      );
    }
    const linked = { item, rows };

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('[data-testid="search-input"]', linked.item.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      linked.item.asset_id,
      { timeout: 5000 },
    );
    await page.locator(`.card[data-asset-id="${linked.item.asset_id}"]`).click();
    await page.waitForSelector('[data-testid="inspector-link-state-0"]', { timeout: 5000 });

    const shown = await page.$$eval('[data-testid^="inspector-link-state-"]', (els) =>
      els.map((e) => e.textContent.trim()),
    );
    assert.equal(shown.length, linked.rows.length, "one state chip per edge");
    const allowed = ["✓ in pack", "⤷ in library", "✕ missing", "⊘ ignored"];
    for (const s of shown) assert.ok(allowed.includes(s), `unknown link state "${s}"`);
    // The chip must agree with the row it came from, not just be one of the four.
    const expected = linked.rows.map((r) =>
      r.state === "ignored"
        ? "⊘ ignored"
        : r.asset_id == null
          ? "✕ missing"
          : r.same_pack
            ? "✓ in pack"
            : "⤷ in library",
    );
    assert.deepEqual(shown, expected, "the chips do not match the edges they render");

    await page.fill('[data-testid="search-input"]', "");
    await page.keyboard.press("Escape");
  });

  await check("⊘ Ignore mutes a link and ↺ restore puts it back", async () => {
    // Deliberately over **any** edge, not a broken one. A real library has thousands of resolved
    // edges and a handful of broken ones — the reference library's two sit at sorted position
    // 2414, so a check that hunted for a missing edge in the first N items would quietly assert
    // nothing, which is how the onboarding check spent months passing over a screen it could not
    // see. The mute path is identical either way, and the properties a broken edge adds (an
    // ignore surviving `record_dependencies` re-deriving the harvest, and staying out of the
    // late-binding pass) are unit-tested and mutation-checked in `ignored_edges_survive_a_reharvest`.
    const edges = await invoke("asset_edges");
    if (edges.length === 0) return; // a library with no harvested edges
    const assetId = edges[0].from;
    const live = (await invoke("related_assets", { assetId })).find((r) => r.state !== "ignored");
    assert.ok(live, `asset ${assetId} has edges but related_assets shows none live`);
    const target = { edge: live.edge_id, was: live.state, outbound: live.outbound };
    assert.ok(target.outbound, "expected an outbound edge — mute clears the *target* end");

    const stateOf = async () =>
      (await invoke("related_assets", { assetId })).find((r) => r.edge_id === target.edge);
    try {
      await invoke("set_link_ignored", { edgeId: target.edge, ignored: true });
      const muted = await stateOf();
      assert.equal(muted?.state, "ignored", "the edge was not muted");
      assert.equal(muted?.asset_id, null, "a muted edge must resolve to nothing");
      assert.equal(muted?.same_pack, null, "and so has no side of the pack boundary");
    } finally {
      // Always hand the user's catalog back the way it was found.
      await invoke("set_link_ignored", { edgeId: target.edge, ignored: false });
    }
    assert.equal((await stateOf())?.state, target.was, "restore did not put the edge back");
  });

  await check("the Inspector shows the licence a pack carries", async () => {
    // §13.9's other half. Until 2026-08-28 no pack in the catalog had a licence at all: the
    // desktop write path read `read_package_meta` at discovery, kept `version`, and threw the
    // licence away without ever calling `set_package_metadata` - so the row was left out of the
    // panel on the grounds that it would always read "unknown".
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    let licensed = null;
    for (const item of p.items.filter((i) => i.package).slice(0, 40)) {
      const d = await invoke("asset_detail", { assetId: item.asset_id });
      if (d?.license) { licensed = { item, detail: d }; break; }
    }
    if (!licensed) return; // no pack in this library states a licence - nothing to render

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('[data-testid="search-input"]', licensed.item.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      licensed.item.asset_id,
      { timeout: 5000 },
    );
    await page.locator(`.card[data-asset-id="${licensed.item.asset_id}"]`).click();
    await page.waitForSelector('[data-testid="inspector-license"]', { timeout: 5000 });
    const shown = (await page.textContent('[data-testid="inspector-license"]'))?.trim();
    assert.equal(shown, licensed.detail.license, `the Inspector reads the licence as "${shown}"`);
    await page.fill('[data-testid="search-input"]', "");
    await page.keyboard.press("Escape");
  });

  /**
   * The tray is one object now, not two (ADR-0010).
   *
   * The shell kept `Library::staged` in memory while ENGINE-API §6 read
   * `collection(is_staging=1)`, so "what is staged" had two answers and `list_staged` returned
   * `[]` over a tray holding picks. This stages through the UI and reads the count back off the
   * **catalog** — the tray is a collection, so its membership is a catalog fact now.
   */
  await check("staging writes to the catalog, not to a list beside it", async () => {
    await page.fill('[data-testid="search-input"]', "");
    await page.waitForFunction(() => document.querySelectorAll(".card").length > 1, undefined, {
      timeout: 5000,
    });
    const before = await invoke("staged");
    const ids = await page.evaluate(() =>
      [...document.querySelectorAll(".card")].slice(0, 2).map((c) => Number(c.getAttribute("data-asset-id"))),
    );
    try {
      await invoke("stage", { ids });
      const now = await invoke("staged");
      for (const id of ids) assert.ok(now.includes(id), `${id} did not reach the tray`);

      // Rebuilding the library from the catalog must not lose them — the tray is not view state.
      await invoke("reload_library");
      const after = await invoke("staged");
      for (const id of ids) {
        assert.ok(after.includes(id), `${id} fell out of the tray on reload`);
      }

      // Clearing empties it, and reports how many picks it dropped.
      const cleared = await invoke("clear_staged");
      assert.ok(cleared.changed >= ids.length, `clear reported ${cleared.changed} of ${ids.length}`);
      assert.deepEqual(await invoke("staged"), [], "clear left picks behind");
    } finally {
      if (before.length > 0) await invoke("stage", { ids: before });
    }
  });

  await check("the Inspector offers Stage, and it reaches the tray", async () => {
    await page.fill('[data-testid="search-input"]', "");
    await page.waitForFunction(() => document.querySelectorAll(".card").length > 0, undefined, {
      timeout: 5000,
    });
    await page.locator(".card").first().click();
    await page.waitForSelector('[data-testid="action-stage"]', { timeout: 5000 });
    const id = Number(
      await page.evaluate(() => document.querySelector(".card.selected")?.getAttribute("data-asset-id")),
    );
    const before = (await invoke("staged")).length;
    try {
      await page.click('[data-testid="action-stage"]');
      await waitFor(
        async () => (await invoke("staged")).includes(id),
        "the Inspector's Stage did not reach the tray",
      );
      // And it is a toggle, not an add — the label has to answer back.
      await waitFor(
        async () => (await page.textContent('[data-testid="action-stage"]')).includes("Staged"),
        "the button never said it was staged",
      );
      await page.click('[data-testid="action-stage"]');
      await waitFor(
        async () => !(await invoke("staged")).includes(id),
        "Stage would not un-stage",
      );
    } finally {
      await invoke("unstage", { ids: [id] });
    }
    assert.equal((await invoke("staged")).length, before, "the tray did not come back to where it was");
  });

  await check("sort goes inert while AI results are ranked", async () => {
    // The toolbar's own copy of the sort (the ⋯ popover holds the other, and only when open). This
    // read `[data-testid="toolbar"] .sort select` until 2026-09-12 — a test id the toolbar never
    // had — so the first assertion waited out Playwright's 30 s default, was caught, and skipped:
    // a check that took thirty seconds to assert less than it said.
    const sort = ".sort-inline .sort select";
    await page.fill('[data-testid="search-input"]', "");
    assert.equal(await page.isDisabled(sort), false, "sort is disabled with no query at all");

    // Escalate to the AI the way the UI offers it — the row pinned under the filtered results. The
    // row appears after the substring search settles, so wait for results rather than reading the
    // DOM on the keystroke; a query with no results is offered the AI by the empty state instead.
    await page.fill('[data-testid="search-input"]', "stone");
    const offered = await page
      .waitForSelector('[data-testid="escalate"]', { timeout: 3000 })
      .then(() => true)
      .catch(() => false);
    if (!offered) {
      const cards = await page.evaluate(() => document.querySelectorAll(".card").length);
      assert.equal(cards, 0, `"stone" shows ${cards} results and no Ask AI row under them`);
      return;
    }
    await page.click('[data-testid="escalate"]');
    await waitFor(
      async () => await page.isDisabled(sort),
      "the sort stayed live over AI-ranked results, where changing it throws the ranking away",
    );
    await page.fill('[data-testid="search-input"]', "");
    await waitFor(
      async () => !(await page.isDisabled(sort)),
      "clearing the query left the sort disabled",
    );
  });

  // ---- The keyboard map (UI-SPEC §0.4) ----------------------------------------------------
  // Every row below was wrong or absent until the 2026-08-26 audit, and none of it was visible
  // from a screenshot: a key that does nothing looks exactly like a key you pressed wrong.

  await check("/ focuses the search box from anywhere", async () => {
    await page.fill('[data-testid="search-input"]', "");
    await page.locator(".card").first().click();
    await page.waitForSelector('[data-testid="inspector-name"]', { timeout: 5000 });
    await page.keyboard.press("/");
    const focused = await page.evaluate(
      () => document.activeElement?.getAttribute("data-testid"),
    );
    assert.equal(focused, "search-input", "/ did not reach the search box");
    // And it must not type itself into the box it just focused.
    const value = await page.inputValue('[data-testid="search-input"]');
    assert.equal(value, "", `/ left "${value}" in the box`);
    await page.keyboard.press("Escape");
    await page.locator("body").click({ position: { x: 5, y: 5 } });
  });

  // Budget: two waits on whole-library favourites queries (page_size 100000), polled every 250 ms
  // so they do not starve the write they wait for — ~4.6 s on the real library, too close to the
  // default ceiling to leave on it.
  await check("F toggles the star rather than only adding it", { budget: 12_000 }, async () => {
    await page.fill('[data-testid="search-input"]', "");
    await page.waitForFunction(() => document.querySelectorAll(".card").length > 0, undefined, {
      timeout: 5000,
    });
    await page.locator(".card").first().click();
    await page.waitForSelector('[data-testid="inspector-name"]', { timeout: 5000 });
    // Wait for the *selection* too, not just the Inspector. Reading `.card.selected` before the
    // class lands gave `id = NaN`, so every later locator matched nothing and the check died on a
    // 30-second Playwright timeout with no hint of why — which is how this read as a flake in the
    // toggle rather than a missing wait here.
    await page.waitForSelector(".card.selected", { timeout: 5000 });
    const id = Number(
      await page.evaluate(() => document.querySelector(".card.selected")?.getAttribute("data-asset-id")),
    );
    assert.ok(Number.isFinite(id), "no card is selected — the rest of this check cannot mean anything");
    // The **catalog** is the authority here, not the DOM. Two earlier attempts read the star off
    // the card and both broke on rendering, not on the toggle: the grid is virtualised to ~100
    // cards, and a refresh after the write drops the selected card out of the rendered window
    // ("card 2427 is not in the DOM (100 cards rendered)"). What this check is about is whether a
    // second F undoes the first, which is a fact about stored state.
    //
    // Polled every 250 ms, not the default 50: this predicate is a whole-library query, and firing
    // ~100 of them inside a five-second window competed with the very write being waited for. That
    // was the original flake, and it reported itself as "the second F did not toggle it back".
    const starred = async () => {
      const p = await invoke("grid_page", {
        query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
                 favorites_only: true, trashed_only: false, page: 0, page_size: 100000 },
      });
      return p.items.some((i) => i.asset_id === id);
    };
    const was = await starred();
    // Record what each F actually sends, so a failure names the culprit: a stale `on` means the
    // shell's view of the star lagged; the right `on` with no effect means the write did not land.
    await page.evaluate(() => {
      window.__favSent = [];
      const prev = window.fetch.bind(window);
      window.fetch = (input, init) => {
        const url = String(input && input.url ? input.url : input);
        if (url.startsWith("http://ipc.localhost/set_favorite")) {
          window.__favSent.push(typeof init?.body === "string" ? init.body : "?");
        }
        return prev(input, init);
      };
    });
    const sent = () => page.evaluate(() => JSON.stringify(window.__favSent));
    try {
      await page.keyboard.press("f");
      await waitFor(async () => (await starred()) !== was, "F did not change the star", 8000, 250)
        .catch(async (e) => { throw new Error(`${e.message}; set_favorite sent ${await sent()}`); });
      await page.keyboard.press("f");
      await waitFor(
        async () => (await starred()) === was,
        "the second F did not toggle it back",
        8000,
        250,
      ).catch(async (e) => { throw new Error(`${e.message}; set_favorite sent ${await sent()}`); });
    } finally {
      await invoke("set_favorite", { assetIds: [id], on: was });
    }
  });

  await check("Ctrl-A picks the page, and a filter change disarms the bulk bar", async () => {
    await page.fill('[data-testid="search-input"]', "");
    await page.waitForFunction(() => document.querySelectorAll(".card").length > 2, undefined, {
      timeout: 5000,
    });
    await page.locator(".card").first().click();
    const cards = await page.evaluate(() => document.querySelectorAll(".card").length);

    await page.keyboard.press("Control+a");
    await page.waitForSelector('[data-testid="bulk-count"]', { timeout: 5000 });
    const picked = Number(
      (await page.textContent('[data-testid="bulk-count"]')).replace(/\D+/g, ""),
    );
    assert.equal(picked, cards, `Ctrl-A picked ${picked} of the ${cards} cards on the page`);

    // The pick set is what the bulk bar writes tags to. Narrowing the filter has to disarm it,
    // or `+ Add tag` stays live and aimed at assets that are no longer on screen.
    await page.fill('[data-testid="search-input"]', "zzzz-no-such-asset");
    await waitFor(
      async () => (await page.locator('[data-testid="bulk-count"]').count()) === 0,
      "the bulk bar survived a filter change, still holding the old picks",
    );
    await page.fill('[data-testid="search-input"]', "");
    await page.keyboard.press("Escape");
  });

  // Budget: ends by waiting out the Undo toast's lifetime (up to 8 s) so the next check cannot
  // mistake this one's toast for its own — ~8.8 s every run.
  await check("Delete moves the selection to Trash, through the confirm gate", { budget: 15_000 }, async () => {
    const live = () =>
      invoke("grid_page", {
        query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
                 favorites_only: false, trashed_only: false, page: 0, page_size: 100000 },
      });
    await page.fill('[data-testid="search-input"]', "");
    await page.waitForFunction(() => document.querySelectorAll(".card").length > 0, undefined, {
      timeout: 5000,
    });
    await page.locator(".card").first().click();
    await page.waitForSelector('[data-testid="inspector-name"]', { timeout: 5000 });
    const id = Number(
      await page.evaluate(() => document.querySelector(".card.selected")?.getAttribute("data-asset-id")),
    );
    const before = (await live()).total;
    try {
      // First press arms; §11.7's gate is on by default, so one press must NOT bin anything.
      await page.keyboard.press("Delete");
      await page.waitForSelector(".toast", { timeout: 5000 });
      assert.equal((await live()).total, before, "the first Delete binned it without confirming");

      await page.keyboard.press("Delete");
      await waitFor(async () => (await live()).total === before - 1, "Delete never moved it to Trash");
    } finally {
      // Catalog-only and reversible — the file on disk was never touched (DATA-MODEL §8).
      await invoke("set_trashed", { assetIds: [id], trashed: false });
    }
    assert.equal((await live()).total, before, "restore did not bring it back");

    // Leave no Undo toast behind. The next check waits 1.5 s for a `toast-action` to decide
    // whether its own trash click armed or committed — this check's Undo would still be on
    // screen, it would read it as "committed", skip its confirming click, and fail on a trash
    // that never happened. A leaked toast is as much cross-check state as a leaked window size.
    await page.waitForFunction(
      () => !document.querySelector('[data-testid="toast-action"]'),
      undefined,
      { timeout: 8000 },
    );
  });

  await check("the toast that announces a trash offers the undo that reverses it", async () => {
    await page.fill('input[type="text"]', "");
    await page.waitForFunction(() => document.querySelectorAll(".card").length > 3, undefined, {
      timeout: 5000,
    });
    const card = page.locator(".card").first();
    const name = (await card.locator(".name").textContent())?.trim();
    const before = await invoke("library_status");

    let victim = null;
    try {
      // Driven from the inspector rather than the context menu: same `write()` path, one fewer
      // moving part, and it needs the selection open anyway.
      await card.click();
      await page.waitForSelector('[data-testid="action-trash"]', { timeout: 5000 });
      await page.click('[data-testid="action-trash"]');
      // Trash is confirm-gated (UI-SPEC §11.6), and the gate is an *arming* click rather than a
      // dialog: the first press arms, the second commits. Tolerate both settings by asking for the
      // toast, and only pressing again if the first press merely armed it.
      const armed = await page
        .waitForSelector('[data-testid="toast-action"]', { timeout: 1500 })
        .then(() => true)
        .catch(() => false);
      if (!armed) await page.click('[data-testid="action-trash"]');

      await page.waitForSelector('[data-testid="toast-action"]', { timeout: 5000 });
      const label = (await page.locator('[data-testid="toast-action"]').textContent())?.trim();
      assert.equal(label, "Undo", `the trash toast offered "${label}" instead of an Undo`);

      const mid = await invoke("library_status");
      assert.equal(mid.trashed, before.trashed + 1, `trashing "${name}" did not reach the catalog`);
      victim = mid;

      await page.click('[data-testid="toast-action"]');
      await page.waitForFunction(
        () => !document.querySelector('[data-testid="toast-action"]'),
        undefined,
        { timeout: 5000 },
      );
      const after = await invoke("library_status");
      assert.equal(after.trashed, before.trashed, "Undo did not put the asset back");
    } finally {
      // Belt and braces: if the UI path failed halfway, the catalog still goes back to where it was.
      if (victim) {
        const binned = await invoke("grid_page", {
          query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
                   favorites_only: false, trashed_only: true, page: 0, page_size: 100000 },
        });
        const ids = binned.items.map((i) => i.asset_id);
        if (ids.length > 0) await invoke("set_trashed", { assetIds: ids, trashed: false });
      }
    }
  });

  await check("trash hides an asset, the Trash lists it, restore brings it back (UI-SPEC §10)", async () => {
    const live = () =>
      invoke("grid_page", {
        query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
                 favorites_only: false, trashed_only: false, page: 0, page_size: 100000 },
      });
    const trash = () =>
      invoke("grid_page", {
        query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
                 favorites_only: false, trashed_only: true, page: 0, page_size: 100000 },
      });
    const before = await live();
    const a = before.items[0];
    assert.ok(a, "empty library");
    try {
      await invoke("set_trashed", { assetIds: [a.asset_id], trashed: true });
      const after = await live();
      assert.equal(after.total, before.total - 1, "the library count did not drop");
      assert.ok(!after.items.some((i) => i.asset_id === a.asset_id), "still in the library view");
      const binned = await trash();
      assert.ok(binned.items.some((i) => i.asset_id === a.asset_id), "not in the Trash either");
      const status = await invoke("library_status");
      assert.ok(status.trashed > 0, "library_status does not count it");
    } finally {
      await invoke("set_trashed", { assetIds: [a.asset_id], trashed: false });
    }
    const back = await live();
    assert.equal(back.total, before.total, "restore did not bring it back");
  });

  // The delete fence, exercised through the real command against a real asset. An in-pack asset
  // has no file of its own — its `abs_path` is the .unitypackage holding hundreds of others — so
  // deleting it must refuse, change nothing, and leave the asset exactly where it was.
  await check("delete_from_disk refuses an in-pack asset instead of touching its archive", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, trashed_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.find((i) => i.in_pack);
    assert.ok(packed, "no in-pack asset loaded");
    const r = await invoke("delete_from_disk", { assetIds: [packed.asset_id] });
    assert.equal(r.changed, 0, "it reported deleting something");
    assert.equal(r.refused, 1, "it was not refused by the fence");
    const still = await invoke("asset_detail", { assetId: packed.asset_id });
    assert.ok(still && !still.trashed, "the asset was trashed despite nothing being deleted");
  });

  // The dialog is the whole point of the feature: it must name the file, say the word, and offer
  // the safe alternative. Cancelling must leave the asset alone.
  await check("the delete dialog names the path, says permanently, and offers Remove", async () => {
    // An on-disk asset specifically: an in-pack one has no file of its own, so the Inspector
    // deliberately offers no delete button for it (checked separately below).
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, trashed_only: false, page: 0, page_size: 100000 },
    });
    const local = p.items.find((i) => !i.in_pack);
    assert.ok(local, "no on-disk asset in the library");
    await page.fill('input[type="text"]', local.name);
    await page.waitForFunction(
      (n) => document.querySelector(".card .name")?.textContent?.trim() === n,
      local.name, { timeout: 5000 });
    await page.locator(".card").first().click();
    // Wait for the Inspector to be describing *this* asset. It keeps showing the previous
    // selection until its detail fetch resolves, so asserting straight after the click reads
    // whatever the last check left behind.
    await page.waitForFunction(
      (n) => document.querySelector('[data-testid="inspector-name"]')?.textContent?.trim() === n,
      local.name, { timeout: 5000 });
    await page.waitForSelector('[data-testid="inspector-actions"]', { timeout: 5000 });
    const name = local.name;

    await page.locator('[data-testid="action-delete"]').click();
    await page.waitForSelector('[data-testid="confirm-delete"]', { timeout: 5000 });

    const text = await page.locator('[data-testid="confirm-delete"]').textContent();
    assert.match(text, /permanently/i, "the dialog never says the word");
    const paths = await page.locator('[data-testid="confirm-delete-paths"]').textContent();
    assert.match(paths, /[\\/]/, "no path shown — the user cannot see what is about to go");
    assert.ok(
      await page.locator('[data-testid="confirm-delete-remove"]').isVisible(),
      "the safe alternative is not offered where the decision is made",
    );
    // Delete must not be the default action: Escape and the scrim both cancel.
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="confirm-delete"]', { state: "detached", timeout: 5000 });

    const shown = (await page.locator('[data-testid="inspector-name"]').textContent())?.trim();
    assert.equal(shown, name, "cancelling the dialog changed the selection");
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(400);
  });

  // The counterpart: an in-pack asset offers no delete button at all. A button that can only ever
  // fail reads as a broken app rather than an impossible action — and the empty path list this
  // check was born from told the user nothing about what was at stake.
  await check("an in-pack asset offers no delete button", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, trashed_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.find((i) => i.in_pack);
    await page.fill('input[type="text"]', packed.name);
    await page.waitForFunction(
      (n) => document.querySelector(".card .name")?.textContent?.trim() === n,
      packed.name, { timeout: 5000 });
    await page.locator(".card").first().click();
    await page.waitForFunction(
      (n) => document.querySelector('[data-testid="inspector-name"]')?.textContent?.trim() === n,
      packed.name, { timeout: 5000 });
    await page.waitForSelector('[data-testid="inspector-actions"]', { timeout: 5000 });
    assert.equal(
      await page.locator('[data-testid="action-delete"]').count(),
      0,
      "an in-pack asset must not offer a delete that cannot work",
    );
    // Remove from library still applies — it hides the row without touching any bytes.
    assert.ok(await page.locator('[data-testid="action-remove"]').isVisible());
    await page.fill('input[type="text"]', "");
    await page.waitForTimeout(400);
  });

  await check("an unknown lodestash:// route is a 404, not a hang", async () => {
    const status = await page.evaluate(async () => {
      const res = await fetch("http://lodestash.localhost/nonsense/1");
      return res.status;
    });
    assert.equal(status, 404);
  });

  await check("an unknown asset id is a 404", async () => {
    const status = await page.evaluate(async () => {
      const res = await fetch("http://lodestash.localhost/file/999999999");
      return res.status;
    });
    assert.equal(status, 404);
  });

  // Regression: NEW TRAIT was dead — upsertTrait omitted the non-optional `teach`, so Tauri
  // rejected every create with "missing required key" and only the error bar showed. The full
  // UI path, verified through a separate read call, not the response.
  await check("NEW TRAIT creates a trait through the screen", async () => {
    // A **fixed** name, not a timestamped one. `unpromote_trait` below demotes the row; it
    // does not delete it, so a per-run name left one plain tag behind every single run —
    // 171 of the 210 rows in the reference catalog were this check's residue, and the
    // cleanup comment claimed the opposite. `upsert_trait` is idempotent on the
    // normalised name, so a fixed one re-promotes the same row and the suite costs the
    // vocabulary exactly one entry, for ever.
    const name = "e2e-trait";
    await page.getByRole("button", { name: "Traits" }).click();
    await page.waitForSelector('[data-testid="new-trait"]', { timeout: 5000 });
    await page.fill('[data-testid="new-trait-name"]', name);
    await page.getByTestId("new-trait-go").click();
    // The row appears in the list only if the write actually landed and the screen re-read.
    await page.waitForFunction(
      (n) =>
        [...document.querySelectorAll('[data-testid="trait-row"]')].some((el) =>
          el.textContent.includes(n),
        ),
      name,
      { timeout: 5000 },
    );
    const vocab = await invoke("vocabulary");
    const row = vocab.find((v) => v.value === name);
    assert.ok(row, `trait ${name} never reached the vocabulary`);
    assert.equal(row.is_trait, true, "created row must be a trait, not a plain tag");
    assert.equal(row.teach, false, "a NEW TRAIT should default to not teaching");
    // Demote, so the Traits screen is left as it was found. The row itself remains — there
    // is no delete-tag path on any plane — which is why the name is fixed.
    await invoke("unpromote_trait", { tagId: row.id });
    // And prove the suite is not seeding vocabulary: exactly one e2e row, this one.
    const residue = (await invoke("vocabulary")).filter((v) => v.value.startsWith("e2e-"));
    assert.equal(
      residue.length,
      1,
      `the suite has left ${residue.length} rows in the user's vocabulary: ` +
        residue.map((r) => r.value).join(", "),
    );
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // Regression: the tier inputs were one-way (`value=` with no bind), so a typed edit never
  // reached state and blur saved the stale number. Verified through get_settings, not the
  // blur handler's own response.
  await check("editing a size tier persists", async () => {
    const before = await invoke("get_settings");
    const target = before.m_kb === 1000 ? 1400 : 1000;
    await page.getByRole("button", { name: "Settings" }).click();
    await page.waitForSelector('[data-testid="tier-m_kb"]', { timeout: 5000 });
    await page.fill('[data-testid="tier-m_kb"]', String(target));
    await page.keyboard.press("Enter"); // blur → fixBoundaries + save
    let saved = null;
    for (let i = 0; i < 20 && saved?.m_kb !== target; i++) {
      await page.waitForTimeout(200);
      saved = await invoke("get_settings");
    }
    assert.equal(saved?.m_kb, target, `tier edit did not persist (got ${saved?.m_kb})`);
    // Restore the pre-test value so the library's badges keep their previous thresholds.
    await invoke("set_settings", { settings: before });
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // B10k: the grid pins its tracks at `--card` (229.25px), but a `.card` is a <button> and
  // buttons default to `fit-content` — so a long asset name used to stretch that one card past
  // its track and leave the row ragged. Every card must sit exactly in its grid track, and the
  // track is the `.cell` that wraps it, so both are pinned here.
  await check("every card is exactly its grid track", async () => {
    await page.waitForSelector(".grid .card", { timeout: 5000 });
    const widths = await page.evaluate(() =>
      [...document.querySelectorAll(".grid .card")].map((c) => {
        const cell = c.closest(".cell");
        return { card: c.getBoundingClientRect().width, cell: cell?.getBoundingClientRect().width ?? null };
      }),
    );
    assert.ok(widths.length > 0, "no cards on screen — nothing was exercised");
    const cardWidths = widths.map((w) => w.card);
    const spread = Math.max(...cardWidths) - Math.min(...cardWidths);
    assert.ok(spread < 1, `cards span ${spread.toFixed(3)}px — a card is stretching past its track`);
    const offTrack = widths.filter((w) => w.cell !== null && Math.abs(w.card - w.cell) >= 1);
    assert.equal(offTrack.length, 0, "a card is wider than the grid cell that wraps it");
  });

  // The sound card's ▶ used to be a glyph inside an `aria-hidden` wrapper with no handler: clicking
  // it selected the card, and a screen reader could not reach it at all. 1,841 of this library's
  // 7,691 assets are audio, so that was a quarter of the grid with no way to hear anything.
  //
  // "It rendered" is not the assertion — `currentTime` advancing is. A transport that draws a ◼ and
  // plays silence looks identical to one that works.
  await check("the sound card's ▶ actually plays, and only one plays at a time", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "sounds", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 200 },
    });
    const sounds = p.items.filter((i) => !i.in_pack);
    if (sounds.length < 2) return; // needs two on-disk sounds to prove the "one at a time" rule

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.getByRole("button", { name: /^Sounds/ }).click();
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"] [data-testid="card-play"]`),
      sounds[0].asset_id,
      { timeout: 5000 },
    );

    const play = (id) => page.locator(`.card[data-asset-id="${id}"] [data-testid="card-play"]`);
    const first = play(sounds[0].asset_id);
    assert.equal(await first.getAttribute("tabindex"), "0", "the transport is not reachable by keyboard");

    await first.click();
    // The element is the app's only <audio>; poll it rather than trusting the button's glyph.
    await page.waitForFunction(
      () => {
        const a = document.querySelector("audio") ?? null;
        return !!a && !a.paused && a.currentTime > 0;
      },
      undefined,
      { timeout: 8000 },
    );
    assert.equal(await first.getAttribute("aria-pressed"), "true", "the button did not flip to playing");
    // The scrubber and clock belong to the active card only (§2.5).
    const scrubs = await page.locator('[data-testid="card-scrub"]').count();
    assert.equal(scrubs, 1, `${scrubs} cards drew a scrubber — only the active one should`);

    // Starting another sound stops the first, because there is one element, not one per card.
    await play(sounds[1].asset_id).click();
    await page.waitForFunction(
      (id) =>
        document
          .querySelector(`.card[data-asset-id="${id}"] [data-testid="card-play"]`)
          ?.getAttribute("aria-pressed") === "false",
      sounds[0].asset_id,
      { timeout: 8000 },
    );
    assert.equal(
      await page.locator('[data-testid="card-scrub"]').count(), 1,
      "two cards claim to be the active audio",
    );

    // Stop, so the suite does not leave a sound running behind every later check.
    await play(sounds[1].asset_id).click();
    await page.waitForFunction(() => (document.querySelector("audio")?.paused ?? true), undefined, {
      timeout: 5000,
    });
    await page.getByRole("button", { name: /^All/ }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // UI-SPEC §13.2. The rule worth a check is the one that is easy to get backwards: the scrubber
  // appears **after first play**, with a hint before it. A timeline you cannot scrub yet is a
  // control that lies about what it does.
  await check("the inspector's transport hints before first play, then scrubs", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "sounds", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 200 },
    });
    const sound = p.items.find((i) => !i.in_pack);
    if (!sound) return;

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.getByRole("button", { name: /^Sounds/ }).click();
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      sound.asset_id,
      { timeout: 5000 },
    );
    // Select it *without* hitting the card's own ▶ — the inspector is what is under test.
    await page.locator(`.card[data-asset-id="${sound.asset_id}"] .name`).click();
    await page.waitForSelector('[data-testid="inspector-transport"]', { timeout: 5000 });

    assert.equal(
      await page.locator('[data-testid="inspector-scrub"]').count(), 0,
      "the scrubber is offered before anything has played",
    );
    const hint = await page.locator('[data-testid="inspector-transport"] .hint').textContent();
    assert.ok(hint?.includes("play to scrub"), `the hint reads: ${hint}`);

    await page.locator('[data-testid="inspector-play"]').click();
    await page.waitForSelector('[data-testid="inspector-scrub"]', { timeout: 8000 });
    await page.waitForFunction(
      () => {
        const a = document.querySelector("audio");
        return !!a && !a.paused && a.currentTime > 0;
      },
      undefined,
      { timeout: 8000 },
    );
    const clock = await page.locator('[data-testid="inspector-clock"]').textContent();
    assert.ok(/\d+:\d\d/.test(clock ?? ""), `the readout is not a clock: ${clock}`);

    // §13.2: leaving the screen stops it. Audio with no visible transport is audio you cannot stop.
    await page.getByRole("button", { name: "Settings" }).click();
    await page.waitForFunction(() => (document.querySelector("audio")?.paused ?? true), undefined, {
      timeout: 5000,
    });
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.getByRole("button", { name: /^All/ }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // The grid's metadata is a control surface, not decoration (UI-SPEC §2.5: "pack name is a link
  // → Pack stats §15", "chips are click-to-search"). Both were spec'd, neither was wired — the
  // pack name was baked into a joined string, so there was nothing to attach a link to.
  await check("a card's pack name opens Pack stats for that pack", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const withPack = p.items.find((i) => i.package);
    if (!withPack) return; // a library of loose files only — nothing to link

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('input[type="text"]', withPack.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      withPack.asset_id,
      { timeout: 5000 },
    );
    const card = page.locator(`.card[data-asset-id="${withPack.asset_id}"]`);
    await card.locator('[data-testid="card-pack"]').click();
    await page.waitForSelector('[data-testid="pack-stats"]', { timeout: 5000 });
    await page.waitForSelector('[data-testid="pack-stats-grid"]', { timeout: 5000 });
    const title = (await page.locator("#pack-stats-title").textContent())?.trim();
    assert.equal(title, withPack.package, `Pack stats opened on "${title}"`);
    // It must not also have opened Quick Look: the click has to stop at the link.
    assert.equal(await page.locator('[data-testid="quicklook"]').count(), 0, "the pack link also opened Quick Look");
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="pack-stats"]', { state: "detached", timeout: 3000 });
    await page.fill('input[type="text"]', "");
  });

  await check("a list row's pack name opens it too", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const withPack = p.items.find((i) => i.package);
    if (!withPack) return;

    await page.fill('input[type="text"]', withPack.name);
    await page.waitForSelector(".card", { timeout: 5000 });
    // Switch to list rows — the whole point of D6 is that they carry what cards carry.
    await page.locator('[aria-label="View mode"] button').nth(1).click();
    await page.waitForSelector('[data-testid="row-pack"]', { timeout: 5000 });
    await page.locator('[data-testid="row-pack"]').first().click();
    await page.waitForSelector('[data-testid="pack-stats"]', { timeout: 5000 });
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="pack-stats"]', { state: "detached", timeout: 3000 });
    await page.locator('[aria-label="View mode"] button').first().click();
    await page.waitForSelector(".card", { timeout: 5000 });
    await page.fill('input[type="text"]', "");
  });

  // A chip that looks clickable and does nothing is worse than no chip. It must reach the ⚑ filter
  // — not the search box — so it composes with what is already narrowed and is removable from the
  // active-filter bar like every other axis (D2).
  await check("a trait chip filters by that trait, and clicking it again clears it", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    // Chips are only drawn for an *identified* asset — an unidentified card shows "???" in the
    // tag row instead (Card.svelte `unidentified`). Tags alone are not enough: promotion also
    // wants an embedding (DATA-MODEL §8), and on a machine where ONNX will not load a tagged
    // asset stays `indexed` forever. Picking on traits alone made this check depend on which
    // asset happened to sort first.
    const tagged = p.items.find(
      (i) => i.traits.length > 0 && (i.status === "identified" || i.status === "confirmed"),
    );
    if (!tagged) return; // nothing identified yet — no chips exist to click

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('input[type="text"]', tagged.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"] [data-testid="card-chip"]`),
      tagged.asset_id,
      { timeout: 5000 },
    );
    const chip = page
      .locator(`.card[data-asset-id="${tagged.asset_id}"] [data-testid="card-chip"]`)
      .first();
    const label = (await chip.textContent())?.trim();
    await chip.click();

    await page.waitForSelector('[data-testid="filter-bar"]', { timeout: 5000 });
    const bar = (await page.locator('[data-testid="filter-bar"]').textContent()) ?? "";
    assert.ok(bar.includes(label), `the active-filter bar does not carry "${label}": ${bar}`);
    assert.equal(
      await page.locator('[data-testid="quicklook"]').count(), 0,
      "the chip click fell through and opened Quick Look",
    );

    // Same click undoes itself — a chip can never be added twice.
    await page.fill('input[type="text"]', tagged.name);
    await page.waitForTimeout(300);
    const again = page
      .locator(`.card[data-asset-id="${tagged.asset_id}"] [data-testid="card-chip"]`)
      .first();
    if ((await again.count()) > 0) {
      await again.click();
      await page.waitForTimeout(300);
      const after = (await page.locator('[data-testid="filter-bar"]').textContent().catch(() => "")) ?? "";
      assert.ok(!after.includes(label), `"${label}" survived a second click: ${after}`);
    }
    await page.fill('input[type="text"]', "");
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // Export to folder (UI-SPEC §16.2) — the level-1 way out of the library, and until the Unity
  // plugin exists the only one. Run for real against the live catalog: the whole point of this
  // feature is that a file appears on disk, and nothing short of looking proves that.
  //
  // Everything it writes goes into a fresh temp directory it made itself, and it removes only
  // that. It never touches a library file — the originals are read, and the sandbox check below
  // is the reason it cannot write into one either.
  await check("Export to folder copies a real file, and refuses the library itself", async () => {
    const s0 = await invoke("library_status");
    const gone = new Set(s0.sources.filter((x) => x.state === "unavailable").map((x) => x.id));
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const loose = p.items.find((i) => !i.in_pack && !gone.has(i.source_id));
    if (!loose) return; // nothing on disk to copy — the check would prove nothing

    const dest = path.join(os.tmpdir(), `lodestash-e2e-export-${Date.now()}`);
    try {
      const r = await invoke("export_assets", {
        assetIds: [loose.asset_id],
        destPath: dest,
        withDependencies: false,
      });
      assert.ok(r.written >= 1, `nothing was written: ${JSON.stringify(r)}`);
      const landed = fs.readdirSync(dest);
      assert.ok(landed.length >= 1, "the report claimed a write but the folder is empty");
      // The original is a copy source, never a move source (standing invariant).
      const detail = await invoke("asset_detail", { assetId: loose.asset_id });
      assert.ok(fs.existsSync(detail.abs_path), "the export moved the original instead of copying it");
    } finally {
      fs.rmSync(dest, { recursive: true, force: true });
    }

    // The sandbox outlives the approval gate: a destination inside a library source root is
    // refused for the user exactly as it is for an agent, because that rule protects the
    // non-destructive invariant rather than the agent boundary.
    const root = s0.sources[0]?.root_path;
    if (!root) return;
    const inside = `${root}/lodestash-e2e-should-never-exist`;
    await assert.rejects(
      () => invoke("export_assets", { assetIds: [loose.asset_id], destPath: inside, withDependencies: false }),
      "exporting into a library source root was allowed",
    );
    assert.ok(!fs.existsSync(inside), "a refused export still created its destination");
  });

  // FEATURES §7.1/§7.6 — an export leaves a history entry, and that entry can undo it.
  //
  // Every file this touches is one the check itself exported, into a temp folder it made. It never
  // deletes anything of the user's: the originals are only ever read, and the undo path is fenced
  // to the names the run recorded under the folder it recorded.
  await check("an export is recorded, listed, and can be undone", async () => {
    const s0 = await invoke("library_status");
    const gone = new Set(s0.sources.filter((x) => x.state === "unavailable").map((x) => x.id));
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const loose = p.items.find((i) => !i.in_pack && !gone.has(i.source_id));
    if (!loose) return;

    const dest = path.join(os.tmpdir(), `lodestash-e2e-history-${Date.now()}`);
    let batchId = null;
    try {
      const r = await invoke("export_assets", {
        assetIds: [loose.asset_id],
        destPath: dest,
        withDependencies: false,
      });
      assert.ok(r.written >= 1, `nothing was written: ${JSON.stringify(r)}`);
      const before = fs.readdirSync(dest);
      assert.ok(before.length >= 1, "the export wrote nothing to undo");

      // The run is in the history, counted by files written rather than assets picked.
      const runs = await invoke("recent_folder_exports", { limit: 10 });
      const mine = runs.find((x) => x.dest_path === dest);
      assert.ok(mine, `the export is not in the history: ${JSON.stringify(runs.slice(0, 3))}`);
      assert.equal(mine.files, r.written, "the history disagrees with what was written");
      batchId = mine.batch_id;

      const undone = await invoke("undo_export_batch", { batchId });
      assert.equal(undone.changed, r.written, `undo reported ${undone.changed} of ${r.written}`);
      assert.equal(undone.failed, 0, "undo failed on a file it wrote itself");
      assert.equal(
        fs.existsSync(dest) ? fs.readdirSync(dest).length : 0, 0,
        "the exported copies survived their own undo",
      );

      // The original is untouched — undo deletes copies, never sources.
      const detail = await invoke("asset_detail", { assetId: loose.asset_id });
      assert.ok(fs.existsSync(detail.abs_path), "undoing an export deleted the original");

      // And the run is gone from the history, so it cannot be undone twice.
      const after = await invoke("recent_folder_exports", { limit: 10 });
      assert.ok(!after.some((x) => x.batch_id === batchId), "a finished undo left its record behind");
      batchId = null;
    } finally {
      // If anything above failed mid-way, clean up rather than leaving a record pointing at temp.
      if (batchId) await invoke("undo_export_batch", { batchId }).catch(() => {});
      fs.rmSync(dest, { recursive: true, force: true });
    }
  });

  // 72% of a real Unity library is still inside a .unitypackage. An export that skipped those
  // silently would look like it worked and quietly deliver nothing.
  await check("an in-pack pick is reported as in-pack, not silently dropped", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.find((i) => i.in_pack);
    if (!packed) return;

    const dest = path.join(os.tmpdir(), `lodestash-e2e-inpack-${Date.now()}`);
    try {
      const r = await invoke("export_assets", {
        assetIds: [packed.asset_id],
        destPath: dest,
        withDependencies: false,
      });
      assert.equal(r.written, 0, "an in-pack asset was copied — extraction must stay explicit");
      assert.equal(r.in_pack, 1, `the skip was not attributed to the pack: ${JSON.stringify(r)}`);
      assert.equal(r.unreachable, 0, "an in-pack asset was miscounted as unreachable");
    } finally {
      fs.rmSync(dest, { recursive: true, force: true });
    }
  });

  await check("Export to folder is offered where the picks are", async () => {
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
    // Context menu. Two things this must prove beyond "the item is there", because both were
    // broken and both are invisible: the menu has to *render at all* on a right-click (it was
    // nested inside Quick Look's `{#if}`, so it only existed while the modal that hides the grid
    // was open), and Escape has to close it (its own key handler sat on a never-focused div, so
    // the only way out was a click — and its full-screen backdrop eats every click until then).
    await page.locator(".card").first().click({ button: "right" });
    await page.waitForSelector('[data-testid="ctx-export"]', { timeout: 3000 });
    await page.keyboard.press("Escape");
    await page.waitForSelector(".ctx-menu", { state: "detached", timeout: 3000 });
    assert.equal(
      await page.locator(".menu-backdrop").count(), 0,
      "the backdrop outlived the menu — every click on the app behind it is swallowed",
    );
    // Inspector, on the selected asset.
    await page.locator(".card").first().click();
    await page.waitForSelector('[data-testid="action-export"]', { timeout: 5000 });
    // Staging tray, once something is staged.
    await page.locator(".card").first().locator('[data-testid="card-stage"]').click();
    await page.waitForSelector('[data-testid="tray-export"]', { timeout: 3000 });
    await page.locator('[data-testid="staged-clear"]').click();
    await page.waitForSelector('[data-testid="staging-tray"]', { state: "detached", timeout: 3000 });
  });

  // D3b — the appraisal queue follows the user (UI-SPEC §3). The ordering itself is proved in
  // Rust (`appraise_focus_orders_every_queue_without_narrowing_it`, `focus_order_cuts_the_page…`);
  // what only a live app can prove is that the query the grid sends is a shape the command
  // accepts. `links` is top-level on GridQuery while the frontend keeps it inside `filter`, so a
  // browse call that silently 400s would leave the queue in its default order forever — with
  // nothing on screen to show for it.
  // DESIGN-GUIDELINES §5 names four icon rungs and the shell shipped nine sizes, none of which
  // were three of the four. Docs cannot police that on their own — `check-docs` reads prose, not
  // rendered pixels — so the ladder is asserted where it is actually visible.
  await check("every icon on screen sits on one of the four sizes the guidelines name", async () => {
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
    const sizes = await page.$$eval("svg.icon", (els) =>
      els
        .filter((e) => e.getBoundingClientRect().width > 0) // a hidden icon measures 0, not a rung
        .map((e) => Math.round(e.getBoundingClientRect().width)),
    );
    assert.ok(sizes.length > 0, "no icons rendered — the check would pass on an empty page");
    const ladder = new Set([16, 20, 24, 32]);
    const strays = [...new Set(sizes.filter((w) => !ladder.has(w)))].sort((a, b) => a - b);
    assert.deepEqual(strays, [], `off-ladder icon width(s): ${strays.join(", ")}`);
    // The nav rung is the one the three specs disagreed about, so name it outright.
    // `nav[aria-label="Main"]` — the sidebar. A bare `nav` matches TopBar's category tabs first,
    // which sit on the 20px toolbar rung and would have made this assert the wrong element.
    const nav = await page
      .locator('nav[aria-label="Main"] svg.icon')
      .first()
      .evaluate((e) => Math.round(e.getBoundingClientRect().width))
      .catch(() => null);
    if (nav !== null) assert.equal(nav, 24, "the nav glyph is not the 24px DESIGN-GUIDELINES §5 rung");
  });

  // `ort` is `load-dynamic`: it dlopens `onnxruntime.dll` from `ORT_DYLIB_PATH`, panics when it
  // cannot, and every loader turns that panic into "ONNX Runtime unavailable or incompatible" —
  // the same sentence for a missing runtime and an unset variable. `apply_ort_dylib_env()` lost
  // its only caller in the shell port and nothing noticed for five weeks: the model files were
  // installed, the DLL was installed, and `mode=ai` quietly answered `ai_unavailable`. The grid
  // cannot catch this (its AI mode degrades to FTS on purpose), so the check goes through the
  // engine plane, which is the honest one.
  //
  // Budget: the first `mode=ai` query of a launch loads the SigLIP text encoder through ONNX
  // Runtime and builds the resident embedding snapshot — cold work, ~4.5 s on this library.
  await check("with the models installed, AI search is actually live", { budget: 20_000 }, async () => {
    const home = process.env.APPDATA;
    const models = home && path.join(home, "Lodestash", "models", "siglip");
    const installed =
      models &&
      ["vision.onnx", "text.onnx", "tokenizer.json"].every((f) => fs.existsSync(path.join(models, f)));
    if (!installed) return; // no model on this machine — nothing to assert

    const hs = path.join(process.env.LOCALAPPDATA, "Lodestash", "engine.json");
    const h = JSON.parse(fs.readFileSync(hs, "utf8"));
    const res = await fetch(`http://127.0.0.1:${h.port}/v1/search?mode=ai&q=a%20mossy%20stone%20wall`, {
      headers: { authorization: `Bearer ${h.token}` },
    });
    const body = await res.json();
    assert.notEqual(
      body?.error?.code, "ai_unavailable",
      "the models are installed but no embedder loaded — ORT_DYLIB_PATH is probably unset",
    );
    assert.equal(res.status, 200, `AI search answered ${res.status}: ${JSON.stringify(body).slice(0, 200)}`);
    assert.ok(Array.isArray(body.results) && body.results.length > 0, "AI search matched nothing");
  });

  await check("the grid's own query is a shape set_appraise_focus accepts", async () => {
    const query = {
      tab: "all", search: "", search_mode: "tags", ids: [], status: "all",
      sort_by: "name", sort_asc: true, favorites_only: false, trashed_only: false,
      filter: { traits: [], tri_buckets: [], size_tiers: [] }, links: null,
      package: null, collection: null, source: null, page: 0, page_size: 50,
    };
    // Both commands must take it: they are the same query, and the whole design rests on that.
    const p = await invoke("grid_page", { query });
    assert.ok(p.items.length > 0, "the probe query matched nothing — it proves nothing");
    await invoke("set_appraise_focus", { query });
    // It is a hint, not a write: calling it twice, and on a page past the end, must not error.
    await invoke("set_appraise_focus", { query: { ...query, page: 9999 } });
  });

  await check("the Identify screen states the queue order it now uses", async () => {
    // Scoped to the rail: every card carries an "Identify <name>" action, so a bare role+name
    // lookup matches 200 of them.
    await page.locator('nav[aria-label="Main"] .row').filter({ hasText: "Identify" }).click();
    await page.waitForSelector('[data-testid="queue-order"]', { timeout: 5000 });
    const tiers = await page.locator('[data-testid="queue-order"] li').allTextContents();
    assert.equal(tiers.length, 3, `the queue order lists ${tiers.length} tiers, not 3`);
    assert.ok(tiers[0].includes("on screen"), `tier 1 reads: ${tiers[0]}`);
    assert.ok(tiers[1].includes("current filter"), `tier 2 reads: ${tiers[1]}`);
    assert.ok(tiers[2].includes("newest first"), `tier 3 reads: ${tiers[2]}`);
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // D7b — source availability, the rest (UI-SPEC §11.1). These run against whatever the real
  // catalog holds: if every source is reachable there is nothing to prove and they skip, which is
  // honest. On this machine source 4's root is a scratchpad folder from an old session, so the
  // whole path — plate, inert ⟡, Quick Look panel, relink count — is exercised for real.
  await check("an asset on an unavailable source draws a plate, not a broken image", async () => {
    const s = await invoke("library_status");
    const gone = s.sources.find((x) => x.state === "unavailable");
    if (!gone) return; // every drive is plugged in — nothing to say
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const item = p.items.find((i) => i.source_id === gone.id);
    assert.ok(
      item,
      `source ${gone.name} is unavailable but owns no listed asset — the index did not survive the drive`,
    );

    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.fill('input[type="text"]', item.name);
    await page.waitForFunction(
      (id) => !!document.querySelector(`.card[data-asset-id="${id}"]`),
      item.asset_id,
      { timeout: 5000 },
    );
    const card = page.locator(`.card[data-asset-id="${item.asset_id}"]`);
    assert.equal(
      await card.locator('[data-testid="card-unavailable"]').count(), 1,
      "no `file unavailable` plate on a card whose source is gone",
    );
    // Inert, but still there and still focusable — a control that vanishes teaches nothing.
    const stage = card.locator('[data-testid="card-stage"]');
    assert.equal(await stage.count(), 1, "the stage button disappeared instead of going inert");
    assert.equal(await stage.getAttribute("aria-disabled"), "true", "the stage action is live on an unreachable file");
    assert.equal(await stage.getAttribute("tabindex"), "0", "the inert stage action dropped out of the tab order");
    assert.ok(
      (await stage.getAttribute("title"))?.includes("unavailable"),
      "the inert stage action does not explain itself",
    );
    // Everything the catalog knows still reads — the whole point of having an index.
    const name = (await card.locator(".name").textContent())?.trim();
    assert.equal(name, item.name, `the card lost its name: "${name}"`);
  });

  await check("Quick Look says the file is not reachable, and offers the path and a relink", async () => {
    const s = await invoke("library_status");
    const gone = s.sources.find((x) => x.state === "unavailable");
    if (!gone) return;
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const item = p.items.find((i) => i.source_id === gone.id);
    if (!item) return;

    await page.locator(`.card[data-asset-id="${item.asset_id}"]`).first().dblclick();
    await page.waitForSelector('[data-testid="quicklook"]', { timeout: 5000 });
    await page.waitForSelector('[data-testid="ql-unavailable"]', { timeout: 5000 });
    const panel = (await page.locator('[data-testid="ql-unavailable"]').textContent()) ?? "";
    assert.ok(panel.includes("reachable"), `the panel reads: ${panel}`);
    assert.ok(panel.includes(gone.name), "the panel does not name the library that went away");
    // The declared path is what the user needs to recognise the drive; it must be the *source's*
    // root, not the asset's, because that is what Relink changes.
    const declared = await page.locator('[data-testid="ql-unavailable"] .declared').textContent();
    assert.equal(declared?.trim(), gone.root_path, "the panel shows something other than the source's declared root");
    assert.equal(await page.locator('[data-testid="ql-relink"]').count(), 1, "no Relink action");
    assert.equal(await page.locator('[data-testid="ql-copy-path"]').count(), 1, "no Copy path action");
    // The full-size action stays live precisely so this panel is reachable; no viewer may be
    // spinning behind it waiting for bytes that are not coming.
    assert.equal(
      await page.locator('[data-testid="ql-stage"]').getAttribute("data-kind"), "unreachable",
      "Quick Look tried to decode a file that is not there",
    );
    await page.keyboard.press("Escape");
    await page.waitForSelector('[data-testid="quicklook"]', { state: "detached", timeout: 5000 });
    await page.fill('input[type="text"]', "");
  });

  // The number the relink panel puts in front of the decision. Read-only on purpose: committing a
  // relink would rewrite the user's own catalog, and the write half is mutation-checked in Rust
  // (`relink_counts_before_it_commits_and_rewrites_only_paths`).
  await check("relink_preview counts what a candidate folder actually holds", async () => {
    const s = await invoke("library_status");
    const gone = s.sources.find((x) => x.state === "unavailable");
    if (!gone) return;
    const empty = await invoke("relink_preview", { sourceId: gone.id, newRoot: process.cwd() });
    assert.ok(empty.total > 0, "a source with listed assets previewed a total of 0");
    assert.equal(empty.matched, 0, `an unrelated folder matched ${empty.matched} of ${empty.total} files`);
    // A folder that is not one must be refused rather than counted as a total miss — "0 of 4,206
    // matched" for a mistyped path reads as a real answer.
    await assert.rejects(
      () => invoke("relink_preview", { sourceId: gone.id, newRoot: `${process.cwd()}/no-such-folder-here` }),
      "a nonexistent folder previewed as a legitimate zero",
    );
  });

  await check("an unavailable source offers Relink, and the panel names the declared root", async () => {
    const s = await invoke("library_status");
    const gone = s.sources.find((x) => x.state === "unavailable");
    if (!gone) return;
    await page.getByRole("button", { name: "Settings" }).click();
    await page.waitForSelector('[data-testid="sources"]', { timeout: 5000 });
    const toggle = page.locator(`[data-testid="relink-toggle-${gone.id}"]`);
    assert.equal(await toggle.count(), 1, "an unavailable source offers no way to relink it");
    await toggle.click();
    await page.waitForSelector(`[data-testid="relink-${gone.id}"]`, { timeout: 3000 });
    const declared = await page.locator(`[data-testid="relink-${gone.id}"] .declared`).textContent();
    assert.equal(declared?.trim(), gone.root_path, "the panel shows the wrong declared root");
    // Nothing commits until a folder is chosen and counted.
    assert.equal(
      await page.locator(`[data-testid="relink-commit-${gone.id}"]`).count(), 0,
      "the relink button is live before anything has been counted",
    );
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // B2b: the Unity Assets batch bar. Ticking a pack must surface "▶ Extract selected (N)" —
  // the queue the job tray shows while packs unpack one at a time (UI-SPEC §5).
  await check("ticking a pack arms the batch extract bar", async () => {
    await page.getByRole("button", { name: "Unity Assets" }).click();
    await page.waitForSelector('[data-testid="unity-packs"]', { timeout: 5000 });
    const packs = await page.locator(".pack").count();
    if (packs === 0) return; // no packs in this catalog — nothing to arm
    await page.locator('[data-testid^="tick-"]').first().click();
    await page.waitForSelector('[data-testid="extract-batch"]', { timeout: 3000 });
    const label = await page.locator('[data-testid="extract-selected"]').textContent();
    assert.ok(label?.includes("Extract selected (1)"), `batch bar says: ${label}`);
    await page.locator('[data-testid^="tick-"]').first().click(); // untick — no side effects
    await page.waitForFunction(
      () => document.querySelector('[data-testid="extract-batch"]') === null,
      { timeout: 3000 },
    );
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForSelector(".card", { timeout: 5000 });
  });

  // ADR-0016 (S02): another writer's change raises "Library updated · Refresh" instead of
  // re-reading the grid under the user; a refresh clears only what it had seen when it started;
  // and the user's own write is not announced back to them. Every write here is real — two agent
  // stages through the engine API and one user stage through the command path — and all three
  // are undone at the end.
  //
  // It is also the check that the shell hears `ls:changed` at all. Until 2026-09-10 it never did:
  // no capability granted `core:event:allow-listen`, so every subscription was rejected, silently,
  // and nothing in the suite noticed because nothing depended on an event arriving.
  await check("another writer's change asks for a refresh instead of moving the grid", async () => {
    // Count the reads a change event used to trigger, and let this check hold a refresh open.
    await page.evaluate(() => {
      // Tauri defines both `window.__TAURI_INTERNALS__` and its `invoke` non-writable and
      // non-configurable, so wrapping either is a silent no-op — which is how this check first
      // passed its "no re-read" assertion while counting nothing. The IPC itself is a `fetch` to
      // `http://ipc.localhost/<command>`, and `fetch` is an ordinary writable global: count there,
      // and hold `reload_library` there.
      const realFetch = window.fetch.bind(window);
      window.__lsCalls = {};
      window.__lsHold = null;
      window.fetch = async (input, init) => {
        const url = String(input && input.url ? input.url : input);
        const m = url.match(/^http:\/\/ipc\.localhost\/([^?#]+)/);
        if (m) {
          const cmd = decodeURIComponent(m[1]);
          window.__lsCalls[cmd] = (window.__lsCalls[cmd] ?? 0) + 1;
          if (cmd === "reload_library" && window.__lsHold) await window.__lsHold;
        }
        return realFetch(input, init);
      };
    });
    const reads = () =>
      page.evaluate(() => ({
        grid_page: window.__lsCalls.grid_page ?? 0,
        library_status: window.__lsCalls.library_status ?? 0,
      }));
    const banner = page.locator('[data-testid="library-updated"]');
    const h = JSON.parse(fs.readFileSync(HANDSHAKE, "utf8"));
    const agentStage = async (asset_id) => {
      const res = await fetch(`http://127.0.0.1:${h.port}/v1/stage`, {
        method: "POST",
        headers: { authorization: `Bearer ${h.token}`, "content-type": "application/json" },
        body: JSON.stringify({ asset_id }),
      });
      assert.ok(res.ok, `stage through the engine API: ${res.status} ${await res.text()}`);
    };

    const before = await invoke("staged");
    assert.ok(
      await page.evaluate(() => (window.__lsCalls.staged ?? 0) >= 1),
      "the call counter is not intercepting — every read assertion below would pass vacuously",
    );
    const ids = (
      await page.locator(".card").evaluateAll((els) => els.map((e) => Number(e.dataset.assetId)))
    ).filter((i) => !before.includes(i));
    assert.ok(ids.length >= 3, "the page needs three unstaged cards");
    const [a, b, mine] = ids;
    try {
      await agentStage(a);
      await banner.waitFor({ state: "visible", timeout: 3000 });
      assert.deepEqual(
        await reads(),
        { grid_page: 0, library_status: 0 },
        "an agent's write re-read the page under the user",
      );
      assert.equal(
        await page.locator('[role="status"]:has([data-testid="library-updated"])').count(),
        1,
        "the indication is outside a live region, so a screen reader never hears it",
      );

      // A change that lands while the refresh is still reading stays pending.
      await page.evaluate(() => {
        window.__lsHold = new Promise((r) => (window.__lsRelease = r));
      });
      await banner.click();
      await agentStage(b);
      await page.waitForTimeout(800); // past one publish window: b's event is in while held
      await page.evaluate(() => {
        window.__lsHold = null;
        window.__lsRelease();
      });
      await waitFor(async () => (await reads()).grid_page >= 1, "the refresh never re-read the page");
      await waitFor(
        async () => (await banner.count()) === 1 && (await banner.isEnabled()),
        "a change that landed mid-refresh was cleared along with it",
      );

      // A second refresh clears it, and the user's own write is not announced back to them.
      const beforeSecond = (await reads()).grid_page;
      await banner.click();
      await banner.waitFor({ state: "detached", timeout: 3000 });
      // The refresh's own page read is asynchronous and can land after the banner goes. Wait for
      // it, then for the counters to go quiet, before taking the baseline the user's write is
      // held to — otherwise that late read is blamed on the user's write.
      await waitFor(
        async () => (await reads()).grid_page > beforeSecond,
        "the second refresh never re-read the page",
      );
      let settled = await reads();
      for (let i = 0; i < 10; i++) {
        await page.waitForTimeout(300);
        const now = await reads();
        if (JSON.stringify(now) === JSON.stringify(settled)) break;
        settled = now;
      }
      await invoke("stage", { ids: [mine] });
      await page.waitForTimeout(800);
      assert.equal(await banner.count(), 0, "the user's own write was announced as someone else's");
      assert.deepEqual(await reads(), settled, "the user's own write was re-read a second time");
    } finally {
      await invoke("unstage", { ids: [a, b, mine] }).catch(() => {});
    }
  });

} finally {
  await app.close();
}

// Runs **after** the suite's app is closed, on purpose: it needs its own app instance (the
// handshake file is one per machine, so two running apps fight over it) and it must run without
// `LODESTASH_NO_PREVIEW_DRAIN`, which the harness sets for every other check.

// The flag the rest of the suite sets is precisely what hid the bug this guards: the idle preview
// backfill checked its time budget once per batch of 16 renders, so a "1 second" slice held the
// single writer for ~17 s and every user write queued behind it. Measured on the real app, a
// `POST /v1/stage` took 16.4 s against 2 ms before the drain started. No unit test can see this —
// it is a latency property of a running writer.
async function writerLatencyCheck() {
  // The suite's browser is closed by now, so disarm the per-check viewport guard — it
  // measures the page, and measuring a closed page is what failed this check the first
  // time it ran here.
  baseline = null;
  // Budget: launches a second app and waits for it to publish engine.json (up to 60 s on a cold
  // start), then deliberately idles 3 s so the preview drain is running before it measures.
  await check("a user write stays fast while the preview backfill runs", { budget: 75_000 }, async () => {
    const { spawn } = await import("node:child_process");
    try { fs.rmSync(HANDSHAKE); } catch { /* not running */ }
    const app = spawn(BIN, { stdio: "ignore" }); // no drain flag — that is the point
    try {
      const t0 = Date.now();
      while (!fs.existsSync(HANDSHAKE) && Date.now() - t0 < 60000) {
        await new Promise((r) => setTimeout(r, 250));
      }
      assert.ok(fs.existsSync(HANDSHAKE), "the app never published engine.json");
      const h = JSON.parse(fs.readFileSync(HANDSHAKE, "utf8"));
      const stage = async () => {
        const started = Date.now();
        const res = await fetch(`http://127.0.0.1:${h.port}/v1/stage`, {
          method: "POST",
          headers: { authorization: `Bearer ${h.token}`, "content-type": "application/json" },
          body: JSON.stringify({ asset_id: 1 }),
        });
        await res.text();
        return Date.now() - started;
      };
      await stage(); // the first write lands before the drain has started — it proves nothing
      await new Promise((r) => setTimeout(r, 3000)); // let the idle drain get going
      const worst = Math.max(await stage(), await stage(), await stage());
      assert.ok(
        worst < 4000,
        `a stage write took ${worst} ms with the preview drain running — the idle slice is not ` +
          `honouring its budget per render`,
      );
    } finally {
      app.kill();
    }
  });
}

await writerLatencyCheck();

const failed = results.filter((r) => !r.ok);
console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
console.log(`report: ${path.relative(REPO_ROOT, writeReport())}`);
process.exit(failed.length === 0 ? 0 : 1);
