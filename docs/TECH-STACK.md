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
| `lodestash-ingest` | crawl, detect, untar, normalize, thumbnail | `flate2`+`tar` (.unitypackage), `zip` (zip packs/.kra/.aseprite), `serde_yaml` (.meta), `image`, `image_dds`, `psd`, `texture2ddecoder`, `gltf`/`easy-gltf`, `ufbx` (FBX; single C file via `cc`), `symphonia`, `ab_glyph`/`fontdue` (font specimens), `blake3`, `img_hash` (pHash) |
| `lodestash-ai` | Tagger/Embedder traits + impls | `ort` (ONNX Runtime; CUDA/DirectML/CPU EPs) — alt `candle`/`fastembed`, `tokenizers`, `ndarray`, `reqwest` (Ollama + cloud HTTP), `whisper-rs` (speech), `image` (preprocess) |
| `lodestash-ui` | egui app + 3D/audio/image preview | `eframe`/`egui` + **`egui_glow`** (OpenGL renderer — see §7), `three-d` + `three-d-asset` (3D viewport; glow-lockstep with eframe, §7), `rodio` (+ `symphonia`) audio, `egui_extras` (`svg` feature — resvg; renders the sidebar nav icons, tinted per row state) + `egui_plot` (waveform), `rfd` (file dialogs), `poll-promise` (async UI) |
| `lodestash-mcp` | MCP server | `rmcp` (official Rust MCP SDK), `tokio`, `serde_json` |
| `lodestash-api` | loopback HTTP/JSON | `axum` + `tower`/`tower-http` (limits, CORS-off), `tokio`, `serde_json` |
| `lodestash-cli` | headless ops (scan/index/tag) | `clap`, `lodestash-core` |
| **shared** | — | `serde`, `serde_json`, `thiserror`, `anyhow`, `tracing`+`tracing-subscriber`, `tokio`, `rayon`, `uuid`, `blake3` |

---

## 2. Subsystem choices

| Concern | Choice | Notes |
|---|---|---|
| **DB / search** | SQLite (WAL) via `rusqlite` (bundled) + **FTS5** + **sqlite-vec** (loadable ext) | one file; `VectorIndex` trait → **LanceDB** (`lancedb` crate) past ~500k–1M vectors |
| **3D render** | `three-d` **0.18** on eframe's shared **Glow** GL context (§7 — glow-version lockstep) | FBX flattens in-process via `ufbx`; glTF/GLB/OBJ load directly via `three-d-asset` (Assimp/`FBX2glTF` deferred — §7) |
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
| **Tests** | `cargo test` + `insta` (snapshots) + `proptest` (properties) + `criterion` (benches); a **regression corpus** of small real asset samples — full layer matrix in **§5.1** |
| **CI** | GitHub Actions: `fmt` + `clippy` (`-D warnings`) + build + test on Windows/macOS/Linux; sccache; artifact upload |
| **Packaging** | `cargo-dist` (installers) / `cargo-bundle` / WiX (Windows); single binary + bundled sidecar; first-run **model download** step |
| **Versioning** | semver for the app; the engine **REST/MCP surface is `/v1/`** and independently versioned |
| **Errors/logging** | `thiserror` (libs) + `anyhow` (boundaries) + `tracing` (structured logs) |

**Test dev-dependencies** (workspace `[dev-dependencies]`, not shipped): `insta` (snapshot
assertions), `proptest` (property-based), `criterion` (benchmarks), `tempfile` (fixture dirs,
already used). Real-model AI inference is **not** run in CI (non-deterministic, weight-dependent,
slow) — gate those behind `--features model-tests` for local/nightly runs; CI uses mock
`Tagger`/`Embedder` impls.

### 5.1 QA matrix — what each layer tests

The rule: **logic lives in pure, testable functions; the render loop and I/O stay thin.** Anything
non-trivial (a branch, a parser, a ranking, a money/security/provenance path) leaves a test behind.

| Layer | What gets tested | Tooling |
|---|---|---|
| `lodestash-core` — catalog | schema migrations apply clean on an empty DB; asset/file CRUD round-trips; soft-delete + move/rename (vanished path + same `content_hash` ⇒ `rel_path` updated, `asset_id`/tags/embeddings preserved) | `cargo test` on in-memory SQLite |
| `lodestash-core` — search | **RRF fusion is deterministic** given fixed BM25 + KNN rank lists; pre-filter is a hard boolean (tag/category/source never leak past it); `k≈60` weighting | unit + `proptest` (fused order invariant under rank perturbation) |
| `lodestash-core` — mapping | rule priority resolution (lower wins); **pinned overrides survive re-apply**; re-map on `taxonomy.version` bump regenerates only non-pinned rows | unit |
| `lodestash-core` — relationships | GUID-pair resolution → expected `asset_relationship` edges; **cross-source edges** resolve (texture in source A ← material in source B); inbound/outbound traversal; dup-guard `UNIQUE(asset_id, project_id)` | unit + corpus |
| `lodestash-ingest` | format detect by magic bytes (table-driven); `.unitypackage` untar → GUID→pathname map; `.meta` `labels:` + GUID-ref parse; content/non-content classification; **existing**: crawl, BLAKE3, dedup | unit + `insta` (parse snapshots) + corpus |
| `lodestash-ai` | tag normalization + confidence thresholding; per-modality `local\|api` routing; trait contracts via **mock** impls | unit (mocks) |
| `lodestash-mcp` + `lodestash-api` | **the MCP↔REST 1:1 invariant is a test** — assert the tool set and the endpoint set are identical (fails CI if one plane gains an op the other lacks); request/response schema snapshots; handshake-token rejection | `insta` + a shared op-registry test |
| `lodestash-ui` | pure helpers only: `rebuild_cache` sort/filter order, `thumbnail_grid` column math, lookahead-window index range; **not** rendering | unit |
| `lodestash-cli` | scan a fixture dir → assert JSON output shape | unit (calls core lib directly) |

**Regression corpus** (`tests/corpus/`, committed): tiny real samples — a few-KB `.unitypackage`
(GUID folders + `.meta` with `labels:` and GUID refs), a BCn `.dds`, an `.fbx`, a `.wav`, a `.psd`,
a material + its textures (to exercise the relationship harvest). Each pairs with an `insta`
snapshot of its parsed output, so a parser regression shows as a reviewable diff. **Keep samples
small and license-clean** (self-authored or CC0) — they ship in the repo.

**Performance gate** (`criterion`, dev/nightly — not a hard CI fail; shared runners are noisy):
crawl throughput, `rebuild_cache` on 100k synthetic rows, hybrid-search latency at 100k vectors.
Tracks the §6/§3 perf claims (FTS5 sub-10 ms, KNN tens-of-ms) so a regression is visible before
release.

---

## 6. Licensing watch-list

- **Permissive only** in the shipped binary. `russimp`/Assimp = BSD ✅; `FBX2glTF` precompiled embeds
  the proprietary Autodesk FBX SDK — prefer Assimp, or treat FBX2glTF as an optional external tool.
- **AssetRipper is GPL-3.0** — only ever invoke as a *separate CLI subprocess*, never link.
- `UnityPy` MIT ✅; `CUE4Parse` Apache-2.0 ✅ (Unreal, later).

---

## 7. Phase-0 spike decisions (resolved)

| Spike | Decision | Rationale |
|---|---|---|
| `ort` vs `candle` | **`ort`** (ONNX Runtime) | Broadest model coverage (SigLIP 2, WD v3, CLAP, YAMNet); pre-built DirectML/CUDA/CPU EPs on Windows; no need to retrain or convert to candle format. |
| `ort` linking (P3, shipped) | **`ort =2.0.0-rc.10` + `load-dynamic`** | Compiles with **no native binary at build** (dlopens `onnxruntime.dll` at runtime), so CI/dev machines don't need the ~200MB runtime. **rc.10 requires ORT 1.22.x** — the app must **bundle the matching `onnxruntime.dll`** (or set `ORT_DYLIB_PATH`); a mismatched/missing runtime makes ort *panic*, so `OnnxTextEmbedder::load` wraps init in `catch_unwind` → clean `Err` → falls back to the stub embedder. **Validated 2026-07-20** against ORT **1.22.0** (win-x64 CPU) via `ORT_DYLIB_PATH`. The shipped **Search-with-AI** embedder is **SigLIP 2 base** (`onnx-community/siglip2-base-patch16-256`, uint8 towers — `lodestash_ai::siglip::SiglipEmbedder`, joint image↔text 768-d): query text → text tower, asset image → vision tower, cosine in one space (`--test siglip_search`, `#[ignore]`). Packaging (P6) must place `onnxruntime.dll` beside `lodestash.exe` so a stock install needs no env var. Model files per [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md), fetched by the P4 model manager into `<data>/Lodestash/models/siglip/` (where the app already looks; `SIGLIP_{VISION,TEXT,TOKENIZER}` env override for dev). *(The MiniLM `OnnxTextEmbedder` from the first `ort` slice remains as tested lib code but is no longer the search path — SigLIP's joint space is the spec.)* `tokenizers` (onig via `cc`) + `ndarray` for token/tensor prep. |
| model download (P4, shipped) | **`ureq`** (blocking, rustls) | The in-app model manager streams SigLIP from HuggingFace on a worker thread — a blocking client with TLS + redirects is the right shape; `ureq` is tiny (no tokio/async) vs `reqwest`. The only outbound HTTP in the app; the local-first invariant is intact (cloud AI stays opt-in, embeddings stay local). |
| `axum` vs `hyper` | **`axum`** | Tower middleware ecosystem (rate-limit, tracing, auth); ergonomic routing; axum is the standard for production Rust HTTP servers. |
| Assimp vs FBX2glTF vs ufbx | **`ufbx`** (P1, shipped) | Single-file C FBX importer building with `cc` alone — russimp/Assimp needs cmake **and** libclang on the build machine for ~zero extra value (FBX is the dominant Unity mesh format; glTF/GLB/OBJ load via `three-d-asset`). Assimp stays the fallback if long-tail formats (DAE/3DS/LWO…) ever matter; FBX2glTF optional subprocess for Autodesk-SDK edge cases. |
| Chromaprint vs `fpcalc` | **`fpcalc` subprocess** (P2) | Avoids C++ build complexity at P0/P1; shell to system `fpcalc` binary; binding optional later if cross-platform `fpcalc` bundling proves easier. |
| wgpu vs Glow (egui renderer) | **`egui_glow`** (OpenGL/WGL) | wgpu requires explicit `surface.configure()` on every resize; the brief gap between reconfiguration and the first presented frame lets the Windows DWM compositor fill with a scaled old frame, producing visible "revert" jitter. OpenGL/WGL buffer swaps are synchronous with the window system and don't have this gap. **Do not switch to wgpu without re-validating resize smoothness on Windows.** |
| three-d on Glow: shared context vs offscreen handoff | **shared context** — `three_d::Context::from_gl_context(frame gl)` + egui `PaintCallback` | three-d is glow-based, so it renders straight into egui's frame (`RenderTarget::screen` + scissor to the widget rect): no second context, no readback. Constraint: **eframe and three-d must agree on one `glow` version** (eframe 0.29 ↔ three-d 0.18 ↔ glow 0.14) — upgrade the pair together. egui_glow re-arms its own GL state after each callback, so three-d's state doesn't leak. |
