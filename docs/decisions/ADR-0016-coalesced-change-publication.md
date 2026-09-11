# ADR-0016 — Coalesced change publication and explicit external refresh

- **Status:** accepted — publication (S01) landed; the shell's pending-refresh half (S02) lands in
  its own commit
- **Date:** 2026-09-10
- **Amends:** [ADR-0008](ADR-0008-appraisal-is-a-background-job.md) and
  [ADR-0014](ADR-0014-the-backlog-lane-is-duty-cycled.md) — how the backlog lane's results reach the
  shell. The duty cycle and the single writer ([ADR-0002](ADR-0002-single-writer-thread.md)) are
  unchanged.
- **Plan:** [v1 architecture](../superpowers/plans/2026-09-07-v1-architecture.md), S01–S02

## Context

The writer emitted `ls:changed {kind:"catalog"}` **and** `{kind:"jobs"}` after every op — including
ops that failed and ops that changed nothing — and the idle lane emitted `catalog` after every slice
that stored anything. The shell answered each `catalog` event with a status fetch plus a bump of the
broad library version, so the visible page re-read. Fifty tag writes meant a hundred IPC events,
fifty status reads and fifty page reloads; a draining backlog meant a page reload per slice,
indefinitely, under whoever was browsing.

The same loop's idle probe was `rx.recv_timeout(ZERO).is_ok()`: it received a waiting message and
dropped it. An op arriving at that instant never ran, and its caller was told the writer had dropped
the job.

## Decision

1. **Mark, don't emit.** The writer and the job registry mark domains; nothing is emitted on the
   writer thread or from inside a transaction. `catalog` is marked only when SQLite's
   `total_changes` advanced during a turn that succeeded, and it carries the origin that caused it:
   `user` (a Tauri command), `agent` (the REST/MCP write bridge) or `background` (the idle lane,
   drains, watch rescans, and extract/manifest completions, whose outcome is reported in the tray).
   `jobs` is marked by the registry itself on start, progress and finish.
2. **One publisher, fixed window.** A publisher thread drains the marks at most once per 250 ms,
   the window opening at the first mark after a quiet period. Not a trailing debounce: the preview
   drain commits continuously, and a debounce waiting for quiet would never fire while it ran. Each
   event carries both domains' flags, every origin in the window, and a generation per domain that
   only climbs.
3. **Headless is event-free.** With no window there is no publisher; a mark is a flag and a
   counter, so marks do not grow.
4. **The probe holds what it finds** (`Inbox::waiting` in `writer.rs`), and the loop runs it next.
5. **The shell refreshes for its own user and only tells about everyone else** (S02). A `user`
   catalog change refreshes the visible page once. `agent` and `background` changes raise an
   accessible *Library updated · Refresh* indication instead, and a refresh clears only what was
   pending through the generation it read.

## Consequences

- A burst inside one window is one event. A longer burst is bounded by windows (about four a
  second), not collapsed to one — that is the honest promise.
- Background results (a pack's previews filling in, identifications landing) reach the visible page
  when the user refreshes or navigates. That is the trade: the grid stops moving under the pointer.
  Revisit only if a measured workflow needs selective live updates that still meet the frame budget.
- The event shape changed from `{kind}` to `{catalog, jobs, origins, catalog_gen, jobs_gen}`; both
  consumers moved in the same commit.
- A multi-transaction op that fails midway after committing some rows publishes nothing, because
  the op failed. Making such ops atomic is the plan's S04, not the publisher's job.
