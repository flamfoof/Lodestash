# Roadmap & Build Plan

A phased plan that prioritizes the **main purpose first** — local identification and retrieval — and
layers richer features and engine integration on top.

Principle: **the search/retrieve loop must work end-to-end before polishing any single subsystem.**

---

## Phase 0 — Foundations (walking skeleton)
**Goal:** a Rust workspace that can scan a folder of plain files and show them in a grid.

- Cargo workspace + crate layout (`lodestash-core/ingest/ai/mcp/api/ui/cli`).
- SQLite (WAL) catalog with the base schema; `import_source`, `asset`, `asset_file`, `media_type`.
- Folder crawler + magic-byte detection + BLAKE3 hashing; incremental `(size, mtime)` diff.
- **Tauri 2 + Svelte 5 shell** (WebView2) — supersedes the egui/wgpu shell (B2a–B7, 2026-08); virtualized thumbnail grid with placeholders.
- Image thumbnailing (the easy modality) into the on-disk cache.

**Exit criteria:** point at a folder of images → they appear in a searchable-by-name grid.

---

## Phase 1 — View everything
**Goal:** all three media types preview/play.

- 3D viewport (hand-rolled WebGL2 `viewer3d.ts`, TECH-STACK §5.3) — the egui `three-d`/wgpu viewer and normalize-to-glTF were retired at the B7 cutover; previews are SVG/proxy based.
- Audio: `symphonia` decode + `rodio` playback + waveform.
- Image/texture viewer: `image` + `image_dds` (BCn) + `psd`; HDR/EXR tone-map; channel toggles.
- Offscreen 3D thumbnail render-to-texture; audio waveform thumbnails.
- Metadata extraction (tris/verts/materials/bones; dims/colorspace; duration/rate).
- **2D / material / font previews**: animated sprite + spritesheet playback; **pixel-perfect**
  (nearest-neighbor) rendering; **material sphere** preview; **font** glyph-sheet.
- **Browser QoL (M1.5)** — cross-cutting asset-browser UX layered on this phase: virtualized grid,
  detail/inspector pane, multi-select + context menu, view/sort + thumbnail-zoom, drag in/out, Quick
  Look, jobs/activity panel, undo/redo, trash, sources manager, settings, tag manager, dup-review.
  Tracked as the **[QoL]** lane in [`KANBAN.md`](KANBAN.md) / **M1.5** in
  [`MILESTONES.md`](MILESTONES.md); UI specced in [`DESIGN-GUIDELINES.md`](DESIGN-GUIDELINES.md) §7/§9.

**Exit criteria:** view models, play audio, view images/textures — all from the grid.

---

## Phase 2 — Unity ingestion (the primary input)
**Goal:** import real Unity libraries.

- `.unitypackage` untar (pure Rust gzip+tar); `.meta` YAML parse → GUID, importer type, **labels**
  (seed the tag index from existing labels).
- **Asset Store Library source** (first-class): auto-discover the download cache
  (`ASSETSTORE_CACHE_PATH` → per-OS default → custom root), browse the full owned library by
  publisher/category **without unpacking**, and **unpack + ingest one or many packages on demand**
  (batch job); model each as a `package`, link assets to it, watch the cache for new downloads.
  **Harvest license/attribution** (cache tree + embedded pack metadata) into the `package` row.
- **Python sidecar (UnityPy)**: bundle + spawn-on-demand for compiled bundles / native
  `Texture2D`/`Mesh`/`AudioClip` blobs; `texture2ddecoder`/`decrunch` decode; `fsb5` audio.
- Routing: raw source → direct loader; Unity-native/bundle → sidecar.
- Dedup (BLAKE3 exact groups; pHash/Chromaprint near-dup review queue); move/rename detection.
- **Broader source formats**: Aseprite/Krita/PSD/Paint.NET (2D), `.blend`, fonts, MaterialX — see
  [`INGESTION.md`](INGESTION.md) §4.

**Exit criteria:** auto-discover the Asset Store download cache, browse the owned library, and
**selectively unpack one or many packages**; import a standalone `.unitypackage` / folder too; assets
viewable + deduped, with imported Unity labels already searchable.

---

## Phase 3 — Search & identification (the core purpose)
**Goal:** find the right asset fast.

- FTS5 (BM25) over name/description/tags; faceted filter sidebar (type/source/tags/category).
- Local embeddings via **`ort`** (SigLIP 2 image; multiview→SigLIP 3D; CLAP audio); `vec_asset`
  (sqlite-vec) with the `VectorIndex` seam.
- Hybrid search: pre-filter → BM25 ⊕ vector KNN → **RRF**; semantic toggle in the search bar.
- **Variants** (collapse near-duplicate assets into one tile) and **color search** (`dominant_color`)
  as additional facets.
- **Two search surfaces + query understanding**: Search by Tags (structural) and Search with AI
  (NL → `{semantic_text, filters}` parser → hybrid). See
  [`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md).
- Background embed jobs keyed by `content_hash + model + version`; `embedding_dirty` flow.

**Exit criteria:** keyword + tag-filter + natural-language semantic search all return good results on
a 10k+ library.

---

## Phase 4 — Auto-tagging & mapping library
**Goal:** identification quality + clean organization.

- `Tagger`/`Embedder` traits; **local** taggers (WD v3 image; YAMNet/PANNs + CLAP audio; Whisper
  speech; Ollama VLM captions) and the **API** tagger (Claude vision, JSON-schema, Batches + caching).
- Per-modality `local | api` config; embeddings stay local.
- Tag normalization + confidence thresholds; editable confidence chips with accept/reject logging;
  **color-coded / nested tags** (`tag.color` / `tag.parent_id`).
- **Faceted identification schema** (subject/style/material/mood/usage/…) filled by a local VLM or
  Claude vision via structured outputs — see [`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md) §2, §4.
- **Mapping library**: taxonomy/category/alias/rule schema + apply engine + pinned overrides + re-map;
  category as a search facet. Ship a starter taxonomy (Environment/Props/Characters/SFX/Music/…).

**Exit criteria:** auto-tag a library locally or via API; map raw tags onto a canonical vocabulary;
filter/search by category.

---

## Phase 5 — MCP & Unity retrieval (use it in the engine)
**Goal:** pull assets into Unity and via AI — the closed round-trip.

- Loopback **HTTP/JSON API** (`/v1/`, 127.0.0.1 + token via handshake file) over `lodestash-core` — the
  full contract in [`ENGINE-API.md`](ENGINE-API.md); endpoints **1:1 with the MCP surface**.
- **MCP server** (`rmcp`, stdio): the §2 surface in [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md);
  previews as `image` blocks + `resource_link`s; `export_asset` with sandboxed paths; `get_job_status`.
- **Project registration + import queue**: `register_project`, `import_to_project`, and the long-poll
  `import-queue` the plugin drains on Unity's **main thread**, acking the in-project path+GUID →
  **`import_record`** provenance + **dup-guard** ("already imported?").
- **Staging shortlist** shared across GUI / MCP / panel (`stage_asset` / `import_staged`; the
  `is_staging` collection).
- **Unity plugin** (UPM C# `EditorWindow`): read the handshake file, register the project, browse over
  REST, import into `Assets/` via `AssetDatabase`; surface license/attribution and "already imported".
- **Unity-side MCP-client** path documented (Claude Code in the project folder drives `import_to_project`).
- Job system for long ops; surface versioning + `get_server_info`.
- *(Unreal plugin is deferred to Phase 6.)*

**Exit criteria:** from Claude/Claude Code, search the catalog and **import an asset into the open
Unity project** via `import_to_project` (with `export_asset` fallback when no editor is registered);
from the Unity editor window, browse, stage, and import — with provenance recorded and re-imports
dup-guarded.

---

## Phase 6 — More engines & polish (Godot + Unreal — both best-effort, priority TBD)

> Godot and Unreal are **co-equal later-phase** targets; final ordering decided then (Godot suits the
> indie/2D audience, Unreal the AAA one). Both are thin REST clients over the same engine API
> ([`ENGINE-API.md`](ENGINE-API.md)).

- **Godot ingestion + plugin**: parse `.tres`/`.tscn`/`.res`, walk the resource graph for the
  dependency closure, and import via an EditorPlugin dock (drag-drop into the FileSystem with folder
  reconstruction).
- **Unreal ingestion**: CUE4Parse sidecar for `.pak`/IoStore extraction (best-effort; AES/`.usmap` are
  the user's responsibility).
- **Unreal editor plugin**: Slate tab (browse+import into `/Content`).
- Optional UI-polish track: evaluate **Slint** (same wgpu architecture) or **Tauri+three.js** if visual
  fidelity becomes a priority.
- Optional **OpenShape** native-3D similarity (Python sidecar) for shape search.
- Cross-encoder rerank toggle; LanceDB swap if a library exceeds ~500k–1M vectors.
- Packaging/installer; auto-update; first-run model download/management.

---

## Phase 7 — Power features & insights (backlog)

Higher-leverage features that build directly on the architecture already in place:

- **"More like this" / example search** — one-click similar assets via cosine over `vec_asset`; **drag
  an external image/audio/model in** → embed → find owned assets like it. (Reuses the `VectorIndex`.)
- **Auto-collections & tag propagation** — cluster embeddings to *propose* collections; propagate a
  Confirmed exemplar's tags to nearby Unidentified assets. (embeddings + collections + `asset_tag`.)
- **Credits / attribution export** — generate a `CREDITS.md` for a project from `import_record` +
  `license`/`attribution`, so shipping with bought assets stays compliant. (Builds on the engine
  round-trip + license model.)
- **Project asset audit (reverse sync)** — scan an open Unity project, match assets back to the catalog
  by GUID/hash, and report **used / unused / source pack / license / in-project duplicates**. (Unity
  plugin + `import_record` + hashes; a new REST/MCP op.)
- **Batch operations** — multi-select batch tag / map / identify / export / import / stage, through the
  job engine.
- **Command palette (Ctrl-K) + keyboard-first** — fast search/nav/actions for a dense power tool.
- **Library insights dashboard** — a home view: % Identified, untagged, duplicates, **unlicensed**,
  by-type, largest packs, "where used" — extends the design's library-health meter.
- **In-app model manager** — download/manage local AI models by GPU tier from within the app (see
  [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md)).
- **Pack update diff** — when a pack's `content_hash` changes, show added/removed/modified assets.
- **Catalog snapshot** — export/import the SQLite catalog (not the assets) for backup or to share a
  taxonomy / mapping library.
- **★ Unity Account Sync (extra win)** — use the Unity login to list owned (incl. not-downloaded) packs,
  **bulk-download** them (rate-limited queue) into the cache, and detect updates; opt-in, unofficial,
  keychain-stored creds. Post-scaffolding bonus — full scope in [`MILESTONES.md`](MILESTONES.md) ★ MX.

---

## Tech stack summary

| Layer | Primary | Notes / alternatives |
|---|---|---|
| Language (core) | **Rust** | + optional bundled **Python sidecar** (UnityPy; later CUE4Parse/PyTorch) |
| GUI | **Tauri 2 + Svelte 5** (shipped B2a–B7) | Slint 1.12 (polish) — the egui/wgpu shell and the Tauri fallback are history |
| 3D | **WebGL2 viewer3d.ts** (TECH-STACK §5.3) | normalize→glTF (Assimp/FBX2glTF) — egui three-d retired |
| Audio | **symphonia** + **rodio** | kira for advanced scrubbing |
| Images | **image** + **image_dds** + **psd** | + texture2ddecoder/decrunch (Unity-compressed) |
| DB | **SQLite (WAL)** + **FTS5** + **sqlite-vec** | LanceDB upgrade seam |
| Hashing | **BLAKE3** + pHash/blockhash + Chromaprint | — |
| AI runtime | **ort (ONNX Runtime)** + **Ollama** (HTTP) + cloud API | candle/FastEmbed alt; whisper-rs |
| Image/3D embed | **SigLIP 2** (multiview for 3D) | OpenShape later for 3D similarity |
| Audio embed/tag | **CLAP** + YAMNet/PANNs + Whisper | — |
| Image tags | **WD v3** (ONNX) | Ollama VLM captions; Claude vision (API) |
| MCP | **rmcp** (stdio) | optional Streamable HTTP |
| Engine API | loopback **HTTP/JSON** (token) | — |
| Plugins | **Unity UPM C#** (primary); Godot + Unreal (planned, co-equal) | thin REST clients |

---

## Key risks & mitigations

| Risk | Mitigation |
|---|---|
| Unity format/version churn breaks deep extraction | Prefer raw source files; isolate deep parsing in the UnityPy sidecar; keep a regression corpus of real packs; pin versions. |
| Compressed-texture long tail (rare ASTC/Crunch) fails | "Unsupported format" fallback; don't promise 100%. |
| 3D tagging is shallow (category only) | Pair multiview→SigLIP with file-derived structured tags; OpenShape later. |
| PBR map channels invisible to vision models | Tag albedo; classify other maps by filename/hue; group as material. |
| FBX is proprietary | ufbx parses FBX in-process; decimate → cached preview proxies (render3d/decimate, 2026-08-01). glTF normalization was the egui path, retired at B7. |
| Windows packaging pain | Rust single static binary sidesteps it; sidecar bundled, optional. |
| Vector search outgrows brute force | `VectorIndex` trait → swap sqlite-vec for LanceDB past ~500k–1M. |
| Cloud-AI privacy concerns | Local-first default; API strictly opt-in, per-modality. |
| Editor integration fragility (main thread, experimental Unreal MCP) | MCP = AI plane, REST = engine plane; browse+import scope; version checks + graceful degrade. |
| Wwise audio / shipped-game extraction | Out of initial scope; positioned around owned packs/projects. |

---

## Definition of "done" for v1

A user can: add their folders and `.unitypackage`/Asset-Store packs → view models, play audio, view
textures → search by keyword, tag-filter, and natural language → auto-tag locally (or via Claude) and
map onto a clean taxonomy → **stage candidates** → and **pull a chosen asset into an open Unity
project** — both from the desktop UI and from Claude via MCP (`import_to_project`, landing in the open
editor via the import queue), with **provenance recorded, re-imports dup-guarded, and
license/attribution surfaced** — entirely on their own PC. (Unreal support is a later-phase addition;
v1 is Unity-focused.)
