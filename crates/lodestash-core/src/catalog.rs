//! Catalog — SQLite-backed asset store.
//! Schema spec and invariants: docs/DATA-MODEL.md §2–10.
//!
//! Migration strategy: `PRAGMA user_version` tracks the schema version;
//! `apply_migrations` runs numbered SQL steps in order on first open.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use tracing::info;

use crate::error::{CoreError, Result};
use crate::fusion::rrf_fuse;
use crate::vector::{BruteForceIndex, VectorIndex};

/// One crawled file to persist — the crawl→catalog handoff (INGESTION.md §1).
#[derive(Debug, Clone)]
pub struct CrawlRecord {
    /// Path relative to the source root, `/`-separated for portability.
    pub rel_path: String,
    /// Display name (file stem).
    pub name: String,
    /// `media_type.code` (DATA-MODEL §2) — unclassified files are not persisted.
    pub media_type: String,
    pub size_bytes: i64,
    pub mtime: i64,
    /// For package-extracted files: path inside the extraction workspace (workspace-relative,
    /// `/`-separated). None for plain on-disk files.
    pub workspace_rel: Option<String>,
}

/// A caller's classification of one in-pack manifest entry (DATA-MODEL §7 "In-pack assets", M14),
/// keyed by the entry's GUID. Core stays classification-free, exactly like [`CrawlRecord`]: the UI
/// runs `detect::by_extension` over each pathname and passes only the entries it could classify —
/// an unclassified entry is simply absent, so it never becomes a grid asset (the same invariant the
/// crawl enforces).
#[derive(Debug, Clone)]
pub struct InPackClass {
    /// Display name (file stem).
    pub name: String,
    /// `media_type.code`.
    pub media_type: String,
}

/// How many rows a package write (locators, in-pack asset mint) commits per transaction. Chunking
/// keeps a huge pack from holding the single write lock long enough to starve a concurrent
/// scan-persist ("database is locked").
const PKG_WRITE_CHUNK: usize = 500;

/// What `record_crawl` did with a batch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CrawlStats {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
}

/// Catalog metadata for one `import_source` row — feeds the Settings Sources panel
/// (UI-SPEC §11.1) and the watcher (DATA-MODEL §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceInfo {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub kind: String,
    pub last_scanned_at: Option<i64>,
    pub watch_enabled: bool,
    pub asset_count: i64,
}

/// Per-status asset counts (soft-deleted rows excluded). `unidentified` folds
/// `discovered|indexed` per DATA-MODEL §8.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCounts {
    pub unidentified: usize,
    pub identified: usize,
    pub confirmed: usize,
}

/// One asset awaiting an embedding (the embed worker's unit of work): the display text to
/// encode plus the pre-filter columns stored alongside its vector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedJob {
    pub asset_id: i64,
    pub text: String,
    pub media_type_id: i64,
    pub source_id: i64,
}

/// A dirty asset the **vision** embedder can encode (SigLIP): a 2D image, or a 3D model that
/// gets multiview-rendered first. `media_code` tells the caller which path to take.
/// Where the job's bytes are, in the §7 precedence order — exactly one location applies:
/// `disk_path` for a true on-disk source file, `workspace_rel` for a package-extracted file (the
/// caller joins it onto the extraction workspace dir), or the in-pack locator
/// (`pack_path`+`pack_offset`+`pack_size`) for an asset whose bytes are still inside a
/// `.unitypackage` (DATA-MODEL §7 / M14) — the drain pulls that one payload into memory to identify
/// it without extracting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VisualEmbedJob {
    pub asset_id: i64,
    pub disk_path: Option<String>,
    pub workspace_rel: Option<String>,
    pub media_type_id: i64,
    pub source_id: i64,
    /// `media_type.code` — `image`/`texture`/`sprite` decode directly, `model_3d` renders first.
    pub media_code: String,
    /// In-pack locator: absolute `.unitypackage` path holding this asset's payload.
    pub pack_path: Option<String>,
    pub pack_offset: Option<i64>,
    pub pack_size: Option<i64>,
    /// The file's extension (lowercased, no dot) — the decode-dispatch hint for an in-pack
    /// payload pulled into memory, where there is no path to read it from.
    pub ext: Option<String>,
}

impl StatusCounts {
    pub fn total(&self) -> usize {
        self.unidentified + self.identified + self.confirmed
    }
}

/// One catalog row hydrated for the grid at startup (no disk access needed).
#[derive(Debug, Clone)]
pub struct HydratedAsset {
    /// `/`-separated path relative to the source root.
    pub rel_path: String,
    pub asset_id: i64,
    /// `media_type.code`.
    pub media_type: String,
    pub size_bytes: i64,
    pub mtime: i64,
    /// `asset.status` code.
    pub status: String,
    pub trashed: bool,
    /// 1..=5 stars; None = unrated (DATA-MODEL §12).
    pub rating: Option<u8>,
    pub metrics: AssetMetrics,
    /// BLAKE3 hash of the primary file; exact-dedup key (DATA-MODEL).
    pub content_hash: Option<String>,
}

/// Technical facets extracted from the file itself (DATA-MODEL §14). Every field is optional:
/// a facet may not apply to the media type, and extraction may fail on a file we still catalogue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AssetMetrics {
    /// Triangles in a 3D model (all meshes, after triangulation).
    pub tri_count: Option<i64>,
    pub width_px: Option<i64>,
    pub height_px: Option<i64>,
    pub duration_ms: Option<i64>,
}

impl AssetMetrics {
    fn from_row(row: &rusqlite::Row, first: usize) -> rusqlite::Result<Self> {
        Ok(Self {
            tri_count: row.get(first)?,
            width_px: row.get(first + 1)?,
            height_px: row.get(first + 2)?,
            duration_ms: row.get(first + 3)?,
        })
    }
}

/// One asset as the agent-facing planes present it (ENGINE-API §3 `AssetSummary`, minus the
/// fields whose data doesn't exist yet — tags arrive with P4). Shared by REST and MCP so the two
/// can't describe an asset differently.
#[derive(Debug, Clone, PartialEq)]
pub struct AssetSummaryRow {
    pub id: i64,
    pub uuid: String,
    pub name: String,
    /// `media_type.code`.
    pub media_type: String,
    /// Primary file's extension, lowercased (`fbx`, `png`, …).
    pub format: Option<String>,
    /// `asset.status` code.
    pub status: String,
    pub size_bytes: i64,
    pub package_id: Option<i64>,
    pub package_name: Option<String>,
    /// Absolute path of the primary file, or `None` for a package-extracted payload that only
    /// exists in the workspace (the caller joins `workspace_rel` onto its workspace dir).
    pub disk_path: Option<String>,
    pub workspace_rel: Option<String>,
    pub metrics: AssetMetrics,
}

/// One asset's catalog identity for the UI join (grid read path).
#[derive(Debug, Clone)]
pub struct AssetIdentity {
    /// `/`-separated primary-file path relative to the source root.
    pub rel_path: String,
    pub asset_id: i64,
    /// `asset.status` code.
    pub status: String,
    pub trashed: bool,
    pub rating: Option<u8>,
    pub metrics: AssetMetrics,
    /// BLAKE3 hash of the primary file; exact-dedup key (DATA-MODEL).
    pub content_hash: Option<String>,
}

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

    pub fn schema_version(&self) -> Result<u32> {
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
            (6, MIGRATION_06_PACKAGE_ENTRIES),
            (7, MIGRATION_07_PACKAGE_PINNED),
            (8, MIGRATION_08_EMBEDDINGS),
            (9, MIGRATION_09_ASSET_METRICS),
            (10, MIGRATION_10_DEPENDENCY_SCAN),
            (11, MIGRATION_11_TAGGING),
            (12, MIGRATION_12_FAVORITES),
            (13, MIGRATION_13_ONE_PACKAGE_PER_PATH),
            (14, MIGRATION_14_IN_PACK_ASSETS),
            (15, MIGRATION_15_VECTOR_PREVIEWS),
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

    /// Find-or-create an `import_source` by `root_path`; returns its id. The name is
    /// only set on insert (renames are a user action, not a scan side effect).
    pub fn upsert_import_source(&self, name: &str, root_path: &str, kind: &str) -> Result<i64> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM import_source WHERE root_path = ?1",
                params![root_path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("source lookup: {e}")))?;
        if let Some(id) = existing {
            return Ok(id);
        }
        self.conn
            .execute(
                "INSERT INTO import_source (uuid, name, root_path, kind) VALUES (?1, ?2, ?3, ?4)",
                params![uuid::Uuid::new_v4().to_string(), name, root_path, kind],
            )
            .map_err(|e| CoreError::Catalog(format!("source insert: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Metadata for every `import_source` row, with a live asset count.
    pub fn source_infos(&self) -> Result<Vec<SourceInfo>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT s.id, s.name, s.root_path, s.kind, s.last_scanned_at, s.watch_enabled,
                        COUNT(a.id) AS asset_count
                 FROM import_source s
                 LEFT JOIN asset a ON a.source_id = s.id AND a.deleted_at IS NULL
                 GROUP BY s.id, s.name, s.root_path, s.kind, s.last_scanned_at, s.watch_enabled
                 ORDER BY s.name",
            )
            .map_err(|e| CoreError::Catalog(format!("source infos prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(SourceInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    root_path: r.get(2)?,
                    kind: r.get(3)?,
                    last_scanned_at: r.get(4)?,
                    watch_enabled: r.get::<_, i64>(5)? != 0,
                    asset_count: r.get::<_, i64>(6)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("source infos query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("source infos row: {e}")))
    }

    /// Metadata for one source by its absolute root path, if it exists.
    pub fn source_info_by_root(&self, root_path: &str) -> Result<Option<SourceInfo>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT s.id, s.name, s.root_path, s.kind, s.last_scanned_at, s.watch_enabled,
                        COUNT(a.id) AS asset_count
                 FROM import_source s
                 LEFT JOIN asset a ON a.source_id = s.id AND a.deleted_at IS NULL
                 WHERE s.root_path = ?1
                 GROUP BY s.id, s.name, s.root_path, s.kind, s.last_scanned_at, s.watch_enabled",
            )
            .map_err(|e| CoreError::Catalog(format!("source info prep: {e}")))?;
        let row = stmt
            .query_row(params![root_path], |r| {
                Ok(SourceInfo {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    root_path: r.get(2)?,
                    kind: r.get(3)?,
                    last_scanned_at: r.get(4)?,
                    watch_enabled: r.get::<_, i64>(5)? != 0,
                    asset_count: r.get::<_, i64>(6)?,
                })
            })
            .optional()
            .map_err(|e| CoreError::Catalog(format!("source info row: {e}")))?;
        Ok(row)
    }

    /// Enable or disable filesystem watching for one source.
    pub fn set_source_watch(&self, source_id: i64, enabled: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE import_source SET watch_enabled = ?2 WHERE id = ?1",
                params![source_id, enabled as i64],
            )
            .map_err(|e| CoreError::Catalog(format!("set source watch: {e}")))?;
        Ok(())
    }

    /// Find-or-create a `package` row by `package_path` — the archive on disk *is* the pack
    /// (M13), so the same file found again through a different source is the same row. The
    /// browse-only population of DATA-MODEL §7 (`asset_count`/`unpacked_at` stay NULL until
    /// ingest); a later sighting fills in a publisher/category the first one didn't know.
    pub fn upsert_package(
        &self,
        source_id: i64,
        package_path: &str,
        name: &str,
        publisher: Option<&str>,
        category: Option<&str>,
    ) -> Result<i64> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM package WHERE package_path = ?1",
                params![package_path],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("package lookup: {e}")))?;
        if let Some(id) = existing {
            self.conn
                .execute(
                    "UPDATE package SET publisher = COALESCE(publisher, ?2),
                                        category  = COALESCE(category, ?3)
                      WHERE id = ?1",
                    params![id, publisher, category],
                )
                .map_err(|e| CoreError::Catalog(format!("package fill: {e}")))?;
            return Ok(id);
        }
        self.conn
            .execute(
                "INSERT INTO package (uuid, source_id, package_path, name, publisher, category)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    source_id,
                    package_path,
                    name,
                    publisher,
                    category
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("package insert: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// The root path of the `import_source` a package belongs to — what the UI must track as a
    /// (pack-only) scan root for the package's in-pack assets to hydrate into the grid (M14).
    pub fn package_source_root(&self, package_id: i64) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT s.root_path FROM package p
                   JOIN import_source s ON s.id = p.source_id
                  WHERE p.id = ?1",
                params![package_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("package source root: {e}")))
    }

    /// Stamp a package as unpacked with its ingested asset count (DATA-MODEL §7).
    pub fn mark_package_unpacked(&self, package_id: i64, asset_count: usize) -> Result<()> {
        self.conn
            .execute(
                "UPDATE package SET asset_count = ?2, unpacked_at = ?3 WHERE id = ?1",
                params![package_id, asset_count as i64, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("package unpacked: {e}")))?;
        Ok(())
    }

    /// Persist one source's crawl into `asset` + `asset_file`, incrementally:
    /// new rel_path ⇒ insert (status `discovered`); same `(size, mtime)` ⇒ untouched;
    /// changed ⇒ update the file row and mark the asset dirty (DATA-MODEL §6).
    /// New assets get `package_id` when the batch came out of a package (DATA-MODEL §7).
    /// One transaction per call; also stamps `import_source.last_scanned_at`.
    pub fn record_crawl(
        &mut self,
        source_id: i64,
        package_id: Option<i64>,
        records: &[CrawlRecord],
    ) -> Result<CrawlStats> {
        let now = unix_now();
        let mut stats = CrawlStats::default();

        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("record_crawl begin: {e}")))?;
        {
            // media_type code → id, resolved once per batch.
            let mut type_ids: HashMap<String, i64> = HashMap::new();
            {
                let mut stmt = tx
                    .prepare("SELECT code, id FROM media_type")
                    .map_err(|e| CoreError::Catalog(format!("media_type prep: {e}")))?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                    .map_err(|e| CoreError::Catalog(format!("media_type query: {e}")))?;
                for row in rows {
                    let (code, id) =
                        row.map_err(|e| CoreError::Catalog(format!("media_type row: {e}")))?;
                    type_ids.insert(code, id);
                }
            }

            let mut lookup = tx
                .prepare(
                    "SELECT af.id, af.size_bytes, af.mtime, af.asset_id
                     FROM asset_file af JOIN asset a ON a.id = af.asset_id
                     WHERE a.source_id = ?1 AND af.rel_path = ?2",
                )
                .map_err(|e| CoreError::Catalog(format!("lookup prep: {e}")))?;

            for rec in records {
                let Some(&media_type_id) = type_ids.get(&rec.media_type) else {
                    return Err(CoreError::Catalog(format!(
                        "unknown media_type code '{}' (seed_media_types not run?)",
                        rec.media_type
                    )));
                };
                let existing: Option<(i64, Option<i64>, Option<i64>, i64)> = lookup
                    .query_row(params![source_id, rec.rel_path], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                    })
                    .optional()
                    .map_err(|e| CoreError::Catalog(format!("lookup: {e}")))?;

                match existing {
                    None => {
                        tx.execute(
                            "INSERT INTO asset (uuid, source_id, media_type_id, name,
                                                package_id, status, created_at, updated_at)
                             VALUES (?1, ?2, ?3, ?4, ?5, 'discovered', ?6, ?6)",
                            params![
                                uuid::Uuid::new_v4().to_string(),
                                source_id,
                                media_type_id,
                                rec.name,
                                package_id,
                                now
                            ],
                        )
                        .map_err(|e| CoreError::Catalog(format!("asset insert: {e}")))?;
                        let asset_id = tx.last_insert_rowid();
                        let ext = rec.rel_path.rsplit('.').next().map(str::to_lowercase);
                        tx.execute(
                            "INSERT INTO asset_file (asset_id, rel_path, ext, size_bytes, mtime,
                                                     workspace_rel)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                            params![
                                asset_id,
                                rec.rel_path,
                                ext,
                                rec.size_bytes,
                                rec.mtime,
                                rec.workspace_rel
                            ],
                        )
                        .map_err(|e| CoreError::Catalog(format!("asset_file insert: {e}")))?;
                        tx.execute(
                            "UPDATE asset SET primary_file_id = ?2 WHERE id = ?1",
                            params![asset_id, tx.last_insert_rowid()],
                        )
                        .map_err(|e| CoreError::Catalog(format!("primary_file: {e}")))?;
                        stats.inserted += 1;
                    }
                    Some((_, Some(size), Some(mtime), _))
                        if size == rec.size_bytes && mtime == rec.mtime =>
                    {
                        stats.unchanged += 1;
                    }
                    Some((file_id, _, _, asset_id)) => {
                        tx.execute(
                            "UPDATE asset_file SET size_bytes = ?2, mtime = ?3,
                                                   content_hash = NULL
                             WHERE id = ?1",
                            params![file_id, rec.size_bytes, rec.mtime],
                        )
                        .map_err(|e| CoreError::Catalog(format!("asset_file update: {e}")))?;
                        tx.execute(
                            // The bytes changed, so the extracted facets and the outbound
                            // dependency refs are stale too.
                            "UPDATE asset SET updated_at = ?2, embedding_dirty = 1,
                                              metrics_dirty = 1, deps_dirty = 1,
                                              tags_dirty = 1
                             WHERE id = ?1",
                            params![asset_id, now],
                        )
                        .map_err(|e| CoreError::Catalog(format!("asset touch: {e}")))?;
                        stats.updated += 1;
                    }
                }
            }
        }
        tx.execute(
            "UPDATE import_source SET last_scanned_at = ?2 WHERE id = ?1",
            params![source_id, now],
        )
        .map_err(|e| CoreError::Catalog(format!("last_scanned_at: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("record_crawl commit: {e}")))?;
        Ok(stats)
    }

    /// Mint an `asset` + `asset_file` for each classified in-pack entry (DATA-MODEL §7
    /// "In-pack assets", M14). Reads the package's persisted `package_entry` locators and, for every
    /// entry whose GUID the caller classified, inserts an asset whose bytes stay **inside** the
    /// `.unitypackage`: `workspace_rel` is NULL and `package_entry_id` links the locator holding its
    /// `tar_offset`/`size_bytes`, so the embedding/tag/deps drains identify it in place by a surgical
    /// single-payload pull. Unclassified GUIDs (absent from `classified`) are skipped — that is the
    /// "unclassified files never reach the grid" invariant.
    ///
    /// Idempotent by `package_entry_id` (re-importing mints nothing twice). Freshly inserted rows
    /// carry the default dirty flags (`embedding_dirty`/`metrics_dirty`/`tags_dirty`/`deps_dirty` all
    /// DEFAULT 1), so identify picks them up with no extra flagging. The owning `source_id` is read
    /// from the `package` row — the pack already knows which library it belongs to.
    pub fn mint_in_pack_assets(
        &mut self,
        package_id: i64,
        classified: &HashMap<String, InPackClass>,
    ) -> Result<CrawlStats> {
        let now = unix_now();
        let mut stats = CrawlStats::default();

        let source_id: i64 = self
            .conn
            .query_row(
                "SELECT source_id FROM package WHERE id = ?1",
                params![package_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("mint source_id: {e}")))?;

        // A pack extracted *without ever being analyzed* carries its assets as workspace_rel rows
        // (via record_crawl) that don't link a package_entry — the per-locator idempotency probe
        // below can't see them and would mint in-pack duplicates. Skip minting once such a row
        // exists. (When a pack is analyzed *then* extracted, `materialize_in_pack_assets` fills
        // workspace_rel on the minted rows in place — no parallel set, so this guard never trips on
        // that ordering.)
        let extracted: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM asset a JOIN asset_file af ON af.asset_id = a.id
                    WHERE a.package_id = ?1 AND af.workspace_rel IS NOT NULL)",
                params![package_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("mint extracted probe: {e}")))?;
        if extracted {
            return Ok(stats);
        }

        // The persisted locators to mint from (id + guid + pathname + size), read before the
        // write borrow so the transaction can hold the only mutable borrow of the connection.
        let locators: Vec<(i64, String, Option<String>, Option<i64>)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id, guid, pathname, size_bytes FROM package_entry
                     WHERE package_id = ?1 ORDER BY id",
                )
                .map_err(|e| CoreError::Catalog(format!("mint locators prep: {e}")))?;
            let rows = stmt
                .query_map(params![package_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .map_err(|e| CoreError::Catalog(format!("mint locators query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("mint locators row: {e}")))?
        };

        // media_type code → id — a read, resolved once before the write chunks below.
        let mut type_ids: HashMap<String, i64> = HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT code, id FROM media_type")
                .map_err(|e| CoreError::Catalog(format!("media_type prep: {e}")))?;
            let rows = stmt
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
                .map_err(|e| CoreError::Catalog(format!("media_type query: {e}")))?;
            for row in rows {
                let (code, id) =
                    row.map_err(|e| CoreError::Catalog(format!("media_type row: {e}")))?;
                type_ids.insert(code, id);
            }
        }

        // Mint in chunks: a huge pack (tens of thousands of entries) must not hold the single write
        // lock long enough to starve a concurrent scan-persist ("database is locked").
        for batch in locators.chunks(PKG_WRITE_CHUNK) {
            let tx = self
                .conn
                .transaction()
                .map_err(|e| CoreError::Catalog(format!("mint_in_pack begin: {e}")))?;
            {
                let mut seen = tx
                    .prepare("SELECT 1 FROM asset_file WHERE package_entry_id = ?1")
                    .map_err(|e| CoreError::Catalog(format!("mint probe prep: {e}")))?;

                for (entry_id, guid, pathname, size_bytes) in batch {
                    let Some(class) = classified.get(guid) else {
                        continue; // unclassified — never becomes a grid asset
                    };
                    let Some(&media_type_id) = type_ids.get(&class.media_type) else {
                        return Err(CoreError::Catalog(format!(
                            "unknown media_type code '{}' (seed_media_types not run?)",
                            class.media_type
                        )));
                    };
                    if seen
                        .exists(params![entry_id])
                        .map_err(|e| CoreError::Catalog(format!("mint probe: {e}")))?
                    {
                        stats.unchanged += 1;
                        continue;
                    }
                    // In-pack files have no source-root path; the in-archive pathname is rel_path.
                    let rel_path = pathname.clone().unwrap_or_else(|| guid.clone());

                    tx.execute(
                        "INSERT INTO asset (uuid, source_id, media_type_id, name,
                                            package_id, status, created_at, updated_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, 'discovered', ?6, ?6)",
                        params![
                            uuid::Uuid::new_v4().to_string(),
                            source_id,
                            media_type_id,
                            class.name,
                            package_id,
                            now
                        ],
                    )
                    .map_err(|e| CoreError::Catalog(format!("in-pack asset insert: {e}")))?;
                    let asset_id = tx.last_insert_rowid();
                    let ext = rel_path.rsplit('.').next().map(str::to_lowercase);
                    tx.execute(
                        "INSERT INTO asset_file (asset_id, rel_path, ext, size_bytes,
                                                 package_entry_id)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![asset_id, rel_path, ext, size_bytes, entry_id],
                    )
                    .map_err(|e| CoreError::Catalog(format!("in-pack asset_file insert: {e}")))?;
                    tx.execute(
                        "UPDATE asset SET primary_file_id = ?2 WHERE id = ?1",
                        params![asset_id, tx.last_insert_rowid()],
                    )
                    .map_err(|e| CoreError::Catalog(format!("in-pack primary_file: {e}")))?;
                    stats.inserted += 1;
                }
            }
            tx.commit()
                .map_err(|e| CoreError::Catalog(format!("mint_in_pack commit: {e}")))?;
        }
        Ok(stats)
    }

    /// Extraction demoted to *materialize* (DATA-MODEL §7 "In-pack assets", M14 step 8). When a pack
    /// whose entries were already minted in-pack (`mint_in_pack_assets`) is extracted to disk, this
    /// fills `workspace_rel` on those **existing** rows instead of inserting a parallel set — so a
    /// materialized file resolves *extracted* (fast on-disk path) and stops shadowing itself.
    ///
    /// Each pair is `(pathname, workspace_rel)`, where `pathname` is the manifest path recorded on the
    /// `package_entry` (the mint's key) and `workspace_rel` locates the extracted file in the content-
    /// addressed workspace. Matches by `package_entry.pathname` within the pack. Returns the set of
    /// pathnames it actually materialized, so the caller can fall back to `record_crawl` for any file
    /// with no in-pack row (a pack extracted without ever being analyzed). Idempotent.
    pub fn materialize_in_pack_assets(
        &mut self,
        package_id: i64,
        pairs: &[(String, String)],
    ) -> Result<HashSet<String>> {
        let mut matched = HashSet::new();
        for batch in pairs.chunks(PKG_WRITE_CHUNK) {
            let tx = self
                .conn
                .transaction()
                .map_err(|e| CoreError::Catalog(format!("materialize begin: {e}")))?;
            {
                let mut stmt = tx
                    .prepare(
                        "UPDATE asset_file SET workspace_rel = ?3
                         WHERE package_entry_id = (SELECT id FROM package_entry
                             WHERE package_id = ?1 AND pathname = ?2)",
                    )
                    .map_err(|e| CoreError::Catalog(format!("materialize prep: {e}")))?;
                for (pathname, workspace_rel) in batch {
                    let changed = stmt
                        .execute(params![package_id, pathname, workspace_rel])
                        .map_err(|e| CoreError::Catalog(format!("materialize update: {e}")))?;
                    if changed > 0 {
                        matched.insert(pathname.clone());
                    }
                }
            }
            tx.commit()
                .map_err(|e| CoreError::Catalog(format!("materialize commit: {e}")))?;
        }
        Ok(matched)
    }

    /// Un-materialize a package (M14 §7, "unpin/evict clears `workspace_rel`"): revert its
    /// materialized in-pack rows to in-pack (clear `workspace_rel`; the `package_entry_id` locator
    /// stays, so bytes are still resolvable from the archive) and **return the workspace files that
    /// are safe to delete** — those no *other* live asset shares (the workspace is content-addressed,
    /// so a file two packs both extracted must survive). Called when a pack leaves the Library so its
    /// extracted bytes (potentially tens of GB) are reclaimed without stranding a shared payload.
    pub fn unmaterialize_package(&mut self, package_id: i64) -> Result<Vec<String>> {
        let files: Vec<String> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT DISTINCT af.workspace_rel
                     FROM asset a JOIN asset_file af ON af.asset_id = a.id
                     WHERE a.package_id = ?1 AND af.package_entry_id IS NOT NULL
                       AND af.workspace_rel IS NOT NULL
                       AND NOT EXISTS (
                         SELECT 1 FROM asset_file af2 JOIN asset a2 ON a2.id = af2.asset_id
                         WHERE af2.workspace_rel = af.workspace_rel
                           AND af2.id != af.id AND a2.deleted_at IS NULL
                           -- The sharer must be OUTSIDE this pack. Counting a sibling row in the
                           -- pack being unmaterialized meant two deduped assets each saw the other
                           -- as a live user, so neither path was returned — and the very next
                           -- statement cleared both `workspace_rel`s, leaking the bytes forever.
                           AND (a2.package_id IS NULL OR a2.package_id != ?1))",
                )
                .map_err(|e| CoreError::Catalog(format!("unmaterialize query prep: {e}")))?;
            let rows = stmt
                .query_map(params![package_id], |r| r.get::<_, String>(0))
                .map_err(|e| CoreError::Catalog(format!("unmaterialize query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("unmaterialize row: {e}")))?
        };
        self.conn
            .execute(
                "UPDATE asset_file SET workspace_rel = NULL
                 WHERE package_entry_id IS NOT NULL
                   AND asset_id IN (SELECT id FROM asset WHERE package_id = ?1)",
                params![package_id],
            )
            .map_err(|e| CoreError::Catalog(format!("unmaterialize update: {e}")))?;
        Ok(files)
    }

    /// The read path for the grid: every asset under a source as
    /// `(rel_path of its primary file, asset id, status code, trashed)`. Trashed
    /// (soft-deleted) rows are included with the flag set so the UI can both hide
    /// them from the grid and list them on the Trash screen (DATA-MODEL §8).
    pub fn assets_for_source(&self, source_id: i64) -> Result<Vec<AssetIdentity>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT af.rel_path, a.id, a.status, a.deleted_at IS NOT NULL, a.rating,
                        a.tri_count, a.width_px, a.height_px, a.duration_ms, a.content_hash
                 FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE a.source_id = ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("assets prep: {e}")))?;
        let rows = stmt
            .query_map(params![source_id], |r| {
                Ok(AssetIdentity {
                    rel_path: r.get(0)?,
                    asset_id: r.get(1)?,
                    status: r.get(2)?,
                    trashed: r.get(3)?,
                    rating: r.get(4)?,
                    metrics: AssetMetrics::from_row(r, 5)?,
                    content_hash: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("assets query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("assets row: {e}")))
    }

    /// Everything the UI needs to show a source's grid **without touching the disk**
    /// (startup hydrate). Excludes files flagged missing (not on disk last reconcile)
    /// and package-extracted assets (their payloads live in the workspace, not under
    /// the source root — they re-enter via extraction until cross-session rehydration
    /// stores the workspace path).
    pub fn entries_for_source(&self, source_id: i64) -> Result<Vec<HydratedAsset>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT af.rel_path, a.id, mt.code, af.size_bytes, af.mtime,
                        a.status, a.deleted_at IS NOT NULL, a.rating,
                        a.tri_count, a.width_px, a.height_px, a.duration_ms, a.content_hash
                 FROM asset a
                 JOIN asset_file af ON af.id = a.primary_file_id
                 JOIN media_type mt ON mt.id = a.media_type_id
                 WHERE a.source_id = ?1 AND af.missing = 0 AND a.package_id IS NULL",
            )
            .map_err(|e| CoreError::Catalog(format!("hydrate prep: {e}")))?;
        let rows = stmt
            .query_map(params![source_id], |r| {
                Ok(HydratedAsset {
                    rel_path: r.get(0)?,
                    asset_id: r.get(1)?,
                    media_type: r.get(2)?,
                    size_bytes: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                    mtime: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                    status: r.get(5)?,
                    trashed: r.get(6)?,
                    rating: r.get(7)?,
                    metrics: AssetMetrics::from_row(r, 8)?,
                    content_hash: r.get(12)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("hydrate query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("hydrate row: {e}")))
    }

    /// Reconcile the `missing` flag for a source after a crawl: files in the catalog
    /// but absent from `present` are flagged missing (metadata preserved — the file
    /// may be on an unplugged drive); files that reappeared are unflagged. Package-
    /// extracted assets are exempt (never present in a folder crawl). Returns
    /// (newly missing, reappeared).
    pub fn reconcile_missing(
        &mut self,
        source_id: i64,
        present: &std::collections::HashSet<String>,
    ) -> Result<(usize, usize)> {
        let rows: Vec<(i64, String, bool)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT af.id, af.rel_path, af.missing
                     FROM asset_file af JOIN asset a ON a.id = af.asset_id
                     WHERE a.source_id = ?1 AND a.package_id IS NULL",
                )
                .map_err(|e| CoreError::Catalog(format!("missing prep: {e}")))?;
            let mapped = stmt
                .query_map(params![source_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .map_err(|e| CoreError::Catalog(format!("missing query: {e}")))?;
            mapped
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("missing row: {e}")))?
        };

        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("missing begin: {e}")))?;
        let (mut gone, mut back) = (0, 0);
        for (file_id, rel_path, missing) in rows {
            let on_disk = present.contains(&rel_path);
            if on_disk != missing {
                continue; // flag already correct
            }
            tx.execute(
                "UPDATE asset_file SET missing = ?2 WHERE id = ?1",
                params![file_id, !on_disk],
            )
            .map_err(|e| CoreError::Catalog(format!("missing update: {e}")))?;
            if on_disk {
                back += 1;
            } else {
                gone += 1;
            }
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("missing commit: {e}")))?;
        Ok((gone, back))
    }

    /// Set (or clear, with None) the 1–5 star rating on assets (DATA-MODEL §12).
    pub fn set_rating(&self, asset_ids: &[i64], rating: Option<u8>) -> Result<usize> {
        if asset_ids.is_empty() {
            return Ok(0);
        }
        let id_list = asset_ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        self.conn
            .execute(
                &format!("UPDATE asset SET rating = ?1 WHERE id IN ({id_list})"),
                params![rating],
            )
            .map_err(|e| CoreError::Catalog(format!("set_rating: {e}")))
    }

    /// Flag (or unflag) assets as favourites (§12). Catalog-only, like ratings and the Trash: the
    /// files on disk are never touched. Returns rows changed.
    pub fn set_favorite(&self, asset_ids: &[i64], favorite: bool) -> Result<usize> {
        if asset_ids.is_empty() {
            return Ok(0);
        }
        // asset_ids are i64s the caller already holds — safe to inline (rusqlite has no array binds).
        let id_list = asset_ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        self.conn
            .execute(
                &format!("UPDATE asset SET favorite = ?1 WHERE id IN ({id_list})"),
                params![favorite as i64],
            )
            .map_err(|e| CoreError::Catalog(format!("set_favorite: {e}")))
    }

    /// Every live favourite's id — read once per catalog sync, like the tag and edge sets, so the
    /// grid can mark and filter without a query per row.
    pub fn favorite_ids(&self) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM asset WHERE favorite = 1 AND deleted_at IS NULL")
            .map_err(|e| CoreError::Catalog(format!("favorites prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| CoreError::Catalog(format!("favorites query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("favorites row: {e}")))
    }

    /// Move assets to / restore them from the Trash (catalog-only soft delete —
    /// files on disk are never touched; DATA-MODEL §8). Returns rows changed.
    pub fn set_trashed(&self, asset_ids: &[i64], trashed: bool) -> Result<usize> {
        if asset_ids.is_empty() {
            return Ok(0);
        }
        // Ids are i64s we produced — safe to inline (rusqlite has no array binds).
        let id_list = asset_ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let sql = if trashed {
            format!(
                "UPDATE asset SET deleted_at = {} WHERE id IN ({id_list}) AND deleted_at IS NULL",
                unix_now()
            )
        } else {
            format!("UPDATE asset SET deleted_at = NULL WHERE id IN ({id_list})")
        };
        self.conn
            .execute(&sql, [])
            .map_err(|e| CoreError::Catalog(format!("set_trashed: {e}")))
    }

    /// Per-status counts across the given sources (live rows only). Feeds the health
    /// meter and the sidebar STATUS facet (UI-SPEC §0.1/§0.2).
    pub fn status_counts_for_sources(&self, source_ids: &[i64]) -> Result<StatusCounts> {
        let mut counts = StatusCounts::default();
        if source_ids.is_empty() {
            return Ok(counts);
        }
        // source_ids are i64s we produced — safe to inline (rusqlite has no array binds).
        let id_list = source_ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT status, COUNT(*) FROM asset
             WHERE source_id IN ({id_list}) AND deleted_at IS NULL
             GROUP BY status"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| CoreError::Catalog(format!("counts prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| CoreError::Catalog(format!("counts query: {e}")))?;
        for row in rows {
            let (status, n) = row.map_err(|e| CoreError::Catalog(format!("counts row: {e}")))?;
            let n = n as usize;
            match status.as_str() {
                "discovered" | "indexed" => counts.unidentified += n,
                "identified" => counts.identified += n,
                "confirmed" => counts.confirmed += n,
                other => {
                    return Err(CoreError::Catalog(format!(
                        "unknown asset status '{other}'"
                    )))
                }
            }
        }
        Ok(counts)
    }

    /// Reconcile every live asset's `status` with the ingest work actually done
    /// (DATA-MODEL §8): facets extracted ⇒ `indexed`; an embedding **and** at least one tag ⇒
    /// `identified`; a file whose bytes changed (so `embedding_dirty` is back on), or one that
    /// carries no tags, drops back to `indexed`. `confirmed` is human-set — never touched here.
    /// Idempotent; run it on the writer connection after the drains. Returns the number of
    /// status *writes* — a row promoted two steps in one pass counts twice.
    ///
    /// Requiring both halves is the spec's definition and the honest one: an embedding makes an
    /// asset *findable*, but nothing has been said about it until it is tagged. A meter counting
    /// embeddings alone reported a library as identified that no one could describe — which is
    /// exactly how this was noticed.
    ///
    /// ponytail: an undecodable file stores a *zero* vector to guarantee drain progress,
    /// and that still reads as `identified` here. Filter those out when a zero vector
    /// starts costing something (a wrong search hit, a wrong count).
    pub fn promote_statuses(&self) -> Result<usize> {
        let now = unix_now();
        let mut changed = 0usize;
        for sql in [
            // Demote first: a stale embedding means the asset is no longer identified.
            "UPDATE asset SET status = 'indexed', updated_at = ?1
             WHERE deleted_at IS NULL AND status = 'identified' AND embedding_dirty = 1",
            "UPDATE asset SET status = 'indexed', updated_at = ?1
             WHERE deleted_at IS NULL AND status = 'discovered' AND metrics_dirty = 0",
            // An untagged asset is not identified, however well it embeds: nothing has been said
            // about it, so the Identify backlog must still contain it.
            "UPDATE asset SET status = 'indexed', updated_at = ?1
             WHERE deleted_at IS NULL AND status = 'identified'
               AND NOT EXISTS (SELECT 1 FROM asset_tag t WHERE t.asset_id = asset.id)",
            "UPDATE asset SET status = 'identified', updated_at = ?1
             WHERE deleted_at IS NULL AND status IN ('discovered', 'indexed')
               AND embedding_dirty = 0
               AND EXISTS (SELECT 1 FROM asset_embedding e WHERE e.asset_id = asset.id)
               AND EXISTS (SELECT 1 FROM asset_tag t WHERE t.asset_id = asset.id)",
        ] {
            changed += self
                .conn
                .execute(sql, params![now])
                .map_err(|e| CoreError::Catalog(format!("promote_statuses: {e}")))?;
        }
        Ok(changed)
    }
}

/// One facet value on an asset (IDENTIFICATION-AND-SEARCH §2): `namespace` is the facet
/// (`subject|style|material|color|mood|usage|audio|technical|free`), `value` the term, and
/// `confidence` the model's self-estimate — `None` for user-set and file-derived tags, which are
/// exact rather than guessed.
#[derive(Debug, Clone, PartialEq)]
pub struct TagAssignment {
    pub namespace: String,
    pub value: String,
    pub confidence: Option<f64>,
}

/// A teaching-vocabulary row (AI-PIPELINE §6): `(namespace, value, scope, usage_note)`.
pub type TeachRow = (String, String, Option<String>, Option<String>);

/// One row of the tagging **vocabulary** for the Traits screen (UI-SPEC §9, DATA-MODEL §20): a
/// `tag` with its usage count and its Trait-Library promotion fields. `manual_count`/`ai_count`
/// let the UI decide promote-eligibility (a manual tag, or an AI tag used ≥2×, that isn't a trait).
#[derive(Debug, Clone)]
pub struct VocabTrait {
    pub id: i64,
    pub namespace: String,
    pub value: String,
    pub usage_count: i64,
    pub manual_count: i64,
    pub ai_count: i64,
    pub is_trait: bool,
    pub scope: Option<String>,
    pub usage_note: Option<String>,
    pub teach: bool,
    pub color: Option<String>,
    pub icon: Option<String>,
}

impl TagAssignment {
    /// A tag with no confidence — manual or file-derived.
    pub fn exact(namespace: &str, value: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            value: value.to_string(),
            confidence: None,
        }
    }
}

/// Normalize a facet value into `(norm_name, display_name)`, or `None` when nothing survives.
///
/// `tag.norm_name` is UNIQUE **across the whole table**, so the namespace has to be part of it:
/// `metal` is a legitimate `material` *and* a legitimate `mood`, and without the prefix the second
/// one would silently attach to the first one's row. The display name stays unprefixed.
///
/// ponytail: lowercase, trim, collapse inner whitespace, and fold `_`/`.` to spaces. The spec also
/// asks for singularization and de-Booru'ing (AI-PIPELINE §6) — those belong with the mapping
/// library's alias table, where a user can see and correct them, not hardcoded here.
pub fn normalize_tag(namespace: &str, value: &str) -> Option<(String, String)> {
    let display: String = value
        .trim()
        .replace(['_', '.'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if display.is_empty() {
        return None;
    }
    let ns = namespace.trim().to_lowercase();
    let ns = if ns.is_empty() { "free" } else { ns.as_str() };
    Some((format!("{ns}:{}", display.to_lowercase()), display))
}

/// The mapping-rule `match_type` values (DATA-MODEL §4). `regex` and `embedding_knn` are defined in
/// the schema but not yet evaluated by [`Catalog::apply_mapping`] — it skips them with a warning.
pub const MAPPING_MATCH_TYPES: &[&str] = &[
    "exact_tag",
    "alias",
    "media_type",
    "glob_path",
    "regex",
    "embedding_knn",
];

/// One row of [`Catalog::list_rules`]: `(id, category_id, category_path, match_type, pattern,
/// priority, enabled)`.
pub type MappingRuleRow = (i64, i64, String, String, String, i64, bool);

/// The space-joined tag text FTS indexes for an asset — the same expression the triggers use.
fn tags_concat_in(conn: &Connection, asset_id: i64) -> Result<String> {
    conn.query_row(
        "SELECT COALESCE(group_concat(t.name, ' '), '')
           FROM asset_tag at JOIN tag t ON t.id = at.tag_id
          WHERE at.asset_id = ?1",
        params![asset_id],
        |r| r.get(0),
    )
    .map_err(|e| CoreError::Catalog(format!("tags_concat: {e}")))
}

/// Re-index one asset in `asset_fts`, removing the entry written under `old_concat` first.
/// External-content FTS5 stores no copy of the old row, so a `delete` carrying the wrong text
/// leaves its tokens behind — which is exactly how a renamed or re-tagged asset would start
/// matching terms it no longer has.
fn refresh_fts(conn: &Connection, asset_id: i64, old_concat: &str) -> Result<()> {
    let (name, description): (String, Option<String>) = conn
        .query_row(
            "SELECT name, description FROM asset WHERE id = ?1",
            params![asset_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(|e| CoreError::Catalog(format!("fts row: {e}")))?;
    conn.execute(
        "INSERT INTO asset_fts(asset_fts, rowid, name, description, tags_concat)
         VALUES ('delete', ?1, ?2, ?3, ?4)",
        params![asset_id, name, description, old_concat],
    )
    .map_err(|e| CoreError::Catalog(format!("fts delete: {e}")))?;
    let new_concat = tags_concat_in(conn, asset_id)?;
    conn.execute(
        "INSERT INTO asset_fts(rowid, name, description, tags_concat)
         VALUES (?1, ?2, ?3, ?4)",
        params![asset_id, name, description, new_concat],
    )
    .map_err(|e| CoreError::Catalog(format!("fts insert: {e}")))?;
    Ok(())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn configure_connection(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA cache_size=-32768;  -- 32 MiB page cache
         PRAGMA temp_store=MEMORY;
         PRAGMA busy_timeout=15000;  -- writers queue instead of erroring; chunked pack writes keep the wait short",
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

/// M6 — extracted-pack persistence + surgical extraction (DATA-MODEL §13).
/// `asset_file.workspace_rel` locates a package-extracted file inside the content-addressed
/// workspace (workspace-relative, so the workspace can move). `package_entry` records every
/// payload's offset/size in the decompressed tar so selected assets can be re-extracted with
/// one skip-scan instead of a full unpack (INGESTION.md §2).
const MIGRATION_06_PACKAGE_ENTRIES: &str = "
ALTER TABLE asset_file ADD COLUMN workspace_rel TEXT;
CREATE TABLE IF NOT EXISTS package_entry (
    id          INTEGER PRIMARY KEY,
    package_id  INTEGER NOT NULL REFERENCES package(id) ON DELETE CASCADE,
    guid        TEXT    NOT NULL,
    pathname    TEXT,
    tar_offset  INTEGER,               -- payload offset in the decompressed tar
    size_bytes  INTEGER,
    UNIQUE(package_id, guid)
);
CREATE INDEX IF NOT EXISTS idx_pkgentry_pkg ON package_entry(package_id);
";

/// M7 — Unity Assets library pinning (DATA-MODEL §7, FEATURES.md §5.5): a pinned
/// package is promoted to a durable sidebar LIBRARIES row; its extracted entries
/// rehydrate regardless of the global "cache extracted packs" toggle.
const MIGRATION_07_PACKAGE_PINNED: &str = "
ALTER TABLE package ADD COLUMN pinned INTEGER NOT NULL DEFAULT 0;
";

/// M8 — semantic embeddings (DATA-MODEL §2 vec_asset, §3 hybrid search). Interim store:
/// a plain BLOB table (dim × f32 little-endian) with brute-force cosine KNN in Rust
/// ([`crate::vector`]) behind the `VectorIndex` seam. The sqlite-vec `vec0` virtual table
/// is the same-file optimisation adopted behind that seam once brute force stops being
/// comfortable (~1M vectors). `media_type_id`/`source_id` are the pre-filter columns.
const MIGRATION_08_EMBEDDINGS: &str = "
CREATE TABLE IF NOT EXISTS asset_embedding (
    asset_id      INTEGER NOT NULL REFERENCES asset(id) ON DELETE CASCADE,
    model_id      TEXT    NOT NULL,
    modality      TEXT    NOT NULL,   -- image|audio|text (which encoder produced it)
    dim           INTEGER NOT NULL,
    media_type_id INTEGER,            -- pre-filter / shard column
    source_id     INTEGER,            -- pre-filter column
    embedding     BLOB    NOT NULL,   -- dim * f32, little-endian
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (asset_id, model_id)
);
CREATE INDEX IF NOT EXISTS idx_embed_prefilter ON asset_embedding(media_type_id);
";

/// M9 — technical facets (DATA-MODEL §2 `asset`, §14): tri counts, pixel dimensions and audio
/// duration, extracted lazily by a dirty-flag drain like embeddings. Nullable because a facet only
/// applies to some media types, and because extraction can fail on a file we still want catalogued.
const MIGRATION_09_ASSET_METRICS: &str = "
ALTER TABLE asset ADD COLUMN tri_count     INTEGER;
ALTER TABLE asset ADD COLUMN width_px      INTEGER;
ALTER TABLE asset ADD COLUMN height_px     INTEGER;
ALTER TABLE asset ADD COLUMN duration_ms   INTEGER;
ALTER TABLE asset ADD COLUMN metrics_dirty INTEGER NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS idx_asset_metrics_dirty ON asset(metrics_dirty) WHERE metrics_dirty=1;
";

/// One row of the inspector's LINKED FILES panel (UI-SPEC §13.6): an edge seen from the selected
/// asset. `asset_id`/`name`/`media_code` are `None` for an outbound edge whose target isn't in the
/// library — the `missing` case the panel calls out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelatedAsset {
    /// True: the selected asset *uses* this one. False: this one uses the selected asset.
    pub outbound: bool,
    pub kind: String,
    pub state: String,
    /// What the parent declared — a GUID for Unity-harvested edges.
    pub declared_path: Option<String>,
    pub asset_id: Option<i64>,
    pub name: Option<String>,
    pub media_code: Option<String>,
}

/// M10 — dependency-graph queue (§10). `deps_dirty` is the third drain flag, next to
/// `embedding_dirty` and `metrics_dirty`: set on insert and whenever a file's bytes change,
/// cleared once the asset's `.meta` GUID and outbound references have been harvested.
/// M12 — favourites (§12). A pin, not a rating: "I want to find this again" is a different
/// question from "how good is it", and collapsing the two would make a 5-star asset a favourite by
/// accident. Partial index because the interesting set is always the small one.
/// M13 — one `package` row per archive (DATA-MODEL §7). A pack is identified by the file on
/// disk, not by whichever source happened to find it: the same `.unitypackage` reachable from
/// both a folder source and the Asset Store cache source used to get two rows, so the Unity
/// Assets screen listed it twice and the pin, the asset count and the recorded locators landed
/// on different rows.
///
/// The merge keeps the row that carries data — extracted (`asset_count`), then pinned, then the
/// lowest id — and repoints the losers' assets and locators onto it. `package_entry` is
/// `INSERT OR IGNORE`d because both rows may have recorded the same GUID; a duplicate locator is
/// the same fact twice, not a conflict.
const MIGRATION_13_ONE_PACKAGE_PER_PATH: &str = "
CREATE TEMP TABLE pkg_survivor AS
SELECT package_path,
       (SELECT id FROM package p2 WHERE p2.package_path = p.package_path
         ORDER BY (p2.asset_count IS NOT NULL) DESC, p2.pinned DESC, p2.id ASC
         LIMIT 1) AS keep_id
  FROM package p GROUP BY package_path;

UPDATE asset SET package_id = (
    SELECT s.keep_id FROM pkg_survivor s JOIN package p ON p.package_path = s.package_path
     WHERE p.id = asset.package_id)
 WHERE package_id IS NOT NULL;

INSERT OR IGNORE INTO package_entry (package_id, guid, pathname, tar_offset, size_bytes)
SELECT s.keep_id, e.guid, e.pathname, e.tar_offset, e.size_bytes
  FROM package_entry e
  JOIN package p ON p.id = e.package_id
  JOIN pkg_survivor s ON s.package_path = p.package_path
 WHERE e.package_id <> s.keep_id;

DELETE FROM package WHERE id NOT IN (SELECT keep_id FROM pkg_survivor);
DROP TABLE pkg_survivor;

CREATE UNIQUE INDEX IF NOT EXISTS idx_package_path ON package(package_path);
";

/// M15 — vector previews (DESIGN-GUIDELINES §7). One traced-contour SVG per 3D asset, holding every
/// rendered viewpoint as its own `<g>`, replacing the per-size PNG set: the same artifact serves a
/// 32 px list row and a 780 px Quick Look, and thousands of small cache files become rows.
///
/// Its **own table**, so `SELECT` on `asset` never drags a blob. `source_key` is the mesh cache key
/// — `(path, size, mtime)` plus the proxy budget — so an edited file regenerates instead of serving
/// a preview of geometry that no longer exists.
///
/// The budget is in the key, so raising on-screen quality also invalidates: the metrics drain
/// generates at `config.preview_quality.proxy_budget()`, and a different budget is a different
/// `source_key`, which regenerates rather than serving a coarser preview than the user asked for.
const MIGRATION_15_VECTOR_PREVIEWS: &str = "
CREATE TABLE IF NOT EXISTS asset_preview (
  asset_id     INTEGER PRIMARY KEY REFERENCES asset(id) ON DELETE CASCADE,
  svg          BLOB    NOT NULL,
  view_count   INTEGER NOT NULL,
  source_key   TEXT    NOT NULL,
  generated_at TEXT    NOT NULL
);
";

const MIGRATION_12_FAVORITES: &str = "
ALTER TABLE asset ADD COLUMN favorite INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_asset_favorite ON asset(favorite) WHERE favorite=1;
";

/// M14 — in-pack assets (DATA-MODEL §7 \"In-pack assets\"). Links an `asset_file` to the
/// `package_entry` locator holding its `tar_offset`/`size_bytes`, so a file's bytes resolve
/// *extracted* (`workspace_rel`) → *in-pack* (`package_entry_id`) → *on-disk* (`rel_path`).
/// Importing a pack mints an in-pack asset per manifest entry; the embedding/tag/deps drains
/// identify it in place by surgically pulling one payload at a time, so a pack is a searchable
/// library without ever being unpacked. `materialized` is derived from the ladder, never stored.
const MIGRATION_14_IN_PACK_ASSETS: &str = "
ALTER TABLE asset_file ADD COLUMN package_entry_id INTEGER REFERENCES package_entry(id);
CREATE INDEX IF NOT EXISTS idx_file_pkgentry
  ON asset_file(package_entry_id) WHERE package_entry_id IS NOT NULL;
";

/// M11 — tagging queue + working FTS tag column (§2 facets, IDENTIFICATION-AND-SEARCH §2).
/// `tags_dirty` is the fourth drain flag, beside `metrics_dirty`/`embedding_dirty`/`deps_dirty`.
///
/// The three FTS triggers are **replaced**, not added to: they wrote a literal `''` into
/// `tags_concat`, so a tag could never be searchable and — worse — an asset rename issued a
/// `delete` carrying `''` instead of the row's real tag text, which would leave stale tokens in the
/// index the moment tags existed. The subquery makes each trigger self-consistent.
const MIGRATION_11_TAGGING: &str = "
ALTER TABLE asset ADD COLUMN tags_dirty INTEGER NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS idx_asset_tags_dirty ON asset(tags_dirty) WHERE tags_dirty=1;

DROP TRIGGER IF EXISTS asset_fts_ai;
DROP TRIGGER IF EXISTS asset_fts_ad;
DROP TRIGGER IF EXISTS asset_fts_au;
CREATE TRIGGER asset_fts_ai AFTER INSERT ON asset BEGIN
    INSERT INTO asset_fts(rowid, name, description, tags_concat)
    VALUES (new.id, new.name, new.description,
            (SELECT COALESCE(group_concat(t.name, ' '), '')
               FROM asset_tag at JOIN tag t ON t.id = at.tag_id
              WHERE at.asset_id = new.id));
END;
CREATE TRIGGER asset_fts_ad AFTER DELETE ON asset BEGIN
    INSERT INTO asset_fts(asset_fts, rowid, name, description, tags_concat)
    VALUES ('delete', old.id, old.name, old.description,
            (SELECT COALESCE(group_concat(t.name, ' '), '')
               FROM asset_tag at JOIN tag t ON t.id = at.tag_id
              WHERE at.asset_id = old.id));
END;
CREATE TRIGGER asset_fts_au AFTER UPDATE ON asset BEGIN
    INSERT INTO asset_fts(asset_fts, rowid, name, description, tags_concat)
    VALUES ('delete', old.id, old.name, old.description,
            (SELECT COALESCE(group_concat(t.name, ' '), '')
               FROM asset_tag at JOIN tag t ON t.id = at.tag_id
              WHERE at.asset_id = old.id));
    INSERT INTO asset_fts(rowid, name, description, tags_concat)
    VALUES (new.id, new.name, new.description,
            (SELECT COALESCE(group_concat(t.name, ' '), '')
               FROM asset_tag at JOIN tag t ON t.id = at.tag_id
              WHERE at.asset_id = new.id));
END;
";

const MIGRATION_10_DEPENDENCY_SCAN: &str = "
ALTER TABLE asset ADD COLUMN deps_dirty INTEGER NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS idx_asset_deps_dirty ON asset(deps_dirty) WHERE deps_dirty=1;
CREATE INDEX IF NOT EXISTS idx_asset_unity_guid ON asset(unity_guid) WHERE unity_guid IS NOT NULL;
";

/// A package-extracted asset rehydrated from the catalog (UI "cache extracted packs" toggle).
#[derive(Debug, Clone)]
pub struct HydratedExtracted {
    pub package_name: String,
    /// Path inside the extraction workspace (workspace-relative).
    pub workspace_rel: String,
    /// The owning package's Add-to-Library pin (M7).
    pub pinned: bool,
    pub asset: HydratedAsset,
}

/// An in-pack asset rehydrated from the catalog (DATA-MODEL §7 / M14): its bytes are still inside
/// the `.unitypackage`, located by `pack_path` + `tar_offset` + `size` for on-demand pull.
#[derive(Debug, Clone)]
pub struct HydratedInPack {
    pub package_name: String,
    /// The owning package's Add-to-Library pin (M7) — a pinned pack rehydrates into the grid.
    pub pinned: bool,
    /// Absolute path to the `.unitypackage` holding this asset's payload.
    pub pack_path: String,
    pub tar_offset: i64,
    pub size: i64,
    pub asset: HydratedAsset,
}

/// One `package` row for the Unity Assets screen (DATA-MODEL §7).
#[derive(Debug, Clone, PartialEq)]
pub struct PackageListing {
    pub id: i64,
    pub name: String,
    pub publisher: Option<String>,
    pub category: Option<String>,
    pub package_path: String,
    pub asset_count: Option<i64>,
    pub unpacked_at: Option<i64>,
    pub pinned: bool,
    /// Harvested from the pack (DATA-MODEL §7). `None` = the pack didn't say.
    pub version: Option<String>,
    pub license: Option<String>,
}

/// A registered Unity editor (ENGINE-API §5.1 / DATA-MODEL §9). The plugin upserts its row on
/// panel open and heartbeats `last_seen_at`; exactly one row is `is_active` at a time.
#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub id: i64,
    pub name: String,
    pub project_path: String,
    pub unity_version: Option<String>,
    pub pipeline: Option<String>,
    pub is_active: bool,
    pub last_seen_at: Option<i64>,
}

/// One asset to record as imported into a project (the durable outcome of the ENGINE-API §5 flow).
#[derive(Debug, Clone)]
pub struct ImportSpec {
    pub asset_id: i64,
    pub source_package_id: Option<i64>,
    /// In-project path, e.g. `Assets/Wildlife/Beaver.fbx`.
    pub dest_path: String,
    /// GUID Unity assigned; `None` until a plugin ack refines it (ENGINE-API §5.3).
    pub unity_guid: Option<String>,
    pub with_deps: bool,
}

/// What [`Catalog::record_imports`] did with one spec — the `UNIQUE(asset_id, project_id)`
/// dup-guard (DATA-MODEL §9) makes a re-import of the same asset into the same project
/// `AlreadyImported` instead of a second row.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportOutcome {
    Recorded { asset_id: i64, import_id: i64 },
    AlreadyImported { asset_id: i64 },
}

/// One payload's location inside a `.unitypackage` (DATA-MODEL §13 M6; INGESTION §2
/// surgical extraction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageEntryRecord {
    pub guid: String,
    pub pathname: Option<String>,
    pub tar_offset: Option<i64>,
    pub size_bytes: Option<i64>,
}

impl Catalog {
    /// Package-extracted assets of a source (the rows `entries_for_source` skips), with
    /// enough location info to rebuild grid entries from the workspace.
    pub fn extracted_entries_for_source(&self, source_id: i64) -> Result<Vec<HydratedExtracted>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT af.rel_path, a.id, mt.code, af.size_bytes, af.mtime,
                        a.status, a.deleted_at IS NOT NULL, a.rating,
                        p.name, af.workspace_rel, p.pinned,
                        a.tri_count, a.width_px, a.height_px, a.duration_ms, a.content_hash
                 FROM asset a
                 JOIN asset_file af ON af.id = a.primary_file_id
                 JOIN media_type mt ON mt.id = a.media_type_id
                 JOIN package p     ON p.id = a.package_id
                 WHERE a.source_id = ?1 AND af.missing = 0
                   AND a.package_id IS NOT NULL AND af.workspace_rel IS NOT NULL",
            )
            .map_err(|e| CoreError::Catalog(format!("extracted prep: {e}")))?;
        let rows = stmt
            .query_map(params![source_id], |r| {
                Ok(HydratedExtracted {
                    package_name: r.get(8)?,
                    workspace_rel: r.get(9)?,
                    pinned: r.get(10)?,
                    asset: HydratedAsset {
                        rel_path: r.get(0)?,
                        asset_id: r.get(1)?,
                        media_type: r.get(2)?,
                        size_bytes: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                        mtime: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                        status: r.get(5)?,
                        trashed: r.get(6)?,
                        rating: r.get(7)?,
                        metrics: AssetMetrics::from_row(r, 11)?,
                        content_hash: r.get(15)?,
                    },
                })
            })
            .map_err(|e| CoreError::Catalog(format!("extracted query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("extracted row: {e}")))
    }

    /// In-pack assets for a source's grid (DATA-MODEL §7 / M14): assets minted from a pack's
    /// manifest whose bytes are still **inside** the `.unitypackage` — `workspace_rel IS NULL` and
    /// a `package_entry_id` present. Carries the pack name + pin (for the sidebar library) and the
    /// locator (`pack_path`/`tar_offset`/`size`) the grid needs to pull a thumbnail or preview on
    /// demand. Complement of [`Self::extracted_entries_for_source`]; the caller gates on `pinned`.
    pub fn in_pack_entries_for_source(&self, source_id: i64) -> Result<Vec<HydratedInPack>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT af.rel_path, a.id, mt.code, af.size_bytes, af.mtime,
                        a.status, a.deleted_at IS NOT NULL, a.rating,
                        p.name, p.pinned, p.package_path, pe.tar_offset, pe.size_bytes,
                        a.tri_count, a.width_px, a.height_px, a.duration_ms, a.content_hash
                 FROM asset a
                 JOIN asset_file af   ON af.id = a.primary_file_id
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN package p       ON p.id = a.package_id
                 JOIN package_entry pe ON pe.id = af.package_entry_id
                 WHERE a.source_id = ?1 AND af.missing = 0
                   AND af.workspace_rel IS NULL AND af.package_entry_id IS NOT NULL",
            )
            .map_err(|e| CoreError::Catalog(format!("in-pack hydrate prep: {e}")))?;
        let rows = stmt
            .query_map(params![source_id], |r| {
                Ok(HydratedInPack {
                    package_name: r.get(8)?,
                    pinned: r.get(9)?,
                    pack_path: r.get(10)?,
                    tar_offset: r.get::<_, Option<i64>>(11)?.unwrap_or(0),
                    size: r.get::<_, Option<i64>>(12)?.unwrap_or(0),
                    asset: HydratedAsset {
                        rel_path: r.get(0)?,
                        asset_id: r.get(1)?,
                        media_type: r.get(2)?,
                        size_bytes: r.get::<_, Option<i64>>(3)?.unwrap_or(0),
                        mtime: r.get::<_, Option<i64>>(4)?.unwrap_or(0),
                        status: r.get(5)?,
                        trashed: r.get(6)?,
                        rating: r.get(7)?,
                        metrics: AssetMetrics::from_row(r, 13)?,
                        content_hash: r.get(17)?,
                    },
                })
            })
            .map_err(|e| CoreError::Catalog(format!("in-pack hydrate query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("in-pack hydrate row: {e}")))
    }

    /// Upsert the payload locations of a package's manifest (browse pass) so selected
    /// assets can later be pulled with one skip-scan (surgical extraction).
    pub fn record_package_entries(
        &mut self,
        package_id: i64,
        entries: &[PackageEntryRecord],
    ) -> Result<()> {
        // Commit in chunks so a huge pack (tens of thousands of entries) never holds the single
        // write lock long enough to starve a concurrent scan-persist ("database is locked").
        for batch in entries.chunks(PKG_WRITE_CHUNK) {
            let tx = self
                .conn
                .transaction()
                .map_err(|e| CoreError::Catalog(format!("pkg entries begin: {e}")))?;
            for e in batch {
                tx.execute(
                    "INSERT INTO package_entry (package_id, guid, pathname, tar_offset, size_bytes)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(package_id, guid) DO UPDATE SET
                        pathname = excluded.pathname,
                        tar_offset = excluded.tar_offset,
                        size_bytes = excluded.size_bytes",
                    params![package_id, e.guid, e.pathname, e.tar_offset, e.size_bytes],
                )
                .map_err(|e2| CoreError::Catalog(format!("pkg entry upsert: {e2}")))?;
            }
            tx.commit()
                .map_err(|e| CoreError::Catalog(format!("pkg entries commit: {e}")))?;
        }
        Ok(())
    }

    /// The manifest locators recorded for a package, in-archive path order — what the
    /// Unity Assets browse view lists so a pack indexed in an earlier session can still be
    /// surgically re-extracted without re-reading its manifest (FEATURES.md §5.4).
    pub fn list_package_entries(&self, package_id: i64) -> Result<Vec<PackageEntryRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT guid, pathname, tar_offset, size_bytes
                 FROM package_entry WHERE package_id = ?1 ORDER BY pathname, guid",
            )
            .map_err(|e| CoreError::Catalog(format!("pkg entry prep: {e}")))?;
        let rows = stmt
            .query_map(params![package_id], |r| {
                Ok(PackageEntryRecord {
                    guid: r.get(0)?,
                    pathname: r.get(1)?,
                    tar_offset: r.get(2)?,
                    size_bytes: r.get(3)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("pkg entry query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("pkg entry row: {e}")))
    }

    /// Pin/unpin a package as a sidebar library (M7; FEATURES.md §5.5 Add to Library).
    pub fn set_package_pinned(&self, package_id: i64, pinned: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE package SET pinned = ?2 WHERE id = ?1",
                params![package_id, pinned as i64],
            )
            .map_err(|e| CoreError::Catalog(format!("package pin: {e}")))?;
        Ok(())
    }

    /// Every `package` row, publisher-then-name ordered — the Unity Assets screen's
    /// listing (DATA-MODEL §7).
    pub fn list_packages(&self) -> Result<Vec<PackageListing>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, publisher, category, package_path,
                        asset_count, unpacked_at, pinned, version, license
                 FROM package ORDER BY publisher, name",
            )
            .map_err(|e| CoreError::Catalog(format!("pkg list prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(PackageListing {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    publisher: r.get(2)?,
                    category: r.get(3)?,
                    package_path: r.get(4)?,
                    asset_count: r.get(5)?,
                    unpacked_at: r.get(6)?,
                    pinned: r.get(7)?,
                    version: r.get(8)?,
                    license: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("pkg list query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("pkg list row: {e}")))
    }

    /// Upsert an asset's embedding (M8; DATA-MODEL §3). Stores `vector` as little-endian
    /// f32 BLOB and clears `asset.embedding_dirty` in one transaction (index-sync rule).
    /// `media_type_id`/`source_id` are denormalised for KNN pre-filtering.
    /// Assets whose embedding is stale (`embedding_dirty=1`), with the display text to
    /// embed (name + description) and the pre-filter columns — the embed worker's queue.
    pub fn assets_needing_embedding(&self, limit: usize) -> Result<Vec<EmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, description, media_type_id, source_id
                 FROM asset
                 WHERE embedding_dirty = 1 AND deleted_at IS NULL
                 ORDER BY id LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("embed queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                let name: String = r.get(1)?;
                let desc: Option<String> = r.get(2)?;
                let text = match desc {
                    Some(d) if !d.trim().is_empty() => format!("{name} {d}"),
                    _ => name,
                };
                Ok(EmbedJob {
                    asset_id: r.get(0)?,
                    text,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("embed queue query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("embed queue row: {e}")))
    }

    /// Dirty assets the vision tower can encode — 2D (`image`/`texture`/`sprite`), 3D
    /// (`model_3d`, multiview-rendered by the caller) and `material` (rendered as a shader ball,
    /// its albedo resolved by the caller from the dependency graph). Returns on-disk sources
    /// (`disk_path = root_path/rel_path`) *and* package-extracted files (`workspace_rel` set —
    /// the caller joins it onto the workspace dir). `missing` rows excluded.
    pub fn visual_assets_needing_embedding(&self, limit: usize) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id,
                        CASE WHEN af.workspace_rel IS NULL AND af.package_entry_id IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel, a.media_type_id, a.source_id, mt.code,
                        pk.package_path, pe.tar_offset, pe.size_bytes, af.ext
                 FROM asset a
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN import_source s ON s.id = a.source_id
                 JOIN asset_file af   ON af.asset_id = a.id
                 LEFT JOIN package_entry pe ON pe.id = af.package_entry_id
                 LEFT JOIN package pk       ON pk.id = pe.package_id
                 WHERE a.embedding_dirty = 1 AND a.deleted_at IS NULL
                   AND af.missing = 0
                   AND mt.code IN ('image', 'texture', 'sprite', 'model_3d', 'material')
                 ORDER BY a.id LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("visual embed queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(VisualEmbedJob {
                    asset_id: r.get(0)?,
                    disk_path: r.get(1)?,
                    workspace_rel: r.get(2)?,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                    media_code: r.get(5)?,
                    pack_path: r.get(6)?,
                    pack_offset: r.get(7)?,
                    pack_size: r.get(8)?,
                    ext: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("visual embed queue query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("visual embed queue row: {e}")))
    }

    /// Dirty **audio** assets for the CLAP drain. Same shape and path rules as
    /// [`Self::visual_assets_needing_embedding`]; separate because audio is encoded by a
    /// different model into a different vector space.
    /// Audio assets awaiting **tags** (`tags_dirty`), for the CLAP zero-shot tagger (AI-PIPELINE §6,
    /// §110). Its own queue rather than a filter over `assets_needing_tags`, so the audio pass loops
    /// to completion instead of spinning on a batch of non-audio rows it can't clear.
    pub fn audio_assets_needing_tags(&self, limit: usize) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id,
                        CASE WHEN af.workspace_rel IS NULL AND af.package_entry_id IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel, a.media_type_id, a.source_id, mt.code,
                        pk.package_path, pe.tar_offset, pe.size_bytes, af.ext
                 FROM asset a
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN import_source s ON s.id = a.source_id
                 JOIN asset_file af   ON af.id = a.primary_file_id
                 LEFT JOIN package_entry pe ON pe.id = af.package_entry_id
                 LEFT JOIN package pk       ON pk.id = pe.package_id
                 WHERE a.tags_dirty = 1 AND a.deleted_at IS NULL
                   AND af.missing = 0
                   AND mt.code = 'audio'
                 ORDER BY a.id LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("audio tag queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(VisualEmbedJob {
                    asset_id: r.get(0)?,
                    disk_path: r.get(1)?,
                    workspace_rel: r.get(2)?,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                    media_code: r.get(5)?,
                    pack_path: r.get(6)?,
                    pack_offset: r.get(7)?,
                    pack_size: r.get(8)?,
                    ext: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("audio tag queue query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("audio tag queue row: {e}")))
    }

    pub fn audio_assets_needing_embedding(&self, limit: usize) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id,
                        CASE WHEN af.workspace_rel IS NULL AND af.package_entry_id IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel, a.media_type_id, a.source_id, mt.code,
                        pk.package_path, pe.tar_offset, pe.size_bytes, af.ext
                 FROM asset a
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN import_source s ON s.id = a.source_id
                 JOIN asset_file af   ON af.asset_id = a.id
                 LEFT JOIN package_entry pe ON pe.id = af.package_entry_id
                 LEFT JOIN package pk       ON pk.id = pe.package_id
                 WHERE a.embedding_dirty = 1 AND a.deleted_at IS NULL
                   AND af.missing = 0
                   AND mt.code = 'audio'
                 ORDER BY a.id LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("audio embed queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(VisualEmbedJob {
                    asset_id: r.get(0)?,
                    disk_path: r.get(1)?,
                    workspace_rel: r.get(2)?,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                    media_code: r.get(5)?,
                    pack_path: r.get(6)?,
                    pack_offset: r.get(7)?,
                    pack_size: r.get(8)?,
                    ext: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("audio embed queue query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("audio embed queue row: {e}")))
    }

    /// Assets whose technical facets need (re)extracting — any media type, since the extractor
    /// dispatches per type. Same path rules and job shape as the embedding queues (on-disk
    /// `disk_path`, or `workspace_rel` for package-extracted payloads); `missing` rows excluded.
    pub fn assets_needing_metrics(&self, limit: usize) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id,
                        CASE WHEN af.workspace_rel IS NULL AND af.package_entry_id IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel, a.media_type_id, a.source_id, mt.code,
                        pk.package_path, pe.tar_offset, pe.size_bytes, af.ext
                 FROM asset a
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN import_source s ON s.id = a.source_id
                 JOIN asset_file af   ON af.id = a.primary_file_id
                 LEFT JOIN package_entry pe ON pe.id = af.package_entry_id
                 LEFT JOIN package pk       ON pk.id = pe.package_id
                 WHERE a.metrics_dirty = 1 AND a.deleted_at IS NULL
                   AND af.missing = 0
                 ORDER BY a.id LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("metrics queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(VisualEmbedJob {
                    asset_id: r.get(0)?,
                    disk_path: r.get(1)?,
                    workspace_rel: r.get(2)?,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                    media_code: r.get(5)?,
                    pack_path: r.get(6)?,
                    pack_offset: r.get(7)?,
                    pack_size: r.get(8)?,
                    ext: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("metrics queue query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("metrics queue row: {e}")))
    }

    /// Assets whose `.meta` GUID and outbound references still need harvesting (§10). Same path
    /// rules as the other queues; `missing` rows excluded. The scanner needs the on-disk path
    /// because the `.meta` lives *beside* the asset, not inside the catalog.
    pub fn assets_needing_dep_scan(&self, limit: usize) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id,
                        CASE WHEN af.workspace_rel IS NULL AND af.package_entry_id IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel, a.media_type_id, a.source_id, mt.code,
                        pk.package_path, pe.tar_offset, pe.size_bytes, af.ext
                 FROM asset a
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN import_source s ON s.id = a.source_id
                 JOIN asset_file af   ON af.id = a.primary_file_id
                 LEFT JOIN package_entry pe ON pe.id = af.package_entry_id
                 LEFT JOIN package pk       ON pk.id = pe.package_id
                 WHERE a.deps_dirty = 1 AND a.deleted_at IS NULL
                   AND af.missing = 0
                 ORDER BY a.id LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("dep queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(VisualEmbedJob {
                    asset_id: r.get(0)?,
                    disk_path: r.get(1)?,
                    workspace_rel: r.get(2)?,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                    media_code: r.get(5)?,
                    pack_path: r.get(6)?,
                    pack_offset: r.get(7)?,
                    pack_size: r.get(8)?,
                    ext: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("dep queue query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("dep queue row: {e}")))
    }

    /// Record one asset's harvested `.meta` GUID plus the GUIDs it declares, and clear
    /// `deps_dirty` (§10). Edges land with `declared_path` = the referenced GUID and resolve
    /// against `asset.unity_guid`; a reference whose target isn't catalogued **yet** is kept
    /// with `to_asset_id NULL` / `state='missing'` and picked up later by
    /// [`Self::resolve_pending_relationships`] — a package extracted after its consumer is the
    /// normal case, not an error.
    ///
    /// The flag clears even when nothing was found (no `.meta`, no refs), so an asset without
    /// Unity bookkeeping isn't rescanned on every drain.
    pub fn record_dependencies(
        &mut self,
        asset_id: i64,
        guid: Option<&str>,
        kind: &str,
        ref_guids: &[String],
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("deps begin: {e}")))?;
        if let Some(guid) = guid {
            tx.execute(
                "UPDATE asset SET unity_guid = ?2 WHERE id = ?1",
                params![asset_id, guid],
            )
            .map_err(|e| CoreError::Catalog(format!("deps guid: {e}")))?;
        }
        // Refs are re-derived from the file, so the old harvest is replaced wholesale. This is
        // also what keeps a rescan from duplicating `missing` edges: SQLite treats NULL
        // `to_asset_id`s as distinct, so UNIQUE can't dedupe them.
        tx.execute(
            "DELETE FROM asset_relationship WHERE from_asset_id = ?1 AND source = 'unity_meta'",
            params![asset_id],
        )
        .map_err(|e| CoreError::Catalog(format!("deps clear edges: {e}")))?;
        for reference in ref_guids {
            tx.execute(
                "INSERT INTO asset_relationship
                    (from_asset_id, to_asset_id, kind, source, declared_path, state)
                 VALUES (
                    ?1,
                    (SELECT id FROM asset WHERE unity_guid = ?2 AND deleted_at IS NULL),
                    ?3, 'unity_meta', ?2,
                    CASE WHEN EXISTS (SELECT 1 FROM asset
                                       WHERE unity_guid = ?2 AND deleted_at IS NULL)
                         THEN 'resolved' ELSE 'missing' END)
                 ON CONFLICT(from_asset_id, to_asset_id, kind) DO NOTHING",
                params![asset_id, reference, kind],
            )
            .map_err(|e| CoreError::Catalog(format!("deps edge: {e}")))?;
        }
        tx.execute(
            "UPDATE asset SET deps_dirty = 0 WHERE id = ?1",
            params![asset_id],
        )
        .map_err(|e| CoreError::Catalog(format!("deps clear: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("deps commit: {e}")))
    }

    /// Models (`model_3d`) that carry **no** dependency edge at all — raw exports with no `.meta`,
    /// so no GUID and no meta/prefab edge — as `(asset_id, source_id, rel_path)`. These are the only
    /// candidates for the §10 step-2 naming heuristic: a model that already reaches a material/texture
    /// through a real edge is left alone. Pairs with [`Self::inference_textures`] +
    /// [`Self::insert_inferred_texture_edges`]; the name matching itself lives in the ingest layer.
    pub fn edgeless_models(&self) -> Result<Vec<(i64, i64, String)>> {
        self.asset_locations(
            &["model_3d"],
            "AND NOT EXISTS (SELECT 1 FROM asset_relationship r WHERE r.from_asset_id = a.id)",
        )
    }

    /// Catalogued textures/images/sprites as `(asset_id, source_id, rel_path)` — the pool the naming
    /// heuristic pairs an edge-less model against (same source + folder).
    pub fn inference_textures(&self) -> Result<Vec<(i64, i64, String)>> {
        self.asset_locations(&["texture", "image", "sprite"], "")
    }

    /// Shared `(id, source_id, rel_path)` query for the naming heuristic; `codes` filters media type,
    /// `extra` appends an optional predicate. Live rows with a present primary file only.
    fn asset_locations(&self, codes: &[&str], extra: &str) -> Result<Vec<(i64, i64, String)>> {
        let placeholders = vec!["?"; codes.len()].join(",");
        let sql = format!(
            "SELECT a.id, a.source_id, af.rel_path
               FROM asset a
               JOIN asset_file af ON af.id = a.primary_file_id AND af.missing = 0
               JOIN media_type mt ON mt.id = a.media_type_id
              WHERE mt.code IN ({placeholders}) AND a.deleted_at IS NULL {extra}"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| CoreError::Catalog(format!("infer locations prep: {e}")))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(codes), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| CoreError::Catalog(format!("infer locations query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("infer locations rows: {e}")))
    }

    /// Insert `uses_texture` edges the naming heuristic resolved (DATA-MODEL §10 step 2):
    /// `(from_model_id, to_texture_id, texture_rel_path)`, each `source='inferred'`, `confidence
    /// 0.7`, `state='resolved'`. Idempotent — `ON CONFLICT(from,to,kind) DO NOTHING` — so a re-run
    /// adds nothing, and because inserting an edge makes the model non-edge-less it drops out of
    /// [`Self::edgeless_models`] next pass. Returns the count actually inserted.
    pub fn insert_inferred_texture_edges(&mut self, edges: &[(i64, i64, &str)]) -> Result<usize> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("infer edges begin: {e}")))?;
        let mut inserted = 0usize;
        for (from_id, to_id, declared) in edges {
            inserted += tx
                .execute(
                    "INSERT INTO asset_relationship
                        (from_asset_id, to_asset_id, kind, source, confidence, declared_path, state)
                     VALUES (?1, ?2, 'uses_texture', 'inferred', 0.7, ?3, 'resolved')
                     ON CONFLICT(from_asset_id, to_asset_id, kind) DO NOTHING",
                    params![from_id, to_id, declared],
                )
                .map_err(|e| CoreError::Catalog(format!("infer edge insert: {e}")))?;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("infer edges commit: {e}")))?;
        Ok(inserted)
    }

    /// Write one asset's tags for a given `source` (`'ai'|'manual'|'imported'`), replacing
    /// whatever that source wrote before, and clear `tags_dirty` (IDENTIFICATION-AND-SEARCH §2).
    ///
    /// Replacing per *source* rather than wholesale is what keeps a re-run of the tagger from
    /// wiping the user's own tags — the two live side by side on the same asset, which is also why
    /// `asset_tag`'s primary key carries `source`.
    ///
    /// Values are normalized (see [`normalize_tag`]) and confidence is clamped to `[0,1]`; the
    /// caller is responsible for facet validation (unknown namespaces, per-facet caps) since that
    /// is model-shaped policy, not storage. The flag clears even when `tags` is empty — a model
    /// that found nothing must not be re-run forever.
    pub fn set_tags(
        &mut self,
        asset_id: i64,
        source: &str,
        model_id: Option<&str>,
        tags: &[TagAssignment],
    ) -> Result<usize> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("tags begin: {e}")))?;
        // FTS5 external-content tables need the *old* row removed with its old column values,
        // so capture the pre-write tag text before touching anything.
        let old_concat = tags_concat_in(&tx, asset_id)?;
        // Clear the flag *before* the tags change: the FTS update trigger fires on any UPDATE to
        // `asset`, and while the old tags are still in place its delete+insert cancel out. Doing it
        // afterwards would have the trigger delete under the new text and leave the old tokens.
        tx.execute(
            "UPDATE asset SET tags_dirty = 0 WHERE id = ?1",
            params![asset_id],
        )
        .map_err(|e| CoreError::Catalog(format!("tags clear flag: {e}")))?;
        tx.execute(
            "DELETE FROM asset_tag WHERE asset_id = ?1 AND source = ?2",
            params![asset_id, source],
        )
        .map_err(|e| CoreError::Catalog(format!("tags clear: {e}")))?;

        let now = unix_now();
        let mut written = 0usize;
        for t in tags {
            let Some((norm, display)) = normalize_tag(&t.namespace, &t.value) else {
                continue; // an empty or punctuation-only value is not a tag
            };
            tx.execute(
                "INSERT INTO tag (name, norm_name, namespace) VALUES (?1, ?2, ?3)
                 ON CONFLICT(norm_name) DO NOTHING",
                params![display, norm, t.namespace],
            )
            .map_err(|e| CoreError::Catalog(format!("tag upsert: {e}")))?;
            let tag_id: i64 = tx
                .query_row(
                    "SELECT id FROM tag WHERE norm_name = ?1",
                    params![norm],
                    |r| r.get(0),
                )
                .map_err(|e| CoreError::Catalog(format!("tag lookup: {e}")))?;
            tx.execute(
                "INSERT INTO asset_tag (asset_id, tag_id, source, confidence, model_id, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(asset_id, tag_id, source) DO UPDATE SET
                    confidence = excluded.confidence, model_id = excluded.model_id",
                params![
                    asset_id,
                    tag_id,
                    source,
                    t.confidence.map(|c| c.clamp(0.0, 1.0)),
                    model_id,
                    now
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("asset_tag insert: {e}")))?;
            written += 1;
        }
        refresh_fts(&tx, asset_id, &old_concat)?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("tags commit: {e}")))?;
        Ok(written)
    }

    /// Reject a single facet value on an asset (P4 accept/reject, DATA-MODEL §8): delete every
    /// `asset_tag` row for this (asset, tag) pair regardless of source — the user is saying the term
    /// is wrong, not that one model guessed it. Re-indexes FTS with the pre-write text so the
    /// rejected term stops matching. The `tag` vocabulary row is left in place (other assets use it).
    /// Returns the number of rows removed.
    pub fn remove_asset_tag(
        &mut self,
        asset_id: i64,
        namespace: &str,
        value: &str,
    ) -> Result<usize> {
        let Some((norm, _)) = normalize_tag(namespace, value) else {
            return Ok(0); // an empty or punctuation-only value never became a tag
        };
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("reject begin: {e}")))?;
        let old_concat = tags_concat_in(&tx, asset_id)?;
        let n = tx
            .execute(
                "DELETE FROM asset_tag
                  WHERE asset_id = ?1
                    AND tag_id = (SELECT id FROM tag WHERE norm_name = ?2)",
                params![asset_id, norm],
            )
            .map_err(|e| CoreError::Catalog(format!("reject delete: {e}")))?;
        refresh_fts(&tx, asset_id, &old_concat)?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("reject commit: {e}")))?;
        Ok(n)
    }

    /// Mark an asset **confirmed** (human-reviewed tags → trusted, DATA-MODEL §8), or clear that
    /// back to the status the ingest work actually earns. `confirmed` is the one status a human
    /// sets; `promote_statuses` never touches it, so un-confirming recomputes the earned status
    /// inline with the same predicate the reconcile uses (embedding **and** a tag ⇒ `identified`;
    /// facets ⇒ `indexed`) — otherwise the row would sit stale until the next drain.
    pub fn set_confirmed(&self, asset_id: i64, confirmed: bool) -> Result<()> {
        let now = unix_now();
        let sql = if confirmed {
            "UPDATE asset SET status = 'confirmed', updated_at = ?2
               WHERE id = ?1 AND deleted_at IS NULL"
        } else {
            "UPDATE asset SET status = CASE
                 WHEN embedding_dirty = 0
                      AND EXISTS (SELECT 1 FROM asset_embedding e WHERE e.asset_id = asset.id)
                      AND EXISTS (SELECT 1 FROM asset_tag t WHERE t.asset_id = asset.id)
                      THEN 'identified'
                 WHEN metrics_dirty = 0 THEN 'indexed'
                 ELSE 'discovered' END,
                 updated_at = ?2
               WHERE id = ?1 AND deleted_at IS NULL"
        };
        self.conn
            .execute(sql, params![asset_id, now])
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("set_confirmed: {e}")))
    }

    /// One asset's tags, newest source-agnostic view: `(namespace, value, source, confidence)`.
    pub fn tags_for_asset(&self, asset_id: i64) -> Result<Vec<TagAssignment>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT COALESCE(t.namespace, 'free'), t.name, at.confidence
                   FROM asset_tag at JOIN tag t ON t.id = at.tag_id
                  WHERE at.asset_id = ?1
                  ORDER BY t.namespace, t.name",
            )
            .map_err(|e| CoreError::Catalog(format!("tags read prep: {e}")))?;
        let rows = stmt
            .query_map(params![asset_id], |r| {
                Ok(TagAssignment {
                    namespace: r.get(0)?,
                    value: r.get(1)?,
                    confidence: r.get(2)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("tags read query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("tags read row: {e}")))
    }

    // ── Mapping library (DATA-MODEL §4) ─────────────────────────────────────────────────────────
    //
    // Raw `asset_tag`s are observations; `asset_category` is the canonical interpretation, produced
    // by applying a taxonomy's rules. The two are separate so a re-map is non-destructive — it never
    // touches the tags. `apply_mapping` is the engine; the rest is minimal authoring + read.

    // ── Projects & imports (ENGINE-API §5, DATA-MODEL §9) ────────────────────────────────────

    /// Upsert the registered Unity project (ENGINE-API §5.1) and make it the single active editor.
    /// Keyed on `project_path`, so a re-registration refreshes the heartbeat / version / pipeline
    /// rather than minting a second row. Ensures the project's empty staging tray exists (§9). The
    /// partial UNIQUE index `idx_project_active` allows only one `is_active=1`, so every other row is
    /// cleared first. Returns the project id.
    pub fn register_project(
        &self,
        project_path: &str,
        unity_version: Option<&str>,
        pipeline: Option<&str>,
    ) -> Result<i64> {
        let name = project_path
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(project_path)
            .to_string();
        self.conn
            .execute("UPDATE project SET is_active = 0", [])
            .map_err(|e| CoreError::Catalog(format!("project deactivate: {e}")))?;
        self.conn
            .execute(
                "INSERT INTO project
                     (uuid, name, project_path, unity_version, pipeline, last_seen_at, is_active)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1)
                 ON CONFLICT(project_path) DO UPDATE SET
                     name = excluded.name,
                     unity_version = excluded.unity_version,
                     pipeline = excluded.pipeline,
                     last_seen_at = excluded.last_seen_at,
                     is_active = 1",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    name,
                    project_path,
                    unity_version,
                    pipeline,
                    unix_now(),
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("project upsert: {e}")))?;
        let id: i64 = self
            .conn
            .query_row(
                "SELECT id FROM project WHERE project_path = ?1",
                params![project_path],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("project id: {e}")))?;
        // One staging tray per project (§9), created empty on first registration; the partial
        // UNIQUE index enforces uniqueness, the guard keeps a re-register from erroring.
        self.conn
            .execute(
                "INSERT INTO collection (uuid, name, is_staging, project_id)
                     SELECT ?1, 'Staging', 1, ?2
                     WHERE NOT EXISTS
                       (SELECT 1 FROM collection WHERE is_staging=1 AND project_id=?2)",
                params![uuid::Uuid::new_v4().to_string(), id],
            )
            .map_err(|e| CoreError::Catalog(format!("staging tray: {e}")))?;
        Ok(id)
    }

    /// Registered editors, active first (ENGINE-API §5.1 `list_projects` / `GET /v1/projects`). The
    /// active one is the `is_active` row — a client reads "which is active" from this list rather
    /// than a separate call.
    pub fn list_projects(&self) -> Result<Vec<ProjectRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, name, project_path, unity_version, pipeline, is_active, last_seen_at
                   FROM project
                  ORDER BY is_active DESC, last_seen_at DESC, name",
            )
            .map_err(|e| CoreError::Catalog(format!("projects prepare: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ProjectRow {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    project_path: r.get(2)?,
                    unity_version: r.get(3)?,
                    pipeline: r.get(4)?,
                    is_active: r.get::<_, i64>(5)? != 0,
                    last_seen_at: r.get(6)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("projects query: {e}")))?;
        rows.collect::<std::result::Result<_, _>>()
            .map_err(|e| CoreError::Catalog(format!("projects collect: {e}")))
    }

    /// Which of `asset_ids` already have an `import_record` for `project_id` (ENGINE-API §5.4
    /// dup-guard advisory). A caller warns "already imported — re-import?" before enqueuing.
    pub fn already_imported(&self, asset_ids: &[i64], project_id: i64) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT 1 FROM import_record WHERE asset_id = ?1 AND project_id = ?2")
            .map_err(|e| CoreError::Catalog(format!("advisory prepare: {e}")))?;
        let mut out = Vec::new();
        for &aid in asset_ids {
            if stmt
                .exists(params![aid, project_id])
                .map_err(|e| CoreError::Catalog(format!("advisory query: {e}")))?
            {
                out.push(aid);
            }
        }
        Ok(out)
    }

    /// Write the durable `import_record` per asset (ENGINE-API §5.3, the ack outcome). The
    /// `UNIQUE(asset_id, project_id)` dup-guard (DATA-MODEL §9) is enforced by `ON CONFLICT … DO
    /// NOTHING`: a second import of the same asset into the same project is reported
    /// [`ImportOutcome::AlreadyImported`], never a duplicate row. All rows in one call share
    /// `batch_id` so the run can be listed and undone (§9 batches).
    pub fn record_imports(
        &self,
        project_id: i64,
        specs: &[ImportSpec],
        batch_id: &str,
    ) -> Result<Vec<ImportOutcome>> {
        let now = unix_now();
        let mut out = Vec::with_capacity(specs.len());
        for s in specs {
            let changed = self
                .conn
                .execute(
                    "INSERT INTO import_record
                         (asset_id, project_id, source_package_id, dest_path, unity_guid,
                          with_deps, imported_at, batch_id)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(asset_id, project_id) DO NOTHING",
                    params![
                        s.asset_id,
                        project_id,
                        s.source_package_id,
                        s.dest_path,
                        s.unity_guid,
                        s.with_deps as i64,
                        now,
                        batch_id,
                    ],
                )
                .map_err(|e| CoreError::Catalog(format!("import_record insert: {e}")))?;
            out.push(if changed == 1 {
                ImportOutcome::Recorded {
                    asset_id: s.asset_id,
                    import_id: self.conn.last_insert_rowid(),
                }
            } else {
                ImportOutcome::AlreadyImported {
                    asset_id: s.asset_id,
                }
            });
        }
        Ok(out)
    }

    /// Create a taxonomy (mapping library). `activate` makes it the single active one.
    pub fn create_taxonomy(&self, name: &str, activate: bool) -> Result<i64> {
        if activate {
            self.conn
                .execute("UPDATE taxonomy SET is_active = 0", [])
                .map_err(|e| CoreError::Catalog(format!("taxonomy deactivate: {e}")))?;
        }
        self.conn
            .execute(
                "INSERT INTO taxonomy (uuid, name, version, is_active) VALUES (?1, ?2, 1, ?3)",
                params![uuid::Uuid::new_v4().to_string(), name, activate as i64],
            )
            .map_err(|e| CoreError::Catalog(format!("taxonomy insert: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Create a curated starter mapping library (DATA-MODEL §4 "import a starter pack") and make it
    /// active. Categories + rules are shaped to the vocabulary the shipped taggers actually emit
    /// (usage/subject facet values, `media_type` codes), so a fresh library gets useful categories
    /// from one **Apply** with no authoring. Returns the new taxonomy id.
    ///
    /// Rules use `alias` (bare value, matches a tag in any namespace) for specifics at priority 100
    /// and `media_type` fallbacks at 200 — first-match-wins means a music clip lands in Audio/Music
    /// (alias) rather than the coarse Audio (media_type).
    pub fn create_starter_taxonomy(&mut self) -> Result<i64> {
        // (path, [rules as (match_type, pattern, priority)]). Categories are flat/prefixed paths.
        #[allow(clippy::type_complexity)]
        const PACK: &[(&str, &[(&str, &str, i64)])] = &[
            (
                "Characters",
                &[
                    ("alias", "character", 100),
                    ("alias", "creature", 100),
                    ("alias", "monster", 100),
                ],
            ),
            (
                "Environment",
                &[
                    ("alias", "environment", 100),
                    ("alias", "tree", 120),
                    ("alias", "terrain", 120),
                    ("alias", "building", 120),
                ],
            ),
            (
                "Props",
                &[
                    ("alias", "prop", 150),
                    ("alias", "barrel", 150),
                    ("alias", "crate", 150),
                    ("alias", "chest", 150),
                ],
            ),
            (
                "Weapons",
                &[
                    ("alias", "weapon", 100),
                    ("alias", "sword", 100),
                    ("alias", "gun", 100),
                ],
            ),
            (
                "Vehicles",
                &[("alias", "vehicle", 100), ("alias", "ship", 100)],
            ),
            (
                "UI",
                &[
                    ("alias", "ui", 100),
                    ("alias", "icon", 100),
                    ("alias", "button", 100),
                ],
            ),
            ("Audio/Music", &[("alias", "music", 100)]),
            (
                "Audio/SFX",
                &[("alias", "sfx", 100), ("alias", "sound effect", 100)],
            ),
            (
                "Audio/Ambience",
                &[("alias", "ambient", 100), ("alias", "ambience", 100)],
            ),
            ("Audio", &[("media_type", "audio", 200)]),
            ("Materials/Textures", &[("media_type", "texture", 200)]),
            ("Materials", &[("media_type", "material", 200)]),
        ];
        let tax = self.create_taxonomy("Starter", true)?;
        for (path, rules) in PACK {
            let label = path.rsplit('/').next().unwrap_or(path);
            let cat = self.add_category(tax, None, label, path)?;
            for (mt, pat, prio) in *rules {
                self.add_rule(tax, cat, mt, pat, *prio)?;
            }
        }
        Ok(tax)
    }

    /// The active taxonomy's id, if one is set — the taxonomy the apply pass and search pre-filter use.
    pub fn active_taxonomy(&self) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM taxonomy WHERE is_active = 1 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("active taxonomy: {e}")))
    }

    /// Add a category. `path` is the materialized "Environment/Foliage/Trees" (UNIQUE per taxonomy).
    pub fn add_category(
        &self,
        taxonomy_id: i64,
        parent_id: Option<i64>,
        pref_label: &str,
        path: &str,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO category (taxonomy_id, parent_id, pref_label, path)
                 VALUES (?1, ?2, ?3, ?4)",
                params![taxonomy_id, parent_id, pref_label, path],
            )
            .map_err(|e| CoreError::Catalog(format!("category insert: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Add a mapping rule. `match_type` is one of [`MAPPING_MATCH_TYPES`]; `priority` lower wins.
    pub fn add_rule(
        &self,
        taxonomy_id: i64,
        category_id: i64,
        match_type: &str,
        pattern: &str,
        priority: i64,
    ) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO mapping_rule (taxonomy_id, category_id, match_type, pattern, priority)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![taxonomy_id, category_id, match_type, pattern, priority],
            )
            .map_err(|e| CoreError::Catalog(format!("rule insert: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// A manual category assignment — pinned, so `apply_mapping` never clobbers it (DATA-MODEL §4
    /// step 3). Idempotent: re-pinning the same category is a no-op update.
    pub fn assign_category_manual(
        &self,
        asset_id: i64,
        category_id: i64,
        taxonomy_id: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO asset_category
                    (asset_id, category_id, taxonomy_id, assigned_by, rule_id, confidence, is_pinned)
                 VALUES (?1, ?2, ?3, 'manual', NULL, NULL, 1)
                 ON CONFLICT(asset_id, category_id, taxonomy_id)
                 DO UPDATE SET assigned_by = 'manual', is_pinned = 1, rule_id = NULL",
                params![asset_id, category_id, taxonomy_id],
            )
            .map_err(|e| CoreError::Catalog(format!("manual assign: {e}")))?;
        Ok(())
    }

    /// Remove one asset↔category assignment (pinned or not).
    pub fn remove_category_assignment(
        &self,
        asset_id: i64,
        category_id: i64,
        taxonomy_id: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "DELETE FROM asset_category
                 WHERE asset_id = ?1 AND category_id = ?2 AND taxonomy_id = ?3",
                params![asset_id, category_id, taxonomy_id],
            )
            .map_err(|e| CoreError::Catalog(format!("remove assignment: {e}")))?;
        Ok(())
    }

    /// Bump the taxonomy version — the marker a re-map records (DATA-MODEL §4 step 4). `apply_mapping`
    /// regenerates the non-pinned rows regardless; this is the durable "the library changed" signal.
    pub fn bump_taxonomy_version(&self, taxonomy_id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE taxonomy SET version = version + 1 WHERE id = ?1",
                params![taxonomy_id],
            )
            .map_err(|e| CoreError::Catalog(format!("version bump: {e}")))?;
        Ok(())
    }

    /// Categories an asset belongs to in a taxonomy: `(category_id, path, assigned_by, is_pinned)`.
    pub fn categories_of(
        &self,
        asset_id: i64,
        taxonomy_id: i64,
    ) -> Result<Vec<(i64, String, String, bool)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT c.id, c.path, ac.assigned_by, ac.is_pinned
                   FROM asset_category ac JOIN category c ON c.id = ac.category_id
                  WHERE ac.asset_id = ?1 AND ac.taxonomy_id = ?2
                  ORDER BY c.path",
            )
            .map_err(|e| CoreError::Catalog(format!("categories_of prep: {e}")))?;
        let rows = stmt
            .query_map(params![asset_id, taxonomy_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0))
            })
            .map_err(|e| CoreError::Catalog(format!("categories_of query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("categories_of row: {e}")))
    }

    /// Category paths assigned to each live asset in a taxonomy, as `asset_id → [path]` — the map
    /// the Library CATEGORY facet filters on (mirrors `all_asset_tags`).
    pub fn all_asset_categories(
        &self,
        taxonomy_id: i64,
    ) -> Result<std::collections::HashMap<i64, Vec<String>>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT ac.asset_id, c.path
                   FROM asset_category ac
                   JOIN category c ON c.id = ac.category_id
                   JOIN asset a    ON a.id = ac.asset_id AND a.deleted_at IS NULL
                  WHERE ac.taxonomy_id = ?1
                  ORDER BY ac.asset_id, c.path",
            )
            .map_err(|e| CoreError::Catalog(format!("all_asset_categories prep: {e}")))?;
        let rows = stmt
            .query_map(params![taxonomy_id], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(|e| CoreError::Catalog(format!("all_asset_categories query: {e}")))?;
        let mut out: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
        for row in rows {
            let (id, path) =
                row.map_err(|e| CoreError::Catalog(format!("all_asset_categories row: {e}")))?;
            out.entry(id).or_default().push(path);
        }
        Ok(out)
    }

    /// A taxonomy's `(name, version)`, if it exists.
    pub fn taxonomy_meta(&self, taxonomy_id: i64) -> Result<Option<(String, i64)>> {
        self.conn
            .query_row(
                "SELECT name, version FROM taxonomy WHERE id = ?1",
                params![taxonomy_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("taxonomy_meta: {e}")))
    }

    /// Every category in a taxonomy: `(id, pref_label, path)`, ordered by path.
    pub fn list_categories(&self, taxonomy_id: i64) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, pref_label, path FROM category
                  WHERE taxonomy_id = ?1 ORDER BY path",
            )
            .map_err(|e| CoreError::Catalog(format!("list_categories prep: {e}")))?;
        let rows = stmt
            .query_map(params![taxonomy_id], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| CoreError::Catalog(format!("list_categories query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("list_categories row: {e}")))
    }

    /// Every rule in a taxonomy with its category's path, ordered as the apply engine runs them
    /// (`priority ASC, id`): `(id, category_id, category_path, match_type, pattern, priority, enabled)`.
    pub fn list_rules(&self, taxonomy_id: i64) -> Result<Vec<MappingRuleRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT r.id, r.category_id, c.path, r.match_type, r.pattern, r.priority, r.enabled
                   FROM mapping_rule r JOIN category c ON c.id = r.category_id
                  WHERE r.taxonomy_id = ?1
                  ORDER BY r.priority ASC, r.id",
            )
            .map_err(|e| CoreError::Catalog(format!("list_rules prep: {e}")))?;
        let rows = stmt
            .query_map(params![taxonomy_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get::<_, i64>(6)? != 0,
                ))
            })
            .map_err(|e| CoreError::Catalog(format!("list_rules query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("list_rules row: {e}")))
    }

    /// Delete a mapping rule (and, via re-apply, the non-pinned rows it produced).
    pub fn delete_rule(&self, rule_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM mapping_rule WHERE id = ?1", params![rule_id])
            .map_err(|e| CoreError::Catalog(format!("delete_rule: {e}")))?;
        Ok(())
    }

    /// Apply a taxonomy's rules to the whole library (DATA-MODEL §4 step 2). Deterministic and
    /// non-destructive:
    /// - Non-pinned rule rows for the taxonomy are dropped and regenerated; **pinned rows survive**.
    /// - Rules run in `priority` order (lower first) then id; the **first** rule to match an asset
    ///   claims it (a `NOT EXISTS` guard on any existing assignment for the taxonomy), so priority is
    ///   the tie-break and the result never depends on rule insertion order or row iteration order.
    /// - Raw `asset_tag`s are untouched.
    ///
    /// ponytail: one winning category per (asset, taxonomy) — the deterministic core the P3 tests
    /// pin. Multi-category assignment and the `regex`/`embedding_knn` match types are deferred (the
    /// latter needs the embedder + seed vectors); unsupported types are skipped with a warning.
    pub fn apply_mapping(&mut self, taxonomy_id: i64) -> Result<usize> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("apply begin: {e}")))?;
        tx.execute(
            "DELETE FROM asset_category WHERE taxonomy_id = ?1 AND is_pinned = 0",
            params![taxonomy_id],
        )
        .map_err(|e| CoreError::Catalog(format!("apply clear: {e}")))?;

        let rules: Vec<(i64, i64, String, String)> = {
            let mut stmt = tx
                .prepare(
                    "SELECT id, category_id, match_type, pattern FROM mapping_rule
                     WHERE taxonomy_id = ?1 AND enabled = 1
                     ORDER BY priority ASC, id ASC",
                )
                .map_err(|e| CoreError::Catalog(format!("apply rules prep: {e}")))?;
            let rows = stmt
                .query_map(params![taxonomy_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .map_err(|e| CoreError::Catalog(format!("apply rules query: {e}")))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("apply rules row: {e}")))?;
            rows
        };

        // Each rule: insert (asset, its category) for matching, not-yet-assigned, live assets. The
        // NOT EXISTS guard + priority ordering makes the lowest-priority matching rule the winner.
        const HEAD: &str = "INSERT OR IGNORE INTO asset_category
                (asset_id, category_id, taxonomy_id, assigned_by, rule_id, confidence, is_pinned)
             SELECT a.id, ?1, ?2, 'rule', ?3, NULL, 0 FROM asset a
             WHERE a.deleted_at IS NULL
               AND NOT EXISTS (SELECT 1 FROM asset_category ac
                               WHERE ac.asset_id = a.id AND ac.taxonomy_id = ?2)
               AND ";
        let mut assigned = 0usize;
        for (rule_id, category_id, match_type, pattern) in rules {
            let cond = match match_type.as_str() {
                "media_type" => "a.media_type_id = (SELECT id FROM media_type WHERE code = ?4)",
                "exact_tag" => {
                    "EXISTS (SELECT 1 FROM asset_tag at JOIN tag t ON t.id = at.tag_id
                             WHERE at.asset_id = a.id AND t.norm_name = ?4)"
                }
                "alias" => {
                    "EXISTS (SELECT 1 FROM asset_tag at JOIN tag t ON t.id = at.tag_id
                             WHERE at.asset_id = a.id
                               AND (t.norm_name = ?4
                                    -- ESCAPE, because ?4 is the user's rule pattern: an alias
                                    -- literally named `low_poly` would otherwise have its `_` read
                                    -- as a wildcard and match unrelated tags.
                                    OR t.norm_name LIKE '%:' || ?5 ESCAPE '\\'))"
                }
                "glob_path" => {
                    "EXISTS (SELECT 1 FROM asset_file af
                             WHERE af.asset_id = a.id AND af.rel_path GLOB ?4)"
                }
                other => {
                    tracing::warn!(match_type = %other, "mapping: unsupported match_type skipped");
                    continue;
                }
            };
            // exact_tag/alias compare against the lowercased norm text; media_type/glob bind as-is.
            let bind = match match_type.as_str() {
                "exact_tag" | "alias" => pattern.to_lowercase(),
                _ => pattern.clone(),
            };
            // ?5 is the same pattern with LIKE metacharacters escaped, for the alias arm. Bound
            // unconditionally so every branch takes the same parameter list; only `alias` reads it.
            let like_escaped = bind
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_");
            let sql = format!("{HEAD}{cond}");
            // Only the alias arm references ?5, and SQLite rejects a parameter list longer than the
            // statement uses — so bind it per branch rather than unconditionally.
            let mut binds: Vec<&dyn rusqlite::ToSql> =
                vec![&category_id, &taxonomy_id, &rule_id, &bind];
            if match_type == "alias" {
                binds.push(&like_escaped);
            }
            let n = tx
                .execute(&sql, binds.as_slice())
                .map_err(|e| CoreError::Catalog(format!("apply rule {rule_id}: {e}")))?;
            assigned += n;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("apply commit: {e}")))?;
        Ok(assigned)
    }

    /// Store a model-written caption as the asset's description (AI-PIPELINE section 6). The FTS
    /// update trigger re-indexes the row, so a caption is searchable the moment it lands.
    pub fn set_description(&self, asset_id: i64, description: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE asset SET description = ?2, updated_at = ?3 WHERE id = ?1",
                params![asset_id, description, unix_now()],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("set_description: {e}")))
    }

    /// How many assets still await tagging — the job tray's total, so a run shows a real bar
    /// instead of a spinner.
    pub fn count_needing_tags(&self) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM asset a
                   JOIN asset_file af ON af.id = a.primary_file_id
                  WHERE a.tags_dirty = 1 AND a.deleted_at IS NULL AND af.missing = 0",
                [],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("tag queue count: {e}")))
    }

    /// Re-flag assets for a scoped **Identify** pass (embedding + tags, AI-PIPELINE §2). Setting the
    /// dirty flags is all that is needed — the drains then re-embed and re-tag, and `drain_tags` only
    /// touches `source='ai'` rows, so a re-run never clobbers the user's own tags. `Full` re-does the
    /// whole live library; `Unidentified` re-does only assets that have **no tag at all** (never
    /// tagged, or the tagger produced nothing); `New` needs no flag — a never-processed asset is still
    /// dirty from its mint. Returns the number of rows flagged.
    pub fn flag_identify_all(&self) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE asset SET embedding_dirty = 1, tags_dirty = 1 WHERE deleted_at IS NULL",
                [],
            )
            .map_err(|e| CoreError::Catalog(format!("flag identify all: {e}")))
    }

    pub fn flag_identify_untagged(&self) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE asset SET embedding_dirty = 1, tags_dirty = 1
                 WHERE deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM asset_tag t WHERE t.asset_id = asset.id)",
                [],
            )
            .map_err(|e| CoreError::Catalog(format!("flag identify untagged: {e}")))
    }

    /// Flag a specific set of assets (right-click "Identify selected").
    pub fn flag_identify_assets(&self, ids: &[i64]) -> Result<usize> {
        let mut n = 0;
        let tx_ids: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        // Chunked IN-lists keep the statement bounded for a huge multi-select.
        for chunk in tx_ids.chunks(500) {
            let placeholders = chunk.join(",");
            n += self
                .conn
                .execute(
                    &format!(
                        "UPDATE asset SET embedding_dirty = 1, tags_dirty = 1
                         WHERE deleted_at IS NULL AND id IN ({placeholders})"
                    ),
                    [],
                )
                .map_err(|e| CoreError::Catalog(format!("flag identify assets: {e}")))?;
        }
        Ok(n)
    }

    /// Store what a pack says about itself (DATA-MODEL §7): version, licence, attribution and a
    /// licence URL, harvested from its embedded manifest/licence files or defaulted by origin.
    ///
    /// Every field is "set only if we learned something": a later browse of a pack that states
    /// less must not erase what an earlier ingest found.
    pub fn set_package_metadata(
        &self,
        package_id: i64,
        version: Option<&str>,
        license: Option<&str>,
        attribution: Option<&str>,
        license_url: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE package
                    SET version     = COALESCE(?2, version),
                        license     = COALESCE(?3, license),
                        attribution = COALESCE(?4, attribution),
                        license_url = COALESCE(?5, license_url)
                  WHERE id = ?1",
                params![package_id, version, license, attribution, license_url],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("set_package_metadata: {e}")))
    }

    /// For every material that resolves a `uses_texture` edge, the **absolute path** of the
    /// texture to paint on its preview sphere (DATA-MODEL §10 → UI-SPEC §13.1).
    ///
    /// A material with several textures gets its lowest-id one: the albedo is what a shader ball
    /// should show, and in a Unity `.mat` the albedo binding is written before the normal and
    /// emissive maps, so insertion order is a better guess than any name heuristic.
    /// Package-extracted textures are skipped — they live in the workspace, not under a source
    /// root, and the caller would need the workspace path to resolve them.
    pub fn material_albedo_paths(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT r.from_asset_id, MIN(r.id), s.root_path || '/' || af.rel_path
                   FROM asset_relationship r
                   JOIN asset m          ON m.id = r.from_asset_id AND m.deleted_at IS NULL
                   JOIN asset t          ON t.id = r.to_asset_id   AND t.deleted_at IS NULL
                   JOIN asset_file af    ON af.id = t.primary_file_id AND af.missing = 0
                                        AND af.workspace_rel IS NULL
                   JOIN import_source s  ON s.id = t.source_id
                  WHERE r.kind = 'uses_texture' AND r.to_asset_id IS NOT NULL
                  GROUP BY r.from_asset_id",
            )
            .map_err(|e| CoreError::Catalog(format!("albedo prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(2)?)))
            .map_err(|e| CoreError::Catalog(format!("albedo query: {e}")))?;
        rows.collect::<std::result::Result<std::collections::HashMap<_, _>, _>>()
            .map_err(|e| CoreError::Catalog(format!("albedo row: {e}")))
    }

    /// Every live asset's tags in one query, keyed by asset id — what the grid needs to draw
    /// chips and filter on a trait without a query per row.
    ///
    /// ponytail: whole-library read, refreshed per catalog sync like the dependency edge sets. At
    /// ~10 tags per asset that is tens of thousands of small rows, which SQLite hands over in
    /// milliseconds; page it if a library ever makes that false.
    pub fn all_asset_tags(&self) -> Result<std::collections::HashMap<i64, Vec<TagAssignment>>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT at.asset_id, COALESCE(t.namespace, 'free'), t.name, at.confidence
                   FROM asset_tag at
                   JOIN tag t   ON t.id = at.tag_id
                   JOIN asset a ON a.id = at.asset_id AND a.deleted_at IS NULL
                  ORDER BY at.asset_id, t.namespace, t.name",
            )
            .map_err(|e| CoreError::Catalog(format!("all tags prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    TagAssignment {
                        namespace: r.get(1)?,
                        value: r.get(2)?,
                        confidence: r.get(3)?,
                    },
                ))
            })
            .map_err(|e| CoreError::Catalog(format!("all tags query: {e}")))?;
        let mut out: std::collections::HashMap<i64, Vec<TagAssignment>> =
            std::collections::HashMap::new();
        for row in rows {
            let (id, tag) = row.map_err(|e| CoreError::Catalog(format!("all tags row: {e}")))?;
            out.entry(id).or_default().push(tag);
        }
        Ok(out)
    }

    /// Tag vocabulary with usage counts, most-used first — the Filters TRAIT chips and the Traits
    /// screen. Returns `(namespace, value, asset count)`.
    pub fn tag_usage(&self, limit: usize) -> Result<Vec<(String, String, i64)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT COALESCE(t.namespace, 'free'), t.name, COUNT(DISTINCT at.asset_id) n
                   FROM tag t
                   JOIN asset_tag at ON at.tag_id = t.id
                   JOIN asset a      ON a.id = at.asset_id AND a.deleted_at IS NULL
                  GROUP BY t.id
                  ORDER BY n DESC, t.name
                  LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("tag usage prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .map_err(|e| CoreError::Catalog(format!("tag usage query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("tag usage row: {e}")))
    }

    /// The whole tagging vocabulary for the Traits screen (UI-SPEC §9). Every `tag` with its live
    /// usage count and Trait-Library fields, traits first. A trait with no assignments yet (freshly
    /// created) still lists, so the LEFT JOINs keep it; deleted assets never count.
    pub fn list_vocabulary(&self) -> Result<Vec<VocabTrait>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT t.id, COALESCE(t.namespace, 'free'), t.name,
                        COUNT(DISTINCT CASE WHEN a.id IS NOT NULL THEN at.asset_id END) AS n,
                        COALESCE(SUM(CASE WHEN at.source='manual' AND a.id IS NOT NULL THEN 1 END), 0),
                        COALESCE(SUM(CASE WHEN at.source='ai'     AND a.id IS NOT NULL THEN 1 END), 0),
                        t.is_trait, t.scope, t.usage_note, t.teach, t.color, t.icon
                   FROM tag t
                   LEFT JOIN asset_tag at ON at.tag_id = t.id
                   LEFT JOIN asset a      ON a.id = at.asset_id AND a.deleted_at IS NULL
                  GROUP BY t.id
                  ORDER BY t.is_trait DESC, n DESC, t.name",
            )
            .map_err(|e| CoreError::Catalog(format!("vocab prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(VocabTrait {
                    id: r.get(0)?,
                    namespace: r.get(1)?,
                    value: r.get(2)?,
                    usage_count: r.get(3)?,
                    manual_count: r.get(4)?,
                    ai_count: r.get(5)?,
                    is_trait: r.get::<_, i64>(6)? != 0,
                    scope: r.get(7)?,
                    usage_note: r.get(8)?,
                    teach: r.get::<_, i64>(9)? != 0,
                    color: r.get(10)?,
                    icon: r.get(11)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("vocab query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("vocab row: {e}")))
    }

    /// Teaching vocabulary fed to the identifier (AI-PIPELINE §6): the traits a user has marked
    /// `teach=1`, as `(namespace, value, scope, usage_note)`. `scope` is a `media_type` code or
    /// `None` for any modality; a paused trait (`teach=0`) is excluded but keeps its tags.
    pub fn teaching_traits(&self) -> Result<Vec<TeachRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT COALESCE(t.namespace, 'free'), t.name, t.scope, t.usage_note
                   FROM tag t
                  WHERE t.is_trait = 1 AND t.teach = 1
                  ORDER BY t.namespace, t.name",
            )
            .map_err(|e| CoreError::Catalog(format!("teach prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
            .map_err(|e| CoreError::Catalog(format!("teach query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("teach row: {e}")))
    }

    /// Create or promote a trait (UI-SPEC §9 NEW TRAIT + PROMOTE). Upserts the `tag` row by its
    /// namespaced `norm_name` and flags it a first-class trait; an existing tag (already in use on
    /// assets) keeps its assignments and simply gains trait status. Returns the tag id.
    pub fn upsert_trait(
        &self,
        namespace: &str,
        value: &str,
        scope: Option<&str>,
        usage_note: Option<&str>,
        teach: bool,
    ) -> Result<i64> {
        let Some((norm, display)) = normalize_tag(namespace, value) else {
            return Err(CoreError::Catalog("empty trait name".into()));
        };
        self.conn
            .execute(
                "INSERT INTO tag (name, norm_name, namespace, is_trait, scope, usage_note, teach)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6)
                 ON CONFLICT(norm_name) DO UPDATE SET
                    is_trait = 1, scope = ?4, usage_note = ?5, teach = ?6",
                params![display, norm, namespace, scope, usage_note, teach as i64],
            )
            .map_err(|e| CoreError::Catalog(format!("upsert_trait: {e}")))?;
        self.conn
            .query_row(
                "SELECT id FROM tag WHERE norm_name = ?1",
                params![norm],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("upsert_trait id: {e}")))
    }

    /// Demote a trait back to a plain tag (UI-SPEC §9 delete: *"existing tags on assets are kept"*).
    /// Clears every Trait-Library field but never touches `asset_tag`, so the vocabulary term and
    /// its assignments survive — only its first-class status and teaching are removed.
    pub fn unpromote_trait(&self, tag_id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tag SET is_trait = 0, teach = 0, scope = NULL, usage_note = NULL,
                                color = NULL, icon = NULL
                 WHERE id = ?1",
                params![tag_id],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("unpromote_trait: {e}")))
    }

    /// Toggle a trait's teaching flag (UI-SPEC §9 ✦ teaching / ⏸ paused).
    pub fn set_trait_teach(&self, tag_id: i64, teach: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tag SET teach = ?2 WHERE id = ?1 AND is_trait = 1",
                params![tag_id, teach as i64],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("set_trait_teach: {e}")))
    }

    /// Set a trait's chip style (UI-SPEC §9 ✎ style — 6-colour palette + glyph). `None` clears.
    pub fn set_trait_style(
        &self,
        tag_id: i64,
        color: Option<&str>,
        icon: Option<&str>,
    ) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tag SET color = ?2, icon = ?3 WHERE id = ?1 AND is_trait = 1",
                params![tag_id, color, icon],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("set_trait_style: {e}")))
    }

    /// Assets still awaiting identification (`tags_dirty`), for the tagging drain. Same job shape
    /// and path rules as the other queues.
    pub fn assets_needing_tags(&self, limit: usize) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id,
                        CASE WHEN af.workspace_rel IS NULL AND af.package_entry_id IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel, a.media_type_id, a.source_id, mt.code,
                        pk.package_path, pe.tar_offset, pe.size_bytes, af.ext
                 FROM asset a
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN import_source s ON s.id = a.source_id
                 JOIN asset_file af   ON af.id = a.primary_file_id
                 LEFT JOIN package_entry pe ON pe.id = af.package_entry_id
                 LEFT JOIN package pk       ON pk.id = pe.package_id
                 WHERE a.tags_dirty = 1 AND a.deleted_at IS NULL
                   AND af.missing = 0
                 ORDER BY a.id LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("tag queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(VisualEmbedJob {
                    asset_id: r.get(0)?,
                    disk_path: r.get(1)?,
                    workspace_rel: r.get(2)?,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                    media_code: r.get(5)?,
                    pack_path: r.get(6)?,
                    pack_offset: r.get(7)?,
                    pack_size: r.get(8)?,
                    ext: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("tag queue query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("tag queue row: {e}")))
    }

    /// Both sides of an asset's dependency graph for the inspector's LINKED FILES panel
    /// (UI-SPEC §13.6): what it uses (outbound) and what uses it (inbound). Unresolved outbound
    /// refs are included — a link the library can't satisfy is exactly what the panel must show.
    pub fn related_assets(&self, asset_id: i64) -> Result<Vec<RelatedAsset>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT 1, r.kind, r.state, r.declared_path, t.id, t.name, mt.code
                   FROM asset_relationship r
                   LEFT JOIN asset t      ON t.id = r.to_asset_id AND t.deleted_at IS NULL
                   LEFT JOIN media_type mt ON mt.id = t.media_type_id
                  WHERE r.from_asset_id = ?1
                 UNION ALL
                 SELECT 0, r.kind, r.state, r.declared_path, f.id, f.name, mt.code
                   FROM asset_relationship r
                   JOIN asset f            ON f.id = r.from_asset_id AND f.deleted_at IS NULL
                   LEFT JOIN media_type mt ON mt.id = f.media_type_id
                  WHERE r.to_asset_id = ?1
                  ORDER BY 1 DESC, 6",
            )
            .map_err(|e| CoreError::Catalog(format!("related prep: {e}")))?;
        let rows = stmt
            .query_map(params![asset_id], |r| {
                Ok(RelatedAsset {
                    outbound: r.get::<_, i64>(0)? == 1,
                    kind: r.get(1)?,
                    state: r.get(2)?,
                    declared_path: r.get(3)?,
                    asset_id: r.get(4)?,
                    name: r.get(5)?,
                    media_code: r.get(6)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("related query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("related row: {e}")))
    }

    /// The two id sets behind the `has_dependencies` / `has_dependents` filter chips
    /// (DATA-MODEL §10): assets with at least one outbound edge, and assets something points at.
    /// Read once per catalog sync — cheaper than a per-row `EXISTS` while filtering the grid.
    pub fn assets_with_edges(&self) -> Result<(Vec<i64>, Vec<i64>)> {
        let collect = |sql: &str| -> Result<Vec<i64>> {
            let mut stmt = self
                .conn
                .prepare(sql)
                .map_err(|e| CoreError::Catalog(format!("edge set prep: {e}")))?;
            let rows = stmt
                .query_map([], |r| r.get(0))
                .map_err(|e| CoreError::Catalog(format!("edge set query: {e}")))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("edge set row: {e}")))
        };
        let has_deps = collect(
            "SELECT DISTINCT from_asset_id FROM asset_relationship WHERE to_asset_id IS NOT NULL",
        )?;
        let has_dependents = collect(
            "SELECT DISTINCT to_asset_id FROM asset_relationship WHERE to_asset_id IS NOT NULL",
        )?;
        Ok((has_deps, has_dependents))
    }

    /// Replace the prefab-derived edges with `pairs` of `(mesh GUID, material GUID)` (§10,
    /// `source='unity_prefab'`). Pairs whose mesh isn't catalogued are dropped — an edge needs a
    /// real `from_asset_id` — while an unresolved *material* is kept `missing` like any other,
    /// so a later extraction can late-bind it.
    ///
    /// Rebuild-in-full rather than diff: the pass walks every prefab anyway, and at this scale
    /// (thousands of prefabs, a handful of pairs each) one delete + reinsert inside a transaction
    /// is both simpler and correct when a prefab is edited or deleted.
    pub fn replace_prefab_relationships(&mut self, pairs: &[(String, String)]) -> Result<usize> {
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("prefab edges begin: {e}")))?;
        tx.execute(
            "DELETE FROM asset_relationship WHERE source = 'unity_prefab'",
            [],
        )
        .map_err(|e| CoreError::Catalog(format!("prefab edges clear: {e}")))?;
        let mut written = 0usize;
        for (mesh_guid, material_guid) in pairs {
            written += tx
                .execute(
                    "INSERT INTO asset_relationship
                        (from_asset_id, to_asset_id, kind, source, confidence, declared_path, state)
                     SELECT a.id,
                            (SELECT id FROM asset
                              WHERE unity_guid = ?2 AND deleted_at IS NULL),
                            'uses_material', 'unity_prefab', 0.9, ?2,
                            CASE WHEN EXISTS (SELECT 1 FROM asset
                                               WHERE unity_guid = ?2 AND deleted_at IS NULL)
                                 THEN 'resolved' ELSE 'missing' END
                       FROM asset a
                      WHERE a.unity_guid = ?1 AND a.deleted_at IS NULL
                     ON CONFLICT(from_asset_id, to_asset_id, kind) DO NOTHING",
                    params![mesh_guid, material_guid],
                )
                .map_err(|e| CoreError::Catalog(format!("prefab edge insert: {e}")))?;
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("prefab edges commit: {e}")))?;
        Ok(written)
    }

    /// Every asset reachable from `seeds` over resolved dependency edges, seeds included
    /// (DATA-MODEL §10; ENGINE-API §4 "export with dependencies"). Unresolved (`missing`) edges
    /// point at nothing in the library, so they contribute nothing to a copy.
    ///
    /// The recursive `UNION` dedupes, which is also what terminates on a reference cycle
    /// (material ↔ material is legal in Unity).
    pub fn dependency_closure(&self, seeds: &[i64]) -> Result<Vec<i64>> {
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        // seeds are i64s the caller already holds — safe to inline (rusqlite has no array binds).
        let seed_list = seeds
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("),(");
        let sql = format!(
            "WITH RECURSIVE closure(id) AS (
                 VALUES ({seed_list})
                 UNION
                 SELECT r.to_asset_id FROM asset_relationship r
                   JOIN closure c ON r.from_asset_id = c.id
                  WHERE r.to_asset_id IS NOT NULL
             )
             SELECT c.id FROM closure c
               JOIN asset a ON a.id = c.id
              WHERE a.deleted_at IS NULL
              ORDER BY c.id"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| CoreError::Catalog(format!("closure prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| CoreError::Catalog(format!("closure query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("closure row: {e}")))
    }

    /// Late-bind the edges whose target has since been catalogued. One statement, run after every
    /// dependency drain — cheaper than re-parsing the referencing files, and the reason a
    /// `missing` edge is recorded rather than dropped. Returns how many edges resolved.
    pub fn resolve_pending_relationships(&self) -> Result<usize> {
        self.conn
            .execute(
                "UPDATE asset_relationship
                    SET to_asset_id = (SELECT id FROM asset
                                        WHERE unity_guid = asset_relationship.declared_path
                                          AND deleted_at IS NULL),
                        state = 'resolved'
                  WHERE to_asset_id IS NULL
                    AND EXISTS (SELECT 1 FROM asset
                                 WHERE unity_guid = asset_relationship.declared_path
                                   AND deleted_at IS NULL)",
                [],
            )
            .map_err(|e| CoreError::Catalog(format!("resolve pending: {e}")))
    }

    /// Store extracted facets and clear the dirty flag. The flag clears even when every field is
    /// `None` — same forward-progress rule as the embedding drain's zero vector, so an
    /// unparseable file isn't retried on every scan.
    pub fn set_metrics(&self, asset_id: i64, m: AssetMetrics) -> Result<()> {
        self.conn
            .execute(
                "UPDATE asset SET tri_count = ?2, width_px = ?3, height_px = ?4,
                                  duration_ms = ?5, metrics_dirty = 0
                 WHERE id = ?1",
                params![
                    asset_id,
                    m.tri_count,
                    m.width_px,
                    m.height_px,
                    m.duration_ms
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("set_metrics: {e}")))?;
        Ok(())
    }

    /// Store an asset's traced vector preview (M15). `source_key` is the mesh cache key it was
    /// generated from, so a later scan can tell a stale preview from a current one without
    /// re-rendering. Replaces any existing row — one preview per asset, always the current one.
    pub fn set_vector_preview(
        &self,
        asset_id: i64,
        svg: &str,
        view_count: i64,
        source_key: &str,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO asset_preview (asset_id, svg, view_count, source_key, generated_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))
                 ON CONFLICT(asset_id) DO UPDATE SET
                     svg = excluded.svg,
                     view_count = excluded.view_count,
                     source_key = excluded.source_key,
                     generated_at = excluded.generated_at",
                params![asset_id, svg.as_bytes(), view_count, source_key],
            )
            .map_err(|e| CoreError::Catalog(format!("set_vector_preview: {e}")))?;
        Ok(())
    }

    /// An asset's vector preview and the key it was generated from. `None` when there is none.
    pub fn vector_preview(&self, asset_id: i64) -> Result<Option<(String, String)>> {
        self.conn
            .query_row(
                "SELECT svg, source_key FROM asset_preview WHERE asset_id = ?1",
                params![asset_id],
                |r| {
                    let svg: Vec<u8> = r.get(0)?;
                    Ok((String::from_utf8_lossy(&svg).into_owned(), r.get(1)?))
                },
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("vector_preview: {e}")))
    }

    /// Whether `asset_id` already has a preview built from `source_key` — the skip check the drain
    /// runs before spending four CPU renders on a model nothing has changed about.
    pub fn has_current_preview(&self, asset_id: i64, source_key: &str) -> Result<bool> {
        Ok(self
            .vector_preview(asset_id)?
            .is_some_and(|(_, key)| key == source_key))
    }

    /// Models that have no stored vector preview yet.
    ///
    /// Preview generation used to ride inside the metrics drain, which visits only
    /// `metrics_dirty = 1` rows. That silently did nothing on any library indexed before previews
    /// existed: those assets were already metriced, so they were never revisited and never got a
    /// preview. This queue asks the question previews actually depend on — "is there a row in
    /// `asset_preview`?" — so a backlog drains once and stays drained.
    /// `after_id` is an exclusive cursor. A model whose preview generation *fails* writes no
    /// `asset_preview` row, so it stays in this queue; without a cursor a drain would re-attempt
    /// the same broken batch on every pass and never reach the models behind it. The caller
    /// advances the cursor past everything it attempted, successful or not.
    pub fn assets_needing_preview(
        &self,
        after_id: i64,
        limit: usize,
    ) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id,
                        CASE WHEN af.workspace_rel IS NULL AND af.package_entry_id IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel, a.media_type_id, a.source_id, mt.code,
                        pk.package_path, pe.tar_offset, pe.size_bytes, af.ext
                 FROM asset a
                 JOIN media_type mt   ON mt.id = a.media_type_id
                 JOIN import_source s ON s.id = a.source_id
                 JOIN asset_file af   ON af.id = a.primary_file_id
                 LEFT JOIN package_entry pe ON pe.id = af.package_entry_id
                 LEFT JOIN package pk       ON pk.id = pe.package_id
                 LEFT JOIN asset_preview ap ON ap.asset_id = a.id
                 WHERE mt.code = 'model_3d'
                   AND ap.asset_id IS NULL
                   AND a.id > ?1
                   AND a.deleted_at IS NULL
                   AND af.missing = 0
                 ORDER BY a.id LIMIT ?2",
            )
            .map_err(|e| CoreError::Catalog(format!("preview queue prep: {e}")))?;
        let rows = stmt
            .query_map(params![after_id, limit as i64], |r| {
                Ok(VisualEmbedJob {
                    asset_id: r.get(0)?,
                    disk_path: r.get(1)?,
                    workspace_rel: r.get(2)?,
                    media_type_id: r.get(3)?,
                    source_id: r.get(4)?,
                    media_code: r.get(5)?,
                    pack_path: r.get(6)?,
                    pack_offset: r.get(7)?,
                    pack_size: r.get(8)?,
                    ext: r.get(9)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("preview queue: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("preview queue row: {e}")))
    }

    /// The albedo texture a model paints with, via the dependency graph (DATA-MODEL §10):
    /// `model --uses_material--> material --uses_texture--> image`.
    ///
    /// **Two hops on purpose.** One hop lands on the material, which is not an image; a caller that
    /// stopped there would hand a material id to an image loader and get a silent blank texture.
    /// Only `resolved` edges count — a `missing` edge names a texture that was never found.
    /// A model with several materials yields the lowest texture id, deterministically, so repeated
    /// calls agree (see the one-albedo-per-model limitation in the viewer).
    pub fn model_albedo_texture(&self, asset_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT MIN(t.id)
                   FROM asset_relationship m
                   JOIN asset_relationship x ON x.from_asset_id = m.to_asset_id
                   JOIN asset t              ON t.id = x.to_asset_id
                   JOIN media_type mt        ON mt.id = t.media_type_id
                  WHERE m.from_asset_id = ?1
                    AND m.kind = 'uses_material' AND m.state = 'resolved'
                    AND x.kind = 'uses_texture'  AND x.state = 'resolved'
                    AND t.deleted_at IS NULL
                    AND mt.code IN ('image', 'texture', 'sprite')",
                params![asset_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(|e| CoreError::Catalog(format!("model_albedo_texture: {e}")))
    }

    /// Every asset id carrying a stored preview.
    ///
    /// One query for the whole library rather than `vector_preview` per row: a grid deciding
    /// which cards can draw vector previews asks about thousands of assets at once, and the
    /// answer is a set of ids, not blobs — the blobs are fetched per card over `ls://`.
    pub fn assets_with_preview(&self) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT asset_id FROM asset_preview")
            .map_err(|e| CoreError::Catalog(format!("assets_with_preview: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get(0))
            .map_err(|e| CoreError::Catalog(format!("assets_with_preview: {e}")))?;
        rows.collect::<std::result::Result<Vec<i64>, _>>()
            .map_err(|e| CoreError::Catalog(format!("assets_with_preview: {e}")))
    }

    /// Total bytes of stored previews — the catalog half of the footprint the Storage settings
    /// report, now that previews are rows rather than cache files.
    pub fn vector_preview_bytes(&self) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COALESCE(SUM(LENGTH(svg)), 0) FROM asset_preview",
                [],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("vector_preview_bytes: {e}")))
    }

    /// Summaries for `ids`, in the order given (the agent planes rank first, then hydrate).
    /// Unknown or soft-deleted ids are dropped rather than erroring — a stale id from a client
    /// is a miss, not a fault.
    pub fn asset_summaries(&self, ids: &[i64]) -> Result<Vec<AssetSummaryRow>> {
        let mut found: HashMap<i64, AssetSummaryRow> = HashMap::new();
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id, a.uuid, a.name, mt.code, af.ext, a.status,
                        COALESCE(af.size_bytes, 0),
                        a.package_id, p.name,
                        CASE WHEN af.workspace_rel IS NULL
                             THEN s.root_path || '/' || af.rel_path END,
                        af.workspace_rel,
                        a.tri_count, a.width_px, a.height_px, a.duration_ms
                 FROM asset a
                 JOIN media_type mt    ON mt.id = a.media_type_id
                 JOIN import_source s  ON s.id = a.source_id
                 JOIN asset_file af    ON af.id = a.primary_file_id
                 LEFT JOIN package p   ON p.id = a.package_id
                 WHERE a.id = ?1 AND a.deleted_at IS NULL",
            )
            .map_err(|e| CoreError::Catalog(format!("summary prep: {e}")))?;
        for id in ids {
            let row = stmt
                .query_row(params![id], |r| {
                    Ok(AssetSummaryRow {
                        id: r.get(0)?,
                        uuid: r.get(1)?,
                        name: r.get(2)?,
                        media_type: r.get(3)?,
                        format: r.get(4)?,
                        status: r.get(5)?,
                        size_bytes: r.get(6)?,
                        package_id: r.get(7)?,
                        package_name: r.get(8)?,
                        disk_path: r.get(9)?,
                        workspace_rel: r.get(10)?,
                        metrics: AssetMetrics::from_row(r, 11)?,
                    })
                })
                .optional()
                .map_err(|e| CoreError::Catalog(format!("summary query: {e}")))?;
            if let Some(row) = row {
                found.insert(row.id, row);
            }
        }
        Ok(ids.iter().filter_map(|id| found.remove(id)).collect())
    }

    /// The most recently catalogued assets — what a search with no query returns, so a client
    /// that just connected sees something real instead of an empty list.
    pub fn recent_asset_ids(&self, limit: usize) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id FROM asset WHERE deleted_at IS NULL
                 ORDER BY updated_at DESC, id DESC LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("recent prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| r.get::<_, i64>(0))
            .map_err(|e| CoreError::Catalog(format!("recent query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("recent row: {e}")))
    }

    /// The asset's own embedding, if it has one: `(model_id, vector)`. "More like this" runs in
    /// whichever space the asset was encoded into — SigLIP for visuals, CLAP for audio — because
    /// the two are not comparable (IDENTIFICATION-AND-SEARCH §5.B). An asset with several rows
    /// yields the largest vector, which in practice is the visual one.
    pub fn embedding_of(&self, asset_id: i64) -> Result<Option<(String, Vec<f32>)>> {
        self.conn
            .query_row(
                "SELECT model_id, embedding FROM asset_embedding
                 WHERE asset_id = ?1 ORDER BY dim DESC LIMIT 1",
                params![asset_id],
                |r| {
                    let model_id: String = r.get(0)?;
                    let blob: Vec<u8> = r.get(1)?;
                    Ok((model_id, decode_vector(&blob)))
                },
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("embedding_of: {e}")))
    }

    /// Every registered source root. The export sandbox refuses to write inside one — the
    /// library is read-only by invariant (README "non-destructive"), so a client must not be able
    /// to turn an export into a write into the user's own asset folders.
    pub fn source_roots(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT root_path FROM import_source")
            .map_err(|e| CoreError::Catalog(format!("roots prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| CoreError::Catalog(format!("roots query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("roots row: {e}")))
    }

    /// Look up a `media_type.code`'s id (the pre-filter column the search paths take).
    pub fn media_type_id(&self, code: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM media_type WHERE code = ?1",
                params![code],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("media_type lookup: {e}")))
    }

    pub fn upsert_embedding(
        &mut self,
        asset_id: i64,
        model_id: &str,
        modality: &str,
        media_type_id: Option<i64>,
        source_id: Option<i64>,
        vector: &[f32],
    ) -> Result<()> {
        let blob = encode_vector(vector);
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("embed begin: {e}")))?;
        tx.execute(
            "INSERT INTO asset_embedding
                (asset_id, model_id, modality, dim, media_type_id, source_id, embedding, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(asset_id, model_id) DO UPDATE SET
                modality = excluded.modality, dim = excluded.dim,
                media_type_id = excluded.media_type_id, source_id = excluded.source_id,
                embedding = excluded.embedding, created_at = excluded.created_at",
            params![
                asset_id,
                model_id,
                modality,
                vector.len() as i64,
                media_type_id,
                source_id,
                blob,
                unix_now(),
            ],
        )
        .map_err(|e| CoreError::Catalog(format!("embed upsert: {e}")))?;
        tx.execute(
            "UPDATE asset SET embedding_dirty = 0 WHERE id = ?1",
            params![asset_id],
        )
        .map_err(|e| CoreError::Catalog(format!("embed clear dirty: {e}")))?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("embed commit: {e}")))?;
        Ok(())
    }

    /// Load candidate vectors for a model, optionally pre-filtered by media type (the
    /// KNN shard column). Soft-deleted assets are excluded. Feeds
    /// [`crate::vector::BruteForceIndex`] — the structural pre-filter of DATA-MODEL §3.
    pub fn embeddings_for_knn(
        &self,
        model_id: &str,
        media_type_id: Option<i64>,
    ) -> Result<Vec<(i64, Vec<f32>)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT e.asset_id, e.embedding
                 FROM asset_embedding e
                 JOIN asset a ON a.id = e.asset_id
                 WHERE e.model_id = ?1 AND a.deleted_at IS NULL
                   AND (?2 IS NULL OR e.media_type_id = ?2)",
            )
            .map_err(|e| CoreError::Catalog(format!("knn load prep: {e}")))?;
        let rows = stmt
            .query_map(params![model_id, media_type_id], |r| {
                let id: i64 = r.get(0)?;
                let blob: Vec<u8> = r.get(1)?;
                Ok((id, decode_vector(&blob)))
            })
            .map_err(|e| CoreError::Catalog(format!("knn load query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("knn load row: {e}")))
    }

    /// Search by Tags (IDENTIFICATION-AND-SEARCH §5.A): FTS5/BM25 over asset name +
    /// description + denormalised tags. Returns matching asset ids **most-relevant
    /// first**, capped at `limit`, soft-deleted rows excluded. An empty or
    /// punctuation-only query yields `None` (the caller shows the unfiltered set).
    ///
    /// The raw user text never reaches FTS5's MATCH grammar directly — [`fts_match_query`]
    /// tokenises it and prefix-quotes each term, so a stray `"`/`*`/`AND` can't error the
    /// query or change its meaning (typeahead-friendly: every term matches as a prefix).
    pub fn search_assets(&self, query: &str, limit: usize) -> Result<Option<Vec<i64>>> {
        let Some(match_expr) = fts_match_query(query) else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare(
                "SELECT f.rowid
                 FROM asset_fts f
                 JOIN asset a ON a.id = f.rowid
                 WHERE asset_fts MATCH ?1 AND a.deleted_at IS NULL
                 ORDER BY bm25(asset_fts) LIMIT ?2",
            )
            .map_err(|e| CoreError::Catalog(format!("search prep: {e}")))?;
        let rows = stmt
            .query_map(params![match_expr, limit as i64], |r| r.get::<_, i64>(0))
            .map_err(|e| CoreError::Catalog(format!("search query: {e}")))?;
        let ids = rows
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("search row: {e}")))?;
        Ok(Some(ids))
    }

    /// Hybrid search (IDENTIFICATION-AND-SEARCH §5.B, DATA-MODEL §3): run every retriever over
    /// the (media-type pre-filtered) set and fuse them with RRF.
    ///
    /// - **BM25** over `query` (skipped when the query is empty/punctuation-only).
    /// - **Vector KNN** once per entry in `spaces`, each `(model_id, query_vec)`.
    ///
    /// `spaces` is a list because the modalities live in **separate, non-comparable vector
    /// spaces** (§5.B "joint SigLIP/CLAP space"): SigLIP embeds visuals at 768-d, CLAP embeds
    /// audio at 512-d, and a query is embedded into each. Searching them independently and
    /// fusing on *rank* is precisely what RRF is for — raw scores across different models are
    /// meaningless to compare. An empty `spaces` degrades to pure structural (Search by Tags).
    ///
    /// Returns fused asset ids, most-relevant first, capped at `limit`. The caller supplies the
    /// query vectors (embedding is `lodestash-ai`'s job — keeps this crate model-free).
    /// ponytail: no cross-encoder rerank step yet (§5.B step 4, opt-in) — add it behind a flag
    /// if top-N precision needs it.
    pub fn hybrid_search(
        &self,
        query: &str,
        spaces: &[(&str, &[f32])],
        media_type_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<i64>> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        // Retrieve deeper than `limit` from each side so RRF has ranks to fuse; the fused
        // list is truncated to `limit` at the end.
        let depth = limit.saturating_mul(5).max(50);

        let mut rankings = Vec::with_capacity(spaces.len() + 1);
        rankings.push(self.search_assets(query, depth)?.unwrap_or_default());

        for (model_id, query_vec) in spaces {
            let candidates = self.embeddings_for_knn(model_id, media_type_id)?;
            rankings.push(
                BruteForceIndex::new(candidates)
                    .knn(query_vec, depth)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect(),
            );
        }

        // RRF fuses on rank, so an empty retriever simply contributes nothing (k≈60, §3).
        Ok(rrf_fuse(&rankings, 60.0, limit))
    }
}

/// Turn free user text into a safe FTS5 MATCH expression: split on non-alphanumerics,
/// prefix-quote each term (`"term"*`), AND them together. Quoting neutralises FTS5
/// operators/syntax in the term body; the trailing `*` makes each a prefix match so
/// typing "roc" already finds "rock". Returns `None` when nothing tokenisable remains.
/// Pack an f32 vector into little-endian bytes for BLOB storage (M8). `decode_vector`
/// is the exact inverse; trailing bytes that don't fill a full f32 are ignored (never
/// panics on a malformed BLOB).
fn encode_vector(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn decode_vector(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn fts_match_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{}\"*", t.to_lowercase()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

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
        assert_eq!(version, 15, "all 15 migrations should have run");
    }

    /// The viewer's texture lookup must walk **both** hops. Stopping at the first lands on the
    /// material — not an image — which the viewer would hand to an image loader and render blank,
    /// with no error anywhere. Also checks that an unresolved edge is not followed.
    #[test]
    fn model_albedo_texture_walks_both_hops_and_ignores_unresolved_edges() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("source");
        fn mk(c: &mut Catalog, src: i64, rel: &str, code: &str) -> i64 {
            c.record_crawl(
                src,
                None,
                &[CrawlRecord {
                    media_type: code.to_string(),
                    ..rec(rel, 1, 1)
                }],
            )
            .expect("crawl");
            c.conn
                .query_row(
                    "SELECT a.id FROM asset a JOIN asset_file af ON af.id=a.primary_file_id
                      WHERE af.rel_path = ?1",
                    params![rel],
                    |r| r.get::<_, i64>(0),
                )
                .expect("id")
        }
        let model = mk(&mut c, src, "rock.fbx", "model_3d");
        let material = mk(&mut c, src, "rock.mat", "material");
        let texture = mk(&mut c, src, "rock.png", "image");
        let lonely = mk(&mut c, src, "bare.fbx", "model_3d");
        let dead_mat = mk(&mut c, src, "bare.mat", "material");

        let edge = |c: &Catalog, from: i64, to: i64, kind: &str, state: &str| {
            c.conn
                .execute(
                    "INSERT INTO asset_relationship
                        (from_asset_id, to_asset_id, kind, source, confidence, state)
                     VALUES (?1, ?2, ?3, 'test', 1.0, ?4)",
                    params![from, to, kind, state],
                )
                .expect("edge");
        };
        edge(&c, model, material, "uses_material", "resolved");
        edge(&c, material, texture, "uses_texture", "resolved");
        // `lonely` reaches a material, but that material's texture edge never resolved.
        edge(&c, lonely, dead_mat, "uses_material", "resolved");
        edge(&c, dead_mat, texture, "uses_texture", "missing");

        assert_eq!(
            c.model_albedo_texture(model).expect("query"),
            Some(texture),
            "two resolved hops must reach the image"
        );
        assert_ne!(
            c.model_albedo_texture(model).expect("query"),
            Some(material),
            "must not stop at the material — it is not an image"
        );
        assert_eq!(
            c.model_albedo_texture(lonely).expect("query"),
            None,
            "an unresolved second hop yields no texture"
        );
        assert_eq!(c.model_albedo_texture(texture).expect("query"), None);
    }

    /// The regression this queue exists for: previews used to be generated only inside the metrics
    /// drain, which visits `metrics_dirty = 1` rows. On a library indexed before previews existed
    /// every asset is already clean, so nothing was ever revisited and the grid stayed blank
    /// forever. A model with no preview must surface here regardless of `metrics_dirty`.
    #[test]
    fn preview_queue_finds_clean_models_and_walks_past_failures() {
        let mut catalog = Catalog::open_in_memory().expect("catalog open");
        catalog.seed_media_types().expect("seed");
        let source = catalog
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("source");
        let model = |rel: &str| CrawlRecord {
            media_type: "model_3d".to_string(),
            ..rec(rel, 1, 1)
        };
        catalog
            .record_crawl(
                source,
                None,
                &[model("a.fbx"), model("b.fbx"), rec("tex.png", 1, 1)],
            )
            .expect("crawl");
        // Exactly what the real catalog looked like: everything already measured.
        catalog
            .conn
            .execute("UPDATE asset SET metrics_dirty = 0", [])
            .expect("clean");

        let ids: Vec<i64> = catalog
            .assets_needing_preview(0, 10)
            .expect("queue")
            .iter()
            .map(|j| j.asset_id)
            .collect();
        assert_eq!(
            ids.len(),
            2,
            "both clean models queue; the texture does not"
        );

        // A model whose render fails writes no row, so it stays queued. The cursor is what stops
        // it from wedging the queue ahead of everything behind it.
        let after_first = catalog
            .assets_needing_preview(ids[0], 10)
            .expect("queue")
            .iter()
            .map(|j| j.asset_id)
            .collect::<Vec<_>>();
        assert_eq!(
            after_first,
            vec![ids[1]],
            "cursor advances past the failure"
        );

        catalog
            .set_vector_preview(ids[0], "<svg/>", 4, "k")
            .expect("store");
        let left: Vec<i64> = catalog
            .assets_needing_preview(0, 10)
            .expect("queue")
            .iter()
            .map(|j| j.asset_id)
            .collect();
        assert_eq!(left, vec![ids[1]], "a stored preview leaves the queue");
    }

    #[test]
    fn vector_preview_round_trips_and_tracks_staleness() {
        let mut catalog = Catalog::open_in_memory().expect("catalog open");
        catalog.seed_media_types().expect("seed");
        let source = catalog
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("source");
        catalog
            .record_crawl(source, None, &[rec("Rock.png", 1, 1)])
            .expect("crawl");
        let asset: i64 = catalog
            .conn
            .query_row("SELECT id FROM asset LIMIT 1", [], |r| r.get(0))
            .expect("asset");

        assert_eq!(catalog.vector_preview(asset).unwrap(), None);
        assert!(!catalog.has_current_preview(asset, "key-a").unwrap());

        let svg = r#"<svg viewBox="0 0 256 256"><g id="v0"/></svg>"#;
        catalog
            .set_vector_preview(asset, svg, 4, "key-a")
            .expect("store");
        assert_eq!(
            catalog.vector_preview(asset).unwrap(),
            Some((svg.to_string(), "key-a".into()))
        );
        assert!(catalog.has_current_preview(asset, "key-a").unwrap());
        // A changed file (or a raised quality) yields a different key, so the drain regenerates
        // rather than serving a preview of the old geometry.
        assert!(!catalog.has_current_preview(asset, "key-b").unwrap());
        assert_eq!(
            catalog.vector_preview_bytes().unwrap(),
            svg.len() as i64,
            "the Storage readout counts catalog preview bytes"
        );

        // Upsert replaces rather than duplicating — one preview per asset, always the current one.
        catalog
            .set_vector_preview(asset, "<svg/>", 1, "key-b")
            .expect("replace");
        assert_eq!(
            catalog.vector_preview(asset).unwrap(),
            Some(("<svg/>".to_string(), "key-b".into()))
        );
        assert_eq!(catalog.vector_preview_bytes().unwrap(), 6);

        // Previews are derived data: deleting the asset must not leave an orphan row behind.
        catalog
            .conn
            .execute("DELETE FROM asset WHERE id = ?1", params![asset])
            .expect("delete asset");
        assert_eq!(catalog.vector_preview(asset).unwrap(), None);
        assert_eq!(catalog.vector_preview_bytes().unwrap(), 0);
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
        assert_eq!(
            project_id,
            Some(1),
            "legacy tray reassigned to active project"
        );
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
            .query_row(
                "SELECT COUNT(*) FROM collection WHERE is_staging=1",
                [],
                |r| r.get(0),
            )
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
        assert!(
            dup.is_err(),
            "second staging tray for the same project rejected"
        );
    }

    #[test]
    fn register_project_upserts_activates_and_dup_guards_imports() {
        let c = Catalog::open_in_memory().expect("open");
        c.conn
            .execute_batch(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                     VALUES (1, 's1', 'Src', 'C:/lib', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at)
                     VALUES (1, 'a1', 1, 1, 'rock', 0, 0), (2, 'a2', 1, 1, 'tree', 0, 0);",
            )
            .expect("fixture");

        // Re-register the same path: an upsert, not a second row; refreshes fields, stays active.
        let p1 = c
            .register_project("C:/dev/MyGame", Some("6000.0.23f1"), Some("urp"))
            .unwrap();
        let p1b = c
            .register_project("C:/dev/MyGame", Some("6000.0.30f1"), Some("urp"))
            .unwrap();
        assert_eq!(p1, p1b, "same path upserts the same row");

        // A different project registering becomes the sole active editor.
        let p2 = c.register_project("D:/other", None, None).unwrap();
        let projects = c.list_projects().unwrap();
        assert_eq!(projects.len(), 2, "two distinct projects");
        let active: Vec<i64> = projects
            .iter()
            .filter(|p| p.is_active)
            .map(|p| p.id)
            .collect();
        assert_eq!(
            active,
            vec![p2],
            "exactly one active — the most recent register"
        );
        assert_eq!(projects[0].name, "other", "name is the last path segment");
        assert_eq!(
            projects
                .iter()
                .find(|p| p.id == p1)
                .unwrap()
                .unity_version
                .as_deref(),
            Some("6000.0.30f1"),
            "upsert refreshed the version"
        );

        let trays: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM collection WHERE is_staging=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(trays, 2, "one staging tray per registered project");

        let specs = |ids: &[i64]| {
            ids.iter()
                .map(|&asset_id| ImportSpec {
                    asset_id,
                    source_package_id: None,
                    dest_path: format!("Assets/{asset_id}.png"),
                    unity_guid: None,
                    with_deps: true,
                })
                .collect::<Vec<_>>()
        };

        let first = c.record_imports(p1, &specs(&[1, 2]), "batch-1").unwrap();
        assert!(matches!(
            first[0],
            ImportOutcome::Recorded { asset_id: 1, .. }
        ));
        assert!(matches!(
            first[1],
            ImportOutcome::Recorded { asset_id: 2, .. }
        ));

        // Advisory: both imported for p1, neither for p2.
        assert_eq!(c.already_imported(&[1, 2], p1).unwrap(), vec![1, 2]);
        assert!(c.already_imported(&[1, 2], p2).unwrap().is_empty());

        // Re-import asset 1 into p1: dup-guarded, no second row.
        let again = c.record_imports(p1, &specs(&[1]), "batch-2").unwrap();
        assert_eq!(again, vec![ImportOutcome::AlreadyImported { asset_id: 1 }]);
        let rows: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM import_record WHERE asset_id=1 AND project_id=?1",
                [p1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(rows, 1, "dup-guard kept it at one row");

        // Same asset into a *different* project is a fresh import, not a dup.
        let cross = c.record_imports(p2, &specs(&[1]), "batch-3").unwrap();
        assert!(matches!(
            cross[0],
            ImportOutcome::Recorded { asset_id: 1, .. }
        ));
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
        assert_eq!(
            ids[0], ids[1],
            "same (project, imported_at) group → same batch id"
        );
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
                (
                    "C:/a".into(),
                    "[{\"kind\":\"asset\",\"value\":\"x.dll\"}]".into(),
                ),
                (
                    "C:/b".into(),
                    "[{\"kind\":\"folder\",\"value\":\"Gizmos\"}]".into(),
                ),
                ("C:/missing".into(), "[]".into()),
            ])
            .expect("adopt");
        assert_eq!(updated, 1, "only the empty, matching source is filled");

        let b: String = c
            .conn
            .query_row(
                "SELECT blacklist FROM import_source WHERE root_path='C:/b'",
                [],
                |r| r.get(0),
            )
            .expect("b");
        assert!(b.contains("Editor"), "existing catalog blacklist untouched");
    }

    fn rec(rel: &str, size: i64, mtime: i64) -> CrawlRecord {
        CrawlRecord {
            workspace_rel: None,
            rel_path: rel.to_string(),
            name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
            media_type: "image".to_string(),
            size_bytes: size,
            mtime,
        }
    }

    /// `apply_mapping` is deterministic: rules run in priority order, the lowest-priority match wins,
    /// and the result never depends on rule *insertion* order.
    #[test]
    fn mapping_rule_priority_lower_wins_and_is_order_independent() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(src, None, &[rec("barrel.png", 1, 1)])
            .expect("crawl");
        let asset: i64 = c
            .conn
            .query_row("SELECT id FROM asset LIMIT 1", [], |r| r.get(0))
            .expect("asset");
        c.set_tags(
            asset,
            "manual",
            None,
            &[TagAssignment::exact("subject", "barrel")],
        )
        .expect("tag");

        let tax = c.create_taxonomy("Default", true).expect("tax");
        let props = c.add_category(tax, None, "Props", "Props").expect("props");
        let junk = c.add_category(tax, None, "Junk", "Junk").expect("junk");
        // Insert the HIGH-priority-number (loses) rule FIRST, so a correct engine must sort by
        // priority, not insertion/id order.
        c.add_rule(tax, junk, "exact_tag", "subject:barrel", 200)
            .expect("r1");
        c.add_rule(tax, props, "exact_tag", "subject:barrel", 50)
            .expect("r2");

        let n = c.apply_mapping(tax).expect("apply");
        assert_eq!(n, 1, "exactly one winning assignment");
        let cats = c.categories_of(asset, tax).expect("cats");
        assert_eq!(cats.len(), 1, "one category per asset");
        assert_eq!(
            cats[0].1, "Props",
            "priority 50 beats 200 regardless of insert order"
        );
        assert_eq!(cats[0].2, "rule");
    }

    /// The starter pack creates a usable library and its rules categorize real assets end to end.
    #[test]
    fn starter_taxonomy_applies_to_real_assets() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        let music = CrawlRecord {
            workspace_rel: None,
            rel_path: "Audio/theme.wav".into(),
            name: "theme.wav".into(),
            media_type: "audio".into(),
            size_bytes: 1,
            mtime: 1,
        };
        c.record_crawl(src, None, &[rec("hero.png", 1, 1), music])
            .expect("crawl");
        let hero: i64 = c
            .conn
            .query_row("SELECT a.id FROM asset a JOIN asset_file af ON af.asset_id=a.id WHERE af.rel_path GLOB '*hero*'", [], |r| r.get(0))
            .expect("hero");
        let theme: i64 = c
            .conn
            .query_row("SELECT a.id FROM asset a JOIN asset_file af ON af.asset_id=a.id WHERE af.rel_path GLOB '*theme*'", [], |r| r.get(0))
            .expect("theme");
        c.set_tags(
            hero,
            "ai",
            Some("m"),
            &[TagAssignment::exact("usage", "character")],
        )
        .expect("tag hero");
        c.set_tags(
            theme,
            "ai",
            Some("m"),
            &[TagAssignment::exact("usage", "music")],
        )
        .expect("tag theme");

        let tax = c.create_starter_taxonomy().expect("starter");
        assert_eq!(c.active_taxonomy().expect("active"), Some(tax));
        assert!(
            c.list_categories(tax).expect("cats").len() >= 10,
            "a real category tree"
        );

        let n = c.apply_mapping(tax).expect("apply");
        assert!(n >= 2, "both assets categorized");
        assert_eq!(c.categories_of(hero, tax).expect("cats")[0].1, "Characters");
        // The music clip lands in Audio/Music (alias, prio 100), not the coarse Audio (media_type 200).
        assert_eq!(
            c.categories_of(theme, tax).expect("cats")[0].1,
            "Audio/Music"
        );
    }

    /// A pinned manual assignment survives a re-apply; non-pinned rule rows regenerate. Also proves
    /// `alias`, `media_type`, and `glob_path` match, and that a pinned asset is not re-claimed by a rule.
    #[test]
    fn mapping_pinned_survives_reapply_and_match_types_work() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        let sfx = CrawlRecord {
            workspace_rel: None,
            rel_path: "Audio/step.wav".into(),
            name: "step.wav".into(),
            media_type: "audio".into(),
            size_bytes: 1,
            mtime: 1,
        };
        c.record_crawl(src, None, &[rec("props/barrel.png", 1, 1), sfx])
            .expect("crawl");
        let by_name = |c: &Catalog, like: &str| -> i64 {
            c.conn
                .query_row(
                    "SELECT a.id FROM asset a JOIN asset_file af ON af.asset_id=a.id
                     WHERE af.rel_path GLOB ?1 LIMIT 1",
                    params![like],
                    |r| r.get(0),
                )
                .expect("asset by path")
        };
        let barrel = by_name(&c, "*barrel*");
        let step = by_name(&c, "*step*");
        c.set_tags(
            barrel,
            "manual",
            None,
            &[TagAssignment::exact("subject", "barrel")],
        )
        .expect("tag");

        let tax = c.create_taxonomy("Default", true).expect("tax");
        let props = c.add_category(tax, None, "Props", "Props").expect("props");
        let sfx_cat = c.add_category(tax, None, "SFX", "SFX").expect("sfx");
        let manual_cat = c.add_category(tax, None, "Hero", "Hero").expect("hero");
        // alias (bare value) matches the namespaced tag; media_type matches the wav.
        c.add_rule(tax, props, "alias", "barrel", 100)
            .expect("r-alias");
        c.add_rule(tax, sfx_cat, "media_type", "audio", 100)
            .expect("r-media");

        // Pin the barrel to a manual category *before* applying — the rule must not override it.
        c.assign_category_manual(barrel, manual_cat, tax)
            .expect("pin");

        c.apply_mapping(tax).expect("apply 1");
        let barrel_cats = c.categories_of(barrel, tax).expect("cats");
        assert_eq!(
            barrel_cats.len(),
            1,
            "pinned asset keeps only its manual row"
        );
        assert_eq!(barrel_cats[0].1, "Hero");
        assert!(barrel_cats[0].3, "row is pinned");
        assert_eq!(
            c.categories_of(step, tax).expect("cats")[0].1,
            "SFX",
            "media_type rule matched"
        );

        // Re-map: bump version, re-apply. Pinned row survives; the SFX rule row regenerates.
        c.bump_taxonomy_version(tax).expect("bump");
        c.apply_mapping(tax).expect("apply 2");
        let barrel_cats = c.categories_of(barrel, tax).expect("cats");
        assert_eq!(barrel_cats.len(), 1);
        assert_eq!(
            barrel_cats[0].1, "Hero",
            "pinned override survived the re-apply"
        );
        assert_eq!(
            c.categories_of(step, tax).expect("cats").len(),
            1,
            "non-pinned regenerated once"
        );

        // The Library facet map reflects the assignments: barrel→Hero, step→SFX.
        let by_asset = c.all_asset_categories(tax).expect("facet map");
        assert_eq!(
            by_asset.get(&barrel).map(|v| v.as_slice()),
            Some(["Hero".to_string()].as_slice())
        );
        assert_eq!(
            by_asset.get(&step).map(|v| v.as_slice()),
            Some(["SFX".to_string()].as_slice())
        );
    }

    /// New assets queue for extraction; storing facets clears the flag and the values reach the
    /// read paths; a changed file re-queues.
    #[test]
    fn metrics_queue_clears_on_write_and_re_dirties_on_change() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(src, None, &[rec("a.png", 1, 1)])
            .expect("crawl");

        let job = c.assets_needing_metrics(10).expect("queue");
        assert_eq!(job.len(), 1, "a fresh asset needs extraction");
        assert_eq!(job[0].disk_path.as_deref(), Some("C:/lib/a.png"));

        let m = AssetMetrics {
            width_px: Some(1024),
            height_px: Some(512),
            ..Default::default()
        };
        c.set_metrics(job[0].asset_id, m).expect("set");
        assert!(c.assets_needing_metrics(10).expect("queue").is_empty());
        assert_eq!(c.entries_for_source(src).expect("hydrate")[0].metrics, m);
        assert_eq!(c.assets_for_source(src).expect("identity")[0].metrics, m);

        // Extraction that finds nothing still clears the flag (forward progress).
        c.record_crawl(src, None, &[rec("a.png", 9, 9)])
            .expect("rescan");
        assert_eq!(c.assets_needing_metrics(10).expect("queue").len(), 1);
        c.set_metrics(job[0].asset_id, AssetMetrics::default())
            .expect("set empty");
        assert!(c.assets_needing_metrics(10).expect("queue").is_empty());
    }

    #[test]
    fn asset_summaries_keep_request_order_and_drop_misses() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(src, None, &[rec("a.png", 10, 1), rec("b.png", 20, 1)])
            .expect("crawl");
        let ids = c.recent_asset_ids(10).expect("recent");
        assert_eq!(ids.len(), 2);

        // Reversed order, plus an id that doesn't exist.
        let want = vec![ids[1], 9999, ids[0]];
        let rows = c.asset_summaries(&want).expect("summaries");
        assert_eq!(rows.len(), 2, "the unknown id is a miss, not an error");
        assert_eq!(rows[0].id, ids[1], "request order is preserved");
        assert_eq!(rows[1].id, ids[0]);
        assert_eq!(rows[0].media_type, "image");
        assert_eq!(rows[0].format.as_deref(), Some("png"));
        assert_eq!(
            rows[0].disk_path.as_deref(),
            Some(format!("C:/lib/{}", rows[0].name).as_str()),
            "the summary carries the primary file's absolute path"
        );
        assert!(rows[0].uuid.len() >= 32, "uuid travels to the agent planes");

        assert!(c.media_type_id("audio").expect("lookup").is_some());
        assert_eq!(c.media_type_id("nope").expect("lookup"), None);
    }

    #[test]
    fn embedding_round_trips_and_prefilters() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 2, 1)])
            .expect("crawl");
        let ids: Vec<i64> = c
            .conn
            .prepare("SELECT id FROM asset ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        let img: i64 = c
            .conn
            .query_row("SELECT id FROM media_type WHERE code='image'", [], |r| {
                r.get(0)
            })
            .expect("image media type");

        c.upsert_embedding(
            ids[0],
            "siglip2",
            "image",
            Some(img),
            Some(src),
            &[1.0, 0.0, 0.0],
        )
        .expect("embed 0");
        c.upsert_embedding(
            ids[1],
            "siglip2",
            "image",
            Some(img),
            Some(src),
            &[0.0, 1.0, 0.0],
        )
        .expect("embed 1");

        // Round-trip: vectors decode exactly, and embedding_dirty was cleared.
        let cands = c.embeddings_for_knn("siglip2", None).expect("knn load");
        assert_eq!(cands.len(), 2);
        let v0 = &cands.iter().find(|(id, _)| *id == ids[0]).unwrap().1;
        assert_eq!(v0, &vec![1.0, 0.0, 0.0]);
        let dirty: i64 = c
            .conn
            .query_row(
                "SELECT embedding_dirty FROM asset WHERE id=?1",
                [ids[0]],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dirty, 0, "upsert clears embedding_dirty");

        // Upsert replaces (same asset+model), doesn't duplicate.
        c.upsert_embedding(
            ids[0],
            "siglip2",
            "image",
            Some(img),
            Some(src),
            &[0.5, 0.5, 0.0],
        )
        .expect("re-embed");
        assert_eq!(c.embeddings_for_knn("siglip2", None).unwrap().len(), 2);

        // Media-type pre-filter: a bogus type yields nothing.
        assert!(c
            .embeddings_for_knn("siglip2", Some(9999))
            .unwrap()
            .is_empty());
        // Soft-deleted assets drop out.
        c.conn
            .execute("UPDATE asset SET deleted_at=1 WHERE id=?1", [ids[0]])
            .unwrap();
        assert_eq!(c.embeddings_for_knn("siglip2", None).unwrap().len(), 1);
    }

    #[test]
    fn hybrid_search_fuses_bm25_and_knn() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(
            src,
            None,
            &[
                rec("rock_wall.png", 1, 1), // BM25-matches "rock"; vector identical to query
                rec("rock_floor.png", 2, 1), // BM25-matches "rock"; vector orthogonal
                rec("tree_oak.png", 3, 1),  // no BM25 match; vector close to query
            ],
        )
        .expect("crawl");
        // Map name → id so assertions don't depend on insertion order.
        let id_of = |name: &str| -> i64 {
            c.conn
                .query_row("SELECT id FROM asset WHERE name=?1", [name], |r| r.get(0))
                .unwrap()
        };
        let (wall, floor, oak) = (
            id_of("rock_wall.png"),
            id_of("rock_floor.png"),
            id_of("tree_oak.png"),
        );
        let img: i64 = c
            .conn
            .query_row("SELECT id FROM media_type WHERE code='image'", [], |r| {
                r.get(0)
            })
            .unwrap();
        c.upsert_embedding(wall, "m", "image", Some(img), Some(src), &[1.0, 0.0, 0.0])
            .unwrap();
        c.upsert_embedding(floor, "m", "image", Some(img), Some(src), &[0.0, 1.0, 0.0])
            .unwrap();
        c.upsert_embedding(oak, "m", "image", Some(img), Some(src), &[0.9, 0.1, 0.0])
            .unwrap();

        // Query "rock" (BM25 → wall, floor) + vector [1,0,0] (KNN → wall, oak, floor).
        let fused = c
            .hybrid_search("rock", &[("m", &[1.0, 0.0, 0.0][..])], None, 10)
            .expect("hybrid");
        assert_eq!(fused[0], wall, "wall wins — high in both retrievers");
        assert!(
            fused.contains(&oak),
            "oak enters via the vector retriever alone"
        );
        assert!(fused.contains(&floor));

        // query_vec = None → pure structural (BM25 only): oak (no name match) is absent.
        let structural = c.hybrid_search("rock", &[], None, 10).expect("structural");
        assert!(!structural.contains(&oak), "no vector → tree not found");
        assert!(structural.contains(&wall) && structural.contains(&floor));
        assert!(c.hybrid_search("rock", &[], None, 0).unwrap().is_empty());
    }

    #[test]
    fn fts_match_query_is_injection_safe() {
        // Every term is prefix-quoted; FTS5 operators in the body are neutralised.
        assert_eq!(fts_match_query("rock"), Some("\"rock\"*".into()));
        assert_eq!(
            fts_match_query("low poly"),
            Some("\"low\"* \"poly\"*".into())
        );
        // Quotes / operators / stars never reach MATCH as syntax.
        assert_eq!(
            fts_match_query("a\" OR b*"),
            Some("\"a\"* \"or\"* \"b\"*".into())
        );
        assert_eq!(fts_match_query(""), None);
        assert_eq!(fts_match_query("  -–—  "), None, "punctuation only → None");
    }

    #[test]
    fn search_assets_ranks_and_prefix_matches() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(
            src,
            None,
            &[
                rec("rock_wall.png", 1, 1),
                rec("rocky_path.png", 2, 1),
                rec("tree_oak.png", 3, 1),
            ],
        )
        .expect("crawl");

        // Prefix match: "rock" finds rock_wall AND rocky_path, not the tree.
        let hits = c.search_assets("rock", 50).expect("search").expect("some");
        assert_eq!(hits.len(), 2, "rock* matches both rock names");
        let tree = c.search_assets("tree", 50).expect("s").expect("some");
        assert_eq!(tree.len(), 1);

        // Empty / punctuation-only → None (caller shows everything).
        assert!(c.search_assets("", 50).expect("s").is_none());
        assert!(c.search_assets("***", 50).expect("s").is_none());

        // Soft-deleted rows drop out of results.
        c.conn
            .execute(
                "UPDATE asset SET deleted_at = 1 WHERE name = 'rock_wall.png'",
                [],
            )
            .expect("trash");
        let hits = c.search_assets("rock", 50).expect("s").expect("some");
        assert_eq!(hits.len(), 1, "trashed asset excluded");
    }

    /// Perf probe (not an assertion of speed — a measurement): how does the brute-force KNN
    /// path scale? Splits the two costs so we optimise the real one instead of guessing —
    /// reloading+decoding every BLOB per query vs. the cosine scan + full sort.
    /// `cargo test -p lodestash-core perf_probe -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn perf_probe_knn_scaling() {
        use std::time::Instant;
        const DIM: usize = 768;

        for n in [10_000usize, 50_000] {
            let dir = tempfile::tempdir().expect("tmp");
            let mut c = Catalog::open(&dir.path().join("perf.db")).expect("open");
            c.seed_media_types().expect("seed");
            let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");

            let recs: Vec<CrawlRecord> = (0..n)
                .map(|i| {
                    let mut r = rec(&format!("a{i}.png"), 1, 1);
                    r.media_type = "image".into();
                    r
                })
                .collect();
            c.record_crawl(sid, None, &recs).expect("crawl");

            // Deterministic pseudo-random unit-ish vectors.
            let mk = |seed: usize| -> Vec<f32> {
                let mut h = seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
                (0..DIM)
                    .map(|_| {
                        h ^= h << 13;
                        h ^= h >> 7;
                        h ^= h << 17;
                        (h as u32 as f32 / u32::MAX as f32) - 0.5
                    })
                    .collect()
            };
            let t = Instant::now();
            for i in 0..n {
                c.upsert_embedding(i as i64 + 1, "m", "image", Some(1), Some(sid), &mk(i))
                    .expect("upsert");
            }
            let seed_time = t.elapsed();

            let t = Instant::now();
            let cands = c.embeddings_for_knn("m", None).expect("load");
            let load = t.elapsed();
            assert_eq!(cands.len(), n);

            let q = mk(12345);
            let t = Instant::now();
            let idx = crate::vector::BruteForceIndex::new(cands);
            let hits =
                <crate::vector::BruteForceIndex as crate::vector::VectorIndex>::knn(&idx, &q, 20);
            let scan = t.elapsed();
            assert_eq!(hits.len(), 20);

            println!(
                "n={n:>6} dim={DIM}  seed={seed_time:>10.2?}  LOAD={load:>10.2?}  SCAN+SORT={scan:>10.2?}  total_query={:?}",
                load + scan
            );
        }
    }

    #[test]
    fn visual_embed_queue_covers_images_models_on_disk_and_extracted() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c
            .upsert_import_source("src", "G:/src", "folder")
            .expect("sid");
        let pid = c
            .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
            .expect("pid");

        // On-disk image, on-disk 3D model, extracted-package sprite, and audio (NOT visual).
        let mut on_disk = rec("tex/brick.png", 5, 1);
        on_disk.media_type = "image".into();
        let mut model = rec("models/sword.fbx", 8, 1);
        model.media_type = "model_3d".into();
        let mut extracted = rec("Pack/Sprites/hero.png", 6, 1);
        extracted.media_type = "sprite".into();
        extracted.workspace_rel = Some("hash7/hero.png".into());
        let mut audio = rec("sfx/boom.wav", 7, 1);
        audio.media_type = "audio".into();
        c.record_crawl(sid, None, &[on_disk, model, audio])
            .expect("crawl1");
        c.record_crawl(sid, Some(pid), &[extracted])
            .expect("crawl2");

        let jobs = c.visual_assets_needing_embedding(100).expect("queue");
        assert_eq!(jobs.len(), 3, "2 images + 1 model queued; audio excluded");

        let img = jobs
            .iter()
            .find(|j| j.media_code == "image")
            .expect("image job");
        assert_eq!(img.disk_path.as_deref(), Some("G:/src/tex/brick.png"));
        assert!(img.workspace_rel.is_none());

        let m = jobs
            .iter()
            .find(|j| j.media_code == "model_3d")
            .expect("model job");
        assert_eq!(m.disk_path.as_deref(), Some("G:/src/models/sword.fbx"));

        let ext = jobs
            .iter()
            .find(|j| j.workspace_rel.is_some())
            .expect("extracted job");
        assert_eq!(ext.media_code, "sprite");
        assert_eq!(ext.workspace_rel.as_deref(), Some("hash7/hero.png"));
        assert!(
            ext.disk_path.is_none(),
            "extracted files resolve via the workspace, not root"
        );
    }

    #[test]
    fn m6_extracted_rehydration_and_package_entries_round_trip() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c
            .upsert_import_source("src", "G:/src", "folder")
            .expect("sid");
        let pid = c
            .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
            .expect("pid");

        // Extracted asset: recorded with a workspace_rel, hidden from the normal
        // hydrate, visible to the extracted hydrate with its package name.
        let mut extracted = rec("Pack/Models/Rock.fbx", 10, 1);
        extracted.media_type = "model_3d".into();
        extracted.workspace_rel = Some("abc123/Rock.fbx".into());
        c.record_crawl(sid, Some(pid), &[extracted])
            .expect("record");
        c.record_crawl(sid, None, &[rec("plain.png", 5, 1)])
            .expect("record2");

        let normal = c.entries_for_source(sid).expect("normal");
        assert_eq!(
            normal.len(),
            1,
            "extracted rows stay out of the folder hydrate"
        );
        let ex = c.extracted_entries_for_source(sid).expect("extracted");
        assert_eq!(ex.len(), 1);
        assert_eq!(ex[0].package_name, "Pack");
        assert_eq!(ex[0].workspace_rel, "abc123/Rock.fbx");
        assert_eq!(ex[0].asset.rel_path, "Pack/Models/Rock.fbx");
        assert!(!ex[0].pinned, "packages start unpinned (M7)");

        // M7 pin round-trip: listing + the extracted hydrate both see the flag.
        c.set_package_pinned(pid, true).expect("pin");
        let listing = c.list_packages().expect("list");
        assert_eq!(listing.len(), 1);
        assert!(listing[0].pinned);
        assert_eq!(listing[0].name, "Pack");
        assert!(
            c.extracted_entries_for_source(sid).expect("extracted")[0].pinned,
            "pin visible on rehydrated entries"
        );
        c.set_package_pinned(pid, false).expect("unpin");
        assert!(!c.list_packages().expect("list")[0].pinned);

        // package_entry offsets round-trip + upsert semantics.
        let entries = vec![
            PackageEntryRecord {
                guid: "g1".into(),
                pathname: Some("Assets/Rock.fbx".into()),
                tar_offset: Some(1536),
                size_bytes: Some(4096),
            },
            PackageEntryRecord {
                guid: "g2".into(),
                pathname: None,
                tar_offset: Some(8192),
                size_bytes: Some(100),
            },
        ];
        c.record_package_entries(pid, &entries).expect("entries");
        let mut updated = entries.clone();
        updated[1].tar_offset = Some(9000);
        c.record_package_entries(pid, &updated).expect("upsert");
        // Path-ordered read: NULL pathnames (the .meta-less locators) sort first in SQLite.
        let got = c.list_package_entries(pid).expect("entries read");
        assert_eq!(
            got,
            vec![updated[1].clone(), updated[0].clone()],
            "guid-keyed upsert, path-ordered read"
        );
    }

    #[test]
    fn mint_in_pack_assets_links_locators_and_skips_unclassified() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c
            .upsert_import_source("src", "G:/src", "asset_store_library")
            .expect("sid");
        let pid = c
            .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
            .expect("pid");

        // Three manifest locators; the caller classifies only two (the third is a .cs script
        // with no media_type — it must never become a grid asset).
        c.record_package_entries(
            pid,
            &[
                PackageEntryRecord {
                    guid: "g_rock".into(),
                    pathname: Some("Assets/Models/Rock.fbx".into()),
                    tar_offset: Some(1536),
                    size_bytes: Some(4096),
                },
                PackageEntryRecord {
                    guid: "g_tex".into(),
                    pathname: Some("Assets/Textures/Rock_Albedo.png".into()),
                    tar_offset: Some(8192),
                    size_bytes: Some(2048),
                },
                PackageEntryRecord {
                    guid: "g_script".into(),
                    pathname: Some("Assets/Scripts/Spin.cs".into()),
                    tar_offset: Some(16384),
                    size_bytes: Some(512),
                },
            ],
        )
        .expect("entries");

        let classified = HashMap::from([
            (
                "g_rock".to_string(),
                InPackClass {
                    name: "Rock".into(),
                    media_type: "model_3d".into(),
                },
            ),
            (
                "g_tex".to_string(),
                InPackClass {
                    name: "Rock_Albedo".into(),
                    media_type: "texture".into(),
                },
            ),
        ]);

        let stats = c.mint_in_pack_assets(pid, &classified).expect("mint");
        assert_eq!(stats.inserted, 2, "two classified entries minted");
        // The owning source_id is derived from the package row, not passed in.
        let minted_source: i64 = c
            .conn
            .query_row(
                "SELECT DISTINCT source_id FROM asset WHERE package_id = ?1",
                params![pid],
                |r| r.get(0),
            )
            .expect("minted source");
        assert_eq!(
            minted_source, sid,
            "minted assets belong to the pack's source"
        );

        // Every minted file links its locator and has NO workspace_rel (bytes stay in-pack).
        let linked: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset_file af JOIN package_entry pe
                   ON pe.id = af.package_entry_id
                 WHERE pe.package_id = ?1 AND af.workspace_rel IS NULL",
                params![pid],
                |r| r.get(0),
            )
            .expect("linked count");
        assert_eq!(
            linked, 2,
            "both in-pack files carry a locator, none extracted"
        );

        // The unclassified script produced no asset.
        let script_assets: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset_file WHERE package_entry_id =
                   (SELECT id FROM package_entry WHERE guid = 'g_script')",
                [],
                |r| r.get(0),
            )
            .expect("script count");
        assert_eq!(
            script_assets, 0,
            "unclassified entry never becomes an asset"
        );

        // A minted asset is queued for every drain (default dirty flags).
        let dirty: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset WHERE package_id = ?1
                   AND embedding_dirty = 1 AND tags_dirty = 1 AND deps_dirty = 1",
                params![pid],
                |r| r.get(0),
            )
            .expect("dirty count");
        assert_eq!(dirty, 2, "minted in-pack assets await identify");

        // Idempotent: re-minting the same classification inserts nothing new.
        let again = c.mint_in_pack_assets(pid, &classified).expect("re-mint");
        assert_eq!(again.inserted, 0, "re-import mints nothing twice");
        assert_eq!(again.unchanged, 2);

        // Guard: once the pack has any extracted (workspace_rel) row, minting is skipped wholesale
        // so an extracted pack can't grow duplicate in-pack rows. Add a new classified locator and
        // an extracted row, then confirm the new locator is NOT minted.
        c.record_package_entries(
            pid,
            &[PackageEntryRecord {
                guid: "g_new".into(),
                pathname: Some("Assets/Audio/Hit.wav".into()),
                tar_offset: Some(32768),
                size_bytes: Some(1024),
            }],
        )
        .expect("new entry");
        c.conn
            .execute(
                "INSERT INTO asset_file (asset_id, rel_path, workspace_rel)
                 VALUES ((SELECT id FROM asset WHERE package_id = ?1 LIMIT 1),
                         'Pack/extracted.fbx', 'hash/extracted.fbx')",
                params![pid],
            )
            .expect("extracted row");
        let with_extracted = HashMap::from([(
            "g_new".to_string(),
            InPackClass {
                name: "Hit".into(),
                media_type: "audio".into(),
            },
        )]);
        let skipped = c
            .mint_in_pack_assets(pid, &with_extracted)
            .expect("guarded mint");
        assert_eq!(
            skipped.inserted, 0,
            "an extracted pack does not grow in-pack duplicates"
        );
    }

    #[test]
    fn flag_identify_scopes_untagged_and_selected() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");
        let rec = |p: &str| CrawlRecord {
            rel_path: p.into(),
            name: p.into(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 0,
            workspace_rel: None,
        };
        c.record_crawl(sid, None, &[rec("a.png"), rec("b.png"), rec("c.png")])
            .unwrap();
        // Simulate an already-identified library, then tag only 'a'.
        c.conn
            .execute("UPDATE asset SET embedding_dirty = 0, tags_dirty = 0", [])
            .unwrap();
        let a_id: i64 = c
            .conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE af.rel_path = 'a.png'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        c.set_tags(
            a_id,
            "ai",
            None,
            &[TagAssignment {
                namespace: "style".into(),
                value: "foo".into(),
                confidence: None,
            }],
        )
        .unwrap();

        // Unidentified scope: b + c (no tags) re-flagged; a (tagged) left clean.
        assert_eq!(c.flag_identify_untagged().unwrap(), 2);
        let a_dirty: i64 = c
            .conn
            .query_row(
                "SELECT tags_dirty FROM asset WHERE id = ?1",
                params![a_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            a_dirty, 0,
            "a tagged asset is not re-flagged by Unidentified"
        );

        // Selected scope: exactly the given ids.
        c.conn
            .execute("UPDATE asset SET embedding_dirty = 0, tags_dirty = 0", [])
            .unwrap();
        assert_eq!(c.flag_identify_assets(&[a_id]).unwrap(), 1);
        let total_dirty: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM asset WHERE tags_dirty = 1", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(total_dirty, 1, "only the selected asset is flagged");
    }

    #[test]
    fn confirm_flow_sets_and_reverts_status_and_reject_stops_matching() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");
        c.record_crawl(
            sid,
            None,
            &[CrawlRecord {
                rel_path: "a.png".into(),
                name: "a.png".into(),
                media_type: "image".into(),
                size_bytes: 1,
                mtime: 0,
                workspace_rel: None,
            }],
        )
        .unwrap();
        let id: i64 = c
            .conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE af.rel_path = 'a.png'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        // An identified asset: embedding present + clean, one tag, facets extracted.
        c.conn
            .execute(
                "UPDATE asset SET embedding_dirty = 0, metrics_dirty = 0, status = 'identified'
                 WHERE id = ?1",
                params![id],
            )
            .unwrap();
        c.conn
            .execute(
                "INSERT INTO asset_embedding
                    (asset_id, model_id, modality, dim, embedding, created_at)
                 VALUES (?1, 'm', 'image', 1, X'00000000', 0)",
                params![id],
            )
            .unwrap();
        c.set_tags(
            id,
            "ai",
            None,
            &[
                TagAssignment {
                    namespace: "subject".into(),
                    value: "dragon".into(),
                    confidence: Some(0.8),
                },
                TagAssignment {
                    namespace: "material".into(),
                    value: "scales".into(),
                    confidence: Some(0.6),
                },
            ],
        )
        .unwrap();

        let status = |c: &Catalog| -> String {
            c.conn
                .query_row("SELECT status FROM asset WHERE id = ?1", params![id], |r| {
                    r.get(0)
                })
                .unwrap()
        };

        // Confirm → confirmed; un-confirm recomputes the earned status (embedding + tag ⇒ identified).
        c.set_confirmed(id, true).unwrap();
        assert_eq!(status(&c), "confirmed");
        c.set_confirmed(id, false).unwrap();
        assert_eq!(status(&c), "identified");

        // Reject a tag: it is gone from the asset and no longer matches FTS.
        assert_eq!(c.remove_asset_tag(id, "subject", "dragon").unwrap(), 1);
        let left = c.tags_for_asset(id).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].value, "scales");
        let hits: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset_fts WHERE asset_fts MATCH 'dragon'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(hits, 0, "rejected term stops matching FTS");
    }

    #[test]
    fn trait_promote_teach_and_unpromote_keeps_assignments() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");
        c.record_crawl(
            sid,
            None,
            &[CrawlRecord {
                rel_path: "a.png".into(),
                name: "a.png".into(),
                media_type: "image".into(),
                size_bytes: 1,
                mtime: 0,
                workspace_rel: None,
            }],
        )
        .unwrap();
        let id: i64 = c
            .conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE af.rel_path = 'a.png'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        c.set_tags(
            id,
            "ai",
            None,
            &[TagAssignment {
                namespace: "material".into(),
                value: "metal".into(),
                confidence: Some(0.7),
            }],
        )
        .unwrap();

        // Promote the in-use tag to a teaching trait scoped to models.
        let tid = c
            .upsert_trait(
                "material",
                "metal",
                Some("model_3d"),
                Some("shiny hard surface"),
                true,
            )
            .unwrap();
        let vocab = c.list_vocabulary().unwrap();
        let row = vocab.iter().find(|v| v.id == tid).unwrap();
        assert!(row.is_trait && row.teach);
        assert_eq!(
            row.usage_count, 1,
            "promotion keeps the existing assignment"
        );
        assert_eq!(row.ai_count, 1);

        // Teaching vocabulary now carries it, scope + usage note intact.
        let teach = c.teaching_traits().unwrap();
        assert_eq!(
            teach,
            vec![(
                "material".to_string(),
                "metal".to_string(),
                Some("model_3d".to_string()),
                Some("shiny hard surface".to_string())
            )]
        );
        // Pause it: excluded from teaching, still a trait.
        c.set_trait_teach(tid, false).unwrap();
        assert!(c.teaching_traits().unwrap().is_empty());

        // Un-promote: trait status gone, but the tag and its assignment survive.
        c.unpromote_trait(tid).unwrap();
        let vocab = c.list_vocabulary().unwrap();
        let row = vocab.iter().find(|v| v.id == tid).unwrap();
        assert!(!row.is_trait);
        assert_eq!(row.usage_count, 1, "un-promote never drops assignments");
    }

    /// An `alias` rule pattern is user text, so LIKE metacharacters must be literals. Note `_`
    /// can't reach `norm_name` — `normalize_tag` turns it into a space — but `%` survives, so that
    /// is the case that actually bites.
    #[test]
    fn alias_rule_treats_like_metacharacters_literally() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 1, 1)])
            .unwrap();
        let ids: Vec<i64> = c
            .conn
            .prepare("SELECT id FROM asset ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        // `50zzz off` must NOT match the rule `50% off` — unescaped, `%` matches any run.
        c.set_tags(
            ids[0],
            "manual",
            None,
            &[TagAssignment::exact("style", "50% off")],
        )
        .unwrap();
        c.set_tags(
            ids[1],
            "manual",
            None,
            &[TagAssignment::exact("style", "50zzz off")],
        )
        .unwrap();

        let tax = c.create_taxonomy("Default", true).expect("tax");
        let cat = c
            .add_category(tax, None, "Low Poly", "Low Poly")
            .expect("cat");
        c.add_rule(tax, cat, "alias", "50% off", 10).expect("rule");
        c.apply_mapping(tax).expect("apply");

        assert_eq!(
            c.categories_of(ids[0], tax).expect("cats").len(),
            1,
            "the literal alias still matches"
        );
        assert!(
            c.categories_of(ids[1], tax).expect("cats").is_empty(),
            "`%` must be a literal percent sign, not a wildcard"
        );
    }

    #[test]
    fn unmaterialize_returns_only_unshared_files_and_reverts_rows() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c
            .upsert_import_source("s", "G:/s", "asset_store_library")
            .expect("sid");
        let p1 = c
            .upsert_package(sid, "G:/s/P1.unitypackage", "P1", None, None)
            .expect("p1");
        let p2 = c
            .upsert_package(sid, "G:/s/P2.unitypackage", "P2", None, None)
            .expect("p2");
        let entry = |guid: &str, path: &str| PackageEntryRecord {
            guid: guid.into(),
            pathname: Some(path.into()),
            tar_offset: Some(0),
            size_bytes: Some(1),
        };
        let tex = |n: &str| InPackClass {
            name: n.into(),
            media_type: "texture".into(),
        };
        // P1 has A + B; P2 has A too (same content → same workspace_rel).
        c.record_package_entries(
            p1,
            &[entry("g_a", "Assets/A.png"), entry("g_b", "Assets/B.png")],
        )
        .unwrap();
        c.record_package_entries(p2, &[entry("g_c", "Assets/A.png")])
            .unwrap();
        c.mint_in_pack_assets(
            p1,
            &HashMap::from([("g_a".into(), tex("A")), ("g_b".into(), tex("B"))]),
        )
        .unwrap();
        c.mint_in_pack_assets(p2, &HashMap::from([("g_c".into(), tex("A"))]))
            .unwrap();
        // Materialize: A.png in both packs points at the *same* content-addressed file.
        c.materialize_in_pack_assets(
            p1,
            &[
                ("Assets/A.png".into(), "hashA/A.png".into()),
                ("Assets/B.png".into(), "hashB/B.png".into()),
            ],
        )
        .unwrap();
        c.materialize_in_pack_assets(p2, &[("Assets/A.png".into(), "hashA/A.png".into())])
            .unwrap();

        let freed = c.unmaterialize_package(p1).expect("unmaterialize");
        assert_eq!(
            freed,
            vec!["hashB/B.png".to_string()],
            "only B is deletable — A is shared with P2 and must survive"
        );

        // Two assets *inside one pack* that deduped to the same file must still be reclaimed. The
        // sharer has to be outside the pack being unmaterialized; counting a sibling row meant each
        // saw the other as a live user, so neither was returned — and the update cleared both
        // `workspace_rel`s in the next statement, leaking those bytes permanently.
        let p3 = c
            .upsert_package(sid, "G:/s/P3.unitypackage", "P3", None, None)
            .expect("p3");
        c.record_package_entries(
            p3,
            &[entry("g_d", "Assets/D.png"), entry("g_e", "Assets/E.png")],
        )
        .unwrap();
        c.mint_in_pack_assets(
            p3,
            &HashMap::from([("g_d".into(), tex("D")), ("g_e".into(), tex("E"))]),
        )
        .unwrap();
        c.materialize_in_pack_assets(
            p3,
            &[
                ("Assets/D.png".into(), "hashD/D.png".into()),
                ("Assets/E.png".into(), "hashD/D.png".into()),
            ],
        )
        .unwrap();
        assert_eq!(
            c.unmaterialize_package(p3).expect("unmaterialize p3"),
            vec!["hashD/D.png".to_string()],
            "an intra-pack duplicate is still reclaimed, exactly once"
        );
        // P1's rows reverted to in-pack (workspace_rel NULL, locator kept); P2 untouched.
        let p1_extracted: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset a JOIN asset_file af ON af.asset_id=a.id
                 WHERE a.package_id=?1 AND af.workspace_rel IS NOT NULL",
                params![p1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(p1_extracted, 0, "P1 fully un-materialized");
        let p2_extracted: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset a JOIN asset_file af ON af.asset_id=a.id
                 WHERE a.package_id=?1 AND af.workspace_rel IS NOT NULL",
                params![p2],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(p2_extracted, 1, "P2's materialized row is untouched");
    }

    #[test]
    fn materialize_fills_workspace_rel_on_minted_rows_no_shadow() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c
            .upsert_import_source("src", "G:/src", "asset_store_library")
            .expect("sid");
        let pid = c
            .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
            .expect("pid");
        c.set_package_pinned(pid, true).expect("pin");

        c.record_package_entries(
            pid,
            &[
                PackageEntryRecord {
                    guid: "g_rock".into(),
                    pathname: Some("Assets/Models/Rock.fbx".into()),
                    tar_offset: Some(1536),
                    size_bytes: Some(4096),
                },
                PackageEntryRecord {
                    guid: "g_tex".into(),
                    pathname: Some("Assets/Textures/Rock_Albedo.png".into()),
                    tar_offset: Some(8192),
                    size_bytes: Some(2048),
                },
            ],
        )
        .expect("entries");
        let classified = HashMap::from([
            (
                "g_rock".to_string(),
                InPackClass {
                    name: "Rock".into(),
                    media_type: "model_3d".into(),
                },
            ),
            (
                "g_tex".to_string(),
                InPackClass {
                    name: "Rock_Albedo".into(),
                    media_type: "texture".into(),
                },
            ),
        ]);
        c.mint_in_pack_assets(pid, &classified).expect("mint");

        // Both start in-pack (workspace_rel NULL), neither extracted.
        assert_eq!(c.in_pack_entries_for_source(sid).unwrap().len(), 2);
        assert_eq!(c.extracted_entries_for_source(sid).unwrap().len(), 0);

        // Extract only Rock: materialize fills its workspace_rel in place. The pathname with no
        // minted row (a stray extracted file) is reported unmatched so the caller can record_crawl it.
        let matched = c
            .materialize_in_pack_assets(
                pid,
                &[
                    ("Assets/Models/Rock.fbx".into(), "h0/Rock.fbx".into()),
                    ("Assets/Nope.txt".into(), "h0/Nope.txt".into()),
                ],
            )
            .expect("materialize");
        assert_eq!(
            matched,
            HashSet::from(["Assets/Models/Rock.fbx".to_string()]),
            "only the minted pathname is materialized"
        );

        // Rock flipped in-pack → extracted (no parallel row); the texture stays in-pack.
        let in_pack = c.in_pack_entries_for_source(sid).unwrap();
        let extracted = c.extracted_entries_for_source(sid).unwrap();
        assert_eq!(in_pack.len(), 1, "texture still in-pack");
        assert_eq!(in_pack[0].asset.rel_path, "Assets/Textures/Rock_Albedo.png");
        assert_eq!(extracted.len(), 1, "rock now resolves on disk");
        assert_eq!(extracted[0].workspace_rel, "h0/Rock.fbx");
        assert_eq!(extracted[0].asset.rel_path, "Assets/Models/Rock.fbx");

        // The materialized row keeps its package_entry link (still an in-pack asset, just on disk).
        let still_linked: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset_file
                 WHERE package_entry_id IS NOT NULL AND workspace_rel = 'h0/Rock.fbx'",
                [],
                |r| r.get(0),
            )
            .expect("link count");
        assert_eq!(still_linked, 1, "materialize does not sever the locator");

        // Total rows unchanged: 2 minted, 0 added — the whole point of step 8.
        let total: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset WHERE package_id = ?1",
                params![pid],
                |r| r.get(0),
            )
            .expect("total");
        assert_eq!(total, 2, "no parallel set — one row per file");
    }

    #[test]
    fn in_pack_entries_surface_with_pack_and_locator() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let sid = c
            .upsert_import_source("src", "G:/src", "asset_store_library")
            .expect("sid");
        let pid = c
            .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
            .expect("pid");
        c.record_package_entries(
            pid,
            &[PackageEntryRecord {
                guid: "g_rock".into(),
                pathname: Some("Assets/Models/Rock.fbx".into()),
                tar_offset: Some(1536),
                size_bytes: Some(4096),
            }],
        )
        .expect("entries");
        c.set_package_pinned(pid, true).expect("pin");
        c.mint_in_pack_assets(
            pid,
            &HashMap::from([(
                "g_rock".to_string(),
                InPackClass {
                    name: "Rock".into(),
                    media_type: "model_3d".into(),
                },
            )]),
        )
        .expect("mint");

        let rows = c.in_pack_entries_for_source(sid).expect("in-pack hydrate");
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.package_name, "Pack");
        assert!(
            r.pinned,
            "the pack's pin rides through so the grid can gate on it"
        );
        assert_eq!(r.pack_path, "G:/src/Pack.unitypackage");
        assert_eq!(
            (r.tar_offset, r.size),
            (1536, 4096),
            "locator for on-demand pull"
        );
        assert_eq!(r.asset.rel_path, "Assets/Models/Rock.fbx");
        assert_eq!(r.asset.media_type, "model_3d");

        // An in-pack asset is NOT returned by the plain folder hydrate (package_id excludes it),
        // so it can't become a broken on-disk card.
        assert!(
            c.entries_for_source(sid)
                .expect("folder hydrate")
                .is_empty(),
            "in-pack assets stay out of the folder hydrate"
        );
    }

    #[test]
    fn upsert_import_source_is_idempotent_by_root_path() {
        let c = Catalog::open_in_memory().expect("open");
        let a = c
            .upsert_import_source("Synty", "C:/lib", "folder")
            .expect("insert");
        let b = c
            .upsert_import_source("Renamed", "C:/lib", "folder")
            .expect("reuse");
        assert_eq!(a, b, "same root_path resolves to the same source");
        let name: String = c
            .conn
            .query_row("SELECT name FROM import_source WHERE id=?1", [a], |r| {
                r.get(0)
            })
            .expect("name");
        assert_eq!(name, "Synty", "name only set on insert");
    }

    #[test]
    fn record_crawl_inserts_then_skips_then_updates() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");

        // First crawl: everything new.
        let stats = c
            .record_crawl(
                src,
                None,
                &[rec("rocks/a.png", 10, 100), rec("rocks/b.png", 20, 100)],
            )
            .expect("crawl 1");
        assert_eq!(
            stats,
            CrawlStats {
                inserted: 2,
                updated: 0,
                unchanged: 0
            }
        );

        // Second crawl: one untouched, one changed (new mtime).
        let stats = c
            .record_crawl(
                src,
                None,
                &[rec("rocks/a.png", 10, 100), rec("rocks/b.png", 22, 200)],
            )
            .expect("crawl 2");
        assert_eq!(
            stats,
            CrawlStats {
                inserted: 0,
                updated: 1,
                unchanged: 1
            }
        );

        // The changed file's asset was marked dirty; sizes stored per file row.
        let (size, dirty): (i64, i64) = c
            .conn
            .query_row(
                "SELECT af.size_bytes, a.embedding_dirty
                 FROM asset_file af JOIN asset a ON a.id = af.asset_id
                 WHERE af.rel_path = 'rocks/b.png'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row");
        assert_eq!(size, 22);
        assert_eq!(dirty, 1);

        // Assets carry a primary file and 'discovered' status.
        let n: i64 = c
            .conn
            .query_row(
                "SELECT COUNT(*) FROM asset
                 WHERE status='discovered' AND primary_file_id IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .expect("count");
        assert_eq!(n, 2);
    }

    #[test]
    fn assets_for_source_flags_trashed_and_set_trashed_round_trips() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 2, 2)])
            .expect("crawl");
        c.conn
            .execute(
                "UPDATE asset SET status='identified' WHERE name='a.png';",
                [],
            )
            .expect("promote");

        let rows = c.assets_for_source(src).expect("read");
        assert_eq!(rows.len(), 2);
        let a = rows.iter().find(|r| r.rel_path == "a.png").expect("a");
        assert_eq!(a.status, "identified");
        assert!(!a.trashed, "not trashed");

        let b_id = rows
            .iter()
            .find(|r| r.rel_path == "b.png")
            .expect("b")
            .asset_id;
        assert_eq!(c.set_trashed(&[b_id], true).expect("trash"), 1);
        let rows = c.assets_for_source(src).expect("read 2");
        assert!(
            rows.iter()
                .find(|r| r.rel_path == "b.png")
                .expect("b")
                .trashed,
            "flagged"
        );
        // Trashed rows leave the status counts (search, dup groups, staging).
        assert_eq!(
            c.status_counts_for_sources(&[src]).expect("counts").total(),
            1
        );
        // Restore round-trips.
        assert_eq!(c.set_trashed(&[b_id], false).expect("restore"), 1);
        let rows = c.assets_for_source(src).expect("read 3");
        assert!(
            !rows
                .iter()
                .find(|r| r.rel_path == "b.png")
                .expect("b")
                .trashed
        );
        assert_eq!(c.set_trashed(&[], true).expect("noop"), 0);
    }

    #[test]
    fn hydrate_and_missing_reconcile_round_trip() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", "C:/lib", "folder")
            .expect("src");
        c.record_crawl(src, None, &[rec("a.png", 1, 10), rec("sub/b.png", 2, 20)])
            .expect("crawl");

        let rows = c.entries_for_source(src).expect("hydrate");
        assert_eq!(rows.len(), 2);
        let a = rows.iter().find(|r| r.rel_path == "a.png").expect("a");
        assert_eq!(
            (a.media_type.as_str(), a.size_bytes, a.mtime),
            ("image", 1, 10)
        );
        assert_eq!(a.status, "discovered");
        assert!(!a.trashed);
        assert_eq!(a.rating, None);

        // Rating round-trips through both read paths.
        assert_eq!(c.set_rating(&[a.asset_id], Some(4)).expect("rate"), 1);
        let a2 = c
            .entries_for_source(src)
            .expect("hydrate rated")
            .into_iter()
            .find(|r| r.rel_path == "a.png")
            .expect("a");
        assert_eq!(a2.rating, Some(4));
        let read = c.assets_for_source(src).expect("read");
        assert_eq!(
            read.iter()
                .find(|r| r.rel_path == "a.png")
                .expect("a")
                .rating,
            Some(4)
        );
        assert_eq!(c.set_rating(&[a.asset_id], None).expect("clear"), 1);

        // b vanishes from disk → flagged missing, hidden from hydrate, metadata kept.
        let present: std::collections::HashSet<String> = ["a.png".to_string()].into();
        assert_eq!(c.reconcile_missing(src, &present).expect("gone"), (1, 0));
        assert_eq!(c.entries_for_source(src).expect("hydrate 2").len(), 1);

        // b reappears → unflagged, hydrates again.
        let present: std::collections::HashSet<String> =
            ["a.png".to_string(), "sub/b.png".to_string()].into();
        assert_eq!(c.reconcile_missing(src, &present).expect("back"), (0, 1));
        assert_eq!(c.entries_for_source(src).expect("hydrate 3").len(), 2);

        // Extracted package assets are exempt from missing-reconcile and hydrate.
        let pkg = c
            .upsert_package(src, "C:/lib/p.unitypackage", "p", None, None)
            .expect("pkg");
        c.record_crawl(src, Some(pkg), &[rec("p/x.png", 3, 30)])
            .expect("pkg crawl");
        assert_eq!(
            c.reconcile_missing(src, &present).expect("exempt"),
            (0, 0),
            "package asset not flagged despite being absent from the crawl"
        );
        assert_eq!(
            c.entries_for_source(src).expect("hydrate 4").len(),
            2,
            "package assets stay out of the folder hydrate"
        );
    }

    #[test]
    fn upsert_package_and_unpack_stamp() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Store", "C:/cache", "asset_store_library")
            .expect("src");

        // Browse-only: row exists, unpack fields NULL, idempotent by (source, path).
        let p1 = c
            .upsert_package(
                src,
                "C:/cache/Synty/3D/POLYGON City.unitypackage",
                "POLYGON City",
                Some("Synty"),
                Some("3D"),
            )
            .expect("insert");
        let p2 = c
            .upsert_package(
                src,
                "C:/cache/Synty/3D/POLYGON City.unitypackage",
                "POLYGON City",
                Some("Synty"),
                Some("3D"),
            )
            .expect("reuse");
        assert_eq!(p1, p2);
        let unpacked: Option<i64> = c
            .conn
            .query_row("SELECT unpacked_at FROM package WHERE id=?1", [p1], |r| {
                r.get(0)
            })
            .expect("row");
        assert_eq!(unpacked, None, "browse-only leaves unpacked_at NULL");

        // Ingest: extracted assets carry package_id; unpack stamp fills.
        let stats = c
            .record_crawl(src, Some(p1), &[rec("POLYGON City/rock.png", 5, 50)])
            .expect("extract crawl");
        assert_eq!(stats.inserted, 1);
        c.mark_package_unpacked(p1, 1).expect("stamp");
        let (pkg_of_asset, count): (i64, i64) = c
            .conn
            .query_row(
                "SELECT a.package_id, p.asset_count FROM asset a
                 JOIN package p ON p.id = a.package_id WHERE a.name='rock.png'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("joined");
        assert_eq!(pkg_of_asset, p1);
        assert_eq!(count, 1);

        // M13: the same archive found through a *second* source is the same pack, and the
        // second sighting fills in what the first didn't know rather than making a new row.
        let folder = c
            .upsert_import_source("Synty", "C:/cache/Synty", "folder")
            .expect("second source");
        let bare = c
            .upsert_package(
                folder,
                "C:/cache/Synty/3D/Bare.unitypackage",
                "Bare",
                None,
                None,
            )
            .expect("bare insert");
        let again = c
            .upsert_package(
                src,
                "C:/cache/Synty/3D/POLYGON City.unitypackage",
                "POLYGON City",
                Some("Synty"),
                Some("3D"),
            )
            .expect("same archive, other source");
        assert_eq!(again, p1, "one row per archive, whichever source found it");
        c.upsert_package(
            src,
            "C:/cache/Synty/3D/Bare.unitypackage",
            "Bare",
            Some("Synty"),
            Some("3D"),
        )
        .expect("fill");
        let filled: (Option<String>, Option<String>) = c
            .conn
            .query_row(
                "SELECT publisher, category FROM package WHERE id=?1",
                [bare],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("row");
        assert_eq!(
            filled,
            (Some("Synty".into()), Some("3D".into())),
            "a later sighting fills blanks"
        );
        assert_eq!(
            c.list_packages().expect("list").len(),
            2,
            "two archives, two rows"
        );
    }

    #[test]
    fn status_counts_scope_to_sources_and_fold_discovered() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let a = c.upsert_import_source("A", "C:/a", "folder").expect("a");
        let b = c.upsert_import_source("B", "C:/b", "folder").expect("b");
        c.record_crawl(a, None, &[rec("x.png", 1, 1), rec("y.png", 2, 2)])
            .expect("a crawl");
        c.record_crawl(b, None, &[rec("z.png", 3, 3)])
            .expect("b crawl");
        c.conn
            .execute(
                "UPDATE asset SET status='identified' WHERE name='y.png'",
                [],
            )
            .expect("promote");

        let counts = c.status_counts_for_sources(&[a]).expect("counts a");
        assert_eq!(
            counts,
            StatusCounts {
                unidentified: 1,
                identified: 1,
                confirmed: 0
            }
        );
        assert_eq!(counts.total(), 2, "source B excluded");
        assert_eq!(
            c.status_counts_for_sources(&[]).expect("empty"),
            StatusCounts::default()
        );
    }

    #[test]
    fn promote_statuses_walks_the_lifecycle() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
        c.record_crawl(
            s,
            None,
            &[rec("x.png", 1, 1), rec("y.png", 2, 2), rec("z.png", 3, 3)],
        )
        .expect("crawl");
        let id_of = |c: &Catalog, name: &str| -> i64 {
            c.conn
                .query_row("SELECT id FROM asset WHERE name=?1", params![name], |r| {
                    r.get(0)
                })
                .expect("id")
        };
        let status_of = |c: &Catalog, name: &str| -> String {
            c.conn
                .query_row(
                    "SELECT status FROM asset WHERE name=?1",
                    params![name],
                    |r| r.get(0),
                )
                .expect("status")
        };
        let (x, y, z) = (id_of(&c, "x.png"), id_of(&c, "y.png"), id_of(&c, "z.png"));

        // Nothing drained yet: everything stays `discovered`.
        assert_eq!(c.promote_statuses().expect("noop"), 0);
        assert_eq!(status_of(&c, "x.png"), "discovered");

        // Facets drained ⇒ indexed. An embedding alone is *not* enough: the asset is findable
        // but nothing has been said about it, so it stays in the Identify backlog.
        c.conn
            .execute("UPDATE asset SET metrics_dirty = 0", [])
            .expect("metrics drained");
        c.upsert_embedding(y, "m", "image", None, None, &[0.5, 0.5])
            .expect("embed y");
        assert_eq!(c.promote_statuses().expect("promote"), 3);
        assert_eq!(status_of(&c, "x.png"), "indexed");
        assert_eq!(
            status_of(&c, "y.png"),
            "indexed",
            "an embedding without tags is not identification"
        );

        // Tags on top of the embedding complete it.
        c.set_tags(
            y,
            "ai",
            Some("m"),
            &[TagAssignment::exact("subject", "rock")],
        )
        .expect("tag y");
        assert_eq!(c.promote_statuses().expect("promote with tags"), 1);
        assert_eq!(status_of(&c, "y.png"), "identified");
        assert_eq!(c.promote_statuses().expect("idempotent"), 0);

        // A confirmed row is human-set and never demoted or re-promoted.
        c.conn
            .execute(
                "UPDATE asset SET status='confirmed', embedding_dirty=1 WHERE id=?1",
                params![z],
            )
            .expect("confirm z");
        // y's bytes changed: dirty embedding drops it back to indexed.
        c.conn
            .execute("UPDATE asset SET embedding_dirty=1 WHERE id=?1", params![y])
            .expect("dirty y");
        assert_eq!(c.promote_statuses().expect("demote"), 1);
        assert_eq!(status_of(&c, "y.png"), "indexed");
        assert_eq!(status_of(&c, "z.png"), "confirmed");
        let _ = x;

        assert_eq!(
            c.status_counts_for_sources(&[s]).expect("counts"),
            StatusCounts {
                unidentified: 2,
                identified: 0,
                confirmed: 1
            }
        );
    }

    #[test]
    fn naming_heuristic_candidates_and_idempotent_insert() {
        // §10 step-2 catalog contract: an edge-less model is a candidate, the insert is idempotent,
        // and inserting an edge drops the model out of the candidate set. (The name matching itself
        // lives in the ingest layer and is tested there — `deps::texture_matches_model`.)
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
        let mk = |rel: &str, mt: &str| CrawlRecord {
            workspace_rel: None,
            rel_path: rel.to_string(),
            name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
            media_type: mt.to_string(),
            size_bytes: 1,
            mtime: 1,
        };
        c.record_crawl(
            s,
            None,
            &[
                mk("Chars/Human3.fbx", "model_3d"),
                mk("Chars/Human3_Albedo.png", "image"),
                mk("Chars/Human3_Normal.png", "image"),
                mk("Chars/Rock_Albedo.png", "image"), // unrelated stem
            ],
        )
        .expect("crawl");
        let id = |rel: &str| -> i64 {
            c.conn
                .query_row(
                    "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                     WHERE af.rel_path = ?1",
                    params![rel],
                    |r| r.get(0),
                )
                .expect("id")
        };
        let model = id("Chars/Human3.fbx");
        assert_eq!(
            c.edgeless_models()
                .expect("models")
                .iter()
                .map(|(m, _, _)| *m)
                .collect::<Vec<_>>(),
            vec![model],
            "the model with no edges is the sole candidate"
        );
        assert_eq!(c.inference_textures().expect("tex").len(), 3);

        let (alb, nrm) = (id("Chars/Human3_Albedo.png"), id("Chars/Human3_Normal.png"));
        assert_eq!(
            c.insert_inferred_texture_edges(&[
                (model, alb, "Chars/Human3_Albedo.png"),
                (model, nrm, "Chars/Human3_Normal.png"),
            ])
            .expect("insert"),
            2
        );
        assert!(
            c.edgeless_models().expect("m2").is_empty(),
            "a model that gained edges is no longer a candidate (idempotent re-run)"
        );
        assert_eq!(
            c.insert_inferred_texture_edges(&[(model, alb, "x")])
                .expect("reinsert"),
            0,
            "the UNIQUE conflict makes a re-insert a no-op"
        );

        let mut got: Vec<i64> = c
            .conn
            .prepare(
                "SELECT to_asset_id FROM asset_relationship
                 WHERE from_asset_id = ?1 AND source = 'inferred'",
            )
            .unwrap()
            .query_map(params![model], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        got.sort();
        let mut want = vec![alb, nrm];
        want.sort();
        assert_eq!(
            got, want,
            "exactly the resolved edges, confidence-tagged inferred"
        );
    }

    #[test]
    fn dependency_edges_resolve_now_or_later() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
        c.record_crawl(s, None, &[rec("rock.mat", 1, 1), rec("albedo.png", 2, 2)])
            .expect("crawl");
        let id_of = |c: &Catalog, name: &str| -> i64 {
            c.conn
                .query_row("SELECT id FROM asset WHERE name=?1", params![name], |r| {
                    r.get(0)
                })
                .expect("id")
        };
        let (mat, tex) = (id_of(&c, "rock.mat"), id_of(&c, "albedo.png"));
        let tex_guid = "b24b57e0cfed05643be34b0256df875e".to_string();
        let gone_guid = "ffffffffffffffffffffffffffffffff".to_string();

        // The material is scanned first — its target has no GUID yet, so the edge is `missing`.
        c.record_dependencies(
            mat,
            Some("1111"),
            "uses_texture",
            std::slice::from_ref(&tex_guid),
        )
        .expect("mat deps");
        let edge = |c: &Catalog| -> (Option<i64>, String) {
            c.conn
                .query_row(
                    "SELECT to_asset_id, state FROM asset_relationship WHERE from_asset_id=?1",
                    params![mat],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .expect("edge")
        };
        assert_eq!(edge(&c), (None, "missing".into()));
        assert_eq!(c.resolve_pending_relationships().expect("early"), 0);

        // The texture lands later; one statement late-binds the edge.
        c.record_dependencies(tex, Some(&tex_guid), "uses_texture", &[])
            .expect("tex deps");
        assert_eq!(c.resolve_pending_relationships().expect("late"), 1);
        assert_eq!(edge(&c), (Some(tex), "resolved".into()));

        // A rescan replaces the harvest wholesale rather than duplicating it, and a reference to
        // something that isn't in the library stays honestly unresolved.
        c.record_dependencies(mat, Some("1111"), "uses_texture", &[tex_guid, gone_guid])
            .expect("rescan");
        let counts: (i64, i64) = c
            .conn
            .query_row(
                "SELECT COUNT(*), SUM(to_asset_id IS NULL) FROM asset_relationship
                 WHERE from_asset_id=?1",
                params![mat],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("counts");
        assert_eq!(counts, (2, 1), "two edges, one still unresolved");

        // Every scanned asset is off the queue.
        let dirty: i64 = c
            .conn
            .query_row("SELECT COUNT(*) FROM asset WHERE deps_dirty=1", [], |r| {
                r.get(0)
            })
            .expect("dirty");
        assert_eq!(dirty, 0);
    }

    #[test]
    fn prefab_edges_rebuild_and_extend_the_closure() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
        c.record_crawl(
            s,
            None,
            &[
                rec("hero.fbx", 1, 1),
                rec("hero.mat", 2, 2),
                rec("t.png", 3, 3),
            ],
        )
        .expect("crawl");
        let id_of = |c: &Catalog, name: &str| -> i64 {
            c.conn
                .query_row("SELECT id FROM asset WHERE name=?1", params![name], |r| {
                    r.get(0)
                })
                .expect("id")
        };
        let (mesh, mat, tex) = (
            id_of(&c, "hero.fbx"),
            id_of(&c, "hero.mat"),
            id_of(&c, "t.png"),
        );
        c.record_dependencies(mesh, Some("meshguid"), "uses_texture", &[])
            .expect("mesh");
        c.record_dependencies(tex, Some("texguid"), "uses_texture", &[])
            .expect("tex");
        c.record_dependencies(mat, Some("matguid"), "uses_texture", &["texguid".into()])
            .expect("mat");

        // A prefab binds the mesh to the material; the mesh's closure now reaches the texture
        // through it — which is the whole point of the pass.
        let written = c
            .replace_prefab_relationships(&[
                ("meshguid".into(), "matguid".into()),
                ("nosuchmesh".into(), "matguid".into()),
            ])
            .expect("prefab edges");
        assert_eq!(written, 1, "a pair whose mesh isn't catalogued is dropped");
        assert_eq!(
            c.dependency_closure(&[mesh]).expect("closure"),
            vec![mesh, mat, tex],
            "mesh → material → texture, transitively"
        );

        // Rebuilding replaces rather than accumulates, and the `unity_meta` edges survive it.
        c.replace_prefab_relationships(&[("meshguid".into(), "matguid".into())])
            .expect("rebuild");
        let by_source: Vec<(String, i64)> = c
            .conn
            .prepare("SELECT source, COUNT(*) FROM asset_relationship GROUP BY 1 ORDER BY 1")
            .and_then(|mut st| {
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()
            })
            .expect("by source");
        assert_eq!(
            by_source,
            vec![
                ("unity_meta".to_string(), 1),
                ("unity_prefab".to_string(), 1)
            ]
        );
    }

    /// The decisive one: a tag makes an asset findable by a word that appears nowhere in its
    /// name or path — which is what `tags_concat` was silently failing to do while the trigger
    /// wrote `''`. Re-tagging must also *remove* the old term, or search starts lying.
    #[test]
    fn tags_are_searchable_and_replaced_per_source() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
        c.record_crawl(s, None, &[rec("asset_0192.fbx", 1, 1)])
            .expect("crawl");
        let id = c
            .conn
            .query_row("SELECT id FROM asset", [], |r| r.get::<_, i64>(0))
            .expect("id");
        let found = |c: &Catalog, q: &str| -> bool {
            !c.search_assets(q, 10)
                .expect("search")
                .unwrap_or_default()
                .is_empty()
        };

        assert!(!found(&c, "beaver"), "nothing to match yet");
        c.set_tags(
            id,
            "ai",
            Some("stub-v1"),
            &[
                TagAssignment {
                    namespace: "subject".into(),
                    value: "Beaver".into(),
                    confidence: Some(1.7), // clamped
                },
                TagAssignment::exact("style", "low_poly"),
            ],
        )
        .expect("ai tags");
        assert!(found(&c, "beaver"), "tag term reaches FTS");
        assert!(found(&c, "low poly"), "underscores fold to spaces");
        assert_eq!(
            c.conn
                .query_row(
                    "SELECT confidence FROM asset_tag WHERE confidence IS NOT NULL",
                    [],
                    |r| r.get::<_, f64>(0)
                )
                .expect("confidence"),
            1.0,
            "confidence clamped into [0,1]"
        );

        // A user tag lands beside the model's, not on top of it.
        c.set_tags(id, "manual", None, &[TagAssignment::exact("mood", "cosy")])
            .expect("manual tag");
        assert!(found(&c, "beaver") && found(&c, "cosy"), "sources coexist");

        // Re-running the tagger replaces only its own tags — and the retired term stops matching.
        c.set_tags(
            id,
            "ai",
            Some("stub-v2"),
            &[TagAssignment::exact("subject", "otter")],
        )
        .expect("retag");
        assert!(found(&c, "otter"), "new term indexed");
        assert!(!found(&c, "beaver"), "old term removed from the index");
        assert!(found(&c, "cosy"), "the manual tag survived the re-run");

        let tags = c.tags_for_asset(id).expect("read back");
        assert_eq!(
            tags.iter()
                .map(|t| (t.namespace.as_str(), t.value.as_str()))
                .collect::<Vec<_>>(),
            vec![("mood", "cosy"), ("subject", "otter")]
        );
        let dirty: i64 = c
            .conn
            .query_row(
                "SELECT tags_dirty FROM asset WHERE id=?1",
                params![id],
                |r| r.get(0),
            )
            .expect("flag");
        assert_eq!(dirty, 0, "tagging clears the queue flag");
    }

    /// The grid reads tags in bulk, and the filter chips read the vocabulary with counts; both
    /// must exclude trashed assets, or the UI offers a facet that filters to nothing.
    /// A favourite is a pin, independent of rating and status, and a trashed asset stops being
    /// listed as one — it left the grid, so offering it as a favourite would be a dead link.
    #[test]
    fn favorites_are_independent_of_rating_and_hide_when_trashed() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
        c.record_crawl(s, None, &[rec("a.png", 1, 1), rec("b.png", 2, 2)])
            .expect("crawl");
        let ids: Vec<i64> = c
            .conn
            .prepare("SELECT id FROM asset ORDER BY id")
            .and_then(|mut st| {
                st.query_map([], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()
            })
            .expect("ids");

        assert!(c.favorite_ids().expect("none yet").is_empty());
        assert_eq!(c.set_favorite(&ids[..1], true).expect("fav"), 1);
        assert_eq!(c.favorite_ids().expect("one"), vec![ids[0]]);

        // Rating the *other* asset must not make it a favourite, and vice versa.
        c.set_rating(&ids[1..], Some(5)).expect("rate");
        assert_eq!(c.favorite_ids().expect("still one"), vec![ids[0]]);

        c.set_trashed(&ids[..1], true).expect("trash");
        assert!(
            c.favorite_ids().expect("after trash").is_empty(),
            "a trashed favourite is not listed"
        );
        c.set_trashed(&ids[..1], false).expect("restore");
        assert_eq!(
            c.favorite_ids().expect("after restore"),
            vec![ids[0]],
            "restoring brings the pin back — it was never cleared"
        );
        assert_eq!(c.set_favorite(&ids[..1], false).expect("unfav"), 1);
        assert!(c.favorite_ids().expect("cleared").is_empty());
    }

    #[test]
    fn bulk_tag_reads_skip_trashed_assets() {
        let mut c = Catalog::open_in_memory().expect("open");
        c.seed_media_types().expect("seed");
        let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
        c.record_crawl(s, None, &[rec("a.png", 1, 1), rec("b.png", 2, 2)])
            .expect("crawl");
        let ids: Vec<i64> = c
            .conn
            .prepare("SELECT id FROM asset ORDER BY id")
            .and_then(|mut st| {
                st.query_map([], |r| r.get(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()
            })
            .expect("ids");
        for id in &ids {
            c.set_tags(
                *id,
                "ai",
                Some("m"),
                &[TagAssignment::exact("subject", "rock")],
            )
            .expect("tag");
        }
        assert_eq!(c.all_asset_tags().expect("bulk").len(), 2);
        assert_eq!(
            c.tag_usage(10).expect("usage"),
            vec![("subject".to_string(), "rock".to_string(), 2)]
        );

        c.set_trashed(&[ids[0]], true).expect("trash");
        assert_eq!(
            c.all_asset_tags().expect("bulk after trash").len(),
            1,
            "a trashed asset leaves the grid, so its tags do too"
        );
        assert_eq!(
            c.tag_usage(10).expect("usage after trash")[0].2,
            1,
            "the chip count follows"
        );
    }

    /// Namespaces share the `tag` table, and `norm_name` is globally UNIQUE — so the same word in
    /// two facets must not collapse onto one row.
    #[test]
    fn the_same_word_in_two_facets_stays_two_tags() {
        assert_eq!(
            normalize_tag("material", "Metal"),
            Some(("material:metal".into(), "Metal".into()))
        );
        assert_eq!(
            normalize_tag("mood", "metal").map(|(n, _)| n),
            Some("mood:metal".into())
        );
        assert_eq!(
            normalize_tag("", "  spaced   out  "),
            Some(("free:spaced out".into(), "spaced out".into())),
            "empty namespace falls back to free; whitespace collapses"
        );
        assert_eq!(normalize_tag("subject", "   "), None);
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
