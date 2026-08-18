# Lodestash — Svelte re-write board (B-series status)

Status board compiled 2026-08-07 from a six-way audit (UI-SPEC / DATA-MODEL / ENGINE-API+MCP /
INGESTION / AI-PIPELINE / KANBAN truth-check) against the live code. The egui crate
(`crates/lodestash-ui`) was deleted at the B7 cutover — the Svelte shell +
`lodestash-desktop`/`lodestash-app` are the app. **Card state = verified in code, not comments.**

Lanes: 🟢 Done · 🟡 Partial (delta named) · 🔴 Missing · ⚪ Built but inert (needs wiring) ·
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
- [ ] **`⇢ Add to project` / `⇢ Add all N to project` affordances.** Absent everywhere (FEATURES
      §5.4/§5.5, PACK-DIRECTORY-NAV §7) — the engine write path doesn't exist in the shell.

## ⚪ Built but inert — wire into the shell

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

- [ ] **`extract_package` answers at spawn, not at completion.** `Writer::run` (`writer.rs:803`)
      does a single `wait.recv()` and drops the receiver, and the direct arm sends the
      `Ok{changed:0}` "queued" sentinel the moment a worker starts. So the `reply.send` the workers
      make from `ExtractDone`/`ManifestDone` **always lands on a dead channel and is discarded** —
      the real `changed` count never reaches the frontend, `UnityAssetsScreen`'s per-row spinner
      clears while the pack is still unpacking, and `extractSelected` marks pack N+1 running while
      N is still going. Two coherent designs; pick one: **(a)** the tray owns progress and outcome
      (right for a multi-GB unpack) — then delete the dead `reply` plumbing from both workers and
      have the row follow the tray's job state; or **(b)** the command blocks to completion — then
      the direct arm must not send. Today's code is half of each, and the comments claimed (b).
- [ ] **A panicking worker wedges the queue for the session.** `jobs.finish` only runs when a worker
      sends its `WriterMsg`; a panic in `read_manifest_with_progress` / `extract_at_offsets_with_progress`
      (corrupt gzip, malformed tar) unwinds a detached thread, the send never happens, the panic is
      swallowed, and `has_active_extract()` stays true forever — every later extract takes the
      `push_back` branch and nothing drains it. `start_next_extract` fixed the `Err` road to this
      jam, not this one. Wants the worker's join handle watched, or a job lease that expires.
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
- [ ] **The test budget is inverted, not oversized.** 324 tests workspace-wide is not the problem;
      *where* they sit is. `catalog.rs` spends 4,809 lines on CRUD while `writer.rs` — 2,040 lines,
      the only genuinely concurrent code in the repo, owning the extract queue, cooperative cancel
      and the spawn/drain invariant — has **zero**. The queue-jam bug fixed above is exactly what
      one test over `start_next_extract` with a two-op queue and a failing first op would have
      caught. Cut from the catalog CRUD tests, spend a fraction here: `start_next_extract` is a
      pure function over `(catalog, queue)` and needs no Tauri.

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
