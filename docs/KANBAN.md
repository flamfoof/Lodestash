# Lodestash — Kanban

The working board for building Lodestash. Cards are grouped Backlog → Ready → In Progress → Review →
Done; **move a card across columns as you work it** (per the `CLAUDE.md` rule). Each card names the
design doc that governs it — read that doc before implementing, and update it if the implementation
diverges.

Card tags: **[P0]**–**[P7]** = roadmap phase ([`ROADMAP.md`](ROADMAP.md)) · **[spike]** = decision to
resolve first.

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

---

## ▶ Ready — Phase 0: Foundations (walking skeleton)

- [ ] **[P0][spike]** Resolve open stack choices: `ort` vs `candle`, `axum` vs `hyper`, Assimp vs FBX2glTF, Chromaprint binding vs `fpcalc` → update [`TECH-STACK.md`](TECH-STACK.md)
- [ ] **[P0]** Cargo workspace + crate skeletons (`lodestash-core/ingest/ai/mcp/api/ui/cli`) + `rust-toolchain.toml`, clippy/rustfmt, CI
- [ ] **[P0]** SQLite (WAL) catalog: base schema (`import_source`, `media_type`, `asset`, `asset_file`) + migrations ([`DATA-MODEL.md`](DATA-MODEL.md))
- [ ] **[P0]** Folder crawler + magic-byte detection + BLAKE3 hashing + `(size,mtime)` incremental diff ([`INGESTION.md`](INGESTION.md))
- [ ] **[P0]** egui/eframe shell on wgpu; virtualized thumbnail grid with placeholders ([`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md))
- [ ] **[P0]** Image thumbnailing → on-disk cache (keyed `content_hash`+params)
- [ ] **[P0]** Exit: point at a folder of images → searchable-by-name grid

---

## 📋 Backlog

### Phase 1 — View everything
- [ ] **[P1]** 3D viewport (`three-d` on shared wgpu) + normalize→glTF cache (Assimp/FBX2glTF)
- [ ] **[P1]** Audio: `symphonia` decode + `rodio` playback + waveform
- [ ] **[P1]** Image/texture viewer: `image`+`image_dds`+`psd`; HDR/EXR tone-map; channel toggles; **pixel-perfect NN**
- [ ] **[P1]** Animated sprite/spritesheet playback; **material sphere** preview; **font** glyph-sheet
- [ ] **[P1]** Offscreen 3D thumbnails; audio waveform thumbnails; metadata extraction (technical facets)

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
