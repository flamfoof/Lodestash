# Lodestash — Local Game-Asset Identifier, Viewer & Retriever

> **Lodestash** = *lodestone* (it guides you to the right asset) + *stash* (the asset store you draw from).

A **local-first desktop application** that catalogs, previews, tags, and — most importantly —
**lets you and your AI tools find and pull the right asset into Unity fast.** (Unreal support is
planned but secondary — Unity is the primary target.)

It runs entirely on your PC. Nothing about your asset libraries has to leave the machine.

---

## What it is (and its main purpose)

The headline job is **identification and retrieval**: you have thousands of bought/downloaded
Unity assets — 3D models, audio, and images/textures — scattered across asset packs and
folders, and you can never find the one you need. Lodestash indexes all of them, understands what they
*are* (via AI tagging + embeddings), and gives you fast keyword + semantic + tag-filtered search,
plus a one-click / one-tool-call path to bring a chosen asset **into your open Unity project**.

Tagging and AI identification exist **to serve search and retrieval** — that ordering drives every
design decision in this spec.

### Core capabilities
- **Ingest** large asset libraries — by folder, or from Unity packages (`.unitypackage`), and
  (best-effort) compiled Unity bundles. Unreal `.pak`/IoStore support is planned for a later phase.
- **Tap your Unity Asset Store library directly** — auto-discover Unity's download cache, browse
  everything you own by publisher/category, and unpack + ingest one or many packages on demand
  (no running Editor required).
- **Ignore the noise** — auto-detect non-content assets (scripts, DLLs, editor tools, shaders) and
  keep them out of your content searches.
- **View / play everything**: interactive 3D viewport for models, audio playback with waveform,
  image/texture viewing (incl. HDR/EXR and compressed GPU textures). Auto-generated icons/thumbnails
  for every asset.
- **Tag & identify automatically** with either **local AI** (offline, free, private) **or an online
  API** (e.g. Claude vision) — user's choice, per modality, swappable.
- **Two ways to search**, with a category filter (Models / Sounds / Images):
  - **Search by Tags** — exact tag/metadata lookup: fast, precise, deterministic.
  - **Search with AI** — describe it in plain language ("pickup sound for rocks", "something with
    wings", "a fruit model") and a local AI semantically sieves the whole library. Returns ranked
    matches — or nothing, when there's no good fit.
- **Sorted vs unsorted at a glance** — every asset has a status (**Unidentified → Identified →
  Confirmed**); an **Identify mode** lets you work through the freshly-imported backlog (batch AI
  tagging + hand-tagging) while the **Library** stays clean and searchable.
- **2D & more**: animated sprites (pixel-perfect), **material previews on a 3D sphere**, and **fonts** —
  plus Aseprite/Krita/PSD/Paint.NET, `.blend`, and MaterialX formats.
- **Find more like it**: one-click similar-asset search (or drag in a reference to find owned matches);
  collapse near-duplicate **variants** into one tile; **color** search and color-coded/nested tags.
- **Apply a "mapping library"**: a user-defined taxonomy that maps messy raw/AI tags onto a clean
  canonical project vocabulary (Environment / Props / Characters / SFX / Music / …).
- **MCP server** so Claude, Claude Code, Cursor, and other AI clients can search the catalog and
  retrieve assets directly — **including importing into your open Unity project** (`import_to_project`),
  not just exporting to a path.
- **Selective engine import**: a **Unity editor plugin** that browses the catalog and imports
  **only the asset(s) you pick — plus their dependencies — never the whole package**, pulling from
  many libraries at once. (An Unreal plugin is planned for a later phase.)
- **Stage, then import**: a shared **shortlist/staging tray** to collect candidates across the GUI,
  MCP, and the Unity panel and import the picks together.
- **Import provenance & dup-guard**: Lodestash remembers which asset went into which project (path +
  GUID), so it warns "already imported" instead of duplicating, and shows where an asset is used.
- **License & attribution** captured per pack/asset and surfaced on every import/export, so shipping
  with bought assets stays traceable.

---

## The decision: language & stack

**Core application: Rust.** A single, fast, self-contained native binary — best fit for a local-first
tool that must do real-time 3D rendering, scan/hash/index 100k+ files, and ship without a runtime.

**One pragmatic Python sidecar**, bundled and optional, for the *one* area where Python is clearly
more mature: **deep Unity extraction via UnityPy** (compiled AssetBundles, `Texture2D`/`Mesh`/
`AudioClip` blobs). The common 80% of ingestion (`.unitypackage` is just gzip+tar of raw
FBX/PNG/WAV) needs no Python at all.

**AI runs natively in Rust** via ONNX Runtime (`ort`) for the embedding/tagging models, Ollama over
HTTP for local vision-LLM captions, and HTTP for the optional cloud API — so the heavy ML stack does
**not** force a Python dependency.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the full rationale and the considered
alternatives (Tauri+three.js, pure-Python/PySide6).

### Stack at a glance

| Concern | Choice |
|---|---|
| Language (core) | **Rust** (+ optional bundled Python sidecar for UnityPy) |
| GUI | **egui / eframe** on **wgpu** (Slint 1.12 as polish-focused alternative) |
| 3D viewport | **three-d** on the shared wgpu device; normalize to glTF on import |
| Audio | **symphonia** (decode) + **rodio** (playback) + egui waveform |
| Images | **image** + **image_dds** (BCn/DDS) + **psd** |
| Catalog DB | **SQLite** (WAL) — single file |
| Full-text search | **FTS5** (BM25) |
| Vector search | **sqlite-vec** (abstracted seam → LanceDB past ~500k vectors) |
| AI inference | **ONNX Runtime (`ort`)** + **Ollama** (HTTP) + cloud API (HTTP) |
| Image/3D embeddings | **SigLIP 2** (multiview render for 3D) |
| Audio embeddings/tags | **CLAP** + **YAMNet/PANNs** + Whisper (speech) |
| Image tags | **WD v3** taggers (ONNX) |
| Dedup | **BLAKE3** (exact) + pHash/blockhash + Chromaprint (near-dup) |
| MCP server | **rmcp** (Rust), in-process, stdio default |
| Engine API | loopback **HTTP/JSON** (127.0.0.1 + token); 1:1 with the MCP surface — see [`docs/ENGINE-API.md`](docs/ENGINE-API.md) |
| Unity round-trip | shared **import queue**: GUI/Claude enqueue → plugin imports on Unity's main thread → acks provenance |
| Engine plugins | **Unity** (UPM C# `EditorWindow`) — primary; Godot + Unreal — planned (co-equal) |

---

## Documentation map

| Doc | Contents |
|---|---|
| [`docs/USER-GUIDE.md`](docs/USER-GUIDE.md) | **Start here:** step-by-step guide for users (download-don't-import, run identification first, search, import) |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | System architecture, components, data flow, the language decision & alternatives |
| [`docs/INGESTION.md`](docs/INGESTION.md) | Import pipeline, Unity formats (primary), Unreal formats (planned), viewers/players, thumbnailing |
| [`docs/AI-PIPELINE.md`](docs/AI-PIPELINE.md) | Tagging & embeddings per modality; local-vs-API abstraction |
| [`docs/AI-MODELS-BY-GPU.md`](docs/AI-MODELS-BY-GPU.md) | **User guide:** which local AI models to run per GPU VRAM tier (CPU / 4 / 6 / 8 / 12 / 16 / 24 GB / cloud) |
| [`docs/DESIGN-GUIDELINES.md`](docs/DESIGN-GUIDELINES.md) | **Design system** for devs & designers — dark-mode JRPG theme, tokens, components, icons (+ live [`design/style-guide.html`](docs/design/style-guide.html), [`design/icons.svg`](docs/design/icons.svg)) |
| [`docs/DATA-MODEL.md`](docs/DATA-MODEL.md) | SQLite schema, hybrid search, the mapping library, dedup |
| [`docs/IDENTIFICATION-AND-SEARCH.md`](docs/IDENTIFICATION-AND-SEARCH.md) | The identification engine + the **three ways to search** (tags / plain language / AI-MCP), faceted schema, query understanding |
| [`docs/MCP-AND-ENGINES.md`](docs/MCP-AND-ENGINES.md) | MCP surface, the closed Unity import round-trip, Unity-side MCP-client topology, Unreal (planned) |
| [`docs/ENGINE-API.md`](docs/ENGINE-API.md) | **The interface layer:** loopback HTTP/JSON contract — endpoints (1:1 with MCP), token handshake, project registration & import queue, staging |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Phased build plan, milestones, risks |
| [`docs/TECH-STACK.md`](docs/TECH-STACK.md) | Implementation-ready stack — crates/libraries per module, AI runtime, sidecar, build/test/CI/packaging |
| [`docs/KANBAN.md`](docs/KANBAN.md) | Kanban board (Backlog → Ready → In Progress → Review → Done) tracking the build |

> Working in this repo? Read [`CLAUDE.md`](CLAUDE.md) — the docs are the source of truth, and code must
> conform to them (update the doc in the same change if it diverges).

---

## Design principles

1. **Retrieval first.** Every feature is judged by whether it helps you find and pull the right asset faster.
2. **Local-first & private.** Fully functional offline; cloud AI is strictly opt-in.
3. **Non-destructive.** Original asset files are never modified or moved; the catalog only references them.
4. **Prefer source files.** Use the raw FBX/PNG/WAV inside packages directly; deep-parse only when there is no source.
5. **One core, many frontends.** GUI, MCP server, and engine REST API are thin presentations over one shared service layer — the MCP tools and REST endpoints are **1:1** ([`docs/ENGINE-API.md`](docs/ENGINE-API.md)), so the AI and the Unity plugin reach the *same* operations and meet at one shared import queue.
6. **Swappable intelligence.** Local model ↔ cloud API is a config switch behind a stable interface.
