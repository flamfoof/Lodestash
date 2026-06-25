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
- [x] **[P0]** Folder crawler + magic-byte detection + BLAKE3 hashing + `(size,mtime)` incremental diff ([`INGESTION.md`](INGESTION.md))
- [x] **[P0]** Image thumbnailing → on-disk cache (keyed `content_hash`+params, Lanczos3)

---

## ▶ Ready — Phase 0: Foundations (walking skeleton)

- [x] **[P0]** egui/eframe shell on wgpu (dark JRPG theme); responsive thumbnail grid with image textures + typed placeholders; name search ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md))
- [x] **[P0]** Exit: point at a folder of images → searchable-by-name grid ✅ **M0 Walking Skeleton complete**
- [x] **[P0]** Media-type category tab bar (All / 3D Models / Audio / Images / Textures / Materials / Fonts / Sprites) — in-memory filter, precursor to P3 faceted sidebar
- [x] **[P0]** Async thumbnail loading — rayon + `std::sync::mpsc` channel; UI shows placeholders and fills in textures as they arrive; `ctx.request_repaint()` wakes frame
- [x] **[P0]** Library persistence — `AppConfig` JSON at OS config dir; remembers last opened root and reopens it on startup; precursor to P2 full `import_source` catalog

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
- [x] **[QoL]** View/sort toolbar — sort (name/date/size/type ±) · thumbnail-size zoom slider; persist in config ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [ ] **[QoL]** OS drag-out (asset → Unity/Explorer/DCC) + drag-in to ingest (drop overlay) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7, [`INGESTION.md`](INGESTION.md) §1)
- [x] **[QoL]** Keyboard nav + **Quick Look** (arrows · Space=peek · Enter=open · Del=trash) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §9) — scaffolded with inspector pane above; Enter/Del deferred to P3 (needs catalog)
- [ ] **[QoL]** Activity / jobs panel — running scans/tagging/imports with progress + cancel + completion toasts (surfaces the core job engine) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7, [`ARCHITECTURE.md`](ARCHITECTURE.md)) *(design exists)*
- [ ] **[QoL]** Undo/redo for catalog mutations (tag/collection/mapping/trash); ingest + imports excluded ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [ ] **[QoL]** Trash & restore — surface `asset.deleted_at`; "remove from library" vs explicit "delete from disk" ([`DATA-MODEL.md`](DATA-MODEL.md) §8, [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [ ] **[QoL]** Sources manager — add/remove/rescan/toggle-watch multiple `import_source` roots + recent libraries (extends M0 persistence) ([`DATA-MODEL.md`](DATA-MODEL.md) §1, [`INGESTION.md`](INGESTION.md) §2) *(`LibraryProject` + named-save persistence done; full manager UI remains)*
- [ ] **[QoL]** Settings / Preferences — Library, Appearance (density §4), AI provider, Storage (cache location/size/clear), Engine (export path/port) ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)
- [ ] **[QoL]** Tag manager — rename/merge/delete library-wide + color/nesting + autocomplete ([`DATA-MODEL.md`](DATA-MODEL.md) §2)
- [ ] **[QoL]** Duplicate-review queue — grouped keep/merge/trash, never auto-delete ([`DATA-MODEL.md`](DATA-MODEL.md) §5, [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7)

### Phase 2 — Unity ingestion (primary input)
- [ ] **[P2]** `.unitypackage` untar (pure Rust) + `.meta` parse (GUID/type/**labels**); archive memory model (stream-once → manifest → LRU workspace)
- [ ] **[P2]** **Asset Store Library** source: multi-root discovery (`ASSETSTORE_CACHE_PATH`→default→custom) + first-run validation; browse-without-unpack; batch unpack+ingest; watch
- [ ] **[P2]** Python sidecar (UnityPy) spawn-on-demand: compiled bundles / native blobs; texture/audio decode
- [ ] **[P2]** Packages model + license/attribution harvest; dedup (BLAKE3 + pHash/Chromaprint) + move/rename detection
- [ ] **[P2]** Broader formats: Aseprite/Krita/PSD/Paint.NET, `.blend`, fonts, MaterialX
- [ ] **[P2]** Non-content classification (scripts/DLLs/editor tools/shaders → `is_content=0`)

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
- [ ] **[P5]** MCP server (`rmcp`, stdio): search/preview/tag/collection/export/import tools; previews as image blocks + `resource_link`
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

---

## In Progress
*(empty — pull from Ready)*

## In Review
*(empty)*
