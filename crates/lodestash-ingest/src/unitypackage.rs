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

/// Pack-level metadata harvested from a `.unitypackage` (DATA-MODEL §7, INGESTION §2): the icon
/// the Asset Store ships with every pack, plus whatever licence and version the pack states about
/// itself.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PackageMeta {
    /// Raw bytes of the pack's `.icon.png`, when it has one (Asset Store packs always do).
    pub icon_png: Option<Vec<u8>>,
    /// From an embedded UPM `package.json`.
    pub version: Option<String>,
    pub display_name: Option<String>,
    /// SPDX-ish identifier: from `package.json`'s `license` field, or recognised from a bundled
    /// LICENSE file. `None` means "the pack does not say" — the caller supplies the Asset Store
    /// EULA default, since that is a fact about *where the pack came from*, not about the pack.
    pub license: Option<String>,
    pub license_url: Option<String>,
}

/// Largest text payload worth reading while harvesting: a licence or manifest that exceeds this
/// is not one.
const META_PAYLOAD_CAP: u64 = 256 * 1024;
/// The pack icon is a small square PNG; anything larger is not it.
const ICON_CAP: u64 = 4 * 1024 * 1024;

/// Does this in-pack path look like a licence document?
fn is_license_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let stem = name.split('.').next().unwrap_or(&name);
    matches!(stem, "license" | "licence" | "copying" | "eula")
        || stem.starts_with("third-party")
        || stem.starts_with("third_party")
}

fn is_package_json(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|n| n == "package.json")
}

/// Recognise a licence from its text. Deliberately a short table of the identifiers that actually
/// show up in game-asset packs, matched on the distinctive phrase rather than the title line — a
/// file headed "License" says nothing, its body says which one.
///
/// ponytail: no SPDX matcher crate for six needles. Add one when a pack ships something exotic
/// *and* the answer matters.
pub fn detect_license(text: &str) -> Option<String> {
    let head: String = text.chars().take(4000).collect::<String>().to_lowercase();
    let id = if head.contains("mit license")
        || head.contains("permission is hereby granted, free of charge")
    {
        "MIT"
    } else if head.contains("apache license") {
        "Apache-2.0"
    } else if head.contains("creative commons zero") || head.contains("cc0 1.0") {
        "CC0-1.0"
    } else if head.contains("creative commons attribution") {
        "CC-BY-4.0"
    } else if head.contains("gnu general public license") {
        "GPL-3.0"
    } else if head.contains("redistribution and use in source and binary forms") {
        "BSD-3-Clause"
    } else if head.contains("unity asset store") {
        "Unity Asset Store EULA"
    } else {
        return None;
    };
    Some(id.to_string())
}

/// Pull `version`, `displayName` and `license` out of a UPM `package.json` without a schema: the
/// file is small and only three fields matter.
fn parse_package_json(text: &str, meta: &mut PackageMeta) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let get = |key: &str| v.get(key).and_then(|x| x.as_str()).map(str::to_string);
    // Never overwrite a value an earlier (outer) manifest set: a pack can embed several UPM
    // packages, and the first one seen is the closest to the pack root.
    if meta.version.is_none() {
        meta.version = get("version");
    }
    if meta.display_name.is_none() {
        meta.display_name = get("displayName");
    }
    if meta.license.is_none() {
        meta.license = get("license");
    }
    if meta.license_url.is_none() {
        meta.license_url = get("licensesUrl");
    }
}

/// Harvest [`PackageMeta`] from a `.unitypackage`, given the manifest [`read_manifest`] already
/// produced (which is how the browse pass learns the pathnames without buffering payloads).
///
/// Runs a second, targeted pass: only the pack icon and the handful of GUID folders whose pathname
/// looks like a manifest or a licence are read, and the scan stops as soon as every candidate has
/// been seen — a 20 GB pack is not re-inflated for a 2 KB file it does not have.
pub fn read_package_meta(path: &Path, entries: &[PackageEntry]) -> io::Result<PackageMeta> {
    // guid → (is_package_json)
    let wanted: BTreeMap<&str, bool> = entries
        .iter()
        .filter_map(|e| {
            let p = e.pathname.as_deref()?;
            // Only real payloads; a folder entry has no `asset`.
            // ponytail: map_or, not is_none_or — the workspace MSRV is 1.80.
            if e.asset_size
                .map_or(true, |n| n == 0 || n > META_PAYLOAD_CAP)
            {
                return None;
            }
            if is_package_json(p) {
                Some((e.guid.as_str(), true))
            } else if is_license_path(p) {
                Some((e.guid.as_str(), false))
            } else {
                None
            }
        })
        .collect();

    let mut meta = PackageMeta::default();
    let file = File::open(path)?;
    let gz = GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    let mut icon_seen = false;
    let mut found = 0usize;

    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_path = entry.path()?.to_path_buf();
        let name = entry_path.to_string_lossy().replace('\\', "/");
        if name == ".icon.png" {
            icon_seen = true;
            if entry.header().size().unwrap_or(0) <= ICON_CAP {
                let mut buf = Vec::new();
                entry.read_to_end(&mut buf)?;
                meta.icon_png = Some(buf);
            }
        } else if let Some((guid, member)) = guid_and_member(&entry_path) {
            if member == "asset" {
                if let Some(&is_json) = wanted.get(guid.as_str()) {
                    let mut text = String::new();
                    // A binary payload that happened to be named LICENSE is simply skipped.
                    if entry.read_to_string(&mut text).is_ok() {
                        if is_json {
                            parse_package_json(&text, &mut meta);
                        } else if meta.license.is_none() {
                            meta.license = detect_license(&text);
                        }
                    }
                    found += 1;
                }
            }
        }
        // Everything asked for has been read; the rest of a multi-GB archive is not our business.
        if found >= wanted.len() && (icon_seen || meta.icon_png.is_some()) {
            break;
        }
    }
    Ok(meta)
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
    read_manifest_with_progress(path, &|_, _| true)
}

/// [`read_manifest`] with progress + cancel. `.unitypackage` is tar.gz with no random access, so
/// even reading only the small manifest entries means **decompressing the whole archive** (the tiny
/// `pathname`/`asset.meta` entries sit behind the huge `asset` payloads, and gzip can't seek). On a
/// 20 GB pack that is real CPU, so the read reports **compressed bytes consumed vs. the file size**
/// for a progress bar, and `on_progress` returning `false` cancels it (errors out).
pub fn read_manifest_with_progress(
    path: &Path,
    on_progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<Vec<PackageEntry>> {
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

/// Wraps a reader and reports cumulative bytes read after every chunk. The callback
/// returns `false` to abort the extraction (cooperative cancel): the read errors with
/// `ErrorKind::Interrupted`... which `io` retries — so `Other` is used instead.
struct ProgressReader<'a, R> {
    inner: R,
    read: u64,
    total: u64,
    on_progress: &'a (dyn Fn(u64, u64) -> bool + Sync),
}

/// Sentinel message a [`ProgressReader`] raises when its callback cancels, so callers can tell a
/// user-requested stop apart from a genuine I/O failure.
const CANCEL_MSG: &str = "extraction cancelled";

/// Was this error a cooperative cancel (vs. a real I/O failure)?
fn is_cancel(e: &io::Error) -> bool {
    e.get_ref().map(|inner| inner.to_string()) == Some(CANCEL_MSG.to_string())
}

impl<R: Read> Read for ProgressReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        if !(self.on_progress)(self.read, self.total) {
            return Err(io::Error::other(CANCEL_MSG));
        }
        Ok(n)
    }
}

/// Ingest pass: one streaming extract of every `asset` payload into `dest/<blake3-hex>/<basename>`.
/// Content-addressed and idempotent — if the destination already exists (same bytes, same hash),
/// the freshly-extracted temp copy is discarded rather than rewritten.
pub fn extract_to_workspace(path: &Path, dest: &Path) -> io::Result<Vec<ExtractedAsset>> {
    extract_to_workspace_with_progress(path, dest, &|_, _| true)
}

/// [`extract_to_workspace`] with byte-level progress: `on_progress(read, total)` is called with
/// **compressed** bytes consumed vs. the archive's file size — accurate activity even when the
/// entry count isn't known upfront (streamed tar). Return `false` to cancel: extraction stops and
/// returns **what it placed so far** (a cancel is not an error). Payloads are placed incrementally
/// and content-addressed, so a cancelled or crashed run leaves valid files and a re-run **resumes**,
/// skipping them by hash.
pub fn extract_to_workspace_with_progress(
    path: &Path,
    dest: &Path,
    on_progress: &(dyn Fn(u64, u64) -> bool + Sync),
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

    // Place each payload **as soon as both its `asset` bytes and its `pathname` are seen** (they are
    // adjacent entries within a GUID folder), so an interrupted run leaves valid, content-addressed
    // files on disk. A re-run skips those by hash and continues — this is what makes a 21 GB
    // extraction resumable (gzip can't seek, so a resume still inflates, but never re-writes what
    // is already placed).
    let mut pathnames: BTreeMap<String, String> = BTreeMap::new();
    let mut pending: BTreeMap<String, (PathBuf, String, u64)> = BTreeMap::new();
    let mut extracted = Vec::new();

    let walk: io::Result<()> = (|| {
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
                    let pn = parse_pathname(&s);
                    if let Some((tmp, hash, size)) = pending.remove(&guid) {
                        extracted.push(place_payload(dest, guid, Some(pn), tmp, hash, size)?);
                    } else {
                        pathnames.insert(guid, pn);
                    }
                }
                "asset" => {
                    let tmp_path = tmp_dir.join(&guid);
                    let (hash, size) = stream_hash_to_file(&mut entry, &tmp_path)?;
                    if let Some(pn) = pathnames.remove(&guid) {
                        extracted.push(place_payload(dest, guid, Some(pn), tmp_path, hash, size)?);
                    } else {
                        pending.insert(guid, (tmp_path, hash, size));
                    }
                }
                _ => {}
            }
        }
        Ok(())
    })();

    // Assets fully read before the walk stopped but still awaiting a pathname are complete — place
    // them (guid basename if the pathname never came) so they count toward a resume.
    for (guid, (tmp, hash, size)) in std::mem::take(&mut pending) {
        let pn = pathnames.remove(&guid);
        extracted.push(place_payload(dest, guid, pn, tmp, hash, size)?);
    }
    let _ = std::fs::remove_dir(&tmp_dir); // best-effort; empty once every payload is moved out

    // Cancel is not a failure: the placed files are valid and a re-run resumes from them. Any other
    // I/O error still propagates, but what was already placed likewise survives for a manual retry.
    if let Err(e) = walk {
        if !is_cancel(&e) {
            return Err(e);
        }
    }
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
    wanted: Vec<EntryLocator>,
) -> io::Result<Vec<ExtractedAsset>> {
    let gz = GzDecoder::new(File::open(path)?);
    extract_at_offsets_from(gz, dest, wanted)
}

/// Like [`extract_at_offsets`] but drives `on_progress(compressed_read, compressed_total)` as it
/// skip-scans and stops (clean cancel) when the callback returns false. Progress is in **compressed**
/// bytes — the honest denominator, since the cost is inflating the gzip up to the payload's offset (a
/// big pack has no random access, INGESTION §2). A cancel surfaces as the sentinel error
/// [`is_cancel`] recognizes; on a huge pack this is what lets a single-file preview pull show a real
/// bar and be aborted instead of appearing frozen.
pub fn extract_at_offsets_with_progress(
    path: &Path,
    dest: &Path,
    wanted: Vec<EntryLocator>,
    on_progress: &(dyn Fn(u64, u64) -> bool + Sync),
) -> io::Result<Vec<ExtractedAsset>> {
    let file = File::open(path)?;
    let total = file.metadata()?.len();
    let reader = ProgressReader {
        inner: file,
        read: 0,
        total,
        on_progress,
    };
    let gz = GzDecoder::new(reader);
    extract_at_offsets_from(gz, dest, wanted)
}

/// Shared skip-scan loop for both surgical-extract entry points — generic over the reader so the
/// plain and progress-reporting variants differ only in how the gzip stream is wrapped.
fn extract_at_offsets_from<R: Read>(
    mut gz: GzDecoder<R>,
    dest: &Path,
    mut wanted: Vec<EntryLocator>,
) -> io::Result<Vec<ExtractedAsset>> {
    wanted.sort_by_key(|w| w.offset);
    let tmp_dir = dest.join(".tmp");
    std::fs::create_dir_all(&tmp_dir)?;

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

/// Upper bound on the capacity hint when pulling a payload into memory — guards against a
/// corrupt manifest size triggering a huge speculative allocation. `take(size)` still limits
/// the actual read, so a legitimately larger payload simply grows past the hint.
const PULL_CAP_HINT: usize = 64 * 1024 * 1024;

/// Pull **one** payload out of a `.unitypackage` **into memory** — the in-pack identify path
/// (DATA-MODEL §7 / M14). One gzip skip-scan to `offset`, read `size` bytes, return them; no file
/// is written (unlike [`extract_at_offsets`]), so identifying an in-pack asset leaves nothing on
/// disk. A short read means a stale locator (the archive changed) and errors rather than handing
/// back a truncated buffer. The caller decides whether a payload is small enough to pull into RAM.
pub fn read_payload_at(path: &Path, offset: u64, size: u64) -> io::Result<Vec<u8>> {
    let mut gz = GzDecoder::new(File::open(path)?);
    io::copy(&mut (&mut gz).take(offset), &mut io::sink())?;
    let mut buf = Vec::with_capacity((size as usize).min(PULL_CAP_HINT));
    let n = (&mut gz).take(size).read_to_end(&mut buf)?;
    if n as u64 != size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("payload truncated: wanted {size} got {n} bytes"),
        ));
    }
    Ok(buf)
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
    fn extraction_is_idempotent_and_survives_cancel() {
        // A package with two GUID folders (two payloads).
        let dir = tempfile::tempdir().unwrap();
        let pkg_path = dir.path().join("two.unitypackage");
        {
            let gz = flate2::write::GzEncoder::new(
                File::create(&pkg_path).unwrap(),
                flate2::Compression::default(),
            );
            let mut b = tar::Builder::new(gz);
            let mut add = |name: String, bytes: &[u8]| {
                let mut h = tar::Header::new_gnu();
                h.set_size(bytes.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                b.append_data(&mut h, name, bytes).unwrap();
            };
            for (guid, file) in [("g1", "One.png"), ("g2", "Two.png")] {
                add(format!("{guid}/asset"), &b"payload".repeat(200));
                add(
                    format!("{guid}/pathname"),
                    format!("Assets/{file}").as_bytes(),
                );
            }
            b.into_inner().unwrap().finish().unwrap();
        }

        // Immediate cancel: returns Ok (a cancel is not a failure), not an Err.
        let ws = dir.path().join("ws");
        let cancelled = extract_to_workspace_with_progress(&pkg_path, &ws, &|_, _| false).unwrap();
        assert!(
            cancelled.len() < 2,
            "an immediate cancel doesn't place everything"
        );

        // A re-run completes the extraction (resumes), skipping anything already placed.
        let full = extract_to_workspace_with_progress(&pkg_path, &ws, &|_, _| true).unwrap();
        assert_eq!(full.len(), 2, "re-run completes all payloads");

        // Idempotent: extracting again places nothing new and points at the same files.
        let again = extract_to_workspace_with_progress(&pkg_path, &ws, &|_, _| true).unwrap();
        assert_eq!(again.len(), 2);
        for a in &again {
            assert!(a.workspace_path.exists());
        }
    }

    #[test]
    fn read_manifest_with_progress_reports_and_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = build_test_package(
            dir.path(),
            "abc123",
            "Assets/Props/Barrel.png",
            "guid: abc123\n",
            &b"payload".repeat(500),
        );
        // Progress fires with a non-zero total (the compressed file size).
        let seen_total = std::sync::atomic::AtomicU64::new(0);
        let out = read_manifest_with_progress(&pkg, &|_read, total| {
            seen_total.store(total, std::sync::atomic::Ordering::Relaxed);
            true
        })
        .unwrap();
        assert_eq!(out.len(), 1);
        assert!(seen_total.load(std::sync::atomic::Ordering::Relaxed) > 0);

        // Returning false cancels — the read errors rather than completing.
        let cancelled = read_manifest_with_progress(&pkg, &|_, _| false);
        assert!(cancelled.is_err(), "cancel surfaces as an error");
    }

    #[test]
    fn read_payload_at_pulls_one_payload_into_memory() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"in-memory identify payload".repeat(50); // spans tar blocks
        let pkg = build_test_package(
            dir.path(),
            "aaaa1111bbbb2222cccc3333dddd4444",
            "Assets/Textures/Rock.png",
            "guid: aaaa1111bbbb2222cccc3333dddd4444\n",
            &payload,
        );
        let entry = &read_manifest(&pkg).unwrap()[0];
        let (offset, size) = (entry.asset_offset.unwrap(), entry.asset_size.unwrap());

        let got = read_payload_at(&pkg, offset, size).unwrap();
        assert_eq!(
            got, payload,
            "in-memory pull is byte-identical, no file written"
        );

        // A stale locator (size past EOF) errors rather than returning a short buffer.
        assert!(read_payload_at(&pkg, offset, size + 4096).is_err());
    }

    #[test]
    fn license_text_is_recognised_by_its_body_not_its_title() {
        assert_eq!(
            detect_license(
                "MIT License

Permission is hereby granted, free of charge"
            ),
            Some("MIT".into())
        );
        assert_eq!(
            detect_license(
                "                     Apache License
     Version 2.0"
            ),
            Some("Apache-2.0".into())
        );
        assert_eq!(
            detect_license("Creative Commons Attribution 4.0 International"),
            Some("CC-BY-4.0".into())
        );
        // A file headed "License" that says nothing recognisable stays unknown rather than
        // guessing — a wrong licence is worse than no licence.
        assert_eq!(
            detect_license(
                "License

See our website."
            ),
            None
        );
        assert_eq!(detect_license(""), None);
    }

    #[test]
    fn license_and_manifest_paths_are_spotted() {
        for p in [
            "Assets/Pack/LICENSE",
            "Assets/Pack/license.txt",
            "Assets/Pack/COPYING.md",
            "Assets/EULA.txt",
            "Assets/Third-Party Notices.md",
        ] {
            assert!(is_license_path(p), "{p} should look like a licence");
        }
        for p in ["Assets/Pack/LicensePlate.fbx", "Assets/Readme.txt"] {
            assert!(!is_license_path(p), "{p} should not");
        }
        assert!(is_package_json("Packages/com.x.y/package.json"));
        assert!(!is_package_json("Assets/manifest/package.json.bak"));
    }

    #[test]
    fn package_json_fills_version_and_licence_without_overwriting() {
        let mut meta = PackageMeta::default();
        parse_package_json(
            r#"{"name":"com.x.y","displayName":"Pathfinding","version":"5.2.1","license":"Custom"}"#,
            &mut meta,
        );
        assert_eq!(meta.version.as_deref(), Some("5.2.1"));
        assert_eq!(meta.display_name.as_deref(), Some("Pathfinding"));
        assert_eq!(meta.license.as_deref(), Some("Custom"));
        // A nested package must not overwrite the outer one's answer.
        parse_package_json(r#"{"version":"0.0.1","license":"MIT"}"#, &mut meta);
        assert_eq!(meta.version.as_deref(), Some("5.2.1"));
        assert_eq!(meta.license.as_deref(), Some("Custom"));
        // Malformed JSON is ignored, not fatal.
        parse_package_json("{not json", &mut meta);
        assert_eq!(meta.version.as_deref(), Some("5.2.1"));
    }

    /// Against the real Asset Store cache: every pack there ships a `.icon.png`, and newer ones
    /// embed a UPM manifest. `#[ignore]` — it needs that cache.
    #[test]
    #[ignore = "needs a real Asset Store cache"]
    fn harvest_real_packages() {
        let root = std::path::PathBuf::from(
            std::env::var("ASSET_STORE_ROOT").expect("set ASSET_STORE_ROOT"),
        );
        let mut seen = 0;
        for entry in walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("unitypackage"))
            })
            .take(6)
        {
            let path = entry.path();
            let entries = read_manifest(path).expect("manifest");
            let meta = read_package_meta(path, &entries).expect("meta");
            println!(
                "{:44} icon={:>7} version={:?} license={:?}",
                path.file_stem().unwrap().to_string_lossy(),
                meta.icon_png.as_ref().map_or(0, |b| b.len()),
                meta.version,
                meta.license
            );
            assert!(meta.icon_png.is_some(), "Asset Store packs carry an icon");
            seen += 1;
        }
        assert!(seen > 0, "no packages found under ASSET_STORE_ROOT");
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
