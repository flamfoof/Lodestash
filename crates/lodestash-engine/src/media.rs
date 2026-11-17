//! Media generation for the byte routes: thumbnails, viewer meshes, slot textures, preview fitting
//! and byte ranges (v2 plan Task 2.3).
//!
//! Pure over paths and the read model's [`ThumbJob`]: no HTTP types and no Tauri. The
//! `lodestash://` scheme handler in the desktop shell calls these today; the `/v1/assets/{id}/…`
//! byte routes call them from P3. What a failure *means* is a [`MediaError`]; which status code
//! says so is the adapter's business.
//!
//! **Non-destructive by construction.** Nothing here writes to an asset, a pack, or anything
//! outside the engine's own cache. A thumbnail may pull one in-pack payload into that cache, under
//! a size cap, because a grid of typed placeholders is not a library; everything else reads.

use std::path::{Path, PathBuf};

use crate::config::PreviewQuality;
use crate::read_model::ThumbJob;

/// Cap on a single byte-range response. A client asking for `bytes=0-` on a 20 GB pack must not
/// make us allocate 20 GB.
pub const MAX_CHUNK: u64 = 8 * 1024 * 1024;

/// Triangle count above which a viewer mesh is decimated rather than shipped whole.
///
/// Everything under this travels whole so textures stay correct (see [`model_mesh`]). At ~50 bytes
/// per triangle of `LSM1` this caps a response near 15 MB, which a local protocol and a GPU buffer
/// both shrug at; beyond it, a banded texture beats a stalled viewer.
pub const MESH_DECIMATE_ABOVE: usize = 300_000;

/// Tessellation of the material shader ball. The same numbers `material::render_sphere` uses for
/// the grid thumbnail, so the card and the viewer show the same object — ~3k triangles, which is
/// nothing to a GPU and smooth under the orbit camera.
const SPHERE_SEGMENTS: u32 = 48;
const SPHERE_RINGS: u32 = 32;

/// Why there are no bytes to serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// No such asset, or its file is gone from disk.
    NotFound(&'static str),
    /// A known asset whose bytes cannot be reached right now — inside a pack over the pull cap, or
    /// a pull that failed. Not "missing": the asset is on screen in the grid.
    NotLocal(&'static str),
    /// The bytes are there and do not decode into what was asked for.
    Unsupported(&'static str),
    /// Parsed fine, and there is nothing to show (a mesh with no geometry, an unpainted slot).
    Empty(&'static str),
    /// Reading our own cache failed.
    Io(&'static str),
}

impl MediaError {
    pub fn message(&self) -> &'static str {
        match self {
            MediaError::NotFound(m)
            | MediaError::NotLocal(m)
            | MediaError::Unsupported(m)
            | MediaError::Empty(m)
            | MediaError::Io(m) => m,
        }
    }
}

/// Fit a traced-contour preview to its own content so every card renders the artwork at the
/// same apparent size.
///
/// The stored SVGs are a fixed `256×256` canvas, and the silhouette sits wherever the source
/// render happened to place it — across a real catalog that is 43% to 81% of the width, which
/// is exactly the "3D cards smaller than audio cards, and some models smaller than others"
/// look. The tracer emits only absolute `M/L/Z` polygon rings, so a coordinate scan gives the
/// exact content box; rewrite the viewBox to that box (square, 4% margin) and the card's
/// square artwork box is filled the same way for every asset — no regeneration needed.
pub fn fit_preview_to_content(svg: &str) -> String {
    // Collect every absolute M/L coordinate pair across all views.
    let mut min_x = f64::INFINITY;
    let mut min_y = f64::INFINITY;
    let mut max_x = f64::NEG_INFINITY;
    let mut max_y = f64::NEG_INFINITY;
    let mut found = false;
    let mut search_from = 0usize;
    while let Some(rel) = svg[search_from..].find("d=\"") {
        let start = search_from + rel + 3;
        let Some(end) = svg[start..].find('"') else {
            break;
        };
        let d = &svg[start..start + end];
        search_from = start + end + 1;
        let bytes = d.as_bytes();
        let mut i = 0usize;
        while i < bytes.len() {
            match bytes[i] {
                // The tracer writes absolute `M x y L x y … Z` — no relative commands, no
                // curves, no commas. Anything else just leaves the bounds untouched.
                b'M' | b'L' => {
                    let rest = &d[i + 1..];
                    let mut nums = rest.split_whitespace();
                    if let (Some(xs), Some(ys)) = (nums.next(), nums.next()) {
                        if let (Ok(x), Ok(y)) = (xs.parse::<f64>(), ys.parse::<f64>()) {
                            min_x = min_x.min(x);
                            min_y = min_y.min(y);
                            max_x = max_x.max(x);
                            max_y = max_y.max(y);
                            found = true;
                        }
                    }
                    // Jump past this pair to the next command letter.
                    let jump = rest
                        .find(['M', 'L', 'Z'])
                        .map(|p| p + 1)
                        .unwrap_or(rest.len());
                    i += 1 + jump;
                }
                _ => i += 1,
            }
        }
    }
    if !found || max_x <= min_x || max_y <= min_y {
        return svg.to_string(); // degenerate or non-contour content — serve as stored
    }
    let side = (max_x - min_x).max(max_y - min_y);
    let pad = side * 0.04;
    let cx = (min_x + max_x) / 2.0;
    let cy = (min_y + max_y) / 2.0;
    let half = side / 2.0 + pad;
    let view = format!(
        "viewBox=\"{:.1} {:.1} {:.1} {:.1}\"",
        cx - half,
        cy - half,
        half * 2.0,
        half * 2.0
    );
    const KEY: &str = "viewBox=";
    match svg.find(KEY) {
        Some(pos) => {
            let after = &svg[pos + KEY.len()..];
            let Some(q) = after.find('"') else {
                return svg.to_string();
            };
            let close = after[q + 1..].find('"').map(|e| q + 1 + e);
            match close {
                Some(end) => format!("{}{}{}", &svg[..pos], view, &after[end + 1..]),
                None => svg.to_string(),
            }
        }
        None => svg.to_string(),
    }
}

/// Materialize an in-pack payload into the thumbnail cache so a generator or parser can read it.
///
/// **This is the one place a read writes anything**, and the exception is narrow by construction:
/// it writes only into our own cache directory, and only the bytes of the one asset being drawn.
/// The `.unitypackage` and the asset's own bytes are untouched, so the non-destructive invariant
/// holds; what it costs is CPU, not correctness.
///
/// **No archive-size cap here.** `.unitypackage` is gzip, which has no random access: pulling one
/// payload inflates the archive from byte zero to that payload's offset, so a single pull from a
/// deep entry in a big pack costs a bounded decompress-skip (seconds on the first hit, then the
/// scratch is cached). That is fine for the *file* and *mesh* routes, which serve one explicitly
/// opened asset. The grid-stampede guard lives in [`render_thumb`], the only caller that fans out
/// across a whole pack.
///
/// ponytail: O(page) independent pulls under the cap; batching a page into one sorted
/// `extract_at_offsets` skip-scan is what lifts it.
pub fn in_pack_scratch(job: &ThumbJob, cache: &Path) -> Option<PathBuf> {
    let loc = job.in_pack.as_ref()?;
    crate::thumbs::ensure_in_pack_scratch(
        Some(cache),
        &loc.pack_path,
        loc.offset,
        loc.size,
        &loc.ext,
    )
    .map(|(p, _)| p)
}

/// A raster thumbnail for one asset, generated on first request and cached under `cache`.
///
/// Images and textures through `generate`, audio through `generate_waveform`, materials through
/// `generate_material`, models through the textured hero render. Generation is synchronous but
/// writes through the on-disk cache, so it happens once per asset.
///
/// `quality` is the persisted grid setting (UI-SPEC §11.3): `thumb_px()` and the matching mesh
/// `proxy_budget()`, so a grid thumb and a cached preview proxy agree on the triangle budget. The
/// cache key carries width, height and the proxy tag, so a quality change re-caches.
pub fn render_thumb(
    job: &ThumbJob,
    cache: &Path,
    quality: PreviewQuality,
) -> Result<Vec<u8>, MediaError> {
    let path = match job.path.clone().filter(|p| p.exists()) {
        Some(p) => p,
        None => {
            // Grid-stampede guard: `.unitypackage` is gzip with no random access, so pulling one
            // payload inflates the archive from byte zero to its offset. On a real multi-GB pack
            // one page of cards would re-inflate gigabytes each — above the cap the card keeps its
            // type placeholder until the pack is extracted (the fast random-access path).
            let over_cap = job
                .in_pack
                .as_ref()
                .and_then(|loc| std::fs::metadata(&loc.pack_path).ok())
                .map(|m| m.len() > crate::thumbs::IN_PACK_THUMB_MAX_BYTES)
                .unwrap_or(false);
            if over_cap {
                return Err(MediaError::NotLocal("pack over the thumbnail pull cap"));
            }
            match in_pack_scratch(job, cache) {
                Some(p) => p,
                // A known in-pack asset whose pull failed. Not "missing".
                None if job.in_pack.is_some() => return Err(MediaError::NotLocal("not_local")),
                None => return Err(MediaError::NotFound("file missing")),
            }
        }
    };

    let side = quality.thumb_px();
    let params = lodestash_ingest::thumb::ThumbParams {
        width: side,
        height: side,
        proxy: quality.proxy_budget(),
    };
    // `(path, size, mtime)`, all three. Passing 0 for the mtime made a re-exported file at the same
    // byte size serve its old thumbnail forever — `generate` returns early on `dest.exists()` and
    // nothing revalidates.
    let meta = std::fs::metadata(&path).ok();
    let key = lodestash_ingest::thumb::meta_key(
        &path,
        meta.as_ref().map(|m| m.len()).unwrap_or(0),
        meta.as_ref()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    );
    let made = match job.media_type.as_str() {
        "audio" => lodestash_ingest::thumb::generate_waveform(cache, &key, params, |bars| {
            crate::audio::load_waveform(&path, bars).map(|m| m.peaks)
        }),
        "material" => {
            // The texture is part of the picture, so it is part of the key: swap a material's
            // albedo and the previously cached ball must not be served.
            let key = match &job.albedo {
                Some(a) => format!("{key}-{}", lodestash_ingest::thumb::meta_key(a, 0, 0)),
                None => format!("{key}-flat"),
            };
            lodestash_ingest::thumb::generate_material(cache, &key, params, job.albedo.as_deref())
        }
        // A model wears what its own file declares and what the graph resolved for the slots it
        // does not (v1 plan S08). A model nothing paints, one too big to rasterise whole, or one
        // that will not parse falls back to the untextured hero render.
        "model_3d" => lodestash_ingest::thumb::generate_model_textured(
            &path,
            cache,
            &key,
            params,
            &job.slot_textures,
        )
        .or_else(|_| lodestash_ingest::thumb::generate(&path, cache, &key, params)),
        _ => lodestash_ingest::thumb::generate(&path, cache, &key, params),
    };
    made.and_then(std::fs::read).map_err(|e| {
        tracing::debug!(%e, ?path, "thumbnail generation failed");
        MediaError::Unsupported("no thumbnail for this file")
    })
}

/// The material shader ball as `LSM1` — the same object the grid thumbnail renders, generated, so
/// it needs no file at all: an in-pack material previews without pulling anything out of its pack.
pub fn material_sphere() -> Vec<u8> {
    let ball = lodestash_ingest::material::uv_sphere(SPHERE_SEGMENTS, SPHERE_RINGS, 1.0);
    lodestash_ingest::mesh::encode_meshes(std::slice::from_ref(&ball))
}

/// A model's geometry as `LSM1` for the WebGL viewer, split by material slot.
///
/// Deliberately the **same encoder as the on-disk mesh cache** (`mesh::encode_meshes`): the wire
/// format and the cache format are one thing, so the TypeScript parser and `decode_meshes` cannot
/// drift apart.
///
/// **Served undecimated.** `decimate` *averages* UVs across each vertex cluster, which is fine for
/// silhouettes and untextured embeddings but wrong for a textured viewer: these models paint from
/// small palette atlases, so an averaged UV lands *between* two cells and the model comes out
/// speckled. Only a mesh over [`MESH_DECIMATE_ABOVE`] is decimated, and its textures may band.
///
/// The slot trailer tells the viewer which mesh wears which slot; the textures come from
/// [`slot_texture_png`], one request per slot.
pub fn model_mesh(path: &Path) -> Result<Vec<u8>, MediaError> {
    let slotted = lodestash_ingest::mesh::load_textured(path, &std::collections::BTreeMap::new());
    let meshes = match slotted
        .map(|v| v.into_iter().map(|(s, m)| (Some(s), m)).collect::<Vec<_>>())
        .or_else(|_| {
            lodestash_ingest::mesh::load_any(path, &std::collections::BTreeMap::new())
                .map(|v| v.into_iter().map(|m| (None, m)).collect::<Vec<_>>())
        }) {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => return Err(MediaError::Empty("mesh has no geometry")),
        Err(e) => {
            tracing::debug!(%e, ?path, "mesh load failed");
            return Err(MediaError::Unsupported("mesh could not be parsed"));
        }
    };
    let slots: Vec<u32> = meshes.iter().filter_map(|(s, _)| *s).collect();
    let slots = (slots.len() == meshes.len()).then_some(slots);
    let meshes: Vec<lodestash_ingest::mesh::MeshData> =
        meshes.into_iter().map(|(_, m)| m).collect();
    let tris: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
    let served = if tris > MESH_DECIMATE_ABOVE {
        tracing::debug!(
            tris,
            "mesh over the wire cap; decimating (textures may band)"
        );
        std::borrow::Cow::Owned(lodestash_ingest::decimate::decimate_meshes(
            &meshes,
            lodestash_ingest::decimate::VIEWER_BUDGET,
        ))
    } else {
        std::borrow::Cow::Borrowed(&meshes)
    };
    let mut bytes = lodestash_ingest::mesh::encode_meshes(&served);
    if let Some(slots) = slots {
        bytes.extend_from_slice(&lodestash_ingest::mesh::encode_slots(&slots));
    }
    Ok(bytes)
}

/// The texture one material slot of a model is painted with, as a PNG (v1 plan S08).
///
/// The same resolution the cold thumbnail renders from (`mesh::load_textured_with`): the model's
/// own declared or embedded texture where it has one, the catalog's resolved albedo (`slots`) where
/// it does not — so the warm viewer cannot paint a model differently from its own thumbnail.
/// Cached beside the thumbnails on the model's bytes, the slot, and what is painted on it.
pub fn slot_texture_png(
    path: &Path,
    slot: u32,
    slots: &std::collections::BTreeMap<u32, PathBuf>,
    cache: Option<&Path>,
) -> Result<Vec<u8>, MediaError> {
    let cached = cache.map(|c| {
        c.join("slottex").join(format!(
            "{}-{slot}.png",
            lodestash_ingest::thumb::textured_model_key(
                &lodestash_ingest::thumb::meta_key(
                    path,
                    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                    0,
                ),
                slots,
            )
        ))
    });
    if let Some(hit) = cached.as_ref().filter(|p| p.exists()) {
        return std::fs::read(hit).map_err(|_| MediaError::Io("slottex: read"));
    }
    let meshes =
        lodestash_ingest::mesh::load_textured_with(path, &std::collections::BTreeMap::new(), slots)
            .map_err(|e| {
                tracing::debug!(%e, ?path, "textured load failed");
                MediaError::Unsupported("mesh could not be parsed")
            })?;
    let Some(bytes) = meshes
        .iter()
        .find(|(s, m)| *s == slot && m.albedo.is_some())
        .and_then(|(_, m)| lodestash_ingest::mesh::albedo_png(m.albedo.as_ref()?))
    else {
        return Err(MediaError::Empty("slot is unpainted"));
    };
    if let Some(dest) = cached {
        if let Some(dir) = dest.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&dest, &bytes);
    }
    Ok(bytes)
}

/// Parse a single-range `bytes=start-end` header against a known length.
///
/// Returns an inclusive `(start, end)` clamped to the file and to [`MAX_CHUNK`], or `None` for a
/// syntactically invalid or unsatisfiable range (the caller then serves the whole file).
/// Multi-range requests are not supported — no client we serve issues them.
pub fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let spec = header.strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        // `bytes=-N`: the final N bytes.
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            if n == 0 {
                return None;
            }
            (len.saturating_sub(n), len - 1)
        }
        (s, "") => (s.parse().ok()?, len - 1),
        (s, e) => (s.parse().ok()?, e.parse::<u64>().ok()?.min(len - 1)),
    };
    if start > end || start >= len {
        return None;
    }
    Some((start, end.min(start + MAX_CHUNK - 1)))
}

/// The `Content-Type` an asset file is served with.
pub fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("flac") => "audio/flac",
        Some("m4a" | "aac") => "audio/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_ended_range_runs_to_the_end_of_a_small_file() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-", 100), Some((10, 99)));
    }

    #[test]
    fn suffix_range_takes_the_last_n_bytes() {
        assert_eq!(parse_range("bytes=-20", 100), Some((80, 99)));
        // Longer than the file: clamp to the whole file rather than underflow.
        assert_eq!(parse_range("bytes=-500", 100), Some((0, 99)));
    }

    #[test]
    fn explicit_range_is_clamped_to_the_file() {
        assert_eq!(parse_range("bytes=0-9", 100), Some((0, 9)));
        assert_eq!(parse_range("bytes=90-999", 100), Some((90, 99)));
    }

    #[test]
    fn a_response_is_never_larger_than_the_chunk_cap() {
        // The gate case: `bytes=0-` on a file far bigger than memory must not ask for the file.
        let huge = 20 * 1024 * 1024 * 1024u64;
        let (start, end) = parse_range("bytes=0-", huge).expect("satisfiable");
        assert_eq!(start, 0);
        assert_eq!(end - start + 1, MAX_CHUNK, "capped at one chunk");
    }

    #[test]
    fn unsatisfiable_and_malformed_ranges_are_rejected() {
        assert_eq!(parse_range("bytes=100-", 100), None, "start at EOF");
        assert_eq!(parse_range("bytes=50-10", 100), None, "inverted");
        assert_eq!(parse_range("chunks=0-1", 100), None, "wrong unit");
        assert_eq!(parse_range("bytes=abc-", 100), None, "not a number");
        assert_eq!(parse_range("bytes=0-1,5-6", 100), None, "multi-range");
        assert_eq!(parse_range("bytes=0-", 0), None, "empty file");
    }

    #[test]
    fn preview_fit_centers_and_scales_the_content_to_fill() {
        // A small silhouette at the top-left of the 256 canvas — the "model looks tiny" case.
        let svg = concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">"#,
            r#"<g id="v0"><path d="M30.0 20.0 L70.0 20.0 L70.0 60.0 L30.0 60.0 Z"/></g>"#,
            "</svg>",
        );
        let out = fit_preview_to_content(svg);
        // Content box 30..70 in x and y → side 40, pad 1.6, centre (50, 40).
        assert!(
            out.contains(r#"viewBox="28.4 18.4 43.2 43.2""#),
            "unexpected viewBox: {out}"
        );
        // Only the viewBox changed; the document and path are untouched.
        assert!(out.contains(r#"d="M30.0 20.0 L70.0 20.0 L70.0 60.0 L30.0 60.0 Z""#));
        assert!(out.starts_with(r#"<svg xmlns="http://www.w3.org/2000/svg" "#));
    }

    #[test]
    fn preview_fit_leaves_degenerate_or_unparseable_svg_alone() {
        assert_eq!(
            fit_preview_to_content(r#"<svg viewBox="0 0 256 256"></svg>"#),
            r#"<svg viewBox="0 0 256 256"></svg>"#
        );
        let curve = r#"<svg viewBox="0 0 256 256"><path d="M10 10 C20 20 30 20 40 10 Z"/></svg>"#;
        assert_eq!(fit_preview_to_content(curve), curve);
    }

    /// A thumbnail for an asset whose file is gone and that is not in a pack is "missing", not
    /// "not local" — the two need different answers (404 vs 503).
    #[test]
    fn a_missing_loose_file_is_not_found_and_an_unpullable_pack_entry_is_not_local() {
        let dir = tempfile::tempdir().expect("tmp");
        let job = |in_pack| ThumbJob {
            path: Some(dir.path().join("gone.png")),
            media_type: "image".into(),
            albedo: None,
            slot_textures: Default::default(),
            in_pack,
        };
        assert_eq!(
            render_thumb(&job(None), dir.path(), PreviewQuality::default()),
            Err(MediaError::NotFound("file missing"))
        );
        let locator = crate::read_model::InPackLocator {
            pack_path: dir
                .path()
                .join("no-such.unitypackage")
                .to_string_lossy()
                .into(),
            offset: 0,
            size: 4,
            ext: "png".into(),
        };
        assert_eq!(
            render_thumb(&job(Some(locator)), dir.path(), PreviewQuality::default()),
            Err(MediaError::NotLocal("not_local"))
        );
    }
}
