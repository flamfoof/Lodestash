//! Recursive folder crawler with BLAKE3 hashing and (size, mtime) incremental diff.
//! Spec: INGESTION.md §1, DATA-MODEL.md §6.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
}

/// Crawl `root` recursively and yield every content file.
/// Skips hidden directories (`.` prefix) and noise directories (`target/`, `node_modules/`),
/// but **never** the root directory itself (depth 0).
pub fn crawl(root: &Path) -> Vec<CrawledFile> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_skip_dir(e))
        .filter_map(|entry| entry.ok())
        .filter(|e| e.file_type().is_file())
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
            let media_type = detect::by_extension(&abs_path);
            Some(CrawledFile {
                abs_path,
                rel_path,
                size_bytes,
                mtime,
                media_type,
            })
        })
        .collect()
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

        let files = crawl(root);
        assert_eq!(
            files.len(),
            2,
            "hidden dir should be skipped; got: {files:?}"
        );
    }

    #[test]
    fn crawl_rel_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();

        fs::create_dir(root.join("sub")).unwrap();
        fs::write(root.join("sub").join("mesh.fbx"), b"FBX").unwrap();

        let files = crawl(root);
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
        };
        assert!(!needs_rehash(&file, Some(100), Some(999)), "unchanged");
        assert!(needs_rehash(&file, Some(101), Some(999)), "size changed");
        assert!(needs_rehash(&file, Some(100), Some(1000)), "mtime changed");
        assert!(needs_rehash(&file, None, None), "new file");
    }
}
