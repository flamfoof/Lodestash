//! Grid thumbnail + texture generation, split out of app.rs. Child module of `app`, reaches
//! app's private items via `use super::*`.
#![allow(clippy::too_many_arguments)]
use super::*;

/// What kind of thumbnail an asset needs. Materials and audio have no pixels of their own, so
/// each gets a rendered stand-in rather than a decode.
#[derive(Debug, Clone)]
pub(super) enum ThumbKind {
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
pub(super) fn under_known_root(
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
pub(super) const IN_PACK_THUMB_MAX_BYTES: u64 = 256 * 1024 * 1024;

pub(super) fn needs_thumbnail(mt: Option<MediaType>) -> bool {
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
pub(super) fn ensure_in_pack_scratch(
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
#[allow(clippy::too_many_arguments)]
pub(super) fn generate_texture(
    ctx: &egui::Context,
    abs_path: &std::path::Path,
    tex_name: &str,
    size_bytes: u64,
    mtime: i64,
    cache_dir: Option<&std::path::Path>,
    gen_px: u32,
    proxy: lodestash_ingest::decimate::Budget,
    kind: ThumbKind,
) -> Option<TextureHandle> {
    // Serve from / populate the on-disk thumbnail cache (INGESTION.md §6): restarts then
    // decode cached PNGs instead of full-size originals. The cache filename embeds WxH
    // (`thumb::cache_path`), so each on-screen-quality size gets its own cache entry. Falls back
    // to a direct decode when the cache dir is unavailable or generation fails (exotic format).
    // A stored vector preview short-circuits the whole cache: it needs no source file, no mesh
    // parse and no PNG on disk — just a rasterise at whatever size the grid is showing.
    if let ThumbKind::Vector { svg, view } = &kind {
        let one = lodestash_ingest::vectorize::view_svg(svg, *view);
        if let Some(img) = lodestash_ingest::vectorize::rasterize(&one, gen_px) {
            let (w, h) = img.dimensions();
            let color = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img);
            return Some(ctx.load_texture("thumb", color, egui::TextureOptions::LINEAR));
        }
        // A preview that won't rasterise falls through to the ordinary render path below.
    }
    let img = cache_dir
        .and_then(|dir| {
            let key = thumb::meta_key(abs_path, size_bytes, mtime);
            let params = thumb::ThumbParams {
                width: gen_px,
                height: gen_px,
                proxy,
            };
            match &kind {
                // Audio has no pixels: draw the peak waveform the Quick Look player already
                // decodes (rodio lives in this crate, so the decode stays here and `thumb` only
                // owns the cache key + drawing).
                ThumbKind::Audio => thumb::generate_waveform(dir, &key, params, |bars| {
                    crate::audio::load_waveform(abs_path, bars).map(|m| m.peaks)
                })
                .ok(),
                ThumbKind::Material { albedo } => {
                    // The texture is part of the picture, so it is part of the cache key: swap the
                    // material's albedo and the old ball must not be served.
                    let key = match albedo {
                        Some(a) => format!("{key}-{}", thumb::meta_key(a, 0, 0)),
                        None => format!("{key}-flat"),
                    };
                    thumb::generate_material(dir, &key, params, albedo.as_deref()).ok()
                }
                ThumbKind::Raster => thumb::generate(abs_path, dir, &key, params).ok(),
                // Handled above; only reachable if its SVG failed to rasterise, in which case the
                // ordinary render is the right fallback.
                ThumbKind::Vector { .. } => thumb::generate(abs_path, dir, &key, params).ok(),
            }
        })
        .and_then(|cached| image::open(cached).ok())
        .or_else(|| image::open(abs_path).ok())?;
    let img = img.thumbnail(gen_px, gen_px).to_rgba8();
    let (w, h) = img.dimensions();
    let color_img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img);
    Some(ctx.load_texture(tex_name, color_img, egui::TextureOptions::LINEAR))
}
