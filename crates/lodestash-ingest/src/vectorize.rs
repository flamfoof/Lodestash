//! Vector previews: turn a rendered view into a **traced contour SVG** — a silhouette plus a
//! couple of nested tone bands. Spec: DESIGN-GUIDELINES §7 (asset card), DATA-MODEL §6.
//!
//! Why trace the raster instead of projecting the geometry: SVG has no z-buffer, so emitting the
//! mesh means one painter-sorted `<path>` per triangle — ~1500 paths and 120–200 KB per model,
//! rendering interpenetrating faces wrong. Tracing the alpha of a render the CPU rasteriser already
//! produces gives ~7 paths and a few KB, scales from a 32 px list row to a 780 px Quick Look off one
//! artifact, and suits the design's flat low-fidelity cards better than a shaded render would.
//!
//! The bands are **nested by construction** (each is a subset of the one below it), so painting them
//! in order replaces depth sorting entirely.
//!
//! ponytail: boundary-edge chaining rather than textbook marching squares — the saddle cases that
//! make marching squares fiddly simply don't arise when every edge is a unit segment between an
//! inside and an outside pixel, and Douglas-Peucker smooths the staircase away regardless.

use std::collections::HashMap;

use image::RgbaImage;

/// A closed ring in image pixel coordinates.
pub type Contour = Vec<[f32; 2]>;
/// One rendered view: its bands, outermost (silhouette) first, each a set of rings.
pub type ViewBands = Vec<Vec<Contour>>;

/// Tracing parameters.
#[derive(Debug, Clone, Copy)]
pub struct Bands {
    /// Total bands including the silhouette. 1 = outline only; 3 is the design default.
    pub levels: usize,
    /// Douglas-Peucker tolerance in pixels — the "decimate close distances" knob. ~1.5 px at 256²
    /// removes the staircase without visibly rounding corners.
    pub epsilon: f32,
    /// Rings enclosing less than this many pixels are dropped as speckle.
    pub min_area: f32,
}

impl Default for Bands {
    fn default() -> Self {
        Self {
            levels: 3,
            epsilon: 1.5,
            min_area: 12.0,
        }
    }
}

/// Alpha at or below this is background. The rasteriser writes hard edges, so this only rejects
/// stray near-transparent pixels.
const ALPHA_FLOOR: u8 = 8;

/// Trace one rendered view into nested bands.
pub fn trace(img: &RgbaImage, opts: Bands) -> ViewBands {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 || opts.levels == 0 {
        return Vec::new();
    }

    // Luminance inside the silhouette, and its peak — the band thresholds are fractions of the peak
    // rather than absolutes, so a dark material still gets interior shading instead of one flat blob.
    let lum_at = |x: u32, y: u32| -> Option<f32> {
        let p = img.get_pixel(x, y).0;
        (p[3] > ALPHA_FLOOR)
            .then(|| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
    };
    let mut peak = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            if let Some(l) = lum_at(x, y) {
                peak = peak.max(l);
            }
        }
    }

    (0..opts.levels)
        .map(|level| {
            let mask: Vec<bool> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| match lum_at(x, y) {
                    // Band 0 is the silhouette: everything the render covers.
                    Some(_) if level == 0 => true,
                    // Bands 1.. tighten toward the lit side. Evenly spaced across the upper half of
                    // the observed range, so `levels` stays a free knob.
                    Some(l) => l >= peak * (0.35 + 0.25 * level as f32),
                    None => false,
                })
                .collect();
            rings(&mask, w, h)
                .into_iter()
                .filter_map(|ring| {
                    let simplified = simplify_ring(&ring, opts.epsilon);
                    (simplified.len() >= 3 && area(&simplified).abs() >= opts.min_area)
                        .then_some(simplified)
                })
                .collect()
        })
        .collect()
}

/// Chain the boundary edges of a binary mask into closed rings.
///
/// Every inside pixel contributes a unit edge for each outside neighbour, wound so an outer boundary
/// runs one way and a hole runs the other. Each ring vertex then has matching in/out degree, so
/// following edges start→end always closes — and the opposite hole winding is exactly what
/// `fill-rule="evenodd"` needs to punch the hole out.
fn rings(mask: &[bool], w: u32, h: u32) -> Vec<Vec<[u32; 2]>> {
    let inside = |x: i64, y: i64| -> bool {
        x >= 0 && y >= 0 && x < w as i64 && y < h as i64 && mask[(y as u32 * w + x as u32) as usize]
    };

    // start point -> the ends it can travel to (a vertex touched by two diagonal blobs has two).
    let mut out: HashMap<[u32; 2], Vec<[u32; 2]>> = HashMap::new();
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            if !inside(x, y) {
                continue;
            }
            let (x0, y0, x1, y1) = (x as u32, y as u32, x as u32 + 1, y as u32 + 1);
            let mut edge = |a: [u32; 2], b: [u32; 2]| out.entry(a).or_default().push(b);
            if !inside(x, y - 1) {
                edge([x0, y0], [x1, y0]);
            }
            if !inside(x + 1, y) {
                edge([x1, y0], [x1, y1]);
            }
            if !inside(x, y + 1) {
                edge([x1, y1], [x0, y1]);
            }
            if !inside(x - 1, y) {
                edge([x0, y1], [x0, y0]);
            }
        }
    }

    let mut result = Vec::new();
    // Deterministic output regardless of HashMap iteration order — the SVG is cached, so a stable
    // byte-for-byte encoding matters.
    let mut starts: Vec<[u32; 2]> = out.keys().copied().collect();
    starts.sort_unstable();
    for start in starts {
        while out.get(&start).is_some_and(|v| !v.is_empty()) {
            let mut ring = vec![start];
            let mut at = start;
            // A missing successor can't happen for a well-formed mask (in/out degree match), but
            // ending the walk rather than spinning is the safe reading.
            while let Some(next) = out.get_mut(&at).and_then(Vec::pop) {
                if next == start {
                    break;
                }
                ring.push(next);
                at = next;
                if ring.len() > (w as usize + h as usize) * 4 + 8 {
                    break; // Belt and braces: no ring can be longer than the image perimeter.
                }
            }
            if ring.len() >= 3 {
                result.push(ring);
            }
        }
    }
    result
}

/// Shoelace area — sign carries the winding, which distinguishes a hole from an outer ring.
fn area(ring: &[[f32; 2]]) -> f32 {
    let n = ring.len();
    if n < 3 {
        return 0.0;
    }
    let mut acc = 0.0;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        acc += a[0] * b[1] - b[0] * a[1];
    }
    acc / 2.0
}

/// Douglas-Peucker over a **closed** ring. DP is defined on open polylines, so the ring is cut at
/// two anchors — the first point and the point farthest from it — simplified as two polylines, and
/// rejoined. Cutting at the extremes keeps the shape's most distinctive corners pinned.
fn simplify_ring(ring: &[[u32; 2]], epsilon: f32) -> Contour {
    let pts: Contour = ring.iter().map(|p| [p[0] as f32, p[1] as f32]).collect();
    if pts.len() < 4 || epsilon <= 0.0 {
        return pts;
    }
    let far = (1..pts.len())
        .max_by(|&a, &b| {
            dist2(pts[0], pts[a])
                .partial_cmp(&dist2(pts[0], pts[b]))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(pts.len() - 1);

    // First half runs pts[0]..=pts[far]; second runs pts[far]..=pts[0] (wrapped).
    let mut out = simplify_open(&pts[..=far], epsilon);
    let mut tail: Contour = pts[far..].to_vec();
    tail.push(pts[0]);
    let second = simplify_open(&tail, epsilon);
    // Both halves carry the two shared anchors, and the ring closes implicitly — so append the
    // second half without its first point (== pts[far]) or its last (== pts[0]).
    let inner = second.len().saturating_sub(1);
    out.extend(second.into_iter().take(inner).skip(1));
    out
}

/// Iterative Douglas-Peucker on an open polyline. Iterative rather than recursive: a 256² ring can
/// carry ~1000 points, and a degenerate split would recurse that deep.
fn simplify_open(pts: &[[f32; 2]], epsilon: f32) -> Contour {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;

    let mut stack = vec![(0usize, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let mut worst = 0.0f32;
        let mut worst_i = a;
        for (i, p) in pts.iter().enumerate().take(b).skip(a + 1) {
            let d = perp_distance(*p, pts[a], pts[b]);
            if d > worst {
                worst = d;
                worst_i = i;
            }
        }
        if worst > epsilon {
            keep[worst_i] = true;
            stack.push((a, worst_i));
            stack.push((worst_i, b));
        }
    }
    pts.iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

fn dist2(a: [f32; 2], b: [f32; 2]) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    dx * dx + dy * dy
}

/// Distance from `p` to the segment `a`–`b`. Degenerate segment falls back to point distance.
fn perp_distance(p: [f32; 2], a: [f32; 2], b: [f32; 2]) -> f32 {
    let len2 = dist2(a, b);
    if len2 <= f32::EPSILON {
        return dist2(a, p).sqrt();
    }
    let t = (((p[0] - a[0]) * (b[0] - a[0])) + ((p[1] - a[1]) * (b[1] - a[1]))) / len2;
    let t = t.clamp(0.0, 1.0);
    let proj = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
    dist2(proj, p).sqrt()
}

/// Per-band opacity, outermost first — the silhouette reads as a tinted mass, the lit bands sit on
/// top of it. Beyond the listed bands everything holds the last value.
const BAND_ALPHA: &[f32] = &[0.30, 0.55, 0.85];

/// Emit one SVG document holding every view as a `<g id="v0">…`, with only `v0` displayed. One
/// artifact per asset means one cache entry, one DB row, and a hover-spin that is a visibility
/// toggle rather than four fetches.
///
/// Paths paint `currentColor`, and the root carries `color` — so a webview restyles the whole
/// preview with one CSS rule (`color: var(--ls-type-model)`) while renderers with no CSS support
/// still get the right colour from the attribute.
pub fn to_svg(views: &[ViewBands], size: u32, color: &str) -> String {
    let mut s = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" color="{color}">"#
    );
    for (i, bands) in views.iter().enumerate() {
        s.push_str(&format!(
            r#"<g id="v{i}"{}>"#,
            if i == 0 { "" } else { r#" display="none""# }
        ));
        for (level, rings) in bands.iter().enumerate() {
            if rings.is_empty() {
                continue;
            }
            let alpha = BAND_ALPHA[level.min(BAND_ALPHA.len() - 1)];
            s.push_str(&format!(
                r#"<path fill="currentColor" fill-rule="evenodd" fill-opacity="{alpha:.2}" d=""#
            ));
            for ring in rings {
                push_ring(&mut s, ring);
            }
            s.push_str(r#""/>"#);
        }
        s.push_str("</g>");
    }
    s.push_str("</svg>");
    s
}

/// One ring as `M x y L … Z`, coordinates rounded to 1 decimal — the extra precision is invisible at
/// any preview size and is pure bytes in a cached artifact.
fn push_ring(s: &mut String, ring: &Contour) {
    for (i, p) in ring.iter().enumerate() {
        s.push_str(if i == 0 { "M" } else { "L" });
        s.push_str(&format!("{:.1} {:.1}", p[0], p[1]));
        s.push(' ');
    }
    s.push('Z');
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an RGBA image from a `#`/`.` picture — opaque mid-grey where `#`, transparent where `.`.
    fn mask_img(rows: &[&str]) -> RgbaImage {
        let h = rows.len() as u32;
        let w = rows[0].len() as u32;
        RgbaImage::from_fn(w, h, |x, y| {
            if rows[y as usize].as_bytes()[x as usize] == b'#' {
                image::Rgba([128, 128, 128, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        })
    }

    /// These fixtures are only a few pixels across, so they use a sub-pixel epsilon: the production
    /// default (1.5 px at 256²) would legitimately collapse a 2×2 square past a polygon.
    fn silhouette(rows: &[&str], epsilon: f32) -> Vec<Contour> {
        trace_at(&mask_img(rows), epsilon)
    }

    fn trace_at(img: &RgbaImage, epsilon: f32) -> Vec<Contour> {
        trace(
            img,
            Bands {
                levels: 1,
                epsilon,
                min_area: 0.0,
            },
        )
        .remove(0)
    }

    #[test]
    fn a_filled_square_traces_to_a_four_corner_ring() {
        let out = silhouette(&["....", ".##.", ".##.", "...."], 0.5);
        assert_eq!(out.len(), 1, "one ring");
        assert_eq!(
            out[0].len(),
            4,
            "Douglas-Peucker collapses the staircase to corners: {:?}",
            out[0]
        );
        // The ring bounds the filled pixels exactly (edges run on pixel boundaries).
        let xs: Vec<f32> = out[0].iter().map(|p| p[0]).collect();
        let ys: Vec<f32> = out[0].iter().map(|p| p[1]).collect();
        assert_eq!(xs.iter().cloned().fold(f32::MAX, f32::min), 1.0);
        assert_eq!(xs.iter().cloned().fold(f32::MIN, f32::max), 3.0);
        assert_eq!(ys.iter().cloned().fold(f32::MAX, f32::min), 1.0);
        assert_eq!(ys.iter().cloned().fold(f32::MIN, f32::max), 3.0);
    }

    #[test]
    fn a_ring_with_a_hole_yields_two_contours_with_opposite_winding() {
        let out = silhouette(
            &["......", ".####.", ".#..#.", ".#..#.", ".####.", "......"],
            0.5,
        );
        assert_eq!(out.len(), 2, "outer boundary + hole");
        let areas: Vec<f32> = out.iter().map(|r| area(r)).collect();
        assert!(
            areas.iter().any(|a| *a > 0.0) && areas.iter().any(|a| *a < 0.0),
            "opposite winding is what makes fill-rule=evenodd punch the hole: {areas:?}"
        );
    }

    #[test]
    fn disjoint_islands_each_get_a_ring() {
        let out = silhouette(&[".....", ".#.#.", ".....", ".#.#.", "....."], 0.5);
        assert_eq!(out.len(), 4, "four separate pixels, four rings");
    }

    /// The epsilon knob is the decimation — mutation-check it by proving a larger tolerance keeps
    /// strictly fewer points, and that zero tolerance keeps the raw staircase.
    #[test]
    fn epsilon_governs_how_many_points_survive() {
        // A 64px disc: its traced boundary is a long staircase, which is exactly what DP exists to
        // eat. Big enough that the production epsilon is in range, unlike the ASCII fixtures.
        let disc = RgbaImage::from_fn(64, 64, |x, y| {
            let (dx, dy) = (x as f32 - 32.0, y as f32 - 32.0);
            if dx * dx + dy * dy <= 24.0 * 24.0 {
                image::Rgba([180, 180, 180, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            }
        });
        let raw = trace_at(&disc, 0.0);
        let tight = trace_at(&disc, 0.5);
        let loose = trace_at(&disc, 2.0);
        assert!(
            raw[0].len() > tight[0].len() && tight[0].len() > loose[0].len(),
            "more tolerance, fewer points: {} > {} > {}",
            raw[0].len(),
            tight[0].len(),
            loose[0].len()
        );
        assert!(loose[0].len() >= 8, "a disc still reads as a disc");
    }

    #[test]
    fn bands_are_nested_and_shrink_inward() {
        // A bright core inside a dim body: band 1 must be strictly inside band 0.
        let img = RgbaImage::from_fn(16, 16, |x, y| {
            let inside = (4..12).contains(&x) && (4..12).contains(&y);
            let bright = (6..10).contains(&x) && (6..10).contains(&y);
            match (inside, bright) {
                (_, true) => image::Rgba([255, 255, 255, 255]),
                (true, _) => image::Rgba([60, 60, 60, 255]),
                _ => image::Rgba([0, 0, 0, 0]),
            }
        });
        let bands = trace(
            &img,
            Bands {
                levels: 2,
                epsilon: 0.0,
                min_area: 0.0,
            },
        );
        assert_eq!(bands.len(), 2);
        let extent = |rings: &[Contour]| {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for p in rings.iter().flatten() {
                lo = lo.min(p[0]);
                hi = hi.max(p[0]);
            }
            (lo, hi)
        };
        let (lo0, hi0) = extent(&bands[0]);
        let (lo1, hi1) = extent(&bands[1]);
        assert!(
            lo1 >= lo0 && hi1 <= hi0 && (hi1 - lo1) < (hi0 - lo0),
            "band 1 ({lo1},{hi1}) must sit strictly inside band 0 ({lo0},{hi0})"
        );
    }

    #[test]
    fn empty_and_degenerate_inputs_produce_nothing_rather_than_panicking() {
        let blank = RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 0]));
        let bands = trace(&blank, Bands::default());
        assert!(bands.iter().all(|b| b.is_empty()), "no geometry, no rings");
        assert!(trace(&RgbaImage::new(0, 0), Bands::default()).is_empty());
        // A fully-opaque image is one ring around the whole frame, not a crash.
        let full = RgbaImage::from_pixel(8, 8, image::Rgba([200, 200, 200, 255]));
        assert_eq!(
            trace(
                &full,
                Bands {
                    levels: 1,
                    ..Bands::default()
                }
            )[0]
            .len(),
            1
        );
    }

    /// Rasterise `svg` at `size`² over the dark surface token, so it can be viewed as a PNG.
    fn rasterize(svg: &str, size: u32) -> RgbaImage {
        use resvg::usvg::TreeParsing;
        let parsed = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default())
            .expect("the generated SVG must parse");
        let scale = size as f32 / parsed.size.width();
        let tree = resvg::Tree::from_usvg(&parsed);
        // The design's surface token, so the preview is judged against the card it will sit on.
        let mut pix = resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap");
        pix.fill(resvg::tiny_skia::Color::from_rgba8(0x16, 0x1B, 0x26, 0xFF));
        tree.render(
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pix.as_mut(),
        );
        RgbaImage::from_raw(size, size, pix.data().to_vec()).expect("pixmap -> image")
    }

    /// The generated SVG must survive a real SVG parser, not just look like markup.
    #[test]
    fn generated_svg_parses_and_renders_ink() {
        let views: Vec<ViewBands> = vec![vec![silhouette(
            &["......", ".####.", ".####.", ".####.", ".####.", "......"],
            0.5,
        )]];
        let img = rasterize(&to_svg(&views, 6, "#E7B765"), 96);
        let inked = img.pixels().filter(|p| p.0[0] > 0x40).count();
        assert!(
            inked > 96 * 96 / 8,
            "the traced square should cover a real share of the frame, got {inked} px"
        );
    }

    /// Visual probe — a coverage assertion passes happily on garbage, so the preview is only
    /// trusted once the PNG is viewed (HANDOFF §5). Writes the source render and the traced SVG's
    /// rasterisation side by side, and reports the SVG's byte size:
    /// `LODESTASH_PROBE_FBX=<path> LODESTASH_PROBE_OUT=<dir> cargo test -p lodestash-ingest
    /// probe_vector -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_vector_preview() {
        let path = std::env::var("LODESTASH_PROBE_FBX").expect("set LODESTASH_PROBE_FBX");
        let out = std::path::PathBuf::from(
            std::env::var("LODESTASH_PROBE_OUT").expect("set LODESTASH_PROBE_OUT"),
        );
        let full = crate::mesh::load_any(std::path::Path::new(&path), &Default::default())
            .expect("load model");
        let proxy = crate::decimate::decimate_meshes(&full, crate::decimate::PREVIEW_BUDGET);
        let renders = crate::render3d::render_views_on(
            &proxy,
            256,
            crate::render3d::DEFAULT_VIEWS,
            [0, 0, 0, 0],
        );

        let views: Vec<ViewBands> = renders.iter().map(|r| trace(r, Bands::default())).collect();
        let svg = to_svg(&views, 256, "#E7B765");
        println!(
            "{} views, {} paths, {} bytes ({:.1} KB)",
            views.len(),
            svg.matches("<path").count(),
            svg.len(),
            svg.len() as f32 / 1024.0
        );
        std::fs::write(out.join("preview.svg"), &svg).expect("write svg");
        renders[0]
            .save(out.join("vec-source.png"))
            .expect("write source");
        rasterize(&svg, 256)
            .save(out.join("vec-traced.png"))
            .expect("write traced");
        println!(
            "wrote vec-source.png / vec-traced.png / preview.svg to {}",
            out.display()
        );
    }

    #[test]
    fn svg_is_well_formed_and_only_the_first_view_is_shown() {
        let views: Vec<ViewBands> = (0..4)
            .map(|_| silhouette(&["....", ".##.", ".##.", "...."], 0.5))
            .map(|rings| vec![rings])
            .collect();
        let svg = to_svg(&views, 256, "#7BD88F");

        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        assert_eq!(svg.matches("<g id=").count(), 4, "one group per view");
        assert_eq!(
            svg.matches(r#"display="none""#).count(),
            3,
            "v0 visible, the other three hidden for hover-spin"
        );
        // r##…##: the hex colour contains `"#`, which would close a plain r#"…"# literal.
        assert!(
            svg.contains(r##"color="#7BD88F""##),
            "root carries the token colour"
        );
        assert!(svg.contains("currentColor"), "paths follow it");
        assert!(svg.contains("fill-rule=\"evenodd\""), "holes punch through");
        // It parses as XML (cheap structural proof without pulling in a parser).
        assert_eq!(svg.matches("<g ").count(), svg.matches("</g>").count());
    }
}
