//! `lodestash-desktop --mcp`, spawned the way an agent host spawns it (v1 plan S11, MCP handoff
//! Task 1): a real process, a real pipe, an isolated catalog.
//!
//! What only a spawned process can show: that stdout carries nothing but JSON-RPC while logging is
//! turned all the way up, that the server leaves on stdin EOF, and that startup refuses a catalog it
//! cannot read without touching the file.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use lodestash_core::catalog::{Catalog, SCHEMA_VERSION};

fn spawn(catalog: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_lodestash-desktop"))
        .arg("--mcp")
        .env("LODESTASH_CATALOG", catalog)
        .env("LODESTASH_CONFIG", catalog.with_file_name("config.json"))
        .env("LODESTASH_HANDSHAKE", catalog.with_file_name("engine.json"))
        // Every log line the process can emit: one of them on stdout is a malformed message.
        .env("RUST_LOG", "trace")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lodestash-desktop --mcp")
}

fn wait_exit(child: &mut Child, within: Duration) -> std::process::ExitStatus {
    let until = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().expect("wait") {
            return status;
        }
        if Instant::now() > until {
            let _ = child.kill();
            panic!("the server did not exit within {within:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Read `user_version` without opening a connection that could itself change the file: the header
/// stores it big-endian at offset 60.
fn user_version(db: &Path) -> u32 {
    let bytes = std::fs::read(db).expect("catalog bytes");
    u32::from_be_bytes([bytes[60], bytes[61], bytes[62], bytes[63]])
}

/// A missing catalog is not created, and an older one is neither served nor upgraded — both exit
/// with the instruction on stderr.
#[test]
fn a_missing_or_older_catalog_is_refused_and_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");

    let mut child = spawn(&db);
    let status = wait_exit(&mut child, Duration::from_secs(20));
    let mut stderr = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
    assert_eq!(status.code(), Some(2), "stderr: {stderr}");
    assert!(stderr.contains("no catalog on disk"), "stderr: {stderr}");
    assert!(!db.exists(), "a headless server created a catalog");

    drop(Catalog::open(&db).unwrap());
    {
        // Mark it one version behind, through the header, as an app that has not upgraded yet.
        let mut bytes = std::fs::read(&db).unwrap();
        bytes[60..64].copy_from_slice(&(SCHEMA_VERSION - 1).to_be_bytes());
        std::fs::write(&db, bytes).unwrap();
    }
    let mut child = spawn(&db);
    let status = wait_exit(&mut child, Duration::from_secs(20));
    let mut stderr = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
    assert_eq!(status.code(), Some(2), "stderr: {stderr}");
    assert!(
        stderr.contains("open the Lodestash app once"),
        "stderr: {stderr}"
    );
    assert_eq!(
        user_version(&db),
        SCHEMA_VERSION - 1,
        "the headless server migrated the catalog"
    );
}

/// initialize → initialized → tools/list → get_server_info → EOF, with every stdout line parsed as
/// JSON-RPC. The tool names are the agent operations, and server info names this catalog.
#[test]
fn the_stdio_protocol_is_clean_and_exits_on_eof() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    let catalog_id = {
        let c = Catalog::open(&db).unwrap();
        c.seed_media_types().unwrap();
        c.catalog_id().unwrap()
    };

    let mut child = spawn(&db);
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut send = |v: serde_json::Value| {
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    };
    let mut lines_seen = 0;
    let mut reply = |id: i64| -> serde_json::Value {
        loop {
            let line = rx
                .recv_timeout(Duration::from_secs(20))
                .unwrap_or_else(|_| panic!("no reply to request {id}"));
            lines_seen += 1;
            let msg: serde_json::Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line ({e}): {line}"));
            assert_eq!(msg["jsonrpc"], "2.0", "not JSON-RPC: {line}");
            if msg["id"] == id {
                return msg;
            }
        }
    };

    send(
        serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "mcp_stdio test", "version": "0"}}}),
    );
    let init = reply(1);
    assert!(
        init["result"]["protocolVersion"].is_string(),
        "no negotiated protocol version: {init}"
    );
    send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    send(serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}));
    let listed = reply(2);
    let mut names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    let mut expected: Vec<String> = lodestash_api::AGENT_OPERATIONS
        .iter()
        .map(|(n, _)| n.to_string())
        .collect();
    names.sort();
    expected.sort();
    assert_eq!(names, expected);

    send(
        serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "get_server_info", "arguments": {}}}),
    );
    let info = reply(3);
    let text = info["result"]["content"][0]["text"].as_str().expect("text");
    let body: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(body["catalog_id"], catalog_id.as_str(), "{body}");
    assert_eq!(body["schema"], SCHEMA_VERSION, "{body}");
    // No app has this fixture open, whatever else is running on the machine.
    assert_eq!(body["writer"], "app_unavailable", "{body}");

    drop(stdin); // EOF: the host went away
    let status = wait_exit(&mut child, Duration::from_secs(10));
    assert!(status.success(), "exit on EOF was {status:?}");
    // Every remaining stdout line must parse too.
    while let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
        let msg: serde_json::Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout carried a non-JSON line ({e}): {line}"));
        assert_eq!(msg["jsonrpc"], "2.0");
    }
    assert_eq!(
        user_version(&db),
        SCHEMA_VERSION,
        "the catalog's version moved"
    );
    assert!(lines_seen >= 3);
}
