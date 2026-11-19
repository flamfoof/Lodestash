# ADR-0026 — One appraisal plan; a stage owns its media

- **Status:** accepted (2026-11-19), implemented on branch `rewrite/v2`
- **Amends:** [ADR-0013](ADR-0013-inference-off-the-writer.md) (how the idle pass is told not to
  run inference inline, and which stages leave the writer) and
  [ADR-0014](ADR-0014-the-backlog-lane-is-duty-cycled.md) (what the duty cycle gates)
- **Relates to:** [ADR-0008](ADR-0008-appraisal-is-a-background-job.md),
  [ADR-0017](ADR-0017-producer-scoped-atomic-tag-batches.md),
  [`AI-PIPELINE.md`](../AI-PIPELINE.md) §2.2, v2 spec §4.6
- **Plan:** v2 rewrite plan, Task 2.5 (review findings B3, B4, B5)

## Context

One pass over the appraisal queues was steered by one parameter, `only: Option<&[i64]>`, which had
come to mean three things: the *scope* of a kick; inside the pass, *"skip the library-wide
backfills"*; and — passed as an empty set by the idle tick — *"do not tag inline"*
(ADR-0013's mechanism). Three defects followed, each verified against the code before it was fixed:

- **B4.** The idle tick's empty set also switched off metrics, previews and dependency scans, and
  scoped the audio queues to nothing. With auto-identify on, a freshly scanned asset was never
  measured in the background; with it off, the idle lane ran previews only. Technical facets
  appeared only after someone pressed Identify.
- **B5.** The generic tag stage ran first, over every queued asset. `tags_dirty` is one flag: a
  tagger that reads file names answered for an audio clip, and a vision model answered "nothing"
  for it — either way the clip was finished, and the audio stage, which ran second on the same
  flag, found nothing queued. CLAP never tagged it.
- **B3.** The preview queue listed models whose bytes are still inside a `.unitypackage`. The
  preview stage pulled each one's payload out of the archive — gzip has no random access, so that
  is an inflate up to the payload — and then skipped it, because an in-pack model has no stable
  path to key a preview on. It never left the queue, so this repeated on every idle pass.

## Decision

1. **A pass is a plan.** `AppraisePlan { scope, stages, budget, origin }`. Scope is
   `All | Assets(ids) | Source(id) | Package(id)`; an explicit empty set is no assets, and such a
   pass reads and writes nothing. Callers build a plan; they do not order stages.
2. **One order, in one place:** metrics → dependencies → previews → audio embed → audio tags →
   image/3D embed → tags → promote (`Stage::ORDER`).
3. **A scope confines every stage,** the status reconcile included. A scoped pass runs the
   backfills *for its assets* — it no longer skips them — and reports as promoted only what it
   promoted. The one thing it leaves for a library-wide pass is the dependency **graph**
   reconcile (prefab evidence, the naming heuristic, late binding), which reads every source: a
   pass that harvests without reconciling marks it due, and the next library-wide pass whose
   dependency queue has run dry reconciles.
4. **A stage owns its media.** Audio is tagged by the audio stage and by nothing else: the generic
   tag queue — and the count behind the cloud cost estimate — never holds an audio row. A clip no
   audio model has tagged **stays queued**. This relaxes the old rule that every tagger clears
   every row it is handed: that rule is how work no model did came to be recorded as done.
5. **The preview queue lists only models a preview can be made for.** One still inside its package
   joins when its pack is extracted. The stage itself resolves a job without ever pulling.
6. **The idle lane always measures.** Metrics, dependencies, previews and the status lifecycle are
   not identification and need no model; they run in the idle slice whether or not
   `auto_identify` is on. Identification runs there when it is on and a model is installed, and
   then **every** model stage — audio embed and audio tags as well as the two ADR-0013 moved —
   runs off the writer, one slice of each in flight. The plan hands those stages to the lane
   (`Offload`); there is no empty-scope signal any more.
7. **Budgets are per unit, and say what they bound.** A kick gives each stage 20 s; the idle slice
   gives the whole pass one second, taken by the stages in order. The generic tag stage is bounded
   by a count, not a clock, because its unit is a whole model call.

## Consequences

- A freshly scanned library fills in its technical facets, previews and dependency graph on its
  own, at the idle lane's duty cycle (one second in four). The backfills take that second in stage
  order, so on a large fresh library previews wait behind metrics — which, for models, also draws
  the preview.
- An audio asset without a CLAP model installed is no longer tagged from its file name, and is no
  longer counted as tagged. Its name is still searchable as text. It is tagged when the model
  arrives, without a re-flag.
- `identify_asset(ids)` now measures and previews those ids as well as identifying them.
- **Cost:** the dependency graph reconcile after a harvest is one untimed step on the writer —
  a walk of every source tree for prefabs. It used to run only inside kicks; it now also runs in
  the idle lane, once per harvest that drains. Moving the dependency stage to a worker lane (v2
  spec §4.1) removes it; until then it is the one idle step that can exceed its second.
- **Cost:** the reconcile-is-due mark is in memory. A crash between a harvest and its reconcile,
  or queued rows vanishing before they are harvested, leaves the graph un-reconciled until the
  next dependency change — which reconciles everything, because the reconcile is global.
- Not decided here: what a stage records when its source is **unreachable** (review B6) and
  results computed against bytes that changed meanwhile (B7). Both need a schema addition and are
  v2 phase 8.
