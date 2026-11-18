//! The `lodestash://` binary protocol — the old UI's byte routes, until P5 replaces them with
//! `/v1/assets/{id}/…`.
//!
//! Everything that is bytes rather than JSON comes through here: preview SVGs, thumbnails, and
//! original asset files. Commands stay for operations; a custom scheme is what lets `<img>`,
//! `<audio>` and `fetch` address asset bytes directly, with the webview's own cache in front.
//!
//! This module is routing and HTTP only: what to read comes from the engine's read model, and the
//! bytes from [`lodestash_engine::media`] (v2 plan Task 2.3).
//!
//! - **Non-destructive by construction.** No handler ever writes to an asset, a pack, or anything
//!   outside the engine's own cache. `file` and `mesh` serve whole assets and never extract; `thumb`
//!   may pull a single payload into the thumbnail cache, under a size cap.
//! - **Range-capable.** `lodestash://file` honours `Range`, returning `206` with only the requested
//!   window. A multi-GB audio file is never read into memory to be scrubbed.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use tauri::http::response::Builder;
use tauri::http::{Request, Response, StatusCode};

use lodestash_engine::media::{self, MediaError, MAX_CHUNK};
use lodestash_engine::Engine;

pub fn handle(engine: &Engine, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let uri = req.uri();
    // Tauri normalises every form of the scheme to `lodestash://localhost/<kind>/<rest>` before the
    // handler sees it — including the `http://lodestash.localhost/...` shape WebView2 requires. So the
    // host carries no routing information and the path carries all of it.
    let (kind, rest) = split_route(uri.path());

    let res = match kind {
        "preview" => preview(engine, rest),
        "mesh" => mesh(engine, rest),
        "slottex" => slot_texture(engine, rest),
        "thumb" => thumb(engine, rest),
        "file" => file(engine, rest, req),
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

/// The library cannot be read right now — the engine is not Ready, or its snapshot could not be
/// rebuilt. `503` with the reason: answering `404 unknown asset` here would tell the client an
/// asset does not exist when it is the *library* that is unavailable.
fn unreadable(e: &lodestash_engine::ReadError) -> Response<Vec<u8>> {
    status(StatusCode::SERVICE_UNAVAILABLE, &e.to_string())
}

/// The status each media failure answers with.
fn media_status(e: &MediaError) -> Response<Vec<u8>> {
    let code = match e {
        MediaError::NotFound(_) => StatusCode::NOT_FOUND,
        MediaError::NotLocal(_) => StatusCode::SERVICE_UNAVAILABLE,
        MediaError::Unsupported(_) => StatusCode::UNSUPPORTED_MEDIA_TYPE,
        MediaError::Empty(_) => StatusCode::NO_CONTENT,
        MediaError::Io(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    status(code, e.message())
}

/// `lodestash://preview/{asset_id}.svg` — the stored traced-contour preview (M15).
fn preview(engine: &Engine, rest: &str) -> Response<Vec<u8>> {
    let Some(id) = rest.trim_end_matches(".svg").parse::<i64>().ok() else {
        return status(StatusCode::BAD_REQUEST, "preview: bad asset id");
    };
    let svg = match engine.read() {
        Ok(lib) => lib.preview_svg(id),
        Err(e) => return unreadable(&e),
    };
    match svg {
        Some(svg) => reply(StatusCode::OK)
            .header("Content-Type", "image/svg+xml")
            // The body changes when a preview is first generated or re-fitted, and the URL is
            // a bare asset id — a long max-age would serve the pre-fix/pre-generation body for
            // an hour. 60s is short enough to pick changes up quickly and still skips the
            // re-parse on every navigation.
            .header("Cache-Control", "max-age=60")
            .body(media::fit_preview_to_content(&svg).into_bytes())
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "preview: build")),
        None => status(StatusCode::NOT_FOUND, "no preview for that asset"),
    }
}

/// `lodestash://thumb/{asset_id}.png` — a raster thumbnail, generated on first request and cached.
///
/// Models are refused here: they are served by the vector-preview pipeline, because a raster model
/// thumbnail costs a full mesh parse and a grid page would trigger a hundred of them. Refusing at
/// the route means a hand-written URL cannot cause the stampede either.
fn thumb(engine: &Engine, rest: &str) -> Response<Vec<u8>> {
    let Some(id) = rest
        .split('/')
        .next()
        .and_then(|s| s.trim_end_matches(".png").parse::<i64>().ok())
    else {
        return status(StatusCode::BAD_REQUEST, "thumb: bad asset id");
    };
    let (job, cache_dir) = {
        let lib = match engine.read() {
            Ok(lib) => lib,
            Err(e) => return unreadable(&e),
        };
        (
            lib.thumb_job(id),
            lib.thumb_cache_dir().map(Path::to_path_buf),
        )
    };
    let (Some(job), Some(cache)) = (job, cache_dir) else {
        return status(StatusCode::NOT_FOUND, "unknown asset");
    };
    if job.media_type == "model_3d" {
        return status(
            StatusCode::NOT_FOUND,
            "models use lodestash://preview, not lodestash://thumb",
        );
    }
    // Grid render quality comes from the persisted setting (UI-SPEC §11.3). The client URL carries
    // `?q=` (see api.ts thumbUrl) purely as a cache-buster — a bare id URL with max-age would keep
    // serving the old-size PNG after a Settings change.
    let quality = engine.config_store().read(|c| c.preview_quality);
    match media::render_thumb(&job, &cache, quality) {
        Ok(bytes) => reply(StatusCode::OK)
            .header("Content-Type", "image/png")
            // Short — the client re-keys on a quality change via `?q=`, and the body can
            // legitimately change (fresh decode after a re-crawl).
            .header("Cache-Control", "max-age=60")
            .header("Content-Length", bytes.len().to_string())
            .body(bytes)
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "thumb: build")),
        Err(e) => media_status(&e),
    }
}

/// `lodestash://mesh/{asset_id}` — geometry as `LSM1`, for the WebGL viewer
/// ([`media::model_mesh`]).
fn mesh(engine: &Engine, rest: &str) -> Response<Vec<u8>> {
    let Some(id) = rest
        .split('/')
        .next()
        .and_then(|s| s.trim_end_matches(".msh").parse::<i64>().ok())
    else {
        return status(StatusCode::BAD_REQUEST, "mesh: bad asset id");
    };
    let (path, known, media_type, cache, job) = {
        let lib = match engine.read() {
            Ok(lib) => lib,
            Err(e) => return unreadable(&e),
        };
        (
            lib.asset_path(id).map(Path::to_path_buf),
            lib.knows(id),
            lib.media_type(id),
            lib.thumb_cache_dir().map(Path::to_path_buf),
            lib.thumb_job(id),
        )
    };

    // A material has no geometry of its own — the viewer gets the same shader ball the grid
    // thumbnail renders, painted by the texture `AssetDetail::texture_asset_id` resolves.
    if media_type == Some(lodestash_core::types::MediaType::Material) {
        let bytes = media::material_sphere();
        return reply(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            // The sphere is a constant; only the texture varies, and that is a separate URL.
            .header("Cache-Control", "max-age=3600")
            .header("Content-Length", bytes.len().to_string())
            .body(bytes)
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "sphere: build"));
    }

    let path = match path.filter(|p| p.exists()) {
        Some(p) => p,
        None => {
            // Same contract as `lodestash://file`: a known asset whose bytes are in a pack is
            // not missing — materialize the one payload through the scratch and parse it here
            // (FEATURES §5.4), so an in-pack model's Quick Look renders like a loose one.
            let local = match (job.as_ref(), cache.as_ref()) {
                (Some(j), Some(c)) if j.in_pack.is_some() => media::in_pack_scratch(j, c),
                _ => None,
            };
            match local {
                Some(p) => p,
                None => {
                    return if known {
                        status(StatusCode::SERVICE_UNAVAILABLE, "not_local")
                    } else {
                        status(StatusCode::NOT_FOUND, "unknown asset")
                    }
                }
            }
        }
    };
    match media::model_mesh(&path) {
        Ok(bytes) => reply(StatusCode::OK)
            .header("Content-Type", "application/octet-stream")
            // The cache key is `(path, size, mtime)`, so an edited file yields a new URL.
            .header("Cache-Control", "max-age=3600")
            .header("Content-Length", bytes.len().to_string())
            .body(bytes)
            .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "mesh: build")),
        Err(e) => media_status(&e),
    }
}

/// `lodestash://slottex/{asset_id}/{slot}` — the texture one material slot of a model is painted
/// with, as a PNG ([`media::slot_texture_png`]). `204` when that slot is unpainted.
fn slot_texture(engine: &Engine, rest: &str) -> Response<Vec<u8>> {
    let mut parts = rest.split('/');
    let (Some(id), Some(slot)) = (
        parts.next().and_then(|s| s.parse::<i64>().ok()),
        parts
            .next()
            .and_then(|s| s.trim_end_matches(".png").parse::<u32>().ok()),
    ) else {
        return status(StatusCode::BAD_REQUEST, "slottex: bad asset id or slot");
    };
    let (path, slots, cache) = {
        let lib = match engine.read() {
            Ok(lib) => lib,
            Err(e) => return unreadable(&e),
        };
        (
            lib.asset_path(id).map(Path::to_path_buf),
            lib.slot_textures(id),
            lib.thumb_cache_dir().map(Path::to_path_buf),
        )
    };
    let Some(path) = path.filter(|p| p.exists()) else {
        return status(StatusCode::SERVICE_UNAVAILABLE, "not_local");
    };
    match media::slot_texture_png(&path, slot, &slots, cache.as_deref()) {
        Ok(bytes) => png(bytes),
        Err(e) => media_status(&e),
    }
}

fn png(bytes: Vec<u8>) -> Response<Vec<u8>> {
    reply(StatusCode::OK)
        .header("Content-Type", "image/png")
        .header("Cache-Control", "max-age=3600")
        .header("Content-Length", bytes.len().to_string())
        .body(bytes)
        .unwrap_or_else(|_| status(StatusCode::INTERNAL_SERVER_ERROR, "png: build"))
}

/// `lodestash://file/{asset_id}` — the original bytes, with Range support.
fn file(engine: &Engine, rest: &str, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(id) = rest.split('/').next().and_then(|s| s.parse::<i64>().ok()) else {
        return status(StatusCode::BAD_REQUEST, "file: bad asset id");
    };
    let (path, known, cache_dir, job) = {
        let lib = match engine.read() {
            Ok(lib) => lib,
            Err(e) => return unreadable(&e),
        };
        (
            lib.asset_path(id).map(Path::to_path_buf),
            lib.knows(id),
            lib.thumb_cache_dir().map(Path::to_path_buf),
            lib.thumb_job(id),
        )
    };
    // An in-pack asset has no local path but is very much a real asset. Answering 404 there
    // would tell the client it does not exist while it is on screen in the grid. Materialize
    // the single payload into the scratch instead (FEATURES §5.4). Extraction of whole packs
    // stays a command, never a GET.
    let local = match path {
        Some(p) if p.exists() => Some(p),
        _ => match (job.as_ref(), cache_dir.as_ref()) {
            (Some(job), Some(cache)) if job.in_pack.is_some() => media::in_pack_scratch(job, cache),
            _ => None,
        },
    };
    let Some(path) = local else {
        return if known {
            status(StatusCode::SERVICE_UNAVAILABLE, "not_local")
        } else {
            status(StatusCode::NOT_FOUND, "unknown asset")
        };
    };

    let Ok(mut f) = File::open(&path) else {
        return status(StatusCode::NOT_FOUND, "open failed");
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return status(StatusCode::INTERNAL_SERVER_ERROR, "stat failed");
    };
    let mime = media::mime_for(&path);

    let range = req
        .headers()
        .get("range")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| media::parse_range(v, len));

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
}
