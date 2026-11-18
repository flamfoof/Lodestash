# ADR-0006 — Redo by forward payloads (op_log migration 17)

- **Status:** accepted
- **Date:** 2026-08-08
- **Supersedes:** the pre-image-only op log (undo deleted the row — redo was impossible)
- **Amended by:** [ADR-0017](ADR-0017-producer-scoped-atomic-tag-batches.md) — tag operations record
  complete before/after snapshots; the inverse-only tag payloads refuse to replay
- **Amended 2026-11-17:** redo re-arms the row instead of consuming it, and the redo stack is read
  last-undone-first — see "Amendment" below

## Context

The op log stored only revert data (`payload_json`): an undo reverted the change and DELETED
the row, so Shift-Z redo was impossible. Storing the forward state in a separate table would
have split one record across two.

## Decision

`op_log` gained `forward_json TEXT` + `undone_at INTEGER` (migration 17). Undo marks the row
undone instead of deleting; `redo_op` applies the forward payload (for the change-shaped ops
the payload IS the forward) and consumes the row. `record_op` clears the redo stack — new work
invalidates undone ops — and `recent_ops` hides undone rows.

## Consequences

- UI-SPEC §2's Shift-Z / Ctrl-Cmd-Y works; the Toolbar shows a Redo button.
- `record_set_tags_op` captures the forward tag set — the only op whose payload is a pre-image.
- The change was mutation-checked (undo → redo → consumed → new work clears the stack).

## Amendment (2026-11-17) — redo re-arms, it does not consume

"Consumes the row" was wrong. After a redo the op is the last thing that happened, so it is what
the next Ctrl/Cmd-Z must revert; with the row deleted, undo → redo → undo reached past it and
reverted whatever the user had done *before* — an unrelated edit. `redo_op` now clears `undone_at`
(for a collection create it also points the payload at the re-created row, whose id is new), so
the op returns to the undo stack. The redo stack is read ascending by id: undo always takes the
newest live op, so the op undone last has the lowest id, and reading it newest-first replayed two
undone ops in the wrong order. New work still clears the redo stack.

Related, `DATA-MODEL.md` §9: an asset taken out for good (Delete from disk, Remove from library)
is removed from every `set_trashed` row, so no undo or redo can flip it back.

Tradeoff: an op can now cycle undo ↔ redo indefinitely until new work or the 200-row cap retires
it, so a replay must stay safe on every pass — which the conflict check (ADR-0017) and the
state-guarded trash flip already make it.
