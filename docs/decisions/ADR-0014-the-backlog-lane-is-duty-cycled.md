# ADR-0014 — The backlog lane is duty-cycled, not continuous

- **Status:** accepted
- **Date:** 2026-09-04
- **Amends:** [ADR-0008](ADR-0008-appraisal-is-a-background-job.md) (appraisal is a background job)
- **Amended by:** [ADR-0016](ADR-0016-coalesced-change-publication.md) (how the lane's results reach
  the shell) and [ADR-0026](ADR-0026-one-appraisal-plan-and-a-stage-owns-its-media.md) (what the
  duty cycle gates: the model-free backfills, and four model stages each behind its own in-flight
  flag)

## Context

ADR-0008 moved appraisal off a blocking screen and onto the writer thread's idle time. "Idle time"
was implemented as: wait one second on the channel, and if nothing arrived, do one second of
backlog work. That is a 100% duty cycle wearing the word *idle*.

Measured on the reference library (7,869 assets still queued), debug build, window open, nothing
asked of the app:

| | `lodestash-desktop.exe` CPU |
|---|---|
| Backlog lane every tick | **0.57 cores, indefinitely** |
| Backlog lane one tick in four | **0.26 cores** |
| Backlog lane disabled (`LODESTASH_NO_PREVIEW_DRAIN=1`) | ~0.00 cores |

This is an asset viewer. A user who opens it to look at a texture should not find half a core gone
for as long as the window is open, and there is no deadline on the backlog that justifies it.

An ONNX Runtime thread cap was tried in the same session and **measured to contribute nothing**
(0.58 cores with the cap and no duty cycle — indistinguishable from the 0.57 baseline), so it was
not kept. The whole win is the duty cycle.

## Decision

The idle lane takes one tick in `IDLE_DUTY_PERIOD` (4), each tick a second. Skipped ticks still
wait on the writer channel, so a user write is picked up the moment it arrives — the throttle costs
no operation latency, only backlog throughput.

One counter gates all three lanes at once: the embed and tag workers run on their own threads but
gate on `embed_inflight` / `tag_inflight`, and only the writer thread clears those flags.

## Consequences

- The backlog converges roughly 4× slower in wall-clock terms. It is durable and resumes across
  restarts, so this costs patience, not correctness.
- A user who *wants* the backlog drained now has a reason to use the explicit path — `✦ Identify`
  on a card, a source, or a pack — which is unthrottled, because there the user asked and is
  watching a progress bar.
- `IDLE_DUTY_PERIOD` is a constant, not a setting. If the ratio turns out to be wrong for real
  machines it becomes one; guessing at a Settings row before that is a knob nobody asked for.
- **Not addressed:** the measurements above are debug builds. A release build changes the absolute
  numbers (likely by a lot) but not the shape — a 100% duty cycle is a 100% duty cycle either way.
