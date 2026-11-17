//! The catalog's `Agent` operations are today's MCP tools: same names, same descriptions, same
//! input properties (v2 plan Task 2.1). The tool list is read from the running server over
//! JSON-RPC — what a host sees — so P4's catalog-derived server can be held to the same answer.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lodestash_api::ServerState;
use lodestash_protocol::{Audience, OPERATIONS};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// `tools/list` from today's server, by name.
fn todays_tools() -> BTreeMap<String, Value> {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    lodestash_core::catalog::Catalog::open(&db).unwrap();
    let state =
        Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move {
        let (client, server) = tokio::io::duplex(1 << 20);
        let _server = tokio::spawn(async move {
            use rmcp::ServiceExt;
            let running = lodestash_mcp::Lodestash::new(state)
                .serve(tokio::io::split(server))
                .await
                .expect("serve");
            let _ = running.waiting().await;
        });
        let (read, mut write) = tokio::io::split(client);
        let mut lines = BufReader::new(read).lines();
        let wait = std::time::Duration::from_secs(5);
        let send = |v: Value| {
            let mut line = v.to_string();
            line.push('\n');
            line
        };
        write
            .write_all(
                send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                           "clientInfo": {"name": "parity", "version": "0"}}}))
                .as_bytes(),
            )
            .await
            .unwrap();
        tokio::time::timeout(wait, lines.next_line())
            .await
            .unwrap()
            .unwrap();
        write
            .write_all(
                send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).as_bytes(),
            )
            .await
            .unwrap();
        write
            .write_all(send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).as_bytes())
            .await
            .unwrap();
        let line = tokio::time::timeout(wait, lines.next_line())
            .await
            .expect("tools/list answered within 5 s")
            .unwrap()
            .expect("a reply");
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], 2, "{reply}");
        assert!(
            reply["result"]["nextCursor"].is_null(),
            "one page holds every tool"
        );
        reply["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["name"].as_str().unwrap().to_string(), t.clone()))
            .collect()
    })
}

fn words(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn keys(schema: &Value, field: &str) -> BTreeSet<String> {
    match &schema[field] {
        Value::Object(m) => m.keys().cloned().collect(),
        Value::Array(a) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => BTreeSet::new(),
    }
}

#[test]
fn agent_operations_are_exactly_todays_tools() {
    let tools = todays_tools();
    let agent: BTreeMap<&str, &str> = OPERATIONS
        .iter()
        .filter(|op| op.audience == Audience::Agent)
        .map(|op| (op.name, op.summary))
        .collect();
    assert_eq!(
        tools.keys().map(String::as_str).collect::<Vec<_>>(),
        agent.keys().copied().collect::<Vec<_>>(),
        "tool names"
    );
    for (name, summary) in agent {
        let description = tools[name]["description"].as_str().unwrap_or_default();
        assert_eq!(words(summary), words(description), "{name}'s description");
    }
}

/// Every property a tool takes today, the operation's input takes, and the same ones are required —
/// so any arguments an agent sends now remain valid input.
#[test]
fn each_tool_takes_the_same_properties_as_its_operation() {
    let tools = todays_tools();
    for op in OPERATIONS
        .iter()
        .filter(|op| op.audience == Audience::Agent)
    {
        let tool = &tools[op.name]["inputSchema"];
        let ours = serde_json::to_value((op.input_schema)()).unwrap();
        assert_eq!(
            keys(&ours, "properties"),
            keys(tool, "properties"),
            "{} properties",
            op.name
        );
        assert_eq!(
            keys(&ours, "required"),
            keys(tool, "required"),
            "{} required",
            op.name
        );
    }
}
