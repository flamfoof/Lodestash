# ADR-0013 — Model inference runs off the writer thread

- **Status:** accepted (2026-08-30)
- **Relates to:** [ADR-0002](ADR-0002-single-writer-thread.md) (one writer),
  [ADR-0008](ADR-0008-appraisal-is-a-background-job.md) (appraisal is a background job),
  [`ARCHITECTURE.md`](../ARCHITECTURE.md) §5, [`AI-PIPELINE.md`](../AI-PIPELINE.md) §2

## Context

Every catalog write funnels through one dedicated writer thread (ADR-0002). The idle appraisal
work — preview renders, embeddings, tagging — was written as `&mut Catalog` drains that read a
slice, compute, and write, all inline. So the thread that serialises writes was also doing the
compute, and while it did, no user write could land.

The drains were time-boxed against exactly this, and the boxes did not hold, because **a budget
checked once per batch is not a budget**. `drain_previews` took a 1-second slice from the writer's
idle tick and checked the deadline between batches of **16** renders; one render is ~1.1 s here, so
a "1 second" slice ran ~17 s. Measured on the real app, no test flags:

```
POST /v1/stage   round 1:    2 ms     (drain not started yet)
POST /v1/stage   rounds 2-6: 16,4xx ms
GET  /v1/assets/2689:         3-18 ms  (reads never queue — WAL)
```

Sixteen seconds to star an asset. The suite never saw it: `harness.mjs` sets
`LODESTASH_NO_PREVIEW_DRAIN=1` because the backfill is CPU-heavy and a suite that launches the app
dozens of times would grind the machine — so the one flag that makes e2e bearable is the flag that
hid this.

Two things followed from believing the budget: the writer looked cheap to give work to, and the
latency was invisible until someone measured a write on a normally-launched app.

## Decision

**Compute does not run on the writer thread.** The heavy drains split into three phases:

| phase | thread | cost |
|---|---|---|
| read a queue slice | writer | one indexed query |
| compute (render / encode / caption) | worker | seconds to minutes |
| persist the result | writer | N upserts |

`visual_embed_batch` → `compute_visual_embeddings` → `store_visual_embeddings`, and
`tag_batch` → `compute_tags` → `store_tags`. The worker posts finished work back as
`WriterMsg::EmbeddingsDone` / `TagsDone`, which is the same shape the writer already used for
`ExtractDone` / `ManifestDone`. One slice of each is in flight at a time.

The compute phases take no `&mut Catalog`. `compute_tags` needs no catalog at all once zero-shot
stopped reading stored model vectors; the tag worker still opens a **read** connection for the
user's taught vocabulary, which WAL admits alongside any number of others. **The single-writer rule
is about writes**, and every write still lands on the writer thread.

Where a drain does still run inline — the ✦ kick paths, where the user asked for it and is watching
a progress bar — **the budget is checked per unit, not per batch**.

## Consequences

- A user write with the backfill running: **16.4 s → 0.37 s**. The idle tick overshoots by at most
  one render (~1.4 s), which is the granularity of the work and honest.
- The e2e suite gains a check that launches its **own** app *without* the drain flag and asserts a
  stage write stays under 4 s. It costs ~20 s of runtime and it is the only thing in the suite that
  can see this class of bug — a latency property of a running writer, invisible to unit tests.
- Idle tagging no longer runs inside `appraise_pass`; the worker owns it, and the pass is told so
  explicitly (`only: Some(&[])` — an empty set means *no assets*, which is the one thing it cannot
  be confused with *everything*).
- More threads to reason about, and a slice can be computed against a catalog that changed
  underneath it. Acceptable: the queue is durable and every drain is idempotent — a stale slice
  writes a value the next pass corrects, which is already true of the kick paths.
