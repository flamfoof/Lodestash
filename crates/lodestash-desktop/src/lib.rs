//! Lodestash desktop shell (Tauri 2).
//!
//! Hosts the Svelte frontend and owns the command surface. Today it reads the catalog directly
//! (see [`library`]); the write path, the pump thread and the rest of the `lodestash://` handlers arrive
//! with `Engine` once the B1 extraction lands. Nothing here writes to the catalog — the
//! single-writer rule (ARCHITECTURE §5) puts every write on the persist thread.

pub mod library;
pub mod protocol;

use std::sync::Mutex;

use serde::Serialize;
use tauri::Manager;

use crate::library::{AssetDetail, GridPage, GridQuery, Library, LibraryStatus};

#[derive(Serialize)]
pub struct AppInfo {
    pub name: &'static str,
    pub version: &'static str,
    /// True once a catalog is open behind the shell. The frontend says so plainly rather than
    /// rendering an empty grid that looks like an empty library.
    pub engine_ready: bool,
}

struct AppState {
    library: Mutex<Library>,
}

#[tauri::command]
fn app_info(state: tauri::State<'_, AppState>) -> AppInfo {
    let open = state
        .library
        .lock()
        .map(|l| l.status().open)
        .unwrap_or(false);
    AppInfo {
        name: "Lodestash",
        version: env!("CARGO_PKG_VERSION"),
        engine_ready: open,
    }
}

#[tauri::command]
fn library_status(state: tauri::State<'_, AppState>) -> Result<LibraryStatus, String> {
    state
        .library
        .lock()
        .map(|l| l.status())
        .map_err(|_| "library lock poisoned".to_string())
}

#[tauri::command]
fn grid_page(state: tauri::State<'_, AppState>, query: GridQuery) -> Result<GridPage, String> {
    state
        .library
        .lock()
        .map(|l| l.page(&query))
        .map_err(|_| "library lock poisoned".to_string())
}

/// Everything the Inspector shows for one asset (UI-SPEC §13).
#[tauri::command]
fn asset_detail(
    state: tauri::State<'_, AppState>,
    asset_id: i64,
) -> Result<Option<AssetDetail>, String> {
    state
        .library
        .lock()
        .map(|l| l.detail(asset_id))
        .map_err(|_| "library lock poisoned".to_string())
}

/// Re-read the catalog. The egui app writes it, so a scan there shows up here on request —
/// there is no change notification until the pump thread exists.
#[tauri::command]
fn reload_library(state: tauri::State<'_, AppState>) -> Result<LibraryStatus, String> {
    let mut lib = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?;
    lib.reload();
    Ok(lib.status())
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lodestash_desktop=info".into()),
        )
        .init();

    let library = Library::open();
    let status = library.status();
    tracing::info!(
        open = status.open,
        assets = status.total_assets,
        in_pack = status.in_pack,
        favorites = status.favorites,
        previews = status.with_preview,
        sources = status.sources.len(),
        "catalog"
    );

    tauri::Builder::default()
        .manage(AppState {
            library: Mutex::new(library),
        })
        .register_uri_scheme_protocol("lodestash", |ctx, req| {
            let state = ctx.app_handle().state::<AppState>();
            protocol::handle(&state.library, &req)
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            library_status,
            grid_page,
            asset_detail,
            reload_library
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lodestash");
}
