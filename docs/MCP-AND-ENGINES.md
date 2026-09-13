# MCP Server & Engine Integration

How AI clients (Claude, Claude Code, IDEs) and the **Unity editor** find and retrieve assets from the
catalog. This is where "identify and retrieve into the engine" is delivered. **Unity is the primary
integration target;** Unreal editor integration is planned for a later phase (§4).

> Reviewed for the analysis workflow 2026-09-10. The proposed
> [asset-analysis design](MCP-ASSET-ANALYSIS.md) extends the existing server; its exact tool/REST
> contracts live in [ENGINE-API §8](ENGINE-API.md#8-proposed-asset-analysis-contract).
> `get_asset_evidence` and `get_vocabulary` are available; the remaining analysis writes are planned.
> The clarified scope includes vocabulary reads, identification followed by association review,
> and approved trait/category proposals; ENGINE-API §8.5 owns those additional tool/REST pairs.

Verify MCP protocol specifics against the current spec (revision **2025-11-25**) and Claude specifics
against the `claude-api` skill before implementing.

---

## 1. Two control planes, one backend

```
                         ┌──────────── Lodestash core service layer ────────────┐
                         │   catalog · search · preview-render · export      │
                         └───▲───────────────────────────────▲───────────────┘
              MCP (stdio)    │ in-process            in-process│   HTTP (127.0.0.1 + token)
          ┌──────────────────┴─────┐                  ┌────────┴──────────────────────┐
          │ AI clients             │                  │ Unity editor plugin (primary)  │
          │ (Claude Desktop/Code,  │                  │ (Unreal: planned, later phase) │
          │  Cursor, IDE MCP)      │                  └────────────────────────────────┘
          └────────────────────────┘
```

- **MCP** is the **AI control plane** — for LLM clients that reason and decide which assets to pull.
- **Local HTTP/JSON API** is the **deterministic integration plane** — for the Unity plugin and any
  scripting. Plugins do **not** speak MCP; they speak REST. (Unreal plugin will use the same API when
  added.) Its full contract — endpoints, the token handshake, entity shapes, project registration and
  the import queue — lives in [`ENGINE-API.md`](ENGINE-API.md).
- Both are thin frontends over the same `lodestash-core` operations — implemented once. **Every MCP
  tool maps 1:1 onto a REST endpoint** ([`ENGINE-API.md`](ENGINE-API.md) §2), so the two planes can't
  drift. The canonical request/response **entity shapes** (asset summary/detail, package, job) are
  defined once in [`ENGINE-API.md`](ENGINE-API.md) §3 and referenced from here.

---

## 2. MCP server

- **SDK**: **`rmcp`** (official Rust SDK) — matches the core language, mature, `#[tool]` macros
  generate JSON Schema from function signatures, built-in stdio + Streamable HTTP transports.
- **Shared Rust services.** The desktop embeds the API. An LLM host starts a separate
  `lodestash-desktop --mcp` child for stdio: catalog reads are local, through a read-only connection
  that refuses any schema it would have to migrate, and every app-owned tool is forwarded whole to
  the app with the same catalog open ([ADR-0011](decisions/ADR-0011-headless-mcp-forwards-writes.md),
  amended by v1 plan S11; the wire rules are in [`ENGINE-API.md`](ENGINE-API.md) §2 and §4). stdout
  carries only JSON-RPC — logs go to stderr — and the process exits on stdin EOF. `LODESTASH_CATALOG`
  points it (or the app) at a catalog file other than the default.
- **Transport**: **stdio by default** (zero network surface, lowest latency, exactly what Claude
  Desktop/Code expect). Optional **Streamable HTTP** for multiple/remote clients (same loopback +
  token + origin rules): bound to a **stable configurable port (default 4319)** — unlike the engine
  API's ephemeral port — since MCP clients configure a URL by hand. The Settings → MCP card
  ([`UI-SPEC.md`](UI-SPEC.md) §11.5) toggles this transport, shows endpoint + rotating token, and
  gates tools by **literal per-tool allow-list**; side-effecting
  import/export tools stay confirm-gated per call regardless. The old HTTP+SSE transport is
  deprecated — do not build on it.
- **Registration**: configure the host to spawn the built `lodestash-desktop` executable with
  `--mcp`; see [USER-GUIDE](USER-GUIDE.md#using-an-llm-through-mcp). The branch runs without a window.
  Logs **must** go to stderr because stdout carries the protocol; explicit routing and a spawned
  protocol test are required by the new plan. Do not configure Vite's port as an MCP endpoint.

> **Implementation status (code inspected 2026-09-10).** There are 23 registered tool names in
> `crates/lodestash-mcp/src/lib.rs`, matching `AGENT_OPERATIONS` in
> `crates/lodestash-api/src/lib.rs`. They include semantic search, stored-tag reads, previews,
> single-tag writes and identify jobs. `suggest_tags` currently returns stored tags/confidences;
> it does not generate new proposals. The table below mixes shipped operations with broader target
> aliases (such as `retag` and `get_active_project`), which are not separate registered tools.
> Library-scoped enumeration, batch reads, bounded evidence manifests and attributed batch writes
> are the explicit additions in ENGINE-API §8. Registration in the actual host is still required.

### Proposed MCP surface

**Tools (model-controlled):**

| Tool | Input | Output |
|---|---|---|
| `search_assets` | `query`, filters (`type`, `tags[]`, `category`, `format`, `min/max` size…), `limit`, `cursor` | paginated asset summaries (id, name, type, format, tags, thumbnail URI) |
| `semantic_search` | `query` (natural language), `type?`, `limit` | ranked summaries (embedding index) |
| `list_libraries` | `limit?`, `cursor?` | libraries (id, name, kind, asset_count), cursor-paged ([ENGINE-API §8.1](ENGINE-API.md#81-discovery-selection-and-detail)) |
| `list_assets` | `source_id`, `package_id?`, `media_type?`, `status?`, `tag_state?`, `selection?` (ordered/random), `limit?`, `cursor?` | one library walked completely, or sampled — search ranks, this visits (§8.1) |
| `get_assets` | `asset_ids` (1–100), `source_id?` | full detail in the order asked, plus the ids that could not be returned (§8.1) |
| `find_similar` | `id`, `limit?` | ranked summaries most similar to the given asset (cosine over `vec_asset`) — "more like this" for agents |
| `get_asset` | `id` | full metadata (paths, dims/poly-count/duration, formats, tags, collections, category, license) |
| `get_asset_preview` | `id`, `kind?` (thumbnail/turntable-frame/waveform) | **image content block** (base64+mimeType); audio → waveform image or `resource_link` |
| `get_asset_evidence` | `id`, `kind?`, `max_dimension?`, `views?` | shared manifest plus one available preview image block and an `asset://{id}/file` link; byte consent can return `denied` |
| `list_tags` | `prefix?` | tags with counts |
| `add_tag` / `retag` | `id`, `tags[]` | updated asset *(mutating — confirm)*; written as `ai:legacy-agent` |
| `write_tags` | `request_id`, `assets[]` (revision, producer, mode, tags, description) | job `awaiting_approval` → receipt; same contract as `POST /v1/tags/batch` ([ENGINE-API §8.3](ENGINE-API.md#83-attributed-result-writes)) |
| `list_collections` | — | collections (id, name, count) |
| `apply_mapping` | `id`/`scope`, `taxonomy_id` | resulting category assignments |
| `identify_asset` | `id`(s), `tiers?` | runs/refreshes identification (tiers 0–4, [`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md) §3) for the asset(s) → `job_id` *(long-running)* |
| `suggest_tags` | `id` | proposed facet tags + confidences for review — read-only; nothing is written until accepted via `add_tag` |
| `list_packages` | filters (`publisher`, `category`), `cursor` | downloaded Asset Store packages (publisher, category, name, version, asset count) |
| `unpack_package` | `package_id`(s) | job id — unpacks + ingests one or many Asset Store packages *(long-running)* |
| `export_asset` | `id`(s), `dest_path`, `format?`, `with_dependencies?` | writes the selected asset(s) **+ their dependency closure** (a mesh plus its materials/textures) to an arbitrary **sandboxed** path, never the whole pack — **no editor required** *(side-effecting — confirm + sandbox path)* |
| `import_to_project` | `id`(s), `dest_subfolder?`, `with_dependencies?`, `project_id?` | imports the asset(s) + dependency closure **into the open Unity project** (vs. `export_asset`'s arbitrary path). Enqueues onto the shared import queue the running plugin drains on Unity's main thread (§3). Returns a `job_id`; `409 project_not_registered` if no editor is listening *(side-effecting — confirm)* |
| `register_project` | `project_path`, `unity_version`, `can_execute_main_thread` | registers the open editor so the model knows where "into Unity" means *(usually called by the plugin, exposed for completeness)* |
| `list_projects` / `get_active_project` | — | registered editors + which is active |
| `stage_asset` / `list_staged` | `id`(s), `project_id?` / `project_id?` | add to / read a **project's staging tray** (one `is_staging` collection per project, default: active — §2 agent loop) |
| `import_staged` | `dest_subfolder?`, `project_id?`, `clear?` | import the whole staged shortlist into the open project via the import queue |
| `get_job_status` | `job_id` | progress/result for long ops |
| `cancel_job` | `job_id` | requests cancellation of a running job (idempotent; job flips to `cancelled`) |
| `get_server_info` | — | app + schema + API version |

**Resources (application-controlled), via URI templates:**
- `asset://{id}` → metadata (JSON).
- `asset://{id}/preview` / `thumbnail://{id}` → image blob.
- `asset://{id}/file` → **`resource_link`** to the on-disk file (never inline large binaries).
  *As built (v1 plan S12b):* `resources/read` of it answers with the bytes inline up to 4 MiB,
  through the same resolver and byte policy as the REST file route — an in-pack asset included,
  pulled and verified without unpacking ([`ENGINE-API.md`](ENGINE-API.md) §8.2). It is the only
  template served; `asset://{id}` and the preview URIs above are not.

**Prompts:** a few canned workflows ("find and stage assets matching a brief"; "audit a collection for
missing tags/licenses").

### Returning previews — the rule
MCP content blocks: `text`, `image` (base64 `data`+`mimeType`), `audio`, `resource_link` (URI only),
embedded `resource`.
- **Images / 3D**: return an **`image` block** (server-rendered thumbnail/turntable frame, PNG/JPEG,
  ≤~1–2 MP) so Claude can *see* the asset — plus a `resource_link` to the real file.
- **3D models**: never inline `.glb`/`.fbx`; preview image + `resource_link`.
- **Audio**: a generated **waveform image** (directly viewable by Claude) + `resource_link` to the file
  (or an `audio` block where the client supports it).
- **Always pair a viewable preview with a `resource_link`** — the agent reasons over the preview; the
  consumer (engine plugin/export) fetches bytes by path/URI.
- **A missing preview is not an error.** `get_asset_preview` returns what it has — the image when
  there is one, the `resource_link` when a real file exists, and always a small JSON block naming
  the asset — so a caller with no image still has something to reason from. It used to answer
  `Err` before adding anything, which meant *no* content at all.

*(Fixed 2026-08-29, and it was total: previews resolved for **0 of 60** sampled assets of every
type, so the agent plane could not see a single asset in the library. Two causes. The resolver was
handed only a **source path**, and the summary query fabricates `root_path/rel_path` for an in-pack
asset — a file that is not there — so every one of the 5,963 in-pack assets failed; and a
**material** has no pixels of its own, so decoding its `.mat` was never going to produce an image.
`ServerState::preview_subject` now resolves the whole subject from the catalog — verified on-disk
path, in-pack locator for a surgical pull, albedo texture for a shader ball — and the desktop's
renderer uses the same in-pack scratch the grid's own thumbnail route uses, so the two cannot
disagree. Same sample afterwards: **57 of 60**, the remainder being assets whose render genuinely
fails, which now answer with metadata instead of an error.)*

> **Desktop MCP settings** (UI-SPEC §16, FEATURES.md §6.5): the app's MCP card exposes a **literal
> per-tool allow-list** — one toggle per tool in the table above — plus the enable flag, port
> (default **4319**), and rotating token. Toggling a tool gates it on **both** planes (MCP↔REST 1:1);
> import/export tools stay per-call confirm regardless. Names in the UI must match this canonical
> inventory exactly.

Realistic agent loop (into the open editor): *"find low-poly stylized rocks under 5k tris, show the
top 6, stage the 3 I pick and tag them `level-3`, then import them into my project under `Rocks/`"* →
`semantic_search` → `get_asset_preview×N` → `stage_asset` → `add_tag` → `import_to_project` →
`get_job_status`. The same loop ending in `export_asset` writes to an arbitrary path instead (no
editor needed). The whole entity/endpoint contract for these is in [`ENGINE-API.md`](ENGINE-API.md).

---

## 3. Unity integration (primary)

> **Two directions.** (1) Lodestash reads the Unity **Asset Store download cache directly off disk — no
> Editor required** — to browse, unpack, and ingest your owned library ([`INGESTION.md`](INGESTION.md)
> §2). (2) The editor plugin handles the *other* direction: importing a chosen catalog asset into the
> open project.

- **Plugin**: UPM C# package (`Editor/` assembly), `EditorWindow` UI, installed via Package Manager git
  URL.
- **How it talks**: a **thin client over the loopback HTTP API** ([`ENGINE-API.md`](ENGINE-API.md)) —
  it reads the handshake file for port+token, queries search (by tag or AI), fetches thumbnails from
  the preview endpoint, **registers the open project**, and imports **only the selected asset(s) plus
  their dependency closure** (Lodestash resolves `.meta`/prefab GUID references → the mesh + its
  materials + textures) into `Assets/<subfolder>/` — **never the entire package**. Select one or many
  across different libraries in a single import. Drag-and-drop into Project/Hierarchy via the
  `DragAndDrop` API is a nice-to-have.

### The closed round-trip — GUI **and** Claude land assets in the *open* editor

The plugin still does **not** speak MCP. Instead, **MCP and the plugin meet at one shared place: the
import queue** in [`ENGINE-API.md`](ENGINE-API.md) §5. That single rendezvous is what lets an AI agent
(MCP) and the desktop GUI both drop an asset into the same open project:

```
  Claude (MCP import_to_project)  ─┐
  Desktop GUI "Import"            ─┼──▶  lodestash-core  ──▶  per-project import queue
  Unity panel "Import"           ─┘        (resolves dep closure, stages bytes,
                                            checks dup-guard, returns job_id)
                                                          │
                          long-poll GET /v1/import-queue  ▼
                                   Unity plugin  ──▶  main-thread import
                                   (AssetDatabase.ImportAsset / Refresh)
                                                          │  POST …/ack {dest_path, guid}
                                                          ▼
                                   core writes import_record  ──▶  job = done
                                   (MCP get_job_status / GUI both observe it)
```

Step by step:
1. On panel open, the plugin **`register_project`** (project path, Unity version) → core marks it
   active.
2. A request from **any** caller (`import_to_project` over MCP, or `POST /v1/import` from the GUI/panel)
   resolves the **dependency closure**, stages the bytes, runs the **dup-guard** check
   (`import_record` on `(asset, project)`), and appends an import request to that project's queue —
   returning a `job_id` immediately.
3. The plugin's **long-poll loop** drains the queue on Unity's **main thread**, writes files under
   `Assets/<subfolder>/`, calls `AssetDatabase.ImportAsset` / `Refresh`, and **acks** the real
   in-project path + GUID.
4. The core persists an **`import_record`** (provenance) and flips the job to `done`; MCP
   `get_job_status` and the GUI both see completion identically.

**If no editor is registered**, `import_to_project` returns `409 project_not_registered` with a clear
"open the Lodestash panel in Unity" message — and the agent **falls back to `export_asset`** to a path.
This is exactly how "do it from Claude" in [`USER-GUIDE.md`](USER-GUIDE.md) Step 6 works.

- **Reference patterns** (community, unofficial): CoplayDev/unity-mcp (C# UPM package + bridge over a
  local socket), IvanMurzak/unity-mcp, CoderGamester/mcp-unity. They confirm the "C# editor agent
  executes operations on external commands" pattern — the import-queue here is the same idea with
  Lodestash core as the broker.
- **Constraints**: editor must be open/listening; asset import must run on Unity's main thread; large
  binaries travel by path/URL, not inline; **dependency resolution is within a package** (cross-package
  references are flagged for the user, since GUIDs are reassigned per import). A **render-pipeline
  mismatch** (pack targets Built-in, project runs URP/HDRP) is surfaced as an import advisory, not an
  error — see [`ENGINE-API.md`](ENGINE-API.md) §5.4 and [`DATA-MODEL.md`](DATA-MODEL.md) §12.

### 3b. Two MCP topologies (both supported)

`import_to_project` works the same regardless of **where the MCP client runs**:

1. **Claude Desktop / a remote IDE drives a separate editor.** The user has the Unity panel open
   (registered); Claude reasons over previews and calls `import_to_project`; the asset lands in that
   editor via the queue above. The MCP server is the local Lodestash app (stdio).
2. **Claude Code / Cursor running *in the Unity project folder* (Unity-side MCP client).** A developer
   working alongside the editor registers the Lodestash MCP server in their project
   (`claude mcp add --transport stdio lodestash -- <lodestash --mcp>`) and asks it to find + import
   assets into the very project they're in. **No new surface** — it's the same `import_to_project`
   flow against the already-open editor; only the client's *placement* differs. Use **stdio** for this
   single-developer case; reserve **Streamable HTTP** for when multiple/remote clients must reach one
   running instance (same loopback + token + origin rules — §5).

In both, the plugin remains a pure REST client; MCP only ever enters through the shared import queue.

---

## 4. Unreal integration (planned, later phase)

- **Plugin**: `.uplugin` with a C++ editor module (Slate tab) and/or **Python Editor Scripting**;
  import into `/Content` via `UAssetImportTask` / `FAssetToolsModule` / `EditorAssetLibrary`.
- **How it talks**: same shape — a Slate tab as a **thin client over the HTTP API**.
- **Reference**: Epic now ships an **official, experimental "Unreal MCP" inside the editor** (UE 5.8) —
  an in-editor MCP server any client can drive over local HTTP (toolsets from `UToolsetDefinition` C++
  / `unreal.ToolsetDefinition` Python). Treat as a moving target. Community: kvick-games/UnrealMCP,
  flopperam/unreal-engine-mcp.
- **Constraints**: experimental APIs; imports run on the editor/game thread; scope to browse+import.

---

## 5. Security, async, versioning

**Loopback security:**
- Bind the HTTP API to **`127.0.0.1` only** (never `0.0.0.0`).
- Require a **per-session bearer token** the GUI generates and hands to plugins (written to a
  user-readable file the plugin reads, or shown in the UI to paste); validate `Origin`/host headers —
  blocks other local processes and DNS-rebinding-style attacks.
- **stdio MCP has no network surface** (child process over pipes) — a security argument for stdio as the
  default. If Streamable HTTP MCP is added, apply the same loopback + token + origin rules.
- Treat all tool inputs as untrusted: validate and **sandbox `export_asset`/`apply_mapping` paths**
  (no traversal), rate-limit, and **confirm mutating/destructive ops server-side** (don't rely solely
  on the client's human-in-the-loop).

**Async / long-running ops** (export batches, re-indexing, embedding): return a **job id immediately**
and expose `get_job_status`; emit MCP progress + `list_changed`/resource-update notifications. Keep each
`tools/call` response fast. (The spec's task-augmented execution is an alternative once stable.)

**Versioning:** pin the negotiated MCP protocol version (support 2025-11-25 with graceful fallback);
version your own surface (`catalog.v1.*` tools / `/v1/` REST) so plugins/clients detect compatibility.
Additive changes are safe; breaking changes get a new version. `get_server_info` reports app + schema
version; plugins check on connect and degrade gracefully.

---

## 6. Bottom line

- **MCP server**: `rmcp`, in-process, stdio default — the surface in §2, previews as `image` blocks +
  `resource_link`s, never inline large binaries.
- **Engine plugins**: **Unity** is primary — a thin client over the loopback HTTP API
  ([`ENGINE-API.md`](ENGINE-API.md)), importing into `Assets/`; Unreal plugin is planned for a later
  phase. Both reuse the same core operations as MCP.
- **One backend, two planes**: MCP for the AI, REST for deterministic tooling — **1:1 operations**, so
  they never drift.
- **The Unity round-trip closes at the import queue** (§3): GUI and Claude (`import_to_project`) both
  land assets in the *open* editor; the plugin executes on the main thread and acks back an
  `import_record` (provenance + dup-guard). Falls back to `export_asset` when no editor is registered.

**Top risks**: editors must be open/listening; main-thread import constraints; community Unity
servers are unofficial; Epic's Unreal MCP is experimental and shifting (and Unreal support is a
later-phase addition anyway). Mitigate by keeping MCP as the AI-control plane and REST as the
reliable engine plane over one shared backend.
