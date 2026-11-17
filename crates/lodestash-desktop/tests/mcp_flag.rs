//! `--mcp` moved to its own binary (ADR-0022). A host still configured to spawn
//! `lodestash-desktop --mcp` gets an exit and a pointer — not a GUI window it cannot see, and not a
//! second app overwriting the first one's `engine.json`.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn the_mcp_flag_exits_with_a_pointer_to_lodestash_mcp() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_lodestash-desktop"))
        .arg("--mcp")
        .env("LODESTASH_CATALOG", dir.path().join("catalog.db"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lodestash-desktop --mcp");
    // It exits before anything starts: no window, no engine, no catalog.
    let until = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > until {
            let _ = child.kill();
            panic!("lodestash-desktop --mcp did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(status.code(), Some(2), "stderr: {stderr}");
    assert!(stderr.contains("lodestash-mcp"), "stderr: {stderr}");
    assert!(out.stdout.is_empty(), "stdout must stay empty for a host");
    assert!(!dir.path().join("catalog.db").exists());
}
