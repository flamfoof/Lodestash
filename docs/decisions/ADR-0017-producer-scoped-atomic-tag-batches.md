# ADR-0017 — Producer-scoped atomic tag batches, and undo that checks before it replays

- **Status:** accepted
- **Date:** 2026-09-10
- **Amends:** [ADR-0006](ADR-0006-redo-forward-payloads.md) (redo by forward payloads) — tag
  operations now record complete before/after snapshots; the inverse-only tag payloads are retired
  and refuse to replay.
- **Plan:** [v1 architecture](../superpowers/plans/2026-09-07-v1-architecture.md), S04

## Context

Four findings, each checked against the code:

1. **Not atomic.** `BulkTag` recorded one history row, but mutated each asset in its own
   transaction first. A failure midway left some assets changed and no history for them.
2. **Undo was an inverse, not a snapshot.** Undoing an add removed the tag from *every* source;
   undoing a removal re-added it as the person's own tag, without the score or model it had.
3. **Attribution merged producers.** The local drain stored its zero-shot proposals under the VLM's
   model id, and a cloud pass stored its local fallback's tags under the cloud model's. Every AI row
   was `source='ai'`, so re-running any one producer replaced every producer's rows.
4. **No boundary validation.** A model's output reached the catalog through a lenient parser only:
   a number in the label slot or a non-finite confidence was never refused.

## Decision

- **One write contract**, `Catalog::apply_tag_batch`. Items are keyed by asset and source, with an
  explicit mode — append, replace this source's set, remove scoped associations, or (a person only)
  reject a term across every source — and an optional caption. The batch is validated whole before
  anything is written and holds at most 100 assets. One transaction applies the associations, the
  caption, one FTS re-index per asset, the queue flag when the tagger finished the asset, and — when
  asked — one versioned history row with the complete before/after association rows (source, model,
  confidence, created_at) of every asset and source it touched.
- **AI associations are `ai:<producer>`**, the model in `model_id`. Producer ids name the tagger kind
  and endpoint host — `ollama@localhost:11434`, `openai-compatible@<host>`, `siglip-zeroshot`,
  `clap-zeroshot`, `filename` — never a key or credential. A fallback's and the zero-shot pass's tags
  go under their own producers.
- **M26** marks every pre-existing `ai` row `ai:legacy`, keeping its recorded model id: which
  producer wrote those rows is unrecoverable, and is not invented.
- **One validator**, `assignment_problem` in core: allowed namespaces per author (a model's facet set;
  a person's every facet except `technical`), labels 1–128 characters and never a number or a
  serialized object in a model's label slot, confidence finite within [0,1] for a model and absent for
  a person, deduplication before the 8-per-namespace and 32-total caps. The model-output parsers drop
  what it refuses, so a drain never hands the boundary a batch it would reject.
- **Retention is partitioned by origin** (`op_log.origin`): the interactive tail (user, agent) and
  the background lane each keep their own 200 rows, the undo and redo buttons see only the
  interactive tail, and new work clears only its own partition's redo stack.
- **Undo and redo check before they replay.** A snapshot is restored only where the touched
  associations still look the way the operation left them; otherwise the result is a conflict and
  nothing changes. An independent producer's newer rows are never touched.
- **Legacy tag history refuses to replay**, with the reason. No batch ever sets confirmation, and
  lifecycle promotion rules are unchanged.

## Consequences

- Provenance is more visible: one label can sit on an asset under several sources. Search reads the
  distinct labels (the M25 view); showing provenance in the Inspector is S05's work.
- Every model result now records background history. Retention bounds it.
- A write that changes nothing writes and publishes nothing.
- The agent bridge's existing add/remove-tag operations still write as the person until S05 gives
  agent writes their own producer.
- Reverse the retention strategy only on a measured size or latency failure — never by giving up
  attribution.
