# Engine API — the loopback HTTP/JSON contract

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

   The file is created with user-only permissions and is **rewritten each session** (token rotates);
   it is removed on clean shutdown. A stale file (dead `pid`) is ignored by clients.
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
| `get_asset` | `GET /v1/assets/{id}` | full **asset detail** (§3) |
| `get_asset_preview` | `GET /v1/assets/{id}/preview?kind=` | streams PNG/JPEG (thumbnail/turntable/waveform) |
| — | `GET /v1/assets/{id}/file` | streams the on-disk source bytes (the MCP `resource_link` target) |
| `list_tags` | `GET /v1/tags?prefix=` | tags + counts |
| `add_tag` / `retag` | `POST /v1/assets/{id}/tags` | *mutating* |
| `list_collections` | `GET /v1/collections` | incl. the staging collection (§staging) |
| `apply_mapping` | `POST /v1/mapping/apply` | *mutating*; `{scope, taxonomy_id}` |
| `list_packages` | `GET /v1/packages` | filters: `publisher`, `category`; `cursor` |
| `unpack_package` | `POST /v1/packages/unpack` | *long-running* → returns `job_id` |
| `export_asset` | `POST /v1/export` | *side-effecting*; arbitrary **sandboxed** `dest_path` |
| `import_to_project` | `POST /v1/import` | *side-effecting*; into the **open project** (§5) |
| `register_project` | `POST /v1/projects/register` | plugin announces the open editor (§5) |
| `list_projects` / `get_active_project` | `GET /v1/projects` | registered/active editors |
| `stage_asset` / `list_staged` / `import_staged` | `GET/POST/DELETE /v1/stage`, `POST /v1/stage/import` | shared shortlist (§6) |
| `get_job_status` | `GET /v1/jobs/{id}` | progress/result for long ops |
| `get_server_info` | `GET /v1/server-info` | app + schema + API version |

`export` vs `import`: **`/v1/export`** writes a dependency closure to an arbitrary sandboxed path (no
editor needed); **`/v1/import`** routes into the **currently open** Unity project and triggers an
in-editor import (§5). Both reuse the **same dependency-closure resolution** described in
[`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §3.

---

## 3. Canonical entity shapes (defined here, referenced by MCP)

These shapes are the **single source of truth** for both planes. The MCP doc references them rather
than redefining, so summaries/details never diverge.

```jsonc
// AssetSummary — search results, staging, queue payloads
{
  "id": 4821, "uuid": "…", "name": "Beaver_LowPoly",
  "media_type": "model_3d",            // model_3d | texture | audio | image
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
  "license": "Unity Asset Store EULA", "attribution": "AcmeArt" }

// Job — get_job_status (job engine; see ARCHITECTURE.md §3.1)
{ "job_id": "imp_9f…", "kind": "import", "state": "running",   // queued|running|done|error
  "progress": 0.66, "result": null, "error": null }
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
  `already_imported` (409, advisory — see §5), `job_failed` (500).
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
  "can_execute_main_thread": true }
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

### 5.4 Dup-guard ("already imported")
Before enqueuing, the core checks `import_record` for `(asset_id, project_id)`:
- if present and the on-disk `dest_path` still resolves, it returns the asset in an
  **`already_imported`** advisory list in the `/v1/import` response (HTTP `200` with
  `{ "imported_existing": [...], "job_id": null }` when *all* assets are already present, or proceeds
  for the rest). Clients surface "already in project — re-import?" rather than silently duplicating.

### 5.5 Constraints
- Editor must be **open and polling**; imports run on Unity's **main thread**; large binaries travel
  by `file_url` (path/stream), never inline.
- **Dependency resolution is within a package** — cross-package GUID references are flagged for the
  user (GUIDs are reassigned per import). Same rule as
  [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §3.

---

## 6. Staging (the shared shortlist)

A **staging area** is a candidates tray shared across the GUI, MCP, and the Unity panel — "collect
the picks, then import them together," matching the agent loop in
[`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §2. It is modeled as the single **`is_staging=1`
collection** (see [`DATA-MODEL.md`](DATA-MODEL.md) §9) so it reuses all collection machinery.

```http
GET    /v1/stage                       → AssetSummary[]   (current shortlist)
POST   /v1/stage      { "asset_ids": [4821, 5099] }       (add)
DELETE /v1/stage      { "asset_ids": [5099] }             (remove; empty body clears)
POST   /v1/stage/import { "project_id": 2, "dest_subfolder": "Staged", "clear": true }
       → 202 { "job_id": "imp_a1…" }   (imports the whole tray via §5, optionally clears it)
```

Because it's one shared collection, an asset Claude stages over MCP appears in the GUI tray and the
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
