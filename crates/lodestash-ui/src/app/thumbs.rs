//! Grid texture upload — the egui half of thumbnailing.
//!
//! Choosing *what* to render and pulling in-pack payloads moved to `lodestash_app::thumbs`
//! (B1); this is only the part that needs a graphics context.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn generate_texture(
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
