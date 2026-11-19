# ADR-0025 — Identity follows content

- **Status:** accepted (2026-11-18), implemented on branch `rewrite/v2`
- **Amends:** [`DATA-MODEL.md`](../DATA-MODEL.md) §6 (move detection; the rule is §6.1)
- **Relates to:** [ADR-0002](ADR-0002-single-writer-thread.md) (the scan's catalog half runs on
  the writer), [ADR-0024](ADR-0024-the-boundary-fails-closed.md) (what cannot be verified is not
  acted on)
- **Plan:** v2 rewrite plan, Task R.3

## Context

An asset's identity is its `asset_id`: tags, description, status, favourite, collections, import
and export records, embeddings and undo history are all keyed by it. Since 2026-08-08 a scan kept
that identity through a move — a row whose path vanished was repointed at a new path holding the
same `content_hash` — but only when the new path was one **no row held**.

A review (2026-11-17) found two ordinary workflows that rule strands hand tags in:

- **A rename onto a held path.** `rock.png` is renamed `rock_old.png` and `rock_v2.png` is renamed
  `rock.png`, in one scan. `rock.png` is held, so it is nobody's destination: its row keeps its
  tags and takes the other file's bytes, the `rock_v2.png` row goes missing with its tags, and
  `rock_old.png` is a new, untagged asset. A swap and a rotation fail the same way.
- **Copy, then delete, seen as two scans** — a cross-volume move under the watcher. The first
  scan inserts the copy as a new row (the original is still there). The second sees the original
  vanish, and its bytes are at a path a row already holds. The tagged row goes missing; the
  visible one is untagged.

It also found the tie-break guessing: among byte-identical files, `pair_moves` took the first
same-named path in sort order, and paired leftovers of equal count in sort order.

## Decision

**Identity follows content. A path is evidence of identity only when content says nothing, and
where the evidence does not single out one answer, nothing is matched.** The full rule is
`DATA-MODEL.md` §6.1; these are the decisions in it.

### One scan is one matching problem

Rows whose path still holds their recorded hash stay. The rest of the movable rows are
*displaced* — vanished, or overwritten in place. Crawl paths whose file was read and that are
free, or held by a displaced row, are *open*. Displaced rows and open paths are matched one to
one by hash, and all matches of a scan are applied in one transaction — so a swap, a rotation
and a rename onto a held path are each just a permutation.

### A path that still exists is given up only to another identity

An overwritten row leaves its path only when another row's bytes arrive there. With no arrival
it stays, as the in-place edit it has always been, even when its old bytes are found at an open
path. That configuration is ambiguous on its face — a backup copy followed by an edit, or a
rename followed by an unrelated file of the old name — and the two readings want opposite
outcomes. Staying is what the catalog did before, so nothing a user relied on changes there.

### A row with no file left takes over an untouched duplicate

A vanished (or superseded) row that carries user data replaces a settled row with the same hash
that carries none: one row survives, at the duplicate's path, with everything the vanished row
had. "No user data" is enumerated in §6.1 and is deliberately wide — anything a person or an
approved agent did blocks it, including actions that left only history behind. This is a
standing rule, evaluated on every scan, not a window after the copy: there is no interval to
tune, and a row that could not be placed while two candidates existed is placed when one is left.

### Never guess

Within one hash, a row and a place pair when they are the only ones, or when they share a
trailing run of path components nobody else of that hash shares (a folder moved rigidly keeps
its duplicates apart), or by elimination when exactly one of each is left. Otherwise the rows go
missing and the places become new assets. Sort order decides nothing.

### A superseded row is kept, where no scan can find it by path

When another row's bytes land on an overwritten row's path and that row's own bytes are nowhere,
it cannot stay (one row per path) and has nowhere to go. It is kept, `missing`, at
`.lodestash-superseded/<asset id>/<file name>`. The crawler never walks a dot-directory, so no
file can appear "at" that path and revive the row with unrelated bytes; its own bytes turning up
anywhere bring it back through the ordinary matching.

## Consequences

- Renaming files over each other, swapping them, rotating them, and moving them across volumes
  while the app watches keep each file's tags, favourite, collections, import records and undo
  history with its bytes.
- **A file moved over another file takes the path's identity with it.** `mv -f rock_v2.png
  rock.png` leaves `rock.png` carrying the `rock_v2` row — its tags, not the old `rock.png`'s.
  The old row is superseded: hidden, intact, and back if its bytes return. It used to be the
  other way round (the path kept its row and the moved row went missing).
- **Byte-identical files renamed together with nothing to tell them apart are no longer paired
  in sort order.** `tile_1.png`, `tile_2.png` (identical) renamed `floor_a.png`, `floor_b.png` in
  one scan: both rows go missing and two new assets appear. The pairing was a guess that
  happened to be harmless when the rows carried the same data and wrong when they did not.
- Deleting the tagged one of two identical files moves its identity to the untouched other. The
  duplicate's row id disappears; a client holding it gets `not_found` and finds the asset under
  the surviving id.
- A scan now reads every row of the source even when no record carries a hash — the second half
  of a copy-then-delete is exactly such a scan.
- `record_crawl` treats a record whose hash differs from the row's as changed even when size and
  mtime agree, and fills in a missing hash without re-queuing the asset.

## Alternatives rejected

- **Move an overwritten row to wherever its old bytes are, always** (the plain reading of
  "match displaced rows to open paths"). Correct for a rename onto a held path; wrong for the far
  more common backup-then-edit, where it would hand the tags to the backup and leave the working
  file a new, untagged asset on every save.
- **Let the row at the path keep it when a file is moved over it** (the mover goes missing).
  Leaves bytes that are on disk, and were hand-tagged, showing another asset's tags — the defect
  this ADR exists to remove.
- **Park a superseded row at the path the arriving row left.** No invented path, but a later
  file of that name would revive it by path with bytes it never had.
- **Merge the duplicate's data into the survivor** instead of requiring it to have none. Every
  merge of two annotated rows is a guess about which annotation wins.
- **A time window for "fresh" duplicates.** A cross-volume move of a large folder takes as long
  as it takes; any window is wrong for some move, and the no-user-data test already carries the
  safety.
- **Read every file on every scan** to close the gate's blind spot (same size, same mtime,
  names exchanged). A full re-hash of the library per scan for a coincidence.
