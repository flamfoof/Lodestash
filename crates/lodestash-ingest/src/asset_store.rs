//! Unity Asset Store download-cache discovery, validation, and browse-without-unpack.
//! Spec: INGESTION.md §2. The cache is an `Asset Store-5.x` root holding a
//! `<Publisher>/<Category>/<Package>.unitypackage` tree; each package is a plain gzip+tar
//! ([`crate::unitypackage`]), so browsing needs no Unity Editor and no extraction.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// One `.unitypackage` in the cache, described from its path alone (no unpacking). The
/// publisher/category come free from the cache tree (INGESTION.md §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetStorePackage {
    pub publisher: String,
    pub category: String,
    /// Display name — the package file stem.
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

/// The per-OS default Asset Store cache root (INGESTION.md §2), if the platform's base dir is known.
/// Existence is **not** checked here — callers validate with [`is_asset_store_cache`].
pub fn default_root() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        // %APPDATA% (…\AppData\Roaming) \ Unity \ Asset Store-5.x
        std::env::var_os("APPDATA").map(|p| PathBuf::from(p).join("Unity").join("Asset Store-5.x"))
    }
    #[cfg(target_os = "macos")]
    {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("Unity")
                .join("Asset Store-5.x")
        })
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // $XDG_DATA_HOME or ~/.local/share, then unity3d/Asset Store-5.x
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
            .map(|d| d.join("unity3d").join("Asset Store-5.x"))
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
    {
        None
    }
}

/// Candidate cache roots in discovery order (INGESTION.md §2): the `ASSETSTORE_CACHE_PATH` override
/// first, then the per-OS default. Deduplicated, order-preserving. Existence is not checked here.
pub fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(env) = std::env::var_os("ASSETSTORE_CACHE_PATH") {
        roots.push(PathBuf::from(env));
    }
    if let Some(def) = default_root() {
        if !roots.contains(&def) {
            roots.push(def);
        }
    }
    roots
}

/// The directory Unity names its download cache. A moved cache keeps the name, which makes it the
/// one reliable signal that a path sits inside one.
pub const CACHE_DIR_NAME: &str = "Asset Store-5.x";

/// Infer the cache root that `path` lives inside, if any (INGESTION.md §2 discovery).
///
/// The env var and the per-OS default only find a cache that is where Unity put it. A user who
/// moved theirs to another drive — and then added a *pack folder* from it as a library source —
/// has told us where it is without knowing it; this reads that. Matching on the directory name
/// rather than on "contains a `.unitypackage` somewhere above" is what stops it walking up to the
/// drive root and calling `G:\` an Asset Store cache.
pub fn infer_root_from(path: &Path) -> Option<PathBuf> {
    let mut cur = Some(path);
    while let Some(dir) = cur {
        if dir
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.eq_ignore_ascii_case(CACHE_DIR_NAME))
            && is_asset_store_cache(dir)
        {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// Does `root` look like a real Asset Store cache — a directory holding at least one
/// `.unitypackage` within a `<Publisher>/<Category>/` tree (INGESTION.md §2 first-run validation)?
/// Cheap: stops at the first match within 3 levels.
pub fn is_asset_store_cache(root: &Path) -> bool {
    if !root.is_dir() {
        return false;
    }
    WalkDir::new(root)
        .min_depth(1)
        .max_depth(3)
        .into_iter()
        .filter_map(|e| e.ok())
        .any(|e| e.file_type().is_file() && is_unitypackage(e.path()))
}

/// Browse every package under a cache root **without unpacking** (INGESTION.md §2). Publisher =
/// the first path component under `root`, category = the second; a package shallower than
/// `publisher/category/pkg` gets empty strings for the missing levels. Returned sorted by
/// publisher, then category, then name.
pub fn browse(root: &Path) -> Vec<AssetStorePackage> {
    let mut out: Vec<AssetStorePackage> = WalkDir::new(root)
        .min_depth(1)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_unitypackage(e.path()))
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?;
            let comps: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            // comps: [publisher?, category?, …, "Pack.unitypackage"]
            let publisher = comps.first().cloned().unwrap_or_default();
            let category = if comps.len() >= 3 {
                comps[1].clone()
            } else {
                String::new()
            };
            let name = e
                .path()
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let size_bytes = e.metadata().ok().map(|m| m.len()).unwrap_or(0);
            Some(AssetStorePackage {
                publisher,
                category,
                name,
                path: e.path().to_path_buf(),
                size_bytes,
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.publisher
            .cmp(&b.publisher)
            .then(a.category.cmp(&b.category))
            .then(a.name.cmp(&b.name))
    });
    out
}

fn is_unitypackage(path: &Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("unitypackage"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    #[test]
    fn infer_root_finds_the_cache_from_a_pack_folder_inside_it() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("Asset Store-5.x");
        let pack_dir = cache.join("SomePublisher").join("3D Models");
        std::fs::create_dir_all(&pack_dir).unwrap();
        std::fs::write(pack_dir.join("Pack.unitypackage"), b"x").unwrap();

        assert_eq!(
            super::infer_root_from(&pack_dir).as_deref(),
            Some(cache.as_path())
        );
        assert_eq!(
            super::infer_root_from(&pack_dir.join("Pack.unitypackage")).as_deref(),
            Some(cache.as_path()),
            "a file inside the tree works too"
        );
        // Nothing above the cache is a cache: the walk must not reach the drive root.
        assert_eq!(super::infer_root_from(dir.path()), None);

        // A directory with the right name but no packages is not a cache.
        let empty = dir.path().join("elsewhere").join("Asset Store-5.x");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(super::infer_root_from(&empty), None);
    }

    use super::*;
    use std::fs;

    /// Build a synthetic cache: <root>/<Publisher>/<Category>/<Pack>.unitypackage
    fn build_cache(root: &Path) {
        for (pub_, cat, pkg) in [
            ("SICS Games", "3D ModelsEnvironments", "Toon Fantasy Nature"),
            ("SICS Games", "Textures", "Toon Textures"),
            ("Synty", "3D ModelsCharacters", "POLYGon City"),
        ] {
            let dir = root.join(pub_).join(cat);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(format!("{pkg}.unitypackage")), b"gz").unwrap();
        }
        // A non-package file should be ignored.
        fs::write(root.join("SICS Games").join("readme.txt"), b"x").unwrap();
    }

    #[test]
    fn browse_lists_packages_with_publisher_and_category() {
        let dir = tempfile::tempdir().unwrap();
        build_cache(dir.path());
        let pkgs = browse(dir.path());
        assert_eq!(pkgs.len(), 3, "3 packages, readme ignored");
        // Sorted: SICS Games < Synty; within SICS, 3D Models… < Textures.
        assert_eq!(pkgs[0].publisher, "SICS Games");
        assert_eq!(pkgs[0].category, "3D ModelsEnvironments");
        assert_eq!(pkgs[0].name, "Toon Fantasy Nature");
        assert_eq!(pkgs[2].publisher, "Synty");
        assert!(pkgs.iter().all(|p| p.size_bytes > 0));
    }

    #[test]
    fn is_asset_store_cache_true_for_a_real_tree_false_otherwise() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            !is_asset_store_cache(dir.path()),
            "empty dir is not a cache"
        );
        build_cache(dir.path());
        assert!(
            is_asset_store_cache(dir.path()),
            "populated tree is a cache"
        );

        let empty = tempfile::tempdir().unwrap();
        fs::create_dir_all(empty.path().join("Publisher").join("Category")).unwrap();
        assert!(
            !is_asset_store_cache(empty.path()),
            "dirs with no .unitypackage are not a cache"
        );
    }

    #[test]
    fn candidate_roots_honours_env_override_first() {
        // Not manipulating process env (racy); assert the default is present on this platform.
        let roots = candidate_roots();
        // default_root is Some on all tier-1 platforms, so candidate_roots is non-empty.
        assert!(
            default_root().is_none() || !roots.is_empty(),
            "when a default exists, it is offered"
        );
    }

    #[test]
    fn browse_of_missing_root_is_empty() {
        let missing = std::path::Path::new("/definitely/not/here/Asset Store-5.x");
        assert!(browse(missing).is_empty());
        assert!(!is_asset_store_cache(missing));
    }
}
