# Lodestash v1 architecture review and sequenced plan

Reviewed 2026-09-07. Planning artifact only: no production refactor, catalog migration, board replacement, or new runtime dependency is implemented by this review.

**2026-09-10 focused refinement:** [MCP asset analysis](../../MCP-ASSET-ANALYSIS.md) and its
[implementation handoff](2026-09-10-mcp-asset-analysis.md) refine S04/S05/S10/S11/S12 for the
interactive one/list/ten-model use case. [ENGINE-API §8](../../ENGINE-API.md#8-proposed-asset-analysis-contract)
owns its proposed wire details, including an app-approved batch job and durable retry receipt.
Those details refine the earlier S05 request sketch; they do not supersede shared FTS/history,
privacy or bulk-job prerequisites. All implementation remains deferred. The new documentation card
does not perform the broader board migration described below.

**Goal:** an agent can enumerate the library, obtain enough evidence to judge an asset, and persist attributable tags in bounded batches while browsing remains responsive and undo remains useful.

**Scope:** retain the architecture and repair its shared contracts. The owning specifications remain authoritative; proposed changes below become normative only when their ADR and owning-doc changes land with the corresponding implementation. This plan follows the work order R1 → R2 → R3 → R4 → R5 → proxies/pack walks → release decision.

**Execution:** use the single reconciled backlog in this document for this work. On implementation kickoff, publish it into `docs/KANBAN.md`, archive the two older boards, and make `KANBAN-SVELTE.md` a pointer. Those file changes are deliberately not made during this planning-only review. Move each implementation card Ready → In Progress → Review → Done; a listed test is an acceptance requirement, not a claim that it already exists or passed.

## This stays

- Rust core and app orchestration; Tauri/Svelte desktop and current WebGL2 viewer. No shell rewrite, new Engine actor, generic plugin framework, or service extraction.
- SQLite WAL and one writer in the app, with read connections for GUI/REST/MCP. Keep ADR-0002 and ADR-0011: app-closed MCP writes remain unavailable. Throughput comes from bounded batches and doing expensive work outside the writer.
- `asset_tag`, `tag`, `op_log`, lifecycle and the seven media types. The tag association table can represent simultaneous models using its existing TEXT source key; it does not need replacement.
- Local inference and configured hosted/subscription inference. Embeddings remain local. Endpoint policy, not transport name or price, determines egress permission. Audio bytes never go to a remote provider.
- Shared REST/MCP services, versioned APIs, and the existing agent-operation inventory. Keep the documented REST-only file/plugin transport exceptions.
- Exact brute-force cosine search, separate SigLIP/CLAP spaces, RRF and `VectorIndex`. Cache vectors before considering another index.
- Existing mesh parsing, CPU rasterization, material shader ball, waveform/audio playback, SVG fallback, geometry proxies and surgical extraction. Consolidate identity and scheduling around these implementations.
- Source references, stable asset IDs, Unity GUIDs, human confirmations and import provenance. Originals and Unity cache are never changed; derived files stay in Lodestash caches.
- The current paged grid. Virtualization is not a prerequisite for fixing invalidation; measure the bounded page after R1 before adding it.

Non-goals remain Unreal/Godot, UnityPy, smart collections, variant grouping, an ANN/index swap and the deliberately deferred 4k thumbnail decode spike. First-class prefabs, a standalone daemon and new provider integrations are separate decisions, not concealed prerequisites.

## Evidence that changes the diagnosis

Paths and citations below are relative to the repository. The README doc map, owning area specifications and ADRs were checked against their current implementations.

| Finding | Consequence for the plan |
|---|---|
| The desktop `writer.rs` (line 899 then; `crates/lodestash-engine/src/writer/mod.rs` since v2 Task 2.3) emitted catalog and jobs after every ordinary op, including errors/no-ops; idle paths at 1147/1158 also emit. `crates/lodestash-desktop/ui/src/App.svelte:105` fetches status and increments the broad version. | Fix the emitter and all producers first. Quieting only forwarded writes misses the continuously active source. Count mounted effect executions/IPC calls, not every textual reference as a simultaneous fetch. |
| The zero-duration receive in the desktop `writer.rs` (line 1048 then) consumed and discards a queued message. | R1 must also preserve that message. A responsiveness change cannot leave writes occasionally disappearing at the idle boundary. |
| BulkTag records one history entry already, but mutates assets in separate transactions before recording it (the pre-S04 `BulkTag` handler and `record_bulk_tag_op`; fixed by S04, ADR-0017). | No new grouped-undo concept. Make data and full before/after history atomic; a failure midway must leave neither partial changes nor orphaned history. |
| Current undo inverses can remove other sources and restore manual rows without their confidence/model (the pre-S04 `revert_bulk_tag` / `revert_set_tags`; fixed by S04, ADR-0017). | Undo needs complete association snapshots and explicit scope, not merely fewer log rows. |
| Fallback/zero-shot results can be stored under the requested API model (`drain_api_tags`, `crates/lodestash-engine/src/appraise/cloud.rs`). | Preserve producer identity before merging proposals; changing source strings alone cannot repair falsely attributed input. |
| M11 replaced the original empty-tag FTS triggers (`crates/lodestash-core/src/catalog/mod.rs:2223`). | Do not report `promote_statuses` clearing tags to an empty string as an established M24 bug. It still causes needless FTS work on every updated asset. |
| FTS external content is `asset`, while `tags_concat` is not an asset column (`crates/lodestash-core/src/catalog/mod.rs:1725`). | There is a distinct maintenance defect: ordinary MATCH rowids can work while external-content reads/rebuild fail. Repair the content contract and rebuild derived FTS in a new migration. |
| Seven empty-map mesh calls comprise four production callers and three tests/probes. `mesh::load_gltf` discards textures; cached mesh decoding restores `albedo: None`. | Supplying a map is insufficient. Test native materials, slots and cold/warm preview equivalence. Keep embedding and metrics callers untextured. |
| `model_albedo_texture` picks the lowest texture ID; parser loses texture property names and prefab pairing is whole-file Cartesian product. | Preserve material-role and renderer-slot evidence. A lower-ID normal map must never become the albedo. |
| Prefab evidence refresh skips extracted workspaces and depends on asset dependency jobs; its wholesale edge replacement can lose mute state. | Refresh prefab evidence independently, incrementally and per successfully visited source. Preserve unreadable-source evidence and user suppression. |
| Materialization fills `workspace_rel` but drops hashes from matched extracted records (`crates/lodestash-core/src/catalog/mod.rs:989`, egui-era `lodestash-app/src/persist.rs:184`, deleted in v2 Task 2.2). | NULL identity is not only a pre-extraction condition. Persist verified hashes on existing IDs, while retaining a valid locator fallback for unread payloads. |
| Recursive desktop eviction exists (`crates/lodestash-engine/src/storage.rs:237`), but lacks the documented preservation exceptions; `thumb::evict_cache` is an unused PNG-only alternative. | Consolidate the existing maintenance policy. Do not build a third evictor or claim there is no recursive implementation. |
| Cloud controls were restored in September: presets/Test/paid confirmation and keychain code now have Svelte callers. | The July cutover hypothesis was worth checking, but it does not justify rebuilding these controls now. Per-modality consent and endpoint classification remain real gaps. |
| The board checker calls 18 historical-citation notes plus five absence warnings “23 absence claims.” Its substring test also treats the Svelte board as historical because a card quotes the marker. | Exit 0 is not board conformance. Reconcile the 23 items individually and later fix marker parsing/counters. |

## Measurements and acceptance budgets

**No product “after” measurements exist in this review.** The user narrowed the task to planning. Numbers below distinguish a current executable probe, historical evidence, arithmetic estimates and proposed gates; none represents a shipped optimization.

| Experiment | Before evidence | After estimate or required measurement |
|---|---|---|
| R1: 50 ordinary tag writer ops | The code emits 50 catalog + 50 job events, excluding concurrent workers; catalog handling schedules 50 status requests/version bumps. This is a code-derived count, not a frame trace. | With all commits inside one 250 ms coalescing window: one catalog publication; no jobs publication unless jobs changed. Agent-origin catalog changes cause zero automatic grid/status rereads, one pending indication and one refresh when requested. Longer bursts are bounded by windows, not promised to collapse to one event. |
| R1: grid while backlog drains | Reported reference backlog: 8,155 dirty rows. ADR-0014 historically measured 0.57 → 0.26 CPU cores from duty cycling, not from event coalescing. No fresh UI p95 is available. | Record a 60 s trace on the same visible page/cache state while actual drain work completes. Background catalog changes must not refetch the grid; job progress remains live. Target input-to-paint p95 ≤100 ms and no catalog-refresh task >50 ms; retain throughput and writer-queue timings alongside UI measurements. |
| R4: current release probe, 10k × 768 | Existing `perf_probe_knn_scaling`: LOAD 41.28 ms; SCAN+SORT 6.34 ms; total 47.6237 ms in this session. One sample, synthetic catalog, no encoder/HTTP/UI cost. | Reusing the measured index would remove the repeated 41.28 ms load: a 6.34 ms component floor, approximately 7.5× less retriever work. This is arithmetic, not a measured cached product query. |
| R4: historical 50k × 768 | TECH-STACK §2 records 212 ms/query and load ≈5.6× scan on 2026-07-20. | That ratio implies about 180 ms load and 32 ms scan: a ~6.6× component ceiling from eliminating reload. Re-measure current release; do not compare different machines/build profiles as before/after. |

The release acceptance run uses an online SQLite backup of the live M24 catalog, plus deterministic 6k/50k/100k vector fixtures. Do not copy only a live `.db` while WAL exists. Use a fresh baseline and candidate build, identical configuration, viewport, assets and cache warmth; run one warmup and at least 20 paired repetitions. Record median/p95, frame long tasks, writer queue wait, events, status/grid IPC counts, index loads, drain throughput and resident memory. Confirm equal result IDs/scores before comparing speed. Exclude model loading/encoding only in a separately labelled retriever measurement.

For R1, run both an uninterrupted drain and 50 tag writes, including concurrent drain completion. Never benchmark against the current e2e harness unchanged: it disables the preview drain and uses the user's real configuration. Isolate the catalog/config/cache and do not kill an existing user app.

For R4, warm queries must perform **zero embedding BLOB loads** until the embedding generation changes. Test a GUI write observed by the headless process as well as an in-process write. Publish cold load, first query after invalidation and warm query separately. A cache with a fast stale answer fails the gate.

## Slice sequence

Each numbered item is independently reviewable and intended to fit one commit. Estimates are focused engineer-days, including its focused tests/doc edits, excluding waiting for model inference and corpus acquisition. If a slice exceeds its upper estimate, split at the stated contract boundary rather than broadening it.

**Every slice gate:** run `cargo test -p lodestash-mcp tool_set_matches_the_rest_surface`, the affected crate tests, and `node scripts/check-docs.mjs`; run `node scripts/check-board.mjs` for a board change. UI changes also require the Svelte check and the named real-binary interaction check. Name parity alone is insufficient for slices changing wire behavior: compare REST/MCP inputs, normalized outputs, failures, authorization and catalog effects. No tool or route lands alone. Existing v1 defaults/response shapes stay compatible; an unavoidable breaking shape receives a new version on both planes.

Migration identifiers below are **provisional** from current M24; allocate the next free version at implementation time. Do not rewrite an applied migration. “No migration” still means replaying the acceptance fixture on a copied M24 catalog without resetting it.

### S01 — R1: lossless queue and coalesced change publication

**Change:** `crates/lodestash-desktop/src/writer.rs`, including every `emit_changed` caller and worker/job completion. Replace unconditional broadcasts with committed-change accumulation by domain and origin. A single timed publisher drains pending changes on a fixed maximum interval (start at 250 ms), rather than a trailing-only debounce that starves during continuous work. Publish only actual changed domains. Preserve the message found at the idle probe by processing it or retaining it for the next loop; never receive-and-discard.

**Contract:** catalog generation increases after committed catalog mutations; jobs have their own generation. Carry `user`, `agent` or `background` origin through the bridge; mixed windows preserve every origin. Headless with no AppHandle remains event-free. Publication waits must not hold the writer or database transaction.

**Check:** deterministic clock/channel test: 50 writes, continuous producers, a no-op, an error, mixed origins and an op arriving exactly at the idle boundary; every accepted op replies once, no event is lost or indefinitely deferred, no failed transaction advertises success. Then the R1 trace above.

**Migration/docs/cost:** no DDL; new process-local event shape updated with all consumers in the same commit. ARCHITECTURE §5, UI-SPEC §§2/18, proposed ADR-0016. **1–2 days.**

### S02 — R1: stable browsing with pending updates

**Change:** `crates/lodestash-desktop/ui/src/App.svelte`, `crates/lodestash-desktop/ui/src/lib/api.ts`, `nav.svelte.ts`, Library/Inspector and job consumers. Agent/background catalog publications set an accessible “Library updated · Refresh” indication; they do not bump the broad library version or fetch status. Refresh captures generation G, reads the required current page/count/detail state once, then clears pending only through G. Changes arriving during the fetch remain pending. Navigation may consume pending changes explicitly as a new view load.

User mutations refresh affected visible state once after success. Remove duplicate local-plus-event refreshes. Jobs stay live on their own channel. Preserve selected asset IDs, scroll position, focus and audio playback; disappearance of an asset on explicit refresh has an intentional nearest/empty selection behavior. A per-asset preview readiness update may replace its placeholder without rerunning the library query.

**Check:** drain plus 50 forwarded writes produces no automatic grid/status refresh; pending appears; one click refreshes and leaves a racing later change pending. A manual tag shows promptly once. Undo, job cancel, keyboard focus and screen readers still work.

**Migration/docs/cost:** no DDL; no new UI state library. UI-SPEC §§2/13/18 and DESIGN-GUIDELINES feedback/accessibility. **1–2 days.**

### S03 — R2: repair FTS before increasing tag throughput

**Change:** core catalog schema/FTS helpers and catalog tests. Make external content a canonical SQL view exposing `id`, `name`, `description`, and deterministic tag text. Recreate FTS against that view and rebuild from canonical rows. Narrow asset triggers to searchable field changes; lifecycle/dirty/timestamp updates do not touch FTS. Tag changes refresh once per changed asset; combined tag/caption writes coordinate one refresh, not a caption trigger followed by another tag refresh. Tag rename/delete and asset deletion must also preserve correct old/new indexed text.

**Check:** reproduce the existing external-content rebuild failure first. After migration, MATCH, content reads and rebuild agree; replacing/removing/renaming tags removes old tokens; promotion and embedding-dirty updates do zero FTS maintenance. Test caption+tags, rollback and reopen. Measure FTS statement counts for a 50-asset batch.

**Migration/docs/cost:** **M25**, transactional rebuild of derived FTS only; no source, tag or embedding rewrite. Backup first, measure upgrade time on M24. DATA-MODEL FTS contract and IDENTIFICATION-AND-SEARCH. **1–2 days.**

### S04 — R2: one atomic tag mutation with truthful provenance

**Change:** core tag functions/oplog, app drains and desktop writer callers. One batch contract accepts N assets with explicit **append**, **replace this producer's set**, or **remove scoped associations**. Human rejection across all sources is a distinct explicit scope, never the implicit meaning of removing a manual tag. Actor, producer and transport are separate facts. Human-only confirmation remains outside agent authoring.

Use `source = ai:<canonical producer id>` with the actual `model_id`; manual/imported sources retain their meaning. Keep each model's associations independently replaceable. Preserve source/model on fallback and zero-shot contributions before persistence; never attribute a local fallback to the failed remote model. Two endpoints using an ambiguous bare model alias need distinguishable producer IDs; do not put keys or credentials in identifiers. Keep descriptions scalar: only the configured caption-producing result replaces the caption, atomically with its tag result; simultaneous tag models do not imply a new multi-caption system.

Normalize and deduplicate before facet limits. A shared core boundary validator enforces namespace/modality policy, nonempty labels, length/count limits, and finite confidence in [0,1]. It rejects confidence objects/numbers in label slots. Human free-form and audio/technical namespaces follow their own explicit allowed policy, rather than inheriting the vision parser's six-facet limit. Parser output is untrusted even when it came from a local model.

Within **one writer op and one transaction**, capture complete touched-row before/after state (source, model, confidence, timestamps), apply tags/caption, maintain FTS, and record one versioned op-log entry. A no-op neither consumes undo nor raises catalog generation. Completion is explicit: successful/unsupported work can clear its queue state; rate-limited/failed work stays dirty; a manual edit does not pretend that model work completed. Validate the entire batch before mutation; a mid-batch failure rolls it all back.

Start with a maximum of 100 assets and a bounded serialized payload (1 MiB) per request. These are proposed measured limits, not new settings. A 6k walk then consumes about 60 logical batch rows, not 6k rows. The 200-row policy remains a cap on logical operations. Background model batches must not silently consume the entire interactive undo tail: partition retention by recorded origin, reserving the 200 recent interactive operations and bounding background history separately. Test interleaving explicitly. No promise of permanent history.

Undo/redo restores only touched associations and caption state; it must not erase a newer independent model's rows. If those exact associations changed after the recorded operation, report a conflict rather than overwriting them. This is snapshot-based undo, not a new event-sourcing architecture.

**Check:** manual+two-model coexistence; producer-specific rerun; null confidence still AI; fallback attribution; duplicate normalization; confidence-in-label input; NaN/infinity; missing asset at batch midpoint; 100-asset undo/redo; 201 separate logical operations; thousands of background results interleaved with a manual edit; no-op; failure/unsupported/rate-limit queue states; confirmation never created by an agent.

**Migration/docs/cost:** **M26** data migration for source keys where legacy `model_id` is known; unknown rows become explicitly legacy AI, never invented provenance. Keep legacy incorrectly merged attribution marked unrecoverable. Preserve old inverse-only history but disable only unsafe legacy tag replay with a visible explanation; new history uses complete versioned payloads. `asset_tag` needs no DDL. DATA-MODEL §§2/8/13, AI-PIPELINE, IDENTIFICATION-AND-SEARCH and proposed ADR-0017. **2–3 days; split producer migration from transactional writer implementation if review exceeds this scope.**

### S05 — R2: all adapters share the contract

**Change:** REST routes, MCP tools, agent bridge, desktop commands and tag presentation. Add `write_tags` ? `POST /v1/tags/batch`, with identical validation, permissions, mutation semantics and readback. Proposed request: a unique `request_id` and `assets[]`, each containing `asset_id`, explicit mutation mode/scope, producer/model, tag assignments and optional caption. Actor authority comes from the trusted ingress/configuration, not a client-provided `human=true` field. Existing add-tag remains a compatibility adapter; do not silently reinterpret already-recorded manual rows as AI. New legacy-agent writes without a configured producer are explicitly `ai:legacy-agent` with unknown model/confidence, not human manual tags. Preserve the old wire request/response and append semantics; document the provenance correction. The new operation carries explicit producer information and is the required path for model-authored batches. Return operation ID and changed asset count so retry/review can locate its history.

Replace every exact `source='ai'` reader with one shared AI-source classification covering legacy and namespaced sources; UI AI styling uses provenance rather than `confidence != null`. Read APIs expose source/model as well as confidence. Undo operates through the same core contract. Keep other operations, such as unstage, out of this tag slice unless separately carded.

**Check:** shared REST/MCP fixture submits the same batch and compares tags, caption, history, errors, limits and tool-disabled behavior. Two-model readback through GUI and both planes; legacy add remains compatible; source filter/counts include namespaced AI. Test a lost-response retry: choose a request id recorded with the batch and return its existing result instead of appending history twice.

**Migration/docs/cost:** no association-table DDL; add durable request-id uniqueness with the next migration if the existing log cannot support it. Additive v1 operation; do not break old envelopes. ENGINE-API §2 owns the wire contract; MCP-AND-ENGINES links to it. UI-SPEC/AI docs update in this commit. **1–2 days.**

### S06 — R3: component-scoped prefab evidence

**Change:** `crates/lodestash-ingest/src/deps.rs`, app dependency drain/persist, core relationship storage. Parse Unity YAML documents with document fileIDs, rather than whole-file regex buckets. Join MeshFilter and MeshRenderer by owning `m_GameObject`; retain SkinnedMeshRenderer's own mesh and material slots. Preserve texture property names such as `_BaseMap`/`_MainTex`. Do not generate a Cartesian product across unrelated objects.

Give evidence a source locator/revision and component/slot identity. Schedule refresh on prefab-only edits and extraction, including workspace prefabs. Upsert visited evidence and remove only evidence proven deleted in a successfully scanned scope. Preserve muted links, unresolved GUIDs and unavailable-source evidence. Do not claim nested prefab instances/variant overrides are solved by GameObject grouping; report unresolved bindings honestly.

**Check:** two GameObjects yield two intended pairings rather than four; skinned mesh with two slots; edit/delete only the prefab; workspace-only prefab; source unavailable; late GUID resolution; ignored edge survives refresh. General export closure remains correct.

**Migration/docs/cost:** next migration for durable evidence identity/slot fields if existing relationship rows cannot express them; resumable one-time evidence reharvest, no broad deletion before successful replacement. DATA-MODEL §10, INGESTION, proposed ADR-0018. **2–3 days.**

### S07 — R3: resolve native material inputs for previews

**Change:** core albedo lookup, material parser, `crates/lodestash-ingest/src/mesh.rs`. Resolve by material slot and property, not minimum texture ID. Precedence: native embedded/declared material bindings, resolved Unity evidence for that renderer, then documented naming heuristic for edge-less exports. Multiple prefab appearances of one mesh must not be arbitrarily merged into one “correct” appearance: choose a deterministic evidenced variant and expose ambiguity.

Read glTF/GLB material/image data on the **model** path, including embedded buffer views/data URIs and external images. Keep FBX multiple material slots and declared/embedded inputs. Resolve only available, nonignored dependencies; normals/masks are not albedo. Keep geometry metrics independent of texture loading.

**Check:** lower-ID normal and higher-ID albedo; two-material FBX; GLB embedded PNG; external glTF texture; direct heuristic edge; missing dependency; late resolution; ambiguous prefab appearances.

**Migration/docs/cost:** no asset/media-type migration; version only affected preview input identities and queue their redraw. DATA-MODEL §10, INGESTION and TECH-STACK renderer limitations. **1–2 days.**

### S08 — R3: wire those inputs through cold and warm rendering

**Change:** CPU model preview and Quick Look paths, mesh cache hydration, frontend material-slot binding. Feed the resolved inputs from S07 through each **preview** path. Geometry-only `.msh`/`.lod` caches may remain geometry-only; attach texture bindings after load so warm/cold pixels agree. Do not serialize raw textures into every mesh cache entry merely to achieve parity.

Protect the embedding route explicitly: it retains the original empty-map/material behavior, geometry proxy parameters and rendering inputs. A shared loader default must not quietly color/re-embed the library. The current UV-averaging simplifier is acceptable for silhouettes; textured inspection uses full geometry initially, or a clearly labelled limited proxy. No new simplifier in this slice.

**Check:** cold and warm textured preview pixels agree; UV orientation and slots are correct; same input yields byte-identical embedding render and vector under the existing model; source triangle counts unchanged. Real corpus review must show that material appearance improves judgment, not just a colorful wrong texture.

**Migration/docs/cost:** no DB migration; bump preview render version and lazily regenerate affected previews; retain old fallback until replacement succeeds. Do not raise `embedding_dirty`. INGESTION/TECH-STACK/AI-PIPELINE and proposed ADR-0019. **2–3 days.**

### S09 — R4: resident embedding snapshots with correct invalidation

**Change:** core vector/query seam, API `ServerState::similar`, hybrid search and desktop query callers. Load an immutable snapshot per catalog/model/dimension once and reuse it. Share the handle inside each process; headless has its own bounded process cache. Filtering is performed before scoring/top-K using current eligible IDs, so trash/source/status/tag changes cannot leak stale candidates. BM25 must use the same hard-filter set before its LIMIT/RRF, too.

Use a durable per-model embedding generation updated in the embedding write transaction (including deletes/cascades/model replacement). This solves cross-process invalidation; GUI events cannot notify an independent headless cache. Read generation and vectors in one consistent read snapshot; publish the replacement atomically. One rebuild per generation, not one per waiting query. Tag-only writes do not reload vector BLOBs. Use current asset eligibility separately; scope caches to the canonical DB identity so two catalogs cannot share entries.

Retain cosine scoring, sorting, RRF and separate model spaces. No ANN, DB vector extension or normalization rewrite. Budget raw memory: 50k×768×4 ≈146.5 MiB; 100k ≈293 MiB **per visual snapshot**, plus IDs/allocations/audio and temporary old+new snapshots. Avoid retaining unused models indefinitely.

**Check:** equal ranked results/scores; concurrent misses load once; embedding update/delete/restart; a GUI-process write observed headlessly; tag-only update loads zero vectors; filtered candidates never leak; corrupted dimensions cannot panic. Run the paired R4 measurement protocol and report peak memory during rebuild.

**Migration/docs/cost:** next additive migration for generation metadata; empty cache on restart, existing embeddings untouched. A rebuild failure returns an explicit error or valid uncached result, never silently stale success. DATA-MODEL §3, IDENTIFICATION-AND-SEARCH §5, TECH-STACK §2, ARCHITECTURE §5. **1–2 days.**

### S10 — R5 prerequisite: enforce endpoint-based egress and consent

**Change:** configured provider policy, app config/resolvers and shared preview/file byte-release boundary. Parse URL scheme/host rather than checking whether a URL contains `localhost` or `127.0.0.1`. Price/free-tier status is separate from network locality. Deny or revalidate redirects before credentials/payload cross a new origin. Keep arbitrary endpoint support within explicit remote consent.

Per-modality permission governs image/model/material/sprite-derived pixels as well as originals. A spectrogram is derived audio evidence: local use is allowed; do not route it to a remote model to sidestep the audio prohibition. Remote audio enrichment receives only permitted local text. Introduce the specified `mcp_bytes_enabled` policy where bytes leave the catalog for a configured remote agent endpoint; metadata access alone must not imply permission to upload files. A local stdio connection does not establish that its client model is local: unknown destination gets metadata/links or a denied byte response until configured consent exists. Apply the same policy to direct REST, MCP, push providers and file/resource delivery.

**Check:** loopback endpoint, remote free endpoint, `localhost.example`, misleading URL paths, IPv6 loopback, redirects, consent revoked mid-run, image denied/audio denied through every entry point. Negative cases assert **zero outbound payload requests**, not just an error returned afterward.

**Migration/docs/cost:** config expansion defaults new per-modality permissions off; preserve old settings without silently widening consent. No catalog migration. AI-PIPELINE §§5/5.1, ENGINE-API/MCP privacy sections, proposed ADR-0020. **1–2 days.**

### S11 — R5: headless startup and nonblocking forwarding

**Change:** desktop `run`/headless initialization, agent bridge, MCP/REST handlers. Verify that headless service construction uses a genuinely read-only catalog open: `ServerState::new` currently calls `Catalog::open`, which can initialize/migrate. Schema setup belongs to the sole writer; a headless reader must report an upgrade requirement rather than migrate the live catalog. Apply managed ORT runtime setup before the `--mcp` branch initializes models. Use bounded blocking workers for ureq, rendering, archive reads and inference; an async handler must not sleep/block the current-thread runtime. Move work outside the shared catalog mutex after collecting its inputs.

Long operations return the app's durable/observable job identity promptly. Proxy status/cancel to the owning app; do not wrap one app job in a misleading second job, or await it for 30 minutes inside a tool request. Re-read the handshake for each new operation so app restart does not strand the client. Never automatically retry an ambiguous mutation without S05's request-id semantics. App absent remains a clear read-only limitation.

**Check:** managed-runtime-only install, no ambient ORT env; semantic search through spawned stdio; a deliberately slow preview/forwarded job while info/status/cancel still respond; app restart between requests; app closed. Verify both SigLIP and CLAP query wiring rather than describing visual-only search as multimodal.

**Migration/docs/cost:** no DDL except shared job persistence in S13 if needed; preserve v1 job response compatibility or version both planes together. ADR-0011 amendments, ENGINE-API, MCP-AND-ENGINES, TECH-STACK. **1–2 days.**

### S12 — R5: a bounded, resumable library walk

**Change:** `list_assets` ? `GET /v1/assets`, one shared browse/filter request with `import_source` library ID, media, lifecycle/tag state and keyset cursor. Expose it additively on both planes; retain existing search responses for old clients. Search ranking is not a stable full-library traversal order.

Cursor order is asset ID with a captured upper ID bound and filter signature. New assets wait for the next walk. Mutable tag/status filters are evaluated consistently for each requested page; disclose that a changed eligibility set can require another pass. A complete bulk job uses its durable selected workset, rather than relying on a mutable search cursor. Reject malformed/mismatched cursors and enforce page limits. Apply source/media/status/tag filters before LIMIT on both BM25 and vector candidates.

Preview requests select a bounded evidence tier, dimensions/views and supported kind. Return availability/limitations, producer/dependency context and continuation without stuffing thousands of previews into one response. Existing thumbnails remain the cheap tier; richer proxy generation is S16. In-pack resources resolve through the existing surgical/cache path subject to S10's byte policy, not only when an extracted file happens to exist.

**Check:** enumerate >page-size assets across sources without duplicates; ties, inserts, eligibility changes, missing/trashed assets and cursor misuse; only ten qualifying models behind hundreds of other media still return those ten; REST/MCP wire/effect parity; missing archive and prohibited byte response.

**Migration/docs/cost:** no asset-schema change; indexes only if an EXPLAIN/profile shows a missing access path. ENGINE-API owns exact cursor/version/errors; MCP-AND-ENGINES links to it; DATA-MODEL/IDENTIFICATION-AND-SEARCH share filter vocabulary. **1–2 days.**

### S13 — R5: the app owns unattended identification

**Plane decision:** the push worker in the app owns the complete bulk pass. MCP is for selection, inspection, submitting a bounded job, checking progress and correcting results. Do not make an LLM conversation remember a 6k loop. Both REST and MCP still expose the same controls; neither transport owns a second execution engine.

Persist the run scope/workset, configured actual producer, consent/budget revision, progress and per-item result/retry state. Reuse the existing jobs and dirty queues; add only the durable run/item data they lack. Compute off the writer, commit through S04/S05 in bounded batches. Resume local interrupted work automatically only within the previously authorized scope; remote work resumes only if endpoint, consent and remaining budget still match. Do not silently turn the idle lane into paid cloud work.

Use bounded retry with not-before timing so rate-limited dirty assets remain retryable without a hot loop. Unsupported is terminal for that configured producer; failure is not success. Cancellation stops scheduling, lets the current atomic commit resolve, and leaves remaining work visible. Never claim exactly-once remote inference across a crash after the provider accepted a request: use provider idempotency if available; otherwise record an uncertain attempt and do not automatically incur duplicate spend.

**Check:** client disconnect and app restart midway; resume without duplicate tag/history effects; cancellation; 429 with Retry-After; unsupported modality; changed endpoint/consent; budget exhausted; mixed local fallback identities; remote uncertain attempt. Job status remains responsive while running.

**Migration/docs/cost:** additive durable run/item migration if needed; existing dirty flags seed a new explicitly scoped run, not an automatic 6k remote run. No second writer or standalone daemon. AI-PIPELINE, ARCHITECTURE §5, ENGINE-API jobs and proposed ADR-0020. **2–3 days.**

### S14 — Proxy identity that works before extraction

**Change:** persist hashes from materialization onto existing asset/file IDs; hash during an already-required payload stream, not a separate archive sweep. Until payload hash exists, use archive revision + entry GUID/path + offset/size as identity. A catalog-verified archive revision must change on observed replacement; same-path/same-offset is not content identity. Revalidate against the manifest when the archive revision changes. Where exact change detection needs hashing, hash the archive once per validation event, not every entry pull.

Preview keys include generator version, tier/parameters and material/dependency revisions; late binding or a texture edit invalidates dependent model/material previews. Model embedding keys stay on their unchanged rendering contract. No global “make hashes non-NULL” migration.

**Check:** 5,963 reported unhashed in-pack entries remain representable; analyze→extract preserves IDs/tags and fills hashes; same-path archive replacement invalidates scratch; changed texture/late edge invalidates preview; unaffected assets retain caches.

**Migration/docs/cost:** add archive revision metadata only if existing package identity cannot provide it. Background backfill hashes already-materialized bytes; no archive inflation/model loading inside migration transactions. DATA-MODEL §7/10, INGESTION cache table, proposed ADR-0019. **1–2 days.**

### S15 — One bounded cache maintenance policy

**Change:** move the existing recursive desktop policy into shared app ownership and retire the unused PNG-only alternative. Enforce at startup and bounded generation checkpoints in GUI/headless use. Traverse only the resolved cache root; do not follow symlinks/junctions. Protect active writes/readers, retained workspaces, package icons without a regeneration path and scratch whose original archive is unavailable. Preserve unknown legacy scratch conservatively until its provenance is known.

Use size accounting and oldest-regenerable-first eviction; do not label mtime as read-LRU. Preview artifacts have the existing configured budget; extracted workspaces retain their separate policy. The archive-size guard and cache-byte budget are different limits: preserve ADR-0005's thumbnail-only archive guard and explicit-open behavior.

**Check:** nested inpack growth is capped, mixed extensions handled, protected artifacts survive, temp files cannot be served half-written, concurrent generation is safe, directory links never escape the cache root. A long headless run triggers maintenance without an AppHandle.

**Migration/docs/cost:** no DDL required for the basic policy; adopt provenance from S14. No eager deletion of unknown legacy entries. INGESTION retention table, UI-SPEC Storage, proposed ADR-0019. **1–2 days.**

### S16 — Shared visual tiers, reusing the three existing systems

**Tier ladder:** metadata/type icon → cheap existing thumbnail/SVG → bounded judgment preview → deliberate original/full-detail open. A model's judgment preview is a textured hero or fixed multiview contact sheet, not a silhouette mislabeled as its material appearance. A material uses the current shader ball; images use bounded raster; audio uses current waveform/local playback plus locally produced description and optional local spectrogram. Never imply a waveform proves timbre/semantic identity.

Use one resolved preview request/key for GUI and agent paths, with a bounded generation queue and in-flight deduplication. M15 SVG remains fallback; color raster becomes primary for model judgment as it is generated. Keep geometry proxies for their existing silhouette role and >300k-triangle Quick Look protection; disclose fidelity loss. Eviction regenerates lower tiers on demand; no generic artifact framework or duplicate image copies keyed by irrelevant geometry budget.

**Check:** simultaneous GUI/agent requests generate once; cancellation frees queue capacity; warm/cold pixels and dependency invalidation; all seven media return an honest availability result; no network required for local tiers; embedding artifacts remain unchanged. Review models stratified by FBX/glTF, materials, pack/folder storage and ambiguous bindings.

**Migration/docs/cost:** versioned cache rollout, lazy backfill, old SVG retained until successful replacement. No asset/media-type change. INGESTION, UI-SPEC previews, DESIGN-GUIDELINES and proposed ADR-0019. **1–2 days.**

### S17 — Font specimens, explicitly real missing work

**Change:** add font to preview eligibility and implement a bounded glyph-sheet raster through one permissively licensed font rasterizer (`fontdue` is already named in TECH-STACK but is not currently installed). No font-family installation or OS-wide registration. Define supported TTF/OTF inputs; WOFF/WOFF2 remain explicitly unsupported unless the chosen decoder supports them.

**Check:** valid TTF/OTF, corrupt font, huge/unusual metrics, missing glyph fallback, repeatable specimen text and real rendered output. Offline generation; source untouched.

**Migration/docs/cost:** no DDL; new preview generator version queues font rows. Update TECH-STACK with the actual dependency/license and INGESTION support matrix. **1–2 days.** Font support is required if “each asset” includes fonts at v1; it is not hidden inside cache consolidation.

### S18 — Pack walks: one inflate per bounded batch

**Change:** reuse `extract_at_offsets`' sorted traversal for page/job payload pulls. Group requests by archive revision, sort offsets and stream once, emitting requested payloads into our cache with cancellation and size bounds. A single explicit asset read remains supported. A model evidence batch includes needed material/texture payloads, not just meshes.

Repeated `read_payload_at` currently costs approximately Σ(offset + payload size). For k uniformly spread targets in an archive of uncompressed size U this is about kU/2; a sorted pass costs at most U plus requested writes. It is quadratic in entry count when U grows proportionally with k. Do not invent a gzip random-access index first. Batching cannot remove the cost of repeatedly opening unrelated isolated assets; the retained scratch cache covers revisits.

**Check:** N payloads in one archive require one decoder/traversal; bytes match independent reads; cancellation/truncation/oversized member/path traversal; page and agent batch both call the new seam; warm requests do no inflate; original archive is unchanged.

**Migration/docs/cost:** no DDL; scratch keys use S14 revision identity and S15 eviction. INGESTION and TECH-STACK measurements; ADR-0005 remains unless its explicit-open behavior changes. **1–2 days.** This is before the final 6k acceptance run, even though R5's paging API can land earlier.

### S19 — Reconcile project records and publish the release evidence

Make `docs/KANBAN.md` the sole operational board using the reconciliation below; preserve both old boards as explicitly historical archives and replace `KANBAN-SVELTE.md` with a link. Fix the checker to recognize a standalone historical marker, include partial `[~]` cards and distinguish notes from absence warnings; one tiny CRLF/quoted-marker fixture is enough. Do not solve this by marking the live board historical.

Make root `AGENTS.md` defer to `CLAUDE.md` rather than duplicate it. Preserve the session's explicit instructions, but give repository rules one authoritative home. Add README map links and correct its current egui/Glow/sqlite-vec claims by pointing to ADR-0001 and TECH-STACK. Update ARCHITECTURE's obsolete shared-wgpu/static-binary claims; mark optional sidecar and proposed features as unimplemented. Correct UI-SPEC's virtualization claim to current pagination. Do not present a documentation cleanup as a renderer rewrite.

**Check:** both doc scripts, manual docs-vs-diff gate, the paired R1/R4 traces, migrated catalog comparison, and full local/remote-configured walk acceptance. The requested `docs-conformance` skill was not found in the available skill locations during this review; use CLAUDE rule 6's explicit rubric rather than claim that missing playbook ran. Install/locate it at implementation kickoff if required by the team workflow.

**Migration/docs/cost:** no catalog migration; archive/pointer changes only. Proposed ADR-0015 records board/rule ownership. **1–2 days**, plus elapsed full-library validation.

## Draft ADR decisions for the implementation commits

These numbers reserve a proposed sequence after ADR-0014; verify the next free number when implementing. **Status: proposed** for all six. No existing invariant is relaxed by this review.

| ADR | Decision and owning slice | Supersession/amendment; accepted tradeoff and reversal condition |
|---|---|---|
| **0015 — One board and one rules source** | KANBAN is operational; prior boards become history; AGENTS links to CLAUDE. S19. | Supersedes competing operational-board claims, not their historical record. Costs one reconciliation pass; reverse only if genuinely independent products need separate ownership, not to evade drift checks. |
| **0016 — Coalesced publication and explicit external refresh** | Domain/origin generations, bounded publication, stable current view with pending indication. S01–02. | Amends ADR-0008/0014's notification behavior, retains duty cycle and single writer. The user sees slightly stale background results until refresh; revisit only if observed workflow needs selective live updates that can meet the same frame budget. |
| **0017 — Producer-scoped atomic tag batches and retained undo** | Namespaced AI sources, complete snapshots, atomic data/history, bounded batches, separate interactive/background retention. S04–05. | Amends ADR-0006 and source semantics in DATA-MODEL. Preserves simultaneous models at the cost of more visible provenance and a bounded history policy; reverse retention strategy only on measured size/latency failure, never by losing attribution. |
| **0018 — Prefabs are scoped evidence for v1** | GameObject/component/slot evidence replaces whole-file n×m; prefabs remain evidence, not asset rows. S06–07. | Supersedes DATA-MODEL §10's whole-file pairing. First-class prefabs are a separate parked schema/API/UI decision; reopen when users need prefab-as-a-unit selection/import, not just textured mesh judgment. Nested variant overrides remain unresolved evidence. |
| **0019 — Derived preview identity and fidelity tiers** | Hash-or-versioned-locator identity, dependency-aware previews, colored raster judgment tier over SVG fallback, shared protected eviction. S08/S14–17. | Amends M15 preview primacy and current cache policy; retains ADR-0005 and byte-identical embedding inputs. Costs cache-version churn and initially more rendering for textured full geometry; reconsider simplification only after a measured cost/fidelity failure. |
| **0020 — App-owned bulk jobs with endpoint consent** | Durable scoped push jobs; MCP interactive/corrective controls; per-modality destination-based egress. S10–13. | Clarifies/amends ADR-0011 and ADR-0003/0014 without introducing another writer or silently authorizing cloud idle work. App must stay running for writes. A demonstrated app-closed requirement triggers a separate single-owner daemon ADR. |

The standard patterns used are timed event coalescing, immutable cache snapshots with generation invalidation, transactional command/history writes, keyset pagination, durable work queues and sorted streaming archive reads. No custom event bus, event-sourced catalog, distributed job system, ANN backend or new artifact framework is proposed. Any later invention must name which of these patterns failed and show the measurement or correctness requirement it could not meet.

## One reconciled execution board

This is the **proposed authoritative board content**, reconciled against code on 2026-09-07. Existing board files remain unchanged at the user's request. Historical dates alone never mark a current capability Done. Slice details above are the authoritative acceptance criteria; do not duplicate them into multiple cards.

| Lane | Card | Current state / next action |
|---|---|---|
| Done — review | Architecture/code/board audit | This document; code claims checked, measured and unmeasured quantities separated. |
| Ready | R1 — S01–02 | Shared event storm and idle message loss; stable browsing first. |
| Ready, after R1 | R2 — S03–05 | FTS maintenance, atomic/provenanced batches, safe undo and paired adapters. |
| Ready, after R2 | R3 — S06–08 | Correct evidence/slots/native materials and textured preview flow; embeddings frozen. |
| Ready, after R3 | R4 — S09 | Resident index and cross-process invalidation; no index swap. |
| Ready, after R4 | R5 — S10–13 | Endpoint policy, headless liveness, bounded walk, durable app-owned bulk run. |
| Ready, after R5 | Proxies/packs — S14–18 | NULL-hash-safe identity, protected bounded caches, judgment tiers, font specimens, batched inflate. |
| Ready, after preceding slices | Records/release proof — S19 | Publish sole board and evidence; commit nothing solely because a checkbox was historically ticked. |
| In Progress / Review | None | This is a plan, not a claim implementation is underway. |
| Done — current code | Provider presets, Test, paid-run prompt, cap plumbing, Windows keychain | Restored Svelte callers exist. Retain endpoint/per-modality and non-Windows secret limitations as separate work; do not rebuild the card. |
| Done — current code | Paged library, Quick Look, HTML audio, inspector relink/provenance, staging/import app half, traits/teaching | Keep; these do not imply virtualization, complete Unity-plugin validation or perfect AI quality. |
| Partial | Human review | Reject and session Confirm all exist. Old inspector Confirm/revise UI is not current proof; tag undo/provenance belongs to R2. |
| Partial | Audio identification | CLAP tagger is implemented/wired; effectiveness requires a held-out listening/label check. If it fails, prioritize local audio descriptions; do not invent remote audio upload. |
| Partial | Inspector conformance | Replace stale broad “gaps” card with the actual confidence-underbar residual; provenance styling is R2. |
| Partial | Unity plugin | App register/approval/queue/lease/ack/provenance/dup-guard and folder-export undo exist; real C# plugin and Unity verification remain separate. Relevant to full product release, not required to demonstrate the specified agent-library loop. |
| Backlog, separate | `.meta` labels | Parsed but not persisted; seed imported tags through R2 plus existing-library backfill. |
| Backlog, separate | Non-content filtering | Classifier has no production caller; catalog defaults everything to content. Wire at ingest and backfill before claiming automatic noise exclusion. |
| Backlog, separate | Color search, embedding provenance metadata | `dominant_color` and `embedding_meta` remain DDL-only. Do not describe them as finished; split from variants/smart collections. |
| Backlog, separate | Additional formats/viewer refinements | Normalized export formats, HDR/channel controls, sprites and other old mixed cards require their own format contract/corpus; not hidden work in R3. |
| Deferred by explicit scope | First-class prefabs, smart collections, variant grouping, UnityPy, Unreal/Godot, ANN swap, 4k-decode spike | Park, not false Done. |
| Deferred until demonstrated | Full Engine actor/33 ms pump, CLI skeleton, app-closed daemon, dead-egui cleanup | Current writer/jobs/paging suffice for this plan. Delete legacy helpers only after tracing their live neighbors. |
| Retained historical/unverified backlog | Other old roadmap extras: pack-update diffs, account sync, project audits, auto-collections, packaging/installer matrix, corpus/perf expansion | Keep their original record; no completion or new priority inferred from this targeted review. Recheck code before promoting a card. |

### All 23 checker reports reconciled

The checker currently exits 0 with 516 citations and “23 absence claims.” Actually **18 are missing-citation notes and five are open-card absence warnings**. A quoted historical marker suppresses live-board errors; `[~]` cards are ignored entirely. These are checker defects to fix in S19, not proof of missing product features.

| # | Reported item | Disposition / current evidence |
|---|---|---|
| 1 | request_repaint_after | Retired egui scheduling; native lazy images at `crates/lodestash-desktop/ui/src/lib/Card.svelte:276`. Archive. |
| 2 | fonts.rs | Retired UI font loader; CSS fonts at `crates/lodestash-desktop/ui/src/styles/fonts.css:10`. Archive. This is unrelated to asset font specimens. |
| 3 | Sink::try_seek | Replaced by HTML audio at `crates/lodestash-desktop/ui/src/lib/audio.svelte.ts:31`. Archive. |
| 4 | ScrollArea::show_rows | Virtualization did not survive; ordinary paged grid at `crates/lodestash-desktop/ui/src/lib/LibraryScreen.svelte:582`. Correct spec/current board. |
| 5 | spawn_thumbnails | Old explicit throttle retired; protocol generation at `crates/lodestash-desktop/src/protocol.rs:273`. S16 owns bounded demand. |
| 6 | pending_thumbs | Same throttle; egui-era `lodestash-app/src/grid.rs:214` helper had no callers (deleted in v2 Task 2.2). Do not claim it limits current requests. |
| 7 | extract_whole_pack | Helper retired, extraction survives in `crates/lodestash-ingest/src/unitypackage.rs:422` and desktop writer. Archive name. |
| 8 | in_pack_scratch_pulls_once_and_is_stable | Old test absent; production scratch at `crates/lodestash-engine/src/thumbs.rs:69` remains. Replace proof with S14/S18 checks. |
| 9 | storage_size_rx | Old egui receiver analogy in cloud history. Archive. |
| 10 | api_test_rx | Test restored through `crates/lodestash-desktop/src/lib.rs:1129`, `crates/lodestash-desktop/ui/src/lib/api.ts:314`. Archive name. |
| 11 | receive_api_test | Same restoration as #10. |
| 12 | pending_paid_run | Current UI is `pendingPaidRun`, `crates/lodestash-desktop/ui/src/lib/IdentifyScreen.svelte:444`. Archive old name. |
| 13 | warm_thumbnails | Replaced by protocol demand at `crates/lodestash-desktop/src/protocol.rs:177`. Archive. |
| 14 | Inspector gaps | Relink/provenance exist (`crates/lodestash-engine/src/read_model.rs:2015`, `crates/lodestash-desktop/ui/src/lib/Inspector.svelte:385`); numeric confidence lacks specified underbar. Narrow residual. |
| 15 | variants/color | Accurate DDL-only claim (`crates/lodestash-core/src/catalog/mod.rs:1641`). Split color backlog from deferred variants. |
| 16 | remove_tag | Missing public op is a negative statement inside a done card; internal removal exists. Explicit scoped removal belongs to R2 parity, not a checker false Done. |
| 17 | with_intra_threads | Reverted experiment described historically; current idle duty-cycle fix is ADR-0014. Archive. |
| 18 | meta labels | Parsed `crates/lodestash-ingest/src/unitypackage.rs:41`, populated at 336, not persisted. Remains open with backfill. |
| 19 | smart collections / embedding_meta | Independent DDL-only items (`crates/lodestash-core/src/catalog/mod.rs:1700`, 1713). Split and park appropriately. |
| 20 | catalog.rs in split record | Historical filename; current directory module is `crates/lodestash-core/src/catalog/mod.rs`. Archive. |
| 21 | catalog.rs in writer-test record | Same filename drift; cite current tests when asserting proof. |
| 22 | non-content classification | `crates/lodestash-ingest/src/detect.rs:70` has tests but no production classifier use. Accurate open work. |
| 23 | imported_into_active_project() | Renamed to `import_states` (`crates/lodestash-core/src/catalog/mod.rs:3748`), consumed by desktop library. Archive old name. |

### July 26 survival check

All eight old-board July-26-dated cards were inspected, including their subfeatures; surviving Rust alone was not treated as a shipped UI.

| Historical card | Current disposition |
|---|---|
| Mesh cache/multiview | Geometry cache, proxy path, metrics and multiview embeddings survive. Viewer is WebGL2; texture/cache limitations are R3. |
| App split | App drains/persist/thumbs/grid modules survive; the old egui file/extraction task is retired. |
| Thumbnail throttle | Explicit in-flight throttle did not survive; helper is definition-only. Lazy images/paging are different mechanisms. |
| Accept/reject, traits, audio | Reject and human confirmation core survive; current session Confirm all replaces the old inspector toggle. Traits/teaching/mapping survive. Audio tagger is wired but effectiveness remains open. |
| OpenAI-compatible provider | Provider and current dispatch survive/restored. Native vendor/batch extras remain deferred. |
| Duplicate accept/reject card | Merge with current human-review card; no second implementation task. |
| Reliability controls | Presets/Test/quote/cap/paid confirmation/Windows keychain now have current callers. Do not rebuild from the stale receiver names. |
| Visual zero-shot | 2D survives. 3D tag voting is deliberately disabled (`zeroshot_tags`, `crates/lodestash-engine/src/appraise/tag.rs`); multiview embeddings remain. Texture repair does not automatically justify restoring the bad classifier. |

Dead-egui follow-up: `LibraryFilter`, UnitySort, PendingMint, LargePackPrompt, legacy DirNode/tree helpers, ScanDone, IdentifyScope, HydrateDone, CatalogSynced, PackagePeek, ImportScan, VocabState/VocabTab, Screen/SearchMode and grid layout/throttle helpers appear unreachable outside their legacy cluster. Keep this as a separate deletion card. Category/ContentFilter/TriBucket/LinkFilter/StatusFilter/PersistBatch/PendingPackage have live uses; do not sweep by file name.

## Live-catalog rollout and rollback

1. Capture a consistent online backup, schema version, counts/IDs by storage class, status, tags by source/model, known NULL hashes, representative FTS answers, history and import provenance. The ~6k / 5,963 NULL-hash / 8,155 dirty figures describe supplied reference conditions, not interchangeable counts or a freshly measured snapshot.
2. Apply each new migration to the copy; test both fresh creation and M24 upgrade/reopen. Keep schema transactions bounded: no model inference, recursive source IO, archive inflation or full-library reidentification inside them.
3. Preserve IDs, sources, GUIDs, manual tags, human confirmations, muted links and import records. Rebuild only derived FTS/evidence/cache state where specified. Dirty work resumes in bounded batches and survives interruption.
4. Old previews remain a fallback during lazy backfill. Version keys prevent stale reuse. Missing sources stay missing without destructive cleanup. Embedding inputs/rows remain unchanged by the preview refactor.
5. Before accepting new writes, restore the consistent backup to roll back an incompatible migration. After accepting writes, use a forward repair; restoring the old DB would discard user work. Legacy unknown provenance/unsafe undo cannot be reconstructed and must be disclosed, never fabricated.

## Feasibility, after costing the repairs

R1 costs **2–4 days**; R2 **4–7**; R3 **5–8**; R4 **1–2**: approximately **12–21 focused engineer-days** to stabilize the existing foundations. R5 adds **5–9**; proxy/pack/font slices **5–10**; records and acceptance **1–2**. Overall planning envelope: **23–42 focused engineer-days**, plus actual library/model runtime and corpus/Unity verification where required. These are estimates, not a deadline commitment; no hardware/provider throughput was invented.

**Proceed with this architecture while these conditions hold:** bounded batches fit the single-writer latency budget; the coalesced/pending UI meets the R1 trace gates; resident exact search meets the 100k warm-query/memory budget; representative previews let a reviewer correctly recognize assets; and an interrupted bulk run resumes with accurate provenance and usable undo. All identified foundational repairs fit existing seams. There is no evidence here that a rewrite would reduce the required work.

**Reverse a decision only on these observations:**

- If a ≤100-asset batch still holds the writer beyond ~100 ms in release after FTS repair, reduce batch size and move remaining compute/IO off-thread. If smaller batches would evict interactive history, retain logical run grouping/retention independently. Do not add a writer.
- If UI p95 still exceeds 100 ms after refresh suppression, profile the active page. Fix its measured fetch/render cost; consider virtualization only if bounded DOM work is the limiting cost. Keep the explicitly deferred decode spike separate.
- If warm exact retrieval still exceeds a proposed 100 ms at 100k vectors, or peak snapshots exceed the supported memory budget, first measure filtering, sorting, duplicate snapshots and cache sharing. Revisit the documented index threshold only if the scan itself is now dominant. That is when an index ADR has evidence.
- If textured held-out models remain hard to judge, inspect missing bindings, material variants, camera framing and unsupported formats before changing the classifier. Set a held-out recognition target before implementation (proposed ≥90% correct human identification of supported representative assets); report coverage and unknowns separately from accuracy. Do not hide unsupported cases in an average.
- If a local audio description/spectrogram cannot support useful tags, prioritize local audio-model effectiveness and human audition. Remote audio upload remains outside the invariant.
- If reliable app-closed unattended work becomes a release requirement, revisit ADR-0011 with one shared daemon owning the writer. That is a real architectural cut; a fallback second writer is not acceptable.
- Any cross-source undo loss, forbidden egress, filter leakage or nonresumable migration blocks release regardless of speed. Those failures reverse the candidate implementation, not the local-first/single-writer commitments.

The stated agent-library v1 loop is therefore a bounded repair program, conditional on those checks. Full product v1 still needs the real Unity plugin/retrieval acceptance if that remains part of the release definition; this architecture review does not mark the unverified plugin shipped.

## Verification actually performed in this review

- Current built MCP name-parity test: **1 passed, 0 failed** (`tool_set_matches_the_rest_surface`). This does not prove request/response or behavioral parity.
- Existing release KNN probe: 10k and 50k samples above; it separates load from scan and checks result count, not product caching or tail latency.
- Existing documentation checker before this artifact: **41 docs, 0 dead references, 0 warnings**.
- Existing board checker: **516 cited symbols, exit 0**, with the 18+5 reporting/suppression defects reconciled above.
- Static call-path and owning-doc review; isolated in-memory FTS shape reproduction confirmed the external-content rebuild/read mismatch. No live catalog migration or tag write.

Unperformed: R1 frame/IPC trace on a draining GUI, candidate R1/R4 after benchmarks, full behavioral parity, live M24 upgrade timings, held-out preview/tag quality and a 6k resumable run. These are named gates in the slices, not claims of completed validation. No code edits or commits are part of the final planning deliverable.
