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

  // The check that would have caught the `ls://localhost` routing bug on the spot. A broken
  // image is indistinguishable from a styled placeholder in a screenshot, but its naturalWidth
  // is 0 — so the failure that cost a debugging session becomes one assertion.
  await check("every vector preview decodes (naturalWidth > 0)", async () => {
    const imgs = await page
      .locator(".thumb img")
      .evaluateAll((els) => els.map((e) => ({ src: e.getAttribute("src"), w: e.naturalWidth })));
    assert.ok(imgs.length > 0, "no preview images on screen — nothing was exercised");
    const broken = imgs.filter((i) => i.w === 0);
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
    await page.getByRole("button", { name: "Favorites" }).click();
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
      .evaluateAll((els) => els.filter((e) => e.querySelector(".star")).length);
    assert.equal(starred, cards, "every card on Favorites must carry the star");
    await page.getByRole("button", { name: "Library", exact: true }).click();
    await page.waitForTimeout(500);
  });

  await check("ls:// serves a preview as SVG", async () => {
    // Ask for an asset that actually has a preview rather than whatever is on screen — which
    // page 1 shows depends on the sort, and most of this library is in-pack with no preview.
    const p = await invoke("grid_page", {
      query: { tab: "models", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const withPreview = p.items.find((i) => i.has_preview);
    assert.ok(withPreview, "no asset in the library has a stored preview");
    const r = await page.evaluate(async (id) => {
      const res = await fetch(`http://ls.localhost/preview/${id}.svg`);
      return { status: res.status, type: res.headers.get("content-type"), len: (await res.text()).length };
    }, withPreview.asset_id);
    assert.equal(r.status, 200);
    assert.equal(r.type, "image/svg+xml");
    assert.ok(r.len > 100, "preview body is implausibly small");
  });

  await check("ls://file honours Range with a 206 and the right window", async () => {
    // Must be an on-disk asset: most of this library is in-pack, and those correctly answer 503.
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const local = p.items.find((i) => !i.in_pack);
    assert.ok(local, "no on-disk asset in the library to range-request");
    const r = await page.evaluate(async (assetId) => {
      const res = await fetch(`http://ls.localhost/file/${assetId}`, {
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
  await check("an in-pack asset answers 503 not_local, not 404", async () => {
    const p = await invoke("grid_page", {
      query: { tab: "all", search: "", status: "all", sort_by: "name", sort_asc: true,
               favorites_only: false, page: 0, page_size: 100000 },
    });
    const packed = p.items.find((i) => i.in_pack);
    assert.ok(packed, "no in-pack asset loaded");
    const r = await page.evaluate(async (assetId) => {
      const res = await fetch(`http://ls.localhost/file/${assetId}`);
      return { status: res.status, body: await res.text() };
    }, packed.asset_id);
    assert.equal(r.status, 503, `expected 503 not_local, got ${r.status}`);
    assert.match(r.body, /not_local/);
  });

  await check("an unknown ls:// route is a 404, not a hang", async () => {
    const status = await page.evaluate(async () => {
      const res = await fetch("http://ls.localhost/nonsense/1");
      return res.status;
    });
    assert.equal(status, 404);
  });

  await check("an unknown asset id is a 404", async () => {
    const status = await page.evaluate(async () => {
      const res = await fetch("http://ls.localhost/file/999999999");
      return res.status;
    });
    assert.equal(status, 404);
  });
} finally {
  await app.close();
}

const failed = results.filter(([ok]) => !ok);
console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
process.exit(failed.length === 0 ? 0 : 1);
