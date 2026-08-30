//! Catalog — SQLite-backed asset store.
//! Schema spec and invariants: docs/DATA-MODEL.md §2–10.
//!
//! Migration strategy: `PRAGMA user_version` tracks the schema version;
//! `apply_migrations` runs numbered SQL steps in order on first open.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
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
    /// BLAKE3 of the file contents — the exact-duplicate key (DATA-MODEL §6). `None` for
    /// package-extracted records (their files land in the workspace cache, not the source).
    pub content_hash: Option<String>,
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
    /// `available` | `paused` | `unavailable` (M18). Derived by [`Catalog::refresh_source_health`],
    /// never authored by the user — `paused` is `watch_enabled` surfaced as a state.
    pub state: String,
    /// Unix seconds the root last resolved. Drives "unavailable since …" in the UI.
    pub last_seen_at: Option<i64>,
}

/// The answer to "what would relinking this source to that folder actually reach?" (UI-SPEC §11.1).
///
/// `total` counts the assets a relink can speak for — every one the source owns whose bytes are
/// expected under its root. Assets already extracted into the workspace cache are excluded from
/// both numbers, because moving the root does not move them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RelinkPreview {
    pub matched: i64,
    pub total: i64,
}

/// One export run, as the Recent-exports list reads it (FEATURES §7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportBatch {
    pub batch_id: String,
    /// The folder the run wrote into. Absolute — an export goes wherever the user pointed it.
    pub dest_path: String,
    /// Files written, which is what undo would delete. Not the number of assets picked: a
    /// dependency closure turns one pick into several files.
    pub files: i64,
    pub exported_at: i64,
}

/// What the `✓ in <project>` badge needs to know about one asset (DESIGN-GUIDELINES §7).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportState {
    /// Imported into the project that is active right now.
    pub in_active: bool,
    /// Imported here, but the source file has changed on disk since — the actionable state.
    pub stale: bool,
    /// How many *other* projects hold it. History, not a warning.
    pub others: i64,
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
    /// The asset's tags (ENGINE-API §3). Filled by [`Catalog::asset_summaries`] — an agent that
    /// has just written a tag has to be able to read it back, and this field answered `[]`
    /// unconditionally while the plane called that "interim, tagging is P4".
    pub tags: Vec<TagAssignment>,
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

/// Which assets the appraisal queues should reach **first** (UI-SPEC §3).
///
/// Three tiers, in the order the spec names them: what is on screen right now, the rest of the
/// current filter, then everything else newest-first. Held in memory rather than written to a
/// column — it changes on every scroll, and a browse that dirties thousands of rows to record
/// where the eye is would be a write storm for a hint.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppraiseFocus {
    /// The page the user is looking at, in the order it is drawn.
    pub visible: Vec<i64>,
    /// The rest of the current filter, in the grid's own sort order. Capped by the caller.
    pub filtered: Vec<i64>,
}

impl AppraiseFocus {
    /// The `ORDER BY` the appraisal queues share, over an `asset` aliased `a` (or bare, when the
    /// query has no alias — pass `"id"`).
    ///
    /// Every queue orders by this one expression, which is the only reason "browsing re-orders the
    /// queue" is one change rather than eight: tags, embeddings, metrics, previews and dependency
    /// scans all have to move together, or an on-screen asset gets tagged and then waits behind
    /// 3,000 others for the embedding that would let it be promoted.
    ///
    /// The ids are `i64` read out of this same database and formatted by Rust, so they are inlined
    /// rather than bound — a bound list would need placeholders generated per call and threaded
    /// through eight different parameter tuples for no gain.
    ///
    /// With no focus it is `ORDER BY <col> DESC`: newest first, the spec's third tier, which is
    /// also what a user with no filter is most likely to want.
    fn order_by(&self, col: &str) -> String {
        let bucket = |ids: &[i64]| {
            ids.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        match (self.visible.is_empty(), self.filtered.is_empty()) {
            (true, true) => format!("ORDER BY {col} DESC"),
            (false, true) => format!(
                "ORDER BY CASE WHEN {col} IN ({}) THEN 0 ELSE 1 END, {col} DESC",
                bucket(&self.visible)
            ),
            (true, false) => format!(
                "ORDER BY CASE WHEN {col} IN ({}) THEN 0 ELSE 1 END, {col} DESC",
                bucket(&self.filtered)
            ),
            (false, false) => format!(
                "ORDER BY CASE WHEN {col} IN ({}) THEN 0 WHEN {col} IN ({}) THEN 1 ELSE 2 END, {col} DESC",
                bucket(&self.visible),
                bucket(&self.filtered)
            ),
        }
    }
}

/// The fully-opened, migration-applied catalog database.
pub struct Catalog {
    pub(crate) conn: Connection,
    /// Appraisal queue order (UI-SPEC §3). Not persisted: it is where the user is looking, and
    /// that is only true for as long as they are looking there.
    appraise_focus: AppraiseFocus,
}

impl Catalog {
    /// Open (or create) the catalog at `path`. Runs all pending migrations.
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).map_err(|e| CoreError::Catalog(format!("open failed: {e}")))?;

        configure_connection(&conn)?;
        let mut catalog = Self {
            conn,
            appraise_focus: AppraiseFocus::default(),
        };
        catalog.apply_migrations()?;
        Ok(catalog)
    }

    /// In-memory catalog (tests / CLI dry-run).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()
            .map_err(|e| CoreError::Catalog(format!("in-memory open failed: {e}")))?;
        configure_connection(&conn)?;
        let mut catalog = Self {
            conn,
            appraise_focus: AppraiseFocus::default(),
        };
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
            (16, MIGRATION_16_OP_LOG),
            (17, MIGRATION_17_OP_LOG_REDO),
            (18, MIGRATION_18_SOURCE_STATE),
            (19, MIGRATION_19_EXPORT_HISTORY),
            (20, MIGRATION_20_UNPROJECTED_STAGING),
            (21, MIGRATION_21_STORE_PACK_LICENSE),
            (22, MIGRATION_22_DROP_BUILTIN_EDGES),
            (23, MIGRATION_23_KEEP_CACHED),
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

    /// Flag one live asset for re-identification — the per-card ✦ (KANBAN B11b). The next
    /// appraise pass that sees it (a kicked Identify pass, or auto-identify) re-embeds and
    /// re-tags it and promotes the result.
    pub fn mark_for_identify(&self, asset_id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE asset SET embedding_dirty = 1, tags_dirty = 1, updated_at = ?2
                 WHERE id = ?1 AND deleted_at IS NULL",
                params![asset_id, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("mark_for_identify: {e}")))?;
        Ok(())
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
                        COUNT(a.id) AS asset_count, s.state, s.last_seen_at
                 FROM import_source s
                 LEFT JOIN asset a ON a.source_id = s.id AND a.deleted_at IS NULL
                 GROUP BY s.id, s.name, s.root_path, s.kind, s.last_scanned_at, s.watch_enabled,
                          s.state, s.last_seen_at
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
                    state: r.get(7)?,
                    last_seen_at: r.get(8)?,
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
                        COUNT(a.id) AS asset_count, s.state, s.last_seen_at
                 FROM import_source s
                 LEFT JOIN asset a ON a.source_id = s.id AND a.deleted_at IS NULL
                 WHERE s.root_path = ?1
                 GROUP BY s.id, s.name, s.root_path, s.kind, s.last_scanned_at, s.watch_enabled,
                          s.state, s.last_seen_at",
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
                    state: r.get(7)?,
                    last_seen_at: r.get(8)?,
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
                            "UPDATE asset SET primary_file_id = ?2, content_hash = ?3 WHERE id = ?1",
                            params![asset_id, tx.last_insert_rowid(), rec.content_hash],
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
                            "UPDATE asset_file SET size_bytes = ?2, mtime = ?3
                             WHERE id = ?1",
                            params![file_id, rec.size_bytes, rec.mtime],
                        )
                        .map_err(|e| CoreError::Catalog(format!("asset_file update: {e}")))?;
                        tx.execute(
                            // The bytes changed, so the extracted facets and the outbound
                            // dependency refs are stale too — and the primary content hash
                            // follows the new bytes (the crawl hashed the changed file).
                            "UPDATE asset SET updated_at = ?2, embedding_dirty = 1,
                                              metrics_dirty = 1, deps_dirty = 1,
                                              tags_dirty = 1, content_hash = ?3
                             WHERE id = ?1",
                            params![asset_id, now, rec.content_hash],
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

    /// Move/rename detection (DATA-MODEL §6): a file whose **path vanished** from a crawl while
    /// its **content hash reappeared at another path** is the same file moved — update `rel_path`
    /// in place, preserving `asset_id`, tags, embeddings, mappings and the hash. Trashed assets
    /// are exempt (trash is a user decision; a moved file must not resurrect it). Returns the
    /// new rel_paths that were consumed, so the caller can drop them from `record_crawl` and
    /// avoid a duplicate insert.
    ///
    /// Call this **before** `record_crawl` on a full-source crawl. Missing rows (`af.missing=1`)
    /// are included: a file that reappears at a new path is a move back, not a duplicate.
    pub fn reconcile_moves(
        &mut self,
        source_id: i64,
        records: &[CrawlRecord],
    ) -> Result<Vec<String>> {
        let present: std::collections::HashSet<String> =
            records.iter().map(|r| r.rel_path.clone()).collect();
        // New-path records with a hash are the move candidates: same bytes, new location.
        let hash_to_rel: std::collections::HashMap<&str, &CrawlRecord> = records
            .iter()
            .filter_map(|r| r.content_hash.as_deref().map(|h| (h, r)))
            .collect();
        if hash_to_rel.is_empty() {
            return Ok(Vec::new());
        }
        // Vanished files: in the catalog, not in this crawl, with a known hash.
        let vanished: Vec<(i64, String, String, i64)> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT af.id, af.rel_path, a.content_hash, a.id
                     FROM asset_file af JOIN asset a ON a.id = af.asset_id
                     WHERE a.source_id = ?1 AND a.package_id IS NULL
                       AND a.status != 'trashed'
                       AND a.content_hash IS NOT NULL",
                )
                .map_err(|e| CoreError::Catalog(format!("moves prep: {e}")))?;
            let mapped = stmt
                .query_map(params![source_id], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })
                .map_err(|e| CoreError::Catalog(format!("moves query: {e}")))?;
            mapped
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("moves row: {e}")))?
        };

        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("moves begin: {e}")))?;
        let mut moved = Vec::new();
        for (file_id, rel_path, hash, asset_id) in vanished {
            if present.contains(&rel_path) {
                continue; // still on disk at the same path — not a move
            }
            let Some(rec) = hash_to_rel.get(hash.as_str()) else {
                continue; // vanished with no hash match — the caller's reconcile_missing flags it
            };
            tx.execute(
                "UPDATE asset_file SET rel_path = ?2, ext = ?3, size_bytes = ?4,
                                       mtime = ?5, missing = 0
                 WHERE id = ?1",
                params![
                    file_id,
                    rec.rel_path,
                    rec.rel_path.rsplit('.').next().map(str::to_lowercase),
                    rec.size_bytes,
                    rec.mtime,
                ],
            )
            .map_err(|e| CoreError::Catalog(format!("moves file update: {e}")))?;
            tx.execute(
                "UPDATE asset SET name = ?2, updated_at = ?3 WHERE id = ?1",
                params![asset_id, rec.name, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("moves asset update: {e}")))?;
            moved.push(rec.rel_path.clone());
        }
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("moves commit: {e}")))?;
        Ok(moved)
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
    pub fn promote_statuses(&self) -> Result<Vec<i64>> {
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
        ] {
            changed += self
                .conn
                .execute(sql, params![now])
                .map_err(|e| CoreError::Catalog(format!("promote_statuses: {e}")))?;
        }
        // Capture the promotion candidates first so the pass can hand the appraised set to the
        // Identify screen's session view (UI-SPEC §3) — the rows that just became identified.
        let promoted: Vec<i64> = {
            let mut stmt = self
                .conn
                .prepare(
                    "SELECT id FROM asset
                     WHERE deleted_at IS NULL AND status IN ('discovered', 'indexed')
                       AND embedding_dirty = 0
                       AND EXISTS (SELECT 1 FROM asset_embedding e WHERE e.asset_id = asset.id)
                       AND EXISTS (SELECT 1 FROM asset_tag t WHERE t.asset_id = asset.id)",
                )
                .map_err(|e| CoreError::Catalog(format!("promote candidates: {e}")))?;
            let ids = stmt
                .query_map([], |r| r.get(0))
                .map_err(|e| CoreError::Catalog(format!("promote candidates query: {e}")))?;
            ids.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| CoreError::Catalog(format!("promote candidates row: {e}")))?
        };
        changed += self
            .conn
            .execute(
                "UPDATE asset SET status = 'identified', updated_at = ?1
                 WHERE deleted_at IS NULL AND status IN ('discovered', 'indexed')
                   AND embedding_dirty = 0
                   AND EXISTS (SELECT 1 FROM asset_embedding e WHERE e.asset_id = asset.id)
                   AND EXISTS (SELECT 1 FROM asset_tag t WHERE t.asset_id = asset.id)",
                params![now],
            )
            .map_err(|e| CoreError::Catalog(format!("promote_statuses: {e}")))?;
        // Keep the old count contract for callers that only care about the delta.
        let _ = changed;
        Ok(promoted)
    }
}

/// One facet value on an asset (IDENTIFICATION-AND-SEARCH §2): `namespace` is the facet
/// (`subject|style|material|color|mood|usage|audio|technical|free`), `value` the term, and
/// `confidence` the model's self-estimate — `None` for user-set and file-derived tags, which are
/// exact rather than guessed.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
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

/// Swap one absolute path's root prefix for another.
///
/// `None` when `path` does not sit under `old_root` — the caller then leaves the row alone rather
/// than inventing a location for it. Uses `strip_prefix` rather than string surgery so a root
/// stored `C:/cache` still matches a pack stored `C:\cache\x.unitypackage`: the separators differ,
/// the components do not.
fn reanchor(path: &Path, old_root: &Path, new_root: &Path) -> Option<PathBuf> {
    path.strip_prefix(old_root)
        .ok()
        .map(|rest| new_root.join(rest))
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
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RelatedAsset {
    /// `asset_relationship.id` — what ⊘ Ignore link drops (UI-SPEC §13.6).
    pub edge_id: i64,
    /// True: the selected asset *uses* this one. False: this one uses the selected asset.
    pub outbound: bool,
    pub kind: String,
    pub state: String,
    /// What the parent declared — a GUID for Unity-harvested edges.
    pub declared_path: Option<String>,
    pub asset_id: Option<i64>,
    pub name: Option<String>,
    pub media_code: Option<String>,
    /// UI-SPEC §13.6's **✓ in pack** vs **⤷ in library**: does the other end live in the *same*
    /// package as the selected asset? `None` when the edge resolves to nothing (✕ missing).
    ///
    /// It is not decoration. A dependency inside the pack travels with it; one that resolves
    /// across packs is exactly what ENGINE-API §5.5 flags, because Unity reassigns GUIDs per
    /// import and the reference will not survive the trip.
    pub same_pack: Option<bool>,
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

/// M16 — undo op-log (UI-SPEC §2 "Ctrl/Cmd-Z" + DESIGN-GUIDELINES §7 "Trash & restore", L5).
/// A durable, append-only ledger of catalog mutations the UI can revert. Each row names the
/// operation and carries its pre-image (or the id needed to invert it) as JSON — see the
/// `revert_*` family on `Catalog`. `created_at` is unix seconds; `recent_ops`/`delete_ops`
/// manage the tail.
const MIGRATION_16_OP_LOG: &str = "
CREATE TABLE IF NOT EXISTS op_log (
  id           INTEGER PRIMARY KEY,
  op_kind      TEXT    NOT NULL,        -- 'set_tags' | 'set_trashed' | 'create_collection' | ...
  payload_json TEXT    NOT NULL,
  created_at   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_oplog_created ON op_log(created_at);
";

const MIGRATION_17_OP_LOG_REDO: &str = "
ALTER TABLE op_log ADD COLUMN forward_json TEXT;  -- the redo payload; NULL = payload_json IS the forward too
ALTER TABLE op_log ADD COLUMN undone_at INTEGER;  -- set on undo; redo applies the forward then deletes the row
CREATE INDEX IF NOT EXISTS idx_oplog_undone ON op_log(undone_at);
";

/// M18 — source availability (DATA-MODEL §1, UI-SPEC §11.1).
///
/// A source root going away *after* indexing is the normal end state for an external drive or a
/// moved folder, and nothing in the schema could say so: `watch_enabled` is the user's intent, not
/// the disk's answer. `state` is **derived** — the open/scan health check writes it — and nothing
/// is ever deleted because a root went missing.
const MIGRATION_18_SOURCE_STATE: &str = "
ALTER TABLE import_source ADD COLUMN state TEXT NOT NULL DEFAULT 'available';  -- available|paused|unavailable
ALTER TABLE import_source ADD COLUMN last_seen_at INTEGER;                     -- unix seconds the root last resolved
";

/// M19 — export history (FEATURES §7.1 "Recent exports", §7.6).
///
/// A table of its own rather than a column on `import_record`, because the two are different acts
/// and the product keeps their verbs apart on purpose (UI-SPEC §16.2): **Import** puts files into a
/// registered project, **Export** copies them to a folder the user chose. `import_record` cannot
/// hold an export even if we wanted it to — its `project_id` is `NOT NULL` and its `dest_path` is
/// project-relative, so undo resolves it against the project root.
///
/// One row per **file written**, not per asset: undo has to delete exactly what was written, and a
/// dependency closure means one pick can produce several files. `asset_id` goes `NULL` rather than
/// cascading, so trashing an asset later does not quietly erase the record that it was exported —
/// history the user might still want to undo.
const MIGRATION_19_EXPORT_HISTORY: &str = "
CREATE TABLE IF NOT EXISTS export_record (
    id          INTEGER PRIMARY KEY,
    batch_id    TEXT    NOT NULL,           -- one export run
    asset_id    INTEGER REFERENCES asset(id) ON DELETE SET NULL,
    dest_path   TEXT    NOT NULL,           -- absolute folder the run wrote into
    file_name   TEXT    NOT NULL,           -- the file written inside it
    exported_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_export_batch ON export_record(batch_id);
";

/// M20 — one staging tray that belongs to no project yet (DATA-MODEL §9, UI-SPEC §16.2).
///
/// M5 made trays per-project and **deleted** the legacy shared one, on the reading that a
/// shortlist only means something once there is an editor to send it to. That was true of the
/// agent plane and false of the app: the desktop tray is the way a person collects picks, and
/// the Unity plugin does not exist yet, so *every* user is in the state M5 treated as impossible.
/// The shell kept its own in-memory list instead — two trays that never agreed, and picks that
/// did not survive a restart. See ADR-0010.
///
/// The index is what M5's could not express. `idx_collection_staging` is
/// `UNIQUE(project_id) WHERE is_staging=1`, and SQLite treats NULLs as distinct in a UNIQUE
/// index, so it constrains nothing for a project-less row — which is why M5 had to delete them
/// rather than rely on it. This one is over a constant, so it admits exactly one such row.
const MIGRATION_20_UNPROJECTED_STAGING: &str = "
CREATE UNIQUE INDEX IF NOT EXISTS idx_collection_staging_unprojected
  ON collection(is_staging) WHERE is_staging=1 AND project_id IS NULL;
";

/// M23 — per-pack workspace retention (UI-SPEC §15 / §4, INGESTION §2 "Workspace retention").
///
/// Deliberately **not** `package.pinned`, which already means "Add to Library" (M7): whether a
/// pack shows in the grid and whether its extracted bytes are worth hundreds of megabytes are
/// different questions, and one switch answering both would make freeing disk space cost you the
/// pack's visibility.
///
/// A pack that is *not* kept is still fully searchable, filterable and previewable — its payloads
/// resolve out of the archive by surgical pull instead of off disk, which is slower per open, not
/// absent.
const MIGRATION_23_KEEP_CACHED: &str = "
ALTER TABLE package ADD COLUMN keep_cached INTEGER NOT NULL DEFAULT 0;
";

/// M22 — drop edges that point at Unity's own built-in resource libraries.
///
/// A prefab painted with Unity's *default* material declares the built-in-resources GUID, and
/// nothing in any catalog will ever be that file — so the edge is `missing` for good. It made the
/// Inspector warn that "missing links are skipped on export" about a link that is not broken, and
/// offered a relink the user has nothing to point at. `deps::guid_in` now filters these at harvest;
/// this clears the ones already minted (one in the reference library).
///
/// Scoped to unresolved rows, so a real asset that somehow carries such a GUID is left alone.
const MIGRATION_22_DROP_BUILTIN_EDGES: &str = "
DELETE FROM asset_relationship
 WHERE to_asset_id IS NULL
   AND declared_path IN ('0000000000000000f000000000000000',
                         '0000000000000000e000000000000000',
                         '0000000000000000d000000000000000');
";

/// M21 — backfill the Asset Store EULA onto packs that came from the store cache.
///
/// The licence harvest (INGESTION.md §2) has always been able to state this: a pack with a
/// `publisher` was read out of the `<Publisher>/<Category>/` cache tree, and where the pack
/// itself says nothing, where it came from is the licence. Only the desktop write path never
/// called the setter, so 87 packs in the reference library recorded no licence at all — and a
/// re-harvest cannot reach them without re-inflating every archive to learn a fact the
/// `publisher` column already carries.
///
/// Guarded on `license IS NULL`, so a pack that stated its own licence keeps it, and the
/// migration is a no-op the second time.
const MIGRATION_21_STORE_PACK_LICENSE: &str = "
UPDATE package
   SET license     = 'Unity Asset Store EULA',
       license_url = 'https://unity.com/legal/as-terms'
 WHERE publisher IS NOT NULL AND license IS NULL;
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
    pub license_url: Option<String>,
    pub attribution: Option<String>,
    /// M23 — its extracted bytes are worth keeping. Not `pinned`; see the migration.
    pub keep_cached: bool,
    /// `built_in` | `urp` | `hdrp` | `mixed` (DATA-MODEL §12); `None` = not determined.
    pub pipeline: Option<String>,
}

/// A registered Unity editor (ENGINE-API §5.1 / DATA-MODEL §9). The plugin upserts its row on
/// panel open and heartbeats `last_seen_at`; exactly one row is `is_active` at a time.
#[derive(Debug, Clone, serde::Serialize)]
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

/// A pack holding extracted bytes in the workspace (M23) — the Settings storage list and the
/// input to the reclaim sweep.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WorkspacePack {
    pub id: i64,
    pub name: String,
    /// The `.unitypackage` these bytes came out of. When it no longer resolves on disk, the
    /// extracted copy is the only one left and the sweep must never take it.
    pub package_path: String,
    pub keep_cached: bool,
    /// Workspace files this pack still resolves through.
    pub files: usize,
}

/// One pack in an import request whose render pipeline disagrees with the target project's
/// (ENGINE-API §5.4). Grouped per pack: the assets share one cause and one sentence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PipelineAdvisory {
    pub package_id: i64,
    pub package: String,
    /// `built_in` | `urp` | `hdrp` — never `mixed`, which is not a mismatch.
    pub package_pipeline: String,
    pub project_pipeline: String,
    pub asset_ids: Vec<i64>,
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
                        asset_count, unpacked_at, pinned, version, license,
                        license_url, attribution, pipeline, keep_cached
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
                    license_url: r.get(10)?,
                    attribution: r.get(11)?,
                    pipeline: r.get(12)?,
                    keep_cached: r.get::<_, i64>(13)? == 1,
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
            .prepare(&format!(
                "SELECT id, name, description, media_type_id, source_id
                 FROM asset
                 WHERE embedding_dirty = 1 AND deleted_at IS NULL
                 {order} LIMIT ?1",
                order = self.appraise_focus.order_by("id")
            ))
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
            .prepare(&format!(
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
                 {order} LIMIT ?1",
                order = self.appraise_focus.order_by("a.id")
            ))
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
            .prepare(&format!(
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
                 {order} LIMIT ?1",
                order = self.appraise_focus.order_by("a.id")
            ))
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
            .prepare(&format!(
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
                 {order} LIMIT ?1",
                order = self.appraise_focus.order_by("a.id")
            ))
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
            .prepare(&format!(
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
                 {order} LIMIT ?1",
                order = self.appraise_focus.order_by("a.id")
            ))
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
            .prepare(&format!(
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
                 {order} LIMIT ?1",
                order = self.appraise_focus.order_by("a.id")
            ))
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
        // …except the ones the user muted (UI-SPEC §13.6 ⊘). Re-deriving from the file would
        // otherwise resurrect every ignored link the moment the asset's bytes changed, which is
        // the one thing "ignore" has to survive.
        tx.execute(
            "DELETE FROM asset_relationship
              WHERE from_asset_id = ?1 AND source = 'unity_meta' AND state <> 'ignored'",
            params![asset_id],
        )
        .map_err(|e| CoreError::Catalog(format!("deps clear edges: {e}")))?;
        for reference in ref_guids {
            // The `WHERE NOT EXISTS` is what keeps an ignored edge ignored: its `to_asset_id`
            // is NULL, and SQLite treats NULLs as distinct, so `ON CONFLICT` cannot dedupe it.
            tx.execute(
                "INSERT INTO asset_relationship
                    (from_asset_id, to_asset_id, kind, source, declared_path, state)
                 SELECT ?1,
                    (SELECT id FROM asset WHERE unity_guid = ?2 AND deleted_at IS NULL),
                    ?3, 'unity_meta', ?2,
                    CASE WHEN EXISTS (SELECT 1 FROM asset
                                       WHERE unity_guid = ?2 AND deleted_at IS NULL)
                         THEN 'resolved' ELSE 'missing' END
                  WHERE NOT EXISTS (SELECT 1 FROM asset_relationship
                                     WHERE from_asset_id = ?1 AND declared_path = ?2
                                       AND state = 'ignored')
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

    /// Add one tag to an asset without touching its other tags (the bulk-bar / inspector
    /// quick-add path, D9/G4). Unlike [`Self::set_tags`] — which replaces the whole per-source
    /// set — this appends: adding "shiny" must never delete the asset's other manual tags.
    /// Already-present tags are a no-op (ON CONFLICT DO NOTHING, confidence untouched).
    /// Returns the number of rows actually inserted. Re-indexes FTS like every tag write.
    pub fn add_asset_tag(
        &mut self,
        asset_id: i64,
        source: &str,
        namespace: &str,
        value: &str,
    ) -> Result<usize> {
        let Some((norm, display)) = normalize_tag(namespace, value) else {
            return Ok(0); // an empty or punctuation-only value never became a tag
        };
        let tx = self
            .conn
            .transaction()
            .map_err(|e| CoreError::Catalog(format!("tag add begin: {e}")))?;
        let old_concat = tags_concat_in(&tx, asset_id)?;
        tx.execute(
            "INSERT INTO tag (name, norm_name, namespace) VALUES (?1, ?2, ?3)
             ON CONFLICT(norm_name) DO NOTHING",
            params![display, norm, namespace],
        )
        .map_err(|e| CoreError::Catalog(format!("tag upsert: {e}")))?;
        let tag_id: i64 = tx
            .query_row(
                "SELECT id FROM tag WHERE norm_name = ?1",
                params![norm],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("tag lookup: {e}")))?;
        let written = tx
            .execute(
                "INSERT INTO asset_tag (asset_id, tag_id, source, confidence, model_id, created_at)
                 VALUES (?1, ?2, ?3, NULL, NULL, ?4)
                 ON CONFLICT(asset_id, tag_id, source) DO NOTHING",
                params![asset_id, tag_id, source, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("asset_tag insert: {e}")))?;
        refresh_fts(&tx, asset_id, &old_concat)?;
        tx.commit()
            .map_err(|e| CoreError::Catalog(format!("tag add commit: {e}")))?;
        Ok(written)
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
        //
        // **Unless the project-less tray (M20) is holding picks** — then that tray *becomes* this
        // project's, keeping what the user collected before they had anywhere to send it. This is
        // the same move M5's data step made for the legacy shared tray, for the same reason: a
        // shortlist built by hand is the expensive thing on screen, and connecting an editor is
        // the moment it finally has a destination. Dropping it there would punish the user for
        // doing the two steps in the order the app encourages.
        let adopted = self
            .conn
            .execute(
                "UPDATE collection SET project_id = ?1
                  WHERE is_staging = 1 AND project_id IS NULL
                    AND EXISTS (SELECT 1 FROM collection_asset ca WHERE ca.collection_id = collection.id)
                    AND NOT EXISTS (SELECT 1 FROM collection WHERE is_staging = 1 AND project_id = ?1)",
                params![id],
            )
            .map_err(|e| CoreError::Catalog(format!("adopt staging tray: {e}")))?;
        if adopted == 0 {
            self.conn
                .execute(
                    "INSERT INTO collection (uuid, name, is_staging, project_id)
                         SELECT ?1, 'Staging', 1, ?2
                         WHERE NOT EXISTS
                           (SELECT 1 FROM collection WHERE is_staging=1 AND project_id=?2)",
                    params![uuid::Uuid::new_v4().to_string(), id],
                )
                .map_err(|e| CoreError::Catalog(format!("staging tray: {e}")))?;
        }
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

    /// A project's staging tray (`collection.is_staging`, one per project — created empty by
    /// [`Self::register_project`]). `None` when the project has no tray (never registered, or
    /// a pre-tray project). The staging ops (ENGINE-API §6) resolve through this.
    pub fn staging_tray_id(&self, project_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM collection WHERE is_staging = 1 AND project_id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("staging tray lookup: {e}")))
    }

    /// **The** staging tray right now: the active project's, or the project-less one when no
    /// editor is registered (M20). Read-only — `None` means nothing has ever been staged, which
    /// is not the same as an error and must not create a row on a read path.
    ///
    /// This is the single resolver both planes use, and the reason they can no longer disagree:
    /// the shell used to keep its picks in an in-memory `Vec` while ENGINE-API §6 read the
    /// catalog, so `list_staged` and the tray on screen were two different answers to the same
    /// question (ADR-0010).
    pub fn active_staging_tray_id(&self) -> Result<Option<i64>> {
        match self.active_project_id()? {
            Some(p) => self.staging_tray_id(p),
            None => self
                .conn
                .query_row(
                    "SELECT id FROM collection WHERE is_staging = 1 AND project_id IS NULL",
                    [],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| CoreError::Catalog(format!("staging tray lookup: {e}"))),
        }
    }

    /// The same tray, created if it does not exist yet. **Write path only** — the writer thread
    /// owns every call (ARCHITECTURE §5), which is what keeps this from being a second writer.
    ///
    /// A project that predates its tray gets one here rather than erroring: `register_project`
    /// creates trays now, but a catalog written before it did would otherwise have a project you
    /// can never stage into.
    pub fn ensure_active_staging_tray(&self) -> Result<i64> {
        if let Some(id) = self.active_staging_tray_id()? {
            return Ok(id);
        }
        let project = self.active_project_id()?;
        self.conn
            .execute(
                "INSERT INTO collection (uuid, name, is_staging, project_id) VALUES (?1, 'Staging', 1, ?2)",
                params![uuid::Uuid::new_v4().to_string(), project],
            )
            .map_err(|e| CoreError::Catalog(format!("staging tray create: {e}")))?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Empty the current tray, answering how many picks were dropped (UI-SPEC §16.2 `clear`).
    pub fn clear_staging_tray(&self) -> Result<usize> {
        let Some(tray) = self.active_staging_tray_id()? else {
            return Ok(0);
        };
        self.conn
            .execute(
                "DELETE FROM collection_asset WHERE collection_id = ?1",
                params![tray],
            )
            .map_err(|e| CoreError::Catalog(format!("clear staging: {e}")))
    }

    /// The active project's id, if any (ENGINE-API §5.1 — `is_active` is a unique flag).
    pub fn active_project_id(&self) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT id FROM project WHERE is_active = 1 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("active project lookup: {e}")))
    }

    /// Switch which registered Unity editor is active (ENGINE-API §5.1, UI-SPEC §16.1). The
    /// partial unique index guarantees at most one active row — clear first, then set.
    pub fn set_active_project(&self, project_id: i64) -> Result<()> {
        self.conn
            .execute("UPDATE project SET is_active = 0", [])
            .map_err(|e| CoreError::Catalog(format!("project deactivate: {e}")))?;
        let n = self
            .conn
            .execute(
                "UPDATE project SET is_active = 1 WHERE id = ?1",
                params![project_id],
            )
            .map_err(|e| CoreError::Catalog(format!("project activate: {e}")))?;
        if n == 0 {
            return Err(CoreError::Catalog(format!(
                "no registered editor {project_id}"
            )));
        }
        Ok(())
    }

    /// Record what an export run wrote (FEATURES §7.6), so §7.1 can list it and undo it.
    ///
    /// One row per file, taken from what was **actually copied** rather than what was requested:
    /// an export skips in-pack and unreachable picks, and a cancelled run stops part-way. Undo
    /// deletes files, so a record of files that were never written would be a record that points
    /// undo at someone else's.
    pub fn record_exports(
        &self,
        batch_id: &str,
        dest_path: &str,
        files: &[(Option<i64>, String)],
    ) -> Result<usize> {
        let now = unix_now();
        let mut stmt = self
            .conn
            .prepare(
                "INSERT INTO export_record (batch_id, asset_id, dest_path, file_name, exported_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .map_err(|e| CoreError::Catalog(format!("export record prep: {e}")))?;
        let mut n = 0usize;
        for (asset_id, name) in files {
            stmt.execute(params![batch_id, asset_id, dest_path, name, now])
                .map_err(|e| CoreError::Catalog(format!("export record insert: {e}")))?;
            n += 1;
        }
        Ok(n)
    }

    /// The most recent export runs, newest first (FEATURES §7.1 shows four).
    pub fn recent_export_batches(&self, limit: usize) -> Result<Vec<ExportBatch>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT batch_id, dest_path, COUNT(*), MAX(exported_at)
                   FROM export_record
                  GROUP BY batch_id
                  ORDER BY MAX(exported_at) DESC, batch_id DESC
                  LIMIT ?1",
            )
            .map_err(|e| CoreError::Catalog(format!("export list prep: {e}")))?;
        let rows = stmt
            .query_map(params![limit as i64], |r| {
                Ok(ExportBatch {
                    batch_id: r.get(0)?,
                    dest_path: r.get(1)?,
                    files: r.get(2)?,
                    exported_at: r.get(3)?,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("export list query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("export list row: {e}")))
    }

    /// The files one run wrote, as `(dest_path, file_name)` — what undo deletes.
    pub fn export_batch_files(&self, batch_id: &str) -> Result<Vec<(String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT dest_path, file_name FROM export_record WHERE batch_id = ?1")
            .map_err(|e| CoreError::Catalog(format!("export files prep: {e}")))?;
        let rows = stmt
            .query_map(params![batch_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| CoreError::Catalog(format!("export files query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("export files row: {e}")))
    }

    /// Drop a run from the history. Called **after** its files are dealt with, never before: a
    /// record removed first would leave orphaned copies nothing can find again.
    pub fn forget_export_batch(&self, batch_id: &str) -> Result<usize> {
        self.conn
            .execute(
                "DELETE FROM export_record WHERE batch_id = ?1",
                params![batch_id],
            )
            .map_err(|e| CoreError::Catalog(format!("export forget: {e}")))
    }

    /// Point the appraisal queues at what the user is looking at (UI-SPEC §3).
    ///
    /// Costs nothing until the next queue query: it replaces an in-memory list, writes no rows and
    /// touches no file, which is what makes it safe to call on every browse. The queues themselves
    /// are unchanged in every other respect — the same rows, the same budget, the same durability
    /// — only the order they are handed back in moves.
    ///
    /// Callers cap the lists. An unbounded focus would inline every id in the library into eight
    /// `ORDER BY` clauses, which turns a hint into the most expensive part of the query.
    pub fn set_appraise_focus(&mut self, focus: AppraiseFocus) {
        self.appraise_focus = focus;
    }

    /// Re-derive every source's availability from the disk (UI-SPEC §11.1, M18).
    ///
    /// One `is_dir` per source **root**, never per asset: a single missing file is a per-asset fact
    /// and must never mark a whole library unavailable. Roots that resolve stamp `last_seen_at`, so
    /// the UI can say *since when*; roots that do not keep the timestamp they had.
    ///
    /// Nothing is deleted, trashed or hidden by this — the catalog stays fully browsable while the
    /// files are away, which is the entire point of having an index. Callers run it off the UI
    /// thread: a slow network root can take seconds to answer, and a false "unavailable" flash on
    /// every open would be worse than the gap it closes.
    ///
    /// Returns the ids whose state actually changed, so a caller can decide whether to re-render.
    pub fn refresh_source_health(&self) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, root_path, state, watch_enabled FROM import_source")
            .map_err(|e| CoreError::Catalog(format!("source health prep: {e}")))?;
        let rows: Vec<(i64, String, String, bool)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0))
            })
            .map_err(|e| CoreError::Catalog(format!("source health query: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("source health row: {e}")))?;

        let now = unix_now();
        let mut changed = Vec::new();
        for (id, root, was, watching) in rows {
            let present = std::path::Path::new(&root).is_dir();
            // A root that is not there outranks everything: that is the fact stopping files from
            // opening. `paused` is the user's own intent, and only means anything while the source
            // is actually reachable.
            let now_state = match (present, watching) {
                (false, _) => "unavailable",
                (true, false) => "paused",
                (true, true) => "available",
            };
            if present {
                self.conn
                    .execute(
                        "UPDATE import_source SET state = ?1, last_seen_at = ?2 WHERE id = ?3",
                        params![now_state, now, id],
                    )
                    .map_err(|e| CoreError::Catalog(format!("source health update: {e}")))?;
            } else {
                // Keep the timestamp it had — "unavailable since" is the last time it *was* there.
                self.conn
                    .execute(
                        "UPDATE import_source SET state = ?1 WHERE id = ?2",
                        params![now_state, id],
                    )
                    .map_err(|e| CoreError::Catalog(format!("source health update: {e}")))?;
            }
            if was != now_state {
                changed.push(id);
            }
        }
        Ok(changed)
    }

    /// What a relink would achieve, counted **before** it commits (UI-SPEC §11.1).
    ///
    /// A relink that would strand half the library is a thing you see first, so this walks every
    /// asset the source owns and asks whether its backing file resolves under the candidate root.
    /// Two location kinds resolve differently and both must be counted, or the number lies:
    ///
    /// - **loose files** — `<new_root>/<asset_file.rel_path>`, one `exists` each;
    /// - **in-pack assets** — the file that must be there is the `.unitypackage`, not the entry.
    ///   Its `package.package_path` is absolute, so it is re-anchored by swapping the old root
    ///   prefix. Existence is memoised per pack: 5,963 in-pack assets on this library reach ~50
    ///   distinct archives, and stat-ing each asset would turn a preview into a disk crawl.
    ///
    /// Assets already extracted into the workspace cache (`workspace_rel`) do not live under the
    /// root at all, so they are outside the count entirely — calling them "matched" would credit
    /// the new root with files it does not hold, and "unmatched" would threaten files that are
    /// perfectly reachable.
    ///
    /// Read-only, and slow by nature (one `exists` per loose file, on a drive that may be cold).
    /// Callers run it off the UI thread.
    pub fn relink_preview(&self, source_id: i64, new_root: &Path) -> Result<RelinkPreview> {
        let old_root: String = self
            .conn
            .query_row(
                "SELECT root_path FROM import_source WHERE id = ?1",
                params![source_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("relink source lookup: {e}")))?;
        let old_root = PathBuf::from(old_root);

        let mut stmt = self
            .conn
            .prepare(
                "SELECT af.rel_path, p.package_path
                   FROM asset a
                   JOIN asset_file af  ON af.id = a.primary_file_id
                   LEFT JOIN package p ON p.id = a.package_id
                                      AND af.package_entry_id IS NOT NULL
                  WHERE a.source_id = ?1
                    AND af.missing = 0
                    AND af.workspace_rel IS NULL",
            )
            .map_err(|e| CoreError::Catalog(format!("relink preview prep: {e}")))?;
        let rows: Vec<(String, Option<String>)> = stmt
            .query_map(params![source_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| CoreError::Catalog(format!("relink preview query: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("relink preview row: {e}")))?;

        let mut packs: HashMap<String, bool> = HashMap::new();
        let mut matched = 0i64;
        let total = rows.len() as i64;
        for (rel_path, pack_path) in rows {
            let hit = match pack_path {
                Some(pack) => *packs.entry(pack.clone()).or_insert_with(|| {
                    reanchor(Path::new(&pack), &old_root, new_root).is_some_and(|p| p.exists())
                }),
                None => new_root.join(&rel_path).exists(),
            };
            if hit {
                matched += 1;
            }
        }
        Ok(RelinkPreview { matched, total })
    }

    /// Point a source at a new root (UI-SPEC §11.1). Returns the packs re-anchored with it.
    ///
    /// Only two things hold an absolute path: the source root, and every `package.package_path`
    /// underneath it. `asset_file.rel_path` is relative by definition (schema, M1) and is what makes
    /// a relink a rename rather than a re-scan — the identity, tags, stars and import history of
    /// every row survive it.
    ///
    /// A pack that does not sit under the declared old root is left exactly as it is rather than
    /// rewritten to a guess; it will read as unavailable, which is true, instead of pointing
    /// somewhere plausible and wrong.
    ///
    /// Nothing is deleted and no asset row is touched: this is the non-destructive answer to a
    /// moved drive, and the reason the catalog keeps browsing while the files are gone.
    pub fn relink_source(&self, source_id: i64, new_root: &Path) -> Result<usize> {
        let old_root: String = self
            .conn
            .query_row(
                "SELECT root_path FROM import_source WHERE id = ?1",
                params![source_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("relink source lookup: {e}")))?;
        let old_root = PathBuf::from(old_root);
        let new_str = new_root.to_string_lossy().into_owned();

        let mut stmt = self
            .conn
            .prepare("SELECT id, package_path FROM package WHERE source_id = ?1")
            .map_err(|e| CoreError::Catalog(format!("relink packs prep: {e}")))?;
        let packs: Vec<(i64, String)> = stmt
            .query_map(params![source_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(|e| CoreError::Catalog(format!("relink packs query: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("relink packs row: {e}")))?;

        let mut moved = 0usize;
        for (id, path) in packs {
            let Some(next) = reanchor(Path::new(&path), &old_root, new_root) else {
                continue;
            };
            self.conn
                .execute(
                    "UPDATE package SET package_path = ?2 WHERE id = ?1",
                    params![id, next.to_string_lossy()],
                )
                .map_err(|e| CoreError::Catalog(format!("relink pack update: {e}")))?;
            moved += 1;
        }

        // The new root is on disk right now — the user just picked it — so the source is available
        // again and `last_seen_at` is now. Leaving the old state would keep the banner up over
        // files that open.
        self.conn
            .execute(
                "UPDATE import_source
                    SET root_path = ?2, state = 'available', last_seen_at = ?3
                  WHERE id = ?1",
                params![source_id, new_str, unix_now()],
            )
            .map_err(|e| CoreError::Catalog(format!("relink source update: {e}")))?;
        Ok(moved)
    }

    /// The four import states the `✓ in <project>` badge distinguishes (DESIGN-GUIDELINES §7),
    /// for the whole library, in two queries.
    ///
    /// [`Self::already_imported`] answers the same question per asset and is the right shape for a
    /// dup-guard over a handful of picks; a grid of 8k cards asking it per row is 8k statements.
    ///
    /// - **in the active project** — a row in `import_record` joined to `project.is_active`;
    /// - **older copy** — that row's `imported_at` predates the file's `mtime`, i.e. the source has
    ///   changed on disk since it was imported. This is the only actionable state, so it must not
    ///   be inferred from anything softer than the two timestamps;
    /// - **in N others** — imported somewhere, but not here. History, not a warning;
    /// - **never** — absent from the map entirely, which is the common case and must cost nothing.
    ///
    /// No active project is not an error: it is a library with nothing to compare against, and the
    /// badge simply does not draw.
    pub fn import_states(&self) -> Result<std::collections::HashMap<i64, ImportState>> {
        let mut out: std::collections::HashMap<i64, ImportState> = std::collections::HashMap::new();

        let mut active = self
            .conn
            .prepare(
                "SELECT ir.asset_id,
                        MAX(CASE WHEN f.mtime IS NOT NULL AND f.mtime > ir.imported_at
                                 THEN 1 ELSE 0 END)
                 FROM import_record ir
                 JOIN project p ON p.id = ir.project_id AND p.is_active = 1
                 LEFT JOIN asset_file f ON f.asset_id = ir.asset_id
                 GROUP BY ir.asset_id",
            )
            .map_err(|e| CoreError::Catalog(format!("import states prep: {e}")))?;
        let rows = active
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? != 0)))
            .map_err(|e| CoreError::Catalog(format!("import states query: {e}")))?;
        for r in rows {
            let (id, stale) =
                r.map_err(|e| CoreError::Catalog(format!("import states row: {e}")))?;
            let e = out.entry(id).or_default();
            e.in_active = true;
            e.stale = stale;
        }

        let mut others = self
            .conn
            .prepare(
                "SELECT ir.asset_id, COUNT(DISTINCT ir.project_id)
                 FROM import_record ir
                 JOIN project p ON p.id = ir.project_id AND p.is_active = 0
                 GROUP BY ir.asset_id",
            )
            .map_err(|e| CoreError::Catalog(format!("import others prep: {e}")))?;
        let rows = others
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
            .map_err(|e| CoreError::Catalog(format!("import others query: {e}")))?;
        for r in rows {
            let (id, n) = r.map_err(|e| CoreError::Catalog(format!("import others row: {e}")))?;
            out.entry(id).or_default().others = n;
        }
        Ok(out)
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

    /// Packs among `asset_ids` whose render pipeline disagrees with the target project's —
    /// the **`pipeline_mismatch`** import advisory (ENGINE-API §5.4). Grouped per pack, because
    /// that is the sentence a user gets told: "this pack targets Built-in; your project runs URP".
    ///
    /// Advisory, never an error, and deliberately silent in every case where the answer is not
    /// known to be a mismatch:
    /// - the project's pipeline is unknown (an older plugin registration did not report one),
    /// - the pack's is unknown (`pipeline::resolve` found nothing to go on), or
    /// - the pack is `mixed` — it ships a variant per pipeline, so whatever the project runs is
    ///   in there.
    ///
    /// Material *conversion* stays out of scope (DATA-MODEL §12): we warn, we don't rewrite.
    pub fn pipeline_mismatches(
        &self,
        asset_ids: &[i64],
        project_id: i64,
    ) -> Result<Vec<PipelineAdvisory>> {
        if asset_ids.is_empty() {
            return Ok(Vec::new());
        }
        let Some(project_pipeline): Option<String> = self
            .conn
            .query_row(
                "SELECT pipeline FROM project WHERE id = ?1",
                params![project_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("project pipeline: {e}")))?
            .flatten()
        else {
            return Ok(Vec::new());
        };
        let placeholders = vec!["?"; asset_ids.len()].join(",");
        let mut stmt = self
            .conn
            .prepare(&format!(
                "SELECT p.id, p.name, p.pipeline, a.id
                   FROM asset a
                   JOIN package p ON p.id = a.package_id
                  WHERE a.id IN ({placeholders})
                    AND p.pipeline IS NOT NULL
                    AND p.pipeline NOT IN ('mixed', ?{n})
                  ORDER BY p.id, a.id",
                n = asset_ids.len() + 1
            ))
            .map_err(|e| CoreError::Catalog(format!("pipeline advisory prep: {e}")))?;
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = asset_ids
            .iter()
            .map(|id| Box::new(*id) as Box<dyn rusqlite::ToSql>)
            .collect();
        args.push(Box::new(project_pipeline.clone()));
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| CoreError::Catalog(format!("pipeline advisory query: {e}")))?;
        let mut out: Vec<PipelineAdvisory> = Vec::new();
        for row in rows {
            let (pkg_id, name, pipeline, asset_id) =
                row.map_err(|e| CoreError::Catalog(format!("pipeline advisory row: {e}")))?;
            match out.last_mut() {
                Some(last) if last.package_id == pkg_id => last.asset_ids.push(asset_id),
                _ => out.push(PipelineAdvisory {
                    package_id: pkg_id,
                    package: name,
                    package_pipeline: pipeline,
                    project_pipeline: project_pipeline.clone(),
                    asset_ids: vec![asset_id],
                }),
            }
        }
        Ok(out)
    }

    /// Everything a pack whose `pipeline` is still unknown states about itself in *names*:
    /// one row per `(package_id, package name, entry pathname)`, with a NULL pathname for a pack
    /// whose manifest was never recorded. Feeds the backfill classifier
    /// (`lodestash_ingest::pipeline`) — the harvest only runs at import, so packs already in the
    /// catalog would otherwise stay unknown forever and never raise the §5.4 advisory.
    ///
    /// Scoped to `pipeline IS NULL`, so it shrinks as packs are classified; a pack that genuinely
    /// names no pipeline is re-examined each run, which is a tokenise over strings already in
    /// memory.
    pub fn packages_needing_pipeline(&self) -> Result<Vec<(i64, String, Option<String>)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.id, p.name, e.pathname
                   FROM package p
                   LEFT JOIN package_entry e ON e.package_id = p.id
                  WHERE p.pipeline IS NULL
                  ORDER BY p.id",
            )
            .map_err(|e| CoreError::Catalog(format!("pipeline backfill prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(|e| CoreError::Catalog(format!("pipeline backfill query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("pipeline backfill row: {e}")))
    }

    /// Everything needed to pull an in-pack asset's bytes out of its archive:
    /// `(package_path, tar_offset, size_bytes, extension)` — the same locator the grid's
    /// thumbnail route uses (INGESTION §2 "Surgical pull").
    ///
    /// `None` unless the asset is genuinely still in the archive: a materialized one has a real
    /// file and an offset-less entry has nothing to seek to.
    pub fn in_pack_locator(&self, asset_id: i64) -> Result<Option<(String, i64, i64, String)>> {
        self.conn
            .query_row(
                "SELECT p.package_path, pe.tar_offset, pe.size_bytes, af.ext
                   FROM asset a
                   JOIN asset_file af    ON af.id = a.primary_file_id
                   JOIN package_entry pe ON pe.id = af.package_entry_id
                   JOIN package p        ON p.id = pe.package_id
                  WHERE a.id = ?1
                    AND af.workspace_rel IS NULL
                    AND pe.tar_offset IS NOT NULL
                    AND pe.size_bytes IS NOT NULL",
                params![asset_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Option<String>>(3)?.unwrap_or_default(),
                    ))
                },
            )
            .optional()
            .map_err(|e| CoreError::Catalog(format!("in_pack_locator: {e}")))
    }

    /// Where an **in-pack** asset lives inside its archive: `(pathname, package_path)` —
    /// the `Assets/…` path the pack declared for it, and the `.unitypackage` holding it
    /// (UI-SPEC §13.9).
    ///
    /// `None` for anything on disk or in the workspace. That is the point of the row: an in-pack
    /// asset has no file to reveal in Explorer, so "where is this?" has no answer anywhere else —
    /// and on this library 5,963 of 7,691 assets are in that state.
    pub fn in_pack_location(&self, asset_id: i64) -> Result<Option<(String, String)>> {
        self.conn
            .query_row(
                "SELECT pe.pathname, p.package_path
                   FROM asset_file af
                   JOIN package_entry pe ON pe.id = af.package_entry_id
                   JOIN package p        ON p.id = pe.package_id
                   JOIN asset a          ON a.primary_file_id = af.id
                  WHERE a.id = ?1 AND af.workspace_rel IS NULL",
                params![asset_id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map(|row| row.and_then(|(name, pack)| name.map(|n| (n, pack))))
            .map_err(|e| CoreError::Catalog(format!("in_pack_location: {e}")))
    }

    /// The licence that actually applies to an asset: its own override, else its pack's
    /// (DATA-MODEL §9). Returns `(license, license_url)`; `None` when neither states one.
    ///
    /// The two fall back **independently**: a pack states the EULA and its URL, an asset can
    /// override the licence without knowing a URL, and pairing that asset's licence with the
    /// pack's URL would cite the wrong document.
    pub fn effective_license(&self, asset_id: i64) -> Result<Option<(String, Option<String>)>> {
        self.conn
            .query_row(
                "SELECT COALESCE(a.license, p.license), COALESCE(a.license_url, p.license_url)
                   FROM asset a
                   LEFT JOIN package p ON p.id = a.package_id
                  WHERE a.id = ?1",
                params![asset_id],
                |r| {
                    Ok((
                        r.get::<_, Option<String>>(0)?,
                        r.get::<_, Option<String>>(1)?,
                    ))
                },
            )
            .optional()
            .map(|row| row.and_then(|(lic, url)| lic.map(|l| (l, url))))
            .map_err(|e| CoreError::Catalog(format!("effective_license: {e}")))
    }

    /// Record which render pipeline a pack targets (DATA-MODEL §12), harvested from what the
    /// pack names — see `lodestash_ingest::pipeline`.
    ///
    /// Same "set only if we learned something" rule as [`Catalog::set_package_metadata`]: `None`
    /// means the harvest could not tell, which must not erase an answer an earlier pass found.
    pub fn set_package_pipeline(&self, package_id: i64, pipeline: Option<&str>) -> Result<()> {
        self.conn
            .execute(
                "UPDATE package SET pipeline = COALESCE(?2, pipeline) WHERE id = ?1",
                params![package_id, pipeline],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("set_package_pipeline: {e}")))
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
        // `tag.color` is later interpolated into a CSS `var()` on the card — a hand-edited
        // payload could smuggle arbitrary CSS through it, so the server accepts only the six
        // palette tokens the UI offers (TraitsScreen's swatches) or a strict `#RRGGBB` hex
        // (the legacy egui build sends hex; it degrades to the default chip in the Svelte
        // render, where `var(#rrggbb)` is inert). Anything else is a reject, not a storage.
        const TRAIT_COLOR_TOKENS: &[&str] = &[
            "--ls-type-model",
            "--ls-type-sound",
            "--ls-type-image",
            "--ls-type-material",
            "--ls-type-sprite",
            "--ls-type-font",
        ];
        let valid = |c: &str| {
            TRAIT_COLOR_TOKENS.contains(&c)
                || (c.len() == 7
                    && c.starts_with('#')
                    && c[1..].chars().all(|ch| ch.is_ascii_hexdigit()))
        };
        if let Some(c) = color {
            if !valid(c) {
                return Err(CoreError::Catalog(format!(
                    "set_trait_style: unknown colour token {c:?} — the six-swatch palette or #RRGGBB is the contract"
                )));
            }
        }
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
        self.assets_needing_tags_within(limit, None)
    }

    /// The tag queue, optionally **restricted to an explicit set** of assets.
    ///
    /// `identify_asset(ids)` needs this. Without it the op flagged the requested assets and then
    /// ran a generic bounded pass, which takes the front of the *global* queue — 8,155 rows deep
    /// on the reference library, ordered by the appraise focus — so the three assets an agent
    /// asked about were nowhere near it and came back untouched while the call reported success.
    pub fn assets_needing_tags_within(
        &self,
        limit: usize,
        only: Option<&[i64]>,
    ) -> Result<Vec<VisualEmbedJob>> {
        // Inlined rather than bound: an id list is a variable-length set and these are i64s the
        // caller already holds, never user text.
        let scope = match only {
            Some(ids) if !ids.is_empty() => format!(
                "AND a.id IN ({})",
                ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",")
            ),
            // An explicit *empty* set asks for nothing, which is not the same as "everything".
            Some(_) => "AND 0".to_string(),
            None => String::new(),
        };
        let mut stmt = self
            .conn
            .prepare(&format!(
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
                 {scope}
                 {order} LIMIT ?1",
                order = self.appraise_focus.order_by("a.id")
            ))
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
                "SELECT 1, r.kind, r.state, r.declared_path, t.id, t.name, mt.code, r.id,
                        CASE WHEN t.id IS NULL THEN NULL
                             WHEN t.package_id IS NOT NULL
                              AND t.package_id = (SELECT package_id FROM asset WHERE id = ?1)
                             THEN 1 ELSE 0 END
                   FROM asset_relationship r
                   LEFT JOIN asset t      ON t.id = r.to_asset_id AND t.deleted_at IS NULL
                   LEFT JOIN media_type mt ON mt.id = t.media_type_id
                  WHERE r.from_asset_id = ?1
                 UNION ALL
                 SELECT 0, r.kind, r.state, r.declared_path, f.id, f.name, mt.code, r.id,
                        CASE WHEN f.package_id IS NOT NULL
                              AND f.package_id = (SELECT package_id FROM asset WHERE id = ?1)
                             THEN 1 ELSE 0 END
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
                    edge_id: r.get(7)?,
                    same_pack: r.get::<_, Option<i64>>(8)?.map(|n| n == 1),
                })
            })
            .map_err(|e| CoreError::Catalog(format!("related query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("related row: {e}")))
    }

    /// Mark a pack's extracted bytes as worth keeping (M23). See the migration for why this is
    /// not `pinned`.
    pub fn set_package_keep_cached(&self, package_id: i64, keep: bool) -> Result<()> {
        self.conn
            .execute(
                "UPDATE package SET keep_cached = ?2 WHERE id = ?1",
                params![package_id, keep as i64],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("set_package_keep_cached: {e}")))
    }

    /// Every `workspace_rel` a **live** asset still resolves through — the set the workspace
    /// sweep must not touch.
    ///
    /// Live means not soft-deleted: a trashed asset is recoverable (DATA-MODEL §8), so its bytes
    /// are not garbage. Returned as the stored relative paths; the caller resolves them against
    /// its workspace dir.
    pub fn referenced_workspace_paths(&self) -> Result<std::collections::HashSet<String>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT DISTINCT af.workspace_rel
                   FROM asset_file af
                   JOIN asset a ON a.id = af.asset_id
                  WHERE af.workspace_rel IS NOT NULL AND a.deleted_at IS NULL",
            )
            .map_err(|e| CoreError::Catalog(format!("workspace refs prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(|e| CoreError::Catalog(format!("workspace refs query: {e}")))?;
        rows.collect::<std::result::Result<std::collections::HashSet<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("workspace refs row: {e}")))
    }

    /// Packs holding extracted bytes, for the Settings storage list and the sweep: id, name,
    /// archive path, whether the user marked it kept, and how many workspace files it holds.
    ///
    /// Ordered by file count descending — "what is costing me the most" is the question this list
    /// exists to answer.
    pub fn workspace_packs(&self) -> Result<Vec<WorkspacePack>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT p.id, p.name, p.package_path, p.keep_cached, COUNT(af.id)
                   FROM package p
                   JOIN asset a       ON a.package_id = p.id AND a.deleted_at IS NULL
                   JOIN asset_file af ON af.asset_id = a.id AND af.workspace_rel IS NOT NULL
                  GROUP BY p.id
                  ORDER BY COUNT(af.id) DESC, p.name",
            )
            .map_err(|e| CoreError::Catalog(format!("workspace packs prep: {e}")))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(WorkspacePack {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    package_path: r.get(2)?,
                    keep_cached: r.get::<_, i64>(3)? == 1,
                    files: r.get::<_, i64>(4)? as usize,
                })
            })
            .map_err(|e| CoreError::Catalog(format!("workspace packs query: {e}")))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| CoreError::Catalog(format!("workspace packs row: {e}")))
    }

    /// How many of a pack's recorded entries a candidate archive still accounts for, by GUID —
    /// the evidence behind §15's **Retarget…** (mirrors `relink_preview` for a moved source root).
    ///
    /// Returns `(matched, total)`. A low match is **not** an error: a publisher updating a pack
    /// legitimately reshuffles GUIDs, so the count is shown and the user decides.
    pub fn retarget_match(
        &self,
        package_id: i64,
        candidate_guids: &[String],
    ) -> Result<(usize, usize)> {
        let total: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM package_entry WHERE package_id = ?1",
                params![package_id],
                |r| r.get(0),
            )
            .map_err(|e| CoreError::Catalog(format!("retarget total: {e}")))?;
        if total == 0 || candidate_guids.is_empty() {
            return Ok((0, total as usize));
        }
        let have: std::collections::HashSet<&str> =
            candidate_guids.iter().map(String::as_str).collect();
        let mut stmt = self
            .conn
            .prepare("SELECT guid FROM package_entry WHERE package_id = ?1")
            .map_err(|e| CoreError::Catalog(format!("retarget guids prep: {e}")))?;
        let rows = stmt
            .query_map(params![package_id], |r| r.get::<_, String>(0))
            .map_err(|e| CoreError::Catalog(format!("retarget guids query: {e}")))?;
        let mut matched = 0usize;
        for g in rows {
            let g = g.map_err(|e| CoreError::Catalog(format!("retarget guid row: {e}")))?;
            if have.contains(g.as_str()) {
                matched += 1;
            }
        }
        Ok((matched, total as usize))
    }

    /// Point a pack at a different archive on disk (§15 Retarget…). Only `package_path` moves —
    /// every asset, entry locator and extracted byte stays exactly where it is, because what the
    /// rows describe has not changed, only where the bytes can be re-read from.
    pub fn retarget_package(&self, package_id: i64, new_path: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE package SET package_path = ?2 WHERE id = ?1",
                params![package_id, new_path],
            )
            .map(|_| ())
            .map_err(|e| CoreError::Catalog(format!("retarget_package: {e}")))
    }

    /// Mute a dependency edge — UI-SPEC §13.6's **⊘ Ignore link**.
    ///
    /// The row is kept, not deleted, for two reasons: a deleted edge is re-derived from the file
    /// on the next dependency scan (`record_dependencies` replaces a harvest wholesale), and the
    /// panel needs somewhere to offer **restore** — an ignore with no way back is a one-way door
    /// over a parse result the user may have misread. `to_asset_id` is cleared so a muted edge
    /// stops counting as a dependency anywhere else (export closure, the LINKS filter).
    pub fn ignore_relationship(&self, edge_id: i64) -> Result<bool> {
        self.conn
            .execute(
                "UPDATE asset_relationship SET state = 'ignored', to_asset_id = NULL
                  WHERE id = ?1 AND state <> 'ignored'",
                params![edge_id],
            )
            .map(|n| n > 0)
            .map_err(|e| CoreError::Catalog(format!("ignore_relationship: {e}")))
    }

    /// Undo an ⊘ Ignore. Back to `missing`, then the ordinary late-binding pass
    /// ([`Self::resolve_pending_relationships`]) re-resolves it if the target is catalogued —
    /// so restoring never has to guess what the edge pointed at.
    pub fn restore_relationship(&self, edge_id: i64) -> Result<bool> {
        let changed = self
            .conn
            .execute(
                "UPDATE asset_relationship SET state = 'missing'
                  WHERE id = ?1 AND state = 'ignored'",
                params![edge_id],
            )
            .map_err(|e| CoreError::Catalog(format!("restore_relationship: {e}")))?;
        if changed > 0 {
            self.resolve_pending_relationships()?;
        }
        Ok(changed > 0)
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
                    AND state <> 'ignored'
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

    /// Store the primary file's perceptual hash (dHash, DATA-MODEL §5) — the near-duplicate key.
    /// Hex-encoded 64 bits; `None` clears it (e.g. the file changed and re-decoding failed).
    pub fn set_perceptual_hash(&self, asset_id: i64, hash: Option<[u8; 8]>) -> Result<()> {
        let hex = hash.map(|h| h.iter().map(|b| format!("{b:02x}")).collect::<String>());
        self.conn
            .execute(
                "UPDATE asset_file SET perceptual_hash = ?2
                 WHERE id = (SELECT primary_file_id FROM asset WHERE id = ?1)",
                params![asset_id, hex],
            )
            .map_err(|e| CoreError::Catalog(format!("perceptual hash: {e}")))?;
        Ok(())
    }

    /// All perceptual hashes, keyed by asset id (DATA-MODEL §5) — the near-duplicate review reads
    /// these once per library load, like every other facet.
    pub fn perceptual_hashes(&self) -> Result<std::collections::HashMap<i64, [u8; 8]>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT a.id, af.perceptual_hash
                 FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE af.perceptual_hash IS NOT NULL",
            )
            .map_err(|e| CoreError::Catalog(format!("phash prep: {e}")))?;
        let mapped = stmt
            .query_map([], |r| {
                let hex: String = r.get(1)?;
                let bytes: [u8; 8] = hex
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|c| {
                        std::str::from_utf8(c)
                            .ok()
                            .and_then(|s| u8::from_str_radix(s, 16).ok())
                            .unwrap_or(0)
                    })
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap_or([0u8; 8]);
                Ok((r.get(0)?, bytes))
            })
            .map_err(|e| CoreError::Catalog(format!("phash query: {e}")))?;
        mapped
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| CoreError::Catalog(format!("phash row: {e}")))
    }

    /// The catalog's last-known (size, mtime) per loose-file rel_path for one source root — the
    /// crawl's incremental diff (`crawler::needs_rehash`, DATA-MODEL §6). The writer uses this
    /// for watch-rescans and the desktop for re-adding a folder: unchanged files skip BLAKE3.
    pub fn known_sizes_for_source(
        &self,
        root: &str,
    ) -> Result<std::collections::HashMap<String, (u64, i64)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT af.rel_path, af.size_bytes, af.mtime
                 FROM asset_file af
                 JOIN asset a ON a.id = af.asset_id
                 JOIN import_source s ON s.id = a.source_id
                 WHERE s.root_path = ?1 AND a.package_id IS NULL AND a.status != 'trashed'",
            )
            .map_err(|e| CoreError::Catalog(format!("known sizes prep: {e}")))?;
        let mapped = stmt
            .query_map(params![root], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get::<_, i64>(1)? as u64, r.get::<_, i64>(2)?),
                ))
            })
            .map_err(|e| CoreError::Catalog(format!("known sizes query: {e}")))?;
        mapped
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| CoreError::Catalog(format!("known sizes row: {e}")))
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
    /// `attempted` is what this pass has already tried. A model whose preview generation *fails*
    /// writes no `asset_preview` row, so it stays in this queue; without the exclusion a drain
    /// would re-attempt the same broken batch on every iteration and never reach the models
    /// behind it. The caller adds every id it looks at, successful or not.
    ///
    /// It is a **set**, not the `a.id > cursor` this used to be: an id cursor only skips a failure
    /// while the queue is ordered by ascending id, and since D3b (UI-SPEC §3) the order is whatever
    /// the user is looking at. A cursor under a reordered queue silently returns nothing, which
    /// looks exactly like "the backlog is drained".
    pub fn assets_needing_preview(
        &self,
        attempted: &[i64],
        limit: usize,
    ) -> Result<Vec<VisualEmbedJob>> {
        let mut stmt = self
            .conn
            .prepare(&format!(
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
                   {skip}
                   AND a.deleted_at IS NULL
                   AND af.missing = 0
                 {order} LIMIT ?1",
                skip = if attempted.is_empty() {
                    String::new()
                } else {
                    format!(
                        "AND a.id NOT IN ({})",
                        attempted
                            .iter()
                            .map(|i| i.to_string())
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                },
                order = self.appraise_focus.order_by("a.id")
            ))
            .map_err(|e| CoreError::Catalog(format!("preview queue prep: {e}")))?;
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

    /// The texture a **material** paints with — one hop, `material --uses_texture--> image`.
    ///
    /// The model variant above needs two hops because a model reaches its texture *through* a
    /// material; a material is already there. Keeping them separate makes the hop count explicit
    /// at each call site rather than a parameter someone gets wrong.
    pub fn material_albedo_texture(&self, material_id: i64) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "SELECT MIN(t.id)
                   FROM asset_relationship x
                   JOIN asset t       ON t.id = x.to_asset_id
                   JOIN media_type mt ON mt.id = t.media_type_id
                  WHERE x.from_asset_id = ?1
                    AND x.kind = 'uses_texture' AND x.state = 'resolved'
                    AND t.deleted_at IS NULL
                    AND mt.code IN ('image', 'texture', 'sprite')",
                params![material_id],
                |r| r.get::<_, Option<i64>>(0),
            )
            .optional()
            .map(Option::flatten)
            .map_err(|e| CoreError::Catalog(format!("material_albedo_texture: {e}")))
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
                        tags: Vec::new(),
                    })
                })
                .optional()
                .map_err(|e| CoreError::Catalog(format!("summary query: {e}")))?;
            if let Some(mut row) = row {
                // One small query per row rather than a join: `tags_for_asset` already
                // exists and the caller has just ranked these ids, so the set is a page,
                // not the library. Tag read failure is not summary failure.
                row.tags = self.tags_for_asset(row.id).unwrap_or_default();
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

/// One row of the Collections screen (DATA-MODEL §9): a pinned loadout and its live member
/// count. The staging tray (`is_staging = 1`) is the shortlist and never appears here.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CollectionRow {
    pub id: i64,
    pub name: String,
    pub asset_count: i64,
}

// ---------------------------------------------------------------------------
// Stats Tag Graph (UI-SPEC §8.2), LINKS edge set (DATA-MODEL §10), per-source
// blacklist (UI-SPEC §11.1 / DATA-MODEL §10), Mapping-tab authoring (UI-SPEC §9.2),
// undo op-log (UI-SPEC §2 / DESIGN-GUIDELINES §7, L5) and the collection membership
// toggle (UI-SPEC §6).
// ---------------------------------------------------------------------------

/// One node of the Stats Tag Graph (UI-SPEC §8.2): a tag with its usage count. The
/// force-directed canvas draws a gold node per entry, radius ∝ `usage_count`, labeled.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TagGraphNode {
    pub tag_id: i64,
    pub namespace: String,
    pub value: String,
    pub usage_count: i64,
}

/// One co-occurrence edge of the Stats Tag Graph (UI-SPEC §8.2): two tags that appear
/// together on at least one asset, `weight` = the number of assets bearing both.
/// `tag_a < tag_b` (by tag id), so each unordered pair appears exactly once.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TagGraphEdge {
    pub tag_a: i64,
    pub tag_b: i64,
    pub weight: i64,
}

/// The full Stats Tag Graph payload (UI-SPEC §8.2): tag nodes, tag↔tag co-occurrence
/// edges, and the header's `tags · links · assets` counts. `asset_count` is the number
/// of live assets carrying at least one tag (the identified library the canvas draws as
/// small asset nodes); `link_count` is the number of resolved `asset_relationship`
/// edges between live assets — the dashed arcane edges rendered on top.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TagGraph {
    pub nodes: Vec<TagGraphNode>,
    pub edges: Vec<TagGraphEdge>,
    pub asset_count: i64,
    pub link_count: i64,
}

/// One resolved asset→asset edge (DATA-MODEL §10) — the rows behind the ⚑ Filters LINKS
/// chips and the Stats Tag Graph's dashed arcane dependency arcs (UI-SPEC §8.2).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssetEdge {
    pub from: i64,
    pub to: i64,
    pub kind: String,
    pub source: String,
}

/// One per-source blacklist entry (DATA-MODEL §10, UI-SPEC §11.1): `kind` is `folder`
/// (the crawler prunes that source-relative subtree) or `asset` (it drops that single
/// source-relative path / bare file name). Stored as JSON in `import_source.blacklist`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceExclude {
    pub kind: String,
    pub value: String,
}

/// The `import_source.blacklist` column as a decoded list — `None`/empty means no
/// exclusions (a fresh source has no blacklist at all).
fn source_excludes_in(conn: &Connection, source_id: i64) -> Result<Vec<SourceExclude>> {
    // `blacklist` starts NULL on a fresh source — read Option<String> and flatten, so both
    // "no row" and "NULL column" come back as an empty list.
    let raw: Option<Option<String>> = conn
        .query_row(
            "SELECT blacklist FROM import_source WHERE id = ?1",
            params![source_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(|e| CoreError::Catalog(format!("source blacklist read: {e}")))?;
    match raw.flatten() {
        None => Ok(Vec::new()),
        Some(s) if s.trim().is_empty() => Ok(Vec::new()),
        Some(s) => serde_json::from_str(&s)
            .map_err(|e| CoreError::Catalog(format!("source blacklist parse: {e}"))),
    }
}

/// Persist a source's full exclusion list as the JSON column.
fn write_source_excludes(conn: &Connection, source_id: i64, list: &[SourceExclude]) -> Result<()> {
    let json = serde_json::to_string(list)?;
    conn.execute(
        "UPDATE import_source SET blacklist = ?2 WHERE id = ?1",
        params![source_id, json],
    )
    .map_err(|e| CoreError::Catalog(format!("source blacklist write: {e}")))?;
    // An UPDATE against a missing source_id touches zero rows — surface it as an error
    // instead of silently pretending the blacklist was written.
    let exists = conn
        .query_row(
            "SELECT 1 FROM import_source WHERE id = ?1",
            params![source_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| CoreError::Catalog(format!("source check: {e}")))?;
    if exists.is_none() {
        return Err(CoreError::Catalog(format!(
            "write_source_excludes: no such source {source_id}"
        )));
    }
    Ok(())
}

/// The absolute root of one source, for syncing a blacklist edit into the app config the
/// crawler actually honours. `None` when the source id doesn't exist.
fn source_root_in(conn: &Connection, source_id: i64) -> Result<Option<String>> {
    conn.query_row(
        "SELECT root_path FROM import_source WHERE id = ?1",
        params![source_id],
        |r| r.get(0),
    )
    .optional()
    .map_err(|e| CoreError::Catalog(format!("source root read: {e}")))
}

/// One row of the Mapping tab's taxonomy switcher (UI-SPEC §9.2): a mapping library, its
/// version (`v<version>` readout), and whether it is the active one.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TaxonomyRow {
    pub id: i64,
    pub name: String,
    pub version: i64,
    pub is_active: bool,
}

/// One alias chip of a category's detail card (UI-SPEC §9.2 ALIASES).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CategoryAliasRow {
    pub id: i64,
    pub alias: String,
}

/// One undo-log row (UI-SPEC §2 "Ctrl/Cmd-Z" / DESIGN-GUIDELINES §7, L5): the op kind plus
/// the JSON payload — a pre-image or the id needed to invert — that the corresponding
/// revert consumes. Ordered by `id` (append order); the tail is managed by
/// [`Catalog::recent_ops`] / [`Catalog::delete_ops`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OpLogRow {
    pub id: i64,
    pub op_kind: String,
    pub payload_json: String,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------------------------
// Domain modules, split out 2026-08-22 (see each file's header). Only whole `impl` blocks moved;
// the types they return stay here because more than one module uses them.
//
// The two large `impl Catalog` blocks above stay too: the 94 functions in the second do not fall
// into clean domains without a judgement pass over each one, and a bad split is worse than none.

mod collections;
mod excludes;
mod graph;
mod oplog;
mod taxonomy;
#[cfg(test)]
mod tests_catalog;
#[cfg(test)]
mod tests_slices;
