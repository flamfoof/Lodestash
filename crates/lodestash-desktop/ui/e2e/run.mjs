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
  await check("Quick Look on an in-pack asset explains itself", async () => {
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
    assert.equal(kind, "not_local", "an in-pack asset must render the not_local explanation");
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
  await check("in-pack image thumbnails are pulled, or refused over the size cap", async () => {
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
        assert.match(r.body, /not_local/);
      }
      seen.push(r.status);
    }
    console.log(`       ${seen.filter((s) => s === 200).length}/${seen.length} pulled, rest over cap`);
    assert.ok(seen.includes(200), "every sampled pack was over the cap — no pull was exercised");
  });

  // The pull must not resurrect the model stampede: a model is refused before any gzip scan,
  // in-pack or not. If this ever answers 200, a grid page of in-pack models re-inflates the
  // archive once per card *and* parses a mesh each time.
  await check("an in-pack model is refused before any pull", async () => {
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
  await check("an in-pack asset answers 503 not_local, not 404", async () => {
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
    assert.equal(r.status, 503, `expected 503 not_local, got ${r.status}`);
    assert.match(r.body, /not_local/);
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
} finally {
  await app.close();
}

const failed = results.filter(([ok]) => !ok);
console.log(`\n${results.length - failed.length}/${results.length} checks passed`);
process.exit(failed.length === 0 ? 0 : 1);
