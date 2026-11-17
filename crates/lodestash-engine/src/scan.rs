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

use std::path::Path;

use lodestash_core::catalog::PackageEntryRecord;
use lodestash_core::types::Exclude;
use lodestash_ingest::crawler::{self, CrawledFile};
use lodestash_ingest::{detect, unitypackage};

use crate::types::PendingPackage;

/// What a crawl found. `entries` carries the source index so a multi-source scan stays attributable.
#[derive(Default)]
pub struct Crawled {
    pub entries: Vec<(usize, CrawledFile)>,
    pub pending_packages: Vec<PendingPackage>,
}

/// Crawl one source, attributing everything to `source_idx`.
///
/// A `.unitypackage` is never surfaced as an asset: the archive itself is not a thing you browse,
/// so it becomes a [`PendingPackage`] the writer records as a pack (INGESTION §2). Anything the
/// crawler could not classify is dropped — an unrecognised file is not content, and putting it in
/// the grid means a card with no type, no thumbnail and nothing to do with it.
///
/// `known` is the catalog's last-known `(size, mtime)` per `rel_path`: a file it matches is not
/// re-hashed (the incremental diff, DATA-MODEL §6).
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

/// Browse a freshly-crawled `.unitypackage` (cheap manifest read, no extraction) into a
/// [`PendingPackage`] the writer records as a browsable pack.
///
/// Runs on the crawl's thread, so a large package's manifest read — still O(file size) to
/// decompress, but O(1) memory — never lands on the writer. Returns `None` if the archive cannot
/// be read at all: corrupt or unsupported packs are skipped rather than surfaced as error cards,
/// matching INGESTION §8's "don't promise 100%".
pub fn resolve_pending_package(source_idx: usize, file: &CrawledFile) -> Option<PendingPackage> {
    let manifest = unitypackage::read_manifest(&file.abs_path).ok()?;
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
        entries,
    })
}

/// The stored `package.category` vocabulary for the import dialog (FEATURES §5.2/§5.3): the four
/// content-derivable buckets plus the two override-only ones. `Code`/`UI` can never be
/// auto-detected — scripts have no `media_type` and UI-vs-2D game-art cannot be told from bytes
/// (FEATURES §5.3).
pub const CATEGORY_3D: &str = "3D";
pub const CATEGORY_MATERIALS: &str = "Materials";
pub const CATEGORY_AUDIO: &str = "Audio";
pub const CATEGORY_2D: &str = "2D";
pub const CATEGORY_CODE: &str = "Code";
pub const CATEGORY_UI: &str = "UI";

/// The override dropdown's full offer (FEATURES §5.2): the four content buckets plus the two
/// override-only labels.
pub const IMPORT_CATEGORIES: [&str; 6] = [
    CATEGORY_3D,
    CATEGORY_MATERIALS,
    CATEGORY_AUDIO,
    CATEGORY_2D,
    CATEGORY_CODE,
    CATEGORY_UI,
];

/// Map a `media_type` code to its content category (FEATURES §5.2 auto-detect). Textures ride
/// with materials; image/sprite/font are 2D. `None` for anything without a derivable bucket.
pub fn content_category(media_type_code: &str) -> Option<&'static str> {
    match media_type_code {
        "model_3d" => Some(CATEGORY_3D),
        "material" | "texture" => Some(CATEGORY_MATERIALS),
        "audio" => Some(CATEGORY_AUDIO),
        "image" | "sprite" | "font" => Some(CATEGORY_2D),
        _ => None,
    }
}

/// Dominant content category for a pack's manifest entries — the most common classified media
/// type's bucket. Ties break toward the fixed order above (`3D` first). `None` when no entry
/// classifies.
pub fn dominant_content_category(entries: &[PackageEntryRecord]) -> Option<&'static str> {
    const ORDER: [&str; 4] = [CATEGORY_3D, CATEGORY_MATERIALS, CATEGORY_AUDIO, CATEGORY_2D];
    let mut counts = [0usize; 4];
    for e in entries {
        let Some(p) = e.pathname.as_deref() else {
            continue;
        };
        // Classify by extension, exactly like `classify_in_pack_entries` — `.meta` sidecars and
        // scripts/prefabs drop out (`by_extension` returns None).
        let Some(media) = detect::by_extension(std::path::Path::new(p)) else {
            continue;
        };
        let Some(cat) = content_category(media.code()) else {
            continue;
        };
        if let Some(i) = ORDER.iter().position(|c| *c == cat) {
            counts[i] += 1;
        }
    }
    let mut best: Option<(usize, &'static str)> = None;
    for (i, cat) in ORDER.iter().enumerate() {
        if counts[i] > 0 && best.map_or(true, |(bi, _)| counts[i] > counts[bi]) {
            best = Some((i, cat));
        }
    }
    best.map(|(_, cat)| cat)
}

/// Normalize a pack name for the §5.6 duplicate check: lowercase, keep alphanumerics only.
/// Name-only matching (content hashing is the future refinement, FEATURES §5.6).
pub fn normalize_pack_name(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
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

    /// Every entry carries the index of the source it came from, or a multi-source scan cannot
    /// say which library an asset belongs to — the question the whole browse design is built to
    /// keep answerable.
    #[test]
    fn entries_are_attributed_to_their_source() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("one.fbx"), b"x").expect("write");
        let known = std::collections::HashMap::new();
        let crawled = crawl_source(3, dir.path(), &[], &known);
        let idxs: Vec<usize> = crawled.entries.iter().map(|(i, _)| *i).collect();
        assert_eq!(idxs, [3]);
    }

    /// The blacklist is what **Remove from library** writes (B5), so a crawl that ignored it would
    /// resurrect every removed asset on the next scan — the one thing Remove promises it will not do.
    #[test]
    fn an_excluded_asset_does_not_come_back() {
        use lodestash_core::types::ExcludeKind;
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(dir.path().join("keep.fbx"), b"x").expect("write");
        std::fs::write(dir.path().join("drop.fbx"), b"x").expect("write");
        let excludes = [Exclude {
            kind: ExcludeKind::Asset,
            value: "drop.fbx".into(),
        }];
        let crawled = crawl_source(0, dir.path(), &excludes, &std::collections::HashMap::new());
        let names: Vec<String> = crawled
            .entries
            .iter()
            .map(|(_, f)| f.rel_path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["keep.fbx"]);
    }

    fn entry(pathname: Option<&str>) -> PackageEntryRecord {
        PackageEntryRecord {
            guid: pathname.map(|p| format!("g:{p}")).unwrap_or_default(),
            pathname: pathname.map(str::to_owned),
            tar_offset: Some(0),
            size_bytes: Some(1),
        }
    }

    #[test]
    fn the_dominant_category_counts_classifiable_entries_only() {
        let entries = vec![
            entry(Some("Assets/Rock.fbx")),
            entry(Some("Assets/Rock.fbx.meta")),
            entry(Some("Assets/Wood.png")),
            entry(Some("Assets/Hum.wav")),
            entry(Some("Assets/Hum.wav")),
            entry(Some("Assets/Shotgun.cs")),
        ];
        // Audio (2) beats 3D (1) and Materials (1); .meta and .cs classify to nothing.
        assert_eq!(dominant_content_category(&entries), Some(CATEGORY_AUDIO));
    }

    #[test]
    fn a_pack_with_no_classifiable_entries_has_no_category() {
        assert_eq!(
            dominant_content_category(&[entry(Some("Assets/Readme.txt"))]),
            None
        );
    }

    #[test]
    fn content_category_maps_the_seven_value_enum_to_four_buckets() {
        assert_eq!(content_category("model_3d"), Some(CATEGORY_3D));
        assert_eq!(content_category("material"), Some(CATEGORY_MATERIALS));
        assert_eq!(content_category("texture"), Some(CATEGORY_MATERIALS));
        assert_eq!(content_category("audio"), Some(CATEGORY_AUDIO));
        assert_eq!(content_category("image"), Some(CATEGORY_2D));
        assert_eq!(content_category("sprite"), Some(CATEGORY_2D));
        assert_eq!(content_category("font"), Some(CATEGORY_2D));
    }

    #[test]
    fn pack_names_normalize_for_duplicate_matching() {
        assert_eq!(normalize_pack_name("POLYGon City"), "polygoncity");
        assert_eq!(
            normalize_pack_name("Toon-Fantasy_Nature"),
            "toonfantasynature"
        );
    }
}
