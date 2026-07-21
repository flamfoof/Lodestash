//! Thumbnail generation and on-disk cache.
//! Cache key: `{content_hash}_{width}x{height}.png`
//! Spec: INGESTION.md §1 (thumbnailing), DATA-MODEL.md §6 (non-destructive).

use std::path::{Path, PathBuf};

use image::{DynamicImage, ImageFormat};

/// Parameters for thumbnail generation.
#[derive(Debug, Clone, Copy)]
pub struct ThumbParams {
    pub width: u32,
    pub height: u32,
}

impl Default for ThumbParams {
    fn default() -> Self {
        Self {
            width: 256,
            height: 256,
        }
    }
}

/// Derive the cache path for a thumbnail.
/// `cache_dir` is the app's thumbnail cache directory (e.g. `~/.lodestash/thumbs/`).
pub fn cache_path(cache_dir: &Path, content_hash: &str, params: ThumbParams) -> PathBuf {
    cache_dir.join(format!(
        "{}_{}x{}.png",
        content_hash, params.width, params.height
    ))
}

/// Cache key for a file identified only by path metadata — same shape as a content hash.
/// ponytail: (path,size,mtime) key until the catalog wires content_hash into the UI;
/// an edited-in-place file with identical size+mtime serves a stale thumb, same as the crawler diff.
pub fn meta_key(path: &Path, size_bytes: u64, mtime: i64) -> String {
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(&size_bytes.to_le_bytes());
    hasher.update(&mtime.to_le_bytes());
    hasher.finalize().to_hex()[..32].to_string()
}

/// Generate a thumbnail from an image file and write it to the cache.
/// Returns the path of the written thumbnail.
/// Skips writing if the cache file already exists (content_hash guarantees freshness).
pub fn generate(
    source: &Path,
    cache_dir: &Path,
    content_hash: &str,
    params: ThumbParams,
) -> std::io::Result<PathBuf> {
    let dest = cache_path(cache_dir, content_hash, params);
    if dest.exists() {
        return Ok(dest);
    }

    std::fs::create_dir_all(cache_dir)?;

    // 3D models have no pixels to decode — render one hero view instead. Same cache key and
    // same output contract, so callers don't care which branch produced the PNG.
    let img = if is_model(source) {
        render_model_thumb(source, params.width.max(params.height))?
    } else {
        crate::raster::decode(source)?
    };

    let thumb = resize_fit(&img, params.width, params.height);

    thumb
        .save_with_format(&dest, ImageFormat::Png)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    Ok(dest)
}

/// The sound token (`#49C9E0`), so a waveform thumb reads as audio at a glance the same way the
/// type diamond does (DESIGN-GUIDELINES §7).
pub const WAVEFORM_RGB: [u8; 3] = [0x49, 0xC9, 0xE0];

/// Bars a waveform thumbnail of `width` px draws (2 px bar + 1 px gap).
pub fn waveform_bars(width: u32) -> usize {
    (width as usize / 3).max(8)
}

/// Write a waveform thumbnail to the cache, calling `peaks` only on a cache miss — decoding the
/// audio is the expensive half. `peaks` yields per-bar magnitudes in `0.0..=1.0`
/// ([`waveform_bars`] of them); audio decoding lives with the player, so the caller supplies it.
pub fn generate_waveform(
    cache_dir: &Path,
    content_hash: &str,
    params: ThumbParams,
    peaks: impl FnOnce(usize) -> Result<Vec<f32>, String>,
) -> std::io::Result<PathBuf> {
    let dest = cache_path(cache_dir, content_hash, params);
    if dest.exists() {
        return Ok(dest);
    }
    std::fs::create_dir_all(cache_dir)?;
    let peaks = peaks(waveform_bars(params.width)).map_err(std::io::Error::other)?;
    draw_waveform(&peaks, params.width, params.height)
        .save_with_format(&dest, ImageFormat::Png)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(dest)
}

/// Draw peaks as a centre-mirrored bar waveform on a **transparent** background (same contract as
/// the 3D hero render: the dark grid card shows through). Silence still draws a 1 px centre line so
/// an empty clip looks like a clip, not a failed thumbnail.
pub fn draw_waveform(peaks: &[f32], width: u32, height: u32) -> DynamicImage {
    let (w, h) = (width.max(1), height.max(1));
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([0, 0, 0, 0]));
    let [r, g, b] = WAVEFORM_RGB;
    let mid = (h - 1) as f32 / 2.0;
    let bars = peaks.len().max(1);
    for x in 0..w {
        // Bars are 2 px wide with a 1 px gap; skip the gap column.
        if x % 3 == 2 {
            continue;
        }
        let p = peaks[(x as usize * bars / w as usize).min(bars - 1)].clamp(0.0, 1.0);
        let half = (p * mid).max(0.5);
        let (y0, y1) = (
            (mid - half).round() as u32,
            (mid + half).round().min((h - 1) as f32) as u32,
        );
        for y in y0..=y1 {
            img.put_pixel(x, y, image::Rgba([r, g, b, 255]));
        }
    }
    DynamicImage::ImageRgba8(img)
}

/// Formats the mesh loaders handle (`mesh::load_any`): FBX/OBJ via ufbx, glTF/GLB via
/// three-d-asset.
fn is_model(path: &Path) -> bool {
    crate::mesh::is_fbx_model(path)
        || crate::mesh::is_gltf_model(path)
        || path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("obj"))
}

/// Render a 3D model to a single 3/4 hero view on a **transparent** background, so the grid card
/// shows through. Reuses the same CPU rasteriser the embedding pipeline uses — no GPU context,
/// so this runs happily on the thumbnail worker threads.
fn render_model_thumb(source: &Path, size: u32) -> std::io::Result<DynamicImage> {
    let meshes = crate::mesh::load_any(source, &std::collections::BTreeMap::new())
        .map_err(std::io::Error::other)?;
    let views = crate::render3d::render_views_on(
        &meshes,
        size.max(1),
        &crate::render3d::DEFAULT_VIEWS[..1],
        [0, 0, 0, 0],
    );
    let img = views
        .into_iter()
        .next()
        .ok_or_else(|| std::io::Error::other("model has no renderable geometry"))?;
    Ok(DynamicImage::ImageRgba8(img))
}

/// Resize an image to fit within `max_w × max_h`, preserving aspect ratio.
/// Uses Lanczos3 for downscaling (best quality for asset previews).
fn resize_fit(img: &DynamicImage, max_w: u32, max_h: u32) -> DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w <= max_w && h <= max_h {
        return img.clone();
    }
    img.resize(max_w, max_h, image::imageops::FilterType::Lanczos3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_test_png(path: &Path) {
        // 64×64 red square
        let img = DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
            64,
            64,
            image::Rgba([255u8, 0, 0, 255]),
        ));
        img.save_with_format(path, ImageFormat::Png).unwrap();
    }

    #[test]
    fn generate_creates_thumbnail() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("test.png");
        make_test_png(&src);

        let cache_dir = dir.path().join("thumbs");
        let params = ThumbParams {
            width: 32,
            height: 32,
        };
        let hash = "deadbeef";

        let dest = generate(&src, &cache_dir, hash, params).unwrap();
        assert!(dest.exists(), "thumbnail should be created");

        let thumb = image::open(&dest).unwrap();
        assert!(thumb.width() <= 32 && thumb.height() <= 32);
    }

    #[test]
    fn generate_skips_existing() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("test.png");
        make_test_png(&src);

        let cache_dir = dir.path().join("thumbs");
        let params = ThumbParams::default();
        let hash = "abc123";

        let dest1 = generate(&src, &cache_dir, hash, params).unwrap();
        let mtime1 = fs::metadata(&dest1).unwrap().modified().unwrap();

        // Sleep a tiny bit then regenerate — mtime should be identical (skipped)
        std::thread::sleep(std::time::Duration::from_millis(10));
        let dest2 = generate(&src, &cache_dir, hash, params).unwrap();
        let mtime2 = fs::metadata(&dest2).unwrap().modified().unwrap();

        assert_eq!(
            mtime1, mtime2,
            "existing thumbnail should not be regenerated"
        );
    }

    #[test]
    fn cache_path_deterministic() {
        let p1 = cache_path(
            Path::new("/cache"),
            "abc",
            ThumbParams {
                width: 128,
                height: 128,
            },
        );
        let p2 = cache_path(
            Path::new("/cache"),
            "abc",
            ThumbParams {
                width: 128,
                height: 128,
            },
        );
        assert_eq!(p1, p2);
        assert!(p1.to_str().unwrap().contains("128x128"));
    }

    /// A waveform thumb must be transparent outside the wave, taller where the clip is louder,
    /// and still draw something for silence.
    #[test]
    fn waveform_thumb_is_transparent_and_amplitude_shaped() {
        let bars = waveform_bars(96);
        let loud: Vec<f32> = (0..bars)
            .map(|i| if i < bars / 2 { 0.9 } else { 0.05 })
            .collect();
        let img = draw_waveform(&loud, 96, 96).to_rgba8();
        assert_eq!(
            img.get_pixel(0, 0)[3],
            0,
            "corner transparent for the grid card"
        );
        let ink = |x: u32| (0..96).filter(|&y| img.get_pixel(x, y)[3] > 0).count();
        assert!(
            ink(0) > ink(90),
            "loud half must be taller than the quiet half"
        );
        assert_eq!(ink(2), 0, "every third column is the bar gap");

        let silence = draw_waveform(&vec![0.0; bars], 96, 96).to_rgba8();
        assert!(
            silence.pixels().any(|p| p[3] > 0),
            "silence still draws a centre line"
        );
    }

    #[test]
    fn generate_waveform_caches_and_only_decodes_on_a_miss() {
        let dir = tempfile::tempdir().unwrap();
        let params = ThumbParams {
            width: 64,
            height: 64,
        };
        let calls = std::cell::Cell::new(0);
        let peaks = |n: usize| {
            calls.set(calls.get() + 1);
            Ok(vec![0.7; n])
        };
        let a = generate_waveform(dir.path(), "wav1", params, peaks).unwrap();
        let b = generate_waveform(dir.path(), "wav1", params, |n| {
            calls.set(calls.get() + 1);
            Ok(vec![0.7; n])
        })
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(calls.get(), 1, "cache hit must not decode the audio again");
        assert!(image::open(&a).unwrap().width() == 64);
    }

    /// A 3D model must thumbnail through the render path: real geometry drawn, and the corners
    /// left transparent so the dark grid card shows through.
    #[test]
    fn generates_a_thumbnail_for_a_3d_model() {
        let dir = std::env::temp_dir().join(format!("lodestash_thumb3d_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A wide, flat box as a handwritten OBJ (ufbx parses OBJ).
        let mut obj = String::new();
        for (dx, dy, dz) in [
            (-1.0f32, -0.3f32, -0.3f32),
            (1.0, -0.3, -0.3),
            (1.0, 0.3, -0.3),
            (-1.0, 0.3, -0.3),
            (-1.0, -0.3, 0.3),
            (1.0, -0.3, 0.3),
            (1.0, 0.3, 0.3),
            (-1.0, 0.3, 0.3),
        ] {
            obj.push_str(&format!("v {dx} {dy} {dz}\n"));
        }
        for f in [
            [1, 2, 3],
            [1, 3, 4],
            [6, 5, 8],
            [6, 8, 7],
            [5, 1, 4],
            [5, 4, 8],
            [2, 6, 7],
            [2, 7, 3],
            [4, 3, 7],
            [4, 7, 8],
            [5, 6, 2],
            [5, 2, 1],
        ] {
            obj.push_str(&format!("f {} {} {}\n", f[0], f[1], f[2]));
        }
        let src = dir.join("box.obj");
        std::fs::write(&src, obj).unwrap();

        let params = ThumbParams {
            width: 96,
            height: 96,
        };
        let out = generate(&src, &dir, "hash3d", params).expect("generate 3d thumb");
        assert!(out.exists());
        let img = image::open(&out).expect("decode thumb").to_rgba8();
        assert!(img.width() <= 96 && img.height() <= 96);

        let opaque = img.pixels().filter(|p| p[3] > 0).count();
        assert!(
            opaque > 200,
            "expected rendered geometry, got {opaque} opaque px"
        );
        assert_eq!(
            img.get_pixel(0, 0)[3],
            0,
            "corner must stay transparent for the grid card"
        );

        // Regenerating is a cache hit (same path, no re-render).
        assert_eq!(generate(&src, &dir, "hash3d", params).unwrap(), out);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
