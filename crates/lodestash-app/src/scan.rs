//! The source crawl: walk the scan roots, classify what turns up, and hand back the two lists the
//! rest of the app works from — browsable assets and `.unitypackage`s awaiting a decision.
//!
//! **B1 extraction.** This is the first piece of the egui shell's `Engine` to move out, chosen
//! because it is the piece with no UI in it at all: it takes paths and returns data, runs on a
//! background thread in both frontends, and is what a Tauri "add this folder" command needs. The
//! egui shell calls it from two places that had the same loop written out twice; the classify
//! rules (what counts as browsable, what is a pack, what a pack's manifest says) now have exactly
//! one home.
//!
//! Pure and synchronous on purpose. Threading, generations and progress rows are the caller's —
//! they are how a *frontend* stays responsive, not part of what a crawl is.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use lodestash_core::catalog::PackageEntryRecord;
use lodestash_core::types::Exclude;
use lodestash_ingest::crawler::{self, CrawledFile};
use lodestash_ingest::unitypackage;

use crate::types::PendingPackage;

/// One source to crawl, with the per-source rules that decide what comes back.
pub struct Source {
    pub root: PathBuf,
    /// The source's blacklist (INGESTION §1) — resolved by the caller, since it lives in config.
    pub excludes: Vec<Exclude>,
    /// Pack-only sources (FEATURES §5.2) are tracked but not crawled for loose files: their packs
    /// surface as in-pack libraries through the hydrate instead (M14).
    pub pack_only: bool,
}

/// What a crawl found. `entries` carries the source index so a multi-source scan stays attributable.
#[derive(Default)]
pub struct Crawled {
    pub entries: Vec<(usize, CrawledFile)>,
    pub pending_packages: Vec<PendingPackage>,
}

/// Crawl every source in order.
///
/// A `.unitypackage` is never surfaced as an asset: the archive itself is not a thing you browse,
/// so it becomes a [`PendingPackage`] offering Extract instead (INGESTION §2). Anything the
/// crawler could not classify is dropped — an unrecognised file is not content, and putting it in
/// the grid means a card with no type, no thumbnail and nothing to do with it.
pub fn crawl_sources(sources: &[Source]) -> Crawled {
    let mut out = Crawled::default();
    // Multi-source wrapper has no DB context, so every browseable file hashes (the desktop's
    // single-source `crawl_source` passes the incremental diff).
    let known = std::collections::HashMap::new();
    for (source_idx, source) in sources.iter().enumerate() {
        if source.pack_only {
            continue;
        }
        crawl_one(source_idx, &source.root, &source.excludes, &known, &mut out);
    }
    out
}

/// Crawl a single source, attributing everything to `source_idx` — the partial-rescan path.
pub fn crawl_source(
    source_idx: usize,
    root: &Path,
    excludes: &[Exclude],
    known: &std::collections::HashMap<String, (u64, i64)>,
) -> Crawled {
    let mut out = Crawled::default();
    crawl_one(source_idx, root, excludes, known, &mut out);
    out
}

fn crawl_one(
    source_idx: usize,
    root: &Path,
    excludes: &[Exclude],
    known: &std::collections::HashMap<String, (u64, i64)>,
    out: &mut Crawled,
) {
    for file in crawler::crawl(root, excludes) {
        if is_unitypackage(&file.abs_path) {
            if let Some(pkg) = resolve_pending_package(source_idx, &file) {
                out.pending_packages.push(pkg);
            }
            continue;
        }
        if !is_browseable(&file) {
            continue;
        }
        // BLAKE3 the payload so the Duplicates screen has a real signal (B6, DATA-MODEL §6).
        // Only browseable files — hashing dropped ones would be wasted IO. The crawl skips
        // re-hashing files the catalog already knows at the same (size, mtime) — the
        // incremental diff (DATA-MODEL §6, `crawler::needs_rehash`) — so re-adding an indexed
        // folder costs one stat per file, not a full re-read.
        let mut file = file;
        let rel = file.rel_path.to_string_lossy().replace('\\', "/");
        let known = known.get(&rel).copied();
        file.hash = if crawler::needs_rehash(&file, known.map(|(s, _)| s), known.map(|(_, m)| m)) {
            crawler::hash_file(&file.abs_path).ok()
        } else {
            None
        };
        out.entries.push((source_idx, file));
    }
}

/// Is this file a game asset the library should show?
///
/// The catalog only ever stored classified files (`asset.media_type_id` is NOT NULL), so an
/// unclassified one — a `.url` shortcut, a `.pdf` readme, a `.prefab`, a `.cs` script — could only
/// ever be a grid-only ghost: no status, no facets, no embedding, nothing an agent could retrieve.
/// Dropping them at the crawl makes the grid and the catalog agree (INGESTION §3 "non-content",
/// DATA-MODEL §2 media_type).
///
/// ponytail: extension-only, matching `detect::by_extension`. A real asset with an unknown
/// extension is dropped too — wire the magic-byte fallback in if that shows up in practice.
pub fn is_browseable(file: &CrawledFile) -> bool {
    file.media_type.is_some()
}

pub fn is_unitypackage(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("unitypackage"))
        .unwrap_or(false)
}

/// Display name for a package: the file stem of its path, extension and directories stripped.
pub fn package_display_name(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Browse a freshly-crawled `.unitypackage` (cheap manifest read, no extraction) and turn it into
/// a [`PendingPackage`] ready for the user to act on.
///
/// Runs on the caller's background thread, so a large package's manifest read — still O(file size)
/// to decompress, but O(1) memory — never blocks a frame. Returns `None` if the archive cannot be
/// read at all: corrupt or unsupported packs are skipped silently rather than surfaced as error
/// cards, matching INGESTION §8's "don't promise 100%".
pub fn resolve_pending_package(source_idx: usize, file: &CrawledFile) -> Option<PendingPackage> {
    let manifest = unitypackage::read_manifest(&file.abs_path).ok()?;
    let nested = unitypackage::nested_package_candidates(&manifest);
    let chosen_variant = unitypackage::preferred_variant_index(&nested);
    let nested_candidates = nested
        .iter()
        .filter_map(|e| {
            e.pathname
                .clone()
                .map(|p| (package_display_name(Path::new(&p)), p))
        })
        .collect();

    let entries = manifest
        .iter()
        .map(|e| PackageEntryRecord {
            guid: e.guid.clone(),
            pathname: e.pathname.clone(),
            tar_offset: e.asset_offset.map(|o| o as i64),
            size_bytes: e.asset_size.map(|s| s as i64),
        })
        .collect();

    Some(PendingPackage {
        source_idx,
        abs_path: file.abs_path.clone(),
        display_name: package_display_name(&file.rel_path),
        nested_candidates,
        chosen_variant,
        extracting: false,
        progress: Arc::default(),
        entries,
        job: None,
        browsing: false,
        picked: std::collections::BTreeSet::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unitypackage_is_recognised_whatever_its_case() {
        assert!(is_unitypackage(Path::new("a/Pack.unitypackage")));
        assert!(is_unitypackage(Path::new("a/Pack.UnityPackage")));
        assert!(!is_unitypackage(Path::new("a/Pack.unitypackage.meta")));
        assert!(!is_unitypackage(Path::new("a/Rock.fbx")));
    }

    #[test]
    fn a_display_name_is_the_stem_without_directories_or_extension() {
        assert_eq!(
            package_display_name(Path::new("Synty/City/PolygonCity.unitypackage")),
            "PolygonCity"
        );
    }

    /// A pack-only source is tracked but never walked for loose files: its packs arrive through
    /// the hydrate instead. Crawling it anyway would surface the archive's neighbours as assets
    /// of a source the user explicitly said holds only packs.
    #[test]
    fn a_pack_only_source_is_not_crawled() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("Rock.fbx"), b"not really an fbx").expect("write");
        let crawled = crawl_sources(&[Source {
            root: dir.path().to_path_buf(),
            excludes: Vec::new(),
            pack_only: true,
        }]);
        assert!(crawled.entries.is_empty(), "pack-only sources are not walked");

        let crawled = crawl_sources(&[Source {
            root: dir.path().to_path_buf(),
            excludes: Vec::new(),
            pack_only: false,
        }]);
        assert_eq!(crawled.entries.len(), 1, "…but the same source is when it is not");
    }

    /// Every entry carries the index of the source it came from, or a multi-source scan cannot
    /// say which library an asset belongs to — the question the whole browse design is built to
    /// keep answerable.
    #[test]
    fn entries_are_attributed_to_their_source() {
        let a = tempfile::tempdir().expect("temp dir");
        let b = tempfile::tempdir().expect("temp dir");
        std::fs::write(a.path().join("one.fbx"), b"x").expect("write");
        std::fs::write(b.path().join("two.fbx"), b"x").expect("write");
        let crawled = crawl_sources(&[
            Source {
                root: a.path().to_path_buf(),
                excludes: Vec::new(),
                pack_only: false,
            },
            Source {
                root: b.path().to_path_buf(),
                excludes: Vec::new(),
                pack_only: false,
            },
        ]);
        let mut idxs: Vec<usize> = crawled.entries.iter().map(|(i, _)| *i).collect();
        idxs.sort_unstable();
        assert_eq!(idxs, [0, 1]);
    }

    /// The blacklist is what **Remove from library** writes (B5), so a crawl that ignored it would
    /// resurrect every removed asset on the next scan — the one thing Remove promises it will not do.
    #[test]
    fn an_excluded_asset_does_not_come_back() {
        use lodestash_core::types::ExcludeKind;
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("keep.fbx"), b"x").expect("write");
        std::fs::write(dir.path().join("drop.fbx"), b"x").expect("write");
        let crawled = crawl_sources(&[Source {
            root: dir.path().to_path_buf(),
            excludes: vec![Exclude {
                kind: ExcludeKind::Asset,
                value: "drop.fbx".into(),
            }],
            pack_only: false,
        }]);
        let names: Vec<String> = crawled
            .entries
            .iter()
            .map(|(_, f)| f.rel_path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["keep.fbx"]);
    }
}
