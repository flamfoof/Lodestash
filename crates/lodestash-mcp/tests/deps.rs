//! The MCP server is a client of the app, never of the catalog (ADR-0022, v2 spec §1 and §7): it
//! must not link catalog, SQLite, engine or server code. Dev-dependencies (the end-to-end tests
//! start a real `/v1` server) are allowed; the shipped binary's tree is not.

use std::process::Command;

const FORBIDDEN: &[&str] = &[
    "rusqlite",
    "lodestash-core",
    "lodestash-engine",
    "lodestash-app",
    "lodestash-api",
    "lodestash-ingest",
    "lodestash-ai",
    "axum",
];

#[test]
fn the_mcp_server_links_no_catalog_engine_or_server_code() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let out = match Command::new(&cargo)
        .args([
            "tree",
            "--offline",
            "-e",
            "normal",
            "--prefix",
            "none",
            "--format",
            "{p}",
        ])
        .args(["--manifest-path", manifest, "-p", "lodestash-mcp"])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            println!("skipped: cannot run `{cargo} tree` ({e})");
            return;
        }
    };
    assert!(
        out.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let tree = String::from_utf8(out.stdout).unwrap();
    let packages: Vec<&str> = tree
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    for wanted in ["lodestash-mcp", "lodestash-protocol", "rmcp"] {
        assert!(packages.contains(&wanted), "{wanted} missing from:\n{tree}");
    }
    for bad in FORBIDDEN {
        assert!(
            !packages.contains(bad),
            "lodestash-mcp depends on {bad}:\n{tree}"
        );
    }
}
