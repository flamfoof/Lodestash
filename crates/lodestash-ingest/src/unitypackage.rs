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
    /// Render pipelines the pack names — in its own paths and in any embedded UPM manifest's
    /// dependencies (`pipeline::from_path` / `from_manifest`). Collapse with `pipeline::resolve`
    /// to get the single `package.pipeline` value; empty means the pack never said.
    pub pipeline: std::collections::BTreeSet<&'static str>,
}

/// Largest text payload worth reading while harvesting: a licence or manifest that exceeds this
/// is not one.
const META_PAYLOAD_CAP: u64 = 256 * 1024;
/// The pack icon is a small square PNG; anything larger is not it.
const ICON_CAP: u64 = 4 * 1024 * 1024;

/// Most GUID folders one package may list. A real pack holds tens of thousands at the outside;
/// the bound is on what a manifest read keeps in memory for an archive that lists millions.
pub const MAX_PACKAGE_ENTRIES: usize = 500_000;

/// Largest single payload an extraction writes into the workspace. Unity itself cannot import a
/// file near this size; the bound is on how much disk one entry of a hostile (or merely broken)
/// archive can fill — a payload of zeros compresses a thousandfold.
pub const MAX_EXTRACTED_PAYLOAD_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Most bytes a tar extension record — a PAX header, a GNU long name — may hold. They carry a
/// path and a few numbers.
const EXTENSION_RECORD_CAP: u64 = 64 * 1024;

fn invalid(what: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what.into())
}

/// Read `reader` whole if it holds at most `cap` bytes; `None` if it holds more.
///
/// **Decided by reading, never by a declared size.** The reader is limited to `cap + 1` bytes, so
/// nothing an archive says about an entry can make this allocate more. The caps used to be checked
/// against the tar header's size while `read_to_end` followed the PAX size that overrides it: a
/// header saying "0 bytes" passed every cap and the read then took whatever the PAX record said
/// (integrity review, finding 12).
fn read_capped(reader: &mut impl Read, cap: u64) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    reader.by_ref().take(cap + 1).read_to_end(&mut buf)?;
    Ok((buf.len() as u64 <= cap).then_some(buf))
}

/// Whether [`for_each_member`] goes on to the next member.
enum Walk {
    On,
    Stop,
}

/// Walk an archive's members, with every read this module does not make itself bounded too.
///
/// The archive is read **raw**: `tar` then hands over its extension records (PAX `x`, GNU `L`)
/// as ordinary entries instead of reading each into memory whole before the member it describes
/// — which it does for whatever size the record declares. They are applied here, under
/// [`EXTENSION_RECORD_CAP`]: a long name or a PAX `path` names the next member, and everything
/// else in a PAX record (mtime, uid…) is ignored. Two things are refused outright, as
/// `InvalidData`: a record over the cap, and a PAX `size` that disagrees with the header's own —
/// honouring it is how a 512-byte header came to "hold" a multi-gigabyte payload, and a raw walk
/// could not follow it anyway.
///
/// `visit` gets each member's path with `./` dropped (archives packed with `tar -C dir .`), and
/// the entry to read its bytes from; bytes it leaves unread are skipped through a fixed buffer.
fn for_each_member<R: Read>(
    archive: &mut tar::Archive<R>,
    mut visit: impl FnMut(&Path, &mut tar::Entry<'_, R>) -> io::Result<Walk>,
) -> io::Result<()> {
    let lossy_path = |bytes: &[u8]| PathBuf::from(String::from_utf8_lossy(bytes).into_owned());
    let mut named: Option<PathBuf> = None;
    let mut pax_size: Option<u64> = None;
    for entry in archive.entries()?.raw(true) {
        let mut entry = entry?;
        let kind = entry.header().entry_type();
        if kind.is_gnu_longname() || kind.is_pax_local_extensions() {
            let record = read_capped(&mut entry, EXTENSION_RECORD_CAP)?.ok_or_else(|| {
                invalid(format!(
                    "a tar extension record is larger than {EXTENSION_RECORD_CAP} bytes"
                ))
            })?;
            if kind.is_gnu_longname() {
                let end = record.iter().position(|b| *b == 0).unwrap_or(record.len());
                named = Some(lossy_path(&record[..end]));
            } else {
                for field in tar::PaxExtensions::new(&record).flatten() {
                    match field.key() {
                        Ok("path") => named = Some(lossy_path(field.value_bytes())),
                        Ok("size") => {
                            pax_size = Some(
                                std::str::from_utf8(field.value_bytes())
                                    .ok()
                                    .and_then(|v| v.trim().parse().ok())
                                    .ok_or_else(|| invalid("an unreadable PAX size record"))?,
                            );
                        }
                        _ => {}
                    }
                }
            }
            continue;
        }
        if kind.is_gnu_longlink() || kind.is_pax_global_extensions() {
            continue; // nothing in them names or sizes a member
        }
        if let Some(declared) = pax_size.take() {
            if declared != entry.size() {
                return Err(invalid(format!(
                    "a PAX record gives a member {declared} bytes where its header says {} — \
                     not read",
                    entry.size()
                )));
            }
        }
        let path = named
            .take()
            .unwrap_or_else(|| lossy_path(&entry.path_bytes()));
        if let Walk::Stop = visit(&member_path(&path), &mut entry)? {
            break;
        }
    }
    Ok(())
}

/// Does this in-pack path look like a licence document?
fn is_license_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
    let stem = name.split('.').next().unwrap_or(&name);
    matches!(stem, "license" | "licence" | "copying" | "eula")
        || stem.starts_with("third-party")
        || stem.starts_with("third_party")
}

/// Is this in-pack path something [`read_package_meta`] could learn from?
///
/// Callers use it to decide whether to reopen the archive at all: the metadata pass streams until
/// it has found its candidates, so a pack whose manifest declares none would pay a **full
/// inflate** to discover there was nothing to read.
pub fn is_metadata_pathname(path: &str) -> bool {
    is_package_json(path) || is_license_path(path)
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
    // Unlike the fields above this is a union, not a first-writer-wins: a pack embedding one UPM
    // package per pipeline is exactly the `mixed` case, and each manifest is evidence.
    meta.pipeline.extend(crate::pipeline::from_manifest(text));
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
    // Free: the pathnames are already in hand, and a pack that ships a variant per pipeline says
    // so in its folder and sub-package names (INGESTION §2 "Wrapper packages").
    for e in entries {
        if let Some(p) = e.pathname.as_deref() {
            meta.pipeline.extend(crate::pipeline::from_path(p));
        }
    }
    let file = File::open(path)?;
    let gz = GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);
    let mut icon_seen = false;
    let mut found = 0usize;

    for_each_member(&mut archive, |member, entry| {
        let name = member.to_string_lossy().replace('\\', "/");
        if name == ".icon.png" {
            icon_seen = true;
            // Larger than an icon is not the icon — found out by reading, under the cap.
            meta.icon_png = read_capped(entry, ICON_CAP)?;
        } else if let Some((guid, part)) = guid_and_member(member) {
            if part == "asset" {
                if let Some(&is_json) = wanted.get(guid.as_str()) {
                    // The manifest said this one is small; the read holds it to that. A binary
                    // payload that happened to be named LICENSE is simply skipped, like one
                    // that turns out larger than it was listed.
                    let text = read_capped(entry, META_PAYLOAD_CAP)?
                        .and_then(|bytes| String::from_utf8(bytes).ok());
                    if let Some(text) = text {
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
        Ok(if found >= wanted.len() && icon_seen {
            Walk::Stop
        } else {
            Walk::On
        })
    })?;
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
///
/// Trailing dots and spaces go too: Windows drops them when it creates the file, so the name
/// recorded would not be the name on disk — and `NUL.` is still the device.
fn sanitize_basename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !c.is_control() && !r#"<>:"/\|?*"#.contains(*c))
        .collect();
    cleaned.trim_end_matches(['.', ' ']).to_string()
}

/// Is `name` one Windows reserves for a device — `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`,
/// `LPT1`–`LPT9` (and the superscript digits it also accepts) — in any case, with or without an
/// extension?
///
/// Such a name in any folder opens the device, not a file: a payload written to
/// `<workspace>/<hash>/NUL` is gone, the rename onto it fails the whole extraction, and a row
/// that pointed there would read back empty. Refused on every OS, because the workspace layout
/// is the same on every OS.
fn is_reserved_device_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    let stem = stem.to_ascii_uppercase();
    if matches!(
        stem.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) {
        return true;
    }
    let mut chars = stem.chars();
    let port: String = chars.by_ref().take(3).collect();
    matches!(port.as_str(), "COM" | "LPT")
        && matches!(chars.next(), Some('1'..='9' | '¹' | '²' | '³'))
        && chars.next().is_none()
}

/// A tar member's path with any `.` components dropped. Archives packed with `tar -C dir .` name
/// every member `./<guid>/…`, and `Path::components` keeps a leading `.` — so without this the
/// first component of every member was `.` and such a pack read as empty (review B11).
fn member_path(entry_path: &Path) -> PathBuf {
    entry_path
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect()
}

fn guid_and_member(entry_path: &Path) -> Option<(String, String)> {
    let path = member_path(entry_path);
    let mut components = path.components();
    let guid = components.next()?.as_os_str().to_string_lossy().to_string();
    let member = components.next()?.as_os_str().to_string_lossy().to_string();
    Some((guid, member))
}

/// Unity names each GUID folder by the asset's GUID: 32 hex digits. A GUID becomes a file name in
/// the workspace (the temporary payload, and the basename when a pathname is missing), so anything
/// else — `..`, `../x`, a device name — is never used as one.
fn is_guid(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Most bytes a `pathname` or `asset.meta` entry may hold. A path is one line and a `.meta` a few
/// hundred bytes; anything beyond this is not one, and reading it whole would let an archive make
/// the reader allocate whatever it declares.
const SMALL_ENTRY_CAP: u64 = 64 * 1024;

/// Read a `pathname` or `asset.meta` entry, or `None` — with a warning — when it is larger than
/// [`SMALL_ENTRY_CAP`] or is not UTF-8. Bytes past the cap are never read into memory, and the
/// entry beside it is unaffected: one `pathname` that was not UTF-8 used to fail the whole
/// manifest, and every other asset in the pack with it.
fn read_small_entry<R: Read>(
    entry: &mut tar::Entry<'_, R>,
    guid: &str,
    member: &str,
) -> io::Result<Option<String>> {
    let Some(bytes) = read_capped(entry, SMALL_ENTRY_CAP)? else {
        tracing::warn!(guid, member, "skipping an oversized package entry");
        return Ok(None);
    };
    match String::from_utf8(bytes) {
        Ok(text) => Ok(Some(text)),
        Err(_) => {
            tracing::warn!(guid, member, "skipping a package entry that is not UTF-8");
            Ok(None)
        }
    }
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
    read_manifest_capped(path, on_progress, MAX_PACKAGE_ENTRIES)
}

/// [`read_manifest_with_progress`] with the entry bound as an argument, so a test can reach it
/// without building half a million folders. More than `max_entries` GUID folders is `InvalidData`.
fn read_manifest_capped(
    path: &Path,
    on_progress: &(dyn Fn(u64, u64) -> bool + Sync),
    max_entries: usize,
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

    for_each_member(&mut archive, |member, entry| {
        let Some((guid, part)) = guid_and_member(member) else {
            return Ok(Walk::On);
        };
        // The same rule the extract pass applies: a folder that is not a GUID is not an asset,
        // and its name would otherwise become a catalog row — and later a file name.
        if !is_guid(&guid) {
            if part == "asset" {
                tracing::warn!(guid, "not listing a payload whose folder is not a GUID");
            }
            return Ok(Walk::On);
        }
        if !by_guid.contains_key(&guid) && by_guid.len() >= max_entries {
            return Err(invalid(format!(
                "the package lists more than {max_entries} assets — not read"
            )));
        }
        let rec = by_guid.entry(guid.clone()).or_insert_with(|| PackageEntry {
            guid: guid.clone(),
            ..Default::default()
        });

        match part.as_str() {
            "pathname" => {
                if let Some(s) = read_small_entry(entry, &guid, &part)? {
                    rec.pathname = Some(parse_pathname(&s));
                }
            }
            "asset.meta" => {
                if let Some(s) = read_small_entry(entry, &guid, &part)? {
                    rec.meta = parse_meta(&s);
                }
            }
            "asset" => {
                // The entry's own extent, which is what a later pull reads and what
                // `read_entry_payload` checks against the header (a PAX override is refused).
                rec.asset_size = Some(entry.size());
                rec.asset_offset = Some(entry.raw_file_position());
            }
            "preview.png" => rec.has_preview = true,
            _ => {}
        }
        Ok(Walk::On)
    })?;

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

    let walk = for_each_member(&mut archive, |member, entry| {
        let Some((guid, part)) = guid_and_member(member) else {
            return Ok(Walk::On);
        };
        if !is_guid(&guid) {
            if part == "asset" {
                tracing::warn!(guid, "skipping a payload whose folder is not a GUID");
            }
            return Ok(Walk::On);
        }
        // What this pass holds in memory is one small record per folder; bound the folders.
        if extracted.len() + pending.len() >= MAX_PACKAGE_ENTRIES
            || pathnames.len() >= MAX_PACKAGE_ENTRIES
        {
            return Err(invalid(format!(
                "the package holds more than {MAX_PACKAGE_ENTRIES} assets — not extracted further"
            )));
        }
        match part.as_str() {
            "pathname" => {
                let Some(s) = read_small_entry(entry, &guid, &part)? else {
                    return Ok(Walk::On); // placed under its GUID, as if the pathname never came
                };
                let pn = parse_pathname(&s);
                if let Some((tmp, hash, size)) = pending.remove(&guid) {
                    extracted.push(place_payload(dest, guid, Some(pn), tmp, hash, size)?);
                } else {
                    pathnames.insert(guid, pn);
                }
            }
            "asset" => {
                let tmp_path = tmp_dir.join(&guid);
                let Some((hash, size)) =
                    stream_hash_to_file(entry, &tmp_path, MAX_EXTRACTED_PAYLOAD_BYTES)?
                else {
                    tracing::warn!(
                        guid,
                        cap = MAX_EXTRACTED_PAYLOAD_BYTES,
                        "skipping a payload over the extract ceiling"
                    );
                    return Ok(Walk::On);
                };
                if let Some(pn) = pathnames.remove(&guid) {
                    extracted.push(place_payload(dest, guid, Some(pn), tmp_path, hash, size)?);
                } else {
                    pending.insert(guid, (tmp_path, hash, size));
                }
            }
            _ => {}
        }
        Ok(Walk::On)
    });

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
/// big pack has no random access, INGESTION §2); on a huge pack this is what lets an extraction show
/// a real bar and be aborted instead of appearing frozen.
///
/// A cancel is **not an error**: it returns the payloads placed so far, exactly like
/// [`extract_to_workspace_with_progress`]. Payloads are placed incrementally and content-addressed,
/// so a cancelled run leaves valid files and the caller can still record them — returning `Err`
/// instead threw away a list of files that were already on disk, leaving their rows unmaterialized
/// and showing the user a raw I/O error for pressing Cancel.
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
    // A locator comes from the catalog, but its GUID still becomes a file name here — and its
    // size is how much of the stream gets written to disk, so one over the extract ceiling is
    // skipped like any other entry that is not what it claims (the rest still extract).
    wanted.retain(|w| {
        if !is_guid(&w.guid) {
            tracing::warn!(guid = %w.guid, "skipping a locator whose GUID is not a GUID");
            return false;
        }
        if w.size > MAX_EXTRACTED_PAYLOAD_BYTES {
            tracing::warn!(guid = %w.guid, size = w.size, cap = MAX_EXTRACTED_PAYLOAD_BYTES,
                "skipping a locator over the extract ceiling");
            return false;
        }
        true
    });
    wanted.sort_by_key(|w| w.offset);
    let tmp_dir = dest.join(".tmp");
    std::fs::create_dir_all(&tmp_dir)?;

    let mut pos: u64 = 0;
    let mut extracted = Vec::with_capacity(wanted.len());
    let walk: io::Result<()> = (|| {
        for w in wanted {
            if w.offset < pos {
                continue; // overlapping/duplicate locator — already consumed
            }
            io::copy(&mut (&mut gz).take(w.offset - pos), &mut io::sink())?;
            pos = w.offset;
            let tmp_path = tmp_dir.join(&w.guid);
            let (hash, size) = {
                let mut region = (&mut gz).take(w.size);
                // A cancel can land mid-payload, and `?` would carry it out past the cleanup
                // below — leaving the partial write in `.tmp/<guid>`. Now that a cancel returns
                // `Ok`, nothing downstream would ever come back for it: cancel a 400 MB payload
                // often enough and `.tmp` grows without bound (`remove_dir` is non-recursive and
                // silently fails on a non-empty dir).
                match stream_hash_to_file(&mut region, &tmp_path, MAX_EXTRACTED_PAYLOAD_BYTES) {
                    Ok(Some(v)) => v,
                    // Unreachable while `w.size` is under the ceiling (checked above) and the
                    // region is limited to it — but a skip, never a panic, if that ever changes.
                    Ok(None) => continue,
                    Err(e) => return Err(e),
                }
            };
            pos += size;
            if size != w.size {
                // Truncated read: stale locator (package changed on disk), *or* a cancel landing
                // mid-payload. A cancel is reported by the sentinel error the reader already
                // returned, so only a genuine short read reaches this branch with the stream
                // healthy — and silently placing a short payload would poison the
                // content-addressed store.
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
        Ok(())
    })();
    let _ = std::fs::remove_dir(&tmp_dir);
    // Cancel keeps what was placed (see the doc comment); every other error still propagates,
    // and the files it already placed likewise survive for a retry.
    if let Err(e) = walk {
        if !is_cancel(&e) {
            return Err(e);
        }
    }
    Ok(extracted)
}

/// Most bytes one in-pack payload may be pulled for analysis or served as a file (ENGINE-API §8.2)
/// — the same ceiling the identify drain holds.
pub const PAYLOAD_PULL_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Why an in-pack payload could not be read.
#[derive(Debug)]
pub enum PayloadError {
    /// The archive is not where the catalog says it is.
    ArchiveMissing(String),
    /// The archive no longer holds this entry at the catalogued place — replaced, truncated or
    /// re-exported since it was scanned. A rescan fixes that; nothing here guesses.
    SourceChanged(String),
    /// Over the pull ceiling. Refused before the archive is opened.
    TooLarge {
        size: u64,
        cap: u64,
    },
    Io(io::Error),
}

impl std::fmt::Display for PayloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ArchiveMissing(why) => write!(f, "the package archive is missing: {why}"),
            Self::SourceChanged(why) => {
                write!(
                    f,
                    "the package changed since it was scanned ({why}) — rescan it"
                )
            }
            Self::TooLarge { size, cap } => {
                write!(
                    f,
                    "the payload is {size} bytes, over the {cap}-byte ceiling"
                )
            }
            Self::Io(e) => write!(f, "reading the package failed: {e}"),
        }
    }
}

/// Pull one entry's payload, **verified against the archive's own tar header** (ENGINE-API §8.2).
///
/// [`read_payload_at`] trusts the catalogued offset: if the archive was replaced since the scan, the
/// same offset holds some other entry's bytes — or the middle of one — and they are returned as if
/// they were this asset. Here the 512-byte header immediately before the payload must name
/// `<guid>/asset` and carry exactly `size`, or the read is refused as [`PayloadError::SourceChanged`].
/// The ceiling is checked before the file is opened, so an absurd catalogued size costs nothing.
pub fn read_entry_payload(
    path: &Path,
    guid: &str,
    offset: u64,
    size: u64,
) -> Result<Vec<u8>, PayloadError> {
    if size > PAYLOAD_PULL_MAX_BYTES {
        return Err(PayloadError::TooLarge {
            size,
            cap: PAYLOAD_PULL_MAX_BYTES,
        });
    }
    if offset < 512 {
        return Err(PayloadError::SourceChanged(format!(
            "offset {offset} leaves no room for a tar header"
        )));
    }
    let file = File::open(path).map_err(|e| match e.kind() {
        io::ErrorKind::NotFound => PayloadError::ArchiveMissing(e.to_string()),
        _ => PayloadError::Io(e),
    })?;
    let mut gz = GzDecoder::new(file);
    let skipped = io::copy(&mut (&mut gz).take(offset - 512), &mut io::sink()).map_err(|e| {
        PayloadError::SourceChanged(format!("the archive does not decompress: {e}"))
    })?;
    let mut block = [0u8; 512];
    if skipped != offset - 512 || gz.read_exact(&mut block).is_err() {
        return Err(PayloadError::SourceChanged(
            "the archive ends before the catalogued entry".into(),
        ));
    }
    let header = tar::Header::from_byte_slice(&block);
    let expected = format!("{guid}/asset");
    let named = header
        .path()
        .map(|p| p.to_string_lossy().trim_start_matches("./").to_string())
        .unwrap_or_default();
    if named != expected {
        return Err(PayloadError::SourceChanged(format!(
            "the entry at that offset is {named:?}, not {expected:?}"
        )));
    }
    match header.entry_size() {
        Ok(n) if n == size => {}
        other => {
            return Err(PayloadError::SourceChanged(format!(
                "the entry holds {other:?} bytes, the catalog says {size}"
            )))
        }
    }
    let mut buf = Vec::with_capacity((size as usize).min(PULL_CAP_HINT));
    let n = (&mut gz).take(size).read_to_end(&mut buf).map_err(|e| {
        PayloadError::SourceChanged(format!("the payload does not decompress: {e}"))
    })?;
    if n as u64 != size {
        return Err(PayloadError::SourceChanged(format!(
            "payload truncated: wanted {size} got {n} bytes"
        )));
    }
    Ok(buf)
}

/// Upper bound on the capacity hint when pulling a payload into memory — guards against a
/// corrupt manifest size triggering a huge speculative allocation. `take(size)` still limits
/// the actual read, so a legitimately larger payload simply grows past the hint.
const PULL_CAP_HINT: usize = 64 * 1024 * 1024;

/// Pull **one** payload out of a `.unitypackage` **into memory** — the in-pack identify path
/// (DATA-MODEL §7 / M14). One gzip skip-scan to `offset`, read `size` bytes, return them; no file
/// is written (unlike [`extract_at_offsets`]), so identifying an in-pack asset leaves nothing on
/// disk. A short read means a stale locator (the archive changed) and errors rather than handing
/// back a truncated buffer.
///
/// `size` comes from a manifest, which is to say from the archive. Over
/// [`PAYLOAD_PULL_MAX_BYTES`] it is refused here (`InvalidInput`), before the archive is opened —
/// callers have their own, smaller, limits, but the buffer this fills must not depend on every
/// caller remembering one.
pub fn read_payload_at(path: &Path, offset: u64, size: u64) -> io::Result<Vec<u8>> {
    if size > PAYLOAD_PULL_MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("the payload is {size} bytes, over the {PAYLOAD_PULL_MAX_BYTES}-byte ceiling"),
        ));
    }
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
    // A name that is empty once cleaned, or that Windows reserves for a device, is not used: the
    // payload goes under its GUID, which is always a plain file name.
    let basename = pathname
        .as_deref()
        .and_then(|p| Path::new(p).file_name())
        .map(|n| sanitize_basename(&n.to_string_lossy()))
        .filter(|n| !n.is_empty() && !is_reserved_device_name(n))
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

/// Stream a payload to `dest` while hashing it, without holding it in memory — **at most `cap`
/// bytes**, whatever the entry declares. `Some((hash, size))` when it fit; `None` when the source
/// held more. Nothing is left at `dest` unless it fit: not after `None`, and not after an error
/// (a cancel landing mid-payload used to leave its partial write behind in `.tmp`).
fn stream_hash_to_file(
    source: &mut impl Read,
    dest: &Path,
    cap: u64,
) -> io::Result<Option<(String, u64)>> {
    let streamed = (|| {
        let mut out = File::create(dest)?;
        let mut limited = source.by_ref().take(cap.saturating_add(1));
        let mut hasher = blake3::Hasher::new();
        let mut buf = [0u8; 64 * 1024];
        let mut size: u64 = 0;
        loop {
            let n = limited.read(&mut buf)?;
            if n == 0 {
                return Ok(Some((hasher.finalize().to_hex().to_string(), size)));
            }
            size += n as u64;
            if size > cap {
                return Ok(None);
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
        }
    })();
    if !matches!(streamed, Ok(Some(_))) {
        let _ = std::fs::remove_file(dest);
    }
    streamed
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

    /// A payload is read only when the archive still says it is the entry the catalog recorded
    /// (v1 plan S12b): the right guid and size in the header just before it. A replaced archive,
    /// a shifted offset, a truncated file and a missing one are each named; an oversized entry is
    /// refused without opening anything.
    #[test]
    fn an_entry_is_read_only_where_the_archive_still_holds_it() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"model bytes ".repeat(300);
        let guid = "aaaa1111bbbb2222cccc3333dddd4444";
        let pkg = build_test_package(dir.path(), guid, "Assets/A.fbx", "guid: x\n", &payload);
        let entry = read_manifest(&pkg).unwrap().remove(0);
        let (offset, size) = (entry.asset_offset.unwrap(), entry.asset_size.unwrap());

        assert_eq!(
            read_entry_payload(&pkg, guid, offset, size).unwrap(),
            payload
        );

        let changed =
            |r: Result<Vec<u8>, PayloadError>| matches!(r, Err(PayloadError::SourceChanged(_)));
        assert!(
            changed(read_entry_payload(
                &pkg,
                "ffff1111bbbb2222cccc3333dddd4444",
                offset,
                size
            )),
            "another guid"
        );
        assert!(
            changed(read_entry_payload(&pkg, guid, offset + 512, size)),
            "a shifted offset"
        );
        assert!(
            changed(read_entry_payload(&pkg, guid, offset, size - 1)),
            "another size"
        );
        assert!(
            changed(read_entry_payload(&pkg, guid, 100, size)),
            "no room for a header"
        );

        // The same place in a re-exported archive now holds a different pack's entry.
        let other = tempfile::tempdir().unwrap();
        let replaced = build_test_package(
            other.path(),
            "9999888877776666555544443333aaaa",
            "Assets/B.fbx",
            "guid: y\n",
            &payload,
        );
        assert!(
            changed(read_entry_payload(&replaced, guid, offset, size)),
            "a replaced archive"
        );

        let bytes = std::fs::read(&pkg).unwrap();
        std::fs::write(&pkg, &bytes[..bytes.len() / 3]).unwrap();
        assert!(
            changed(read_entry_payload(&pkg, guid, offset, size)),
            "a truncated archive"
        );

        let gone = dir.path().join("gone.unitypackage");
        assert!(matches!(
            read_entry_payload(&gone, guid, offset, size),
            Err(PayloadError::ArchiveMissing(_))
        ));
        assert!(
            matches!(
                read_entry_payload(&gone, guid, offset, PAYLOAD_PULL_MAX_BYTES + 1),
                Err(PayloadError::TooLarge { .. })
            ),
            "the ceiling is checked before the archive is opened"
        );
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
                guid: "5ca1e0005ca1e0005ca1e0005ca1e000".into(),
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
            for (guid, file) in [
                ("11111111111111111111111111111111", "One.png"),
                ("22222222222222222222222222222222", "Two.png"),
            ] {
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

    /// A cancelled **surgical** extract keeps what it placed, like the workspace path does.
    ///
    /// It used to return the cancel sentinel as an `Err`, which threw away a list of payloads that
    /// were already on disk: the writer never materialized them (rows kept `workspace_rel` NULL)
    /// and the user got a raw I/O error string for pressing Cancel.
    #[test]
    fn a_cancelled_surgical_extract_keeps_what_it_placed() {
        let dir = tempfile::tempdir().unwrap();
        let payload = b"surgical".repeat(400);
        let pkg = build_test_package(
            dir.path(),
            "cafe1234cafe1234cafe1234cafe1234",
            "Assets/Props/Crate.png",
            "guid: cafe1234cafe1234cafe1234cafe1234\n",
            &payload,
        );
        let entry = &read_manifest(&pkg).unwrap()[0];
        let wanted = vec![EntryLocator {
            guid: entry.guid.clone(),
            pathname: entry.pathname.clone(),
            offset: entry.asset_offset.unwrap(),
            size: entry.asset_size.unwrap(),
        }];

        // Cancel before a single byte is read: no payload placed, but still not an error.
        let ws = dir.path().join("ws");
        let none = extract_at_offsets_with_progress(&pkg, &ws, wanted.clone(), &|_, _| false)
            .expect("a cancel is not a failure");
        assert!(none.is_empty(), "nothing was placed before the cancel");

        // Cancel only *after* the payload has been read: the placed file is kept and usable.
        let seen = std::sync::atomic::AtomicU64::new(0);
        let placed = extract_at_offsets_with_progress(&pkg, &ws, wanted.clone(), &|read, _| {
            seen.store(read, std::sync::atomic::Ordering::Relaxed);
            read == 0 // allow the first read, cancel every one after it
        })
        .expect("a cancel is not a failure");
        assert!(
            seen.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "the progress callback never ran — the test proves nothing"
        );
        for a in &placed {
            assert!(
                a.workspace_path.exists(),
                "a placed payload must be on disk"
            );
            assert_eq!(std::fs::read(&a.workspace_path).unwrap(), payload);
        }

        // A genuine error still propagates — a stale locator must never come back as Ok.
        let bad = extract_at_offsets_with_progress(
            &pkg,
            &ws,
            vec![EntryLocator {
                guid: "5ca1e0005ca1e0005ca1e0005ca1e000".into(),
                pathname: None,
                offset: wanted[0].offset,
                size: wanted[0].size + 4096,
            }],
            &|_, _| true,
        );
        assert!(bad.is_err(), "a truncated payload must still fail loudly");
    }

    #[test]
    fn read_manifest_with_progress_reports_and_cancels() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = build_test_package(
            dir.path(),
            "abc123abc123abc123abc123abc123ab",
            "Assets/Props/Barrel.png",
            "guid: abc123abc123abc123abc123abc123ab\n",
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
            "abc123abc123abc123abc123abc123ab",
            "Assets/Props/Barrel.png",
            "fileFormatVersion: 2\nguid: abc123abc123abc123abc123abc123ab\nlabels:\n- Prop\nTextureImporter:\n  mipmaps: 1\n",
            b"PNGDATA_PAYLOAD",
        );

        let manifest = read_manifest(&pkg).unwrap();
        assert_eq!(manifest.len(), 1);
        let entry = &manifest[0];
        assert_eq!(entry.guid, "abc123abc123abc123abc123abc123ab");
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
            "abc123abc123abc123abc123abc123ab",
            "Assets/Props/Barrel.png",
            "fileFormatVersion: 2\nguid: abc123abc123abc123abc123abc123ab\n",
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
            "abc123abc123abc123abc123abc123ab",
            "Assets/Toon Fantasy Nature/_ReadFirst.txt\n00",
            "fileFormatVersion: 2\nguid: abc123abc123abc123abc123abc123ab\n",
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

    // ---- hostile and unusual archives (v2 plan Task 1.7, review B11) --------------------------

    /// A `.unitypackage` whose members are written exactly as named. `tar::Builder::append_data`
    /// strips a leading `./` and refuses `..` — which are exactly the archives these tests need.
    fn raw_package(dir: &Path, members: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("raw.unitypackage");
        let gz = flate2::write::GzEncoder::new(
            File::create(&path).unwrap(),
            flate2::Compression::default(),
        );
        let mut b = tar::Builder::new(gz);
        for (name, bytes) in members {
            let mut h = tar::Header::new_gnu();
            h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            h.set_size(bytes.len() as u64);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            h.set_cksum();
            b.append(&h, *bytes).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap();
        path
    }

    const GUID: &str = "aaaa1111bbbb2222cccc3333dddd4444";

    /// Archives packed with `tar -C dir .` name every member `./<guid>/…`. They used to read as
    /// empty packs: the first path component was `.`, so no member was ever an `asset`.
    #[test]
    fn dot_slash_members_index_the_same_as_plain_ones() {
        let payload = b"crate bytes ".repeat(100);
        let meta = format!("guid: {GUID}\nlabels:\n- Prop\nTextureImporter:\n  x: 1\n");
        let pack = |dir: &Path, prefix: &str| {
            let names = [
                format!("{prefix}.icon.png"),
                format!("{prefix}{GUID}/asset"),
                format!("{prefix}{GUID}/asset.meta"),
                format!("{prefix}{GUID}/pathname"),
            ];
            let bodies: [&[u8]; 4] = [
                b"icon",
                &payload,
                meta.as_bytes(),
                b"Assets/Props/Crate.png",
            ];
            let members: Vec<(&str, &[u8])> =
                names.iter().map(String::as_str).zip(bodies).collect();
            raw_package(dir, &members)
        };
        let (plain_dir, dotted_dir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let plain = pack(plain_dir.path(), "");
        let dotted = pack(dotted_dir.path(), "./");

        let manifest = read_manifest(&dotted).unwrap();
        assert_eq!(
            manifest,
            read_manifest(&plain).unwrap(),
            "indexed differently"
        );
        assert_eq!(manifest.len(), 1);
        let e = &manifest[0];
        assert_eq!(e.pathname.as_deref(), Some("Assets/Props/Crate.png"));
        assert_eq!(e.meta.labels, vec!["Prop"]);

        let meta = read_package_meta(&dotted, &manifest).unwrap();
        assert_eq!(meta.icon_png.as_deref(), Some(b"icon".as_slice()));
        let (offset, size) = (e.asset_offset.unwrap(), e.asset_size.unwrap());
        assert_eq!(
            read_entry_payload(&dotted, GUID, offset, size).unwrap(),
            payload
        );
        let ws = dotted_dir.path().join("ws");
        let placed = extract_to_workspace(&dotted, &ws).unwrap();
        assert_eq!(placed.len(), 1);
        assert_eq!(std::fs::read(&placed[0].workspace_path).unwrap(), payload);
        assert_eq!(placed[0].workspace_path.file_name().unwrap(), "Crate.png");
    }

    /// A `pathname` or `asset.meta` over 64 KiB is not one: it is skipped, not read into memory,
    /// and the payload beside it is still indexed and extracted.
    #[test]
    fn an_oversized_pathname_or_meta_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let huge = vec![b'a'; 64 * 1024 + 1];
        let pkg = raw_package(
            dir.path(),
            &[
                (&format!("{GUID}/asset"), b"payload"),
                (&format!("{GUID}/asset.meta"), &huge),
                (&format!("{GUID}/pathname"), &huge),
            ],
        );
        let manifest = read_manifest(&pkg).unwrap();
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].pathname, None, "an oversized pathname was read");
        assert_eq!(
            manifest[0].meta,
            UnityMeta::default(),
            "an oversized meta was read"
        );
        assert_eq!(manifest[0].asset_size, Some(7));

        let placed = extract_to_workspace(&pkg, &dir.path().join("ws")).unwrap();
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].pathname, None);
        assert_eq!(placed[0].workspace_path.file_name().unwrap(), GUID);

        // At the cap, it is still a pathname.
        let dir = tempfile::tempdir().unwrap();
        let at_cap = vec![b'a'; 64 * 1024];
        let pkg = raw_package(dir.path(), &[(&format!("{GUID}/pathname"), &at_cap)]);
        assert_eq!(
            read_manifest(&pkg).unwrap()[0]
                .pathname
                .as_deref()
                .map(str::len),
            Some(64 * 1024)
        );
    }

    /// A GUID becomes a file name in the workspace, so a folder or locator that is not a GUID —
    /// `..`, `../x` — is skipped rather than joined onto a path. The rest of the pack is unaffected.
    #[test]
    fn a_guid_that_is_not_a_guid_never_names_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = raw_package(
            dir.path(),
            &[
                ("../asset", b"hostile"),
                ("../pathname", b"Assets/Evil.png"),
                (&format!("{GUID}/asset"), b"honest"),
                (&format!("{GUID}/pathname"), b"Assets/Good.png"),
            ],
        );
        let ws = dir.path().join("ws");
        let placed = extract_to_workspace(&pkg, &ws).expect("the honest payload still extracts");
        assert_eq!(placed.len(), 1);
        assert_eq!(placed[0].guid, GUID);

        let good = read_manifest(&pkg)
            .unwrap()
            .into_iter()
            .find(|e| e.guid == GUID)
            .unwrap();
        let ws = dir.path().join("surgical");
        for hostile in ["../x", "../../x", ".."] {
            let placed = extract_at_offsets(
                &pkg,
                &ws,
                vec![EntryLocator {
                    guid: hostile.into(),
                    pathname: None,
                    offset: good.asset_offset.unwrap(),
                    size: good.asset_size.unwrap(),
                }],
            )
            .expect("a refused locator is skipped, not an error");
            assert!(placed.is_empty(), "{hostile:?} named a file");
        }
        assert!(!ws.join("x").exists() && !dir.path().join("x").exists());
    }

    // ---- sizes an archive declares are not sizes it gets (integrity review, finding 12) -------

    /// One part of a hand-built archive.
    enum Part<'a> {
        /// An ordinary member, written exactly as named.
        Member(&'a str, &'a [u8]),
        /// A PAX extended header applying to the member after it.
        Pax(&'a [(&'a str, &'a [u8])]),
        /// A member whose header says `declared` bytes while `body` follows it.
        Lying {
            name: &'a str,
            declared: u64,
            body: &'a [u8],
        },
        /// A GNU long-name record of this many bytes.
        LongName(usize),
    }

    fn hand_built(dir: &Path, parts: &[Part<'_>]) -> PathBuf {
        let path = dir.join("hand.unitypackage");
        let gz = flate2::write::GzEncoder::new(
            File::create(&path).unwrap(),
            flate2::Compression::fast(),
        );
        let mut b = tar::Builder::new(gz);
        let header = |name: &str, size: u64, kind: tar::EntryType| {
            let mut h = tar::Header::new_gnu();
            h.as_old_mut().name[..name.len()].copy_from_slice(name.as_bytes());
            h.set_size(size);
            h.set_mode(0o644);
            h.set_entry_type(kind);
            h.set_cksum();
            h
        };
        for part in parts {
            match part {
                Part::Member(name, body) => b
                    .append(
                        &header(name, body.len() as u64, tar::EntryType::Regular),
                        *body,
                    )
                    .unwrap(),
                Part::Pax(records) => b.append_pax_extensions(records.iter().copied()).unwrap(),
                Part::Lying {
                    name,
                    declared,
                    body,
                } => {
                    // The header alone, then the bytes written straight behind it, padded to a
                    // block — the tar the header describes and the one on disk now disagree.
                    b.append(
                        &header(name, *declared, tar::EntryType::Regular),
                        std::io::empty(),
                    )
                    .unwrap();
                    let w = b.get_mut();
                    w.write_all(body).unwrap();
                    let pad = (512 - body.len() % 512) % 512;
                    w.write_all(&vec![0u8; pad]).unwrap();
                }
                Part::LongName(len) => b
                    .append(
                        &header("././@LongLink", *len as u64, tar::EntryType::GNULongName),
                        vec![b'a'; *len].as_slice(),
                    )
                    .unwrap(),
            }
        }
        b.into_inner().unwrap().finish().unwrap();
        path
    }

    /// The caps used to be checked against the tar **header's** size while the read followed the
    /// **PAX** size, which overrides it: a header saying "0 bytes" over a PAX record saying
    /// "8 MiB" passed the 4 MiB icon cap and was read whole — on the writer thread, for whatever
    /// size a hostile pack cared to declare. Such an archive is refused; nothing it declares is
    /// buffered.
    #[test]
    fn a_pax_size_never_carries_a_read_past_its_cap() {
        let dir = tempfile::tempdir().unwrap();
        let big = vec![0u8; 8 * 1024 * 1024];
        let size = big.len().to_string();
        let records: [(&str, &[u8]); 1] = [("size", size.as_bytes())];
        let pkg = hand_built(
            dir.path(),
            &[
                Part::Pax(&records),
                Part::Lying {
                    name: ".icon.png",
                    declared: 0,
                    body: &big,
                },
                Part::Member(&format!("{GUID}/asset"), b"honest"),
                Part::Member(&format!("{GUID}/pathname"), b"Assets/Good.png"),
            ],
        );
        let honest = [PackageEntry {
            guid: GUID.into(),
            pathname: Some("Assets/Good.png".into()),
            asset_size: Some(6),
            ..Default::default()
        }];
        match read_package_meta(&pkg, &honest) {
            Ok(meta) => panic!(
                "an icon of {} bytes was buffered past the {ICON_CAP}-byte cap",
                meta.icon_png.map_or(0, |b| b.len())
            ),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::InvalidData, "{e}"),
        }
        assert!(
            read_manifest(&pkg).is_err(),
            "a manifest was built from sizes the archive lies about"
        );

        // The same lie on a licence: 10 bytes in the header (so the manifest says it is small
        // enough to read), a megabyte in the PAX record.
        let dir = tempfile::tempdir().unwrap();
        let text = vec![b'x'; 1024 * 1024];
        let size = text.len().to_string();
        let records: [(&str, &[u8]); 1] = [("size", size.as_bytes())];
        let pkg = hand_built(
            dir.path(),
            &[
                Part::Pax(&records),
                Part::Lying {
                    name: &format!("{GUID}/asset"),
                    declared: 10,
                    body: &text,
                },
            ],
        );
        let licence = [PackageEntry {
            guid: GUID.into(),
            pathname: Some("Assets/LICENSE".into()),
            asset_size: Some(10),
            ..Default::default()
        }];
        assert!(
            read_package_meta(&pkg, &licence).is_err(),
            "a megabyte of 'licence' was read behind a 10-byte header"
        );
    }

    /// The tar reader itself reads an extension record — a PAX header, a GNU long name — into
    /// memory whole before handing the next member over. Those are read here instead, under a
    /// cap: one that declares a megabyte is refused, and an ordinary one still applies.
    #[test]
    fn an_oversized_extension_record_is_refused_and_a_normal_one_applies() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = hand_built(
            dir.path(),
            &[
                Part::LongName(1024 * 1024),
                Part::Member(&format!("{GUID}/asset"), b"payload"),
            ],
        );
        let refused = read_manifest(&pkg).expect_err("a megabyte long-name record was read");
        assert_eq!(refused.kind(), io::ErrorKind::InvalidData, "{refused}");
        assert!(extract_to_workspace(&pkg, &dir.path().join("ws")).is_err());

        // What third-party packers really write: a PAX record per member carrying its path (and
        // an mtime). The path applies; the rest is ignored.
        let dir = tempfile::tempdir().unwrap();
        let asset = format!("{GUID}/asset");
        let pathname = format!("{GUID}/pathname");
        let for_asset: [(&str, &[u8]); 2] =
            [("path", asset.as_bytes()), ("mtime", b"1700000000.5")];
        let for_pathname: [(&str, &[u8]); 1] = [("path", pathname.as_bytes())];
        let pkg = hand_built(
            dir.path(),
            &[
                Part::Pax(&for_asset),
                Part::Member("placeholder-a", b"payload"),
                Part::Pax(&for_pathname),
                Part::Member("placeholder-b", b"Assets/Rock.png"),
            ],
        );
        let manifest = read_manifest(&pkg).expect("a PAX path record is ordinary");
        assert_eq!(manifest.len(), 1);
        assert_eq!(manifest[0].guid, GUID);
        assert_eq!(manifest[0].pathname.as_deref(), Some("Assets/Rock.png"));
        assert_eq!(manifest[0].asset_size, Some(7));
        let (offset, size) = (manifest[0].asset_offset.unwrap(), 7);
        assert_eq!(read_payload_at(&pkg, offset, size).unwrap(), b"payload");
    }

    /// The browse pass keeps only GUID folders, like the extract pass: `..`, a device name or any
    /// other folder is not an asset, and its "guid" would become a catalog row and a file name.
    /// And the number of folders one package may list is bounded.
    #[test]
    fn the_manifest_lists_only_guid_folders_and_a_bounded_number_of_them() {
        let dir = tempfile::tempdir().unwrap();
        let other = "bbbb1111bbbb2222cccc3333dddd4444";
        let pkg = raw_package(
            dir.path(),
            &[
                ("../asset", b"hostile"),
                ("NUL/asset", b"hostile"),
                ("abc123/asset", b"not a guid"),
                ("abc123/pathname", b"Assets/NotAGuid.png"),
                (&format!("{GUID}/asset"), b"honest"),
                (&format!("{GUID}/pathname"), b"Assets/Good.png"),
                (&format!("{other}/asset"), b"second"),
            ],
        );
        let guids: Vec<String> = read_manifest(&pkg)
            .unwrap()
            .into_iter()
            .map(|e| e.guid)
            .collect();
        assert_eq!(guids, vec![GUID.to_string(), other.to_string()]);

        assert_eq!(
            read_manifest_capped(&pkg, &|_, _| true, 2).unwrap().len(),
            2,
            "at the bound is fine"
        );
        let over = read_manifest_capped(&pkg, &|_, _| true, 1)
            .expect_err("a package over the entry bound was listed");
        assert_eq!(over.kind(), io::ErrorKind::InvalidData, "{over}");
    }

    /// One `pathname` that is not UTF-8 used to fail the whole manifest — every other asset in
    /// the pack with it. That entry reads as if it had no pathname; the rest are untouched.
    #[test]
    fn a_pathname_that_is_not_utf8_costs_one_entry_not_the_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let other = "bbbb1111bbbb2222cccc3333dddd4444";
        let pkg = raw_package(
            dir.path(),
            &[
                (&format!("{GUID}/asset"), b"payload"),
                (&format!("{GUID}/pathname"), b"Assets/\xff\xfe\xfd.png"),
                (&format!("{other}/asset"), b"second"),
                (&format!("{other}/pathname"), b"Assets/Fine.png"),
            ],
        );
        let manifest = read_manifest(&pkg).expect("one bad pathname must not fail the pack");
        assert_eq!(manifest.len(), 2);
        assert_eq!(manifest[0].pathname, None, "the unreadable pathname");
        assert_eq!(
            manifest[0].asset_size,
            Some(7),
            "its payload is still listed"
        );
        assert_eq!(manifest[1].pathname.as_deref(), Some("Assets/Fine.png"));

        let placed = extract_to_workspace(&pkg, &dir.path().join("ws")).expect("extracts");
        assert_eq!(placed.len(), 2);
    }

    /// `NUL`, `CON`, `COM1`… name devices on Windows, with or without an extension: creating
    /// `<workspace>/<hash>/NUL` opens the device, the payload is lost, and the catalog is told it
    /// landed. Such a payload is placed under its GUID, like one with no usable name.
    #[test]
    fn a_payload_named_for_a_windows_device_is_placed_under_its_guid() {
        for (i, hostile) in [
            "Assets/NUL",
            "Assets/con.png",
            "Assets/Com1.fbx",
            "Assets/lpt9",
            "Assets/aux.tar.gz",
            "Assets/PRN ",
            "Assets/nul.",
        ]
        .into_iter()
        .enumerate()
        {
            let dir = tempfile::tempdir().unwrap();
            let payload = format!("payload {i}").into_bytes();
            let pkg = build_test_package(dir.path(), GUID, hostile, "guid: x\n", &payload);
            let placed = extract_to_workspace(&pkg, &dir.path().join("ws")).unwrap();
            assert_eq!(placed.len(), 1);
            assert_eq!(
                placed[0].workspace_path.file_name().unwrap(),
                GUID,
                "{hostile:?} named the workspace file"
            );
            assert_eq!(
                std::fs::read(&placed[0].workspace_path).unwrap(),
                payload,
                "{hostile:?}: the payload did not land"
            );
        }
        // Names that merely start like one are ordinary files.
        for fine in ["Assets/Console.png", "Assets/nulls.txt", "Assets/COM10.fbx"] {
            let dir = tempfile::tempdir().unwrap();
            let pkg = build_test_package(dir.path(), GUID, fine, "guid: x\n", b"ok");
            let placed = extract_to_workspace(&pkg, &dir.path().join("ws")).unwrap();
            assert_eq!(
                placed[0].workspace_path.file_name().unwrap(),
                Path::new(fine).file_name().unwrap()
            );
        }
    }

    /// Nothing a locator or a header declares makes an extraction or a pull unbounded: a payload
    /// over the extract ceiling is skipped (the rest of the pack still extracts), a stream that
    /// runs past its cap leaves no file behind, and an in-memory pull over the pull ceiling is
    /// refused before a byte is read.
    #[test]
    fn a_payload_over_its_ceiling_is_skipped_not_streamed_or_buffered() {
        let dir = tempfile::tempdir().unwrap();
        let pkg = build_test_package(dir.path(), GUID, "Assets/A.png", "guid: x\n", b"payload");
        let entry = read_manifest(&pkg).unwrap().remove(0);
        let offset = entry.asset_offset.unwrap();
        let ws = dir.path().join("ws");

        let placed = extract_at_offsets(
            &pkg,
            &ws,
            vec![
                EntryLocator {
                    guid: "bbbb1111bbbb2222cccc3333dddd4444".into(),
                    pathname: Some("Assets/Bomb.bin".into()),
                    offset,
                    size: MAX_EXTRACTED_PAYLOAD_BYTES + 1,
                },
                EntryLocator {
                    guid: GUID.into(),
                    pathname: entry.pathname.clone(),
                    offset,
                    size: 7,
                },
            ],
        )
        .expect("an oversized locator is skipped, not an error");
        assert_eq!(placed.len(), 1, "the honest payload still extracts");
        assert_eq!(placed[0].guid, GUID);

        // The stream itself is capped, whatever was declared: past the cap, no file.
        let out = dir.path().join("capped.bin");
        let mut endless = io::repeat(7).take(10_000);
        assert!(stream_hash_to_file(&mut endless, &out, 4_096)
            .unwrap()
            .is_none());
        assert!(
            !out.exists(),
            "a payload over the cap left its bytes behind"
        );
        let mut exact = io::repeat(7).take(4_096);
        let (_, size) = stream_hash_to_file(&mut exact, &out, 4_096)
            .unwrap()
            .expect("at the cap is fine");
        assert_eq!(size, 4_096);

        let refused = read_payload_at(&pkg, offset, PAYLOAD_PULL_MAX_BYTES + 1)
            .expect_err("a pull over the ceiling was attempted");
        assert_eq!(
            refused.kind(),
            io::ErrorKind::InvalidInput,
            "refused for its size, before reading: {refused}"
        );
    }
}
