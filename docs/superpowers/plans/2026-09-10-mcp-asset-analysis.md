# MCP Asset Analysis Implementation Plan

> **For agentic workers:** Use `superpowers:executing-plans` to implement task by task. This is
> a handoff for a later model; no implementation is authorized by completion of this planning turn.

**Goal:** retrieve one or many library assets, search them, deliver usable analysis evidence and
file references, run identification, review assignments against the vocabulary, persist approved
tags/description and vocabulary extensions, and verify the result through MCP.

**Architecture:** retain the stdio MCP child, shared API services, existing archive/preview helpers
and the app's single writer. The LLM host analyzes evidence; Lodestash validates and persists its
output. App-closed writes remain unavailable.

**Tech Stack:** existing Rust/rmcp/axum/rusqlite/Tokio/ureq, Tauri/Svelte and Node test tooling.
No new runtime dependency planned.

**Spec:** [MCP asset-analysis design](../../MCP-ASSET-ANALYSIS.md), with exact wire contracts in
[ENGINE-API §8](../../ENGINE-API.md#8-proposed-asset-analysis-contract).

Prepared 2026-09-10. All unchecked items below are future work, not verified behavior.

**User choices confirmed:** Codex; 3D preview images and metadata; tags and a short description.
The required pilot includes existing-tag reporting, AI identification, acceptance/rejection and an
explicit route for new traits/categories. Library and evidence consent are chosen before the live run.

## Global constraints and prerequisites

- [CLAUDE.md](../../../CLAUDE.md) and the owning docs govern implementation. Commit/push only when asked.
- Preserve local-first, source-file safety, human-only confirmation, the single writer and MCP↔REST parity.
- Apply exact limits/validation/permissions from ENGINE-API §8; do not copy new competing values here.
- Preserve existing v1 containers and required arguments. New tools/routes land together.
- Keep source IDs distinct from package IDs. Resolve files by catalog ID, never caller-supplied disk paths.
- Respect the pre-existing dirty worktree. Do not modify unrelated work, generated assets or live tags as a test.
- The v1 architecture plan's S03/S04/S05 own FTS, atomic tag history/provenance and shared adapters;
  S10/S11/S12 own egress, startup/forwarding and selection. Reuse those slices instead of building
  parallel implementations. This handoff narrows their order for an interactive pilot; it does not
  authorize the rest of the architecture backlog.
- Before schema work, inspect the current migration head and allocate the next free number. Never
  blindly reuse proposed M25/M26 numbers if another model has already shipped them.

## Task 1 — Make the existing MCP entry point dependable

**Files:** modify `crates/lodestash-desktop/src/lib.rs`,
`crates/lodestash-desktop/src/agent_bridge.rs`, `crates/lodestash-api/src/lib.rs`,
`crates/lodestash-core/src/catalog/mod.rs` and tests in the owning modules.
Create `mcp-analysis-smoke.mjs` inside the existing `scripts/` directory for the spawned protocol test.

**Consumes:** `lodestash-desktop --mcp`, `serve_stdio`, `build_engine_state`, `ServerState::new`,
`forwarding_write_fn`. **Produces:** trustworthy startup/read behavior and capability reporting;
not yet new authoring operations. Governed by v1 S11, ADR-0011 and ENGINE-API §8.4.

- [x] Add a spawned stdio test against an isolated temporary catalog: initialize → initialized
  notification → tools/list → get_server_info → stdin EOF. Parse every stdout line as JSON-RPC;
  deliberately enable logging so stdout contamination fails the test. Check timely exit and no window.
- [x] Add checks that headless startup never migrates/writes a missing, current or older-schema
  catalog. Initialize the fixture through the app/core setup path before opening the reader.
- [x] Make reader construction truly read-only, initialize managed ORT before headless model use,
  explicitly direct tracing to stderr, and keep expensive work off the current-thread async loop.
  Probe capabilities without downloading/loading models as an unconditional startup dependency.
- [x] Extend server information with catalog identity, supported/enabled analysis operations and
  writer availability. Report unavailable writer separately from failed MCP connection.
- [x] Run `cargo test -p lodestash-mcp`, `cargo test -p lodestash-api`, and the affected desktop/core
  startup tests. Run the spawned harness against the freshly built executable; record the negotiated
  protocol version and names actually returned, without logging handshake secrets.

**Acceptance:** a local MCP client can connect without Vite, the GUI or the computer-use helper;
existing catalog reads work and the absence of a writer is explicit. Client registration is Task 6.

*Done with v1 plan S11 (2026-09-12).* The spawned test is `crates/lodestash-desktop/tests/mcp_stdio.rs`
rather than a script — a Rust integration test builds the executable it spawns and runs in the gate,
where a `.mjs` would need a separately built binary and a manual step. It isolates the catalog with
`LODESTASH_CATALOG`, runs with `RUST_LOG=trace`, and records the negotiated protocol version. Server
info reports `catalog_id`, `writer`, `bytes_released` and `enabled_tools`; "analysis operations"
are the enabled tools until Tasks 2–5 add their own. Model availability is not probed at startup —
`semantic_search` still resolves its embedders lazily.

## Task 2 — Library discovery, correct search, single/list/random selection

**Files:** `crates/lodestash-core/src/catalog/mod.rs`,
`crates/lodestash-core/src/catalog/tests_catalog.rs`, `crates/lodestash-api/src/lib.rs`,
`crates/lodestash-api/src/routes.rs`, `crates/lodestash-mcp/src/lib.rs`.

**Consumes:** `source_infos`, `asset_summaries`, search/ranking helpers, Task 1 catalog identity.
**Produces:** `list_libraries`, `list_assets`, `get_assets`, scoped existing search and richer
`get_asset`/summary output, exactly as ENGINE-API §8.1. Add shared DTOs in the API/core owning
modules rather than independently validating MCP and REST.

- [x] Seed a fixture with two sources, more than one page, ten models behind hundreds of images,
  duplicate names, missing files, trashed/excluded rows and tags from multiple producers.
- [x] Add failing tests for source/media/tag filtering before LIMIT, ordered paging, random
  eligibility/uniqueness, exact-ID read order, partial unavailable results and provenance readback.
  Test both keyword and semantic candidate filtering; use a deterministic embedder fixture.
- [x] Reuse the core source/summary queries; implement bounded selection with the spec's cursor
  contract. Do not require file extraction to return a catalog row.
- [x] Extend both search adapters, route permission mapping and `AGENT_OPERATIONS` together. Add
  new tools/routes and shared detail serialization; preserve old response containers.
- [x] Test changed filters, malformed/cross-catalog cursors, concurrent insert/delete, fewer than
  ten matches, empty/duplicate explicit IDs, invalid enums/limits and disabled tools on both planes.
- [x] Run the affected core/API/MCP tests and `cargo test -p lodestash-mcp tool_set_matches_the_rest_surface`.

**Acceptance:** ten qualifying models are selectable even when none appear in the old limited
search page. Random tests assert membership/uniqueness, not probabilistic frequency or exact order.

*Done with v1 plan S12 (2026-09-12)* — tests in `catalog/tests_browse.rs`, `lodestash-api`
`discovery.rs`, and a plane test on each side. Not covered by a fixture: duplicate names, a file
missing from disk (`file_availability: missing` is exercised only by its policy-first `denied`
branch) and tags from several producers in one read — provenance readback is S05's `get_asset`
test. Real corpus: a headless server walked all 8,237 assets of the six libraries in 45 pages of
200, counts equal to `asset_count`, no duplicates, strictly ascending, median page 51 ms (debug).

## Task 3 — Evidence, access URLs and surgical package analysis

**Files:** `crates/lodestash-api/src/lib.rs`, `crates/lodestash-api/src/routes.rs`,
`crates/lodestash-mcp/src/lib.rs`, `crates/lodestash-app/src/thumbs.rs`,
`crates/lodestash-ingest/src/unitypackage.rs`, desktop renderer setup in
`crates/lodestash-desktop/src/lib.rs`, and the existing app configuration modules.
Update the existing MCP Settings/approval components identified from UI-SPEC, without redesigning the shell.

**Consumes:** Task 2 detail/revisions, `preview_subject`, `ensure_in_pack_scratch`, `read_payload_at`
and existing rendered previews. **Produces:** `get_asset_evidence`, its REST counterpart, supported
preview-view parameters, resource reads and file resolution governed by ENGINE-API §8.2.

- [ ] First implement/verify S10's configured producer/destination and modality checks at byte
  release. Test metadata-only unknown destinations and denied bytes through every delivery path;
  a caller's `local=true`/producer argument must not grant access. Do not change global defaults to
  allow cloud access. Record permitted host/model capabilities for the pilot.
- [ ] Create a tiny synthetic Unity package with two unrelated model entries and known payload
  bytes, plus a model/material/albedo dependency set. Reuse existing package fixtures/helpers.
- [ ] Add failing evidence tests for an ordinary file and an in-pack asset. Compare extracted bytes
  to the exact fixture payload; assert unrelated entries were not written, originals are unchanged,
  `workspace_rel` was not set and no duplicate asset was inserted.
- [ ] Implement the shared evidence manifest and bounded byte delivery, using image content blocks
  plus JSON text/structured metadata over MCP and authenticated representation URLs over REST.
  Resource reads must use this resolver and policy, not unrestricted filesystem access.
- [ ] Implement bounded workers, deadlines, safe cache publication and archive-identity checks.
  Test cold/warm reads, two concurrent reads of one entry, truncated archive, negative/overflowing
  offsets, missing dependency, oversize payload and cache invalidation after archive replacement.
- [ ] Verify file Range responses and authorized host resolution of both ordinary and in-pack file
  references. Test that a remote model cannot be assumed to fetch localhost/file URIs directly.
- [ ] Compare pixels/manifest limitations on MCP and REST; ensure a slow extraction does not block
  server-info. Run affected ingest/app/API/MCP tests and the existing desktop preview checks.

**Acceptance:** the host receives actual usable image evidence for models and can retrieve the
specific authorized source payload. A metadata-only response is never reported as visual analysis.

## Task 4 — Safe, attributable tags and optional description

**Files:** `crates/lodestash-core/src/catalog/mod.rs`,
`crates/lodestash-core/src/catalog/oplog.rs`, core schema migrations/tests,
`crates/lodestash-desktop/src/writer.rs`, `crates/lodestash-desktop/src/agent_bridge.rs`,
`crates/lodestash-api/src/lib.rs`, `crates/lodestash-api/src/routes.rs`,
`crates/lodestash-mcp/src/lib.rs`, and existing inspector/tag display and approval components.

**Consumes:** S03/S04 shared FTS/atomic history/provenance work, Task 2 revisions, Task 3 configured
producer policy and evidence. **Produces:** `write_tags` ↔ `POST /v1/tags/batch` and observable
app-owned jobs/receipts, with exact behavior in ENGINE-API §8.3. This is the focused S05 adapter.

- [ ] Establish failing transaction tests with manual/imported/two-producer tags and description.
  Cover append, producer replacement, empty producer set, stale revision, changed evidence,
  invalid label/confidence, missing mid-batch asset, permission revoked before approval and no-op.
- [ ] Implement the core validator and one transactional writer operation; do not loop calls to
  `set_tags` that commit independently or erase every AI producer. Write tags, optional description,
  FTS, history and durable request receipt together. Preserve queue/lifecycle semantics.
- [ ] Implement request-body digest/replay protection independently of undo retention. Test same
  ID/body replay, same ID/different body conflict, rollback, reopen and an ambiguous lost response.
- [ ] Add the new operation on both planes, app diff approval, producer enforcement and provenance
  presentation. Approval to apply model output must not set human confirmation.
- [ ] Rework forwarding/status/cancel to use the owning app job and current handshake. Test an app
  restart, startup without a handshake, timeout, cancellation before execution and committed replay;
  no nested job may report done before the catalog commit. Cancellation after commit cannot undo it.
- [ ] Test conflict-aware undo/redo and independent producer preservation; run core/API/MCP and
  affected desktop/UI tests. Read final values using a separate connection, not the write response.

**Acceptance:** one approved batch changes exactly the intended associations/description, records
AI provenance, survives a lost-response retry, and is independently visible through both planes.

## Task 4a — Vocabulary-guided review and approved extensions

**Files:** `crates/lodestash-core/src/catalog/mod.rs`,
`crates/lodestash-core/src/catalog/taxonomy.rs`, `crates/lodestash-core/src/catalog/oplog.rs`,
`crates/lodestash-desktop/src/writer.rs`, `crates/lodestash-desktop/src/agent_bridge.rs`,
`crates/lodestash-api/src/lib.rs`, `crates/lodestash-api/src/routes.rs`,
`crates/lodestash-mcp/src/lib.rs`, existing vocabulary/settings/approval UI and owning tests.

**Consumes:** Task 4's transaction/history/revision/job infrastructure, existing traits/taxonomy
helpers, and `identify_asset`. **Produces:** `get_vocabulary`, `review_tags`,
`propose_vocabulary_changes` and corresponding REST routes per ENGINE-API §8.5. No new classifier
or vocabulary store; distinguish raw tags, promoted traits and taxonomy categories.

- [ ] Create fixtures with applicable and wrong-modality traits, paused teaching, duplicate/synonym
  labels, two taxonomies, pinned category assignments, raw unmatched AI tags and protected manual tags.
- [ ] Implement bounded revisioned vocabulary reads and app-configured open/controlled/approved-
  extension policy. Test pagination invalidation, empty taxonomy and source-of-truth guidance.
  Feed vocabulary/revision into Task 4 writes; do not make `teach=false` mean forbidden.
- [ ] Extend the existing approved operation flow for exact association accept/reject decisions.
  Preserve producer provenance and human confirmation; record decision/reason and invalidate stale
  review claims. Reject protected associations and whole-batch stale revisions. Do not reuse the
  current all-source `remove_asset_tag` behavior for an AI-only rejection.
- [ ] Add approved vocabulary proposals: promote/create traits and create/reuse a category under
  an existing taxonomy. Test canonical collisions, invalid parent/taxonomy, unknown namespace,
  denied approval, teaching opt-in, lost-response replay and atomic rollback. No implicit mapping
  rule, cross-taxonomy change, new namespace or assignment follows category creation.
- [ ] Add tools/routes/allow-list mappings together and compare normalized errors/effects. Test
  a rejected AI tag also present manually, an accepted tag retaining AI source, and a stale receipt
  after reidentification. Test that a later model run can repropose a rejected term: suppression is
  not implemented by deleting one association.
- [ ] Exercise identification → fresh read → vocabulary proposal/approval → fresh vocabulary read
  → review/write → final read in a disposable fixture. Inject failure at each boundary and report
  prior completed stages accurately; this is not one global transaction.

**Acceptance:** Codex can read the governing vocabulary, keep/reject specific AI assignments, and
propose useful new traits/categories, while the app validates/applies approved changes. It cannot
forge human confirmation, remove protected tags or silently expand vocabulary through a tag write.

## Task 5 — Contract and workflow checks

**Files:** extend the API/MCP test modules and the spawned smoke harness from Task 1.

**Consumes:** Tasks 1–4a. **Produces:** reproducible evidence that the workflow is usable, not just
equal tool-name sets. The host's workflow instruction uses the template in USER-GUIDE.

- [ ] Add a contract fixture that compares normalized REST/MCP input semantics, output, error
  codes, disabled-tool behavior, consent and catalog/cache effects for every new operation.
- [ ] Extend the spawned harness: discover source → search → select one/list/random ten → save
  existing tags/evidence → read vocabulary → identify with a stub model → fresh read → review and
  propose new vocabulary → approve using a disposable app fixture → poll → refresh revisions →
  write/review → final independent readback → undo. Never forge a human flag in an agent payload.
- [ ] Keep approval automation restricted to a test fixture; the real pilot uses the app's normal
  approval. Use a stub analysis result for deterministic tests, not a paid/cloud model call.

The harness must assert the following facts using Node's existing standard assertion library:

```js
assert.equal(new Set(selectedIds).size, 10);
assert.ok(selectedAssets.every(a => a.source_id === chosenSourceId && a.media_type === 'model_3d'));
assert.ok(evidence.every(e => e.status === 'ready' || e.status === 'partial'));
assert.deepEqual(after.map(a => a.id), selectedIds);
assert.deepEqual(persistedProducerTags, normalizedSubmittedTags);
assert.deepEqual(preservedManualTags, originalManualTags);
assert.equal(replayedOperationId, firstOperationId);
```

`selectedIds`/`selectedAssets` come from the saved selection; `evidence` from Task 3 manifests;
`after` is the ordered Task 4 readback. Compute the tag arrays from full association records,
sorted by asset ID/namespace/value/source, and operation IDs from first/replayed job receipts.
Compare descriptions, lifecycle, originals and rollback state separately in the owning tests.

## Task 6 — Register the real host and perform the ten-model pilot

**Files:** setup/workflow guidance in `docs/USER-GUIDE.md` and verified status in the board;
host-local MCP configuration is changed only during implementation/setup, not this planning turn.

- [ ] Use the confirmed Codex host and tags/short-description scope; resolve library/consent and
  vocabulary-policy choices in the design. Use the existing
  repo launch instructions; do not register `cargo run`, Vite or port 5173 as the MCP service.
- [ ] Register the freshly verified executable with `--mcp` and a host allow-list restricted to the
  workflow. Restart/reconnect the host's MCP server and verify tools are callable in the actual
  conversation. A shell-only test does not establish that the assistant sees the tools.
- [ ] Run one asset first, then an explicit list, then ten random 3D models. Save the existing tags
  and evidence, identify only the selected IDs, then review against the current vocabulary. Keep
  IDs/request IDs across stages, approve proposed batches and independently read each result back.
- [ ] Report names/IDs, source/package, evidence quality, existing/identified/final tags,
  accept/reject reasons, new vocabulary outcomes, descriptions and failures.
  Human-review the visual labels. Do not use missing evidence as grounds to invent tags or mark
  an asset confirmed. Leave any skipped assets explicit.
- [ ] Mark implementation cards done only after their own gates. Update shipped/proposed status
  in owning docs, run `node scripts/check-docs.mjs` and `node scripts/check-board.mjs`, and inspect
  the final diff against the design. Commit/push only if separately requested.

## Handoff stop condition

This planning turn stops after saving/reconciling these documents and running the documentation
checks. No app launch, build, migration, MCP registration, package extraction, model call or tag
mutation is necessary to make the plan reviewable. Implementation remains deferred to another model.
