# Data Model, Search & the Mapping Library

> Last reviewed: 2026-08-11 (check-docs verified every citation resolves)

The storage backbone, the schema, hybrid search, the user-defined "mapping library," and dedup.

---

## 1. Storage stack

**Planned MCP annotation work (2026-09-10):** [MCP-ASSET-ANALYSIS](MCP-ASSET-ANALYSIS.md)
refines S04/S05 of the v1 architecture plan. Reuse `tag`, `asset_tag`, the scalar asset description
and `op_log`; model-authored associations use that plan's producer-specific AI source convention.
Full provenance readback, atomic tags/description/FTS/history, conflict revisions and durable
idempotency receipts are requirements, not claims about the current schema. Migrations/retention
and conflict-aware undo remain owned by S03/S04; allocate actual migration numbers at implementation.
ENGINE-API §8 defines the wire fields. Human-only confirmation and embedding lifecycle rules in §8
below remain intact. No catalog migration is performed by this design update.

The clarified review flow (ENGINE-API §8.5) also records agent accept/reject decisions in the shared
versioned operation history, retaining original assignment provenance and reviewed revisions.
Reuse trait columns on `tag` and existing taxonomy/category tables for approved vocabulary changes;
do not conflate categories with namespaces or build another vocabulary database. History retention
still applies; an association rejection is not a persistent negative-training/suppression rule.

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
The agent planes' connection is the exception: `Catalog::open_reader` opens the file
`SQLITE_OPEN_READ_ONLY`, issues no pragma that writes, and refuses a schema version other than
`SCHEMA_VERSION` (v1 plan S11) — migrating is the writer's job.

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
  state           TEXT NOT NULL DEFAULT 'available',  -- 'available' | 'paused' | 'unavailable' (M18; UI-SPEC §11.1)
  last_seen_at    INTEGER,                -- unix seconds the root last resolved (M18)
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
  norm_name TEXT NOT NULL UNIQUE,         -- '<namespace>:<lowercased value>' — see below
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
  source     TEXT NOT NULL,               -- 'manual' | 'imported' | 'ai:<producer>' (M26; 'ai:legacy' = pre-M26, producer unknown)
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
  is_staging INTEGER NOT NULL DEFAULT 0,  -- a staging tray (§9); one per project, plus one with none
  project_id INTEGER REFERENCES project(id) ON DELETE CASCADE,  -- staging trays only; NULL = no editor yet
  query_json TEXT
);
-- Enforce one staging collection per project (trays shared across GUI / MCP / Unity panel, §9).
CREATE UNIQUE INDEX idx_collection_staging ON collection(project_id) WHERE is_staging=1;
-- …and exactly one tray belonging to no project (M20). The index above cannot say this: SQLite
-- treats NULLs as distinct in a UNIQUE index, so it constrains nothing for a project-less row.
CREATE UNIQUE INDEX idx_collection_staging_unprojected
  ON collection(is_staging) WHERE is_staging=1 AND project_id IS NULL;
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

-- Full-text search (M25): one canonical definition of an asset's searchable text…
CREATE VIEW asset_search_text AS
  SELECT a.id, a.name, a.description,
         <distinct tag names, sorted, space-joined> AS tags_concat
    FROM asset a;
-- …indexed by a plain FTS5 table. It keeps its own copy of the text, so a re-index is "delete the
-- rowid, insert the view's row" and no writer ever has to supply the old values.
CREATE VIRTUAL TABLE asset_fts USING fts5(
  name, description, tags_concat,
  tokenize='unicode61 remove_diacritics 2'
);
-- Triggers: asset INSERT; UPDATE OF name, description (only those — status, dirty-flag and
-- timestamp updates do no FTS work); DELETE (by rowid); tag rename. Tag writes re-index once per
-- asset, in the same transaction.
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
**One hard-filter set, both retrievers** (v1 plan S09). `eligible_for_search` is computed once per
query — live assets, of the requested media type — and applied to *both* sides before either ranks:
BM25's list is narrowed to it, and the vector side scores only those ids (`knn_eligible`). Filtering
after a top-K is how a trashed asset spends a slot a live one should have had.

**The vectors are resident, not reloaded** (M30). `Catalog::embedding_snapshot` keeps one immutable
snapshot per `(database, model)` and rebuilds only when that model's generation moves; every
embedding write bumps it inside its own transaction, so a headless process sees a GUI process's
write without being told. Measured on the reference library: 6,395 × 768 vectors are 18.7 MiB and
172 ms to load, against 66 µs to reuse — and every "more like this" used to pay the 172 ms. Peak
memory during a rebuild is the old snapshot plus the new one; three models stay resident at most.

3. **Fuse with RRF**: `score(d) = Σ 1/(k + rank_i(d))`, `k≈60`. RRF combines on **ranks**, sidestepping
   the BM25-vs-cosine score-incompatibility; reliably beats either retriever alone.
4. **(Optional) cross-encoder rerank** of the top-N for the final 10–20 — opt-in (cost/latency).

**Index sync:** because FTS5 and sqlite-vec are in the same file, every asset/tag mutation runs in one
transaction updating base table + FTS together — asset rows through triggers, tag writes through one re-index per
asset in the same transaction (M25). Embeddings are produced async: mark
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
  UNIQUE(source_id, package_path)       -- superseded by M13's UNIQUE(package_path)
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

### In-pack assets — analyze without extracting (M14)

A pack the user imports is **analyzed, not unpacked**: `read_manifest` ([`INGESTION.md`](INGESTION.md)
§2) is the whole cost, and it mints one **`asset` + `asset_file` per entry** whose bytes live *inside*
the archive. There is no "virtual asset" type — an asset is an asset; where its bytes come from is a
property of its file, resolved in this precedence:

1. `workspace_rel` set **and** that file exists → **extracted** (materialized on disk)
2. `asset_file.package_entry_id` set → **in-pack**: the bytes are `size_bytes` at `tar_offset` inside
   `package.package_path` (one gzip skip-scan, [`INGESTION.md`](INGESTION.md) §2)
3. else → **on-disk**: `import_source.root_path / rel_path`

*Materialized* is derived from that ladder, never a stored flag or a status — so nothing in the
schema forks on it. **M14** adds the single link:

```sql
ALTER TABLE asset_file ADD COLUMN package_entry_id INTEGER REFERENCES package_entry(id);
CREATE INDEX idx_file_pkgentry ON asset_file(package_entry_id) WHERE package_entry_id IS NOT NULL;
```

- **Import mints the rows.** Importing a pack (was: browse-only `package` row, `asset_count` NULL)
  now also writes an `asset`/`asset_file` per `package_entry`, `package_entry_id` set, `workspace_rel`
  NULL, `embedding_dirty`/`tags_dirty`/`deps_dirty` all queued. `asset.package_id` therefore joins
  immediately, so a pinned pack is a real sidebar **LIBRARIES** row *before* any extraction (§8, §5.5).
- **Dependency edges harvest with zero pulls.** `read_manifest` already parses each entry's `.meta`
  (GUID/labels), so the §10 GUID pass runs off the manifest — an in-pack asset has its graph before a
  byte is read.
- **Identify runs in place, one file at a time.** The embedding, metrics and tag drains resolve bytes
  through the ladder above; for an in-pack asset they surgically pull that one payload, identify it,
  then drop it — no workspace copy, no persistent identify cache (see §8 and
  [`INGESTION.md`](INGESTION.md) §2/§3). **Images** decode straight from the in-memory bytes
  (`raster::decode_bytes`). **Metrics, audio and tags (and 3D multiview embedding)** run through
  path-based extractors, so the pulled payload is spilled to an **ephemeral OS-temp scratch file**
  (`resolve_job_path`) that is deleted the instant the drain finishes with it — still not a workspace
  copy and not the persistent extract cache, just a decode buffer that happens to live on disk because
  those decoders take a path. Serial by design (the persist thread is the sole writer), and identify
  does not need to be fast.
- **Extraction is demoted to *materialize*.** `extract_at_offsets` fills `workspace_rel` on rows that
  already exist; it is only needed to leave a **persistent file on disk** (Add to project, or the
  "keep extracted" cache). Un-materializing (unpin/evict) clears `workspace_rel` and deletes the cached
  file — the `asset` row, its embedding and its tags **stay in `catalog.db`**, so re-opening the
  library is a pure DB read.

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
| `discovered`/`indexed` → `identified` | a row in `asset_embedding` (with `embedding_dirty = 0`) **and** at least one `asset_tag` |
| `identified → indexed` | `embedding_dirty = 1` again (the file's bytes changed, so the vector is stale), **or** the asset has no tags |

**`embedding_dirty` means the bytes changed — nothing else may raise it.** An **Identify** kick
(`mark_for_identify`, `flag_identify_all` / `_untagged` / `_assets`) re-tags; it does not alter a
single byte, so it leaves an existing vector alone and flags only an asset that has **no**
`asset_embedding` row — which the embed queue selects on, so a missing vector still gets built.
Raising it on a good vector makes an asset *less* promotable for having been identified, and on a
machine with no working embedder nothing ever lowers it again: a 200-asset agent-driven pass on
2026-08-30 tagged 195 and promoted **zero** for exactly that reason.

`confirmed` is human-set (the P4 accept/reject flow) and the reconcile never touches it. The pass is
idempotent, so it is safe to run after every drain.

**Reachable since 2026-09-06** — `set_confirmed` had no non-test caller for months, so the one
status a person sets could not be set. Identify's session set now offers **✓ Confirm all** over the
batch just appraised (`Op::SetConfirmed` → the single writer). It stays off both agent planes on
purpose: `confirmed` is the claim that a *human* checked the tags, and an MCP client signing off as
the user would empty the word. That matters more once a bulk cloud pass can tag thousands of assets
in a run — it is then the only record of which of them anyone actually looked at.

Both halves are required, and that is not pedantry: an embedding makes an asset *findable*, but
nothing has been **said** about it until it is tagged. While `identified` was earned by embeddings
alone (2026-07-22 → 07-23), the health meter reported 2067 of 2274 identified for a library where
19 assets had tags — the UI asserting work that had not happened. The condition landed the moment
that was noticed.

**The lifecycle is source-agnostic — an in-pack asset moves through it without ever being
extracted.** The drains that clear `metrics_dirty`, write `asset_embedding` and write `asset_tag`
resolve an asset's bytes through §7's precedence ladder, so an asset whose bytes are still inside a
`.unitypackage` earns `indexed` (facets + thumbnail from the one surgical pull) and then `identified`
(embedding **and** tag from that same pull) exactly like an on-disk file. Extraction is not a step in
this table — materializing a file on disk changes *where the bytes are*, not *how far it has been
processed*. A pack the user imports is therefore fully searchable from `catalog.db` after its identify
pass, with the archive touched only to preview full-res or to extract.

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
  - **Plus exactly one tray with `project_id IS NULL`** (M20, [ADR-0010](decisions/ADR-0010-one-staging-tray.md)):
    the tray you pick into **before** any editor is registered, which — until the Unity plugin
    exists — is every user. M5 deleted such rows on the reading that a shortlist means nothing
    without a destination; that is true of the agent plane and false of the app, and the shell
    answered it by keeping a second tray in memory that no other plane could see. One resolver,
    `Catalog::active_staging_tray_id`, answers "the tray right now" for both planes: the active
    project's, else this one.
  - **The first `register_project` adopts it**, picks and all, rather than stranding them in a tray
    nothing resolves to — the same move M5's data step made for the legacy shared tray. A *second*
    project gets its own empty tray, so trays still never mix.
- **Import batches**: `import_record` rows from one tray/queue run share a `batch_id`, so the UI can
  list "Recent exports" per project and **undo a batch** — delete the copied files it produced and
  its records; catalog assets are untouched (confirm-gated, [`UI-SPEC.md`](UI-SPEC.md) §16.1).
**Delete from disk is the one destructive exception.** The catalog is otherwise non-destructive —
Trash is a soft delete and originals are never touched. The Settings danger flow may permanently
remove a file, and it is fenced: an explicit confirmation modal naming the file count, a guard that
refuses any path outside the user's source roots and the app's own extraction workspace, and the
catalog row is marked trashed **only after** the file actually left the disk, so a failed delete
cannot leave the catalog claiming otherwise.

**Favourites** (`asset.favorite`, M12) are a **pin, not a rating**: "I want to find this again" is
a different question from "how good is this", and folding the two together would make every 5-star
asset a favourite by accident. Catalog-only like ratings and the Trash — the file on disk is never
touched — and the pin survives a trip through the Trash, so restoring an asset brings its
favourite back. Surfaced per the design bundle's prototype as a **star**: `☆`/`★` top-right of the
card thumbnail (`GOLD`, `GOLD_BRIGHT` on hover), `F` on the selection, a **★ Favorites** toolbar
toggle carrying a gold count pill, and a dedicated **Favorites screen** second in the nav. A trashed
asset stops being listed as a favourite because it has left the grid.

- **License** resolves **asset → package**: an asset's effective license is `asset.license` when set,
  else `package.license`. Surfaced on the detail stat block and on every export/import so credits
  travel with the asset.

---

### 9.1 Export history (`export_record`, M19)

An **export to a folder** is not an import, and `import_record` cannot hold one: its `project_id`
is `NOT NULL` and its `dest_path` is project-relative, resolved against the project root at undo
time. A folder export has no project and an absolute destination the user chose, so it gets its own
table — which also keeps the two verbs apart the way UI-SPEC §16.2 does (**Import** into a project,
**Export** to anywhere else).

| column | meaning |
|---|---|
| `batch_id` | one export run; the unit Recent-exports lists and undo acts on |
| `asset_id` | the asset it came from, `ON DELETE SET NULL` — trashing an asset later must not erase the record that a copy of it is still sitting in someone's folder |
| `dest_path` | the absolute folder the run wrote into |
| `file_name` | **one row per file written**, because undo deletes files and a dependency closure turns one pick into several |
| `exported_at` | unix seconds |

Rows are written from what was **actually copied**, never from what was requested: an export skips
in-pack and unreachable picks and a cancelled run stops part-way, so a history built from the
request would point undo at files that were never created. Both export paths record — the desktop's
**⤓ Export to folder** and the agent plane's `export_asset` — because an export an agent ran still
put files on the user's disk.

Undo is the one delete in the app that touches a location the *user* chose, so it is fenced rather
than trusted: `file_name` must be a bare name, the file must sit directly inside the recorded
`dest_path`, and that destination is re-checked against the export sandbox **at undo time** (a
folder that was legal to export into may since have become a library source root). A missing file
counts as already-undone; a run that could not finish keeps its record, so copies still on disk stay
findable.

---

## 10.## 10. Library Source labeling & asset relationships

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
**Availability** (`state`, `last_seen_at` — M18). A source root is checked when the library opens and
after every scan: one `is_dir` per **root**, never per asset, off the UI thread, defaulting to
`available` while pending. A missing *file* is a per-asset fact and never marks the source. Adding a
root that does not exist is refused up front; a root that goes away **after** indexing is the normal
end state for external drives and moved folders, and is what `unavailable` exists for. Rows are never
deleted, trashed or purged because their source went away — the catalog stays fully browsable and
only the file access fails (UI-SPEC §11.1). Relinking rewrites `root_path` and re-resolves
`asset_file.rel_path` against the new root; the match count is shown before it commits.

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
-- source:      'unity_meta'   (harvested from .meta / material GUID refs)
--              | 'unity_prefab' (the binding a .prefab declares: mesh ↔ material)
--              | 'inferred'     (path-proximity / naming heuristics)
--              | 'manual'       (user-asserted)
CREATE TABLE asset_relationship (
  id            INTEGER PRIMARY KEY,
  from_asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  to_asset_id   INTEGER REFERENCES asset(id) ON DELETE CASCADE,  -- NULL ⇒ unresolved reference
  kind          TEXT NOT NULL,
  source        TEXT NOT NULL DEFAULT 'unity_meta',
  confidence    REAL,                     -- 0..1; null for unity_meta (deterministic)
  declared_path TEXT,                     -- the path/name the parent declared (FBX binding, .mtl, GUID)
  state         TEXT NOT NULL DEFAULT 'resolved',  -- 'resolved' | 'missing' | 'ignored' (muted by hand)
  role          TEXT,                     -- M29: the property a texture binding fills ('_MainTex', '_BumpMap'…)
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
1b. **Unity prefab bindings** — a stock FBX embeds its materials, so the model asset declares
   nothing and step 1 can never produce a `uses_material` edge. The `.prefab` is where the binding
   is written down (`m_Mesh` + `m_Materials` GUID refs), so prefabs are read as *evidence about*
   catalogued assets — they are not assets themselves (`prefab` is not a `media_type`). Pairs land
   as `source = 'unity_prefab'`, `confidence 0.9`. The file is read per YAML document: a
   `MeshRenderer` pairs with the `MeshFilter` on its own GameObject (`m_GameObject`), a
   `SkinnedMeshRenderer` with its own `m_Mesh`, and a `ParticleSystemRenderer` in mesh mode
   paints its meshes with its first material (the second slot is its trail's). A `MeshCollider`'s
   mesh and a particle system's emission-shape mesh are not painted. Never a whole-file mesh ×
   material product
   (superseded 2026-09-11, [ADR-0018](decisions/ADR-0018-prefabs-are-scoped-evidence.md)). Each binding keeps its renderer and slot.
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
- **Grid badge:** a small link-chain mark on cards with **outbound** edges — what an export would
  drag along (being *used by* something costs an export nothing). Painted as two interlocking rings
  rather than typed: the bundled text fonts have no chain glyph. Hover preview of the parents is
  not built.
- **Search:** `has_dependents` / `has_dependencies` as boolean filter chips — shipped as the
  ⚑ Filters **LINKS** section (*uses others* / *used by others*, single-select; an asset in the
  middle of a chain satisfies either). `"textures used by
  assets from source: Synty"` resolves via a two-hop join.
- **Export/import with deps:** when importing an asset into Unity, the relationship graph is walked
  to offer "import with dependencies" — selecting a material auto-selects its textures. Already
  referenced in [`ENGINE-API.md`](ENGINE-API.md) §4; `asset_relationship` is the backing table.

**What harvesting looks like in practice.** Step 1 runs as a drain off `asset.deps_dirty` (M10),
beside the facet and embedding drains: each dirty asset's own GUID is read from its `.meta` sidecar
into `asset.unity_guid`, and a material's `m_Texture` bindings become `uses_texture` edges. The flag
clears either way, so an asset with no Unity bookkeeping isn't rescanned forever.

An edge is recorded even when its target isn't catalogued yet — `to_asset_id NULL`, `state='missing'`,
`declared_path` holding the GUID — because ordering is not guaranteed (a texture may be indexed after
the material that uses it, or live in a package extracted later). `Catalog::resolve_pending_relationships`
late-binds those in one statement after every drain, which is why a `missing` edge is kept rather than
dropped. An edge that never resolves is an honest record of a reference the library doesn't hold.

**Two exceptions to "record it and let it resolve later".** A reference to Unity's own built-in
resource libraries — the default material a fresh prefab paints with, the white/bump fallback
textures — is dropped at harvest (`deps::guid_in`, three well-known GUIDs), because nothing in any
catalog will ever *be* that file: the edge would be `missing` for good and would make the Inspector
warn about a link that is not broken (M22 cleared the ones already minted). And a user can **mute**
an edge — `state='ignored'`, UI-SPEC §13.6's ⊘ — which keeps the row but clears `to_asset_id`, so a
muted link counts as a dependency nowhere. Muting is a state rather than a `DELETE` because
`record_dependencies` re-derives an asset's whole harvest from the file: a deleted edge would come
back the moment the bytes changed. `resolve_pending_relationships` skips ignored rows, and restoring
one returns it to `missing` and lets the ordinary late-binding pass re-resolve it.

`Catalog::dependency_closure` walks the resolved edges transitively (recursive `UNION`, so a
reference cycle terminates) and is the single definition of "with dependencies" for both
`AssetDetail.dependencies` and `export_asset`.

**Prefab evidence is kept, and the edges are derived from it** (M28, [ADR-0018](decisions/ADR-0018-prefabs-are-scoped-evidence.md)). Each
`.prefab` is recorded under the scope it was walked in — a source root or the package workspace —
with the size/mtime it was read at; what it declares is one row per renderer slot:

```sql
CREATE TABLE prefab_file (
  id    INTEGER PRIMARY KEY,
  scope TEXT NOT NULL,     -- the root walked: a source root, the package workspace, or 'legacy' (M28)
  path  TEXT NOT NULL,     -- the evidence locator
  size  INTEGER NOT NULL,  -- (size, mtime) is the revision: only a prefab whose revision moved is re-read
  mtime INTEGER NOT NULL,
  UNIQUE(scope, path)
);
CREATE TABLE prefab_binding (
  prefab_id     INTEGER NOT NULL REFERENCES prefab_file(id) ON DELETE CASCADE,
  game_object   INTEGER NOT NULL,  -- the owning GameObject's fileID within the prefab
  renderer      INTEGER NOT NULL,  -- the Mesh-, SkinnedMesh- or ParticleSystemRenderer document's fileID
  slot          INTEGER NOT NULL,  -- index into m_Materials; an empty or built-in slot keeps its index
  mesh_guid     TEXT NOT NULL,
  material_guid TEXT NOT NULL,
  PRIMARY KEY (prefab_id, renderer, slot, mesh_guid)  -- a particle renderer paints up to 4 meshes from one slot
);
```

`Catalog::reconcile_prefab_edges` derives one `uses_material` edge per distinct (catalogued mesh,
material GUID) the bindings name — so a mesh catalogued after its prefab was read gets its edge on
the next pass — and removes an edge no binding backs any more, except a muted one, which is never
deleted or duplicated. A walk re-reads only prefabs whose revision moved. A walk that completed
forgets the prefabs it no longer found; one that did not — a source whose drive is gone, a directory
it could not open — keeps what it said before. The pass runs after a dependency drain that scanned
something, after a watch rescan and a manual scan over their roots (a prefab-only edit changes no
asset row), and after an extraction over the workspace. Nested prefab instances and variant
overrides are counted and left unresolved: an override is a property modification on another
prefab's renderer, not a binding. A material the model file embeds names the model's own GUID and
mints no self-edge.

**Which texture paints a preview** (M29, v1 plan S07). A `uses_texture` edge harvested from a
material records the property it fills (`role`). `Catalog::material_albedo_texture` takes the
binding to an albedo property (`_BaseMap`, `_MainTex`…); a normal, mask, height or emission map is
never an albedo, and an edge without a role — name-matched, or harvested before M29 — is judged by
the texture's file name (`albedo_rank`). A model's inputs come per slot
(`Catalog::model_material_inputs`): from the prefab renderers that draw its mesh first; when
several prefabs paint it differently the most common appearance is taken, ties by prefab path, and
the others are counted as `alternatives`, never merged. Without prefab evidence the resolved
`uses_material` edges give the slots in edge order, and without those a name-matched texture paints
slot 0. Only resolved, unmuted links to live textures whose file is present count. The model file's
own embedded or declared textures (`mesh::load_textured`) outrank all of this where they exist.
Previews only: the embedding render keeps the lowest-id texture (`material_albedo_paths`), so no
vector moves until re-embedding is decided.

> **Coverage today.** `uses_texture` (material → texture, from the material YAML) and
> `uses_material` (mesh → material, from prefabs) — so a model's closure reaches its textures
> through its material. **Step 2's naming heuristic is in** (2026-07-29): a model with *no* edge at
> all (a raw export — no `.meta`, so no GUID) gets `uses_texture` edges to sibling textures in the
> same source folder whose file stem name-matches it (`Human3.fbx` → `Human3_Albedo.png`), as
> `source='inferred'`, `confidence 0.7`. It applies only to edge-less models, so it never overrides
> the deterministic GUID/prefab graph; the name match is `deps::texture_matches_model` (exact stem or
> stem + separator), run in the dependency drain over `Catalog::edgeless_models` ×
> `inference_textures`. Not yet: `uses_mesh`/`uses_audio`/`uses_script` from prefabs. Prefabs inside
> the extracted package workspace are read since M28, and in-pack assets take their Unity GUID from
> the package manifest — an extracted pack has no `.meta` beside its files.

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

- **Rating** — **parked, not part of the product surface (2026-08-03).** The column stays and the
  read paths keep carrying it, but no rating UI ships in the Tauri frontend: nobody else browses
  your library, so scoring your own files earns nothing that Favourites (a pin) does not already
  give. It becomes worth building **if Lodestash ever hosts or distributes assets**, where a rating
  is a signal between users rather than a note to yourself — which is why the column is left in
  place rather than migrated away. The egui build's stars remain until B7 deletes that crate.
  Never AI-written in any case — it is pure user curation.
- **Pipeline** — which Unity render pipeline a pack targets, harvested at ingest
  ([`INGESTION.md`](INGESTION.md) §2, which owns *how* — names the pack states, not shader GUIDs;
  see the narrowing recorded there). Packs shipping a variant per pipeline are `mixed`, and a pack
  we cannot classify stays **NULL**, which is "unknown", not Built-in. Compared against the
  registered project's pipeline on import → the **`pipeline_mismatch` advisory**
  ([`ENGINE-API.md`](ENGINE-API.md) §5.4), which is silent whenever either side is unknown or the
  pack is `mixed`. Material *conversion* is explicitly out of scope — we warn, we don't rewrite.

  *(Shipped 2026-08-28 — `lodestash_ingest::pipeline` + `Catalog::set_package_pipeline`, written by
  the desktop import path.)*

---

## 13. Migration ledger

`PRAGMA user_version` tracks the schema version; each migration runs exactly once inside a
transaction, in order, on open.

**Lifecycle** (v2 spec §4.2; `Catalog::open` / `Catalog::open_reader` in
`crates/lodestash-core/src/catalog/mod.rs`):

- `open` creates a missing catalog and migrates an existing one to `SCHEMA_VERSION`. A catalog at a
  **newer** version is refused with `CoreError::SchemaTooNew` and left untouched — an older build
  never writes a schema it does not know.
- Before the first step touches an **existing** catalog (version > 0), it is copied with
  `VACUUM INTO` to `<catalog>.v<from>.bak` beside it — one consistent snapshot even while another
  connection writes, written under a temporary name and renamed, so a backup is whole or absent. The
  newest two backups are kept. A new file and a current one are not backed up.
- Each step takes the write lock first (`BEGIN IMMEDIATE`) and re-reads `user_version` under it, so
  two processes opening one file apply every step once; the loser of the race skips the steps the
  winner applied. A failing step rolls back alone and the open fails, leaving the catalog at the
  last good version with its backup beside it.
- `open_reader` never migrates: it refuses an older catalog (`SchemaTooOld`, "open the Lodestash
  app once") and a newer one (`SchemaTooNew`).

Shipped so far:

| ver | Contents |
|---|---|
| **M1** | Base catalog: `import_source`, `media_type`, `asset`, `asset_file`, `tag`, `asset_tag`, `collection`(+staging index), `collection_asset`, `embedding_meta`, FTS5 `asset_fts` + triggers |
| **M2** | Mapping library: `taxonomy`, `category`, `category_alias`, `mapping_rule`, `asset_category` (§4) |
| **M3** | Engine round-trip: `package` + `asset.package_id`, `project`, `import_record`, `asset.license/attribution/license_url` (§7, §9) |
| **M4** | `asset.rating`, `package.pipeline`, `project.pipeline` (§12) |
| **M31** | Catalog identity (v1 plan S11): `catalog_meta(key, value)` holding a random `catalog_id`, minted by the migration. A path is not an identity — it changes when the file moves — and a headless MCP server must only forward to an app serving the same catalog (ENGINE-API §1). Readers refuse any schema but their own (`Catalog::open_reader`), so an agent host connecting first cannot upgrade a catalog the app has not |
| **M30** | Embedding generations (v1 plan S09): `embedding_generation(model_id, generation, updated_at)`, bumped inside every `upsert_embedding` transaction. A resident vector snapshot (§3) is only safe if staleness is a fact in the catalog — a GUI process cannot notify an independent headless one. Existing models start at generation 1; no vector is touched and the cache starts empty on every restart |
| **M29** | Texture roles (v1 plan S07): `asset_relationship.role` holds the material property a `uses_texture` edge fills (`_MainTex`, `_BumpMap`…), so a preview paints with the albedo instead of the lowest-id texture (§10). Every live material is queued for one more dependency scan to fill it; an edge without a role is judged by its texture's file name |
| **M28** | Prefab evidence (v1 plan S06, [ADR-0018](decisions/ADR-0018-prefabs-are-scoped-evidence.md)): `prefab_file` (scope, path, size/mtime revision) and `prefab_binding` (GameObject, renderer, slot, mesh GUID, material GUID); the `unity_prefab` edges are derived from them (§10). The edges already recorded were whole-file pairings no row attributes to a prefab, so they become the evidence of the reserved scope `legacy`: a source's legacy pairings are retired once a walk of that source completes, and the rest once a walk of every scope does — no edge is deleted before a reharvest has replaced it. In-pack assets take the GUID their package manifest names, and materialized in-pack rows are queued for the dependency scan the archive could not give them |
| **M27** | `tag_request` — durable receipts for agent `write_tags` requests (ENGINE-API §8.3): request id, digest of the validated body, the history row and the terminal result, written in the batch's own transaction. Not the undo log: that tail is capped, and a retry must find its receipt after its history has scrolled away. Same id and body replays the recorded result; same id with a different body is `request_conflict` |
| **M26** | Tag provenance ([ADR-0017](decisions/ADR-0017-producer-scoped-atomic-tag-batches.md)). `op_log.origin` (`user` default — every earlier row was a person's) partitions undo retention: the interactive tail and the background lane keep 200 rows each. Every pre-existing `asset_tag.source = 'ai'` row becomes `ai:legacy` with its recorded `model_id` kept: the local drain had stored zero-shot proposals under the VLM's model id and a cloud pass its local fallback under the cloud model's, so the producer is unrecoverable and is not invented. New AI rows are `ai:<producer>`; every write goes through `Catalog::apply_tag_batch` — one transaction with its FTS re-index and a versioned before/after history row |
| **M25** | FTS over a canonical view (v1 plan S03). `asset_search_text` (id, name, description, distinct sorted tag names) is the one definition of searchable text, and `asset_fts` becomes a plain FTS5 table populated from it. M1 had named `asset` as the external-content table while `tags_concat` is not an `asset` column: MATCH returned rowids while reading the text back and FTS5's `rebuild` both failed (`no such column: T.tags_concat`). Triggers narrowed to `UPDATE OF name, description`, so status, flag and timestamp updates do no FTS work; an asset delete is a rowid delete; a tag rename re-indexes its assets. Derived state only — 20 ms on the 8,237-asset reference catalog, where it also dropped a stale `metal` token M24 had kept for an asset whose tags no longer said so. *Divergence from the plan's wording:* the plan asked for external content over the view; a plain table was chosen so that no writer has to supply the old values, which is the requirement behind M11's stale tokens |
| **M24** | Re-queue the assets the broken API tagger swallowed: `UPDATE asset SET tags_dirty = 1 WHERE tags_dirty = 0 AND NOT EXISTS (asset_tag)`. `OpenAiTagger` reads `TagInput::image`, the generic batch path never fills it (the caption phase is meant to carry the picture), so it short-circuited to an empty result **without one HTTP request** and the caller stored that as finished — clearing `tags_dirty` on assets no model had seen. An empty write leaves no `asset_tag` row and therefore no `model_id`, so the guilty rows cannot be targeted precisely; the repair takes every asset marked done with no tags. It over-covers by the assets a local pass honestly found nothing for, which cost one pass to redo, while a swallowed asset is invisible for ever. Safe against `asset_fts_au` (which re-indexes `tags_concat = ''`): every row it touches has no tags |
| **M23** | `package.keep_cached` — per-pack workspace retention (UI-SPEC §11.6 / §15). Deliberately **not** `pinned`, which already means "Add to Library" (M7): whether a pack appears in the grid and whether its extracted bytes are worth hundreds of megabytes are different questions, and one switch answering both would make freeing disk space cost the pack its visibility |
| **M22** | Drop unresolved edges declaring a Unity built-in resource GUID (`f000…`/`e000…`/`d000…`). A prefab painted with Unity's *default* material minted an edge nothing could ever resolve, so §13.6 warned "missing links are skipped on export" about a link that is not broken. `deps::guid_in` now filters these at harvest; this clears the already-minted ones |
| **M21** | Asset Store EULA backfill: `UPDATE package SET license/license_url WHERE publisher IS NOT NULL AND license IS NULL`. Data, not DDL — the harvest could always state this (a `publisher` means the pack came out of the store cache tree), but the desktop write path never called the setter, so every pack it imported recorded no licence. Guarded on `license IS NULL`, so a pack that stated its own keeps it |
| **M20** | One staging tray with no project: `idx_collection_staging_unprojected`. Relaxes M5's rule that a tray must belong to a project, so the shell can stop keeping its own in-memory shortlist beside the catalog's ([ADR-0010](decisions/ADR-0010-one-staging-tray.md)) |
| **M18** | Source availability: `import_source.state` + `last_seen_at` — the third source state a drive going away needs (UI-SPEC §11.1). `state` is **derived, not authored**: the open/scan health check writes it, `paused` is the user's `watch_enabled` intent surfaced as a state, and nothing in the catalog is deleted because a root went missing |
| **M5** | UI-SPEC alignment (below) |
| **M6** | Extracted-pack persistence + surgical extraction: `asset_file.workspace_rel` (locates a package-extracted file in the content-addressed workspace, so extracted packs rehydrate across sessions behind the **Keep extracted packs cached** toggle, default off) + `package_entry` (per-payload guid/pathname/**tar offset**/size from the browse pass — selected assets re-extract with one gzip skip-scan instead of a full unpack, [`INGESTION.md`](INGESTION.md) §2) |
| **M7** | Unity Assets library pinning: `package.pinned` (`ALTER TABLE package ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0`) — backs the Add-to-Library pin (FEATURES.md §5.5, §7 above) so a promoted pack is a durable sidebar LIBRARIES row |
| **M8** | Semantic embeddings (interim): `asset_embedding` BLOB table (dim × f32 little-endian + `model_id`/`modality`/`media_type_id`/`source_id`) — `Catalog::upsert_embedding` clears `embedding_dirty` in-txn; `embeddings_for_knn` loads the pre-filtered candidate set for `crate::vector::BruteForceIndex`. sqlite-vec `vec_asset` is the drop-in optimisation behind the `VectorIndex` seam (§2). Since M30 the loaded set is held per model generation rather than rebuilt per query. |
| **M9** | Technical facets: `asset.tri_count/width_px/height_px/duration_ms` + `asset.metrics_dirty` (partial index) — extracted from the file itself by a dirty-flag drain, exactly like embeddings (§14) |
| **M14** | In-pack assets — analyze without extracting (§7 "In-pack assets"): `asset_file.package_entry_id` (+ partial index) links a file to the `package_entry` locator holding its `tar_offset`/`size_bytes`, so a file's bytes resolve to *extracted* (`workspace_rel`) → *in-pack* (`package_entry_id`) → *on-disk* (`rel_path`). Importing a pack mints an `asset`/`asset_file` per entry (locator set, `workspace_rel` NULL); the embedding/tag/deps drains identify it in place by surgically pulling one payload at a time, and extraction is demoted to filling `workspace_rel` when a persistent on-disk copy is actually wanted. No `media_type` change; no lifecycle fork (materialized is derived, not stored) |
| **M13** | One `package` row per archive: `UNIQUE INDEX idx_package_path ON package(package_path)`, replacing M3's `UNIQUE(source_id, package_path)`. A pack is identified by the file on disk, not by whichever source found it — the same `.unitypackage` reachable from both a folder source and the Asset Store cache source used to get two rows, so the Unity Assets screen listed it twice and the pin, the asset count and the recorded locators landed on different ones. The migration merges duplicates onto the row that carries data (extracted, then pinned, then lowest id), repointing `asset.package_id` and `package_entry` (`INSERT OR IGNORE` — the same GUID recorded twice is one fact, not a conflict). `upsert_package` now keys on the path and fills a blank publisher/category from a later sighting |
| **M12** | Favourites: `asset.favorite` + partial index — a pin, independent of `rating` (§12) |
| **M11** | Tagging: `asset.tags_dirty` (partial index) — the fourth drain flag — and the three `asset_fts` triggers **replaced** so `tags_concat` is a real denormalisation of `asset_tag` instead of the literal `''` they wrote before (a rename would otherwise `delete` under the wrong text and strand tokens in the index) |
| **M10** | Dependency scan queue: `asset.deps_dirty` (partial index) + a partial index on `asset.unity_guid` — the third drain flag beside `embedding_dirty`/`metrics_dirty`, cleared once an asset's `.meta` GUID and outbound refs are harvested into `asset_relationship` (§10) |
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
