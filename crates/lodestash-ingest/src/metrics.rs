//! Technical-facet extraction: what a file measurably *is* (DATA-MODEL §14).
//!
//! Triangle counts and pixel dimensions only — audio duration needs a decoder, and the one this
//! workspace already has (`rodio`) lives with the player in `lodestash-ui`, so the UI fills that
//! field in. Everything here is read-only on the source file and returns `None` rather than an
//! error: a facet that can't be measured is simply unknown (§14).

use std::path::Path;

use lodestash_core::catalog::AssetMetrics;

/// Extract whatever facets `media_code` implies. Unknown/unsupported types yield an empty set,
/// which the caller still stores — clearing the dirty flag is the forward-progress rule (§14).
pub fn extract(path: &Path, media_code: &str) -> AssetMetrics {
    match media_code {
        "model_3d" => AssetMetrics {
            tri_count: triangle_count(path),
            ..Default::default()
        },
        "image" | "texture" | "sprite" => {
            let (width_px, height_px) = match dimensions(path) {
                Some((w, h)) => (Some(w), Some(h)),
                None => (None, None),
            };
            AssetMetrics {
                width_px,
                height_px,
                ..Default::default()
            }
        }
        _ => AssetMetrics::default(),
    }
}

/// Triangles across every mesh in the file. Needs a full parse — the same one the thumbnail and
/// multiview renders do, and there is no cheaper header answer for FBX/OBJ/glTF.
pub fn triangle_count(path: &Path) -> Option<i64> {
    let meshes = crate::mesh::load_any(path, &std::collections::BTreeMap::new()).ok()?;
    let tris: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
    (tris > 0).then_some(tris as i64)
}

/// Pixel dimensions. Tries the header-only path first (`image` reads a few bytes); DDS/PSD/TGA and
/// anything else exotic fall back to the shared decode seam, which costs a full decode.
pub fn dimensions(path: &Path) -> Option<(i64, i64)> {
    if let Ok((w, h)) = image::image_dimensions(path) {
        return Some((w as i64, h as i64));
    }
    let img = crate::raster::decode(path).ok()?;
    Some((img.width() as i64, img.height() as i64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_dimensions_come_from_the_header() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("wide.png");
        image::DynamicImage::ImageRgba8(image::ImageBuffer::from_pixel(
            40,
            10,
            image::Rgba([1u8, 2, 3, 255]),
        ))
        .save(&p)
        .unwrap();

        let m = extract(&p, "texture");
        assert_eq!((m.width_px, m.height_px), (Some(40), Some(10)));
        assert_eq!(m.tri_count, None, "a texture has no triangles");
    }

    #[test]
    fn a_mesh_reports_its_triangles() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tri.obj");
        // Two triangles sharing an edge (a quad, triangulated by hand).
        std::fs::write(&p, "v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3\nf 1 3 4\n").unwrap();
        assert_eq!(extract(&p, "model_3d").tri_count, Some(2));
    }

    #[test]
    fn an_unreadable_file_is_unknown_not_zero() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("broken.png");
        std::fs::write(&p, b"not a png").unwrap();
        assert_eq!(extract(&p, "image"), AssetMetrics::default());
        assert_eq!(extract(&p, "audio"), AssetMetrics::default());
    }
}
