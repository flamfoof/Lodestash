# Lodestash — Svelte re-write board (B-series status)

Status board compiled 2026-08-07 from a six-way audit (UI-SPEC / DATA-MODEL / ENGINE-API+MCP /
INGESTION / AI-PIPELINE / KANBAN truth-check) against the live code. The egui crate
(`crates/lodestash-ui`) was deleted at the B7 cutover — the Svelte shell +
`lodestash-desktop`/`lodestash-app` are the app. **Card state = verified in code, not comments.**

Lanes: 🟢 Done · 🟡 Partial (delta named) · 🔴 Missing · ⚪ Built but inert (needs wiring) ·
🎯 UX pass (the 2026-08-22 design review, D-series) ·
🔧 Extraction & structure (the 2026-08-14 audit) ·
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
- [x] **Import dialog (§5.2).** `+ Add source…` opens an **Add a source chooser** (Watch a folder /
      Unity Asset Store packages); the package path opens a native folder picker →
      `discover_package_folder` (reads each archive's manifest) → a modal listing packs **grouped by
      containing folder** (select-all/clear per group, collapsible), per-pack `vX · size · N assets`,
      auto-detected content-category badge + dropdown override, multi-version "newest shown"
      disclosure, and the §5.6 `⧉ already indexed` note, with category filter tabs and a footer
      ("indexed in place", N selected, **Import as libraries**). `import_packages` runs on the single
      writer (`Op::ImportPacks`): one source row + one pinned, in-pack-minted `package` row per
      ticked pack — nothing copied or unpacked.
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
- [x] **QA.** e2e 56/56 over the real binary (CDP), svelte-check 0/0, `cargo test` green; name-level
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
- [x] **Pack stats modal (2026-08-08).** Four tiles (indexed, unidentified, size, triangles)
      + a live per-media-type breakdown — `PackageRow.by_type` reuses the Stats screen's
      `TypeStat` shape, so colours/icons map for free. The "isn't available in this build"
      note is gone.
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
- [x] **Trait create form (2026-08-08, FEATURES §4.6).** Scope picker (any/model/audio/image),
      usage-note field, teach checkbox — all already supported by the `upsert_trait` command,
      now surfaced. Usage-note *editing* on existing traits is the remaining gap.
      image), no usage-description field, defaults `teach:false`; no usage-note editing.
- [x] **Unity Assets landing (2026-08-08, §5.3).** `↻ Sync` re-crawls every Asset Store cache
      root (incremental, skips unchanged files; env override honoured; pack-only/empty roots
      skipped) and content-derived category tabs filter the pack list — the search box is no
      longer the only tool.
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
      packs still extract by command only). The 256 MB pull cap now guards **only the thumb
      route** (the one that fans out across a pack — the 21.9 GB-pack stampede); a single
      explicit file/mesh pull is never refused by archive size (the deep-entry decompress-skip
      is bounded + cached). Remaining: per-entry "Extract selected" page-batching.
- [x] **The way out of the library (2026-08-22).** `⇢ Add to project` is on the Inspector, the
      tray and the context menu, and **⤓ Export to folder…** is beside each of them — the level-1
      copy (UI-SPEC §16.3) that needs no editor, no plugin and no registered project. Until the
      Unity plugin exists it is the only way a file actually leaves the library, which is why it is
      a button and not a menu entry. One implementation shared with the agent plane’s
      `export_asset`, so what an export copies cannot drift; the approval gate is skipped (the
      person clicking *is* the approval — ENGINE-API §4 amended) but the **sandbox is not**.
      Skips are counted by reason: *in a pack* needs an extract, *unreachable* needs the drive
      (§11.1). An export never unpacks an archive on the user’s behalf.

## ⚪ Built but inert — wire into the shell

- [x] **The right-click menu never rendered (found + fixed 2026-08-22).** The whole `{#if menu}`
      block sat **inside** `{#if quickLook !== null}`, so it existed only while the Quick Look modal
      was open — which is exactly when no card can be right-clicked. Every action reachable only
      through it (Copy name, Copy path, Toggle Favorite, Identify now, ⇢ Add to project, Move to
      Trash) was dead from the day it shipped, D16’s Copy name/Copy path included. Nothing had ever
      right-clicked a card in a test.
- [x] **…and Escape could not close it (same fix).** Its `Escape` handler was on the
      `role="presentation"` backdrop, which never takes focus, so it never fired. The only way out
      was a click — and until then the full-screen backdrop swallowed every click on the app
      behind it. Escape now lives on the window handler, ahead of everything else.

- [x] **REST + MCP servers start with the app (2026-08-08, B12).** One `ServerState` →
      `api::serve` (ephemeral loopback port + engine.json written) + `mcp::serve_stdio` on its
      own thread (self-terminates on stdin EOF); **`--mcp` headless branch shipped 2026-08-08**
      — `lodestash-desktop --mcp` serves the stdio plane without a window (smoke-tested with a
      real initialize handshake).
      agent host pipes stdin to the running app). Settings' ENGINE BRIDGE shows the live port.
- [x] **10 doc'd agent ops live on both planes (2026-08-11, ENGINE-API §2).** `list_tags`,
      `add_tag`, `list_collections` (incl. the active project's staging tray, flagged),
      `apply_mapping`, `identify_asset` (job), `suggest_tags`, `unpack_package` (job), and the
      staging group (`stage_asset` / `list_staged` / `import_staged`) — REST routes + MCP tools,
      gated 1:1 by the parity test (23 ops). Mutations flow through the **write bridge**
      (ADR-0002): `AgentWrite` → desktop `Op` (BulkTag/ApplyMapping/IdentifyIds/ExtractPackage/
      StageAsset) → the single writer; `identify_asset`/`unpack_package` park API-side jobs that
      poll via `get_job_status`. New writer ops: `IdentifyIds`, `StageAsset`, `UnstageAsset`;
      new catalog accessors: `staging_tray_id`, `active_project_id`.
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
      honored; workspace cap/7-day retention not enforced. **Re-confirmed 2026-08-14:**
      `workspace_cache_cap_gb` and `workspace_cache_retention_days` (`config.rs:452,460`) have
      **zero readers outside `config.rs`** — grep the workspace for either name. INGESTION §2
      "Workspace retention" specifies size-cap-first-then-LRU with a retention window plus a
      Settings → Storage cap / retention / **Clear workspace** action; none of it exists, and full
      extraction writes *every* payload (scripts and prefabs included — that part is per spec), so
      the workspace grows without bound and nothing ever reclaims it. The largest remaining hole in
      the extraction feature.

## 🔧 Extraction & structure — the 2026-08-14 audit

Its own lane on purpose: these came out of a full read of the per-pack Extract flow plus the max
review of that work, and they are neither "built but inert" (the code runs) nor purely visual.
The fixed rows are kept beside the open ones because the open ones are what the fixes *exposed*.

**Extraction defects found and fixed (2026-08-14)** — full read of the per-pack Extract flow
(`writer.rs` `Op::ExtractPackage` → manifest worker → `mint_in_pack` → `extract_at_offsets` →
`materialize_extract`):

- [x] **The extract queue could stall for the whole session.** UI-SPEC §5 unpacks ticked packs one
      at a time through `extract_queue`, drained by whichever worker reports back. But an
      `Op::ExtractPackage` can fail *before spawning anything* — package row gone, no workspace
      configured, `spawn_extract_worker`'s "no extractable payloads" — and the drain sites popped
      exactly one entry, so a failing pack left everything behind it queued with no worker coming.
      `extract_queue.is_empty()` then stays false forever, so **every later extract request in the
      session joins a queue nobody drains**: no job in the tray, no error, and `extract_package`
      never resolves — the row's spinner turns until the app is restarted. Both drain sites now go
      through `start_next_extract`, which keeps popping until one op actually spawns a worker.
- [x] **A cancelled surgical extract threw away the payloads it had already placed.**
      `extract_at_offsets_from` let the cancel sentinel escape as an `Err`, so `materialize_extract`
      discarded the list — rows kept `workspace_rel` NULL despite valid content-addressed files
      being on disk, and pressing Cancel surfaced a raw I/O error string. It now returns what it
      placed, matching `extract_to_workspace_with_progress`'s contract (a cancel is not a failure;
      genuine truncation still fails loudly). Covered by
      `a_cancelled_surgical_extract_keeps_what_it_placed`, mutation-checked.
- [x] **Cancel reported as failure.** "The pack extracted no assets" / "the pack has no manifest
      entries" fired even when the user cancelled before the first payload or manifest row landed.
      Both are now conditioned on `!cancelled`, and a cancel during the *analyze* phase keeps the
      partial locators (the next run resumes from them) without rolling on into extraction.

**Still open after the 2026-08-14 max review** (found by review, not fixed — each needs a decision
or more than a patch):

- [x] **`extract_package` answers at spawn — settled as (a), the tray owns it (2026-08-18).**
      `Writer::run` does a single `wait.recv()` and drops the receiver, so the `reply.send` the
      workers made from `ExtractDone`/`ManifestDone` always landed on a dead channel: the real
      `changed` count never reached the frontend, the per-row spinner cleared while the pack was
      still unpacking, and the code was half of design (a) wearing the plumbing of (b). **Shipped:**
      the `reply` field is gone from both `WriterMsg` variants and both worker spawns; the command
      replies exactly once, at *acceptance*, whether it runs now or queues (it used to reply at
      spawn when the queue was empty but only at start when it wasn't — one command with two
      meanings, depending on queue depth); `extract_package` no longer goes through
      `AppState::write`, which reloaded the whole 8k-row library snapshot at spawn, before a single
      payload existed; and `UnityAssetsScreen` derives each row's busy state from the tray's active
      jobs instead of awaiting a promise that resolves in milliseconds.
- [x] **A panicking worker wedged the queue for the session.** `jobs.finish` only runs when a worker
      sends its `WriterMsg`; a panic in `read_manifest_with_progress` / `extract_at_offsets_with_progress`
      (corrupt gzip, malformed tar) unwinds a detached thread, the send never happens, the panic is
      swallowed, and `has_active_extract()` stays true forever — every later extract takes the
      `push_back` branch and nothing drains it. `start_next_extract` fixed the `Err` road to this
      jam, not this one. **Fixed 2026-08-18:** both workers wrap their body in `catch_unwind` and
      send an error outcome on the way out, so the send that runs `jobs.finish` and drains the queue
      happens even when the archive is corrupt enough to panic the gzip/tar path.
- [ ] **The sound card's ▶ is decoration.** `<span class="play">▶</span>` sits inside an
      `aria-hidden` wrapper with no handler, so clicking it selects the card and screen readers
      cannot reach it. UI-SPEC §2.5 specifies a working ▶/◼ with an inline scrubber and one asset
      playing at a time (§13.2) — none of it wired.
- [ ] **Doc drift: card status rail width.** UI-SPEC §2.5 says 3px, DESIGN-GUIDELINES §6 says 1px,
      the app follows DG (ALIGNMENT-AUDIT F1). UI-SPEC is the stale one — fix it per CLAUDE.md
      rule 2 so the next reader is not told the code diverges when it conforms.

**Structural audit (2026-08-14)** — answering "too much code per file, too many tests":

- [ ] **`catalog.rs` is 9,423 lines, 51% of it tests.** 153 `pub fn`, **8 `impl Catalog` blocks**
      (lines 213, 2044, 7575, 7704, 7799, 7899, 7968, 8262, 8732) and 3 `#[cfg(test)]` modules
      (4615, 7643, 8759) that each own one domain and duplicate their own `rec()` fixture (5096,
      8763). Every crate depends on this one, so touching a single query recompiles 9.4k lines and
      re-runs 69 tests. The impl blocks already mark the seams: split to
      `catalog/{mod,assets,tags,collections,taxonomy,oplog}.rs`, tests moving with their domain —
      a mechanical move, no behaviour change.
- [x] **The test budget was inverted — `writer.rs` has tests now (2026-08-18).** It had **zero**
      over the only genuinely concurrent code in the repo, and the reason turned out to be
      structural rather than neglect: Tauri's window layer (tao/muda/rfd) imports comctl32
      **v6-only** entry points (`SetWindowSubclass`, `RemoveWindowSubclass`, `DefSubclassProc`,
      `TaskDialogIndirect`); the app exe carries a manifest declaring that assembly and a `cargo
      test` binary does not, so it bound comctl32 v5, found none of those exports, and died with
      `STATUS_ENTRYPOINT_NOT_FOUND` **before the first test ran**. Any test that so much as
      constructed an `Op` made the whole binary unloadable — which reads as a broken test, not a
      missing manifest. `build.rs` now passes the Common-Controls v6 `/MANIFESTDEPENDENCY` under
      MSVC, and the queue invariant is split into `drain_to_first_spawn` with two tests over it,
      both mutation-checked. **Still open:** the catalog-side trim — `catalog.rs` spends 4,809 lines
      on CRUD, and its 8 `impl` seams are still one file.

Checked and **not** a defect: extracting unclassifiable payloads (`.cs`, `.prefab`, `.meta`) is
INGESTION §2's "full extraction … extracts **every payload** once", which is what makes every
sibling in a pack random-access on disk. Entries dropped from `wanted` for lacking a `tar_offset`
are pathname-only folder rows — `read_manifest` records an offset for every real payload
(`unitypackage.rs:317`).
- [ ] **Non-content classification.** `detect::is_content` zero callers; `is_content`/`content_kind`
      never set.
- [x] **Model manager (2026-08-12).** Settings gained an AI MODELS card: SigLIP 2 (visual
      search + tagging), CLAP (audio) and the ONNX Runtime dll — install status, Download /
      Re-download / Remove buttons, progress through the job tray (`models_status` /
      `model_download` / `model_remove` commands; the downloader in `lodestash-app::models`
      was egui-era-only). The ORT dll loads at startup, so a fresh download needs a restart
      (noted in the UI). Cloud API keys were already reachable via AI TAGGING — the remaining
      opt-in surface gap is gone.

## 🎯 UX pass — the 2026-08-22 design review (D-series)

Fourteen proposals from a review of the specs against the code, designed as artboards and written
into UI-SPEC / DESIGN-GUIDELINES / DATA-MODEL in the same change. Three needed an ADR rather than an
edit: [ADR-0007](decisions/ADR-0007-one-search-box.md) (the search-mode toggle goes),
[ADR-0008](decisions/ADR-0008-appraisal-is-a-background-job.md) (appraisal leaves its own screen),
[ADR-0009](decisions/ADR-0009-responsive-ladder.md) (the responsive ladder narrows the 2026-08-06
facet-group owner call).

- [x] **D1 — One search box.** Drop the Tags/AI segmented toggle; typing always filters, the AI is a
      pinned escalation row + the empty state's primary action; arcane border while ranked results
      are shown, `← back to name filter` returns. UI-SPEC §0.1.3, §2.6; DG §7 "Search bar".
- [x] **D2 — Active-filter bar.** One strip under the toolbar, one removable chip per active
      constraint, `n of N shown` + Clear all, not rendered when nothing is filtered; `+N more`
      popover past one row. UI-SPEC §2 item 7; DG §7 "Active-filter bar".
- [x] **D3a — Appraisal runs itself.** The background pass already existed in the writer's idle
      loop; it was gated on `auto_identify`, which **defaulted to off** — so the product's central
      capability was opt-in and onboarding's first sentence to a new user was false. Default flipped
      (ADR-0008), and the Identify screen is a dashboard: a running/paused row whose Pause writes
      that same setting (it *is* the switch — a second flag would give the app two answers to "is
      appraisal running?"), plus per-source backlog rows with their own ✦ Appraise via a new
      `identify_ids` command, so 400 assets is one bounded pass rather than 400.
      **Note:** the default only affects fresh configs; an existing `config.json` keeps the value it
      has, so anyone who ran this build before still has it off.
- [x] **D3b — The priority queue.** `AppraiseFocus` on the `Catalog` + one shared `ORDER BY`
      expression, so **all eight** appraisal queues (tags, audio tags, three embedding queues,
      metrics, previews, dep scans) move together — prioritising only the tag queue would leave an
      on-screen asset tagged and then waiting behind 3,000 others for the embedding that promotes
      it. `Library::focus_order` cuts the tiers from the query the grid just ran (the page on
      screen, then the filter running **forward** from it, capped at 500), and reaches the writer
      through shared state rather than an `Op`: browsing must never queue behind a running appraise
      pass. Debounced 400ms — the only debounced thing on that path, because the grid must answer
      every keystroke but the backlog need not chase a half-typed word.
      **Caught by the change:** `assets_needing_preview` walked its queue with an `a.id > cursor`,
      which silently returns nothing once the order is no longer ascending — and "nothing" looks
      exactly like a drained backlog. Now an attempted-set. The queue order is stated on the
      Identify screen, as §3 requires.
- [x] **D4 — Keep-it ladder.** The staging tray can promote itself: **▣ Save as collection…**
      names the set and moves every staged id into it, so a shortlist built by hand is no longer
      thrown away on import. Empty states teach the relationship — a star is a pin, ⟡ is a cart for
      right now, ▣ is a set worth naming. `create_collection` answers with a change count rather
      than the new id, so the id comes from re-listing and matching the name; a collision resolves
      to the newest row, which is the one that click created.
- [x] **D5 — Inspector collapse + compare.** 26px edge tab when nothing is selected (closes N5.3);
      460px two-up compare when exactly two are picked, differ-is-gold stat table. UI-SPEC §13.
- [x] **D6 — List-row parity.** star / ⤢ / stage / pack link / size badge on rows, actions in a
      trailing column (closes audit **F6**). UI-SPEC §2.5.
- [x] **D7a — Source availability is known and shown.** **M18** (`import_source.state` +
      `last_seen_at`), `Catalog::refresh_source_health()` — one `is_dir` per **root**, never per
      asset — carried on `SourceSummary`, run through the writer as `Op::RefreshSourceHealth` (it
      writes, so ARCHITECTURE §5 says it belongs there) and fired once at open, after the first
      status read so a slow network root cannot block the first paint. Shown as the red `✕` on the
      sidebar facet and the Library banner. Mutation-checked on the three things that would make it
      dangerous: a missing root must not delete or trash rows, a returning root must clear the
      state, and `last_seen_at` must not move while the root is gone.
- [x] **D7b — Source availability, the rest.** `GridItem.source_id` (one i64, stamped in the loop
      that already had the source in hand) plus `sources.svelte.ts`, so the plate, the inert ⟡,
      Quick Look's *"this file isn't reachable"* panel and the tray's refusal are **one fact read
      four ways** rather than four copies of `state` on 8k cards. Relink is a rename, not a re-scan:
      `Catalog::relink_preview` counts first (loose files by `rel_path`, in-pack assets through
      their **archive**, memoised per pack; workspace-extracted rows excluded from both numbers),
      `Catalog::relink_source` rewrites only `root_path` and the `package_path`s under it — a pack
      outside the old root is left alone rather than re-anchored to a guess. ⇢ the tray refuses
      **partially and before the click**: unreachable picks are not sent, stay staged, and the
      button reads `⇢ Add N of M`. **Divergence recorded in UI-SPEC §11.1:** `⤢` stays live (it is
      the only route to the panel that offers Relink); only `⟡` and the Inspector's ⇢ go inert.
      Verified on the real catalog — source 4 is genuinely gone, so all four e2e checks ran rather
      than skipped. `relink_source` is Rust-tested only: committing one in e2e would rewrite the
      user's own catalog.
- [x] **D8 — Undo in the toast.** One action slot (`Undo` | `Show`), 6s with an action, hover
      freezes, 3 at a time with the rest collapsed into `+N more`. Each toast pins **its own oplog
      id** at push time — undoing the head of the log from the second-newest toast reverses the
      wrong action silently. DG §7 "Feedback". Wired for trash/restore **and** bulk tag edits — the latter bypasses App's
      `write()` helper, which is why it was the one action announcing itself with no way back. Only
      3 ops are recorded at all (trash, collection create, bulk tag); no head means no slot, rather
      than a button that lies.
- [x] **D9 — Focus ring + reduced motion.** The focus half was already global (`app.css`
      `:focus-visible` → `--el-focus-ring`, covering the `role="button"` spans too) — the review had
      that wrong and DG §9 is corrected to say so. Built: the §8 reduced-motion table. Spinners are
      the stated exception — a frozen spinner reads as a stuck UI and is the only signal a job is
      alive. *(Focusable **cards** still wait on a roving `tabindex` — see D15.)*
- [x] **D10 — `✓ in <project>` badge.** `GridItem.in_project`, filled by one
      `imported_into_active_project()` read per reload (`mark_imported`, beside `mark_traits`) —
      the data was always there and the bulk read was not (closes **N1.5** / B12e). Card shows
      `✓ in project` beside the size tier; rows show the check alone. Mutation-checked on the two
      ways it could lie: keying off `is_active`, and reading empty rather than erroring with no
      active project. All four of DG §7's states now: `✓ in project`, `✓ older copy` (amber —
      `asset_file.mtime` past `import_record.imported_at`, the one actionable one), `✓ in N others`
      (dim — history, not a warning), and nothing at all for never-imported. `import_states()`
      answers all of it in two queries per reload.
- [x] **D11 — Duplicates as a queue.** Group list (280px, resolved rows keep their place and show
      their outcome) + the compare pane, `n / N groups reviewed` over a gold bar, keeper ribbon,
      `2 of N ›` cycling with the keeper always holding the left pane, ↑↓ to move. The differ-is-gold
      rule now lives in one shared `CompareRows` component used by both §13 and §5 — two copies of
      it would have drifted. Exact groups queue ahead of visual ones: a byte-identical copy is
      unambiguously safe to trash and a dHash match is not.
      **Merge tags onto keeper** is built: the union of every member's traits applied to the
      keeper, never removing anything — a merge that could *lose* a tag would make trashing the
      duplicate afterwards a data loss, which is the one thing this screen must never be.
      **Still not verified end-to-end:** the check now needs only one group instead of two, and this
      library has **zero**, so the queue path has never actually run here.
- [x] **D12 — Responsive ladder.** All four stops, and now actually working: 1400 hides the
      keyboard hint, 1280 makes the inspector *overlay* the grid keeping its edge tab, 1120
      collapses the sidebar to a 48px icon rail whose hover / keyboard-focus flyout carries the
      labels, counts and both facet groups (ADR-0009's narrowing in the flesh), 960 folds sort and
      its direction button into a `⋯` overflow — **moved, not removed**, which the first cut got
      wrong. ★, ⚑ and the view toggle never fold.
      Steps 3 and 4 live in `Sidebar.svelte` / `Toolbar.svelte`, not the global sheet: a component's
      scoped rules outrank (or tie and beat, on source order) a bare selector in `app.css`, so the
      first version of both did **nothing at any width** while type-checking and "parsing" clean.
      Driven end-to-end by `Browser.setWindowBounds` — `setViewportSize` moves nothing here and
      leaves an emulation override that poisons later checks.
- [x] **D13 — Onboarding copy.** The shipped onboarding is the one-screen explainer, not §1's
      two-step scan wizard, so the change landed there: step 2 now says appraisal **starts by
      itself** rather than implying there is something you must go and run. Assets still arrive
      Unidentified — softening that would hide why early AI searches come back thin.
      **Delta:** §1's scan wizard (progress log, completion callout, Open library / Watch progress)
      is still unbuilt; its copy in the spec is written for ADR-0008 and waits on the wizard.
- [x] **D14 — Empty states.** One `EmptyState` component enforcing the anatomy (32px nav glyph ·
      statement title · one teaching line · at most one action), across Collections / Duplicates /
      Packages / Unity Assets / Identify. Filtered-empty cases stay separate — those need a way out,
      not a lesson (UI-SPEC §2.6). DG §7 "Feedback". Trash uses it too now.
- [x] **D16 — Selectable content + Copy name/path.** Reported in use: `user-select: none` on
      `<body>` made every asset name, path, pack and stat impossible to copy. Content opts back in;
      cards and rows (both `<button>`) get **Copy name** / **Copy path** in the context menu, where
      DG §7 had already specified Copy path and nothing built it. Also added: `Escape` clears the
      selection per UI-SPEC §0.4, which was never implemented and without which D5's collapsed rail
      is unreachable from the keyboard.
- [x] **D15 — Grid keyboard nav.** Roving `tabindex` (one tab stop), arrows move the lead with
      focus following it, ↑↓ by a whole row read off the *resolved* `grid-template-columns` rather
      than recomputed from density, Home/End, and Shift+arrow/click extending the pick set. Left out
      of the design pass and pulled back in because D9 cannot put a ring on a card without it.
      Pinned by a check asserting focus and the lead never come apart, and that the grid offers
      exactly one tab stop. UI-SPEC §0.4; DG §9 no longer records it as a gap.

---

## 🟠 Visual conformance & doc drift (DESIGN-GUIDELINES audit)

**Real bugs (UI violates the executable token file):**

- [x] **Density tokens live (2026-08-11).** The frame sets `data-density` (compact default, DG §4)
      but `docs/design/tokens.css` keyed the stops on `:root[data-density]` — never matched, so
      every density rendered 32px rows and `--control-h` stayed 30px. Selectors now key on
      `.frame[data-density=…]` (the element that carries the attribute): compact 32/30, cozy
      36/34, large 40/38. `--fs-2xs` typo and the inline `--row-h` bypass were already gone.
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

**B12 — visual parity vs the 2026-08-13 mockup.** First pixel-level comparison (a rendered mockup
now exists: `docs/design/screenshots/library-mockup-2026-08-13.png`). Full matrix + `file:line`
evidence in [`docs/design/ALIGNMENT-AUDIT.md`](design/ALIGNMENT-AUDIT.md) **§N** — read it before
starting any card here; the ids below are its row ids. **The mockup drops the Unity Assets nav row
and the Materials tab — `FEATURES.md` wins on both, do not "fix" the app to match.**

- [x] **B12a — the card reads as its media type (2026-08-14).** Thumb at the spec's **104px**;
      5-bar `▶` waveform for audio the backend cannot rasterise (the `<img>` still wins when
      `lodestash://thumb` *can* draw the real envelope — `protocol.rs:264`); `cover` for tileable
      art, `contain` for subjects, so a 2048² albedo fills the strip while a sprite keeps its head.
- [x] **B12b — subtract the duplicates (2026-08-14).** Toolbar status chips deleted (the sidebar
      facet is UI-SPEC §0.2.2's control and §2.1's header row never listed them), which also puts
      the result label first (§N2.5); no `LIBRARY` heading — Favorites/Trash keep theirs per
      FEATURES §4.7 and the e2e that pins it.
- [x] **B12c — the star became a control; `⤢` exists (2026-08-14).** The grid could previously only
      *report* a favourite. **Three of this card's four items were wrong:** UI-SPEC §2.5 puts the
      pick checkbox and the `⤢`/`⟡` tray on **hover**, not at rest — the mockup loses. See §N6.
- [x] **B12d — treatment polish (2026-08-14).** Two-row chip wrap; slate-italic unidentified names
      *plus* the §2.5 30%-grayscale art behind a large slate `?`; `1.2k tris`; badges clamped at
      `999+`; and the **confirmed at-rest gold ring** §2.5 asks for (audit row **F4**, open since
      08-04). Full-perimeter status border **rejected** — §2.5 specifies a left border.
- [ ] **B12e — `✓ in <project>` membership pill (§N1.5).** *Not blocked* — the earlier "no command
      exists" was wrong. `import_record` is populated and per-asset membership is queried at
      `catalog.rs:2985`; what is missing is the bulk read (an `in_project` flag on `GridItem`,
      filled from the active project's import set at reload).
- [x] **B12f — resolved by reading the spec (2026-08-14).** Tab counts stay (UI-SPEC §0.3); the
      PINNED sidebar group stays (audit row C6). No code, no doc change — the mockup predates both.
- [x] **B12g — top bar (2026-08-14).** Health meter as `Identified n / total` over a 6px
      gold-gradient bar (§0.1.5); project chip `Name · Engine` (§0.1.6); Tags/AI toggle ahead of the
      search box per §0.1's order. **GPU name still open** — nothing in `lodestash-desktop`/`-app`
      exposes an execution provider or adapter name, so the pill would have to invent one (**B8**).
- [ ] **B12h — the list rows never got it (audit F6).** `AssetList.svelte` still lacks the star,
      `⤢` and stage treatment UI-SPEC §2.5 specifies for rows; chips are not click-to-search and the
      pack name is not a link to Pack stats §15. Also **§N5.3**: the inspector holds its rail open
      showing "Select an asset." when nothing is selected.

> **The trap this pass paid for, in one line:** a hover-revealed control at an `opacity: 0` still
> takes clicks, so the 104px thumb pulled the action tray under the card's *centre* and ate the
> double-click that opens Quick Look — surfacing three checks later as the 3D viewer hanging. The
> e2e now asserts on geometry: *whatever sits at a card's centre must be the card.*

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
