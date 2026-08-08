# Lodestash — Svelte re-write board (B-series status)

Status board compiled 2026-08-07 from a six-way audit (UI-SPEC / DATA-MODEL / ENGINE-API+MCP /
INGESTION / AI-PIPELINE / KANBAN truth-check) against the live code. The egui crate
(`crates/lodestash-ui`) was deleted at the B7 cutover — the Svelte shell +
`lodestash-desktop`/`lodestash-app` are the app. **Card state = verified in code, not comments.**

Lanes: 🟢 Done · 🟡 Partial (delta named) · 🔴 Missing · ⚪ Built but inert (needs wiring) ·
🟠 Visual conformance & doc drift · 🔵 Engine-project / deferred. The main board
([`KANBAN.md`](KANBAN.md)) holds the full history; the card titles here match the main board's
B-card ids where they exist. Evidence is `file:line` in `crates/lodestash-desktop/`.

---

## 🟢 Done — the Svelte re-write surface

- [x] **Shell & frame.** Top bar (logo, health meter, type tabs), sidebar nav (11 destinations) +
      STATUS/LIBRARIES facets + real `+ Add source…` (native dialog), 12 routes, onboarding explainer,
      DropZone, ConfirmDelete, palette (Ctrl-K), window-state persistence.
- [x] **Library grid + list.** Real catalog (three storage classes), density, sort, bulk bar
      (multi-select + Add/Remove tag), per-card ✦ Identify now, stage/star chips, size tiers, source
      label, link-chain mark; track-pinned cards (e2e-pinned, 229.25 px).
- [x] **Favorites screen.** Second in nav, gold count pill, toolbar toggle.
- [x] **Collections screen.** List/create/delete, members via the grid (staging trays excluded).
- [x] **Identify screen.** Backlog with counts, ✦ Identify all + per-asset kicks (B11a–d), appraising
      states, pump-active polling. *(Tagging itself is 🟡 below — rows stall at `indexed`.)*
- [x] **Duplicates screen (exact).** Hash-based groups, keeper first.
- [x] **Stats overview.** Facet counts, size tiers, Tag Graph ring (tags·links·assets header, empty
      state). *(Interaction is 🟡.)*
- [x] **Traits + Mapping tabs.** Trait CRUD (upsert/promote/teach/style), Mapping rules/categories
      (add/delete/apply).
- [x] **Unity Assets.** Pins → sidebar rows, browse, per-pack Extract/Stats/Open→, batch
      Extract-selected queue + Cancel, **M14 surgical flow wired end-to-end** (B2c): browse
      persistence → manifest worker → mint in-pack rows → surgical `extract_at_offsets` →
      materialize → render from workspace cache.
- [x] **Settings.** Sources + per-source blacklist editor, AI tier ↔ quality, size tiers, watch +
      auto-identify toggles, thumb cache, **Unity Editors (Connect wizard app half)**, danger flow
      (confirm-gated delete-from-disk). *(CLOUD card is 🔴.)*
- [x] **Inspector.** Identity, stats, source, related, stage/import, trash/delete/fav, links.
      *(TRAITS shows no real tags — 🟡.)*
- [x] **Quick Look.** 3D WebGL2 viewer (`viewer3d.ts`, no dep), audio player + waveform, image.
- [x] **Job tray + pump events.** Bottom-left tray (progress bars, per-job ✕ cancel, session ring),
      `ls:changed` events — the UI doesn't poll.
- [x] **File-explorer iconset + tree wiring (B12).** `ls-folder` / `ls-folder-open` (gold) /
      `ls-file` glyphs designed (24×24, stroke 1.8) in `docs/design/icons/` + both sprites (56
      symbols), Glyph Catalog §07, DG §5 table; FoldersView tree rows now use the sprite chevrons
      (`ls-action-expand-open/closed`) + folder glyphs (open = current path), folder list + tiles
      rows use `ls-folder` (PACK-DIRECTORY-NAV §3).
- [x] **Backend live in the shell.** M1–M16 migrations, catalog functions, drains (metrics →
      previews → deps → embeddings → promote), persist/scan, single-writer thread, `lodestash://`
      protocol (`preview`/`file`/`mesh`, Range-capable, in-pack thumbnails via scratch).
- [x] **QA.** e2e 52/52 over the real binary (CDP), svelte-check 0/0, `cargo test` green; name-level
      REST↔MCP parity test (test-only — see ⚪).

## 🟡 Partial — working, delta named

- [x] **Identify: the tagging half (B11 tail, shipped 2026-08-07).** `drain_tags` / `drain_audio_tags`
      had zero call sites — no tagger resolver, rows stalled at `indexed`. Now wired into
      `appraise_pass` (`writer.rs:602`): local-only, zero new models — the already-resolved SigLIP
      embedder drives `ImageZeroShot`, CLAP drives `AudioZeroShot`, taught traits (Traits screen,
      AI-PIPELINE §6) become candidate labels, `FilenameTagger` is the captioning stub. Both drains
      gained a `max_assets` budget guard (AI-PIPELINE §5.1, `0` = uncapped) so a writer-thread kick
      never blocks writes on a long backlog — the remainder stays queued for the next kick / idle
      slice (64 visual / 48 audio per pass). `cargo test --workspace` green.
      - [x] **Ollama/cloud captioning endpoint (2026-08-08).** Settings gained an AI TAGGING
            section (local Ollama text+vision models, opt-in OpenAI-compatible API with
            base URL/model/key — stored in config.json, never logged); `appraise_pass`
            resolves `OpenAiTagger` → `OllamaTagger` → `FilenameTagger`.
- [x] **AI search wired (2026-08-08).** `GridQuery.search_mode` + `Library::ai_ranks`: the
      ⟨Tags⟩/⟨AI⟩ toggle now drives `catalog::hybrid_search` (RRF-fused FTS + SigLIP/CLAP query
      embeddings, resolved once per process); without a local model it honestly degrades to FTS.
- [x] **Identify UI polish (2026-08-08).** Shared `Arc<JobProgress>` on AppState, reset per
      pass + filled by the tag/embed drains; `pump_progress` command; IdentifyScreen renders a
      real done/total bar (+ failed count) while a pass runs. Inspector gained a per-asset
      **✦ Identify** button (B11b) that flags + appraises and bumps the page.
- [x] **Identify session set (2026-08-08).** `promote_statuses` now returns the promoted ids;
      the appraise pass threads them into `WriteResult.promoted`; `GridQuery.ids` restricts a
      page to exactly those ids. After ✦ all, the header flips to "N just appraised" with a
      back-to-backlog link — the set no longer vanishes into the library.
- [x] **Inspector TRAITS live (2026-08-08).** `AssetDetail.tags` (via `tags_for_asset`) renders
      the asset's real tag set with AI confidence % and a ✕ reject (same undoable `bulkTag` op);
      accept/promote-to-trait stays on the Traits screen where the design owns it.
      confidence underbars, or trait quick-add.
- [ ] **Inspector gaps.** No dependency **relink** interaction (edges render, nothing re-resolves a
      `missing` edge); no in-archive provenance block (Path-in-package / Package ⧉-copy rows);
      card ★ is display-only (only the inspector toggles favourites).
- [x] **Duplicates — exact hash wired (2026-08-08).** The crawl now BLAKE3s browseable files
      (`CrawledFile.hash` → `CrawlRecord.content_hash` → `asset.content_hash` on insert) — the
      Duplicates screen's groups are real. `needs_rehash` (size/mtime-gated re-hash for
      watch-rescans) is the documented ponytail; perceptual near-dup still out of scope.
- [ ] **Pack stats modal is a stub.** 3 tiles ("isn't available in this build"); no triangles tile,
      no per-type contents/tags/heaviest. Packages screen rows have **no ◉ Stats** button at all.
- [x] **Keyboard map (2026-08-08).** Ctrl/Cmd-Z undoes the newest op; **Shift-Z / Ctrl/Cmd-Y
      redo** (op log now carries forward payloads, migration 17 — see the redo card); Space
      toggles Quick Look (open on selection, close when open); F favourites the selection.
      Ctrl/Cmd-A (multi-select model) still open.
- [x] **Right-click context menu (2026-08-08, FEATURES §1.1).** Cards + list rows emit
      `oncontextmenu`; LibraryScreen floats a menu at the cursor (clamped, Escape/click-away
      closes): Quick Look, Toggle Favorite, Identify now, Move to Trash (through the
      confirm-before-trash gate). App owns the write fns.
- [x] **Trash renders the ordinary grid + per-row Restore (2026-08-08).** The trash view is the
      ordinary grid/list; the list's rows gained a Restore action in trash mode
      (`AssetList row-restore`), alongside the inspector's Restore.
- [ ] **Onboarding is a static explainer.** No two-step source/scan flow.
- [x] **Tag Graph interactive (2026-08-08).** Drag to pan, wheel to zoom (clamped 0.5–4×), click a
      node to filter the library to that tag and navigate — dependency arcs still belong to the
      LINKS graph (separate card).
- [x] **Settings gaps (2026-08-08).** Clear-cache/usage bar live (STORAGE CAPS — bytes + files,
      thumbs + mesh/inpack scratch only, workspace untouched); confirm-before-trash toggle
      (two-step: first click arms a 3s confirm toast, second trashes — inspector + Duplicates
      paths). CLOUD card (the AI TAGGING section covers the API endpoint) + the engine
      port/token surface shipped earlier; model-download button still open.
- [ ] **Staging is in-memory only.** Tray rows exist; `batch_id` recorded but no Recent-exports list
      and no batch undo in the UI. Top-bar "No project" chip is static — **no project switcher
      popover** (recent exports · ↩ undo · Connect wizard, FEATURES §7.1).
- [x] **In-pack Quick Look pulls the one payload (2026-08-08, FEATURES §5.4).** `lodestash://file`
      now materializes a known in-pack asset through the size-capped scratch (same path the thumb
      route uses) instead of answering `503 not_local`; whole-pack extraction stays a command.
- [ ] **Trait create form is under-specified** vs FEATURES §4.6 — no scope picker (any/model/sound/
      image), no usage-description field, defaults `teach:false`; no usage-note editing.
- [ ] **Unity Assets landing lacks `↻ Sync` + content-derived filter tabs** (§5.3) — only a text box
      + pinned toggle. Pack-browser tree has **no user-collapsible state** (expansion derives from
      the current path) and no responsive fallback below ~900px (PACK-DIRECTORY-NAV §2/§5/§9).

## 🔴 Missing — not built at all

- [x] **Perceptual near-dup — image pHash (2026-08-08).** `metrics::perceptual_hash` = 64-bit
      dHash (downscale 9×8 grayscale, row-difference), stored by the metrics drain on
      `asset_file.perceptual_hash`; `near_duplicate_groups` union-finds members within 6 bits
      (bounded to the first 4k hashed rows) and the Duplicates screen gained a **Similar — same
      picture, different bytes** tier with its own verify-before-trash hint. Audio Chromaprint
      still deferred (needs a native dep).
      implementation.
- [ ] **Variants + color search.** `variant_group_id` / `dominant_color` DDL-only; no clustering, no
      grid collapse, no color facet.
- [ ] **Compiled bundles + UnityPy sidecar.** No implementation; detect is extension-only.
- [ ] **Format normalization table.** No glTF artifact, no texture→PNG (KTX/ASTC/ETC…), no FSB5→WAV,
      no Aseprite/KRA/.blend/font specimens (INGESTION §4).
- [ ] **Render-pipeline harvest.** `package.pipeline` never written ⇒ `pipeline_mismatch` advisory
      (ENGINE-API §5.4) has no data.
- [ ] **License/attribution harvest.** Columns exist, no setter; `.meta` `labels:` parsed but never
      seeded into tags.
- [ ] **Smart collections / `embedding_meta`.** DDL-only, never written.
- [ ] **Trait-vocabulary teaching.** `teach=1` machinery exists; nothing feeds it to a tagger.
- [x] **Full-res in-pack pulls (2026-08-08).** `lodestash://file` and `lodestash://mesh` now
      materialize a known in-pack asset through the size-capped scratch instead of answering
      `503 not_local` — Quick Look renders in-pack images/audio/models like loose ones (whole
      packs still extract by command only). Remaining: per-entry "Extract selected"
      page-batching (lift the 256 MB `IN_PACK_THUMB_MAX_BYTES` cap).
- [ ] **Import dialog (§5.2).** Folder → discovered-package list (group/select-all, category badge +
      tabs, version disclosure, "import as libraries") — `add_source_dialog` is a plain folder
      picker today.
- [ ] **`⇢ Add to project` / `⇢ Add all N to project` affordances.** Absent everywhere (FEATURES
      §5.4/§5.5, PACK-DIRECTORY-NAV §7) — the engine write path doesn't exist in the shell.

## ⚪ Built but inert — wire into the shell

- [x] **REST + MCP servers start with the app (2026-08-08, B12).** One `ServerState` →
      `api::serve` (ephemeral loopback port + engine.json written) + `mcp::serve_stdio` on its
      own thread (self-terminates on stdin EOF); **`--mcp` headless branch shipped 2026-08-08**
      — `lodestash-desktop --mcp` serves the stdio plane without a window (smoke-tested with a
      real initialize handshake).
      agent host pipes stdin to the running app). Settings' ENGINE BRIDGE shows the live port.
- [ ] **10 doc'd agent ops absent on both planes.** `list_tags`, `add_tag`, `list_collections`,
      `apply_mapping`, `identify_asset`, `suggest_tags`, `unpack_package`, staging group
      (ENGINE-API §2).
- [x] **Hash/dedup wired for loose files (2026-08-08, B6).** `crawler::hash_file` called at
      crawl for browseable files ⇒ `content_hash` set ⇒ Duplicates groups real; **move/rename
      detection shipped 2026-08-08** (`reconcile_moves`: vanished path + same hash ⇒ rel_path
      updated in place, asset_id/tags/embeddings preserved, trashed exempt — before record_crawl
      so no duplicate insert); **incremental re-hash** (crawl skips BLAKE3 over files known at the
      same size/mtime via `needs_rehash` + `Library::known_sizes`). **Perceptual near-dup shipped
      2026-08-08**: `metrics::perceptual_hash` (64-bit dHash over the shared decode seam) written
      by the metrics drain (`set_perceptual_hash`), `near_duplicate_groups` (union-find, Hamming
      ≤ 6 bits, capped pairwise) + the Duplicates screen's **Similar** tier.
- [x] **Magic-byte detection (2026-08-08).** The crawler falls back to `detect::by_magic` (16-byte
      header read per *unclassified* file) when the extension misses, so unknown-ext real assets
      are recovered. `is_content` for the in-pack importer path is still unwired (the loose path
      subsumes it).
- [x] **Filesystem watch (2026-08-08, DATA-MODEL §6).** `ingest::watch::watch_roots` (notify,
      debounced) now arms in the writer's idle pass over the `watch_enabled` source roots;
      a signal rescans every watched source with the **incremental crawl** (unchanged files skip
      BLAKE3) + the standard persist, reloading the UI only when something actually changed;
      the watch re-arms after source-set changes. Skipped under the e2e escape hatch.
- [ ] **Eviction / keep-extracted.** `unmaterialize_package` zero callers; `cache_extracted` not
      honored; workspace cap/7-day retention not enforced.
- [ ] **Non-content classification.** `detect::is_content` zero callers; `is_content`/`content_kind`
      never set.
- [ ] **Model manager / cloud config.** Downloads + cloud API keys unreachable — no UI, no config
      readers (cloud AI stays opt-in per invariant, but the opt-in surface is missing).

## 🟠 Visual conformance & doc drift (DESIGN-GUIDELINES audit)

**Real bugs (UI violates the executable token file):**

- [ ] **Undefined `--fs-2xs`** — `JobTray.svelte:232` uses `var(--fs-2xs, 11px)`; tokens.css has no
      such token (scale starts at `--fs-xs`). The fallback hides the typo. Fix: `var(--fs-xs)`.
- [ ] **Undefined 40px row stop** — `App.svelte:51` `ROW_H = {compact:32, cozy:36, large:40}`;
      tokens.css defines only 32/36. Either add the 40px stop to tokens.css or drop large to 36.
- [ ] **Dead density mechanism** — `:root[data-density="comfortable"]` (tokens.css:98-101) is never
      set by any component (0 hits); `--control-h` stays 30px at every density; App.svelte:251
      bypasses it with an inline `--row-h`. Wire the attribute or delete the block.
- [ ] **Density default is cozy, not compact** — `nav.svelte.ts:17` defaults `"cozy"` (36px rows);
      DG §4 says default **compact** (32px) with Comfortable as opt-in.
- [x] **Status gems wired (2026-08-07)** — `Card.svelte` renders the sprite's
      `ls-status-{unidentified,identified,confirmed}` gems; confirmed now shows the gold ★
      (DG §6).
- [x] **Card status rail (2026-08-07)** — 1px wide (was 3px); Identified **verdant**
      (`--ls-status-identified`) vs Confirmed **gold** (`--ls-gold`).
- [x] **Toasts (2026-08-07)** — left border 1px (was 3px); `warn|info` kinds added
      (`--ls-warning`/`--ls-info` accents); DG §7.
- [ ] **Icon sizes diverge** — nav 18 (DG 24), toolbar 14–16 (DG 20), type marks 28–40 (DG 32).
- [ ] **Sort options** — SORTS = Relevance/Name/Type/Size; DG: Name · Date added · Date modified ·
      Size · Type · Status.
- [ ] **No `prefers-reduced-motion` policy** beyond one DropZone gate; no shimmer on
      Unidentified→Identified (≤400ms, reduced-motion-gated).

**Doc drift (docs name things that no longer exist — fix per CLAUDE.md "docs are source of truth"):**

- [ ] **DESIGN-GUIDELINES §5** — "individual files not yet folded into the sprite" list is fully
      folded (16/16 in both sprites); the "Unicode glyph stand-ins until SVG rendering lands"
      caveat is obsolete (and names the deleted egui `theme.rs`).
- [ ] **ALIGNMENT-AUDIT.md** — rows it marks Missing (toasts, staging tray, Identify, Settings) are
      now built.
- [ ] **ROADMAP.md** — "egui/eframe on wgpu" shell (Phase 0), "three-d on shared wgpu" + Tauri-as-
      fallback tables, "normalize-to-glTF on import / runtime only sees glTF" — all superseded by
      the Tauri 2 + Svelte 5 + WebGL2 shell (the former fallback shipped).
- [ ] **MILESTONES.md** — "v1 ships at M5" scope note + M1.5 ratings are stale vs the board.
- [ ] **PACK-DIRECTORY-NAV.md §1** — status table references deleted egui components.
- [ ] **USER-GUIDE.md** — the core loop (auto-discovery → cache watch → identify flips to
      Identified → AI search → Unity/MCP import) cannot complete today; the guide should mark the
      blocked steps or the board should become the guide's status reference.

## 🔵 Engine-project / deferred

- [ ] **UPM C# Unity plugin.** handshake → register/heartbeat → browse → selective import w/ deps →
      main-thread `AssetDatabase` → ack. (Needs a Unity install to verify.)
- [ ] **Connect wizard plugin half.** App half done (Settings → Unity Editors); the plugin side
      registers/heartbeats.
- [ ] **Staging-commit half + batch undo.** Import enqueue → queue poll → ack → `import_record` →
      Recent exports / batch undo (REST-only transport endpoints exist).
- [ ] **LINKS full graph harvest.** prefab `uses_mesh`/`uses_audio`/`uses_script`; prefabs inside an
      extracted pack workspace.
- [ ] **The 33 ms Engine pump (B1).** Writer idle pass + pump events cover the spirit; the full
      `Engine` actor stays B1-blocked.
- [ ] **Unreal plugin + CUE4Parse sidecar** (planned phase).
- [ ] **Python sidecar (UnityPy)** for compiled bundles (planned).
- [ ] **B1.5 `lodestash-cli`** (planned; crate lands with the first real command).
- [ ] **Parked by design (not open work):** rating UI (column stays), `sqlite-vec`/LanceDB (behind
      the `VectorIndex` seam when brute force stops being comfortable), Unreal (later phase).
