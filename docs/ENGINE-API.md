# Engine API — the loopback HTTP/JSON contract

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

The **deterministic integration plane**: the loopback HTTP/JSON API that the **Unity editor plugin**
(and any scripting/CI) speaks. This is the load-bearing interface between the Lodestash core and the
engine — specified **once, here** — so the GUI, the MCP server, and the Unity plugin are provably
three presentations of the **same** `lodestash-core` operations (see
[`ARCHITECTURE.md`](ARCHITECTURE.md) §2).

> **Relationship to MCP.** [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) is the **AI control plane** (for
> LLM clients that *reason* about which assets to pull). This doc is the **deterministic plane** (for
> the plugin and tooling that *execute* exact operations). Every MCP tool maps **1:1** onto an
> endpoint here (§2) — neither plane has an operation the other can't reach, so they can never drift.
> Plugins do **not** speak MCP; they speak this REST API.

Verify HTTP/security specifics against [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §5 before
implementing — the loopback/token/sandbox rules there govern this API too.

---

## 1. Lifecycle & the token handshake

The API is **loopback-only** and **per-session token-gated**. The GUI is the server; the plugin is a
client. Startup sequence:

1. On launch, `lodestash-core` binds an HTTP listener to **`127.0.0.1:<port>`** (ephemeral port; never
   `0.0.0.0`).
2. It generates a **per-session bearer token** and writes a **handshake file** the plugin can read:

   ```json
   // engine.json
   { "version": 1, "port": 51731, "token": "b3f1…", "pid": 8123, "api_version": "v1" }
   ```

   | OS | Handshake file |
   |---|---|
   | **Windows** | `%LOCALAPPDATA%\Lodestash\engine.json` |
   | **macOS** | `~/Library/Application Support/Lodestash/engine.json` |
   | **Linux** | `~/.local/share/lodestash/engine.json` |

   The file is **rewritten each session** (token rotates) and removed on clean shutdown. A stale
   file (dead `pid`) is ignored by clients.

   **Permissions, precisely** (`handshake.rs`): on Unix the file is `chmod 0600` after writing. On
   **Windows there is no explicit mode set** — the protection is the per-user ACL that
   `%LOCALAPPDATA%` already carries, which keeps other non-administrator users out of the profile.
   That is adequate, and it is not the same guarantee, so it is written down rather than implied by
   "user-only permissions": anything running **as this user** can read the token, and the loopback +
   `Origin` checks in §4 are what stand between that and a hostile local process.
3. The plugin reads `engine.json`, then sends every request with
   `Authorization: Bearer <token>` and a loopback `Origin`/`Host`.
4. The server **validates token + `Origin`/host** on every request:
   - missing/!= token → **`401 Unauthorized`**;
   - non-loopback `Origin`/`Host` → **`403 Forbidden`** (blocks other local processes and
     DNS-rebinding-style attacks).

> The GUI can also **display** the token/port for a manual paste flow when a plugin can't read the
> file (sandboxed editors, remote-ish setups). stdio MCP has no network surface and needs no token;
> this token applies only to the HTTP plane (and to Streamable-HTTP MCP if ever enabled).

All endpoints are versioned under **`/v1/`**. Content type is `application/json` unless noted
(previews/files stream bytes).

---

## 2. Endpoint catalog — 1:1 with the MCP surface

Each row is the **same core operation** as the matching MCP tool in
[`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §2. Mutating/side-effecting routes are flagged; the server
**confirms/sandboxes them regardless of client** (§4).

| MCP tool | REST endpoint | Notes |
|---|---|---|
| `search_assets` | `GET /v1/search` | query/filter params; paginated (`cursor`, `limit`) |
| `semantic_search` | `GET /v1/search?mode=ai` | natural-language `q`; requires embeddings |
| `find_similar` | `GET /v1/assets/{id}/similar` | `limit`; cosine over `vec_asset` ("more like this") |
| `get_asset` | `GET /v1/assets/{id}` | full **asset detail** (§3) |
| `get_asset_preview` | `GET /v1/assets/{id}/preview?kind=` | streams PNG/JPEG (thumbnail/turntable/waveform) |
| — | `GET /v1/assets/{id}/file` | streams the on-disk source bytes (the MCP `resource_link` target) |
| `list_tags` | `GET /v1/tags?prefix=` | tags + counts |
| `add_tag` / `retag` | `POST /v1/assets/{id}/tags` | *mutating*; writes as `ai:legacy-agent`, never as the person (S05) |
| `write_tags` | `POST /v1/tags/batch` | *mutating, approval-gated* → `202` job; attributed batch of tags + caption (§8.3) |
| `list_collections` | `GET /v1/collections` | incl. the staging collection (§staging) |
| `apply_mapping` | `POST /v1/mapping/apply` | *mutating*; `{scope, taxonomy_id}` |
| `identify_asset` | `POST /v1/identify` | *long-running* → `job_id`; `{asset_ids, tiers?}` |
| `suggest_tags` | `GET /v1/assets/{id}/suggest-tags` | read-only tag proposals + confidences |
| `list_packages` | `GET /v1/packages` | filters: `publisher`, `category`; `cursor` |
| `unpack_package` | `POST /v1/packages/unpack` | *long-running* → returns `job_id` |
| `export_asset` | `POST /v1/export` | *side-effecting*; arbitrary **sandboxed** `dest_path` |
| `import_to_project` | `POST /v1/import` | *side-effecting*; into the **open project** (§5) |
| `register_project` | `POST /v1/projects/register` | plugin announces the open editor (§5) |
| `list_projects` / `get_active_project` | `GET /v1/projects` | registered/active editors |
| `stage_asset` / `list_staged` / `import_staged` | `GET/POST/DELETE /v1/stage`, `POST /v1/stage/import` | the shortlist (§6) |
| `get_job_status` | `GET /v1/jobs/{id}` | progress/result for long ops |
| `cancel_job` | `POST /v1/jobs/{id}/cancel` | idempotent; job flips to `cancelled` |
| `get_server_info` | `GET /v1/server-info` | app + schema + API version |

`export` vs `import`: **`/v1/export`** writes a dependency closure to an arbitrary sandboxed path (no
editor needed); **`/v1/import`** routes into the **currently open** Unity project and triggers an
in-editor import (§5). Both reuse the **same dependency-closure resolution** described in
[`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §3.

> **Writes from a headless MCP server** are forwarded to the running app over this same loopback
> plane, using the `engine.json` port + token ([ADR-0011](decisions/ADR-0011-headless-mcp-forwards-writes.md)).
> `lodestash-desktop --mcp` keeps a read-only catalog connection — WAL admits any number of
> readers — and posts each mutating op to the app, so ADR-0002's single writer holds across
> processes without a lock. With no app running the forward fails and the tool says so.
> `RemoveTag` and `UnstageAsset` cannot be forwarded: neither has a route here (nor an MCP tool),
> and they are refused rather than reported as zero rows changed.

> **Parity scope.** "1:1" covers the **agent-facing** surface. A few routes are **REST-only by
> design** — `GET /v1/assets/{id}/file` (the MCP `resource_link` target) and the plugin-transport
> endpoints `GET /v1/import-queue` + `POST /v1/import-queue/{req}/ack` (§5.3) — because they are
> plugin↔core plumbing, not operations an LLM should call. No *agent* operation is missing from
> either plane.

> **Server-side confirmation (shipped).** `POST /v1/export` returns `202` with a job in state
> **`awaiting_approval`** — an extra state beyond the §3 set. Nothing is copied until the person at
> the desktop app approves the prompt; denial cancels the job permanently.
>
> The gate is **scoped to the agent plane**, which is the whole reason it exists: it is the person
> at the desktop app confirming what something *else* asked for. The desktop’s own **Export to
> folder** (UI-SPEC §16.2) therefore runs without it — that person is the one asking, and they said
> so by choosing a destination in a native folder dialog. Both paths share one implementation, so
> what an export copies cannot drift between them, and the **export sandbox below applies to both**:
> it protects the non-destructive invariant, not the agent boundary. This is
> [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §5 "confirm mutating ops server-side, don't rely
> solely on the client's human-in-the-loop", made literal. Clients must poll `GET /v1/jobs/{id}`
> and handle a job that never leaves `awaiting_approval`.
>
> **Export sandbox (shipped).** `dest_path` must be absolute, free of `..` (`400 path_traversal`),
> and outside every registered library source root (`400 forbidden_destination`) — the library is
> read-only by invariant, so an agent can never turn an export into a write into the user's own
> asset folders.

> **Implementation status (interim).** Until the remaining slices land:
> `GET /v1/search?mode=ai` answers `501 ai_unavailable` **only while no embedding model is
> loaded** — with SigLIP/CLAP installed it runs the same RRF-fused hybrid search as the desktop
> grid. `AssetSummary.tags` **carries the asset's real tags since 2026-08-30** — it answered `[]`
> unconditionally while this note called it interim, so an agent that wrote a tag through
> `add_tag` read the asset back and saw none, which makes a write unverifiable from the plane that
> made it. Its `package` still carries `{id, name}` without `publisher`.
> `Package` is complete as of 2026-08-28 — `version`, `license`, `license_url`, `attribution` and
> `pipeline` all ship (the licence harvest, INGESTION §2). `license` is `null` only where the pack
> stated none **and** did not come from the Asset Store cache; `pipeline` is `null` where detection
> could not tell, which is not the same as `built_in`. `find_similar` answers `[]` for an
> asset with no embedding (an un-encoded library is not an error).
> `export` honours `with_dependencies` over the harvested relationship graph (DATA-MODEL §10) and
> reports `dependencies_included` plus `dependencies_added`; the graph carries material → texture
> (material YAML) and mesh → material (prefab bindings), so a model exports with its material and
> that material's textures. An asset outside a Unity project has no `.meta`, hence no GUID and no
> edges — it exports alone. `AssetDetail` extends the summary with `size_bytes`, `stats` and
> `dependencies` (the same closure an export would copy) — `description`, `files`, `collections`,
> `license` and `imported_in` arrive with the features that populate them. Clients should treat
> missing fields as "not known yet", never as "empty".

---

## 3. Canonical entity shapes (defined here, referenced by MCP)

These shapes are the **single source of truth** for both planes. The MCP doc references them rather
than redefining, so summaries/details never diverge.

```jsonc
// AssetSummary — search results, staging, queue payloads
{
  "id": 4821, "uuid": "…", "name": "Beaver_LowPoly",
  "media_type": "model_3d",            // model_3d | texture | audio | image | sprite | material | font
  "format": "fbx", "status": "identified",
  "package": { "id": 12, "name": "Forest Pack", "publisher": "AcmeArt" },
  "tags": ["animal", "lowpoly"],
  "preview_url": "/v1/assets/4821/preview",
  "file_url": "/v1/assets/4821/file"
}

// AssetDetail — get_asset; extends AssetSummary with:
{
  "description": "…",
  "stats": { "tris": 1240, "verts": 720, "materials": 2, "bones": 0 },  // modality-specific
  "files": [ { "role": "mesh", "rel_path": "…", "ext": "fbx", "size_bytes": 33012 } ],
  "collections": [ … ], "categories": ["Props/Animals"],
  "license": { "name": "Unity Asset Store EULA", "attribution": "AcmeArt", "url": "…" },
  "dependencies": { "count": 3, "asset_ids": [4822, 4823, 4824] },
  "imported_in": [ { "project_id": 2, "dest_path": "Assets/Wildlife/Beaver_LowPoly.fbx" } ]
}

// Package — list_packages (see DATA-MODEL.md §7)
{ "id": 12, "name": "Forest Pack", "publisher": "AcmeArt", "category": "3D/Environments",
  "version": "2.1.0", "asset_count": 412, "unpacked_at": 1716500000,
  "license": "Unity Asset Store EULA", "license_url": "https://unity.com/legal/as-terms",
  "attribution": "AcmeArt", "pipeline": "urp" }   // pipeline: built_in|urp|hdrp|mixed|null

// Job — get_job_status (job engine; see ARCHITECTURE.md §3.1)
{ "job_id": "imp_9f…", "kind": "import", "state": "running",   // queued|running|done|error|cancelled
  "progress": 0.66, "result": null, "error": null,
  "detail": null }                  // what an approval shows beyond the summary (write_tags: the diff)
```

`media_type`, `status`, and `content_kind` use the exact enums in [`DATA-MODEL.md`](DATA-MODEL.md) §2.

---

## 4. Error model, confirmation & versioning

- **Errors** are JSON with a stable machine code:

  ```json
  { "error": { "code": "project_not_registered",
               "message": "No Unity project is currently registered.",
               "detail": "Open the Lodestash panel in the Unity editor, then retry." } }
  ```

  Common codes: `unauthorized` (401), `forbidden_origin` (403), `not_found` (404),
  `path_traversal` (400), `project_not_registered` (409), `editor_unavailable` (409),
  `job_failed` (500). **`already_imported` is advisory, not an error** — it is a field in the `200`
  body of `/v1/import` (§5.4), letting callers warn before duplicating rather than fail.
- **Server-side guards (never trust the client):** validate and **sandbox** every write path
  (`/v1/export`, `/v1/import`, `apply_mapping`) against traversal; **confirm mutating ops
  server-side**; rate-limit. Mirrors [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §5.
- **Async:** any long op (`unpack`, `export` batches, re-index/embed) returns a **`job_id`
  immediately**; poll `GET /v1/jobs/{id}`. Keep each request fast.
- **Versioning:** path-prefixed (`/v1/`); `GET /v1/server-info` reports `{app, schema, api}` versions.
  Additive changes are safe; breaking changes get `/v2/`. Plugins check `server-info` on connect and
  degrade gracefully. The handshake file's `api_version` lets a client pick the right prefix.

---

## 5. Project registration & import-into-the-open-project (the Unity heart)

This is the mechanism that lets **both** the GUI **and** Claude (via MCP `import_to_project`) land an
asset in the **open** editor. The plugin owns Unity's main thread; the core owns the catalog. They
meet at a **shared import queue**.

### 5.1 Registration
On panel open, the plugin announces itself:

```http
POST /v1/projects/register
{ "project_path": "C:/dev/MyGame", "unity_version": "6000.0.23f1",
  "pipeline": "urp", "can_execute_main_thread": true }
→ 200 { "project_id": 2, "is_active": true }
```

The core records/updates a `project` row (see [`DATA-MODEL.md`](DATA-MODEL.md) §9) and marks it
**active**. `GET /v1/projects` lists registered editors and which is active. Registration is
**heartbeated** (`last_seen_at`); an editor that stops polling is considered closed.

### 5.2 Enqueue an import
The GUI, a plugin "Import" click, or MCP `import_to_project` all call:

```http
POST /v1/import
{ "asset_ids": [4821], "project_id": 2,
  "dest_subfolder": "Wildlife", "with_dependencies": true }
→ 202 { "job_id": "imp_9f…" }
```

The core resolves the **dependency closure** (mesh + materials + textures — same resolver as
`export`), stages the bytes into a temp area, and appends an **import request** to that project's
queue. If `project_id` is omitted it targets the **active** project; if **none is registered** it
returns `409 project_not_registered` (the AI/GUI then falls back to `export` to a path).

### 5.3 The plugin drains the queue on Unity's main thread
The plugin runs a **long-poll** loop (on an `EditorApplication.update` / main-thread pump):

```http
GET /v1/import-queue?project_id=2        # long-poll, ~25s, returns [] on timeout
→ 200 [ { "request_id": "req_3a…", "job_id": "imp_9f…",
          "assets": [ /* AssetSummary[] incl. file_url + dependency file_urls */ ],
          "dest_subfolder": "Wildlife" } ]
```

For each request the plugin (on the main thread): fetches the files, writes them under
`Assets/<dest_subfolder>/`, calls `AssetDatabase.ImportAsset(...)` / `Refresh()`, then **acks** with
the real in-project result:

```http
POST /v1/import-queue/req_3a…/ack
{ "imported": [ { "asset_id": 4821,
                  "dest_path": "Assets/Wildlife/Beaver_LowPoly.fbx",
                  "unity_guid": "8e1c…" } ] }
→ 200
```

On ack the core writes an **`import_record`** per asset (provenance + dup-guard, §dup) and flips the
`import` job to `done`. MCP `get_job_status` / GUI both observe completion the same way.

> **Transient vs durable.** The import **queue** is in-memory (job-engine backed); only the
> **`import_record`** result is persisted. A crash mid-import loses the queue, not the catalog.

### 5.4 Import advisories (dup-guard & pipeline mismatch)
Before enqueuing, the core checks `import_record` for `(asset_id, project_id)`:
- if present and the on-disk `dest_path` still resolves, it returns the asset in an
  **`already_imported`** advisory list in the `/v1/import` response (HTTP `200` with
  `{ "imported_existing": [...], "job_id": null }` when *all* assets are already present, or proceeds
  for the rest). Clients surface "already in project — re-import?" rather than silently duplicating.

It also compares the asset's `package.pipeline` (Built-in/URP/HDRP, harvested at ingest — see
[`DATA-MODEL.md`](DATA-MODEL.md) §12) against the target project's pipeline (reported by the plugin at
registration). A mismatch adds a **`pipeline_mismatch`** advisory list to the response —
"this pack targets Built-in; your project runs URP (materials may appear pink)" — and the import
proceeds. Advisory, never an error: the user/agent decides.

*(Shipped 2026-08-28.* `Catalog::pipeline_mismatches`, on **both** the `202` awaiting-approval body
and the `200` nothing-new body, and on the desktop's own `⇢ Add to project` as a second toast
beside the outcome — an advisory the app dropped would be a warning the agent gets and the person
looking at the screen does not. Entries are grouped **per pack**, which is the sentence a user is
told:

```json
"pipeline_mismatch": [
  { "package_id": 12, "package": "Forest Pack", "package_pipeline": "built_in",
    "project_pipeline": "urp", "asset_ids": [4821, 4822] }
]
```

The key is **omitted when empty**, so a client that never sees it behaves as it did before the
advisory existed. It stays silent in all three "we don't know it is a mismatch" cases — the
project's pipeline unreported, the pack's undetermined, or the pack `mixed` (it ships a variant per
pipeline, so whatever the project runs is in there). It is computed over what the caller *asked
for*, not only what is new: re-importing an already-present asset is still worth the warning. Both
planes render it from one builder (`ImportKickoff::to_json`) rather than two hand-copied bodies,
which is how §2's 1:1 rule survives a change like this.)*

### 5.5 Constraints
- Editor must be **open and polling**; imports run on Unity's **main thread**; large binaries travel
  by `file_url` (path/stream), never inline.
- **Dependency resolution is within a package** — cross-package GUID references are flagged for the
  user (GUIDs are reassigned per import). Same rule as
  [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §3.

### 5.6 Shipped (interim) — cards #2 & queue, 2026-07-27/28

The **full server-side round-trip** is in: `register_project` / `list_projects` / `import_to_project`
on both planes (parity green, allow-list-gated), plus the **REST-only** `GET /v1/import-queue` +
`POST /v1/import-queue/{req}/ack` plugin transport (§5.3). `import_to_project` parks
`awaiting_approval`; on approval it **enqueues** (nothing recorded yet); a client polling the queue
gets the request with the assets' `file_url`s; its **ack writes the durable `import_record`** with
Unity's real `dest_path`/`guid` and flips the job `done` — the `UNIQUE(asset_id, project_id)`
dup-guard applies at that write. This is exercised end-to-end headless by a simulated plugin
(register → approve → poll → ack → re-import is dup-guarded).

Still interim until the real **UPM plugin** lands:

- **The queue poll returns immediately**, not the spec's ~25 s long-poll — a busy-poll cost, not a
  correctness gap (a `tokio::sync::Notify` wait slots in behind the same method).
- **`get_active_project` is folded into `list_projects`** — active-first, each row carries
  `is_active`, so no separate call. The §2 row keeps both names; only `list_projects` exists.
- **Staging (§6) is built** (ADR-0010) and the **`pipeline_mismatch` advisory (§5.4) shipped
  2026-08-28**, alongside the `already_imported` dup-guard. Only a real editor doing
  `AssetDatabase.ImportAsset` remains unbuilt — every step it talks to exists and is tested.

---

## 6. Staging (the shortlist)

A **staging area** is a candidates tray shared across the GUI, MCP, and the Unity panel — "collect
the picks, then import them together," matching the agent loop in
[`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §2. There is **one staging collection per registered
project** (`is_staging=1` + `project_id`, [`DATA-MODEL.md`](DATA-MODEL.md) §9) — an import target
owns its shortlist, and switching projects never mixes picks ([`UI-SPEC.md`](UI-SPEC.md) §16).
All stage endpoints take an optional `project_id`; omitted ⇒ the **active** project.

**With no project registered there is still a tray** — exactly one, with `project_id IS NULL`
(M20, [ADR-0010](decisions/ADR-0010-one-staging-tray.md)). These endpoints used to answer `[]` and
refuse writes in that state, on the reading that nothing can be staged without somewhere to send
it. The app disagreed and kept its own tray in memory, so `list_staged` reported an empty shortlist
while the user was looking at six picks. Both planes now resolve through
`Catalog::active_staging_tray_id` — the active project's tray, else the project-less one — and the
first `register_project` **adopts** that tray rather than orphaning what it holds.

```http
GET    /v1/stage?project_id=2          → AssetSummary[]   (that project's shortlist)
POST   /v1/stage      { "asset_ids": [4821, 5099], "project_id": 2 }   (add)
DELETE /v1/stage      { "asset_ids": [5099] }             (remove; empty body clears)
POST   /v1/stage/import { "project_id": 2, "dest_subfolder": "Staged", "clear": true }
       → 202 { "job_id": "imp_a1…" }   (imports that tray via §5, optionally clears it)
```

Because each tray is one collection, an asset Claude stages over MCP appears in the GUI tray and the
Unity panel instantly, and vice-versa.

---

## 7. Bottom line

- **One contract, three callers.** GUI, MCP, and the Unity plugin all reduce to these `/v1/`
  operations over `lodestash-core` — the parity table (§2) is the guarantee they don't drift.
- **The Unity round-trip closes here:** `register` → `/v1/import` → `import-queue` (plugin, main
  thread) → `ack` → `import_record`. MCP's `import_to_project` is just an AI-side entry into this same
  flow ([`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §3).
- **Provenance, dup-guard, staging, and license** are first-class in the payloads, so the deterministic
  plane carries the asset-management state the GUI and AI both rely on.

## 8. Proposed asset-analysis contract

Drafted 2026-09-10. **§8.3 `write_tags` shipped 2026-09-11 (v1 plan S05)** — REST, MCP and the
approval gate, with durable receipts (M27). Still open within it: `vocabulary_revision` is accepted
and recorded but enforced only once §8.5 lands, and a headless `--mcp` process parks approvals in
its own memory, where the desktop tray cannot see them, until S11 routes requests to the app that
owns the gate. The rest of this section is still proposed. This section owns the wire
contract for [MCP asset analysis](MCP-ASSET-ANALYSIS.md). It refines the v1 architecture plan's
S04/S05/S10/S11/S12. Existing §2 operations retain their current containers and required parameters;
the rows below are additive unless explicitly described as an extension. New fields are not evidence
that current binaries implement them. The implementation must add each tool, route, permission
mapping and behavioral parity check together.

### 8.1 Discovery, selection and detail

| MCP tool | REST endpoint | Shared request and response |
|---|---|---|
| `list_libraries` (new) | `GET /v1/libraries` | Optional `limit` (25, maximum 200), `cursor`; `{libraries:[{id,name,kind,asset_count}],next_cursor}` ordered by source ID. IDs are `import_source.id`. No root paths needed for discovery. |
| `search_assets` (extend) | `GET /v1/search` | Existing MCP `query` / REST `q`, `limit`, `media_type`; add optional `source_id`, `package_id`, `tags` (canonical `namespace:value`, AND semantics). Apply all supplied filters before ranking/LIMIT. Keep MCP's array and REST's `{results}` containers. |
| `semantic_search` (extend) | `GET /v1/search?mode=ai` | Same optional scope filters; apply them before candidate scoring/top-K on both retrieval branches. Missing embedder remains `ai_unavailable`; do not silently substitute keyword search. |
| `list_assets` (new) | `GET /v1/assets` | `source_id` required, optional `package_id`, `media_type`, `status`, `tag_state` (`any`, `tagged`, `untagged`; default `any`), `selection` (`ordered`, `random`; default `ordered`), `limit` (25, maximum 200), `cursor`. Returns `{assets,next_cursor,selection,upper_id}`. |
| `get_assets` (new) | `POST /v1/assets/read` | Read-only despite HTTP method: `{asset_ids:[...],source_id?}`, 1–100 unique positive IDs. Returns `{assets,unavailable:[{id,code}]}` in requested order within each array; `source_id` mismatch is unavailable. |
| `get_asset` (extend) | `GET /v1/assets/{id}` | Same detail additions as `get_assets`; unknown/trashed asset is not returned as live content. |

REST tags use repeated `tags=namespace:value` parameters. Invalid enum, nonpositive IDs, out-of-range
limits, empty ID lists and duplicate explicit IDs produce `invalid_argument`, rather than silent
widening. Existing search callers retain their historical defaults/caps; new filters must mean the
same thing for every accepted limit. A supplied package must belong to the supplied source.

All selection excludes trashed, excluded and non-content assets. Search returns ranked candidates,
not a complete traversal. For ordered browse, use ascending ID keyset pagination with a captured
upper ID bound; encode cursor version, last ID, upper ID, catalog identity and filter signature.
Reject malformed/cross-catalog/filter-mismatched cursors as `invalid_cursor`. New insertions beyond
the upper bound wait for a new walk; mutable eligibility is evaluated per page, so this is not a
snapshot of all tags across the entire walk. Never collect all result objects into one tool response.

Random selection uses a single eligible-ID query with `ORDER BY random() LIMIT ?`, then hydrates
those IDs. It samples without replacement across the entire scoped population, not the first
search page. `cursor` is invalid for random selection and `next_cursor` is null. Repeated requests
are fresh samples; save returned IDs for replay. If fewer matches exist, return all and let the
caller report the shortfall. This intentionally accepts an O(N) eligible-ID scan for occasional
small audits; profile before replacing it with a maintained sampling index.

Summary/detail additions: `source_id`, `package_id` (retain existing `package`), `evidence_url`,
`file_url`, `file_resource_uri`, and explicit `file_availability` (`available`, `in_pack`, `missing`,
`denied`). URLs are relative authenticated `/v1/` paths, resolved against the current handshake by
the local host. They never include bearer tokens. `asset://{id}/file` is a resource reference, not
a promise that a remote provider can fetch the local machine's files.

Detail also exposes `description`, `analysis_revision` and each tag's existing namespace/value/
confidence plus `source` and nullable `model_id`. Preserve the tag-object array already shipped
by `summary_json` (the old string-tag example in §3 describes the target-era shorthand, not the
current JSON). `analysis_revision` is an opaque versioned fingerprint of the source/preview input
identity, sorted complete tag associations, description and review state; generate it in one read
snapshot. It must change when the evidence or edited annotations change, even without a new asset ID.

### 8.2 Evidence and file access

`get_asset_evidence` (new) ↔ `GET /v1/assets/{id}/evidence` accepts `id` (REST path), optional
`kind` (`auto`, `thumbnail`, `model_views`, `text`; default `auto`), `max_dimension` (512, maximum
1024) and `views` (1, maximum 4, only for models). Invalid media/kind pairs fail explicitly.
The JSON manifest is shared by REST and MCP:

```json
{
  "asset_id": 4821,
  "analysis_revision": "opaque-v1-revision",
  "status": "ready",
  "representations": [
    {"kind":"model_views","mime_type":"image/png","url":"/v1/assets/4821/preview?kind=model_views&view=0&max_dimension=512","width":512,"height":512}
  ],
  "file_url": "/v1/assets/4821/file",
  "file_resource_uri": "asset://4821/file",
  "dependencies_used": [],
  "limitations": []
}
```

Status is `ready`, `partial`, `unavailable` or `denied`. `limitations` contains machine codes and
plain explanations (e.g. missing texture, geometry-only proxy, unavailable archive, unsupported
format). Never label a silhouette proxy as a textured render. Add the view parameters above to
the existing preview operation on both planes, keeping the existing no-argument behavior.

MCP sends this manifest as `structuredContent` and JSON text, plus bounded `image` blocks for
available raster views and a resource link for file access. REST sends the manifest and uses its
authenticated representation URLs for the same bytes. Resource reads revalidate source identity,
visibility and byte policy; link possession does not bypass these checks. An LLM host must actually
deliver the image blocks to a vision-capable model; raw FBX/GLB bytes or base64 text are not a substitute.

Initial ceilings: at most four images and 4 MiB total decoded image-file bytes per evidence call;
at most 64 KiB UTF-8 extracted text, with `truncated`/original size disclosed. One asset per evidence
call avoids a large batch of images exhausting the host context. Render/archive work runs in a
bounded blocking worker (initial maximum two), outside the catalog lock; enforce a 30-second
deadline and cooperative cancellation between extraction/render stages. Busy/deadline results
are explicit retryable errors, never fabricated evidence. Reuse cached previews where possible.

`GET /v1/assets/{id}/file` remains REST-only transport; MCP `asset://{id}/file` reads use the same
resolver. A direct file request materializes only the requested payload when it is in a Unity
package. It does not unpack the package, ingest a second asset, or write `workspace_rel` just for
analysis. Bound an extracted payload at the existing 256 MiB preview-payload ceiling. Evidence
dependencies share that aggregate byte ceiling and a maximum of 32 resolved entries per call;
only necessary mesh/material/albedo inputs are pulled. Above those bounds return `asset_too_large`
or partial evidence, with no partial file advertised as complete. Raw file reads have no automatic
dependency expansion. Large binary resources remain links/streamed HTTP responses, never inline
MCP text; inline binary resource reads exceeding the evidence byte limit return `resource_too_large`.

Resolve the entry exclusively from the catalog's package locator/manifest. Validate offsets, sizes,
archive identity, output containment and any external dependency path. Reuse surgical/cache helpers;
concurrent requests must not publish partially written cache entries. On archive revision mismatch
return `source_changed` and require rescan; no full-extract fallback. Report missing local data as
`not_local`/`archive_missing`, and unsupported compiled bundles separately. Existing `Range` support
must remain correct after materialization. Cache creation is a derived artifact side effect only.

Byte release requires configured destination/per-modality consent, including dependency modalities.
Apply it to MCP images, resource reads, preview URLs and raw files. Unknown destination defaults to
metadata only; deny remote audio bytes. Use an app-configured producer profile for the stdio child,
and authenticated REST policy on the shared server; request arguments cannot widen permission.
The token protects the API against unauthenticated clients, not malicious software running as the
same OS user. Hosts remain responsible for what they do with bytes after authorized delivery.

### 8.3 Attributed result writes

`write_tags` (new) ↔ `POST /v1/tags/batch`. A single item uses the same batch envelope:

```json
{
  "request_id": "730a3a8b-61d4-4c29-9253-f96c50a12848",
  "vocabulary_revision": "opaque-vocabulary-revision",
  "assets": [{
    "asset_id": 4821,
    "expected_revision": "opaque-v1-revision",
    "producer_id": "configured-analysis-host",
    "model_id": null,
    "mode": "replace_producer",
    "tags": [{"namespace":"subject","value":"stone arch","confidence":null}],
    "description": "A weathered stone arch with a broken upper edge."
  }]
}
```

Require 1–100 unique asset IDs, at most 1 MiB serialized UTF-8 request bytes, a UUID `request_id`,
the applicable `vocabulary_revision` (§8.5),
nonempty revision and configured producer ID. `mode` is `append` or `replace_producer`; no global
replace/delete. The submitted producer must match trusted host configuration; the server records
`source=ai:<producer_id>`, actor=agent and transport separately. Model ID is reported provenance,
nullable when the host cannot establish it; never invent a model/version or accept `source=manual`.

Tag policy: model-authored namespaces follow IDENTIFICATION-AND-SEARCH §2 and the shared
validator (`subject`, `style`, `material`, `mood`, `usage`, `free` for the initial 3D/image path;
modality-specific audio facets follow that owning schema). AI-PIPELINE §6 owns normalization policy.
Tag values are nonempty strings up to 128 Unicode scalar values after normalization,
at most 32 assignments and 8 per namespace after normalization/deduplication. Normalize through
the existing canonical tag policy before limits. Confidence is null or finite in [0,1]. Reject
unknown fields, invalid types and unsupported namespaces; do not stringify arbitrary model objects.
Description is optional, up to 2,048 Unicode scalar values; omitted means preserve. Empty text/null
is invalid (clearing is not in this slice). Only an app-configured caption producer can replace it,
and its proposed replacement must appear in approval. Other fields cannot be updated by this tool.

Return the existing job envelope with `state=awaiting_approval` (HTTP 202); one batch is one approval
showing source/IDs, producer, tag diff and description diff. Extend the existing app approval flow,
not a new agent authority system. Denial changes no catalog annotations. Revalidate permission,
asset visibility and all revisions at execution time. The entire batch commits or rolls back.
No agent call can create human confirmation. Preserve existing lifecycle rules; external tags alone
do not complete embeddings or clear another producer's work queue. Status promotion still requires
the existing DATA-MODEL §8 prerequisites; never assign `identified` unconditionally after a write.

At completion, `job.result` includes `operation_id`, `changed_assets`, `unchanged_assets` and
`asset_ids`. The client separately calls `get_assets` for authoritative final tags/description and
revision. Record tags, optional description, FTS maintenance, history and durable request identity
in the single writer transaction. Manual/imported/other producer associations survive replacement;
a no-op does not consume undo history or publish a change. Conflict-aware undo follows S04.

Idempotency is catalog-scoped: persist request ID plus a canonical validated-body digest and terminal
receipt independently of the capped undo tail. Same ID/body returns the original pending/terminal
result; same ID/different body is `request_conflict`. At least the mutation receipt survives restart
so an ambiguous retry cannot apply twice. Uncommitted approvals lost on restart fail visibly and may
be resubmitted with the same ID for fresh approval. Never automatically replay an unknown write.

The headless bridge re-reads the handshake for each new operation, checks catalog identity/API
version and forwards to the app. Status/cancel use the app's job identity, not a nested local job;
completed receipts remain discoverable after restart. An old missing job is `job_not_found`, never
assumed successful. An app-closed write is `app_unavailable`; catalog reads continue where safe.

### 8.4 Failures and parity

Use §4's error envelope. New errors have stable codes: `invalid_argument`/`invalid_cursor` (400),
`producer_mismatch` (403), `producer_not_configured` (409, no `agent_producer_id` set),
`asset_not_found`/`job_not_found` (404), `revision_conflict`/`request_conflict`/`source_changed` (409),
`asset_too_large`/`resource_too_large` (413), `unsupported_evidence` (422), `busy` (429),
`deadline_exceeded` (504), `app_unavailable`/`archive_missing`/`not_local` (503), and
`evidence_denied` (403). Existing auth and `tool_disabled` behavior stays intact. MCP exposes domain
failures as `isError=true` with the same code/details; malformed protocol calls remain protocol errors.

New structured MCP outputs include a JSON-text fallback. Name-set parity is necessary but insufficient:
compare normalized responses, eligibility, provenance, authorization, cache effects, persistence,
limits and failures. Batch reads may report individual unavailable IDs; batch writes never partly
succeed. Check capability fields in `get_server_info` (catalog identity, enabled analysis operations,
writer availability and byte policy) instead of assuming support from a saved example.

MCP content blocks, structured-output schemas and domain-error handling are based on the
[MCP tools specification](https://modelcontextprotocol.io/specification/2025-11-25/server/tools),
checked 2026-09-10. Structured tool output describes the server's result; the write input schema
separately constrains the LLM's proposed annotations. Revalidate host support during implementation.

### 8.5 Vocabulary-guided identification and review

Clarified 2026-09-10: the complete workflow includes existing tag/preview reporting, identification,
then vocabulary-based acceptance/rejection, with approved vocabulary growth. Codex is the first host;
analysis receives 3D preview images and metadata and may write tags plus a short description.
These additions are planned; `suggest_tags` still means stored suggestions in the shipped server.

| MCP tool | REST endpoint | Contract |
|---|---|---|
| `get_vocabulary` (new) | `GET /v1/vocabulary` | Optional `media_type`, `taxonomy_id` (active by default), `prefix`, `limit` (25, maximum 200), `cursor`. Returns `{revision,policy,traits,categories,aliases,rules,next_cursor}`. |
| `review_tags` (new) | `POST /v1/tags/review` | `{request_id,vocabulary_revision,assets:[{asset_id,expected_revision,decisions:[{namespace,value,source,action,reason}]}]}`. Actions: `accept`, `reject`. App-approved job. |
| `propose_vocabulary_changes` (new) | `POST /v1/vocabulary/proposals` | `{request_id,expected_revision,changes:[...]}`. App-approved job; approved changes update the existing vocabulary tables. |

Vocabulary reads return the canonical tag ID/namespace/value, trait scope/usage note/teach flag;
categories return ID/taxonomy/parent/path/label; aliases and rules retain their existing taxonomy
ownership and priority. Raw non-trait tags remain discoverable via `list_tags`. No taxonomy is
`categories:[]`, not an implicit request to create one. The revision fingerprints applicable traits,
taxonomy structure/aliases/rules and the configured policy. Paginate a stable typed-ID order over
the combined entries, with at most `limit` entries total across the arrays. Pin revision/filter
signature in the cursor; changed vocabulary invalidates continuation rather than mixing revisions.
Returned configuration/usage text is evidence, not authority to execute arbitrary instructions.

`policy` is app-configured, not a request override. Modes: `open` (normalized in-schema new values
allowed), `controlled` (only applicable canonical traits), `extend_with_approval` (controlled plus
the proposal flow). Proposed default for this workflow is `extend_with_approval`; do not silently
change the existing global tagger or human-tagging defaults. Scope/namespace restrictions always
apply. `teach=false` pauses identification teaching but does not forbid the trait. Map synonyms only
through explicit applicable rules; a taxonomy category alias is not a raw-tag synonym. A confidence
threshold alone never proves semantic correctness. Expose usage guidance so Codex can judge fit.

`write_tags` validates proposed assignments against this policy and revision at approval/commit;
unknown controlled terms produce `vocabulary_term_unknown` (422), stale revisions produce
`vocabulary_conflict` (409). Internal identification may initially produce raw out-of-vocabulary
assignments under its existing policy; the review stage must disclose these rather than pretending
the model output was pre-approved. Strict/extension review cannot accept such a term until a valid
canonical trait exists. Wrong-modality/technical values are not admissible through vocabulary creation.

`review_tags` uses §8.3's batch size/payload, idempotency, approval, transaction and revision rules.
Each decision targets the existing normalized `(asset_id,namespace,value,source)` association;
duplicate or missing targets fail the batch. Only AI sources (including legacy AI) are reviewable
for mutation; manual/imported/file-derived targets return `protected_assignment` (403). A different
AI producer may be targeted explicitly, without claiming its identity as the reviewing producer.
Record the reviewer from trusted host configuration and keep the original assignment provenance.

`accept` records an agent decision/reason; it does not rewrite the source/model/confidence or set
asset confirmation. `reject` removes only that named AI association, refreshes FTS and records the
reason/before-state; matching manual or other-source associations survive. Reasons are required
nonempty text, bounded by the description limit. Read-only reports may flag protected assignments
for a human, but must never label them removed. Missing evidence should produce an explicit skipped
report entry, not a reject call. The approval diff names each producer and assignment affected.

Review is a meaningful history event even if acceptance keeps the tag row unchanged. Persist its
decisions in the shared versioned operation history; normal retention applies, so permanent review
history is not promised. Retry receipts follow §8.3 independently of undo retention. Readback exposes
retained review decisions with their reviewed revision; stale decisions are not claims about current
assignments. Rejection is not a global blacklist or a durable per-model suppression rule. A later
identify run may propose the tag again and must be reviewed as new output. Undo restores only the
touched association/review event when conflict checks permit it.

Vocabulary proposals have 1–100 changes, the same request byte bound, and explicit `kind`:

- `trait`: `{kind,namespace,value,scope,usage_note,teach}`. Use supported namespaces, existing media
  scope or null, and tag/description text bounds from §8.3. `teach` defaults false; enabling it is
  an explicit approval of future candidate-vocabulary expansion. Promote an existing raw tag when
  its canonical key matches; do not create a duplicate or overwrite an existing trait's guidance.
- `category`: `{kind,taxonomy_id,parent_id,label,reason}`. Taxonomy must already exist; parent must
  belong to it or be null for a root. Label uses tag text bounds, has no path separators, and path
  is computed by the server. Duplicate canonical sibling/path resolves to the existing node;
  incompatible requests fail. Category creation does not assign assets or create mapping rules.

Approval displays vocabulary scope and downstream teaching effects. Denial leaves vocabulary and
asset assignments unchanged. All approved changes in a proposal commit atomically through the single
writer, advance vocabulary revision and return real created/reused IDs in the job result. Follow with
a fresh vocabulary read and a separate asset write/review; no temporary IDs or names-as-identities.
No namespace/schema expansion, merges, renames, global rule edits or whole-library remapping here.
Nested tag grouping beyond trait promotion is a separate design; taxonomy categories are not silently
written into `tag.parent_id`.

For the ten-model pilot, save a before-state, call `identify_asset` only on those IDs, poll completion,
then read new assignments/revisions and perform this review flow. Internal identification writes are
not rolled back when a later review batch fails; report the completed and failed stages separately.
Reuse configured models, privacy/budget controls and app-owned job/cancel handling. The final report
must distinguish pre-existing tags, identification output, vocabulary proposals, accepted/rejected
associations with reasons, and final persisted tags/description.
