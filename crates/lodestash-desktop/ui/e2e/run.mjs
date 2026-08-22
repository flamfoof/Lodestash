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
import { launch } from "./harness.mjs";

const results = [];
async function check(name, fn) {
  try {
    await fn();
    results.push([true, name]);
    console.log(`  ok   ${name}`);
  } catch (e) {
    results.push([false, name]);
    console.log(`  FAIL ${name}\n       ${e.message.split("\n")[0]}`);
  }
}

const app = await launch();
const { page, invoke } = app;
console.log(`\ndriving ${page.url()}\n`);

try {
  // First-run onboarding (FEATURES §8) covers the window on a fresh config. Every DOM
  // interaction below needs it gone — and dismissing it is itself the behaviour under test:
  // the modal must never trap the library behind it. A returning user (skip_onboarding set)
  // never sees it, which this check also tolerates.
  await check("onboarding is dismissed, and the library stays reachable", async () => {
    const box = page.locator('[data-testid="onboarding"]');
    if ((await box.count()) > 0) {
      await page.locator('[data-testid="onboarding-go"]').click();
      await box.waitFor({ state: "detached", timeout: 5000 });
    }
    const s = await invoke("library_status");
    assert.equal(s.open, true, "no catalog open — run a scan in the egui build first");
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
  // `Browser.setWindowBounds` resizes the actual window, so media queries follow — unlike
  // `setViewportSize`, which changes nothing here and leaves an emulation override that poisons
  // every later check. Worth the trouble: the first version of this ladder had two of its four
  // stops doing *nothing* at any width, because rules written in the global sheet lose to a
  // component's own scoped ones, and nothing said so.
  //
  // Reachability is `offsetParent !== null`, not computed display: what decides whether a control
  // can be used is whether an ancestor hid it, and the first probe of this got that wrong.
  await check("each rung of the responsive ladder moves the chrome it promises", async () => {
    const cdp = await page.context().newCDPSession(page);
    const { windowId } = await cdp.send("Browser.getWindowForTarget");
    const { bounds: original } = await cdp.send("Browser.getWindowBounds", { windowId });
    const at = async (width) => {
      await cdp.send("Browser.setWindowBounds", {
        windowId,
        bounds: { width, height: 800, windowState: "normal" },
      });
      // The icon rail widens on hover *by design*, and earlier checks leave the pointer wherever
      // they last clicked — often on a nav row. Park it over the grid before measuring, or step 3
      // reads 210px and looks like a broken media query.
      await page.mouse.move(width - 60, 400);
      // Earlier checks leave focus on whatever they last clicked; a nav row still holding focus
      // would expand the rail through `:has(:focus-visible)` and read as a broken media query.
      await page.evaluate(() => (document.activeElement instanceof HTMLElement ? document.activeElement.blur() : null));
      await page.waitForTimeout(500);
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
      await cdp.send("Browser.setWindowBounds", { windowId, bounds: original }).catch(() => {});
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
    const name = `e2e-trait-${Date.now()}`;
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
    // Cleanup: demote so repeated runs do not accumulate vocabulary.
    await invoke("unpromote_trait", { tagId: row.id });
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
} finally {
  await app.close();
}

const failed = results.filter(([ok]) => !ok);
console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
process.exit(failed.length === 0 ? 0 : 1);
