//! Crawl-record building + catalog persistence for scans, split out of the egui `app.rs` (B1);
//! carries no renderer.
use lodestash_core::catalog::{
    Catalog, CrawlRecord, InPackClass, PackageEntryRecord, StatusCounts,
};
use lodestash_core::types::AssetStatus;
use lodestash_ingest::crawler::CrawledFile;
use lodestash_ingest::detect;

use crate::types::*;

/// Classified files → catalog crawl records; unclassified files stay grid-only
/// (`asset.media_type_id` is NOT NULL). Paths are stored `/`-separated.
pub fn crawl_records<'a>(files: impl Iterator<Item = &'a CrawledFile>) -> Vec<CrawlRecord> {
    files
        .filter(|f| f.media_type.is_some())
        .map(crawl_record)
        .collect()
}

/// One classified file as a catalog record. Callers that know where the file lives fill in
/// `workspace_rel` afterwards; an on-disk crawl leaves it None.
pub fn crawl_record(f: &CrawledFile) -> CrawlRecord {
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
        content_hash: f.hash.clone(),
    }
}

/// Classify a package's manifest entries for minting (M14): map each entry's GUID to the media
/// type + display name the in-pack asset should carry. Entries whose extension classifies to no
/// `media_type` (scripts, prefabs, manifests) are dropped — the same "unclassified never reaches
/// the grid" rule the crawl applies, enforced here by their absence from the returned map.
pub fn classify_in_pack_entries(
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
pub fn crawl_records_in_workspace<'a>(
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

/// Read every live asset row back for the given `(root, source_id)` pairs, keyed for
/// the UI-side join (unknown status strings are dropped, not fatal).
pub fn read_back_assets(
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
pub fn persist_to_catalog(
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
        // Move/rename detection (DATA-MODEL §6) runs **before** record_crawl: a vanished row
        // whose hash reappeared at a new path is the same file moved — rel_path updated in
        // place (asset_id, tags, embeddings, mappings preserved) — and the consumed records
        // are dropped so they don't insert a duplicate. Trashed assets are exempt.
        let moved = catalog
            .reconcile_moves(id, &batch.records)
            .map_err(|e| e.to_string())?;
        let remaining: Vec<CrawlRecord> = batch
            .records
            .iter()
            .filter(|r| !moved.contains(&r.rel_path))
            .cloned()
            .collect();
        let stats = catalog
            .record_crawl(id, None, &remaining)
            .map_err(|e| e.to_string())?;
        // Files gone from disk are flagged missing (metadata preserved, hidden from
        // hydrate + grid); files that came back are unflagged. Present means every path the
        // crawl saw — a moved file's new path included, or its repointed row is flagged missing
        // and leaves the grid (review B1).
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
            moved = moved.len(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn batch(files: &[(&str, &str)]) -> PersistBatch {
        PersistBatch {
            root: "C:/lib".into(),
            name: "Lib".into(),
            excludes_json: None,
            records: files
                .iter()
                .map(|(rel, hash)| CrawlRecord {
                    rel_path: rel.to_string(),
                    name: rel.rsplit('/').next().unwrap().to_string(),
                    media_type: "image".into(),
                    size_bytes: 1,
                    mtime: 1,
                    workspace_rel: None,
                    content_hash: Some(hash.to_string()),
                })
                .collect(),
        }
    }

    /// `(rel_path, asset_id)` the grid would show for the source after a persist: live rows only,
    /// read back through a separate connection.
    fn shown(db: &std::path::Path) -> Vec<(String, i64)> {
        let c = Catalog::open(db).unwrap();
        let src = c.source_info_by_root("C:/lib").unwrap().unwrap().id;
        let mut rows: Vec<(String, i64)> = c
            .entries_for_source(src)
            .unwrap()
            .into_iter()
            .map(|a| (a.rel_path, a.asset_id))
            .collect();
        rows.sort();
        rows
    }

    /// A renamed file keeps its identity and stays on screen (review B1): the repointed row is on
    /// disk at its new path, so the missing pass must count it present. It used to be flagged
    /// missing and vanish from the grid until something else in the source changed.
    #[test]
    fn a_renamed_file_keeps_its_identity_and_stays_visible() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        persist_to_catalog(&db, &[batch(&[("rock.png", "H1"), ("tree.png", "H2")])]).unwrap();
        let before = shown(&db);
        let rock = before.iter().find(|(p, _)| p == "rock.png").unwrap().1;
        {
            let mut c = Catalog::open(&db).unwrap();
            c.add_asset_tag(rock, "manual", "subject", "boulder")
                .unwrap();
        }

        persist_to_catalog(
            &db,
            &[batch(&[("stones/big rock.png", "H1"), ("tree.png", "H2")])],
        )
        .unwrap();
        let after = shown(&db);
        assert!(
            after.contains(&("stones/big rock.png".to_string(), rock)),
            "the renamed file is shown under its old id: {after:?}"
        );
        assert_eq!(
            after.len(),
            2,
            "nothing went missing, nothing was duplicated"
        );
        let tags = Catalog::open(&db).unwrap().tags_for_asset(rock).unwrap();
        assert_eq!(tags[0].value, "boulder", "its tags came with it");
    }

    /// A folder of byte-identical files moves file by file: two ids on two paths, both shown,
    /// each with its own tags (review B2, end to end).
    #[test]
    fn a_moved_folder_of_identical_files_keeps_both_identities() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        persist_to_catalog(&db, &[batch(&[("old/a.png", "H"), ("old/b.png", "H")])]).unwrap();
        let before = shown(&db);
        let labels = ["alpha", "beta"];
        {
            let mut c = Catalog::open(&db).unwrap();
            for ((_, id), label) in before.iter().zip(labels) {
                c.add_asset_tag(*id, "manual", "subject", label).unwrap();
            }
        }

        persist_to_catalog(&db, &[batch(&[("new/a.png", "H"), ("new/b.png", "H")])]).unwrap();
        let after = shown(&db);
        let ids = |rows: &[(String, i64)]| rows.iter().map(|r| r.1).collect::<Vec<_>>();
        assert_eq!(
            after.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
            vec!["new/a.png", "new/b.png"]
        );
        assert_eq!(ids(&after), ids(&before), "each file kept its id");
        let c = Catalog::open(&db).unwrap();
        for ((_, id), label) in before.iter().zip(labels) {
            let tag = c.tags_for_asset(*id).unwrap()[0].value.clone();
            assert_eq!(tag, label, "asset {id} kept its own tag");
        }
    }
}
