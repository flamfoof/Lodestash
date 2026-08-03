//! The `lodestash://` binary protocol.
//!
//! Everything that is bytes rather than JSON comes through here: preview SVGs, thumbnails, and
//! original asset files. Commands stay for operations; a custom scheme is what lets `<img>`,
//! `<audio>` and `fetch` address asset bytes directly, with the webview's own cache in front.
//!
//! Two properties are structural rather than conventional:
//!
//! - **Non-destructive by construction.** No handler ever writes to an asset, a pack, or anything
//!   outside our own cache. `file` and `mesh` serve whole assets and never extract: bytes inside an
//!   unextracted `.unitypackage` return `503 not_local`, and pulling them is a command. `thumb` is
//!   the one deliberate exception — it may pull a single payload into the thumbnail cache, under a
//!   size cap, because a grid of 5,963 typed placeholders is not a library (see [`in_pack_scratch`]).
//! - **Range-capable.** `lodestash://file` honours `Range`, returning `206` with only the requested
//!   window. A multi-GB audio file is never read into memory to be scrubbed.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

use tauri::http::response::Builder;
use tauri::http::{Request, Response, StatusCode};

use crate::library::Library;

/// Cap on a single Range response. WebView2 asks for what it needs; this bounds one response so a
/// client asking for `bytes=0-` on a 20 GB pack cannot make us allocate 20 GB.
const MAX_CHUNK: u64 = 8 * 1024 * 1024;

/// Triangle count above which `lodestash://mesh` decimates rather than shipping the source.
///
/// Everything under this travels whole so textures stay correct (see [`mesh`]). At ~50 bytes per
/// triangle of `LSM1` this caps a response near 15 MB, which a local protocol and a GPU buffer both
/// shrug at; beyond it, a banded texture beats a stalled viewer.
const MESH_DECIMATE_ABOVE: usize = 300_000;

pub fn handle(lib: &Mutex<Library>, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let uri = req.uri();
    // Tauri normalises every form of the scheme to `lodestash://localhost/<kind>/<rest>` before the
    // handler sees it — including the `http://lodestash.localhost/...` shape WebView2 requires. So the
    // host carries no routing information and the path carries all of it.
    let (kind, rest) = split_route(uri.path());

    let res = match kind {
        "preview" => preview(lib, rest),
        "mesh" => mesh(lib, rest),
        "thumb" => thumb(lib, rest),
        "file" => file(lib, rest, req),
        _ => status(StatusCode::NOT_FOUND, "unknown lodestash:// kind"),
    };
    tracing::debug!(path = %uri.path(), status = res.status().as_u16(), bytes = res.body().len(), "lodestash://");
    res
}

/// Split `/kind/rest` into its two parts. Leading slashes and an absent tail both yield empty
/// strings rather than panicking — a malformed URL is a 404, not a crash.
fn split_route(path: &str) -> (&str, &str) {
    let trimmed = path.trim_start_matches('/');
    match trimmed.split_once('/') {
        Some((kind, rest)) => (kind, rest),
        None => (trimmed, ""),
    }
}

/// `lodestash://preview/{asset_id}.svg` — the stored traced-contour preview (M15).
fn preview(lib: &Mutex<Library>, rest: &str) -> Response<Vec<u8>> {
    let Some(id) = rest.trim_end_matches(".svg").parse::<i64>().ok() else {
        return status(StatusCode::BAD_REQUEST, "preview: bad asset id");
    };
    let svg = lib.lock().ok().and_then(|l| l.preview_svg(id));
    match svg {
        Some(svg) => reply(StatusCode::OK)
            .header("Content-Type", "image/svg+xml")
            // The preview is regenerated under a new source_key when the file changes, and the
            // frontend re-reads the id list on reload, so a cached body is never stale in a way
            // the client cannot see.
            .header("Cache-Control", "max-age=3600")
            .body(svg.into_bytes())
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "preview: build")),
        None => status(StatusCode::NOT_FOUND, "no preview for that asset"),
    }
}

/// `lodestash://thumb/{asset_id}.png` — a raster thumbnail, generated on first request and cached.
///
/// This is what stops the grid being a wall of type icons: only models with a stored vector
/// preview had a picture, which was 208 of 7,691 assets. Every branch here already existed in
/// `lodestash_ingest::thumb` — images and models through `generate`, audio through
/// `generate_waveform`, materials through `generate_material` — none of it was reachable from the
/// frontend.
///
/// Generation is synchronous but writes through the existing on-disk cache, so it happens once per
/// asset and the webview's own cache absorbs the rest.
fn thumb(lib: &Mutex<Library>, rest: &str) -> Response<Vec<u8>> {
    let Some(id) = rest
        .split('/')
        .next()
        .and_then(|s| s.trim_end_matches(".png").parse::<i64>().ok())
    else {
        return status(StatusCode::BAD_REQUEST, "thumb: bad asset id");
    };
    let (job, cache_dir) = match lib.lock() {
        Ok(l) => (l.thumb_job(id), l.thumb_cache_dir().map(Path::to_path_buf)),
        Err(_) => (None, None),
    };
    let (Some(job), Some(cache)) = (job, cache_dir) else {
        return status(StatusCode::NOT_FOUND, "unknown asset");
    };
    // Models are served by the vector-preview pipeline, not here: a raster model thumbnail costs a
    // full mesh parse, and a grid page would trigger a hundred of them. Refusing at the route
    // means a hand-written URL cannot cause the stampede either. Checked before the in-pack pull
    // below so a model never pays for a gzip scan we are about to refuse anyway.
    if job.media_type == "model_3d" {
        return status(
            StatusCode::NOT_FOUND,
            "models use lodestash://preview, not lodestash://thumb",
        );
    }

    let path = match job.path.clone().filter(|p| p.exists()) {
        Some(p) => p,
        None => match in_pack_scratch(&job, &cache) {
            Some(p) => p,
            // A known in-pack asset over the size cap, or one whose pull failed. Not "missing".
            None if job.in_pack.is_some() => {
                return status(StatusCode::SERVICE_UNAVAILABLE, "not_local")
            }
            None => return status(StatusCode::NOT_FOUND, "file missing"),
        },
    };

    let params = lodestash_ingest::thumb::ThumbParams::default();
    let key = lodestash_ingest::thumb::meta_key(
        &path,
        std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        0,
    );
    let made = match job.media_type.as_str() {
        "audio" => lodestash_ingest::thumb::generate_waveform(&cache, &key, params, |bars| {
            lodestash_app::audio::load_waveform(&path, bars).map(|m| m.peaks)
        }),
        "material" => {
            // The texture is part of the picture, so it is part of the key: swap a material's
            // albedo and the previously cached ball must not be served.
            let key = match &job.albedo {
                Some(a) => format!("{key}-{}", lodestash_ingest::thumb::meta_key(a, 0, 0)),
                None => format!("{key}-flat"),
            };
            lodestash_ingest::thumb::generate_material(&cache, &key, params, job.albedo.as_deref())
        }
        _ => lodestash_ingest::thumb::generate(&path, &cache, &key, params),
    };
    match made.and_then(std::fs::read) {
        Ok(bytes) => reply(StatusCode::OK)
            .header("Content-Type", "image/png")
            .header("Cache-Control", "max-age=3600")
            .header("Content-Length", bytes.len().to_string())
            .body(bytes)
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "thumb: build")),
        Err(e) => {
            tracing::debug!(%e, ?path, "thumbnail generation failed");
            status(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "no thumbnail for this file",
            )
        }
    }
}

/// Materialize an in-pack payload into the thumbnail cache so the generator can decode it.
///
/// **This is the one place a GET writes anything**, and the exception is narrow by construction:
/// it writes only into our own cache directory, and only the bytes of the one asset being drawn.
/// The `.unitypackage` and the asset's own bytes are untouched, so the non-destructive invariant
/// holds; what it costs is CPU, not correctness. `lodestash://file` and `lodestash://mesh` still
/// never pull — those serve whole assets, where the honest answer to "not local" is to extract
/// the pack.
///
/// **Gated on archive size.** `.unitypackage` is gzip, which has no random access: pulling one
/// payload inflates the archive from byte zero to that payload's offset. On the real 21.9 GB
/// music pack a single page of cards re-inflated gigabytes each and pegged the CPU. Above
/// [`IN_PACK_THUMB_MAX_BYTES`] the card keeps its type placeholder until the pack is extracted,
/// which is the fast random-access path anyway.
///
/// ponytail: O(page) independent pulls under the cap; batching a page into one sorted
/// `extract_at_offsets` skip-scan is what lifts it.
fn in_pack_scratch(job: &crate::library::ThumbJob, cache: &Path) -> Option<std::path::PathBuf> {
    let loc = job.in_pack.as_ref()?;
    let archive = std::fs::metadata(&loc.pack_path).ok()?.len();
    if archive > lodestash_app::thumbs::IN_PACK_THUMB_MAX_BYTES {
        tracing::debug!(pack = %loc.pack_path, archive, "pack over the thumbnail pull cap");
        return None;
    }
    lodestash_app::thumbs::ensure_in_pack_scratch(
        Some(cache),
        &loc.pack_path,
        loc.offset,
        loc.size,
        &loc.ext,
    )
    .map(|(p, _)| p)
}

/// `lodestash://mesh/{asset_id}` — decimated geometry as `LSM1`, for the WebGL viewer.
///
/// Deliberately the **same encoder as the on-disk mesh cache** (`mesh::encode_meshes`): the wire
/// format and the cache format are one thing, so the TypeScript parser and `decode_meshes` cannot
/// drift apart. Drift would surface as garbled geometry, not an error.
///
/// **Served undecimated.** `decimate` *averages* UVs across each vertex cluster
/// (`decimate.rs:195`), which is fine for its real consumers — silhouette previews and untextured
/// multiview embeddings — but wrong for a textured viewer: these models paint from small palette
/// atlases of flat colour blocks, so an averaged UV lands *between* two cells and the model comes
/// out speckled with colours that are in neither. Verified by eye; the artifact is obvious.
///
/// Full geometry is cheap here anyway: a typical model is ~1-2 MB of `LSM1` over a local protocol,
/// straight into a GPU buffer. Only a pathologically large mesh is decimated, and its textures may
/// band for the reason above.
fn mesh(lib: &Mutex<Library>, rest: &str) -> Response<Vec<u8>> {
    let Some(id) = rest
        .split('/')
        .next()
        .and_then(|s| s.trim_end_matches(".msh").parse::<i64>().ok())
    else {
        return status(StatusCode::BAD_REQUEST, "mesh: bad asset id");
    };
    let (path, known) = match lib.lock() {
        Ok(l) => (l.asset_path(id).map(Path::to_path_buf), l.knows(id)),
        Err(_) => (None, false),
    };
    let Some(path) = path.filter(|p| p.exists()) else {
        // Same contract as `lodestash://file`: a known asset whose bytes are in a pack is not missing.
        return if known {
            status(StatusCode::SERVICE_UNAVAILABLE, "not_local")
        } else {
            status(StatusCode::NOT_FOUND, "unknown asset")
        };
    };

    let meshes = match lodestash_ingest::mesh::load_any(&path, &std::collections::BTreeMap::new()) {
        Ok(m) if !m.is_empty() => m,
        Ok(_) => return status(StatusCode::NO_CONTENT, "mesh has no geometry"),
        Err(e) => {
            tracing::debug!(%e, ?path, "mesh load failed");
            return status(
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "mesh could not be parsed",
            );
        }
    };
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
    let bytes = lodestash_ingest::mesh::encode_meshes(&served);
    reply(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        // The cache key is `(path, size, mtime)`, so an edited file yields a new URL.
        .header("Cache-Control", "max-age=3600")
        .header("Content-Length", bytes.len().to_string())
        .body(bytes)
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "mesh: build"))
}

/// `lodestash://file/{asset_id}` — the original bytes, with Range support.
fn file(lib: &Mutex<Library>, rest: &str, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(id) = rest.split('/').next().and_then(|s| s.parse::<i64>().ok()) else {
        return status(StatusCode::BAD_REQUEST, "file: bad asset id");
    };
    let (path, known) = match lib.lock() {
        Ok(l) => (l.asset_path(id).map(Path::to_path_buf), l.knows(id)),
        Err(_) => (None, false),
    };
    // An in-pack asset has no local path but is very much a real asset. Answering 404 there
    // would tell the client it does not exist while it is on screen in the grid.
    let Some(path) = path else {
        return if known {
            status(StatusCode::SERVICE_UNAVAILABLE, "not_local")
        } else {
            status(StatusCode::NOT_FOUND, "unknown asset")
        };
    };
    if !path.exists() {
        // Extracting is a command, never a GET — the handler stays read-only by construction.
        return status(StatusCode::SERVICE_UNAVAILABLE, "not_local");
    }

    let Ok(mut f) = File::open(&path) else {
        return status(StatusCode::NOT_FOUND, "open failed");
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return status(StatusCode::INTERNAL_SERVER_ERROR, "stat failed");
    };
    let mime = mime_for(&path);

    let range = req
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_range(v, len));

    match range {
        Some((start, end)) => {
            let count = end - start + 1;
            let mut buf = vec![0u8; count as usize];
            if f.seek(SeekFrom::Start(start)).is_err() || f.read_exact(&mut buf).is_err() {
                return status(StatusCode::INTERNAL_SERVER_ERROR, "read failed");
            }
            reply(StatusCode::PARTIAL_CONTENT)
                .header("Content-Type", mime)
                .header("Accept-Ranges", "bytes")
                .header("Content-Range", format!("bytes {start}-{end}/{len}"))
                .header("Content-Length", count.to_string())
                .body(buf)
                .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "range: build"))
        }
        None => {
            // No Range header. Advertise support and, for anything large, hand back the first
            // chunk as a 206 so the client asks for the rest instead of us buffering the file.
            if len > MAX_CHUNK {
                let end = MAX_CHUNK - 1;
                let mut buf = vec![0u8; MAX_CHUNK as usize];
                if f.read_exact(&mut buf).is_err() {
                    return status(StatusCode::INTERNAL_SERVER_ERROR, "read failed");
                }
                return reply(StatusCode::PARTIAL_CONTENT)
                    .header("Content-Type", mime)
                    .header("Accept-Ranges", "bytes")
                    .header("Content-Range", format!("bytes 0-{end}/{len}"))
                    .header("Content-Length", MAX_CHUNK.to_string())
                    .body(buf)
                    .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "chunk: build"));
            }
            let mut buf = Vec::with_capacity(len as usize);
            if f.read_to_end(&mut buf).is_err() {
                return status(StatusCode::INTERNAL_SERVER_ERROR, "read failed");
            }
            reply(StatusCode::OK)
                .header("Content-Type", mime)
                .header("Accept-Ranges", "bytes")
                .header("Content-Length", len.to_string())
                .body(buf)
                .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "file: build"))
        }
    }
}

/// Parse a single-range `bytes=start-end` header against a known length.
///
/// Returns an inclusive `(start, end)` clamped to the file and to [`MAX_CHUNK`], or `None` for a
/// syntactically invalid or unsatisfiable range (the caller then serves the whole file).
/// Multi-range requests are not supported — no client we serve issues them.
fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
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

fn mime_for(path: &Path) -> &'static str {
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

/// Start a response with the headers every `lodestash://` reply carries.
///
/// `Access-Control-Allow-Origin` is required because the page and the protocol are different
/// origins: `<img src>` is a no-CORS load and works regardless, but `fetch()` — which the mesh
/// loader and the e2e checks use — is blocked without it. The scheme is only routable from
/// inside this app's webview, so a wildcard grants nothing a page here could not already read.
fn reply(code: StatusCode) -> Builder {
    Response::builder()
        .status(code)
        .header("Access-Control-Allow-Origin", "*")
        // A cross-origin response only exposes the CORS-safelisted headers to script.
        // `Content-Range` and `Accept-Ranges` are not on that list, so without this a client
        // doing its own range arithmetic reads them as null while the bytes arrive fine —
        // a failure that looks like a header bug in the server rather than a CORS rule.
        .header(
            "Access-Control-Expose-Headers",
            "Content-Range, Accept-Ranges, Content-Length, Content-Type",
        )
}

fn status(code: StatusCode, msg: &str) -> Response<Vec<u8>> {
    reply(code)
        .header("Content-Type", "application/json")
        .body(format!("{{\"error\":{:?}}}", msg).into_bytes())
        .expect("static response builds")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The routing bug this replaced: the handler tried to read the kind from the URL *host*,
    /// but Tauri rewrites every request to `lodestash://localhost/...`, so "localhost" was taken as the
    /// kind and every request 404'd. It failed as a broken image with nothing in any log.
    #[test]
    fn the_route_comes_from_the_path_whatever_the_host_is() {
        assert_eq!(split_route("/preview/44.svg"), ("preview", "44.svg"));
        assert_eq!(split_route("/file/1728"), ("file", "1728"));
        assert_eq!(split_route("preview/9.svg"), ("preview", "9.svg"));
        assert_eq!(split_route("/file"), ("file", ""));
        assert_eq!(split_route("/"), ("", ""));
        assert_eq!(split_route(""), ("", ""));
    }

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
}
