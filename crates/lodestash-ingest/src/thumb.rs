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
        Self { width: 256, height: 256 }
    }
}

/// Derive the cache path for a thumbnail.
/// `cache_dir` is the app's thumbnail cache directory (e.g. `~/.lodestash/thumbs/`).
pub fn cache_path(cache_dir: &Path, content_hash: &str, params: ThumbParams) -> PathBuf {
    cache_dir.join(format!("{}_{}x{}.png", content_hash, params.width, params.height))
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

    let img = image::open(source)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;

    let thumb = resize_fit(&img, params.width, params.height);

    thumb
        .save_with_format(&dest, ImageFormat::Png)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

    Ok(dest)
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
        let params = ThumbParams { width: 32, height: 32 };
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

        assert_eq!(mtime1, mtime2, "existing thumbnail should not be regenerated");
    }

    #[test]
    fn cache_path_deterministic() {
        let p1 = cache_path(Path::new("/cache"), "abc", ThumbParams { width: 128, height: 128 });
        let p2 = cache_path(Path::new("/cache"), "abc", ThumbParams { width: 128, height: 128 });
        assert_eq!(p1, p2);
        assert!(p1.to_str().unwrap().contains("128x128"));
    }
}
