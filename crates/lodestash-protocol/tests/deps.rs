//! The protocol is wire types only (v2 spec §3): the MCP binary links it, and it must not drag the
//! catalog, the engine, a server or a runtime in with it.

use std::process::Command;

const FORBIDDEN: &[&str] = &[
    "rusqlite",
    "lodestash-core",
    "lodestash-engine",
    "lodestash-app",
    "lodestash-api",
    "axum",
    "tokio",
];

#[test]
fn the_protocol_links_no_catalog_engine_or_server_code() {
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
        .args(["--manifest-path", manifest, "-p", "lodestash-protocol"])
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
    assert!(
        packages.contains(&"lodestash-protocol"),
        "unexpected tree:\n{tree}"
    );
    for bad in FORBIDDEN {
        assert!(
            !packages.contains(bad),
            "lodestash-protocol depends on {bad}:\n{tree}"
        );
    }
}
