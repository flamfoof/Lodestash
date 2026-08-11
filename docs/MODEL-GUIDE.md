# Model Guide — which AI model should I pick?

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

A plain-language companion to the identification settings. It answers one question: **"I want good
tags on my assets — what do I turn on?"** Organised by the kind of asset (image, 3D, audio) and by
whether you want to run **locally (free, private)** or through a **cloud/API model (costs money, but
often better and needs no GPU)**.

> If you read nothing else:
> - **Just want it to work, free, offline?** Install [Ollama](https://ollama.com), pull
>   **`moondream`**, pick it in *Settings ▸ AI · IDENTIFY*. Done.
> - **Want better tags with no GPU?** Turn on *Settings ▸ AI · IDENTIFY (CLOUD)*, pick the
>   **Gemini (free)** preset, paste a free-tier key, set a flash-tier vision model. Done.
>   **No OpenAI account is needed** — "OpenAI-compatible" is just the request format; the free
>   tiers (Gemini/Groq/OpenRouter) and your own local server all speak it.
> - **Audio always stays local** (Ollama/CLAP) — there is no cloud-audio path, by design.
> - Model names and prices change. **Verify the current model IDs / free-tier limits** on the
>   provider's own page before relying on them; the names below are guidance, not gospel.

The two settings cards mirror this guide:

| Card | What it drives |
|---|---|
| **AI · IDENTIFY** | Local tagging via **Ollama** (a model already on your machine) |
| **AI · IDENTIFY (CLOUD)** | Optional cloud tagging over the **OpenAI-compatible** format — *no OpenAI account needed* (Gemini free / Groq free / OpenRouter / your own local server) |

There is also a per-VRAM deep-dive in [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) if you want to push
local quality as far as your GPU allows. This page is the friendly "just tell me what to pick" version.

---

## 🖼️ Images & textures

The most common asset, and the easiest to tag well.

| Goal | Local pick | Cloud pick |
|---|---|---|
| **Best default** | **`moondream`** (~1.7 GB) in Ollama — captions the preview, a text model turns it into traits | **Gemini Flash** (free tier) via the CLOUD card |
| **Most tags, no LLM, no GPU** | **WD-EVA02-Large (v3)** — a small ONNX tagger that emits hundreds of labels with scores; great for stylized/anime art. Or **SigLIP zero-shot** to tag against *your own* trait vocabulary (reuses the search model — nothing to install) | — (local already covers this well) |
| **Higher quality, have VRAM/patience** | **`qwen2.5vl:3b`** or **`llava`** (7B, needs ~6–8 GB) | **GPT-4o-mini** class / **Claude Haiku** class — cheap, strong |
| **Fastest cloud, free** | — | **Groq** vision model, or **OpenRouter** free model |

**Textures** tag the same way — the albedo/base-colour map is what gets described; normal/roughness maps
are detected by filename and merged, not tagged separately.

---

## 🧊 3D models

3D reuses the **image** models: Lodestash renders a few turntable views of the mesh and tags those,
then merges file-derived facts (poly count, material slots) it already knows.

| Goal | Pick |
|---|---|
| **Best default** | Same as images — **`moondream`** locally, or **Gemini Flash** in the cloud |
| **Sharper results** | A larger VLM (`qwen2.5vl`, `llava`) locally, or a mid cloud model — more views resolve better |

> You do **not** pick a separate "3D model." If image tagging is set up, 3D tagging works. The only
> extra cost is render time for the preview views, which Lodestash caches.

---

## 🔊 Audio

Audio is **always local** — no provider offers a cloud path Lodestash uses, so your clips never leave
the machine. Three complementary pieces, in order of what ships today:

| Piece | What it does | Status |
|---|---|---|
| **CLAP zero-shot** (shipped) | Matches a clip against a sound vocabulary (explosion, footsteps, music, ambient, voice…) in a joint audio↔text space | **On now** — comes free with the audio embedder; no extra model to install |
| **PANNs / YAMNet** | Discrete sound-event tags (hundreds of AudioSet classes) — PANNs (CNN14) is the quality pick, YAMNet the tiny/fast one; both ONNX, free, CPU | **Next audio slice** (specced) |
| **Whisper** | Transcribes **spoken** audio → searchable text + a `voice` tag | Planned (only for voice/dialogue clips) |

**What to do today:** nothing extra. If the audio embedder is set up (part of *Search with AI*),
running Identify tags audio automatically via CLAP zero-shot. Music, SFX, and ambience get useful
subject/usage/mood traits out of the box.

---

## Local vs. cloud — how to choose

| You care most about… | Choose |
|---|---|
| **Privacy / zero cost / offline** | **Local** (Ollama). Nothing leaves your PC. |
| **Best tags, no GPU, don't mind a bill** | **Cloud**. Start with **Gemini free tier** — often no bill at all for a hobby library. |
| **A big one-time bulk run** | Cloud with a **cheap flash/mini model**, scoped to *New* or *Unidentified*, or overnight locally. |
| **Running your own server** | Point the CLOUD card's **Endpoint** at your local **LM Studio / vLLM** URL — OpenAI-compatible, free, private, and as big a model as your hardware allows. |

**Cost control (cloud):** the CLOUD card's **Run on New** / **Run on Unidentified** buttons (and
right-click ▸ *Identify selected*) exist so you never pay to re-tag what's already done. A flash/mini
model plus scoped runs keeps a big library to pennies — but **check the provider's current pricing**.

---

## Setting up the cloud card (no OpenAI account required)

"OpenAI-compatible" is only the *request format* — every option below speaks it, and none of them is
OpenAI unless you deliberately pick it. Pick the **Endpoint preset** and the base URL is filled for you:

1. *Settings ▸ AI · IDENTIFY (CLOUD)* ▸ tick **Use cloud/API tagging**.
2. **Endpoint** — choose a preset:
   - **Local server** — your own LM Studio / llama.cpp / vLLM (zero cost, fully private).
   - **Gemini (free)** — Google's free tier; the default.
   - **Groq (free)** / **OpenRouter** — free model tiers.
   - **Custom…** — type any OpenAI-compatible base URL (for anything not preset).
3. **Model** — a **vision-capable** model id at that endpoint (verify the current name on the provider).
4. **API key** — paste it. Kept in your OS credential store (Windows Credential Manager) where
   available, never logged. (A local server usually accepts any non-empty string; the free tiers hand
   you a key with no card.)
5. Press **Test** — it tags one sample image and shows the result or the exact error, so a wrong
   endpoint / non-vision model / dead key is caught *before* a bulk run.
6. Press **Run on New**. Watch the job tray; cancel anytime — it resumes where it stopped, and
   rate-limited or failed assets stay queued for the next run rather than being marked done.

---

## Teaching the identifier your words

Whatever model you pick, the **Traits** screen lets you mark traits **✦ teaching**. Those are fed into
the tagging prompt (local *and* cloud), scoped to the right modality — so future scans tag with *your*
vocabulary, not just the model's. See [`AI-PIPELINE.md`](AI-PIPELINE.md) §6.

---

## See also

- [`AI-MODELS-BY-GPU.md`](AI-MODELS-BY-GPU.md) — push local quality to your GPU's limit (per-VRAM tiers).
- [`AI-PIPELINE.md`](AI-PIPELINE.md) — how tagging/embedding actually work, per modality (the spec).
- [`USER-GUIDE.md`](USER-GUIDE.md) — the end-to-end "download → identify → search → import" walkthrough.
- [`SELF-HOSTED-PROVIDER.md`](SELF-HOSTED-PROVIDER.md) — the planned *hosted* Lodestash model service
  (a separate, future spec; the cloud card already works with any OpenAI-compatible host today).
