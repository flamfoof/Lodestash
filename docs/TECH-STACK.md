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
| `lodestash-core` | catalog, search, mapping, jobs, asset ops | `rusqlite` (bundled SQLite + FTS5 + loadable-ext), `sqlite-vec`, `blake3`, `serde`/`serde_json`, `tokio`, `rayon`, `uuid`, `thiserror`/`anyhow`, `tracing` |
| `lodestash-ingest` | crawl, detect, untar, normalize, thumbnail, **vector preview**, watch | `resvg` *(rasterises a stored preview SVG for renderers with no SVG support — the egui grid; a webview renders it natively and needs none of it. Already in the tree via `egui_extras`' `svg` feature, so it adds no new build)*, `notify` (source-root watch, DATA-MODEL §6), `flate2`+`tar` (.unitypackage), `zip` (zip packs/.kra/.aseprite), `serde_yaml` (.meta), `image`, `image_dds`, `psd`, `texture2ddecoder`, `gltf`/`easy-gltf`, `ufbx` (FBX; single C file via `cc`), `symphonia`, `ab_glyph`/`fontdue` (font specimens), `blake3`, `img_hash` (pHash) |
| `lodestash-ai` | Tagger/Embedder traits + impls | `ort` (ONNX Runtime; CUDA/DirectML/CPU EPs) — alt `candle`/`fastembed`, `tokenizers`, `ndarray`, `ureq` (Ollama over loopback + cloud HTTP), `whisper-rs` (speech), `image` (preprocess) |
| `lodestash-app` | **headless desktop orchestration** — jobs, model downloads, config, the catalog scan/persist loop and the identify drains | `lodestash-core`, `lodestash-ingest`, `ureq`+`zip` (model artifacts), `dirs`, `serde`. **Never a UI dependency** — this crate exists so the frontend is replaceable (the Tauri/Svelte port consumes it unchanged). Filled incrementally; `jobs`/`models` moved 2026-08-01, the rest still parked in `lodestash-ui`. |
| `lodestash-ui` | egui app + 3D/audio/image preview | `eframe`/`egui` + **`egui_glow`** (OpenGL renderer — see §7), `three-d` + `three-d-asset` (3D viewport; glow-lockstep with eframe, §7), `rodio` (+ `symphonia`) audio, `egui_extras` (`svg` feature — resvg; renders the sidebar nav icons, tinted per row state) + `egui_plot` (waveform), `rfd` (file dialogs), `poll-promise` (async UI), `keyring` *(Windows-only; cloud API key in Credential Manager, AI-PIPELINE §5.1 — other platforms fall back to plaintext config)* |
| `lodestash-desktop` | **Tauri 2 desktop shell** — hosts the Svelte frontend, owns the window, the `#[tauri::command]` surface, and (from B3) the `ls://` protocol and pump thread | `tauri` 2 + `tauri-build` 2 (MIT/Apache-2.0; WebView2 on Windows — no bundled browser), `serde`/`serde_json`, `tracing`. Its frontend lives in `crates/lodestash-desktop/ui/` and is **deliberately four dependencies**: `svelte` 5, `vite`, `typescript`, `@tauri-apps/api` (all MIT). No component library, no CSS framework, no state library, no icon package — see §5. **`bun` is the package manager and script runner** (MIT); `bun.lock` is committed, and there is no npm lockfile — one lockfile or they drift. A fifth package, `playwright-core`, is a **dev**-dependency only (see §5.2). Replaces `lodestash-ui` at the B7 cutover. |
| `lodestash-mcp` | MCP server | `rmcp` **2.2** (official Rust MCP SDK; features `server, macros, transport-io, schemars`), `schemars` (tool JSON Schema from the arg structs), `base64` (image content blocks), `tokio`, `serde_json` |
| `lodestash-api` | loopback HTTP/JSON | `axum`, `tokio`, `serde_json`, `uuid` (session token), `dirs` (handshake path); `tower` is a dev-dependency for `oneshot` route tests. `tower-http` limits/tracing arrive with the mutating routes. |
| `lodestash-cli` *(planned)* | headless ops (scan/index/tag) | `clap`, `lodestash-core`. **Not a crate yet** — an empty skeleton was carrying seven unused dependencies, so it was removed until a real command exists. |
| **shared** | — | `serde`, `serde_json`, `thiserror`, `anyhow`, `tracing`+`tracing-subscriber`, `tokio`, `rayon`, `uuid`, `blake3` |

---

## 2. Subsystem choices

| Concern | Choice | Notes |
|---|---|---|
| **DB / search** | SQLite (WAL) via `rusqlite` (bundled) + **FTS5**; vectors are BLOBs scanned by `BruteForceIndex` behind the `VectorIndex` trait | one file. **`sqlite-vec` is deferred, not shipped** — measured 2026-07-20 (`perf_probe_knn_scaling`): at 50k×768-d a query is 212ms, of which the BLOB reload+decode is ~5.6× the cosine scan, so an ANN index would optimise the cheap half *and* reintroduce a native loadable-extension to bundle. AI search is submit-driven (once per Enter), so this is fine to ~100k. Swap in `sqlite-vec` (or LanceDB past ~500k–1M) behind the trait when a real library shows it's needed |
| **3D render** | `three-d` **0.18** on eframe's shared **Glow** GL context (§7 — glow-version lockstep) | FBX flattens in-process via `ufbx`; glTF/GLB/OBJ load directly via `three-d-asset` (Assimp/`FBX2glTF` deferred — §7) |
| **Audio** | `symphonia` (decode) + `rodio` (playback) | waveform drawn with egui painter |
| **Images** | `image` + `image_dds` (BCn/DDS) + `psd` + EXR/HDR via `image` | dominant color via `color-thief`/k-means → `dominant_color` |
| **2D-authoring** | `.aseprite`/`.kra` = zip → embedded composite + frames; `.pdn` via parser; `.blend` embedded thumbnail / Blender headless | see [`INGESTION.md`](INGESTION.md) §4 |
| **AI runtime** | `ort` (ONNX) for SigLIP 2 / WD v3 / CLAP / YAMNet-PANNs; **Ollama** (HTTP `:11434`) for VLM tagging; `whisper-rs` for speech; cloud via `ureq` | model files per [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) |
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
| **Frontend build** | `bun` + Vite, in `crates/lodestash-desktop/ui/`. `bun run build` gates on `svelte-check`, so a type error fails the build. **Runtime dependencies are capped at four** (§1) — a webview app is where a dependency tree metastasises, and every added package is another thing to audit for a runtime network fetch against the offline invariant. `bun.lock` is committed and installs are offline-reproducible; there is no npm lockfile. |
| **CI** | GitHub Actions: `fmt` + `clippy` (`-D warnings`) + build + test on Windows/macOS/Linux; sccache; artifact upload |
| **Packaging** | `cargo-dist` (installers) / `cargo-bundle` / WiX (Windows); single binary + bundled sidecar; first-run **model download** step |
| **Versioning** | semver for the app; the engine **REST/MCP surface is `/v1/`** and independently versioned |
| **Errors/logging** | `thiserror` (libs) + `anyhow` (boundaries) + `tracing` (structured logs) |

**Test dev-dependencies** (workspace `[dev-dependencies]`, not shipped): `insta` (snapshot
assertions), `proptest` (property-based), `criterion` (benchmarks). `tempfile` is a **runtime**
dependency (test fixture dirs, plus the OS-temp scratch file the in-pack identify drains pull a
payload into — DATA-MODEL §7 / M14). Real-model AI inference is **not** run in CI (non-deterministic, weight-dependent,
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
| `lodestash-cli` *(planned)* | scan a fixture dir → assert JSON output shape | unit (calls core lib directly) |

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

### 5.2 Desktop end-to-end checks (`bun run e2e`)

WebView2 is Edge, so it speaks **CDP** natively. Launching the app with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222` lets `playwright-core`
attach to the *real* binary — real catalog connection, real `ls://` handler, real
`#[tauri::command]` surface. No WebDriver, no mocked IPC. (`tauri-driver` is the official route
but speaks WebDriver Classic, which Playwright does not.)

**`playwright-core` is a fifth frontend package, and a deliberate exception to the four-package
cap in §1** — dev-only, never bundled, and it needs no browser download because it attaches to
the WebView2 already installed. The cap still governs anything shipped.

**Scope rule: this suite covers only what fails *silently*.** Behaviour invisible to `cargo test`
because it lives in the browser, and invisible in a screenshot because a failure looks like an
empty state. The motivating case is real: a routing bug made every `ls://` request 404, and the
only symptom was a broken `<img>` — indistinguishable from a styled placeholder in a screenshot,
absent from every log. `naturalWidth > 0` catches it in one assertion. Anything assertable in
Rust stays in Rust; this is not a second home for the paging or Range-parsing tests.

No test framework — plain `node:assert` and one runner, so the package count stays at one.

**The debugging port is opened by the harness and nowhere else.** It must never appear in
`tauri.conf.json`: while open, anything on localhost can drive the app and read the catalog
through it.

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
| `ort` linking (P3, shipped) | **`ort =2.0.0-rc.10` + `load-dynamic`** | Compiles with **no native binary at build** (dlopens `onnxruntime.dll` at runtime), so CI/dev machines don't need the ~200MB runtime. **rc.10 requires ORT 1.22.x** — the app must **bundle the matching `onnxruntime.dll`** (or set `ORT_DYLIB_PATH`); a mismatched/missing runtime makes ort *panic*, so `OnnxTextEmbedder::load` wraps init in `catch_unwind` → clean `Err` → falls back to the stub embedder. **Validated 2026-07-20** against ORT **1.22.0** (win-x64 CPU) via `ORT_DYLIB_PATH`. The shipped **Search-with-AI** embedder is **SigLIP 2 base** (`onnx-community/siglip2-base-patch16-256`, uint8 towers — `lodestash_ai::siglip::SiglipEmbedder`, joint image↔text 768-d): query text → text tower, asset image → vision tower, cosine in one space (`--test siglip_search`, `#[ignore]`). **Zero-setup runtime (shipped 2026-07-20)**: rather than wait on packaging, the in-app model manager downloads the official ORT **1.22.0** win-x64 release, `zip`-extracts `onnxruntime.dll` into `<data>/Lodestash/runtime/`, and sets `ORT_DYLIB_PATH` there at startup — a stock win-x64 user needs neither a system runtime nor an installer bundle. (Packaging can still ship it beside the exe later; non-win-x64 uses the manual path.) Model files per [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md), fetched by the P4 model manager into `<data>/Lodestash/models/siglip/` (where the app already looks; `SIGLIP_{VISION,TEXT,TOKENIZER}` env override for dev). *(The MiniLM `OnnxTextEmbedder` from the first `ort` slice remains as tested lib code but is no longer the search path — SigLIP's joint space is the spec.)* `tokenizers` (onig via `cc`) + `ndarray` for token/tensor prep. |
| 3D multiview render for embeddings (P3, shipped) | **CPU rasteriser** (`lodestash_ingest::render3d`, no new deps) | The embed drain runs on a worker thread, but eframe's Glow context is UI-thread-bound — GPU offscreen rendering would need a headless context plus cross-thread coordination. A small z-buffered rasteriser (orthographic, Lambert, 4 orbit views) is thread-safe, dependency-free, and ample at encoder resolution (~256px). **Tradeoff: photoreal/textured output is harder later** — it renders silhouette + material colour only (`MeshData` already carries uvs/albedo if quality demands it). Also the natural engine for the P1 offscreen-thumbnail card. **The real cost is the CPU parse** (`mesh::load_any` via ufbx), not the raster, so GPU offscreen would help little **and** couldn't use eframe's UI-thread Glow context from a worker thread anyway. The fix is a **parse-once cache** (`mesh::load_cached`, 2026-07-26): normalized world-space `MeshData` serialized to a bounds-checked binary keyed by `(path,size,mtime)`; the metrics drain warms it on scan so browse-time thumbnails skip the parse. Chose a MeshData binary over the KANBAN's "normalize→glTF" because only MeshData is consumed internally (glTF would add a writer dep + a parse-back); textures are not cached (thumbnails render untextured). **Low-poly preview proxies (2026-08-01, `decimate.rs`)**: previews no longer render the full mesh — `mesh::load_preview_cached` decimates once by **vertex-cluster (grid-quantization) collapse**, caches the proxy as `<key>-<budget>.lod`, and drops the staging `.msh`. Chosen over quadric-error-metric because QEM needs a half-edge structure + an error heap, while at 128–256 px the silhouette is the entire signal (drop-in replaceable if a proxy ever feeds something that reads surface detail). Budget is a **ratio clamped to a floor and ceiling** per `PreviewQuality`, since a bare ratio leaves a 500k-tri hull heavy and erases a 400-tri prop. Measured on a 31.9k-tri character: 1,026 tris and **3,361 KB → 38 KB** of cache, silhouette intact. The `triangles` facet still comes from `load_cached` — it must report the source count, not the proxy's. No new dependency. |
| Ollama tagging (P4, shipped) | **`ureq`** (blocking, `json` feature) | The tagger runs one call at a time on the persist thread, so a blocking client is the right shape and `reqwest` would drag tokio into `lodestash-ai` for nothing. Loopback only — asset previews must not leave the machine. Base64 for the `images` field is ~15 lines rather than a crate. |
| model download (P4, shipped) | **`ureq`** (blocking, rustls) + **`zip`** (deflate) | The in-app model manager streams SigLIP from HuggingFace + the ORT release from GitHub on a worker thread — a blocking client with TLS + redirects is the right shape; `ureq` is tiny (no tokio/async) vs `reqwest`. `zip` (deflate only) extracts `onnxruntime.dll` from the ORT archive. The only outbound HTTP in the app; the local-first invariant is intact (cloud AI stays opt-in, embeddings stay local). |
| `axum` vs `hyper` | **`axum`** | Tower middleware ecosystem (rate-limit, tracing, auth); ergonomic routing; axum is the standard for production Rust HTTP servers. |
| Assimp vs FBX2glTF vs ufbx | **`ufbx`** (P1, shipped) | Single-file C FBX importer building with `cc` alone — russimp/Assimp needs cmake **and** libclang on the build machine for ~zero extra value (FBX is the dominant Unity mesh format; glTF/GLB/OBJ load via `three-d-asset`). Assimp stays the fallback if long-tail formats (DAE/3DS/LWO…) ever matter; FBX2glTF optional subprocess for Autodesk-SDK edge cases. |
| Chromaprint vs `fpcalc` | **`fpcalc` subprocess** (P2) | Avoids C++ build complexity at P0/P1; shell to system `fpcalc` binary; binding optional later if cross-platform `fpcalc` bundling proves easier. |
| wgpu vs Glow (egui renderer) | **`egui_glow`** (OpenGL/WGL) | wgpu requires explicit `surface.configure()` on every resize; the brief gap between reconfiguration and the first presented frame lets the Windows DWM compositor fill with a scaled old frame, producing visible "revert" jitter. OpenGL/WGL buffer swaps are synchronous with the window system and don't have this gap. **Do not switch to wgpu without re-validating resize smoothness on Windows.** |
| three-d on Glow: shared context vs offscreen handoff | **shared context** — `three_d::Context::from_gl_context(frame gl)` + egui `PaintCallback` | three-d is glow-based, so it renders straight into egui's frame (`RenderTarget::screen` + scissor to the widget rect): no second context, no readback. Constraint: **eframe and three-d must agree on one `glow` version** (eframe 0.29 ↔ three-d 0.18 ↔ glow 0.14) — upgrade the pair together. egui_glow re-arms its own GL state after each callback, so three-d's state doesn't leak. |
