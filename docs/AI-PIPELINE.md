# AI Tagging & Embeddings

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

How assets are auto-identified (tags) and made semantically searchable (embeddings), with a clean
**local ↔ online-API** switch. Embeddings power the "search by description" that makes retrieval fast,
so this subsystem exists to serve search.

---

## 1. Shape of the pipeline

**External LLM authoring (proposed 2026-09-10):** the connected MCP host can analyze selected
asset evidence and submit tags plus an optional scalar description. This complements the app's
internal `identify_asset` pipeline; it does not run embeddings or require Lodestash to call a second
LLM. [MCP-ASSET-ANALYSIS](MCP-ASSET-ANALYSIS.md) owns the workflow and ENGINE-API §8 owns validation,
producer attribution and byte consent. Stored `suggest_tags` results are not newly inferred labels.
External annotations must not clear internal model work or manufacture human confirmation. Model
confidence may be unknown/null; provenance, rather than a confidence value, determines AI authorship.
Remote audio-byte exclusion and destination-based consent follow S10 of the existing v1 plan;
these proposed checks must cover resource/file delivery as well as preview images.

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
pass to that selection. AI tags are written `source='ai:<producer>'` ([ADR-0017](decisions/ADR-0017-producer-scoped-atomic-tag-batches.md)), so any re-run replaces only that producer's tags and
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

**Zero-shot rides on authored pixels, never on our own render.** The SigLIP label pass
(`zeroshot_tags`) runs for images, textures, sprites and materials — not `model_3d`. A model's
stored vector is a mean-pooled multiview of an untextured, flat-shaded orthographic render (the
proxy loader returns geometry only, cache hit or miss, so the same bytes always encode the same
picture — v1 plan S08), far
outside SigLIP's training distribution: over 20 models (2026-08-30) only ~20% produced any label
and half of those were wrong, against 4/4 correct and confident on real texture pixels. Models are
tagged by the captioning path instead, which reads the same preview as prose and does well with it.

**Calibration is not a detail — it was the difference between working and silent.** `LOGIT_SCALE`
is 100, the temperature CLIP-family models are trained at. At the earlier 30 the tagger wrote
**zero** tags in the entire reference catalog: SigLIP's cosines sit in a narrow band (a real facet
topped out at 0.113 against 0.078 and 0.063), and at that scale the per-facet softmax came out
near-uniform, so nothing ever cleared `MIN_PROB`. If a future model changes the spread, re-measure
before assuming the gate is wrong.

**For flat art, the name outranks the picture — for a 3D model it does not.** Measured on the
Artificer pack (2026-08-30): `round crates.png` is a texture **atlas** — wood planks and two rope
strips laid out for UVs, with no crate in the image at all. `qwen2.5vl:7b` describes it accurately
as *"a rustic wooden door with horizontal planks and rope accents"*, and the facet model then wrote
`subject:wooden door` onto an asset the artist named *round crates*. Four of five samples failed
that way (`sacks` → seed packet, `wheelbarow` and `bucket` → wooden crate); the one **3D model** in
the set, whose preview is a render of the object itself, came out right. So the facet prompt now
carries one extra clause for `texture`/`image`/`sprite`: the pixels are evidence about the surface,
the filename is the evidence about the subject. A/B against the live model, same caption, prompt
the only variable:

| asset | without the clause | with it |
|---|---|---|
| `round crates` | `subject: door` | `subject: crates` |
| `bucket` | `subject: crate` | `subject: bucket` |

This is **not** a model-size problem and a bigger VLM does not fix it — the information is not in
the image. It is also why upgrading moondream → `qwen2.5vl:7b` improved the 3D case (a labelled
beer crate went from *urn* to *wooden crate*) while leaving the texture cases wrong.

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
| `ApiTagger` | Cloud/remote vision LLM over the **OpenAI-compatible wire format** (vendor-neutral — **no OpenAI account required**; reaches Gemini/Groq/OpenRouter free tiers and any local OpenAI-API server, paid vendors only if opted in); Claude-native is a later drop-in. JSON facet response. |
| `LocalEmbedder` | SigLIP 2 (image) · CLAP (audio) · multiview-SigLIP (3D) — via `ort`/candle |
| `ApiEmbedder` | provider embeddings *(optional — see policy)* |

**Policy:**
- **Embeddings always run locally** (cheap, private, keeps one consistent offline-searchable index).
- **Tagging** is the user-switchable knob (`local` | `api`), configurable **per modality**.
- The embedder is chosen **per modality as a unit** (text+asset towers must match); you cannot mix a
  local image embedder with an API text embedder.

### 2.1 Local tagging strategies (why more than one)

The local tagger is not a single model but a small set of **complementary strategies**, because each
covers a different weakness — and the two highest-leverage ones reuse infrastructure already in the
box, so they cost a label list, not a new dependency:

- **Discrete multi-label (WD / RAM).** `WD-EVA02-Large-v3` (ONNX, via `ort`) emits hundreds of
  Booru-style tags with native probabilities — dense recall, no LLM, no two-model caption dance.
  Strong on stylized/anime game art; RAM++ is the broader open-set option for realistic props. This
  is a **primary local image/3D tagger**, not a fallback: for many libraries it out-tags the VLM
  caption path and is faster.
- **Zero-shot vs a controlled vocabulary (CLIP/SigLIP for image/3D, CLAP for audio).** Score the
  asset embedding against label-prompt embeddings and keep the closest per facet — the exact
  temperature-scaled-softmax method already **shipped for audio** (`clap_tags`, §6). The image/3D
  version reuses the **SigLIP embedder already loaded for search** — zero new model, zero download —
  and is the natural home for **teaching vocabulary** (§6): a user's own trait becomes a candidate
  label, so their words win without prompt-engineering a VLM.
- **VLM caption → text-facet (Ollama two-stage, shipped).** The rich free-form path (§2 above): one
  sentence of description (→ `asset.description`, FTS-indexed) plus a text model mapping it to the
  schema. Best when you want prose/novel concepts the fixed vocabularies above can't name.

These are **union, not exclusive** — WD gives recall, zero-shot enforces the controlled/teaching
vocabulary, the VLM adds description; `validate` + `set_tags` dedupe and cap across all of them. The
API path (§5) is a fourth strategy the user can switch *in* per modality, never a prerequisite.

---

## 3. Per-modality defaults

| Modality | Embedding (local, always) | Tags (local default / API option) |
|---|---|---|
| **Image / texture** | **SigLIP 2 So400m** (768-d; ViT-B fallback on weak CPUs) | **WD-EVA02-Large-v3** (ONNX, dense multi-label) **+ SigLIP zero-shot** over the controlled/teaching vocab (§2.1) / Ollama VLM caption / OpenAI-compatible vision JSON (opt-in). Textures: embed the **albedo** channel; detect other maps by filename and merge as one material. |
| **Audio** | **LAION-CLAP** (512-d) | **YAMNet/PANNs** event tags + CLAP zero-shot description; **Whisper** only when VAD detects speech / API enriches text tags |
| **3D model** | **multiview render (4–8 views) → SigLIP 2**, mean-pooled & renormalized | **WD tagger + SigLIP zero-shot** on the rendered views (vote/union) + **file-derived structured tags** (poly count, material slots, bone count) / OpenAI-compatible vision on the views (opt-in) |
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

## 5. Online API option (opt-in; OpenAI-compatible wire format)

API tagging is **opt-in enrichment, never required** — the local Ollama VLM path already tags
images/3D/material/font offline for free, so the cloud path exists only to buy better tags when a
user wants to spend for them.

**"OpenAI-compatible" names a wire format, not a vendor account.** The `ApiTagger` speaks the
`POST /chat/completions` + base64 `image_url` shape, so it is provider-agnostic by construction: an
OpenAI account is **never** required, and none of the reachable endpoints are OpenAI's own. One impl
(`base_url` + `api_key` + `model`) talks to whatever server answers that shape — the practical set is
**free tiers** (Google **Gemini Flash**, **Groq**, **OpenRouter** free models), a **local
OpenAI-API server** (LM Studio / llama.cpp / vLLM — zero cost, fully private), and paid vendors only
if the user opts in. The default `base_url` points at Gemini's OpenAI-compatible endpoint (free tier,
no card); a local server URL is a first-class alternative. Claude's native Messages API is a
different request shape and lands later as a second impl behind the same `ApiTagger` trait for users
who specifically want Anthropic.

**Endpoint presets (UI spec).** Because the raw base_url reads as "insert OpenAI key" to a new user —
the exact opposite of the truth — the CLOUD settings card presents a **preset picker**, not a bare
URL field: `Local server · Gemini (free) · Groq (free) · OpenRouter · Custom…`. Selecting a preset
fills `base_url` and hints a sensible vision-model id; **Custom** exposes the raw field for power
users. This is pure client wiring over the same `base_url` the tagger already accepts — no backend or
trait change — and it is the primary lever that makes the free/local paths discoverable. See
[`MODEL-GUIDE.md`](MODEL-GUIDE.md).

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

### 5.0 What may leave this machine (v1 plan S10, [ADR-0020](decisions/ADR-0020-egress-is-a-parsed-endpoint-and-a-per-modality-consent.md))

Three separate questions, answered separately:

1. **Where is the endpoint?** The host is parsed out of the URL (`lodestash_ai::endpoint`). Loopback
   literals and the reserved `localhost` name are this machine; everything else is a network someone
   else runs. `https://localhost.example.com/v1` is remote, and the old substring test called it
   local.
2. **Does it bill?** A separate answer (`openai::is_free_endpoint`). A free remote endpoint is still
   remote; a self-hosted server that costs nothing is still the one place bytes go without asking.
3. **May these bytes go there?** `lodestash_app::egress::may_send`, per modality, default off.
   Images — and the pixels derived from models, materials and sprites — need explicit consent
   (`remote_image_bytes`). **Audio bytes never leave**, consent or not, and neither do pixels derived
   from audio: a spectrogram is evidence about the clip, so routing it to a remote model would be the
   prohibition with an extra step. A remote model may receive text about audio, nothing more.

A refusal happens **before any request is built**: the cloud pass stops before it renders a preview,
so a denied modality produces no outbound traffic to observe. Redirects are refused outright — the
request carries the key and the picture, and a `3xx` would hand both to whatever origin the response
names.

### 5.1 Reliability & cost controls

The cloud path is designed for the **free tiers first** (Gemini/Groq/OpenRouter), and free tiers
throttle hard — Gemini free is on the order of ~15 requests/minute. A naive tight-loop drain sprays
429s and silently loses those assets, so the design mandates:

- **Rate-limit backoff + resume.** On `429`/`503` (honour `Retry-After` when present), back off
  exponentially with jitter and retry a bounded number of times; a call that still fails leaves the
  asset **`tags_dirty` = 1** so the next run picks it up. An asset is only cleared once it is actually
  tagged — a drain must never mark a rate-limited asset "done, no tags." (Contrast the *unsupported*
  case, e.g. audio through a vision API, which is cleared deliberately to avoid a spin loop.)
- **Attribution survives the fallback.** When the endpoint fails and a local model substitutes, its
  tags are stored under the *local* producer and model, never the failed endpoint's; the SigLIP
  zero-shot proposals always go under their own producer (`ai:siglip-zeroshot`). Before M26 both
  were merged under the endpoint's model id, which is why those rows are now `ai:legacy`.
- **Local fallback on failure.** A hard API error (bad key, exhausted quota, network) falls back to
  the **local Ollama VLM** for that asset when a local model is available; if none is, the asset stays
  dirty and a per-run **failure count** surfaces in the job tray. Either way the asset is never left
  looking complete-with-no-tags — the whole point of `confirmed` vs `identified` (DATA-MODEL §8)
  depends on "untagged" meaning "not yet done," not "silently failed."
- **Connection test + vision preflight.** The CLOUD card exposes a **Test** action that runs the
  tagger against one bundled sample image and shows the raw result or the raw error. This is the
  single cheapest fix for the #1 failure mode — a wrong base_url, a non-vision model id, or a dead key
  produces *nothing* from a bulk run with no signal. Test surfaces it in one click before any spend or
  any large pass.
- **Budget guard.** Scoped runs (New/Unidentified/Selected) already bound *what* is tagged; on top of
  that a **hard per-run asset cap** and, for any **non-free preset**, a pre-run confirm showing
  *N assets ≈ M API calls* (and an est. cost line when the provider's pricing is known) prevent a
  mis-clicked **Full Run** on a paid endpoint from becoming a surprise bill. Free/local presets skip
  the confirm.
- **Key at rest.** `api_key` is the one secret this local-first app holds; store it in the **OS
  credential store** (Windows Credential Manager) when available, with a plaintext-config fallback so
  the app still works headless. Never logged, never sent anywhere but the chosen endpoint.
- **The idle lane never spends money.** A cloud pass is always an explicit, scoped, capped kick —
  the background auto-identify slice does not tag when an API model is configured. Two reasons, and
  either alone is sufficient: a lane that bills an endpoint while the user is not looking is not
  something a budget guard can bound, and the off-writer slice can only reach the generic batch path,
  which cannot drive an image-reading tagger (below).
- **A tagger that reads the pixels itself cannot run on the generic batch path.** The two-phase shape
  above means that path never fills `TagInput::image` — the caption carries the picture — so
  `Tagger::needs_image()` marks the taggers that must go through `drain_api_tags` instead, and the
  batch path refuses them rather than storing their emptiness as an answer.

> **The defect this section was written against, made real (2026-09-06).** `OpenAiTagger` never
> overrode `caption()`, and the live drain was the generic one, so the cloud path short-circuited on
> the missing image and returned an empty result **without making a single HTTP request** — which the
> caller then wrote as a finished tagging, clearing `tags_dirty`. Switching on cloud tagging marked
> every asset *done, no tags* and drained the Identify backlog for good. Every mandate above already
> existed; `drain_api_tags` implemented backoff, resume, fallback and the cap, and had **no callers**.
> The fix routes the kick path to that drain, marks the tagger `needs_image()`, and M24 re-queues
> what was swallowed (`DATA-MODEL.md` §13).

---

## 6. Tag taxonomy & human-in-the-loop

**Proposed MCP review (2026-09-10 clarification):** Codex reads current assignments/previews and
vocabulary, runs identification for selected IDs, then assesses assigned tags using trait scope and
usage guidance. Acceptance/rejection is an agent review, not human-confirmed status. Unknown concepts
can become approved traits or taxonomy categories through ENGINE-API §8.5; they cannot silently
create new namespaces. `teach` governs future model candidates, not whether a trait is permitted.
Open, controlled and approved-extension policies are app-selected; rejection of one AI association
does not blacklist a term or suppress future model output. See [MCP-ASSET-ANALYSIS](MCP-ASSET-ANALYSIS.md).

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
