//! Catalog — SQLite-backed asset store.
//! Schema spec and invariants: docs/DATA-MODEL.md §2–10.
//!
//! Migration strategy: `PRAGMA user_version` tracks the schema version;
//! `apply_migrations` runs numbered SQL steps in order on first open.

use std::path::Path;

use rusqlite::{params, Connection};
use tracing::info;

use crate::error::{CoreError, Result};

/// The fully-opened, migration-applied catalog database.
pub struct Catalog {
    pub(crate) conn: Connection,
}

impl Catalog {
    /// Open (or create) the catalog at `path`. Runs all pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).map_err(|e| CoreError::Catalog(format!("open failed: {e}")))?;

        configure_connection(&conn)?;
        let mut catalog = Self { conn };
        catalog.apply_migrations()?;
        Ok(catalog)
    }

    /// In-memory catalog (tests / CLI dry-run).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| CoreError::Catalog(format!("in-memory open failed: {e}")))?;
        configure_connection(&conn)?;
        let mut catalog = Self { conn };
        catalog.apply_migrations()?;
        Ok(catalog)
    }

    fn schema_version(&self) -> Result<u32> {
        self.conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(|e| CoreError::Catalog(format!("user_version query: {e}")))
    }

    fn apply_migrations(&mut self) -> Result<()> {
        self.apply_migrations_up_to(u32::MAX)
    }

    /// Run pending migrations up to and including `max_version` (tests use this to
    /// build a fixture at an older schema, then re-run to exercise one migration's
    /// data steps). Each migration runs inside its own transaction (DATA-MODEL §13).
    fn apply_migrations_up_to(&mut self, max_version: u32) -> Result<()> {
        let current = self.schema_version()?;
        let migrations: &[(u32, &str)] = &[
            (1, MIGRATION_01_BASE_SCHEMA),
            (2, MIGRATION_02_PACKAGES),
            (3, MIGRATION_03_PROJECTS_IMPORTS),
            (4, MIGRATION_04_RATING_PIPELINE),
            (5, MIGRATION_05_UI_SPEC_ALIGNMENT),
        ];

        for (version, sql) in migrations {
            if current < *version && *version <= max_version {
                info!(version, "applying schema migration");
                let tx = self
                    .conn
                    .transaction()
                    .map_err(|e| CoreError::Catalog(format!("migration {version} begin: {e}")))?;
                tx.execute_batch(sql)
                    .map_err(|e| CoreError::Catalog(format!("migration {version} failed: {e}")))?;
                // PRAGMA user_version cannot be parameterised — format is safe (u32 literal).
                tx.execute_batch(&format!("PRAGMA user_version = {version};"))
                    .map_err(|e| CoreError::Catalog(format!("set user_version: {e}")))?;
                tx.commit()
                    .map_err(|e| CoreError::Catalog(format!("migration {version} commit: {e}")))?;
            }
        }
        Ok(())
    }

    /// Seed the static `media_type` lookup rows (idempotent).
    pub fn seed_media_types(&self) -> Result<()> {
        const TYPES: &[(&str, &str)] = &[
            ("model_3d", "3D Model"),
            ("texture", "Texture"),
            ("audio", "Audio"),
            ("image", "Image"),
            ("sprite", "Sprite (Animated 2D)"),
            ("material", "Material"),
            ("font", "Font"),
        ];
        for (code, label) in TYPES {
            self.conn
                .execute(
                    "INSERT OR IGNORE INTO media_type (code, label) VALUES (?1, ?2)",
                    params![code, label],
                )
                .map_err(|e| CoreError::Catalog(format!("seed media_type: {e}")))?;
        }
        Ok(())
    }

    /// One-time M5 data step driven by the app after open: copy the interim config
    /// blacklists (`LibraryProject.blacklist`) into `import_source.blacklist`, matched
    /// by `root_path`. Only fills sources that have no catalog blacklist yet — the
    /// catalog column is authoritative afterwards (DATA-MODEL §13). Returns how many
    /// sources were updated.
    pub fn adopt_config_blacklists(&self, entries: &[(String, String)]) -> Result<usize> {
        let mut updated = 0;
        for (root_path, blacklist_json) in entries {
            updated += self
                .conn
                .execute(
                    "UPDATE import_source SET blacklist = ?2
                     WHERE root_path = ?1 AND blacklist IS NULL",
                    params![root_path, blacklist_json],
                )
                .map_err(|e| CoreError::Catalog(format!("adopt blacklist: {e}")))?;
        }
        Ok(updated)
    }
}

fn configure_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA cache_size=-32768;  -- 32 MiB page cache
         PRAGMA temp_store=MEMORY;",
    )
    .map_err(|e| CoreError::Catalog(format!("configure pragmas: {e}")))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Migration SQL
// ---------------------------------------------------------------------------

/// M1 — core tables: sources, media types, assets, files, tags, collections, embeddings, FTS5.
/// Spec: DATA-MODEL.md §2 (base schema).
const MIGRATION_01_BASE_SCHEMA: &str = "
-- Sources / libraries
CREATE TABLE IF NOT EXISTS import_source (
    id              INTEGER PRIMARY KEY,
    uuid            TEXT    NOT NULL UNIQUE,
    name            TEXT    NOT NULL,
    root_path       TEXT    NOT NULL,
    kind            TEXT    NOT NULL,       -- 'folder'|'asset_store_library'|'unitypackage'|'bundle'|'unreal_pak'
    last_scanned_at INTEGER,
    watch_enabled   INTEGER NOT NULL DEFAULT 1
);

-- Media-type taxonomy (seeded at runtime via seed_media_types)
CREATE TABLE IF NOT EXISTS media_type (
    id    INTEGER PRIMARY KEY,
    code  TEXT NOT NULL UNIQUE,            -- model_3d|texture|audio|image|sprite|material|font
    label TEXT NOT NULL
);

-- Logical asset
CREATE TABLE IF NOT EXISTS asset (
    id               INTEGER PRIMARY KEY,
    uuid             TEXT    NOT NULL UNIQUE,
    source_id        INTEGER NOT NULL REFERENCES import_source(id),
    media_type_id    INTEGER NOT NULL REFERENCES media_type(id),
    name             TEXT    NOT NULL,
    description      TEXT,
    primary_file_id  INTEGER,
    content_hash     TEXT,                 -- BLAKE3 of primary file
    unity_guid       TEXT,                 -- from .meta
    is_content       INTEGER NOT NULL DEFAULT 1,
    content_kind     TEXT,                 -- 'model_3d'|'audio'|'image'|'material'|'prefab'|'script'|...
    status           TEXT    NOT NULL DEFAULT 'discovered',  -- discovered→indexed→identified→confirmed
    dominant_color   TEXT,
    variant_group_id INTEGER,
    embedding_dirty  INTEGER NOT NULL DEFAULT 1,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    deleted_at       INTEGER
);
CREATE INDEX IF NOT EXISTS idx_asset_source  ON asset(source_id);
CREATE INDEX IF NOT EXISTS idx_asset_media   ON asset(media_type_id);
CREATE INDEX IF NOT EXISTS idx_asset_hash    ON asset(content_hash);
CREATE INDEX IF NOT EXISTS idx_asset_dirty   ON asset(embedding_dirty) WHERE embedding_dirty=1;
CREATE INDEX IF NOT EXISTS idx_asset_content ON asset(is_content)      WHERE is_content=1;
CREATE INDEX IF NOT EXISTS idx_asset_status  ON asset(status);
CREATE INDEX IF NOT EXISTS idx_asset_variant ON asset(variant_group_id) WHERE variant_group_id IS NOT NULL;

-- Physical files (LODs, PBR maps, variants)
CREATE TABLE IF NOT EXISTS asset_file (
    id              INTEGER PRIMARY KEY,
    asset_id        INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    rel_path        TEXT    NOT NULL,      -- relative to import_source.root_path
    role            TEXT,                  -- 'mesh'|'lod0'|'albedo'|'normal'|'variant'|'preview'
    ext             TEXT,
    size_bytes      INTEGER,
    mtime           INTEGER,
    content_hash    TEXT,
    perceptual_hash TEXT,
    missing         INTEGER NOT NULL DEFAULT 0,
    UNIQUE(asset_id, rel_path)
);
CREATE INDEX IF NOT EXISTS idx_file_hash  ON asset_file(content_hash);
CREATE INDEX IF NOT EXISTS idx_file_phash ON asset_file(perceptual_hash);
CREATE INDEX IF NOT EXISTS idx_file_asset ON asset_file(asset_id);

-- Tags (raw vocabulary)
CREATE TABLE IF NOT EXISTS tag (
    id        INTEGER PRIMARY KEY,
    name      TEXT NOT NULL,
    norm_name TEXT NOT NULL UNIQUE,        -- lowercased/trimmed
    namespace TEXT,                        -- subject|style|material|color|mood|usage|audio|technical|free
    color     TEXT,
    parent_id INTEGER REFERENCES tag(id)
);
CREATE TABLE IF NOT EXISTS asset_tag (
    asset_id   INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES tag(id),
    source     TEXT    NOT NULL,           -- 'manual'|'ai'|'imported'
    confidence REAL,
    model_id   TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (asset_id, tag_id, source)
);
CREATE INDEX IF NOT EXISTS idx_assettag_tag ON asset_tag(tag_id);

-- Collections (+ staging shortlist reuses the is_staging=1 row)
CREATE TABLE IF NOT EXISTS collection (
    id         INTEGER PRIMARY KEY,
    uuid       TEXT    NOT NULL UNIQUE,
    name       TEXT    NOT NULL,
    parent_id  INTEGER REFERENCES collection(id),
    is_smart   INTEGER NOT NULL DEFAULT 0,
    is_staging INTEGER NOT NULL DEFAULT 0,
    query_json TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_collection_staging ON collection(is_staging) WHERE is_staging=1;
CREATE TABLE IF NOT EXISTS collection_asset (
    collection_id INTEGER NOT NULL REFERENCES collection(id) ON DELETE CASCADE,
    asset_id      INTEGER NOT NULL REFERENCES asset(id)      ON DELETE CASCADE,
    added_at      INTEGER NOT NULL,
    PRIMARY KEY (collection_id, asset_id)
);

-- Embeddings metadata
CREATE TABLE IF NOT EXISTS embedding_meta (
    asset_id   INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    model_id   TEXT    NOT NULL,           -- 'siglip2-so400m'|'clap-htsat'|...
    modality   TEXT    NOT NULL,           -- 'image'|'audio'|'text'
    dim        INTEGER NOT NULL,
    version    INTEGER NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (asset_id, model_id)
);

-- FTS5 (BM25) over name + description + denormalised tags
-- sqlite-vec (vec_asset) added in P3 when ort embeddings land.
CREATE VIRTUAL TABLE IF NOT EXISTS asset_fts USING fts5(
    name, description, tags_concat,
    content='asset', content_rowid='id',
    tokenize='unicode61 remove_diacritics 2'
);

-- FTS5 sync triggers (keep asset_fts consistent with asset rows)
CREATE TRIGGER IF NOT EXISTS asset_fts_ai AFTER INSERT ON asset BEGIN
    INSERT INTO asset_fts(rowid, name, description, tags_concat)
    VALUES (new.id, new.name, new.description, '');
END;
CREATE TRIGGER IF NOT EXISTS asset_fts_ad AFTER DELETE ON asset BEGIN
    INSERT INTO asset_fts(asset_fts, rowid, name, description, tags_concat)
    VALUES ('delete', old.id, old.name, old.description, '');
END;
CREATE TRIGGER IF NOT EXISTS asset_fts_au AFTER UPDATE ON asset BEGIN
    INSERT INTO asset_fts(asset_fts, rowid, name, description, tags_concat)
    VALUES ('delete', old.id, old.name, old.description, '');
    INSERT INTO asset_fts(rowid, name, description, tags_concat)
    VALUES (new.id, new.name, new.description, '');
END;

-- Mapping library tables (DATA-MODEL.md §4)
CREATE TABLE IF NOT EXISTS taxonomy (
    id        INTEGER PRIMARY KEY,
    uuid      TEXT    NOT NULL UNIQUE,
    name      TEXT    NOT NULL,
    version   INTEGER NOT NULL DEFAULT 1,
    is_active INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS category (
    id          INTEGER PRIMARY KEY,
    taxonomy_id INTEGER NOT NULL REFERENCES taxonomy(id) ON DELETE CASCADE,
    parent_id   INTEGER REFERENCES category(id),
    pref_label  TEXT NOT NULL,
    path        TEXT NOT NULL,
    UNIQUE(taxonomy_id, path)
);
CREATE TABLE IF NOT EXISTS category_alias (
    id          INTEGER PRIMARY KEY,
    category_id INTEGER NOT NULL REFERENCES category(id) ON DELETE CASCADE,
    alias       TEXT NOT NULL,
    norm_alias  TEXT NOT NULL,
    UNIQUE(category_id, norm_alias)
);
CREATE TABLE IF NOT EXISTS mapping_rule (
    id             INTEGER PRIMARY KEY,
    taxonomy_id    INTEGER NOT NULL REFERENCES taxonomy(id)  ON DELETE CASCADE,
    category_id    INTEGER NOT NULL REFERENCES category(id)  ON DELETE CASCADE,
    match_type     TEXT    NOT NULL,  -- 'exact_tag'|'alias'|'regex'|'glob_path'|'media_type'|'embedding_knn'
    pattern        TEXT    NOT NULL,
    priority       INTEGER NOT NULL DEFAULT 100,
    min_confidence REAL    DEFAULT 0,
    enabled        INTEGER NOT NULL DEFAULT 1
);
CREATE TABLE IF NOT EXISTS asset_category (
    asset_id    INTEGER NOT NULL REFERENCES asset(id)    ON DELETE CASCADE,
    category_id INTEGER NOT NULL REFERENCES category(id) ON DELETE CASCADE,
    taxonomy_id INTEGER NOT NULL REFERENCES taxonomy(id),
    assigned_by TEXT    NOT NULL,  -- 'rule'|'manual'|'override'
    rule_id     INTEGER REFERENCES mapping_rule(id),
    confidence  REAL,
    is_pinned   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (asset_id, category_id, taxonomy_id)
);
";

/// M2 — packages (Unity Asset Store packs). Spec: DATA-MODEL.md §7.
/// Runs exactly once (user_version gate), so ALTER TABLE is safe without a guard.
const MIGRATION_02_PACKAGES: &str = "
CREATE TABLE IF NOT EXISTS package (
    id            INTEGER PRIMARY KEY,
    uuid          TEXT    NOT NULL UNIQUE,
    source_id     INTEGER NOT NULL REFERENCES import_source(id),
    publisher     TEXT,
    category      TEXT,
    name          TEXT    NOT NULL,
    version       TEXT,
    package_path  TEXT    NOT NULL,
    content_hash  TEXT,
    asset_count   INTEGER,
    unpacked_at   INTEGER,
    license       TEXT,
    attribution   TEXT,
    license_url   TEXT,
    UNIQUE(source_id, package_path)
);
CREATE INDEX IF NOT EXISTS idx_package_source ON package(source_id);
ALTER TABLE asset ADD COLUMN package_id INTEGER REFERENCES package(id);
CREATE INDEX IF NOT EXISTS idx_asset_package ON asset(package_id);
";

/// M3 — projects, import provenance, per-asset license overrides. Spec: DATA-MODEL.md §9.
const MIGRATION_03_PROJECTS_IMPORTS: &str = "
CREATE TABLE IF NOT EXISTS project (
    id            INTEGER PRIMARY KEY,
    uuid          TEXT    NOT NULL UNIQUE,
    name          TEXT    NOT NULL,
    project_path  TEXT    NOT NULL UNIQUE,
    unity_version TEXT,
    last_seen_at  INTEGER,
    is_active     INTEGER NOT NULL DEFAULT 0
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_project_active ON project(is_active) WHERE is_active=1;

CREATE TABLE IF NOT EXISTS import_record (
    id                INTEGER PRIMARY KEY,
    asset_id          INTEGER NOT NULL REFERENCES asset(id)    ON DELETE CASCADE,
    project_id        INTEGER NOT NULL REFERENCES project(id)  ON DELETE CASCADE,
    source_package_id INTEGER REFERENCES package(id),
    dest_path         TEXT    NOT NULL,
    unity_guid        TEXT,
    with_deps         INTEGER NOT NULL DEFAULT 1,
    imported_at       INTEGER NOT NULL,
    UNIQUE(asset_id, project_id)
);
CREATE INDEX IF NOT EXISTS idx_import_project ON import_record(project_id);
CREATE INDEX IF NOT EXISTS idx_import_asset   ON import_record(asset_id);

-- Per-asset license override (falls back to package.license when NULL). DATA-MODEL.md §9.
ALTER TABLE asset ADD COLUMN license     TEXT;
ALTER TABLE asset ADD COLUMN attribution TEXT;
ALTER TABLE asset ADD COLUMN license_url TEXT;
";

/// M4 — user ratings + render-pipeline compatibility. Spec: DATA-MODEL.md §12.
const MIGRATION_04_RATING_PIPELINE: &str = "
ALTER TABLE asset   ADD COLUMN rating   INTEGER;  -- 1..5 stars; NULL = unrated
ALTER TABLE package ADD COLUMN pipeline TEXT;     -- 'built_in'|'urp'|'hdrp'|'mixed'; NULL = unknown
ALTER TABLE project ADD COLUMN pipeline TEXT;     -- registered editor's pipeline (register_project)
";

/// M5 — UI-SPEC alignment: per-project staging trays, import batches, trait columns
/// on `tag`, the `asset_relationship` graph (final form), and the per-source
/// blacklist. DDL + data steps verbatim from DATA-MODEL.md §13.
const MIGRATION_05_UI_SPEC_ALIGNMENT: &str = "
-- 1. Per-project staging trays (§9; UI-SPEC §16)
ALTER TABLE collection ADD COLUMN project_id INTEGER REFERENCES project(id) ON DELETE CASCADE;
DROP INDEX IF EXISTS idx_collection_staging;                       -- was: single shared tray
CREATE UNIQUE INDEX idx_collection_staging
  ON collection(project_id) WHERE is_staging=1;                    -- one tray per project

-- 2. Import batches (§9; UI-SPEC §16.1 'Recent exports' + batch undo)
ALTER TABLE import_record ADD COLUMN batch_id TEXT;
CREATE INDEX idx_import_batch ON import_record(batch_id);

-- 3. Trait Library (UI-SPEC §9)
ALTER TABLE tag ADD COLUMN is_trait   INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tag ADD COLUMN scope      TEXT;                        -- media_type; NULL = any
ALTER TABLE tag ADD COLUMN usage_note TEXT;                        -- fed to the identifier (AI-PIPELINE §6)
ALTER TABLE tag ADD COLUMN teach      INTEGER NOT NULL DEFAULT 0;
ALTER TABLE tag ADD COLUMN icon       TEXT;

-- 4. Dependency graph (§10; UI-SPEC §13.6) — created directly in its final form
CREATE TABLE asset_relationship (
  id            INTEGER PRIMARY KEY,
  from_asset_id INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
  to_asset_id   INTEGER REFERENCES asset(id) ON DELETE CASCADE,    -- NULL = unresolved reference
  kind          TEXT NOT NULL,
  source        TEXT NOT NULL DEFAULT 'unity_meta',
  confidence    REAL,
  declared_path TEXT,
  state         TEXT NOT NULL DEFAULT 'resolved',                  -- 'resolved'|'library'|'missing'
  UNIQUE(from_asset_id, to_asset_id, kind)
);
CREATE INDEX idx_rel_from ON asset_relationship(from_asset_id);
CREATE INDEX idx_rel_to   ON asset_relationship(to_asset_id);

-- 5. Per-source blacklist moves into the catalog (until M5 it lives in app config)
ALTER TABLE import_source ADD COLUMN blacklist TEXT;               -- JSON [{kind:'folder'|'asset', value}]

-- Data: the legacy shared tray is reassigned to the active project when one
-- exists, else deleted (collection_asset rows cascade).
UPDATE collection SET project_id = (SELECT id FROM project WHERE is_active=1)
 WHERE is_staging=1 AND project_id IS NULL
   AND EXISTS (SELECT 1 FROM project WHERE is_active=1);
DELETE FROM collection WHERE is_staging=1 AND project_id IS NULL;

-- Data: synthetic batch id per (project_id, imported_at) group so pre-M5 imports
-- appear as history rows and batch undo works on them.
UPDATE import_record SET batch_id = 'legacy-' || project_id || '-' || imported_at
 WHERE batch_id IS NULL;
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_applies_migrations() {
        let catalog = Catalog::open_in_memory().expect("catalog open");
        let version: u32 = catalog
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("user_version");
        assert_eq!(version, 5, "all 5 migrations should have run");
    }

    /// A catalog stopped at schema v4, for exercising M5's data steps.
    fn catalog_at_v4() -> Catalog {
        let conn = Connection::open_in_memory().expect("open");
        configure_connection(&conn).expect("pragmas");
        let mut c = Catalog { conn };
        c.apply_migrations_up_to(4).expect("migrations to v4");
        c
    }

    #[test]
    fn m5_reassigns_legacy_tray_to_active_project() {
        let mut c = catalog_at_v4();
        c.conn
            .execute_batch(
                "INSERT INTO project (id, uuid, name, project_path, is_active)
                 VALUES (1, 'p1', 'Game', 'C:/game', 1);
                 INSERT INTO collection (id, uuid, name, is_staging)
                 VALUES (10, 'c1', 'Staging', 1);",
            )
            .expect("fixture");
        c.apply_migrations().expect("M5");

        let project_id: Option<i64> = c
            .conn
            .query_row("SELECT project_id FROM collection WHERE id=10", [], |r| {
                r.get(0)
            })
            .expect("tray survives");
        assert_eq!(project_id, Some(1), "legacy tray reassigned to active project");
    }

    #[test]
    fn m5_deletes_orphan_legacy_tray_when_no_active_project() {
        let mut c = catalog_at_v4();
        c.conn
            .execute_batch(
                "INSERT INTO collection (id, uuid, name, is_staging)
                 VALUES (10, 'c1', 'Staging', 1);",
            )
            .expect("fixture");
        c.apply_migrations().expect("M5");

        let n: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM collection WHERE is_staging=1", [], |r| {
                r.get(0)
            })
            .expect("count");
        assert_eq!(n, 0, "orphan legacy tray deleted");
    }

    #[test]
    fn m5_enforces_one_tray_per_project() {
        let c = Catalog::open_in_memory().expect("open");
        c.conn
            .execute_batch(
                "INSERT INTO project (id, uuid, name, project_path) VALUES (1, 'p1', 'A', '/a');
                 INSERT INTO collection (uuid, name, is_staging, project_id) VALUES ('c1', 'S', 1, 1);",
            )
            .expect("first tray");
        let dup = c.conn.execute(
            "INSERT INTO collection (uuid, name, is_staging, project_id) VALUES ('c2', 'S2', 1, 1)",
            [],
        );
        assert!(dup.is_err(), "second staging tray for the same project rejected");
    }

    #[test]
    fn m5_backfills_batch_ids_per_project_and_time() {
        let mut c = catalog_at_v4();
        c.conn
            .execute_batch(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 's1', 'Src', 'C:/lib', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at)
                 VALUES (1, 'a1', 1, 1, 'rock', 0, 0), (2, 'a2', 1, 1, 'tree', 0, 0);
                 INSERT INTO project (id, uuid, name, project_path) VALUES (1, 'p1', 'Game', '/g');
                 INSERT INTO import_record (asset_id, project_id, dest_path, imported_at)
                 VALUES (1, 1, 'Assets/rock.png', 1000), (2, 1, 'Assets/tree.png', 1000);",
            )
            .expect("fixture");
        c.apply_migrations().expect("M5");

        let ids: Vec<String> = c
            .conn
            .prepare("SELECT batch_id FROM import_record ORDER BY asset_id")
            .expect("prep")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<std::result::Result<_, _>>()
            .expect("rows");
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0], ids[1], "same (project, imported_at) group → same batch id");
        assert_eq!(ids[0], "legacy-1-1000");
    }

    #[test]
    fn adopt_config_blacklists_fills_only_empty_matching_sources() {
        let c = Catalog::open_in_memory().expect("open");
        c.conn
            .execute_batch(
                "INSERT INTO import_source (uuid, name, root_path, kind, blacklist) VALUES
                 ('s1', 'A', 'C:/a', 'folder', NULL),
                 ('s2', 'B', 'C:/b', 'folder', '[{\"kind\":\"folder\",\"value\":\"Editor\"}]');",
            )
            .expect("fixture");

        let updated = c
            .adopt_config_blacklists(&[
                ("C:/a".into(), "[{\"kind\":\"asset\",\"value\":\"x.dll\"}]".into()),
                ("C:/b".into(), "[{\"kind\":\"folder\",\"value\":\"Gizmos\"}]".into()),
                ("C:/missing".into(), "[]".into()),
            ])
            .expect("adopt");
        assert_eq!(updated, 1, "only the empty, matching source is filled");

        let b: String = c
            .conn
            .query_row("SELECT blacklist FROM import_source WHERE root_path='C:/b'", [], |r| {
                r.get(0)
            })
            .expect("b");
        assert!(b.contains("Editor"), "existing catalog blacklist untouched");
    }

    #[test]
    fn seed_media_types_idempotent() {
        let catalog = Catalog::open_in_memory().expect("catalog open");
        catalog.seed_media_types().expect("first seed");
        catalog
            .seed_media_types()
            .expect("second seed (idempotent)");

        let count: i64 = catalog
            .conn
            .query_row("SELECT COUNT(*) FROM media_type", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 7, "all 7 media types seeded");
    }

    #[test]
    fn foreign_keys_enforced() {
        let catalog = Catalog::open_in_memory().expect("catalog open");
        let result = catalog.conn.execute(
            "INSERT INTO asset (uuid, source_id, media_type_id, name, status, created_at, updated_at)
             VALUES ('abc', 9999, 9999, 'test', 'discovered', 0, 0)",
            [],
        );
        assert!(result.is_err(), "FK violation should be rejected");
    }
}
