# ADR-0006 — Redo by forward payloads (op_log migration 17)

- **Status:** accepted
- **Date:** 2026-08-08
- **Supersedes:** the pre-image-only op log (undo deleted the row — redo was impossible)

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
