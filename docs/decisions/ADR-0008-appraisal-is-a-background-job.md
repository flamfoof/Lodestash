# ADR-0008 — Appraisal is a background job, not a screen you sit on

- **Status:** accepted
- **Date:** 2026-08-22
- **Supersedes:** the Identify screen as the only path out of the backlog (UI-SPEC §3's
  `✦ Identify all (N)` as a foreground batch), and §1's step-2 completion fork
- **Amended by:** [ADR-0014](ADR-0014-the-backlog-lane-is-duty-cycled.md) (the lane is duty-cycled)
  and [ADR-0016](ADR-0016-coalesced-change-publication.md) (how its results reach the shell)

## Context

Every asset enters as Unidentified, and DATA-MODEL §8 makes that invisible to AI search. The single
way out was `✦ Identify all (N)` on a dedicated screen, running as one flat batch over the whole
backlog. Three consequences, all bad for a first-run user with 7k+ assets:

1. The product's central capability was gated behind one long upfront wait.
2. Order was arbitrary with respect to intent — the assets appraised first were not the ones the
   user was looking at.
3. Onboarding had to ask a new user to choose between "Run identification now" and "Skip to
   Library" before they had any basis to choose.

## Decision

Appraisal runs as a **job in the tray**, started automatically after a scan, and ordered by what the
user is looking at:

1. Assets on the current page.
2. The rest of the current filter.
3. Everything else, newest first.

Browsing re-orders the queue. Any single asset can jump it with the per-card `✦`. A source or pack
can be appraised on its own from the Identify screen, which becomes a dashboard (progress,
throughput, where the backlog lives) rather than a place you wait. The job is pausable and resumes
where it stopped.

## Consequences

- The Identify screen keeps its title and copy but stops being a blocking workflow; `✦ Identify all`
  becomes "appraise this source/pack" plus a global pause/resume.
- UI-SPEC §1's amber completion callout becomes an arcane statement — *"Appraisal has started"* —
  and the two-way fork collapses to **Open library →** / **Watch progress** (ADR-0008 is why
  §1's old copy is wrong, not merely out of date).
- **Cost:** results now arrive in an order that depends on browsing history, which is harder to
  reason about when something looks wrong than a flat batch was. The queue's ordering is therefore
  shown on the Identify screen rather than left implicit.
- The auto-start respects the existing `auto_identify` setting; turning it off leaves the manual
  per-source path, which is the old behaviour minus the wall.
