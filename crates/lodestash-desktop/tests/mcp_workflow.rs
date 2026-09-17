//! Actual desktop stdio child → disposable loopback app → core writer. Never a live model/catalog.
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use lodestash_api::{Handshake, ServerState};
use lodestash_core::catalog::{
    Catalog, CrawlRecord, HistoryOrigin, TagAssignment, TagBatch, TagBatchItem, TagMode,
};
use serde_json::{json, Value};

struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<String>,
    serial: u64,
}

impl Client {
    fn start(root: &Path) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lodestash-desktop"))
            .arg("--mcp")
            .env("LODESTASH_CATALOG", root.join("catalog.db"))
            .env("LODESTASH_CONFIG", root.join("config.json"))
            .env("LODESTASH_HANDSHAKE", root.join("engine.json"))
            .env("RUST_LOG", "trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(root.join("stderr.log")).unwrap())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(output).lines() {
                if tx.send(line.unwrap()).is_err() {
                    break;
                }
            }
        });
        let mut client = Self {
            child,
            input,
            output: rx,
            serial: 0,
        };
        let init = client.request("initialize", json!({"protocolVersion":"2025-06-18", "capabilities":{}, "clientInfo":{"name":"workflow fixture","version":"1"}}));
        assert!(init["protocolVersion"].is_string());
        writeln!(
            client.input.as_mut().unwrap(),
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .unwrap();
        client
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let msg = self.rpc(method, params);
        assert!(msg.get("error").is_none(), "{msg}");
        msg["result"].clone()
    }

    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.serial += 1;
        let input = self.input.as_mut().unwrap();
        writeln!(
            input,
            "{}",
            json!({"jsonrpc":"2.0","id":self.serial,"method":method,"params":params})
        )
        .unwrap();
        input.flush().unwrap();
        loop {
            let line = self
                .output
                .recv_timeout(Duration::from_secs(30))
                .expect("MCP response deadline");
            let msg: Value =
                serde_json::from_str(&line).expect("stdout must contain only JSON-RPC");
            assert_eq!(msg["jsonrpc"], "2.0");
            if msg["id"] == self.serial {
                return msg;
            }
        }
    }

    fn call(&mut self, name: &str, args: Value) -> Value {
        let result = self.request("tools/call", json!({"name":name,"arguments":args}));
        assert_ne!(result["isError"], true, "{name}: {result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    fn settle(&mut self, job_id: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let job = self.call("get_job_status", json!({"job_id":job_id}));
            if !matches!(job["state"].as_str(), Some("queued" | "running")) {
                assert_eq!(job["state"], "done", "{job}");
                return job;
            }
            assert!(Instant::now() < deadline, "job did not finish: {job}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn approve(&mut self, state: &Arc<ServerState>, parked: Value) -> Value {
        assert_eq!(parked["state"], "awaiting_approval", "{parked}");
        let id = parked["job_id"].as_str().unwrap();
        // Only the disposable app fixture approves, never a new agent-facing permission.
        state.approve_job(id);
        self.settle(id)
    }

    fn finish(&mut self) {
        self.input.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < deadline, "stdio child ignored EOF");
            std::thread::sleep(Duration::from_millis(20));
        }
        for line in self.output.try_iter() {
            let msg: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(msg["jsonrpc"], "2.0");
        }
    }
}

fn http(h: &Handshake, method: &str, path: &str, body: &Value) -> (u16, Value) {
    let request = ureq::request(method, &format!("http://127.0.0.1:{}{path}", h.port))
        .set("authorization", &format!("Bearer {}", h.token));
    let response = if method == "GET" {
        request.call()
    } else {
        request.send_json(body.clone())
    };
    let response = match response {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(error) => panic!("fixture HTTP: {error}"),
    };
    (response.status(), response.into_json().unwrap())
}

fn assignments(ids: &[i64], source: &str, values: &[&str]) -> TagBatch {
    TagBatch {
        items: ids
            .iter()
            .map(|id| TagBatchItem {
                asset_id: *id,
                source: source.into(),
                model_id: None,
                mode: TagMode::ReplaceSource,
                tags: values
                    .iter()
                    .map(|v| TagAssignment::exact("subject", v))
                    .collect(),
                description: None,
                expected_revision: None,
            })
            .collect(),
        completes_queue: false,
        history: Some(HistoryOrigin::Agent),
        receipt: None,
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn spawned_workflow_uses_only_its_disposable_app() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    let mut catalog = Catalog::open(&db).unwrap();
    catalog.seed_media_types().unwrap();
    let catalog_id = catalog.catalog_id().unwrap();
    let source = catalog
        .upsert_import_source("Models", dir.path().to_str().unwrap(), "folder")
        .unwrap();
    let obj = b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
    for i in 0..15 {
        let name = format!("model{i}.obj");
        std::fs::write(dir.path().join(&name), obj).unwrap();
        catalog
            .record_crawl(
                source,
                None,
                &[CrawlRecord {
                    rel_path: name.clone(),
                    name,
                    media_type: "model_3d".into(),
                    size_bytes: obj.len() as i64,
                    mtime: 1,
                    workspace_rel: None,
                    content_hash: None,
                }],
            )
            .unwrap();
    }
    let all_ids = catalog.recent_asset_ids(20).unwrap();
    catalog
        .apply_tag_batch(&assignments(&all_ids, "manual", &["wrong"]))
        .unwrap();
    let writer = Arc::new(std::sync::Mutex::new(catalog));
    let mut config = serde_json::to_value(lodestash_app::config::AppConfig::default()).unwrap();
    config["auto_identify"] = json!(false);
    config["agent_producer_id"] = json!("fixture-host");
    config["mcp_bytes_enabled"] = json!(true);
    config["mcp_byte_profile"] = json!({"host":"local fixture","destination":"local","visual_bytes":true,"audio_bytes":false});
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let state = Arc::new(ServerState::new(&db, dir.path().into(), Box::new(|_| None)).unwrap());
    state.set_agent_producer(Some("fixture-host".into()));
    state.set_byte_release(true);
    state.set_byte_profile(serde_json::from_value(config["mcp_byte_profile"].clone()).unwrap());
    state.set_write(Box::new({
        let writer = writer.clone();
        move |operation| {
            use lodestash_api::AgentWrite;
            let mut cat = writer.lock().unwrap();
            match operation {
                AgentWrite::Identify { asset_ids } => cat
                    .apply_tag_batch(&assignments(&asset_ids, "ai:stub", &["tower", "wrong"]))
                    .map(|out| out.changed_assets.len())
                    .map_err(|e| e.to_string()),
                AgentWrite::TagBatch { request, digest } => cat
                    .apply_tag_batch_guarded(
                        &request.to_core_batch(&digest),
                        Some((
                            request.vocabulary_revision.as_deref().unwrap(),
                            "extend_with_approval",
                        )),
                    )
                    .map(|out| out.changed_assets.len())
                    .map_err(|e| e.to_string()),
                AgentWrite::TagReview {
                    batch,
                    vocabulary_revision,
                } => cat
                    .apply_tag_review_guarded(
                        &batch,
                        Some((&vocabulary_revision, "extend_with_approval")),
                    )
                    .map(|out| out.changed_assets.len())
                    .map_err(|e| e.to_string()),
                AgentWrite::VocabularyProposal { proposal, digest } => cat
                    .apply_vocabulary_proposal(&proposal, "extend_with_approval", &digest)
                    .map(|_| 1)
                    .map_err(|e| e.to_string()),
                other => Err(format!("unexpected fixture operation {other:?}")),
            }
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let handshake = Handshake::new(listener.local_addr().unwrap().port(), state.token())
        .with_catalog(catalog_id);
    std::fs::write(
        dir.path().join("engine.json"),
        serde_json::to_vec(&handshake).unwrap(),
    )
    .unwrap();
    let server = tokio::spawn({
        let state = state.clone();
        async move {
            axum::serve(listener, lodestash_api::router(state))
                .await
                .unwrap()
        }
    });
    let mut client = Client::start(dir.path());
    let info = client.call("get_server_info", json!({}));
    assert_eq!(info["writer"], "app", "{info}");
    assert_eq!(info["bytes_released"], true);
    // Every new operation uses the same validator across the real stdio/HTTP boundary.
    let cases = [
        (
            "list_libraries",
            "GET",
            "/v1/libraries?cursor=bad".into(),
            json!({"cursor":"bad"}),
        ),
        (
            "list_assets",
            "GET",
            format!("/v1/assets?source_id={source}&selection=bad"),
            json!({"source_id":source,"selection":"bad"}),
        ),
        (
            "get_assets",
            "POST",
            "/v1/assets/read".into(),
            json!({"asset_ids":[]}),
        ),
        (
            "get_asset_evidence",
            "GET",
            format!("/v1/assets/{}/evidence?kind=bad", all_ids[0]),
            json!({"id":all_ids[0],"kind":"bad"}),
        ),
        (
            "get_vocabulary",
            "GET",
            "/v1/vocabulary?media_type=bad".into(),
            json!({"media_type":"bad"}),
        ),
        (
            "write_tags",
            "POST",
            "/v1/tags/batch".into(),
            json!({"request_id":"bad","assets":[]}),
        ),
        (
            "review_tags",
            "POST",
            "/v1/tags/review".into(),
            json!({"request_id":"bad","vocabulary_revision":"bad","assets":[]}),
        ),
        (
            "propose_vocabulary_changes",
            "POST",
            "/v1/vocabulary/proposals".into(),
            json!({"request_id":"bad","expected_revision":"bad","changes":[]}),
        ),
    ];
    let history_count = writer.lock().unwrap().recent_ops(100).unwrap().len();
    for (name, method, path, args) in &cases {
        let (status, rest) = http(&handshake, method, path, args);
        let mcp = client.request("tools/call", json!({"name":name,"arguments":args}));
        assert_eq!(mcp["isError"], true, "{name}: {mcp}");
        let body: Value =
            serde_json::from_str(mcp["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(body["error"]["code"], rest["error"]["code"], "{name}");
        assert_eq!(body["error"]["status"], status, "{name}");
    }
    assert_eq!(
        writer.lock().unwrap().recent_ops(100).unwrap().len(),
        history_count
    );
    assert_eq!(
        client.call("list_libraries", json!({}))["libraries"][0]["id"],
        source
    );
    let search = client.call(
        "search_assets",
        json!({"query":"wrong", "source_id":source,"media_type":"model_3d"}),
    );
    assert!(!search.as_array().unwrap().is_empty(), "{search}");
    let selected = client.call(
        "list_assets",
        json!({"source_id":source,"media_type":"model_3d","selection":"random","limit":10}),
    );
    let rows = selected["assets"].as_array().unwrap();
    let ids: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        10
    );
    assert!(rows
        .iter()
        .all(|r| r["source_id"] == source && r["media_type"] == "model_3d"));
    assert_eq!(client.call("get_asset", json!({"id":ids[0]}))["id"], ids[0]);
    let before = client.call("get_assets", json!({"asset_ids":ids,"source_id":source}));
    for (name, method, path, args) in [
        ("list_libraries", "GET", "/v1/libraries".into(), json!({})),
        (
            "list_assets",
            "GET",
            format!("/v1/assets?source_id={source}&limit=5"),
            json!({"source_id":source,"limit":5}),
        ),
        (
            "get_assets",
            "POST",
            "/v1/assets/read".into(),
            json!({"asset_ids":ids,"source_id":source}),
        ),
        (
            "get_asset_evidence",
            "GET",
            format!("/v1/assets/{}/evidence?kind=text", ids[0]),
            json!({"id":ids[0],"kind":"text"}),
        ),
        (
            "get_vocabulary",
            "GET",
            "/v1/vocabulary?media_type=model_3d".into(),
            json!({"media_type":"model_3d"}),
        ),
    ] {
        let (status, rest) = http(&handshake, method, &path, &args);
        assert_eq!(status, 200, "{name}: {rest}");
        assert_eq!(client.call(name, args), rest, "{name} output drifted");
    }
    for id in &ids {
        let result = client.request("tools/call", json!({"name":"get_asset_evidence","arguments":{"id":id,"kind":"model_views","views":2,"max_dimension":32}}));
        assert_ne!(result["isError"], true, "{result}");
        let manifest: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(
            matches!(manifest["status"].as_str(), Some("ready" | "partial")),
            "{manifest}"
        );
        assert_eq!(
            result["content"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|c| c["type"] == "image")
                .count(),
            2
        );
    }
    let resource = client.request(
        "resources/read",
        json!({"uri":format!("asset://{}/file",ids[0])}),
    );
    assert!(resource["contents"][0]["blob"]
        .as_str()
        .is_some_and(|s| !s.is_empty()));
    let started = client.call("identify_asset", json!({"asset_ids":ids}));
    client.settle(started["job_id"].as_str().unwrap());
    let vocabulary = client.call("get_vocabulary", json!({"media_type":"model_3d"}));
    let proposal = json!({"request_id":"70000000-0000-4000-8000-000000000001","expected_revision":vocabulary["revision"],"changes":[{"kind":"trait","namespace":"subject","value":"tower","scope":"model_3d","teach":false}]});
    let parked = client.call("propose_vocabulary_changes", proposal.clone());
    let approved = client.approve(&state, parked);
    assert_eq!(
        http(&handshake, "POST", "/v1/vocabulary/proposals", &proposal).1["result"],
        approved["result"]
    );
    assert_eq!(
        client.call("propose_vocabulary_changes", proposal)["result"],
        approved["result"]
    );
    let vocabulary = client.call("get_vocabulary", json!({"media_type":"model_3d"}));
    let identified = client.call("get_assets", json!({"asset_ids":ids,"source_id":source}));
    let review = json!({"request_id":"70000000-0000-4000-8000-000000000002","vocabulary_revision":vocabulary["revision"],"assets":identified["assets"].as_array().unwrap().iter().map(|a| json!({
        "asset_id":a["id"],"expected_revision":a["annotation_revision"],"decisions":[
            {"namespace":"subject","value":"tower","source":"ai:stub","action":"accept","reason":"Fixture result matches trait"},
            {"namespace":"subject","value":"wrong","source":"ai:stub","action":"reject","reason":"Fixture rejects this term"}
        ]})).collect::<Vec<_>>()});
    let parked = client.call("review_tags", review.clone());
    let reviewed = client.approve(&state, parked);
    assert_eq!(
        http(&handshake, "POST", "/v1/tags/review", &review).1["result"],
        reviewed["result"]
    );
    let fresh = client.call("get_assets", json!({"asset_ids":ids,"source_id":source}));
    for asset in fresh["assets"].as_array().unwrap() {
        assert!(asset["tag_reviews"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["matches_current_revision"] == true));
    }
    let write = json!({"request_id":"70000000-0000-4000-8000-000000000003","vocabulary_revision":vocabulary["revision"],"assets":fresh["assets"].as_array().unwrap().iter().map(|a| json!({
        "asset_id":a["id"],"expected_revision":a["annotation_revision"],"producer_id":"fixture-host","mode":"replace_producer",
        "tags":[{"namespace":"subject","value":"tower"}],"description":"Synthetic workflow fixture."
    })).collect::<Vec<_>>()});
    let parked = client.call("write_tags", write.clone());
    let committed = client.approve(&state, parked);
    let op = committed["result"]["operation_id"].as_i64().unwrap();
    assert_eq!(
        http(&handshake, "POST", "/v1/tags/batch", &write).1["result"]["operation_id"],
        op
    );
    assert_eq!(
        client.call("write_tags", write)["result"]["operation_id"],
        op
    );
    let after = client.call("get_assets", json!({"asset_ids":ids,"source_id":source}));
    assert_eq!(
        after["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    let independent = Catalog::open_reader(&db).unwrap();
    for (index, id) in ids.iter().enumerate() {
        let tags = independent.asset_tag_rows(*id).unwrap();
        let mut tuples: Vec<_> = tags
            .iter()
            .map(|t| (t.source.as_str(), t.namespace.as_str(), t.value.as_str()))
            .collect();
        tuples.sort();
        assert_eq!(
            tuples,
            vec![
                ("ai:fixture-host", "subject", "tower"),
                ("ai:stub", "subject", "tower"),
                ("manual", "subject", "wrong")
            ]
        );
        assert_eq!(
            independent.asset_description(*id).unwrap().as_deref(),
            Some("Synthetic workflow fixture.")
        );
        assert_eq!(
            after["assets"][index]["status"], before["assets"][index]["status"],
            "agent writes cannot confirm assets"
        );
    }
    writer.lock().unwrap().revert_op(op).unwrap();
    let undone = client.call("get_assets", json!({"asset_ids":ids,"source_id":source}));
    for (index, id) in ids.iter().enumerate() {
        assert_eq!(
            undone["assets"][index]["description"],
            fresh["assets"][index]["description"]
        );
        assert!(!independent
            .asset_tag_rows(*id)
            .unwrap()
            .iter()
            .any(|t| t.source == "ai:fixture-host"));
    }
    for i in 0..15 {
        assert_eq!(
            std::fs::read(dir.path().join(format!("model{i}.obj"))).unwrap(),
            obj
        );
    }
    let rerun = client.call("identify_asset", json!({"asset_ids":ids}));
    client.settle(rerun["job_id"].as_str().unwrap());
    assert_eq!(
        client.call("review_tags", review)["result"],
        reviewed["result"],
        "old receipt is replayed, not re-applied"
    );
    for id in &ids {
        assert!(independent
            .asset_tag_rows(*id)
            .unwrap()
            .iter()
            .any(|t| t.source == "ai:stub" && t.value == "wrong"));
        let detail = client.call("get_asset", json!({"id":id}));
        assert!(detail["tag_reviews"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["matches_current_revision"] == false));
    }
    client.finish();
    // Disabled tools normalize to the same refusal while keeping each transport's envelope.
    config["mcp_disabled_tools"] = json!(cases.iter().map(|c| c.0).collect::<Vec<_>>());
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    state.set_allowed(std::collections::HashSet::new());
    let mut disabled = Client::start(dir.path());
    for (name, method, path, args) in &cases {
        let (status, rest) = http(&handshake, method, path, args);
        assert_eq!(status, 403, "{name}");
        assert_eq!(rest["error"]["code"], "tool_disabled");
        let mcp = disabled.rpc("tools/call", json!({"name":name,"arguments":args}));
        assert_eq!(mcp["error"]["code"], -32602, "{name}: {mcp}");
        assert!(mcp["error"]["message"]
            .as_str()
            .unwrap()
            .contains("disabled"));
    }
    disabled.finish();
    server.abort();
    let _ = server.await;
}
