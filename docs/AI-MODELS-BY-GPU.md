# Local AI Models by GPU Tier — Setup Guide

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

A practical guide to **which local AI models to run for auto-tagging and "Search with AI,"** chosen by
your GPU's **VRAM**. Everything here runs **offline** on your own PC (via ONNX Runtime or Ollama). If
you'd rather not run anything locally, jump to [Cloud / API](#cloud--api-no-gpu-needed).

> **TL;DR**
> - **Semantic search ("Search with AI") needs only embeddings, which run fine even on CPU** — don't
>   let a weak GPU stop you.
> - **More VRAM mostly buys you better *free-form caption* tagging** (the optional vision-LLM step).
>   The core discrete taggers and embedders are light.
> - Lodestash **auto-detects your VRAM and picks a profile**; you can override any choice.

---

## What the AI actually does (the four jobs)

| Job | Powers | Weight | Needed? |
|---|---|---|---|
| **Embeddings** (image / 3D-via-render / audio) | **Search with AI** (semantic) | Light | Core — runs everywhere, even CPU |
| **Discrete taggers** (image WD, audio YAMNet/PANNs) | **Search by Tags** (auto tags) | Medium | Core |
| **Vision-LLM captions** (optional) | Rich free-form descriptions | **Heavy — scales with VRAM** | Optional polish |
| **Speech (Whisper)** | Tagging spoken audio / voice clips | Light–Medium | Optional, only for voice |

3D models reuse the **image** models on 4–8 rendered views, so 3D's VRAM need = the image tier (plus
render time). Embeddings are always kept **local** even if you send tagging to the cloud, so search
stays offline and consistent.

---

## Pick your tier

VRAM figures are **approximate**; vision-LLMs (VLMs) are quantized GGUF run through **Ollama** (default
`q4_K_M`). "Embed" = image/3D embedding model; "Tagger" = discrete image tagger; everything also runs
the small audio stack (CLAP + YAMNet/PANNs) which fits in well under 1 GB.

| Tier (GPU example) | Embed (image/3D) | Image tagger | Optional VLM caption | Speech | Experience |
|---|---|---|---|---|---|
| **CPU only** (no/iGPU) | SigLIP 2 **Base** | WD **SwinV2** v3 | — (skip, or moondream2 slowly) | Whisper base/small | Works; run big batches overnight |
| **4 GB** (GTX 1650, 1050 Ti) | SigLIP 2 Base/**Large** | WD **ViT-Large** v3 | **moondream2** (~2–3 GB) | Whisper small | Smooth tags+search; light captions |
| **6 GB** (GTX 1660, RTX 2060) | SigLIP 2 **Large** / So400m | WD **EVA02-Large** v3 | moondream2 / LLaVA-7B (q4, tight) | Whisper medium | Good all-rounder |
| **8 GB** ⭐ (RTX 3060 Ti, 4060, 3070) | SigLIP 2 **So400m** | WD EVA02-Large v3 | **Qwen2.5-VL-7B** or LLaVA-7B (q4/q5) | Whisper large-v3 | **Recommended local sweet spot** |
| **12 GB** (RTX 3060-12G, 4070, 3080) | SigLIP 2 So400m (g optional) | WD EVA02-Large, big batches | Qwen2.5-VL-7B (q5/q8) or **Llama-3.2-Vision-11B** (q4) | large-v3 | Comfortable, fast batches |
| **16 GB** (RTX 4060 Ti-16G, 4080) | SigLIP 2 **g** (1B, top quality) | WD EVA02-Large | Llama-3.2-Vision-11B (q5/q8) | large-v3 | High quality + throughput |
| **24 GB+** (RTX 3090, 4090) | SigLIP 2 g (fp16) | WD EVA02-Large (fp16) | **Qwen2.5-VL-32B** (q4, ~20 GB) | large-v3 | Best local quality; can add OpenShape (native-3D) |

⭐ **8 GB is the recommended target** for a great fully-local experience.

---

## Recommended defaults (what most people should pick)

- **Best bang-for-buck, fully local:** an **8 GB** GPU running **SigLIP 2 So400m** (embeddings) +
  **WD-EVA02-Large v3** (tags) + **Qwen2.5-VL-7B** (optional captions). Excellent search and tagging,
  entirely offline and free.
- **No discrete GPU?** Use **SigLIP 2 Base** + **WD-SwinV2** on CPU for embeddings/tags (search still
  works great), and either skip captions or use the **Cloud** option for the occasional rich pass.
- **Want the best tag quality without buying hardware?** Keep embeddings local and send the *tagging*
  step to the **Cloud / API** (below).

---

## The models (reference)

**Embeddings (image / 3D-via-render) — joint text+image space, enables "search by description":**
| Model | Params | ~VRAM (fp16) | Runtime | License |
|---|---|---|---|---|
| SigLIP 2 **Base** | 86M | ~0.5 GB | ONNX / candle | Apache-2.0 |
| SigLIP 2 **Large** | 303M | ~1.2 GB | ONNX / candle | Apache-2.0 |
| SigLIP 2 **So400m** | 400M | ~1.6 GB | ONNX / candle | Apache-2.0 |
| SigLIP 2 **g** | 1B | ~4 GB | ONNX / candle | Apache-2.0 |

**Image discrete taggers (scored tags, great for game/art assets):**
| Model | ~VRAM | Runtime |
|---|---|---|
| WD **SwinV2** v3 | ~0.5 GB | ONNX |
| WD **ViT-Large** v3 | ~1.5 GB | ONNX |
| WD **EVA02-Large** v3 | ~2.5 GB | ONNX |

**Audio (all light — fit together in <1 GB):** LAION-**CLAP** (embeddings + zero-shot description,
joint text↔audio), **YAMNet**/**PANNs** (event tags: explosion/footstep/wind…), **Whisper** for speech.

**Optional vision-LLM captions (via Ollama, quantized):**
| Model | ~VRAM (q4) | Notes |
|---|---|---|
| **moondream2** (1.9B) | ~2–3 GB | Tiny, low-VRAM captions |
| **LLaVA-7B** / **Qwen2.5-VL-7B** | ~5–6 GB | Strong general captions; Qwen good for text/icons in images |
| **Llama-3.2-Vision-11B** | ~7–8 GB | Best general 11B vision |
| **Qwen2.5-VL-32B** | ~20 GB | Top local free-form quality |

**Native-3D similarity (optional, 24 GB+):** **OpenShape** gives CLIP-aligned 3D embeddings for
shape-similarity that survives texture/render differences (served via the Python sidecar).

---

## Cloud / API (no GPU needed)

Prefer not to run models locally, or want top-quality free-form tags? Switch the **tagging** step to a
cloud vision model — **Claude** is the built-in default (others can be plugged in).

- **Models:** use a cheaper tier (Haiku) for bulk, a stronger tier (Sonnet) for quality. *Verify
  current model IDs/pricing in-app against the official reference before running large jobs.*
- **Cost control:** Lodestash uses the **Batches API (~50% off)** + **prompt caching** of the shared
  instruction prompt for big libraries.
- **Privacy:** cloud is **opt-in and per-modality**. **Embeddings still run locally** (they're tiny and
  CPU-friendly), so your semantic search stays offline and your asset *contents* aren't sent unless you
  enable cloud tagging.
- **Audio:** there's no direct cloud-audio path — speech/sound is described locally (Whisper/CLAP) and
  the cloud only enriches the resulting **text** tags.

---

## Setup — installing the models

> Lodestash ships a **"Download recommended models for my GPU"** button that runs all of this for you
> (it auto-detects VRAM and picks the tier). The commands below are for transparency, advanced/headless
> setups, and anyone who wants to pin specific variants. All repos/tags below are **verified real**.

**Prerequisites**
```bash
pip install -U "huggingface_hub[cli]"      # the `huggingface-cli download` tool
# Ollama (only needed for optional vision-LLM captions): install from https://ollama.com/download

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
# ONNX Runtime is bundled in the app (Rust `ort`); GPU acceleration (CUDA / DirectML) is auto-selected.

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
```

**Embedding model — image & 3D (pick one for your tier)**
```bash
# 4 GB / CPU

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download onnx-community/siglip2-base-patch16-256-ONNX  --local-dir models/siglip2-base
# 6 GB

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download onnx-community/siglip2-large-patch16-384-ONNX --local-dir models/siglip2-large
# 8 GB+  (recommended)

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download onnx-community/siglip2-so400m-patch16-512-ONNX --local-dir models/siglip2-so400m
# 16 GB+ "g" (1B): no prebuilt ONNX — export from source if you want top quality:

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
#   optimum-cli export onnx -m google/siglip2-giant-opt-patch16-384 models/siglip2-giant

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
```

**Image tagger — discrete tags (pick one)**
```bash
# light (4 GB / CPU)

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download SmilingWolf/wd-swinv2-tagger-v3    --local-dir models/wd-swinv2
# balanced

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download SmilingWolf/wd-vit-large-tagger-v3 --local-dir models/wd-vit-large
# best quality (8 GB+)

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download SmilingWolf/wd-eva02-large-tagger-v3 --local-dir models/wd-eva02-large
# (each repo ships model.onnx + selected_tags.csv label list)

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
```

**Audio**
```bash
# CLAP — audio embeddings + zero-shot description (ONNX build for runtime use)

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download Xenova/clap-htsat-unfused --local-dir models/clap
#   or export the original yourself: optimum-cli export onnx -m laion/clap-htsat-unfused models/clap

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
# YAMNet / PANNs (sound-event tags) ship pre-converted to ONNX with Lodestash — no manual download.

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

# Whisper (only if you tag speech/voice clips) — ggml weights for whisper.cpp:

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)
huggingface-cli download ggerganov/whisper.cpp ggml-base.bin     --local-dir models/whisper   # small/fast
huggingface-cli download ggerganov/whisper.cpp ggml-large-v3.bin --local-dir models/whisper   # 8 GB+
```

**Optional vision-LLM captions (Ollama)**
```bash
ollama pull moondream         # 4 GB tier   (1.9B, ~2 GB)
ollama pull llava:7b          # 6 GB tier
ollama pull qwen2.5vl:7b      # 8 GB tier   (recommended)
ollama pull llama3.2-vision   # 12–16 GB    (11B, ~8 GB)
ollama pull qwen2.5vl:32b     # 24 GB+      (~20 GB, best local free-form)
```

### Copy-paste bundles

**Minimal / CPU-only** (great search + tags, no GPU, no captions)
```bash
huggingface-cli download onnx-community/siglip2-base-patch16-256-ONNX --local-dir models/siglip2-base
huggingface-cli download SmilingWolf/wd-swinv2-tagger-v3 --local-dir models/wd-swinv2
huggingface-cli download Xenova/clap-htsat-unfused --local-dir models/clap
```

**8 GB recommended** (the local sweet spot)
```bash
huggingface-cli download onnx-community/siglip2-so400m-patch16-512-ONNX --local-dir models/siglip2-so400m
huggingface-cli download SmilingWolf/wd-eva02-large-tagger-v3 --local-dir models/wd-eva02-large
huggingface-cli download Xenova/clap-htsat-unfused --local-dir models/clap
huggingface-cli download ggerganov/whisper.cpp ggml-large-v3.bin --local-dir models/whisper
ollama pull qwen2.5vl:7b
```

---

## Notes & tips

- **Quantization:** Ollama defaults to `q4_K_M`. Higher quant (`q5`/`q8`) = better quality, more VRAM.
- **Batching:** larger VRAM → larger batches → faster whole-library processing. Decode/resize happens on
  CPU worker threads to keep the GPU fed.
- **Throughput ballpark (single consumer GPU):** 10k images → minutes for embeddings, up to ~1 hr if you
  also run the heavy tagger + VLM captions. Audio is fast. 3D is gated by render time (cached).
- **One-time cost:** every result is cached by `content hash + model + version` — assets are never
  re-processed unless they change or you upgrade the model.
- **DirectML/CUDA/CPU:** ONNX Runtime picks the best execution provider for your hardware automatically
  (CUDA for NVIDIA, DirectML on Windows for AMD/Intel, CPU fallback).
