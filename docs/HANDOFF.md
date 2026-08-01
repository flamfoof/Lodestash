# Handoff — picking up Lodestash

Working notes for whoever (or whatever) continues the build. Unlike the rest of `docs/`, this file
is **not** a design spec — it's the state of play, the decisions already settled, and the traps.
The design docs still win on any question of what the software *should* do.

Last updated: **2026-08-01**, at `afadc97`.

---

## 1. Orientation

Read in this order:

1. [`../CLAUDE.md`](../CLAUDE.md) — the design-conformance rule. `docs/` is the source of truth; code
   conforms, and any divergence gets written back into the doc **in the same change**.
2. [`KANBAN.md`](KANBAN.md) — the board. Every card touched carries a detailed *shipped / remaining*
   note; read the card before starting its work.
3. The doc owning your area — [`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md),
   [`AI-PIPELINE.md`](AI-PIPELINE.md), [`DATA-MODEL.md`](DATA-MODEL.md),
   [`INGESTION.md`](INGESTION.md), [`TECH-STACK.md`](TECH-STACK.md) (deps + resolved spikes),
   [`UI-SPEC.md`](UI-SPEC.md).

Sanity check before starting: `cargo test --workspace` and `cargo clippy --workspace --tests`
(both green at handoff), then `cargo fmt --all`.

---

## 2. Where the build is

**Search-with-AI is complete against its design**, and the grid is fully visual.

| Modality | State |
|---|---|
| Image / texture / sprite | Decoded thumbnails + SigLIP embeddings, on-disk **and** inside `.unitypackage`s |
| 3D models | FBX/OBJ (ufbx) + glTF/GLB (three-d-asset) → CPU multiview render → SigLIP; rendered grid thumbnails |
| Audio | CLAP joint space (512-d), fused with the visual space by RRF; peak-waveform grid thumbnails |

Every asset also carries **technical facets** — triangles, pixel dimensions, audio duration —
extracted by a `metrics_dirty` drain (M9, [`DATA-MODEL.md`](DATA-MODEL.md) §14) that runs after every
scan whether or not the AI stack is installed. They show in the inspector, drive the ⚑ Filters
Triangles chips, and are parseable from a query (*"under 5k tris"*, *"under 500ms"*).

Plain-language queries are split into semantic text + structural filters before search. The whole AI
stack is one-click installable on win-x64 (fetches the ONNX Runtime *and* both models, ~645 MB).

**The agent plane is open** (P5, [`ENGINE-API.md`](ENGINE-API.md) + [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md)).
`lodestash-api` binds an ephemeral loopback port at launch, writes `engine.json`, and serves `/v1/`
behind a bearer token + loopback `Host`/`Origin` guard. `lodestash --mcp` runs the same operations
over rmcp stdio. **Twelve** operations exist on **both** planes, and a parity test fails the build if
one plane grows a name the other lacks:

| Tool / route | Note |
|---|---|
| `get_server_info` · `search_assets` · `get_asset` | |
| `semantic_search` (`?mode=ai`) | 501 only while no model is loaded |
| `get_asset_preview` | image block + `resource_link`; PNG from the grid's own thumb cache |
| `find_similar` · `list_packages` | |
| `register_project` · `list_projects` · `import_to_project` | **P5 Unity round-trip** (2026-07-28); import is side-effecting |
| `export_asset` · `get_job_status` · `cancel_job` | **side-effecting**, see below |

Both planes call the *same* `ServerState` methods the REST handlers call — that is what makes the 1:1
rule structural rather than a convention. `GET /v1/assets/{id}/file` and the plugin transport
(`GET /v1/import-queue`, `POST /v1/import-queue/{req}/ack`) stay **REST-only** by design.

**Side-effecting ops are gated on the human.** `export_asset` **and `import_to_project`** return a job
in state `awaiting_approval` and write nothing until someone approves the desktop modal (UI-SPEC
§11.6b); denial cancels it. Export's `dest_path` must be absolute, `..`-free, and outside every
library source root. This is MCP-AND-ENGINES §5 ("confirm server-side, don't rely on the client's
human-in-the-loop") made literal — keep it that way for every mutating op that follows.

**The per-tool allow-list gates both planes** (card 178, 2026-07-28). One list on `ServerState`
(`allows`/`set_allowed`, default all-on, config `mcp_disabled_tools`); REST checks it in a `gate`
middleware after auth, MCP checks it per tool — so a disabled tool is off 1:1. **The Unity import
round-trip is in** (ENGINE-API §5.6): approval **enqueues**; the plugin drains `GET /v1/import-queue`
and its **ack** writes the durable `import_record` with Unity's real path+guid (the dup-guard is
`UNIQUE(asset_id, project_id)`). Exercised headless by a simulated plugin. The two unbuilt pieces are
the real **UPM C# plugin** (needs a Unity install to verify) and the allow-list **toggle UI**.

**This machine's catalog is embedded** (2026-07-22): 1983 SigLIP vectors (1723 `model_3d`, 260
`image`) + 84 CLAP vectors, from ~2274 assets. The 207 rows that were `embedding_dirty` are `.mat`
materials; **fixed 2026-08-01** — the recorded reason ("no material renderer yet") was only half of
it. `visual_assets_needing_embedding` filtered `mt.code IN ('image','texture','sprite','model_3d')`,
so materials were never *queued*, and wiring a renderer alone would have been dead code. The queue
now admits `material` and the drain encodes the shader ball (opaque background — SigLIP sees RGB and
alpha composites to black). Unresolved albedo still renders a flat ball, which beats a permanently
dirty row.

**Local tagging runs** (P4, [`AI-PIPELINE.md`](AI-PIPELINE.md) §2). Settings ▸ AI · IDENTIFY picks
an Ollama model (and optionally a captioning model) and starts a pass in the job tray: manual,
cancellable, resumable, because a full library is hours of CPU on this box. Two stages, forced by
measurement — `moondream` answers facet-JSON requests with arrays of numbers but captions correctly
in prose, so the VLM captions and a text model writes the facets; the caption becomes
`asset.description` and is FTS-indexed. Installed here: `qwen3:8b` (facets) + `moondream:latest`
(captions). Tags are written under `source='ai'`, so a re-run never touches the user's own.

**P4 is now most of the way in (2026-07-26).** Four slices landed this session:
- **Accept/reject → `confirmed`** — the last lifecycle state is reachable. `Catalog::remove_asset_tag`
  (reject one trait, re-indexes FTS) + `set_confirmed` (human-set status; un-confirm recomputes the
  earned status inline). Inspector TRAITS chips get a `×` reject and a Confirm toggle (tagged assets only).
- **Traits screen** (`Screen::Vocabulary`, UI-SPEC §9.1) — NEW TRAIT card, ✦ PROMOTE EXISTING TAGS,
  trait cards (6-colour palette, ✦ teaching / ⏸ paused, ⌕ find, ✕ un-promote). The **teaching**
  contract is wired end to end: `spawn_tagging` loads `teaching_traits` and injects them into the
  facet prompt, scoped per modality (`OllamaTagger`/`OpenAiTagger::with_teaching`). Schema needed no
  migration — the trait columns were already in MIGRATION_05. (The Mapping tab beside it was stubbed
  then; **shipped 2026-07-27**, see the P4-tail block below.)
- **Audio tagging** — **CLAP zero-shot** (`lodestash_ai::clap_tags`), no new model: scores a clip
  against a game-audio label vocabulary in the shipped CLAP joint space, per-facet temperature-scaled
  softmax over `MIN_PROB`. Own `tags_dirty` queue + `drain_audio_tags`, runs before the VLM drain
  (which refuses audio). YAMNet/PANNs + Whisper remain the heavier future path.
- **API tagger** — `lodestash_ai::openai::OpenAiTagger`, the **OpenAI-compatible** vision shape, so
  one impl reaches OpenAI/**Gemini (free tier)**/Groq/OpenRouter/local servers. Chosen over
  Claude-native after a cost review (Claude API is dear; a chat membership can't be used
  programmatically). `drain_api_tags` single pass; `spawn_tagging` routes API-vs-Ollama on config.
  Settings ▸ **AI · IDENTIFY (CLOUD)** card (endpoint/model/key, scoped Run buttons, cost warning);
  key never logged. Audio never uploaded. AI-PIPELINE §5 rewritten accordingly. New user docs:
  [`MODEL-GUIDE.md`](MODEL-GUIDE.md) (which model by modality) and
  [`SELF-HOSTED-PROVIDER.md`](SELF-HOSTED-PROVIDER.md) (a **separate/future** spec — a hosted
  Lodestash endpoint is just another OpenAI-compatible URL the cloud card already accepts).

**P4's tail landed almost whole (2026-07-27).** Six slices this session:
- **Cloud tagging reliability & cost controls (card 170, DONE)** — the free-tier path is now usable.
  `OpenAiTagger::try_tag` classifies each call (`ApiError::Throttled{retry_after}` vs `Hard`); the
  drain owns backoff+resume (honour `Retry-After`, else exp 2/4/8/16 s + jitter, cancel-polled). **The
  core bug is fixed**: a failed call no longer wrote an empty tag set that cleared the asset "done, no
  tags" — a throttled/failed asset stays `tags_dirty` for the next run. Hard error → local-Ollama
  fallback per asset (latches `api_dead`); no fallback → leave dirty + stop. Plus an **endpoint-preset
  picker** (`openai::PRESETS` Local/Gemini/Groq/OpenRouter + `is_free_endpoint`), a **Test connection**
  preflight, a **budget guard** (`api_max_per_run` cap + a paid-endpoint confirm), and **key-at-rest**
  in **Windows Credential Manager** (`config::secret` via the new `keyring` dep; blanked on disk,
  plaintext fallback off-Windows). AI-PIPELINE §5.1 governs it.
- **SigLIP zero-shot image/3D tagger (card 171)** — `zeroshot::score_facets` extracted as the shared
  per-facet softmax core (CLAP + SigLIP both use it now); `image_tags::ImageZeroShot` mirrors
  `clap_tags`, scoring a preview (or a **3D asset's stored multiview embedding** via `tag_vector`) against
  a base vocabulary **plus the user's teaching traits**. Wired as a **union** pass into both drains
  (`drain_tags` + `drain_api_tags`) over the preview each already decodes — merged before
  `validate`+`set_tags` (source `ai`, one write). No new dep. Card 171 remainder = **WD-EVA02 only**
  (blocked on the model-registry work in card 173).
- **Mapping library (card 172, DONE end-to-end)** — the schema tables already existed; this session
  added the engine + UI + facet + starter pack. `Catalog::apply_mapping` is deterministic + non-destructive
  (drops non-pinned rule rows and regenerates; **pinned survives**; rules run `priority ASC, id`, first
  match claims an asset via `NOT EXISTS`, so order-independent; raw tags untouched). Match types
  `exact_tag`/`alias`/`media_type`/`glob_path` each in one SQL statement (SQLite `GLOB` for paths);
  `regex`/`embedding_knn` skipped with a warning. The Traits screen's **⇢ Mapping** tab is real
  (create library, add categories/rules, ▶ Apply); a **CATEGORY** facet in ⚑ Filters filters the grid
  (`all_asset_categories` → per-sync map, hard structural pre-filter kept out of `passes` so its 20
  tests stayed put); and a **✦ Load starter library** button seeds a curated game-asset taxonomy. This
  also closes the P3 mapping-determinism test card. DATA-MODEL §4 governs it.
- **Model-download hardening (card 173)** — `fetch_all`/`download_ort` verify bytes-written against
  `Content-Length` and reject a **truncated** file (the silent "looks installed, panics at ort load"
  bug); a **Remove models** button (`models::remove_models`) deletes SigLIP+CLAP for a re-fetch.

*P4 still open*: WD-EVA02 dense tagger (needs the GPU-tier model registry — card 173) + YAMNet/PANNs +
Whisper; Claude-native API impl + Batches; the optional `regex`/`embedding_knn` mapping match types;
`+ trait` quick-add from the inspector; trait colour on inspector chips.

**The dependency graph is harvested and export honours it** (P2, [`DATA-MODEL.md`](DATA-MODEL.md) §10).
A third drain flag (`deps_dirty`, M10) walks each asset's `.meta` sidecar for its own GUID and a
material's `m_Texture` refs; unresolved refs are kept as `missing` edges and late-bound after every
drain, so index order doesn't matter. `dependency_closure` backs both `AssetDetail.dependencies` and
`export_asset(with_dependencies)` — the preview is the export. On this library: 1580 GUIDs, 200
resolved edges, 2 missing. Mesh → material comes from a second pass over `.prefab` files: a stock
FBX embeds its materials, so the model declares nothing and the prefab is the only place the binding
is written down. Prefabs are read as *evidence*, never catalogued (`prefab` isn't a `media_type`),
and the pass rebuilds `source='unity_prefab'` edges wholesale — 1228 resolved on this library, so
`SM_Gen_Env_Ground_Grass_Large_01` now returns material + 3 textures. **Coverage caveat**: an asset
outside a Unity project has no `.meta`, so no GUID and no edges at all — step 2's naming heuristic
(DATA-MODEL §10) is what would reach those.

Three UI surfaces read the graph: the inspector's **LINKED FILES** panel (UI-SPEC §13.6, both
directions, click to select, missing refs called out), a **painted** link badge on cards with
outbound edges (the bundled fonts have no chain glyph — U+26AD is tofu, like the dropped ◉), and
**⚑ Filters ▸ LINKS**. The relink panel from §13.6 is not built.

**The grid shows game assets only.** A file with no `media_type` (`.url`, `.pdf`, `.txt`,
`.prefab`, `.cs`) is dropped by the crawl — the catalog never held it anyway, so it was a
grid-only ghost with no status, no facets and no embedding. Grid total went 3768 → 1728, which
is exactly the catalog's on-disk row count. Prefabs are still read for the dependency graph.
The category tabs are now five: Materials split out of Models.

**The status lifecycle is live** (P3, [`DATA-MODEL.md`](DATA-MODEL.md) §8). `status` is *derived*,
not hand-set: `Catalog::promote_statuses` reads the ingest state already on the row — facets
extracted ⇒ `indexed`; an embedding **and** at least one tag ⇒ `identified`; embedding re-dirtied
(file changed) or no tags ⇒ back to `indexed` — and `confirmed` stays human-set. All three writer paths (scan persist, package
persist, backlog drain) funnel through one `drain_promote_resync`: drain facets, drain vectors,
promote, then re-read counts + identities and send them, because a drain is no longer count-neutral.
Since tags became part of the definition (2026-07-23) the meter reads the *tagged* count — 19
on this machine until a full Identify pass runs, which is the honest number.

**Materials, packages and the Unity cache.** Materials render as **shader balls** in the grid
(`lodestash_ingest::material` → the same CPU rasteriser as model thumbs), wearing the albedo the
dependency graph resolved — the first thing that graph pays for outside export. `read_package_meta`
harvests a pack's own `.icon.png` (every Asset Store pack has one), any embedded UPM `package.json`
(version / displayName / license) and any bundled licence file, identified by its **body** rather
than its filename; it runs at extraction, not browse, and early-exits, so a 20 GB pack is not
re-inflated for a 2 KB file. Asset Store discovery now **infers a moved cache** from the library
sources already added — this machine's cache is on `G:`, which is why the Unity Assets screen was
empty (env var + per-OS default only find a cache where Unity put it).

**Unity Assets works end to end** (2026-07-24, FEATURES.md §5). The pieces that existed separately
are joined, and each step was looked at in the running app:

| Step | State |
|---|---|
| **§5.2 Import** | **+ Import packages…** picks a folder, scans it off-thread (`crawler::crawl` + the extension test — no second walk), lists packs **grouped by the folder holding them** (the publisher/category tree in a store cache), and imports the ticked ones as one **pinned** `package` row apiece. Nothing copied, nothing unpacked. |
| **§5.3 Landing** | Every catalog pack, with **Open →** and the Add-to-Library pin. |
| **§5.4 Browse** | Contents as a **folder tree** (`PendingPackage::folders`) with per-folder counts + All/None, a type-coloured dot per row, leaf filenames. Groups default open at ≤8 folders; the bar grows to 360 px and is drag-resizable. |
| **§5.4 Preview** | Clicking a filename pulls **that one payload** to an OS-temp scratch file and opens it in the grid's own `quick_look_content` — image zoom/pan, 3D viewport, audio transport. No catalog row, no grid entry, no workspace write. |
| **§5.5 Pin** | A pinned pack rehydrates on startup **regardless** of the `cache_extracted` toggle; unpinning pulls its assets out of the grid and resets the filter. |

Two things make that work in a session that never crawled the pack. `Catalog::list_package_entries`
reads the persisted locators back, and a pack the catalog knows only by *name* (87 of the 88 rows
here) **reads its manifest on demand**, off-thread with a job row, writing the locators back so every
later open is a catalog read. Browsing and previewing are read-only, so a pack outside every library
source opens fine — only *extraction* needs a source row to persist into, and both extract paths say
so at that point (`NO_SOURCE`).

Two real defects surfaced under that work, both fixed:

- **`asset_file.workspace_rel` was mis-assigned.** The package persist zipped the record list against
  the *file* list, but `crawl_records` drops unclassified files — one stray `.txt` shifted every
  location after it onto the wrong asset. `crawl_records_in_workspace` resolves the location in the
  same pass that builds the record. Rows written before this fix are NULL, so a pinned rehydrate on
  this machine still needs one fresh extraction to demonstrate.
- **A pack could be two packs** (M13). `package` was keyed `UNIQUE(source_id, package_path)`, so an
  archive reachable from both a folder source and the cache source got two rows — the pin, the asset
  count and the locators landing on different ones. A pack *is* the archive: M13 merges duplicates
  onto the row carrying data, repoints assets and locators, and puts a UNIQUE index on
  `package_path`. Verified on the real catalog: 88 rows → 87, 546 assets still linked, both locator
  sets intact.

**In-pack assets — analyze & identify a pack without extracting it** (2026-07-25, M14,
[`DATA-MODEL.md`](DATA-MODEL.md) §7 "In-pack assets", FEATURES.md §5). This is the session's spine.
The old model gated everything on a full workspace extraction; now a pack is a first-class,
identifiable library **before any unpack**. Design was written first (spec commits `256c66c`), then
built.

- **One asset type, three byte-sources.** `asset_file.package_entry_id` (M14) links a file to its
  `package_entry` locator (`tar_offset`/`size`). A file's bytes resolve *extracted* (`workspace_rel`
  + the file exists) → *in-pack* (`package_entry_id`) → *on-disk* (`rel_path`). "Materialized" is
  derived from that ladder, never stored — no lifecycle fork. `media_type` enum untouched.
- **Mint on analyze.** `Catalog::mint_in_pack_assets(package_id, classified)` reads the pack's
  persisted locators and inserts one `asset` + `asset_file` per classifiable entry (UI supplies the
  guid→`InPackClass` map; core stays classification-free). Idempotent by `package_entry_id`; **skips
  a pack that already has extracted rows** (guard against duplicate rows until step 8 unifies them).
  Dep edges harvest from the manifest `.meta` with zero pulls.
- **Identify in place, from bytes (full parity, 2026-07-25).** The drain queues carry the in-pack
  locator (`pack_path`+`offset`+`size`+`ext`). **Images** decode straight from bytes
  (`read_payload_at` → `raster::decode_bytes`, `resolve_job_image`, no file written). **Metrics,
  audio embedding, tags and 3D embedding** now go through `resolve_job_path` (in `app.rs`): on-disk →
  workspace → **in-pack payload surgically pulled to an OS-temp scratch file** (a `tempfile::TempPath`
  guard deletes it on drop), so the path-based extractors (`metrics::extract`, `decode_audio_48k_mono`,
  `tag_preview`/`thumb::generate`, and the multiview renderer — geometry-only, so an untextured temp
  `.fbx`/`.glb` embeds by shape) identify a purely in-pack asset without a workspace extraction. So a
  pack the user only *analyzed* now reaches `indexed`/`identified` end to end. **Remaining gaps:** an
  in-pack asset's tag `name` is passed empty (the scratch file has a random stem — the caption carries
  the signal; the real name needs the job to carry it); in-pack **deps** still resolve only a real path
  (a `.meta` sidecar isn't the payload — dep edges are harvested from the manifest at mint anyway).
- **Import mints; the folder is a pack-only source.** §5.2 import marks the folder `pack_only` in
  config (a tracked library that is **not crawled** for loose files), pins each pack, and mints in a
  background pass (`ensure_in_pack_minted`), then a `rehydrate` channel reloads the grid. In-pack rows
  carry `package_id`, so `entries_for_source`/`reconcile_missing` exclude them from the folder hydrate
  (no broken cards); they surface via `in_pack_entries_for_source` (pinned packs only), with their
  locator kept in the `in_pack_locators` map beside the grid.
- **The pack browser is the library grid.** **Open →** (only shown once a pack is pinned) routes to
  the library folder-explorer filtered to that pack (`browse_package_folders`): pack-rooted
  breadcrumb, ← / → history, up-one, and a thumbnails ⇄ list toggle (`folders_as_list`). `Add to
  Library` is what indexes (mints); Open just navigates. `folder_children` is pack-aware (several
  packs share a source and all use `Assets/…` paths, so it filters by the active pack); folder tiles
  render as a **vertical list** with `direct (recursive)` counts (e.g. `3 (50)`). A **persistent left
  directory-tree pane** (2026-07-25, nested `SidePanel`) shows the pack's whole hierarchy — see §7.2.
- **In-pack grid thumbnails** (M14 step 4, 2026-07-25). An in-pack card used to show only a typed
  placeholder; the worker now pulls the one payload to a **stable scratch file** in the thumb cache
  (`inpack/<key>.<ext>`, `ensure_in_pack_scratch`, keyed by pack+offset+size so it is reused across
  runs) and thumbnails that path — images, models (hero render) and audio (waveform) all work.
  **Gated on archive size** (`IN_PACK_THUMB_MAX_BYTES`, 256 MB): a per-file pull inflates the gzip up
  to the payload's offset, so on a big pack (the real 21.9 GB "Total Music Collection") every visible
  thumbnail re-inflated gigabytes and pegged the CPU — above the cap the card keeps its placeholder
  until the pack is extracted (the fast random-access path). **ponytail:** batch the visible page into
  one sorted `extract_at_offsets` skip-scan to lift the cap.
- **Oversized packs prompt.** `config.rapid_index_max_gb` (default 4 GB, Settings ▸ Storage). A pack
  over it that isn't indexed prompts **Extract to temp & index / Index in place / Cancel** — indexing
  in place inflates the whole archive (a 21 GB pack is minutes of CPU). The gate lives in
  `pin_and_mint`, so both Add and Open hit it. Manifest read streams with a **% job bar + cancel**
  (`read_manifest_with_progress`); extraction is now **incremental + resumable** (each payload placed
  as its bytes+pathname are seen, so a cancel/crash leaves valid files and a re-run resumes by hash).
- **Viewing an in-pack asset pulls its payload** (`spawn_scratch_pull` → `extract_at_offsets` to an
  OS-temp scratch, cached per asset), because `abs_path` for an in-pack asset **is the
  `.unitypackage`** — decoding that as audio/image is the "Unrecognized format" bug that bit the mp3.
  The inspector shows the **in-archive path** (copyable) + a **Package** row. **The pull now shows a
  job-tray % bar + ✕** (`extract_at_offsets_with_progress`, `scratch_jobs`): on the real 21.9 GB music
  pack a single track sits near the end of the archive, so the gzip skip-scan inflates ~20 GB (minutes)
  — with only the old static toast it read as "can't play any audio" (user-reported). It is **inherent
  to gzip** (no random access): the honest fixes are the visible/cancellable bar, and *extract the pack
  once* for instant repeat playback. **ponytail:** for heavy browsing of one big pack, prompt to
  extract rather than pull per track. **Center folder tiles are hidden when the directory-tree pane is
  present** (§7.2) — the tree owns folder nav, so the old center strip would be redundant buttons.
- **In-pack 3D models rendered blank — fixed 2026-07-25.** The Quick Look computed `direct_model` from
  `e.file.abs_path`, which for an in-pack asset **is the `.unitypackage` archive**, so
  `is_viewable_model` read false and the display's `is_model` branch never ran — the viewport
  `ensure_viewer_loaded` had actually loaded (from the scratch pull) was never shown (the "blank 3D
  default" the user saw). Now it tests `e.file.rel_path` (the asset's own extension, correct for
  in-pack **and** on-disk). Audio was never a display bug — `is_audio` keys off `media_type`; its "can't
  play" was purely the slow pull.
- **"Extract for instant previews" button** (`extract_active_pack` → `extract_whole_pack`,
  `pack_extract_banner`) in the pack browser when the browsed pack still has in-pack assets and is
  bigger than `IN_PACK_THUMB_MAX_BYTES`. One full streaming extraction materializes every row (M14
  step 8) so all assets then preview from disk — the fast answer to a big pack's per-track pull cost.
  Preview pulls now land in the **managed workspace** (content-addressed, LRU-capped, dedups with a
  later full extract) rather than unbounded OS-temp. **ponytail:** cross-session reuse of a single
  previewed track still needs its row materialized on the persist thread (single-writer) — deferred;
  the extract button is the full fast path. **A pack over `rapid_index_max_gb` auto-extracts** once on
  browse (guarded by `auto_extracted`, cancellable) — the user's call that a 20 GB archive should just
  be unpacked for fast access rather than pulled per file; the band between `IN_PACK_THUMB_MAX_BYTES`
  and the limit keeps the manual button.
- **Removing a pack from Library reclaims its extracted bytes** (`unmaterialize_package`, M14 §7).
  Unpin reverts the pack's materialized rows to in-pack (clears `workspace_rel`, keeps the
  `package_entry_id` locator so bytes stay resolvable from the archive) and deletes the workspace files
  **no other live asset shares** (content-addressed dedup — a payload two packs both extracted
  survives). Runs on the same detached connection the pin flip already uses. A 21.9 GB auto-extract is
  therefore not a permanent commit — removing the pack frees it.

**Identification is manual by default** (`config.auto_identify`, off; Settings ▸ AI · IDENTIFY toggle,
[`AI-PIPELINE.md`](AI-PIPELINE.md) §2). The SigLIP/CLAP embedding drain auto-ran on every hydrate, and
in-pack minting flags thousands of new assets `embedding_dirty` — so it churned models on the CPU for
minutes, holding `catalog_persisting=true` (30fps repaint). That was the app-wide high-CPU. Now a
hydrate runs only the cheap drains (metrics/deps/promote); the manual Identify pass does embeddings
**and** tags. The Ollama tagger was already manual.

**Package writes are chunked** (`PKG_WRITE_CHUNK = 500`, `busy_timeout` 15 s). `record_package_entries`
and `mint_in_pack_assets` committed each in one transaction; a huge pack held the single WAL write
lock long enough to starve a concurrent scan-persist ("database is locked"). They now commit in
500-row batches so writers interleave.

**Also this session:** window size + position persist across restarts (eframe `persistence` feature);
audio decodes more codecs (rodio `symphonia-all` — aiff/m4a/aac/alac, not opus/wma/fsb); Unity Assets
list has a **sort** (Indexed first / Name / Size / Date), pack cards show archive **size** (KB/MB/GB);
`docs/AI-PIPELINE.md` §2, `DATA-MODEL.md` §7/§8, `FEATURES.md` §5.3/§5.4/§6.6, `INGESTION.md` §2 all
reconciled. The **design for a better pack directory navigation** (persistent tree pane,
pack-rooted breadcrumb, folders-first contents) is **stored** at
[`design/PACK-DIRECTORY-NAV.md`](design/PACK-DIRECTORY-NAV.md) (captured 2026-07-28 from the Artifact
`357c39f4-0809-4f06-adc2-53c1d3be95b0`); the remaining deltas are the KANBAN card "Pack directory
navigation — finish the three-pane file view".

**Favourites exist** (`asset.favorite`, M12): a pin, not a rating, catalog-side, surviving the Trash.
The **shipped UI diverges from the design bundle** on four presentation points (star not heart, gold
not danger, top-right not top-left, a toolbar toggle + dedicated screen rather than a Filters-popover
section) — written up on the KANBAN card; storage is unaffected.

**A second lane ships UI/Settings work in this same tree** (the SWE-1.7 Max handoff): per-source
rescan/watch/counts, Storage caps, workspace cap + retention, confirm-before-Trash, the Duplicates
badge, the **list-row view**, and marquee drag-select. Consequences worth knowing: the list view does
**not** yet draw the favourite star, the link badge, or material spheres; and `app.rs` is now ~12k
lines, which is a splitting card waiting to be written.

**Delete-from-disk is the one destructive exception**, and it is fenced: an explicit confirmation
modal, `under_known_root` refusing anything outside the user's source roots and the app's own
workspace, `abs_path` (never a recomposed `scan_root.join(rel_path)`, which mis-resolved
package-extracted entries), no `remove_dir_all`, and the catalog row marked trashed **only after**
the file actually left the disk. DATA-MODEL §12 and FEATURES.md §9 name the exception — an
undocumented exception is an invariant break.

**Session 2026-07-30 — UI conformance + two audits (commits `329324e`…`1e9954c`).** Driven by real
in-app screenshots this session, so most of it is eyeballed. Shipped:
- **Thumbnails now fill the whole page** (`d0f5af7`): the grid/list/pack-contents views recomputed
  the spawn buffer only on scroll, so with the per-frame spawn throttle only the first ~cap
  thumbnails ever generated on a static page — the rest kept `3D`/`MAT`/`IMG` placeholders. Now
  recomputed every frame (the `thumb_spawned` guard keeps it cheap). This was the "why are only some
  cards rendered" report.
- **Quick Look fixes**: fills the window centered (`85a130b`), **fixed** canvas height so it stops
  self-growing vertically (`2b8040d` — `available_size().y` fed an unbounded height into an
  auto-height window), and **draggable again** (`881773d` — `.anchor` pins a window; use `.pivot` +
  centered `.default_pos`).
- **Interactive material shader ball in Quick Look** (`b8a1bd1`): `viewport3d::load_material` builds
  a `material::uv_sphere` wearing the dependency-graph albedo (`Catalog::material_albedo_paths` →
  `AlbedoTex`) through the existing flattened-mesh upload path; falls back to the static thumbnail
  with no GL viewport.
- **Favorites screen** (`37e9ee4`, §4.7): the design's 2nd nav destination, was toolbar-toggle-only.
  `Screen::Favorites` reuses the library grid via a shared `on_grid()` predicate (grid keyboard nav /
  toolbar / inspector / pager all key off it) + a hard favourites filter in `rebuild_cache` + a
  "Nothing starred yet → Browse Library" empty state.
- **`database is locked` on pack open fixed** (`086ff26`): the mint is deferred into `pending_mint`
  and started by `receive_catalog` after the hydrate persist lands, so the two writers never overlap
  (see §6 trap).
- **View mode no longer leaks Unity↔Library** (`881773d`): opening a pack explorer forces Folders,
  but the Library's own view mode + folder position are saved in `saved_library_view` and restored in
  `go_to_library` (position only when it was Folders).
- **Grid default preview quality → Low** (`1e9954c`): `PreviewQuality::default()` flipped Medium→Low
  (128 px) to match the design's low-fidelity cards and cut raster/VRAM. **Only lowers *resolution*,
  not poly count** — the real 3D preview cost is the mesh parse, addressed by the cache + throttle,
  not this. See the low-poly-preview item in §7. Existing configs keep their saved value.
- Small conformance: inspector 260→300 px (`6d9ec70`, §0.1); folder rows left-aligned (`54ba2bd`);
  hand cursor on breadcrumb/folder rows (`cd109bf`); in-pack card thumbnail refresh on payload pull
  (`881773d`).
- **Over-engineering audit** (`aca9f1d`): dropped unused `tokio` (core, ai) + `rayon` (core, ingest)
  deps; gated the test-only `HashEmbedder` `#[cfg(test)]`.
- **Design-conformance audit**: Favorites screen (above), FEATURES §0.2 reconciled to the shipped
  5-tab category row (`16d142c`), inspector width. **Not done, deliberately:** the **File menu** is
  load-bearing (only working "Open folder" + project switcher; the design's project-chip switcher §7.1
  is a stub) — retire it only after building the chip switcher.

**Session 2026-08-01 — low-poly proxies + vector previews (`e08da23`…`afadc97`).** HANDOFF §7 item A,
then the storage half of it. Shipped:

- **Low-poly preview proxies** (`e08da23`). `lodestash_ingest::decimate` collapses `MeshData` onto a
  uniform voxel grid — binary-search the grid resolution for the finest grid inside the budget,
  average each cell's members, drop degenerate + duplicate triangles. **Vertex-cluster, not QEM**:
  QEM wants a half-edge structure and an error heap, and at 128–256 px the silhouette is the whole
  signal (`collapse` is drop-in replaceable if that changes). `mesh::load_preview_cached` caches it
  as `<key>-<budget>.lod` and **deletes the staging `.msh`** — that is where the disk win comes from
  (ceiling: a re-scan re-parses). Budget is a **ratio clamped to a floor and a ceiling**
  (`PreviewQuality::proxy_budget()`), because a bare ratio leaves a 500k-tri hull heavy and erases a
  400-tri prop. Thumbnails **and** the multiview embedding both render the proxy — SigLIP reads
  silhouette. `metrics::triangle_count` deliberately stays on `load_cached`: the `triangles` facet is
  the **source** count. Measured on a real 31,869-tri character: **1,026 tris, 3,361 KB → 38 KB (88×)**,
  eyeballed against the full render.
- **Vector previews** (`9eee857`). `lodestash_ingest::vectorize` traces a *rendered view* into a
  contour SVG: nested bands (band 0 = alpha silhouette, bands 1.. = luminance thresholds inside it,
  as fractions of the observed peak), boundary-edge chaining instead of marching squares (no saddle
  cases; holes fall out with opposite winding, which is what `fill-rule="evenodd"` wants), then
  Douglas-Peucker cut at two anchors and rejoined. One SVG holds **all four `DEFAULT_VIEWS` as `<g>`s
  with only `v0` shown**, so hover-spin is a visibility toggle rather than four fetches; paths paint
  `currentColor` with the token hex on the root, so a webview restyles it with one CSS rule while
  resvg still gets the colour. Real model: **4 views, 12 paths, 12.8 KB**, viewed beside the source
  render. `resvg` added **dev-dependency only**, to rasterise a generated SVG so it can be looked at.
- **Catalog storage + generation** (`afadc97`). **M15 `asset_preview`** — own table so `SELECT` on
  `asset` never drags a blob, `ON DELETE CASCADE` because a preview is derived data. `source_key` is
  the mesh cache key **plus the budget tag**, so an edited file *or* a raised quality regenerates.
  Generation hangs off `drain_metrics`, the only place that already has the mesh parsed *and* already
  owns the persist thread's connection — no second parse, no second writer.

**Not done — the visible half.** Nothing renders these yet: grid cards still show the raster
thumbnail. The remaining slice is (a) a `HashMap<asset_id, String>` of previews refreshed per catalog
sync (the §6 "state beside `AssetEntry`" pattern — the rayon thumbnail worker has no catalog handle,
which is the whole reason it can't just query), (b) `generate_texture` preferring it via
`egui_extras`' svg loader, (c) hover-spin cycling `v0..v3`, and (d) the design-doc sections that
hover-spin needs written (`DESIGN-GUIDELINES` §7 asset card, `UI-SPEC` §2, `FEATURES` §1) since the
affordance isn't in the designs today. Also open from the same plan: material-embedding backfill
(the 207 rows, now that a renderer exists), preview-cache accounting across disk **and** catalog
bytes, and the shape-signature search (design-first). Note the egui payoff is limited — resvg
rasterises the SVG anyway; the artifact is native in a webview.

---

## 3. Settled decisions — don't re-litigate

Each is recorded with rationale in [`TECH-STACK.md`](TECH-STACK.md) §7 and the relevant KANBAN card.

- **CPU rasteriser, not GPU offscreen** (`lodestash_ingest::render3d`). The embed drain runs on a
  worker thread while eframe's Glow context is UI-thread-bound, so GPU rendering would need a headless
  context plus cross-thread coordination. *Tradeoff: photoreal/textured output is harder later.*
  It now serves both multiview embedding **and** grid thumbnails.
- **`sqlite-vec` deferred as YAGNI — backed by measurement.** At 50k×768-d a query is 212 ms, of which
  the SQLite BLOB reload is **~5.6×** the cosine scan. An ANN index would optimise the *cheap* half and
  reintroduce a native loadable extension to bundle. AI search is submit-driven (once per Enter), so
  this is fine to ~100k. Re-open only with evidence; `perf_probe_knn_scaling` is the harness.
- **Single-writer rule** (ARCHITECTURE §5). Every embedding write happens on the persist thread's own
  connection, after the grid sync. Do not add a second writer thread.
- **`media_type` stays a 7-value enum** — a data-model invariant. The UI's 4 category buckets map onto
  it; they are not a replacement.
- **No job engine until something long-running needs it** — and now that `export_asset` does, it is
  in `lodestash-api::jobs`: in-memory, atomics, cooperative cancel. Resist adding job *persistence*
  or a priority queue before an operation actually demands them.
- **The agent planes share one ops layer.** Adding an operation means adding a `ServerState` method
  plus a route plus a tool plus a row in `AGENT_OPERATIONS`. Skipping the last one fails the parity
  test, which is the point.
- **The local tagger is two models, and the drain is staged — both forced by measurement.** Asked
  for facet JSON directly, `moondream` answers with arrays of *numbers* (its JSON mode is trained for
  grounding boxes) while captioning the same preview correctly in prose; so the VLM captions and a
  text model writes the facets. And running both per asset made Ollama evict multi-GB weights on
  every call — **~50 s/asset**, against **~7-8 s** once each model stays resident for a batch. Hence
  caption-the-batch-then-facet-the-batch, plus `keep_alive`. Re-measure before assuming a bigger
  model is affordable.
- **Identification means tags *and* embeddings.** An embedding makes an asset findable; nothing has
  been *said* about it until it is tagged. Counting embeddings alone had the meter reporting
  2067/2274 identified for a library where 19 assets had tags. Don't relax it back.
- **Unclassified files never reach the grid.** No `media_type` ⇒ the catalog could never hold it, so
  it was a grid-only ghost. Widen `detect::by_extension` if a real asset goes missing; do not
  re-admit everything.
- **Favourites are a pin, not a rating.** Separate column, separate affordance — folding them into
  the star rating would make every 5-star asset a favourite by accident.
- **A pack is the archive on disk, not the source that found it** (M13). Identity is
  `package_path`; a second sighting through another source fills blanks rather than minting a row.
  Anything that mints packages must go through `upsert_package`.
- **Browse and preview are read-only; only extraction needs a library source.** A pack can be
  opened, walked and previewed from anywhere on disk. Extraction is what writes catalog rows, so it
  is the only step that requires an `import_source` to persist into.
- **An in-pack asset is an asset, not a virtual type** (M14). Where its bytes are is a property of
  its file, resolved through the §7 ladder — not a boolean threaded through counts/filters/status.
  Extraction is demoted to "materialize" (fill `workspace_rel`), never a gate on identification.
- **Identification is CPU-heavy, so it is manual** (`auto_identify`, default off). A hydrate never
  auto-embeds/tags; the user asks for it, or opts into auto. Don't relax it back — auto-embedding an
  imported multi-GB pack's thousands of new dirty rows pegs a core for minutes.
- **In-pack pull cost is inherent to gzip.** `.unitypackage` is tar.gz with no random access, so
  reading a manifest or pulling any one payload inflates from the start up to that offset. Cache the
  manifest (locators) and the pulled scratch file; for a big pack the fast path is **extract once**,
  then everything is on-disk random-access. Don't chase a "seekable" in-pack preview.
- **Separate, non-comparable vector spaces.** SigLIP (768-d visual) and CLAP (512-d audio) are searched
  independently and fused on *rank* by RRF — comparing raw scores across models is meaningless.
  `hybrid_search` therefore takes `spaces: &[(model_id, query_vec)]`.

---

## 4. Running the AI parts

**The ONNX Runtime version is strict.** `ort 2.0.0-rc.10` binds ORT **1.22.x**. A mismatched dll does
not degrade — it *panics*. Both `SiglipEmbedder::load` and `ClapEmbedder::load` wrap init in
`catch_unwind`, so it surfaces as a clean `Err` and the app falls back to the honest "no AI" stub.
Don't casually bump the dll.

The model-backed tests are all `#[ignore]` and env-gated, so a normal `cargo test` never needs models:

```bash
export ORT_DYLIB_PATH="…/onnxruntime-win-x64-1.22.0/lib/onnxruntime.dll"
export SIGLIP_VISION=…/siglip/vision.onnx SIGLIP_TEXT=…/siglip/text.onnx SIGLIP_TOKENIZER=…/siglip/tokenizer.json
export CLAP_AUDIO=…/clap/audio.onnx     CLAP_TEXT=…/clap/text.onnx     CLAP_TOKENIZER=…/clap/tokenizer.json

cargo test -p lodestash-ai   --test siglip_search    -- --ignored --nocapture  # visual search
cargo test -p lodestash-ai   --test siglip_multiview -- --ignored --nocapture  # 3D by shape
cargo test -p lodestash-ai   --test clap_audio       -- --ignored --nocapture  # audio matrix
cargo test -p lodestash-core perf_probe              -- --ignored --nocapture  # KNN scaling
```

**Embedding only drains where a writer already is.** The drains live on the persist thread (single-writer
rule), so a *hydrate* — the fast path when the catalog already knows the library — used to skip them
entirely, and a library catalogued before the models arrived would never encode. `spawn_backlog_drain`
now runs them after a hydrate, taking the same writer slot. If you add a third path that opens the
catalog for writing, it must respect that slot too.

To get the artifacts: run the app and use **Settings ▸ AI ▸ Download** (sources are in
`crates/lodestash-ui/src/models.rs`). They install to the OS data dir under
`Lodestash/models/{siglip,clap}/` and `Lodestash/runtime/onnxruntime.dll`; the same layout works if you
copy them in by hand, which is worth doing to avoid re-downloading ~645 MB.

---

## 5. Verification standard

This is the bar the existing work was held to; please keep it.

- **Anything visual gets looked at**, not just asserted. Coverage and colour assertions pass happily on
  garbage output — a render was only ever trusted after viewing the PNG.
- **Anything semantic gets a matrix**, not one ordering. A single "A beats B" can pass with a subtly
  wrong front end; a diagonal winning across several distinct inputs cannot easily.
- **Property tests get mutation-checked** — break the implementation and confirm the test fails.
  A test that cannot fail is worthless.
- **Prefer reading the reference to inferring it.** The CLAP mel front end looked like an unverifiable
  risk until the authoritative `ClapFeatureExtractor` was read; it turned out to hinge on one detail
  (Slaney vs HTK mel scale) that no amount of reasoning would have settled.

A related note: a test "failing" is not proof the code is wrong. One renderer test failed because a
*cube is legitimately identical across 90°-apart views* (4-fold symmetry) — correct geometry, bad
fixture.

---

## 6. Traps in this repo/environment

- **Never `git add -A`.** It has swept in an agent's live git worktree and daemon state. Use explicit
  paths or `git add -u`. `.gitignore` covers `.claudenator-wt-*/` and `.claude-flow/`, but stray
  tooling artifacts (numerically-named files) still appear in the repo root — check `git status` before
  every commit.
- **Duplicate `pub mod` lines appear** after adding a module to a crate's `lib.rs` (seen twice). It's a
  hard `E0428`; check `lib.rs` immediately after.
- **Run `cargo fmt --all` before committing.** Format drift accumulated silently over ~13 commits once.
- Python heredocs used for bulk edits need **raw** strings (`r'''…'''`) or `\u{…}` in Rust source gets
  eaten as a Python escape.
- **Reverting a mutation-check with `shutil.move`/`copy2` restores the file's old mtime**, so cargo
  thinks the crate is unchanged and reruns the *mutated* binary — the test looks broken after you
  fixed it. `touch` the file (or write it in place) after any revert.
- **To look at the running app**, `PrintWindow(hwnd, hdc, 2)` from PowerShell captures the window
  even when another window covers it; `CopyFromScreen` captures whatever is on top, which in this
  setup is usually the terminal. Screenshot coordinates are window-relative *originals* — the
  images you view are downscaled, so multiply by the ratio the viewer reports before clicking.
- **Synthetic clicks into the app are unreliable.** `PrintWindow` screenshots work every time;
  driving the UI does not. `mouse_event` after `SetForegroundWindow` worked once and then stopped
  (foreground lock), and `PostMessage(WM_LBUTTON*)` never landed at all. Note also that PrintWindow
  captures the **whole window** while `GetClientRect` reports the **client area**, so image
  coordinates are offset by the title bar (~31 px) before you convert them. When a click won't
  land, verify the mechanism headlessly instead of burning the session on input synthesis.
- **Screenshot to a fresh filename every time.** Re-reading the same path served a *stale* image
  twice in a row, once hiding a fix that had actually worked and once hiding that the capture script
  was pointed at a dead PID (PrintWindow on an invalid handle leaves the old file in place). Write
  `shot-1.png`, `shot-2.png`, … and pass the PID as a parameter rather than editing the script.
- **Reaching a screen the app doesn't open on needs temporary instrumentation**, since clicks don't
  land: flip the default `Screen::` and, if the state is behind a dialog, set that state directly for
  one run. Grep for a marker (`// TEMP-SHOT`) and revert every one before committing — verify with
  `grep -c TEMP` and re-check the default screen line by number.
- **`cargo build` fails with "Access is denied. (os error 5)"** when the app is running — it holds
  `lodestash.exe`. `Stop-Process -Name lodestash -Force` first. `cargo check` works regardless.
- **You may not be the only writer in this tree.** A second agent lane edits `crates/lodestash-ui`
  concurrently. Two rules: never run `git checkout <path>` on a file you have not read (that
  discarded a live edit to `docs/design/FEATURES.md` — unstaged changes are unrecoverable), and
  prefer state that lives *beside* `AssetEntry` (a `HashSet`/`HashMap` refreshed per catalog sync)
  over new fields on it, which is what let favourites, tags and dependency edges merge cleanly
  against a rewritten grid.
- **Ollama evicts models between calls.** Alternating two models per asset costs ~6× — batch by
  stage and send `keep_alive`. A small VLM asked for JSON may degenerate into numeric arrays; ask it
  for prose and let a text model do the schema.
- Git Bash maps `/tmp` to the Windows temp dir, and job-scoped env vars aren't guaranteed to persist
  between shell invocations — prefer absolute paths in long-running work.
- **In-pack ↔ extracted rows are unified (M14 step 8 shipped 2026-07-25).** Extraction no longer
  mints a parallel set: `persist_package_to_catalog` records the pack's `package_entry` locators and
  then `Catalog::materialize_in_pack_assets` fills `workspace_rel` on the pack's **already-minted**
  in-pack rows (matched by `package_entry.pathname`); `record_crawl` runs only for leftover files with
  no in-pack row (a pack extracted without ever being analyzed — the empty-`entries` large-pack path).
  So a materialized file resolves *extracted* (fast on-disk) and stops shadowing itself. **Residue:**
  packs extracted under the *old* code still carry their pre-existing parallel rows (no migration
  reconciles them); the new path is only exercised by a *fresh* extract of an analyzed pack, which was
  not driven in the running app this session (headless can't click Extract).
- **`pin_and_mint` used to fire `load_paths` (a crawl+persist) while the background mint wrote** — two
  writers, which surfaced as `last_scanned_at: database is locked`. **Fixed 2026-07-30 by serializing:**
  `pin_and_mint_now` now stashes the mint in `pending_mint` and kicks only the hydrate crawl;
  `receive_catalog` starts the mint (`start_pack_mint`) once that scan's persist has landed, so the
  two writers never overlap. Keep this ordering — don't re-add a mint spawn that runs concurrently
  with `load_paths`.

---

## 7. Suggested next work

**Recommended next: low-poly preview meshes (#A below)** — the focus the user called out. It cuts the
overall footprint (smaller cached meshes, lower render + VRAM) and is the honest fix for 3D preview
cost that the resolution knob (`1e9954c`) is *not*. After that, **P5** (#5, the Unity round-trip) is
the biggest open frontier (server side shipped 2026-07-28; left: the UPM C# plugin (needs Unity) + the
allow-list toggle UI). Contained, verifiable-headless: the low-poly work (#A) and the remaining Unity
Assets §5 items (#3). In rough order:

**A big decision landed this session and is not yet started: the UI is being ported to Tauri 2 +
Svelte 5.** The reasoning is structural rather than taste — the design source of truth is *already*
web tech (`DESIGN-GUIDELINES.md:50` says CSS custom properties are canonical;
`docs/design/Lodestash Prototype.dc.html` is a runnable HTML/CSS prototype of every screen;
`docs/design/icons/` ships 50 real SVGs that egui renders as Unicode stand-ins), so egui forces a
lossy translation on every screen and the docs have already bent to it
(`docs/design/README.md:52-58` tells implementers *not* to reproduce HTML/CSS specifics). Settled:
Svelte 5 + TS + Vite; `#[tauri::command]` for ops plus an `ls://` scheme for binary, with the axum
`/v1/` + MCP agent plane **untouched** (the 1:1 invariant covers the agent surface only); extract a
headless `lodestash-app` crate first and keep egui buildable as the reference until parity; Rust
parses/decimates and three.js orbits in WebGL. Four features ride along (drag out to Unity, drag
in, Ctrl-K palette, Quick Look as its own window) plus a real `lodestash-cli`. Phase A (previews,
above) ships on `main` first and the port inherits it.

**Extraction started (2026-08-01, `cdd5c40`).** `crates/lodestash-app` exists and holds `jobs` +
`models` (433 LOC out of `lodestash-ui`), aliased under their old `crate::jobs` / `crate::models`
paths in `main.rs` so no call site changed. **Beachhead only** — `config`, `drains`, `persist`,
`thumbs`, `grid` and the ~900 lines of domain types at `app.rs:1-903` have **not** moved, and the
four `app/*.rs` children still open with `use super::*` + `pub(super)`, which is the real work
(move + visibility flip only, no cleanups; success = a byte-identical screenshot).

A. ~~**Low-poly preview meshes**~~ **DONE 2026-08-01** (`e08da23`), and extended into vector
   previews — see the session block in §2. What remains of that thread is the *display* half
   (§2, "Not done"). Original note kept below for the reasoning.

   Grid/Quick-Look 3D previews currently render the
   *full* mesh; `1e9954c` only lowered the render *resolution*, not the triangle count, and the real
   cost is the parse + the cached `MeshData` size. Decimate each model to a low-poly proxy **once**,
   cache it beside the existing normalized mesh (`mesh::load_cached` → `cache_dir/mesh/<key>.msh`,
   key `(path,size,mtime)`; see the P1 mesh-cache card), and render previews from the proxy. Wins:
   smaller cache on disk, cheaper raster, lower VRAM — a lower overall footprint. *Approach that fits
   what's here:* a pure decimation pass over `MeshData` (positions/indices/normals/uvs are already
   the in-house vertex format) — quadric-error-metric collapse, or the lazier vertex-cluster / grid
   quantization which is "relatively easy" and good enough for a thumbnail (the user's call). Keep it
   in `lodestash-ingest` (no GPU, worker-thread, same as the rasteriser). Target e.g. ≤2–5k tris for
   the proxy, or a ratio of the source. Full-detail stays for the textured Quick Look orbit *only if*
   you want it sharp — otherwise the proxy is fine everywhere and the cache shrinks. Verify: a pure
   `decimate(MeshData, target) -> MeshData` with a unit test (tri-count drops, AABB preserved within
   tolerance, no degenerate/NaN verts — mutation-check the target-count guard), then eyeball a
   rendered proxy PNG. `media_type` enum + the tri-count facet (which must stay the *source* count,
   not the proxy's) are untouched. ponytail: don't add a mesh-processing crate if a ~100-line grid
   collapse does the job at thumbnail fidelity.


0. ~~M14 step 8 — unify extraction with in-pack rows~~ **DONE 2026-07-25** (`materialize_in_pack_assets`;
   see §6 trap). Extraction now materializes a pack's existing in-pack rows instead of minting a
   parallel set — one row per file, no shadowing. Not yet driven end-to-end in the running app.
1. ~~In-pack identify parity~~ **DONE 2026-07-25.** `resolve_job_path` pulls an in-pack payload to an
   OS-temp scratch file, so `drain_metrics`/`drain_audio_embeddings`/`drain_tags` and 3D embedding now
   identify a purely in-pack asset (see §2). Leftover polish: real tag `name` for in-pack (currently
   empty); not driven end-to-end in the running app.
2. **Pack directory navigation redesign** — design **stored** at
   [`design/PACK-DIRECTORY-NAV.md`](design/PACK-DIRECTORY-NAV.md) (2026-07-28); tracked by the KANBAN
   card "Pack directory navigation — finish the three-pane file view". *Left tree pane shipped
   2026-07-25.* The mockup asked for a persistent left directory tree; it now renders as a nested
   `SidePanel::left("pack_dir_tree_pane")` in the pack browser (Folders mode over a pack): the pack's
   whole hierarchy, indented, expand chevrons, `direct (recursive)` counts, a gold selection rail, and
   click-to-navigate (drives/driven by `current_dir`). Data = `build_pack_dir_tree` (pure, cached in
   `rebuild_cache`), expansion in `dir_tree_expanded`, seeded open down the single-child spine to the
   first branch (`seed_expanded`). Ran clean against the real 1737-asset "Total Music Collection" (no
   panic) but **the pixels weren't captured** — both PrintWindow (blank; glow's GL surface isn't in the
   GDI DC) and CopyFromScreen (black; foreground-lock) hit the §6 traps, so **layout/rail geometry
   needs a human eyeball**. *Delta (b) shipped 2026-07-29:* center **List** mode now renders the
   folders-first `Name / Size / Items` list (`pack_contents_list`) — folder rows first
   (`direct (recursive)`), then files with real size + a duration/dimensions third column + a
   colour-coded type glyph, virtualized `[folders…, files…]` over one `show_rows`; file rows reuse
   `asset_list`'s click/context/marquee plumbing, folder rows navigate `current_dir`. The third-column
   derivation (`grid::content_third_col`) is unit- + mutation-checked; `format_bytes` reused. *Delta (a) shipped 2026-07-29:*
   for a pack the breadcrumb + nav + grid⇄list toggle moved to a full-width
   `TopBottomPanel::top("pack_breadcrumb")` above all three panes (shown on `ctx` before the inspector
   + central; body breadcrumb gated out when the bar is up; ordinary Folders view keeps its inline
   crumb). So the three-pane redesign is now **code-complete** (tree + full-width breadcrumb + List
   contents + global inspector); **what remains is one human eyeball** — the tree rail, breadcrumb bar,
   and List row/rail pixels were never captured (Glow → blank PrintWindow, §6). The right inspector is
   the existing global one (preview + in-archive path + Package + stats).
3. **Unity Assets §5 leftovers** — pack rows now carry **↻ Sync** (2026-07-29): pinned-only, force-
   re-reads the manifest and refreshes `package_entry` locators + re-mints new entries via
   `ensure_in_pack_minted(.., force=true)`, reusing the Open→ job + rehydrate channel (the guid-keyed
   locator upsert it relies on is core-tested; end-to-end wants a rebuilt pack in the running app,
   and refresh is additive — no prune of a GUID removed from the archive, ponytail-noted). The Unity
   Assets **landing now has a content-derived category badge + filter tabs** (2026-07-30, §5.3):
   All / 3D / Materials / Audio / 2D from a pack's dominant catalogued media type
   (`dominant_category`/`package_categories`, both mutation-checked); browse-only packs read Unknown.
   The design's Code/UI tabs aren't content-derivable (no script `media_type`; UI vs 2D-art
   indistinguishable) — FEATURES §5.3 reconciled. **Still deferred:** the **override dropdown** (needs
   a stored/overridable per-pack category) and the **§5.2 import-dialog tabs** (candidates are
   unscanned → nothing to categorize until the stored category lands). **§5.6's `⧉` duplicate flag
   is built** (2026-07-29): the import dialog flags a candidate whose normalized name
   (`pack_name_key`/`duplicate_pack_name`, unit- + mutation-checked) matches an already-indexed pack —
   a `⧉ possibly already indexed as "…"` note per row + a non-blocking warning toast on import;
   normalization is name-only for now (content hashing is the FEATURES §5.6 refinement). The sidebar
   facet's **`✕` remove is built** (2026-07-29): each LIBRARIES package facet row carries an ✕ at its
   right edge that unpins the pack via the existing `set_package_pinned(false)` path (pulls its assets,
   resets the filter, keeps catalog rows); the ✕ is evaluated before the row click so it removes rather
   than navigating (the browse-header **✓ In Libraries · remove** remains the other path to it).
   **script/prefab/material
   previews** show only their type. (Pack **size** now shows on cards; **sort** shipped.)
4. ~~**Favourites design conformance**~~ **DONE 2026-07-30** — the star/gold/top-right marker + the
   ★ toolbar toggle already matched the reconciled design; the last delta, the **Favorites screen**
   (2nd nav destination reusing the grid), shipped `37e9ee4` (§4.7). Nothing left here.
5. **P5 — the Unity round-trip** *(the biggest open frontier)*: shipped so far — the **MCP per-tool
   allow-list** (card 178, engine half), the **register→import round-trip** on both planes
   (`register_project`, `list_projects`, `import_to_project`, approval-gated, dup-guarded), and the
   **REST-only import-queue + ack** plugin transport (`GET /v1/import-queue`, `POST
   /v1/import-queue/{req}/ack`) — so `import_record` now writes on the plugin's **ack** with Unity's
   real path+guid, exercised headless by a simulated plugin (ENGINE-API §5.6). What's left: the real
   **UPM C# plugin** (the one piece unverifiable without a Unity install), `pipeline_mismatch`
   advisory + staging endpoints (§6), the queue's real ~25 s long-poll, and the card-178 **toggle UI**.
6. **P4 remainder** — *the P4 tail shipped almost whole 2026-07-27* (cloud reliability/cost card 170,
   SigLIP zero-shot + 3D multiview card 171, the whole Mapping library card 172, download hardening
   card 173 — see the P4-tail block in §2). Still open: **WD-EVA02** dense tagger (needs the GPU-tier
   model registry — card 173) + YAMNet/PANNs + Whisper; **Claude-native** API impl + **Batches**; the
   optional `regex`/`embedding_knn` mapping match types; `+ trait` quick-add + trait colour on inspector
   chips. A full local Identify pass over this library is ~5 h of CPU; manual + resumable for that reason.
7. **P2 remainder** — ~~the naming heuristic for assets with no `.meta`~~ **DONE 2026-07-29**
   (DATA-MODEL §10 step 2): an edge-less model gains `uses_texture` edges to sibling textures whose
   stem name-matches it (`deps::texture_matches_model`, `source='inferred'` conf 0.7), run in the
   dependency drain (`edgeless_models` × `inference_textures` → `insert_inferred_texture_edges`);
   pure matcher + catalog contract both mutation-checked; end-to-end count wants the running catalog.
   Still open: prefabs inside an extracted workspace, the §13.6 relink panel, and `has_dependents`/
   `has_dependencies` as search filters rather than only chips.
8. **List-view parity** — the new list rows show neither the favourite star, the link badge, nor
   material previews. Also worth a card: `app.rs` is ~12k lines.
9. Optional AI refinements: an LLM query parser (slots in behind `ParsedQuery::validate`),
   cross-encoder rerank, glTF material textures. **Non-win-x64 ORT auto-fetch stays skipped** — it
   can't be tested from this machine.

**Design assets** live in the Claude Design project *LodeStash*
(`f1fd4bce-f7bc-4b33-b099-d74b78f31998`, path `design_handoff_settings_and_library/`), reachable via
the `DesignSync` tool (`list_files` / `get_file`). Its markdown is the **original** handoff and is
older than `docs/design/*.md` — the provenance block at the top of `docs/design/README.md` lists what
a straight re-import would regress. Ask for the prototype HTML to be exported locally rather than
pulling ~260 KB through context.

Known gaps that are *recorded* rather than hidden — see the interim-status notes in
[`ENGINE-API.md`](ENGINE-API.md) §2: `AssetSummary.tags` is always `[]` (tags exist in the catalog but the summary doesn't carry them
yet), `Package` now has `license`/`version` for packs that state them, `AssetDetail` carries only `size_bytes` + `stats`, the MCP per-tool allow-list
(UI-SPEC §16) has its **gating engine shipped** (2026-07-27, card 178 — both planes consult one
`ServerState` allow-list, default all-on; driven by `config.mcp_disabled_tools`) but **no toggle UI
yet**, and job state is in-memory (a crash loses the queue, never the catalog).
