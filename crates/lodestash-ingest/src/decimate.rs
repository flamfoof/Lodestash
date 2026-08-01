//! Mesh decimation for **preview proxies** — vertex-cluster (grid-quantization) collapse over the
//! in-house [`MeshData`] vertex format. Spec: HANDOFF §7 item A.
//!
//! A grid/Quick-Look thumbnail is 128–256 px; rendering a 200k-triangle hero mesh at that size is
//! wasted parse, raster and cache. Collapsing the mesh onto a uniform voxel grid keeps the
//! silhouette (which is all a thumbnail and a SigLIP multiview embedding read) at a fraction of the
//! triangles.
//!
//! ponytail: vertex-cluster, not quadric-error-metric. QEM keeps sharp features better but wants a
//! half-edge structure and an error-heap; at thumbnail fidelity the silhouette is the whole signal.
//! Upgrade path is a drop-in replacement of [`collapse`] if proxies ever feed something that reads
//! surface detail.
//!
//! The **source** triangle count (the `triangles` facet) is never taken from a proxy — `metrics`
//! keeps parsing the full mesh through `mesh::load_cached`.

use std::collections::{HashMap, HashSet};

use crate::mesh::MeshData;

/// How many triangles a preview proxy may keep, as a **ratio of the source clamped to a floor and a
/// ceiling**. A bare ratio misbehaves at both ends — 2% of a 500k-triangle hull is still 10k (no
/// footprint win where it's needed most) while 2% of a 400-triangle prop is nothing — and a bare
/// fixed count spends the same budget on both regardless of what that costs each. The clamps handle
/// the ends; the ratio preserves relative fidelity through the common band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Budget {
    pub ratio: f32,
    pub floor: usize,
    pub ceiling: usize,
}

impl Budget {
    pub const fn new(ratio: f32, floor: usize, ceiling: usize) -> Self {
        Self {
            ratio,
            floor,
            ceiling,
        }
    }

    /// Triangle target for a source of `src_tris`. Never exceeds the source — a proxy bigger than
    /// what it proxies is a contradiction, and [`decimate`] would refuse it anyway.
    pub fn target_for(&self, src_tris: usize) -> usize {
        let scaled = (src_tris as f32 * self.ratio).round() as usize;
        scaled
            .clamp(self.floor.min(self.ceiling), self.ceiling)
            .min(src_tris)
    }

    /// Short, filename-safe identity for this budget. The proxy cache keys on
    /// `(path, size, mtime)` — which says nothing about *how coarsely* the proxy was built — so the
    /// tag goes in the filename too. Without it, raising the on-screen quality would keep serving
    /// the low-quality proxy for every already-cached model.
    pub fn tag(&self) -> String {
        format!(
            "r{}f{}c{}",
            (self.ratio * 1000.0).round() as i64,
            self.floor,
            self.ceiling
        )
    }
}

/// The default preview budget, matching `PreviewQuality::Low` (the grid's default). Callers that
/// have a quality setting should pass its own budget instead.
pub const PREVIEW_BUDGET: Budget = Budget::new(0.02, 300, 1500);

/// Coarsest and finest grid resolution the search will consider. 256³ cells is already finer than
/// any thumbnail budget; 2³ is the floor below which nothing is recognizable.
const MIN_GRID: u32 = 2;
const MAX_GRID: u32 = 256;

/// Decimate one mesh to at most `target_tris` triangles. `None` when it is already under budget (the
/// caller keeps the original) or there is nothing to collapse.
///
/// Binary-searches the grid resolution for the *finest* grid that still fits the budget, so the
/// proxy is as close to the target as the clustering allows rather than arbitrarily coarse.
pub fn decimate(mesh: &MeshData, target_tris: usize) -> Option<MeshData> {
    let src_tris = mesh.indices.len() / 3;
    if target_tris == 0 || src_tris <= target_tris || mesh.positions.is_empty() {
        return None;
    }

    let (mut lo, mut hi) = (MIN_GRID, MAX_GRID);
    let mut best: Option<MeshData> = None;
    while lo <= hi {
        let mid = lo + (hi - lo) / 2;
        let cand = collapse(mesh, mid);
        if cand.indices.len() / 3 <= target_tris {
            best = Some(cand);
            lo = mid + 1;
        } else {
            hi = mid - 1; // mid >= MIN_GRID >= 2, so this can't underflow
        }
    }
    // Even the coarsest grid can overshoot a tiny budget on a dense mesh; take it anyway — it is
    // still far smaller than the source, which is the point.
    let out = best.unwrap_or_else(|| collapse(mesh, MIN_GRID));
    (out.indices.len() < mesh.indices.len()).then_some(out)
}

/// Apply [`decimate`] across a file's meshes under one `budget`, splitting the resolved target
/// between them in proportion to their share of the source triangles (so a 100-triangle prop beside
/// a 90k-triangle hull doesn't get half the budget). Meshes already under their share are cloned
/// through untouched.
pub fn decimate_meshes(meshes: &[MeshData], budget: Budget) -> Vec<MeshData> {
    let total: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
    if total == 0 {
        return meshes.to_vec();
    }
    let target_tris = budget.target_for(total);
    meshes
        .iter()
        .map(|m| {
            let tris = m.indices.len() / 3;
            // Floor of 64 so a small mesh in a big file still keeps a recognizable shape.
            let share = (target_tris * tris / total).max(64);
            decimate(m, share).unwrap_or_else(|| m.clone())
        })
        .collect()
}

/// Collapse every vertex onto an `n`-cell (along the longest axis) uniform grid, average each
/// cell's members into one representative vertex, and re-emit the triangles whose three corners
/// landed in three distinct cells.
fn collapse(mesh: &MeshData, n: u32) -> MeshData {
    let (min, extent) = aabb(&mesh.positions);
    let span = extent[0].max(extent[1]).max(extent[2]);
    // Cubic cells (uniform along every axis) so a flat or elongated mesh isn't stretched.
    let cell = if span > 0.0 { span / n as f32 } else { 1.0 };
    let dims = [
        grid_dim(extent[0], cell),
        grid_dim(extent[1], cell),
        grid_dim(extent[2], cell),
    ];

    let cell_key = |p: &[f32; 3]| -> u64 {
        // A non-finite coordinate casts to 0 here (Rust saturating float→int), so a NaN vertex
        // lands in cell 0 rather than poisoning the key.
        let axis = |v: f32, mn: f32, d: u64| -> u64 {
            (((v - mn) / cell).floor() as i64).clamp(0, d as i64 - 1) as u64
        };
        axis(p[0], min[0], dims[0])
            + axis(p[1], min[1], dims[1]) * dims[0]
            + axis(p[2], min[2], dims[2]) * dims[0] * dims[1]
    };

    let want_normals = mesh.normals.is_some();
    let want_uvs = mesh.uvs.is_some();

    let mut slot: HashMap<u64, u32> = HashMap::new();
    let mut vert_slot: Vec<u32> = Vec::with_capacity(mesh.positions.len());
    let mut pos_acc: Vec<[f64; 3]> = Vec::new();
    let mut nrm_acc: Vec<[f64; 3]> = Vec::new();
    let mut uv_acc: Vec<[f64; 2]> = Vec::new();
    let mut counts: Vec<f64> = Vec::new();

    for (vi, p) in mesh.positions.iter().enumerate() {
        let key = cell_key(p);
        let s = *slot.entry(key).or_insert_with(|| {
            pos_acc.push([0.0; 3]);
            counts.push(0.0);
            if want_normals {
                nrm_acc.push([0.0; 3]);
            }
            if want_uvs {
                uv_acc.push([0.0; 2]);
            }
            (counts.len() - 1) as u32
        }) as usize;
        vert_slot.push(s as u32);
        // Skip a non-finite contribution rather than averaging NaN into the whole cell.
        if p.iter().all(|c| c.is_finite()) {
            for a in 0..3 {
                pos_acc[s][a] += p[a] as f64;
            }
            counts[s] += 1.0;
        }
        if let Some(nv) = mesh.normals.as_ref().and_then(|n| n.get(vi)) {
            for a in 0..3 {
                nrm_acc[s][a] += nv[a] as f64;
            }
        }
        if let Some(uv) = mesh.uvs.as_ref().and_then(|u| u.get(vi)) {
            for a in 0..2 {
                uv_acc[s][a] += uv[a] as f64;
            }
        }
    }

    let n_slots = counts.len();
    let mut positions = Vec::with_capacity(n_slots);
    for s in 0..n_slots {
        let c = counts[s];
        // c == 0 only when every member vertex was non-finite; the AABB minimum is a finite
        // in-bounds stand-in, so the proxy never carries a NaN vertex.
        let p = if c > 0.0 {
            [
                (pos_acc[s][0] / c) as f32,
                (pos_acc[s][1] / c) as f32,
                (pos_acc[s][2] / c) as f32,
            ]
        } else {
            min
        };
        positions.push(p);
    }
    let normals = want_normals.then(|| {
        nrm_acc
            .iter()
            .map(|v| {
                let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                if len > 0.0 && len.is_finite() {
                    [
                        (v[0] / len) as f32,
                        (v[1] / len) as f32,
                        (v[2] / len) as f32,
                    ]
                } else {
                    [0.0, 1.0, 0.0]
                }
            })
            .collect()
    });
    let uvs = want_uvs.then(|| {
        uv_acc
            .iter()
            .zip(&counts)
            .map(|(v, c)| {
                let c = if *c > 0.0 { *c } else { 1.0 };
                [(v[0] / c) as f32, (v[1] / c) as f32]
            })
            .collect()
    });

    // Drop degenerate (two corners in one cell) and duplicate triangles — clustering produces both,
    // and a duplicate costs cache bytes and fill rate for nothing.
    let mut seen: HashSet<[u32; 3]> = HashSet::new();
    let mut indices = Vec::new();
    for tri in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (
            vert_slot[tri[0] as usize],
            vert_slot[tri[1] as usize],
            vert_slot[tri[2] as usize],
        );
        if a == b || b == c || a == c {
            continue;
        }
        let mut sorted = [a, b, c];
        sorted.sort_unstable();
        if seen.insert(sorted) {
            indices.extend_from_slice(&[a, b, c]);
        }
    }

    MeshData {
        name: mesh.name.clone(),
        positions,
        normals,
        uvs,
        indices,
        base_color: mesh.base_color,
        albedo: mesh.albedo.clone(),
    }
}

/// `(min, extent)` of the position cloud. `f32::max`/`min` return the non-NaN operand, so a stray
/// NaN vertex can't collapse the box.
fn aabb(positions: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in positions {
        for a in 0..3 {
            if p[a].is_finite() {
                min[a] = min[a].min(p[a]);
                max[a] = max[a].max(p[a]);
            }
        }
    }
    for a in 0..3 {
        if !min[a].is_finite() || !max[a].is_finite() {
            (min[a], max[a]) = (0.0, 0.0);
        }
    }
    (min, [max[0] - min[0], max[1] - min[1], max[2] - min[2]])
}

fn grid_dim(extent: f32, cell: f32) -> u64 {
    ((extent / cell).ceil() as i64).clamp(1, MAX_GRID as i64) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dense UV sphere: real curved geometry with per-corner normals/uvs, so clustering has
    /// something to actually collapse.
    fn sphere(segments: u32, rings: u32) -> MeshData {
        let m = crate::material::uv_sphere(segments, rings, 1.0);
        assert!(m.indices.len() / 3 > 1000, "fixture is dense enough");
        m
    }

    fn bounds(m: &MeshData) -> ([f32; 3], [f32; 3]) {
        let (min, ext) = aabb(&m.positions);
        (min, [min[0] + ext[0], min[1] + ext[1], min[2] + ext[2]])
    }

    #[test]
    fn decimate_cuts_triangles_and_keeps_the_silhouette() {
        let src = sphere(64, 48);
        let src_tris = src.indices.len() / 3;
        let out = decimate(&src, 500).expect("a dense sphere decimates");
        let tris = out.indices.len() / 3;

        assert!(tris <= 500, "hit the budget: {tris} > 500");
        assert!(tris * 4 < src_tris, "meaningful cut: {tris} of {src_tris}");
        assert!(tris > 0, "not collapsed to nothing");

        // AABB preserved within one grid cell's worth of averaging (positions are cell means, so
        // the extremes pull inward slightly).
        let ((smin, smax), (omin, omax)) = (bounds(&src), bounds(&out));
        for a in 0..3 {
            let span = smax[a] - smin[a];
            assert!(
                (omin[a] - smin[a]).abs() < span * 0.2 && (omax[a] - smax[a]).abs() < span * 0.2,
                "axis {a} bounds drifted: {:?} vs {:?}",
                (smin[a], smax[a]),
                (omin[a], omax[a])
            );
        }
    }

    #[test]
    fn proxy_geometry_is_well_formed() {
        let out = decimate(&sphere(48, 32), 400).expect("decimates");
        assert!(
            out.positions.iter().flatten().all(|c| c.is_finite()),
            "no NaN/inf vertices"
        );
        assert_eq!(out.indices.len() % 3, 0, "whole triangles");
        assert!(
            out.indices
                .iter()
                .all(|&i| (i as usize) < out.positions.len()),
            "every index is in range"
        );
        for tri in out.indices.chunks_exact(3) {
            assert!(
                tri[0] != tri[1] && tri[1] != tri[2] && tri[0] != tri[2],
                "no degenerate triangle {tri:?}"
            );
        }
        // Attribute arrays stay parallel to positions — the renderer indexes them together.
        assert_eq!(
            out.normals.as_ref().map(Vec::len),
            Some(out.positions.len())
        );
        assert_eq!(out.uvs.as_ref().map(Vec::len), Some(out.positions.len()));
        assert!(
            out.normals
                .as_ref()
                .unwrap()
                .iter()
                .flatten()
                .all(|c| c.is_finite()),
            "no NaN normals"
        );
    }

    /// The target-count guard: a mesh already under budget is left alone (mutation-check — drop the
    /// `src_tris <= target_tris` arm and this fails), and a *tighter* budget must yield fewer
    /// triangles than a looser one.
    #[test]
    fn target_count_guard_and_monotonicity() {
        let src = sphere(48, 32);
        let src_tris = src.indices.len() / 3;
        assert!(
            decimate(&src, src_tris).is_none(),
            "already within budget — no proxy"
        );
        assert!(decimate(&src, src_tris + 1).is_none());

        let tight = decimate(&src, 200).expect("decimates").indices.len() / 3;
        let loose = decimate(&src, 1200).expect("decimates").indices.len() / 3;
        assert!(tight <= 200 && loose <= 1200);
        assert!(tight < loose, "tighter budget gives fewer tris");
    }

    #[test]
    fn degenerate_inputs_dont_panic() {
        // Empty mesh.
        let empty = MeshData {
            name: "e".into(),
            positions: vec![],
            normals: None,
            uvs: None,
            indices: vec![],
            base_color: [0; 4],
            albedo: None,
        };
        assert!(decimate(&empty, 100).is_none());

        // Every vertex in one spot (zero-extent AABB) plus a NaN corner.
        let flat = MeshData {
            name: "f".into(),
            positions: vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [f32::NAN, 0.0, 0.0]],
            normals: None,
            uvs: None,
            indices: vec![0, 1, 2, 0, 1, 2],
            base_color: [0; 4],
            albedo: None,
        };
        // Collapses to nothing renderable, but must not panic or emit NaN.
        let out = collapse(&flat, 8);
        assert!(out.positions.iter().flatten().all(|c| c.is_finite()));
    }

    #[test]
    fn budget_is_split_across_meshes() {
        let big = sphere(48, 32);
        let small = crate::material::uv_sphere(6, 4, 1.0);
        let small_tris = small.indices.len() / 3;
        // Fixed 600 via a degenerate ratio, so the split is what's under test, not the clamp.
        let out = decimate_meshes(&[big, small], Budget::new(1.0, 600, 600));
        assert_eq!(out.len(), 2);
        assert!(out[0].indices.len() / 3 <= 600);
        assert_eq!(
            out[1].indices.len() / 3,
            small_tris,
            "a mesh under its share passes through untouched"
        );
    }

    /// The clamps are the whole point of a ratio budget — mutation-check both ends.
    #[test]
    fn budget_ratio_is_clamped_at_both_ends() {
        let b = Budget::new(0.02, 300, 1500);
        // Common band: the ratio governs.
        assert_eq!(b.target_for(20_000), 400);
        // Dense source: the ceiling governs (2% of 500k would be 10k).
        assert_eq!(b.target_for(500_000), 1500);
        // Sparse source: the floor governs (2% of 5k would be 100)…
        assert_eq!(b.target_for(5_000), 300);
        // …but never above the source itself — a proxy can't out-grow what it proxies.
        assert_eq!(b.target_for(120), 120);
        // A floor above the ceiling must not panic in `clamp`.
        assert_eq!(Budget::new(0.5, 9_000, 100).target_for(10_000), 100);
    }

    #[test]
    fn preview_budget_matches_the_low_quality_tier() {
        assert_eq!(PREVIEW_BUDGET, Budget::new(0.02, 300, 1500));
    }

    /// First file in `dir` whose name ends with `suffix` — probe-only convenience.
    fn glob_one(dir: &std::path::Path, suffix: &str) -> Option<std::path::PathBuf> {
        std::fs::read_dir(dir).ok()?.flatten().find_map(|e| {
            let p = e.path();
            p.to_string_lossy().ends_with(suffix).then_some(p)
        })
    }

    /// Visual probe — coverage assertions pass happily on garbage, so a proxy is only trusted after
    /// the PNG is looked at (HANDOFF §5). Writes the full mesh and its proxy side by side:
    /// `LODESTASH_PROBE_FBX=<path> LODESTASH_PROBE_OUT=<dir> cargo test -p lodestash-ingest
    /// probe_proxy -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_proxy_render() {
        let path = std::env::var("LODESTASH_PROBE_FBX").expect("set LODESTASH_PROBE_FBX");
        let out = std::path::PathBuf::from(
            std::env::var("LODESTASH_PROBE_OUT").expect("set LODESTASH_PROBE_OUT"),
        );
        let full = crate::mesh::load_any(std::path::Path::new(&path), &Default::default())
            .expect("load model");
        let proxy = decimate_meshes(&full, PREVIEW_BUDGET);
        let tris = |m: &[MeshData]| m.iter().map(|x| x.indices.len() / 3).sum::<usize>();
        println!("full {} tris -> proxy {} tris", tris(&full), tris(&proxy));

        // On-disk cache footprint, the actual point of the exercise.
        let cache = out.join("meshcache");
        let src = std::path::Path::new(&path);
        crate::mesh::load_cached(src, &cache).expect("full cache");
        let bytes = |g: &str| -> u64 {
            glob_one(&cache.join("mesh"), g)
                .map_or(0, |p| std::fs::metadata(p).map_or(0, |m| m.len()))
        };
        let full_bytes = bytes(".msh");
        crate::mesh::load_preview_cached(src, &cache, PREVIEW_BUDGET).expect("proxy cache");
        println!(
            "cache: full {} KB -> proxy {} KB",
            full_bytes / 1024,
            bytes(".lod") / 1024
        );

        for (label, meshes) in [("full", &full), ("proxy", &proxy)] {
            let img = &crate::render3d::render_views_on(
                meshes,
                256,
                &crate::render3d::DEFAULT_VIEWS[..1],
                [24, 27, 38, 255],
            )[0];
            let dest = out.join(format!("{label}.png"));
            img.save(&dest).expect("write png");
            println!("wrote {}", dest.display());
        }
    }
}
