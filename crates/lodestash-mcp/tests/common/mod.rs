//! Test support: the MCP binary over real stdio JSON-RPC, today's `/v1` server on a temp catalog,
//! and fake app executables for the launcher.
//!
//! Nothing here reads or writes the user's real `engine.json`: every spawned server is pointed at a
//! temp handshake file with `LODESTASH_ENGINE_JSON`, and at an app binary of the test's choosing
//! with `LODESTASH_APP` — so a test can never launch the real desktop app sitting next to the MCP
//! binary in `target/`.
// Each test binary uses its own part of this module.
#![allow(dead_code, unused_imports)]

pub mod fake_app;
pub mod raw;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub use fake_app::{gone_process, handshake_json_for, launches, FakeApp};
pub use raw::{Behaviour, RawListener};
use serde_json::{json, Value};

/// The PNG the test app serves as every asset's preview (only the signature is real — the route
/// sends whatever bytes the preview resolver names).
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\nlodestash test preview";
/// The seeded asset's source file.
pub const FILE: &[u8] = b"mossy wall pixels";
/// The longest any one step may take before it counts as a hang (the project's 5 s rule). Every
/// reply here takes milliseconds; a launch in these tests runs a script that publishes at once.
pub const STEP: Duration = Duration::from_secs(5);

/// `lodestash-mcp`, spawned the way a host spawns it.
pub struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    pub stderr: Arc<Mutex<String>>,
    next_id: i64,
}

impl Mcp {
    /// Spawn the binary against `handshake`, launching `app` if it has to. Logging is turned all
    /// the way up: a single log line on stdout is a malformed message, and `request` fails on it.
    pub fn spawn(handshake: &Path, app: &Path) -> Mcp {
        let mut child = Command::new(env!("CARGO_BIN_EXE_lodestash-mcp"))
            .env("LODESTASH_ENGINE_JSON", handshake)
            .env("LODESTASH_APP", app)
            .env("RUST_LOG", "trace")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn lodestash-mcp");
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        // Drained, so a chatty trace log can never fill the pipe and stall the server.
        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = stderr.clone();
        let mut err = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                sink.lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
        Mcp {
            stdin: child.stdin.take(),
            child,
            lines,
            stderr,
            next_id: 1,
        }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// One request and its reply, within [`STEP`]. Every stdout line on the way must be a JSON-RPC
    /// message.
    pub fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let until = Instant::now() + STEP;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).unwrap_or_else(|_| {
                panic!(
                    "no reply to {method} #{id}; stderr:\n{}",
                    self.stderr.lock().unwrap()
                )
            });
            let message: Value = serde_json::from_str(&line)
                .unwrap_or_else(|e| panic!("stdout carried a non-JSON line ({e}): {line}"));
            assert_eq!(message["jsonrpc"], "2.0", "not JSON-RPC: {line}");
            if message["id"] == id {
                return message;
            }
        }
    }

    /// Send `tools/call` and return without waiting for its answer.
    pub fn start_call(&mut self, tool: &str, arguments: Value) {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
                         "params": {"name": tool, "arguments": arguments}}));
    }

    /// Close stdin (the host went away) and report how long the process took to exit, or `None`
    /// if it was still running after `within` (it is then killed).
    pub fn close_and_time_exit(mut self, within: Duration) -> Option<Duration> {
        let start = Instant::now();
        drop(self.stdin.take());
        loop {
            if self.child.try_wait().unwrap().is_some() {
                return Some(start.elapsed());
            }
            if start.elapsed() > within {
                let _ = self.child.kill();
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// The MCP handshake: `initialize`, then `notifications/initialized`.
    pub fn initialize(&mut self) -> Value {
        let reply = self.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {},
                   "clientInfo": {"name": "lodestash-mcp tests", "version": "0"}}),
        );
        assert!(
            reply["result"]["protocolVersion"].is_string(),
            "no negotiated protocol version: {reply}"
        );
        self.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        reply["result"].clone()
    }

    /// `tools/call`; the tool result (a JSON-RPC error fails the test).
    pub fn call(&mut self, tool: &str, arguments: Value) -> Value {
        let reply = self.request("tools/call", json!({"name": tool, "arguments": arguments}));
        assert!(
            reply.get("error").is_none(),
            "{tool} answered a protocol error: {reply}"
        );
        reply["result"].clone()
    }

    /// Close stdin (the host went away) and wait for the server to leave.
    pub fn close(mut self) -> ExitStatus {
        drop(self.stdin.take());
        let until = Instant::now() + STEP;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                // Whatever is left on stdout must be JSON-RPC too.
                while let Ok(line) = self.lines.recv_timeout(Duration::from_millis(100)) {
                    let message: Value = serde_json::from_str(&line)
                        .unwrap_or_else(|e| panic!("stdout carried a non-JSON line ({e}): {line}"));
                    assert_eq!(message["jsonrpc"], "2.0");
                }
                return status;
            }
            if Instant::now() > until {
                let _ = self.child.kill();
                panic!("lodestash-mcp did not exit on stdin EOF");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Close stdin and report how long stdout took to reach EOF — the moment a host reading the
    /// pipe sees the server gone.
    pub fn close_and_time_stdout_eof(mut self, within: Duration) -> Option<Duration> {
        let start = Instant::now();
        drop(self.stdin.take());
        let until = start + within;
        loop {
            let left = until.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = self.child.wait();
                    return Some(start.elapsed());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    let _ = self.child.kill();
                    return None;
                }
            }
        }
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The JSON body a tool result carries as its first text block.
pub fn text_json(result: &Value) -> Value {
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("no text block: {result}"));
    serde_json::from_str(text).unwrap_or_else(|e| panic!("text is not JSON ({e}): {text}"))
}

/// Write a v1 handshake for `port`/`token` to `path`.
pub fn write_handshake(path: &Path, port: u16, token: &str) {
    std::fs::write(path, handshake_json(port, token)).unwrap();
}

pub fn handshake_json(port: u16, token: &str) -> String {
    serde_json::to_string(&lodestash_protocol::Handshake {
        version: 1,
        port,
        token: token.into(),
        pid: std::process::id(),
        api_version: "v1".into(),
        catalog_id: None,
        exe: None,
    })
    .unwrap()
}

/// A loopback port nothing listens on: bound, then released.
pub fn refused_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

/// Today's app, minus the window: the `/v1` router over a temp catalog seeded with one asset, on a
/// listener this test bound — `lodestash_api::serve` is not used because it writes the platform
/// handshake file.
pub struct App {
    pub dir: tempfile::TempDir,
    pub port: u16,
    pub token: String,
    /// `mossy_wall.png`'s id.
    pub asset: i64,
    /// Where this app's handshake goes. Not written until a test asks (`publish`).
    pub handshake: PathBuf,
    /// The app's side of things a test plays the person for: approving a parked job, registering
    /// an editor, reading the import queue.
    pub state: Arc<lodestash_api::ServerState>,
}

impl App {
    pub fn start() -> App {
        use lodestash_core::catalog::{Catalog, CrawlRecord};

        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        {
            let mut c = Catalog::open(&db).unwrap();
            c.seed_media_types().unwrap();
            let src = c
                .upsert_import_source("Lib", dir.path().to_str().unwrap(), "folder")
                .unwrap();
            c.record_crawl(
                src,
                None,
                &[CrawlRecord {
                    workspace_rel: None,
                    content_hash: None,
                    rel_path: "mossy_wall.png".into(),
                    name: "mossy_wall.png".into(),
                    media_type: "image".into(),
                    size_bytes: FILE.len() as i64,
                    mtime: 1,
                }],
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("mossy_wall.png"), FILE).unwrap();
        let preview = dir.path().join("preview.png");
        std::fs::write(&preview, PNG).unwrap();

        let state = lodestash_api::ServerState::new(
            &db,
            dir.path().to_path_buf(),
            Box::new(move |_| Some(preview.clone())),
        )
        .unwrap();
        state.set_byte_release(true);
        state.set_write(writer(db.clone()));
        let asset = state.search("mossy", 10, &Default::default()).unwrap()[0].id;
        let token = state.token().to_string();
        let state = Arc::new(state);
        let served = state.clone();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, lodestash_api::router(served))
                    .await
                    .unwrap();
            });
        });
        let handshake = dir.path().join("engine.json");
        App {
            dir,
            port,
            token,
            asset,
            handshake,
            state,
        }
    }

    /// Write this app's handshake where the MCP server looks.
    pub fn publish(&self) {
        write_handshake(&self.handshake, self.port, &self.token);
    }

    /// An app binary that does not exist — for tests that must never launch anything.
    pub fn no_app(&self) -> PathBuf {
        self.dir.path().join("no-such-app.exe")
    }
}

/// The desktop's single writer, reduced to what these tests write: `add_tag` lands as the legacy
/// agent producer, the way the desktop's write bridge maps it.
fn writer(db: PathBuf) -> lodestash_api::AgentWriteFn {
    use lodestash_core::catalog::{
        ai_source, Catalog, HistoryOrigin, TagAssignment, TagBatch, TagBatchItem, TagMode,
    };
    Box::new(move |w| match w {
        lodestash_api::AgentWrite::AddTag {
            asset_id,
            namespace,
            value,
        } => Catalog::open(&db)
            .and_then(|mut c| {
                c.apply_tag_batch(&TagBatch {
                    items: vec![TagBatchItem {
                        asset_id,
                        source: ai_source("legacy-agent"),
                        model_id: None,
                        mode: TagMode::Append,
                        tags: vec![TagAssignment {
                            namespace,
                            value,
                            confidence: None,
                        }],
                        description: None,
                        expected_revision: None,
                    }],
                    completes_queue: false,
                    history: Some(HistoryOrigin::Agent),
                    receipt: None,
                })
            })
            .map(|o| o.changed_assets.len())
            .map_err(|e| e.to_string()),
        lodestash_api::AgentWrite::RegisterProject {
            project_path,
            unity_version,
            pipeline,
        } => Catalog::open(&db)
            .and_then(|c| {
                c.register_project(&project_path, unity_version.as_deref(), pipeline.as_deref())
            })
            .map(|_| 1)
            .map_err(|e| e.to_string()),
        other => Err(format!("the test writer does not do {other:?}")),
    })
}

/// A stand-in for the app binary that publishes `handshake` (if any) at once and then stays alive
/// for `linger_secs`, as a background app would. See [`FakeApp`] for the other things one can do.
pub fn fake_app(dir: &Path, name: &str, handshake: Option<&str>, linger_secs: u32) -> PathBuf {
    FakeApp {
        handshake: handshake.map(str::to_string),
        linger_secs,
        ..FakeApp::default()
    }
    .write(dir, name)
}
