//! Thumbnail selection and the in-pack scratch pull.
//!
//! Moved out of `lodestash-ui/src/app/thumbs.rs` (B1). The egui half — uploading decoded pixels
//! to a `TextureHandle` — stayed behind; everything here is renderer-free.

use lodestash_core::types::MediaType;
use lodestash_ingest::thumb;
use std::path::PathBuf;

/// What kind of thumbnail an asset needs. Materials and audio have no pixels of their own, so
/// each gets a rendered stand-in rather than a decode.
#[derive(Debug, Clone)]
pub enum ThumbKind {
    Raster,
    Audio,
    /// A shader ball wearing the material's albedo, when the dependency graph resolved one.
    Material {
        albedo: Option<PathBuf>,
    },
    /// A stored traced-contour preview (M15), rasterised instead of re-rendering the mesh — no
    /// parse, no raster, no cache file. `view` selects which of the stored viewpoints to show,
    /// which is what makes hover-spin work.
    Vector {
        svg: String,
        view: usize,
    },
}

/// Is `path` inside a source root the user opened, or the extraction workspace this app owns?
/// Pure so the one destructive path in the app has a testable guard (TECH-STACK §5.1).
pub fn under_known_root(
    path: &std::path::Path,
    roots: &[PathBuf],
    workspace: Option<&std::path::Path>,
) -> bool {
    roots.iter().any(|r| path.starts_with(r)) || workspace.is_some_and(|w| path.starts_with(w))
}

/// Archive-size ceiling above which in-pack assets keep their typed placeholder instead of pulling a
/// per-file thumbnail: past this, the gzip re-inflation cost per payload (§7 / INGESTION §2) is not
/// worth it and a big pack should be extracted for thumbnails. ponytail: a per-page batch pull lifts
/// this; the cap is the cheap guard until then.
pub const IN_PACK_THUMB_MAX_BYTES: u64 = 256 * 1024 * 1024;

pub fn needs_thumbnail(mt: Option<MediaType>) -> bool {
    matches!(
        mt,
        // Model3d thumbnails are rendered offscreen by the CPU rasteriser in `thumb::generate`
        // (no GPU context needed, so it runs on the same worker threads as image decoding).
        // Audio takes the waveform branch in `generate_texture`.
        Some(
            MediaType::Image
                | MediaType::Texture
                | MediaType::Sprite
                | MediaType::Model3d
                | MediaType::Audio
                | MediaType::Material
        )
    )
}

/// Materialize an in-pack payload to a **stable** per-payload scratch file in the thumb cache
/// (`inpack/<key>.<ext>`), pulling it once and reusing it across runs (DATA-MODEL §7 / M14 step 4).
/// The path-based thumbnail generator then decodes it exactly like an on-disk file — images, models
/// (hero render) and audio (waveform) all work. Keyed by pack path + offset + size so it is stable
/// (unlike an OS-temp name), which also keeps the thumbnail cache entry itself stable. Runs on the
/// worker thread since the gzip skip-scan can be slow. Returns the scratch path + payload size (the
/// cache-key inputs); `None` if there is no cache dir or the pull fails.
pub fn ensure_in_pack_scratch(
    cache_dir: Option<&std::path::Path>,
    pack_path: &str,
    offset: i64,
    size: i64,
    ext: &str,
) -> Option<(PathBuf, u64)> {
    let cache = cache_dir?;
    let pack = std::path::Path::new(pack_path);
    // The legacy key, deliberately: path, entry size and offset — no archive identity, so a pack
    // replaced with an entry of the same size at the same place keeps its old scratch. Keying this on
    // the archive (as `materialize_in_pack_entry` does) was tried in v1 plan S12b and reverted:
    // it invalidates every in-pack thumbnail in a library at once, and this runs on the protocol
    // handler's thread, so the first grid after an upgrade froze the shell while it re-inflated
    // packs. Invalidating a pack's scratch when a rescan sees its archive change belongs to the
    // cache policy (S15), where that fact is known.
    let key = thumb::meta_key(pack, size as u64, offset);
    let scratch = cache.join("inpack").join(format!("{key}.{ext}"));
    if scratch.exists() {
        return Some((scratch, size as u64));
    }
    let bytes = lodestash_ingest::unitypackage::read_payload_at(pack, offset as u64, size as u64)
        .map_err(|e| tracing::warn!(%e, "in-pack thumbnail pull failed"))
        .ok()?;
    publish(&scratch, &bytes).ok()?;
    Some((scratch, size as u64))
}

/// The scratch key for a verified pull: the archive's path and **current** size and mtime, and the
/// entry's place in it. `None` when the archive cannot be stat'ed.
///
/// The archive's identity is what makes a replaced pack miss the cache — the thumbnail scratch
/// ([`ensure_in_pack_scratch`]) still lacks it, and serves a re-exported pack's old payload.
fn in_pack_key(pack: &std::path::Path, offset: i64, size: i64) -> Option<String> {
    let meta = std::fs::metadata(pack).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let identity = format!("{}|{}|{mtime}|{offset}|{size}", pack.display(), meta.len());
    // `meta_key` hashes a path string with two numbers; the whole identity rides in the string.
    Some(thumb::meta_key(std::path::Path::new(&identity), 0, 0))
}

/// `<key>.<ext>`, keeping only an extension's letters and digits — it comes from a file name in
/// someone's package, and it lands in a path.
fn scratch_name(key: &str, ext: &str) -> String {
    let ext: String = ext
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(12)
        .collect();
    if ext.is_empty() {
        key.to_string()
    } else {
        format!("{key}.{ext}")
    }
}

/// Write `bytes` to `dest` so no reader ever sees a partial file: a temp name unique to this process
/// and call, then a rename. Two concurrent pulls of one entry both succeed — whichever rename lands
/// second replaces identical bytes, and if the replace is refused because the first is being read,
/// the file that is already there is the answer.
fn publish(dest: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let dir = dest
        .parent()
        .ok_or_else(|| std::io::Error::other("scratch path has no parent"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.{}.{}.part",
        dest.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, dest) {
        Ok(()) => Ok(()),
        Err(_) if dest.exists() => {
            let _ = std::fs::remove_file(&tmp);
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// An in-pack asset's payload as a file the file route can serve (ENGINE-API §8.2, v1 plan S12b).
///
/// Verified against the archive's own tar header ([`read_entry_payload`]), refused over the pull
/// ceiling before any read, and published atomically into the thumbnail cache's scratch under a key
/// that includes the archive's identity. It never unpacks the package, never writes into the
/// workspace and never touches the catalog: the asset stays in-pack, and nothing else in the archive
/// is written.
///
/// [`read_entry_payload`]: lodestash_ingest::unitypackage::read_entry_payload
pub fn materialize_in_pack_entry(
    cache: &std::path::Path,
    entry: &lodestash_core::catalog::InPackEntry,
) -> Result<PathBuf, lodestash_ingest::unitypackage::PayloadError> {
    use lodestash_ingest::unitypackage::{read_entry_payload, PayloadError};
    let (Ok(offset), Ok(size)) = (u64::try_from(entry.offset), u64::try_from(entry.size)) else {
        return Err(PayloadError::SourceChanged(format!(
            "the catalogued entry has offset {} and size {}",
            entry.offset, entry.size
        )));
    };
    if size > lodestash_ingest::unitypackage::PAYLOAD_PULL_MAX_BYTES {
        return Err(PayloadError::TooLarge {
            size,
            cap: lodestash_ingest::unitypackage::PAYLOAD_PULL_MAX_BYTES,
        });
    }
    let pack = std::path::Path::new(&entry.package_path);
    let key = in_pack_key(pack, entry.offset, entry.size)
        .ok_or_else(|| PayloadError::ArchiveMissing(entry.package_path.clone()))?;
    // Its own name: the thumbnail pull above reads unverified, and a verified file must not be
    // satisfied by one. The guid is in it too, so a cached pull verified for one entry can never
    // answer a catalog row that names another at the same place.
    let guid: String = entry
        .guid
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(64)
        .collect();
    let path = cache
        .join("inpack")
        .join(scratch_name(&format!("v-{key}-{guid}"), &entry.ext));
    if path.exists() {
        return Ok(path);
    }
    let bytes = read_entry_payload(pack, &entry.guid, offset, size)?;
    publish(&path, &bytes).map_err(PayloadError::Io)?;
    Ok(path)
}

// ponytail: 8 flat args instead of a params struct — one call site, all of them primitives.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A real two-entry `.unitypackage`; returns its path.
    fn package(dir: &std::path::Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("Pack.unitypackage");
        let gz = flate2::write::GzEncoder::new(
            std::fs::File::create(&path).unwrap(),
            flate2::Compression::default(),
        );
        let mut tar = tar::Builder::new(gz);
        for (guid, bytes) in entries {
            for (member, body) in [
                ("asset", *bytes),
                ("asset.meta", b"guid: x\n".as_slice()),
                ("pathname", format!("Assets/{guid}.fbx").as_bytes()),
            ] {
                let mut h = tar::Header::new_gnu();
                h.set_size(body.len() as u64);
                h.set_mode(0o644);
                h.set_cksum();
                tar.append_data(&mut h, format!("{guid}/{member}"), body)
                    .unwrap();
            }
        }
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    fn entry_for(pack: &std::path::Path, guid: &str) -> lodestash_core::catalog::InPackEntry {
        let e = lodestash_ingest::unitypackage::read_manifest(pack)
            .unwrap()
            .into_iter()
            .find(|e| e.guid == guid)
            .unwrap();
        lodestash_core::catalog::InPackEntry {
            package_path: pack.to_string_lossy().into_owned(),
            guid: guid.into(),
            offset: e.asset_offset.unwrap() as i64,
            size: e.asset_size.unwrap() as i64,
            ext: "fbx".into(),
        }
    }

    /// Real-archive probe (v1 plan S12b): pull one catalogued entry out of an actual package into a
    /// temporary cache — the archive is only read — and report the time and the bytes' shape.
    /// `LODESTASH_PROBE_PACK`, `_GUID`, `_OFFSET`, `_SIZE` name the entry.
    #[test]
    #[ignore]
    fn probe_in_pack_payload() {
        let var = |k: &str| std::env::var(format!("LODESTASH_PROBE_{k}")).expect(k);
        let entry = lodestash_core::catalog::InPackEntry {
            package_path: var("PACK"),
            guid: var("GUID"),
            offset: var("OFFSET").parse().unwrap(),
            size: var("SIZE").parse().unwrap(),
            ext: "fbx".into(),
        };
        let cache = tempfile::tempdir().unwrap();
        let cold = std::time::Instant::now();
        let path = materialize_in_pack_entry(cache.path(), &entry).expect("pull");
        let cold = cold.elapsed();
        let warm = std::time::Instant::now();
        assert_eq!(
            materialize_in_pack_entry(cache.path(), &entry).unwrap(),
            path
        );
        let warm = warm.elapsed();
        let bytes = std::fs::read(&path).unwrap();
        println!(
            "pulled {} bytes cold in {cold:?}, warm in {warm:?}; starts {:?}",
            bytes.len(),
            String::from_utf8_lossy(&bytes[..bytes.len().min(20)])
        );
        assert_eq!(bytes.len() as i64, entry.size);
    }

    /// One asset's payload out of a package, exactly (v1 plan S12b): the bytes the archive holds for
    /// that entry and nothing of the other one, the archive itself unchanged, and a single scratch
    /// file — concurrent pulls of the same entry included. A replaced archive is a new key and new
    /// bytes, not the old file.
    #[test]
    fn an_in_pack_file_is_exactly_its_own_payload() {
        let dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let wanted = b"the model we asked for ".repeat(64);
        let unrelated = b"someone else's model ".repeat(64);
        let pack = package(
            dir.path(),
            &[
                ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &wanted),
                ("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &unrelated),
            ],
        );
        let archive_before = std::fs::read(&pack).unwrap();
        let entry = entry_for(&pack, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

        let paths: Vec<PathBuf> = std::thread::scope(|s| {
            let pulls: Vec<_> = (0..4)
                .map(|_| s.spawn(|| materialize_in_pack_entry(cache.path(), &entry).unwrap()))
                .collect();
            pulls.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(
            paths.windows(2).all(|w| w[0] == w[1]),
            "concurrent pulls disagree on the file"
        );
        assert_eq!(std::fs::read(&paths[0]).unwrap(), wanted);
        let written: Vec<_> = std::fs::read_dir(cache.path().join("inpack"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            written.len(),
            1,
            "more than the one payload was written: {written:?}"
        );
        assert_eq!(
            std::fs::read(&pack).unwrap(),
            archive_before,
            "the archive was modified"
        );

        // Re-exported: same guid, same size, same place — different bytes, a later mtime.
        let changed = b"THE MODEL WE ASKED FOR ".repeat(64);
        drop(package(
            dir.path(),
            &[
                ("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", &changed),
                ("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &unrelated),
            ],
        ));
        std::fs::File::options()
            .write(true)
            .open(&pack)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            .unwrap();
        let after = materialize_in_pack_entry(
            cache.path(),
            &entry_for(&pack, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        )
        .unwrap();
        assert_eq!(
            std::fs::read(&after).unwrap(),
            changed,
            "a replaced archive served the old bytes"
        );

        use lodestash_ingest::unitypackage::PayloadError;
        let huge = lodestash_core::catalog::InPackEntry {
            size: (lodestash_ingest::unitypackage::PAYLOAD_PULL_MAX_BYTES + 1) as i64,
            ..entry.clone()
        };
        assert!(matches!(
            materialize_in_pack_entry(cache.path(), &huge),
            Err(PayloadError::TooLarge { .. })
        ));
        // A stale catalog: the other entry's guid at this entry's place. The header disagrees, so
        // nothing is served — the pull is verified, not trusted.
        let stale = lodestash_core::catalog::InPackEntry {
            guid: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
            ..entry.clone()
        };
        assert!(matches!(
            materialize_in_pack_entry(cache.path(), &stale),
            Err(PayloadError::SourceChanged(_))
        ));
        let negative = lodestash_core::catalog::InPackEntry {
            offset: -1,
            ..entry.clone()
        };
        assert!(matches!(
            materialize_in_pack_entry(cache.path(), &negative),
            Err(PayloadError::SourceChanged(_))
        ));
        let gone = lodestash_core::catalog::InPackEntry {
            package_path: dir
                .path()
                .join("gone.unitypackage")
                .to_string_lossy()
                .into_owned(),
            ..entry
        };
        assert!(matches!(
            materialize_in_pack_entry(cache.path(), &gone),
            Err(PayloadError::ArchiveMissing(_))
        ));
    }

    /// The delete-from-disk guard: only files under a source root the user opened, or inside the
    /// workspace this app owns. Everything else is refused, whatever path composition produced it.
    #[test]
    fn only_paths_under_known_roots_may_be_deleted() {
        let roots = vec![
            PathBuf::from("G:/Game_Assets/Synty"),
            PathBuf::from("C:/lib"),
        ];
        let ws = PathBuf::from("C:/cache/Lodestash/workspace");
        let ok = |p: &str| under_known_root(std::path::Path::new(p), &roots, Some(&ws));

        assert!(ok("G:/Game_Assets/Synty/Models/rock.fbx"));
        assert!(
            ok("C:/cache/Lodestash/workspace/abc/tex.png"),
            "workspace is ours"
        );
        assert!(
            !ok("C:/Windows/System32/kernel32.dll"),
            "outside every root"
        );
        assert!(
            !ok("G:/Game_Assets/Other/rock.fbx"),
            "sibling of a root is not in it"
        );
        assert!(
            !under_known_root(std::path::Path::new("C:/anything"), &roots, None),
            "no workspace configured means no workspace deletions"
        );
    }
}
