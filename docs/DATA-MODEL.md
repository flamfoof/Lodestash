# Data Model, Search & the Mapping Library

The storage backbone, the schema, hybrid search, the user-defined "mapping library," and dedup.

---

## 1. Storage stack

| Concern | Choice | Why |
|---|---|---|
| Relational backbone | **SQLite** (WAL) | Single file, transactional, OLTP-shaped (insert on scan, update tags, fetch one asset, filter) — exactly this workload. Standard for desktop local storage; `rusqlite` in Rust. |
| Full-text | **FTS5** (BM25 built in) | Same file, same transactions, zero extra deps. |
| Vector / ANN | **sqlite-vec** | Lives in the same file; brute-force KNN is comfortably fast at this scale (~100k @ 768-d in tens of ms with pre-filtering) and supports **metadata filter + KNN in one query**. |
| Upgrade seam | **LanceDB** behind a `VectorIndex` trait | Disk-based ANN + native hybrid if vectors exceed ~500k–1M. |
| Exact dedup | **BLAKE3** | Fast parallel content hash; powers dedup + move/rename detection. |
| Near-dup | **pHash/blockhash** (image), **Chromaprint** (audio) | Hamming-distance matching for variants. |

**The decisive property:** relational rows + FTS index + vector index live **in one SQLite file**, so a
single transaction keeps all three consistent. A separate vector DB cannot give that for free — it
would need a reconciliation/outbox job. That atomicity outweighs the raw throughput of a dedicated
vector DB at this scale.

Run with `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`, a generous page cache.

---

## 2. Schema (SQLite DDL)

Catalog-only — original files are referenced, never modified. Integer rowids (FTS5/sqlite-vec
friendly) plus a stable `uuid` per entity for cross-library portability.

```sql
PRAGMA journal_mode=WAL;
PRAGMA foreign_keys=ON;

-- ---------- Sources / libraries ----------
CREATE TABLE import_source (
  id              INTEGER PRIMARY KEY,
  uuid            TEXT NOT NULL UNIQUE,
  name            TEXT NOT NULL,
  root_path       TEXT NOT NULL,
  kind            TEXT NOT NULL,          -- 'folder' | 'asset_store_library' | 'unitypackage' | 'bundle' | 'unreal_pak' (planned) | ...
  last_scanned_at INTEGER,
  watch_enabled   INTEGER NOT NULL DEFAULT 1,
  blacklist       TEXT                    -- JSON: [{kind:'folder'|'asset', value:'<source-rel path or file name>'}] (§blacklist)
);

-- ---------- Media-type taxonomy ----------
CREATE TABLE media_type (
  id    INTEGER PRIMARY KEY,
  code  TEXT NOT NULL UNIQUE,             -- 'model_3d' | 'texture' | 'audio' | 'image' | 'sprite' (animated 2D) | 'material' | 'font'
  label TEXT NOT NULL
);

-- ---------- Logical asset ----------
CREATE TABLE asset (
  id              INTEGER PRIMARY KEY,
  uuid            TEXT NOT NULL UNIQUE,
  source_id       INTEGER NOT NULL REFERENCES import_source(id),
  media_type_id   INTEGER NOT NULL REFERENCES media_type(id),
  name            TEXT NOT NULL,
  description     TEXT,
  primary_file_id INTEGER,                -- the "main" file
  content_hash    TEXT,                   -- BLAKE3 of primary file (exact dedup)
  unity_guid      TEXT,                   -- harvested from .meta when present
  is_content      INTEGER NOT NULL DEFAULT 1,  -- 0 = tool/script/editor-only (hidden from default search)
  content_kind    TEXT,                   -- 'model_3d'|'audio'|'image'|'material'|'prefab'|'script'|'dll'|'shader'|'editor_tool'|'data'|'other'
  status          TEXT NOT NULL DEFAULT 'discovered', -- lifecycle: discovered→indexed→identified→confirmed
  dominant_color  TEXT,                   -- hex of the asset's dominant color (color search/sort; §10)
  -- Technical facets, extracted from the file (§14). NULL = not applicable or not yet extracted.
  tri_count       INTEGER,                -- 3D: triangles across all meshes
  width_px        INTEGER,                -- 2D: pixel dimensions
  height_px       INTEGER,
  duration_ms     INTEGER,                -- audio: clip length
  metrics_dirty   INTEGER NOT NULL DEFAULT 1,  -- extraction queue flag (mirrors embedding_dirty)
  variant_group_id INTEGER,               -- groups near-identical variants under one grid tile (§10)
  embedding_dirty INTEGER NOT NULL DEFAULT 1,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  deleted_at      INTEGER                 -- soft delete
);
CREATE INDEX idx_asset_source ON asset(source_id);
CREATE INDEX idx_asset_media  ON asset(media_type_id);
CREATE INDEX idx_asset_hash   ON asset(content_hash);
CREATE INDEX idx_asset_dirty  ON asset(embedding_dirty) WHERE embedding_dirty=1;
CREATE INDEX idx_asset_metrics_dirty ON asset(metrics_dirty) WHERE metrics_dirty=1;
CREATE INDEX idx_asset_content ON asset(is_content) WHERE is_content=1;
CREATE INDEX idx_asset_status  ON asset(status);
CREATE INDEX idx_asset_variant ON asset(variant_group_id) WHERE variant_group_id IS NOT NULL;

-- ---------- Physical files (LODs, variants, PBR maps) ----------
CREATE TABLE asset_file (
  id              INTEGER PRIMARY KEY,
  asset_id        INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  rel_path        TEXT NOT NULL,          -- relative to import_source.root_path
  role            TEXT,                   -- 'mesh'|'lod0'|'albedo'|'normal'|'variant'|'preview'
  ext             TEXT,
  size_bytes      INTEGER,
  mtime           INTEGER,                -- cheap change detection
  content_hash    TEXT,                   -- BLAKE3 per file
  perceptual_hash TEXT,                   -- pHash (image) / chromaprint (audio)
  missing         INTEGER NOT NULL DEFAULT 0,
  UNIQUE(asset_id, rel_path)
);
CREATE INDEX idx_file_hash  ON asset_file(content_hash);
CREATE INDEX idx_file_phash ON asset_file(perceptual_hash);
CREATE INDEX idx_file_asset ON asset_file(asset_id);

-- ---------- Tags (raw vocabulary) ----------
CREATE TABLE tag (
  id        INTEGER PRIMARY KEY,
  name      TEXT NOT NULL,
  norm_name TEXT NOT NULL UNIQUE,         -- lowercased/trimmed for matching
  namespace TEXT,                         -- facet: subject|style|material|color|mood|usage|audio|technical|free (IDENTIFICATION-AND-SEARCH.md §2)
  color     TEXT,                         -- optional hex for color-coded tag chips (UI)
  parent_id INTEGER REFERENCES tag(id),   -- optional nesting (tag groups)
  -- Trait Library promotion (UI-SPEC.md §9): a tag with is_trait=1 is a first-class user trait
  is_trait   INTEGER NOT NULL DEFAULT 0,
  scope      TEXT,                        -- media_type it applies to; NULL = any
  usage_note TEXT,                        -- when-to-apply guidance, fed to the identifier (AI-PIPELINE.md §6)
  teach      INTEGER NOT NULL DEFAULT 0,  -- 1 ⇒ injected into the identifier vocabulary on future scans
  icon       TEXT                         -- optional glyph for the trait chip
);
CREATE TABLE asset_tag (
  asset_id   INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  tag_id     INTEGER NOT NULL REFERENCES tag(id),
  source     TEXT NOT NULL,               -- 'manual' | 'ai' | 'imported'
  confidence REAL,                        -- 0..1 (null for manual)
  model_id   TEXT,                        -- which model produced an 'ai' tag
  created_at INTEGER NOT NULL,
  PRIMARY KEY (asset_id, tag_id, source)
);
CREATE INDEX idx_assettag_tag ON asset_tag(tag_id);

-- ---------- Collections ----------
CREATE TABLE collection (
  id         INTEGER PRIMARY KEY,
  uuid       TEXT NOT NULL UNIQUE,
  name       TEXT NOT NULL,
  parent_id  INTEGER REFERENCES collection(id),
  is_smart   INTEGER NOT NULL DEFAULT 0,  -- saved-search collection
  is_staging INTEGER NOT NULL DEFAULT 0,  -- a staging tray (§9); one per registered project
  project_id INTEGER REFERENCES project(id) ON DELETE CASCADE,  -- staging trays only
  query_json TEXT
);
-- Enforce one staging collection per project (trays shared across GUI / MCP / Unity panel, §9).
CREATE UNIQUE INDEX idx_collection_staging ON collection(project_id) WHERE is_staging=1;
CREATE TABLE collection_asset (
  collection_id INTEGER NOT NULL REFERENCES collection(id) ON DELETE CASCADE,
  asset_id      INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  added_at      INTEGER NOT NULL,
  PRIMARY KEY (collection_id, asset_id)
);

-- ---------- Embeddings (multi-model, multi-modality) ----------
CREATE TABLE embedding_meta (
  asset_id   INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  model_id   TEXT NOT NULL,               -- 'siglip2-so400m' | 'clap-htsat' ...
  modality   TEXT NOT NULL,               -- 'image' | 'audio' | 'text'
  dim        INTEGER NOT NULL,
  version    INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (asset_id, model_id)
);

-- Vector store (sqlite-vec); rowid == asset_id keeps joins trivial.
CREATE VIRTUAL TABLE vec_asset USING vec0(
  asset_id      INTEGER PRIMARY KEY,
  embedding     FLOAT[768],
  media_type_id INTEGER partition key,    -- shard by media type for faster KNN
  source_id     INTEGER,                  -- metadata filter column
  +model_id     TEXT
);
-- SHIPPED (M8, interim): embeddings live in a plain BLOB table with brute-force cosine
-- KNN in Rust (crate::vector::BruteForceIndex behind the VectorIndex seam) — zero new
-- deps, comfortably fast to ~100k vectors (§3). The vec0 virtual table above is the
-- sqlite-vec optimisation adopted *behind the same seam* once brute force stops being
-- comfortable (then LanceDB past ~1M, §1). Both are interchangeable under VectorIndex.
CREATE TABLE asset_embedding (
  asset_id      INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  model_id      TEXT    NOT NULL,
  modality      TEXT    NOT NULL,          -- image|audio|text (which encoder)
  dim           INTEGER NOT NULL,
  media_type_id INTEGER,                   -- pre-filter / shard column
  source_id     INTEGER,                   -- pre-filter column
  embedding     BLOB    NOT NULL,          -- dim * f32, little-endian
  created_at    INTEGER NOT NULL,
  PRIMARY KEY (asset_id, model_id)
);

-- Full-text search (FTS5 external-content over asset)
CREATE VIRTUAL TABLE asset_fts USING fts5(
  name, description, tags_concat,         -- denormalized tag string
  content='asset', content_rowid='id',
  tokenize='unicode61 remove_diacritics 2'
);
-- AFTER INSERT/UPDATE/DELETE triggers keep asset_fts in lockstep (omitted for brevity).
```

Filter + ANN in one query:

```sql
SELECT asset_id, distance
FROM vec_asset
WHERE embedding MATCH :query_vec
  AND k = 100
  AND media_type_id = :mt          -- partition-key prefilter
  AND source_id IN (:s1, :s2);     -- metadata filter
```

> KNN `WHERE` supports `= != < <= > >= BETWEEN IN` — **not** `LIKE`/`GLOB`/`NULL`.

---

## 3. Hybrid search

**Pattern: structural pre-filter → two retrievers → Reciprocal Rank Fusion → optional rerank.**

> **The two user-facing modes** map onto this engine: **Search by Tags** = pure structural retrieval
> (FTS/BM25 over names/tags + tag/category/media-type filters, *no* vector step) — fast, exact,
> deterministic. **Search with AI** = the full hybrid path including the semantic vector retriever, so a
> plain-language description finds assets even when no tag matches. Both default to `is_content=1`
> (non-content hidden); AI search also requires `status IN ('identified','confirmed')` (only
> AI-processed assets have embeddings). The category filter is `media_type` ∈ {Models, Sounds, Images}.

1. **Pre-filter** (relational `WHERE`): media_type, collection, source, **tag set**, **mapped
   category**. Shrinks the candidate set first; pushed into the retrieval (sqlite-vec metadata cols /
   LanceDB scalar indexes). Tag/category filters are **hard boolean constraints**, not ranking signals.
2. **Two ranked retrievals** over the filtered set:
   - **FTS5/BM25** on name + tags + description (keywords, exact terms, named assets).
   - **Vector KNN** (sqlite-vec) for semantic/paraphrase similarity.
3. **Fuse with RRF**: `score(d) = Σ 1/(k + rank_i(d))`, `k≈60`. RRF combines on **ranks**, sidestepping
   the BM25-vs-cosine score-incompatibility; reliably beats either retriever alone.
4. **(Optional) cross-encoder rerank** of the top-N for the final 10–20 — opt-in (cost/latency).

**Index sync:** because FTS5 and sqlite-vec are in the same file, every asset/tag mutation runs in one
transaction updating base table + FTS (via triggers) together. Embeddings are produced async: mark
`embedding_dirty=1` on content change; the embed worker computes + upserts the vector and clears the
flag in its own transaction. There is a reference SQLite BM25+vector+RRF implementation to mirror.

---

## 4. The "mapping library"

**Interpretation:** a user-defined **controlled vocabulary + rules** that map messy raw/AI tags onto a
clean canonical project taxonomy (e.g. Environment / Props / Characters / SFX / Music, with
sub-categories). This is the classic DAM problem; the model borrows from **SKOS** (preferred labels,
alt labels, broader/narrower, match relations), simplified.

Separates **raw tags** (observations) from **canonical categories** (interpretation) — so you can
re-map non-destructively without touching the underlying tags, and swap whole mapping libraries
(e.g. a generic map vs. a per-project Unity vocab).

```sql
-- A named, swappable mapping library (= SKOS concept scheme)
CREATE TABLE taxonomy (
  id INTEGER PRIMARY KEY, uuid TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1, is_active INTEGER NOT NULL DEFAULT 0
);

-- Canonical categories (hierarchical)
CREATE TABLE category (
  id          INTEGER PRIMARY KEY,
  taxonomy_id INTEGER NOT NULL REFERENCES taxonomy(id) ON DELETE CASCADE,
  parent_id   INTEGER REFERENCES category(id),
  pref_label  TEXT NOT NULL,             -- "Environment", "Props", "SFX/Footsteps"
  path        TEXT NOT NULL,             -- materialized: "Environment/Foliage/Trees"
  UNIQUE(taxonomy_id, path)
);

-- Aliases / synonyms that mean a category
CREATE TABLE category_alias (
  id INTEGER PRIMARY KEY,
  category_id INTEGER NOT NULL REFERENCES category(id) ON DELETE CASCADE,
  alias TEXT NOT NULL, norm_alias TEXT NOT NULL,
  UNIQUE(category_id, norm_alias)
);

-- Rules mapping raw tags/metadata → a category
CREATE TABLE mapping_rule (
  id          INTEGER PRIMARY KEY,
  taxonomy_id INTEGER NOT NULL REFERENCES taxonomy(id) ON DELETE CASCADE,
  category_id INTEGER NOT NULL REFERENCES category(id) ON DELETE CASCADE,
  match_type  TEXT NOT NULL,             -- 'exact_tag'|'alias'|'regex'|'glob_path'|'media_type'|'embedding_knn'
  pattern     TEXT NOT NULL,
  priority    INTEGER NOT NULL DEFAULT 100,   -- lower wins on conflict
  min_confidence REAL DEFAULT 0,
  enabled     INTEGER NOT NULL DEFAULT 1
);

-- Materialized assignments (the output)
CREATE TABLE asset_category (
  asset_id    INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  category_id INTEGER NOT NULL REFERENCES category(id) ON DELETE CASCADE,
  taxonomy_id INTEGER NOT NULL REFERENCES taxonomy(id),
  assigned_by TEXT NOT NULL,             -- 'rule' | 'manual' | 'override'
  rule_id     INTEGER REFERENCES mapping_rule(id),
  confidence  REAL,
  is_pinned   INTEGER NOT NULL DEFAULT 0, -- manual overrides survive re-runs
  PRIMARY KEY (asset_id, category_id, taxonomy_id)
);
```

**Workflow:**
1. **Author** the library (categories, aliases, rules) or import a starter pack; multiple taxonomies
   coexist, one active.
2. **Apply** (batch/incremental): evaluate rules in `priority` order against each asset's
   tags/aliases/path/media_type (and optional `embedding_knn` = "near these seed examples"); write
   `asset_category` with `assigned_by='rule'`.
3. **Override**: manual assignments set `is_pinned=1`; re-runs never clobber pinned rows. Conflicts
   resolve by `priority` then confidence.
4. **Re-map**: bump `taxonomy.version` and re-apply; non-pinned rows regenerate. Raw `asset_tag`s are
   untouched — fully non-destructive.
5. **Search**: `asset_category` is just another structural pre-filter ("Props" AND semantically
   "rusty medieval barrel"); hierarchy via `path` prefix or a recursive CTE on `parent_id`.

---

## 5. Dedup

- **Exact**: BLAKE3 over file bytes → `content_hash`. Duplicates across libraries =
  `GROUP BY content_hash HAVING COUNT(*)>1`. Also powers **move/rename detection** (§6).
- **Near-dup**: pHash/blockhash (images) and Chromaprint (audio) → `perceptual_hash`; match by Hamming
  distance (threshold ~≤6 bits). For fuzzy "similar asset," use embeddings instead.
- **Surface, never auto-delete.** Present duplicate groups in a review queue (non-destructive).

---

## 6. Scale, watch & move detection

- **Non-destructive catalog**: store `root_path` + `rel_path` + hashes + derived artifacts (thumbnails,
  embeddings in a sidecar cache). Originals are never written.
- **Incremental scan**: diff crawl results against DB by `(size, mtime)`; only re-hash on change.
- **Watch**: `notify` crate for live updates, **always paired with a periodic full reconcile** (watchers
  drop events under bursts).
- **Move/rename**: a path that vanished + a new path with the **same `content_hash`** ⇒ same file
  moved — update `rel_path` in place, preserving `asset_id`, tags, embeddings, and mappings. Missing
  with no hash match ⇒ `missing=1` (grace period, no hard delete — resilient to unplugged drives).
- **Perf at 100k**: FTS5 BM25 sub-10 ms; sqlite-vec KNN tens-of-ms (less with partition keys +
  pre-filter). Past ~500k–1M vectors, flip the `VectorIndex` seam to LanceDB (HNSW).

---

## 7. Asset Store packages (Unity download library)

Downloaded Asset Store `.unitypackage`s are modeled as **packages** so assets stay grouped by their
origin pack — enabling "show/search everything from pack X" and update detection. The Asset Store
library root is an `import_source` of kind `asset_store_library`; each ingested `.unitypackage` becomes
a `package` row under it. See [`INGESTION.md`](INGESTION.md) §2.

```sql
CREATE TABLE package (
  id            INTEGER PRIMARY KEY,
  uuid          TEXT NOT NULL UNIQUE,
  source_id     INTEGER NOT NULL REFERENCES import_source(id),
  publisher     TEXT,                   -- from cache tree: <Publisher>/
  category      TEXT,                   -- from cache tree: <Category>/
  name          TEXT NOT NULL,          -- package display name
  version       TEXT,                   -- detected pack version
  package_path  TEXT NOT NULL,          -- absolute path to the .unitypackage in the cache
  content_hash  TEXT,                   -- BLAKE3 of the .unitypackage (update detection)
  asset_count   INTEGER,                -- filled after unpack (0/NULL = browsed but not yet ingested)
  unpacked_at   INTEGER,
  pinned        INTEGER NOT NULL DEFAULT 0, -- 1 = promoted to a sidebar LIBRARIES row (M7, §UI)
  license       TEXT,                   -- e.g. 'Unity Asset Store EULA', 'CC-BY-4.0' (harvested at ingest)
  attribution   TEXT,                   -- required credit string, when applicable
  license_url   TEXT,                   -- link to the license/EULA
  UNIQUE(source_id, package_path)
);
ALTER TABLE asset ADD COLUMN package_id INTEGER REFERENCES package(id);
CREATE INDEX idx_asset_package ON asset(package_id);
CREATE INDEX idx_package_source ON package(source_id);
```

- **Browse-only** populates `package` rows (publisher/category/name/version/path) **without** unpacking;
  `asset_count`/`unpacked_at` stay null until the user ingests that pack.
- **License/attribution** is harvested at ingest (cache tree + any embedded pack metadata; see
  [`INGESTION.md`](INGESTION.md) §2) into `license`/`attribution`/`license_url`, then **surfaced on
  export/import** so shipping with bought assets stays traceable. An individual asset may override the
  pack's license (see `asset.license_*` below).
- **`package` is another search facet** (and a grid hierarchy: Publisher → Category → Package).
- **Pinned as a library**: the Unity Assets screen's **Add to Library** sets `package.pinned=1`
  (FEATURES.md §5.5); a pinned pack shows as a sidebar **LIBRARIES** row and surfaces its assets in the
  normal grid via the `asset.package_id` join — no separate config list. Removing it clears the flag.
  This is the durable backing for the in-memory `LibraryFilter::Package` the UI already uses.
- **Update detection**: re-compare the `.unitypackage` `content_hash`; on change, re-ingest and
  reconcile that package's `asset` rows (non-destructive — preserves manual tags/mappings on unchanged
  assets via per-file `content_hash`).

---

## 8. Asset lifecycle & "Identify" mode

Every asset carries a **`status`** tracking how far it's been processed, so the app can separate the
clean, searchable library from the raw backlog — and offer a focused workflow to work through that
backlog. **Recommended user-facing terms: Identified vs Unidentified** (alternatives if you prefer:
*Catalogued/Backlog* or *Indexed/Pending* — "stashed/unstashed" reads ambiguously, so avoid it).

| `status` | Meaning | Searchable by |
|---|---|---|
| `discovered` | Seen on disk / listed from the Asset Store cache, **not yet unpacked/ingested** | browse only |
| `indexed` | Ingested: file catalogued, thumbnail made, in FTS — **no AI yet** | name, folder, Unity labels |
| `identified` | AI tagging **+** embeddings done → fully semantic-searchable | tags **and** AI |
| `confirmed` | A human reviewed/curated the tags (accept/reject) → trusted | tags and AI |

- **Library mode** shows the **Identified/Confirmed** set — your clean, fully searchable collection.
- **Identify mode** (the triage workspace) shows the **Unidentified** backlog (`discovered`/`indexed`)
  and lets you batch-run AI identification, hand-tag, and mark items `confirmed`. This is how you
  whittle a freshly-imported 10k-asset pile down into a searchable library.
- `status` is just another filter ("everything still **Unidentified** in **Sounds**" is one click).
  Re-running AI (e.g. after a model upgrade) cycles an asset back through `identified`.

**How the status actually moves.** `status` is *derived*, not hand-set: `Catalog::promote_statuses`
reconciles every live row against the ingest work recorded on it, and runs on the writer connection
right after the drains (facets, then vectors), so a scan, a package extraction and a backlog drain
all land the same transitions:

| Transition | Earned by |
|---|---|
| `discovered → indexed` | `metrics_dirty = 0` — the technical facets (§14) have been extracted |
| `discovered`/`indexed` → `identified` | a row in `asset_embedding`, with `embedding_dirty = 0` |
| `identified → indexed` | `embedding_dirty = 1` again — the file's bytes changed, so the vector is stale |

`confirmed` is human-set (the P4 accept/reject flow) and the reconcile never touches it. The pass is
idempotent, so it is safe to run after every drain.

> **Interim divergence.** This table says `identified` means "AI tagging **+** embeddings"; until the
> P4 tagging lane exists there is nothing to tag with, so today the embedding alone earns it. When
> tagging lands, `promote_statuses` gains an `asset_tag` condition and this note goes away.

---

## 9. Projects, imports & staging

The state behind the engine round-trip ([`ENGINE-API.md`](ENGINE-API.md) §5,
[`MCP-AND-ENGINES.md`](MCP-AND-ENGINES.md) §3): *which* editors are open, *what* has been imported
where (provenance + dup-guard), and the per-project staging shortlists.

```sql
-- ---------- Registered Unity projects (open editors) ----------
CREATE TABLE project (
  id            INTEGER PRIMARY KEY,
  uuid          TEXT NOT NULL UNIQUE,
  name          TEXT NOT NULL,            -- last path segment, or user label
  project_path  TEXT NOT NULL UNIQUE,     -- absolute path to the Unity project root
  unity_version TEXT,
  last_seen_at  INTEGER,                  -- heartbeat; stale ⇒ editor considered closed
  is_active     INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX idx_project_active ON project(is_active) WHERE is_active=1;

-- ---------- Import provenance (durable result of an import) ----------
CREATE TABLE import_record (
  id                INTEGER PRIMARY KEY,
  asset_id          INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  project_id        INTEGER NOT NULL REFERENCES project(id) ON DELETE CASCADE,
  source_package_id INTEGER REFERENCES package(id),  -- where it came from
  dest_path         TEXT NOT NULL,        -- in-project path, e.g. 'Assets/Wildlife/Beaver.fbx'
  unity_guid        TEXT,                 -- GUID Unity assigned on import
  with_deps         INTEGER NOT NULL DEFAULT 1,
  imported_at       INTEGER NOT NULL,
  batch_id          TEXT,                 -- groups one tray/queue run; powers "Recent exports" + batch undo (UI-SPEC.md §16)
  UNIQUE(asset_id, project_id)            -- dup-guard key ("already imported?")
);
CREATE INDEX idx_import_project ON import_record(project_id);
CREATE INDEX idx_import_asset   ON import_record(asset_id);

-- ---------- Per-asset license override (optional; falls back to package) ----------
ALTER TABLE asset ADD COLUMN license     TEXT;   -- overrides package.license when set
ALTER TABLE asset ADD COLUMN attribution TEXT;
ALTER TABLE asset ADD COLUMN license_url TEXT;
```

- **`project`** powers "import **into Unity**" targeting and the active-project facet. The Unity plugin
  upserts its row on panel open (`register_project`) and heartbeats `last_seen_at`; the import flow
  targets `is_active=1` when no `project_id` is given.
- **`import_record`** is the **durable** outcome of an import (the import *queue* itself is transient,
  job-engine backed — only this result persists). The `UNIQUE(asset_id, project_id)` key is the
  **dup-guard**: before enqueuing, the core checks it and, if the `dest_path` still resolves, surfaces
  "already imported — re-import?" instead of silently duplicating. It also answers "where did this
  asset come from / where is it used," shown as `imported_in` in `AssetDetail`.
- **Staging** reuses the existing `collection` machinery, **one staging collection per project**:
  `is_staging=1` rows carry a `project_id` (created empty when a project is registered), so each
  import target owns its shortlist and switching projects never mixes picks
  ([`UI-SPEC.md`](UI-SPEC.md) §16). Stage operations take an optional `project_id`, defaulting to
  the active project — an item Claude stages over MCP still appears in the GUI tray and Unity panel
  at once. *(Supersedes the earlier single shared shortlist.)*
- **Import batches**: `import_record` rows from one tray/queue run share a `batch_id`, so the UI can
  list "Recent exports" per project and **undo a batch** — delete the copied files it produced and
  its records; catalog assets are untouched (confirm-gated, [`UI-SPEC.md`](UI-SPEC.md) §16.1).
- **License** resolves **asset → package**: an asset's effective license is `asset.license` when set,
  else `package.license`. Surfaced on the detail stat block and on every export/import so credits
  travel with the asset.

---

## 10. Library Source labeling & asset relationships

### Library Source

Every asset row carries `source_id → import_source.name`. In a multi-source project (e.g. "Synty"
folder + "Human3" folder added via *+ Add source*), `import_source.name` is the **Library Source
label** shown in the inspector, grid badge, and search filter chip. It answers "where did this
asset come from?" at the collection level.

- **Display rule:** show the Library Source label on every asset card (small chip) and in the
  inspector detail block whenever the active project has more than one source. Single-source
  projects suppress it (redundant noise).
- **Search facet:** `source_id IN (…)` is already a pre-filter column in `vec_asset`; it becomes
  a UI filter chip ("Library: Synty") in both Search-by-Tags and AI Search modes.
- **`import_source.name`** defaults to the root folder's stem on first add; the user can rename it
  in Project settings without touching the file system.

### Per-source blacklist

Each `import_source` carries an independent **blacklist** — a list of `{kind, value}` exclusions
(`folder` or `asset`) that the crawler **skips when indexing** ([`INGESTION.md`](INGESTION.md) §1),
editable in Settings → Library (design §5.1). Semantics: a `folder` entry prunes that subtree
(source-relative path); an `asset` entry drops a single file (exact source-relative path or bare
file name). Matching is `/`↔`\` and case-insensitive. **Non-destructive** — a blacklist entry only
affects cataloguing, never files on disk (§6). Removing a source drops its blacklist. Interim: while
the SQLite catalog is pre-`import_source`, the pre-catalog UI persists this per source in the app
config (`LibraryProject.blacklist`, keyed by source path); it maps 1:1 onto this column when the
catalog lands.

### Asset relationships (dependency graph)

Assets reference each other — a texture is consumed by a material which is consumed by a mesh
which is assembled into a prefab. These relationships must be **explicit rows**, not inferred at
query time, so they survive re-indexing and are queryable ("what materials use this texture?",
"what is this texture's parent mesh?").

```sql
-- Directed relationship edge between two assets.
-- kind values: 'uses_texture' | 'uses_material' | 'uses_mesh' | 'uses_audio'
--              | 'uses_script' | 'variant_of' | 'derived_from'
-- source:      'unity_meta' (harvested from .meta GUID refs)
--              | 'inferred'  (path-proximity / naming heuristics)
--              | 'manual'    (user-asserted)
CREATE TABLE asset_relationship (
  id            INTEGER PRIMARY KEY,
  from_asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  to_asset_id   INTEGER REFERENCES asset(id) ON DELETE CASCADE,  -- NULL ⇒ unresolved reference
  kind          TEXT NOT NULL,
  source        TEXT NOT NULL DEFAULT 'unity_meta',
  confidence    REAL,                     -- 0..1; null for unity_meta (deterministic)
  declared_path TEXT,                     -- the path/name the parent declared (FBX binding, .mtl, GUID)
  state         TEXT NOT NULL DEFAULT 'resolved',  -- 'resolved' (in pack) | 'library' (cross-source) | 'missing'
  UNIQUE(from_asset_id, to_asset_id, kind)
);
CREATE INDEX idx_rel_from ON asset_relationship(from_asset_id);
CREATE INDEX idx_rel_to   ON asset_relationship(to_asset_id);
```

**Relationship kinds:**

| `kind` | Direction | Example |
|---|---|---|
| `uses_texture` | material → texture | `Sword_Mat` → `Sword_Albedo.png` |
| `uses_material` | mesh/prefab → material | `Sword.fbx` → `Sword_Mat` |
| `uses_mesh` | prefab → mesh | `SwordPickup.prefab` → `Sword.fbx` |
| `uses_audio` | prefab/script → audio | `FootstepController` → `gravel_step.wav` |
| `uses_script` | prefab → script | `Player.prefab` → `PlayerController.cs` |
| `variant_of` | recolor/LOD → source | `Sword_Gold.fbx` → `Sword.fbx` |
| `derived_from` | baked/exported → source | `Sword_Baked.fbx` → `Sword.blend` |

**Harvesting order (ingest pipeline, see [`INGESTION.md`](INGESTION.md) §5):**

1. **Unity `.meta` GUID refs** — highest confidence; deterministic. A material's `.meta` file (or
   the material YAML itself) lists the GUIDs of all textures it references. After all assets in a
   source are indexed and `unity_guid` is populated, a second pass resolves GUID pairs into
   `asset_relationship` rows (`source = 'unity_meta'`).
2. **Path/naming heuristics** — for assets without `.meta` (raw exports): a texture named
   `Human3_Albedo.png` living beside `Human3.fbx` infers `uses_texture` with `source='inferred'`
   and `confidence ≈ 0.7`. Applies only when no `unity_meta` relationship exists.
3. **Cross-source resolution** — GUID refs that resolve to an asset in a *different* source root
   (e.g. a shared texture pack) are still recorded; `from_asset_id` and `to_asset_id` may differ
   in `source_id`. This means "find all assets that use textures from the Synty source" is a
   simple join.

**UI surfaces:**
- **Inspector → Related** tab: lists all outbound and inbound relationships with their kind and
  Library Source label. Clicking navigates to the related asset.
- **Grid badge:** a texture that has at least one `uses_texture` inbound edge shows a small
  link-chain icon; hovering previews the parent material(s).
- **Search:** `has_dependents` / `has_dependencies` as boolean filter chips. `"textures used by
  assets from source: Synty"` resolves via a two-hop join.
- **Export/import with deps:** when importing an asset into Unity, the relationship graph is walked
  to offer "import with dependencies" — selecting a material auto-selects its textures. Already
  referenced in [`ENGINE-API.md`](ENGINE-API.md) §4; `asset_relationship` is the backing table.

---

## 11. Variants, colors & 2D / material / font assets

Backing the AssetHoard-inspired additions, kept cohesive with the existing model:

- **Variants** — near-identical assets (same model at different scales, recolors, re-exports) are
  clustered by perceptual / `content_hash` similarity (reusing dedup, §5) into a `variant_group_id`.
  The grid **collapses a group into one tile** with a count badge; expanding shows members.
  Non-destructive — variants stay separate rows, just visually grouped.
- **Color search** — at ingest, image/texture/sprite assets get a `dominant_color`; it becomes another
  structural facet in hybrid search (§3), enabling "filter / sort by color".
- **Tag colors & nesting** — `tag.color` drives color-coded chips; `tag.parent_id` allows nested tag
  groups. Distinct from the controlled-vocabulary **mapping library** (§4) — tags stay free-form.
- **New media types** — `sprite` (animated 2D: frame/spritesheet metadata via `asset_file` roles +
  stats), `material` (a map set grouped via `asset_file` roles, previewed on a sphere), and `font`
  (glyph metrics; glyph-sheet preview). They reuse `asset`/`asset_file` as-is; only `media_type` and
  preview/normalization differ (see [`INGESTION.md`](INGESTION.md)).

---

## 12. Ratings & render-pipeline compatibility

Two curation/compatibility columns (migration M4):

```sql
ALTER TABLE asset   ADD COLUMN rating   INTEGER;  -- 1..5 stars; NULL = unrated
ALTER TABLE package ADD COLUMN pipeline TEXT;     -- 'built_in'|'urp'|'hdrp'|'mixed'; NULL = unknown/not applicable
ALTER TABLE project ADD COLUMN pipeline TEXT;     -- the registered editor's pipeline (from register_project)
```

- **Rating** — the classic DAM "my go-to assets" signal. Set manually (grid/inspector stars,
  keyboard 1–5); a numeric range filter in the query grammar
  ([`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md) §6) and a sort key. Never
  AI-written — it is pure user curation.
- **Pipeline** — which Unity render pipeline a pack targets, harvested at ingest
  ([`INGESTION.md`](INGESTION.md) §2): shader GUID references in `.mat` files (URP/HDRP shaders have
  well-known GUIDs; custom/`Standard` shaders ⇒ `built_in`) and `com.unity.render-pipelines.*`
  entries in any embedded package manifest. Packs shipping multiple variants are `mixed`. Compared
  against the registered project's pipeline on import → the **`pipeline_mismatch` advisory**
  ([`ENGINE-API.md`](ENGINE-API.md) §5.4). Material *conversion* is explicitly out of scope — we
  warn, we don't rewrite.

---

## 13. Migration ledger

`PRAGMA user_version` tracks the schema version; each migration runs exactly once inside a
transaction, in order, on open. Shipped so far:

| ver | Contents |
|---|---|
| **M1** | Base catalog: `import_source`, `media_type`, `asset`, `asset_file`, `tag`, `asset_tag`, `collection`(+staging index), `collection_asset`, `embedding_meta`, FTS5 `asset_fts` + triggers |
| **M2** | Mapping library: `taxonomy`, `category`, `category_alias`, `mapping_rule`, `asset_category` (§4) |
| **M3** | Engine round-trip: `package` + `asset.package_id`, `project`, `import_record`, `asset.license/attribution/license_url` (§7, §9) |
| **M4** | `asset.rating`, `package.pipeline`, `project.pipeline` (§12) |
| **M5** | UI-SPEC alignment (below) |
| **M6** | Extracted-pack persistence + surgical extraction: `asset_file.workspace_rel` (locates a package-extracted file in the content-addressed workspace, so extracted packs rehydrate across sessions behind the **Keep extracted packs cached** toggle, default off) + `package_entry` (per-payload guid/pathname/**tar offset**/size from the browse pass — selected assets re-extract with one gzip skip-scan instead of a full unpack, [`INGESTION.md`](INGESTION.md) §2) |
| **M7** | Unity Assets library pinning: `package.pinned` (`ALTER TABLE package ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0`) — backs the Add-to-Library pin (FEATURES.md §5.5, §7 above) so a promoted pack is a durable sidebar LIBRARIES row |
| **M8** | Semantic embeddings (interim): `asset_embedding` BLOB table (dim × f32 little-endian + `model_id`/`modality`/`media_type_id`/`source_id`) — `Catalog::upsert_embedding` clears `embedding_dirty` in-txn; `embeddings_for_knn` loads the pre-filtered candidate set for `crate::vector::BruteForceIndex`. sqlite-vec `vec_asset` is the drop-in optimisation behind the `VectorIndex` seam (§2). |
| **M9** | Technical facets: `asset.tri_count/width_px/height_px/duration_ms` + `asset.metrics_dirty` (partial index) — extracted from the file itself by a dirty-flag drain, exactly like embeddings (§14) |
| **M5** | UI-SPEC alignment: per-project staging trays, `import_record.batch_id` + backfill, `tag` trait columns, `asset_relationship` (final form), `import_source.blacklist` — DDL + data steps below |

### M5 — UI-SPEC alignment (shipped 2026-07-16)

Everything [`UI-SPEC.md`](UI-SPEC.md) needed that the M1–M4 schema lacked. Exact DDL
(as shipped in `catalog.rs`):

```sql
-- 1. Per-project staging trays (§9; UI-SPEC §16)
ALTER TABLE collection ADD COLUMN project_id INTEGER REFERENCES project(id) ON DELETE CASCADE;
DROP INDEX IF EXISTS idx_collection_staging;                       -- was: single shared tray
CREATE UNIQUE INDEX idx_collection_staging
  ON collection(project_id) WHERE is_staging=1;                    -- one tray per project

-- 2. Import batches (§9; UI-SPEC §16.1 "Recent exports" + batch undo)
ALTER TABLE import_record ADD COLUMN batch_id TEXT;
CREATE INDEX idx_import_batch ON import_record(batch_id);

-- 3. Trait Library (UI-SPEC §9)
ALTER TABLE tag ADD COLUMN is_trait   INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tag ADD COLUMN scope      TEXT;                        -- media_type; NULL = any
ALTER TABLE tag ADD COLUMN usage_note TEXT;                        -- fed to the identifier (AI-PIPELINE §6)
ALTER TABLE tag ADD COLUMN teach      INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tag ADD COLUMN icon       TEXT;

-- 4. Dependency graph (§10; UI-SPEC §13.6) — new table, created directly in its final form
CREATE TABLE asset_relationship (
  id            INTEGER PRIMARY KEY,
  from_asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  to_asset_id   INTEGER REFERENCES asset(id) ON DELETE CASCADE,    -- NULL ⇒ unresolved reference
  kind          TEXT NOT NULL,
  source        TEXT NOT NULL DEFAULT 'unity_meta',
  confidence    REAL,
  declared_path TEXT,
  state         TEXT NOT NULL DEFAULT 'resolved',                  -- 'resolved'|'library'|'missing'
  UNIQUE(from_asset_id, to_asset_id, kind)
);
CREATE INDEX idx_rel_from ON asset_relationship(from_asset_id);
CREATE INDEX idx_rel_to   ON asset_relationship(to_asset_id);

-- 5. Per-source blacklist moves into the catalog (§10; until M5 it lives in app config)
ALTER TABLE import_source ADD COLUMN blacklist TEXT;               -- JSON [{ "kind": "folder"|"asset", "value": "…" }]
```

**Data steps (same transaction, after the DDL):**

- **Staging**: the legacy shared tray (`is_staging=1`, now `project_id IS NULL`) is reassigned to
  the active project when one exists, else deleted with its `collection_asset` rows. After M5 a
  staging row with `project_id IS NULL` is invalid (the partial unique index does not police NULLs —
  the app enforces it, and each `register_project` creates that project's empty tray).
- **Batches**: backfill `batch_id` on existing `import_record` rows with one synthetic id per
  `(project_id, imported_at)` group, so pre-M5 imports appear as history rows (their file lists are
  reconstructable from `dest_path`, so batch undo works on them too).
- **Blacklist**: one-time import of the interim config blacklist (`LibraryProject.blacklist`) into
  `import_source.blacklist` by matching `root_path`; the config copy is then treated as dead and the
  crawler reads the catalog column. *(Shipped as `Catalog::adopt_config_blacklists` — the app calls
  it after open, since the catalog can't read the config file; it only fills sources whose catalog
  blacklist is still NULL.)*

Not in M5: MCP port/token, size tiers, quality, and cap settings stay in the app config file — they
are per-machine preferences, not catalog data.

---

## 14. Technical facets (extracted metrics)

`asset.tri_count`, `width_px`/`height_px` and `duration_ms` are the **technical facets**: measured
properties of the file rather than anything a model or a user asserts. They back the ⚑ Filters
Triangles chip, the inspector stat block, and the `tris:`/`duration:` structural filters of
[`IDENTIFICATION-AND-SEARCH.md`](IDENTIFICATION-AND-SEARCH.md) §6.

Rules:

- **Extraction is lazy and queued**, mirroring embeddings: `metrics_dirty=1` on insert and whenever
  a crawl sees changed bytes; `Catalog::assets_needing_metrics` is the queue, `set_metrics` stores
  the result and clears the flag. Writes happen on the single writer connection (ARCHITECTURE §5).
- **The flag clears even when nothing was extracted** — an unparseable file must not re-queue on
  every scan (the same forward-progress rule as the embedding drain's zero vector).
- **Every field is nullable.** A facet that does not apply to a media type stays NULL; so does one
  whose extraction failed. UI and filters treat NULL as "unknown", never as zero.
- Extraction is **read-only** on the source file, like everything else in the catalog (§6).
