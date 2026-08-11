# ADR-0005 — The in-pack pull cap guards the thumb route only

- **Status:** accepted
- **Date:** 2026-08-08
- **Supersedes:** the shared-archive-size cap in `protocol::in_pack_scratch`

## Context

`IN_PACK_THUMB_MAX_BYTES` (256 MB) lived in the shared `in_pack_scratch` helper, so
`lodestash://file` and `lodestash://mesh` refused ANY payload from a pack larger than 256 MB.
A real 322 MB "Toon Effects" pack made assets like `$rain drop Egg` fail to load (503
not_local). The cap's purpose was the grid stampede: `.unitypackage` is gzip with no random
access, so one page of in-pack cards from a multi-GB pack re-inflates gigabytes per pull.

## Decision

The 256 MB guard now lives only in the **thumb route** — the one caller that fans out across a
pack. A single explicit `file`/`mesh` pull is never refused by archive size: its cost is the
bounded decompress-skip to the payload's offset (~4 s first hit on a deep entry, cached in the
scratch afterwards), which is acceptable for one explicitly opened asset.

## Consequences

- In-pack Quick Look / mesh viewing works for big packs (FEATURES §5.4).
- The grid-stampede guard is unchanged where it matters.
- Remaining: per-entry "Extract selected" page-batching (a sorted `extract_at_offsets`
  skip-scan lifts the O(page) independent pulls).
