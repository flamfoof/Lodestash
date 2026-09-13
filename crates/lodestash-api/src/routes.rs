//! The `/v1/` read routes and the auth guard (ENGINE-API §1, §2, §4).

use std::collections::HashMap;
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
        .route("/v1/import-queue", get(import_queue))
        .route("/v1/import-queue/{request_id}/ack", post(import_queue_ack))
        .route("/v1/export", post(export))
        .route("/v1/jobs/{id}", get(job_status))
        .route("/v1/jobs/{id}/cancel", post(cancel_job))
        .route("/v1/tags", get(tags))
        .route("/v1/assets/{id}/tags", post(add_tag))
        .route("/v1/tags/batch", post(write_tags))
        .route("/v1/collections", get(collections))
        .route("/v1/mapping/apply", post(apply_mapping))
        .route("/v1/identify", post(identify_asset))
        .route("/v1/assets/{id}/suggest-tags", get(suggest_tags))
        .route("/v1/packages/unpack", post(unpack_package))
        .route("/v1/stage", get(list_staged).post(stage_asset))
        .route("/v1/stage/import", post(import_staged))
        .route("/v1/assets/{id}/file", get(file))
        .route("/v1/libraries", get(list_libraries))
        .route("/v1/assets", get(list_assets))
        .route("/v1/assets/read", post(read_assets))
        // Innermost: every handler below runs off the runtime thread (v1 plan S11).
        .layer(axum::middleware::from_fn(off_runtime))
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
        ("GET", "/v1/tags") => "list_tags",
        ("POST", "/v1/assets/{id}/tags") => "add_tag",
        ("POST", "/v1/tags/batch") => "write_tags",
        ("GET", "/v1/collections") => "list_collections",
        ("POST", "/v1/mapping/apply") => "apply_mapping",
        ("POST", "/v1/identify") => "identify_asset",
        ("GET", "/v1/assets/{id}/suggest-tags") => "suggest_tags",
        ("POST", "/v1/packages/unpack") => "unpack_package",
        ("POST", "/v1/stage") => "stage_asset",
        ("GET", "/v1/stage") => "list_staged",
        ("POST", "/v1/stage/import") => "import_staged",
        ("GET", "/v1/libraries") => "list_libraries",
        ("GET", "/v1/assets") => "list_assets",
        ("POST", "/v1/assets/read") => "get_assets",
        _ => return None,
    })
}

/// Run a handler on the blocking pool (v1 plan S11).
///
/// The server is one current-thread runtime, and the handlers call synchronous state: a catalog
/// behind a mutex, a thumbnail render, an ONNX query embedding, a write that waits its turn at the
/// writer. Any one of those held the only thread, so a preview being rendered stopped `jobs/{id}`
/// and `cancel` from answering — the two requests whose whole point is to answer while something
/// else is slow. Here the handler's future runs on a pool thread instead; auth and the allow-list
/// stay on the runtime thread, where they are cheap.
///
/// `Handle::block_on` from a pool thread is sound on a current-thread runtime while that
/// runtime's own `block_on` is running, which `serve` guarantees — it is what drives the IO and
/// timers a handler awaits.
async fn off_runtime(req: Request, next: Next) -> Response {
    let rt = tokio::runtime::Handle::current();
    match tokio::task::spawn_blocking(move || rt.block_on(next.run(req))).await {
        Ok(response) => response,
        Err(e) => err(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "The request worker failed.",
            &e.to_string(),
        ),
    }
}

/// Per-tool allow-list check (UI-SPEC §16, card 178). Runs after [`guard`]. A disabled tool is a
/// `403 tool_disabled` — the same operation the MCP plane refuses per tool, so the gate is 1:1.
async fn gate(
    State(state): State<Shared>,
    // `Option<MatchedPath>`: attached with `Router::layer`, this also runs for requests that match
    // no route, where the extension is absent. A required extractor rejected there, so an
    // authenticated request to an unknown path answered **500 instead of 404**. `None` means
    // "no route matched" — nothing to gate, so pass it through and let axum answer.
    matched: Option<MatchedPath>,
    method: Method,
    req: Request,
    next: Next,
) -> Response {
    if let Some(tool) = matched
        .as_ref()
        .and_then(|m| tool_for(&method, m.as_str(), req.uri().query()))
    {
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
    match state.server_info_json() {
        Ok(body) => Json(body).into_response(),
        Err(e) => db_err(e),
    }
}

/// A query string as pairs, so a repeated key (`tags=a&tags=b`) keeps every value — the struct form
/// keeps one — and a malformed number is an `invalid_argument` in the error envelope rather than
/// axum's plain-text rejection.
struct Params(Vec<(String, String)>);

impl Params {
    fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    fn all(&self, key: &str) -> Vec<String> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
            .collect()
    }

    fn parsed<T: std::str::FromStr>(&self, key: &str) -> Result<Option<T>, crate::AgentError> {
        self.get(key)
            .map(|v| {
                v.parse::<T>().map_err(|_| {
                    crate::AgentError::invalid(format!("{key} must be a number, not {v:?}"))
                })
            })
            .transpose()
    }
}

fn agent_err(e: crate::AgentError) -> Response {
    err(
        StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_REQUEST),
        e.code,
        &e.message,
        "",
    )
}

/// `search_assets` / `semantic_search` — with the optional scope of ENGINE-API §8.1 applied before
/// ranking. The historical default (50) and cap (500) stay as they were.
async fn search(
    State(state): State<Shared>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let p = Params(pairs);
    let parsed = (|| {
        let limit = p.parsed::<usize>("limit")?.unwrap_or(50).clamp(1, 500);
        let scope = state.search_scope(&crate::SearchScope {
            source_id: p.parsed("source_id")?,
            package_id: p.parsed("package_id")?,
            media_type: p.get("media_type").map(str::to_string),
            tags: p.all("tags"),
        })?;
        Ok::<_, crate::AgentError>((limit, scope))
    })();
    let (limit, scope) = match parsed {
        Ok(v) => v,
        Err(e) => return agent_err(e),
    };
    let q = p.get("q").unwrap_or("");
    if p.get("mode") == Some("ai") {
        return match state.semantic_search(q, limit, &scope) {
            Some(Ok(rows)) => {
                let out: Vec<Value> = rows.iter().map(|r| state.summary(r)).collect();
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
    let rows = match state.search(q, limit, &scope) {
        Ok(rows) => rows,
        Err(e) => return db_err(e),
    };
    let out: Vec<Value> = rows.iter().map(|r| state.summary(r)).collect();
    Json(json!({ "results": out })).into_response()
}

/// ENGINE-API §8.1 `list_libraries` — `GET /v1/libraries`.
async fn list_libraries(
    State(state): State<Shared>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let p = Params(pairs);
    let answer = p.parsed::<usize>("limit").and_then(|limit| {
        state.list_libraries(&crate::LibrariesRequest {
            limit,
            cursor: p.get("cursor").map(str::to_string),
        })
    });
    match answer {
        Ok(body) => Json(body).into_response(),
        Err(e) => agent_err(e),
    }
}

/// ENGINE-API §8.1 `list_assets` — `GET /v1/assets`.
async fn list_assets(
    State(state): State<Shared>,
    Query(pairs): Query<Vec<(String, String)>>,
) -> Response {
    let p = Params(pairs);
    let answer = (|| {
        let req = crate::ListAssetsRequest {
            source_id: p.parsed("source_id")?,
            package_id: p.parsed("package_id")?,
            media_type: p.get("media_type").map(str::to_string),
            status: p.get("status").map(str::to_string),
            tag_state: p.get("tag_state").map(str::to_string),
            selection: p.get("selection").map(str::to_string),
            limit: p.parsed("limit")?,
            cursor: p.get("cursor").map(str::to_string),
        };
        state.list_assets(&req)
    })();
    match answer {
        Ok(body) => Json(body).into_response(),
        Err(e) => agent_err(e),
    }
}

/// ENGINE-API §8.1 `get_assets` — `POST /v1/assets/read`. Read-only despite the method: an id list
/// belongs in a body, not a URL.
async fn read_assets(State(state): State<Shared>, body: axum::body::Bytes) -> Response {
    let req: crate::ReadAssetsRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return agent_err(crate::AgentError::invalid(format!(
                "the request body is not {{asset_ids, source_id?}}: {e}"
            )))
        }
    };
    match state.read_assets(&req) {
        Ok(body) => Json(body).into_response(),
        Err(e) => agent_err(e),
    }
}

async fn asset_detail(State(state): State<Shared>, Path(id): Path<i64>) -> Response {
    match state.asset_detail_json(id) {
        Ok(Some(body)) => Json(body).into_response(),
        Ok(None) => not_found(),
        Err(e) => db_err(e),
    }
}

/// ENGINE-API §8.3 `write_tags` — `POST /v1/tags/batch`. Read raw so the 1 MiB limit is held
/// before anything is parsed; unknown fields refuse the request.
async fn write_tags(State(state): State<Shared>, body: axum::body::Bytes) -> Response {
    if body.len() > crate::MAX_TAG_BATCH_BYTES {
        let (status, code, message) = crate::TagBatchError::TooLarge(body.len()).parts();
        return err(
            StatusCode::from_u16(status).unwrap_or(StatusCode::PAYLOAD_TOO_LARGE),
            code,
            &message,
            "",
        );
    }
    let req: crate::TagBatchRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            return err(
                StatusCode::BAD_REQUEST,
                "invalid_argument",
                "The tag batch is malformed.",
                &e.to_string(),
            )
        }
    };
    match state.request_tag_batch(req, body.len()) {
        Ok(k) => {
            let (status, v) = k.to_response();
            (
                StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                Json(v),
            )
                .into_response()
        }
        Err(e) => {
            let (status, code, message) = e.parts();
            err(
                StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
                code,
                &message,
                "",
            )
        }
    }
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
            let out: Vec<Value> = rows.iter().map(|r| state.summary(r)).collect();
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

/// ENGINE-API §2 `list_tags` — `GET /v1/tags?prefix=&limit=` (read-only).
async fn tags(State(state): State<Shared>, Query(q): Query<HashMap<String, String>>) -> Response {
    let prefix = q.get("prefix").map(String::as_str).unwrap_or("");
    let limit = q
        .get("limit")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(500);
    match state.list_tags(prefix, limit) {
        Ok(rows) => Json(json!({ "tags": rows })).into_response(),
        Err(e) => db_err(e),
    }
}

/// ENGINE-API §2 `add_tag` — `POST /v1/assets/{id}/tags` (mutating, through the writer).
async fn add_tag(
    State(state): State<Shared>,
    Path(id): Path<i64>,
    Json(body): Json<TagBody>,
) -> Response {
    match state.add_tag(id, &body.namespace, &body.value) {
        Ok(changed) => Json(json!({ "changed": changed })).into_response(),
        Err(e) => err(
            StatusCode::BAD_REQUEST,
            "write_failed",
            "The tag write failed.",
            &e,
        ),
    }
}

#[derive(serde::Deserialize)]
struct TagBody {
    #[serde(default = "default_ns")]
    namespace: String,
    value: String,
}
fn default_ns() -> String {
    "free".into()
}

/// ENGINE-API §2 `list_collections` — `GET /v1/collections` (incl. the staging tray).
async fn collections(State(state): State<Shared>) -> Response {
    match state.list_collections() {
        Ok(rows) => Json(json!({ "collections": rows })).into_response(),
        Err(e) => db_err(e),
    }
}

/// ENGINE-API §2 `apply_mapping` — `POST /v1/mapping/apply` (mutating).
async fn apply_mapping(
    State(state): State<Shared>,
    Json(body): Json<HashMap<String, i64>>,
) -> Response {
    let Some(taxonomy_id) = body.get("taxonomy_id").copied() else {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "taxonomy_id is required.",
            "",
        );
    };
    match state.apply_mapping(taxonomy_id) {
        Ok(changed) => Json(json!({ "changed": changed })).into_response(),
        Err(e) => err(
            StatusCode::BAD_REQUEST,
            "write_failed",
            "The mapping run failed.",
            &e,
        ),
    }
}

/// ENGINE-API §2 `identify_asset` — `POST /v1/identify` (long-running → `202` job).
async fn identify_asset(
    State(state): State<Shared>,
    Json(body): Json<HashMap<String, Vec<i64>>>,
) -> Response {
    let Some(asset_ids) = body.get("asset_ids") else {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "asset_ids is required.",
            "",
        );
    };
    match state.identify_asset(asset_ids.clone()) {
        Ok(job) => (StatusCode::ACCEPTED, Json(job.to_json())).into_response(),
        Err(e) => err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Identify refused.",
            &e,
        ),
    }
}

/// ENGINE-API §2 `suggest_tags` — `GET /v1/assets/{id}/suggest-tags` (read-only).
async fn suggest_tags(State(state): State<Shared>, Path(id): Path<i64>) -> Response {
    match state.suggest_tags(id) {
        Ok(tags) => Json(json!({ "tags": tags })).into_response(),
        Err(e) => db_err(e),
    }
}

/// ENGINE-API §2 `unpack_package` — `POST /v1/packages/unpack` (long-running → `202` job).
async fn unpack_package(
    State(state): State<Shared>,
    Json(body): Json<HashMap<String, i64>>,
) -> Response {
    let Some(package_id) = body.get("package_id").copied() else {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "package_id is required.",
            "",
        );
    };
    match state.unpack_package(package_id) {
        Ok(job) => (StatusCode::ACCEPTED, Json(job.to_json())).into_response(),
        Err(e) => err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Unpack refused.",
            &e,
        ),
    }
}

/// ENGINE-API §6 — `GET /v1/stage` (the active project's shortlist).
async fn list_staged(State(state): State<Shared>) -> Response {
    match state.list_staged() {
        Ok(rows) => {
            let out: Vec<Value> = rows.iter().map(|r| state.summary(r)).collect();
            Json(json!({ "assets": out })).into_response()
        }
        Err(e) => db_err(e),
    }
}

/// ENGINE-API §6 — `POST /v1/stage` `{asset_id}`.
async fn stage_asset(
    State(state): State<Shared>,
    Json(body): Json<HashMap<String, i64>>,
) -> Response {
    let Some(asset_id) = body.get("asset_id").copied() else {
        return err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "asset_id is required.",
            "",
        );
    };
    match state.stage_asset(asset_id) {
        Ok(changed) => Json(json!({ "changed": changed })).into_response(),
        Err(e) => err(StatusCode::CONFLICT, "stage_failed", "Staging refused.", &e),
    }
}

/// ENGINE-API §6 — `POST /v1/stage/import` (same approval + queue as import_to_project).
async fn import_staged(State(state): State<Shared>) -> Response {
    match state.import_staged() {
        Ok(k @ crate::ImportKickoff::Awaiting { .. }) => {
            (StatusCode::ACCEPTED, Json(k.to_json())).into_response()
        }
        Ok(k @ crate::ImportKickoff::AllAlreadyImported { .. }) => {
            Json(k.to_json()).into_response()
        }
        Err(crate::ImportError::Empty) => err(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The staging tray is empty.",
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

/// *Side-effecting* (ENGINE-API §5.2). Parks a `202` job on the approval gate like `export`, or
/// answers `200 { imported_existing, job_id: null }` when the dup-guard (§5.4) leaves nothing new,
/// or `409 project_not_registered` when no editor is open.
async fn import_to_project(
    State(state): State<Shared>,
    Json(req): Json<crate::ImportRequest>,
) -> Response {
    match state.request_import(req) {
        Ok(k @ crate::ImportKickoff::Awaiting { .. }) => {
            (StatusCode::ACCEPTED, Json(k.to_json())).into_response()
        }
        Ok(k @ crate::ImportKickoff::AllAlreadyImported { .. }) => {
            Json(k.to_json()).into_response()
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

#[derive(Debug, Deserialize)]
struct QueueParams {
    project_id: i64,
}

/// **REST-only** plugin transport (ENGINE-API §5.3, excluded from the MCP↔REST parity scope): the
/// plugin long-polls this on Unity's main thread for imports to perform. Returns a bare array.
async fn import_queue(State(state): State<Shared>, Query(p): Query<QueueParams>) -> Response {
    match state.poll_import_queue(p.project_id) {
        Ok(reqs) => {
            let out: Vec<Value> = reqs
                .iter()
                .map(|r| {
                    json!({
                        "request_id": r.request_id,
                        "job_id": r.job_id,
                        "dest_subfolder": r.dest_subfolder,
                        "assets": r.assets.iter().map(summary_json).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => db_err(e),
    }
}

#[derive(Debug, Deserialize)]
struct AckBody {
    imported: Vec<crate::AckAsset>,
}

/// **REST-only** plugin transport (ENGINE-API §5.3): the plugin reports what it imported, and the
/// core writes the durable `import_record`s (with the real Unity `dest_path`/`guid`) + flips the
/// job `done`.
async fn import_queue_ack(
    State(state): State<Shared>,
    Path(request_id): Path<String>,
    Json(body): Json<AckBody>,
) -> Response {
    match state.ack_import(&request_id, body.imported) {
        Ok(()) => StatusCode::OK.into_response(),
        Err(crate::AckError::UnknownRequest) => err(
            StatusCode::NOT_FOUND,
            "not_found",
            "No such import request.",
            "It may have been acked already, or the session restarted.",
        ),
        Err(crate::AckError::Db(e)) => db_err(e),
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
    if let Some(denied) = bytes_denied(&state) {
        return denied;
    }
    match state.preview_path(id) {
        Some(path) => serve_file(&path, "image/png").await,
        None => not_found(),
    }
}

async fn file(State(state): State<Shared>, Path(id): Path<i64>) -> Response {
    if let Some(denied) = bytes_denied(&state) {
        return denied;
    }
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
        // Real tags since 2026-08-30. This answered `[]` unconditionally, so an agent that
        // wrote a tag through `add_tag` read back an asset that appeared to have none.
        "tags": r.tags.iter().map(|t| serde_json::json!({
            "namespace": t.namespace,
            "value": t.value,
            "confidence": t.confidence,
        })).collect::<Vec<_>>(),
        "preview_url": format!("/v1/assets/{}/preview", r.id),
        "file_url": format!("/v1/assets/{}/file", r.id),
    })
}

/// ENGINE-API §3 `Package`.
///
/// `license`/`attribution` are the harvest's (INGESTION §2) — the pack's own statement, or the
/// Asset Store EULA where the pack came from the store cache and said nothing itself.
/// `pipeline` is what the `pipeline_mismatch` advisory compares (§5.4); `null` means undetermined,
/// which is not the same as Built-in.
pub fn package_json(p: &lodestash_core::catalog::PackageListing) -> Value {
    json!({
        "id": p.id,
        "name": p.name,
        "publisher": p.publisher,
        "category": p.category,
        "asset_count": p.asset_count,
        "unpacked_at": p.unpacked_at,
        "version": p.version,
        "license": p.license,
        "license_url": p.license_url,
        "attribution": p.attribution,
        "pipeline": p.pipeline,
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

/// The one refusal for every byte-serving route (v1 plan S10): metadata and links stay available,
/// the bytes do not, and the answer says which switch would change that.
fn bytes_denied(state: &Shared) -> Option<Response> {
    if state.byte_release() {
        return None;
    }
    Some(
        (
            StatusCode::FORBIDDEN,
            Json(json!({"error": {"code": "bytes_not_released",
                "message": "this agent may read metadata and links; serving asset bytes needs \
                            `mcp_bytes_enabled` in Settings"}})),
        )
            .into_response(),
    )
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

    /// The POST half of `get_req`. JSON body, same loopback Host and bearer token, because the
    /// guard checks both on every request.
    fn post_req(uri: &str, token: Option<&str>, body: Value) -> HttpRequest<Body> {
        let mut b = HttpRequest::post(uri)
            .header(header::HOST, "127.0.0.1:51731")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(t) = token {
            b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        b.body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap()
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
            content_hash: None,
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

    /// The discovery routes are the shared operations (v1 plan S12): each REST body is exactly what
    /// `ServerState` returns for the same request — the value MCP's tool returns too — refusals
    /// carry the same code, and the allow-list gates the new routes like any other.
    #[tokio::test]
    async fn discovery_routes_are_the_shared_operations() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let ids = seed_embedded(&db);
        let st =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        let token = st.token.clone();
        let get = |uri: &str| {
            let st = st.clone();
            let req = get_req(uri, Some(&token), None);
            async move { send(router(st), req).await }
        };

        let (s, libs) = get("/v1/libraries").await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(libs["libraries"][0]["asset_count"], 3);
        let source = libs["libraries"][0]["id"].as_i64().unwrap();

        let (s, page) = get(&format!("/v1/assets?source_id={source}&limit=2")).await;
        assert_eq!(s, StatusCode::OK);
        let shared = st
            .list_assets(&crate::ListAssetsRequest {
                source_id: Some(source),
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(page, shared, "REST and the shared operation disagree");
        let cursor = page["next_cursor"].as_str().unwrap();
        let (_, rest) = get(&format!(
            "/v1/assets?source_id={source}&limit=2&cursor={cursor}"
        ))
        .await;
        let walked: Vec<i64> = page["assets"]
            .as_array()
            .unwrap()
            .iter()
            .chain(rest["assets"].as_array().unwrap())
            .map(|a| a["id"].as_i64().unwrap())
            .collect();
        assert_eq!(walked, ids, "two pages visit the library in id order");
        assert!(rest["next_cursor"].is_null());

        for (uri, code) in [
            ("/v1/assets".to_string(), "invalid_argument"),
            (
                format!("/v1/assets?source_id={source}&limit=abc"),
                "invalid_argument",
            ),
            (
                format!("/v1/assets?source_id={source}&cursor=nope"),
                "invalid_cursor",
            ),
            (
                "/v1/search?q=a&tags=untagged".to_string(),
                "invalid_argument",
            ),
            (
                "/v1/search?q=a&media_type=mesh".to_string(),
                "invalid_argument",
            ),
        ] {
            let (s, body) = get(&uri).await;
            assert_eq!(
                (s, body["error"]["code"].as_str()),
                (StatusCode::BAD_REQUEST, Some(code)),
                "{uri}"
            );
        }

        let (s, read) = send(
            router(st.clone()),
            post_req(
                "/v1/assets/read",
                Some(&token),
                json!({"asset_ids": [ids[1], 999]}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(
            read,
            st.read_assets(&crate::ReadAssetsRequest {
                asset_ids: vec![ids[1], 999],
                source_id: None
            })
            .unwrap()
        );
        let (s, body) = send(
            router(st.clone()),
            post_req(
                "/v1/assets/read",
                Some(&token),
                json!({"asset_ids": [1], "extra": true}),
            ),
        )
        .await;
        assert_eq!(
            (s, body["error"]["code"].as_str()),
            (StatusCode::BAD_REQUEST, Some("invalid_argument"))
        );

        // A scoped search ranks within its scope, and a summary carries the §8.1 additions.
        let (_, found) = get(&format!(
            "/v1/search?q=png&source_id={source}&media_type=image"
        ))
        .await;
        let first = &found["results"][0];
        assert_eq!(first["source_id"], source);
        assert_eq!(
            first["file_availability"], "denied",
            "bytes are not released in this fixture"
        );
        assert!(first["file_resource_uri"]
            .as_str()
            .unwrap()
            .starts_with("asset://"));

        st.set_allowed(["search_assets".to_string()].into());
        for uri in [
            "/v1/libraries".to_string(),
            format!("/v1/assets?source_id={source}"),
        ] {
            let (s, body) = get(&uri).await;
            assert_eq!(
                (s, body["error"]["code"].as_str()),
                (StatusCode::FORBIDDEN, Some("tool_disabled")),
                "{uri}"
            );
        }
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
        st.set_write(crate::test_writes(&db));

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
            Ok(crate::ImportKickoff::Awaiting { job, .. }) => job,
            _ => panic!("expected an awaiting-approval job"),
        };
        assert_eq!(job.state(), crate::jobs::JobState::AwaitingApproval);
        assert!(
            matches!(
                st.request_import(mk(ids[0], Some(pid))),
                Ok(crate::ImportKickoff::Awaiting { .. })
            ),
            "nothing is recorded before approval"
        );

        // Approve: enqueues for the plugin on a worker thread. Wait until it is queued.
        st.approve_job(&job.id);
        let mut reqs = Vec::new();
        for _ in 0..200 {
            reqs = st.poll_import_queue(pid).unwrap();
            if !reqs.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(reqs.len(), 1, "one request queued for the plugin");
        // Visibility timeout: a plugin that polls again before acking — its own retry, a Unity
        // domain reload, a dropped response — must NOT be handed the same request, or it copies
        // the files into Assets/ twice. The UNIQUE(asset_id, project_id) guard only stops the
        // duplicate *record*.
        assert!(
            st.poll_import_queue(pid).unwrap().is_empty(),
            "a re-poll inside the lease window must return nothing"
        );
        assert_eq!(
            job.state(),
            crate::jobs::JobState::Running,
            "in flight until the plugin acks"
        );

        // A malformed ack — claiming an id this request never queued — is rejected whole, and it
        // must NOT consume the queue entry, or the retry with the real ids below would 404.
        assert!(matches!(
            st.ack_import(
                &reqs[0].request_id,
                vec![crate::AckAsset {
                    asset_id: 999_999,
                    dest_path: "Assets/Bogus".into(),
                    unity_guid: None,
                }],
            ),
            Err(crate::AckError::UnknownRequest)
        ));

        // Simulate the Unity plugin acking with a real in-project path + GUID.
        let req0 = &reqs[0];
        let imported: Vec<crate::AckAsset> = req0
            .assets
            .iter()
            .map(|a| crate::AckAsset {
                asset_id: a.id,
                dest_path: format!("Assets/Wildlife/{}", a.name),
                unity_guid: Some("8e1c".into()),
            })
            .collect();
        st.ack_import(&req0.request_id, imported).unwrap();
        assert_eq!(
            job.state(),
            crate::jobs::JobState::Done,
            "ack completes the job"
        );

        // Queue drained; a second ack of the same request is UnknownRequest.
        assert!(st.poll_import_queue(pid).unwrap().is_empty());
        assert!(matches!(
            st.ack_import(&req0.request_id, vec![]),
            Err(crate::AckError::UnknownRequest)
        ));

        // Re-import the same asset into the same project ⇒ dup-guard: nothing new, no job.
        match st.request_import(mk(ids[0], Some(pid))) {
            Ok(crate::ImportKickoff::AllAlreadyImported {
                imported_existing, ..
            }) => assert_eq!(imported_existing, vec![ids[0]]),
            _ => panic!("re-import must be dup-guarded"),
        }
    }

    /// The **wire** the Unity plugin will be written against (ENGINE-API §5.3), end to end over
    /// HTTP rather than through `ServerState`.
    ///
    /// The state-level round-trip is covered above, but the plugin does not call Rust functions —
    /// it calls two URLs with a bearer token and parses JSON. Everything that can be wrong between
    /// those two layers is invisible to that test and invisible to the C# author until they are
    /// deep in an editor: the query-parameter name, the field names inside each queued request,
    /// whether the response is a bare array or an envelope, which status code an already-acked
    /// request returns, and whether an unauthenticated poll leaks the queue.
    ///
    /// This is the contract. If it changes, the plugin breaks in the field, so it is asserted
    /// field by field rather than by shape.
    #[tokio::test]
    async fn the_plugin_transport_works_over_http_exactly_as_specified() {
        use lodestash_core::catalog::{Catalog, CrawlRecord};

        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("beaver.fbx"), b"mesh").unwrap();
        let db = dir.path().join("catalog.db");
        let asset_id = {
            let mut cat = Catalog::open(&db).unwrap();
            cat.seed_media_types().unwrap();
            let src = cat
                .upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
                .unwrap();
            cat.record_crawl(
                src,
                None,
                &[CrawlRecord {
                    workspace_rel: None,
                    rel_path: "beaver.fbx".into(),
                    name: "beaver.fbx".into(),
                    media_type: "model_3d".into(),
                    size_bytes: 4,
                    mtime: 1,
                    content_hash: None,
                }],
            )
            .unwrap();
            cat.assets_for_source(src).unwrap()[0].asset_id
        };
        let st: Shared =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        st.set_write(crate::test_writes(&db));
        let token = st.token.clone();

        // The plugin registers itself first (§5.1).
        let (s, body) = send(
            router(st.clone()),
            post_req(
                "/v1/projects/register",
                Some(&token),
                json!({ "project_path": "C:/dev/Game", "engine_version": "6000.0.23f1" }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "register: {body}");
        let project_id = body["project_id"]
            .as_i64()
            .expect("register returns project_id");

        // An unauthenticated poll must not hand the queue to anything on loopback.
        let (s, _) = send(
            router(st.clone()),
            get_req(
                &format!("/v1/import-queue?project_id={project_id}"),
                None,
                None,
            ),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::UNAUTHORIZED,
            "the import queue is readable without a token"
        );

        // Queue an import and approve it, the way the desktop app does.
        let job = match st.request_import(crate::ImportRequest {
            project_id: Some(project_id),
            asset_ids: vec![asset_id],
            dest_subfolder: Some("Wildlife".into()),
            with_dependencies: false,
        }) {
            Ok(crate::ImportKickoff::Awaiting { job, .. }) => job,
            _ => panic!("expected an awaiting-approval job"),
        };
        st.approve_job(&job.id);

        // Poll it the way the plugin does. The enqueue runs on a worker thread, so retry.
        let mut queued = Value::Null;
        for _ in 0..200 {
            let (s, body) = send(
                router(st.clone()),
                get_req(
                    &format!("/v1/import-queue?project_id={project_id}"),
                    Some(&token),
                    None,
                ),
            )
            .await;
            assert_eq!(s, StatusCode::OK);
            assert!(body.is_array(), "§5.3 specifies a bare array, got: {body}");
            if !body.as_array().unwrap().is_empty() {
                queued = body;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let reqs = queued.as_array().expect("nothing was ever queued");
        assert_eq!(reqs.len(), 1);

        // Field by field — every one of these names is compiled into the plugin.
        let r = &reqs[0];
        let request_id = r["request_id"].as_str().expect("request_id").to_string();
        assert!(r["job_id"].is_string(), "job_id missing: {r}");
        assert_eq!(r["dest_subfolder"], "Wildlife");
        let assets = r["assets"].as_array().expect("assets array");
        assert_eq!(assets.len(), 1);
        assert_eq!(assets[0]["id"], asset_id, "the plugin acks by this id");
        assert!(
            assets[0]["file_url"].is_string(),
            "§5.3: each asset carries the file_url the plugin fetches — {}",
            assets[0]
        );

        // A second poll inside the lease window returns nothing, or the plugin copies twice.
        let (_, again) = send(
            router(st.clone()),
            get_req(
                &format!("/v1/import-queue?project_id={project_id}"),
                Some(&token),
                None,
            ),
        )
        .await;
        assert_eq!(
            again.as_array().unwrap().len(),
            0,
            "a re-poll re-served the same request"
        );

        // Ack with what Unity actually produced.
        let (s, body) = send(
            router(st.clone()),
            post_req(
                &format!("/v1/import-queue/{request_id}/ack"),
                Some(&token),
                json!({ "imported": [{
                    "asset_id": asset_id,
                    "dest_path": "Assets/Wildlife/beaver.fbx",
                    "unity_guid": "8e1cdeadbeef"
                }]}),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::OK, "ack rejected: {body}");

        // Acking twice is a 404 with the documented envelope, not a 500 and not a silent success —
        // a plugin that retries after a dropped response has to be able to tell those apart.
        let (s, body) = send(
            router(st.clone()),
            post_req(
                &format!("/v1/import-queue/{request_id}/ack"),
                Some(&token),
                json!({ "imported": [] }),
            ),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND);
        assert_eq!(
            body["error"]["code"], "not_found",
            "ack error envelope: {body}"
        );

        // And the durable half: the ack wrote the `import_record` the dup-guard reads.
        match st.request_import(crate::ImportRequest {
            project_id: Some(project_id),
            asset_ids: vec![asset_id],
            dest_subfolder: None,
            with_dependencies: false,
        }) {
            Ok(crate::ImportKickoff::AllAlreadyImported {
                imported_existing, ..
            }) => assert_eq!(imported_existing, vec![asset_id]),
            _ => panic!("the ack did not persist an import_record"),
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

    /// The gate middleware runs via `Router::layer`, which also fires when no route matches. A
    /// *required* `MatchedPath` extractor rejected there, so an authenticated request to an
    /// unknown path answered 500 instead of 404. `Option<MatchedPath>` lets axum answer 404.
    #[tokio::test]
    async fn unknown_path_is_404_not_500() {
        let (_d, st) = state();
        let token = st.token.clone();
        let (s, _) = send(
            router(st),
            get_req("/v1/definitely-not-a-route", Some(&token), None),
        )
        .await;
        assert_eq!(s, StatusCode::NOT_FOUND, "unknown path -> 404, not 500");
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

        // The REST-only file route has no tool, so the allow-list never blocks it — but bytes are
        // their own permission (v1 plan S10) and it is off until the GUI says otherwise.
        let (s, b) = send(
            router(st.clone()),
            get_req("/v1/assets/999/file", Some(&token), None),
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "bytes served without consent");
        assert_eq!(b["error"]["code"], "bytes_not_released");

        // The preview route serves bytes too, and is gated by the same switch.
        let (s, b) = send(
            router(st.clone()),
            get_req("/v1/assets/999/preview", Some(&token), None),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "preview bytes served without consent"
        );
        assert_eq!(b["error"]["code"], "bytes_not_released");

        st.set_byte_release(true);
        let (s, _) = send(
            router(st),
            get_req("/v1/assets/999/file", Some(&token), None),
        )
        .await;
        assert_eq!(
            s,
            StatusCode::NOT_FOUND,
            "with bytes released the route is ungated (404 for a missing asset, not 403)"
        );
    }

    /// The engine is one current-thread runtime (v1 plan S11). A slow handler — here a preview that
    /// takes a second and a half to render — must not stop a quick question from being answered
    /// meanwhile; before `off_runtime`, the render held the only thread and everything queued
    /// behind it.
    #[tokio::test]
    async fn a_slow_handler_does_not_stall_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let ids = seed_embedded(&db);
        let st = Arc::new(
            ServerState::new(
                &db,
                dir.path().to_path_buf(),
                Box::new(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    None
                }),
            )
            .unwrap(),
        );
        st.set_byte_release(true);
        let token = st.token.clone();

        let started = std::time::Instant::now();
        let slow = send(
            router(st.clone()),
            get_req(
                &format!("/v1/assets/{}/preview", ids[0]),
                Some(&token),
                None,
            ),
        );
        let quick = async {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let answer = send(
                router(st.clone()),
                get_req("/v1/server-info", Some(&token), None),
            )
            .await;
            (answer, started.elapsed())
        };
        let ((slow_status, _), ((quick_status, _), quick_at)) = tokio::join!(slow, quick);
        assert_eq!(quick_status, StatusCode::OK);
        assert_eq!(
            slow_status,
            StatusCode::NOT_FOUND,
            "the render produced nothing"
        );
        assert!(
            quick_at < std::time::Duration::from_millis(1000),
            "server-info waited {quick_at:?} for a preview render"
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

    /// A catalog file with one image asset, and the state serving it with a write bridge that
    /// applies approved batches through a second connection — the app's writer, in miniature.
    fn tag_state() -> (tempfile::TempDir, Shared, i64) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let id = {
            let mut c = lodestash_core::catalog::Catalog::open(&db).unwrap();
            c.seed_media_types().unwrap();
            let src = c.upsert_import_source("Lib", "C:/lib", "folder").unwrap();
            c.record_crawl(
                src,
                None,
                &[lodestash_core::catalog::CrawlRecord {
                    workspace_rel: None,
                    content_hash: None,
                    rel_path: "wall.png".into(),
                    name: "wall.png".into(),
                    media_type: "image".into(),
                    size_bytes: 1,
                    mtime: 1,
                }],
            )
            .unwrap();
            c.assets_needing_tags_within(1, None).unwrap()[0].asset_id
        };
        let s =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        let writer_db = db.clone();
        s.set_write(Box::new(move |w| match w {
            crate::AgentWrite::TagBatch { request, digest } => {
                lodestash_core::catalog::Catalog::open(&writer_db)
                    .map_err(|e| e.to_string())?
                    .apply_tag_batch(&request.to_core_batch(&digest))
                    .map(|o| o.changed_assets.len())
                    .map_err(|e| e.to_string())
            }
            other => Err(format!("unexpected write {other:?}")),
        }));
        (dir, s, id)
    }

    fn batch_body(
        request_id: &str,
        asset_id: i64,
        revision: &str,
        producer: &str,
        value: &str,
    ) -> Value {
        json!({
            "request_id": request_id,
            "assets": [{
                "asset_id": asset_id,
                "expected_revision": revision,
                "producer_id": producer,
                "model_id": "gpt-5-codex",
                "mode": "append",
                "tags": [{ "namespace": "subject", "value": value, "confidence": 0.8 }],
            }],
        })
    }

    async fn settle(st: &Shared, job_id: &str) -> Value {
        for _ in 0..250 {
            let j = st.jobs.get(job_id).expect("job").to_json();
            if !matches!(
                j["state"].as_str(),
                Some("awaiting_approval" | "queued" | "running")
            ) {
                return j;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        panic!("job {job_id} never settled");
    }

    const RID_A: &str = "0b4ad2c7-6c1a-4c5e-9a0f-1f2b3c4d5e6a";
    const RID_B: &str = "1c5be3d8-7d2b-4d6f-8b10-2a3c4d5e6f7b";

    /// Everything that can be refused is refused before a request is parked — so the person at the
    /// machine is never asked to approve something that could not be written.
    #[tokio::test]
    async fn write_tags_refuses_before_it_parks() {
        let (_d, st, id) = tag_state();
        let token = st.token.clone();
        let app = || router(st.clone());
        let rev = st.asset_detail_json(id).unwrap().unwrap()["annotation_revision"]
            .as_str()
            .unwrap()
            .to_string();
        let post = |body: Value| post_req("/v1/tags/batch", Some(&token), body);

        let (s, v) = send(
            app(),
            post(batch_body(RID_A, id, &rev, "codex@test", "stone")),
        )
        .await;
        assert_eq!(
            (s, v["error"]["code"].as_str()),
            (StatusCode::CONFLICT, Some("producer_not_configured"))
        );
        st.set_agent_producer(Some("codex@test".into()));

        let mut unknown = batch_body(RID_A, id, &rev, "codex@test", "stone");
        unknown["assets"][0]["confirmed"] = json!(true);
        let mut many = batch_body(RID_A, id, &rev, "codex@test", "stone");
        many["assets"] = json!((0..101)
            .map(|i| json!({"asset_id": i, "expected_revision": "r", "producer_id": "codex@test", "mode": "append"}))
            .collect::<Vec<_>>());
        let mut no_revision = batch_body(RID_A, id, &rev, "codex@test", "stone");
        no_revision["assets"][0]
            .as_object_mut()
            .unwrap()
            .remove("expected_revision");
        for (body, status, code) in [
            (unknown, StatusCode::BAD_REQUEST, "invalid_argument"),
            (
                batch_body("not-a-uuid", id, &rev, "codex@test", "stone"),
                StatusCode::BAD_REQUEST,
                "invalid_argument",
            ),
            (
                batch_body(RID_A, id, &rev, "someone-else", "stone"),
                StatusCode::FORBIDDEN,
                "producer_mismatch",
            ),
            (many, StatusCode::BAD_REQUEST, "invalid_argument"),
            (no_revision, StatusCode::BAD_REQUEST, "invalid_argument"),
            (
                batch_body(RID_A, id, &rev, "codex@test", "0.93"),
                StatusCode::BAD_REQUEST,
                "invalid_argument",
            ),
            (
                batch_body(RID_A, 999_999, &rev, "codex@test", "stone"),
                StatusCode::NOT_FOUND,
                "asset_not_found",
            ),
        ] {
            let (s, v) = send(app(), post(body)).await;
            assert_eq!(
                (s, v["error"]["code"].as_str()),
                (status, Some(code)),
                "{v}"
            );
        }
        assert!(
            st.jobs.awaiting_approval().is_empty(),
            "a refused request was parked"
        );
    }

    /// The whole contract end to end: parked with its diff, written once on approval as the agent's
    /// producer, answered from its receipt on retry, refused on a reused id, refused when the asset
    /// moved under it — and a denial writes nothing.
    #[tokio::test]
    async fn an_approved_batch_writes_once_and_a_retry_gets_the_same_answer() {
        let (_d, st, id) = tag_state();
        st.set_agent_producer(Some("codex@test".into()));
        let token = st.token.clone();
        let app = || router(st.clone());
        let rev = |st: &Shared| {
            st.asset_detail_json(id).unwrap().unwrap()["annotation_revision"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let first = rev(&st);
        let body = batch_body(RID_A, id, &first, "codex@test", "stone");

        let (s, parked) = send(
            app(),
            post_req("/v1/tags/batch", Some(&token), body.clone()),
        )
        .await;
        assert_eq!(s, StatusCode::ACCEPTED, "{parked}");
        assert_eq!(parked["state"], "awaiting_approval");
        assert_eq!(
            parked["detail"]["assets"][0]["proposed"],
            json!(["subject:stone"])
        );
        let job_id = parked["job_id"].as_str().unwrap().to_string();
        let (_, again) = send(
            app(),
            post_req("/v1/tags/batch", Some(&token), body.clone()),
        )
        .await;
        assert_eq!(
            again["job_id"], parked["job_id"],
            "a parked request answers with its job"
        );

        st.approve_job(&job_id);
        let done = settle(&st, &job_id).await;
        assert_eq!(done["state"], "done", "{done}");
        assert_eq!(done["result"]["changed_assets"], json!([id]));
        let detail = st.asset_detail_json(id).unwrap().unwrap();
        assert_eq!(detail["tag_provenance"][0]["source"], "ai:codex@test");
        assert_eq!(detail["tag_provenance"][0]["model_id"], "gpt-5-codex");

        let (s, replay) = send(app(), post_req("/v1/tags/batch", Some(&token), body)).await;
        assert_eq!(
            (s, replay["replayed"].as_bool()),
            (StatusCode::OK, Some(true))
        );
        assert_eq!(
            replay["result"], done["result"],
            "a retry gets the original answer"
        );
        let (s, conflict) = send(
            app(),
            post_req(
                "/v1/tags/batch",
                Some(&token),
                batch_body(RID_A, id, &first, "codex@test", "moss"),
            ),
        )
        .await;
        assert_eq!(
            (s, conflict["error"]["code"].as_str()),
            (StatusCode::CONFLICT, Some("request_conflict"))
        );

        let (_, stale) = send(
            app(),
            post_req(
                "/v1/tags/batch",
                Some(&token),
                batch_body(RID_B, id, &first, "codex@test", "moss"),
            ),
        )
        .await;
        let stale_id = stale["job_id"].as_str().unwrap().to_string();
        st.approve_job(&stale_id);
        let failed = settle(&st, &stale_id).await;
        assert_eq!(failed["state"], "error");
        assert!(
            failed["error"]
                .as_str()
                .unwrap()
                .contains("revision_conflict"),
            "{failed}"
        );

        let (_, denied) = send(
            app(),
            post_req(
                "/v1/tags/batch",
                Some(&token),
                batch_body(
                    "2d6cf4e9-8e3c-4e70-9c21-3b4d5e6f7a8c",
                    id,
                    &rev(&st),
                    "codex@test",
                    "ivy",
                ),
            ),
        )
        .await;
        let denied_id = denied["job_id"].as_str().unwrap().to_string();
        st.deny_job(&denied_id);
        assert_eq!(
            st.jobs.get(&denied_id).unwrap().to_json()["state"],
            "cancelled"
        );
        let values: Vec<String> = st.asset_detail_json(id).unwrap().unwrap()["tag_provenance"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["value"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(values, vec!["stone"], "only the approved write landed");
    }
}
