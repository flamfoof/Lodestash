//! Recursive folder crawler with BLAKE3 hashing and (size, mtime) incremental diff.
//! Spec: INGESTION.md §1, DATA-MODEL.md §6.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use lodestash_core::types::{is_excluded, Exclude};
use walkdir::WalkDir;

use super::detect;

/// A single file discovered by the crawler.
#[derive(Debug, Clone)]
pub struct CrawledFile {
    pub abs_path: PathBuf,
    pub rel_path: PathBuf, // relative to the root passed to `crawl`
    pub size_bytes: u64,
    pub mtime: i64, // seconds since UNIX epoch
    pub media_type: Option<lodestash_core::types::MediaType>,
    /// BLAKE3 of the file contents — the exact-duplicate key (DATA-MODEL §6). Filled by the
    /// caller for persisted (browseable) files only, since hashing every dropped file would
    /// be wasted IO; `None` otherwise.
    pub hash: Option<String>,
}

/// Crawl `root` recursively and yield every content file.
/// Skips hidden directories (`.` prefix), noise directories (`target/`, `node_modules/`), and any
/// path matching the per-source `excludes` blacklist (folders are pruned so their subtrees are
/// never walked — INGESTION.md §1, DATA-MODEL.md §1). Never skips the root itself (depth 0).
/// Excludes are non-destructive: they only affect cataloguing, never files on disk.
pub fn crawl(root: &Path, excludes: &[Exclude]) -> Vec<CrawledFile> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_skip_dir(e) && !is_excluded_entry(e, root, excludes))
        .filter_map(|entry| entry.ok())
        .filter(|e| e.file_type().is_file() && !is_meta_sidecar(e.path()))
        .filter_map(|entry| {
            let meta = entry.metadata().ok()?;
            let abs_path = entry.path().to_path_buf();
            // strip_prefix uses the same path style as WalkDir (consistent, no canonicalize needed)
            let rel_path = abs_path.strip_prefix(root).ok()?.to_path_buf();
            let size_bytes = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let media_type = detect::by_extension(&abs_path).or_else(|| {
                // Magic-byte fallback (INGESTION §1): an unknown extension is not proof of
                // non-content — a `.bin` texture or a headerless export reads as a real type
                // from its first bytes. 16 bytes is one cheap read per *unclassified* file,
                // which is the rare case; classified files never pay it.
                let mut header = [0u8; 16];
                let n = std::io::Read::read(
                    &mut std::fs::File::open(&abs_path).ok()?,
                    &mut header,
                )
                .ok()?;
                detect::by_magic(&header[..n])
            });
            Some(CrawledFile {
                abs_path,
                rel_path,
                size_bytes,
                mtime,
                media_type,
                hash: None,
            })
        })
        .collect()
}

/// Unity `.meta` sidecars are bookkeeping, never content — they are read for GUIDs/labels
/// during package ingest (`unitypackage::parse_meta`) but must not appear as assets.
fn is_meta_sidecar(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("meta"))
}

/// Content-hash a file with BLAKE3, streaming (constant memory even for multi-GB packages).
/// Returns the hex digest string.
pub fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Incremental diff: given a crawl result and the DB's last known (size, mtime) for a rel_path,
/// return true if the file needs re-hashing (changed or new).
pub fn needs_rehash(file: &CrawledFile, db_size: Option<u64>, db_mtime: Option<i64>) -> bool {
    match (db_size, db_mtime) {
        (Some(s), Some(m)) => file.size_bytes != s || file.mtime != m,
        _ => true, // new file
    }
}

/// Returns true if this walkdir entry is a directory that should be skipped (along with
/// all its contents). The root entry (depth == 0) is **never** skipped regardless of its name.
fn is_skip_dir(entry: &walkdir::DirEntry) -> bool {
    if !entry.file_type().is_dir() || entry.depth() == 0 {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    name.starts_with('.') || name == "target" || name == "node_modules"
}

/// True if this entry (dir or file, but never the root) matches the blacklist. Applied in
/// `filter_entry` so excluded **folders are pruned** (their subtrees aren't descended) and
/// excluded files are dropped before yielding.
fn is_excluded_entry(entry: &walkdir::DirEntry, root: &Path, excludes: &[Exclude]) -> bool {
    if entry.depth() == 0 || excludes.is_empty() {
        return false;
    }
    match entry.path().strip_prefix(root) {
        Ok(rel) => is_excluded(rel, excludes),
        Err(_) => false,
    }
}

/// Current time as seconds since UNIX epoch.
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn crawl_finds_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        fs::write(root.join("a.png"), b"PNG_data").unwrap();
        fs::write(root.join("b.wav"), b"RIFF_data").unwrap();
        fs::create_dir(root.join(".hidden")).unwrap();
        fs::write(root.join(".hidden").join("skip.png"), b"PNG_data").unwrap();

        let files = crawl(root, &[]);
        assert_eq!(
            files.len(),
            2,
            "hidden dir should be skipped; got: {files:?}"
        );
    }

    #[test]
    fn crawl_honors_folder_and_asset_excludes() {
        use lodestash_core::types::{Exclude, ExcludeKind};
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        fs::write(root.join("keep.png"), b"PNG").unwrap();
        fs::write(root.join("skip.png"), b"PNG").unwrap();
        fs::create_dir(root.join("Editor")).unwrap();
        fs::write(root.join("Editor").join("tool.png"), b"PNG").unwrap();
        fs::write(root.join("Editor").join("more.png"), b"PNG").unwrap();

        let excludes = vec![
            Exclude {
                kind: ExcludeKind::Folder,
                value: "Editor".into(),
            },
            Exclude {
                kind: ExcludeKind::Asset,
                value: "skip.png".into(),
            },
        ];
        let files = crawl(root, &excludes);
        let names: Vec<_> = files
            .iter()
            .map(|f| f.rel_path.to_string_lossy().into_owned())
            .collect();
        assert_eq!(files.len(), 1, "only keep.png survives; got: {names:?}");
        assert_eq!(names[0], "keep.png");
    }

    #[test]
    fn crawl_ignores_unity_meta_sidecars() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("rock.fbx"), b"mesh").unwrap();
        std::fs::write(dir.path().join("rock.fbx.meta"), b"guid: abc").unwrap();
        std::fs::write(dir.path().join("Rock2.FBX.META"), b"guid: def").unwrap();
        let files = crawl(dir.path(), &[]);
        assert_eq!(files.len(), 1, "sidecars skipped, case-insensitively");
        assert!(files[0].rel_path.to_string_lossy().ends_with("rock.fbx"));
    }

    #[test]
    fn crawl_rel_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("mesh.fbx"), b"FBX").unwrap();

        let files = crawl(root, &[]);
        assert_eq!(files.len(), 1);
        // rel_path must be relative, not absolute
        assert!(!files[0].rel_path.is_absolute());
        assert_eq!(
            files[0].media_type,
            Some(lodestash_core::types::MediaType::Model3d)
        );
    }

    #[test]
    fn hash_file_deterministic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("test.bin");
        fs::write(&p, b"hello world").unwrap();
        let h1 = hash_file(&p).unwrap();
        let h2 = hash_file(&p).unwrap();
        assert_eq!(h1, h2);
        assert!(!h1.is_empty());
    }

    #[test]
    fn needs_rehash_detects_change() {
        let file = CrawledFile {
            abs_path: PathBuf::from("a"),
            rel_path: PathBuf::from("a"),
            size_bytes: 100,
            mtime: 999,
            media_type: None,
            hash: None,
        };
        assert!(!needs_rehash(&file, Some(100), Some(999)), "unchanged");
        assert!(needs_rehash(&file, Some(101), Some(999)), "size changed");
        assert!(needs_rehash(&file, Some(100), Some(1000)), "mtime changed");
        assert!(needs_rehash(&file, None, None), "new file");
    }
}
