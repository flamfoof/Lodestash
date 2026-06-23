# Tech Stack

The implementation-ready parts list. The *why* (language decision, alternatives) is in
[`ARCHITECTURE.md`](ARCHITECTURE.md); this doc is the concrete dependency selection that governs the
build. **New dependencies require updating this doc** (see `CLAUDE.md` rule).

Recap: **Rust** single-binary core · optional bundled **Python sidecar** (UnityPy) · AI via **ONNX
Runtime + Ollama + opt-in cloud**. Targets: Windows 10+, macOS 11+, Ubuntu 20.04+. GPU optional
(CPU fallback).

---

## 1. Crate workspace → key dependencies

Crates mirror [`ARCHITECTURE.md`](ARCHITECTURE.md) §6. All deps are permissive (MIT/Apache/BSD) unless
flagged.

| Crate | Purpose | Key dependencies |
|---|---|---|
| `lodestash-core` | catalog, search, mapping, jobs, asset ops | `rusqlite` (bundled SQLite + FTS5 + loadable-ext), `sqlite-vec`, `blake3`, `serde`/`serde_json`, `tokio`, `rayon`, `notify`, `uuid`, `thiserror`/`anyhow`, `tracing` |
| `lodestash-ingest` | crawl, detect, untar, normalize, thumbnail | `flate2`+`tar` (.unitypackage), `zip` (zip packs/.kra/.aseprite), `serde_yaml` (.meta), `image`, `image_dds`, `psd`, `texture2ddecoder`, `gltf`/`easy-gltf`, `russimp` (Assimp; C++ dep) **or** `FBX2glTF` (subprocess), `symphonia`, `ab_glyph`/`fontdue` (font specimens), `blake3`, `img_hash` (pHash) |
| `lodestash-ai` | Tagger/Embedder traits + impls | `ort` (ONNX Runtime; CUDA/DirectML/CPU EPs) — alt `candle`/`fastembed`, `tokenizers`, `ndarray`, `reqwest` (Ollama + cloud HTTP), `whisper-rs` (speech), `image` (preprocess) |
| `lodestash-ui` | egui app + 3D/audio/image preview | `eframe`/`egui` + `egui-wgpu` + `wgpu`, `three-d` (3D viewport), `rodio` (+ `symphonia`) audio, `egui_extras`/`egui_plot` (waveform), `rfd` (file dialogs), `poll-promise` (async UI) |
| `lodestash-mcp` | MCP server | `rmcp` (official Rust MCP SDK), `tokio`, `serde_json` |
| `lodestash-api` | loopback HTTP/JSON | `axum` + `tower`/`tower-http` (limits, CORS-off), `tokio`, `serde_json` |
| `lodestash-cli` | headless ops (scan/index/tag) | `clap`, `lodestash-core` |
| **shared** | — | `serde`, `serde_json`, `thiserror`, `anyhow`, `tracing`+`tracing-subscriber`, `tokio`, `rayon`, `uuid`, `blake3` |

---

## 2. Subsystem choices

| Concern | Choice | Notes |
|---|---|---|
| **DB / search** | SQLite (WAL) via `rusqlite` (bundled) + **FTS5** + **sqlite-vec** (loadable ext) | one file; `VectorIndex` trait → **LanceDB** (`lancedb` crate) past ~500k–1M vectors |
| **3D render** | `wgpu` + `three-d`, shared device with egui | normalize→glTF on import (Assimp via `russimp`, or `FBX2glTF` binary) |
| **Audio** | `symphonia` (decode) + `rodio` (playback) | waveform drawn with egui painter |
| **Images** | `image` + `image_dds` (BCn/DDS) + `psd` + EXR/HDR via `image` | dominant color via `color-thief`/k-means → `dominant_color` |
| **2D-authoring** | `.aseprite`/`.kra` = zip → embedded composite + frames; `.pdn` via parser; `.blend` embedded thumbnail / Blender headless | see [`INGESTION.md`](INGESTION.md) §4 |
| **AI runtime** | `ort` (ONNX) for SigLIP 2 / WD v3 / CLAP / YAMNet-PANNs; **Ollama** (HTTP `:11434`) for VLM captions; `whisper-rs` for speech; cloud via `reqwest` | model files per [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) |
| **Hashing/dedup** | `blake3` (exact) + `img_hash` pHash (image) + Chromaprint via `fpcalc`/binding (audio) | |
| **FS watch** | `notify` + periodic reconcile | |
| **Concurrency** | `tokio` (async IO/orchestration) + `rayon` (CPU batch) | in-house job engine over tokio tasks + bounded channels |

---

## 3. Python sidecar (optional, bundled)

- **Runtime:** Python 3.11. **Deps:** `UnityPy` (MIT), `texture2ddecoder`, `python-fsb5`, `Pillow`,
  optional `decrunch`/`astc`/`etcpak`.
- **Transport:** JSON over stdio/pipe; spawned on demand only for compiled bundles / native
  `Texture2D`/`Mesh`/`AudioClip` blobs.
- **Packaging:** ship a frozen venv or PyInstaller bundle alongside the app; degrade gracefully if absent.
- **Later:** a separate **CUE4Parse** (.NET, Apache-2.0) sidecar for Unreal (Phase 6).

---

## 4. Engine plugins

| Engine | Plugin tech | Talks to |
|---|---|---|
| **Unity** (primary) | UPM C# package, `Editor/` assembly, `EditorWindow`, `HttpClient`/`UnityWebRequest` | loopback REST ([`ENGINE-API.md`](ENGINE-API.md)); imports via `AssetDatabase` |
| **Godot** (planned) | EditorPlugin dock (GDScript/C#), HTTP client | REST; FileSystem import + resource-graph deps |
| **Unreal** (planned) | C++/Python editor module (Slate), + CUE4Parse sidecar | REST; `/Content` import |

---

## 5. Build, test, packaging

| Area | Choice |
|---|---|
| **Toolchain** | Rust stable, Cargo workspace; pin via `rust-toolchain.toml` |
| **Lint/format** | `rustfmt` + `clippy` (CI: `-D warnings`) |
| **Tests** | `cargo test` + `insta` (snapshots) + a **regression corpus** of small real `.unitypackage`/asset samples |
| **CI** | GitHub Actions: build + clippy + test on Windows/macOS/Linux; sccache; artifact upload |
| **Packaging** | `cargo-dist` (installers) / `cargo-bundle` / WiX (Windows); single binary + bundled sidecar; first-run **model download** step |
| **Versioning** | semver for the app; the engine **REST/MCP surface is `/v1/`** and independently versioned |
| **Errors/logging** | `thiserror` (libs) + `anyhow` (boundaries) + `tracing` (structured logs) |

---

## 6. Licensing watch-list

- **Permissive only** in the shipped binary. `russimp`/Assimp = BSD ✅; `FBX2glTF` precompiled embeds
  the proprietary Autodesk FBX SDK — prefer Assimp, or treat FBX2glTF as an optional external tool.
- **AssetRipper is GPL-3.0** — only ever invoke as a *separate CLI subprocess*, never link.
- `UnityPy` MIT ✅; `CUE4Parse` Apache-2.0 ✅ (Unreal, later).

---

## 7. Open choices to confirm during Phase 0

- `ort` vs `candle` for the embedding/tagger runtime (ONNX breadth vs pure-Rust single binary).
- `axum` vs raw `hyper` for the loopback API (axum's ergonomics vs minimal deps).
- Assimp (`russimp`, C++ build dep) vs an `FBX2glTF` subprocess for FBX→glTF.
- Chromaprint binding vs shelling to `fpcalc` for audio near-dup.

These are flagged in [`KANBAN.md`](KANBAN.md) Phase 0 as spikes; resolving them updates this doc.
