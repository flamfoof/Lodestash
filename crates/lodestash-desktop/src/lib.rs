//! Lodestash desktop shell (Tauri 2).
//!
//! Hosts the Svelte frontend and owns the command surface. Today it reads the catalog directly
//! (see [`library`]); the write path, the pump thread and the rest of the `lodestash://` handlers arrive
//! with `Engine` once the B1 extraction lands. Nothing here writes to the catalog — the
//! single-writer rule (ARCHITECTURE §5) puts every write on the persist thread.

pub mod library;
pub mod protocol;
pub mod writer;

use std::sync::Mutex;

use serde::Serialize;
use tauri::Manager;

use crate::library::{
    AssetDetail, Facets, FolderPage, FolderQuery, GridPage, GridQuery, Library, LibraryStatus,
};
use crate::writer::{Op, WriteResult, Writer};

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
    /// `None` when there is no catalog on disk yet — every write then reports why instead of
    /// pretending to succeed.
    writer: Option<Writer>,
}

impl AppState {
    /// Run a write, then re-read the library so the next `grid_page` reflects it.
    ///
    /// The reload is the honest option while this crate has no change notification: the rows are
    /// an in-memory snapshot, and patching them locally to match what we *think* the write did is
    /// how a UI ends up disagreeing with its own database. 8k rows re-read in a few milliseconds,
    /// and only on a mutation. ponytail: patch in place once the pump (B2b) can tell us what
    /// actually changed.
    fn write(&self, op: Op) -> Result<WriteResult, String> {
        let writer = self
            .writer
            .as_ref()
            .ok_or_else(|| "no catalog is open — nothing to write to".to_string())?;
        let result = writer.run(op)?;
        self.library
            .lock()
            .map_err(|_| "library lock poisoned".to_string())?
            .reload();
        Ok(result)
    }

    fn targets(&self, ids: &[i64]) -> Result<Vec<writer::Target>, String> {
        self.library
            .lock()
            .map(|l| l.write_targets(ids))
            .map_err(|_| "library lock poisoned".to_string())
    }
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

/// One level of the Folders view (DESIGN-GUIDELINES §7). `source: null` is the top level, where
/// the tiles are the sources themselves.
#[tauri::command]
fn folder_page(state: tauri::State<'_, AppState>, query: FolderQuery) -> Result<FolderPage, String> {
    state
        .library
        .lock()
        .map(|l| l.folder_page(&query))
        .map_err(|_| "library lock poisoned".to_string())
}

/// The chips the ⚑ Filters popover draws (UI-SPEC §2.3). Fetched once per library load, not per
/// keystroke — it changes only when the catalog does.
#[tauri::command]
fn filter_facets(state: tauri::State<'_, AppState>) -> Result<Facets, String> {
    state
        .library
        .lock()
        .map(|l| l.facets())
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

/// Star / unstar assets (DATA-MODEL §12). A pin, not a rating — "find this again".
#[tauri::command]
fn set_favorite(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
    on: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetFavorite { ids: asset_ids, on })
}

/// Move to Trash, or restore from it. **Catalog-only** — the file on disk is never touched, and a
/// later scan can still find it (DATA-MODEL §8). This is the recoverable middle option between
/// `remove_from_library` and `delete_from_disk`.
#[tauri::command]
fn set_trashed(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
    trashed: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetTrashed {
        ids: asset_ids,
        trashed,
    })
}

/// **Remove from library** — hide an asset for good without deleting anything.
///
/// The row is soft-deleted *and* the asset is added to its source's blacklist, so re-indexing that
/// folder does not bring it back; importing the file again explicitly is the way back. This is the
/// action most people want when they reach for "delete", which is why the delete dialog offers it.
#[tauri::command]
fn remove_from_library(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
) -> Result<WriteResult, String> {
    let targets = state.targets(&asset_ids)?;
    let project = lodestash_app::config::AppConfig::load().last_project_name;
    state.write(Op::RemoveFromLibrary { targets, project })
}

/// **Delete from disk — irreversible.** The frontend must confirm first; this command does not
/// ask. Fenced by `under_known_root`, and only files that actually left the disk are recorded as
/// gone.
#[tauri::command]
fn delete_from_disk(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
) -> Result<WriteResult, String> {
    let targets = state.targets(&asset_ids)?;
    let roots = state
        .library
        .lock()
        .map(|l| l.roots())
        .map_err(|_| "library lock poisoned".to_string())?;
    let workspace = lodestash_app::paths::default_cache_dirs().1;
    state.write(Op::DeleteFromDisk {
        targets,
        roots,
        workspace,
    })
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

    // One writer for the process (ARCHITECTURE §5), spawned before any command can run.
    let writer = lodestash_app::paths::catalog_db_path()
        .filter(|p| p.exists())
        .map(Writer::spawn);

    tauri::Builder::default()
        .manage(AppState {
            library: Mutex::new(library),
            writer,
        })
        .register_uri_scheme_protocol("lodestash", |ctx, req| {
            let state = ctx.app_handle().state::<AppState>();
            protocol::handle(&state.library, &req)
        })
        .invoke_handler(tauri::generate_handler![
            app_info,
            library_status,
            grid_page,
            filter_facets,
            folder_page,
            asset_detail,
            reload_library,
            set_favorite,
            set_trashed,
            remove_from_library,
            delete_from_disk
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lodestash");
}
