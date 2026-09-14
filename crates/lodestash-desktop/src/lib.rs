//! Lodestash desktop shell (Tauri 2).
//!
//! Hosts the Svelte frontend and owns the command surface. Today it reads the catalog directly
//! (see [`library`]); the write path, the pump thread and the rest of the `lodestash://` handlers arrive
//! with `Engine` once the B1 extraction lands. Nothing here writes to the catalog — the
//! single-writer rule (ARCHITECTURE §5) puts every write on the persist thread.

pub mod agent_bridge;
#[cfg(debug_assertions)]
mod devserver;
pub mod library;
pub mod protocol;
pub mod storage;
pub mod writer;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::Connection;
use serde::Serialize;
use tauri::Manager;
use tauri_plugin_window_state::AppHandleExt;

/// Throttle for the geometry save on Moved/Resized (see the `on_window_event` in [`run`]) —
/// shared across every event so a drag writes at most every 300ms.
static LAST_GEOMETRY_SAVE: Mutex<Option<Instant>> = Mutex::new(None);

use crate::library::{
    AssetDetail, DupGroup, ExportBatchRow, Facets, FolderPage, FolderQuery, GridPage, GridQuery,
    Library, LibraryStatus, PackDirNode, PackageRow, RelinkCount, Settings, Stats, VocabRow,
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
    /// Raised while the writer thread runs an appraise pass (B11d) — the ✦ controls and any
    /// progress surface read it instead of racing the writer on timing guesses.
    pump: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Shared appraise-pass progress (B11d): the tag/embed drains write done/total into this
    /// while a pass runs; `pump_progress` reads it for the Identify screen's bar.
    appraise: std::sync::Arc<lodestash_app::jobs::JobProgress>,
    /// Appraisal queue order (UI-SPEC §3) — what the user is looking at, shared straight with the
    /// writer thread. Deliberately **not** an `Op`: browsing must never queue behind a running
    /// appraise pass, and this is a hint, not a write.
    focus: std::sync::Arc<Mutex<lodestash_core::catalog::AppraiseFocus>>,
    /// The agent plane (B12): the embedded REST/MCP `ServerState`, kept alive for the process.
    /// `None` when there is no catalog on disk yet — the servers then don't exist and the
    /// Connect screen says so instead of pretending.
    engine: Option<std::sync::Arc<lodestash_api::ServerState>>,
    /// The loopback port `engine.json` was written with (ENGINE-API §1).
    engine_port: Option<u16>,
    /// The per-session bearer token (ENGINE-API §1) — surfaced for the Connect screen, never
    /// logged.
    engine_token: String,
    /// How many agent tools the allow-list left enabled (`None` = every tool is live).
    engine_allowed: Option<usize>,
    /// The job tray (B2b): active jobs + the session ring, owned by the writer thread.
    jobs: crate::writer::JobRegistry,
    /// `None` when there is no catalog on disk yet — every write then reports why instead of
    /// pretending to succeed.
    writer: Option<Writer>,
    /// The Import-packages dialog's (FEATURES §5.2) discovery cache: archive absolute path → the
    /// manifest locators `discover_package_folder` read, held so `import_packages` can record +
    /// mint without re-inflating the archive.
    discovered:
        Mutex<std::collections::HashMap<String, Vec<lodestash_core::catalog::PackageEntryRecord>>>,
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

/// Near-duplicate review (DATA-MODEL §5): same picture, different bytes — dHash within 6 bits.
#[tauri::command]
fn near_duplicate_groups(state: tauri::State<'_, AppState>) -> Result<Vec<DupGroup>, String> {
    state
        .library
        .lock()
        .map(|l| l.near_duplicates())
        .map_err(|_| "library lock poisoned".to_string())
}

/// One level of the Folders view (DESIGN-GUIDELINES §7). `source: null` is the top level, where
/// the tiles are the sources themselves.
#[tauri::command]
fn folder_page(
    state: tauri::State<'_, AppState>,
    query: FolderQuery,
) -> Result<FolderPage, String> {
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

/// Re-derive source availability from disk (UI-SPEC §11.1, M18).
///
/// Called when the library opens and after a scan. Nothing here removes, trashes or hides a row —
/// a source going away leaves the catalog fully browsable and only the files out of reach, which is
/// the whole point of having an index.
#[tauri::command]
fn refresh_source_health(state: tauri::State<'_, AppState>) -> Result<WriteResult, String> {
    state.write(Op::RefreshSourceHealth)
}

/// What relinking this source to that folder would actually reach (UI-SPEC §11.1).
///
/// Read-only, and deliberately shown before the button commits: a relink that would strand half the
/// library is a thing you see first. Slow by nature — one `exists` per loose file, on a drive that
/// may be cold — so it is `async` and runs off the UI thread.
#[tauri::command]
async fn relink_preview(
    state: tauri::State<'_, AppState>,
    source_id: i64,
    new_root: String,
) -> Result<RelinkCount, String> {
    let path = std::path::PathBuf::from(&new_root);
    if !path.is_dir() {
        return Err(format!("{new_root} is not a folder"));
    }
    state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?
        .relink_preview(source_id, &path)
}

/// Point a source at a new root (UI-SPEC §11.1).
///
/// Non-destructive by construction: no asset row is deleted, trashed or re-identified, because the
/// only things holding an absolute path are the root itself and the packs under it.
#[tauri::command]
fn relink_source(
    state: tauri::State<'_, AppState>,
    source_id: i64,
    new_root: String,
) -> Result<WriteResult, String> {
    let path = std::path::PathBuf::from(&new_root);
    if !path.is_dir() {
        return Err(format!("{new_root} is not a folder"));
    }
    state.write(Op::RelinkSource {
        source_id,
        new_root: path,
    })
}

/// Tell the appraisal queues where the user is looking (UI-SPEC §3).
///
/// Called on every browse, so it must be cheap and must never block: it resolves the query against
/// rows already in memory and swaps one in-memory list — no catalog round-trip, no writer op, no
/// row written. Going through `write()` instead would make scrolling queue behind whatever appraise
/// pass is running, which is exactly the wait this feature exists to remove.
///
/// The next pass reads it. A pass already running keeps the order it started with rather than being
/// re-sorted mid-flight; it is bounded to seconds, so the new order lands almost immediately.
#[tauri::command]
fn set_appraise_focus(state: tauri::State<'_, AppState>, query: GridQuery) -> Result<(), String> {
    let order = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?
        .focus_order(&query);
    *state
        .focus
        .lock()
        .map_err(|_| "focus lock poisoned".to_string())? = order;
    Ok(())
}

/// The recent **folder** export runs, newest first — the other half of §7.1's "Recent exports".
///
/// `recent_exports` above lists imports *into a registered project*; this lists copies to a folder
/// the user chose. They are separate tables because they are separate acts (UI-SPEC §16.2 keeps the
/// verbs apart) and because `import_record` cannot hold a folder export: its `project_id` is
/// `NOT NULL` and its `dest_path` is project-relative.
///
/// Read-only, and deliberately a *history*: it lists what was actually written to disk, which is
/// the only thing undo can act on.
#[tauri::command]
fn recent_folder_exports(
    state: tauri::State<'_, AppState>,
    limit: usize,
) -> Result<Vec<ExportBatchRow>, String> {
    state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?
        .recent_exports(limit.clamp(1, 50))
}

/// Undo one export run — delete the files it copied out, then forget it (FEATURES §7.1).
///
/// Destructive, so it goes through the writer like every other delete, and the writer fences it to
/// the exact names the run recorded, under the folder it recorded, re-checked against the export
/// sandbox at undo time. Originals are never in scope: an export copies.
#[tauri::command]
fn undo_export_batch(
    state: tauri::State<'_, AppState>,
    batch_id: String,
) -> Result<WriteResult, String> {
    state.write(Op::UndoExportBatch { batch_id })
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

/// Mark assets **confirmed**, or clear that back to the status the ingest work earned
/// (DATA-MODEL §8, AI-PIPELINE §6 human-in-the-loop).
///
/// The one status a person sets. Deliberately **not** on the agent planes: an MCP client must not
/// be able to sign off as the user, because "confirmed" is precisely the claim that a human looked.
#[tauri::command]
fn set_confirmed(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
    on: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetConfirmed { ids: asset_ids, on })
}

/// "More like this" (UI-SPEC §13.8) — nearest neighbours in the asset's own embedding space.
///
/// Read-only, and empty when the asset has no embedding: a library that has not been encoded yet
/// is not an error, it just has no neighbours.
#[tauri::command]
fn similar_assets(
    state: tauri::State<'_, AppState>,
    asset_id: i64,
    limit: Option<usize>,
) -> Result<Vec<crate::library::GridItem>, String> {
    state
        .library
        .lock()
        .map(|l| l.similar_assets(asset_id, limit.unwrap_or(3)))
        .map_err(|_| "library lock poisoned".to_string())
}

/// What a workspace reclaim would remove, without removing it (UI-SPEC §11.4).
///
/// Always a dry run. The button that calls it reports the size first, because the sweep deletes
/// files from the user's disk and the first honest thing to do with 3.6 GB is show it.
#[tauri::command]
fn workspace_usage(
    state: tauri::State<'_, AppState>,
) -> Result<crate::storage::ReclaimPlan, String> {
    let Some(workspace) = lodestash_app::paths::default_cache_dirs().1 else {
        return Ok(crate::storage::ReclaimPlan::default());
    };
    let referenced = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?
        .referenced_workspace_paths();
    Ok(crate::storage::plan_reclaim(&workspace, &referenced))
}

/// Outcome of a reclaim — recomputed from what actually went, plus where the record was written.
#[derive(serde::Serialize)]
struct ReclaimOutcome {
    removed: usize,
    freed: u64,
    /// The append-only log listing every folder removed. `None` means it could not be written,
    /// which the caller says out loud: deleting gigabytes and keeping no record is worth knowing.
    log: Option<String>,
}

/// Reclaim the workspace folders no live asset resolves through (UI-SPEC §11.4).
///
/// Re-plans rather than trusting a plan the frontend held: the catalog may have changed since the
/// dry run, and a stale plan is how a sweep deletes something that has just been re-materialized.
#[tauri::command]
fn reclaim_workspace(state: tauri::State<'_, AppState>) -> Result<ReclaimOutcome, String> {
    let Some(workspace) = lodestash_app::paths::default_cache_dirs().1 else {
        return Err("no workspace directory configured".into());
    };
    let referenced = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?
        .referenced_workspace_paths();
    let plan = crate::storage::plan_reclaim(&workspace, &referenced);
    let log = crate::storage::write_reclaim_log(&workspace, &plan);
    let (removed, freed) = crate::storage::execute_reclaim(&plan);
    tracing::info!(removed, freed, "workspace reclaimed");
    Ok(ReclaimOutcome {
        removed,
        freed,
        log: log.map(|p| p.to_string_lossy().into_owned()),
    })
}

/// Packs holding extracted bytes, largest first (UI-SPEC §11.4 storage list).
#[tauri::command]
fn workspace_packs(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::library::WorkspacePackRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.workspace_packs())
        .map_err(|_| "library lock poisoned".to_string())
}

/// Keep a pack's extracted bytes (UI-SPEC §15). A pack that is *not* kept stays fully searchable
/// and previewable — its payloads resolve out of the archive instead of off disk.
#[tauri::command]
fn set_pack_keep_cached(
    state: tauri::State<'_, AppState>,
    package_id: i64,
    keep: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetPackKeepCached { package_id, keep })
}

/// How much of a pack a candidate archive still accounts for, by GUID (UI-SPEC §15 Retarget…).
///
/// Read-only, and deliberately never a verdict: a publisher updating a pack legitimately
/// reshuffles GUIDs, so a low match is reported and the user decides — the same shape as
/// `relink_preview` for a moved source root.
#[derive(serde::Serialize)]
struct RetargetPreview {
    matched: usize,
    total: usize,
}

#[tauri::command]
fn retarget_preview(
    state: tauri::State<'_, AppState>,
    package_id: i64,
    path: String,
) -> Result<RetargetPreview, String> {
    let candidate = PathBuf::from(&path);
    if !candidate.is_file() {
        return Err(format!("{path} is not a file"));
    }
    let guids: Vec<String> = lodestash_ingest::unitypackage::read_manifest(&candidate)
        .map_err(|e| format!("{path} could not be read: {e}"))?
        .into_iter()
        .map(|e| e.guid)
        .collect();
    let lib = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?;
    let (matched, total) = lib
        .retarget_match(package_id, &guids)
        .ok_or_else(|| "no catalog on disk".to_string())?;
    Ok(RetargetPreview { matched, total })
}

/// Point a pack at a different archive (UI-SPEC §15 Retarget…).
#[tauri::command]
fn retarget_package(
    state: tauri::State<'_, AppState>,
    package_id: i64,
    path: String,
) -> Result<WriteResult, String> {
    if !PathBuf::from(&path).is_file() {
        return Err(format!("{path} is not a file"));
    }
    state.write(Op::RetargetPackage {
        package_id,
        new_path: path,
    })
}

/// ⊘ Ignore / ↺ restore one dependency edge (UI-SPEC §13.6). Catalog-only, and reversible: the
/// row is muted rather than deleted, so the panel can offer it back and the next dependency scan
/// cannot resurrect it.
#[tauri::command]
fn set_link_ignored(
    state: tauri::State<'_, AppState>,
    edge_id: i64,
    ignored: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetLinkIgnored { edge_id, ignored })
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
    import_root(&state, &path)
}

/// Shared by `add_source` and the Unity Assets `↻ Sync`: crawl one folder (incrementally —
/// unchanged files skip re-hashing), persist, and surface any `.unitypackage`s as browsable
/// packs. `add_source` refuses pack-only roots; a sync skips them silently.
fn import_root(state: &tauri::State<'_, AppState>, path: &str) -> Result<WriteResult, String> {
    let root = PathBuf::from(path);
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
    let crawled = {
        // Incremental diff (DATA-MODEL §6): re-adding an already-indexed folder skips re-hashing
        // files the catalog knows at the same (size, mtime) — one stat per file, not a re-read.
        let known = state
            .library
            .lock()
            .map(|lib| lib.known_sizes(&lodestash_app::config::stem_name(&root)))
            .unwrap_or_default();
        lodestash_app::scan::crawl_source(0, &root, &excludes, &known)
    };
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
    // Browse persistence (INGESTION §2): the crawl captured each `.unitypackage`'s manifest
    // locators — hand them to the writer so the packs become browsable + surgically
    // extractable rows without ever re-reading the archive.
    let packs: Vec<crate::writer::DiscoveredPack> = crawled
        .pending_packages
        .into_iter()
        .filter(|p| p.source_idx == 0)
        .map(|p| crate::writer::DiscoveredPack {
            root: root.to_string_lossy().into_owned(),
            abs_path: p.abs_path.to_string_lossy().into_owned(),
            display_name: p.display_name,
            entries: p.entries,
        })
        .collect();
    let discovered_packs = packs.len();
    let result = state.write(Op::PersistScan {
        batches: vec![batch],
        packs,
    })?;
    tracing::info!(%path, indexed = result.changed, packs = discovered_packs, "source added");
    Ok(result)
}

/// Unity Assets' `↻ Sync` (FEATURES §5.3): crawl every Asset Store cache root (the
/// `ASSETSTORE_CACHE_PATH` override first, then the default) so packs that landed since launch
/// show up without re-adding a folder by hand. Same incremental import as `add_source` — a
/// re-scan skips unchanged files via `needs_rehash`. Pack-only roots and empty roots are
/// skipped, not errors.
#[tauri::command]
fn sync_unity_cache(state: tauri::State<'_, AppState>) -> Result<WriteResult, String> {
    let mut changed = 0usize;
    let mut scanned = 0usize;
    for root in lodestash_ingest::asset_store::candidate_roots() {
        if !root.is_dir() {
            continue;
        }
        let config = lodestash_app::config::AppConfig::load();
        let pack_only = match config.last_project_name.as_deref() {
            Some(project) => config.is_pack_only(project, &root),
            None => false,
        };
        if pack_only {
            continue;
        }
        match import_root(&state, &root.to_string_lossy()) {
            Ok(r) => {
                changed += r.changed;
                scanned += 1;
            }
            Err(e) => tracing::info!(root = %root.display(), %e, "sync root skipped"),
        }
    }
    if scanned == 0 {
        return Err(
            "no Unity Asset Store cache found — install Unity or set ASSETSTORE_CACHE_PATH".into(),
        );
    }
    Ok(WriteResult {
        changed,
        ..Default::default()
    })
}

/// Unity Asset Store cache roots that **exist on this machine and are not already a source**
/// (UI-SPEC §1: *"the Unity Asset Store cache is auto-detected … and listed first when present"*).
///
/// Read-only and cheap on purpose — an `is_dir` per candidate root and a `discover` of the pack
/// names, never a crawl. Onboarding has to be able to *show* the cache before the user agrees to
/// index it; `sync_unity_cache` is the version that imports, and running that to find out whether
/// there was anything to find would index the machine to answer a question.
#[tauri::command]
fn unity_cache_candidates(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<CacheCandidate>, String> {
    let known: Vec<String> = state
        .library
        .lock()
        .map(|l| {
            l.status()
                .sources
                .iter()
                .map(|s| s.root_path.to_lowercase())
                .collect()
        })
        .unwrap_or_default();
    Ok(lodestash_ingest::asset_store::candidate_roots()
        .into_iter()
        .filter(|r| r.is_dir())
        .map(|r| r.to_string_lossy().into_owned())
        .filter(|p| !known.contains(&p.to_lowercase()))
        .map(|path| {
            let packs = lodestash_ingest::asset_store::discover(std::path::Path::new(&path)).len();
            CacheCandidate { path, packs }
        })
        .collect())
}

/// One detected-but-not-yet-added Asset Store cache root (§1's source row).
#[derive(serde::Serialize)]
struct CacheCandidate {
    path: String,
    /// How many packs sit in it — the number that makes "add this" an informed click.
    packs: usize,
}

/// The sidebar's "+ Add source…" chooser (B2b): a native folder dialog, then the same
/// import path as dropping the folder. `None` = the user cancelled. Runs on the persist
/// thread via `add_source`'s write; the dialog itself lives on the main thread.
#[tauri::command]
fn add_source_dialog(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Option<WriteResult>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title("Add a library source")
        .pick_folder(move |path| {
            let _ = tx.send(path.map(|p| match p {
                tauri_plugin_dialog::FilePath::Path(p) => p,
                // Folders are never URLs; if one somehow arrives, treat it as a cancel.
                tauri_plugin_dialog::FilePath::Url(_) => PathBuf::new(),
            }));
        });
    match rx.recv().map_err(|_| "folder dialog closed".to_string())? {
        Some(path) if !path.as_os_str().is_empty() => {
            add_source(state, path.to_string_lossy().into_owned()).map(Some)
        }
        _ => Ok(None),
    }
}

/// Native folder chooser, returning the chosen path or `None` on cancel.
///
/// Picking is the *only* thing it does, which is what lets two very different callers share it: the
/// import dialog (FEATURES §5.2) scans afterwards in `discover_package_folder`, and the relink
/// panel (UI-SPEC §11.1) counts afterwards in `relink_preview` — both want a real modal with a
/// spinner rather than a blocking native loop. `title` is what tells the user which one they are in.
#[tauri::command]
fn pick_package_file(
    app: tauri::AppHandle,
    title: Option<String>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title(title.unwrap_or_else(|| "Find this pack's .unitypackage".into()))
        .add_filter("Unity package", &["unitypackage"])
        .pick_file(move |path| {
            let _ = tx.send(path.map(|p| match p {
                tauri_plugin_dialog::FilePath::Path(p) => p.to_string_lossy().into_owned(),
                tauri_plugin_dialog::FilePath::Url(_) => String::new(),
            }));
        });
    match rx.recv().map_err(|_| "file dialog closed".to_string())? {
        Some(path) if !path.is_empty() => Ok(Some(path)),
        _ => Ok(None),
    }
}

/// Native folder chooser for the import dialog. `None` = the user cancelled.
#[tauri::command]
fn pick_folder(app: tauri::AppHandle, title: Option<String>) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = std::sync::mpsc::channel();
    app.dialog()
        .file()
        .set_title(title.unwrap_or_else(|| "Choose a folder of .unitypackage files".into()))
        .pick_folder(move |path| {
            let _ = tx.send(path.map(|p| match p {
                tauri_plugin_dialog::FilePath::Path(p) => p.to_string_lossy().into_owned(),
                tauri_plugin_dialog::FilePath::Url(_) => String::new(),
            }));
        });
    match rx.recv().map_err(|_| "folder dialog closed".to_string())? {
        Some(path) if !path.is_empty() => Ok(Some(path)),
        _ => Ok(None),
    }
}

/// One `.unitypackage` the import dialog lists (FEATURES §5.2): the folder scanned for packs,
/// each described for the modal — group (the containing folder, the publisher/category tree in an
/// Asset Store cache), name, version, size, classifiable entry count, an auto-detected content
/// category, and the §5.6 duplicate flag.
#[derive(serde::Serialize)]
struct DiscoveredPackage {
    group: String,
    publisher: String,
    name: String,
    abs_path: String,
    size_bytes: u64,
    version: Option<String>,
    assets: usize,
    category: Option<String>,
    duplicate_of: Option<String>,
}

/// The Import-packages dialog's scan (FEATURES §5.2, read-only). Walks the folder for
/// `.unitypackage` files and reads each archive's manifest to count classifiable assets, detect a
/// content category, and read the version — stashing the locators for `import_packages` so the
/// write never re-inflates the archive. Nothing is copied or written.
#[tauri::command]
fn discover_package_folder(
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<Vec<DiscoveredPackage>, String> {
    let root = PathBuf::from(&path);
    if !root.is_dir() {
        return Err(format!("{path} is not a folder"));
    }
    let packs = lodestash_ingest::asset_store::discover(&root);
    if packs.is_empty() {
        return Err(format!("no .unitypackage files found in {path}"));
    }

    // §5.6 duplicate check: normalized-name match against what the catalog already holds.
    let known: std::collections::HashMap<String, String> = state
        .library
        .lock()
        .map(|l| {
            l.packages()
                .into_iter()
                .map(|p| (lodestash_app::scan::normalize_pack_name(&p.name), p.name))
                .collect()
        })
        .unwrap_or_default();

    let mut out = Vec::with_capacity(packs.len());
    let mut cache = state
        .discovered
        .lock()
        .map_err(|_| "discovery cache poisoned".to_string())?;
    for p in packs {
        let abs = p.path.to_string_lossy().into_owned();
        let entries: Vec<lodestash_core::catalog::PackageEntryRecord> =
            lodestash_ingest::unitypackage::read_manifest(&p.path)
                .map(|m| {
                    m.iter()
                        .map(|e| lodestash_core::catalog::PackageEntryRecord {
                            guid: e.guid.clone(),
                            pathname: e.pathname.clone(),
                            tar_offset: e.asset_offset.map(|o| o as i64),
                            size_bytes: e.asset_size.map(|s| s as i64),
                        })
                        .collect()
                })
                .unwrap_or_default();
        let assets = entries
            .iter()
            .filter(|e| {
                e.pathname.as_deref().is_some_and(|pn| {
                    lodestash_ingest::detect::by_extension(std::path::Path::new(pn)).is_some()
                })
            })
            .count();
        let category = lodestash_app::scan::dominant_content_category(&entries).map(str::to_owned);
        // Version needs a second, targeted pass (`read_package_meta`); best-effort — a pack with
        // no UPM package.json simply shows no version.
        let version = if entries.is_empty() {
            None
        } else {
            let meta_entries: Vec<lodestash_ingest::unitypackage::PackageEntry> = entries
                .iter()
                .map(|e| lodestash_ingest::unitypackage::PackageEntry {
                    guid: e.guid.clone(),
                    pathname: e.pathname.clone(),
                    asset_size: e.size_bytes.map(|s| s as u64),
                    ..Default::default()
                })
                .collect();
            lodestash_ingest::unitypackage::read_package_meta(&p.path, &meta_entries)
                .ok()
                .and_then(|m| m.version)
        };
        let duplicate_of = known
            .get(&lodestash_app::scan::normalize_pack_name(&p.name))
            .cloned();
        cache.insert(abs.clone(), entries);
        out.push(DiscoveredPackage {
            group: p.group,
            publisher: p.publisher,
            name: p.name,
            abs_path: abs,
            size_bytes: p.size_bytes,
            version,
            assets,
            category,
            duplicate_of,
        });
    }
    Ok(out)
}

/// What the import dialog sends back per ticked pack (FEATURES §5.2) — the display fields plus
/// the (possibly overridden) category. The manifest locators are filled from the discovery cache
/// server-side, so the webview never round-trips thousands of entries.
#[derive(serde::Deserialize)]
struct ImportSelection {
    abs_path: String,
    name: String,
    publisher: Option<String>,
    category: Option<String>,
}

/// Import the ticked packs as libraries (FEATURES §5.2): one `package` row apiece, pinned and
/// in-pack minted — nothing copied, nothing unpacked. Runs on the single writer (ADR-0002); the
/// manifest locators come from this session's discovery scan.
#[tauri::command]
fn import_packages(
    state: tauri::State<'_, AppState>,
    root: String,
    packs: Vec<ImportSelection>,
) -> Result<WriteResult, String> {
    let full = {
        let mut cache = state
            .discovered
            .lock()
            .map_err(|_| "discovery cache poisoned".to_string())?;
        let mut full = Vec::with_capacity(packs.len());
        for sel in packs {
            let entries = cache.remove(&sel.abs_path).ok_or_else(|| {
                format!(
                    "{} was not discovered in this session — re-scan the folder",
                    sel.abs_path
                )
            })?;
            full.push(crate::writer::ImportPack {
                abs_path: sel.abs_path,
                name: sel.name,
                publisher: sel.publisher,
                category: sel.category,
                entries,
            });
        }
        full
    };
    state.write(Op::ImportPacks { root, packs: full })
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
fn pack_dir_tree(
    state: tauri::State<'_, AppState>,
    package: String,
) -> Result<Vec<PackDirNode>, String> {
    state
        .library
        .lock()
        .map(|l| l.pack_tree(&package))
        .map_err(|_| "library lock poisoned".to_string())
}

/// The Svelte shell's persisted view prefs (FEATURES §10). Read once at startup; the frontend
/// keeps its own copy and saves it back through `set_web_prefs` on every change.
#[tauri::command]
fn get_web_prefs() -> Result<lodestash_app::config::WebPrefs, String> {
    lodestash_app::config::edit_config(|c| c.web.clone())
}

/// Persist the Svelte shell's view prefs. All-or-nothing write of the whole group — the
/// frontend is the only writer, and a partial patch would leave fields silently stale.
#[tauri::command]
fn set_web_prefs(prefs: lodestash_app::config::WebPrefs) -> Result<(), String> {
    lodestash_app::config::edit_config(|c| c.web = prefs)
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
fn set_trait_teach(
    state: tauri::State<'_, AppState>,
    tag_id: i64,
    teach: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetTraitTeach { tag_id, teach })
}

#[tauri::command]
fn set_trait_style(
    state: tauri::State<'_, AppState>,
    tag_id: i64,
    color: Option<String>,
    icon: Option<String>,
) -> Result<WriteResult, String> {
    state.write(Op::SetTraitStyle {
        tag_id,
        color,
        icon,
    })
}

/// The Settings screen's snapshot (UI-SPEC §11). Read once; the frontend writes it back whole.
#[tauri::command]
fn get_settings() -> Result<Settings, String> {
    lodestash_app::config::edit_config(|c| Settings::from_config(c))
}

/// The budget guard's pre-run question (AI-PIPELINE §5.1): would this Identify run spend money,
/// and on how many assets?
///
/// `None` means *no confirm* — the pass is local, or the endpoint is free/loopback. `Some` carries
/// what the confirm must say: the queue depth, the cap that will actually bound the run, and the
/// endpoint being billed.
///
/// The free/paid decision stays in Rust because `is_free_endpoint` already owns it, and it treats
/// an **unknown** remote as paid — the safe direction for a guard whose whole job is to ask before
/// spending. Deciding it in the frontend would give the confirm a different opinion from the drain.
#[tauri::command]
fn cloud_run_estimate(
    state: tauri::State<'_, AppState>,
) -> Result<Option<serde_json::Value>, String> {
    let cfg = lodestash_app::config::AppConfig::load();
    let configured =
        cfg.api_tagging_enabled && !cfg.api_key.trim().is_empty() && cfg.api_model.is_some();
    if !configured || lodestash_ai::openai::is_free_endpoint(&cfg.api_base_url) {
        return Ok(None);
    }
    let queued = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?
        .count_needing_tags();
    // One API call per asset (the cloud path is single-call — no caption pre-step), so the cap and
    // the queue depth together are the honest ceiling.
    let calls = if cfg.api_max_per_run == 0 {
        queued
    } else {
        queued.min(cfg.api_max_per_run as usize)
    };
    Ok(Some(serde_json::json!({
        "queued": queued,
        "calls": calls,
        "capped": cfg.api_max_per_run != 0 && queued > cfg.api_max_per_run as usize,
        "endpoint": cfg.api_base_url,
        "model": cfg.api_model,
    })))
}

/// The CLOUD card's endpoint presets (AI-PIPELINE §5 "Endpoint presets").
///
/// Served from the Rust `PRESETS` rather than restated in the frontend: `preset_for` and
/// `is_free_endpoint` already read that list to decide whether a run needs the budget confirm, and
/// a second copy in TypeScript would drift from the one the guard consults.
#[tauri::command]
fn api_presets() -> Vec<serde_json::Value> {
    lodestash_ai::openai::PRESETS
        .iter()
        .map(|p| {
            serde_json::json!({
                "label": p.label,
                "base_url": p.base_url,
                "model_hint": p.model_hint,
                "free": p.free,
            })
        })
        .collect()
}

/// The CLOUD card's **Test** action (AI-PIPELINE §5.1): one call against a generated sample image,
/// showing the model's raw reply or the raw error.
///
/// Runs against the **saved** config — the card writes on change, so what is tested is what a run
/// would use. `Ok` carries the assistant's unparsed text: a model that answers in prose rather than
/// the facet object is reachable but useless for tagging, and only the raw reply shows that.
/// A blocking call on the command thread is right here — the user pressed a button and is waiting,
/// and the probe carries its own 30 s timeout.
#[tauri::command]
fn test_api_tagger() -> Result<String, String> {
    let cfg = lodestash_app::config::AppConfig::load();
    let Some(model) = cfg.api_model.clone().filter(|m| !m.trim().is_empty()) else {
        return Err("No API model set — fill in the model id first.".into());
    };
    if cfg.api_key.trim().is_empty() {
        return Err("No API key set.".into());
    }
    lodestash_ai::openai::OpenAiTagger::new(cfg.api_base_url.clone(), cfg.api_key.clone(), model)
        .probe()
        .map_err(|e| e.to_string())
}

/// Bytes + file count under the preview cache root (Settings' CACHE card).
#[tauri::command]
fn preview_cache_usage(state: tauri::State<'_, AppState>) -> Result<(u64, usize), String> {
    let lib = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?;
    Ok(lib.preview_cache_usage())
}

/// Empty the preview cache (thumbs + mesh/inpack scratch — all regenerable). Runs under the
/// library lock; the workspace (extracted assets) is never touched.
#[tauri::command]
fn clear_preview_cache(state: tauri::State<'_, AppState>) -> Result<usize, String> {
    let lib = state
        .library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?;
    Ok(lib.clear_preview_cache())
}

/// Persist the Settings snapshot and re-read the library so the size tiers and source rows the
/// grid draws follow immediately (badges, ⚑ size facet, watch toggles).
#[tauri::command]
fn set_settings(state: tauri::State<'_, AppState>, settings: Settings) -> Result<(), String> {
    lodestash_app::config::edit_config(|c| settings.apply(c))?;
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
fn set_source_watch(
    state: tauri::State<'_, AppState>,
    id: i64,
    watch: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetSourceWatch { id, watch })
}

/// Every non-staging collection with its member count (UI-SPEC §6).
#[tauri::command]
fn collections(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<lodestash_core::catalog::CollectionRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.collections())
        .map_err(|_| "library lock poisoned".to_string())
}

/// The member ids of one collection — what a collection-filtered grid matches on (UI-SPEC §6).
#[tauri::command]
fn collection_members(
    state: tauri::State<'_, AppState>,
    collection_id: i64,
) -> Result<Vec<i64>, String> {
    state
        .library
        .lock()
        .map(|l| l.collection_members(collection_id))
        .map_err(|_| "library lock poisoned".to_string())
}

#[tauri::command]
fn create_collection(
    state: tauri::State<'_, AppState>,
    name: String,
) -> Result<WriteResult, String> {
    state.write(Op::CreateCollection { name })
}

#[tauri::command]
fn delete_collection(state: tauri::State<'_, AppState>, id: i64) -> Result<WriteResult, String> {
    state.write(Op::DeleteCollection { id })
}

/// Pin/unpin a `.unitypackage` as a library (FEATURES §5.4) — the Unity Assets screen's ⬢.
#[tauri::command]
fn set_package_pinned(
    state: tauri::State<'_, AppState>,
    id: i64,
    pinned: bool,
) -> Result<WriteResult, String> {
    state.write(Op::SetPackagePinned { id, pinned })
}

/// The assets related to `asset_id` through dependency edges (DATA-MODEL §10) — the
/// inspector's LINKED FILES / SIMILAR slices.
#[tauri::command]
fn related_assets(
    state: tauri::State<'_, AppState>,
    asset_id: i64,
) -> Result<Vec<lodestash_core::catalog::RelatedAsset>, String> {
    state
        .library
        .lock()
        .map(|l| l.related_assets(asset_id))
        .map_err(|_| "library lock poisoned".to_string())
}

/// Stage asset ids into the tray (UI-SPEC §16.2).
///
/// Through the writer and into the catalog, like every other mutation — this was an in-memory
/// `Vec` on `Library` until 2026-08-27, which meant the shell's tray and the one ENGINE-API §6
/// reads were different objects, and picks did not survive a restart (ADR-0010).
#[tauri::command]
fn stage(state: tauri::State<'_, AppState>, ids: Vec<i64>) -> Result<WriteResult, String> {
    state.write(Op::StageAssets { asset_ids: ids })
}

#[tauri::command]
fn unstage(state: tauri::State<'_, AppState>, ids: Vec<i64>) -> Result<WriteResult, String> {
    state.write(Op::UnstageAssets { asset_ids: ids })
}

#[tauri::command]
fn clear_staged(state: tauri::State<'_, AppState>) -> Result<WriteResult, String> {
    state.write(Op::ClearStaged)
}

/// The tray's **live** picks, in the order they were staged.
///
/// Trashed members are excluded by `collection_members` (UI-SPEC §7: a trashed asset leaves
/// staging everywhere), and so are assets whose row has gone — the tray only ever draws things
/// that still exist.
#[tauri::command]
fn staged(state: tauri::State<'_, AppState>) -> Result<Vec<i64>, String> {
    state
        .library
        .lock()
        .map(|l| l.staged_ids())
        .map_err(|_| "library lock poisoned".to_string())
}

/// The Stats Tag Graph (UI-SPEC §8.2): tag nodes, tag↔tag co-occurrence edges and the
/// `tags · links · assets` header counts. Read-only.
#[tauri::command]
fn tag_graph(
    state: tauri::State<'_, AppState>,
) -> Result<lodestash_core::catalog::TagGraph, String> {
    state
        .library
        .lock()
        .map(|l| l.tag_graph())
        .map_err(|_| "library lock poisoned".to_string())
}

/// Every resolved asset→asset edge (DATA-MODEL §10) — the set behind the LINKS filter and the
/// tag graph's dashed dependency arcs. Read-only.
#[tauri::command]
fn asset_edges(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<lodestash_core::catalog::AssetEdge>, String> {
    state
        .library
        .lock()
        .map(|l| l.asset_edges())
        .map_err(|_| "library lock poisoned".to_string())
}

/// One source's blacklist (UI-SPEC §11.1): every `{kind, value}` exclusion the crawler skips.
#[tauri::command]
fn source_excludes(
    state: tauri::State<'_, AppState>,
    source_id: i64,
) -> Result<Vec<lodestash_core::catalog::SourceExclude>, String> {
    state
        .library
        .lock()
        .map(|l| l.source_excludes(source_id))
        .map_err(|_| "library lock poisoned".to_string())
}

/// Add one `{kind, value}` exclusion to a source's blacklist (UI-SPEC §11.1 ⊘ add buttons).
#[tauri::command]
fn add_source_exclude(
    state: tauri::State<'_, AppState>,
    source_id: i64,
    kind: String,
    value: String,
) -> Result<WriteResult, String> {
    state.write(Op::AddSourceExclude {
        source_id,
        kind,
        value,
    })
}

/// Remove one `{kind, value}` exclusion from a source's blacklist (UI-SPEC §11.1 chip ✕).
#[tauri::command]
fn remove_source_exclude(
    state: tauri::State<'_, AppState>,
    source_id: i64,
    kind: String,
    value: String,
) -> Result<WriteResult, String> {
    state.write(Op::RemoveSourceExclude {
        source_id,
        kind,
        value,
    })
}

/// Every taxonomy for the Mapping tab's switcher (UI-SPEC §9.2), active first.
#[tauri::command]
fn taxonomies(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<lodestash_core::catalog::TaxonomyRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.taxonomies())
        .map_err(|_| "library lock poisoned".to_string())
}

/// Make `id` the single active taxonomy (UI-SPEC §9.2 `● active` marker).
#[tauri::command]
fn set_active_taxonomy(state: tauri::State<'_, AppState>, id: i64) -> Result<WriteResult, String> {
    state.write(Op::SetActiveTaxonomy { id })
}

/// Delete a taxonomy and everything under it (UI-SPEC §9.2).
#[tauri::command]
fn delete_taxonomy(state: tauri::State<'_, AppState>, id: i64) -> Result<WriteResult, String> {
    state.write(Op::DeleteTaxonomy { id })
}

/// Rename a category (UI-SPEC §9.2 row rename) — rewrites descendant paths in one transaction.
#[tauri::command]
fn rename_category(
    state: tauri::State<'_, AppState>,
    id: i64,
    label: String,
) -> Result<WriteResult, String> {
    state.write(Op::RenameCategory { id, label })
}

/// Delete a category and its whole subtree (UI-SPEC §9.2 row delete — assignments cascade).
#[tauri::command]
fn delete_category(state: tauri::State<'_, AppState>, id: i64) -> Result<WriteResult, String> {
    state.write(Op::DeleteCategory { id })
}

/// Add a category under `parent_id` (or the taxonomy root) with a display label and full path
/// (UI-SPEC §9.2). `path` is the materialized `Taxonomy/Category/…` chain the mapping matches on.
#[tauri::command]
fn add_category(
    state: tauri::State<'_, AppState>,
    taxonomy_id: i64,
    parent_id: Option<i64>,
    pref_label: String,
    path: String,
) -> Result<WriteResult, String> {
    state.write(Op::AddCategory {
        taxonomy_id,
        parent_id,
        pref_label,
        path,
    })
}

/// The mapping rules of one taxonomy, ordered by priority (UI-SPEC §9.2). Read-only.
#[tauri::command]
fn list_rules(
    state: tauri::State<'_, AppState>,
    taxonomy_id: i64,
) -> Result<Vec<lodestash_core::catalog::MappingRuleRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.list_rules(taxonomy_id))
        .map_err(|_| "library lock poisoned".to_string())
}

/// Run the deterministic mapping engine over one taxonomy (UI-SPEC §9.2 ⚙ Apply). Returns the
/// number of assets whose category changed. On the writer thread like every catalog write.
#[tauri::command]
fn apply_mapping(
    state: tauri::State<'_, AppState>,
    taxonomy_id: i64,
) -> Result<WriteResult, String> {
    state.write(Op::ApplyMapping { taxonomy_id })
}

/// ✦ Identify all — one bounded appraise pass on the persist thread (B11). Resolves the local
/// embedders (SigLIP/CLAP) when the models exist; with none, the pass runs the cheap drains
/// and the button's toast says nothing was appraised. Returns when the bounded pass finishes
/// (up to ~20s) — the frontend shows the appraise state meanwhile.
#[tauri::command]
fn identify_all(state: tauri::State<'_, AppState>) -> Result<WriteResult, String> {
    state.write(Op::IdentifyAll)
}

/// ✦ Appraise an explicit set — one bounded pass for the whole set, not one per asset.
///
/// What "Appraise this source" on the Identify dashboard (UI-SPEC §3) runs: a source or a pack is
/// the unit people think in, and asking for 400 assets must not mean 400 passes.
#[tauri::command]
fn identify_ids(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
) -> Result<WriteResult, String> {
    state.write(Op::IdentifyIds { asset_ids })
}

/// Appraise one asset now (per-card ✦, KANBAN B11b).
#[tauri::command]
fn identify_asset(state: tauri::State<'_, AppState>, asset_id: i64) -> Result<WriteResult, String> {
    state.write(Op::IdentifyOne { asset_id })
}

/// True while the writer thread is inside an appraise pass (B11d). The ✦ controls and the
/// Identify screen poll it; nothing races the writer on timing guesses.
#[tauri::command]
fn pump_active(state: tauri::State<'_, AppState>) -> bool {
    state.pump.load(std::sync::atomic::Ordering::Relaxed)
}

/// Appraise pass progress (B11d surface): `active` + the drain's done/total counters, so the
/// Identify screen can render a real bar instead of a spinner (or the drain's first `total`
/// write arrives only after the tag/embed queues are counted).
#[tauri::command]
fn pump_progress(state: tauri::State<'_, AppState>) -> PumpProgress {
    let progress = &state.appraise;
    PumpProgress {
        active: state.pump.load(std::sync::atomic::Ordering::Relaxed),
        done: progress.done.load(std::sync::atomic::Ordering::Relaxed),
        total: progress.total.load(std::sync::atomic::Ordering::Relaxed),
        failed: progress.failed.load(std::sync::atomic::Ordering::Relaxed),
    }
}

#[derive(serde::Serialize)]
struct PumpProgress {
    active: bool,
    done: u64,
    total: u64,
    failed: u64,
}

/// The embedded agent plane's bridge status (B12, ENGINE-API §1): the loopback port the REST
/// server bound (written into engine.json for the Unity plugin / Connect wizard), whether the
/// per-session token is set, and how many agent tools the allow-list left enabled (`null` =
/// every tool is live). `port: null` = no catalog on disk yet, so no servers.
#[tauri::command]
fn engine_bridge(state: tauri::State<'_, AppState>) -> EngineBridge {
    EngineBridge {
        port: state.engine_port,
        token_set: !state.engine_token.is_empty(),
        allowed: state.engine_allowed,
        live: state.engine.is_some(),
    }
}

#[derive(serde::Serialize)]
struct EngineBridge {
    live: bool,
    port: Option<u16>,
    token_set: bool,
    allowed: Option<usize>,
}

/// What `engine_import` decided (ENGINE-API §5.2) — the UI renders each honestly.
#[derive(serde::Serialize)]
#[serde(tag = "kind", content = "detail")]
enum EngineImportOutcome {
    /// Parked on the human approval gate — the job tray shows Approve / Deny.
    AwaitingApproval {
        job: serde_json::Value,
        /// ENGINE-API §5.4 — packs whose render pipeline disagrees with the project's. Carried on
        /// this plane too, not only REST/MCP: an advisory the desktop dropped would be a warning
        /// the agent gets and the person sitting in front of it does not.
        pipeline_mismatch: Vec<lodestash_core::catalog::PipelineAdvisory>,
    },
    /// The dup-guard (§5.4) left nothing new.
    AlreadyImported {
        ids: Vec<i64>,
        pipeline_mismatch: Vec<lodestash_core::catalog::PipelineAdvisory>,
    },
}

/// `⇢ Add to project` (FEATURES §7.2 / ENGINE-API §5.2) — queue an import into the **open**
/// Unity project through the embedded engine, so the approval gate + the plugin queue are one
/// surface. No registered editor errors honestly (the fallback is export-to-a-folder).
#[tauri::command]
fn engine_import(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
) -> Result<EngineImportOutcome, String> {
    let engine = state
        .engine
        .as_ref()
        .ok_or_else(|| "the engine bridge isn't running yet — no catalog on disk".to_string())?;
    match engine.request_import(lodestash_api::ImportRequest {
        project_id: None,
        asset_ids,
        dest_subfolder: None,
        with_dependencies: true,
    }) {
        Ok(lodestash_api::ImportKickoff::Awaiting {
            job,
            pipeline_mismatch,
        }) => Ok(EngineImportOutcome::AwaitingApproval {
            job: job.to_json(),
            pipeline_mismatch,
        }),
        Ok(lodestash_api::ImportKickoff::AllAlreadyImported {
            imported_existing,
            pipeline_mismatch,
        }) => Ok(EngineImportOutcome::AlreadyImported {
            ids: imported_existing,
            pipeline_mismatch,
        }),
        Err(lodestash_api::ImportError::NoProject) => Err(
            "no Unity project registered — open Lodestash in Unity (Settings → Unity Editors), or export to a folder instead".into(),
        ),
        Err(lodestash_api::ImportError::Empty) => Err("nothing to import".into()),
    }
}

/// **Export to folder** (UI-SPEC §16.2) — copy the picks to a folder the user chose, no editor and
/// no plugin involved.
///
/// This is the level-1 way out of the library, and until the Unity plugin exists it is the *only*
/// one: `engine_import` needs a registered project whose plugin drains the queue. A tool for
/// fetching assets has to be able to hand you the file.
///
/// Non-destructive in both directions. The originals are read, never moved (the standing
/// invariant), and the destination is sandboxed: absolute, no `..`, and **never inside a library
/// source root** — exporting into the library would turn a read-only index into a writer of the
/// user’s own asset folders.
///
/// Synchronous, because the toast reports what actually happened. In-pack picks are counted and
/// reported rather than silently unpacked: extraction is an explicit action, and an export that
/// quietly inflated a 2 GB archive would be a surprise, not a convenience.
#[tauri::command]
fn export_assets(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
    dest_path: String,
    with_dependencies: bool,
) -> Result<lodestash_api::export::ExportReport, String> {
    if asset_ids.is_empty() {
        return Err("nothing to export".into());
    }
    let engine = state
        .engine
        .as_ref()
        .ok_or_else(|| "no catalog is open — there is nothing to export".to_string())?;
    engine.export_now(lodestash_api::export::ExportRequest {
        asset_ids,
        dest_path,
        with_dependencies,
    })
}

/// Approve an engine job on the approval gate (ENGINE-API §4) — the import then goes to the
/// Unity plugin's queue.
#[tauri::command]
fn engine_approve(state: tauri::State<'_, AppState>, job_id: String) -> bool {
    match state.engine.as_ref() {
        Some(engine) => {
            engine.approve_job(&job_id);
            true
        }
        None => false,
    }
}

/// Deny (reject) a job on the approval gate — the pending request is dropped.
#[tauri::command]
fn engine_deny(state: tauri::State<'_, AppState>, job_id: String) -> bool {
    match state.engine.as_ref() {
        Some(engine) => {
            engine.deny_job(&job_id);
            true
        }
        None => false,
    }
}

/// The engine jobs currently waiting on human approval (ENGINE-API §4) — the job tray's
/// "agent imports" section.
#[tauri::command]
fn engine_approvals(state: tauri::State<'_, AppState>) -> Vec<serde_json::Value> {
    let Some(engine) = state.engine.as_ref() else {
        return Vec::new();
    };
    let jobs = engine.jobs.awaiting_approval();
    jobs.iter().map(|j| j.to_json()).collect()
}

/// One row of the Settings MODELS section — installed vs not, per model.
#[derive(serde::Serialize)]
struct ModelStatus {
    id: &'static str,
    label: &'static str,
    installed: bool,
    size_mb: u32,
}

/// AI model install status (AI-PIPELINE §2 / AI-MODELS-BY-GPU): SigLIP 2 (visual search),
/// CLAP (audio search) and the ONNX Runtime dll they load. The downloader is
/// `lodestash_app::models` — this just reports what's on disk.
#[tauri::command]
fn models_status() -> Vec<ModelStatus> {
    vec![
        ModelStatus {
            id: "siglip",
            label: "SigLIP 2 — visual search + tagging",
            installed: lodestash_app::models::siglip_installed(),
            size_mb: 412,
        },
        ModelStatus {
            id: "clap",
            label: "CLAP — audio search + tagging",
            installed: lodestash_app::models::clap_installed(),
            size_mb: 163,
        },
        ModelStatus {
            id: "ort",
            label: "ONNX Runtime (native dll)",
            installed: lodestash_app::models::ort_installed(),
            size_mb: 70,
        },
    ]
}

/// Start a model download (Settings MODELS section). Runs on its own worker thread — the
/// download never touches the catalog, so the single-writer rule doesn't apply — and reports
/// through the job tray's progress bar. The ORT dll only loads at startup: a fresh download
/// needs an app restart before search actually uses it (noted in the UI).
#[tauri::command]
fn model_download(state: tauri::State<'_, AppState>, model: String) -> bool {
    use std::sync::atomic::Ordering;
    let jobs = state.jobs.clone();
    let handles = jobs.start(format!("Download {model}"), "download", true);
    // The tray renders the writer's own progress atomics; the downloader writes the app's.
    // A pump copies one into the other while the download runs.
    let app_progress: std::sync::Arc<lodestash_app::jobs::JobProgress> =
        std::sync::Arc::new(lodestash_app::jobs::JobProgress::default());
    let writer_progress = handles.progress.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let done = done.clone();
        let writer_progress = writer_progress.clone();
        let app_progress = app_progress.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if let Some(w) = &writer_progress {
                    w.done
                        .store(app_progress.done.load(Ordering::Relaxed), Ordering::Relaxed);
                    w.total.store(
                        app_progress.total.load(Ordering::Relaxed),
                        Ordering::Relaxed,
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        });
    }
    std::thread::spawn(move || {
        let result = match model.as_str() {
            "siglip" => lodestash_app::models::download_siglip(app_progress.clone()),
            "clap" => lodestash_app::models::download_clap(app_progress.clone()),
            "ort" => lodestash_app::models::download_ort(app_progress.clone()),
            other => Err(format!("unknown model {other:?}")),
        };
        done.store(true, Ordering::Relaxed);
        let ok = result.is_ok();
        if let Err(e) = &result {
            tracing::error!(%e, "model download failed");
        }
        jobs.finish(handles.id, ok, false, 0);
    });
    true
}

/// Remove the SigLIP + CLAP model dirs (Settings MODELS "Remove") so they re-download clean.
#[tauri::command]
fn model_remove() -> Result<(), String> {
    lodestash_app::models::remove_models()
}

/// Per-pack Extract (B2b slice) — unpack the archive into the workspace and reconcile its
/// payloads into the catalog, on the persist thread like every write.
///
/// **Returns "accepted", not "finished."** Extraction hands off to a worker thread; progress, the
/// outcome and the cancel button live in the job tray, and the grid refreshes off the `catalog`
/// change event the writer emits once payloads are materialized. Deliberately *not* `state.write`:
/// that reloads the 8k-row library snapshot the moment the op returns, which for this op is the
/// moment the worker is spawned — before a single payload exists. A wasted whole-library re-read
/// at exactly the wrong time.
#[tauri::command]
fn extract_package(
    state: tauri::State<'_, AppState>,
    package_id: i64,
) -> Result<WriteResult, String> {
    state
        .writer
        .as_ref()
        .ok_or_else(|| "no catalog is open — nothing to write to".to_string())?
        .run(Op::ExtractPackage { package_id })
}

/// The job tray (B2b): active jobs (progress, cancellable) + the session ring of finished ones.
#[tauri::command]
fn jobs(state: tauri::State<'_, AppState>) -> crate::writer::JobsView {
    state.jobs.snapshot()
}

/// Ask one active job to stop (extract workers cancel between reads; the rest ignore it).
#[tauri::command]
fn cancel_job(state: tauri::State<'_, AppState>, job_id: u64) -> bool {
    state.jobs.cancel(job_id)
}

/// Registered Unity editors, active first (ENGINE-API §5.1) — the Connect wizard's app half.
#[tauri::command]
fn projects(state: tauri::State<'_, AppState>) -> Vec<lodestash_core::catalog::ProjectRow> {
    state
        .library
        .lock()
        .map(|l| l.projects())
        .unwrap_or_default()
}

/// Switch which registered Unity editor is active (ENGINE-API §5.1, UI-SPEC §16.1). A write,
/// so it goes through the writer thread with every other catalog mutation.
#[tauri::command]
fn set_active_project(
    state: tauri::State<'_, AppState>,
    project_id: i64,
) -> Result<WriteResult, String> {
    state.write(Op::SetActiveProject { project_id })
}

/// One persisted import batch (UI-SPEC §16.1 Recent exports): every `import_record` sharing a
/// `batch_id` — what one import run copied into a project, undoable as a unit.
#[derive(serde::Serialize)]
pub struct ImportBatch {
    pub batch_id: String,
    pub project_id: i64,
    pub project_name: String,
    /// The destination folder the batch's files were written into (e.g. `Assets/Wildlife`).
    pub dest: String,
    pub file_count: i64,
    pub imported_at: i64,
}

/// The last `limit` import batches, newest first (UI-SPEC §16.1). A read — the short-lived
/// connection never runs migrations and never writes (ARCHITECTURE §5 allows per-task readers).
#[tauri::command]
fn recent_exports(limit: usize) -> Result<Vec<ImportBatch>, String> {
    let Some(db) = lodestash_app::paths::catalog_db_path() else {
        return Ok(Vec::new());
    };
    if !db.exists() {
        return Ok(Vec::new());
    }
    let limit = limit.clamp(1, 100);
    let conn = Connection::open(&db).map_err(|e| format!("catalog unavailable: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT ir.batch_id, ir.project_id, p.name, COUNT(*), MAX(ir.imported_at),
                    (SELECT i2.dest_path FROM import_record i2 WHERE i2.batch_id = ir.batch_id ORDER BY i2.id LIMIT 1)
               FROM import_record ir
               JOIN project p ON p.id = ir.project_id
              WHERE ir.batch_id IS NOT NULL
              GROUP BY ir.batch_id
              ORDER BY MAX(ir.imported_at) DESC
              LIMIT ?1",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([limit as i64], |r| {
            let sample: Option<String> = r.get(5)?;
            Ok(ImportBatch {
                batch_id: r.get(0)?,
                project_id: r.get(1)?,
                project_name: r.get(2)?,
                dest: dest_directory(sample.as_deref()),
                file_count: r.get(3)?,
                imported_at: r.get(4)?,
            })
        })
        .map_err(|e| e.to_string())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())
}

/// The parent folder of an in-project import path (`Assets/Wildlife/Beaver.fbx` →
/// `Assets/Wildlife`) — what the Recent exports row reads as the destination.
fn dest_directory(dest_path: Option<&str>) -> String {
    let Some(p) = dest_path else {
        return String::new();
    };
    let p = p.trim_end_matches(['/', '\\']);
    match p.rfind(['/', '\\']) {
        Some(i) if i > 0 => p[..i].to_string(),
        _ => String::new(),
    }
}

/// Undo an import batch (UI-SPEC §16.1): delete the batch's copied files from the project and
/// its `import_record` rows — catalog assets are never touched. Runs on the single writer.
#[tauri::command]
fn undo_import_batch(
    state: tauri::State<'_, AppState>,
    batch_id: String,
) -> Result<WriteResult, String> {
    state.write(Op::UndoImportBatch { batch_id })
}

/// Add a mapping rule to a taxonomy (UI-SPEC §9.2). `match_type` is one of the shared
/// `MAPPING_MATCH_TYPES`; `priority` lower wins. On the writer thread like every catalog write.
#[tauri::command]
fn add_rule(
    state: tauri::State<'_, AppState>,
    taxonomy_id: i64,
    category_id: i64,
    match_type: String,
    pattern: String,
    priority: i64,
) -> Result<WriteResult, String> {
    state.write(Op::AddRule {
        taxonomy_id,
        category_id,
        match_type,
        pattern,
        priority,
    })
}

/// Delete a mapping rule (UI-SPEC §9.2 row delete). On the writer thread like every catalog write.
#[tauri::command]
fn delete_rule(state: tauri::State<'_, AppState>, rule_id: i64) -> Result<WriteResult, String> {
    state.write(Op::DeleteRule { rule_id })
}

/// The newest undo-log rows, newest first, capped at `limit` (UI-SPEC §2 Ctrl/Cmd-Z / L5).
#[tauri::command]
fn recent_ops(
    state: tauri::State<'_, AppState>,
    limit: usize,
) -> Result<Vec<lodestash_core::catalog::OpLogRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.recent_ops(limit))
        .map_err(|_| "library lock poisoned".to_string())
}

/// The redo stack (UI-SPEC §2 Shift-Z): the newest undone ops, newest first.
#[tauri::command]
fn redo_ops(
    state: tauri::State<'_, AppState>,
    limit: usize,
) -> Result<Vec<lodestash_core::catalog::OpLogRow>, String> {
    state
        .library
        .lock()
        .map(|l| l.redo_candidates(limit))
        .map_err(|_| "library lock poisoned".to_string())
}

/// Undo the logged operation `op_id` (UI-SPEC §2 / DESIGN-GUIDELINES §7 L5). Runs on the writer
/// thread like every catalog mutation; a reverted op is removed from the log.
#[tauri::command]
fn undo(state: tauri::State<'_, AppState>, op_id: i64) -> Result<WriteResult, String> {
    state.write(Op::RevertOp { op_id })
}

/// Redo one undone op (UI-SPEC §2 Shift-Z) — the mirror of `undo`.
#[tauri::command]
fn redo(state: tauri::State<'_, AppState>, op_id: i64) -> Result<WriteResult, String> {
    state.write(Op::RedoOp { op_id })
}

/// Toggle one asset's membership in a collection (UI-SPEC §6 / §13.5). Both directions go
/// through the writer so they can never interleave with another mutation.
#[tauri::command]
fn collection_add_asset(
    state: tauri::State<'_, AppState>,
    collection_id: i64,
    asset_id: i64,
) -> Result<WriteResult, String> {
    state.write(Op::CollectionAddAsset {
        collection_id,
        asset_id,
    })
}

#[tauri::command]
fn collection_remove_asset(
    state: tauri::State<'_, AppState>,
    collection_id: i64,
    asset_id: i64,
) -> Result<WriteResult, String> {
    state.write(Op::CollectionRemoveAsset {
        collection_id,
        asset_id,
    })
}

/// Bulk-bar tag edit (KANBAN B9d): add/remove one `(namespace, value)` tag on many assets at
/// once. Each asset's prior state is snapshotted to the undo log before the write.
#[tauri::command]
fn bulk_tag(
    state: tauri::State<'_, AppState>,
    asset_ids: Vec<i64>,
    namespace: String,
    value: String,
    on: bool,
) -> Result<WriteResult, String> {
    state.write(Op::BulkTag {
        asset_ids,
        namespace,
        value,
        on,
    })
}

/// The REST/MCP `PreviewFn` (ENGINE-API preview endpoints): resolve the asset's cached thumbnail,
/// generating on a miss — the same deterministic key the `lodestash://thumb` route uses. Shared by
/// the embedded engine (B12) and the `--mcp` headless server.
fn engine_preview_fn(cache_root: Option<std::path::PathBuf>) -> lodestash_api::PreviewFn {
    Box::new(move |subject| {
        use lodestash_ingest::thumb::{generate, generate_material, meta_key, ThumbParams};
        let cache = cache_root.as_deref()?;
        let q = lodestash_app::config::AppConfig::load().preview_quality;
        let params = ThumbParams {
            width: q.thumb_px(),
            height: q.thumb_px(),
            proxy: q.proxy_budget(),
        };

        // A material has no pixels of its own — it wears a texture reached through the graph
        // (DATA-MODEL §10), so it is rendered as a shader ball rather than decoded.
        //
        // Both halves of that picture can still be inside a pack, and the old shape assumed
        // neither was: it read the key off the material's own `disk_path`, which an in-pack
        // material does not have, so it returned `None` before rendering anything — every
        // material in every un-extracted pack. Key off whichever locator exists, and pull an
        // in-pack albedo through the same scratch the asset payload uses.
        if subject.media_type == "material" {
            let albedo = match (&subject.albedo, &subject.albedo_in_pack) {
                (Some(p), _) => Some(p.clone()),
                (None, Some((pack, offset, size, ext))) => {
                    lodestash_app::thumbs::ensure_in_pack_scratch(
                        Some(cache),
                        pack,
                        *offset,
                        *size,
                        ext,
                    )
                    .map(|(p, _)| p)
                }
                _ => None,
            };
            let key = match (&subject.disk_path, &subject.in_pack) {
                (Some(p), _) => meta_key(p, std::fs::metadata(p).map(|m| m.len()).unwrap_or(0), 0),
                // The same formula `ensure_in_pack_scratch` keys its scratch with: a pack payload
                // is identified by its archive, offset and size, and there is no file to stat.
                (None, Some((pack, offset, size, _))) => {
                    meta_key(std::path::Path::new(pack), *size as u64, *offset)
                }
                _ => return None,
            };
            // The texture is part of the picture, so it is part of the key (INGESTION
            // "Material previews"): swapping a material's albedo must not serve the old ball.
            let key = match &albedo {
                Some(a) => format!("{key}-{}", meta_key(a, 0, 0)),
                None => format!("{key}-flat"),
            };
            return generate_material(cache, &key, params, albedo.as_deref()).ok();
        }

        // On disk: render from the file. Still in a pack: pull just this payload into the shared
        // in-pack scratch first (INGESTION §2 surgical pull) — one gzip skip-scan, cached, and
        // the same scratch the grid's own thumbnail route uses, so the two never disagree.
        let owned;
        let src = match (&subject.disk_path, &subject.in_pack) {
            (Some(p), _) => p.as_path(),
            (None, Some((pack, offset, size, ext))) => {
                let (path, _) = lodestash_app::thumbs::ensure_in_pack_scratch(
                    Some(cache),
                    pack,
                    *offset,
                    *size,
                    ext,
                )?;
                owned = path;
                owned.as_path()
            }
            _ => return None,
        };
        let key = meta_key(src, std::fs::metadata(src).map(|m| m.len()).unwrap_or(0), 0);
        // A model is painted the same way here as on the grid (v1 plan S08) — one resolution, so an
        // agent and the person at the machine are shown the same picture. Anything unpainted, too
        // big, or unparseable falls back to the untextured hero render.
        if subject.media_type == "model_3d" {
            if let Ok(p) = lodestash_ingest::thumb::generate_model_textured(
                src,
                cache,
                &key,
                params,
                &subject.slot_textures,
            ) {
                return Some(p);
            }
        }
        generate(src, cache, &key, params).ok()
    })
}

/// Apply the Settings MCP allow-list (UI-SPEC §16): config holds the disabled half, the state the
/// enabled set. Returns how many tools stayed enabled (`None` = every tool is live).
fn apply_allow_list(state: &lodestash_api::ServerState) -> Option<usize> {
    let cfg = lodestash_app::config::AppConfig::load();
    if cfg.mcp_disabled_tools.is_empty() {
        return None;
    }
    let enabled: std::collections::HashSet<String> = lodestash_api::AGENT_OPERATIONS
        .iter()
        .map(|(n, _)| n.to_string())
        .filter(|n| !cfg.mcp_disabled_tools.contains(n))
        .collect();
    state.set_allowed(enabled.clone());
    Some(enabled.len())
}

/// The agent planes' write bridge (ADR-0002): each [`lodestash_api::AgentWrite`] maps to the
/// single-writer `Op` and blocks on its reply — the API crate never touches the catalog itself.
fn engine_write_bridge(writer: Writer) -> lodestash_api::AgentWriteFn {
    use lodestash_api::AgentWrite;
    use lodestash_core::catalog::TagMode;
    // Everything through here is an agent's write — the shell shows it as a pending update
    // instead of re-reading under the user (ADR-0016).
    let run = move |op: Op| writer.run_as(op, crate::writer::Origin::Agent);
    Box::new(move |w: AgentWrite| match w {
        // The legacy one-tag operations write as the agent they are — never as the person at the
        // machine (v1 plan S05). They carry no producer of their own, so they land under
        // `ai:legacy-agent`, and a removal reaches only what that producer added.
        AgentWrite::AddTag {
            asset_id,
            namespace,
            value,
        } => run(Op::TagBatch {
            batch: legacy_agent_batch(asset_id, namespace, value, TagMode::Append),
        })
        .map(|r| r.changed),
        AgentWrite::RemoveTag {
            asset_id,
            namespace,
            value,
        } => run(Op::TagBatch {
            batch: legacy_agent_batch(asset_id, namespace, value, TagMode::RemoveScoped),
        })
        .map(|r| r.changed),
        AgentWrite::TagBatch { request, digest } => run(Op::TagBatch {
            batch: request.to_core_batch(&digest),
        })
        .map(|r| r.changed),
        AgentWrite::TagReview { batch } => run(Op::TagReview { batch }).map(|r| r.changed),
        AgentWrite::ApplyMapping { taxonomy_id } => {
            run(Op::ApplyMapping { taxonomy_id }).map(|r| r.changed)
        }
        AgentWrite::Identify { asset_ids } => run(Op::IdentifyIds { asset_ids }).map(|r| r.changed),
        AgentWrite::UnpackPackage { package_id } => {
            run(Op::ExtractPackage { package_id }).map(|r| r.changed)
        }
        AgentWrite::StageAsset { asset_id } => run(Op::StageAssets {
            asset_ids: vec![asset_id],
        })
        .map(|r| r.changed),
        AgentWrite::UnstageAsset { asset_id } => run(Op::UnstageAssets {
            asset_ids: vec![asset_id],
        })
        .map(|r| r.changed),
        AgentWrite::RegisterProject {
            project_path,
            unity_version,
            pipeline,
        } => run(Op::RegisterProject {
            project_path,
            unity_version,
            pipeline,
        })
        .map(|r| r.changed),
        AgentWrite::RecordImports {
            project_id,
            specs,
            batch_id,
        } => run(Op::RecordImports {
            project_id,
            specs,
            batch_id,
        })
        .map(|r| r.changed),
        AgentWrite::RecordExports {
            batch_id,
            dest_path,
            files,
        } => run(Op::RecordExports {
            batch_id,
            dest_path,
            files,
        })
        .map(|r| r.changed),
    })
}

/// A legacy agent tag operation as a batch under `ai:legacy-agent` (v1 plan S05).
fn legacy_agent_batch(
    asset_id: i64,
    namespace: String,
    value: String,
    mode: lodestash_core::catalog::TagMode,
) -> lodestash_core::catalog::TagBatch {
    use lodestash_core::catalog::{
        ai_source, HistoryOrigin, TagAssignment, TagBatch, TagBatchItem,
    };
    TagBatch {
        items: vec![TagBatchItem {
            asset_id,
            source: ai_source("legacy-agent"),
            model_id: None,
            mode,
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
    }
}

/// Build the shared engine `ServerState` (B12) — one read-only catalog connection, the allow-list
/// applied, the per-session token minted. `Err` says why there is none: no catalog on disk yet, or
/// one this build cannot read without the app upgrading it first (v1 plan S11).
fn build_engine_state() -> Result<
    (
        std::sync::Arc<lodestash_api::ServerState>,
        String,
        Option<usize>,
    ),
    String,
> {
    let db = lodestash_app::paths::catalog_db_path()
        .ok_or("no data directory for the catalog on this system")?;
    if !db.exists() {
        return Err("no catalog on disk yet — open the app once to create it".into());
    }
    let (cache_root, workspace) = lodestash_app::paths::default_cache_dirs();
    let payload_cache = cache_root.clone();
    let state = lodestash_api::ServerState::new(
        &db,
        workspace.unwrap_or_else(|| cache_root.clone().unwrap_or_default()),
        engine_preview_fn(cache_root),
    )
    .map_err(|e| e.to_string())?;
    // In-pack files are served from a verified pull of just their payload (ENGINE-API §8.2, v1 plan
    // S12b) — never an unpack, never a catalog write.
    if let Some(cache) = payload_cache {
        state.set_payload_resolver(Box::new(move |entry| {
            use lodestash_api::AgentError;
            use lodestash_ingest::unitypackage::PayloadError;
            lodestash_app::thumbs::materialize_in_pack_entry(&cache, entry).map_err(|e| {
                let why = e.to_string();
                match e {
                    PayloadError::ArchiveMissing(_) => AgentError::archive_missing(why),
                    PayloadError::SourceChanged(_) => AgentError::source_changed(why),
                    PayloadError::TooLarge { .. } => AgentError::asset_too_large(why),
                    PayloadError::Io(_) => AgentError::not_local(why),
                }
            })
        }));
    }
    let state = std::sync::Arc::new(state);
    let allowed = apply_allow_list(&state);
    // ENGINE-API §8.3: `write_tags` writes under the producer the user configured for this host,
    // and refuses without one.
    let cfg = lodestash_app::config::AppConfig::load_display_only();
    state.set_agent_producer(cfg.agent_producer_id);
    // Whether an agent may receive bytes at all (v1 plan S10) — metadata and links are always
    // available, the files are not until the user says so.
    state.set_byte_release(cfg.mcp_bytes_enabled);
    if let Err(error) = state.set_vocabulary_policy(cfg.mcp_vocabulary_policy) {
        tracing::warn!(%error, "invalid MCP vocabulary policy; using extend_with_approval");
    }
    // The REST/MCP semantic-search plane (ENGINE-API §2 `mode=ai`) embeds the query exactly as the
    // grid's AI mode does — SigLIP and CLAP, resolved lazily on the first ai search (model load is
    // seconds; a startup resolve would slow every launch). No spaces means no model → the route
    // answers `ai_unavailable` instead of degrading.
    state.set_query_embedder(Box::new(lodestash_app::drains::query_spaces));
    let token = state.token().to_string();
    Ok((state, token, allowed))
}

/// `--mcp` headless: the MCP stdio server without the GUI. Runs until stdin closes; `tracing`
/// already initialized by `run()`. Exits non-zero if there is no catalog it can serve.
fn run_mcp_headless() {
    let (state, _token, _allowed) = match build_engine_state() {
        Ok(built) => built,
        Err(why) => {
            eprintln!("lodestash-desktop --mcp: {why}");
            std::process::exit(2);
        }
    };
    // Everything the app owns — writes, jobs, approvals — goes to the running app, never to a
    // second writer or a second job registry in this process (ADR-0002, v1 plan S11). The line
    // re-reads the app's handshake on every call, so an app started (or restarted) after this
    // server is found; with no app running, those tools say so and reads keep working.
    match state.catalog_id() {
        Ok(id) => state.set_app_proxy(crate::agent_bridge::app_proxy(id)),
        Err(e) => {
            eprintln!("lodestash-desktop --mcp: {e}");
            std::process::exit(2);
        }
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("mcp runtime builds");
    if let Err(e) = rt.block_on(lodestash_mcp::serve_stdio(state)) {
        tracing::error!(%e, "mcp server exited");
        std::process::exit(1);
    }
}

pub fn run() {
    tracing_subscriber::fmt()
        // stderr, always. Under `--mcp` stdout *is* the JSON-RPC channel, and the fmt layer's
        // default writer is stdout: one log line there is a malformed message to the host.
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lodestash_desktop=info".into()),
        )
        .init();

    // Point `ort` at the runtime the model manager downloaded, before anything tries to load an
    // embedder. `ort` is built with `load-dynamic` (TECH-STACK §5), so it dlopens
    // `onnxruntime.dll` from `ORT_DYLIB_PATH` and *panics* when it cannot — which the loaders
    // catch and report as "ONNX Runtime unavailable or incompatible". That message is what this
    // machine had been showing since the shell was ported: the DLL and both model sets were on
    // disk the whole time and `apply_ort_dylib_env` had **no callers**, so nothing ever set the
    // variable. The egui `main` called it; the port dropped the call and the error text made it
    // look like a broken runtime rather than an unset path. Consequences: no new embeddings, no
    // SigLIP/CLAP zero-shot tagging, and — because promotion needs a clean embedding — assets
    // that could never become Identified.
    //
    // Here rather than inside the loaders: setting an environment variable is only sound while
    // the process is still single-threaded, and this is before the writer, the REST server and
    // Tauri's own threads exist.
    lodestash_app::models::apply_ort_dylib_env();

    // `--mcp` (ENGINE-API §2, MCP↔REST 1:1): serve the MCP stdio plane headless — an agent host
    // spawns `lodestash-desktop --mcp` and speaks JSON-RPC on stdin/stdout. The process owns
    // stdin until the host closes it; no window, no REST server, no writer thread.
    //
    // After the runtime path, not before (v1 plan S11): this branch used to return first, so a
    // host-spawned server — which inherits no `ORT_DYLIB_PATH` from anyone — could never load the
    // query embedder, and `semantic_search` answered "no embedding model" on a machine that had
    // every model installed.
    if std::env::args().any(|a| a == "--mcp") {
        run_mcp_headless();
        return;
    }

    // A debug build's window loads the dev server, so the app starts it rather than expecting the
    // person launching to have started it in another terminal. Release builds serve `ui/dist` from
    // the bundle and skip this entirely.
    #[cfg(debug_assertions)]
    devserver::ensure();

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

    // One writer for the process (ARCHITECTURE §5), spawned in `setup` — it needs the
    // AppHandle to push change events (pump events, B2b) to the Svelte shell.
    let pump = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let appraise = std::sync::Arc::new(lodestash_app::jobs::JobProgress::default());
    let focus = std::sync::Arc::new(Mutex::new(lodestash_core::catalog::AppraiseFocus::default()));
    let job_registry = crate::writer::JobRegistry::default();

    tauri::Builder::default()
        // Window geometry (size/position/maximized) is remembered across launches and restored
        // before the window is shown — the window opens where the user left it instead of
        // re-applying the launch defaults and jumping. State file lives in the app config dir.
        .plugin(tauri_plugin_window_state::Builder::default().build())
        // Native dialogs (B2b): the sidebar's "+ Add source…" folder chooser opens from Rust.
        .plugin(tauri_plugin_dialog::init())
        // The plugin writes the state file only on a graceful exit (`RunEvent::Exit`). A
        // killed process — dev Ctrl+C, the e2e harness's taskkill — never reaches it, so the
        // last move/resize is lost and the next launch restores a stale geometry. Persist on
        // Moved/Resized too, throttled to ~3 writes/sec so a drag doesn't hammer the disk.
        .on_window_event(|window, event| {
            if matches!(
                event,
                tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_)
            ) {
                let mut last = LAST_GEOMETRY_SAVE.lock().unwrap();
                if last
                    .map(|t| t.elapsed() >= Duration::from_millis(300))
                    .unwrap_or(true)
                {
                    *last = Some(Instant::now());
                    drop(last);
                    let _ = window
                        .app_handle()
                        .save_window_state(tauri_plugin_window_state::StateFlags::all());
                }
            }
        })
        .setup(|app| {
            // One writer for the process (ARCHITECTURE §5), spawned before any command can
            // run — setup runs before the event loop. It carries the AppHandle so changes
            // are pushed to the shell (pump events, B2b) instead of the UI polling.
            let writer = lodestash_app::paths::catalog_db_path()
                .filter(|p| p.exists())
                .map(|p| {
                    Writer::spawn(
                        p,
                        pump.clone(),
                        job_registry.clone(),
                        appraise.clone(),
                        focus.clone(),
                    )
                });
            // Change publication (ADR-0016): the writer and the job registry only mark what
            // changed; this thread tells the shell, at most once per window. Started with or
            // without a catalog — model downloads use the tray too.
            job_registry.changes().start_publisher(app.handle().clone());

            // B12 — the agent plane (ENGINE-API §1–2, MCP↔REST 1:1) lives in this process:
            // the REST server binds an ephemeral loopback port and writes engine.json for the
            // Unity plugin / Connect wizard; the MCP stdio server answers on stdin when a host
            // pipes it (a console or no-stdin launch idles or exits on EOF — a background
            // thread either way). Both share one ServerState with its own read-only catalog
            // connection — the API never writes the catalog, so the single-writer rule holds.
            let (engine_state, engine_port, engine_token, engine_allowed) =
                match build_engine_state() {
                    Ok((state, token, allowed)) => {
                        // Wire the write bridge before serving: mutating agent ops (add_tag,
                        // apply_mapping, identify, unpack, staging) go through the single writer.
                        if let Some(w) = writer.clone() {
                            state.set_write(engine_write_bridge(w));
                        }
                        let port = lodestash_api::serve(state.clone())
                            .map(|h| h.port)
                            .map_err(|e| tracing::error!(%e, "engine API failed to bind"))
                            .ok();
                        // MCP stdio plane on its own thread — self-terminates on stdin EOF.
                        let state_arc = state.clone();
                        std::thread::Builder::new()
                            .name("lodestash-mcp".into())
                            .spawn(move || {
                                let rt = tokio::runtime::Builder::new_current_thread()
                                    .enable_all()
                                    .build()
                                    .expect("mcp runtime builds");
                                let _ = rt.block_on(lodestash_mcp::serve_stdio(state));
                            })
                            .ok();
                        (Some(state_arc), port, token, allowed)
                    }
                    Err(why) => {
                        tracing::warn!(%why, "agent planes not started");
                        (None, None, String::new(), None)
                    }
                };
            app.manage(AppState {
                library: Mutex::new(library),
                writer,
                pump,
                appraise,
                focus,
                jobs: job_registry,
                engine: engine_state,
                engine_port,
                engine_token,
                engine_allowed,
                discovered: Mutex::new(std::collections::HashMap::new()),
            });
            Ok(())
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
            near_duplicate_groups,
            asset_detail,
            reload_library,
            set_favorite,
            set_confirmed,
            set_link_ignored,
            similar_assets,
            workspace_usage,
            reclaim_workspace,
            workspace_packs,
            set_pack_keep_cached,
            retarget_preview,
            retarget_package,
            pick_package_file,
            refresh_source_health,
            relink_preview,
            relink_source,
            set_appraise_focus,
            set_trashed,
            remove_from_library,
            delete_from_disk,
            add_source,
            sync_unity_cache,
            unity_cache_candidates,
            add_source_dialog,
            pick_folder,
            discover_package_folder,
            import_packages,
            get_web_prefs,
            set_web_prefs,
            pack_dir_tree,
            upsert_trait,
            unpromote_trait,
            set_trait_teach,
            set_trait_style,
            get_settings,
            set_settings,
            test_api_tagger,
            api_presets,
            cloud_run_estimate,
            preview_cache_usage,
            clear_preview_cache,
            set_source_watch,
            collections,
            collection_members,
            create_collection,
            delete_collection,
            set_package_pinned,
            tag_graph,
            asset_edges,
            source_excludes,
            add_source_exclude,
            remove_source_exclude,
            taxonomies,
            set_active_taxonomy,
            delete_taxonomy,
            rename_category,
            delete_category,
            recent_ops,
            redo_ops,
            undo,
            redo,
            collection_add_asset,
            collection_remove_asset,
            bulk_tag,
            related_assets,
            stage,
            unstage,
            clear_staged,
            staged,
            add_category,
            list_rules,
            apply_mapping,
            add_rule,
            delete_rule,
            identify_all,
            identify_asset,
            identify_ids,
            pump_active,
            pump_progress,
            engine_bridge,
            engine_import,
            export_assets,
            recent_folder_exports,
            undo_export_batch,
            engine_approve,
            engine_deny,
            engine_approvals,
            models_status,
            model_download,
            model_remove,
            extract_package,
            jobs,
            cancel_job,
            projects,
            set_active_project,
            recent_exports,
            undo_import_batch
        ])
        // `build` + `run`, not `run` alone: Tauri ends the process when the last window closes, so
        // anything after a bare `.run(...)` never executes. The exit event is where cleanup lands.
        .build(tauri::generate_context!())
        .expect("error while running Lodestash")
        .run(|_handle, event| {
            // Only what this process started (`devserver::ensure`) — a server that was already
            // listening belongs to whoever started it. A hard kill runs nothing, and the next
            // launch reuses the orphan rather than starting a second one.
            #[cfg(debug_assertions)]
            if matches!(event, tauri::RunEvent::Exit) {
                devserver::shutdown();
            }
            let _ = &event;
        });
}
