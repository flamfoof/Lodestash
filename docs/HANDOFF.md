# Handoff — picking up Lodestash

Working notes for whoever (or whatever) continues the build. Unlike the rest of `docs/`, this file
is **not** a design spec — it's the state of play, the decisions already settled, and the traps.
The design docs still win on any question of what the software *should* do.

Last updated: **2026-07-22**, at `7efcb01` + the P2 dependency-graph slice.

---

## 1. Orientation

Read in this order:

1. [`../CLAUDE.md`](../CLAUDE.md) — the design-conformance rule. `docs/` is the source of truth; code
   conforms, and any divergence gets written back into the doc **in the same change**.
2. [`KANBAN.md`](KANBAN.md) — the board. Every card touched carries a detailed *shipped / remaining*
   note; read the card before starting its work.
3. The doc owning your area — [`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md),
   [`AI-PIPELINE.md`](AI-PIPELINE.md), [`DATA-MODEL.md`](DATA-MODEL.md),
   [`INGESTION.md`](INGESTION.md), [`TECH-STACK.md`](TECH-STACK.md) (deps + resolved spikes),
   [`UI-SPEC.md`](UI-SPEC.md).

Sanity check before starting: `cargo test --workspace` and `cargo clippy --workspace --tests`
(both green at handoff), then `cargo fmt --all`.

---

## 2. Where the build is

**Search-with-AI is complete against its design**, and the grid is fully visual.

| Modality | State |
|---|---|
| Image / texture / sprite | Decoded thumbnails + SigLIP embeddings, on-disk **and** inside `.unitypackage`s |
| 3D models | FBX/OBJ (ufbx) + glTF/GLB (three-d-asset) → CPU multiview render → SigLIP; rendered grid thumbnails |
| Audio | CLAP joint space (512-d), fused with the visual space by RRF; peak-waveform grid thumbnails |

Every asset also carries **technical facets** — triangles, pixel dimensions, audio duration —
extracted by a `metrics_dirty` drain (M9, [`DATA-MODEL.md`](DATA-MODEL.md) §14) that runs after every
scan whether or not the AI stack is installed. They show in the inspector, drive the ⚑ Filters
Triangles chips, and are parseable from a query (*"under 5k tris"*, *"under 500ms"*).

Plain-language queries are split into semantic text + structural filters before search. The whole AI
stack is one-click installable on win-x64 (fetches the ONNX Runtime *and* both models, ~645 MB).

**The agent plane is open** (P5, [`ENGINE-API.md`](ENGINE-API.md) + [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md)).
`lodestash-api` binds an ephemeral loopback port at launch, writes `engine.json`, and serves `/v1/`
behind a bearer token + loopback `Host`/`Origin` guard. `lodestash --mcp` runs the same operations
over rmcp stdio. Nine operations exist on **both** planes, and a parity test fails the build if one
plane grows a name the other lacks:

| Tool / route | Note |
|---|---|
| `get_server_info` · `search_assets` · `get_asset` | |
| `semantic_search` (`?mode=ai`) | 501 only while no model is loaded |
| `get_asset_preview` | image block + `resource_link`; PNG from the grid's own thumb cache |
| `find_similar` · `list_packages` | |
| `export_asset` · `get_job_status` · `cancel_job` | **side-effecting**, see below |

Both planes call the *same* `ServerState` methods the REST handlers call — that is what makes the 1:1
rule structural rather than a convention. `GET /v1/assets/{id}/file` stays REST-only by design.

**Side-effecting ops are gated on the human.** `export_asset` returns a job in state
`awaiting_approval` and copies nothing until someone approves the desktop modal (UI-SPEC §11.6b);
denial cancels it permanently. `dest_path` must be absolute, `..`-free, and outside every library
source root. This is MCP-AND-ENGINES §5 ("confirm server-side, don't rely on the client's
human-in-the-loop") made literal — keep it that way for every mutating op that follows.

**This machine's catalog is embedded** (2026-07-22): 1983 SigLIP vectors (1723 `model_3d`, 260
`image`) + 84 CLAP vectors, from ~2274 assets. The 207 rows still `embedding_dirty` are `.mat`
materials — there is no material renderer yet, so they legitimately have nothing to encode and will
drain themselves once one exists.

**The dependency graph is harvested and export honours it** (P2, [`DATA-MODEL.md`](DATA-MODEL.md) §10).
A third drain flag (`deps_dirty`, M10) walks each asset's `.meta` sidecar for its own GUID and a
material's `m_Texture` refs; unresolved refs are kept as `missing` edges and late-bound after every
drain, so index order doesn't matter. `dependency_closure` backs both `AssetDetail.dependencies` and
`export_asset(with_dependencies)` — the preview is the export. On this library: 1580 GUIDs, 200
resolved edges, 2 missing. Mesh → material comes from a second pass over `.prefab` files: a stock
FBX embeds its materials, so the model declares nothing and the prefab is the only place the binding
is written down. Prefabs are read as *evidence*, never catalogued (`prefab` isn't a `media_type`),
and the pass rebuilds `source='unity_prefab'` edges wholesale — 1228 resolved on this library, so
`SM_Gen_Env_Ground_Grass_Large_01` now returns material + 3 textures. **Coverage caveat**: an asset
outside a Unity project has no `.meta`, so no GUID and no edges at all — step 2's naming heuristic
(DATA-MODEL §10) is what would reach those.

Three UI surfaces read the graph: the inspector's **LINKED FILES** panel (UI-SPEC §13.6, both
directions, click to select, missing refs called out), a **painted** link badge on cards with
outbound edges (the bundled fonts have no chain glyph — U+26AD is tofu, like the dropped ◉), and
**⚑ Filters ▸ LINKS**. The relink panel from §13.6 is not built.

**The grid shows game assets only.** A file with no `media_type` (`.url`, `.pdf`, `.txt`,
`.prefab`, `.cs`) is dropped by the crawl — the catalog never held it anyway, so it was a
grid-only ghost with no status, no facets and no embedding. Grid total went 3768 → 1728, which
is exactly the catalog's on-disk row count. Prefabs are still read for the dependency graph.
The category tabs are now five: Materials split out of Models.

**The status lifecycle is live** (P3, [`DATA-MODEL.md`](DATA-MODEL.md) §8). `status` is *derived*,
not hand-set: `Catalog::promote_statuses` reads the ingest state already on the row — facets
extracted ⇒ `indexed`, an embedding stored ⇒ `identified`, embedding re-dirtied (file changed) ⇒ back
to `indexed` — and `confirmed` stays human-set. All three writer paths (scan persist, package
persist, backlog drain) funnel through one `drain_promote_resync`: drain facets, drain vectors,
promote, then re-read counts + identities and send them, because a drain is no longer count-neutral.
On this machine the meter reads **2067 / 2274 Identified**.

---

## 3. Settled decisions — don't re-litigate

Each is recorded with rationale in [`TECH-STACK.md`](TECH-STACK.md) §7 and the relevant KANBAN card.

- **CPU rasteriser, not GPU offscreen** (`lodestash_ingest::render3d`). The embed drain runs on a
  worker thread while eframe's Glow context is UI-thread-bound, so GPU rendering would need a headless
  context plus cross-thread coordination. *Tradeoff: photoreal/textured output is harder later.*
  It now serves both multiview embedding **and** grid thumbnails.
- **`sqlite-vec` deferred as YAGNI — backed by measurement.** At 50k×768-d a query is 212 ms, of which
  the SQLite BLOB reload is **~5.6×** the cosine scan. An ANN index would optimise the *cheap* half and
  reintroduce a native loadable extension to bundle. AI search is submit-driven (once per Enter), so
  this is fine to ~100k. Re-open only with evidence; `perf_probe_knn_scaling` is the harness.
- **Single-writer rule** (ARCHITECTURE §5). Every embedding write happens on the persist thread's own
  connection, after the grid sync. Do not add a second writer thread.
- **`media_type` stays a 7-value enum** — a data-model invariant. The UI's 4 category buckets map onto
  it; they are not a replacement.
- **No job engine until something long-running needs it** — and now that `export_asset` does, it is
  in `lodestash-api::jobs`: in-memory, atomics, cooperative cancel. Resist adding job *persistence*
  or a priority queue before an operation actually demands them.
- **The agent planes share one ops layer.** Adding an operation means adding a `ServerState` method
  plus a route plus a tool plus a row in `AGENT_OPERATIONS`. Skipping the last one fails the parity
  test, which is the point.
- **Separate, non-comparable vector spaces.** SigLIP (768-d visual) and CLAP (512-d audio) are searched
  independently and fused on *rank* by RRF — comparing raw scores across models is meaningless.
  `hybrid_search` therefore takes `spaces: &[(model_id, query_vec)]`.

---

## 4. Running the AI parts

**The ONNX Runtime version is strict.** `ort 2.0.0-rc.10` binds ORT **1.22.x**. A mismatched dll does
not degrade — it *panics*. Both `SiglipEmbedder::load` and `ClapEmbedder::load` wrap init in
`catch_unwind`, so it surfaces as a clean `Err` and the app falls back to the honest "no AI" stub.
Don't casually bump the dll.

The model-backed tests are all `#[ignore]` and env-gated, so a normal `cargo test` never needs models:

```bash
export ORT_DYLIB_PATH="…/onnxruntime-win-x64-1.22.0/lib/onnxruntime.dll"
export SIGLIP_VISION=…/siglip/vision.onnx SIGLIP_TEXT=…/siglip/text.onnx SIGLIP_TOKENIZER=…/siglip/tokenizer.json
export CLAP_AUDIO=…/clap/audio.onnx     CLAP_TEXT=…/clap/text.onnx     CLAP_TOKENIZER=…/clap/tokenizer.json

cargo test -p lodestash-ai   --test siglip_search    -- --ignored --nocapture  # visual search
cargo test -p lodestash-ai   --test siglip_multiview -- --ignored --nocapture  # 3D by shape
cargo test -p lodestash-ai   --test clap_audio       -- --ignored --nocapture  # audio matrix
cargo test -p lodestash-core perf_probe              -- --ignored --nocapture  # KNN scaling
```

**Embedding only drains where a writer already is.** The drains live on the persist thread (single-writer
rule), so a *hydrate* — the fast path when the catalog already knows the library — used to skip them
entirely, and a library catalogued before the models arrived would never encode. `spawn_backlog_drain`
now runs them after a hydrate, taking the same writer slot. If you add a third path that opens the
catalog for writing, it must respect that slot too.

To get the artifacts: run the app and use **Settings ▸ AI ▸ Download** (sources are in
`crates/lodestash-ui/src/models.rs`). They install to the OS data dir under
`Lodestash/models/{siglip,clap}/` and `Lodestash/runtime/onnxruntime.dll`; the same layout works if you
copy them in by hand, which is worth doing to avoid re-downloading ~645 MB.

---

## 5. Verification standard

This is the bar the existing work was held to; please keep it.

- **Anything visual gets looked at**, not just asserted. Coverage and colour assertions pass happily on
  garbage output — a render was only ever trusted after viewing the PNG.
- **Anything semantic gets a matrix**, not one ordering. A single "A beats B" can pass with a subtly
  wrong front end; a diagonal winning across several distinct inputs cannot easily.
- **Property tests get mutation-checked** — break the implementation and confirm the test fails.
  A test that cannot fail is worthless.
- **Prefer reading the reference to inferring it.** The CLAP mel front end looked like an unverifiable
  risk until the authoritative `ClapFeatureExtractor` was read; it turned out to hinge on one detail
  (Slaney vs HTK mel scale) that no amount of reasoning would have settled.

A related note: a test "failing" is not proof the code is wrong. One renderer test failed because a
*cube is legitimately identical across 90°-apart views* (4-fold symmetry) — correct geometry, bad
fixture.

---

## 6. Traps in this repo/environment

- **Never `git add -A`.** It has swept in an agent's live git worktree and daemon state. Use explicit
  paths or `git add -u`. `.gitignore` covers `.claudenator-wt-*/` and `.claude-flow/`, but stray
  tooling artifacts (numerically-named files) still appear in the repo root — check `git status` before
  every commit.
- **Duplicate `pub mod` lines appear** after adding a module to a crate's `lib.rs` (seen twice). It's a
  hard `E0428`; check `lib.rs` immediately after.
- **Run `cargo fmt --all` before committing.** Format drift accumulated silently over ~13 commits once.
- Python heredocs used for bulk edits need **raw** strings (`r'''…'''`) or `\u{…}` in Rust source gets
  eaten as a Python escape.
- **Reverting a mutation-check with `shutil.move`/`copy2` restores the file's old mtime**, so cargo
  thinks the crate is unchanged and reruns the *mutated* binary — the test looks broken after you
  fixed it. `touch` the file (or write it in place) after any revert.
- **To look at the running app**, `PrintWindow(hwnd, hdc, 2)` from PowerShell captures the window
  even when another window covers it; `CopyFromScreen` captures whatever is on top, which in this
  setup is usually the terminal. Screenshot coordinates are window-relative *originals* — the
  images you view are downscaled, so multiply by the ratio the viewer reports before clicking.
- **Synthetic clicks into the app are unreliable.** `PrintWindow` screenshots work every time;
  driving the UI does not. `mouse_event` after `SetForegroundWindow` worked once and then stopped
  (foreground lock), and `PostMessage(WM_LBUTTON*)` never landed at all. Note also that PrintWindow
  captures the **whole window** while `GetClientRect` reports the **client area**, so image
  coordinates are offset by the title bar (~31 px) before you convert them. When a click won't
  land, verify the mechanism headlessly instead of burning the session on input synthesis.
- **`cargo build` fails with "Access is denied. (os error 5)"** when the app is running — it holds
  `lodestash.exe`. `Stop-Process -Name lodestash -Force` first. `cargo check` works regardless.
- Git Bash maps `/tmp` to the Windows temp dir, and job-scoped env vars aren't guaranteed to persist
  between shell invocations — prefer absolute paths in long-running work.

---

## 7. Suggested next work

Roughly by value:

1. **P5 remainder — the Unity round-trip**: `register_project` → `/v1/import` → import queue →
   plugin ack → `import_record` ([`ENGINE-API.md`](ENGINE-API.md) §5). The job engine and the
   approval gate already exist to carry it. The UPM C# plugin is the big piece and cannot be
   verified without a Unity install.
2. **P2 relationship graph, remainder.** The GUID pass and export closure are in; what's left is
   **mesh → material** (naming heuristic or a prefab reader — see the coverage caveat above), the
   inspector "Related" tab, the grid link-chain badge, and the
   `has_dependents`/`has_dependencies` filters.
3. **P4 tagging.** Unblocks Traits, tag filter chips, `AssetSummary.tags`, pack-stats TAGS, and the
   accept/reject flow. Ollama is installed on this machine (`qwen3:8b`, text-only); a vision tagger
   would want `moondream` (~1.7 GB) or `qwen2.5vl:3b` (~3 GB) — the 4–6 GB VRAM tier of
   [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md). Note the machine is AMD (RX 6750 XT): no CUDA, and
   the bundled ORT is the **CPU** build. DirectML would need a different runtime dll — see the
   version warning in §4 before trying.
4. Optional AI refinements: an LLM query parser (slots in behind `ParsedQuery::validate`),
   cross-encoder rerank, glTF material textures.
5. **Non-win-x64 ORT auto-fetch** — deliberately skipped. It can't be tested from this machine, and
   shipping platform code you can't run is how silent breakage gets in.

Known gaps that are *recorded* rather than hidden — see the interim-status notes in
[`ENGINE-API.md`](ENGINE-API.md) §2: `AssetSummary.tags` is always `[]`, `Package` has no
`license`/`version`, `AssetDetail` carries only `size_bytes` + `stats`, the MCP per-tool allow-list
(UI-SPEC §16) is still a scaffold so every tool is live when the server runs, and job state is
in-memory (a crash loses the queue, never the catalog).
