# Architecture

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

This document covers the system architecture, the component breakdown, the data flow, and the
**language/stack decision** with the alternatives that were considered.

Guiding purpose (from the project brief): a **local PC tool whose main job is to identify and
retrieve assets for use in Unity** (Unreal support planned for a later phase), using a
tagging/identification system as the means.

---

## 1. The language decision

### Decision: **Rust core + a thin, optional Python sidecar for deep Unity extraction.**

Rust is the primary language for everything: GUI, 3D/audio/image preview, the file scanner/indexer,
the SQLite catalog + search, the AI orchestration, the MCP server, and the local HTTP API. A small
**bundled Python helper process** is used only where Python is genuinely ahead — deep deserialization
of Unity-native assets via **UnityPy** — and is not required for the common ingestion paths.

### Why Rust for the core

The five research streams converged on the same conclusion from different directions:

- **Real-time 3D embedding is dramatically easier when the UI and renderer share one GPU API.**
  With Rust, `egui`/`eframe`/`Slint` all run on **wgpu**, and a 3D viewport (`three-d`) paints into
  the UI through the *same* `wgpu::Device`. No FFI, no webview bridge, no second GPU context.
- **Local-first packaging.** Rust ships a **single static binary** (~5–20 MB, sub-millisecond
  startup, no runtime) on Windows. This is the decisive advantage over PyInstaller's fragile native-DLL
  bundling and over Electron/WebView2 runtime dependencies.
- **Indexer performance.** Scanning, hashing (BLAKE3), thumbnailing, and embedding 100k+ files is
  CPU/IO/GPU-bound work that Rust's threadpools (`rayon`/`tokio`) handle without GIL contention.
- **Data layer is first-class.** `rusqlite` + FTS5 + `sqlite-vec` (extension), `notify` (fs watch),
  `blake3`, and native LanceDB/Qdrant SDKs are all available.
- **The MCP SDK (`rmcp`) is mature** and lets the server run in-process sharing the catalog handle.

### Why a Python sidecar (and only there)

- **Unity deep-extraction**: **UnityPy** (MIT) is the de-facto programmatic extractor for
  Unity-serialized assets and compiled bundles — far more battle-tested across Unity versions than the
  young Rust `unity-asset` crates. Reimplementing its coverage in Rust would be a large, brittle effort.
- The sidecar is a **separate child process** Lodestash shells out to (JSON over stdio/pipe), launched
  only when an input actually requires deep Unity parsing. It is bundled but optional; the app degrades
  gracefully if it is absent.

**Crucially, the common case needs no sidecar:** a `.unitypackage` is a gzip+tar of GUID folders
whose payloads are the original FBX/PNG/WAV plus a YAML `.meta` — trivially handled in pure Rust.
Asset-Store packs and project folders are dominated by raw source files. The Python path is the
minority (compiled games, native `Texture2D`/`Mesh`/`AudioClip` blobs). See
[`INGESTION.md`](INGESTION.md).

### Why AI does **not** force Python

- The discrete models (SigLIP 2 embeddings, WD image taggers, CLAP audio embeddings, YAMNet/PANNs
  audio classifiers) all export to **ONNX** and run via the Rust **`ort`** crate (ONNX Runtime, with
  CUDA/DirectML/CPU execution providers). `candle`, `FastEmbed`, and `EmbedAnything` are Rust-native
  alternatives.
- **Local vision-LLM captions** run in **Ollama**, which Lodestash calls over HTTP — language-agnostic.
- **Speech** uses `whisper-rs` / whisper.cpp.
- The **online API** option is just HTTPS calls.

A PyTorch-only model (e.g. OpenShape for native-3D similarity, BLIP-2) can be served by the same
optional Python sidecar if/when added — but it is not on the critical path.

### Alternatives considered (and why not primary)

| Option | Strengths | Why not the primary |
|---|---|---|
| **Tauri (Rust) + web frontend + three.js** | Best UI styling/DX; three.js loads glTF/FBX/OBJ in-browser (sidesteps native 3D); small installer | 3D lives in a sandboxed webview (no shared GPU device, file access via bridge); WebView2 runtime dependency; thumbnail rendering of models is awkward. **Kept as the fallback** if UI polish becomes the top priority. |
| **Pure Python + PySide6 + Qt Quick 3D** | Best Python 3D embedding; proven virtualized 10k-item grids; richest access to ML libs & UnityPy in-process | **Windows distribution is the persistent pain** (PyInstaller + native DLLs); slower startup; larger footprint — directly at odds with "local-first single binary." Chosen only if the team is Python-bound. |
| **Rust UI + heavy Python ML core (PyO3)** | Reuse Python ML ecosystem | Reintroduces the packaging problem Rust solves; unnecessary because ONNX/Ollama cover the AI needs. |
| **Pure Rust incl. Unity parsing** | Single language, single binary | `unity-asset` crates are unproven across Unity versions; high risk for the hardest, most format-volatile area. The sidecar isolates that risk. |

> **Net:** Rust gives the best answers to the two hardest problems — embedded real-time 3D and
> painless local distribution — while ONNX/Ollama keep AI native and a small UnityPy sidecar absorbs
> the one area of real Python advantage.

---

## 2. System overview

```
┌──────────────────────────── Lodestash core process (Rust, single binary) ─────────────────────────────┐
│                                                                                                     │
│   ┌──────────────────────────────────  Core Service Layer  ───────────────────────────────────┐    │
│   │  Catalog (SQLite: relational + FTS5 + sqlite-vec)   Job/queue engine   Asset operations    │    │
│   │  Search service (hybrid: filter + BM25 + vector + RRF)   Mapping engine   Export/import     │    │
│   └───────▲───────────────────▲───────────────────▲───────────────────▲───────────────────▲────┘    │
│           │ in-proc           │ in-proc           │ in-proc           │ in-proc          │ in-proc   │
│   ┌───────┴──────┐   ┌────────┴────────┐  ┌───────┴───────┐  ┌────────┴────────┐ ┌───────┴───────┐  │
│   │ Desktop GUI  │   │ Ingestion       │  │ AI pipeline   │  │ MCP server      │ │ Local HTTP API │  │
│   │ (egui/wgpu)  │   │ (scan/extract)  │  │ (tag/embed)   │  │ (rmcp, stdio)   │ │ (127.0.0.1)    │  │
│   └──────────────┘   └───────┬─────────┘  └──────┬────────┘  └───────▲─────────┘ └──────▲─────────┘  │
│                              │ spawn             │ ort/http          │ MCP              │ HTTP+token │
└──────────────────────────────┼───────────────────┼───────────────────┼──────────────────┼───────────┘
                               │                   │                   │                  │
                     ┌─────────┴──────┐   ┌────────┴────────┐  ┌────────┴───────┐ ┌────────┴─────────┐
                     │ Python sidecar │   │ ONNX models /   │  │ Claude Desktop │ │ Unity (primary)  │
                     │ (UnityPy)      │   │ Ollama / Cloud  │  │ Claude Code /  │ │ (Unreal: planned) │
                     │  [optional]    │   │ API (opt-in)    │  │ IDE MCP clients│ │ (browse+import)  │
                     └────────────────┘   └─────────────────┘  └────────────────┘ └──────────────────┘
```

**One core service layer, many thin frontends.** Asset operations (search, get, tag, export, map)
are implemented **once** in the service layer. The GUI, the MCP server, and the HTTP API are three
presentations of the same operations. Engine plugins talk to the deterministic **HTTP API**; AI
clients talk to the **MCP server**. This avoids duplicating logic and keeps behavior consistent.

---

## 3. Components

### 3.1 Core service layer
The heart of the app — owns the catalog and exposes all operations. Pure library crate (`lodestash-core`)
with no UI dependency, so it can be driven by the GUI, the MCP server, the HTTP API, tests, and a CLI.

Sub-services:
- **Catalog store** — SQLite (WAL) access via `rusqlite`; relational tables + FTS5 + `sqlite-vec`
  in one file. All three indexes mutate inside one transaction for atomic consistency. See
  [`DATA-MODEL.md`](DATA-MODEL.md).
- **Search service** — hybrid retrieval: structural pre-filter → BM25 (FTS5) + vector KNN (sqlite-vec)
  → Reciprocal Rank Fusion → optional cross-encoder rerank.
- **Mapping engine** — applies the user's "mapping library" (taxonomy + alias + priority rules) to
  produce derived `asset_category` assignments; preserves pinned manual overrides.
- **Job engine** — background queue (tokio + rayon) for scan/hash/thumbnail/embed/tag/map work with
  progress reporting and backpressure; everything long-running returns a job id.
- **Asset operations** — get/search/tag/collection/export/import primitives shared by all frontends.
- **Project & import registry** — tracks registered Unity editors (`project`) and the durable
  provenance of every import (`import_record`: asset → project path + GUID), powering "into Unity"
  targeting, dup-guard ("already imported?"), and the staging shortlist. See
  [`DATA-MODEL.md`](DATA-MODEL.md) §9 and [`ENGINE-API.md`](ENGINE-API.md) §5.

### 3.2 Ingestion subsystem (`lodestash-ingest`)
File crawling, format detection (magic bytes), `.unitypackage` untar, `.meta` YAML parsing (harvests
GUIDs + Unity `labels`), routing to direct loaders vs. the Python sidecar, normalization (mesh→glTF,
texture→PNG, audio→WAV/OGG), and thumbnail/preview generation. Detail in [`INGESTION.md`](INGESTION.md).

### 3.3 AI pipeline (`lodestash-ai`)
`Tagger` and `Embedder` traits with local (ONNX/Ollama/Whisper) and API (cloud) implementations,
chosen per modality via config. Embeddings always run locally to keep one consistent, offline index.
Detail in [`AI-PIPELINE.md`](AI-PIPELINE.md).

### 3.4 Desktop GUI (`lodestash-desktop`)
Tauri 2 shell hosting a Svelte 5 frontend (WebView2 on Windows — no bundled browser). Virtualized
thumbnail grid (10k+ items), faceted filter sidebar, search bar (keyword + semantic), and a preview
pane that switches between the **3D viewport** (SVG preview), **audio player** (waveform), and
**image viewer**. Tag editor with confidence chips and accept/reject. Mapping-library editor.
Ships the persist thread (single-writer rule, ARCHITECTURE §5), the `lodestash://` protocol, and
the job tray; *the egui build (`crates/lodestash-ui`) was retired at the B7 cutover (2026-08-07).*
Detail in [`docs/design/README.md`](../docs/design/README.md) and `crates/lodestash-desktop/`.

### 3.5 MCP server (`lodestash-mcp`)
`rmcp`, in-process, **stdio** transport by default (optional Streamable HTTP). Exposes search,
preview, tag, collection, and export tools so AI clients can find and retrieve assets. Detail in
[`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md).

### 3.6 Local HTTP API (`lodestash-api`)
Loopback-only (`127.0.0.1`) JSON API with a per-session bearer token (handed to the plugin via a
handshake file). The deterministic integration surface for the Unity plugin (and the Unreal plugin
when added) and any scripting. Versioned (`/v1/`), with endpoints **1:1 to the MCP surface**. It owns
**project registration** and the **import queue** that closes the Unity round-trip: any caller (GUI,
MCP `import_to_project`, the panel) enqueues an import; the running plugin drains it on Unity's main
thread and acks the in-project path+GUID, which the core records as provenance. Full contract in
[`ENGINE-API.md`](ENGINE-API.md).

### 3.7 Engine plugins
- **Unity** (primary): UPM C# package with an `EditorWindow` that browses the catalog over the HTTP API
  and imports a chosen asset into `Assets/` (`AssetDatabase.ImportAsset` + refresh).
- **Unreal** (planned, later phase): C++/Python Slate editor tab that imports into `/Content` via the
  asset import subsystems.

Both are **thin REST clients**. Detail in [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md).

### 3.8 Python sidecar (`lodestash-unitypy`) — optional
A small Python program wrapping UnityPy (and, later, any PyTorch-only model). Communicates with the
core over JSON (stdio/pipe). Spawned on demand; the app functions without it for non-deep-parse inputs.

---

## 4. Primary data flows

### 4.1 Import a library
```
User adds a folder / .unitypackage
  → crawl & detect (magic bytes)
  → untar .unitypackage (gzip+tar) if needed; parse .meta (GUID, type, labels)
  → route: raw source file → direct loader   |   Unity-native/bundle → Python sidecar (UnityPy)
  → normalize (mesh→glTF cache, texture→PNG, audio→WAV/OGG) + extract metadata (tris, dims, duration)
  → BLAKE3 hash (exact dedup) + perceptual hash (near-dup)
  → insert asset/asset_file rows (one txn updates relational + FTS)
  → enqueue: thumbnail, embed (mark embedding_dirty), AI-tag, map
```

### 4.2 Find & retrieve (the main loop)
```
Query (text, optional semantic toggle, filters)
  → structural pre-filter (media_type, tags, mapped category, source)
  → BM25 (FTS5) ⊕ vector KNN (sqlite-vec)  → RRF fuse  → (optional rerank)
  → results grid with thumbnails
  → preview (3D / audio / image)   → (optional) stage candidates onto the project's staging tray
  → "Import into the open project":
        GUI / Unity panel / AI client(MCP import_to_project)
          → core resolves dependency closure + dup-guard check (import_record)
          → enqueue on the project's import queue → job_id
          → plugin long-polls the queue → imports on Unity's MAIN THREAD (AssetDatabase) → ack path+GUID
          → core writes import_record (provenance) → job done (MCP get_job_status / GUI observe it)
     or "Export to a path":  any caller → HTTP/MCP export_asset → dependency closure to a sandboxed dir
                              (no open editor required; /Content for Unreal later)
```

### 4.3 Tag / identify
```
Background job per asset:
  modality → Embedder.embed_asset (local) → upsert vec_asset, clear embedding_dirty
           → Tagger.tag (local ONNX/Ollama  OR  cloud API) → asset_tag (source, confidence, model_id)
  → mapping engine maps raw/AI tags → asset_category (skips pinned manual overrides)
User edits tags (accept/reject chips) → recorded as training signal + immediate search improvement
```

---

## 5. Concurrency & process model

**Resident embedding snapshots cross processes by fact, not by message** (v1 plan S09). The GUI and a
headless `--mcp` server are independent processes with independent caches; each keeps its own
snapshot per `(database, model)` and checks the model's generation (M30) — written inside the
embedding transaction — before trusting it. No event, no invalidation channel, and a rebuild failure
surfaces as an error rather than a silently stale answer.

- **Single core process**, async runtime (**tokio**) for IO/orchestration + **rayon** for CPU-bound
  batch work (hashing, image decode/resize, offscreen 3D thumbnail renders).
- **The agent planes read, and ask the writer** (v1 plan S11). The engine's `ServerState` holds a
  read-only connection (`Catalog::open_reader`) and routes its few writes — project registration,
  import records, export history — through the same bridge as agent writes. A headless `--mcp`
  process owns no writer, jobs or approvals at all: it forwards those tools to the app
  ([ADR-0011](decisions/ADR-0011-headless-mcp-forwards-writes.md)). Its REST and MCP handlers run on
  tokio's blocking pool, because the runtime is one thread and the state they call is synchronous.
- **SQLite access**: `rusqlite::Connection` is `!Sync`, so the catalog is **not** shared by handle.
  When multiple frontends go live (GUI + REST + MCP + ingest workers, P5), all writes funnel through
  **one dedicated writer** (a thread/actor owning the connection, fed by a channel); reads use
  short-lived per-task connections (WAL makes concurrent readers cheap). Do not reach for a pool of
  writers — SQLite serializes writes anyway.
- **Change publication** ([ADR-0016](decisions/ADR-0016-coalesced-change-publication.md)): nothing
  is emitted from inside a write. The writer marks what a committed turn changed — `catalog` only
  when SQLite's `total_changes` moved, tagged `user` / `agent` / `background`; `jobs` whenever the
  registry starts, advances or finishes a job — and one publisher thread sends at most one
  `ls:changed` per 250 ms window, carrying every origin in that window and a generation per domain.
  Fifty writes inside a window are one event; a continuous drain is one event per window, never one
  per commit. Headless (no window) publishes nothing.
- **Job engine** with bounded queues and progress events; the UI shows placeholders and swaps in
  results as jobs finish (what makes a 10k-grid feel instant).
- **GPU work** (live viewport + offscreen thumbnail render-to-texture + image texture upload) shares
  one wgpu device; thumbnail renders run on worker tasks into an on-disk cache keyed by
  `content_hash + params`.
- **Idempotency**: every derived artifact (thumbnail, embedding, tags) is keyed by
  `content_hash + model/version`; unchanged assets are never recomputed.
- **Python sidecar** and **cloud API** are the only out-of-process / off-machine dependencies, both
  optional and isolated behind interfaces.

---

## 6. Crate/module layout (proposed)

```
lodestash/
├─ crates/
│  ├─ lodestash-core/      # catalog, search, mapping, jobs, asset ops (no UI)
│  ├─ lodestash-ingest/    # crawl, detect, untar, meta-parse, normalize, thumbnail
│  ├─ lodestash-ai/        # Tagger/Embedder traits + ONNX/Ollama/cloud impls
│  ├─ lodestash-mcp/       # rmcp server over lodestash-core
│  ├─ lodestash-api/       # loopback HTTP/JSON API over lodestash-core
│  └─ lodestash-desktop/  # Tauri 2 + Svelte shell — the current app (egui retired at B7)
│                          #   (lodestash-cli — headless ops for power users & CI — is planned;
│                          #    the crate lands with the first CLI command, not before)
├─ sidecar/
│  └─ unitypy/          # optional Python UnityPy extractor (bundled)
├─ plugins/
│  ├─ unity/            # UPM C# package (primary)
│  └─ unreal/           # .uplugin C++/Python (planned)
└─ docs/
```

Keeping `lodestash-core` UI-free is the key structural rule: it guarantees the GUI, MCP, HTTP API, and
CLI all exercise the identical operations.
