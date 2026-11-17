//! Launching the app when nothing answers (v2 spec §7), through the built binary: a fake app
//! executable stands in for `lodestash-desktop --background` and publishes the handshake of a real
//! `/v1` server. The binary-resolution order and the wait budget are unit-tested in `launch.rs`.

mod common;

use std::time::Duration;

use common::{fake_app, handshake_json, App, Mcp};
use serde_json::json;

/// No handshake, so the server launches `LODESTASH_APP` with `--background` (the fake refuses to
/// run without it), waits for the handshake it publishes, and then sends the call — which reaches
/// the app.
#[test]
fn with_no_app_running_the_call_launches_one_and_goes_through() {
    let app = App::start();
    let exe = fake_app(
        app.dir.path(),
        "fake-app",
        Some(&handshake_json(app.port, &app.token)),
        0,
    );
    let mut mcp = Mcp::spawn(&app.handshake, &exe);
    mcp.initialize();
    assert!(!app.handshake.exists());

    let result = mcp.call("search_assets", json!({"query": "mossy"}));
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(
        result["structuredContent"]["results"][0]["id"], app.asset,
        "{result}"
    );
    assert!(
        app.handshake.exists(),
        "the launched app published its handshake"
    );
}

/// A launched app that fails before it publishes anything is reported at once, with its exit
/// status — not after the full wait.
#[test]
fn an_app_that_exits_with_an_error_is_reported_without_waiting_out_the_budget() {
    let app = App::start();
    let broken = app.dir.path().join(if cfg!(windows) {
        "broken.cmd"
    } else {
        "broken"
    });
    std::fs::write(
        &broken,
        if cfg!(windows) {
            "@echo off\r\nexit /b 7\r\n"
        } else {
            "#!/bin/sh\nexit 7\n"
        },
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&broken, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut mcp = Mcp::spawn(&app.handshake, &broken);
    mcp.initialize();
    let start = std::time::Instant::now();
    let result = mcp.call("list_packages", json!({}));
    assert!(start.elapsed() < Duration::from_secs(5), "{result}");
    assert_eq!(result["isError"], true, "{result}");
    let error = &result["structuredContent"]["error"];
    assert_eq!(error["code"], "app_unavailable", "{result}");
    assert!(
        error["message"].as_str().unwrap().contains("exited"),
        "{result}"
    );
}

/// The launched app outlives this server — that is the point of `--background` — so it must not
/// hold the host's pipes: when this server exits, the host sees its stdout close at once, not when
/// the app finally quits. On Windows a child inherits every inheritable handle, and the pipes a
/// host hands its server are inheritable.
#[test]
fn the_launched_app_does_not_keep_the_hosts_pipe_open() {
    let app = App::start();
    let exe = fake_app(
        app.dir.path(),
        "lingering-app",
        Some(&handshake_json(app.port, &app.token)),
        8,
    );
    let mut mcp = Mcp::spawn(&app.handshake, &exe);
    mcp.initialize();
    let result = mcp.call("list_packages", json!({}));
    assert_eq!(result["isError"], false, "{result}");

    let eof = mcp.close_and_time_stdout_eof(Duration::from_secs(4));
    assert!(
        eof.is_some(),
        "stdout stayed open after the server exited: the background app holds the host's pipe"
    );
}
