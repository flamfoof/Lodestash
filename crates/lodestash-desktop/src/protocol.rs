//! The `ls://` binary protocol.
//!
//! Everything that is bytes rather than JSON comes through here: preview SVGs, thumbnails, and
//! original asset files. Commands stay for operations; a custom scheme is what lets `<img>`,
//! `<audio>` and `fetch` address asset bytes directly, with the webview's own cache in front.
//!
//! Two properties are structural rather than conventional:
//!
//! - **Read-only by construction.** No handler extracts, pulls or writes. An asset whose bytes
//!   live inside an unextracted `.unitypackage` returns `503 not_local` and the pull is a
//!   command, so the non-destructive invariant cannot be violated from a URL.
//! - **Range-capable.** `ls://file` honours `Range`, returning `206` with only the requested
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

pub fn handle(lib: &Mutex<Library>, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let uri = req.uri();
    // Tauri normalises every form of the scheme to `ls://localhost/<kind>/<rest>` before the
    // handler sees it — including the `http://ls.localhost/...` shape WebView2 requires. So the
    // host carries no routing information and the path carries all of it.
    let (kind, rest) = split_route(uri.path());

    let res = match kind {
        "preview" => preview(lib, rest),
        "file" => file(lib, rest, req),
        _ => status(StatusCode::NOT_FOUND, "unknown ls:// kind"),
    };
    tracing::debug!(path = %uri.path(), status = res.status().as_u16(), bytes = res.body().len(), "ls://");
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

/// `ls://preview/{asset_id}.svg` — the stored traced-contour preview (M15).
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

/// `ls://file/{asset_id}` — the original bytes, with Range support.
fn file(lib: &Mutex<Library>, rest: &str, req: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    let Some(id) = rest.split('/').next().and_then(|s| s.parse::<i64>().ok()) else {
        return status(StatusCode::BAD_REQUEST, "file: bad asset id");
    };
    let path = match lib.lock() {
        Ok(l) => l.asset_path(id).map(Path::to_path_buf),
        Err(_) => None,
    };
    let Some(path) = path else {
        return status(StatusCode::NOT_FOUND, "unknown asset");
    };
    if !path.exists() {
        // An in-pack asset's bytes are inside the archive; extracting is a command, never a GET.
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

/// Start a response with the headers every `ls://` reply carries.
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
    /// but Tauri rewrites every request to `ls://localhost/...`, so "localhost" was taken as the
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
