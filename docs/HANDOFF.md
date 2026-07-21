# Handoff — picking up Lodestash

Working notes for whoever (or whatever) continues the build. Unlike the rest of `docs/`, this file
is **not** a design spec — it's the state of play, the decisions already settled, and the traps.
The design docs still win on any question of what the software *should* do.

Last updated: **2026-07-21**, at `52b2967`.

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
| Audio | CLAP joint space (512-d), fused with the visual space by RRF |

Plain-language queries are split into semantic text + structural filters before search. The whole AI
stack is one-click installable on win-x64 (fetches the ONNX Runtime *and* both models, ~645 MB).

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
- Git Bash maps `/tmp` to the Windows temp dir, and job-scoped env vars aren't guaranteed to persist
  between shell invocations — prefer absolute paths in long-running work.

---

## 7. Suggested next work

Roughly by value:

1. **Audio waveform thumbnails** (P1, same card as the 3D thumbnails). Cheap reuse: `audio.rs` already
   decodes waveforms for the viewer, and `thumb::generate` already dispatches by type.
2. **Metadata extraction** (tri counts, dimensions, duration). This lights up the `tris`/`duration`
   filters that `lodestash_core::query` deliberately does *not* parse yet, because nothing stores them.
3. **P2 Asset Store batch unpack+ingest**, or **P5 MCP/REST** to open the agent-facing lane — remember
   MCP↔REST must stay 1:1 (add an operation to both planes or neither).
4. Optional AI refinements: an LLM query parser (slots in behind `ParsedQuery::validate`, which exists
   precisely so a hallucinated facet gets clamped), cross-encoder rerank, glTF material textures.
5. **Non-win-x64 ORT auto-fetch** — deliberately skipped. It can't be tested from this machine, and
   shipping platform code you can't run is how silent breakage gets in.
