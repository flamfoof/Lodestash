//! Material previews: the "material sphere" every DCC shows for a shader ball
//! (UI-SPEC §13.1, INGESTION §5). A `.mat` has no geometry of its own, so it is previewed by
//! painting it onto a sphere.
//!
//! The texture comes from the dependency graph (DATA-MODEL §10) — the `uses_texture` edge the
//! `.meta` harvest already resolved — so this module takes decoded pixels and never touches the
//! catalog: same rule as the rest of `ingest`.

use std::sync::Arc;

use image::RgbaImage;

use crate::mesh::{AlbedoTex, MeshData};
use crate::render3d::{self, View};

/// A UV sphere with `segments` around and `rings` down, centred at the origin.
///
/// Spherical UVs (u = longitude, v = latitude) rather than the material's own mapping: a tiling
/// texture has no single "correct" projection onto a ball, and longitude/latitude is what a shader
/// ball shows everywhere else, so it reads as a preview instead of as a claim about the mapping.
pub fn uv_sphere(segments: u32, rings: u32, radius: f32) -> MeshData {
    let segments = segments.max(3);
    let rings = rings.max(2);
    let mut positions = Vec::with_capacity(((rings + 1) * (segments + 1)) as usize);
    let mut normals = Vec::with_capacity(positions.capacity());
    let mut uvs = Vec::with_capacity(positions.capacity());
    for ring in 0..=rings {
        // 0 at the north pole, PI at the south.
        let phi = std::f32::consts::PI * ring as f32 / rings as f32;
        let (sin_phi, cos_phi) = phi.sin_cos();
        for seg in 0..=segments {
            let theta = std::f32::consts::TAU * seg as f32 / segments as f32;
            let (sin_theta, cos_theta) = theta.sin_cos();
            let n = [sin_phi * cos_theta, cos_phi, sin_phi * sin_theta];
            positions.push([n[0] * radius, n[1] * radius, n[2] * radius]);
            normals.push(n);
            // v is flipped so the texture's top lands on the sphere's top: `AlbedoTex` rows are
            // stored bottom-up (see `mesh::AlbedoTex`).
            uvs.push([
                seg as f32 / segments as f32,
                1.0 - ring as f32 / rings as f32,
            ]);
        }
    }
    let mut indices = Vec::with_capacity((rings * segments * 6) as usize);
    let stride = segments + 1;
    for ring in 0..rings {
        for seg in 0..segments {
            let a = ring * stride + seg;
            let b = a + stride;
            // Pole rings degenerate to a single triangle; emitting both is harmless (the
            // degenerate one covers no pixels) and keeps the loop free of special cases.
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    MeshData {
        name: "material_sphere".into(),
        positions,
        normals: Some(normals),
        uvs: Some(uvs),
        indices,
        base_color: [200, 200, 200, 255],
        albedo: None,
    }
}

/// Render a material preview: `albedo` painted onto a sphere, or the flat `base_color` when the
/// material has no texture the library could resolve (an unresolved `uses_texture` edge, or a
/// material that is pure colour).
pub fn render_sphere(
    albedo: Option<&RgbaImage>,
    base_color: [u8; 4],
    size: u32,
    bg: [u8; 4],
) -> Option<RgbaImage> {
    let mut sphere = uv_sphere(48, 32, 1.0);
    sphere.base_color = base_color;
    sphere.albedo = albedo.map(|img| {
        Arc::new(AlbedoTex {
            width: img.width(),
            height: img.height(),
            // Bottom-up, matching what `mesh` produces for model textures — one convention, so
            // the rasteriser's sampler needs no per-source special case.
            rgba: img.rows().rev().flat_map(|row| row.map(|p| p.0)).collect(),
        })
    });
    // One three-quarter view: a shader ball is read at a glance, not orbited.
    render3d::render_views_on(
        &[sphere],
        size,
        &[View {
            azimuth: 0.6,
            elevation: 0.5,
        }],
        bg,
    )
    .into_iter()
    .next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cache path the grid actually calls, against a real texture — a render that only works
    /// in a unit test is not a feature.
    #[test]
    #[ignore = "writes PNGs for visual inspection"]
    fn dump_cached_material_thumb() {
        let out = std::path::PathBuf::from(std::env::var("SPHERE_OUT").expect("set SPHERE_OUT"));
        let tex = std::env::var("SPHERE_TEX").ok();
        let png = crate::thumb::generate_material(
            &out.join("cache"),
            "probe-key",
            crate::thumb::ThumbParams {
                width: 256,
                height: 256,
            },
            tex.as_deref().map(std::path::Path::new),
        )
        .expect("generate_material");
        println!("cached thumb: {}", png.display());
        let img = image::open(&png).expect("decode").to_rgba8();
        assert!(img.pixels().any(|p| p[3] > 0), "thumb is fully transparent");
    }

    /// Writes real previews to look at: the standard is that a render gets eyeballed, not just
    /// asserted (coverage checks pass happily on garbage).
    #[test]
    #[ignore = "writes PNGs for visual inspection"]
    fn dump_material_spheres() {
        let out = std::path::PathBuf::from(std::env::var("SPHERE_OUT").expect("set SPHERE_OUT"));
        std::fs::create_dir_all(&out).unwrap();
        // Flat grey ball.
        render_sphere(None, [200, 200, 200, 255], 256, [24, 26, 32, 255])
            .unwrap()
            .save(out.join("flat.png"))
            .unwrap();
        // A real library texture, if one was named.
        if let Ok(tex) = std::env::var("SPHERE_TEX") {
            let img = image::open(&tex).expect("texture").to_rgba8();
            render_sphere(Some(&img), [255, 255, 255, 255], 256, [24, 26, 32, 255])
                .unwrap()
                .save(out.join("textured.png"))
                .unwrap();
        }
        println!("wrote to {}", out.display());
    }

    #[test]
    fn the_sphere_is_a_sphere() {
        let s = uv_sphere(16, 8, 2.0);
        assert_eq!(s.positions.len(), 17 * 9);
        assert_eq!(s.indices.len(), 8 * 16 * 6);
        // Every vertex sits on the radius, and its normal points straight out from the centre.
        for (p, n) in s.positions.iter().zip(s.normals.as_ref().unwrap()) {
            let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
            assert!((r - 2.0).abs() < 1e-4, "radius {r}");
            // p is n scaled by the radius, so the unit-normalised dot is 1 when n points out.
            let dot = (p[0] * n[0] + p[1] * n[1] + p[2] * n[2]) / r;
            assert!((dot - 1.0).abs() < 1e-3, "normal not radial: {dot}");
        }
        assert!(
            s.indices.iter().all(|&i| (i as usize) < s.positions.len()),
            "index out of range"
        );
    }

    /// The point of the preview: the texture has to actually show up, and a textured ball must not
    /// look like an untextured one.
    #[test]
    fn the_texture_reaches_the_sphere() {
        let flat = render_sphere(None, [200, 200, 200, 255], 96, [0, 0, 0, 0]).expect("flat");
        let red = RgbaImage::from_pixel(8, 8, image::Rgba([220, 30, 30, 255]));
        let textured =
            render_sphere(Some(&red), [200, 200, 200, 255], 96, [0, 0, 0, 0]).expect("textured");

        let centre = |img: &RgbaImage| *img.get_pixel(img.width() / 2, img.height() / 2);
        let c = centre(&textured);
        assert!(c[3] > 0, "the sphere covers the centre");
        assert!(
            c[0] > c[1] * 2 && c[0] > c[2] * 2,
            "textured centre should read red, got {c:?}"
        );
        let f = centre(&flat);
        assert!(
            f[0].abs_diff(f[1]) < 12 && f[1].abs_diff(f[2]) < 12,
            "untextured centre should stay neutral, got {f:?}"
        );

        // A corner is outside the ball, so the transparent background shows through — that is
        // what lets the preview sit on the dark card without a light box around it.
        assert_eq!(textured.get_pixel(0, 0)[3], 0, "corner is background");
    }

    /// Shading has to vary across the ball, or it renders as a flat disc and reads as a swatch
    /// rather than a sphere.
    #[test]
    fn the_ball_is_shaded_not_flat() {
        let img = render_sphere(None, [200, 200, 200, 255], 96, [0, 0, 0, 0]).expect("render");
        let lit: Vec<u8> = img.pixels().filter(|p| p[3] > 0).map(|p| p[0]).collect();
        let (min, max) = lit
            .iter()
            .fold((255u8, 0u8), |(lo, hi), &v| (lo.min(v), hi.max(v)));
        assert!(
            max - min > 40,
            "expected a lighting gradient across the ball, got {min}..{max}"
        );
    }
}
