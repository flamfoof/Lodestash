//! Crawl-record building + catalog persistence for scans and package extraction, split out of
//! app.rs. Child module of `app`: reaches app's private items via `use super::*`.
use super::*;

/// Classified files → catalog crawl records; unclassified files stay grid-only
/// (`asset.media_type_id` is NOT NULL). Paths are stored `/`-separated.
pub(super) fn crawl_records<'a>(files: impl Iterator<Item = &'a CrawledFile>) -> Vec<CrawlRecord> {
    files
        .filter(|f| f.media_type.is_some())
        .map(crawl_record)
        .collect()
}

/// One classified file as a catalog record. Callers that know where the file lives fill in
/// `workspace_rel` afterwards; an on-disk crawl leaves it None.
pub(super) fn crawl_record(f: &CrawledFile) -> CrawlRecord {
    CrawlRecord {
        rel_path: f.rel_path.to_string_lossy().replace('\\', "/"),
        name: f
            .rel_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        media_type: f
            .media_type
            .expect("caller filtered Some")
            .code()
            .to_string(),
        size_bytes: f.size_bytes as i64,
        mtime: f.mtime,
        workspace_rel: None,
    }
}

/// Ensure a pack's in-pack assets exist (M14): record its manifest locators if the catalog
/// doesn't have them yet (reading the archive once), then mint an in-pack asset per classified
/// entry. Idempotent — mint skips duplicates and extracted packs. Background-only: a manifest
/// read streams the whole gzip. Returns how many assets were newly minted.
///
/// `force` re-reads the manifest and re-records locators **even when they already exist** — the
/// ↻ Sync action's "re-scan archive index" (FEATURES.md §5.3), for a `.unitypackage` rebuilt on
/// disk since import (offsets/paths shift). `record_package_entries` upserts by `(package_id, guid)`
/// so a changed entry refreshes in place and a new one is added; mint then picks up new classifiable
/// entries. ponytail: refresh is additive — a GUID *removed* from the archive leaves its stale
/// locator + minted row (no prune); add a diff-and-delete pass if that case shows up in practice.
pub(super) fn ensure_in_pack_minted(
    db: &std::path::Path,
    package_id: i64,
    pack_path: &std::path::Path,
    progress: &crate::jobs::JobProgress,
    force: bool,
) -> Result<usize, String> {
    use std::sync::atomic::Ordering;
    let mut catalog = Catalog::open(db).map_err(|e| e.to_string())?;
    let mut entries = catalog
        .list_package_entries(package_id)
        .map_err(|e| e.to_string())?;
    if force || entries.is_empty() {
        // Streaming manifest read: a big pack (a 21 GB music collection) must inflate its whole
        // tar.gz to list its files, so drive a progress bar off compressed bytes and let the tray's
        // ✕ cancel it. Cancel surfaces as an Err, leaving nothing half-written (locators are
        // recorded only after a complete read).
        let manifest = unitypackage::read_manifest_with_progress(pack_path, &|read, total| {
            progress.total.store(total, Ordering::Relaxed);
            progress.done.store(read, Ordering::Relaxed);
            !progress.is_cancelled()
        })
        .map_err(|e| e.to_string())?;
        entries = manifest
            .iter()
            .map(|e| PackageEntryRecord {
                guid: e.guid.clone(),
                pathname: e.pathname.clone(),
                tar_offset: e.asset_offset.map(|o| o as i64),
                size_bytes: e.asset_size.map(|s| s as i64),
            })
            .collect();
        catalog
            .record_package_entries(package_id, &entries)
            .map_err(|e| e.to_string())?;
    }
    let classified = classify_in_pack_entries(&entries);
    let stats = catalog
        .mint_in_pack_assets(package_id, &classified)
        .map_err(|e| e.to_string())?;
    Ok(stats.inserted)
}

/// Classify a package's manifest entries for minting (M14): map each entry's GUID to the media
/// type + display name the in-pack asset should carry. Entries whose extension classifies to no
/// `media_type` (scripts, prefabs, manifests) are dropped — the same "unclassified never reaches
/// the grid" rule the crawl applies, enforced here by their absence from the returned map.
pub(super) fn classify_in_pack_entries(
    entries: &[PackageEntryRecord],
) -> std::collections::HashMap<String, InPackClass> {
    entries
        .iter()
        .filter_map(|e| {
            let pathname = e.pathname.as_deref()?;
            let path = std::path::Path::new(pathname);
            let media_type = detect::by_extension(path)?;
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.guid.clone());
            Some((
                e.guid.clone(),
                InPackClass {
                    name,
                    media_type: media_type.code().to_string(),
                },
            ))
        })
        .collect()
}

/// [`crawl_records`] for package-extracted files, locating each payload inside the extraction
/// workspace (M6 `asset_file.workspace_rel`) so the catalog can rehydrate it in a later session.
///
/// The location has to be resolved in the same pass that builds the record: `crawl_records` drops
/// unclassified files, so a record list and the file list it came from are different lengths and
/// pairing them positionally mis-assigns every path after the first dropped file.
pub(super) fn crawl_records_in_workspace<'a>(
    files: impl Iterator<Item = &'a CrawledFile>,
    workspace_dir: Option<&std::path::Path>,
) -> Vec<CrawlRecord> {
    files
        .filter(|f| f.media_type.is_some())
        .map(|f| {
            let mut rec = crawl_record(f);
            rec.workspace_rel = workspace_dir.and_then(|ws| {
                f.abs_path
                    .strip_prefix(ws)
                    .ok()
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
            });
            rec
        })
        .collect()
}

/// Runs on the persist thread: the extracted package's `package` row, its asset rows
/// (carrying `package_id`), the unpack stamp, then project-wide status counts.
pub(super) fn persist_package_to_catalog(
    db_path: &std::path::Path,
    job: &PackagePersist,
) -> Result<(StatusCounts, AssetIdentityMaps, Catalog), String> {
    let mut catalog = Catalog::open(db_path).map_err(|e| e.to_string())?;
    catalog.seed_media_types().map_err(|e| e.to_string())?;

    let source_id = catalog
        .upsert_import_source(&job.root_name, &job.root, "folder")
        .map_err(|e| e.to_string())?;
    let package_id = catalog
        .upsert_package(
            source_id,
            &job.package_path,
            &job.name,
            job.publisher.as_deref(),
            job.category.as_deref(),
        )
        .map_err(|e| e.to_string())?;
    harvest_package_metadata(&catalog, package_id, job);
    // Entries first, so `materialize_in_pack_assets` can match already-minted in-pack rows by
    // `package_entry.pathname`.
    catalog
        .record_package_entries(package_id, &job.entries)
        .map_err(|e| e.to_string())?;
    // M14 step 8: extraction is demoted to *materialize*. Fill `workspace_rel` on the pack's
    // existing in-pack rows rather than inserting a parallel set (which would shadow them and force
    // the slow in-pack pull on view). The extracted record's `rel_path` is `display_name/<pathname>`
    // — drop the first component to recover the manifest pathname the mint keyed on.
    let pairs: Vec<(String, String)> = job
        .records
        .iter()
        .filter_map(|r| {
            let ws = r.workspace_rel.clone()?;
            let (_, pathname) = r.rel_path.split_once('/')?;
            Some((pathname.to_string(), ws))
        })
        .collect();
    let materialized = catalog
        .materialize_in_pack_assets(package_id, &pairs)
        .map_err(|e| e.to_string())?;
    // Any file with no in-pack row (a pack extracted without ever being analyzed) still needs a row:
    // insert just those, so a never-analyzed pack behaves exactly as before.
    let leftover: Vec<CrawlRecord> = job
        .records
        .iter()
        .filter(|r| match r.rel_path.split_once('/') {
            Some((_, pathname)) => !materialized.contains(pathname),
            None => true,
        })
        .cloned()
        .collect();
    let stats = catalog
        .record_crawl(source_id, Some(package_id), &leftover)
        .map_err(|e| e.to_string())?;
    catalog
        .mark_package_unpacked(package_id, job.records.len())
        .map_err(|e| e.to_string())?;
    tracing::info!(
        package = %job.name,
        inserted = stats.inserted,
        unchanged = stats.unchanged,
        "package sync"
    );

    let mut sources = Vec::with_capacity(job.all_roots.len());
    for (name, root) in &job.all_roots {
        let id = catalog
            .upsert_import_source(name, root, "folder")
            .map_err(|e| e.to_string())?;
        sources.push((root.clone(), id));
    }
    let ids: Vec<i64> = sources.iter().map(|(_, id)| *id).collect();
    let counts = catalog
        .status_counts_for_sources(&ids)
        .map_err(|e| e.to_string())?;
    let assets = read_back_assets(&catalog, &sources)?;
    Ok((counts, assets, catalog))
}

/// Harvest what a pack says about itself and store it (DATA-MODEL §7, INGESTION §2).
///
/// Runs at **extraction** time rather than at browse time: the harvest re-reads the archive, and a
/// 20 GB pack should not be inflated twice just to fill in a version string the user hasn't asked
/// to see. The entries the extraction already produced tell it which GUIDs are worth reading.
///
/// A pack with a publisher came from the Asset Store cache tree, and that is enough to know its
/// licence — the EULA is a fact about where the pack came from, so it fills in only when the pack
/// itself said nothing.
pub(super) fn harvest_package_metadata(catalog: &Catalog, package_id: i64, job: &PackagePersist) {
    let entries: Vec<unitypackage::PackageEntry> = job
        .entries
        .iter()
        .map(|e| unitypackage::PackageEntry {
            guid: e.guid.clone(),
            pathname: e.pathname.clone(),
            asset_size: e.size_bytes.map(|n| n as u64),
            ..Default::default()
        })
        .collect();
    let path = std::path::Path::new(&job.package_path);
    let meta = match unitypackage::read_package_meta(path, &entries) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(%e, "package metadata harvest failed");
            return;
        }
    };
    if let (Some(png), Some(cache)) = (&meta.icon_png, default_cache_dirs().0) {
        if let Err(e) = thumb::write_package_icon(&cache, &job.package_path, png) {
            tracing::warn!(%e, "pack icon write failed");
        }
    }
    let store_pack = job.publisher.is_some();
    let license = meta
        .license
        .clone()
        .or_else(|| store_pack.then(|| "Unity Asset Store EULA".to_string()));
    let license_url = meta
        .license_url
        .clone()
        .or_else(|| store_pack.then(|| "https://unity.com/legal/as-terms".to_string()));
    if let Err(e) = catalog.set_package_metadata(
        package_id,
        meta.version.as_deref(),
        license.as_deref(),
        job.publisher.as_deref(),
        license_url.as_deref(),
    ) {
        tracing::warn!(%e, "package metadata write failed");
    } else {
        tracing::info!(
            package = %job.name,
            version = ?meta.version,
            license = ?license,
            icon = meta.icon_png.as_ref().map_or(0, |b| b.len()),
            "pack metadata harvested"
        );
    }
}

/// Read every live asset row back for the given `(root, source_id)` pairs, keyed for
/// the UI-side join (unknown status strings are dropped, not fatal).
pub(super) fn read_back_assets(
    catalog: &Catalog,
    sources: &[(String, i64)],
) -> Result<AssetIdentityMaps, String> {
    sources
        .iter()
        .map(|(root, id)| {
            let map = catalog
                .assets_for_source(*id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter_map(|a| {
                    AssetStatus::from_code(&a.status).map(|s| {
                        (
                            a.rel_path,
                            (
                                a.asset_id,
                                s,
                                a.trashed,
                                a.rating,
                                a.metrics,
                                a.content_hash,
                            ),
                        )
                    })
                })
                .collect();
            Ok((root.clone(), map))
        })
        .collect()
}

/// Runs on the persist thread: open the catalog, upsert each source, record its crawl
/// incrementally, adopt any config blacklists (one-time M5 data step), and read back
/// the per-status counts + asset identities for exactly these sources.
pub(super) fn persist_to_catalog(
    db_path: &std::path::Path,
    batches: &[PersistBatch],
) -> Result<(StatusCounts, AssetIdentityMaps, Catalog), String> {
    let mut catalog = Catalog::open(db_path).map_err(|e| e.to_string())?;
    catalog.seed_media_types().map_err(|e| e.to_string())?;

    let mut sources = Vec::with_capacity(batches.len());
    let mut blacklists = Vec::new();
    for batch in batches {
        let id = catalog
            .upsert_import_source(&batch.name, &batch.root, "folder")
            .map_err(|e| e.to_string())?;
        sources.push((batch.root.clone(), id));
        if let Some(json) = &batch.excludes_json {
            blacklists.push((batch.root.clone(), json.clone()));
        }
        let stats = catalog
            .record_crawl(id, None, &batch.records)
            .map_err(|e| e.to_string())?;
        // Files gone from disk are flagged missing (metadata preserved, hidden from
        // hydrate + grid); files that came back are unflagged.
        let present: std::collections::HashSet<String> =
            batch.records.iter().map(|r| r.rel_path.clone()).collect();
        let (gone, back) = catalog
            .reconcile_missing(id, &present)
            .map_err(|e| e.to_string())?;
        tracing::info!(
            root = %batch.root,
            inserted = stats.inserted,
            updated = stats.updated,
            unchanged = stats.unchanged,
            gone,
            back,
            "catalog sync"
        );
    }
    if !blacklists.is_empty() {
        catalog
            .adopt_config_blacklists(&blacklists)
            .map_err(|e| e.to_string())?;
    }
    let ids: Vec<i64> = sources.iter().map(|(_, id)| *id).collect();
    let counts = catalog
        .status_counts_for_sources(&ids)
        .map_err(|e| e.to_string())?;
    let assets = read_back_assets(&catalog, &sources)?;
    // Hand the open connection back so the caller can drain `embedding_dirty` on this same
    // thread — keeps the single-writer rule (ARCHITECTURE §5) with no extra writer thread.
    Ok((counts, assets, catalog))
}
