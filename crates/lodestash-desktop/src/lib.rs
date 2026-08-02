//! Lodestash desktop shell (Tauri 2).
//!
//! B2 scaffold: window + frontend host + the command surface's first entry. The
//! `Arc<Mutex<Engine>>` and the pump thread land once `lodestash-app` exposes `Engine`
//! (plan Phase B, B1); until then this crate owns only the shell.

use serde::Serialize;

#[derive(Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    /// True once the shell is backed by a real `Engine`. The frontend renders stub data
    /// while this is false, so a half-wired build is visible rather than silently empty.
    pub engine_ready: bool,
}

#[tauri::command]
fn app_info() -> AppInfo {
    AppInfo {
        name: "Lodestash",
        version: env!("CARGO_PKG_VERSION"),
        engine_ready: false,
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lodestash_desktop=info".into()),
        )
        .init();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![app_info])
        .run(tauri::generate_context!())
        .expect("error while running Lodestash");
}
