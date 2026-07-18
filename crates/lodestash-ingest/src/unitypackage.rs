//! `.unitypackage` parsing: gzip+tar of GUID folders, each holding `asset` (raw bytes),
//! `asset.meta` (YAML: GUID/importer/labels), and `pathname` (the real Assets/ path).
//! Spec: INGESTION.md §2 (archive memory model), §3 step 3.
//!
//! Two passes, matching the spec's memory model:
//! - [`read_manifest`] — browse pass. Reads only the small entries (`pathname`, `asset.meta`);
//!   the `asset` payload's size comes from the tar header, never its body. O(1) memory regardless
//!   of package size.
//! - [`extract_to_workspace`] — ingest pass. One streaming extract of every `asset` payload into a
//!   content-addressed (BLAKE3) workspace directory, idempotent by hash.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;

/// Parsed `.meta` sidecar: GUID, Unity importer type, and author-assigned labels (seed tags).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnityMeta {
    pub guid: Option<String>,
    /// The importer type key (e.g. `TextureImporter`, `ModelImporter`) — the first top-level YAML
    /// key that isn't one of the known administrative fields. Feeds non-content classification
    /// (INGESTION.md §3).
    pub importer: Option<String>,
    pub labels: Vec<String>,
}

/// Top-level `.meta` keys that are never an importer type.
const META_ADMIN_KEYS: &[&str] = &[
    "fileFormatVersion",
    "guid",
    "labels",
    "assetBundleName",
    "assetBundleVariant",
];

/// Parse a Unity `.meta` YAML document. Malformed YAML yields an empty `UnityMeta` rather than
/// failing the whole package — one bad sidecar shouldn't abort ingest of everything else.
pub fn parse_meta(yaml: &str) -> UnityMeta {
    let mut meta = UnityMeta::default();
    let Ok(serde_yaml::Value::Mapping(map)) = serde_yaml::from_str(yaml) else {
        return meta;
    };
    for (k, v) in &map {
        let Some(key) = k.as_str() else { continue };
        if key == "guid" {
            meta.guid = v.as_str().map(str::to_string);
        } else if key == "labels" {
            if let Some(seq) = v.as_sequence() {
                meta.labels = seq
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect();
            }
        } else if meta.importer.is_none() && !META_ADMIN_KEYS.contains(&key) {
            meta.importer = Some(key.to_string());
        }
    }
    meta
}

/// One GUID folder's worth of manifest data — a browse-pass row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PackageEntry {
    pub guid: String,
    /// Real Assets/-relative path, from the `pathname` entry.
    pub pathname: Option<String>,
    pub meta: UnityMeta,
    /// Byte size of the `asset` payload, read from the tar header only (body never buffered).
    pub asset_size: Option<u64>,
    /// Byte offset of the payload's data in the **decompressed** tar stream — the key to
    /// surgical re-extraction (gzip is sequential, so extraction skip-scans to this offset).
    pub asset_offset: Option<u64>,
    pub has_preview: bool,
}

/// Extract the real path from a `pathname` entry's raw content. Spec is one line, but some
/// third-party repackaging tools append trailing garbage after a newline (observed in the wild:
/// a bundled-variant wrapper package appending a stray `00` line) — only the first line is ever
/// the real path, so anything after a newline is discarded rather than passed through into a
/// filesystem path (a raw newline is an invalid Windows filename character and would panic
/// `File::create`).
fn parse_pathname(raw: &str) -> String {
    raw.lines().next().unwrap_or("").trim().to_string()
}

/// Strip characters that are invalid in a Windows filename (control chars and `<>:"/\|?*`) from
/// a workspace-file basename. Belt-and-suspenders on top of [`parse_pathname`] — a well-formed
/// `pathname` shouldn't hit this, but the workspace write must never panic on hostile input.
fn sanitize_basename(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c))
        .collect()
}

fn guid_and_member(entry_path: &Path) -> Option<(String, String)> {
    let mut components = entry_path.components();
    let guid = components.next()?.as_os_str().to_string_lossy().to_string();
    let member = components.next()?.as_os_str().to_string_lossy().to_string();
    Some((guid, member))
}

/// Browse pass: enumerate every GUID folder's `pathname`/`asset.meta`/`asset` size/`preview.png`
/// presence without ever buffering an `asset` payload's bytes. Returned in GUID order.
pub fn read_manifest(path: &Path) -> io::Result<Vec<PackageEntry>> {
    let file = File::open(path)?;
    let gz = GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let mut by_guid: BTreeMap<String, PackageEntry> = BTreeMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();
        let Some((guid, member)) = guid_and_member(&entry_path) else {
            continue;
        };
        let rec = by_guid.entry(guid.clone()).or_insert_with(|| PackageEntry {
            guid: guid.clone(),
            ..Default::default()
        });

        match member.as_str() {
            "pathname" => {
                let mut s = String::new();
                entry.read_to_string(&mut s)?;
                rec.pathname = Some(parse_pathname(&s));
            }
            "asset.meta" => {
                let mut s = String::new();
                entry.read_to_string(&mut s)?;
                rec.meta = parse_meta(&s);
            }
            "asset" => {
                rec.asset_size = Some(entry.header().size()?);
                rec.asset_offset = Some(entry.raw_file_position());
            }
            "preview.png" => rec.has_preview = true,
            _ => {}
        }
    }

    Ok(by_guid.into_values().collect())
}

/// Entries in a manifest whose `pathname` is itself a `.unitypackage` — a render-pipeline-variant
/// wrapper bundling multiple sub-packages (INGESTION.md §2 "Container nuance"; observed in the
/// wild as a publisher shipping separate URP/Built-in variants inside one outer package).
pub fn nested_package_candidates(manifest: &[PackageEntry]) -> Vec<&PackageEntry> {
    manifest
        .iter()
        .filter(|e| {
            e.pathname
                .as_deref()
                .map(|p| p.to_lowercase().ends_with(".unitypackage"))
                .unwrap_or(false)
        })
        .collect()
}

/// Index into `candidates` of the preferred render-pipeline variant: the first whose pathname
/// mentions URP (case-insensitive), else the first candidate. `candidates` must be non-empty.
pub fn preferred_variant_index(candidates: &[&PackageEntry]) -> usize {
    candidates
        .iter()
        .position(|e| {
            e.pathname
                .as_deref()
                .map(|p| p.to_lowercase().contains("urp"))
                .unwrap_or(false)
        })
        .unwrap_or(0)
}

/// One `asset` payload extracted into the workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct ExtractedAsset {
    pub guid: String,
    pub pathname: Option<String>,
    pub content_hash: String,
    pub workspace_path: PathBuf,
    pub size_bytes: u64,
}

/// Wraps a reader and reports cumulative bytes read after every chunk.
struct ProgressReader<'a, R> {
    inner: R,
    read: u64,
    total: u64,
    on_progress: &'a (dyn Fn(u64, u64) + Sync),
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        (self.on_progress)(self.read, self.total);
        Ok(n)
    }
}

/// Ingest pass: one streaming extract of every `asset` payload into `dest/<blake3-hex>/<basename>`.
/// Content-addressed and idempotent — if the destination already exists (same bytes, same hash),
/// the freshly-extracted temp copy is discarded rather than rewritten.
pub fn extract_to_workspace(path: &Path, dest: &Path) -> io::Result<Vec<ExtractedAsset>> {
    extract_to_workspace_with_progress(path, dest, &|_, _| {})
}

/// [`extract_to_workspace`] with byte-level progress: `on_progress(read, total)` is called with
/// **compressed** bytes consumed vs. the archive's file size — accurate activity even when the
/// entry count isn't known upfront (streamed tar).
pub fn extract_to_workspace_with_progress(
    path: &Path,
    dest: &Path,
    on_progress: &(dyn Fn(u64, u64) + Sync),
) -> io::Result<Vec<ExtractedAsset>> {
    let tmp_dir = dest.join(".tmp");
    std::fs::create_dir_all(&tmp_dir)?;

    let file = File::open(path)?;
    let total = file.metadata()?.len();
    let file = ProgressReader {
        inner: file,
        read: 0,
        total,
        on_progress,
    };
    let gz = GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let mut pathnames: BTreeMap<String, String> = BTreeMap::new();
    let mut payloads: BTreeMap<String, (PathBuf, String, u64)> = BTreeMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();
        let Some((guid, member)) = guid_and_member(&entry_path) else {
            continue;
        };

        match member.as_str() {
            "pathname" => {
                let mut s = String::new();
                entry.read_to_string(&mut s)?;
                pathnames.insert(guid, parse_pathname(&s));
            }
            "asset" => {
                let tmp_path = tmp_dir.join(&guid);
                let (hash, size) = stream_hash_to_file(&mut entry, &tmp_path)?;
                payloads.insert(guid, (tmp_path, hash, size));
            }
            _ => {}
        }
    }

    let mut extracted = Vec::with_capacity(payloads.len());
    for (guid, (tmp_path, hash, size)) in payloads {
        let pathname = pathnames.get(&guid).cloned();
        let basename = pathname
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .map(|n| sanitize_basename(&n.to_string_lossy()))
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| guid.clone());

        let out_dir = dest.join(&hash);
        std::fs::create_dir_all(&out_dir)?;
        let out_path = out_dir.join(&basename);
        if out_path.exists() {
            std::fs::remove_file(&tmp_path)?;
        } else {
            std::fs::rename(&tmp_path, &out_path)?;
        }
        extracted.push(ExtractedAsset {
            guid,
            pathname,
            content_hash: hash,
            workspace_path: out_path,
            size_bytes: size,
        });
    }
    let _ = std::fs::remove_dir(&tmp_dir); // best-effort; empty once every payload is moved out
    Ok(extracted)
}

/// Where one wanted payload lives inside the decompressed tar (from the catalog's
/// `package_entry` rows or a fresh manifest).
#[derive(Debug, Clone)]
pub struct EntryLocator {
    pub guid: String,
    pub pathname: Option<String>,
    pub offset: u64,
    pub size: u64,
}

/// Surgical extraction (INGESTION.md §2): pull **only** `wanted` payloads with one
/// skip-scan over the gzip stream — decompress-and-discard up to each offset, copy the
/// payload out, never touch the rest. Same content-addressed, idempotent placement as
/// [`extract_to_workspace`]. gzip has no random access, so cost is O(bytes up to the
/// last wanted offset) — still no tar parsing, no writes for unwanted entries.
pub fn extract_at_offsets(
    path: &Path,
    dest: &Path,
    mut wanted: Vec<EntryLocator>,
) -> io::Result<Vec<ExtractedAsset>> {
    wanted.sort_by_key(|w| w.offset);
    let tmp_dir = dest.join(".tmp");
    std::fs::create_dir_all(&tmp_dir)?;

    let mut gz = GzDecoder::new(File::open(path)?);
    let mut pos: u64 = 0;
    let mut extracted = Vec::with_capacity(wanted.len());
    for w in wanted {
        if w.offset < pos {
            continue; // overlapping/duplicate locator — already consumed
        }
        io::copy(&mut (&mut gz).take(w.offset - pos), &mut io::sink())?;
        pos = w.offset;
        let tmp_path = tmp_dir.join(&w.guid);
        let (hash, size) = {
            let mut region = (&mut gz).take(w.size);
            stream_hash_to_file(&mut region, &tmp_path)?
        };
        pos += size;
        if size != w.size {
            // Truncated read: stale locator (package changed on disk). Fail loudly —
            // silently placing a short payload would poison the content-addressed store.
            let _ = std::fs::remove_file(&tmp_path);
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "payload {} truncated: wanted {} got {size} bytes",
                    w.guid, w.size
                ),
            ));
        }
        extracted.push(place_payload(
            dest, w.guid, w.pathname, tmp_path, hash, size,
        )?);
    }
    let _ = std::fs::remove_dir(&tmp_dir);
    Ok(extracted)
}

/// Content-addressed placement shared by full and surgical extraction: move the hashed
/// temp payload to `dest/<hash>/<basename>`, discarding it if already present.
fn place_payload(
    dest: &Path,
    guid: String,
    pathname: Option<String>,
    tmp_path: PathBuf,
    hash: String,
    size: u64,
) -> io::Result<ExtractedAsset> {
    let basename = pathname
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| sanitize_basename(&n.to_string_lossy()))
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| guid.clone());
    let out_dir = dest.join(&hash);
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join(&basename);
    if out_path.exists() {
        std::fs::remove_file(&tmp_path)?;
    } else {
        std::fs::rename(&tmp_path, &out_path)?;
    }
    Ok(ExtractedAsset {
        guid,
        pathname,
        content_hash: hash,
        workspace_path: out_path,
        size_bytes: size,
    })
}

/// Stream a tar entry to `dest` while hashing it, without holding the payload in memory.
fn stream_hash_to_file(entry: &mut impl Read, dest: &Path) -> io::Result<(String, u64)> {
    let mut out = File::create(dest)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    let mut size: u64 = 0;
    loop {
        let n = entry.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])?;
        size += n as u64;
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal but real `.unitypackage`: one GUID folder with `asset`/`asset.meta`/`pathname`.
    fn build_test_package(
        dir: &Path,
        guid: &str,
        pathname: &str,
        meta_yaml: &str,
        asset_bytes: &[u8],
    ) -> PathBuf {
        let pkg_path = dir.join("test.unitypackage");
        let file = File::create(&pkg_path).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(gz);

        let mut append = |member: &str, bytes: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("{guid}/{member}"), bytes)
                .unwrap();
        };
        append("asset", asset_bytes);
        append("asset.meta", meta_yaml.as_bytes());
        append("pathname", pathname.as_bytes());

        builder.into_inner().unwrap().finish().unwrap();
        pkg_path
    }

    #[test]
    fn surgical_extraction_matches_full_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"surgical payload bytes".repeat(40); // >1 tar block
        let pkg = build_test_package(
            dir.path(),
            "aaaa1111bbbb2222cccc3333dddd4444",
            "Assets/Props/Barrel.fbx",
            "guid: aaaa1111bbbb2222cccc3333dddd4444
",
            &payload,
        );

        // Manifest records the payload's offset in the decompressed tar.
        let manifest = read_manifest(&pkg).unwrap();
        let entry = &manifest[0];
        let (offset, size) = (entry.asset_offset.unwrap(), entry.asset_size.unwrap());
        assert_eq!(size, payload.len() as u64);
        assert!(offset > 0, "payload sits after its tar header");

        // Full extraction (ground truth) into workspace A.
        let full_ws = dir.path().join("full");
        let full = extract_to_workspace(&pkg, &full_ws).unwrap();

        // Surgical extraction from the recorded offset into workspace B.
        let surgical_ws = dir.path().join("surgical");
        let got = extract_at_offsets(
            &pkg,
            &surgical_ws,
            vec![EntryLocator {
                guid: entry.guid.clone(),
                pathname: entry.pathname.clone(),
                offset,
                size,
            }],
        )
        .unwrap();

        assert_eq!(got.len(), 1);
        assert_eq!(
            got[0].content_hash, full[0].content_hash,
            "same bytes, same address"
        );
        assert_eq!(
            std::fs::read(&got[0].workspace_path).unwrap(),
            payload,
            "surgical payload is byte-identical"
        );

        // A stale locator (wrong size past EOF) must fail loudly, not truncate silently.
        let bad = extract_at_offsets(
            &pkg,
            &surgical_ws,
            vec![EntryLocator {
                guid: "stale".into(),
                pathname: None,
                offset,
                size: size + 4096,
            }],
        );
        assert!(bad.is_err());
    }

    #[test]
    fn parse_meta_extracts_guid_labels_and_importer() {
        let yaml = "fileFormatVersion: 2\nguid: 8f338a7a1b2c3d4e5f6a7b8c9d0e1f2a\nlabels:\n- Prop\n- Medieval\nTextureImporter:\n  mipmaps: 1\n";
        let meta = parse_meta(yaml);
        assert_eq!(
            meta.guid.as_deref(),
            Some("8f338a7a1b2c3d4e5f6a7b8c9d0e1f2a")
        );
        assert_eq!(meta.labels, vec!["Prop", "Medieval"]);
        assert_eq!(meta.importer.as_deref(), Some("TextureImporter"));
    }

    #[test]
    fn parse_meta_malformed_yaml_yields_empty() {
        let meta = parse_meta("not: valid: yaml: [");
        assert_eq!(meta, UnityMeta::default());
    }

    fn entry_with_pathname(pathname: &str) -> PackageEntry {
        PackageEntry {
            guid: "g".into(),
            pathname: Some(pathname.into()),
            meta: UnityMeta::default(),
            asset_size: None,
            asset_offset: None,
            has_preview: false,
        }
    }

    #[test]
    fn nested_package_candidates_finds_only_unitypackage_pathnames() {
        let manifest = vec![
            entry_with_pathname("Assets/Toon Fantasy Nature"),
            entry_with_pathname("Assets/Toon Fantasy Nature/_ReadFirst.txt"),
            entry_with_pathname(
                "Assets/Toon Fantasy Nature/URP_Toon Fantasy Nature_6000.1.0.unitypackage",
            ),
            entry_with_pathname(
                "Assets/Toon Fantasy Nature/BuiltIn_Toon Fantasy Nature_6000.1.0.unitypackage",
            ),
        ];
        let nested = nested_package_candidates(&manifest);
        assert_eq!(nested.len(), 2);
        assert!(nested
            .iter()
            .all(|e| e.pathname.as_deref().unwrap().ends_with(".unitypackage")));
    }

    #[test]
    fn preferred_variant_index_picks_urp_when_present() {
        let a = entry_with_pathname("Assets/BuiltIn_Pack_1.0.unitypackage");
        let b = entry_with_pathname("Assets/URP_Pack_1.0.unitypackage");
        let candidates = vec![&a, &b];
        assert_eq!(preferred_variant_index(&candidates), 1);
    }

    #[test]
    fn preferred_variant_index_falls_back_to_first_when_no_urp() {
        let a = entry_with_pathname("Assets/BuiltIn_Pack_1.0.unitypackage");
        let b = entry_with_pathname("Assets/HDRP_Pack_1.0.unitypackage");
        let candidates = vec![&a, &b];
        assert_eq!(preferred_variant_index(&candidates), 0);
    }

    #[test]
    fn read_manifest_recovers_pathname_meta_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = build_test_package(
            dir.path(),
            "abc123",
            "Assets/Props/Barrel.png",
            "fileFormatVersion: 2\nguid: abc123\nlabels:\n- Prop\nTextureImporter:\n  mipmaps: 1\n",
            b"PNGDATA_PAYLOAD",
        );

        let manifest = read_manifest(&pkg).unwrap();
        assert_eq!(manifest.len(), 1);
        let entry = &manifest[0];
        assert_eq!(entry.guid, "abc123");
        assert_eq!(entry.pathname.as_deref(), Some("Assets/Props/Barrel.png"));
        assert_eq!(entry.meta.labels, vec!["Prop"]);
        assert_eq!(entry.meta.importer.as_deref(), Some("TextureImporter"));
        assert_eq!(entry.asset_size, Some(b"PNGDATA_PAYLOAD".len() as u64));
        assert!(!entry.has_preview);
    }

    #[test]
    fn extract_to_workspace_is_content_addressed_and_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = build_test_package(
            dir.path(),
            "abc123",
            "Assets/Props/Barrel.png",
            "fileFormatVersion: 2\nguid: abc123\n",
            b"PNGDATA_PAYLOAD",
        );
        let workspace = dir.path().join("workspace");

        let extracted = extract_to_workspace(&pkg, &workspace).unwrap();
        assert_eq!(extracted.len(), 1);
        let asset = &extracted[0];
        assert_eq!(asset.pathname.as_deref(), Some("Assets/Props/Barrel.png"));
        assert!(asset.workspace_path.exists());
        assert_eq!(
            std::fs::read(&asset.workspace_path).unwrap(),
            b"PNGDATA_PAYLOAD"
        );
        assert_eq!(asset.workspace_path.file_name().unwrap(), "Barrel.png");

        // idempotent re-extract: same hash, same on-disk path, no error on the second pass
        let extracted_again = extract_to_workspace(&pkg, &workspace).unwrap();
        assert_eq!(extracted_again[0].content_hash, asset.content_hash);
        assert_eq!(extracted_again[0].workspace_path, asset.workspace_path);
    }

    #[test]
    fn pathname_trailing_garbage_line_is_discarded() {
        // Observed in the wild: a third-party repackaging tool appends a stray "00" line after
        // the real path. A raw embedded newline is an invalid Windows filename character, so
        // this must not reach `extract_to_workspace`'s basename derivation.
        let dir = tempfile::tempdir().unwrap();
        let pkg = build_test_package(
            dir.path(),
            "abc123",
            "Assets/Toon Fantasy Nature/_ReadFirst.txt\n00",
            "fileFormatVersion: 2\nguid: abc123\n",
            b"README_PAYLOAD",
        );

        let manifest = read_manifest(&pkg).unwrap();
        assert_eq!(
            manifest[0].pathname.as_deref(),
            Some("Assets/Toon Fantasy Nature/_ReadFirst.txt")
        );

        let workspace = dir.path().join("workspace");
        let extracted = extract_to_workspace(&pkg, &workspace).unwrap();
        assert_eq!(
            extracted[0].workspace_path.file_name().unwrap(),
            "_ReadFirst.txt"
        );
        assert!(extracted[0].workspace_path.exists());
    }
}
