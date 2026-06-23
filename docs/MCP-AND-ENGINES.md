# MCP Server & Engine Integration

How AI clients (Claude, Claude Code, IDEs) and the Unity/Unreal editors find and retrieve assets from
the catalog. This is where "identify and retrieve into the engine" is delivered.

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
          │ AI clients             │                  │ Unity & Unreal editor plugins  │
          │ (Claude Desktop/Code,  │                  │ (deterministic browse + import)│
          │  Cursor, IDE MCP)      │                  └────────────────────────────────┘
          └────────────────────────┘
```

- **MCP** is the **AI control plane** — for LLM clients that reason and decide which assets to pull.
- **Local HTTP/JSON API** is the **deterministic integration plane** — for the engine plugins and any
  scripting. Plugins do **not** speak MCP.
- Both are thin frontends over the same `lodestash-core` operations — implemented once.

---

## 2. MCP server

- **SDK**: **`rmcp`** (official Rust SDK) — matches the core language, mature, `#[tool]` macros
  generate JSON Schema from function signatures, built-in stdio + Streamable HTTP transports.
- **In-process**, sharing the catalog DB/index handle — a desktop tool is single-user and local; a
  separate microservice would add a hop and a second failure mode for no benefit.
- **Transport**: **stdio by default** (zero network surface, lowest latency, exactly what Claude
  Desktop/Code expect). Optional **Streamable HTTP** later for multiple/remote clients (apply the same
  loopback + token + origin rules). The old HTTP+SSE transport is deprecated — do not build on it.
- **Registration**: `claude mcp add --transport stdio lodestash -- <lodestash --mcp>` for Claude Code;
  JSON config entry for Claude Desktop.

### Proposed MCP surface

**Tools (model-controlled):**

| Tool | Input | Output |
|---|---|---|
| `search_assets` | `query`, filters (`type`, `tags[]`, `category`, `format`, `min/max` size…), `limit`, `cursor` | paginated asset summaries (id, name, type, format, tags, thumbnail URI) |
| `semantic_search` | `query` (natural language), `type?`, `limit` | ranked summaries (embedding index) |
| `get_asset` | `id` | full metadata (paths, dims/poly-count/duration, formats, tags, collections, category, license) |
| `get_asset_preview` | `id`, `kind?` (thumbnail/turntable-frame/waveform) | **image content block** (base64+mimeType); audio → waveform image or `resource_link` |
| `list_tags` | `prefix?` | tags with counts |
| `add_tag` / `retag` | `id`, `tags[]` | updated asset *(mutating — confirm)* |
| `list_collections` | — | collections (id, name, count) |
| `apply_mapping` | `id`/`scope`, `taxonomy_id` | resulting category assignments |
| `list_packages` | filters (`publisher`, `category`), `cursor` | downloaded Asset Store packages (publisher, category, name, version, asset count) |
| `unpack_package` | `package_id`(s) | job id — unpacks + ingests one or many Asset Store packages *(long-running)* |
| `export_asset` | `id`(s), `dest_path`, `format?`, `with_dependencies?` | writes the selected asset(s) **+ their dependency closure** (a mesh plus its materials/textures), never the whole pack *(side-effecting — confirm + sandbox path)* |
| `get_job_status` | `job_id` | progress/result for long ops |
| `get_server_info` | — | app + schema version |

**Resources (application-controlled), via URI templates:**
- `asset://{id}` → metadata (JSON).
- `asset://{id}/preview` / `thumbnail://{id}` → image blob.
- `asset://{id}/file` → **`resource_link`** to the on-disk file (never inline large binaries).

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

Realistic agent loop: *"find low-poly stylized rocks under 5k tris, show the top 6, tag the 3 I pick
`level-3`, export them to `./Assets/Rocks/`"* → `semantic_search` → `get_asset_preview×N` → `add_tag` →
`export_asset`.

---

## 3. Unity integration

> **Two directions.** (1) Lodestash reads the Unity **Asset Store download cache directly off disk — no
> Editor required** — to browse, unpack, and ingest your owned library ([`INGESTION.md`](INGESTION.md)
> §2). (2) The editor plugin handles the *other* direction: importing a chosen catalog asset into the
> open project.

- **Plugin**: UPM C# package (`Editor/` assembly), `EditorWindow` UI, installed via Package Manager git
  URL.
- **How it talks**: a **thin client over the loopback HTTP API** — query search (by tag or AI), fetch
  thumbnails from the preview endpoint, and on "Import" pull **only the selected asset(s) plus their
  dependency closure** (Lodestash resolves `.meta`/prefab GUID references → the mesh + its materials +
  textures) into `Assets/<subfolder>/`, then `AssetDatabase.ImportAsset(...)` / `Refresh()` — **never
  the entire package**. Select one or many across different libraries in a single import. Drag-and-drop
  into Project/Hierarchy via the `DragAndDrop` API is a nice-to-have.
- **Reference patterns** (community, unofficial): CoplayDev/unity-mcp (C# UPM package + bridge over a
  local socket), IvanMurzak/unity-mcp, CoderGamester/mcp-unity. They confirm the "C# editor agent
  executes operations on external commands" pattern.
- **Constraints**: editor must be open/listening; asset import must run on Unity's main thread; large
  binaries travel by path/URL, not inline; **dependency resolution is within a package** (cross-package
  references are flagged for the user, since GUIDs are reassigned per import).

---

## 4. Unreal integration (Phase 2)

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
- **Engine plugins**: thin clients over the loopback HTTP API, importing into `Assets/` / `/Content`;
  reuse the same core operations as MCP.
- **One backend, two planes**: MCP for the AI, REST for deterministic tooling.

**Top risks**: editors must be open/listening; main-thread import constraints; Epic's Unreal MCP is
experimental and shifting; community Unity servers are unofficial. Mitigate by keeping MCP as the
AI-control plane and REST as the reliable engine plane over one shared backend.
