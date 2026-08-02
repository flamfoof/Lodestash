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

  await check("the grid renders a card per returned item", async () => {
    const p = await invoke("grid_page", { query: { tab: "all", search: "", page: 0, page_size: 100 } });
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

  await check("ls:// serves a preview as SVG", async () => {
    const r = await page.evaluate(async (src) => {
      const res = await fetch(src);
      return { status: res.status, type: res.headers.get("content-type"), len: (await res.text()).length };
    }, await page.locator(".thumb img").first().getAttribute("src"));
    assert.equal(r.status, 200);
    assert.equal(r.type, "image/svg+xml");
    assert.ok(r.len > 100, "preview body is implausibly small");
  });

  await check("ls://file honours Range with a 206 and the right window", async () => {
    const p = await invoke("grid_page", { query: { tab: "all", search: "", page: 0, page_size: 50 } });
    const id = p.items[0].asset_id;
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
    }, id);
    assert.equal(r.status, 206, `expected 206, got ${r.status} (503 means the file is in-pack)`);
    assert.equal(r.len, 10, "a 10-byte window must return 10 bytes");
    assert.match(r.range, /^bytes 10-19\/\d+$/);
    assert.equal(r.accept, "bytes");
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
