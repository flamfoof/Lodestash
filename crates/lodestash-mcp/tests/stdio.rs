//! The server as a host sees it: the built binary over real stdio JSON-RPC. Listing tools needs no
//! app, so none is running here — and the handshake and app binary point at nothing, so nothing
//! is launched either.

mod common;

use std::collections::BTreeMap;

use common::Mcp;
use lodestash_protocol::{Audience, Effect, OPERATIONS};
use serde_json::{json, Value};

fn nowhere() -> (tempfile::TempDir, Mcp) {
    let dir = tempfile::tempdir().unwrap();
    let mcp = Mcp::spawn(
        &dir.path().join("engine.json"),
        &dir.path().join("no-such-app.exe"),
    );
    (dir, mcp)
}

/// `tools/list` is the catalog: exactly its `Agent` operations, each with the operation's summary
/// as its description and the operation's input schema as its input schema — one page, nothing
/// added, nothing missing.
#[test]
fn tools_list_is_the_catalogs_agent_operations() {
    let (_dir, mut mcp) = nowhere();
    let init = mcp.initialize();
    assert_eq!(init["serverInfo"]["name"], "lodestash");
    assert_eq!(init["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(init["capabilities"]["tools"].is_object(), "{init}");

    let reply = mcp.request("tools/list", json!({}));
    assert!(
        reply["result"]["nextCursor"].is_null(),
        "one page holds every tool"
    );
    let tools: BTreeMap<String, Value> = reply["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|t| (t["name"].as_str().unwrap().to_string(), t.clone()))
        .collect();

    let agent: BTreeMap<&str, _> = OPERATIONS
        .iter()
        .filter(|op| op.audience == Audience::Agent)
        .map(|op| (op.name, op))
        .collect();
    assert_eq!(
        tools.keys().map(String::as_str).collect::<Vec<_>>(),
        agent.keys().copied().collect::<Vec<_>>(),
        "tool names"
    );
    for (name, op) in agent {
        let tool = &tools[name];
        assert_eq!(tool["description"], op.summary, "{name} description");
        assert_eq!(
            tool["inputSchema"],
            serde_json::to_value((op.input_schema)()).unwrap(),
            "{name} input schema"
        );
        let hints = &tool["annotations"];
        assert_eq!(
            hints["readOnlyHint"],
            op.effect == Effect::Read,
            "{name} readOnlyHint"
        );
        if op.effect != Effect::Read {
            assert_eq!(
                hints["destructiveHint"],
                op.effect == Effect::Gated,
                "{name} destructiveHint"
            );
        }
        assert_eq!(hints["openWorldHint"], false, "{name} openWorldHint");
    }

    assert!(mcp.close().success(), "exit on stdin EOF");
}

/// A tool name the catalog does not have is a protocol error — the request cannot be routed — not
/// a tool result.
#[test]
fn an_unknown_tool_is_a_protocol_error() {
    let (_dir, mut mcp) = nowhere();
    mcp.initialize();
    let reply = mcp.request(
        "tools/call",
        json!({"name": "drop_tables", "arguments": {}}),
    );
    assert!(reply["result"].is_null(), "{reply}");
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap()
            .contains("drop_tables"),
        "{reply}"
    );
    // `import_queue` is an operation, but the Unity plugin's — not a tool.
    let reply = mcp.request(
        "tools/call",
        json!({"name": "import_queue", "arguments": {"project_id": 1}}),
    );
    assert!(reply["error"].is_object(), "{reply}");
}

/// The one resource shape the summaries advertise (`file_resource_uri`, ENGINE-API §8.1–8.2).
#[test]
fn the_asset_file_resource_template_is_listed() {
    let (_dir, mut mcp) = nowhere();
    let init = mcp.initialize();
    assert!(init["capabilities"]["resources"].is_object(), "{init}");
    let reply = mcp.request("resources/templates/list", json!({}));
    let templates = reply["result"]["resourceTemplates"].as_array().unwrap();
    assert_eq!(templates.len(), 1, "{reply}");
    assert_eq!(templates[0]["uriTemplate"], "asset://{id}/file");
}
