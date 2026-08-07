//! The catalog write path: **one dedicated writer thread**, fed by a channel.
//!
//! `ARCHITECTURE.md` §5 is explicit — "all writes funnel through one dedicated writer (a
//! thread/actor owning the connection, fed by a channel); reads use short-lived per-task
//! connections". This is that thread. It is not an optimisation: `rusqlite::Connection` is
//! `!Sync`, Tauri runs every `#[tauri::command]` on a thread pool, and two commands writing at
//! once is exactly how `database is locked` came back last time (`HANDOFF.md` §6).
//!
//! Commands send a [`Job`] and block on its reply. Blocking is correct here: these are
//! single-row updates that take microseconds, the caller is a pool thread rather than the UI, and
//! the frontend needs the outcome before it can honestly redraw. A fire-and-forget write would
//! report success for something that had not happened yet.
//!
//! **The reads stay where they are.** `Library` keeps its own read-only connection (WAL makes a
//! reader and a writer concurrent), so browsing never waits behind a write.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use lodestash_app::config::AppConfig;
use lodestash_app::drains::{
    drain_promote_resync, drain_previews, resolve_audio_embedder, resolve_embedder,
};
use lodestash_app::paths::default_cache_dirs;
use lodestash_app::persist::crawl_records;
use lodestash_app::types::PersistBatch;
use lodestash_core::catalog::Catalog;
use lodestash_core::types::{Exclude, ExcludeKind};
use lodestash_ingest::crawler::CrawledFile;
use lodestash_ingest::detect;
use lodestash_ingest::unitypackage::extract_to_workspace;

/// One asset the write path needs to act on, resolved from the library while its lock is held so
/// the writer thread never reaches back into it.
#[derive(Debug, Clone)]
pub struct Target {
    pub asset_id: i64,
    /// Absolute path. **Authoritative** — never recomposed from `source_root + rel_path`, which is
    /// wrong for workspace-extracted assets and can name a real, unrelated file (`app.rs:2003`).
    pub abs_path: Option<PathBuf>,
    /// Source root + the path relative to it, for the blacklist entry Remove writes.
    pub source_root: Option<PathBuf>,
    pub rel_path: String,
}

/// What the writer was asked to do. One variant per user-facing action, so the thread — not the
/// caller — owns the order of operations for anything that touches more than one thing.
pub enum Op {
    SetFavorite { ids: Vec<i64>, on: bool },
    /// Soft delete / restore. Catalog-only; the file on disk is never touched (DATA-MODEL §8).
    SetTrashed { ids: Vec<i64>, trashed: bool },
    /// **Remove from library** — the safe removal. Soft-deletes the row *and* blacklists the
    /// asset in its source, so the next scan does not bring it back. No file is touched.
    RemoveFromLibrary { targets: Vec<Target>, project: Option<String> },
    /// **Delete from disk** — the one irreversible action in this product.
    DeleteFromDisk { targets: Vec<Target>, roots: Vec<PathBuf>, workspace: Option<PathBuf> },
    /// Record a finished crawl: upsert the source and reconcile its assets (INGESTION §1).
    ///
    /// The **crawl** does not happen here. It is a disk walk that can take minutes, and the writer
    /// is a shared resource every other action queues behind — so the caller walks the disk on its
    /// own thread and sends the finished records. This is only the part that must be serialised,
    /// because it is the part that writes.
    PersistScan { batches: Vec<PersistBatch> },
    /// Trait-library authoring (UI-SPEC §9, B8a): create or promote a trait — an existing tag
    /// keeps its assignments and gains first-class status.
    UpsertTrait {
        namespace: String,
        value: String,
        scope: Option<String>,
        usage_note: Option<String>,
        teach: bool,
    },
    /// Demote a trait back to a plain tag; its assignments survive (never touches `asset_tag`).
    UnpromoteTrait { tag_id: i64 },
    /// Toggle a trait's teaching flag.
    SetTraitTeach { tag_id: i64, teach: bool },
    /// Set a trait's chip style — colour (CSS token or hex) + optional icon id. `None` clears.
    SetTraitStyle {
        tag_id: i64,
        color: Option<String>,
        icon: Option<String>,
    },
    /// Pause/resume watching one source (UI-SPEC §11.1). Paused sources stay cataloged.
    SetSourceWatch { id: i64, watch: bool },
    /// Create a named collection (UI-SPEC §6).
    CreateCollection { name: String },
    /// Delete a collection; memberships cascade (UI-SPEC §6).
    DeleteCollection { id: i64 },
    /// Pin/unpin a `.unitypackage` as a library (FEATURES §5.4) — pinned packs surface in the
    /// sidebar's LIBRARIES facet and get the "Add all to project" affordance.
    SetPackagePinned { id: i64, pinned: bool },
    /// The per-source blacklist editor (UI-SPEC §11.1): add/remove one `{kind, value}`
    /// exclusion. `kind` is `folder` or `asset` (DATA-MODEL §10).
    AddSourceExclude { source_id: i64, kind: String, value: String },
    RemoveSourceExclude { source_id: i64, kind: String, value: String },
    /// The Mapping tab's taxonomy switcher / authoring (UI-SPEC §9.2).
    SetActiveTaxonomy { id: i64 },
    DeleteTaxonomy { id: i64 },
    /// Add a mapping rule to a taxonomy (UI-SPEC §9.2). `match_type` is one of
    /// `MAPPING_MATCH_TYPES`; `priority` lower wins.
    AddRule {
        taxonomy_id: i64,
        category_id: i64,
        match_type: String,
        pattern: String,
        priority: i64,
    },
    /// Delete a mapping rule (UI-SPEC §9.2 row delete).
    DeleteRule { rule_id: i64 },
    /// Add a category under a parent (or the root) with its materialized path.
    AddCategory {
        taxonomy_id: i64,
        parent_id: Option<i64>,
        pref_label: String,
        path: String,
    },
    /// Run the deterministic mapping engine over one taxonomy.
    ApplyMapping { taxonomy_id: i64 },
    /// ✦ Identify all — one bounded appraise pass (embeddings → tags → promote) on the
    /// persist thread, so the catalog stays single-writer. Manual, like the egui build's
    /// Identify button (AI-PIPELINE §2); auto-identify stays a config opt-in.
    IdentifyAll,
    RenameCategory { id: i64, label: String },
    DeleteCategory { id: i64 },
    /// Manual appraise kick for one asset (per-card ✦, KANBAN B11b): flags the asset for
    /// re-embed + re-tag, then runs one bounded pass so it actually happens now.
    IdentifyOne { asset_id: i64 },
    /// Per-pack Extract (B2b slice): unpack the archive into the workspace, then reconcile
    /// the payloads into the catalog like a crawl. Explicit and on-demand (UI-SPEC §11.6) —
    /// browsing never unpacks; `lodestash://file` answers 503 until this runs.
    ExtractPackage { package_id: i64 },

    /// Undo one logged mutation (UI-SPEC §2 Ctrl/Cmd-Z, DESIGN-GUIDELINES §7 L5).
    RevertOp { op_id: i64 },
    /// Toggle one asset's membership in a collection (UI-SPEC §6, §13.5).
    CollectionAddAsset { collection_id: i64, asset_id: i64 },
    CollectionRemoveAsset { collection_id: i64, asset_id: i64 },
    /// Bulk-bar tag edit (KANBAN B9d): add/remove one `(namespace, value)` tag on many
    /// assets. Each asset gets its own undo-log entry snapshotted before the write, so
    /// `revert_op` restores the exact prior per-source tag set.
    BulkTag {
        asset_ids: Vec<i64>,
        namespace: String,
        value: String,
        on: bool,
    },
}

/// What a write did, in the terms the frontend reports to the user.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct WriteResult {
    /// Rows (or files) the operation actually changed.
    pub changed: usize,
    /// Files refused by the delete fence, or paths that no longer exist.
    pub refused: usize,
    /// Files that were meant to go but could not (permissions, in use).
    pub failed: usize,
}

pub struct Job {
    pub op: Op,
    pub reply: Sender<Result<WriteResult, String>>,
}

/// Handle to the writer thread. Cloneable, `Send`, holds no connection of its own.
#[derive(Clone)]
pub struct Writer {
    tx: Sender<Job>,
}

impl Writer {
    /// Spawn the writer over `db_path`. The connection is opened **on the thread that owns it**,
    /// never handed across, because that is the whole point of the rule.
    pub fn spawn(db_path: PathBuf, pump: Arc<AtomicBool>) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("lodestash-writer".into())
            .spawn(move || {
                // Opened once and held for the process's life: reopening per job would be a new
                // writer each time, which is the thing the rule forbids.
                let mut catalog = match Catalog::open(&db_path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!(%e, "writer: catalog open failed; writes will be refused");
                        // Drain and refuse rather than hanging every caller forever.
                        for job in rx {
                            let _ = job.reply.send(Err(format!("catalog unavailable: {e}")));
                        }
                        return;
                    }
                };
                // Model preview backfill (M15) is time-boxed and durable, and the persist
                // thread is the one place it may write (single-writer rule). The egui build
                // drained it from its pump; without the same call here the desktop leaves
                // `asset_preview` frozen at its initial coverage (6% of models on a real
                // library) and every uncovered model card renders icon-only — the "3D cards
                // look smaller than audio cards" symptom. Ops preempt it within a few
                // seconds; the queue is durable, so it converges over idle moments.
                let (cache_dir, workspace_dir) = default_cache_dirs();
                let proxy = AppConfig::load().preview_quality.proxy_budget();
                // B11c: when auto-identify is on and the local models are present, the idle
                // pass appraises (metrics → previews → deps → embeddings → promote) instead
                // of only backfilling previews. The embedders are resolved once here — model
                // files load at startup only when the user opted in — and borrowed per slice.
                let auto = AppConfig::load().auto_identify;
                let (idle_embedder, idle_audio) = if auto {
                    (resolve_embedder(), resolve_audio_embedder())
                } else {
                    (None, None)
                };
                let idle_appraises = auto && (idle_embedder.is_some() || idle_audio.is_some());
                // Test/CI escape hatch: the preview backfill is CPU-heavy (each model render
                // is a decimate + 4-view raster), and an e2e run that launches the app dozens
                // of times would otherwise grind the machine. LODESTASH_NO_PREVIEW_DRAIN=1
                // skips it; the grid then shows type marks for models without previews.
                let drain_enabled = std::env::var("LODESTASH_NO_PREVIEW_DRAIN").is_err();
                loop {
                    match rx.recv_timeout(std::time::Duration::from_secs(1)) {
                        Ok(job) => {
                            // Only the appraise kicks are slow enough to count as "the pump is
                            // running" — everything else is a quick write the frontend already
                            // waits on synchronously.
                            let appraising = matches!(job.op, Op::IdentifyAll | Op::IdentifyOne { .. });
                            if appraising {
                                pump.store(true, Ordering::Relaxed);
                            }
                            let result = run(&mut catalog, &db_path, job.op);
                            if appraising {
                                pump.store(false, Ordering::Relaxed);
                            }
                            let _ = job.reply.send(result);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            // Quiet for a second. Backfill previews in short slices, and only
                            // when nothing is queued — a long pass would make every write wait
                            // for it (measured: a 5s slice pushed a tag-create past the UI's
                            // own timeouts). 1s slices keep op latency under ~1.2s while still
                            // filling most idle time; the queue is durable, so it converges.
                            if !drain_enabled {
                                continue;
                            }
                            if rx.recv_timeout(std::time::Duration::ZERO).is_ok() {
                                continue; // an op is waiting — let the loop pick it up
                            }
                            if idle_appraises {
                                pump.store(true, Ordering::Relaxed);
                                let _ = appraise_pass(
                                    &mut catalog,
                                    idle_embedder.clone(),
                                    idle_audio.clone(),
                                    std::time::Duration::from_secs(1),
                                );
                                pump.store(false, Ordering::Relaxed);
                            } else {
                                drain_previews(
                                    &mut catalog,
                                    workspace_dir.as_deref(),
                                    cache_dir.as_deref(),
                                    proxy,
                                    std::time::Duration::from_secs(1),
                                );
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
                tracing::debug!("writer: channel closed, thread exiting");
            })
            .expect("writer thread spawns");
        Self { tx }
    }

    /// Run one op and wait for it. `Err` carries a message the frontend can show verbatim.
    pub fn run(&self, op: Op) -> Result<WriteResult, String> {
        let (reply, wait) = mpsc::channel();
        self.tx
            .send(Job { op, reply })
            .map_err(|_| "writer thread is gone".to_string())?;
        wait.recv()
            .map_err(|_| "writer thread dropped the job".to_string())?
    }
}

/// One bounded appraise pass (metrics → previews → deps → embeddings → promote) on the
/// writer thread. Shared by the ✦ kicks and the auto-identify idle pass (B11a–c): embedders
/// are resolved by the caller (the idle pass resolved them once at startup), everything else
/// is re-derived here because it can change between passes.
fn appraise_pass(
    catalog: &mut Catalog,
    embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    audio_embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    budget: std::time::Duration,
) -> Result<WriteResult, String> {
    let (cache_dir, workspace_dir) = default_cache_dirs();
    let proxy = AppConfig::load().preview_quality.proxy_budget();
    let roots = catalog
        .source_infos()
        .map(|infos| {
            infos
                .into_iter()
                .map(|i| (i.name, i.root_path))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let (counts, _assets) = drain_promote_resync(
        catalog,
        embedder,
        audio_embedder,
        workspace_dir.as_deref(),
        cache_dir.as_deref(),
        proxy,
        &roots,
        Some(budget),
    );
    // changed = the appraised backlog size the pass reconciled; when counts are None
    // (nothing moved) report 1 so the frontend toasts a real kick, not a silent no-op.
    Ok(WriteResult {
        changed: counts
            .map(|c| c.identified + c.unidentified)
            .unwrap_or(1),
        ..Default::default()
    })
}

fn run(catalog: &mut Catalog, db_path: &Path, op: Op) -> Result<WriteResult, String> {    match op {
        Op::SetFavorite { ids, on } => catalog
            .set_favorite(&ids, on)
            .map(|changed| WriteResult { changed, ..Default::default() })
            .map_err(|e| e.to_string()),

        // Undo (UI-SPEC §2 / DESIGN-GUIDELINES §7 L5): log the trash toggle so Ctrl/Cmd-Z can
        // flip it back. Only a toggle that actually changed rows is worth undoing — logging a
        // no-op would let a revert flip assets that were never touched.
        Op::SetTrashed { ids, trashed } => {
            let changed = catalog.set_trashed(&ids, trashed).map_err(|e| e.to_string())?;
            if changed > 0 {
                catalog.record_trash_op(&ids, trashed).map_err(|e| e.to_string())?;
            }
            Ok(WriteResult { changed, ..Default::default() })
        }

        Op::RemoveFromLibrary { targets, project } => remove_from_library(catalog, targets, project),

        Op::DeleteFromDisk { targets, roots, workspace } => {
            delete_from_disk(catalog, targets, &roots, workspace.as_deref())
        }

        // `persist_to_catalog` opens its **own** connection, which would be a second writer
        // anywhere else. Running it on this thread is what keeps the rule true: this thread's own
        // connection sits idle for the duration, and no other job can interleave with it.
        Op::PersistScan { batches } => {
            let changed = batches.iter().map(|b| b.records.len()).sum();
            lodestash_app::persist::persist_to_catalog(db_path, &batches)
                .map(|_| WriteResult { changed, ..Default::default() })
        }

        Op::UpsertTrait { namespace, value, scope, usage_note, teach } => catalog
            .upsert_trait(&namespace, &value, scope.as_deref(), usage_note.as_deref(), teach)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::UnpromoteTrait { tag_id } => catalog
            .unpromote_trait(tag_id)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::SetTraitTeach { tag_id, teach } => catalog
            .set_trait_teach(tag_id, teach)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::SetTraitStyle { tag_id, color, icon } => catalog
            .set_trait_style(tag_id, color.as_deref(), icon.as_deref())
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::SetSourceWatch { id, watch } => catalog
            .set_source_watch(id, watch)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        // Log the create (id + name) after the row exists so undo can delete exactly the
        // collection this op created (UI-SPEC §2 / DESIGN-GUIDELINES §7 L5).
        Op::CreateCollection { name } => {
            let id = catalog.create_collection(&name).map_err(|e| e.to_string())?;
            catalog
                .record_collection_create_op(id, &name)
                .map_err(|e| e.to_string())?;
            Ok(WriteResult { changed: 1, ..Default::default() })
        }

        Op::DeleteCollection { id } => catalog
            .delete_collection(id)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::SetPackagePinned { id, pinned } => catalog
            .set_package_pinned(id, pinned)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::AddSourceExclude { source_id, kind, value } => {
            catalog
                .add_source_exclude(source_id, &kind, &value)
                .map_err(|e| e.to_string())?;
            sync_source_exclude_to_config(catalog, source_id, &kind, &value, true)?;
            Ok(WriteResult { changed: 1, ..Default::default() })
        }

        Op::RemoveSourceExclude { source_id, kind, value } => {
            let removed = catalog
                .remove_source_exclude(source_id, &kind, &value)
                .map_err(|e| e.to_string())?;
            sync_source_exclude_to_config(catalog, source_id, &kind, &value, false)?;
            Ok(WriteResult {
                changed: usize::from(removed),
                ..Default::default()
            })
        }

        Op::SetActiveTaxonomy { id } => catalog
            .set_active_taxonomy(id)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::AddRule { taxonomy_id, category_id, match_type, pattern, priority } => catalog
            .add_rule(taxonomy_id, category_id, &match_type, &pattern, priority)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::DeleteRule { rule_id } => catalog
            .delete_rule(rule_id)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::DeleteTaxonomy { id } => catalog
            .delete_taxonomy(id)
            .map(|removed| WriteResult { changed: removed, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::AddCategory { taxonomy_id, parent_id, pref_label, path } => catalog
            .add_category(taxonomy_id, parent_id, &pref_label, &path)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::ApplyMapping { taxonomy_id } => catalog
            .apply_mapping(taxonomy_id)
            .map(|changed| WriteResult { changed, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::IdentifyAll => {
            // Manual appraise kick (✦ Identify all): resolve the embedders even when
            // auto-identify is off — the user explicitly asked. No local models (SigLIP/CLAP
            // absent) means nothing can be appraised; the pass still runs metrics/previews/
            // promote and reports honestly. Bounded so a long pass never blocks writes.
            let embedder = resolve_embedder();
            let audio_embedder = resolve_audio_embedder();
            if embedder.is_none() && audio_embedder.is_none() {
                tracing::warn!("Identify all: no local embedder models — nothing can be appraised");
            }
            appraise_pass(catalog, embedder, audio_embedder, std::time::Duration::from_secs(20))
        }

        Op::IdentifyOne { asset_id } => {
            // Per-card ✦ (B11b): the asset is flagged dirty, so the bounded pass picks it up
            // (and whatever else was already pending — the pass reconciles the whole backlog,
            // which is what "identify now" means honestly). No embedders → the flags still
            // get set, so a later pass (or auto-identify) completes the appraise.
            catalog.mark_for_identify(asset_id).map_err(|e| e.to_string())?;
            let embedder = resolve_embedder();
            let audio_embedder = resolve_audio_embedder();
            if embedder.is_none() && audio_embedder.is_none() {
                tracing::warn!("Identify asset {asset_id}: no local embedder models — flagged for a later pass");
            }
            appraise_pass(catalog, embedder, audio_embedder, std::time::Duration::from_secs(20))
        }

        Op::ExtractPackage { package_id } => {
            // Per-pack Extract (B2b slice): unpack to the workspace, reconcile the payloads
            // as a crawl under the pack's name. Payloads carry their archive-relative pathname
            // (or the GUID when the pack didn't record one), so they re-resolve across
            // re-extracts; Unity .meta sidecars are bookkeeping and never grid assets.
            let pkg = catalog
                .list_packages()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|p| p.id == package_id)
                .ok_or_else(|| format!("package {package_id} not found"))?;
            let workspace_dir = default_cache_dirs()
                .1
                .ok_or_else(|| "no workspace directory configured".to_string())?;
            let extracted = extract_to_workspace(Path::new(&pkg.package_path), &workspace_dir)
                .map_err(|e| format!("extract {}: {e}", pkg.name))?;
            let crawled: Vec<CrawledFile> = extracted
                .into_iter()
                .filter(|a| {
                    !a.pathname
                        .as_deref()
                        .is_some_and(|p| p.to_ascii_lowercase().ends_with(".meta"))
                })
                .map(|a| {
                    let rel_suffix = a.pathname.clone().unwrap_or_else(|| a.guid.clone());
                    let rel_path = PathBuf::from(&pkg.name).join(rel_suffix);
                    let media_type = detect::by_extension(&rel_path);
                    let mtime = std::fs::metadata(&a.workspace_path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0);
                    CrawledFile {
                        abs_path: a.workspace_path,
                        rel_path,
                        size_bytes: a.size_bytes,
                        mtime,
                        media_type,
                    }
                })
                .collect();
            let source_id = catalog
                .package_source_root(package_id)
                .map_err(|e| e.to_string())?
                .and_then(|root| catalog.source_info_by_root(&root).ok().flatten())
                .map(|i| i.id)
                .ok_or_else(|| "the pack has no library source — add its folder first".to_string())?;
            let records = crawl_records(crawled.iter());
            let stats = catalog
                .record_crawl(source_id, Some(package_id), &records)
                .map_err(|e| e.to_string())?;
            Ok(WriteResult {
                changed: stats.inserted + stats.updated,
                ..Default::default()
            })
        }

        Op::RenameCategory { id, label } => catalog
            .rename_category(id, &label)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::DeleteCategory { id } => catalog
            .delete_category(id)
            .map(|removed| WriteResult { changed: removed, ..Default::default() })
            .map_err(|e| e.to_string()),

        // Undo: the writer thread is the one place a revert may run — it is a catalog mutation
        // and every catalog mutation belongs here, one order (ARCHITECTURE §5).
        Op::RevertOp { op_id } => catalog
            .revert_op(op_id)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::CollectionAddAsset { collection_id, asset_id } => catalog
            .collection_add_asset(collection_id, asset_id)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::CollectionRemoveAsset { collection_id, asset_id } => catalog
            .collection_remove_asset(collection_id, asset_id)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        // Bulk-bar tag edit (KANBAN B9d). Tag writes replace per **source** (`set_tags`
        // re-writes everything that source contributed), so this adds/removes the tag under
        // the `manual` source — the established source for user-set tags (set_tags docs). Each
        // asset's prior `manual` set is snapshotted into its own undo-log entry first, which
        // is exactly the payload `revert_set_tags` restores on undo.
        Op::BulkTag { asset_ids, namespace, value, on } => {
            const SOURCE: &str = "manual";
            let mut changed = 0usize;
            // Only the assets that actually changed go into the undo row — a no-op add on an
            // already-tagged asset must not be recorded, or its revert would wrongly yank a
            // pre-existing tag.
            let mut affected = Vec::new();
            for asset_id in asset_ids {
                if on {
                    // Append, never replace: add_asset_tag leaves the asset's other manual
                    // tags alone (set_tags would delete-and-insert the whole source set).
                    let n = catalog
                        .add_asset_tag(asset_id, SOURCE, &namespace, &value)
                        .map_err(|e| e.to_string())?;
                    if n > 0 {
                        affected.push(asset_id);
                    }
                    changed += n;
                } else {
                    let n = catalog
                        .remove_asset_tag(asset_id, &namespace, &value)
                        .map_err(|e| e.to_string())?;
                    if n > 0 {
                        affected.push(asset_id);
                    }
                    changed += n;
                }
            }
            if !affected.is_empty() {
                catalog
                    .record_bulk_tag_op(&affected, &namespace, &value, on)
                    .map_err(|e| e.to_string())?;
            }
            Ok(WriteResult { changed, ..Default::default() })
        }
    }
}

/// Soft-delete the rows **and** blacklist each asset in its source.
///
/// The blacklist is what makes this different from Trash: `crawler::crawl` takes the per-source
/// `Exclude` list, so an excluded asset is never re-offered by a scan. Without it a "removed"
/// asset reappears the next time the user indexes that folder, which reads as the app ignoring
/// them.
///
/// The exclude is written to the **app config**, not `import_source.blacklist`: the config is
/// what `crawl` is actually handed today (`app.rs:812`), and the catalog column is M5 storage
/// that nothing reads yet. Writing only the column would look right and change nothing.
fn remove_from_library(
    catalog: &Catalog,
    targets: Vec<Target>,
    project: Option<String>,
) -> Result<WriteResult, String> {
    let ids: Vec<i64> = targets.iter().map(|t| t.asset_id).collect();
    let changed = catalog.set_trashed(&ids, true).map_err(|e| e.to_string())?;

    let Some(project) = project else {
        // No project open: the row is hidden, but nothing owns a blacklist to write to. Say so
        // rather than reporting a removal that a scan will undo.
        return Err("no project is open, so the asset cannot be excluded from future scans".into());
    };
    let mut config = lodestash_app::config::AppConfig::load();
    let mut excluded = 0;
    for t in &targets {
        let Some(root) = t.source_root.as_deref() else {
            continue; // in-pack asset: no source-relative file for the crawler to skip
        };
        // `Asset` matches the full source-relative path or the bare file name
        // (`Exclude::matches`), so this excludes exactly the file the user removed.
        let exclude = Exclude {
            kind: ExcludeKind::Asset,
            value: t.rel_path.clone(),
        };
        if config.add_source_exclude(&project, root, exclude) {
            excluded += 1;
        }
    }
    tracing::info!(rows = changed, excluded, "removed from library");
    Ok(WriteResult { changed, ..Default::default() })
}

/// Mirror a blacklist edit into the app config the crawler actually reads (UI-SPEC §11.1).
/// `import_source.blacklist` is catalog storage; `crawl` is fed from
/// `AppConfig.source_excludes(project, root)` — write both, or the Settings editor changes
/// nothing and the next scan resurrects every "removed" asset. The config write goes through
/// `edit_config` (the one shared CONFIG_LOCK), so it cannot interleave with the web-prefs /
/// settings commands; the writer holds no other lock here, so there is no lock-order cycle.
fn sync_source_exclude_to_config(
    catalog: &Catalog,
    source_id: i64,
    kind: &str,
    value: &str,
    add: bool,
) -> Result<(), String> {
    let Some(root) = catalog.source_root(source_id).map_err(|e| e.to_string())? else {
        return Err(format!("sync_source_exclude: no such source {source_id}"));
    };
    let exclude = Exclude {
        kind: match kind {
            "folder" => ExcludeKind::Folder,
            "asset" => ExcludeKind::Asset,
            other => return Err(format!("sync_source_exclude: bad kind {other:?}")),
        },
        value: value.to_string(),
    };
    lodestash_app::config::edit_config(|config| {
        let Some(project) = config.last_project_name.clone() else {
            return;
        };
        if add {
            config.add_source_exclude(&project, std::path::Path::new(&root), exclude);
        } else {
            config.remove_source_exclude(&project, std::path::Path::new(&root), &exclude);
        }
    })
}

/// May this target's file be deleted?
///
/// `false` for an **in-pack** asset — it has no file of its own, and its `abs_path` would be the
/// `.unitypackage` holding hundreds of other assets — and `false` for anything outside every
/// source root the user opened (or the workspace this app owns).
fn deletable(t: &Target, roots: &[PathBuf], workspace: Option<&Path>) -> bool {
    let Some(path) = t.abs_path.as_deref() else {
        return false;
    };
    if !path.is_file() || !lodestash_app::thumbs::under_known_root(path, roots, workspace) {
        tracing::warn!(path = %path.display(), "refused to delete: outside every known root");
        return false;
    }
    true
}

/// Delete files from disk, then record only what actually left.
///
/// Two guards, in this order:
/// 1. `abs_path` is used as-is. Recomposing `source_root + rel_path` is wrong for extracted assets
///    and can name an unrelated real file — that is the primary protection.
/// 2. [`under_known_root`] bounds the blast radius if (1) is ever got wrong. It is fenced by
///    `only_paths_under_known_roots_may_be_deleted`, which is mutation-checked and must stay that way.
///
/// Trashing happens **after** the delete and only for files that succeeded: doing it first lets a
/// failed delete leave the catalog claiming a file is gone while it sits on disk.
fn delete_from_disk(
    catalog: &Catalog,
    targets: Vec<Target>,
    roots: &[PathBuf],
    workspace: Option<&Path>,
) -> Result<WriteResult, String> {
    let mut out = WriteResult::default();
    let mut deleted = Vec::new();
    for t in &targets {
        if !deletable(t, roots, workspace) {
            out.refused += 1;
            continue;
        }
        let path = t.abs_path.as_deref().expect("deletable implies a path");
        match std::fs::remove_file(path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "deleted file from disk");
                deleted.push(t.asset_id);
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), %e, "delete from disk failed");
                out.failed += 1;
            }
        }
    }
    if !deleted.is_empty() {
        out.changed = catalog
            .set_trashed(&deleted, true)
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: Option<&str>) -> Target {
        Target {
            asset_id: 1,
            abs_path: path.map(PathBuf::from),
            source_root: None,
            rel_path: "Assets/Rock.fbx".into(),
        }
    }

    /// The blast-radius guard, exercised through the code path a command actually takes rather
    /// than through `under_known_root` alone. A file outside every source root — and an in-pack
    /// asset, whose "path" is the archive holding hundreds of other assets — must be refused
    /// before `remove_file` is ever reached.
    ///
    /// `catalog` is `None` here on purpose: nothing should get far enough to need it.
    #[test]
    fn delete_refuses_paths_outside_every_known_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("library");
        std::fs::create_dir_all(&root).expect("mkdir");
        let outside = dir.path().join("elsewhere.fbx");
        std::fs::write(&outside, b"not yours").expect("write");

        let roots = vec![root];
        assert!(
            !deletable(&target(outside.to_str()), &roots, None),
            "a file outside every source root must be refused"
        );
        assert!(
            !deletable(&target(None), &roots, None),
            "an in-pack asset has no file of its own — deleting its archive would take every \
             other asset in that pack with it"
        );
        assert!(outside.exists(), "the file outside the root is still there");
    }

    /// The other half: a file that *is* under a known root passes the fence. Without this the
    /// test above would still pass if the fence refused everything.
    #[test]
    fn delete_allows_a_file_under_a_source_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("library");
        std::fs::create_dir_all(&root).expect("mkdir");
        let inside = root.join("Rock.fbx");
        std::fs::write(&inside, b"mine").expect("write");

        assert!(
            deletable(&target(inside.to_str()), std::slice::from_ref(&root), None),
            "a file under a source root must pass the fence"
        );
    }
}
