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

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

use lodestash_app::config::AppConfig;
use lodestash_app::drains::{
    drain_audio_tags, drain_promote_resync, drain_previews, drain_tags, resolve_audio_embedder,
    resolve_embedder,
};
use lodestash_app::jobs::JobProgress as AppJobProgress;
use lodestash_app::paths::default_cache_dirs;
use lodestash_app::types::PersistBatch;
use lodestash_ai::clap_tags::AudioZeroShot;
use lodestash_ai::image_tags::ImageZeroShot;
use lodestash_ai::ollama::OllamaTagger;
use lodestash_ai::openai::OpenAiTagger;
use lodestash_ai::tagger::{FilenameTagger, Tagger, TeachTrait};
use lodestash_core::catalog::{Catalog, PackageEntryRecord};
use lodestash_core::types::{Exclude, ExcludeKind};
use lodestash_ingest::unitypackage::{
    extract_at_offsets_with_progress, read_manifest_with_progress, EntryLocator, ExtractedAsset,
};

/// Bounded tag work per appraise pass (AI-PIPELINE §5.1): a writer-thread kick never blocks
/// writes on a long backlog; the remainder stays queued for the next kick / idle slice.
const TAG_BUDGET_PER_PASS: u32 = 64;
const AUDIO_TAG_BUDGET_PER_PASS: u32 = 48;

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
    PersistScan {
        batches: Vec<PersistBatch>,
        /// `.unitypackage`s the crawl surfaced (browse persistence, INGESTION §2): upsert a
        /// `package` row + record the manifest locators so the pack is browsable and
        /// extractable without re-scanning the archive.
        packs: Vec<DiscoveredPack>,
    },
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
    /// Switch which registered Unity editor is active (ENGINE-API §5.1, UI-SPEC §16.1).
    SetActiveProject { project_id: i64 },

    /// Undo one logged mutation (UI-SPEC §2 Ctrl/Cmd-Z, DESIGN-GUIDELINES §7 L5).
    RevertOp { op_id: i64 },
    /// Redo one undone op (UI-SPEC §2 Shift-Z): applies the op's forward payload and consumes
    /// the log row — the inverse of [`Op::RevertOp`].
    RedoOp { op_id: i64 },
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
    /// Asset ids the appraise pass just promoted to `identified` — the Identify screen's
    /// session set (UI-SPEC §3). Empty for every non-appraise op.
    pub promoted: Vec<i64>,
}

pub struct Job {
    pub op: Op,
    pub reply: Sender<Result<WriteResult, String>>,
}

/// What the writer thread's channel carries: an external op (with its reply), or a worker
/// reporting back so the catalog half runs on the writer thread (single-writer rule).
enum WriterMsg {
    Op(Job),
    /// A manifest worker finished reading the archive (surgical indexing, M14) — the writer
    /// records the locators + mints in-pack assets, then spawns the extract worker.
    ManifestDone {
        job_id: u64,
        package_id: i64,
        reply: Sender<Result<WriteResult, String>>,
        entries: Result<Vec<PackageEntryRecord>, String>,
        cancelled: bool,
    },
    ExtractDone {
        job_id: u64,
        package_id: i64,
        reply: Sender<Result<WriteResult, String>>,
        outcome: Result<Vec<ExtractedAsset>, String>,
        cancelled: bool,
    },
}

/// A `.unitypackage` a crawl surfaced, with its browse-time manifest locators
/// (INGESTION.md §2) — persisted as `package` + `package_entry` rows so the pack is
/// browsable and extractable without ever re-scanning the archive.
pub struct DiscoveredPack {
    /// The source root that contained the archive (matches a `PersistBatch.root`).
    pub root: String,
    pub abs_path: String,
    pub display_name: String,
    pub entries: Vec<PackageEntryRecord>,
}

// ── Job tray (B2b) ─────────────────────────────────────────────────────────────────────────

/// Shared tray state: active jobs (progress + cooperative cancel) and a session ring of
/// finished ones. The writer thread owns the mutations; commands read snapshots.
#[derive(Clone, Default)]
pub struct JobRegistry {
    inner: std::sync::Arc<std::sync::Mutex<JobState>>,
}

#[derive(Default)]
struct JobState {
    next_id: u64,
    active: Vec<ActiveJob>,
    recent: VecDeque<RecentJob>,
}

/// Extract byte progress, read by the tray without taking the registry lock.
#[derive(Default)]
struct JobProgress {
    done: AtomicU64,
    total: AtomicU64,
}

struct ActiveJob {
    id: u64,
    label: String,
    kind: String,
    cancel: Arc<AtomicBool>,
    progress: Option<Arc<JobProgress>>,
}

/// Handles a caller holds after `start`, without touching the registry lock again.
struct JobHandles {
    id: u64,
    cancel: Arc<AtomicBool>,
    progress: Option<Arc<JobProgress>>,
}

#[derive(Clone, serde::Serialize)]
pub struct JobSnapshot {
    pub id: u64,
    pub label: String,
    pub kind: String,
    /// `(done, total)` bytes for extract; `None` = indeterminate (spinner).
    pub progress: Option<(u64, u64)>,
    pub cancellable: bool,
}

#[derive(Clone, serde::Serialize)]
pub struct RecentJob {
    pub id: u64,
    pub label: String,
    pub kind: String,
    pub ok: bool,
    pub cancelled: bool,
    pub changed: usize,
    pub at_unix: i64,
}

#[derive(Default, serde::Serialize)]
pub struct JobsView {
    pub active: Vec<JobSnapshot>,
    pub recent: Vec<RecentJob>,
}

const RECENT_CAP: usize = 20;

impl JobRegistry {
    /// Register an active job; returns the handles the worker uses for progress/cancel.
    fn start(&self, label: String, kind: &str, cancellable: bool) -> JobHandles {
        let mut state = self.inner.lock().expect("job registry lock");
        let id = state.next_id;
        state.next_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = cancellable.then(|| Arc::new(JobProgress::default()));
        state.active.push(ActiveJob {
            id,
            label,
            kind: kind.to_string(),
            cancel: cancel.clone(),
            progress: progress.clone(),
        });
        JobHandles {
            id,
            cancel,
            progress,
        }
    }

    /// Move an active job to the session ring. `changed` is what the tray reports.
    fn finish(&self, id: u64, ok: bool, cancelled: bool, changed: usize) {
        let mut state = self.inner.lock().expect("job registry lock");
        let Some(pos) = state.active.iter().position(|j| j.id == id) else {
            return;
        };
        let job = state.active.remove(pos);
        state.recent.push_front(RecentJob {
            id: job.id,
            label: job.label,
            kind: job.kind,
            ok,
            cancelled,
            changed,
            at_unix: unix_now(),
        });
        state.recent.truncate(RECENT_CAP);
    }

    fn has_active_extract(&self) -> bool {
        self.inner
            .lock()
            .expect("job registry lock")
            .active
            .iter()
            .any(|j| j.kind == "extract")
    }

    /// Request cancellation. The extract worker checks the flag between reads; everything
    /// else finishes on its own and ignores it.
    pub fn cancel(&self, id: u64) -> bool {
        let state = self.inner.lock().expect("job registry lock");
        match state.active.iter().find(|j| j.id == id) {
            Some(j) => {
                j.cancel.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn snapshot(&self) -> JobsView {
        let state = self.inner.lock().expect("job registry lock");
        JobsView {
            active: state
                .active
                .iter()
                .map(|j| JobSnapshot {
                    id: j.id,
                    label: j.label.clone(),
                    kind: j.kind.clone(),
                    progress: j.progress.as_ref().map(|p| {
                        (p.done.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed))
                    }),
                    cancellable: j.progress.is_some(),
                })
                .collect(),
            recent: state.recent.iter().cloned().collect(),
        }
    }
}

/// Handle to the writer thread. Cloneable, `Send`, holds no connection of its own.
#[derive(Clone)]
pub struct Writer {
    tx: Sender<WriterMsg>,
}

/// One writer-side change, pushed to the Svelte shell (pump events, B2b) so the UI stops
/// polling. `catalog` = the library changed (re-read status + the current page); `jobs` =
/// the tray changed (refetch the snapshot).
#[derive(Clone, serde::Serialize)]
struct ChangedEvent {
    kind: &'static str,
}

fn emit_changed(app: &Option<tauri::AppHandle>, kind: &'static str) {
    if let Some(handle) = app {
        use tauri::Emitter;
        let _ = handle.emit("ls:changed", ChangedEvent { kind });
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

impl Writer {
    /// Spawn the writer over `db_path`. The connection is opened **on the thread that owns it**,
    /// never handed across, because that is the whole point of the rule.
    pub fn spawn(
        db_path: PathBuf,
        pump: Arc<AtomicBool>,
        jobs: JobRegistry,
        app: Option<tauri::AppHandle>,
        appraise: Arc<AppJobProgress>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<WriterMsg>();
        let writer_tx = tx.clone();
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
                        for msg in rx {
                            if let WriterMsg::Op(job) = msg {
                                let _ = job.reply.send(Err(format!("catalog unavailable: {e}")));
                            }
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
                // UI-SPEC §5: ticked packs unpack **one at a time**. An ExtractPackage op that
                // arrives while a worker is running (or already queued) waits here; each op's
                // reply resolves to the real outcome when its reconcile runs.
                let mut extract_queue: VecDeque<Job> = VecDeque::new();
                // DATA-MODEL §6 watch: live notify over the watched source roots, debounced by
                // the ingest helper. A signal means "something changed, rescan when convenient"
                // — the rescan is the incremental crawl (needs_rehash skips unchanged files), so
                // it is cheap. Skipped under the e2e escape hatch like every other idle pass.
                let mut watch = if drain_enabled && AppConfig::load().watch_sources {
                    rebuild_watch(&catalog)
                } else {
                    None
                };
                loop {
                    match rx.recv_timeout(std::time::Duration::from_secs(1)) {
                        Ok(WriterMsg::Op(job)) => {
                            if matches!(job.op, Op::ExtractPackage { .. }) {
                                if extract_queue.is_empty() && !jobs.has_active_extract() {
                                    let result = run(
                                        &mut catalog,
                                        &db_path,
                                        job.op,
                                        &writer_tx,
                                        &jobs,
                                        &job.reply,
                                        &app,
                                        &appraise,
                                    );
                                    let _ = job.reply.send(result);
                                } else {
                                    // A pack is unpacking (or queued) — join the queue.
                                    extract_queue.push_back(job);
                                }
                                continue;
                            }
                            // Only the appraise kicks are slow enough to count as "the pump is
                            // running" — everything else is a quick write the frontend already
                            // waits on synchronously.
                            let appraising = matches!(job.op, Op::IdentifyAll | Op::IdentifyOne { .. });
                            if appraising {
                                pump.store(true, Ordering::Relaxed);
                            }
                            let source_set_changed =
                                matches!(job.op, Op::PersistScan { .. } | Op::SetSourceWatch { .. });
                            let result = run(
                                &mut catalog,
                                &db_path,
                                job.op,
                                &writer_tx,
                                &jobs,
                                &job.reply,
                                &app,
                                &appraise,
                            );
                            if appraising {
                                pump.store(false, Ordering::Relaxed);
                            }
                            let _ = job.reply.send(result);
                            emit_changed(&app, "catalog");
                            emit_changed(&app, "jobs");
                            // A source set (or its watch flags) changed — re-arm the watch so
                            // new roots are covered and unwatched ones stop firing.
                            if source_set_changed {
                                watch = if drain_enabled && AppConfig::load().watch_sources {
                                    rebuild_watch(&catalog)
                                } else {
                                    None
                                };
                            }
                        }
                        Ok(WriterMsg::ExtractDone {
                            job_id,
                            package_id,
                            reply,
                            outcome,
                            cancelled,
                        }) => {
                            // The payloads landed on disk off-thread; materialize them here —
                            // the catalog stays single-writer. Then the next queued pack gets
                            // its worker.
                            let result = materialize_extract(&mut catalog, package_id, outcome);
                            let ok = result.is_ok();
                            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
                            jobs.finish(job_id, ok, cancelled, changed);
                            let _ = reply.send(result);
                            emit_changed(&app, "catalog");
                            emit_changed(&app, "jobs");
                            if let Some(next) = extract_queue.pop_front() {
                                let r = run(
                                    &mut catalog,
                                    &db_path,
                                    next.op,
                                    &writer_tx,
                                    &jobs,
                                    &next.reply,
                                    &app,
                                    &appraise,
                                );
                                let _ = next.reply.send(r);
                            }
                        }
                        Ok(WriterMsg::ManifestDone {
                            job_id,
                            package_id,
                            reply,
                            entries,
                            cancelled,
                        }) => {
                            // The archive's manifest is read; record the locators + mint the
                            // in-pack assets (surgical indexing, M14), then spawn the extract
                            // worker over the locators. The op's reply is answered by that
                            // phase's ExtractDone — except when the manifest failed, which is
                            // answered here so the caller never hangs.
                            let result = (|| {
                                let entries = entries?;
                                let minted = mint_in_pack(&mut catalog, package_id, &entries)?;
                                let pkg = catalog
                                    .list_packages()
                                    .map_err(|e| e.to_string())?
                                    .into_iter()
                                    .find(|p| p.id == package_id)
                                    .ok_or_else(|| format!("package {package_id} not found"))?;
                                let workspace_dir = default_cache_dirs()
                                    .1
                                    .ok_or_else(|| "no workspace directory configured".to_string())?;
                                spawn_extract_worker(
                                    &pkg,
                                    &workspace_dir,
                                    &entries,
                                    &writer_tx,
                                    &reply,
                                    &jobs,
                                    &app,
                                )?;
                                Ok(WriteResult {
                                    changed: minted,
                                    ..Default::default()
                                })
                            })();
                            let ok = result.is_ok();
                            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
                            jobs.finish(job_id, ok, cancelled, changed);
                            emit_changed(&app, "catalog");
                            emit_changed(&app, "jobs");
                            if let Err(e) = result {
                                let _ = reply.send(Err(e));
                                if let Some(next) = extract_queue.pop_front() {
                                    let r = run(
                                        &mut catalog,
                                        &db_path,
                                        next.op,
                                        &writer_tx,
                                        &jobs,
                                        &next.reply,
                                        &app,
                                        &appraise,
                                    );
                                    let _ = next.reply.send(r);
                                }
                            }
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
                            // Watch signal (debounced upstream): rescan every watched source with
                            // the incremental crawl. The writer is idle here by construction.
                            if let Some(w) = &watch {
                                if w.rx.recv_timeout(std::time::Duration::ZERO).is_ok() {
                                    if rescan_watched(&mut catalog, &db_path, &app) {
                                        emit_changed(&app, "catalog");
                                    }
                                }
                            }
                            if idle_appraises {
                                pump.store(true, Ordering::Relaxed);
                                let r = appraise_pass(
                                    &mut catalog,
                                    idle_embedder.clone(),
                                    idle_audio.clone(),
                                    std::time::Duration::from_secs(1),
                                    &appraise,
                                );
                                pump.store(false, Ordering::Relaxed);
                                if r.map(|w| w.changed > 0).unwrap_or(false) {
                                    emit_changed(&app, "catalog");
                                }
                            } else {
                                let n = drain_previews(
                                    &mut catalog,
                                    workspace_dir.as_deref(),
                                    cache_dir.as_deref(),
                                    proxy,
                                    std::time::Duration::from_secs(1),
                                );
                                if n > 0 {
                                    emit_changed(&app, "catalog");
                                }
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
            .send(WriterMsg::Op(Job { op, reply }))
            .map_err(|_| "writer thread is gone".to_string())?;
        wait.recv()
            .map_err(|_| "writer thread dropped the job".to_string())?
    }
}

/// (Re)build the notify watch over the catalog's watched source roots (DATA-MODEL §6). `None`
/// when nothing is watchable — watching is an optimisation, so a failure degrades to manual
/// rescan rather than an error the user has to dismiss.
fn rebuild_watch(catalog: &Catalog) -> Option<lodestash_ingest::watch::RootWatcher> {
    let roots: Vec<PathBuf> = catalog
        .source_infos()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| i.watch_enabled)
        .map(|i| PathBuf::from(i.root_path))
        .collect();
    if roots.is_empty() {
        return None;
    }
    lodestash_ingest::watch::watch_roots(&roots, std::time::Duration::from_secs(2))
}

/// One watch-driven rescan of every watched source: the incremental crawl (unchanged files skip
/// BLAKE3 via `needs_rehash`), then the standard persist on this thread (single-writer rule —
/// `persist_to_catalog` opens its own connection here exactly as `Op::PersistScan` does).
/// Returns whether anything actually changed, so the UI only reloads when it did.
fn rescan_watched(catalog: &mut Catalog, db_path: &Path, app: &Option<tauri::AppHandle>) -> bool {
    let cfg = AppConfig::load();
    let project = cfg.last_project_name.as_deref();
    let mut changed = false;
    for info in catalog.source_infos().unwrap_or_default() {
        if !info.watch_enabled {
            continue;
        }
        let root = PathBuf::from(&info.root_path);
        let excludes = project
            .map(|p| cfg.source_excludes(p, &root))
            .unwrap_or_default();
        let known = catalog
            .known_sizes_for_source(&info.root_path)
            .unwrap_or_default();
        let crawled = lodestash_app::scan::crawl_source(0, &root, &excludes, &known);
        if crawled.entries.is_empty() && crawled.pending_packages.is_empty() {
            continue;
        }
        let records =
            lodestash_app::persist::crawl_records(crawled.entries.iter().map(|(_, f)| f));
        // Cheap in-memory diff against the catalog's last-known (size, mtime): nothing moved on
        // disk means nothing to persist (and no spurious `last_scanned_at` write + UI reload).
        let same = records.len() == known.len()
            && records.iter().all(|r| {
                known
                    .get(&r.rel_path)
                    .is_some_and(|&(s, m)| s == r.size_bytes as u64 && m == r.mtime)
            });
        if same {
            continue;
        }
        let batch = lodestash_app::types::PersistBatch {
            root: info.root_path.clone(),
            name: info.name.clone(),
            excludes_json: None,
            records,
        };
        match lodestash_app::persist::persist_to_catalog(db_path, &[batch]) {
            Ok(_) => changed = true,
            Err(e) => tracing::warn!(%e, root = %info.root_path, "watch rescan failed"),
        }
    }
    changed
}

/// One bounded appraise pass (metrics → previews → deps → embeddings → promote) on the/// writer thread. Shared by the ✦ kicks and the auto-identify idle pass (B11a–c): embedders
/// are resolved by the caller (the idle pass resolved them once at startup), everything else
/// is re-derived here because it can change between passes. `progress` is the shared
/// B11d surface the Identify screen polls — reset at pass start, filled by the tag/embed
/// drains as they work.
fn appraise_pass(
    catalog: &mut Catalog,
    embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    audio_embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    budget: std::time::Duration,
    progress: &AppJobProgress,
) -> Result<WriteResult, String> {
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(0, Ordering::Relaxed);
    progress.failed.store(0, Ordering::Relaxed);
    progress.cancelled.store(false, Ordering::Relaxed);
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
    // B11 tail — the tagging half of the appraise pass (was unreachable: the desktop had no
    // tagger resolver). The SigLIP embedder resolved above drives the visual zero-shot tagger,
    // CLAP drives the audio one, and taught traits (Traits screen, AI-PIPELINE §6) become
    // candidate labels. The captioning tagger resolves from settings: OpenAI-compatible endpoint
    // (opt-in, AI-PIPELINE §5) → local Ollama VLM (AI-PIPELINE §2) → filename stub.
    // Bounded per pass so a writer-thread kick never blocks writes on a long backlog — the
    // remainder stays queued for the next kick / idle auto-identify slice.
    let teach = catalog
        .teaching_traits()
        .unwrap_or_default()
        .into_iter()
        .map(|(namespace, value, scope, usage_note)| TeachTrait {
            namespace,
            value,
            scope,
            usage_note,
        })
        .collect::<Vec<_>>();
    let zero_shot = embedder
        .clone()
        .and_then(|e| ImageZeroShot::new(e, &teach));
    let audio_shot = audio_embedder.clone().and_then(AudioZeroShot::new);
    let cfg = AppConfig::load();
    let api_model = cfg
        .api_model
        .clone()
        .filter(|_| cfg.api_tagging_enabled && !cfg.api_key.is_empty());
    let tagger: Box<dyn Tagger> = if let Some(model) = api_model {
        Box::new(
            OpenAiTagger::new(cfg.api_base_url.clone(), cfg.api_key.clone(), model)
                .with_teaching(teach.clone()),
        )
    } else if let Some(model) = cfg.tagger_model.clone() {
        let mut t = OllamaTagger::new(model);
        if let Some(vision) = cfg.tagger_vision_model.clone() {
            t = t.with_vision(vision);
        }
        Box::new(t.with_teaching(teach.clone()))
    } else {
        Box::new(FilenameTagger)
    };
    drain_tags(
        catalog,
        tagger.as_ref(),
        zero_shot.as_ref(),
        workspace_dir.as_deref(),
        cache_dir.as_deref(),
        &progress,
        TAG_BUDGET_PER_PASS,
    );
    if let Some(az) = audio_shot.as_ref() {
        drain_audio_tags(
            catalog,
            az,
            workspace_dir.as_deref(),
            &progress,
            AUDIO_TAG_BUDGET_PER_PASS,
        );
    }
    let (counts, _assets, promoted) = drain_promote_resync(
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
        promoted,
        ..Default::default()
    })
}

fn run(
    catalog: &mut Catalog,
    db_path: &Path,
    op: Op,
    tx: &Sender<WriterMsg>,
    jobs: &JobRegistry,
    reply: &Sender<Result<WriteResult, String>>,
    app: &Option<tauri::AppHandle>,
    progress: &AppJobProgress,
) -> Result<WriteResult, String> {
    match op {
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
        // Browse persistence (INGESTION §2): the crawl already captured each `.unitypackage`'s
        // manifest locators — upsert the `package` row + record them, so the pack appears on
        // Unity Assets and extracts surgically without re-reading the archive.
        Op::PersistScan { batches, packs } => {
            let changed = batches.iter().map(|b| b.records.len()).sum();
            let label = batches
                .first()
                .map(|b| format!("Import {}", b.name))
                .unwrap_or_else(|| "Import".to_string());
            let job = jobs.start(label, "import", false);
            let result = (|| {
                lodestash_app::persist::persist_to_catalog(db_path, &batches)
                    .map_err(|e| e.to_string())?;
                for pack in packs {
                    let Some(source) = catalog
                        .source_info_by_root(&pack.root)
                        .map_err(|e| e.to_string())?
                    else {
                        continue;
                    };
                    let id = catalog
                        .upsert_package(source.id, &pack.abs_path, &pack.display_name, None, None)
                        .map_err(|e| e.to_string())?;
                    catalog
                        .record_package_entries(id, &pack.entries)
                        .map_err(|e| e.to_string())?;
                }
                Ok(WriteResult {
                    changed,
                    ..Default::default()
                })
            })();
            let ok = result.is_ok();
            jobs.finish(job.id, ok, false, changed);
            result
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
            let job = jobs.start("Identify all".into(), "appraise", false);
            let embedder = resolve_embedder();
            let audio_embedder = resolve_audio_embedder();
            if embedder.is_none() && audio_embedder.is_none() {
                tracing::warn!("Identify all: no local embedder models — nothing can be appraised");
            }
            let result = appraise_pass(
                catalog,
                embedder,
                audio_embedder,
                std::time::Duration::from_secs(20),
                progress,
            );
            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
            jobs.finish(job.id, result.is_ok(), false, changed);
            result
        }

        Op::IdentifyOne { asset_id } => {
            // Per-card ✦ (B11b): the asset is flagged dirty, so the bounded pass picks it up
            // (and whatever else was already pending — the pass reconciles the whole backlog,
            // which is what "identify now" means honestly). No embedders → the flags still
            // get set, so a later pass (or auto-identify) completes the appraise.
            catalog.mark_for_identify(asset_id).map_err(|e| e.to_string())?;
            let job = jobs.start(format!("Identify asset {asset_id}"), "appraise", false);
            let embedder = resolve_embedder();
            let audio_embedder = resolve_audio_embedder();
            if embedder.is_none() && audio_embedder.is_none() {
                tracing::warn!("Identify asset {asset_id}: no local embedder models — flagged for a later pass");
            }
            let result = appraise_pass(
                catalog,
                embedder,
                audio_embedder,
                std::time::Duration::from_secs(20),
                progress,
            );
            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
            jobs.finish(job.id, result.is_ok(), false, changed);
            result
        }

        Op::ExtractPackage { package_id } => {
            // Per-pack Extract (M14 surgical flow, INGESTION §2): if the manifest locators
            // aren't recorded yet (no browse persistence for this pack), a manifest worker
            // reads the archive once (progress + cancel through the tray) and the writer
            // records the locators + **mints the in-pack assets** (surgical indexing) when it
            // reports back. The extract worker then pulls only the wanted payloads with one
            // gzip skip-scan (`extract_at_offsets`) and the writer **materializes** them
            // (fills `workspace_rel`) — the rows hydrate from the workspace and render from
            // cache. This command returns "queued" immediately; the op's reply resolves when
            // the whole flow finishes.
            let pkg = catalog
                .list_packages()
                .map_err(|e| e.to_string())?
                .into_iter()
                .find(|p| p.id == package_id)
                .ok_or_else(|| format!("package {package_id} not found"))?;
            let workspace_dir = default_cache_dirs()
                .1
                .ok_or_else(|| "no workspace directory configured".to_string())?;
            let entries = catalog
                .list_package_entries(package_id)
                .map_err(|e| e.to_string())?;
            if entries.is_empty() {
                // Phase 1: the manifest is unknown — read it on a worker (a big pack must
                // inflate its whole tar.gz; never on the writer thread). The kind is
                // "extract" so the one-at-a-time queue gates the whole flow.
                let handles = jobs.start(format!("Analyze {}", pkg.name), "extract", true);
                let wtx = tx.clone();
                let wreply = reply.clone();
                let wcancel = handles.cancel;
                let wprogress = handles.progress.expect("manifest jobs carry progress");
                let wpath = PathBuf::from(&pkg.package_path);
                let wapp = app.clone();
                std::thread::Builder::new()
                    .name("manifest-worker".into())
                    .spawn(move || {
                        let report_cancel = wcancel.clone();
                        let last_progress = AtomicU64::new(0);
                        let report = move |done: u64, total: u64| {
                            wprogress.done.store(done, Ordering::Relaxed);
                            wprogress.total.store(total, Ordering::Relaxed);
                            let now_ms = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_millis() as u64)
                                .unwrap_or(0);
                            if now_ms.saturating_sub(last_progress.load(Ordering::Relaxed)) >= 250
                            {
                                last_progress.store(now_ms, Ordering::Relaxed);
                                emit_changed(&wapp, "jobs");
                            }
                            !report_cancel.load(Ordering::Relaxed)
                        };
                        let outcome = read_manifest_with_progress(&wpath, &report)
                            .map_err(|e| e.to_string())
                            .map(|m| {
                                m.iter()
                                    .map(|e| PackageEntryRecord {
                                        guid: e.guid.clone(),
                                        pathname: e.pathname.clone(),
                                        tar_offset: e.asset_offset.map(|o| o as i64),
                                        size_bytes: e.asset_size.map(|s| s as i64),
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .and_then(|entries| {
                                if entries.is_empty() {
                                    Err("the pack has no manifest entries".to_string())
                                } else {
                                    Ok(entries)
                                }
                            });
                        let cancelled = wcancel.load(Ordering::Relaxed);
                        let _ = wtx.send(WriterMsg::ManifestDone {
                            job_id: handles.id,
                            package_id,
                            reply: wreply,
                            entries: outcome,
                            cancelled,
                        });
                    })
                    .expect("manifest worker spawns");
            } else {
                // Locators already recorded (browse persistence) — mint + extract right away.
                mint_in_pack(catalog, package_id, &entries)?;
                spawn_extract_worker(&pkg, &workspace_dir, &entries, tx, reply, jobs, app)?;
            }
            Ok(WriteResult {
                changed: 0, // queued — the tray reports the real changed count
                ..Default::default()
            })
        }
        Op::RenameCategory { id, label } => catalog
            .rename_category(id, &label)
            .map(|_| WriteResult { changed: 1, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::SetActiveProject { project_id } => catalog
            .set_active_project(project_id)
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

        Op::RedoOp { op_id } => catalog
            .redo_op(op_id)
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

/// Mint a pack's in-pack assets (M14 surgical indexing): classify the recorded locators by
/// extension and create an `asset` + `asset_file` per classifiable entry, `package_entry_id`
/// set, `workspace_rel` NULL — the bytes still live inside the archive, so identify/preview
/// pull them one at a time through the ladder and nothing is materialized yet.
fn mint_in_pack(
    catalog: &mut Catalog,
    package_id: i64,
    entries: &[PackageEntryRecord],
) -> Result<usize, String> {
    let classified = lodestash_app::persist::classify_in_pack_entries(entries);
    let stats = catalog
        .mint_in_pack_assets(package_id, &classified)
        .map_err(|e| e.to_string())?;
    Ok(stats.inserted)
}

/// Spawn the extract worker for one pack (INGESTION §2 surgical extraction): pull **only** the
/// payloads whose locators are recorded, with one gzip skip-scan over the archive, into the
/// workspace. Progress + cooperative cancel ride the job tray; the writer materializes the
/// results when the worker reports back (WriterMsg::ExtractDone).
fn spawn_extract_worker(
    pkg: &lodestash_core::catalog::PackageListing,
    workspace_dir: &Path,
    entries: &[PackageEntryRecord],
    tx: &Sender<WriterMsg>,
    reply: &Sender<Result<WriteResult, String>>,
    jobs: &JobRegistry,
    app: &Option<tauri::AppHandle>,
) -> Result<(), String> {
    let wanted: Vec<EntryLocator> = entries
        .iter()
        .filter_map(|e| {
            Some(EntryLocator {
                guid: e.guid.clone(),
                pathname: e.pathname.clone(),
                offset: e.tar_offset? as u64,
                size: e.size_bytes? as u64,
            })
        })
        .collect();
    if wanted.is_empty() {
        return Err("the pack has no extractable payloads".to_string());
    }
    let handles = jobs.start(format!("Extract {}", pkg.name), "extract", true);
    let wtx = tx.clone();
    let wreply = reply.clone();
    let wcancel = handles.cancel;
    let wprogress = handles.progress.expect("extract jobs carry progress");
    let wpath = PathBuf::from(&pkg.package_path);
    let wworkspace = workspace_dir.to_path_buf();
    let wapp = app.clone();
    let wpkg_id = pkg.id;
    std::thread::Builder::new()
        .name("extract-worker".into())
        .spawn(move || {
            let report_cancel = wcancel.clone();
            let last_progress = AtomicU64::new(0);
            let report = move |done: u64, total: u64| {
                wprogress.done.store(done, Ordering::Relaxed);
                wprogress.total.store(total, Ordering::Relaxed);
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if now_ms.saturating_sub(last_progress.load(Ordering::Relaxed)) >= 250 {
                    last_progress.store(now_ms, Ordering::Relaxed);
                    emit_changed(&wapp, "jobs");
                }
                !report_cancel.load(Ordering::Relaxed)
            };
            let outcome = extract_at_offsets_with_progress(&wpath, &wworkspace, wanted, &report)
                .map_err(|e| e.to_string())
                .and_then(|a| {
                    if a.is_empty() {
                        Err("the pack extracted no assets".to_string())
                    } else {
                        Ok(a)
                    }
                });
            let cancelled = wcancel.load(Ordering::Relaxed);
            let _ = wtx.send(WriterMsg::ExtractDone {
                job_id: handles.id,
                package_id: wpkg_id,
                reply: wreply,
                outcome,
                cancelled,
            });
        })
        .expect("extract worker spawns");
    Ok(())
}

/// Materialize an extract worker's payloads (M14): fill `workspace_rel` on the minted rows
/// whose `package_entry.pathname` matched, then record the unpack. The rows hydrate from the
/// workspace from here on (extracted → in-pack → on-disk ladder, DATA-MODEL §7).
fn materialize_extract(
    catalog: &mut Catalog,
    package_id: i64,
    outcome: Result<Vec<ExtractedAsset>, String>,
) -> Result<WriteResult, String> {
    let extracted = outcome?;
    let pairs: Vec<(String, String)> = extracted
        .iter()
        .filter_map(|a| {
            a.pathname
                .clone()
                .map(|p| (p, a.workspace_path.to_string_lossy().into_owned()))
        })
        .collect();
    let matched = catalog
        .materialize_in_pack_assets(package_id, &pairs)
        .map_err(|e| e.to_string())?;
    let changed = matched.len();
    catalog
        .mark_package_unpacked(package_id, changed)
        .map_err(|e| e.to_string())?;
    Ok(WriteResult {
        changed,
        ..Default::default()
    })
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
