# Identification & Search

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

How Lodestash turns raw assets into richly, *consistently* described records, and the **three ways to
peruse them** — by tags, by plain language, or via an AI/MCP agent. This is the engine the whole tool
is built around; it ties together [`AI-PIPELINE.md`](AI-PIPELINE.md) (models),
[`DATA-MODEL.md`](DATA-MODEL.md) (schema + hybrid search), [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md)
(local model choice), and [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) (the agent surface).

**Proposed agent search/analysis slice (2026-09-10):** see
[MCP-ASSET-ANALYSIS](MCP-ASSET-ANALYSIS.md) and the exact filters/cursor rules in ENGINE-API §8.1.
Library source, package, media and tag eligibility must be applied before search limits and ranking.
Search returns candidates; ordered browsing covers a library, and random auditing samples the full
eligible source. Tag/structural inspection does not require embeddings; semantic search still does.

> Claude-specific facts below (model IDs, structured outputs, batch, caching) were verified against the
> `claude-api` reference; re-verify in-app before shipping, as they move.

---

## 1. The shape of it

```
                       ┌──────────────── Identification engine (lodestash-ai) ───────────────┐
  raw asset ──────────▶│ technical facets → embeddings → discrete tags → structured facets   │
                       │ → normalize + map → status: discovered→indexed→identified→confirmed  │
                       └───────────────────────────────┬────────────────────────────────────┘
                                                        ▼  (one catalog: facets, tags, embeddings)
                       ┌──────────────── Hybrid search core (lodestash-core) ────────────────┐
                       │ structural pre-filter → BM25 ⊕ vector KNN → RRF → (optional rerank)  │
                       └───────┬───────────────────────┬───────────────────────┬─────────────┘
                       Search by Tags          Search with AI            AI / MCP agent
                       (exact, structural)     (plain language)          (reason + tools)
```

The optimization that makes this work: **identification produces a *structured, faceted* description**
(not a flat bag of words), so all three surfaces can filter precisely *and* match semantically over the
same records.

---

## 2. The faceted identification schema (optimized for AI tagging)

Instead of asking a model for "some tags," Lodestash asks every tagger — local or cloud — to fill the
**same structured schema** of independent **facets**. Faceted classification is the dominant model for
modern asset discovery: multiple independent axes the user combines to narrow results. Consistent
facets are what let "Search by Tags" be precise and "Search with AI" turn language into cheap filters.

| Facet (`tag.namespace`) | Meaning | Example values | Source |
|---|---|---|---|
| `subject` | what it *is* | beaver, sword, crate, footstep | AI (vision/VLM/WD) |
| `style` | visual/aural style | low-poly, stylized, realistic, pixel-art, hand-painted, retro | AI |
| `material` | surface/substance | wood, metal, stone, fabric, brick, ORM-map | AI + filename heuristics |
| `color` | dominant + palette | + `asset.dominant_color` hex | derived + AI |
| `mood` | theme/feel | dark, cheerful, horror, medieval, cyberpunk, fantasy | AI |
| `usage` | role in a game | character, prop, environment, UI, VFX, weapon, vehicle, terrain | AI + mapping |
| `audio` | sound nature | sfx, music, voice, ambient, loopable; event (explosion, wind) | CLAP/PANNs/Whisper |
| `technical` | measurable facts | tris bucket, has-rig, has-animation, 2K, 48kHz, tileable, urp/hdrp/built-in | **file-derived (no model)** |
| `free` | anything else | high-recall catch-all | AI |

- Every **AI** facet value carries a **confidence** (`asset_tag.confidence`) and `model_id`; technical
  facets are exact and unscored.
- **`tag.norm_name` is namespaced**: `'<namespace>:<lowercased value>'`. The column is UNIQUE across
  the whole table, and `metal` is a legitimate `material` *and* a legitimate `mood` — without the
  prefix the second facet would silently attach to the first one's row. `tag.name` stays the plain
  display value.
- **Tags are written per `source`.** `Catalog::set_tags(asset, source, …)` replaces only what that
  source wrote, so re-running the tagger never touches the user's own tags — which is what the
  `source` column in `asset_tag`'s primary key is for.
- Facets map onto the existing `tag` table via `tag.namespace` and `asset_tag` — **no new tables**. The
  controlled-vocabulary **mapping library** (DATA-MODEL §4) sits on top, normalizing facet values onto
  canonical project categories.
- The schema *is* the contract: the local VLM, the WD tagger (mapped into facets), and the Claude API
  all emit the same shape, so search behaves identically regardless of which backend identified an asset.

### The canonical tagger output (the contract, concretely)

Every tier-3 tagger — local VLM (Ollama), WD-mapped, or Claude — must emit exactly this JSON shape
(via structured output where the backend supports it, §4):

```jsonc
{
  "subject":  [ { "value": "beaver",   "confidence": 0.93 } ],
  "style":    [ { "value": "low-poly", "confidence": 0.88 },
                { "value": "stylized", "confidence": 0.72 } ],
  "material": [ { "value": "wood",     "confidence": 0.41 } ],
  "mood":     [ { "value": "cheerful", "confidence": 0.66 } ],
  "usage":    [ { "value": "prop",     "confidence": 0.81 } ],
  "free":     [ { "value": "cartoon animal", "confidence": 0.70 } ],
  "caption":  "A low-poly cartoon beaver suitable for a stylized forest scene."
}
```

Rules the pipeline enforces (never trust the model):
- **`color`, `technical`, and `audio` facets are never requested from a vision model** — color and
  technical facts are derived deterministically (tier 0) and audio facets come from the local
  CLAP/PANNs/Whisper pre-step; asking a VLM for them only invites hallucination.
- **Validate then write:** unknown facet keys are dropped, values are normalized
  ([`AI-PIPELINE.md`](AI-PIPELINE.md) §6: lowercase, singularize, de-Booru, alias merge), confidence
  clamped to `[0,1]`, and per-facet value counts capped (≤ ~8) before anything lands in the catalog.
- **Storage mapping:** each value becomes one `tag` row (`namespace` = facet) + one `asset_tag` row
  (`source='ai'`, `confidence`, `model_id`); `caption` fills `asset.description` (FTS-searchable).
- **Confidence is the model's self-estimate**, calibrated over time by the accept/reject log; the UI
  threshold (~0.35) *hides* low-confidence tags, it never deletes them.
- Non-schema backends map in: WD's flat scored labels are routed into facets by a static
  label→namespace table; anything unroutable goes to `free`.

---

## 3. The identification pipeline (optimized, tiered)

Per asset, cheapest-first so assets become useful fast and quality upgrades in the background:

| Tier | Step | Cost | Makes it searchable by |
|---|---|---|---|
| **0** | **Technical facets** — poly/verts/materials/bones, dims/channels/colorspace, duration/rate, `dominant_color`, tileability, PBR-map role | trivial, deterministic | tags (technical), color |
| **1** | **Embeddings** (always local) — SigLIP 2 (image / 3D-multiview), CLAP (audio) → `vec_asset` | light | **AI / plain language** |
| **2** | **Discrete tags** — WD v3 (image), YAMNet/PANNs (audio) → scored tags | medium | tags |
| **3** | **Structured facets** — fill the §2 schema via local VLM (Ollama), or Claude vision (API), or derive from tiers 0–2 | heavy / optional | every facet |
| **4** | **Normalize + map** — facet cleanup, alias merge, mapping library → `asset_category`; `status → identified` | light | category |
| **5** | **Confirm** — human accept/reject on confidence chips → `status → confirmed`; log trains thresholds | — | trusted |

**Optimizations baked in:**
- **Idempotent caching** — every result keyed by `content_hash + model_id + model_version`; unchanged
  assets are never recomputed; identical content across packs is computed **once** and shared.
- **Tiers 0–2 alone make an asset fully searchable** (name + technical + tags + embeddings); the
  expensive tier-3 facet pass is queued and optional — quality, not a gate.
- **Batching** — GPU encoders run in batches (32–256); the API path uses the **Message Batches API
  (~50% cost)** + **prompt caching** of the shared schema/instruction prefix (see §4).
- **Local-default, API-optional** — embeddings always local (keeps the index offline & consistent);
  tier-3 tagging is the swappable `local | api` knob, per modality.

---

## 4. The Claude (API) identification path — exact contract

When a user opts tier-3 tagging to the cloud, the default provider is Claude. Verified mechanics:

- **Model choice:** **Haiku 4.5** ($1/$5 per MTok) for high-volume bulk tagging, **Sonnet 4.6**
  ($3/$15) for quality, **Opus 4.8** ($5/$25) reserved for hard/ambiguous assets. Chosen per job.
- **Vision input:** send rendered previews as **image content blocks** — base64 (`{"type":"image",
  "source":{"type":"base64","media_type":"image/png","data":…}}`) or URL. Works for image assets,
  **3D multiview renders**, and an **audio waveform/spectrogram** image.
- **Structured output (not prefill):** request the §2 schema via
  `output_config={"format":{"type":"json_schema","schema":{…,"additionalProperties":false}}}` (or
  `messages.parse()` with a typed model). **Assistant prefills 400 on current models** — do not use them.
- **Bulk:** the **Message Batches API** runs at **~50% cost**, up to 100k requests / 256 MB, results
  keyed by `custom_id` (unordered — never match by position), and supports vision + caching.
- **Prompt caching:** put the long, stable **system + JSON-schema + tagging instructions** first with a
  cache breakpoint; only the per-asset image varies. Across a 10k-asset run this is the dominant saving.
  Verify with `usage.cache_read_input_tokens`.
- **Audio:** no direct audio input — local Whisper/CLAP/PANNs produce the description; Claude only
  enriches/normalizes the resulting **text** facets.
- **Swappable:** all of this sits behind the `ApiTagger` interface ([`AI-PIPELINE.md`](AI-PIPELINE.md)
  §2), so Gemini/GPT are drop-in alternatives.

---

## 5. Three ways to peruse

All three reduce to the one hybrid core (DATA-MODEL §3); they differ in how the query is formed.

### A. Search by Tags — exact, structural, explainable
Pure structural retrieval, **no vector step**: FTS5/BM25 over names + tags + description, plus
**faceted filters** (`namespace=value`), media-type (Models/Sounds/Images), mapped category, numeric
ranges (tris, size, duration, resolution), color, status, package. Fast, deterministic, and you can see
*why* each result matched. UI: a faceted sidebar (one group per `namespace`), color-coded chips.

### B. Search with AI — plain language (the "deeply efficient" path)
Two stages, so the embedding does the semantic lift while constraints stay cheap:

1. **Query understanding (self-query).** A small local LLM (or Claude Haiku) parses the natural-language
   query into `{ semantic_text, filters[] }` against the known facet/field grammar — e.g.
   *"low-poly stylized rock under 5k tris"* → `semantic_text:"stylized rock"`,
   `filters:{ media_type:model_3d, style:low-poly, tris:<5000 }`. This is the established
   *self-query / hybrid-structured* retrieval pattern: pull hard constraints out of the prose and apply
   them as metadata filters; embed only the residual concept. *(Optional — with no LLM available it
   degrades to pure embedding search.)*
2. **Hybrid retrieval.** Apply the parsed filters as a **structural pre-filter** (shrinks the candidate
   set, keeps it fast), then **vector KNN** on `semantic_text` (joint SigLIP/CLAP space) ⊕ **BM25**,
   fused with **RRF**, optional cross-encoder rerank. Returns ranked matches — *or nothing*, when you
   genuinely own nothing close.

> Why this is fast at 100k+: the parser converts "under 5k tris" into a cheap indexed filter instead of
> hoping the vector encodes it; pre-filtering means the ANN only scores a small candidate set
> (sqlite-vec partition keys + metadata filter in one query, DATA-MODEL §2). The query parse itself is
> cached by query string.

### C. AI / MCP agent — reason, look, refine, retrieve
An MCP client (Claude, Claude Code, Cursor) drives the same catalog through tools, decomposing the
query *itself* (no separate parser) and iterating with vision over previews:
`semantic_search` → `get_asset_preview ×N` (it can *see* the thumbnails/turntables/waveforms) →
refine → `stage_asset` → `import_to_project`. Identification is also agent-drivable — expose
`identify_asset` / `suggest_tags` so an agent can fill or correct facets. Full tool list and the import
round-trip are in [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md).

---

## 6. The query grammar (shared by B and the agent)

The filterable surface the parser (and `search_assets`) target — keep it small and validated:

- **Enums (facets):** `subject`, `style`, `material`, `mood`, `usage`, `audio.*` — matched against the
  known vocabulary; unrecognized terms fall back into `semantic_text`.
- **Numeric ranges:** `tris`, `verts`, `materials`, `bones`, `size_bytes`, `duration_ms`, `resolution`,
  `rating` (1–5 stars; [`DATA-MODEL.md`](DATA-MODEL.md) §12).
- **Categorical:** `media_type` (model_3d/texture/audio/image/sprite/material/font), mapped `category`,
  `package`/`publisher`, `status`, `color` (≈ `dominant_color`), `pipeline` (built_in/urp/hdrp).
- The parser emits this as JSON via the **same structured-output mechanism** (§4) — local model or
  Haiku — then Lodestash **validates and clamps** it against the live vocab before querying, so a
  hallucinated facet can never produce a wrong-but-confident filter.

---

## 7. Cohesion with the architecture

| Concern | Where it lives |
|---|---|
| Taggers/embedders, the §2 schema, `local`↔`api` | `lodestash-ai` ([`AI-PIPELINE.md`](AI-PIPELINE.md)) |
| Facets/tags/embeddings storage, hybrid search, mapping | `lodestash-core` + SQLite ([`DATA-MODEL.md`](DATA-MODEL.md)) |
| Query understanding (parser) | `lodestash-core` search service (local LLM or Haiku) |
| Tags / AI / agent surfaces | GUI (`lodestash-desktop`), MCP (`lodestash-mcp`), REST (`lodestash-api`) |
| Which local models per GPU | [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) |

**Bottom line:** one identification engine fills one faceted, embedded catalog; three surfaces read it.
Tags give exactness, embeddings give plain-language recall, the query parser fuses the two cheaply, and
MCP lets an agent do all of it with vision in the loop — local-first, with the cloud as an opt-in
quality lever.
