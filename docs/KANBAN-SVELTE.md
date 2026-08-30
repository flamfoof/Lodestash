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
- [ ] **Inspector gaps — relink done 2026-08-29, the rest open.** §13.6's four row states ship
      (`same_pack` on `related_assets` separates ✓ in pack from ⤷ in library) along with the
      relink panel's ⊘ Ignore / ↺ Restore (`Catalog::ignore_relationship`, `state='ignored'`,
      which survives `record_dependencies` re-deriving a harvest) and the export-skip warning the
      spec claimed was already there. Name-match suggestions and ⌕ Browse disk… are **dropped as
      unbuildable**, not deferred — every edge declares a GUID, which has no name to match and no
      disk file that resolves it (UI-SPEC §13.6 records the reasoning).
      **In-archive provenance done 2026-08-29** — §13.9 grows **Path in pack** and **Archive**
      rows for an in-pack asset (`Catalog::in_pack_location`), copyable, and absent for an
      extracted one whose bytes are on disk. The copy affordance is the sprite's
      `ls-action-copy`, not the spec's ⧉: U+29C9 is not in the bundled text fonts and would
      render as tofu, like ⌕ and ◉ before it. *(card ★ was stale — it is a control, corrected
      2026-08-27.)*
- [x] **SIMILAR is real similarity — done 2026-08-29.** `Library::similar_assets` does cosine KNN
      in the asset's own embedding space, self and trashed excluded, top 3 with thumb + name +
      pack. **Embeddings, not the shared tags §13.8 first asked for**: 24 of 8,237 assets carry a
      tag against 6,807 carrying an embedding, so a tag ranking would be empty almost everywhere
      (recorded in §13.8). The index is built once per model and held in `Library`, cleared on
      `reload` — 221 ms cold, ~47 ms warm on this library; a rebuild per selection would re-read
      every embedding out of SQLite to answer one panel.
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
- [x] **Onboarding is the two-step setup flow (2026-08-28, UI-SPEC §1).** Step 1 lists the
      sources actually indexed — name, mono path, asset count, `folder` / `auto-detected` pill,
      unavailable roots dimmed with a ✕ — above a verdant *"existing index found — N assets
      across M sources, last scanned X"* banner, with **+ Add another folder…**, **Scan sources**
      and **Open library →**. Step 2 scans and reports. `SourceSummary` gained `kind` and
      `last_scanned_at` (both already on `SourceInfo`, never carried to the frontend), and
      `unity_cache_candidates` detects the Asset Store cache **without importing it** — an
      `is_dir` per candidate root, because the importing version would crawl the machine to
      answer whether there was anything to find.
      **Two narrowings, written into §1:** no *"M new files"* count (finding it out costs the
      same walk as the scan it advertises), and the progress bar counts **sources**, not files —
      `add_source` is a synchronous crawl that reports nothing mid-walk, and its own doc says a
      fake bar is worse than an honest spinner. Each log line is a source that genuinely finished.
      The old copy also promised you could *"pull the right asset into Unity in one step"*, which
      nothing in the app can do until the plugin exists.
- [x] **The e2e onboarding check had never asserted anything (2026-08-28).** `launch()` spends six
      seconds clicking the panel away so the other ninety checks can reach the app — correct, and
      it means the check written as *"if the panel is present, assert things about it"* takes its
      branch **never**. It passed for months over a screen it could not see, which is how a
      rewritten onboarding nearly shipped unlooked-at. The harness gained `keepOnboarding`, the
      check now asserts the thing that actually matters to the suite (the panel is *gone*), and
      the panel itself is inspected by `node e2e/onboarding-shot.mjs`. Two real bugs came out of
      finally looking: the footer wrapped **Open library →** onto a row of its own *below* the
      secondaries, and §1's `⌕` is not in the bundled Inter so it rendered as tofu — the same
      trap §15 hit with `◉`, and the same fix (the icon sprite, since the fonts ship with the app
      for the offline invariant).
- [x] **Tag Graph interactive (2026-08-08).** Drag to pan, wheel to zoom (clamped 0.5–4×), click a
      node to filter the library to that tag and navigate — dependency arcs still belong to the
      LINKS graph (separate card).
- [x] **Settings gaps (2026-08-08).** Clear-cache/usage bar live (STORAGE CAPS — bytes + files,
      thumbs + mesh/inpack scratch only, workspace untouched); confirm-before-trash toggle
      (two-step: first click arms a 3s confirm toast, second trashes — inspector + Duplicates
      paths). CLOUD card (the AI TAGGING section covers the API endpoint) + the engine
      port/token surface shipped earlier; model-download button still open.
- [x] **Recent exports — both halves of it (2026-08-22).** Another card that had gone stale: the
      project-switcher popover, the import-batch list and `undo_import_batch` were **already built**
      in `TopBar.svelte`. What was actually missing is that **⤓ Export to folder wrote no history at
      all** — shipped earlier the same day, and the one export path that actually runs on this
      machine, since `⇢ Add to project` queues into a Unity plugin that does not exist yet. A
      Recent-exports list that could only ever show imports would have been a permanently empty
      panel.
      **M19 `export_record`** (DATA-MODEL §9.1) records one row per **file written** — not per asset,
      because a dependency closure makes those different numbers, and not from the request, because
      an export skips in-pack and unreachable picks. Both export paths record, the agent plane's
      included. The popover now merges imports and folder exports into one list sorted by time: the
      user thinks *where did that go and can I take it back*, not "which table".
      Undo is the only delete in the app aimed at a folder the **user** chose, so it is fenced on
      four counts and mutation-checked on each: a `file_name` carrying a path is refused rather than
      followed out of the folder; the destination is re-checked against the export sandbox **at undo
      time**, because a folder that was legal to export into may since have become a library source;
      a missing file counts as already-undone; and a run that cannot finish keeps its record so the
      copies stay findable. Originals are never in scope — an export copies, so undo deletes copies.
      **Closed 2026-08-27** — the shell's shortlist is the catalog tray now, see the
      cross-feature-audit section below.
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
- [x] **Render-pipeline harvest — done 2026-08-28.** `lodestash_ingest::pipeline` classifies from
      the names a pack states (its own, its `package_entry` pathnames, its UPM manifest deps), and
      `harvest_pack_metadata` writes `package.pipeline` on both import paths. The
      `pipeline_mismatch` advisory (ENGINE-API §5.4) now fires on REST, MCP and the desktop.
- [x] **License/attribution harvest — done 2026-08-28, and the card was half-wrong.** The setter
      (`set_package_metadata`) and the whole harvest (`unitypackage::read_package_meta`) already
      existed; the Tauri writer simply never called them — it ran `read_package_meta` at discovery,
      kept `version`, and dropped the licence. Now written at import, with the Asset Store EULA
      filling a silence for store-cache packs (they have a `publisher`), M21 backfilling the packs
      already in the catalog, and UI-SPEC §13.9's License row reading the effective licence.
      *Still open, and moved to its own card below:* `.meta` `labels:` are parsed but never seeded
      into tags.
- [x] **The agent plane can read assets and write tags — done 2026-08-29.** Probed
      `lodestash-desktop --mcp` over stdio and found the surface half-built: 23 tools, clean
      handshake, reads fine — and `add_tag` refused (`no writer bridge — read-only mode`) while
      `get_asset_preview` failed for **0 of 60** sampled assets of every type, returning `Err`
      before it added even the file link. Writes now forward to the running app's single writer
      over the `engine.json` loopback plane ([ADR-0011](decisions/ADR-0011-headless-mcp-forwards-writes.md));
      previews resolve the whole subject from the catalog (verified on-disk path, in-pack surgical
      pull, material albedo) and reached **57 of 60** on the same sample. Verified end to end: a
      tag written over MCP appears in the app.
      *Still open:* the Streamable-HTTP MCP transport on the stable port 4319 (UI-SPEC §11.5), and
      `remove_tag`/`unstage` have neither a REST route nor a tool.
- [x] **Two tag-vocabulary defects, found while preparing a tagging batch (2026-08-30).**
      **171 of 210 tag rows were the e2e suite's own residue.** The NEW TRAIT check named its
      trait `e2e-trait-${Date.now()}` and "cleaned up" with `unpromote_trait`, which demotes a
      trait to a plain tag rather than deleting it — so every run left one more row and the
      comment claimed the opposite of what happened. Now a **fixed** name (`upsert_trait` is
      idempotent on the normalised name, so the suite costs the vocabulary exactly one row, for
      ever) plus an assertion that no more than one `e2e-` row exists, so the next leak fails a
      check instead of accumulating for weeks. The 171 were purged.
      **And the tagger emitted numbers as labels** — `mood:"0 71"`, `style:"0 67"`,
      `subject:"0 73"`, `material:"01"`: its own confidences landing in label slots. `parse_facets`
      validated the *facet* name and never the value. It now requires a tag to contain a letter
      (digits are fine inside one — `4k`, `pbr2`), which is the guard that keeps a small model's
      arithmetic out of the autocomplete. Six rows purged.
- [x] **`identify_asset(ids)` did not identify those ids — fixed 2026-08-30.** The op flagged the
      requested assets (`mark_for_identify`) and then ran a *generic* bounded appraise pass, which
      takes the front of the **global** queue in appraise-focus order. With 8,155 dirty rows on the
      reference library, three assets an agent asked about were nowhere near the front: the call
      returned a job, the job reported `done` in 5s, and every requested asset came back with
      `tags_dirty` still 1 and no tags — success by every signal the caller could see.
      `assets_needing_tags_within` now takes the explicit set, threaded through `drain_tags` and
      `appraise_pass`; the idle slice still passes `None` for the whole queue. An explicit **empty**
      set means nothing, not everything (mutation-checked — that inversion is how a scoping bug
      turns into a full-library appraisal).
- [x] **Ollama tagging, end to end — proven 2026-08-30, with four defects found doing it.**
      `qwen3:8b` (facets) + `moondream:latest` (caption) over the MCP plane, writing through the
      app's single writer: *tagging pass finished tags=17*, then *tags=15* on a second batch.
      **(1)** `identify_asset`'s job reports `done` in ~5 s while the pass runs ~80 s — an agent
      that polls `get_job_status`, sees `done` and reads the tags gets an empty answer, and a
      caller that exits on it kills the work. Fooled three of my own runs before the app log gave
      it away. *Still open — carded below.*
      **(2)** `identify_asset(ids)` appraised the front of the global queue, not the ids — fixed.
      **(3)** `AssetSummary.tags` answered `[]` unconditionally, so a written tag could not be read
      back through the plane that wrote it — fixed, `asset_summaries` fills it.
      **(4)** A caption that is not prose landed in `asset.description`, which FTS indexes:
      `moondream` answered `ids: [0.17, 0.13, 0.83, 0.37]` when asked to describe a pile of sacks —
      fixed, `clean_caption` requires prose and strips `IMAGE!!!` too.
      **Model quality is now the limit, not the pipeline.** moondream called a labelled beer crate
      an *urn* (the source of the old `subject:urn` across all four facets) and a bucket a *wooden
      board*. A stronger vision model is the next lever.
- [x] **`identify_asset`'s job finished before its work did — fixed 2026-08-30.** Two nested jobs:
      the headless MCP server created one, its thread POSTed `/v1/identify` to the running app, and
      the app answered `202` with a job of *its own* and started the writer pass. The bridge took
      that `202` as completion, so the outer job flipped `done` in about five seconds while the
      writer ran on for eighty. An agent that polled `get_job_status`, saw `done` and read the asset
      back got the tags as they were before it asked; a caller that exited on `done` killed the app
      mid-pass (it fooled three of this session's own diagnostic runs). `changed_from` now
      recognises a job body and `await_job` polls `/v1/jobs/{id}` to a terminal state before
      returning, reporting the inner job's `changed` — so the outer job ends when the inner one
      does. 30-minute backstop, and blowing it says so with the job id rather than pretending
      failure.
- [x] **Local tagging: the vision model was not the bottleneck — fixed 2026-08-30.** Pulled
      `qwen2.5vl:7b` (6 GB) and re-tagged the moondream sample. The 3D case improved outright: a
      labelled beer crate went from `subject:urn` to `subject:wooden crate`. The four **image**
      cases stayed wrong, and pulling the bytes out of the pack said why — `round crates.png` is a
      texture *atlas*, wood planks and rope strips laid out for UVs, with no crate in the picture.
      The model was reading it correctly; the facet model was then trusting the caption over the
      artist's own filename. One clause added to the facet prompt for `texture`/`image`/`sprite`
      (`flat_art_caveat`), A/B'd against the live model with the caption held fixed: `round crates`
      `door` → `crates`, `bucket` `crate` → `bucket`. Unit-tested both directions — a 3D model must
      **not** get the clause, since its preview is a render of the object and the pixels are the
      better evidence. Written up in [`AI-PIPELINE.md`](AI-PIPELINE.md) §2.
- [x] **Icon ladder — settled on DESIGN-GUIDELINES §5 (2026-08-30).** DG said 16/20/24/32, UI-SPEC
      §0.1 said the nav glyph is 16, and the shell shipped nine sizes with nav at 18 — three
      sources, three answers, so "conform to the spec" was not a well-defined action. Both ladders
      were rendered and compared in the running app; DG's wins, UI-SPEC §0.1 is corrected to 24, and
      every `<Icon size>` in the shell is now one of the four rungs (17×16, 12×20, 4×24, 7×32). The
      32 rung gained *art placeholders* — the card / Inspector / Quick Look type mark, previously 28
      and 40 — because that role was real and unnamed. Recorded as
      [ADR-0012](decisions/ADR-0012-one-icon-ladder.md), and **checked**: an e2e case measures every
      rendered `svg.icon` and asserts the widths are a subset of the ladder, with the nav glyph
      pinned at 24 (a bare `nav` selector matches TopBar's tabs first, which are 20 — the check
      names `nav[aria-label="Main"]` for that reason).
- [ ] **In-pack materials get no preview.** `ServerState::preview_subject` resolves a material's
      shader ball from the material's own `disk_path`, which an in-pack material does not have, so
      `engine_preview_fn` bails before it renders. 21 of the Artificer pack's 52 assets, and every
      material in every un-extracted pack. The albedo is reachable — `material_albedo_texture` +
      the in-pack locator — so the fix is to key the render off the locator and pull the albedo
      through the same scratch the asset preview uses. Regression from the 2026-08-29 preview work,
      not a pre-existing gap.
- [ ] **The agent plane can tag but never untag.** `remove_tag` and `unstage` have no REST route
      and no MCP tool, so `AgentWrite::RemoveTag`/`UnstageAsset` are refused by the forwarding
      bridge ([ADR-0011](decisions/ADR-0011-headless-mcp-forwards-writes.md)). That asymmetry
      matters now that an agent can write tags: a wrong label can only be removed in the app.
      Adding the tool requires adding the route first — MCP↔REST 1:1.
- [ ] **`.meta` `labels:` → seed tags.** Parsed by `unitypackage::parse_meta`, never written to
      `tag`/`asset_tag`. INGESTION §2 lists them as seed tags. **Surveyed 2026-08-29 before
      building, and deliberately not built yet:** the reference library carries essentially
      none — **0 of 3,684** on-disk `.meta` sidecars have a `labels:` key, and **1 of ~3,400**
      sampled in-pack `asset.meta` entries across ten packs (four of them real 3D art packs),
      that one being DOTween's package-root marker carrying Asset Store *listing keywords*
      (`Tween, HOTween, iTween, LeanTween`) rather than anything describing an asset. The
      harvest is small and correct; it simply has no input here. Build it when a library's
      packs come from publishers who label, or once assets are labelled by hand. The real
      tagging gap is elsewhere: 7,667 of 8,237 assets are still Unidentified.
- [ ] **Smart collections / `embedding_meta`.** DDL-only, never written.
- [x] **Trait-vocabulary teaching — stale card, corrected 2026-08-28.** It *is* wired:
      `writer.rs` reads `teaching_traits()` into `ImageZeroShot`, and `ollama.rs` tests that they
      enter the prompt only when in scope.
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
- [x] **Storage — done 2026-08-29, and it was worse than the card said.** `cache_extracted`,
      `preview_cache_cap_gb` and `auto_purge_preview_cache` were not merely unenforced: all three
      were loaded from config, written back to config, and read by **nothing**. The workspace had
      no collector at all, so it held **3.55 GB across 1,179 folders that no catalog row
      referenced** on the reference machine. Now: `storage::plan_reclaim` + a **Reclaim** button
      that reports before it deletes and logs what went (`reclaimed.log`), the preview cap
      enforced at writer start, and `package.keep_cached` (M23) marking a pack's bytes worth
      keeping — offered in Pack stats §15, on the Packages row, and in Settings ▸ Storage §11.6.
      A pack whose archive has gone is never reclaimed and offers **Retarget…** instead.
      *Still open:* `unmaterialize_package` remains without a production caller — evicting a
      *referenced* pack (as opposed to collecting unreferenced bytes) is the next slice, and it is
      what `keep_cached` will gate.
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
- [x] **The sound card's ▶ plays (2026-08-22).** It was a glyph in an `aria-hidden` wrapper with
      no handler — clicking it selected the card, and a screen reader could not reach it — across
      **1,841 of 7,691 assets**. Now a real control over the artwork (so it is there whether the
      thumb route drew a real envelope or the hashed bars stood in), with the scrubber + `pos / dur`
      readout on the active card only, EQ animation on the bars while playing, and reduced-motion
      dropping the movement but keeping the lit colour. One `audio.svelte.ts` player owns the single
      `<audio>` element, which is what makes §13.2's "one asset plays at a time" true by
      construction: starting another sound re-points the one element, so the previous card stops
      because it lost the player, not because something remembered to stop it. A pull that fails
      (in-pack over the size cap) says so on the card instead of leaving a ▶ that never starts.
      `GridItem.duration_ms` came with it, so a card prints its length before the first play and a
      list row shows clip length where geometry shows triangles.
      e2e asserts `currentTime` advancing, not that a ◼ rendered — a transport that draws the right
      glyph and plays silence looks identical to one that works.
      **The Inspector's transport followed (§13.2, same day):** round ▶/◼ on the same one element,
      a static waveform in the hero that animates while playing (§13.1), and the scrubber appearing
      **only after first play** with *"play to scrub the timeline"* before it — a timeline you
      cannot scrub yet is a control that lies about what it does, which is the half worth a check.
      §13.2's *"switching assets/screens stops playback"* is implemented for **screens** only:
      leaving the Library with a sound running is audio with no visible transport, but stopping on
      every *selection* change would contradict §2.5's "while that card is the active audio" and
      make the grid transport useless for auditioning several sounds in a row. The one-element
      player already guarantees the "one at a time" the sentence is really about.
- [x] **Doc drift: card status rail width (2026-08-22).** UI-SPEC §2.5 said 3px against
      DESIGN-GUIDELINES §6's 1px, and the app renders 1px. DG wins on pixel details
      (`docs/design/README.md` precedence, ALIGNMENT-AUDIT F1), so UI-SPEC was the stale side and
      now says 1px. The comment in `Card.svelte` that recorded this as an open divergence says the
      two agree instead — leaving it would have told the next reader the code diverges when it
      conforms.

**Structural audit (2026-08-14)** — answering "too much code per file, too many tests":

- [x] **Pre-plugin project review (2026-08-22).** Asked for before starting the Unity plugin.
      **Workspace is clippy-clean** — 20 warnings down to 0. Most were mechanical (`sort_by_key`,
      `&mut Vec` → `&mut [_]`, `let…else` → `?`). Two were mine from the catalog split. One was a
      small lie in the code: a binding named `len` holding `()` under a comment about counting
      cache entries — `remove_dir_all` returns `Result<()>`, so it never counted anything and read
      as a tally. The two 8-argument writer functions keep an `#[allow]` **with the reason**: every
      argument is a distinct capability the writer owns, and a context struct would rename the
      problem rather than remove it.
      **The readiness answer:** the plugin's contract is in better shape than the board said, and
      the one real gap was the wire itself — now closed (see the staging-commit card). Discovery was
      verified against the running app: `engine.json` is written where §1 says, with the documented
      fields. ENGINE-API §1 amended where it overstated the guarantee — the `0600` is Unix-only and
      on Windows the protection is the per-user `%LOCALAPPDATA%` ACL, which is adequate but is not
      the same promise, and anything running as this user can read the session token.
      **Left alone, deliberately:** `catalog/mod.rs` (5,310) and `library.rs` (3,452) are still the
      big two, and `TraitsScreen.svelte` (1,516) and `SettingsScreen.svelte` (1,491) lead the
      frontend. None of them blocks the plugin, and splitting on size alone is how a refactor
      becomes churn.

- [x] **`catalog.rs` split into a directory module (2026-08-22).** It had reached **10,312 lines**.
      Now `catalog/` — `mod.rs` 5,310, `tests_catalog.rs` 3,323, `tests_slices.rs` 673, `oplog.rs`
      482, `taxonomy.rs` 294, `graph.rs` 129, `collections.rs` 128, `excludes.rs` 64. **325 tests
      pass, the same 325 as before**; nothing but locations changed.
      **Two premises on the old card were wrong, and are worth correcting rather than quietly
      dropping.** "Touching a single query recompiles 9.4k lines" — Rust compiles a *crate* at a
      time, so splitting one file into modules of the same crate changes recompilation not at all.
      "re-runs 69 tests" — `cargo test` still runs every test in the crate. The real wins are
      navigation, merge surface, and being able to run `cargo test catalog::oplog::`.
      **The two large `impl Catalog` blocks stayed** (1,892 and 2,937 lines). The 94 functions in
      the second do not fall into clean domains — clustering them lands 36 in "other" — so splitting
      them is a judgement pass over each function, not the mechanical move this was, and a bad split
      is worse than none.
      **The first attempt was wrong and was reverted.** Slicing from "start of this block" to "start
      of the next" swept every `pub struct` sitting *between* two `impl` blocks into the wrong file:
      `CollectionRow` landed at the bottom of the test module and three exclude helpers landed in
      `graph.rs`. 22 compile errors, all of it. The second pass moves whole `impl` blocks found by
      **matching their own closing brace**, and leaves every type and free helper in `mod.rs` where
      more than one module can reach them.
      `check-docs.mjs` then caught 5 citations the move broke — all of which were **already** pointing
      at the wrong line before it (line 3482 of the old file was `Ok(out)`; 3598 was a `"Props"` test
      fixture). Line citations into a 10k-line file rot on every edit, so they now name the function
      and its module instead, which survives the next split too.
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

## ✅ Cross-feature audit — 2026-08-26 (pre-plugin)

A pass over the features **and the seams between them**, asked for before the Unity plugin starts.
Thirteen defects, every one of them silent: none showed as an error, a failing test or a wrong pixel in
a screenshot, and six were the same shape — *a rule stated in one doc and enforced in one place*,
so the second place that needed it never got it. Verification after: **cargo test 329 · clippy 0 ·
e2e 92/92 · svelte-check 0/0 · check-docs 36/0 · check-board 0**.

- [x] **`--font-mono` is not a token — four mono readouts were never JetBrains Mono.**
      `ConfirmDelete`, `DropZone`, `DuplicatesScreen` and `ImportDialog` all styled their mono
      rows `font-family: var(--font-mono, monospace)`. The token is **`--ff-mono`**; the fallback
      silently caught it, so paths and counts in four places rendered in whatever generic
      monospace the system offered. A misspelled custom property does not warn — CSS has no
      concept of one existing. The other 40-odd files got it right, which is why nobody looked.
- [x] **`F` could only ever add a favourite.** UI-SPEC §0.3 specifies a toggle with two toasts;
      `App.svelte` passed `true` unconditionally and always toasted "Added to Favorites". The
      card's ☆/★ tile and the context menu both toggled correctly, so the same gesture meant two
      different things depending on where you made it. Now reads the current value — and reads it
      from **what the key last asked for** when a write is still in flight, because
      `selectedDetail` refetches only after the write lands and one key-repeat outruns that.
- [x] **`Ctrl/Cmd-A`, `Delete` and `/` did nothing.** All three are in a keyboard map. `Ctrl-A` was
      in UI-SPEC §0.4; `Delete` and `/` were in DESIGN-GUIDELINES §9 and **not** in §0.4, which is
      the table anyone implementing the keyboard reads. The two docs disagreed about what the
      keyboard does and the app implemented neither answer. §0.4 now carries all three and DG §9
      points at it as the owner. `Ctrl-A` is deliberately **page-scoped**, a narrowing written into
      the spec: pagination arrived after that line, and one keystroke arming the bulk bar over
      eight thousand invisible rows is a footgun. `Delete` is lead-only and rides the §11.7
      confirm gate.
- [x] **Escape's third rung was missing.** §0.4's ladder is *close Quick Look → clear selection →
      stop audio*. The first two were wired. A card's ▶ plays without selecting anything, so with
      no selection to clear Escape did nothing and the only way to stop a sound was to find the
      card again — or leave the screen, which is what the route effect already did.
- [x] **The pick set outlived the grid it was picked from.** Nothing cleared `selectedIds` on a
      filter, query, sort or route change. Pick three cards, narrow the search — or walk to Trash
      and back — and the bulk bar still read `3 selected` with **+ Add tag** live, aimed at assets
      no longer on screen. It now resets with the page, on the same key that already reset it;
      the *lead* selection is deliberately untouched, since one asset being described is not the
      same as a bulk write being armed.
- [x] **Trashed assets stayed in collections and in both staging trays.** UI-SPEC §7 is explicit
      — *"trashed assets leave search, dup groups, and staging everywhere"* — and search and dup
      groups honoured it (both run through `live()`). Collections did not: `collection_members`
      and `list_collections`' count returned soft-deleted rows, so a collection card counted
      assets its strip would not draw, and `import_staged` would have copied a binned asset into
      a Unity project. The shell's own in-memory tray had the same hole. **Filtered on read at
      both ends, not purged on trash**: Trash is reversible, and a restore has to put the asset
      back in the collection and the tray it was picked into — both covered by a test that trashes
      and restores.
- [x] **The staging tray never re-read itself.** `staged()` was called once at startup, so the
      tray only changed when the tray was the thing clicked — trashing a staged asset left its
      tile sitting there. Now follows `dataVersion` like every other read. *(This is not the
      per-project tray of §16.2; the shell's shortlist is still one in-memory list — see the open
      half of the export-history card.)*
- [x] **The Inspector had no ⟡ Stage.** §13.10 puts Stage beside Import in that action row. The
      card had it, the list row had it, and the panel you open to decide about **one** asset did
      not — you had to go back to the grid and find the card again. §16.2 already assumed the
      pairing, offering Export to folder here "because a single pick should not have to be staged
      first". Wired through the same staging context the cards use, inert on an unreachable file
      for the same reason `⟡` is on the card.
- [x] **Two more tokens that resolve to nothing, found by sweeping every `var()` in the app.**
      `--ls-hover` is not a token, so `.ctx-menu button:hover` and the Trash row's `↩ Restore`
      had **no hover state at all** — the declaration is simply dropped, and a control that does
      not answer the pointer reads as a dead control. `--card` is set on the Library grid element
      (`style="--card: {cardPx}px"`) but the **Identify** grid only read it, with a `148px`
      fallback: the density segmented control worked everywhere the user could watch it change
      and silently did nothing on the one screen they leave running. Both fixed, and the sweep is
      now a permanent e2e check that walks the live stylesheets — the class of bug is invisible
      by construction, so it needed a machine to look.
- [x] **The app and the engine kept two different staging trays.** The shell's was
      `Library::staged: Vec<i64>`, in memory, project-agnostic, gone on exit. ENGINE-API §6's was
      `collection(is_staging=1)`, in the catalog, per-project. Two answers to "what is staged":
      `list_staged` returned `[]` while six picks sat in the tray on screen, and `import_staged`
      would have imported nothing.
      The reason the shell had its own is worth stating, because it was not carelessness. M5 made
      trays per-project and **deleted** the project-less one — correct on the reading that a
      shortlist means nothing without an editor to send it to, and false in practice: the Unity
      plugin does not exist, so *no* user has a registered project, and the agent-plane stage op
      refused with *"no active project registered — register one via the Connect wizard"* for
      everybody. An in-memory list was the only way the tray worked at all.
      **M20** adds `UNIQUE(is_staging) WHERE is_staging=1 AND project_id IS NULL` — which M5's
      index could not express, since SQLite treats NULLs as distinct in a UNIQUE index and so
      constrained nothing there. `Catalog::active_staging_tray_id` is now the one resolver both
      planes use, `stage`/`unstage`/`clear_staged` are writer `Op`s like every other mutation
      (ADR-0002), and the first `register_project` **adopts** the project-less tray with its picks
      rather than orphaning them — the same move M5's own data step made for the legacy tray.
      Picks survive a restart, which they never did. [ADR-0010](decisions/ADR-0010-one-staging-tray.md);
      UI-SPEC §16.2/§20.2, DATA-MODEL §9/§13 and ENGINE-API §6 updated in the same change.
      Mutation-checked on the adoption (break it and the picks are abandoned) and on the live-row
      filter.
- [x] **The Inspector would not say which pack an asset came from.** UI-SPEC §13.9's provenance
      row is `Source · <library · pack>` with the pack linking to its §15 stats. It named the
      library and stopped — so the panel you open to learn about an asset told you *less* about
      its origin than the card you opened it from, which has carried the link all along.
      `AssetDetail` gained `package`, and **PackStats moved from LibraryScreen to App**: the
      Inspector is App's child, and two copies of one modal driven by two pieces of state is how
      they come to disagree about whether one is open.
      **License stays absent**, and deliberately: it needs the license harvest to write
      `package.license`, and a provenance row that always reads "unknown" is not provenance.
- [x] **Sort stayed live over AI-ranked results.** §2.2 says it is disabled there, and it was not.
      `page_with_ranks` consults the ranks **only** under `SortBy::Relevance`, so picking Size or
      Name threw the ranking away while the cards kept their `✦ NN%` badges — percentages sitting
      in an order they no longer described, which is worse than no percentages.

### The e2e suite was hiding its own failures

Found while verifying the above: a run reported **30/87**, and 57 of those failures were one
message — `left the app at 2561×1370, not the 2560×1370 it started at`.

- [x] **The viewport guard and the restore disagreed by a pixel.** The ladder check's restore waits
      for `Math.abs(innerWidth - w) <= 1` and then returns satisfied; the per-check guard added
      alongside it compared **exactly**. One pixel of frame-vs-client rounding therefore passed the
      restore, failed the guard, and failed every one of the eighty checks after it. Both are `<= 1`
      now. The leak this guard exists to catch is a window left an entire responsive rung away.
- [x] **And the restore only corrected two of its four axes** — the real bug under that pixel.
      `Browser.setWindowBounds` reports **frame** geometry and applies **client** geometry; the
      corrective second pass handled `left`/`top` (added when this walked the window down the
      screen a title bar at a time) but not `width`/`height`. So each run put the window back one
      pixel wider, `tauri-plugin-window-state` persisted it, and the next run read *that* as its
      original and added another. A one-pixel bug with no upper bound — and the same mechanism,
      on the same call, as the drift already fixed for position. All four axes are corrected now.
- [x] **A check leaked an Undo toast into the next one.** The new `Delete` check ended with its
      undo toast still on screen; the trash-toast check that follows waits 1.5 s for a
      `toast-action` to decide whether its own click armed or committed, read the leftover as its
      own, skipped its confirming click and failed on a trash that never happened. Toasts are
      cross-check state exactly like window size, and this suite now treats them that way.

**Not defects, confirmed by the same pass** — recorded so the next audit does not re-derive them:
all 11 nav destinations render real screens (no stub is reachable); zero hard-coded hex in any
component, every colour a token (the failures above were *misspelled* tokens, not literals);
`:focus-visible` discipline holds everywhere, with every `outline: none` paired with the ring
shadow; Settings' cards match §11's order (MCP absent, which
§11.5 already marks interim); dup grouping and search already filter trashed rows; the unavailable
source chain (banner → facet → card plate → inert `⟡` with live `⤴` → Quick Look relink) is
complete; deleting a collection clears it from the active filter.

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
      `import_states()` read per reload (`mark_imported`, beside `mark_traits`) —
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
- [x] **The ladder check was shrinking the real app (found by the user, fixed 2026-08-22).**
      Reported as *"the entire layout keeps getting smaller despite the window size staying the
      same"*. D12’s check resizes the **real** window — nothing else makes media queries fire — and
      its restore was `setWindowBounds(originalBounds).catch(() => {})`. Two silent failures under
      that `catch`:
      **(a)** `getWindowBounds` reports a maximized window as geometry *plus*
      `windowState: "maximized"`, and `setWindowBounds` rejects that combination outright — so the
      restore threw and the window stayed at the ladder’s last rung, 900×800.
      **(b)** left/top do not round-trip at all: CDP **reports frame** coordinates and **applies
      client** coordinates, so writing back the `top` it just gave you moves the window down by one
      title bar (measured: 23 → 46) — every run, walking the bottom off the screen.
      The app under test shares the user’s config directory, so `tauri-plugin-window-state`
      persisted each shrink into the next launch. Fixed in two layers, both verified: the check
      restores by **state alone** when the window was not normal, corrects position by measuring
      where it actually landed, and now **asserts** the window came back; and the harness snapshots
      `.window-state.json` before launch and rewrites it after the app exits, so a run killed
      part-way — where no `finally` runs — still cannot touch the user’s geometry. Proven with a
      sentinel geometry that survived a full run, and with the state file byte-identical across
      consecutive runs.
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
- [x] **Card size badge is one short token again (2026-08-28).** It read `S · 412 KB`, which
      repeated the byte size already in the stat line two rows below and was wide enough to wrap
      onto a second row *over the thumbnail* at compact density. UI-SPEC §2.5 asks for the letter
      and only the letter; exact size is on hover, which is what the **list row** has always done
      — the card was the odd one out. `Card.svelte`'s own comment claimed size had "moved to the
      tier chip"; both rendered it.
- [x] **Icon sizes diverge** — closed by [ADR-0012](decisions/ADR-0012-one-icon-ladder.md); the
      same defect as the "Icon ladder" card above, filed twice on one board.
- [x] **Sort options — done 2026-08-29.** *Date added* (`asset.id`, which is insert order, so no
      timestamp column was needed) and *Status* (lifecycle rank, Unidentified first — not the code
      string, which would sort `confirmed` between `discovered` and `indexed`). Both
      mutation-checked in e2e; the first version of that check was dead, because page 0 of this
      library is all one status.
- [x] **Identify shimmer — done 2026-08-29.** 400ms ring in the status colour on the
      Unidentified → Identified edge only, driven off the status the card is handed (identification
      is a catalog write the grid re-reads; there is no event to hang it on). Found and fixed
      underneath it: the global `prefers-reduced-motion` block neutralised `transition-duration`
      only, so any keyframe animation would have run at full length for someone who asked for less
      motion.
      *(The other half of this card was stale, corrected 2026-08-28: there **is** a global
      `prefers-reduced-motion` block in `app.css` killing transitions and decorative transforms
      — it is not "one DropZone gate".)*

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
- [x] **B12e — `✓ in <project>` membership pill (§N1.5). Shipped as D10**, with all four states
      rather than one: in the active project, in N others, never, and `✓ older copy` for an import
      the source has changed under. The bulk read this card asked for is `Catalog::import_states`.
- [x] **B12f — resolved by reading the spec (2026-08-14).** Tab counts stay (UI-SPEC §0.3); the
      PINNED sidebar group stays (audit row C6). No code, no doc change — the mockup predates both.
- [x] **B12g — top bar (2026-08-14).** Health meter as `Identified n / total` over a 6px
      gold-gradient bar (§0.1.5); project chip `Name · Engine` (§0.1.6); Tags/AI toggle ahead of the
      search box per §0.1's order. **GPU name still open** — nothing in `lodestash-desktop`/`-app`
      exposes an execution provider or adapter name, so the pill would have to invent one (**B8**).
- [x] **B12h — rows, chips and pack links (star/⤤/stage in D6; the rest 2026-08-22).** D6 gave
      rows the star, `⤤` and stage, and D5 closed §N5.3's always-open rail. What its card still
      claimed and the code did not do: **the pack name was not a link anywhere in the grid** and
      **chips were not click-to-search**, both spelled out in §2.5 (*"pack name is a link → Pack
      stats §15"*, *"chips are click-to-search"*). On cards the pack was baked into a joined
      `pack · tris · size` string, so there was nothing to attach a link to — that string is why it
      was never built. Now: pack → Pack stats on cards **and** rows, chip → adds that trait to the
      ⚡ filter (not the search box, so it composes with what is already narrowed and is removable
      from the active-filter bar like every other axis, D2), clicking a lit chip clears it.
      `PackStats.svelte` was lifted out of `UnityAssetsScreen`, which is why "**any** pack-name link
      (cards, list rows, Packages, Stats)" had meant exactly one screen: the modal lived inside it.

> **The trap this pass paid for, in one line:** a hover-revealed control at an `opacity: 0` still
> takes clicks, so the 104px thumb pulled the action tray under the card's *centre* and ate the
> double-click that opens Quick Look — surfacing three checks later as the 3D viewer hanging. The
> e2e now asserts on geometry: *whatever sits at a card's centre must be the card.*

**Doc drift (docs name things that no longer exist — fix per CLAUDE.md "docs are source of truth"):**

- [x] **Two things that lied about themselves, deleted (2026-08-22).**
      **A dead button in the staging tray:** `Commit to project`, hardcoded `disabled`, sitting
      beside two that work — and explaining itself with *"commit rides the engine build — the
      Rust/egui collect-and-import pipeline owns the commit side (B9e)"*. That pipeline does not
      exist: there is no egui crate and no `lodestash` binary. `⇢ Add all N to project` **is** the
      commit. Unreferenced by UI-SPEC §16.2, by any e2e check and by any doc — so it went, rather
      than being kept as a third primary-looking action that never fires.
      **A dependency kept alive by its own tests:** `vectorize::rasterize()` had **zero production
      callers**. It existed so a renderer with no SVG support — the egui grid — could show a
      preview; the webview renders SVG natively. `resvg` (with `usvg` and `tiny-skia` behind it) was
      compiling into the shipped binary to serve two tests. Demoted to a **dev-dependency** with the
      function behind `cfg(test)` rather than deleted outright, because one of those tests earns its
      keep: *the generated SVG must survive a real SVG parser, not just look like markup*. Removing
      the rasteriser would remove the only thing checking that, and a preview that renders as
      nothing looks exactly like one that was never generated. Verified out of the graph with
      `cargo tree -e normal`. TECH-STACK updated per CLAUDE.md rule 5.

- [x] **Test review: why the app visibly shrinks, and a guard so it can only ever be one test
      (2026-08-22).** Reported three times from watching a run. Reviewed the whole suite for it:
      **exactly one** check resizes anything — D12's responsive ladder, because media queries cannot
      be driven any other way — and nothing else touches the viewport (the harness only *clears* an
      inherited emulation override). The root cause was already measured with Win32
      `GetWindowRect`: `Browser.setWindowBounds` resizes only the **WebView2 controller**, never the
      Tauri window, so the app renders as a small page pinned inside a full-size frame.
      Two changes. **Dwell time 4.0s → 0.8s**: the ladder waited a fixed 500ms per rung guessing at
      when the relayout had settled; it now waits on `window.innerWidth` reaching the rung's width,
      which is both faster and the actual signal. **A per-check viewport guard** in the `check()`
      wrapper, so all 83 get it: every check must leave the app at the size it found it, or it fails
      *by name* with both sizes. It restores before reporting, so one careless check cannot cascade
      into eighty unrelated failures — the exact shape that wasted two sessions here (a leaked app,
      then a swallowed restore), where the tell is a suite failing everywhere at once and passing in
      isolation.
      Verified by planting a check that resizes and does not restore: it was flagged by name, and
      the check after it still ran at the right size.

- [x] **The board now checks its own claims (2026-08-22).** `scripts/check-board.mjs`, beside
      `check-docs.mjs` and named in CLAUDE.md rule 9. A checkbox is a claim about the code, and this
      session found it rots in both directions: a **ticked** card citing a symbol since renamed, and
      an **open** card still asserting something is missing years after it shipped — the expensive
      one, because building a thing never touches the card that says it does not exist. Five cards
      were in that state; three sent work toward something already finished.
      The audit it automates: **353 cited symbols across 109 cards.** Ticked cards came back clean
      apart from D10 naming `imported_into_active_project()`, which its own follow-up replaced with
      `import_states()` — now corrected. All five open cards flagged for claiming absence were
      **verified accurate**: `unmaterialize_package` and `detect::is_content` have only test
      mentions, and `embedding_meta` / `variant_group_id` / `dominant_color` are genuinely never
      written.
      `KANBAN.md` carries a `<!-- check-board: historical -->` marker: its pre-B7 cards cite the
      deleted egui crate, so its 14 dead citations are history, reported as notes rather than
      failures. Deleting those cards would be tidier and would lose the record; leaving them
      unmarked made the checker cry wolf 14 times, which is how a checker stops being read.
      **Caught while writing it:** the parser matched **zero** cards — the repo is CRLF, `` is a
      line terminator, so a per-line `/^- \[([ x])\] (.*)$/` never matches and the script reports a
      clean board. A silent zero that reads exactly like success, in the tool built to catch exactly
      that.

- [x] **Doc drift, re-checked against the docs themselves (2026-08-22).** Six cards here claimed
      a doc still described the retired egui shell. **Five were wrong — the docs were fixed and the
      cards were never ticked**, which is the same failure as a card claiming code is built:
      `ROADMAP.md` says "supersedes the egui/wgpu shell" and "retired at the B7 cutover" throughout;
      `MILESTONES.md` carries an updated-2026-08 note pointing at this board, with the rating UI
      recorded as parked by design; `USER-GUIDE.md` has a dated status note that is accurate about
      what is live and what still needs a Unity install; `PACK-DIRECTORY-NAV.md` §1 reads **Shipped**
      against Svelte components and mentions egui only as history; `ALIGNMENT-AUDIT.md` scopes itself
      to the Svelte app in its header and notes the egui crate as deleted at B7 (its `Missing` rows
      are a dated audit record, not a claim about today — and the ones spot-checked, e.g. F3's AI
      confidence bars, are **still** missing). `DESIGN-GUIDELINES §5`'s "not yet folded into the
      sprite" list is gone.
      **One was real, and it was the doc nobody carded:** `TECH-STACK.md` §2 — the table that reads
      as *the current stack* — still named `three-d 0.18 on eframe's shared Glow` for 3D and `rodio`
      *for playback* with the waveform "drawn with egui painter", while §5.3 of the same file
      correctly documented the hand-rolled WebGL2 viewer. A doc contradicting itself is worse than
      one merely out of date: both halves look authoritative. §2 now names what ships and points at
      §5.3; the two eframe/Glow rows in the decision table are marked **superseded** rather than
      deleted, because they were real calls and a reader needs to know they were made and outgrown.
      Two `Cargo.toml` comments describing the egui binary as current went with it.

## 🔵 Engine-project / deferred

- [ ] **UPM C# Unity plugin.** handshake → register/heartbeat → browse → selective import w/ deps →
      main-thread `AssetDatabase` → ack. (Needs a Unity install to verify.)
- [ ] **Connect wizard plugin half.** App half done (Settings → Unity Editors); the plugin side
      registers/heartbeats.
- [~] **Staging-commit half + batch undo — the app half is done; the plugin half is not.**
      Reviewed 2026-08-22 before starting the plugin, and this card understated what exists:
      register → approval gate → enqueue → poll → **lease/visibility timeout** → ack → `import_record`
      → dup-guard on re-import is implemented and covered end to end. Batch undo shipped with the
      export history (M19) for folder exports; `undo_import_batch` already existed for project
      imports.
      What the review **added** is the layer the C# actually talks to: an HTTP-level test of
      `GET /v1/import-queue` and `POST /v1/import-queue/{id}/ack` asserting the wire field by field
      — query-parameter name, bare-array response, `request_id` / `job_id` / `dest_subfolder` /
      `assets[].id` / `assets[].file_url`, 401 for an unauthenticated poll, the empty re-poll inside
      the lease window, and `404 not_found` on a second ack. The state-level test proved the logic;
      none of it proved the shape, and the shape is what gets compiled into the plugin.
      **Remaining is genuinely the plugin:** the C# side. Original card text follows.
      Import enqueue → queue poll → ack → `import_record` →
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
