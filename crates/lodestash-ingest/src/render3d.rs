//! Headless multiview renderer for 3D assets (AI-PIPELINE §3, IDENTIFICATION-AND-SEARCH §3
//! Tier 1: "SigLIP 2 (image / **3D-multiview**)"). Renders a mesh from a few orbit viewpoints so
//! the image tower can embed a model the same way it embeds a texture — that's what makes
//! "a stone sword" match a `.fbx` with no useful filename.
//!
//! **CPU rasteriser on purpose.** The embed drain runs on a worker thread while eframe's Glow
//! context is UI-thread-bound, so GPU offscreen rendering would need a headless context and
//! cross-thread coordination. A small z-buffered rasteriser is thread-safe, needs no GPU and no
//! new dependencies, and is plenty at embedding resolution (a few hundred px).
//! Tradeoff: photoreal/textured output gets harder later — this is silhouette + shading only.
//!
//! ponytail: orthographic projection, flat material colour + Lambert shading, no textures, no
//! shadows/AA. Silhouette and form are what the encoder keys on; albedo sampling is the obvious
//! upgrade (`MeshData` already carries `uvs` + `albedo`) if retrieval quality asks for it.

use image::{Rgba, RgbaImage};

use crate::mesh::MeshData;

/// One orbit viewpoint, in degrees.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub azimuth: f32,
    pub elevation: f32,
}

/// Four orbit views at a slight downward tilt — enough to disambiguate front/back/side shapes
/// without paying for more encoder passes.
pub const DEFAULT_VIEWS: &[View] = &[
    View { azimuth: 45.0, elevation: 20.0 },
    View { azimuth: 135.0, elevation: 20.0 },
    View { azimuth: 225.0, elevation: 20.0 },
    View { azimuth: 315.0, elevation: 20.0 },
];

/// Neutral studio-ish background (a plain light backdrop reads like a product render, which is
/// well inside the encoder's training distribution).
const BG: [u8; 3] = [235, 235, 238];
/// Fraction of the frame the model spans (leaves a margin so nothing clips).
const FIT: f32 = 0.88;

type V3 = [f32; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm(v: V3) -> V3 {
    let l = dot(v, v).sqrt();
    if l > 1e-9 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}

/// World-space bounds of every mesh: `(center, radius)`. Radius is 0 for empty/degenerate input.
fn bounds(meshes: &[MeshData]) -> (V3, f32) {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    let mut any = false;
    for m in meshes {
        for p in &m.positions {
            any = true;
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
    }
    if !any {
        return ([0.0; 3], 0.0);
    }
    let center = [
        (lo[0] + hi[0]) * 0.5,
        (lo[1] + hi[1]) * 0.5,
        (lo[2] + hi[2]) * 0.5,
    ];
    // Half-diagonal: a sphere that always contains the box, so no view ever clips.
    let radius = (0..3)
        .map(|i| (hi[i] - lo[i]) * 0.5)
        .fold(0.0f32, |a, d| a + d * d)
        .sqrt()
        .max(1e-6);
    (center, radius)
}

/// Render `meshes` from each of `views` at `size`×`size`. Returns one RGBA image per view;
/// empty if the meshes carry no geometry.
pub fn render_views(meshes: &[MeshData], size: u32, views: &[View]) -> Vec<RgbaImage> {
    let (center, radius) = bounds(meshes);
    if radius <= 0.0 || size == 0 {
        return Vec::new();
    }
    views
        .iter()
        .map(|v| render_one(meshes, size, *v, center, radius))
        .collect()
}

fn render_one(meshes: &[MeshData], size: u32, view: View, center: V3, radius: f32) -> RgbaImage {
    let n = size as usize;
    let mut img = RgbaImage::from_pixel(size, size, Rgba([BG[0], BG[1], BG[2], 255]));
    let mut depth = vec![f32::MAX; n * n];

    // Camera basis: eye on the unit sphere at (azimuth, elevation), looking at the origin.
    let (az, el) = (view.azimuth.to_radians(), view.elevation.to_radians());
    let eye = [el.cos() * az.cos(), el.sin(), el.cos() * az.sin()];
    let forward = norm([-eye[0], -eye[1], -eye[2]]);
    let right = norm(cross(forward, [0.0, 1.0, 0.0]));
    let up = cross(right, forward);
    // Key light slightly up-left of the camera, plus ambient, so form reads clearly.
    let light = norm([-0.4, 0.6, -1.0]);

    for m in meshes {
        let base = m.base_color;
        for tri in m.indices.chunks_exact(3) {
            let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
            let Some(p0) = m.positions.get(i0) else { continue };
            let Some(p1) = m.positions.get(i1) else { continue };
            let Some(p2) = m.positions.get(i2) else { continue };

            // Normalise into the unit sphere, then project onto the camera basis (orthographic).
            let to_view = |p: &[f32; 3]| -> V3 {
                let q = [
                    (p[0] - center[0]) / radius,
                    (p[1] - center[1]) / radius,
                    (p[2] - center[2]) / radius,
                ];
                [dot(q, right), dot(q, up), dot(q, forward)]
            };
            let (v0, v1, v2) = (to_view(p0), to_view(p1), to_view(p2));

            // Face normal in view space (per-face keeps it robust when normals are absent).
            let fnorm = norm(cross(sub(v1, v0), sub(v2, v0)));
            // Two-sided lighting: many game meshes have inconsistent winding.
            let lambert = dot(fnorm, light).abs();
            let shade = (0.35 + 0.65 * lambert).clamp(0.0, 1.0);
            let color = [
                (base[0] as f32 * shade) as u8,
                (base[1] as f32 * shade) as u8,
                (base[2] as f32 * shade) as u8,
            ];

            // View space (x,y in [-1,1]) → pixels, y down.
            let half = size as f32 * 0.5;
            let to_px = |v: V3| -> [f32; 3] {
                [
                    (v[0] * FIT + 1.0) * half,
                    (1.0 - v[1] * FIT) * half,
                    v[2], // depth: smaller = nearer the camera
                ]
            };
            let (a, b, c) = (to_px(v0), to_px(v1), to_px(v2));

            let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
            let max_x = (a[0].max(b[0]).max(c[0]).ceil() as isize).clamp(0, n as isize) as usize;
            let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
            let max_y = (a[1].max(b[1]).max(c[1]).ceil() as isize).clamp(0, n as isize) as usize;

            let area = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
            if area.abs() < 1e-9 {
                continue; // degenerate / edge-on
            }
            for y in min_y..max_y {
                for x in min_x..max_x {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    // Barycentrics via edge functions.
                    let w0 = ((b[0] - px) * (c[1] - py) - (c[0] - px) * (b[1] - py)) / area;
                    let w1 = ((c[0] - px) * (a[1] - py) - (a[0] - px) * (c[1] - py)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                    let idx = y * n + x;
                    if z < depth[idx] {
                        depth[idx] = z;
                        img.put_pixel(
                            x as u32,
                            y as u32,
                            Rgba([color[0], color[1], color[2], 255]),
                        );
                    }
                }
            }
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube centred at the origin (12 triangles), as plain `MeshData`.
    fn cube() -> MeshData {
        let c = [
            [-1.0, -1.0, -1.0f32],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let faces: [[u32; 4]; 6] = [
            [0, 1, 2, 3],
            [5, 4, 7, 6],
            [4, 0, 3, 7],
            [1, 5, 6, 2],
            [3, 2, 6, 7],
            [4, 5, 1, 0],
        ];
        let mut indices = Vec::new();
        for f in faces {
            indices.extend_from_slice(&[f[0], f[1], f[2], f[0], f[2], f[3]]);
        }
        MeshData {
            name: "cube".into(),
            positions: c.to_vec(),
            normals: None,
            uvs: None,
            indices,
            base_color: [200, 90, 60, 255],
            albedo: None,
        }
    }

    fn coverage(img: &RgbaImage) -> f32 {
        let bg = Rgba([BG[0], BG[1], BG[2], 255]);
        let drawn = img.pixels().filter(|p| **p != bg).count();
        drawn as f32 / (img.width() * img.height()) as f32
    }

    #[test]
    fn renders_all_views_with_visible_geometry() {
        let views = render_views(&[cube()], 128, DEFAULT_VIEWS);
        assert_eq!(views.len(), DEFAULT_VIEWS.len());
        for (i, img) in views.iter().enumerate() {
            assert_eq!(img.dimensions(), (128, 128));
            let cov = coverage(img);
            // A centred cube fills a healthy chunk of frame but never the whole thing (margin).
            assert!(
                cov > 0.15 && cov < 0.95,
                "view {i}: coverage {cov} — geometry should be visible and inside the margin"
            );
        }
    }

    #[test]
    fn empty_or_degenerate_input_is_safe() {
        assert!(render_views(&[], 64, DEFAULT_VIEWS).is_empty());
        let mut m = cube();
        m.positions.clear();
        m.indices.clear();
        assert!(render_views(&[m], 64, DEFAULT_VIEWS).is_empty());
    }

    /// A box stretched along X must look narrow end-on (az 0°, looking down -X) and wide
    /// broadside (az 90°, looking down -Z). This is the real proof the camera orbits — note
    /// *coverage alone can't test the default views*, since 45/135/225/315 on an X-symmetric
    /// box give mirror-image silhouettes with identical area.
    #[test]
    fn azimuth_changes_the_projection() {
        let mut m = cube();
        for p in &mut m.positions {
            p[0] *= 3.0;
        }
        let end_on = render_views(
            std::slice::from_ref(&m),
            96,
            &[View { azimuth: 0.0, elevation: 0.0 }],
        );
        let broadside = render_views(
            std::slice::from_ref(&m),
            96,
            &[View { azimuth: 90.0, elevation: 0.0 }],
        );
        let (a, b) = (coverage(&end_on[0]), coverage(&broadside[0]));
        assert!(
            b > a * 2.0,
            "broadside ({b}) should cover far more than end-on ({a}) — camera isn't orbiting"
        );
    }

    /// The default orbit must produce genuinely different images. Uses a lopsided mesh on
    /// purpose: a *cube* has 4-fold symmetry about Y, so views 90° apart are legitimately
    /// pixel-identical — that's correct geometry, not a bug.
    #[test]
    fn default_views_are_not_identical_images() {
        let mut m = cube();
        for p in &mut m.positions {
            p[0] *= 3.0;
        }
        let v = render_views(&[m], 96, DEFAULT_VIEWS);
        let differing = v[1].pixels().zip(v[0].pixels()).filter(|(x, y)| x != y).count();
        assert!(
            differing > 100,
            "views 0 and 1 are near-identical ({differing} px differ)"
        );
    }
}
