# Architecture review — 2026-11-16

<!-- check-docs: historical -->

> Scope: the whole repository at `bfde2d8` (main), reviewed before the v2 rewrite. Two read-only
> audits (backend crates; desktop shell + Svelte UI) plus checks run in the main session. The
> decisions this review led to are in
> [`../superpowers/specs/2026-11-16-v2-architecture-design.md`](../superpowers/specs/2026-11-16-v2-architecture-design.md).
>
> Tags: **[V]** an auditor read the code that proves it · **[C]** confirmed again in the main
> session · **[I]** inferred (runtime effect or likelihood; nothing here was observed running
> except the test suite and the design prototype).

## 1. Verdict

The domain code is sound; the code around it is not. The catalog, ingest and AI crates hold the
hard-won logic (atomic tag batches, scoped search, resumable extraction, the egress gate) and are
well tested. The defects that users see come from three places:

1. **There is no service layer.** The operations live in three partly overlapping facades — the
   desktop writer and read model, the HTTP `ServerState`, and 107 Tauri commands — each with its
   own shapes, error strings and refresh rules.
2. **The UI has no owner for server state and no component library.** Freshness is a counter
   bumped by hand at 38 sites; every screen re-implements buttons, modals and write helpers.
3. **The design has no single authority and nothing compares pixels.** Two branches of the
   prototype diverged, the token scale cannot express what the prototype draws, and the e2e
   suite asserts DOM text against the user's own catalog.

Rewrite the shell, the UI and the API/MCP boundary. Keep `lodestash-core`, `lodestash-ingest` and
`lodestash-ai` with targeted refactors. Reshape `lodestash-app` into the engine rather than
rewriting its logic.

## 2. Evidence baseline

- `cargo test --workspace`: **459 passed, 0 failed, 18 ignored** in 56 s. [C]
- Fix history: 64 `fix` commits; **37 are scoped `ui`/`desktop`/`shell`, 3 are `core`**. [C]
- Size: core 16.7k lines, ingest 7.6k, ai 3.2k, app 6.5k, api 5.4k, mcp 1.7k; desktop 11.2k Rust
  plus 21.2k lines of Svelte/TS, of which 37% is `<style>`. [C]

## 3. Root causes, ranked by what they cost

### R1 — Three facades instead of one service

- Write facade: the `Op` enum (49 variants) and an 822-line match in
  `crates/lodestash-desktop/src/writer.rs:74-361`, `:1764-2585`, plus the idle scheduler, extract
  queue, job registry and change publisher. [V]
- Read facade: `crates/lodestash-desktop/src/library.rs` — an in-memory snapshot of the whole
  library, reloaded in full after every user write (`crates/lodestash-desktop/src/lib.rs:87-98`). [V]
- Agent facade: `ServerState` in `crates/lodestash-api/src/lib.rs` (2,052 lines) answers the same
  questions with per-request SQL and different shapes. The comment claiming the two "rank
  identically" (`crates/lodestash-api/src/lib.rs:641-644`) is false: the GUI calls unscoped `hybrid_search` with a
  100,000-hit cap and filters in Rust (`crates/lodestash-desktop/src/library.rs:2066-2105`); agents call the scoped variant. [V]
- Surfaces: 107 Tauri commands, 28 REST routes, 27 MCP tools, 11 `AgentWrite` variants, 27
  `AGENT_OPERATIONS`, kept consistent by hand plus a name-parity test. [V][C]
- Three job registries: `crates/lodestash-app/src/jobs.rs:71`, `crates/lodestash-desktop/src/writer.rs:575`, `crates/lodestash-api/src/jobs.rs:137`. [V]
- `lodestash-app` depends on `lodestash-api` for one constant (`crates/lodestash-app/src/config.rs:665`): the
  orchestration crate depends on the HTTP boundary. [V][C]

### R2 — Server state has no owner in the UI

- `view.dataVersion` (`crates/lodestash-desktop/ui/src/lib/nav.svelte.ts:65`) is bumped by hand at 38 sites in 15 files. [V]
- App-level `status` is assigned from eight places and refreshed only on route change or inside
  App's own `write()`; children re-implement `write` in at least eight places, each forgetting a
  different subset of status refresh, toast and error (`App.svelte:78-80`, `:249-269`;
  `TraitsScreen.svelte:76`, `:150`; `LibraryScreen.svelte:120-154`; `Inspector.svelte:254-372`;
  `Toolbar.svelte:61-90`; `TopBar.svelte:113-143`). [V]
- The same data is fetched from several places: `libraryStatus` in 4 files, `packages` in 5,
  `getSettings` in 4. [V]
- Agent and background writes never reload the snapshot, which is why a "Library updated ·
  Refresh" banner exists at all (`App.svelte:102-116`). [V]

### R3 — No component library; tokens stop at colour, type, space and radius

- The only shared primitives are `Icon`, `EmptyState` and `CompareRows`. The modal is
  copy-pasted ten times with different scrims (0.5/0.6), blurs, z-indexes (50/90/100) and four
  shadows. `.ghost` has five definitions. About 222 raw `<button>`s. [V]
- Component styles carry 651 raw `px` literals, 33 `z-index` declarations with 15 distinct
  values, 65 `opacity` declarations with 15 values and 9+ animation durations; no tokens exist for
  motion, layers, shadows, tints or control sizes. [V]
- `crates/lodestash-desktop/ui/public/icons.svg` has drifted from `docs/design/icons/icons.svg` (same 56 ids, different
  bytes, 57 baked hex strokes, no `currentColor`). [V]

### R4 — The design has no single authority, and nothing compares pixels

- The canvas the owner points to holds **two prototypes that branched from the same 2026-07-14
  base**: the root page added the Unity Assets screen, the Unity package import dialog and the
  Add-source chooser (+531 lines); the handoff page (the one checked into the repo) added
  Favorites instead. Neither contains the other. A three-way merge over the base conflicts on one
  line — the nav list — and the union is exactly the eleven rows `FEATURES.md` already specifies. [C]
- `docs/design/README.md` documents a four-generation precedence chain and calls the canvas root
  "stale"; the merge shows it is a sibling, not an ancestor. [C]
- The prototype draws 35 distinct colours, 19 font sizes (10 px used 112 times; the token floor
  is 11 px), and radii of 2/5/7/8/9/12 px that the token scale cannot express. An implementer must
  choose between the tokens and the design on every element. [C]
- The e2e suite drives the user's real app and catalog over CDP (`crates/lodestash-desktop/ui/e2e/harness.mjs:138-151`),
  asserts DOM text and IPC agreement, makes **0 pixel comparisons**, and has 21 checks that return
  early when the library lacks the right data — they pass having asserted nothing. Mutating checks
  write to the real catalog. [V]
- `DESIGN-GUIDELINES.md` §7 and §9 disagree on Ctrl-A scope; the shipped context menu has 8 of
  the 13 specified items and no multi-select. [V]

### R5 — The single-writer rule is a convention, not a type

- Core has one `Catalog` type; 36 writing methods take `&self`. A reader connection fails only at
  runtime. [V]
- The migrating `Catalog::open` is used outside the writer at `crates/lodestash-desktop/src/library.rs:733` (the
  "read-only" GUI model, which runs first and is what actually migrates), `crates/lodestash-desktop/src/writer.rs:1491`
  and `crates/lodestash-app/src/persist.rs:340`. Raw `Connection::open` without a busy timeout at
  `crates/lodestash-desktop/src/writer.rs:2672` and `crates/lodestash-desktop/src/lib.rs:1823`. [V]
- Two processes can both migrate: the version is read outside a DEFERRED transaction and never
  re-read (`crates/lodestash-core/src/catalog/mod.rs:483-535`). An older build silently opens and writes a newer
  schema (`:376-388`). No backup before migrating. No single-instance guard. [V]

### R6 — IPC blocks the window's UI thread

106 of 107 commands are synchronous `fn`s, which Tauri runs on the WebView2 UI thread, as is the
synchronous `lodestash://` scheme handler (`crates/lodestash-desktop/src/lib.rs:2460`). Every writer round-trip, the full
snapshot reload, the `add_source` crawl and on-demand thumbnail generation freeze the window and
serialise all IPC. The code comments believe the opposite (`crates/lodestash-desktop/src/writer.rs:6-10`). [V from vendored
Tauri/wry sources; I for the user-visible freeze]

### R7 — Errors are strings, and silence is the default

- 358 `CoreError::Catalog(format!…)` sites; app and desktop return `Result<_, String>`; conflict
  kinds are string prefixes, so an HTTP layer cannot map 404/409/422 without parsing text. [V]
- A corrupt `config.json` loads as defaults and the next save persists them
  (`crates/lodestash-app/src/config.rs:699`, `:737`, write error ignored). [V]
- `library.rs` has 22 warn-and-return-empty sites; the UI cannot tell "none" from "failed". [V]
- 75 `String(e)` surfaces and 35 swallowed catches in the UI; three different error surfaces. [V]
- Startup failures (open, migration, port bind) are logged and the app starts empty; no failure
  reaches the user (`crates/lodestash-desktop/src/library.rs:749-752`, `crates/lodestash-desktop/src/lib.rs:2422-2425`). [V]

### R8 — "Done" flags conflate processed with failed

A missing file becomes a zero vector with `embedding_dirty = 0`; an unreadable preview becomes an
empty tag set that **deletes** the producer's existing tags; an unreadable material deletes its
dependency edges (`crates/lodestash-app/src/drains.rs:1887-1892`, `:1465-1485`, `:706-727`). Unplug a drive during
a drain and the work is recorded as finished. Results computed off-thread clear flags raised after
they were read (`crates/lodestash-core/src/catalog/mod.rs:5932-5935`). [V]

### R9 — The MCP server is coupled to the catalog

`lodestash-desktop --mcp` opens the catalog read-only for reads and forwards writes over REST
(ADR-0011 and its amendment). It works, but two programs depend on one schema, the forwarding
layer needs its own handshake, version and catalog-id checks, and the agent surface exists twice
(in-process and forwarded). [V]

## 4. Bugs found

Ship-blocking:

| ID | Bug | Evidence |
|---|---|---|
| S1 | A fresh install cannot create a catalog: `Library::open` returns empty when the file is missing and the writer is spawned only if it exists, so every write answers "no catalog is open". | `crates/lodestash-desktop/src/library.rs:730-733`, `crates/lodestash-desktop/src/lib.rs:2393-2395` [C] |
| S2 | The two egress consent toggles (`remote_image_bytes`, `mcp_bytes_enabled`) are bound in the UI but absent from the desktop `Settings` view-model; they never persist or apply. Fails closed. | `api.ts:298-300`, `crates/lodestash-desktop/src/library.rs:381` (no field, no `flatten`) [C] |
| S3 | Remove from Library trashes the rows and then errors (`last_project_name` never set); the error path skips the reload, so the asset still shows. | `crates/lodestash-desktop/src/writer.rs:2897-2917`, `crates/lodestash-desktop/src/lib.rs:92` [V] |

Data integrity (backend):

| ID | Bug | Evidence |
|---|---|---|
| B1 | A moved file is marked missing instead of re-pointed; it vanishes from the grid until an unrelated change in that source. | `crates/lodestash-app/src/persist.rs:360-375`, `crates/lodestash-core/src/catalog/mod.rs:1265-1273` [V] |
| B2 | Moving a folder with byte-identical files points every vanished row at the same new path; the other file gets a fresh identity without its tags. | `crates/lodestash-core/src/catalog/mod.rs:1303-1363` [V] |
| B3 | The preview queue never drains in-pack models and re-inflates archives every fourth idle second on a fresh install. | `crates/lodestash-core/src/catalog/mod.rs:5694-5711`, `crates/lodestash-app/src/drains.rs:1123-1131` [V; cost I] |
| B4 | With auto-identify on, the idle lane never runs metrics, previews, dependency scans or audio passes. | `crates/lodestash-desktop/src/writer.rs:1341-1352`, `crates/lodestash-app/src/drains.rs:1262-1287` [V] |
| B5 | The generic tag pass runs before the audio pass, so audio rows lose their CLAP tags. | `crates/lodestash-desktop/src/writer.rs:1632-1679` [V] |
| B6/B7 | Unreachable sources and in-flight changes are recorded as finished work (R8). | see R8 [V] |
| B8 | An IPv6-literal or underscore host makes every tag write fail after the paid API call. | `crates/lodestash-ai/src/tagger.rs:160-165`, `crates/lodestash-core/src/catalog/tagbatch.rs:139-159` [V] |
| B9 | Zero vectors outrank real but negatively-scoring matches in text search. | `crates/lodestash-core/src/vector.rs:11-71` [V] |
| B11 | Archives whose entries start with `./` read as empty packs. | `crates/lodestash-ingest/src/unitypackage.rs:281-286` [V] |
| B12 | Config loss on a truncated file (R7). | `crates/lodestash-app/src/config.rs:699`, `:737` [V] |
| B13 | `register_project` is four statements without a transaction; undo-trash restores ids that were already trashed. | `crates/lodestash-core/src/catalog/mod.rs:3546-3607`, `crates/lodestash-core/src/catalog/oplog.rs:225-245` [V] |
| B14 | The ONNX Runtime DLL and models download without a checksum from mutable URLs, then load in-process. | `crates/lodestash-app/src/models.rs:16-178` [V] |
| B15 | The delete fence is lexical (`starts_with`, no `..` normalisation). | `crates/lodestash-app/src/thumbs.rs:31-37` [V] |
| P1 | Crawl persistence is quadratic: the `record_crawl` lookup has no index led by `rel_path` (2,000 lookups over a 20k-asset replica took 4.5 s). | `crates/lodestash-core/src/catalog/mod.rs:807-811` [V on a replica schema] |

UI and shell:

| ID | Bug | Evidence |
|---|---|---|
| U1 | "Confirm before moving to Trash" has no effect until restart. | `App.svelte:136-141`, `SettingsScreen.svelte:160-174` [V] |
| U2 | Sidebar counts, health meter and status bar go stale after undo/redo, bulk tag, pack import and add-source. | `Toolbar.svelte:61-90` et al. [V] |
| U3 | Inspector tag chips go stale after the Inspector's own writes. | `Inspector.svelte:166-181` vs `:254-267` [V] |
| U4 | Undo targets the newest op in the log, not the user's op; an interleaved agent write gets undone. | `App.svelte:285-298`, `Toolbar.svelte:61-75` [V; I runtime] |
| U5 | Delete and F act on the selection while a modal is open; arrows move the grid behind dialogs. | `App.svelte:412-479` [V] |
| U6 | The grid has roughly 500 tab stops per page (4–5 inner `tabindex="0"` spans per card). | `Card.svelte:302-466` [V] |
| U7 | No stale-response guard on the grid fetch; FIFO IPC masks it today. | `LibraryScreen.svelte:361-366` [V; I runtime] |
| U8 | `get_settings` rewrites the config file on read and sends the cloud API key to the webview. | `crates/lodestash-desktop/src/lib.rs:1069`, `crates/lodestash-desktop/src/library.rs:421` [V] |
| U9 | A panic on the writer thread ends all writes for the session (no `catch_unwind`). | `crates/lodestash-desktop/src/writer.rs:947-1392` [V] |
| U10 | No single-instance guard; a second instance overwrites `engine.json`. | `crates/lodestash-desktop/src/lib.rs:2363-2365` [V] |
| U11 | `/file` without a `Range` header on a file over 8 MiB returns a truncated 206. | `crates/lodestash-desktop/src/protocol.rs:628-641` [V] |

## 5. What to keep

| Crate / module | Verdict | Why |
|---|---|---|
| `lodestash-core` | Keep; refactor | Atomic tag batches with conflict-checked undo (`crates/lodestash-core/src/catalog/tagbatch.rs:494-819`), scope applied before LIMIT (`crates/lodestash-core/src/catalog/browse.rs:234-290`), a genuinely read-only reader (`crates/lodestash-core/src/catalog/mod.rs:398-435`), ~155 behaviour-named tests. Refactor: reader/writer types, typed errors, migration hardening, two indexes, split the 6,325-line `mod.rs`, delete dead API. |
| `lodestash-ingest` | Keep | Pure library; atomic cache writes, cancel-safe content-addressed extraction, bounds-checked mesh decoder. Fix B11 and cap manifest reads. |
| `lodestash-ai` | Keep; light refactor | Clean `Tagger`/`Embedder` seams; no-redirect cloud client; parsed locality. Make `Embedder` fallible; delete `worker.rs`. |
| `lodestash-app` | Reshape into the engine | Keep the read/compute/store split of the drains, the egress gate, resumable downloads, crawl classification. Replace the 8–10-parameter free functions and the overloaded `only: Option<&[i64]>` with an explicit appraisal plan. Atomic config. |
| desktop `writer.rs` actor core, `Changes` publisher, extract pipeline, delete fences, `storage.rs` | Move into the engine | Sound shape; needs supervision and per-area handler modules. |
| desktop `library.rs` semantics and its 40 tests | Move into the engine | They are the read contract to preserve. |
| desktop `agent_bridge.rs` + `tests/mcp_stdio.rs` | Seed of the standalone MCP client | Already the thin-client model. |
| UI: `toasts`, `audio` stores, `routes.ts`, query semantics in `nav.svelte.ts`, `viewer3d.ts`/`lsm1.ts`, the e2e harness mechanics | Carry over | Small, single-owner, or self-contained. |
| Everything else in the Svelte UI and the Tauri command layer | Rewrite | R2, R3, R6. |

## 6. Not covered

No runtime observation of the app; the UI bugs are code-reading claims. `mesh.rs`, `decimate.rs`,
`render3d.rs` and the Ollama parsers were skimmed, not judged. The auditors' notes on coverage
are in the spec's appendix references; the measurement scripts were scratch and are not kept.
