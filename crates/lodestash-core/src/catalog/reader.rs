//! The read-only catalog connection as a type (v2 spec §4.1, plan Task 1.3).

use super::*;

/// The connection that writes: the only one that migrates, and — by convention until the engine
/// owns it (v2 spec §4.1) — held by the one writer thread.
pub type CatalogWriter = Catalog;

/// A catalog connection that can only read — what the agent planes hold (v2 spec §4.1).
///
/// The file is opened `SQLITE_OPEN_READ_ONLY`, so a write would fail at run time anyway; this type
/// makes it fail at compile time. It exposes exactly the read methods listed in `reads!` below, by
/// explicit delegation — not `Deref<Target = Catalog>`, which would hand out every writing method
/// with it. A read the agent planes need and this list lacks is added to the list; a method that
/// writes never is.
///
/// A read compiles:
///
/// ```no_run
/// # fn f(r: &lodestash_core::catalog::CatalogReader) -> lodestash_core::error::Result<()> {
/// let packs = r.list_packages()?;
/// # let _ = packs; Ok(()) }
/// ```
///
/// A write does not:
///
/// ```compile_fail,E0599
/// # fn f(r: &lodestash_core::catalog::CatalogReader) -> lodestash_core::error::Result<()> {
/// r.set_favorite(&[1], true)?;
/// # Ok(()) }
/// ```
pub struct CatalogReader(Catalog);

/// Forward each listed method to the [`Catalog`] method of the same name and signature.
macro_rules! reads {
    ($( fn $name:ident(&self $(, $arg:ident: $ty:ty)* $(,)?) -> $ret:ty; )*) => {
        impl CatalogReader {
            $(
                #[doc = concat!("Read-only [`Catalog::", stringify!($name), "`].")]
                pub fn $name(&self $(, $arg: $ty)*) -> $ret {
                    self.0.$name($($arg),*)
                }
            )*
        }
    };
}

reads! {
    // Identity
    fn schema_version(&self) -> Result<u32>;
    fn catalog_id(&self) -> Result<String>;

    // One asset
    fn asset_summaries(&self, ids: &[i64]) -> Result<Vec<AssetSummaryRow>>;
    fn asset_description(&self, asset_id: i64) -> Result<Option<String>>;
    fn asset_tag_rows(&self, asset_id: i64) -> Result<Vec<AssetTagRow>>;
    fn tags_for_asset(&self, asset_id: i64) -> Result<Vec<TagAssignment>>;
    fn annotation_revision(&self, asset_id: i64) -> Result<String>;
    fn analysis_revision(&self, asset_id: i64) -> Result<Option<String>>;
    fn dependency_closure(&self, seeds: &[i64]) -> Result<Vec<i64>>;
    fn embedding_of(&self, asset_id: i64) -> Result<Option<(String, Vec<f32>)>>;
    fn in_pack_entry(&self, asset_id: i64) -> Result<Option<InPackEntry>>;
    fn in_pack_locator(&self, asset_id: i64) -> Result<Option<(String, i64, i64, String)>>;
    fn material_albedo_texture(&self, material_id: i64) -> Result<Option<i64>>;
    fn model_slot_texture_paths(
        &self,
        asset_id: i64,
        workspace_dir: Option<&Path>,
    ) -> Result<std::collections::BTreeMap<u32, PathBuf>>;

    // Walks, samples and search
    fn recent_asset_ids(&self, limit: usize) -> Result<Vec<i64>>;
    fn max_asset_id(&self) -> Result<i64>;
    fn browse_asset_ids(
        &self,
        scope: &AssetScope,
        after: i64,
        upper: i64,
        limit: usize,
    ) -> Result<Vec<i64>>;
    fn sample_asset_ids(&self, scope: &AssetScope, limit: usize) -> Result<Vec<i64>>;
    fn recent_scoped_asset_ids(&self, scope: &AssetScope, limit: usize) -> Result<Vec<i64>>;
    fn search_assets_scoped(
        &self,
        query: &str,
        scope: &AssetScope,
        limit: usize,
    ) -> Result<Option<Vec<i64>>>;
    fn hybrid_search_scoped(
        &self,
        query: &str,
        spaces: &[(&str, &[f32])],
        scope: &AssetScope,
        limit: usize,
    ) -> Result<Vec<i64>>;
    fn embedding_snapshot(&self, model_id: &str) -> Result<std::sync::Arc<EmbeddingSnapshot>>;
    fn eligible_for_search(&self, media_type_id: Option<i64>) -> Result<HashSet<i64>>;
    fn media_type_id(&self, code: &str) -> Result<Option<i64>>;
    fn tag_usage(&self, limit: usize) -> Result<Vec<(String, String, i64)>>;

    // Libraries and packages
    fn libraries_page(&self, after: i64, limit: usize) -> Result<Vec<LibraryRow>>;
    fn source_exists(&self, source_id: i64) -> Result<bool>;
    fn source_roots(&self) -> Result<Vec<String>>;
    fn package_source_id(&self, package_id: i64) -> Result<Option<i64>>;
    fn list_packages(&self) -> Result<Vec<PackageListing>>;

    // Projects, staging and collections
    fn list_projects(&self) -> Result<Vec<ProjectRow>>;
    fn active_staging_tray_id(&self) -> Result<Option<i64>>;
    fn already_imported(&self, asset_ids: &[i64], project_id: i64) -> Result<Vec<i64>>;
    fn pipeline_mismatches(
        &self,
        asset_ids: &[i64],
        project_id: i64,
    ) -> Result<Vec<PipelineAdvisory>>;
    fn list_collections(&self) -> Result<Vec<CollectionRow>>;
    fn collection_members(&self, id: i64) -> Result<Vec<i64>>;

    // Receipts of agent writes
    fn tag_request_receipt(&self, request_id: &str) -> Result<Option<TagReceipt>>;
}

impl Catalog {
    /// Open an existing catalog **read-only** — the agent planes' own connection (v1 plan S11).
    ///
    /// Schema setup belongs to the one writer (ADR-0002). [`Self::open`] migrates, so a headless
    /// MCP server that used it could upgrade the live catalog underneath a running app, from a
    /// process that owns none of the app's state. This connection cannot write at all — the file is
    /// opened with `SQLITE_OPEN_READ_ONLY`, and no pragma that changes the file is issued — and a
    /// catalog at any other schema version is refused with what to do
    /// ([`CoreError::SchemaTooOld`] / [`CoreError::SchemaTooNew`]), rather than read with queries
    /// written for a different shape.
    pub fn open_reader(path: &Path) -> Result<CatalogReader> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| CoreError::Catalog(format!("open failed: {e}")))?;
        // No `journal_mode`: the writer set WAL on the file, and changing it is a write. A reader
        // needs only its own cache, and to wait on the writer rather than error.
        conn.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-32768;
             PRAGMA temp_store=MEMORY;
             PRAGMA busy_timeout=15000;",
        )
        .map_err(|e| CoreError::Catalog(format!("configure pragmas: {e}")))?;
        let catalog = Self {
            conn,
            db_path: Some(std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())),
            appraise_focus: AppraiseFocus::default(),
        };
        let found = catalog.schema_version()?;
        refuse_newer(found)?;
        if found < SCHEMA_VERSION {
            return Err(CoreError::SchemaTooOld {
                found,
                supported: SCHEMA_VERSION,
            });
        }
        Ok(CatalogReader(catalog))
    }
}

#[cfg(test)]
impl CatalogReader {
    /// The raw connection, so a test can prove SQLite itself refuses a write through it.
    pub(super) fn conn(&self) -> &Connection {
        &self.0.conn
    }
}
