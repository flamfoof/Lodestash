//! Today's router and `lodestash-protocol`'s catalog describe one surface (v2 plan Task 2.1).
//!
//! Requests here are built by the catalog — `OpSpec::request`, the generic path the P4 forwarder
//! takes — and sent to the real router. A route counts as reached only when axum matched the
//! route template the catalog names; a status code alone cannot tell a handler's 404 from the
//! router's. Once P3 derives the router from the catalog, these tests hold by construction.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::MatchedPath;
use axum::http::{header, HeaderValue, Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::Router;
use lodestash_api::ServerState;
use lodestash_protocol::{Audience, OpSpec, WireRequest, OPERATIONS};
use serde_json::{json, Map, Value};
use tower::ServiceExt;

const MATCHED: &str = "x-test-matched-route";

/// Today's router, reporting which route template answered each request.
fn app(state: &Arc<ServerState>) -> Router {
    async fn record(matched: Option<MatchedPath>, req: Request<Body>, next: Next) -> Response {
        let route = matched.map(|m| m.as_str().to_string());
        let mut res = next.run(req).await;
        if let Some(route) = route {
            res.headers_mut()
                .insert(MATCHED, HeaderValue::from_str(&route).unwrap());
        }
        res
    }
    lodestash_api::router(state.clone()).layer(middleware::from_fn(record))
}

/// A catalog with one library holding `rock.png` and `moss.png`; returns the library id.
fn seed(db: &Path) -> i64 {
    let mut cat = lodestash_core::catalog::Catalog::open(db).unwrap();
    cat.seed_media_types().unwrap();
    let src = cat.upsert_import_source("Lib", "C:/lib", "folder").unwrap();
    let rec = |name: &str| lodestash_core::catalog::CrawlRecord {
        workspace_rel: None,
        rel_path: name.into(),
        name: name.into(),
        media_type: "image".into(),
        size_bytes: 1,
        mtime: 1,
        content_hash: None,
    };
    cat.record_crawl(src, None, &[rec("rock.png"), rec("moss.png")])
        .unwrap();
    src
}

fn state() -> (tempfile::TempDir, Arc<ServerState>, i64) {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    let source = seed(&db);
    let state = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap();
    (dir, Arc::new(state), source)
}

async fn send(state: &Arc<ServerState>, r: &WireRequest) -> (StatusCode, Option<String>, Value) {
    let b = Request::builder()
        .method(r.method.as_str())
        .uri(&r.target)
        .header(header::HOST, "127.0.0.1:51731")
        .header(header::AUTHORIZATION, format!("Bearer {}", state.token()));
    let req = match &r.body {
        Some(body) => b
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(serde_json::to_vec(body).unwrap())),
        None => b.body(Body::empty()),
    }
    .unwrap();
    let res = app(state).oneshot(req).await.unwrap();
    let status = res.status();
    let matched = res
        .headers()
        .get(MATCHED)
        .map(|v| v.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    (
        status,
        matched,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// `/v1/jobs/{x}` → `/v1/jobs/{}`: a template's parameter names are not part of its address.
fn shape(path: &str) -> String {
    path.split('/')
        .map(|seg| if seg.starts_with('{') { "{}" } else { seg })
        .collect::<Vec<_>>()
        .join("/")
}

/// A value of the type a schema property declares. `Option<T>` is `["T", "null"]`: the `T`.
fn sample(op: &OpSpec, field: &str, property: &Value) -> Value {
    let kind = match &property["type"] {
        Value::String(kind) => Some(kind.as_str()),
        Value::Array(kinds) => kinds
            .iter()
            .filter_map(Value::as_str)
            .find(|kind| *kind != "null"),
        _ => None,
    };
    match kind {
        Some("integer") => json!(1),
        Some("number") => json!(1.0),
        Some("string") => json!("x1"),
        Some("array") => json!([]),
        Some("boolean") => json!(false),
        other => panic!("{}.{field}: no sample for type {other:?}", op.name),
    }
}

/// The smallest input the operation's schema accepts: each required field with a value of its type.
fn minimal_input(op: &OpSpec) -> Value {
    let s = serde_json::to_value((op.input_schema)()).unwrap();
    let mut input = Map::new();
    for field in s["required"].as_array().into_iter().flatten() {
        let field = field.as_str().unwrap();
        input.insert(
            field.to_string(),
            sample(op, field, &s["properties"][field]),
        );
    }
    Value::Object(input)
}

/// The largest: every field the schema offers, optional ones included — what a model that fills
/// in everything sends.
fn full_input(op: &OpSpec) -> Value {
    let s = serde_json::to_value((op.input_schema)()).unwrap();
    let mut input = Map::new();
    for (field, property) in s["properties"].as_object().into_iter().flatten() {
        input.insert(field.clone(), sample(op, field, property));
    }
    Value::Object(input)
}

#[tokio::test]
async fn every_catalog_operation_reaches_its_route_in_todays_router() {
    let (_dir, state, _) = state();
    for op in OPERATIONS {
        for input in [minimal_input(op), full_input(op)] {
            let request = op
                .request(&input)
                .unwrap_or_else(|e| panic!("{} {input}: {e}", op.name));
            let (status, matched, body) = send(&state, &request).await;
            assert_eq!(
                matched.as_deref().map(shape),
                Some(shape(op.route_path())),
                "{} → {} {} answered {status} from no route of its own: {body}",
                op.name,
                request.method.as_str(),
                request.target
            );
            assert_ne!(status, StatusCode::METHOD_NOT_ALLOWED, "{}", op.name);
            // Matching the route is not reaching the handler. axum answers a query or body its
            // extractor cannot decode by itself — 400, 415 or 422, in plain text — before any
            // handler runs, and that used to count here as "reached". The handler took the
            // request only if the answer is a success or carries the app's own error envelope.
            assert!(
                status.is_success() || body["error"]["code"].is_string(),
                "{} → {} {} {:?} matched its route and was refused before its handler \
                 ({status}, no envelope): today's handler does not read what the catalog sends",
                op.name,
                request.method.as_str(),
                request.target,
                request.body.as_ref().map(Value::to_string)
            );
        }
    }
}

/// The allow-list gate knows every agent operation by the catalog's name for it: with one tool
/// switched off, exactly that operation's request is refused `tool_disabled`, and with only that
/// tool switched on it is the one request that is not. The gate is derived from the catalog, so
/// an operation added there is gated without anyone remembering to list it — this is the test
/// that would say so if it were not.
#[tokio::test]
async fn every_agent_operation_is_gated_under_its_own_name() {
    let (_dir, state, _) = state();
    let agent: Vec<&OpSpec> = OPERATIONS
        .iter()
        .filter(|op| op.audience == Audience::Agent)
        .collect();
    assert!(agent.len() > 20, "the catalog lost its agent operations");
    let disabled = |status: StatusCode, body: &Value| {
        status == StatusCode::FORBIDDEN && body["error"]["code"] == "tool_disabled"
    };
    for op in &agent {
        let request = op.request(&minimal_input(op)).unwrap();

        state.set_allowed(
            agent
                .iter()
                .filter(|other| other.name != op.name)
                .map(|other| other.name.to_string())
                .collect(),
        );
        let (status, _, body) = send(&state, &request).await;
        assert!(
            disabled(status, &body),
            "{} is switched off and its request was let through: {status} {body}",
            op.name
        );

        state.set_allowed([op.name.to_string()].into());
        let (status, _, body) = send(&state, &request).await;
        assert!(
            !disabled(status, &body),
            "{} is the one tool switched on and its request was refused",
            op.name
        );
    }
    // Plumbing that is not an agent operation has no switch: it is never `tool_disabled`.
    state.set_allowed(Default::default());
    for op in OPERATIONS
        .iter()
        .filter(|op| op.audience != Audience::Agent)
    {
        let request = op.request(&minimal_input(op)).unwrap();
        let (status, _, body) = send(&state, &request).await;
        assert!(!disabled(status, &body), "{} was gated as a tool", op.name);
    }
}

/// `(METHOD, path)` for every `.route(…)` in today's `router` — the router exposes no listing, so
/// its declaration is read instead. Each route sits on one line there.
fn todays_routes() -> BTreeSet<(String, String)> {
    let src = include_str!("../src/routes.rs");
    let body = src
        .split("pub fn router(")
        .nth(1)
        .and_then(|rest| rest.split("\n}\n").next())
        .expect("routes.rs still declares `pub fn router(`");
    let mut routes = BTreeSet::new();
    for line in body
        .lines()
        .filter(|l| l.trim_start().starts_with(".route("))
    {
        let path = line.split('"').nth(1).expect("a quoted path");
        let handlers = line.split('"').nth(2).unwrap_or_default();
        let methods: Vec<&str> = ["get", "post", "put", "delete", "patch"]
            .into_iter()
            .filter(|m| {
                handlers.contains(&format!(" {m}(")) || handlers.contains(&format!(".{m}("))
            })
            .collect();
        assert!(!methods.is_empty(), "no method on: {line}");
        for m in methods {
            routes.insert((m.to_uppercase(), shape(path)));
        }
    }
    assert!(routes.len() > 20, "the route parse found only {routes:?}");
    routes
}

#[test]
fn every_route_in_todays_router_is_in_the_catalog() {
    let catalog: BTreeSet<(String, String)> = OPERATIONS
        .iter()
        .filter(|op| op.audience != Audience::Ui)
        .map(|op| (op.method.as_str().to_string(), shape(op.route_path())))
        .collect();
    let routes = todays_routes();
    let missing: Vec<_> = routes.difference(&catalog).collect();
    assert!(missing.is_empty(), "routes with no operation: {missing:?}");
    let extra: Vec<_> = catalog.difference(&routes).collect();
    assert!(extra.is_empty(), "operations with no route: {extra:?}");
}

/// Field names, list fields, the v1 `q` spelling and the `mode=ai` variant all arrive where
/// today's handlers read them: requests built from agent-style input get answers that depend on
/// that input.
#[tokio::test]
async fn catalog_requests_carry_their_input_to_todays_handlers() {
    let (_dir, state, source) = state();
    let call = |name: &str, input: Value| {
        let op = lodestash_protocol::find_op(name).unwrap();
        let r = op.request(&input).unwrap();
        let state = state.clone();
        async move { send(&state, &r).await }
    };
    let names = |v: &Value| -> Vec<String> {
        v["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_string())
            .collect()
    };

    let (s, _, all) = call("search_assets", json!({})).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(names(&all).len(), 2, "{all}");
    let (s, _, found) = call("search_assets", json!({ "query": "rock" })).await;
    assert_eq!(
        (s, names(&found)),
        (StatusCode::OK, vec!["rock.png".to_string()])
    );

    let (s, _, refused) = call(
        "search_assets",
        json!({ "query": "rock", "tags": ["untagged"] }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "list fields reach the handler: {refused}"
    );

    let (s, _, ai) = call("semantic_search", json!({ "query": "rock" })).await;
    assert_eq!(
        (s, ai["error"]["code"].as_str()),
        (StatusCode::NOT_IMPLEMENTED, Some("ai_unavailable")),
        "the mode=ai variant reaches the semantic branch"
    );

    let (s, _, page) = call("list_assets", json!({ "source_id": source, "limit": 1 })).await;
    assert_eq!(s, StatusCode::OK, "{page}");
    assert_eq!(page["assets"].as_array().unwrap().len(), 1);
    assert!(page["next_cursor"].is_string(), "{page}");

    let (s, _, read) = call("get_assets", json!({ "asset_ids": [999] })).await;
    assert_eq!(s, StatusCode::OK, "{read}");
    assert_eq!(read["unavailable"][0]["id"], 999);
}
