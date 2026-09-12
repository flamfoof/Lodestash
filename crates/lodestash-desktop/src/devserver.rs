//! The dev frontend server, started by the app that needs it (debug builds only).
//!
//! A debug build loads `devUrl` from `tauri.conf.json` (`http://localhost:5173`), so the window is
//! blank — "Hmmm… can't reach this page" — unless Vite is already listening. That made launching
//! the app a two-command ritual, and every way *into* the binary that skipped the ritual (an IDE
//! run button, `cargo run`, the built exe) produced the same dead page.
//!
//! So the binary owns it: probe the port, start Vite when nothing answers, wait for it, and take
//! down only what this process started. A release build serves `ui/dist` from the bundle and never
//! comes through here.

use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where a debug build's `devUrl` points. Kept in step with `tauri.conf.json` by
/// `dev_url_matches_tauri_conf` below rather than by memory.
///
/// The host is resolved rather than hard-coded to `127.0.0.1`: Vite binds `localhost`, which on
/// Windows is `::1` first and **only** — an IPv4 probe reports a server that is plainly running as
/// down, and then a second one is started over it.
const DEV_ADDR: &str = "localhost:5173";

/// The Vite this process started, if it started one. A server that was already running belongs to
/// whoever started it and is left alone on exit.
static SPAWNED: Mutex<Option<Child>> = Mutex::new(None);

fn listening() -> bool {
    DEV_ADDR
        .to_socket_addrs()
        .map(|addrs| {
            addrs
                .into_iter()
                .any(|a| TcpStream::connect_timeout(&a, Duration::from_millis(250)).is_ok())
        })
        .unwrap_or(false)
}

/// Make sure the dev server is up before the window opens. Returns once the port answers, or after
/// the wait runs out — a window showing Vite's own error is more useful than no window at all.
pub fn ensure() {
    if listening() {
        tracing::info!("dev server already listening on {DEV_ADDR}");
        return;
    }
    let ui = match std::env::var("CARGO_MANIFEST_DIR") {
        // Running from a build tree: the UI sits beside this crate's manifest.
        Ok(dir) => std::path::PathBuf::from(dir).join("ui"),
        // Running the exe on its own: walk up out of `target/<profile>/`.
        Err(_) => match std::env::current_exe().ok().and_then(|exe| {
            exe.ancestors()
                .nth(3)
                .map(|root| root.join("crates/lodestash-desktop/ui"))
        }) {
            Some(dir) => dir,
            None => return,
        },
    };
    if !ui.join("package.json").is_file() {
        tracing::warn!(?ui, "no frontend to serve; leaving the dev server alone");
        return;
    }
    // Bun is a shim rather than a plain exe on most Windows installs, so go through `cmd.exe` —
    // the same shape `scripts/dev.mjs` and the e2e harness use.
    let child = if cfg!(windows) {
        Command::new("cmd.exe")
            .args(["/c", "bun", "run", "dev"])
            .current_dir(&ui)
            .spawn()
    } else {
        Command::new("bun")
            .args(["run", "dev"])
            .current_dir(&ui)
            .spawn()
    };
    match child {
        Ok(child) => *SPAWNED.lock().expect("dev server lock") = Some(child),
        Err(e) => {
            tracing::error!(%e, ?ui, "could not start the dev server — the window will be blank");
            return;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if listening() {
            tracing::info!("dev server up on {DEV_ADDR}");
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    tracing::error!("dev server did not answer on {DEV_ADDR} within 60s");
}

/// Stop the dev server this process started. Vite is a grandchild (`cmd.exe` → `bun` → `node`), so
/// only a tree kill reaches it; killing the shell alone leaves the port held and the next launch
/// finds a server nobody owns.
pub fn shutdown() {
    let Some(mut child) = SPAWNED.lock().expect("dev server lock").take() else {
        return;
    };
    if cfg!(windows) {
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::DEV_ADDR;

    /// The address here and `devUrl` in `tauri.conf.json` are the same fact written twice; a window
    /// pointed at a port nothing serves is exactly the failure this module exists to prevent.
    #[test]
    fn dev_url_matches_tauri_conf() {
        let conf = include_str!("../tauri.conf.json");
        let (_, rest) = conf
            .split_once("\"devUrl\"")
            .expect("devUrl in tauri.conf.json");
        let url = rest.split('"').nth(1).expect("devUrl value");
        assert_eq!(url.trim_start_matches("http://"), DEV_ADDR);
    }
}
