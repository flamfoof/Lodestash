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
    let key = thumb::meta_key(std::path::Path::new(pack_path), size as u64, offset);
    let dir = cache.join("inpack");
    let scratch = dir.join(format!("{key}.{ext}"));
    if scratch.exists() {
        return Some((scratch, size as u64));
    }
    let bytes = lodestash_ingest::unitypackage::read_payload_at(
        std::path::Path::new(pack_path),
        offset as u64,
        size as u64,
    )
    .map_err(|e| tracing::warn!(%e, "in-pack thumbnail pull failed"))
    .ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    // Write to a `.part` then rename, so a sibling payload's thumbnail can't read a half-written file.
    let tmp = dir.join(format!("{key}.{ext}.part"));
    std::fs::write(&tmp, &bytes).ok()?;
    std::fs::rename(&tmp, &scratch).ok()?;
    Some((scratch, size as u64))
}

// ponytail: 8 flat args instead of a params struct — one call site, all of them primitives.

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

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
