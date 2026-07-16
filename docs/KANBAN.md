# Lodestash — Kanban

The working board for building Lodestash. Cards are grouped Backlog → Ready → In Progress → Review →
Done; **move a card across columns as you work it** (per the `CLAUDE.md` rule). Each card names the
design doc that governs it — read that doc before implementing, and update it if the implementation
diverges.

Card tags: **[P0]**–**[P7]** = roadmap phase ([`ROADMAP.md`](ROADMAP.md)) · **[spike]** = decision to
resolve first · **[QoL]** = cross-cutting browser-UX lane (M1.5), layered on Phase 1.

---

## ✅ Done — Design & spec

- [x] Architecture + language decision ([`ARCHITECTURE.md`](ARCHITECTURE.md))
- [x] Ingestion / formats / viewers, Asset Store cache, archive memory model ([`INGESTION.md`](INGESTION.md))
- [x] AI tagging & embeddings, local↔API abstraction ([`AI-PIPELINE.md`](AI-PIPELINE.md))
- [x] Data model — schema, hybrid search, mapping library, variants/color, projects/imports ([`DATA-MODEL.md`](DATA-MODEL.md))
- [x] Identification & search — faceted schema + 3 search surfaces ([`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md))
- [x] MCP surface + Unity import round-trip ([`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md))
- [x] Engine API contract ([`ENGINE-API.md`](ENGINE-API.md))
- [x] Design system, icon sprite, living style guide ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md))
- [x] App-icon / favicon set (`design/branding/`)
- [x] AI-models-by-GPU guide ([`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md)) + user guide ([`USER-GUIDE.md`](USER-GUIDE.md))
- [x] Tech stack parts list ([`TECH-STACK.md`](TECH-STACK.md))
- [x] Private repo + design-conformance rule (`CLAUDE.md`)
- [x] **[P0][spike]** Stack choices resolved: `ort`, `axum`, `russimp`, `fpcalc` subprocess → [`TECH-STACK.md`](TECH-STACK.md) §7
- [x] **[P0]** Cargo workspace + crate skeletons (`lodestash-core/ingest/ai/mcp/api/ui/cli`) + `rust-toolchain.toml`, `rustfmt.toml`, `.clippy.toml`, CI workflow
- [x] **[P0]** SQLite (WAL) catalog: base schema + packages + projects/imports + mapping library tables + FTS5 (`asset_fts`) + migrations (M1–M3 via `user_version`) ([`DATA-MODEL.md`](DATA-MODEL.md))
- [x] **[P0]** Folder crawler + magic-byte detection + BLAKE3 hashing + `(size,mtime)` incremental diff ([`INGESTION.md`](INGESTION.md)) — hashing is streaming (constant memory on multi-GB files); `.anim` correctly left unclassified (Unity animation clip, not a sprite)
- [x] **[P0]** Image thumbnailing → on-disk cache (keyed `content_hash`+params, Lanczos3) — UI wired to it via an interim `(path,size,mtime)` key until the catalog provides `content_hash`
- [x] **[P0]** Spec/code review fixes (2026-07-01): background-thread crawl (no UI freeze on large trees, generation-guarded), window-based thumbnail texture eviction (bounded VRAM at 50k+), selection preserved by entry across filter/sort changes, M4 migration (`asset.rating`, `package.pipeline`, `project.pipeline` — [`DATA-MODEL.md`](DATA-MODEL.md) §12)
- [x] UI functional spec (2026-07-15): [`UI-SPEC.md`](UI-SPEC.md) — all screens/overlays specced from the design prototype and normalized against the owning docs; 11 prototype↔spec conflicts resolved (sidebar replaces nav rail, per-project staging, MCP permission groups, 7-type taxonomy, unresolved-dependency rows, trait columns on `tag`, import batches + undo) with deltas applied to DESIGN-GUIDELINES / DATA-MODEL / ENGINE-API / MCP-AND-ENGINES / AI-PIPELINE / README; plus the **Vocabulary screen** (Traits + Mapping tabs, UI-SPEC §9) and the **M5 migration ledger** ([`DATA-MODEL.md`](DATA-MODEL.md) §13 — exact DDL + data steps vs the shipped M1–M4 schema)
- [x] Design icon set imported + reconciled (2026-07-15): 35 SVGs + `icons.svg` sprite + Glyph Catalog in `docs/design/`; added missing `ls-type-sprite`/`ls-type-font`; [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §5 synced to reality (stroke 1.8, baked token hex, shipped/planned id split)
- [x] Spec review additions (2026-07-01): `find_similar`/`identify_asset`/`suggest_tags`/`cancel_job` on both planes ([`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §2, [`ENGINE-API.md`](ENGINE-API.md) §2), `pipeline_mismatch` import advisory ([`ENGINE-API.md`](ENGINE-API.md) §5.4), sprite/material/font embedding rows ([`AI-PIPELINE.md`](AI-PIPELINE.md) §3), ratings + pipeline in the query grammar ([`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md) §6), SQLite single-writer rule ([`ARCHITECTURE.md`](ARCHITECTURE.md) §5)

---

## ▶ Ready — Phase 0: Foundations (walking skeleton)

- [x] **[P0]** egui/eframe shell (dark JRPG theme); responsive thumbnail grid with image textures + typed placeholders; name search ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md)). **Renderer: Glow (OpenGL)** — wgpu caused resize jitter from swap-chain reconfiguration on Windows ([`TECH-STACK.md`](TECH-STACK.md) §7)
- [x] **[P0]** Exit: point at a folder of images → searchable-by-name grid ✅ **M0 Walking Skeleton complete**
- [x] **[P0]** Media-type category tab bar (All / 3D Models / Audio / Images / Textures / Materials / Fonts / Sprites) — in-memory filter, precursor to P3 faceted sidebar
- [x] **[P0]** Async thumbnail loading — rayon + `std::sync::mpsc` channel; **demand-based** (only visible rows ± lookahead spawn tasks); UI shows placeholders and fills textures as they arrive; main thread polls via `request_repaint_after` while pending (no repaint from worker threads — avoids resize double-frames)
- [x] **[P0]** Library persistence — `AppConfig` JSON at OS config dir; remembers last opened root and reopens it on startup; precursor to P2 full `import_source` catalog

---

## ▶ Ready — UI-SPEC shell alignment

> Bring the running app in line with [`UI-SPEC.md`](UI-SPEC.md) (the prototype-faithfulness gaps,
> highest impact first). Build to spec; divergences go back into the doc.

- [x] **[QoL]** Bundle Cinzel / Inter / JetBrains Mono into egui `FontDefinitions` (display / body / mono roles per [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §3; offline invariant — no runtime fetch). Biggest single fidelity win ([`UI-SPEC.md`](UI-SPEC.md) §0) — `fonts.rs`: Inter → Proportional, JetBrains Mono → Monospace, Cinzel → named `display` family (`fonts::display(size)` helper; wordmark uses it); OFL licenses bundled beside the TTFs
- [x] **[QoL]** Sidebar per [`UI-SPEC.md`](UI-SPEC.md) §0.2 — replaces the icon nav rail: 9 labeled destinations + warn badges, STATUS facet group, LIBRARIES facet group (absorbs the library pill tabs; §0.3 updated), dashed + Add source; Library row = reset-to-everything. Unbuilt destinations render the §12 stub screen. STATUS is a real single-select (`StatusFilter`); pre-catalog everything folds into Unidentified, so Identified/Confirmed match nothing until the catalog lands
- [x] **[QoL]** Top bar per [`UI-SPEC.md`](UI-SPEC.md) §0.1 — logo diamond + Cinzel wordmark, Tags/AI segmented search (Tags live with mode-colored focus border; AI stub until P4 — query doesn't live-filter, inline "arrives with P4" note), health meter (0-identified until the P3 status flow), project chip (stub popover: engine projects register via the P5 bridge), AI-profile pill (`✦ local` → Settings); background radial gradients (§0, baked 128px radial sprites). Search moved out of the toolbar; File menu kept (functional, additive to spec)
- [x] **[P2]** M5 migration ([`DATA-MODEL.md`](DATA-MODEL.md) §13) — per-project staging trays + legacy-tray data step (reassign to active project, else delete), `import_record.batch_id` + `legacy-<project>-<time>` backfill, `tag` trait columns, `asset_relationship` (final form), `import_source.blacklist` + `Catalog::adopt_config_blacklists` (app-driven one-time config import). Migrations now each run in their own transaction; 5 new tests incl. a v4-fixture path exercising the data steps
- [ ] **[QoL]** Library result-header row per [`UI-SPEC.md`](UI-SPEC.md) §2 — result label, pack/collection filter chips, sort select, hint text; Unidentified nudge banner

---

## 📋 Backlog

### Phase 1 — View everything
- [ ] **[P1]** 3D viewport (`three-d` on shared wgpu) + normalize→glTF cache (Assimp/FBX2glTF)
- [ ] **[P1]** Audio: `symphonia` decode + `rodio` playback + waveform
- [ ] **[P1]** Image/texture viewer: `image`+`image_dds`+`psd`; HDR/EXR tone-map; channel toggles; **pixel-perfect NN**
- [ ] **[P1]** Animated sprite/spritesheet playback; **material sphere** preview; **font** glyph-sheet
- [ ] **[P1]** Offscreen 3D thumbnails; audio waveform thumbnails; metadata extraction (technical facets)

### Phase 1.5 — Browser QoL (M1.5)
> Cross-cutting asset-browser UX layered on M1 — the plumbing that makes the viewer feel like a real
> tool. Cards marked *(design exists)* are already specced in
> [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7/§9 (build to spec); the rest were added to §7 in the
> same change as this lane.

- [x] **[QoL]** Grid virtualization — `ScrollArea::show_rows`, build only visible rows (fixes the non-virtualized M0 grid; required at 10k–100k) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7) *(design exists)*
- [x] **[QoL]** Asset detail / inspector pane — preview + stat block + traits + provenance/license + actions ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7, [`DATA-MODEL.md`](DATA-MODEL.md) §9) *(design exists)* — scaffold: click-to-select, right-side panel (name/type/size/path/mtime), keyboard nav (arrows), Space=Quick Look overlay, Esc=dismiss; traits/provenance/actions wired in when catalog is connected
- [ ] **[QoL]** Multi-select (click/Ctrl/Shift/marquee/Ctrl-A) + right-click context menu (Reveal, Open with, Copy path, Stage, Import, Tag, Trash) — the selection primitive P7 batch-ops needs ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [x] **[QoL]** View/sort toolbar — sort (name/date/size/type ±) · thumbnail-size zoom slider; persist in config ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7). Design-batch update (design/README.md §2–3): zoom slider → **Compact/Cozy/Large grid-size selector** (drives the same persisted `thumb_size`); added a **⚑ Filters** button + count badge opening a **content-filter popover** (Trait / Triangles / **File-size**), with an **S/M/L/XL size badge** on every card. Filter predicate + `size_tier_of` are pure & unit-tested. Trait/Triangles chips shown disabled until their data lands (P4 tags / P1 tri counts); File-size fully works.
- [x] **[QoL]** Browse views — **Flat / Folders** toggle (persisted) · Folders = breadcrumb + folder tiles + direct-child assets, Backspace = up, subtree search · **Library filter** dropdown + inspector "Library" row when >1 source ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7 "Browse views") — pre-catalog scaffold over scan roots; re-points at `import_source` in P2
- [ ] **[QoL]** OS drag-out (asset → Unity/Explorer/DCC) + drag-in to ingest (drop overlay) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7, [`INGESTION.md`](INGESTION.md) §1)
- [x] **[QoL]** Keyboard nav + **Quick Look** (arrows · Space=peek · Enter=open · Del=trash) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §9) — scaffolded with inspector pane above; Enter/Del deferred to P3 (needs catalog)
- [ ] **[QoL]** Activity / jobs panel — running scans/tagging/imports with progress + cancel + completion toasts (surfaces the core job engine) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7, [`ARCHITECTURE.md`](ARCHITECTURE.md)) *(design exists)*
- [ ] **[QoL]** Undo/redo for catalog mutations (tag/collection/mapping/trash); ingest + imports excluded ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [ ] **[QoL]** Trash & restore — surface `asset.deleted_at`; "remove from library" vs explicit "delete from disk" ([`DATA-MODEL.md`](DATA-MODEL.md) §8, [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [ ] **[QoL]** Sources manager — add/remove/rescan/toggle-watch multiple `import_source` roots + recent libraries (extends M0 persistence) ([`DATA-MODEL.md`](DATA-MODEL.md) §1, [`INGESTION.md`](INGESTION.md) §2) *(`LibraryProject` + named-save persistence done; a "Sources" panel now lists each source's absolute path with Remove/Reveal-in-Explorer, `+ Add source`, and the app-data locations (config.json, thumbnail cache) with Reveal — opened by clicking the project detail label in the top bar. A top-bar **File** menu switches between saved projects (current one checkmarked) and shows the current project's location(s). Still missing: rescan, toggle-watch, per-source asset counts/last-scanned time, Asset Store cache add, a proper recent-libraries submenu, and its own Settings/Preferences screen — those need the catalog + job engine.)*
- [~] **[QoL]** Settings / Preferences — Library, Appearance (density §4), AI provider, Storage (caches + **archive-workspace size cap & retention days** + clear actions), Engine (export path/port) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7). **In progress** (design/README.md §5): added a left **nav rail** (Library ⊞ / Settings ⚙; the Library glyph resets to All libraries + clears filters, design §4) and a scrollable **Settings screen**. *Functional*: **File-size labels** editor (editable boundaries + cap, preview track, reset; drives the badge/filter), **On-screen (grid) quality** (Low/Med/High → thumbnail render px; Quick Look stays full-detail), **First-run** toggle (`skip_onboarding`), and the **per-source blacklist** (§5.1): expandable folder/asset exclusion chips per source (⊘ folder = WARNING / ⊘ asset = DANGER, ✕ to remove), persisted (`LibraryProject.blacklist`, interim for `import_source.blacklist`) and **honoured by the crawler** (folder subtrees pruned, assets dropped; non-destructive) — shared `Exclude` type in `lodestash-core`, unit-tested (matching + crawl + config); docs updated ([`DATA-MODEL.md`](DATA-MODEL.md) §1, [`INGESTION.md`](INGESTION.md) §1). Plus the **first-run onboarding overlay** (§1): a centred "Point it at your hoard" modal over the dimmed library (gold dashed drop well + Open folder, quick-open of saved libraries/recents, "Don't show this next time" bound to the same `skip_onboarding` flag as the Settings toggle; suppressed on launch when set). *Scaffold cards (backends not built)*: **AI identify profile** ([`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md)), **MCP** enable/allow-list ([`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §2, keep MCP↔REST 1:1), **Storage caps**, **Engine bridge**.
- [ ] **[QoL]** Tag manager — rename/merge/delete library-wide + color/nesting + autocomplete ([`DATA-MODEL.md`](DATA-MODEL.md) §2)
- [ ] **[QoL]** Ratings — 1–5 stars on grid card + inspector (keys 1–5), rating filter chip + sort key ([`DATA-MODEL.md`](DATA-MODEL.md) §12); schema column shipped in M4
- [ ] **[QoL]** Duplicate-review queue — grouped keep/merge/trash, never auto-delete ([`DATA-MODEL.md`](DATA-MODEL.md) §5, [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [ ] **[QoL]** Asset **list-row view** — the view toggle's table mode: 32px zebra rows, sortable columns, type icon + stats + status gem ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7 "Asset list row") *(design exists; grid/folders shipped, list missing)*
- [ ] **[P3]** **Library health meter** — "8,420 / 10,000 Identified" thin gold bar in the top bar; the backlog-as-quest affordance ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §6) *(needs status flow, so P3)*

### Phase 2 — Unity ingestion (primary input)
- [x] **[P2]** `.unitypackage` untar (pure Rust) + `.meta` parse (GUID/type/**labels**); archive memory model (stream-once → manifest → LRU workspace **with retention**: size-cap-then-age eviction, keep-recent window default 7 days, persists across sessions — [`INGESTION.md`](INGESTION.md) §2) — `unitypackage.rs`: `read_manifest` (browse pass, `asset` payload size from the tar header only, never buffered) + `extract_to_workspace` (ingest pass, content-addressed by BLAKE3, idempotent by hash) + `parse_meta` (GUID/importer/labels); `workspace.rs`: pure `plan_eviction` (size-cap-then-age, retention window). UI now wires the crawl→manifest→extract path end to end: a `.unitypackage` found while crawling resolves its manifest on the background crawl thread and shows as a "Packages found" bar entry instead of an opaque file card; render-pipeline-variant wrappers (nested `.unitypackage` payloads, e.g. URP/Built-in) get a variant picker defaulting to URP (`nested_package_candidates`/`preferred_variant_index`); clicking Extract runs the (possibly two-level, for wrappers) extraction on a background thread and merges the results into the grid, namespaced under the package's display name so Folders view groups them by origin. Verified end to end against a real ~1GB wrapper package (551 extracted assets) plus a synthetic two-level regression test. *Deferred*: wiring into the job engine/catalog (no `package`/`asset` DB rows written yet — that's the "Packages model" card below), the persisted touch-log driving `plan_eviction`'s `last_touched` in a real run, and Settings UI for cap/retention days.
- [ ] **[P2]** **Asset Store Library** source: multi-root discovery (`ASSETSTORE_CACHE_PATH`→default→custom) + first-run validation; browse-without-unpack; batch unpack+ingest; watch
- [ ] **[P2]** Python sidecar (UnityPy) spawn-on-demand: compiled bundles / native blobs; texture/audio decode
- [ ] **[P2]** Packages model + license/attribution harvest; dedup (BLAKE3 + pHash/Chromaprint) + move/rename detection
- [ ] **[P2]** **Render-pipeline harvest** — detect Built-in/URP/HDRP per pack (shader GUIDs in `.mat`, embedded manifests) → `package.pipeline`; feeds the import `pipeline_mismatch` advisory ([`DATA-MODEL.md`](DATA-MODEL.md) §12, [`INGESTION.md`](INGESTION.md) §2, [`ENGINE-API.md`](ENGINE-API.md) §5.4)
- [ ] **[P2]** **Library Source label** — display `import_source.name` chip on asset cards and inspector whenever project has >1 source; "Library" filter facet in search; user-rename source in project settings *(interim shipped in M1.5 browse-views scaffold: inspector "Library" row + toolbar Library filter; card chip + rename remain)*
- [ ] **[P2]** **Asset relationship graph** — GUID-ref pass after ingest (`.meta` → `asset_relationship` rows); heuristic name-prefix pass for raw exports; inspector "Related" tab (inbound/outbound edges with Library Source); grid link-chain badge; `has_dependents`/`has_dependencies` search filters ([`DATA-MODEL.md`](DATA-MODEL.md) §10)
- [ ] **[P2]** Broader formats: Aseprite/Krita/PSD/Paint.NET, `.blend`, fonts, MaterialX
- [x] **[P2]** Non-content classification (scripts/DLLs/editor tools/shaders → `is_content=0`) — `detect::is_content` (ext table + `Editor`/`Gizmos` folder check + `.meta` importer rules: Mono/Plugin/AssemblyDefinitionImporter always non-content, Default/ScriptedImporter only when unclassified). *Deferred*: wiring the result into `asset.is_content`/`content_kind` at ingest time (needs the ingest pipeline's persist step, not yet built) and the "tool-only package" whole-package flag.

### Phase 3 — Search & identification (core purpose)
- [ ] **[P3]** FTS5 (BM25) + faceted filter sidebar; **Search by Tags**
- [ ] **[P3]** Local embeddings via `ort` (SigLIP 2 image; multiview 3D; CLAP audio) → `vec_asset`; `VectorIndex` seam
- [ ] **[P3]** Hybrid search (pre-filter → BM25 ⊕ KNN → RRF) + **query-understanding parser** (NL → `{semantic_text, filters}`); **Search with AI** ([`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md))
- [ ] **[P3]** Variants (collapse), color search (`dominant_color`); Identified/Unidentified status flow

### Phase 4 — Auto-tagging & mapping
- [ ] **[P4]** `Tagger`/`Embedder` traits; local taggers (WD v3, YAMNet/PANNs+CLAP, Whisper, Ollama VLM); **faceted identification schema** fill
- [ ] **[P4]** API tagger (Claude vision, `output_config.format` JSON schema, Batches + prompt caching) — per-modality `local|api`
- [ ] **[P4]** Tag normalization + confidence + accept/reject (→ confirmed); color-coded/nested tags
- [ ] **[P4]** Mapping library (taxonomy/category/alias/rule + apply engine + pinned overrides); starter taxonomy
- [ ] **[P4]** In-app model manager (download by GPU tier) ([`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md))

### Phase 5 — MCP & Unity retrieval
- [ ] **[P5]** Loopback HTTP/JSON API (`/v1/`, 127.0.0.1 + handshake token) over `lodestash-core` ([`ENGINE-API.md`](ENGINE-API.md))
- [ ] **[P5]** MCP server (`rmcp`, stdio): search/preview/tag/collection/export/import tools + `find_similar`/`identify_asset`/`suggest_tags`/`cancel_job`; previews as image blocks + `resource_link`
- [ ] **[P5]** Project registration + import queue + `import_record` provenance + dup-guard; staging shortlist
- [ ] **[P5]** Unity UPM plugin: handshake, register, browse, **selective import (asset + deps)**, license/“already imported” surfacing
- [ ] **[P5]** Exit: import into open Unity project from GUI **and** from Claude via MCP (export fallback when no editor)

### Phase 6 — More engines & polish
- [ ] **[P6]** Godot ingestion (`.tres/.tscn/.res` + resource graph) + EditorPlugin import
- [ ] **[P6]** Unreal ingestion (CUE4Parse sidecar) + Slate plugin
- [ ] **[P6]** Packaging/installer, auto-update, first-run setup; optional Slint/Tauri polish eval; LanceDB swap if >~1M vectors

### Phase 7 — Power features & insights (backlog)
- [ ] **[P7]** "More like this" / drag-in example search (cosine over `vec_asset`)
- [ ] **[P7]** Auto-collections (embedding clustering) + tag propagation
- [ ] **[P7]** Credits/attribution export (`CREDITS.md` from `import_record` + license)
- [ ] **[P7]** Project asset audit (reverse sync: used/unused/source/license/in-project dups)
- [ ] **[P7]** Batch ops; command palette (Ctrl-K); library insights dashboard; pack update diff; catalog snapshot
- [ ] **[P7] ★ Unity Account Sync (extra win)** — list owned (incl. not-downloaded) + **bulk download queue** (rate-limited) + update detection, via Unity login; opt-in, unofficial, keychain creds. Post-scaffolding bonus — see [`MILESTONES.md`](MILESTONES.md) ★ MX

### Cross-cutting — QA & testing ([`TECH-STACK.md`](TECH-STACK.md) §5.1)
- [x] **[P0]** Test scaffolding — `insta`/`proptest` wired with one real test each (`lodestash-ingest`: `insta` snapshot of the full extension→`MediaType` table; `lodestash-ui`: `proptest` on `keep_range`'s slice-bound invariant); `sort_indices` extracted from `rebuild_cache` and unit-tested (name/size/modified/type, asc+desc); `tests/corpus/` dir *(from the earlier review round)*. `criterion` deferred — `lodestash-ui` is bin-only (no lib target), so an external `benches/` harness can't reach the private pure fns (`sort_indices`, `folder_children`) it would bench; needs a lib+bin split first. Regression-corpus real samples (`.unitypackage`, `.fbx`, `.psd`, …) stay blocked on their P1/P2 parsers landing — nothing to snapshot yet.
- [ ] **[P2]** Regression corpus — commit small license-clean samples (`.unitypackage`, `.dds`, `.fbx`, `.wav`, `.psd`, material+textures) + `insta` parse snapshots; ingest detect/untar/`.meta` tests
- [ ] **[P2]** Relationship-harvest tests — GUID-pair resolution → expected edges; **cross-source edge** resolution; inbound/outbound traversal; dup-guard `UNIQUE(asset_id, project_id)`
- [ ] **[P3]** Search/mapping determinism — RRF fusion order (`proptest` invariant under rank perturbation); pre-filter hard-boolean; mapping rule priority + **pinned-override survives re-apply**
- [ ] **[P5]** **MCP↔REST 1:1 invariant test** — assert tool set == endpoint set (CI fails if one plane gains an op the other lacks); request/response schema snapshots; handshake-token rejection
- [ ] **[P6]** `criterion` perf gate (dev/nightly) — crawl throughput, `rebuild_cache` @100k rows, hybrid-search latency @100k vectors; tracks §3/§6 perf claims

---

## In Progress
*(empty — pull from Ready)*

## In Review
*(empty)*
