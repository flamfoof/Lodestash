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
- egui/eframe shell on wgpu; virtualized thumbnail grid with placeholders.
- Image thumbnailing (the easy modality) into the on-disk cache.

**Exit criteria:** point at a folder of images → they appear in a searchable-by-name grid.

---

## Phase 1 — View everything
**Goal:** all three media types preview/play.

- 3D viewport (`three-d` on shared wgpu) + **normalize-to-glTF on import** (Assimp/FBX2glTF cache).
- Audio: `symphonia` decode + `rodio` playback + waveform.
- Image/texture viewer: `image` + `image_dds` (BCn) + `psd`; HDR/EXR tone-map; channel toggles.
- Offscreen 3D thumbnail render-to-texture; audio waveform thumbnails.
- Metadata extraction (tris/verts/materials/bones; dims/colorspace; duration/rate).

**Exit criteria:** view models, play audio, view images/textures — all from the grid.

---

## Phase 2 — Unity ingestion (the real input)
**Goal:** import real Unity libraries.

- `.unitypackage` untar (pure Rust gzip+tar); `.meta` YAML parse → GUID, importer type, **labels**
  (seed the tag index from existing labels).
- **Asset Store Library source** (first-class): auto-discover the download cache
  (`ASSETSTORE_CACHE_PATH` → per-OS default → custom root), browse the full owned library by
  publisher/category **without unpacking**, and **unpack + ingest one or many packages on demand**
  (batch job); model each as a `package`, link assets to it, watch the cache for new downloads.
- **Python sidecar (UnityPy)**: bundle + spawn-on-demand for compiled bundles / native
  `Texture2D`/`Mesh`/`AudioClip` blobs; `texture2ddecoder`/`decrunch` decode; `fsb5` audio.
- Routing: raw source → direct loader; Unity-native/bundle → sidecar.
- Dedup (BLAKE3 exact groups; pHash/Chromaprint near-dup review queue); move/rename detection.

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
- Background embed jobs keyed by `content_hash + model + version`; `embedding_dirty` flow.

**Exit criteria:** keyword + tag-filter + natural-language semantic search all return good results on
a 10k+ library.

---

## Phase 4 — Auto-tagging & mapping library
**Goal:** identification quality + clean organization.

- `Tagger`/`Embedder` traits; **local** taggers (WD v3 image; YAMNet/PANNs + CLAP audio; Whisper
  speech; Ollama VLM captions) and the **API** tagger (Claude vision, JSON-schema, Batches + caching).
- Per-modality `local | api` config; embeddings stay local.
- Tag normalization + confidence thresholds; editable confidence chips with accept/reject logging.
- **Mapping library**: taxonomy/category/alias/rule schema + apply engine + pinned overrides + re-map;
  category as a search facet. Ship a starter taxonomy (Environment/Props/Characters/SFX/Music/…).

**Exit criteria:** auto-tag a library locally or via API; map raw tags onto a canonical vocabulary;
filter/search by category.

---

## Phase 5 — MCP & engine retrieval (use it in the engine)
**Goal:** pull assets into Unity/Unreal and via AI.

- Loopback **HTTP/JSON API** (`/v1/`, 127.0.0.1 + per-session token) over `lodestash-core`.
- **MCP server** (`rmcp`, stdio): the §2 surface in [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md);
  previews as `image` blocks + `resource_link`s; `export_asset` with sandboxed paths; `get_job_status`.
- **Unity plugin** (UPM C# `EditorWindow`): browse over REST, import into `Assets/` via
  `AssetDatabase`.
- Job system for long ops; surface versioning + `get_server_info`.

**Exit criteria:** from Claude/Claude Code, search the catalog and export an asset; from the Unity
editor window, browse and import an asset into the open project.

---

## Phase 6 — Unreal & polish (best-effort + refinement)
- **Unreal**: CUE4Parse sidecar for `.pak`/IoStore extraction (best-effort; AES/`.usmap` are the
  user's responsibility); Unreal editor Slate plugin (browse+import into `/Content`).
- Optional UI-polish track: evaluate **Slint** (same wgpu architecture) or **Tauri+three.js** if visual
  fidelity becomes a priority.
- Optional **OpenShape** native-3D similarity (Python sidecar) for shape search.
- Cross-encoder rerank toggle; LanceDB swap if a library exceeds ~500k–1M vectors.
- Packaging/installer; auto-update; first-run model download/management.

---

## Tech stack summary

| Layer | Primary | Notes / alternatives |
|---|---|---|
| Language (core) | **Rust** | + optional bundled **Python sidecar** (UnityPy; later CUE4Parse/PyTorch) |
| GUI | **egui/eframe** (wgpu) | Slint 1.12 (polish) · Tauri+three.js (fallback) |
| 3D | **three-d** on shared wgpu | normalize→glTF (Assimp/FBX2glTF) |
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
| Plugins | Unity UPM C#; Unreal C++/Python | thin REST clients |

---

## Key risks & mitigations

| Risk | Mitigation |
|---|---|
| Unity format/version churn breaks deep extraction | Prefer raw source files; isolate deep parsing in the UnityPy sidecar; keep a regression corpus of real packs; pin versions. |
| Compressed-texture long tail (rare ASTC/Crunch) fails | "Unsupported format" fallback; don't promise 100%. |
| 3D tagging is shallow (category only) | Pair multiview→SigLIP with file-derived structured tags; OpenShape later. |
| PBR map channels invisible to vision models | Tag albedo; classify other maps by filename/hue; group as material. |
| FBX is proprietary | Normalize→glTF on import via Assimp/FBX2glTF; runtime only sees glTF. |
| Windows packaging pain | Rust single static binary sidesteps it; sidecar bundled, optional. |
| Vector search outgrows brute force | `VectorIndex` trait → swap sqlite-vec for LanceDB past ~500k–1M. |
| Cloud-AI privacy concerns | Local-first default; API strictly opt-in, per-modality. |
| Editor integration fragility (main thread, experimental Unreal MCP) | MCP = AI plane, REST = engine plane; browse+import scope; version checks + graceful degrade. |
| Wwise audio / shipped-game extraction | Out of initial scope; positioned around owned packs/projects. |

---

## Definition of "done" for v1

A user can: add their folders and `.unitypackage`/Asset-Store packs → view models, play audio, view
textures → search by keyword, tag-filter, and natural language → auto-tag locally (or via Claude) and
map onto a clean taxonomy → and **pull a chosen asset into an open Unity project**, both from the
desktop UI and from Claude via MCP — entirely on their own PC.
