# ADR-0003 — Local-first tagging: zero-shot embedders, cloud captioning opt-in

- **Status:** accepted
- **Date:** 2026-08-07 (tagger wiring), 2026-08-08 (cloud endpoint)
- **Supersedes:** the "tagger resolver (Ollama/local ONNX WD)" plan — resolved locally instead

## Context

The appraise pass had no tagger: `drain_tags` / `drain_audio_tags` had zero call sites, so rows
stalled at `indexed` and the lifecycle could never reach `identified` (DATA-MODEL §8). A cloud
captioning dependency would have broken the local-first invariant (CLAUDE.md).

## Decision

The already-resolved local embedders do the tagging, offline, zero new models:

- SigLIP → `ImageZeroShot` (taught traits, AI-PIPELINE §6, become candidate labels)
- CLAP → `AudioZeroShot`
- `FilenameTagger` is the captioning stub when no model is installed
- Ollama text/vision or an opt-in OpenAI-compatible endpoint (Settings → AI TAGGING) upgrade
  the stub; keys live in config.json and are never logged

Both tag drains gained a `max_assets` budget guard (64 visual / 48 audio per pass) so a
writer-thread kick never blocks writes (ADR-0002).

## Consequences

- Identify works end-to-end offline; cloud captioning is strictly opt-in per the invariant.
- Captions land in `description`, making them FTS-searchable.
- The "one piece still stubbed" is gone; the remaining gap is teaching *back-propagation*
  (tagger output → `teach` trait vocabulary), tracked on the board.
