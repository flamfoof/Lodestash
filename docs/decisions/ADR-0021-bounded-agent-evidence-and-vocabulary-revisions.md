# ADR-0021 — Bounded agent evidence and global vocabulary revisions

- **Status:** accepted
- **Date:** 2026-09-15
- **Amends:** [ENGINE-API](../ENGINE-API.md) §8.2 and §8.5; complements
  [ADR-0020](ADR-0020-egress-is-a-parsed-endpoint-and-a-per-modality-consent.md).

## Decision

Agent model views initially use a restricted selected-payload loader and the existing CPU renderer.
FBX/OBJ cannot load sidecars; glTF/GLB inputs must be embedded. Report material/texture references
and rendering limitations separately. Do not claim that a referenced texture was inspected or used.
Primary in-pack materialization retains the verified surgical cache. Views are rendered on demand.
This makes source reads reviewable; textured previews and external glTF buffers require a later
explicit bounded dependency resolver rather than inheriting the interactive loader's broad reads.

Vocabulary revision `v2-…` hashes the full canonical vocabulary and policy independently of read
filters, pagination and tag usage. Validate it in the same transaction as writes, reviews and
proposals. A filtered read must yield a usable write token. The tradeoff is conservative conflict
rejection when an unrelated modality or taxonomy changes; clients refresh and resubmit.

Annotation revisions also include the catalogued primary-file identity. Pending results cannot
commit after a rescan changes that identity or after the asset is trashed. Older uncommitted tokens
conflict and must be refreshed; committed receipts still replay before revision checks.

## Verification

`lodestash-ingest`'s `load_gltf_reads_a_minimal_model` proves embedded input works and an existing
external buffer is refused. API evidence tests compare representation bytes with authenticated URLs.
Core guarded-write tests cover scoped traits, paused teaching, stable usage counts and stale tokens.
The MCP disposable workflow verifies proposal, review, write, retry and independent readback.
