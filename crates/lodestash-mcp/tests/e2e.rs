//! End to end: the built MCP binary over stdio, forwarding to today's `/v1` server on a temp
//! catalog through a temp handshake file. The app's REST answer is the tool's answer (v2 spec §7).

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use base64::Engine as _;
use common::{
    gone_process, handshake_json_for, refused_port, text_json, write_handshake, App, Behaviour,
    Mcp, RawListener, FILE, PNG, STEP,
};
use serde_json::{json, Value};

fn connected() -> (App, Mcp) {
    let app = App::start();
    app.publish();
    let mut mcp = Mcp::spawn(&app.handshake, &app.no_app());
    mcp.initialize();
    (app, mcp)
}

/// The home directory, if `path` is inside it.
fn home_of(path: &Path) -> Option<String> {
    dirs::home_dir()
        .filter(|home| path.starts_with(home))
        .map(|home| home.display().to_string())
}

/// `path` as this server shows it to a model: `~` for the home directory.
fn without_home(path: &Path) -> String {
    match dirs::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_path_buf)) {
        Some(rest) => Path::new("~").join(rest).display().to_string(),
        None => path.display().to_string(),
    }
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
        // The model reads this: the file is named without the home directory (and the OS user
        // name in it).
        assert_eq!(
            error["detail"]["handshake"],
            without_home(&handshake),
            "{case}"
        );
        if let Some(home) = home_of(&handshake) {
            for text in [&error["message"], &error["detail"]["handshake"]] {
                assert!(
                    !text.as_str().unwrap().contains(&home),
                    "{case}: the home directory is named: {text}"
                );
            }
        }
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
    assert_eq!(info["mcp"]["handshake"], without_home(&app.handshake));
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

/// A handshake left behind by an app that is gone — its process no longer exists — is not an
/// address: whoever holds its port now is sent no request and no token, and is not even
/// connected to.
#[test]
fn a_handshake_whose_process_is_gone_is_sent_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let handshake = dir.path().join("engine.json");
    let squatter = RawListener::start(Behaviour::Answer(r#"{"results":[]}"#));
    let gone = gone_process();
    std::fs::write(
        &handshake,
        handshake_json_for(squatter.port, "the-session-token", gone.pid(), "v1"),
    )
    .unwrap();
    let mut mcp = Mcp::spawn(&handshake, &dir.path().join("no-such-app.exe"));
    mcp.initialize();

    let result = mcp.call("search_assets", json!({"query": "rock"}));
    assert_eq!(result["isError"], true, "{result}");
    let error = &result["structuredContent"]["error"];
    assert_eq!(error["code"], "app_unavailable", "{result}");
    assert_eq!(error["detail"]["sent"], false, "{result}");
    assert!(
        squatter.connections().is_empty(),
        "the port's new holder was contacted: {:?}",
        squatter.requests()
    );
}

/// Never retried (ADR-0022): the app takes the request and dies with it. Exactly one request
/// went out — for a read an HTTP client would happily repeat, and for a write — and the result
/// says it was sent, because a write may have been applied.
#[test]
fn a_request_the_app_drops_is_sent_once_and_reported_as_sent() {
    let dir = tempfile::tempdir().unwrap();
    let handshake = dir.path().join("engine.json");
    let app = RawListener::start(Behaviour::Drop);
    write_handshake(&handshake, app.port, "t");
    let mut mcp = Mcp::spawn(&handshake, &dir.path().join("no-such-app.exe"));
    mcp.initialize();

    for (tool, args) in [
        ("search_assets", json!({"query": "rock"})),
        ("add_tag", json!({"asset_id": 7, "value": "rusty"})),
    ] {
        let result = mcp.call(tool, args);
        assert_eq!(result["isError"], true, "{tool}: {result}");
        let error = &result["structuredContent"]["error"];
        assert_eq!(error["code"], "app_unavailable", "{tool}: {result}");
        assert_eq!(error["detail"]["sent"], true, "{tool}: {result}");
    }
    // A retry would follow its failure at once; give one time to show.
    std::thread::sleep(Duration::from_millis(400));
    let requests = app.requests();
    assert_eq!(requests.len(), 2, "one request per call: {requests:#?}");
    assert!(requests[0].starts_with("GET /v1/search?"), "{requests:#?}");
    assert!(
        requests[1].starts_with("POST /v1/assets/7/tags "),
        "{requests:#?}"
    );
}

/// A running app that serves another API version is refused, and the request — with the token —
/// stays here. (Launching would not help: that app is the one running.)
#[test]
fn a_running_app_of_another_api_version_is_refused_and_sent_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let handshake = dir.path().join("engine.json");
    let app = RawListener::start(Behaviour::Answer("{}"));
    std::fs::write(
        &handshake,
        handshake_json_for(app.port, "t", std::process::id(), "v2"),
    )
    .unwrap();
    let mut mcp = Mcp::spawn(&handshake, &dir.path().join("no-such-app.exe"));
    mcp.initialize();

    let result = mcp.call("search_assets", json!({"query": "rock"}));
    assert_eq!(result["isError"], true, "{result}");
    let error = &result["structuredContent"]["error"];
    assert_eq!(error["code"], "app_unavailable", "{result}");
    assert_eq!(error["detail"]["sent"], false, "{result}");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("v2") && message.contains("v1"),
        "{message}"
    );
    assert!(app.requests().is_empty(), "{:?}", app.requests());
}

/// The host closed stdin while a call was waiting on the app: the server leaves now, not when
/// that call's two-minute timeout runs out. Nobody is left to read the answer.
#[test]
fn the_server_leaves_when_the_host_closes_stdin_even_mid_call() {
    let dir = tempfile::tempdir().unwrap();
    let handshake = dir.path().join("engine.json");
    let app = RawListener::start(Behaviour::Hold(Duration::from_secs(60)));
    write_handshake(&handshake, app.port, "t");
    let mut mcp = Mcp::spawn(&handshake, &dir.path().join("no-such-app.exe"));
    mcp.initialize();

    mcp.start_call("search_assets", json!({"query": "rock"}));
    assert!(app.wait_for_request(STEP), "the call never reached the app");
    let log = mcp.stderr.clone();
    let left = mcp.close_and_time_exit(Duration::from_secs(3));
    assert!(
        left.is_some(),
        "still running 3 s after stdin closed, waiting on a call nobody will read; stderr:\n{}",
        log.lock().unwrap()
    );
}

/// What a model reads names the handshake file without the home directory — and so without the
/// OS user name. The file here is under the real home directory and does not exist; nothing is
/// read from or written to it.
#[test]
fn an_error_names_the_handshake_file_without_the_home_directory() {
    let home = dirs::home_dir().expect("a home directory");
    let dir = tempfile::tempdir().unwrap();
    let folder = ".lodestash-mcp-test-no-such-folder";
    let handshake = home.join(folder).join("engine.json");
    assert!(!handshake.exists());
    let mut mcp = Mcp::spawn(&handshake, &dir.path().join("no-such-app.exe"));
    mcp.initialize();

    let result = mcp.call("search_assets", json!({"query": "rock"}));
    let error = &result["structuredContent"]["error"];
    assert_eq!(error["code"], "app_unavailable", "{result}");
    assert_eq!(
        error["detail"]["handshake"],
        Path::new("~")
            .join(folder)
            .join("engine.json")
            .display()
            .to_string()
    );
    let home = home.display().to_string();
    for text in [&error["message"], &error["detail"]["handshake"]] {
        assert!(
            !text.as_str().unwrap().contains(&home),
            "the home directory is named: {text}"
        );
    }
    assert!(!handshake.parent().unwrap().exists(), "nothing was created");
}
