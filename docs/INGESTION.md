# Ingestion, Formats, Viewing & Playback

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

How assets get into the catalog, and how they are viewed/played once there.

Guiding principle: **prefer raw source files; deep-parse Unity serialized formats only when there is
no source.** The most common inputs (Asset-Store packs, project folders) are dominated by standard
FBX/PNG/WAV, which load far more robustly than re-derived blobs. **Unity is the primary ingestion
target;** Unreal `.pak`/IoStore support is planned for a later phase (§7).

---

## 1. Input types & how they arrive

| Input | What it really is | Handling |
|---|---|---|
| **Raw project folder / extracted pack** | `Assets/`-style tree of FBX/OBJ/glTF/PNG/TGA/WAV/OGG + `.meta` | Walk it; load source files directly. **Richest, easiest path.** |
| **`.unitypackage`** | **gzip+tar** of GUID folders; each holds `asset` (the original file bytes), `asset.meta` (YAML), `pathname` (real path) | Untar in pure Rust; `pathname`→real path/ext, `asset`→file, `.meta`→GUID/type/**labels**. Payloads are usually raw FBX/PNG/WAV. |
| **Asset Store library** (downloaded) | Your purchases, cached as `.unitypackage` under `Asset Store-5.x/<Publisher>/<Category>/` | **Auto-discovered, browsable, and unpacked one-or-many on demand — see §2.** |
| **Compiled bundle** (`.bundle`, `.unity3d`, `.assets`, `.resS`) / Addressables | Unity SerializedFile (binary), LZ4/LZMA, often stripped names/TypeTree | **Python sidecar (UnityPy)** → extract & convert. The hard 20%. |
| **Unreal `.pak` / IoStore (`.utoc`+`.ucas`)** | Cooked UE packages; may be AES-encrypted, may need `.usmap` | **Planned, later phase, best-effort** via CUE4Parse (see §7). Not in initial scope. |

The user adds one or more **import sources** (roots). Each is recorded; the catalog references files
in place and is **never destructive**.

---

## 2. Asset Store Library integration (direct Unity download-cache access)

**Proposed analysis access (2026-09-10):** [MCP-ASSET-ANALYSIS](MCP-ASSET-ANALYSIS.md) adds
catalog-ID-based evidence/file access for one or a selected list of in-pack assets. ENGINE-API §8.2
owns the request bounds, access rules and cache effects. Reuse the manifest locator and surgical
payload reader; extract only the selected member and explicitly necessary preview dependencies to
Lodestash-owned scratch. This does not invoke whole-package unpack/import, create duplicate asset
rows or modify the Unity cache. It covers `.unitypackage` manifest payloads; compiled-bundle analysis
continues to depend on supported extraction rather than pretending every bundle is a tar archive.

The user's **downloaded Unity Asset Store library is a first-class import source** — point Lodestash at it
once and it browses everything you own and unpacks/ingests any package(s) on demand. This needs **no
running Unity Editor**: Lodestash reads the on-disk download cache directly, and `.unitypackage` is just
gzip+tar (pure Rust, no sidecar for the common case).

### Where the cache lives (verified, current in Unity 6)
The Package Manager stores downloaded Asset Store packages under an `Asset Store-5.x` root, in a
publisher/category/package tree:

```
<cache-root>/Asset Store-5.x/<Publisher>/<Category>/<Package>.unitypackage
```

| OS | Default cache root |
|---|---|
| **Windows** | `%APPDATA%\Roaming\Unity\Asset Store-5.x`  (`C:\Users\<you>\AppData\Roaming\Unity\Asset Store-5.x`) |
| **macOS** | `~/Library/Unity/Asset Store-5.x` |
| **Linux** | `~/.local/share/unity3d/Asset Store-5.x` |

The location is **user-relocatable**, so on first run Lodestash **auto-detects** candidate roots in
this order, then lets you confirm / add / remove them:
1. The **`ASSETSTORE_CACHE_PATH`** environment variable (Unity's override), if set.
2. The default per-OS path above.
3. Any roots you **add manually** — e.g. the cache was moved to another drive (Package Manager →
   Preferences → My Assets → Cache Location), or you keep per-Unity-version / legacy caches.

**Multiple roots are supported.** Each confirmed root is its own `import_source` of kind
`asset_store_library` (see [`DATA-MODEL.md`](DATA-MODEL.md) §2), so several caches (current +
relocated + leftover) all feed one searchable library.

**First-run flow & validation.** On launch Lodestash probes the candidates and **validates** each
looks like a real cache — an `Asset Store-5.x/<Publisher>/<Category>/…` tree — before offering it, so a
wrong pick is caught early. If none are found (or you decline), an empty state prompts **"Add Asset
Store cache folder…"** with a browse dialog that runs the same validation and warns if the chosen
folder doesn't match the expected layout.

### Browse without unpacking (cheap)
The folder tree already encodes **Publisher / Category / Package name** for free. Enumerate it to
present the whole owned library — publisher, category, name, version, size, on-disk path — *without*
extracting anything. The `.unitypackage` can be peeked for an embedded `preview.png` and counts for a
richer listing.

**Surgical re-extraction:** the browse pass records each `asset` payload's **byte offset in the
decompressed tar** (`package_entry`, [`DATA-MODEL.md`](DATA-MODEL.md) §13 M6). gzip has no random
access, so pulling selected assets is a single **skip-scan**: decompress-and-discard to each wanted
offset, copy that payload out (same content-addressed placement), touch nothing else. Cost is
O(bytes up to the last wanted offset) — no tar parsing, no writes for the unwanted rest. A stale
locator (package changed on disk) fails loudly rather than truncating.

In the UI this surfaces on the pending-packages bar: **Browse** expands a pack's manifest
(pathname + size per pickable entry; `.meta` sidecars and offset-less entries excluded) with
checkboxes and an **Extract selected** action that runs the skip-scan — the pack card stays after a
partial pull so more assets can be picked later. Locators are captured at browse time and persisted
as `package_entry` rows whenever the package persists, so re-extraction never re-scans the archive.
Wrapper packages (render-pipeline variants) get the variant picker instead — their outer manifest
only contains sub-packages.

### Unpack & ingest one or many (on demand)
Selecting one or multiple packages enqueues a **batch unpack+ingest** job (via the job engine):
- `.unitypackage` → **gzip+tar** extract in pure Rust (no Editor, no Python sidecar for the common case).
- Per GUID folder: `pathname` → real path, `asset` → original file bytes, `asset.meta` → GUID, importer
  type, and **`labels`** (seed tags).
- Route raw FBX/PNG/WAV to direct loaders; Unity-native blobs to the UnityPy sidecar.
- Ingest assets into the catalog **linked back to their origin `package`** (publisher/category/version)
  so you can filter/search "everything from pack X." See [`DATA-MODEL.md`](DATA-MODEL.md) §7.
- **Harvest license/attribution** while ingesting a pack: the publisher comes free from the cache tree
  (`<Publisher>/`), and any `LICENSE`/`Third-Party Notices` text or embedded pack metadata is
  captured into `package.license`/`attribution`/`license_url`. *(Shipped 2026-07-23,
  `unitypackage::read_package_meta`: a targeted second pass reads the pack's `.icon.png`, any UPM
  `package.json` — `version`/`displayName`/`license` — and any licence file, matching its **body**
  against a short identifier table rather than its title, because a file headed "License" says
  nothing. It early-exits once its candidates are read, so a 20 GB pack is not re-inflated for a
  2 KB file it doesn't have, and it runs at **extraction** rather than browse for the same reason.
  A pack with a publisher came from the store cache, which is enough to know its licence: the Asset
  Store EULA fills in only where the pack itself said nothing. Verified against the real cache —
  every pack sampled carries a 16-35 KB icon; `A Pathfinding Project Pro` states its version via
  UPM manifests, the older packs state nothing and are recorded as saying nothing.)* This is surfaced on every export/import
  so credits for bought assets travel with them (an individual asset can override its pack's license).
- **Harvest the render pipeline** while ingesting → `package.pipeline`
  (`built_in`/`urp`/`hdrp`/`mixed`; see [`DATA-MODEL.md`](DATA-MODEL.md) §12). Compared against the
  target project on import to raise the `pipeline_mismatch` advisory
  ([`ENGINE-API.md`](ENGINE-API.md) §5.4) — warn-only, never converted. *(Shipped 2026-08-28,
  `lodestash_ingest::pipeline`, written by the desktop import path.)*

  Detection reads **names, not payloads**: the pack's own name, the `pathname`s the browse manifest
  already recorded (`package_entry` — no archive re-inflate), and `com.unity.render-pipelines.*`
  dependencies in any embedded UPM manifest the metadata pass above already reads. Matching is
  whole-token (`URP_10+.unitypackage` names URP; `Burp.wav` and `Highlander.fbx` name nothing), and
  a pack naming several is `mixed` — which is what a pack shipping an `Upgrade/Built_in` beside an
  `Upgrade/URP_10+` actually is. A pack naming none is left **NULL**, so no mismatch is ever
  advised for it. Packs indexed **before** the harvest existed are classified once on writer start
  (`backfill_package_pipelines`) from the same names, so the advisory works on a library you
  already have rather than only on the next thing you import.

  **Shader GUIDs are not used, though this doc first proposed them.** A store pack's `.mat` points
  at *its own* shader asset, so the GUID identifies the pack rather than a pipeline: across the 149
  real `.mat` files in the reference library, eight distinct custom GUIDs appear and the only
  recurring well-known one is Unity's default-resources GUID
  (`0000000000000000f000000000000000`) — which every pipeline uses for UI and legacy materials, so
  reading it as Built-in would mislabel URP packs and fire a false "materials may appear pink"
  warning. Resolving a shader GUID through to a `.shadergraph` and reading its target *would*
  discriminate (Synty's graphs name `UniversalTarget` and `BuiltInTarget` outright), but that costs
  a surgical payload pull per material to learn what the folder names already say. Revisit if a
  pack turns up that targets a pipeline and never names it.

### Archive handling & memory model
A `.unitypackage` is **gzip+tar** — a *sequential* stream with **no central index**, so you can't seek
to a single entry; reaching any entry means decompressing from the start. Lodestash therefore
**streams, never loads the whole archive into memory**:
- **Browse / analyze** = one light streaming pass reading only the small entries (`pathname`,
  `asset.meta`, `preview.png`), skipping the big `asset` payloads, to build a **manifest** cached in
  the catalog — so listing a pack's contents is instant thereafter, at O(1) memory. The manifest
  records each payload's **`tar_offset`/`size_bytes`**, which is what makes the surgical pull below
  possible, and importing the pack mints an **in-pack `asset` per entry** from it
  ([`DATA-MODEL.md`](DATA-MODEL.md) §7 / M14) — no payload is read.
- **Surgical pull** = one gzip skip-scan (decompress-and-discard to a payload's offset, copy just that
  payload, ignore the rest — `extract_at_offsets`). This is the workhorse for **preview**, **identify**
  and single-file **Add to project**: the pack is never fully unpacked. **Identify pulls one payload**
  and identifies off it, then drops it — no workspace copy, no *persistent* identify cache. Images
  decode straight from the in-memory bytes; metrics/audio/tags/3D use path-based extractors, so the
  payload is spilled to an **ephemeral OS-temp scratch file** (`resolve_job_path`, deleted the instant
  the drain is done with it) — a decode buffer that lives on disk only because those decoders take a
  path, not the disposable extract cache. Only **preview** and **explicit extraction** write the
  retained cache dir (Settings → Storage, §6). Serial, one file at a time — identify need not be fast.
- **Full extraction (materialize)** = a single streaming pass (gzip-decode → tar-walk) that extracts
  every payload **once** into a content-addressed, LRU/size-capped **workspace** on disk (evict +
  re-extract on demand, idempotent by hash). This is now **optional** — only when a persistent on-disk
  copy of the whole pack is wanted (e.g. "keep extracted" cache); a pack is fully searchable *without*
  it. When wanted, one full pass beats N partial re-opens, since late entries require streaming the
  whole gzip anyway.
- **Once a pack is fully extracted the `.unitypackage` is not reopened for it**: previews come from
  the normalized/derived caches (thumbnails, mesh→`.glb`, texture→PNG, audio→WAV), and viewing
  **lazy-loads just the one file** (mmap/stream) then releases it — so even multi-GB packs stay light,
  and *every sibling asset in the same pack is random-access on disk* (no re-streaming, no
  whole-pack-in-RAM). An **in-pack** asset that was never materialized instead reopens the archive for
  a single surgical pull on preview/identify/extract (the grid card and search still come from the
  catalog + derived caches, so browsing stays light either way).
- **Reclaiming the workspace (2026-08-29).** Extracted payloads are content-addressed under
  `<workspace>/<hash>/<basename>` and referenced by `asset_file.workspace_rel`. Nothing ever
  collected the ones that stopped being referenced — a pack re-imported or removed left its bytes
  behind unreachable — and the **Keep extracted packs cached** / cap / auto-purge settings were
  loaded, saved, and read by nothing. On the reference machine that was **3.55 GB across 1,179
  folders, none of it referenced by any catalog row**.

  `storage::plan_reclaim` lists the top-level folders no live asset resolves through, and
  Settings ▸ Storage (UI-SPEC §11.6) reports the size before removing anything; the reclaim
  appends the full list to `reclaimed.log` beside the workspace, then deletes. Matching is by
  **path segment**, because `workspace_rel` is written in two formats — the extract path stores
  the absolute `workspace_path` verbatim while `crawl_records_in_workspace` stores a
  workspace-relative path (which is what the column's own doc promises) — and the content-hash
  directory is a segment of both. A pack marked `keep_cached` (M23) keeps its bytes; a pack whose
  `.unitypackage` no longer resolves is never reclaimed at all, because the extracted copy is then
  the only one, and its row offers **Retarget…** (UI-SPEC §15) instead.

- **Workspace retention (warm reopens)** — extracted payloads **persist across sessions**, so a pack
  opened today is still random-access on disk tomorrow with zero re-extraction. Eviction is
  **size-cap first, then age**: within the cap nothing is evicted; under cap pressure, LRU order —
  but anything touched within the **retention window (default 7 days, configurable)** is evicted
  last. Settings → Storage exposes the cap, the retention days, and a "Clear workspace" action.
  Eviction is always safe by construction (idempotent re-extract by hash), and the derived caches
  (thumbnails, normalized glTF/PNG/WAV) are keyed by `content_hash` and **independent of the
  workspace** — browsing stays fast even after a pack's raw payloads are evicted; only raw-file
  access (view original, export/import) triggers a re-extract.
- **Container nuance:** this sequential constraint is specific to tar.gz (`.unitypackage`). **Zip**
  sources (e.g. some Humble bundles) carry a central directory → true per-file random access, so those
  lazy-extract individual files without a full scan (and need no workspace copy at all). The pipeline
  picks the strategy per container.

### Non-destructive & live
- **Never modify the cache.** Lodestash extracts into its own managed workspace (or reads in place) and
  references originals; the Asset Store cache is read-only to us.
- **Watch** the cache root (`notify`) so newly downloaded purchases appear automatically; detect
  re-downloaded/updated packages by `.unitypackage` content hash + version.

### Scope notes
- **Owned-but-not-downloaded (Unity account registry) — optional, best-effort, low priority.** This
  section covers **downloaded** packages (the cache). To also list what you *own but haven't downloaded*
  there is **no official consumer API**; the only routes are the Editor's **My Assets** (Package
  Manager) or **unofficially** querying the My Assets web endpoint (`assetstore.unity.com/account/assets`)
  with the user's logged-in Unity **session cookie** (a `kharma_session`-style token). Lodestash can offer
  an opt-in **"Unity account sync"** that — with the user's consent and login — pulls the owned list +
  basic metadata (name, publisher, thumbnail, package id) and shows those as **"owned, not downloaded"**
  ghost entries (`status='discovered'`) with a one-click "download in Editor" hint. It does **not** get
  asset *contents* (those exist only once downloaded). Caveats: unofficial, may break when Unity changes
  the endpoint, and ToS-grey — hence opt-in and low priority.
- **Two directions** to the Unity story: **cache → Lodestash** (this section, ingest) and **Lodestash → open
  project** (the editor plugin imports a chosen asset into `Assets/`, see
  [`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md)).

---

## 3. Ingestion pipeline (staged)

```
1. Crawl        enumerate files (path, size, mtime) under each source root, honouring the source's
                blacklist (folder subtrees pruned, excluded assets dropped — DATA-MODEL.md §blacklist)
2. Detect       classify by magic bytes (+ extension): unitypackage / bundle / source / archive
3. Unpack       .unitypackage → gzip+tar extract; map GUID→pathname; read .meta (GUID, importer type, labels)
4. Route        recognized source format → direct loader
                Unity-native / compiled bundle → Python sidecar (UnityPy)
5. Normalize    mesh → cached .glb (Assimp/FBX2glTF);  texture → PNG (decode BCn/ASTC/Crunch);
                audio → WAV/OGG (FSB5 via fsb5 when needed)
6. Metadata     tris/verts, materials, bone count (3D); dims/channels/colorspace (image); duration/rate (audio)
7. Hash         BLAKE3 (exact dedup, move detection) + perceptual hash (image pHash / audio Chromaprint)
8. Persist      insert asset + asset_file rows; update FTS (one transaction)
9. Enqueue      thumbnail · embed · AI-tag · map  (background jobs)
```

Steps 5–9 run as background jobs with progress; the asset appears in the grid immediately with a
placeholder thumbnail.

**Step 10 — Relationship harvest** (runs after all assets in the source batch are persisted):

```
10. Relate     GUID-ref pass: resolve .meta fileID/GUID pairs → asset_relationship rows
               Heuristic pass: path-proximity / name-prefix matching for raw exports without .meta
```

- **GUID-ref pass:** Unity `.meta` files and material YAML files contain `fileID`/`guid` references
  to other assets. After step 8 has populated `asset.unity_guid` for every item in the batch, a
  second pass reads each material's GUID references and inserts `asset_relationship` rows
  (`source='unity_meta'`). Prefabs are read beside it as evidence of which mesh each renderer paints
  with which material — per document and slot, kept per file and re-read when the file changes
  (`source='unity_prefab'`, DATA-MODEL §10). Cross-source GUIDs (texture in a shared pack, material in a project
  folder) are resolved across all sources in the active project — relationship rows may cross
  `source_id` boundaries.
- **Heuristic pass:** for raw exports without `.meta`, a lighter inference runs: textures sharing a
  name prefix with a sibling mesh (e.g. `Human3_Albedo.png` + `Human3.fbx`) get an `inferred`
  `uses_texture` edge at `confidence ≈ 0.7`. Only fires when no `unity_meta` edge already covers
  the pair.
- **Library Source label** is set at crawl time: each file's `source_id` maps to its
  `import_source.name` (the named root in the active project, e.g. "Synty", "Human3"). The label
  is stored on the asset row and shown in the inspector + grid badge whenever the project has more
  than one source. See [`DATA-MODEL.md`](DATA-MODEL.md) §10.

### Key extraction facts
- **`.unitypackage` needs no Unity library** — stdlib gzip+tar recovers the original files. Harvest
  the `.meta` **`labels:`** field — publishers/users already tag assets there; it is the single most
  valuable pre-existing metadata and seeds the tag index for free.
- **Compressed textures** require decoding: **`texture2ddecoder`** (Rust crate; BCn/DXT, ASTC, ETC,
  PVRTC), **`decrunch`** for Crunch, plus ASTC/ETC helpers. Expect a long-tail of rare format/version
  combos that fail — provide an "unsupported format" fallback rather than promising 100%.
- **Audio** in Unity is usually FMOD **FSB5** inside `.resS`; extract via the sidecar's `fsb5` path.
  **Wwise** (`.bnk`/`.wem`) is separate middleware — flag as limited/out-of-scope initially.
- **Skinned meshes, animation, materials/shaders** round-trip least reliably; treat as best-effort and
  center the viewer on static geometry + textures + audio first.

### Material previews (the shader ball)

A `.mat` has no pixels and no geometry, so it is previewed the way every DCC does it: painted onto
a sphere. `lodestash_ingest::material::render_sphere` builds a UV sphere and hands it to the same
CPU rasteriser that renders model thumbnails (`render3d`), on a transparent background so the grid
card shows through.

The texture comes from the **dependency graph** (DATA-MODEL §10): the material's resolved
`uses_texture` edge names a catalogued texture, and `Catalog::material_albedo_paths` turns that into
a file path. A material with several textures gets its lowest-id edge — in a Unity `.mat` the albedo
binding is written before the normal and emissive maps, so insertion order beats any name heuristic.
A material with no resolved texture still renders, as a neutral ball.

UVs are longitude/latitude, not the material's own mapping: a tiling texture has no single correct
projection onto a ball, and a shader ball is read as a preview rather than as a claim about how the
material maps. The cache key includes the texture, so swapping a material's albedo cannot serve the
old ball.

Both halves can still be **inside a pack**, and both are pulled through the same surgical-pull scratch
(§2) the asset preview uses: `PreviewSubject` carries the material's own locator *and* its albedo's, so
a material in an un-extracted pack renders rather than falling back to a type icon. Keyed off the
locator (archive + offset + size) when there is no file to stat — the same formula the scratch itself
uses — plus the albedo, on both the grid and the engine plane.

---

### Content vs non-content classification
Asset packs are full of things that aren't browseable *content* — scripts, DLLs, editor extensions,
shaders, assembly definitions, demo scenes, data objects. Lodestash **classifies every file and flags
non-content so it stays out of content searches**:
- **By extension/type**: `.cs`/`.js` scripts, `.dll`, `.asmdef`/`.asmref`, `.shader`/`.cginc`/`.hlsl`/
  `.compute`, `.unity` scenes, editor-only folders (`Editor/`, `Gizmos/`) → `is_content=0`.
- **By Unity importer** (from `.meta`): `MonoImporter`, `PluginImporter`, `AssemblyDefinitionImporter`,
  and `ScriptedImporter`/`DefaultImporter` for non-media → non-content.
- **Tool-only packages**: if a package contains *only* non-content (e.g. an editor utility, a shader
  pack, a C# library), flag the whole `package` so it's hidden by default.
- Non-content rows are **kept, not deleted** (so re-scans stay stable and the user can opt to show
  them), but excluded from the default grid and search. Stored as `asset.content_kind` + `is_content=0`
  (see [`DATA-MODEL.md`](DATA-MODEL.md)).

**Unclassified files never reach the grid.** A file whose extension maps to no `media_type` — a
`.url` shortcut, a `.pdf` readme, a `.txt`, a `.prefab`, a `.cs` — is dropped by the crawl rather
than shown. The catalog could never hold it anyway (`asset.media_type_id` is NOT NULL), so showing
it produced a grid-only ghost: no status, no facets, no embedding, invisible to every agent
operation, and a grid total that disagreed with the health meter. Prefabs are still *read* for the
dependency graph (DATA-MODEL §10) — read as evidence, not listed as assets. The one cost is that a
genuine asset with an unrecognised extension is dropped too; the fix there is to widen
`detect::by_extension` (or wire in the magic-byte fallback), not to re-admit everything.

---

## 4. Normalization targets

| Source | Normalized to | Tooling |
|---|---|---|
| FBX / OBJ / DAE / glTF | **glTF/GLB** (cached) | `russimp` (Assimp, BSD) or `FBX2glTF`; runtime only ever renders glTF |
| Texture2D / DDS / KTX / TGA / PSD | **PNG** (display) | `image` + `image_dds` (BCn) + `psd`; `texture2ddecoder`/`decrunch` for Unity-compressed |
| EXR / HDR | float → tone-mapped PNG for thumbs; float kept for viewer | `image` (EXR/HDR) |
| AudioClip / FSB5 | **WAV/OGG** | UnityPy + `python-fsb5` (sidecar); `symphonia` for re-decode |
| Aseprite / Krita / Paint.NET (`.aseprite`/`.kra`/`.pdn`) | PNG composite + **frame data** | zip-based parse → embedded composite; frames → spritesheet/animation |
| `.blend` (Blender) | glTF/GLB + embedded thumbnail | read the embedded preview; Blender headless export when full geometry is needed |
| Fonts (`.ttf`/`.otf`/`.woff`) | **glyph-sheet PNG** | rasterize a specimen (`ab_glyph`/`fontdue`) |
| MaterialX (`.mtlx`) / `.mat` / PBR map set | **material** (sphere preview) | parse the map set; render on a lit sphere |

**Normalize-to-glTF on import** is the load-bearing decision for 3D: it decouples the renderer and
thumbnailer from format hell (FBX is proprietary) and is identical across all viewers.

---

## 5. Viewing & playback (preview pane)

All three previews share the single **wgpu** device used by the egui UI.

### 3D models
- **Renderer**: `three-d` on shared wgpu — orbit camera, PBR materials, lighting, environment.
- **Input**: cached glTF (from normalization). Show tris/verts/material/bone counts alongside.
- **Thumbnails**: offscreen render-to-texture (a hero 3/4 view) on a worker task → on-disk cache.
  Multiview renders (4–8 orbit views) double as input to the 3D tagging pipeline (see
  [`AI-PIPELINE.md`](AI-PIPELINE.md)).

### Audio
- **Decode**: `symphonia` (WAV/FLAC/MP3/OGG-Vorbis/AAC) → PCM.
- **Playback**: `rodio` (play/pause/seek/volume). `kira` if fancy scrubbing is wanted later.
- **Waveform**: downsample PCM to per-pixel min/max peaks; draw with the egui painter. The same peak
  buckets render a small waveform PNG thumbnail.

### Images / textures
- **Decode**: `image` (PNG/JPG/TGA/BMP/GIF/WebP/TIFF/DDS/HDR/EXR) + `image_dds` (BCn) + `psd`.
- **Display**: upload to a GPU texture; let the GPU handle large images, mip selection, min/mag
  filtering. Tone-mapping/exposure control for float EXR/HDR. Channel toggles for normal/roughness/alpha.
- **Pixel-art aware**: auto-detect pixel art (small dimensions + low unique-color count) and render
  **nearest-neighbor** (no blur), with a manual NN/linear toggle.
- **Material grouping**: detect PBR map sets by filename (`_n`/`_normal`, `_rough`, `_orm`, …) and
  hue heuristics; present albedo + maps as one "material" asset.

---

### 2D sprites & animation
- **Spritesheets / Aseprite**: detect frames (grid slicing, or `.aseprite`/`.kra` frame data) and
  **play the animation inline** (play/pause, FPS, frame scrubber) — the audio transport, for frames.
- **Pixel-perfect**: nearest-neighbor rendering (see Images) keeps pixel art crisp at any zoom.

### Materials
- Group a PBR map set (albedo/normal/roughness/metallic/AO/ORM, MaterialX `.mtlx`) and **render it on a
  lit 3D sphere** in the shared wgpu viewport (reuses the `three-d` renderer) — far more useful than
  flat map thumbnails. The sphere render doubles as the material's thumbnail.

### Fonts
- Render a **glyph specimen sheet** (alphabet + a sample string at a few sizes) as preview and
  thumbnail; capture basic metrics (style, weight) as stats/tags.

---

## 6. Preview artifacts — what a card renders from

A grid card never touches the asset. It renders from a small **derived artifact** on disk, and the
full-resolution source is opened only in Quick Look. Two axes, both driven by the single
`PreviewQuality` setting (Settings ▸ Appearance, UI-SPEC §11.3) — resolution and polygon count are
separate costs and neither implies the other:

| `PreviewQuality` | `thumb_px()` — raster side | `proxy_budget()` — geometry side |
|---|---|---|
| **Low** (default) | 256 px | 2% of source tris, clamped to [300, 1500] |
| Medium | 384 px | 5%, clamped to [600, 4000] |
| High | 512 px | 12%, clamped to [1200, 12000] |

**Every tier covers the card it draws into.** Grid card min-widths are 150 / 205 / 288 px with
`object-fit: cover`, so the old ladder's 128 px *default* upscaled on every display before DPI
scaling entered into it. Low now covers Compact and Cozy at 1×, Medium adds Large, High covers
Large at 1.5×. Checked by `every_quality_tier_covers_the_card_it_draws_into`.

A bare ratio leaves a 500k-tri hull heavy and erases a 400-tri prop; a bare fixed count spends the
same budget on both. Hence ratio-plus-clamp — but note what that means on a real library: the
reference catalog's **median model is 300 triangles**, so 91% of it is already under Low's
ceiling and *all* of it under High's. Decimation is the exception, not the rule, and
`load_preview_cached` therefore writes **no `.lod` at all** when the budget would be a no-op:
it keeps the staging mesh and renders from that, rather than storing a re-encoded copy and
deleting the original.

**Collision hulls are dropped** before measuring or rendering (`mesh::is_collision_hull`) — a
hull is not the prop, it wraps it, and left in it draws a box over the card. Measured first:
17 of 1,307 cached meshes (1.3%) carry one, and every match was `_Collision` with **zero** LOD
levels in the library, so LOD groups are deliberately *not* matched — a loose test would also
eat the asset named `lodestone_gem`.

### The artifacts

| Artifact | Path | What it is | Keyed on |
|---|---|---|---|
| Raster thumbnail | `thumbs/<key>_<w>x<h>_<proxytag>.png` | The card's picture for images, audio waveforms and shader balls | `(path, size, mtime)` + width/height/proxy tag |
| Low-poly proxy | `thumbs/mesh/<key>-<budget>.lod` | Decimated `MeshData`, the thing a model thumbnail and the multiview embedding both render | `(path, size, mtime)` + budget tag |
| Staging geometry | `thumbs/mesh/<key>.msh` | Parse-once full-resolution `MeshData` | `(path, size, mtime)` |
| In-pack scratch | `thumbs/inpack/<key>.<ext>` | One payload pulled out of a `.unitypackage` so a decoder can read it | archive + offset + size |
| Pack icon | `thumbs/pkgicon-<key>.png` | A pack's own `.icon.png`, harvested from the archive | package path |

The first two are **deliverables** — what cards render from, cheap to keep. The last two are
**intermediates**: full-resolution, large, and only needed while the deliverable is being made.
The budget tag on `.lod` is what stops a quality change from serving the old coarse proxy forever;
without it, raising the setting would keep every cached model at the old fidelity.

### The pipeline

1. **Scan** → the metrics drain parses each model once into `mesh/<key>.msh` and persists
   `asset.tri_count` from it. The `triangles` facet must be the *source* count, so this reads the
   full mesh, never the proxy.
2. **Browse or backfill** → `mesh::load_preview_cached` decimates that into `<key>-<budget>.lod`
   and **deletes the staging `.msh`**, which is where the disk win comes from. A re-scan re-parses
   the source; that is the accepted ceiling.
3. **Render** → `thumb::render_model_thumb` rasterises the proxy at `thumb_px()` on the CPU
   (TECH-STACK §7: parsing was the cost, not the raster — a GPU path would not help). Images decode
   through `raster::decode` and downscale (Lanczos3). Audio draws a waveform. Materials paint a
   shader ball.
4. **Serve** → `lodestash://thumb/{id}.png`, with `?q=` carrying the quality so a Settings change
   re-keys the client's cache rather than serving the old size.

Models are refused by the thumb route on purpose: a raster model thumbnail costs a mesh parse, and
one grid page would trigger a hundred. They draw the stored vector preview instead — a 3-band
contour in one flat colour (`MODEL_HEX`), so **every model in the library is the same purple**.

> **Known gaps, measured 2026-09-04 and carded, not yet fixed.** Named here because the spec would
> otherwise read as a description of a system that works better than it does.
>
> - **Models carry no colour into the grid, and cannot as things stand.** The rasteriser does
>   sample an albedo when one is attached (`render3d`, `sample_albedo`), but a model card is not
>   drawn by that path: models are refused by the thumb route, so the card draws the stored
>   **vector preview** — and `vectorize::to_svg` reduces the render to luminance bands filled
>   with a single `currentColor`. Colour is discarded one step after it would be sampled, so
>   every model in the library is the same purple (`MODEL_HEX`). Giving a model card colour
>   means letting models raster, not attaching a texture to the existing path.
> - The **proxy tag is in every PNG name**, including textures, waveforms and shader balls that
>   contain no geometry, so a geometry-budget change invalidates them too.

### Retention — identity first, then the cap

Two mechanisms, and the split matters: **most of this cache is not a cap problem.** The reference
machine held 359 MB against an 8 GB cap, so the cap had never once fired — while 145 MB of that
was already garbage by identity.

**1. The identity sweep** (`storage::sweep_dead_mesh_cache`, at writer startup). Deletes what is
dead the moment it becomes dead, regardless of pressure, from one `read_dir` of `thumbs/mesh/`:

| Class | Recognised by | Why it is dead |
|---|---|---|
| Stale-budget proxy | `<key>-<tag>.lod` where `<tag>` is not the current quality's | A setting change forks the cache; nothing else reaps the loser |
| Orphan staging geometry | `<key>.msh` with no current `<key>-<tag>.lod` sibling | Step 2 deletes it only *if* a preview is later made for that key |
| Crash leftover | `*.part` | An interrupted temp-then-rename |

No catalog lookup: every relation needed is already in the filenames. A race with a drain mid-write
costs one re-parse, the same price a cache miss pays.

**2. The cap** (`storage::purge_previews_over_cap`, same startup block, gated on
`auto_purge_preview_cache`). A flat recursive walk of the whole `thumbs/` tree, oldest mtime first,
until under `preview_cache_cap_gb`. Oldest-by-mtime rather than least-recently-*viewed* because a
read does not touch mtime and recording access times would mean writing on every preview read.

Everything the sweep leaves regenerates on next browse, with two exceptions the cap must respect:

- **`pkgicon-<key>.png`** — a pack's own harvested icon, keyed on the package path. It does not
  come back from browsing; it needs a re-harvest of the archive.
- **In-pack scratch whose archive has been deleted** — those bytes are then the only copy, the same
  invariant `storage`'s module doc states for the extraction workspace.

> **Known gap.** `thumb::evict_cache` implements a size-cap-then-age policy over `*.png` only and
> has **no callers**; `purge_previews_over_cap` is what runs. The cap's flat ordering will delete
> live thumbnails alongside anything the sweep missed, since a freshly-minted file is newer than a
> good thumbnail from last month. Tiering the cap is unearned until a real cache actually crosses
> it — the sweep is what stops the growth that was observed.

---

## 7. Unreal (planned, later phase — best-effort)

- Formats: `.uasset`+`.uexp`+`.ubulk`, `.umap`; `.pak`; IoStore `.utoc`+`.ucas` (often AES-encrypted;
  UE5 stripped builds frequently need `.usmap` type mappings).
- Tooling: **CUE4Parse** (C#, Apache-2.0) is the only realistic embeddable foundation — texture→PNG,
  sound→WAV, static/skeletal mesh→glTF. Runs as its own sidecar (mirrors the Python/UnityPy pattern).
- **Scope honestly**: extract-to-viewable only, no project reconstruction; AES keys & `.usmap` are the
  user's responsibility; format churn makes this inherently fragile. Do not promise Unity parity.

---

## 8. Things not to promise

- Clean extraction from arbitrary **shipped games** (stripped names/TypeTree → partial, unlabeled output).
- Every **compressed texture** variant (rare ASTC block sizes, Crunch variants) decoding successfully.
- **Wwise** audio (separate from the FMOD/FSB path supported initially).
- Reliable **rig/animation/material** round-tripping.
- Unreal parity with Unity (Unreal support is a later-phase, best-effort addition).

Position the product around the **user's owned Unity Asset-Store packs and Unity project folders** —
where the robust 80% is simply: untar → use the embedded raw FBX/PNG/WAV → harvest `.meta` labels.
Unreal ingestion is a later-phase addition.
