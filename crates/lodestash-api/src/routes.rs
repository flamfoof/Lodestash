//! The `/v1/` read routes and the auth guard (ENGINE-API §1, §2, §4).

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{MatchedPath, Path, Query, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::{from_fn_with_state, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
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
        .route("/v1/assets/{id}/similar", get(similar))
        .route("/v1/packages", get(packages))
        .route("/v1/projects/register", post(register_project))
        .route("/v1/projects", get(list_projects))
        .route("/v1/import", post(import_to_project))
        .route("/v1/export", post(export))
        .route("/v1/jobs/{id}", get(job_status))
        .route("/v1/jobs/{id}/cancel", post(cancel_job))
        .route("/v1/assets/{id}/file", get(file))
        // Order matters: `guard` is added last so it wraps outermost and runs **first** — an
        // unauthenticated caller is rejected before `gate` could reveal which tools exist.
        .layer(from_fn_with_state(state.clone(), gate))
        .layer(from_fn_with_state(state.clone(), guard))
        .with_state(state)
}

/// The MCP tool name a request maps to, or `None` for a route with no tool (`…/file` is
/// REST-only by design — always available behind auth, never on the per-tool allow-list). `search`
/// splits on `?mode=ai`, mirroring the handler, so `semantic_search` and `search_assets` gate
/// independently.
fn tool_for(method: &Method, path: &str, query: Option<&str>) -> Option<&'static str> {
    Some(match (method.as_str(), path) {
        ("GET", "/v1/server-info") => "get_server_info",
        ("GET", "/v1/search") => {
            if query.unwrap_or("").split('&').any(|kv| kv == "mode=ai") {
                "semantic_search"
            } else {
                "search_assets"
            }
        }
        ("GET", "/v1/assets/{id}") => "get_asset",
        ("GET", "/v1/assets/{id}/preview") => "get_asset_preview",
        ("GET", "/v1/assets/{id}/similar") => "find_similar",
        ("GET", "/v1/packages") => "list_packages",
        ("POST", "/v1/projects/register") => "register_project",
        ("GET", "/v1/projects") => "list_projects",
        ("POST", "/v1/import") => "import_to_project",
        ("POST", "/v1/export") => "export_asset",
        ("GET", "/v1/jobs/{id}") => "get_job_status",
        ("POST", "/v1/jobs/{id}/cancel") => "cancel_job",
        _ => return None,
    })
}

/// Per-tool allow-list check (UI-SPEC §16, card 178). Runs after [`guard`]. A disabled tool is a
/// `403 tool_disabled` — the same operation the MCP plane refuses per tool, so the gate is 1:1.
async fn gate(
    State(state): State<Shared>,
    matched: MatchedPath,
    method: Method,
    req: Request,
    next: Next,
) -> Response {
    if let Some(tool) = tool_for(&method, matched.as_str(), req.uri().query()) {
        if !state.allows(tool) {
            return err(
                StatusCode::FORBIDDEN,
                "tool_disabled",
                "This tool is disabled in Lodestash's MCP settings.",
                "Enable it in Settings ▸ Model Context Protocol.",
            );
        }
    }
    next.run(req).await
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
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    if p.mode.as_deref() == Some("ai") {
        return match state.semantic_search(&p.q, limit, p.media_type.as_deref()) {
            Some(Ok(rows)) => {
                let out: Vec<Value> = rows.iter().map(summary_json).collect();
                Json(json!({ "results": out })).into_response()
            }
            Some(Err(e)) => db_err(e),
            None => err(
                StatusCode::NOT_IMPLEMENTED,
                "ai_unavailable",
                "No embedding model is loaded.",
                "Download the AI models in Settings, then retry.",
            ),
        };
    }
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
    let deps = state.dependencies(id).unwrap_or_default();
    body["dependencies"] = json!({ "count": deps.len(), "asset_ids": deps });
    Json(body).into_response()
}

#[derive(Debug, Deserialize)]
struct LimitParam {
    limit: Option<usize>,
}

async fn similar(
    State(state): State<Shared>,
    Path(id): Path<i64>,
    Query(p): Query<LimitParam>,
) -> Response {
    match state.similar(id, p.limit.unwrap_or(20).clamp(1, 200)) {
        Ok(rows) => {
            let out: Vec<Value> = rows.iter().map(summary_json).collect();
            Json(json!({ "results": out })).into_response()
        }
        Err(e) => db_err(e),
    }
}

async fn packages(State(state): State<Shared>) -> Response {
    match state.packages() {
        Ok(rows) => {
            let out: Vec<Value> = rows.iter().map(package_json).collect();
            Json(json!({ "packages": out })).into_response()
        }
        Err(e) => db_err(e),
    }
}

#[derive(Debug, Deserialize)]
struct RegisterParams {
    project_path: String,
    unity_version: Option<String>,
    pipeline: Option<String>,
    /// Accepted per ENGINE-API §5.1; unused until the import-queue drain (plugin card) lands.
    #[serde(default)]
    #[allow(dead_code)]
    can_execute_main_thread: bool,
}

/// The plugin announces the open editor (ENGINE-API §5.1). Idempotent on `project_path`.
async fn register_project(State(state): State<Shared>, Json(p): Json<RegisterParams>) -> Response {
    if p.project_path.trim().is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "project_path must not be empty.",
            "",
        );
    }
    match state.register_project(
        &p.project_path,
        p.unity_version.as_deref(),
        p.pipeline.as_deref(),
    ) {
        Ok((id, is_active)) => {
            Json(json!({ "project_id": id, "is_active": is_active })).into_response()
        }
        Err(e) => db_err(e),
    }
}

async fn list_projects(State(state): State<Shared>) -> Response {
    match state.list_projects() {
        Ok(rows) => {
            let out: Vec<Value> = rows.iter().map(project_json).collect();
            Json(json!({ "projects": out })).into_response()
        }
        Err(e) => db_err(e),
    }
}

/// *Side-effecting* (ENGINE-API §5.2). Parks a `202` job on the approval gate like `export`, or
/// answers `200 { imported_existing, job_id: null }` when the dup-guard (§5.4) leaves nothing new,
/// or `409 project_not_registered` when no editor is open.
async fn import_to_project(
    State(state): State<Shared>,
    Json(req): Json<crate::ImportRequest>,
) -> Response {
    match state.request_import(req) {
        Ok(crate::ImportKickoff::Awaiting(job)) => {
            (StatusCode::ACCEPTED, Json(job.to_json())).into_response()
        }
        Ok(crate::ImportKickoff::AllAlreadyImported(ids)) => {
            Json(json!({ "imported_existing": ids, "job_id": null })).into_response()
        }
        Err(crate::ImportError::Empty) => err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "asset_ids must not be empty.",
            "",
        ),
        Err(crate::ImportError::NoProject) => err(
            StatusCode::CONFLICT,
            "project_not_registered",
            "No Unity project is registered.",
            "Open the Lodestash panel in Unity, or export to a folder instead.",
        ),
    }
}

/// *Side-effecting* (ENGINE-API §2). Returns `202` with a job that is **not running**: it waits
/// on the desktop app's approval (MCP-AND-ENGINES §5 "confirm mutating ops server-side").
async fn export(
    State(state): State<Shared>,
    Json(req): Json<crate::export::ExportRequest>,
) -> Response {
    if req.asset_ids.is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "asset_ids must not be empty.",
            "",
        );
    }
    match state.request_export(req) {
        Ok(job) => (StatusCode::ACCEPTED, Json(job.to_json())).into_response(),
        Err(e) => err(StatusCode::BAD_REQUEST, e.code(), e.message(), ""),
    }
}

async fn job_status(State(state): State<Shared>, Path(id): Path<String>) -> Response {
    match state.jobs.get(&id) {
        Some(job) => Json(job.to_json()).into_response(),
        None => err(
            StatusCode::NOT_FOUND,
            "not_found",
            "No such job.",
            "Jobs live for the session that created them.",
        ),
    }
}

async fn cancel_job(State(state): State<Shared>, Path(id): Path<String>) -> Response {
    match state.jobs.cancel(&id) {
        Some(job) => Json(job.to_json()).into_response(),
        None => err(StatusCode::NOT_FOUND, "not_found", "No such job.", ""),
    }
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

/// ENGINE-API §3 `Package`. `license`/`attribution` land with the P2 license harvest.
pub fn package_json(p: &lodestash_core::catalog::PackageListing) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "publisher": p.publisher,
        "category": p.category,
        "asset_count": p.asset_count,
        "unpacked_at": p.unpacked_at,
    })
}

/// ENGINE-API §5.1 project row. The `is_active` one is the import target when none is named.
pub fn project_json(p: &lodestash_core::catalog::ProjectRow) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "project_path": p.project_path,
        "unity_version": p.unity_version,
        "pipeline": p.pipeline,
        "is_active": p.is_active,
        "last_seen_at": p.last_seen_at,
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

    /// Seed three embedded assets and return their ids in insertion order.
    fn seed_embedded(db: &std::path::Path) -> Vec<i64> {
        use lodestash_core::catalog::{Catalog, CrawlRecord};
        let mut cat = Catalog::open(db).unwrap();
        cat.seed_media_types().unwrap();
        let src = cat.upsert_import_source("Lib", "C:/lib", "folder").unwrap();
        let rec = |rel: &str| CrawlRecord {
            workspace_rel: None,
            rel_path: rel.into(),
            name: rel.into(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 1,
        };
        cat.record_crawl(src, None, &[rec("a.png"), rec("b.png"), rec("c.png")])
            .unwrap();
        let recent = cat.recent_asset_ids(10).unwrap();
        let rows = cat.asset_summaries(&recent).unwrap();
        let ids: Vec<i64> = ["a.png", "b.png", "c.png"]
            .iter()
            .map(|n| rows.iter().find(|r| r.name == *n).unwrap().id)
            .collect();
        // a is the query; c sits nearer to it than b.
        let vecs: [(i64, [f32; 3]); 3] = [
            (ids[0], [1.0, 0.0, 0.0]),
            (ids[1], [0.0, 1.0, 0.0]),
            (ids[2], [0.9, 0.1, 0.0]),
        ];
        for (id, v) in vecs {
            cat.upsert_embedding(id, "m", "image", None, None, &v)
                .unwrap();
        }
        ids
    }

    /// find_similar ranks by cosine, never returns the query asset, and answers an unembedded
    /// asset with an empty list rather than an error.
    #[tokio::test]
    async fn similar_ranks_neighbours_and_excludes_the_query() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let ids = seed_embedded(&db);
        let st =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        let token = st.token.clone();

        let (s, b) = send(
            router(st.clone()),
            get_req(
                &format!("/v1/assets/{}/similar", ids[0]),
                Some(&token),
                None,
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        let got: Vec<i64> = b["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_i64().unwrap())
            .collect();
        assert_eq!(got, vec![ids[2], ids[1]], "nearest first, query excluded");

        // An asset with no embedding: empty, not an error.
        let (_d2, empty) = state();
        let t2 = empty.token.clone();
        let (s, b) = send(
            router(empty),
            get_req("/v1/assets/1/similar", Some(&t2), None),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert!(b["results"].as_array().unwrap().is_empty());
    }

    /// The register → import → dup-guard round-trip a client drives (ENGINE-API §5), headless: no
    /// project ⇒ refused; register activates; import parks on approval and records nothing until
    /// approved; a re-import is caught by the `UNIQUE(asset_id, project_id)` dup-guard.
    #[tokio::test]
    async fn import_registers_parks_on_approval_and_dup_guards() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let ids = seed_embedded(&db);
        let st =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());

        let mk = |asset: i64, project: Option<i64>| crate::ImportRequest {
            asset_ids: vec![asset],
            project_id: project,
            dest_subfolder: None,
            with_dependencies: false,
        };

        // No editor registered ⇒ NoProject.
        assert!(matches!(
            st.request_import(mk(ids[0], None)),
            Err(crate::ImportError::NoProject)
        ));

        // Register: becomes the active project.
        let (pid, is_active) = st
            .register_project("C:/dev/Game", Some("6000.0.23f1"), Some("urp"))
            .unwrap();
        assert!(is_active);
        assert_eq!(st.list_projects().unwrap().len(), 1);

        // Import parks on the approval gate — nothing recorded yet, proven by a second request also
        // parking rather than reporting the asset already imported.
        let job = match st.request_import(mk(ids[0], None)) {
            Ok(crate::ImportKickoff::Awaiting(job)) => job,
            _ => panic!("expected an awaiting-approval job"),
        };
        assert_eq!(job.state(), crate::jobs::JobState::AwaitingApproval);
        assert!(
            matches!(
                st.request_import(mk(ids[0], Some(pid))),
                Ok(crate::ImportKickoff::Awaiting(_))
            ),
            "nothing is recorded before approval"
        );

        // Approve: the import records on a worker thread. Wait for the job to finish.
        st.approve_job(&job.id);
        for _ in 0..200 {
            if job.state() == crate::jobs::JobState::Done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(job.state(), crate::jobs::JobState::Done, "import completed");

        // Re-import the same asset into the same project ⇒ dup-guard: nothing new, no job.
        match st.request_import(mk(ids[0], Some(pid))) {
            Ok(crate::ImportKickoff::AllAlreadyImported(v)) => assert_eq!(v, vec![ids[0]]),
            _ => panic!("re-import must be dup-guarded"),
        }
    }

    #[tokio::test]
    async fn unknown_asset_is_a_not_found_envelope() {
        let (_d, st) = state();
        let token = st.token.clone();
        let (s, b) = send(router(st), get_req("/v1/assets/999", Some(&token), None)).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(b["error"]["code"], "not_found");
    }

    /// The per-tool allow-list (UI-SPEC §16, card 178): a disabled tool is `403 tool_disabled`,
    /// a still-enabled one works, and `search_assets`/`semantic_search` gate independently on the
    /// shared `/v1/search` route (the `?mode=ai` split, mirroring the handler).
    #[tokio::test]
    async fn disabled_tools_are_403_and_the_search_route_splits_by_mode() {
        let (_d, st) = state();
        let token = st.token.clone();

        // Everything except get_server_info and semantic_search.
        let enabled: std::collections::HashSet<String> = crate::AGENT_OPERATIONS
            .iter()
            .map(|(t, _)| t.to_string())
            .filter(|t| t != "get_server_info" && t != "semantic_search")
            .collect();
        st.set_allowed(enabled);

        let (s, b) = send(
            router(st.clone()),
            get_req("/v1/server-info", Some(&token), None),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "disabled tool");
        assert_eq!(b["error"]["code"], "tool_disabled");

        // Same route, two tools: plain search is enabled, ?mode=ai (semantic_search) is not.
        let (s, _) = send(
            router(st.clone()),
            get_req("/v1/search", Some(&token), None),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "search_assets still enabled");

        let (s, b) = send(
            router(st.clone()),
            get_req("/v1/search?mode=ai&q=dog", Some(&token), None),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "semantic_search disabled");
        assert_eq!(b["error"]["code"], "tool_disabled");

        // The REST-only file route has no tool, so the allow-list never blocks it.
        let (s, _) = send(
            router(st),
            get_req("/v1/assets/999/file", Some(&token), None),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::NOT_FOUND,
            "file route ungated (404, not 403)"
        );
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
