# ADR-0016 — Coalesced change publication and explicit external refresh

- **Status:** accepted — publication (S01) and the shell's pending refresh (S02,
  [UI-SPEC §0.5](../UI-SPEC.md#05-status-bar)) have both landed
- **Date:** 2026-09-10
- **Amends:** [ADR-0008](ADR-0008-appraisal-is-a-background-job.md) and
  [ADR-0014](ADR-0014-the-backlog-lane-is-duty-cycled.md) — how the backlog lane's results reach the
  shell. The duty cycle and the single writer ([ADR-0002](ADR-0002-single-writer-thread.md)) are
  unchanged.
- **Plan:** [v1 architecture](../superpowers/plans/2026-09-07-v1-architecture.md), S01–S02
- **Amended 2026-11-17** (v2 plan Task 2.3, [ADR-0021](ADR-0021-engine-owns-the-catalog-ui-is-a-web-client.md)):
  the bus moved into `lodestash-engine` unchanged in behaviour. Decision 3 now reads "no
  subscriber, no publisher" (`Engine::subscribe` starts it; the shell subscribes at setup, as it
  started the publisher before). Decision 1's "committed" also counts a commit through a second
  connection the writer opened (SQLite's `data_version`), and the read model rebuilds when the
  catalog generation moves, so the "Library updated · Refresh" path is no longer needed for the
  backend to be current (v2 spec §4.3).
- **Amended 2026-11-17, second** (integrity review, finding 7): Decision 1's "during a turn that
  succeeded" is dropped — a turn is marked whenever SQLite's counters moved, **also when the
  command then failed or panicked**. See "Amendment" below; the consequence it reverses is struck
  through in place.

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

None of those events ever reached the screen, it turned out. The shell had no Tauri capability
granting `core:event:allow-listen`, so every `listen` was rejected — silently, because `onChanged`
never caught the rejection — from the day the pump events landed (`d23ddfe`). The storm was real in
the code and absent in practice, and the job tray never heard one of its own events either.
Verifying S02 against the running app is what found it. The repair is a one-permission capability
file, `crates/lodestash-desktop/capabilities/default.json` (listen and unlisten, nothing else), and a
logged rejection in `onChanged`.

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
- ~~A multi-transaction op that fails midway after committing some rows publishes nothing, because
  the op failed. Making such ops atomic is the plan's S04, not the publisher's job.~~ Reversed —
  see the amendment.

## Amendment (2026-11-17) — a mark means "the catalog changed", not "the command succeeded"

When Decision 1 was written a mark only produced an event. Since v2 Task 2.3 the catalog
generation is also what tells the read model to rebuild (v2 spec §4.3), so not marking a turn that
committed rows and then failed left the read model serving the snapshot from before rows that are
now in the catalog — until some unrelated write moved the generation. A scan whose packs step
errors after the persist, a selection that stops half-way, a command that panics after its first
statement: each showed stale data behind an error.

The writer now marks every turn by SQLite's counters alone (`changes::mark_committed`): commands
(succeeded, failed or panicked), the extract request's own turn (it mints the pack's in-pack rows
before any worker runs, and was never marked), a stored embedding slice, and the idle pass as a
whole — one mark after it, where each branch used to mark by a returned count a panic never
reached. The caller still learns the outcome from the reply; the event only ever says that
something moved.

Tradeoff: `total_changes` also counts rows changed inside a transaction that then rolled back, so
a failed command can cost one rebuild that finds nothing new. That is the cheap direction to be
wrong in.
