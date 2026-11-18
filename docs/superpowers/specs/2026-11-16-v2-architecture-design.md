# Lodestash v2 — architecture and rewrite design

- **Status:** accepted for implementation on branch `rewrite/v2` (2026-11-16)
- **Owner decisions (2026-11-16):** the MCP server is a thin client of the app's API; backend crates
  are kept or rewritten per the review; the UI stays Tauri 2 + Svelte 5 and is rebuilt from the
  design; work lands as local commits on `rewrite/v2`, nothing pushed.
- **Why:** [`../../reviews/2026-11-16-architecture-review.md`](../../reviews/2026-11-16-architecture-review.md)
- **Decisions recorded:** [ADR-0021](../../decisions/ADR-0021-engine-owns-the-catalog-ui-is-a-web-client.md),
  [ADR-0022](../../decisions/ADR-0022-mcp-is-a-standalone-thin-client.md),
  [ADR-0023](../../decisions/ADR-0023-one-visual-authority-and-a-contract-gate.md)
- **Implementation plan:** [`../plans/2026-11-16-v2-rewrite.md`](../plans/2026-11-16-v2-rewrite.md)

This document governs the v2 structure. Where it contradicts `ARCHITECTURE.md`, `ENGINE-API.md`
§1, `MCP-AND-ENGINES.md` §2 or `docs/design/README.md`, this document wins until those are
rewritten in the slice that implements the change (each carries a banner pointing here).
Product behaviour — what a screen does, what an operation means — is unchanged and stays owned by
`UI-SPEC.md`, `FEATURES.md`, `DATA-MODEL.md` and `ENGINE-API.md` §2–§8.

## 1. What done looks like

1. **Two deliverables.** `lodestash-desktop` (the app) and `lodestash-mcp` (the MCP server). The
   MCP binary links no catalog, SQLite or engine code — a build test fails if it does.
2. **One owner of the data.** Only the engine opens the catalog for writing; readers cannot write
   by type. Existing catalogs open and migrate with a backup taken first.
3. **One operation catalog.** Every operation is declared once. HTTP routes, MCP tools and the
   UI's TypeScript client are derived from that declaration. The agent-facing subset is 1:1 by
   construction, not by a parity test.
4. **The UI matches the design by measurement.** Every shipped screen passes the design-contract
   gate (§9) against the reconciled prototype at 1440×900, DPR 1, on the fixture catalog.
5. **No silent failure.** Startup, migration, config and job failures reach the user with a typed
   error; nothing falls back to "empty" or "defaults" without saying so.
6. **The review's ship-blocking and data-integrity bugs are fixed** with a regression test each
   (mapping in §12).
7. **Green gates:** `cargo test --workspace`, `cargo clippy --workspace --tests -D warnings`,
   `svelte-check`, the UI unit tests, the browser e2e + contract gate on the fixture catalog, and
   `scripts/check-docs.mjs` / `check-board.mjs`.

## 2. Process topology

```
                 ┌──────────────────────────── lodestash-desktop (one process) ──────────────────┐
  Tauri window ──┤ shell: single instance, window, --background, --serve                         │
  (WebView2)     │   └─ starts ─▶ lodestash-api (axum, 127.0.0.1:<port>)                          │
       │ HTTP    │                  ├─ GET /            built web UI (embedded)                  │
       └────────▶│                  ├─ /v1/*           operations (JSON) — UI cookie or bearer  │
                 │                  ├─ /v1/events      change feed (SSE)                         │
                 │                  └─ /v1/assets/{id}/{thumb,mesh,file,…}  bytes, Range-capable │
                 │                          │ calls                                              │
                 │                          ▼                                                    │
                 │                  lodestash-engine — the only owner of the catalog             │
                 │                    writer thread · reader pool · jobs · change bus · config   │
                 │                    uses lodestash-core / -ingest / -ai                         │
                 └─────────────────────────────────────────────────────────────────────────────────┘
                          ▲ HTTP + bearer (engine.json)          ▲ HTTP + bearer (engine.json)
  MCP host ── stdio ──▶ lodestash-mcp (separate binary)    Unity plugin (UPM, later)
  (Codex, Claude)       protocol + HTTP client only;
                        launches the app with --background if none is running
```

- The engine and the HTTP server run inside the desktop process. There is no separate daemon.
- `--serve` runs engine + server without a window (headless; dev, e2e, CI) and prints a one-time
  UI URL. `--background` is `--serve` started by the MCP binary; it exits after 30 minutes with no
  request and no window. A normal launch while a background instance runs focuses or opens its
  window instead (single-instance).

## 3. Crates

| Crate | Role | May depend on |
|---|---|---|
| `lodestash-core` | Schema, migrations, catalog storage, pure search. `CatalogWriter` + `CatalogReader`. Typed errors. | rusqlite, serde, blake3, uuid |
| `lodestash-ingest` | Unchanged role: paths in, data out. | core (types only) |
| `lodestash-ai` | Unchanged role; `Embedder` becomes fallible. | core (types only) |
| `lodestash-protocol` | **New.** Wire DTOs, the operation catalog, error envelope, handshake, schemas. No I/O. | serde, schemars |
| `lodestash-engine` | **New.** The service: absorbs `lodestash-app` and the desktop writer, read model, storage and media generation, and the domain half of `lodestash-api::ServerState` (import queue, approvals, export, discovery). | core, ingest, ai, protocol |
| `lodestash-api` | HTTP adapter only: routes derived from the catalog, auth, SSE, static UI, bytes. | engine, protocol |
| `lodestash-mcp` | **Binary.** MCP server over HTTP. | protocol, rmcp, HTTP client |
| `lodestash-desktop` | Thin Tauri shell. | api, engine (start/stop only) |

`lodestash-app` is deleted once absorbed. The web UI moves from `crates/lodestash-desktop/ui/`
to top-level `web/`: it is a client of `/v1`, not a Tauri frontend.

## 4. The engine

### 4.1 Ownership and threads

- **Writer:** one thread owns the only `CatalogWriter`. Requests arrive as typed commands on a
  bounded channel and answer on a one-shot reply with a timeout. The loop runs each command under
  `catch_unwind`; a panic fails that command with `internal` and the thread keeps serving.
  Commands are grouped by area (`writer/{assets,tags,vocab,collections,sources,packages,identify,
  projects,history}.rs`), not one match.
  - *A caller that stops waiting is told the truth, and it stays true.* Each command carries a
    claim both sides race for: the writer **starts** it before running it, a caller whose wait ran
    out **withdraws** it. Withdrawn (it was still queued, or the inbox was full) answers `busy` —
    *not run, nothing written, try again* — and the writer skips it when it reaches it. Already
    started answers `busy` as *still running* — it will finish and commit on its own, so it must
    not be resent. Both are the `busy` kind (§4.4) and the message, which the code leads, says
    which. "Busy, try again" used to be said of a queued Delete from disk that then ran.
  - *Supervision cleans up after itself.* A tray job is opened through a guard that ends it as
    failed if the code that opened it unwinds; a pack worker reports back from `Drop` if it dies,
    so the one-at-a-time extract queue moves; an off-writer slice lowers its lane's in-flight flag
    however it ends.
  - *The generation moves whenever rows moved* — also when the command then failed or panicked
    (ADR-0016, amended): the mark means "the catalog changed", never "the command succeeded".
- **Readers:** a small pool of `CatalogReader`s (`SQLITE_OPEN_READ_ONLY`). `CatalogReader` exposes
  only read methods; writing through it does not compile.
- **Work lanes off the writer:** crawl/hash/rescan, extraction, embedding, tagging (local and
  cloud), previews, metrics, dependency scans. A lane computes off-thread and submits results to the
  writer as one command; the writer applies a result only if the asset's content stamp
  (hash + mtime) still matches what the lane read (fixes B7).
- **One job registry** (replaces three): id, kind, origin, progress, state
  (`queued|running|awaiting_approval|done|failed|cancelled`), error text on failure.
- **Change bus:** the existing mark-then-publish design (ADR-0016) moves here unchanged in
  behaviour: catalog and job generations, origins, at most one publication per 250 ms. It feeds
  `/v1/events` and the read model.

### 4.2 Catalog lifecycle

- **Create if missing** (fixes S1). `catalog_db_path()` resolves; the engine creates the file.
- **Exclusive lock:** the engine holds `std::fs::File::try_lock` on `<catalog>.lock` for its
  lifetime. A second engine on the same catalog fails fast with `catalog_locked`. The lock is
  taken before the config is opened, and an engine that does not get it reads the config but
  cannot write it (§4.5, ADR-0024).
- **Migration:** `BEGIN IMMEDIATE`, re-read `user_version` inside the transaction, then migrate.
  Before the first migration step on an existing file, copy it to
  `<catalog>.v<from>.bak` (kept: the last two written — the new one is never the one pruned, and
  a failed prune never fails the open; `DATA-MODEL.md` §13). A schema newer than this build is
  refused for writing with `schema_too_new` (the reader already refuses).
- **Engine status** is a value the UI and agents can read (`GET /v1/status`):
  `starting | migrating{from,to} | ready | failed{code,message}`. Every failure here renders as a
  blocking screen in the UI with the message and what to do — never an empty library.
- *As built (2026-11-17):* until `GET /v1/status` exists the old shell reads `Engine::report()`
  through one Tauri command, `engine_status` (it also records the dismissal of
  `config_recovered`), and `App.svelte` draws a blocking notice with the code and message while
  the state is not `ready`, and a dismissible line for a recovered config. **A read from an engine
  that is not `ready` is an error** (`Engine::read` → `ReadError::NotReady(status)`), not an empty
  library — so every shell read, and the `lodestash://` byte routes (`503`), say why instead of
  drawing nothing.

### 4.3 Read model

The desktop `library.rs` snapshot model moves into the engine unchanged in semantics (its tests
move with it) and becomes a **generation-checked cache**: each read compares the catalog
generation it was built at with the current one and rebuilds lazily if they differ. Agent and
background writes therefore become visible to the UI without a "Refresh" banner (fixes R2's
backend half). Every read response carries the generation it reflects
(`X-Lodestash-Gen` header).

**A rebuild is all or nothing, and a failed one is never served.** The snapshot is built from a
dozen queries; `Library::reload` returns an error if any of them fails and leaves the previous
snapshot untouched, and `Engine::read` then answers `ReadError::Rebuild` **without stamping** the
generation — so the next read tries again, where a failed query used to be logged, skipped and the
half-built result served as current. A panic while the read lock is held costs one rebuild: the
poison is cleared, where it used to make every later read rebuild the whole library and
`Engine::reload` a no-op. Counts read through the model's reader for a decision (the tagging queue
behind the cloud cost estimate, the workspace paths the reclaim must keep) are `Result`s for the
same reason: a failed count is not zero.

A SQL-backed paged/faceted listing replaces the snapshot later (§13); the snapshot's tests are
its contract.

### 4.4 Errors

`EngineError { kind, message, detail }` with `kind ∈ {not_found, invalid, conflict, denied,
unavailable, busy, too_large, internal}`. Core grows typed kinds (`CoreError::Conflict`,
`::RevisionConflict`, `::Invalid`, `::NotFound`, `::SchemaTooNew`/`::SchemaTooOld`, `::Db`) and
`CoreError::kind()` (`not_found | invalid | conflict | unavailable | internal`), so mapping never
parses strings.
HTTP mapping: 404/422/409/403/503/429/413/500, body `{error:{code,message,detail}}`
(`ENGINE-API.md` §4).

### 4.5 Config

- One `ConfigStore` in the engine; all reads go through it (no direct `AppConfig::load()`).
- Writes are atomic (write temp, fsync, rename) under one lock.
- A config file that fails to parse is moved to `config.json.corrupt-<unix-ts>`, the engine
  continues on defaults **and reports it** in engine status (`config_recovered`) until the user
  dismisses it (fixes B12).
- Settings are read and written by section. Secrets (cloud API key) are write-only over the API:
  reads return `{configured: bool}` (fixes U8). Reading settings never writes the file.
- Settings changes apply live to the engine (allow-list, byte release, egress consent) — fixes S2
  by making the consent fields part of the typed `Settings` DTO in `lodestash-protocol`.
- *As built (Task 2.4):* `crates/lodestash-engine/src/config_store.rs`. The store loads the file
  once and serves reads from memory (an edit made by hand while the app runs is not seen, and the
  next save replaces it); `update` saves temp → `sync_all` → rename under the store's one mutex and
  leaves memory unchanged when the save fails. A file that was read and does not parse is
  renamed to `config.json.corrupt-<unix-ts>`; `Engine::report()` carries `config_recovered` (that
  path) beside the status until `Engine::dismiss_config_recovered`. `Settings` is sectioned in Rust
  (`appearance`, `library`, `identify`, `cloud`, `agents`, `storage`, each applied on its own) and
  flat on the wire — the sections are `#[serde(flatten)]`, so the object is the one the old
  shell's `get_settings`/`set_settings` already exchange. The secret is write-only: a read carries
  `api_key_configured: bool` and no `api_key`. Live application: `ConfigStore::on_change`
  observers; the agent planes subscribe with `ServerState::follow_config` (allow-list, byte
  release, agent producer), and every appraise pass reads the consent from the store when it starts.
- *As built (boundary review, 2026-11-17;
  [ADR-0024](../../decisions/ADR-0024-the-boundary-fails-closed.md)):*
  - **What a write does not say changes nothing.** `api_key` absent, `null`, empty or blank keeps
    the stored key, a value replaces it, and only `clear_api_key: true` removes it (an empty key
    used to clear it, so a field typed into and erased wiped the key on the next save).
    `api_max_per_run` and `api_base_url` are optional on a write and absent keeps the stored value
    — the DTO has no default of its own (an omitted cap used to read as `0`, uncapped). Every
    other optional field is "off" or "none" when absent; `lodestash-protocol`'s
    `tests/dto_wire.rs` lists them and fails when one is added.
  - **A file that was not read is never written over.** Only a file that was read and fails to
    parse is corrupt. One that cannot be read (held open by another program, access denied), or
    that is corrupt and cannot be moved aside, leaves the store *blocked*: it holds a fallback
    that switches nothing on (every agent tool off, no background identification, no rescans),
    `Engine::report()` carries `config_unreadable` (why), and every update is refused until the
    file loads. The read is retried briefly at open and again at each update; the update that
    finds the file readable takes its config, tells the observers and is itself refused
    (`ConfigError::Reloaded`) — it was decided against the fallback.
  - **Only the catalog's owner writes the config.** `Engine::start` takes the catalog lock before
    it opens the config; an engine that is refused the lock gets `ConfigStore::open_read_only`,
    which reads the file and answers every update `ConfigError::ReadOnly`. A second instance used
    to save its start-time copy over the first instance's changes.
  - **Observers hear commits in commit order.** One thread tells them at a time; a commit that
    lands meanwhile is delivered next, and an older config is never delivered after a newer one
    (it used to be possible, leaving a just-revoked consent live in the agent planes).
  - **A save and its key change succeed or fail together.** The credential store is changed
    before the file is written, and put back when the write fails. On Linux and macOS, where the
    key rests in the file, the file is created `0600`.

### 4.6 Identification plan

`AppraisePlan { scope: All | Assets(ids) | Source(id) | Package(id), stages: Set<Stage>, budget,
origin }` replaces the overloaded `only: Option<&[i64]>`. Stage order is fixed in one function:
metrics → dependencies → previews → audio embed → audio tags → image/3D embed → tags → promote.
Audio rows are excluded from the generic tag stage (fixes B5). The idle lane builds a plan with
all backfill stages (fixes B4).

## 5. The operation catalog (`lodestash-protocol`)

Every operation is declared once with a macro:

```rust
operations! {
    /// Search the asset catalog by text; returns asset summaries.
    search_assets: GET "/v1/search" (SearchArgs) -> Vec<AssetSummary>, audience = Agent, effect = Read;
    /// Apply one attributed tag batch (approval-gated for agents).
    write_tags: POST "/v1/tags/batch" (TagBatchRequest) -> TagBatchKickoff, audience = Agent, effect = Gated;
    set_favorite: POST "/v1/assets/{id}/favorite" (SetFavorite) -> WriteResult, audience = Ui, effect = Write;
    …
}
```

The macro generates:

- `OPERATIONS: &[OpSpec]` — name, method, path template, audience, effect, summary, and the
  input/output JSON Schemas (schemars).
- `trait Operations` — one method per operation, `fn op(&self, ctx: &Caller, input: In) ->
  Result<Out, ApiError>`. The engine implements it; a missing operation does not compile.
- The HTTP router (in `lodestash-api`): one route per spec. Input is assembled generically from
  path parameters + query string (GET/DELETE) or path parameters + JSON body (POST/PUT), then
  deserialised into the input type.
- A schema export (`protocol.schema.json`) from which the web client's types are generated
  (`web/src/api/generated.ts`); CI fails if the generated file is stale.

**Audiences.** `Agent` operations are MCP tools and HTTP routes (the 1:1 invariant, now
structural). `Ui` operations are HTTP only and require the UI session. The UI may call `Agent`
operations too. Long-running operations return a job id; `get_job_status`/`cancel_job` are agent
operations.

**Paths stay as `ENGINE-API.md` §2 defines them** — no `/v1` break. New UI-only operations get
REST paths under the same conventions.

## 6. HTTP server (`lodestash-api`)

- **Bind:** `127.0.0.1`, ephemeral port; write `engine.json` (existing fields plus `exe`, the
  path to the running binary, used by the MCP launcher). Remove it on clean exit — *as built
  (2026-11-17):* the desktop calls `lodestash_api::remove_handshake` on `RunEvent::Exit`, and it
  removes the file only while it still names this process's pid (a newer instance's file stays).
- **Caller identity:**
  - *Agent*: `Authorization: Bearer <engine.json token>`. Agent calls go through the per-tool
    allow-list and the approval gates exactly as today (`ENGINE-API.md` §8.4). *As built
    (2026-11-17, ADR-0024):* the gate finds a request's tool in the catalog
    (`routes::operation_for`) rather than in a list of its own, and where two operations share a
    route (`GET /v1/search`) the handler asks the same function which one a request is — the
    decoded, last `mode` — so the gate and the handler cannot read one request two ways.
  - *UI*: the shell opens the window at `/session?code=<one-time, 60 s>`; the server exchanges it
    for an `HttpOnly; SameSite=Strict; Path=/` cookie and redirects to `/`. A UI caller is the
    human; agent approval gates do not apply to it, the destructive-action confirmations in the UI
    do.
- **Request checks:** `Host` must be `127.0.0.1:<port>` or `localhost:<port>` (DNS-rebinding
  defence). Any request carrying an `Origin` header must be same-origin; requests with a cookie
  and no matching `Origin` on a non-GET method are refused (CSRF).
- **Static UI:** the built `web/dist` is embedded in release builds and read from disk in debug.
  The server sets the CSP; no inline script.
- **Bytes:** thumbnails, previews, meshes, slot textures and files are `/v1/assets/{id}/…` routes,
  async, streaming, single-range `Range` support, `ETag` from the content stamp. Agent callers
  pass the existing egress / byte-release gate; the UI caller does not (fixes U11; replaces the
  `lodestash://` scheme).
- **Events:** `GET /v1/events` (SSE): `changed {catalog_gen, jobs_gen, config_gen, origins[]}`
  and `status {engine status}`. Heartbeat every 15 s.
- **System operations** (UI audience): pick folder/file, reveal in Explorer, open external URL.
  They run in-process via `rfd`/`opener`. With these, the UI needs no Tauri IPC at all.

## 7. MCP server (`lodestash-mcp`)

- A standalone binary depending on `lodestash-protocol`, `rmcp`, an HTTP client and serde only.
- **Tools:** listed from `OPERATIONS` filtered to `Agent`; each tool's input schema is the spec's
  schema. A call fills the path template from the input, sends the rest as query or body, and
  relays the app's body and status as the tool result (`isError` on non-2xx). One generic
  implementation serves every tool.
- **Finding the app:** read `engine.json` on every call (an app restarted onto a new port is
  picked up). A handshake is an address only while the process it names (`pid`) is running —
  checked before the version and before anything is sent, because the request carries the token
  ([ADR-0024](../../decisions/ADR-0024-the-boundary-fails-closed.md)). If the file is missing,
  names a dead process, or its port refuses, launch the app with `--background` — binary from
  `LODESTASH_APP`, else a sibling `lodestash-desktop` next to the MCP binary, else `engine.json`'s
  last `exe` (only as an absolute local path to a file named `lodestash-desktop`) — and wait up to
  20 s for a handshake whose `api_version` is `v1`. Otherwise answer `app_unavailable`, or the
  retryable `app_starting` while an app this server started is still coming up: one launched app at
  a time.
- **Never retried.** A request that timed out after it was sent may still land (ADR-0011's rule
  stands).
- **Transport:** stdio first. Streamable HTTP on `127.0.0.1:4319` (the design's Settings → MCP card:
  endpoint, access token, allowed actions) is phase 3: the app's "Enable MCP server" toggle starts
  and stops `lodestash-mcp --http 4319` as a child process.
- The Unity plugin keeps talking to the app's `/v1` directly (`ENGINE-API.md` §5); it is not an
  MCP client.
- *As built (P4, 2026-11-16; `crates/lodestash-mcp/README.md`):*
  - **The app's REST answer is the tool's answer**: REST's containers (`{results}`, `{tags}`, …)
    and default limits; an `image/*` body is an image block; a non-2xx body is `isError` with the
    envelope. Input the operation's type refuses is `isError`/`invalid` and is not sent; an unknown
    tool name is a JSON-RPC error. `get_server_info` adds `mcp: {version, transport, handshake,
    port}`.
  - `LODESTASH_ENGINE_JSON` overrides the handshake path, and a launched app is given it (today's
    desktop ignores it). `LODESTASH_APP`, when set, is the only candidate — it never falls through
    to another binary. Ready means a `v1` handshake whose port accepts a connection; an app that
    exits with a failure before publishing is reported at once instead of after 20 s.
  - The launch is detached: no stdio, its own process group, and outside the host's job object
    where Windows allows breakaway. On Windows the server clears the inherit flag on its own stdio
    handles first, or the background app would hold the host's pipes open after the server exits.
  - The `asset://{id}/file` resource stays, read through the `asset_file` operation's route (the
    resource summaries advertise; a resource is not a tool, so `Engine` audience is no barrier).
- *As built (boundary review, 2026-11-17; ADR-0024):*
  - **Liveness** is `OpenProcess` + `GetExitCodeProcess` on Windows and `kill(pid, 0)` on Linux and
    macOS — no new dependency. A pid this user cannot query is not the app and reads as dead. The
    same test applies to the handshake a launched app publishes, so a dead app's file is never
    mistaken for the new app's. A **live** app of another API version that answers is refused at
    once (launching would not help); a dead one's file is just stale.
  - **One launch.** The started process is remembered while it runs. A call that arrives meanwhile
    waits for that process's handshake for its own 20 s and then answers `app_starting`
    (`detail.sent: false`); a start that exits with a failure is reported to every call waiting on
    it. The app starts in its own directory, not the host's.
  - **Leaving.** The server exits within about a second of the host closing stdin, even with a
    call still waiting on the app (it used to wait out the call — up to 120 s).
  - **What a model reads** names files with the home directory shown as `~`: the handshake path in
    error `detail` and in `get_server_info`'s `mcp.handshake`, and binary paths in messages.

## 8. Desktop shell (`lodestash-desktop`)

- Tauri 2 with `tauri-plugin-single-instance` and `tauri-plugin-window-state`. No commands, no
  custom scheme, no IPC capabilities beyond the defaults.
- Startup: tracing → ORT path → parse flags → take single-instance → start engine (status
  `starting`) → bind server → open the window at the session URL. The window shows the engine
  status screen until `ready`, so a slow migration or a failure is visible, not a white window.
- Window minimum size equals the design floor (900×600, ADR-0009).
- The debug build starts Vite and points the window at it (Vite proxies `/v1` and `/session` to
  the engine), as `devserver.rs` does today.

## 9. The web UI (`web/`)

### 9.1 Layout

```
web/src/
  app/       Shell.svelte (CSS grid only), router, keymap (scope stack), overlay host
  api/       client.ts (fetch, error envelope, AbortSignal), generated.ts, events.ts, media.ts
  data/      query.svelte.ts (resource cache), resources/*, mutations.ts
  state/     view (route, query, filter, sort, density), selection, player, toasts, prefs
  ui/        primitives — the only place with visual CSS
  features/  asset, inspector, filters, staging, jobs, tags, settings/*, traits, mapping,
             onboarding, import, viewer3d
  screens/   thin compositions, one per nav destination
  styles/    tokens.css (imported from docs/design), reset, layers, breakpoints
```

### 9.2 Rules (enforced by lint or test, not by review)

1. **Server data lives in the resource cache only.** A resource is a key → `{data, error, status,
   gen}`; requests are deduplicated and stale responses discarded (AbortSignal + generation).
   Components never fetch, never keep a private copy, never bump a counter.
2. **Invalidation is driven by the change feed.** An SSE `changed` event invalidates the
   resources whose tag matches (catalog / jobs / config); active resources refetch. Mutations
   return the op id they created; undo targets that id, never the log head (fixes U4).
3. **Primitives own all visual CSS.** Screens and features carry grid placement only. Stylelint
   rejects colour, length, shadow, z-index, duration and opacity literals outside `tokens.css`.
4. **Interaction semantics:** real `<button>`/`<a>`/`<input>`; no `role="button"` on a div. One
   `Dialog` primitive owns scrim, Escape, focus trap, focus restore and `inert` on the
   background. One keymap with a scope stack; overlays push a scope, so shortcuts never act
   behind a modal (fixes U5). The asset grid is one tab stop with roving focus (fixes U6).
5. **Errors:** every API failure is a typed `ApiError`; one presentation policy (toast for
   transient, inline for form, blocking screen for engine status). No empty `catch`.
6. **Leaves emit intents.** Asset actions (favourite, trash, restore, identify, stage, export,
   import) live in one `features/asset/actions.ts` used by card, row, menu, inspector and keymap.

### 9.3 Component library

Primitives: Button (primary, secondary, ghost, danger, link × sizes), IconButton, ToggleButton,
SegmentedControl, Tabs, Chip/TraitChip, Badge, Pill, Input, SearchField, Select, Checkbox,
Switch, Slider, Tooltip, Popover, Menu/ContextMenu (positioned by measurement), Dialog,
ConfirmDialog, Toast, Banner, EmptyState, ProgressBar, Spinner, Icon (typed names, one sprite,
`currentColor`), Kbd, Stat, SectionCard, ListRow, TreeRow, Toolbar.

Composites: AssetCard, AssetRow, AssetGrid (virtualised, roving focus), AssetContextMenu,
StatusGem, TypeMark, SizeTier, ImportedBadge, AudioTransport, Waveform, TagEditor, LinkedFiles,
CollectionsPicker, ComparePane, FilterBar, FilterPanel, StagingTray, JobTray, SourceRow,
ProjectSwitcher.

## 10. Design authority and the contract gate

### 10.1 One visual authority

`docs/design/Lodestash Prototype.dc.html` is the **only** visual authority. It is the three-way
merge of the two prototypes on the owner's canvas
(<https://claude.ai/artifact/R3t8jcVqrSz14THqSA9tJF>): the root page (Unity Assets, Unity import,
Add source) and the handoff page (Favorites), merged over their common 2026-07-14 base. The only
conflict, the nav list, resolves to the eleven rows `FEATURES.md` specifies. The file records its
provenance (source hashes) in `docs/design/PROVENANCE.md`; `scripts/design/import-canvas.mjs`
re-imports and re-merges when the canvas changes.

Behaviour stays owned by `UI-SPEC.md` / `FEATURES.md`. Where the prototype's *pixels* and a
spec's *behaviour* disagree, the one table `docs/design/DEVIATIONS.md` records the row (screen,
element, prototype, shipped, reason). An unrecorded difference is a gate failure. The precedence
chain in `docs/design/README.md` and the process in `ALIGNMENT-AUDIT.md` are retired.

### 10.2 Tokens from the prototype

`scripts/design/extract-contract.mjs` renders the prototype, walks every screen, and records the
vocabulary actually drawn: colours (with alpha), font families/sizes/weights/letter-spacing,
radii, spacing, shadows, durations. `docs/design/tokens.css` names every value in that
vocabulary; a check fails if the prototype draws a value with no token, or a token is unused.
Values the prototype uses once or twice are named, not snapped — consolidating them is a design
decision recorded in `DEVIATIONS.md`, not an implementer's.

### 10.3 The contract

For each screen and state (`library`, `library+inspector`, `identify`, `favorites`,
`collections`, `packages`, `unity`, `unity+import`, `traits`, `stats`, `duplicates`, `trash`,
`settings`, `onboarding`, `quicklook`, `staging`, `project-switcher`, `connect-wizard`) the
extractor records, for named anchors, the box (x, y, w, h), padding, gap, font, colours, border
and radius → `docs/design/contract.json`. Anchors are located in the prototype by structure and
text and in the app by `data-anchor`.

### 10.4 The gate

`web/e2e/` runs Playwright in Chromium against `lodestash-desktop --serve` on the **fixture
catalog** (`tests/fixtures/`, built by a Rust helper with the prototype's 19 assets, packs,
statuses and libraries). Per screen/state it:

1. asserts each anchor's measured properties against the contract (±1 px geometry, exact colour
   and font) unless `DEVIATIONS.md` lists it;
2. compares a full-window screenshot against the app's own approved baseline (regression);
3. writes a side-by-side report (prototype | app | diff) for human review.

The suite never touches the user's catalog or config (`LODESTASH_CATALOG`, `LODESTASH_CONFIG_DIR`
point into a temp dir), never returns early, and fails a check that exceeds 5 s unless it
declares a longer budget (the existing CSV report per run is kept).

## 11. Compatibility

- Existing catalogs open unchanged; v2 migrations are additive (indexes for P1; later a per-stage
  failure state for R8). A backup precedes every migration.
- `config.json` keeps its format; new fields default.
- `engine.json` gains `exe`; readers ignore unknown fields.
- `/v1` paths and shapes from `ENGINE-API.md` §2 are preserved; additions only.

## 12. Bugs this design fixes

| Review ID | Fixed by |
|---|---|
| S1 fresh install | §4.2 create-if-missing |
| S2 dead consent toggles | §4.5 typed Settings DTO from protocol |
| S3 Remove from Library errors | engine port of the op, regression test |
| B1, B2 move/rename | core `reconcile_moves` fix (repoint present set; one-to-one hash matching) |
| B4, B5 idle lane, audio tags | §4.6 AppraisePlan |
| B6, B7 failures recorded as done | §4.1 content stamp; per-stage failure state (phase 8) |
| B8 IPv6/underscore producer | canonical producer id in `lodestash-ai` |
| B11 `./` archives | ingest path normalisation |
| B12 config loss | §4.5 |
| B13 non-atomic project registration, undo-trash | core transactions; record changed ids |
| B14 unpinned downloads | SHA-256 pins + read timeouts in the model manager |
| B15 lexical delete fence | canonicalise before `starts_with`; refuse in-pack rows |
| P1 quadratic crawl | index migration |
| U1–U7 | §9.2 rules 1–4 |
| U8 | §4.5 secrets write-only |
| U9 | §4.1 supervised writer |
| U10 | §8 single instance + §4.2 lock |
| U11 | §6 bytes |
| R6 UI-thread IPC | §2/§6: the UI talks async HTTP |

B3 (in-pack preview re-inflation) and B9 (zero vectors) are fixed in phase 8 with R8.

## 13. Phases

Detailed tasks are in the plan. Each phase ends green and committed.

| Phase | Outcome |
|---|---|
| P0 | Review, this spec, ADRs, plan, reconciled prototype, board cards, banners on superseded docs |
| P1 | Core hardening: reader/writer types, typed errors, migration lifecycle, indexes, B1/B2/B11/B13 |
| P2 | `lodestash-protocol` (catalog for today's 27 agent ops) and `lodestash-engine` (absorbs app, writer, read model, storage, media, domain half of ServerState); S1/S3/U8/U9, config, AppraisePlan |
| P3 | `lodestash-api` as an adapter: derived routes, UI session, request checks, SSE, bytes, static UI, system ops |
| P4 | `lodestash-mcp` standalone (stdio), auto-launch, forbidden-dependency test, end-to-end test with a real app on the fixture catalog |
| P5 | Thin Tauri shell; delete Tauri commands, `lodestash://`, old `ui/` (`--mcp` and `agent_bridge.rs` went with P4) |
| P6 | Web foundation: contract extractor, tokens, primitives, data layer, generated client, shell, fixture catalog, gate |
| P7 | Screens, in order: Library (+Inspector, Quick Look, staging, filters) → Identify → Favorites → Collections → Packages → Unity Assets (+import, add source) → Traits/Mapping → Stats → Duplicates → Trash → Settings → Onboarding → project switcher / Connect wizard |
| P8 | R8 failure state, B3/B6/B7/B9/B14/B15, SQL-backed read model, MCP streamable HTTP, doc reconciliation |

## 14. Out of scope

Godot and Unreal plugins; the Unity UPM plugin itself (its `/v1` contract is preserved);
moving library membership and blacklists from `config.json` into the catalog (worth doing, but
it is a schema and UX change of its own); a hosted inference provider.

## 15. Risks

- **The engine crate becomes the largest crate.** Accepted: it replaces three facades. Internal
  modules follow the areas in §4.1 so no file exceeds ~1,000 lines.
- **Serving the UI over loopback HTTP** adds auth and request checks the Tauri IPC model did not
  need. §6 is the full list; a security review runs on P3.
- **The snapshot read model** keeps whole-library memory and rebuild cost until P8. At today's
  library (~8k assets) this matches current behaviour.
- **Contract anchors located by text in the prototype** break if the canvas changes copy. The
  extractor fails loudly on a missing anchor; re-import is a deliberate step.
- **The app has no UI between P5 and the first P7 screen** on this branch. Acceptable on a branch;
  `main` is untouched.
