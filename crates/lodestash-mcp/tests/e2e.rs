//! End to end: the built MCP binary over stdio, forwarding to today's `/v1` server on a temp
//! catalog through a temp handshake file. The app's REST answer is the tool's answer (v2 spec §7).

mod common;

use std::time::{Duration, Instant};

use base64::Engine as _;
use common::{refused_port, text_json, write_handshake, App, Mcp, FILE, PNG};
use serde_json::{json, Value};

fn connected() -> (App, Mcp) {
    let app = App::start();
    app.publish();
    let mut mcp = Mcp::spawn(&app.handshake, &app.no_app());
    mcp.initialize();
    (app, mcp)
}

/// A JSON answer is the structured content, and the same JSON as text for hosts that read only
/// text.
fn body(result: &Value) -> &Value {
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(text_json(result), result["structuredContent"], "{result}");
    &result["structuredContent"]
}

#[test]
fn search_assets_returns_the_seeded_asset() {
    let (app, mut mcp) = connected();
    let result = mcp.call("search_assets", json!({"query": "mossy"}));
    // REST's container, not a bare array: `{results: [...]}`.
    let results = body(&result)["results"].as_array().expect("results");
    assert!(
        results
            .iter()
            .any(|a| a["id"] == app.asset && a["name"] == "mossy_wall.png"),
        "{result}"
    );
    assert!(mcp.close().success());
}

/// The write and the read are separate calls: what `get_asset` shows is what the catalog holds,
/// not an echo of the write's own answer.
#[test]
fn add_tag_is_seen_by_a_separate_get_asset() {
    let (app, mut mcp) = connected();
    let before = mcp.call("get_asset", json!({"id": app.asset}));
    assert!(
        !body(&before)["tags"].to_string().contains("rusty"),
        "{before}"
    );

    let wrote = mcp.call("add_tag", json!({"asset_id": app.asset, "value": "rusty"}));
    assert_eq!(body(&wrote)["changed"], 1, "{wrote}");

    let after = mcp.call("get_asset", json!({"id": app.asset}));
    let tags = body(&after)["tags"].as_array().expect("tags");
    assert!(
        tags.iter()
            .any(|t| t["namespace"] == "free" && t["value"] == "rusty"),
        "{after}"
    );
}

/// A refusal from the app is a tool result with `isError` and the app's error envelope — a domain
/// failure the model can read, not a protocol error.
#[test]
fn a_refusal_from_the_app_is_an_error_result_with_its_envelope() {
    let (_app, mut mcp) = connected();
    let result = mcp.call("get_asset", json!({"id": 987654}));
    assert_eq!(result["isError"], true, "{result}");
    assert_eq!(result["structuredContent"]["error"]["code"], "not_found");
    assert!(result["structuredContent"]["error"]["message"].is_string());
    assert_eq!(text_json(&result), result["structuredContent"]);
}

/// Input the operation cannot take is refused before anything is sent — the handshake here points
/// at a port nothing listens on, and the answer is still `invalid`, not `app_unavailable`.
#[test]
fn input_the_operation_refuses_is_invalid_without_asking_the_app() {
    let dir = tempfile::tempdir().unwrap();
    let handshake = dir.path().join("engine.json");
    write_handshake(&handshake, refused_port(), "t");
    let mut mcp = Mcp::spawn(&handshake, &dir.path().join("no-such-app.exe"));
    mcp.initialize();
    for (tool, args) in [
        ("get_asset", json!({})),
        ("get_asset", json!({"id": "seven"})),
        (
            "write_tags",
            json!({"request_id": "r", "assets": [], "extra": 1}),
        ),
    ] {
        let result = mcp.call(tool, args.clone());
        assert_eq!(result["isError"], true, "{tool} {args}: {result}");
        assert_eq!(
            result["structuredContent"]["error"]["code"], "invalid",
            "{tool} {args}: {result}"
        );
    }
}

/// No app answering and none to launch: `app_unavailable` as a tool result, within a second — not
/// a hang, and not a protocol error.
#[test]
fn a_stopped_app_with_nothing_to_launch_is_app_unavailable_within_a_second() {
    let dir = tempfile::tempdir().unwrap();
    let handshake = dir.path().join("engine.json");
    let app = dir.path().join("no-such-app.exe");
    let mut mcp = Mcp::spawn(&handshake, &app);
    mcp.initialize();

    // The app is gone but left its handshake behind; then, no handshake at all.
    write_handshake(&handshake, refused_port(), "t");
    for case in ["stale handshake", "no handshake"] {
        let start = Instant::now();
        let result = mcp.call("search_assets", json!({"query": "rock"}));
        let took = start.elapsed();
        assert_eq!(result["isError"], true, "{case}: {result}");
        let error = &result["structuredContent"]["error"];
        assert_eq!(error["code"], "app_unavailable", "{case}: {result}");
        assert!(
            error["message"].as_str().unwrap().contains("LODESTASH_APP"),
            "{case}: the message should say which binary it tried: {result}"
        );
        assert_eq!(
            error["detail"]["handshake"],
            handshake.display().to_string(),
            "{case}"
        );
        assert!(took < Duration::from_secs(1), "{case}: took {took:?}");
        std::fs::remove_file(&handshake).ok();
    }
}

/// `get_asset_preview`'s REST answer is PNG bytes; the tool's answer is an MCP image block.
#[test]
fn a_preview_is_an_image_block() {
    let (app, mut mcp) = connected();
    let result = mcp.call("get_asset_preview", json!({"id": app.asset}));
    assert_eq!(result["isError"], false, "{result}");
    let image = &result["content"][0];
    assert_eq!(image["type"], "image", "{result}");
    assert_eq!(image["mimeType"], "image/png");
    assert_eq!(
        image["data"],
        base64::engine::general_purpose::STANDARD.encode(PNG)
    );
}

/// `get_server_info` is the app's answer plus what a host needs to debug its setup: this server's
/// version and the handshake file and port it used.
#[test]
fn server_info_names_this_server_and_the_app_it_reached() {
    let (app, mut mcp) = connected();
    let result = mcp.call("get_server_info", json!({}));
    let info = body(&result);
    assert_eq!(info["api"], "v1", "{info}");
    assert!(info["schema"].is_number(), "{info}");
    assert_eq!(info["mcp"]["version"], env!("CARGO_PKG_VERSION"), "{info}");
    assert_eq!(
        info["mcp"]["handshake"],
        app.handshake.display().to_string()
    );
    assert_eq!(info["mcp"]["port"], app.port);
}

/// `asset://{id}/file` reads the asset's bytes through the app's file route.
#[test]
fn the_asset_file_resource_reads_through_the_app() {
    let (app, mut mcp) = connected();
    let reply = mcp.request(
        "resources/read",
        json!({"uri": format!("asset://{}/file", app.asset)}),
    );
    let contents = &reply["result"]["contents"][0];
    assert_eq!(
        contents["blob"],
        base64::engine::general_purpose::STANDARD.encode(FILE),
        "{reply}"
    );
    assert_eq!(contents["uri"], format!("asset://{}/file", app.asset));

    let reply = mcp.request("resources/read", json!({"uri": "file:///etc/passwd"}));
    assert!(reply["error"].is_object(), "{reply}");
}
