//! Lodestash desktop shell (Tauri 2).
//!
//! Hosts the Svelte frontend and owns the command surface. Today it reads the catalog directly
//! (see [`library`]); the write path, the pump thread and the rest of the `lodestash://` handlers arrive
//! with `Engine` once the B1 extraction lands. Nothing here writes to the catalog — the
//! single-writer rule (ARCHITECTURE §5) puts every write on the persist thread.

pub mod library;
pub mod protocol;
pub mod writer;

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::Manager;

use crate::library::{
    AssetDetail, DupGroup, Facets, FolderPage, FolderQuery, GridPage, GridQuery, Library,
    LibraryStatus, PackDirNode, PackageRow, Settings, Stats, VocabRow,
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

/// The trait vocabulary (UI-SPEC §9). Read-only — authoring is a write this shell has no op for.
#[tauri::command]
fn vocabulary(state: tauri::State<'_, AppState>) -> Result<Vec<VocabRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.vocabulary())
        .map_err(|_| "library lock poisoned".to_string())
}

/// Every pack, publisher → pack (UI-SPEC §4). Counts come from the indexed rows, not from
/// `package.asset_count`, which is populated for 1 of 87 packs on a real cache.
#[tauri::command]
fn packages(state: tauri::State<'_, AppState>) -> Result<Vec<PackageRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.packages())
        .map_err(|_| "library lock poisoned".to_string())
}

/// The Stats overview (UI-SPEC §8.1), derived from rows already in memory — no stats query.
#[tauri::command]
fn library_stats(state: tauri::State<'_, AppState>) -> Result<Stats, String> {
    state
        .library
        .lock()
        .map(|l| l.stats())
        .map_err(|_| "library lock poisoned".to_string())
}

/// Exact-duplicate groups (UI-SPEC §5). A review queue — nothing here resolves itself.
#[tauri::command]
fn duplicate_groups(state: tauri::State<'_, AppState>) -> Result<Vec<DupGroup>, String> {
    state
        .library
        .lock()
        .map(|l| l.duplicates())
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

/// **Add a folder to the library and index it** — what drag-and-drop drops into.
///
/// Three steps, in the only order that respects the single-writer rule: crawl the folder on this
/// command's own pool thread (a read), hand the finished records to the writer thread (the write),
/// then re-read. The crawl never runs on the writer, because a disk walk of a real source takes
/// minutes and everything else — favouriting, trashing — would queue behind it.
///
/// **Synchronous.** The command does not return until the folder is indexed, so the frontend shows
/// a spinner rather than a progress bar. Real progress needs the job tray, which needs the pump
/// (B2b); a fake bar that jumps 0 → 100 is worse than an honest spinner.
#[tauri::command]
fn add_source(state: tauri::State<'_, AppState>, path: String) -> Result<WriteResult, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        // Dropping a *file* is a different feature (import one asset) and pretending otherwise
        // would index its whole parent folder, which is not what anyone meant by dropping a file.
        return Err(format!("{path} is not a folder"));
    }
    // The source's blacklist, or **Remove from library** is a lie: it writes an `Exclude` into
    // the app config precisely so a later scan does not re-offer the asset (B5), and a crawl that
    // ignores it resurrects everything the user removed the moment they re-add the folder.
    // Pack-only sources are honoured for the same reason — the user already said what they want
    // from that folder.
    let config = lodestash_app::config::AppConfig::load();
    let (excludes, pack_only) = match config.last_project_name.as_deref() {
        Some(project) => (
            config.source_excludes(project, &root),
            config.is_pack_only(project, &root),
        ),
        // No open project means no per-source rules exist to honour yet.
        None => (Vec::new(), false),
    };
    if pack_only {
        return Err(format!(
            "{path} is marked pack-only — its packs are indexed from their archives, not by walking it"
        ));
    }
    let crawled = lodestash_app::scan::crawl_source(0, &root, &excludes);
    if crawled.entries.is_empty() && crawled.pending_packages.is_empty() {
        return Err(format!("nothing indexable in {path}"));
    }
    let records = lodestash_app::persist::crawl_records(crawled.entries.iter().map(|(_, f)| f));
    let batch = lodestash_app::types::PersistBatch {
        root: root.to_string_lossy().into_owned(),
        name: lodestash_app::config::stem_name(&root),
        excludes_json: None,
        records,
    };
    let result = state.write(Op::PersistScan { batches: vec![batch] })?;
    tracing::info!(%path, indexed = result.changed, packs = crawled.pending_packages.len(), "source added");
    Ok(result)
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

/// The pack browser's left tree (PACK-DIRECTORY-NAV §5). Rooted at one package — derived from
/// the in-memory rows, so it always agrees with the contents pane.
#[tauri::command]
fn pack_dir_tree(state: tauri::State<'_, AppState>, package: String) -> Result<Vec<PackDirNode>, String> {
    state
        .library
        .lock()
        .map(|l| l.pack_tree(&package))
        .map_err(|_| "library lock poisoned".to_string())
}

/// The Svelte shell's persisted view prefs (FEATURES §10). Read once at startup; the frontend
/// keeps its own copy and saves it back through `set_web_prefs` on every change.
#[tauri::command]
fn get_web_prefs() -> lodestash_app::config::WebPrefs {
    lodestash_app::config::AppConfig::load().web
}

/// Persist the Svelte shell's view prefs. All-or-nothing write of the whole group — the
/// frontend is the only writer, and a partial patch would leave fields silently stale.
#[tauri::command]
fn set_web_prefs(prefs: lodestash_app::config::WebPrefs) -> Result<(), String> {
    let mut config = lodestash_app::config::AppConfig::load();
    config.web = prefs;
    config.save();
    Ok(())
}

/// Trait-library authoring (UI-SPEC §9, B8a). Create or promote a trait; an existing tag keeps
/// its assignments and gains first-class status. Each op runs on the writer thread like every
/// other catalog write — one writer, one order.
#[tauri::command]
fn upsert_trait(
    state: tauri::State<'_, AppState>,
    namespace: String,
    value: String,
    scope: Option<String>,
    usage_note: Option<String>,
    teach: bool,
) -> Result<WriteResult, String> {
    state.write(Op::UpsertTrait {
        namespace,
        value,
        scope,
        usage_note,
        teach,
    })
}

#[tauri::command]
fn unpromote_trait(state: tauri::State<'_, AppState>, tag_id: i64) -> Result<WriteResult, String> {
    state.write(Op::UnpromoteTrait { tag_id })
}

#[tauri::command]
fn set_trait_teach(state: tauri::State<'_, AppState>, tag_id: i64, teach: bool) -> Result<WriteResult, String> {
    state.write(Op::SetTraitTeach { tag_id, teach })
}

#[tauri::command]
fn set_trait_style(
    state: tauri::State<'_, AppState>,
    tag_id: i64,
    color: Option<String>,
    icon: Option<String>,
) -> Result<WriteResult, String> {
    state.write(Op::SetTraitStyle { tag_id, color, icon })
}

/// The Settings screen's snapshot (UI-SPEC §11). Read once; the frontend writes it back whole.
#[tauri::command]
fn get_settings() -> Settings {
    Settings::from_config(&lodestash_app::config::AppConfig::load())
}

/// Persist the Settings snapshot and re-read the library so the size tiers and source rows the
/// grid draws follow immediately (badges, ⚑ size facet, watch toggles).
#[tauri::command]
fn set_settings(state: tauri::State<'_, AppState>, settings: Settings) -> Result<(), String> {
    let mut config = lodestash_app::config::AppConfig::load();
    settings.apply(&mut config);
    config.save();
    state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?
        .reload();
    Ok(())
}

/// Pause/resume watching one source (UI-SPEC §11.1). Goes through the writer like every catalog
/// write; the library reload after it refreshes the Sources rows.
#[tauri::command]
fn set_source_watch(state: tauri::State<'_, AppState>, id: i64, watch: bool) -> Result<WriteResult, String> {
    state.write(Op::SetSourceWatch { id, watch })
}

/// Every non-staging collection with its member count (UI-SPEC §6).
#[tauri::command]
fn collections(state: tauri::State<'_, AppState>) -> Result<Vec<lodestash_core::catalog::CollectionRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.collections())
        .map_err(|_| "library lock poisoned".to_string())
}

#[tauri::command]
fn create_collection(state: tauri::State<'_, AppState>, name: String) -> Result<WriteResult, String> {
    state.write(Op::CreateCollection { name })
}

#[tauri::command]
fn delete_collection(state: tauri::State<'_, AppState>, id: i64) -> Result<WriteResult, String> {
    state.write(Op::DeleteCollection { id })
}

/// Pin/unpin a `.unitypackage` as a library (FEATURES §5.4) — the Unity Assets screen's ⬢.
#[tauri::command]
fn set_package_pinned(state: tauri::State<'_, AppState>, id: i64, pinned: bool) -> Result<WriteResult, String> {
    state.write(Op::SetPackagePinned { id, pinned })
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
        // Window geometry (size/position/maximized) is remembered across launches and restored
        // before the window is shown — the window opens where the user left it instead of
        // re-applying the launch defaults and jumping. State file lives in the app config dir.
        .plugin(tauri_plugin_window_state::Builder::default().build())
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
            packages,
            vocabulary,
            library_stats,
            duplicate_groups,
            asset_detail,
            reload_library,
            set_favorite,
            set_trashed,
            remove_from_library,
            delete_from_disk,
            add_source,
            get_web_prefs,
            set_web_prefs,
            pack_dir_tree,
            upsert_trait,
            unpromote_trait,
            set_trait_teach,
            set_trait_style,
            get_settings,
            set_settings,
            set_source_watch,
            collections,
            create_collection,
            delete_collection,
            set_package_pinned
        ])
        .run(tauri::generate_context!())
        .expect("error while running Lodestash");
}
