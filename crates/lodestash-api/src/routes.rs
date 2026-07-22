//! The `/v1/` read routes and the auth guard (ENGINE-API §1, §2, §4).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use lodestash_core::catalog::AssetSummaryRow;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ServerState;

type Shared = Arc<ServerState>;

/// The `/v1/` surface. Every route sits behind [`guard`] — there is no unauthenticated path,
/// not even `server-info` (a probe shouldn't learn the schema version either).
pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/v1/server-info", get(server_info))
        .route("/v1/search", get(search))
        .route("/v1/assets/{id}", get(asset_detail))
        .route("/v1/assets/{id}/preview", get(preview))
        .route("/v1/assets/{id}/file", get(file))
        .layer(from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

// ---------------------------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------------------------

/// Token + loopback check on every request (ENGINE-API §1.4). Origin is checked *before* the
/// token: a cross-origin browser request must be refused on its origin alone, whether or not it
/// somehow guessed a token.
async fn guard(State(state): State<Shared>, req: Request, next: Next) -> Response {
    if !loopback_ok(req.headers()) {
        return err(
            StatusCode::FORBIDDEN,
            "forbidden_origin",
            "Requests must originate from loopback.",
            "",
        );
    }
    let ok = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| t == state.token);
    if !ok {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Missing or invalid bearer token.",
            "Read the token from engine.json.",
        );
    }
    next.run(req).await
}

/// `Host` must be loopback, and `Origin` — sent by browsers, absent from plugin calls — must be
/// loopback when present. That pair is what blocks DNS-rebinding: the attacker controls the
/// origin and the resolved name, never both as 127.0.0.1.
fn loopback_ok(headers: &HeaderMap) -> bool {
    let host_ok = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        // ponytail: map_or, not is_none_or — the workspace MSRV is 1.80.
        .map_or(true, is_loopback_authority);
    let origin_ok = headers
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map_or(true, |o| {
            o.strip_prefix("http://").is_some_and(is_loopback_authority)
        });
    host_ok && origin_ok
}

fn is_loopback_authority(authority: &str) -> bool {
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    host == "127.0.0.1" || host == "localhost" || host == "[::1]" || host == "::1"
}

// ---------------------------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------------------------

async fn server_info(State(state): State<Shared>) -> Response {
    let (app, schema) = match state.server_info() {
        Ok(v) => v,
        Err(e) => return db_err(e),
    };
    Json(json!({ "app": app, "schema": schema, "api": "v1" })).into_response()
}

#[derive(Debug, Deserialize)]
struct SearchParams {
    #[serde(default)]
    q: String,
    limit: Option<usize>,
    media_type: Option<String>,
    mode: Option<String>,
}

async fn search(State(state): State<Shared>, Query(p): Query<SearchParams>) -> Response {
    if p.mode.as_deref() == Some("ai") {
        return err(
            StatusCode::NOT_IMPLEMENTED,
            "ai_unavailable",
            "Semantic search is not served on this plane yet.",
            "Use the default structural mode.",
        );
    }
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    // ponytail: blocking sqlite on the async thread — these are indexed point queries; move to
    // spawn_blocking if a slow route ever shows up in a trace.
    let rows = match state.search(&p.q, limit, p.media_type.as_deref()) {
        Ok(rows) => rows,
        Err(e) => return db_err(e),
    };
    let out: Vec<Value> = rows.iter().map(summary_json).collect();
    Json(json!({ "results": out })).into_response()
}

async fn asset_detail(State(state): State<Shared>, Path(id): Path<i64>) -> Response {
    let Some(row) = (match state.asset(id) {
        Ok(row) => row,
        Err(e) => return db_err(e),
    }) else {
        return not_found();
    };
    let mut body = summary_json(&row);
    let m = row.metrics;
    body["size_bytes"] = json!(row.size_bytes);
    body["stats"] = json!({
        "tris": m.tri_count, "width_px": m.width_px,
        "height_px": m.height_px, "duration_ms": m.duration_ms,
    });
    Json(body).into_response()
}

async fn preview(State(state): State<Shared>, Path(id): Path<i64>) -> Response {
    match state.preview_path(id) {
        Some(path) => serve_file(&path, "image/png").await,
        None => not_found(),
    }
}

async fn file(State(state): State<Shared>, Path(id): Path<i64>) -> Response {
    let row = match state.asset(id) {
        Ok(Some(row)) => row,
        Ok(None) => return not_found(),
        Err(e) => return db_err(e),
    };
    match state.source_path(&row) {
        Some(path) => serve_file(&path, "application/octet-stream").await,
        None => not_found(),
    }
}

// ---------------------------------------------------------------------------------------------
// Shared bits
// ---------------------------------------------------------------------------------------------

/// ENGINE-API §3 `AssetSummary`. `tags` is present but empty until P4 ships tagging — the shape
/// stays stable for clients rather than growing a field later.
pub fn summary_json(r: &AssetSummaryRow) -> Value {
    json!({
        "id": r.id,
        "uuid": r.uuid,
        "name": r.name,
        "media_type": r.media_type,
        "format": r.format,
        "status": r.status,
        "package": r.package_id.map(|id| json!({ "id": id, "name": r.package_name })),
        "tags": [],
        "preview_url": format!("/v1/assets/{}/preview", r.id),
        "file_url": format!("/v1/assets/{}/file", r.id),
    })
}

/// ponytail: reads the whole file into memory. Fine for thumbnails and typical assets; swap for
/// a `ReaderStream` (needs tokio-util) if multi-hundred-MB sources become normal.
async fn serve_file(path: &std::path::Path, content_type: &str) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            [(header::CONTENT_TYPE, content_type.to_string())],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => not_found(),
    }
}

fn not_found() -> Response {
    err(
        StatusCode::NOT_FOUND,
        "not_found",
        "No such asset.",
        "The catalog may have moved on since the id was issued.",
    )
}

fn db_err(e: lodestash_core::error::CoreError) -> Response {
    tracing::warn!(%e, "api query failed");
    err(
        StatusCode::INTERNAL_SERVER_ERROR,
        "job_failed",
        "The catalog query failed.",
        "",
    )
}

/// ENGINE-API §4 error envelope.
fn err(status: StatusCode, code: &str, message: &str, detail: &str) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message, "detail": detail } })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;
    use tower::ServiceExt;

    fn state() -> (tempfile::TempDir, Shared) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        {
            let cat = lodestash_core::catalog::Catalog::open(&db).unwrap();
            cat.seed_media_types().unwrap();
        }
        let s = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap();
        (dir, Arc::new(s))
    }

    async fn send(app: Router, req: HttpRequest<Body>) -> (StatusCode, Value) {
        let res = app.oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn get_req(uri: &str, token: Option<&str>, origin: Option<&str>) -> HttpRequest<Body> {
        let mut b = HttpRequest::get(uri).header(header::HOST, "127.0.0.1:51731");
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        b.body(Body::empty()).unwrap()
    }

    /// The guard's whole job, one case per rejection reason plus the accept.
    #[tokio::test]
    async fn guard_accepts_only_a_loopback_request_with_the_session_token() {
        let (_d, st) = state();
        let token = st.token.clone();
        let app = || router(st.clone());

        let (s, _) = send(app(), get_req("/v1/server-info", Some(&token), None)).await;
        assert_eq!(s, StatusCode::OK, "loopback + right token");

        let (s, b) = send(app(), get_req("/v1/server-info", None, None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "no token");
        assert_eq!(b["error"]["code"], "unauthorized");

        let (s, _) = send(app(), get_req("/v1/server-info", Some("wrong"), None)).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "wrong token");

        let (s, b) = send(
            app(),
            get_req("/v1/server-info", Some(&token), Some("http://evil.test")),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "off-origin, even with the token");
        assert_eq!(b["error"]["code"], "forbidden_origin");

        let (s, _) = send(
            app(),
            get_req(
                "/v1/server-info",
                Some(&token),
                Some("http://127.0.0.1:51731"),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "loopback origin is fine");
    }

    #[tokio::test]
    async fn unknown_asset_is_a_not_found_envelope() {
        let (_d, st) = state();
        let token = st.token.clone();
        let (s, b) = send(router(st), get_req("/v1/assets/999", Some(&token), None)).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(b["error"]["code"], "not_found");
    }

    #[tokio::test]
    async fn empty_search_falls_back_to_recent_and_ai_mode_is_refused() {
        let (_d, st) = state();
        let token = st.token.clone();

        let (s, b) = send(
            router(st.clone()),
            get_req("/v1/search", Some(&token), None),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(b["results"].as_array().unwrap().len(), 0, "empty catalog");

        let (s, b) = send(
            router(st),
            get_req("/v1/search?mode=ai&q=dog", Some(&token), None),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_IMPLEMENTED);
        assert_eq!(b["error"]["code"], "ai_unavailable");
    }
}
