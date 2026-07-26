# AI Tagging & Embeddings

How assets are auto-identified (tags) and made semantically searchable (embeddings), with a clean
**local ↔ online-API** switch. Embeddings power the "search by description" that makes retrieval fast,
so this subsystem exists to serve search.

---

## 1. Shape of the pipeline

Every asset yields two AI outputs, regardless of modality or backend:

1. **Tags** — discrete labels (free-form and/or controlled), each with a confidence score.
2. **Embedding** — an L2-normalized float vector for semantic search.

```
asset → [normalize/decode] → [embedding model] → vector  (→ vec_asset)
                            ↘ [tagger / caption] → tags + scores  (→ asset_tag)
text query → [SAME model's text encoder] → query vector → ANN search
```

**Load-bearing rule:** use **contrastive joint models** (CLIP/SigLIP for images, CLAP for audio) so a
text query and an asset land in the *same* vector space. That is what enables "search by text" without
a separate query-understanding stage — and it means `embed_text` and `embed_asset` for a modality must
come from the **same model**.

---

## 2. The local-vs-API abstraction

**Identification is manual by default.** Both embeddings (SigLIP/CLAP) and local-LLM tagging run on
the CPU here, so a hydrate never auto-identifies unless the user opts in (`config.auto_identify`,
Settings ▸ AI · IDENTIFY, default off). With it off, a scan/hydrate runs only the cheap drains
(technical facets, dependency graph, status promotion); embeddings and tags run when the user presses
**Identify** — which does embeddings *then* tags in one pass. With it on, new unidentified assets are
drained in the background as they appear. This matters most for in-pack libraries (DATA-MODEL §7 /
M14): importing/opening a pack mints thousands of dirty assets at once, and auto-embedding them all
would peg a core for minutes.

**The manual pass has three scopes** (the drains always run over whatever is `embedding_dirty`/
`tags_dirty`, so a scope simply re-flags the subset it wants redone before draining):

- **Run on New** — the default; no re-flag. A freshly minted asset is dirty until first processed, so
  this drains only assets that have never been through identification.
- **Run on Unidentified** — re-flags every live asset with **no tag** (`flag_identify_untagged`):
  ones that failed to tag or were never tagged. Superset of New.
- **Full Run** — re-flags the whole live library (`flag_identify_all`): redo everything (e.g. after a
  model upgrade).

Right-clicking assets in the grid offers **Identify selected** (`flag_identify_assets`), scoping the
pass to that selection. AI tags are written `source='ai'`, so any re-run replaces only AI tags and
never the user's own.

```rust
trait Tagger {
    fn model_id(&self) -> &str;                       // recorded on every tag (asset_tag.model_id)
    fn supports(&self, media_code: &str) -> bool;     // a vision tagger refuses audio
    fn tag(&self, input: &TagInput) -> Vec<TagAssignment>;   // { namespace, value, confidence }
}
trait Embedder {
    fn embed_asset(&self, asset: &Media) -> Vec<f32>; // unit-normalized
    fn embed_text(&self, query: &str) -> Vec<f32>;    // SAME space as embed_asset
}
```

**Run the stages in batches, not per asset.** Measured on this box: alternating the vision and
text models per asset costs **~50 s/asset**, because Ollama evicts and reloads multi-GB weights on
every call. Captioning a whole batch, then facet-passing that batch, keeps each model resident and
brings a facet call to **~7-8 s**. Requests also send `keep_alive` so the weights survive between
batches. This is why the drain is staged rather than a single loop, and it is the number to
re-measure before assuming a bigger model is affordable.

**Why the local tagger is two models.** Measured, not assumed: asked for facet JSON directly,
`moondream` degenerates into arrays of numbers (its JSON mode is trained for grounding boxes, not
objects) — while the *same* model captions the *same* preview accurately in prose. So the vision
model does what it is good at (one sentence of description), and a text model turns that caption
plus the filename into the schema. The caption is not thrown away: it becomes `asset.description`,
which FTS indexes, so a word the model saw but the filename never contained still finds the asset.
A machine with no vision model installed still tags from names alone, at a lower confidence (0.4 vs
0.7) because a name is a weaker claim than pixels.

The tagger returns **proposals**, never writes: `lodestash_ai::tagger::validate` drops facets a
model may not fill (`color`/`technical`/`audio` — §2 of IDENTIFICATION-AND-SEARCH) and caps values
per facet, then `Catalog::set_tags` normalizes and persists. Same "validate then write" seam the
query parser uses, so a hallucinated facet can never reach the catalog. `TagInput` carries decoded
pixels and the asset name, not a path — ingest stays in `lodestash-ingest`, this crate stays
models-only. **Caption** (→ `asset.description`) is not in the trait yet; it arrives with the VLM
that can actually produce one.

| Impl | Backend |
|---|---|
| `LocalTagger` | **Ollama, two stages: VLM captions the preview → text model writes the facet JSON** (shipped) · WD v3 (image, ONNX) · YAMNet/PANNs + CLAP (audio) · multiview→WD (3D) |
| `ApiTagger` | Cloud/remote vision LLM over the **OpenAI-compatible** chat+vision shape (first concrete provider — reaches OpenAI, **Gemini**, Groq, OpenRouter, and any local OpenAI-API server); Claude-native is a later drop-in. JSON facet response. |
| `LocalEmbedder` | SigLIP 2 (image) · CLAP (audio) · multiview-SigLIP (3D) — via `ort`/candle |
| `ApiEmbedder` | provider embeddings *(optional — see policy)* |

**Policy:**
- **Embeddings always run locally** (cheap, private, keeps one consistent offline-searchable index).
- **Tagging** is the user-switchable knob (`local` | `api`), configurable **per modality**.
- The embedder is chosen **per modality as a unit** (text+asset towers must match); you cannot mix a
  local image embedder with an API text embedder.

---

## 3. Per-modality defaults

| Modality | Embedding (local, always) | Tags (local default / API option) |
|---|---|---|
| **Image / texture** | **SigLIP 2 So400m** (768-d; ViT-B fallback on weak CPUs) | **WD-EVA02-Large-v3** (ONNX) / Claude vision JSON. Textures: embed the **albedo** channel; detect other maps by filename and merge as one material. |
| **Audio** | **LAION-CLAP** (512-d) | **YAMNet/PANNs** event tags + CLAP zero-shot description; **Whisper** only when VAD detects speech / API enriches text tags |
| **3D model** | **multiview render (4–8 views) → SigLIP 2**, mean-pooled & renormalized | **WD tagger** on the rendered views (vote/union) + **file-derived structured tags** (poly count, material slots, bone count) / Claude vision on the views |
| **Sprite (animated 2D)** | SigLIP 2 on the composited hero frame (image path) | WD v3 on sampled frames (union) + file-derived technical tags (frame count, fps, dimensions) |
| **Material** | SigLIP 2 on the **albedo** map (upgrade: the lit sphere render once available) | filename/hue map-role heuristics + WD/Claude vision on albedo — same PBR-map caveat as textures |
| **Font** | SigLIP 2 on the rendered **glyph specimen sheet** ([`INGESTION.md`](INGESTION.md) §5) | file-derived metrics (style, weight, glyph coverage) + VLM/Claude on the specimen (e.g. "handwritten", "pixel", "serif") |

### Notes & known limits
- **PBR map channels are invisible to vision models.** Normal/roughness/metallic/ORM maps look like
  noise to CLIP/VLMs → tag the **albedo** only; classify the rest by filename/hue heuristics.
  Tileability is best detected procedurally (edge continuity / FFT), not by a model.
- **3D multiview captures category, not material/topology.** It reliably gives "chair / sword / tree /
  vehicle"; it does **not** capture PBR detail, topology, rig, or interior geometry — read those from
  the file as structured tags.
- **Audio has no direct cloud-audio path in the default provider** — local CLAP/PANNs/Whisper produce
  the description; the API (if enabled) only normalizes/enriches the resulting **text** tags.
- **Native-3D similarity upgrade path**: OpenShape (CLIP-aligned 3D embeddings) can later supplement
  multiview for shape-similarity that survives render/texture differences — it drops into the same
  vector space. Served by the optional Python sidecar if added.

---

## 4. Runtimes

| Path | Runtime | Use |
|---|---|---|
| Discrete encoders/taggers (SigLIP 2, WD, CLAP, YAMNet) | **ONNX Runtime via `ort`** (CUDA / DirectML / CPU EPs) | Primary local inference; fast, small footprint, no Python |
| Rust-native embeddings | **candle** / `FastEmbed` / `EmbedAnything` | Alternative to ort; pure-Rust single binary |
| Vision-LLM captions | **Ollama** (HTTP): moondream2 (low-VRAM), Llama 3.2-Vision, Qwen-VL | Rich free-form descriptions, offline |
| Speech | **`whisper-rs`** / whisper.cpp | Transcribe detected speech → searchable text + "voice" tag |
| PyTorch-only models (OpenShape, BLIP-2) | optional **Python sidecar** | Only if/when added; off the critical path |
| Online API | HTTPS | Opt-in cloud tagging |

---

## 5. Online API option (opt-in; first provider: OpenAI-compatible)

API tagging is **opt-in enrichment, never required** — the local Ollama VLM path already tags
images/3D/material/font offline for free, so the cloud path exists only to buy better tags when a
user wants to spend for them.

**The first concrete provider is the OpenAI chat-completions + vision shape**, not Claude-native, and
the reason is reach-per-line: one impl (`base_url` + `api_key` + `model`) talks to **OpenAI, Google
Gemini** (its OpenAI-compatible endpoint), **Groq, OpenRouter, and any local OpenAI-API server**
(LM Studio / llama.cpp / vLLM) — including the generous **free tiers** (Gemini Flash, Groq,
OpenRouter free models) that make cloud tagging affordable at all. Claude's native Messages API is a
different request shape and lands later as a second impl behind the same `ApiTagger` trait for users
who specifically want Anthropic. The default `base_url` points at Gemini's OpenAI-compatible endpoint;
a local server URL is a first-class alternative (zero cost, fully private).

> Verify model IDs / pricing / free-tier limits / vision request shape against each provider's
> current docs before relying on them — do not hard-code from memory. The request is a standard
> `POST {base_url}/chat/completions` with the image as a base64 `image_url` content block and the
> shared instruction prefix reused across the batch (prompt-cacheable where the provider supports it).
> For Anthropic specifically, the bundled **`claude-api`** skill is the reference. Cost-sensible
> defaults: a small/flash model for bulk; **scoped runs** (Unidentified/Selected only) and a per-run
> asset cap keep spend bounded; a Batches path (~50% cost) is a later add.

The provider is abstracted (`ApiTagger`), so Claude-native / others are drop-in alternatives. Audio
has **no cloud path** in any provider (§5, above): it goes through the local CLAP/Whisper
description pre-step regardless, and the API only ever enriches text.

---

## 6. Tag taxonomy & human-in-the-loop

- **Two tiers**: free-form tags (high recall, messy) from WD/VLM/Claude, and a **controlled
  vocabulary** mapped onto them for clean faceting (the "mapping library" — see
  [`DATA-MODEL.md`](DATA-MODEL.md)).
- **Normalize**: lowercase, singularize, strip Booru underscores, apply a synonym/alias map, and
  embed tag strings to auto-cluster near-duplicates.
- **Confidence**: WD/PANNs/YAMNet emit native per-tag probabilities — store the raw score and apply an
  adjustable threshold (~0.35 default). For VLM/Claude, request a confidence per tag in the schema.
- **Corrections**: tags shown as editable confidence chips with accept/reject. Two payoffs —
  corrected tags improve search immediately, and the accept/reject log becomes training data to tune
  thresholds or fine-tune a small local tagger over time.
- **User traits teach the identifier**: traits from the Trait Library
  ([`UI-SPEC.md`](UI-SPEC.md) §9; `tag.is_trait/scope/usage_note/teach`, [`DATA-MODEL.md`](DATA-MODEL.md))
  with `teach=1` are injected into the tagger's candidate vocabulary (and, for VLM/Claude paths, the
  prompt — the `usage_note` is the when-to-apply guidance), scoped per modality — so future scans tag
  with the user's words, not just the model's. Paused traits are excluded but keep existing tags.

---

## 7. Performance (10k–100k assets)

- **GPU is 10–20×** faster than CPU for all these models. Rough GPU budgets: SigLIP-B a few ms/img;
  WD-SwinV2 ~100–150 ms, WD-EVA02-L ~200–300 ms; CLAP tens of ms/clip; 3D dominated by **render time**
  (keep view count small, cache renders).
- **Batch** encoders (32–256/batch) on GPU; decode/resize on worker threads to keep the GPU fed.
- **Idempotent caching**: key every result by `content_hash + model_id + model_version`. Never
  recompute unchanged assets; recompute only on model-version change. Persist the ANN index to disk and
  rebuild incrementally.
- **API bulk**: use the Batches API (~50% off) + prompt caching of the shared instruction prefix.
- 10k images finish in minutes (embeddings) to ~1 hr (heavy tagger + VLM captions) on a single
  consumer GPU; audio is fast; 3D is gated by render time → cache the renders.

---

## 8. Why this serves retrieval

Joint-space embeddings turn natural-language queries ("low-poly stylized rock", "metallic footstep
sfx", "stylized fantasy sword") directly into vector searches over the asset library, and the
controlled-vocabulary mapping makes tag/category filters precise. Together with FTS keyword search and
RRF fusion (see [`DATA-MODEL.md`](DATA-MODEL.md)), this is the identification engine the whole tool is
built around.
