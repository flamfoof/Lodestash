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
use std::sync::{Arc, Mutex};

use rusqlite::Connection;

use lodestash_ai::clap_tags::AudioZeroShot;
use lodestash_ai::image_tags::ImageZeroShot;
use lodestash_ai::ollama::OllamaTagger;
use lodestash_ai::openai::OpenAiTagger;
use lodestash_ai::tagger::{FilenameTagger, Tagger, TeachTrait};
use lodestash_app::config::AppConfig;
use lodestash_app::drains::{
    drain_audio_tags, drain_previews, drain_promote_resync, drain_tags, resolve_audio_embedder,
    resolve_embedder,
};
use lodestash_app::jobs::JobProgress as AppJobProgress;
use lodestash_app::paths::default_cache_dirs;
use lodestash_app::types::PersistBatch;
use lodestash_core::catalog::{
    AppraiseFocus, Catalog, HistoryOrigin, PackageEntryRecord, TagBatch, TagBatchItem, TagMode,
};
use lodestash_core::types::{Exclude, ExcludeKind};
use lodestash_ingest::unitypackage::{
    extract_at_offsets_with_progress, read_manifest_with_progress, EntryLocator, ExtractedAsset,
};

/// Bounded tag work per appraise pass (AI-PIPELINE §5.1): a writer-thread kick never blocks
/// writes on a long backlog; the remainder stays queued for the next kick / idle slice.
const TAG_BUDGET_PER_PASS: u32 = 64;
const AUDIO_TAG_BUDGET_PER_PASS: u32 = 48;

/// How often the idle backlog lane takes a turn: one tick in four, each tick a second.
///
/// Every tick used to do a slice, which is a 100% duty cycle dressed up as "idle work" — an
/// app showing a grid and being asked for nothing sat at 0.57 cores indefinitely. Four is the
/// tradeoff: the backlog converges ~4× slower in wall-clock terms, and the machine stays the
/// user's. Raise it to be politer, lower it to drain faster.
const IDLE_DUTY_PERIOD: u32 = 4;

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
    SetFavorite {
        ids: Vec<i64>,
        on: bool,
    },
    /// Mark assets **confirmed** — the human's "I checked these tags" (DATA-MODEL §8,
    /// AI-PIPELINE §6). The one status a person sets: `promote_statuses` never writes or clears it,
    /// and un-confirming recomputes the status the ingest work actually earned rather than leaving
    /// the row stale. Matters most right after a bulk AI pass, which is exactly when "which of
    /// these did I actually look at" stops being answerable any other way.
    SetConfirmed {
        ids: Vec<i64>,
        on: bool,
    },
    /// Mute or unmute one dependency edge — UI-SPEC §13.6's **⊘ Ignore link** / **↺ restore**.
    /// A muted edge keeps its row (an ignore has to survive the next dependency scan, which
    /// re-derives the harvest wholesale) and stops resolving to anything.
    SetLinkIgnored {
        edge_id: i64,
        ignored: bool,
    },
    /// Mark a pack's extracted bytes as worth keeping (M23, UI-SPEC §15). Not `pinned`, which
    /// already means "Add to Library" — see the migration.
    SetPackKeepCached {
        package_id: i64,
        keep: bool,
    },
    /// Point a pack at a different `.unitypackage` (UI-SPEC §15 Retarget…). Only the path moves:
    /// every asset, locator and extracted byte stays, because what the rows describe has not
    /// changed — only where the bytes can be re-read from.
    RetargetPackage {
        package_id: i64,
        new_path: String,
    },
    /// Re-derive every source's availability from disk (UI-SPEC §11.1, M18).
    ///
    /// It writes `import_source.state`, so it belongs on the writer thread rather than on a read
    /// connection — and it is a `stat` per source **root**, not per asset, so it stays cheap enough
    /// to run on open and after every scan.
    RefreshSourceHealth,
    /// Undo one **export** run (FEATURES §7.1): delete the files it copied out, then forget it.
    ///
    /// Destructive, and the only delete in the app that touches files the *user* chose the location
    /// of — so it is fenced to exactly the names the run recorded, under exactly the folder it
    /// recorded, and never anywhere near a library root.
    UndoExportBatch {
        batch_id: String,
    },
    /// Point a source at a new root (UI-SPEC §11.1). The drive came back under a different letter,
    /// the folder moved, the library was copied to a bigger disk.
    ///
    /// A rename, not a re-scan: `asset_file.rel_path` is relative, so every row keeps its identity,
    /// tags, stars and import history. Only `import_source.root_path` and the absolute
    /// `package.package_path` under it are rewritten.
    RelinkSource {
        source_id: i64,
        new_root: std::path::PathBuf,
    },
    /// Soft delete / restore. Catalog-only; the file on disk is never touched (DATA-MODEL §8).
    SetTrashed {
        ids: Vec<i64>,
        trashed: bool,
    },
    /// **Remove from library** — the safe removal. Soft-deletes the row *and* blacklists the
    /// asset in its source, so the next scan does not bring it back. No file is touched.
    RemoveFromLibrary {
        targets: Vec<Target>,
        project: Option<String>,
    },
    /// **Delete from disk** — the one irreversible action in this product.
    DeleteFromDisk {
        targets: Vec<Target>,
        roots: Vec<PathBuf>,
        workspace: Option<PathBuf>,
    },
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
    /// Import selected `.unitypackage`s as libraries (FEATURES §5.2): upsert the folder as one
    /// source, then per pack upsert the `package` row, record its manifest locators, mint the
    /// in-pack assets and pin it — one Library per pack, indexed in place, nothing copied.
    ImportPacks {
        /// The picked folder — one source row for every selected pack (idempotent by root).
        root: String,
        packs: Vec<ImportPack>,
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
    UnpromoteTrait {
        tag_id: i64,
    },
    /// Toggle a trait's teaching flag.
    SetTraitTeach {
        tag_id: i64,
        teach: bool,
    },
    /// Set a trait's chip style — colour (CSS token or hex) + optional icon id. `None` clears.
    SetTraitStyle {
        tag_id: i64,
        color: Option<String>,
        icon: Option<String>,
    },
    /// Pause/resume watching one source (UI-SPEC §11.1). Paused sources stay cataloged.
    SetSourceWatch {
        id: i64,
        watch: bool,
    },
    /// Create a named collection (UI-SPEC §6).
    CreateCollection {
        name: String,
    },
    /// Delete a collection; memberships cascade (UI-SPEC §6).
    DeleteCollection {
        id: i64,
    },
    /// Pin/unpin a `.unitypackage` as a library (FEATURES §5.4) — pinned packs surface in the
    /// sidebar's LIBRARIES facet and get the "Add all to project" affordance.
    SetPackagePinned {
        id: i64,
        pinned: bool,
    },
    /// The per-source blacklist editor (UI-SPEC §11.1): add/remove one `{kind, value}`
    /// exclusion. `kind` is `folder` or `asset` (DATA-MODEL §10).
    AddSourceExclude {
        source_id: i64,
        kind: String,
        value: String,
    },
    RemoveSourceExclude {
        source_id: i64,
        kind: String,
        value: String,
    },
    /// The Mapping tab's taxonomy switcher / authoring (UI-SPEC §9.2).
    SetActiveTaxonomy {
        id: i64,
    },
    DeleteTaxonomy {
        id: i64,
    },
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
    DeleteRule {
        rule_id: i64,
    },
    /// Add a category under a parent (or the root) with its materialized path.
    AddCategory {
        taxonomy_id: i64,
        parent_id: Option<i64>,
        pref_label: String,
        path: String,
    },
    /// Run the deterministic mapping engine over one taxonomy.
    ApplyMapping {
        taxonomy_id: i64,
    },
    /// ✦ Identify all — one bounded appraise pass (embeddings → tags → promote) on the
    /// persist thread, so the catalog stays single-writer. Manual, like the egui build's
    /// Identify button (AI-PIPELINE §2); auto-identify stays a config opt-in.
    IdentifyAll,
    RenameCategory {
        id: i64,
        label: String,
    },
    DeleteCategory {
        id: i64,
    },
    /// Manual appraise kick for one asset (per-card ✦, KANBAN B11b): flags the asset for
    /// re-embed + re-tag, then runs one bounded pass so it actually happens now.
    IdentifyOne {
        asset_id: i64,
    },
    /// Targeted appraise kick for an explicit set (ENGINE-API §2 `identify_asset`): flags
    /// each, then ONE bounded pass so a bulk agent call is a single pass, not N.
    IdentifyIds {
        asset_ids: Vec<i64>,
    },
    /// Stage/unstage assets in the current staging tray — the active project's, or the
    /// project-less one when no editor is registered (ENGINE-API §6, UI-SPEC §16.2, M20).
    ///
    /// The writer resolves the tray so no plane touches the catalog itself (ADR-0002), and both
    /// planes now resolve the *same* tray: the shell used to keep an in-memory list beside this
    /// one and the two never agreed (ADR-0010). Takes many ids because the shell stages many —
    /// `⟡ Stage all` on the bulk bar is one gesture and should be one write.
    StageAssets {
        asset_ids: Vec<i64>,
    },
    UnstageAssets {
        asset_ids: Vec<i64>,
    },
    /// Empty the tray (UI-SPEC §16.2 `clear`).
    ClearStaged,
    /// Per-pack Extract (B2b slice): unpack the archive into the workspace, then reconcile
    /// the payloads into the catalog like a crawl. Explicit and on-demand (UI-SPEC §11.6) —
    /// browsing never unpacks; `lodestash://file` answers 503 until this runs.
    ExtractPackage {
        package_id: i64,
    },
    /// Switch which registered Unity editor is active (ENGINE-API §5.1, UI-SPEC §16.1).
    SetActiveProject {
        project_id: i64,
    },
    /// Undo one import batch (UI-SPEC §16.1): delete the batch's copied files from the project
    /// and its import_record rows — catalog assets are never touched. Runs on this thread like
    /// every catalog write (ADR-0002).
    UndoImportBatch {
        batch_id: String,
    },

    /// Undo one logged mutation (UI-SPEC §2 Ctrl/Cmd-Z, DESIGN-GUIDELINES §7 L5).
    RevertOp {
        op_id: i64,
    },
    /// Redo one undone op (UI-SPEC §2 Shift-Z): applies the op's forward payload and consumes
    /// the log row — the inverse of [`Op::RevertOp`].
    RedoOp {
        op_id: i64,
    },
    /// Toggle one asset's membership in a collection (UI-SPEC §6, §13.5).
    CollectionAddAsset {
        collection_id: i64,
        asset_id: i64,
    },
    CollectionRemoveAsset {
        collection_id: i64,
        asset_id: i64,
    },
    /// Bulk-bar tag edit (KANBAN B9d): add/remove one `(namespace, value)` tag on many
    /// assets. Each asset gets its own undo-log entry snapshotted before the write, so
    /// `revert_op` restores the exact prior per-source tag set.
    BulkTag {
        asset_ids: Vec<i64>,
        namespace: String,
        value: String,
        on: bool,
    },
    /// One validated, attributed tag batch (v1 plan S04/S05) — an approved agent request or a
    /// legacy agent add/remove — applied atomically with its history and receipt.
    TagBatch {
        batch: lodestash_core::catalog::TagBatch,
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
    /// Who asked — decides how the shell hears about it (ADR-0016).
    pub origin: Origin,
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
        entries: Result<Vec<PackageEntryRecord>, String>,
        cancelled: bool,
    },
    ExtractDone {
        job_id: u64,
        package_id: i64,
        outcome: Result<Vec<ExtractedAsset>, String>,
        cancelled: bool,
    },
    /// A background embed worker finished a slice. The writer only **persists** it — the encode
    /// happened off-thread, which is the point: a four-view render plus four SigLIP passes is
    /// ~17 s per 3D model, and doing that on the writer made every user write queue behind it
    /// (a `POST /v1/stage` went from 25 ms to ~18 s the moment an embedder actually loaded).
    EmbeddingsDone {
        model_id: String,
        done: Vec<lodestash_app::drains::ComputedEmbedding>,
    },
    /// A background tag worker finished a slice — same split, same reason: a caption is a whole
    /// VLM inference (~13 s here) and the writer only persists the result.
    TagsDone {
        producers: lodestash_app::drains::TagProducers,
        done: Vec<lodestash_app::drains::ComputedTags>,
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

/// One `.unitypackage` the user chose to import as a library (FEATURES §5.2). Carries the
/// browse-time manifest locators so the writer records + mints without re-inflating the archive
/// (the discovery scan already read it). `publisher`/`category` come from the dialog (publisher
/// = the containing folder; category = the auto-detected badge or the user's dropdown override).
/// Built server-side in `import_packages` — it never crosses the IPC boundary, so no `Deserialize`.
pub struct ImportPack {
    pub abs_path: String,
    pub name: String,
    pub publisher: Option<String>,
    pub category: Option<String>,
    pub entries: Vec<PackageEntryRecord>,
}

/// Record what a pack says about itself: its version and licence (INGESTION §2 "Harvest
/// license/attribution") and the render pipeline it targets (DATA-MODEL §12), which is what the
/// `pipeline_mismatch` import advisory compares against (ENGINE-API §5.4).
///
/// Best-effort throughout: a pack whose archive will not open still imports, it just records
/// less. Nothing here is a guess — an unknown licence or pipeline is left NULL rather than
/// defaulted, except the one default the origin genuinely licenses (below).
fn harvest_pack_metadata(
    catalog: &Catalog,
    package_id: i64,
    abs_path: &str,
    name: &str,
    publisher: Option<&str>,
    entries: &[PackageEntryRecord],
) {
    // Names first: the pack's own name and the pathnames its manifest already recorded. Free —
    // no archive read — and it is how a pack that ships a variant per pipeline says so.
    let mut pipelines = lodestash_ingest::pipeline::from_path(name);
    for e in entries {
        if let Some(p) = e.pathname.as_deref() {
            pipelines.extend(lodestash_ingest::pipeline::from_path(p));
        }
    }

    // The archive is reopened **only** when the manifest says there is something in it to read:
    // `read_package_meta` streams until it has found its candidates, so a pack declaring none
    // would cost a full inflate to learn nothing. (The desktop wants no pack icon, which is the
    // other thing that pass reads.)
    let declares_meta = entries.iter().any(|e| {
        e.pathname
            .as_deref()
            .is_some_and(lodestash_ingest::unitypackage::is_metadata_pathname)
    });
    let meta = declares_meta
        .then(|| {
            let manifest: Vec<lodestash_ingest::unitypackage::PackageEntry> = entries
                .iter()
                .map(|e| lodestash_ingest::unitypackage::PackageEntry {
                    guid: e.guid.clone(),
                    pathname: e.pathname.clone(),
                    asset_size: e.size_bytes.map(|n| n as u64),
                    ..Default::default()
                })
                .collect();
            lodestash_ingest::unitypackage::read_package_meta(Path::new(abs_path), &manifest)
                .map_err(|e| tracing::warn!(%e, pack = %name, "pack metadata harvest failed"))
                .ok()
        })
        .flatten();
    if let Some(m) = &meta {
        pipelines.extend(m.pipeline.iter().copied());
    }

    // A pack with a publisher came out of the Asset Store cache tree, which is enough to know its
    // licence. The pack's own statement wins; the EULA only fills a silence.
    let store_pack = publisher.is_some();
    let license = meta
        .as_ref()
        .and_then(|m| m.license.clone())
        .or_else(|| store_pack.then(|| "Unity Asset Store EULA".to_string()));
    let license_url = meta
        .as_ref()
        .and_then(|m| m.license_url.clone())
        .or_else(|| store_pack.then(|| "https://unity.com/legal/as-terms".to_string()));
    let version = meta.as_ref().and_then(|m| m.version.clone());
    if let Err(e) = catalog.set_package_metadata(
        package_id,
        version.as_deref(),
        license.as_deref(),
        publisher,
        license_url.as_deref(),
    ) {
        tracing::warn!(%e, pack = %name, "pack metadata write failed");
    }
    if let Err(e) =
        catalog.set_package_pipeline(package_id, lodestash_ingest::pipeline::resolve(&pipelines))
    {
        tracing::warn!(%e, pack = %name, "pack pipeline write failed");
    }
}

/// Classify the packs already in the catalog whose `pipeline` was never determined
/// (DATA-MODEL §12). The harvest runs at import, so without this every pack indexed before it
/// existed stays unknown for good — and an advisory that can never fire for the library you
/// already have is not an advisory.
///
/// Reads only names (the pack's own and its recorded `package_entry` pathnames), which are
/// already in the catalog, so this is a tokenise over strings rather than any archive work.
/// Returns how many packs it classified.
fn backfill_package_pipelines(catalog: &Catalog) -> usize {
    let rows = match catalog.packages_needing_pipeline() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "pipeline backfill: query failed");
            return 0;
        }
    };
    let mut written = 0usize;
    let mut current: Option<(i64, std::collections::BTreeSet<&'static str>)> = None;
    let flush = |cur: Option<(i64, std::collections::BTreeSet<&'static str>)>| {
        let Some((id, seen)) = cur else { return 0 };
        let Some(p) = lodestash_ingest::pipeline::resolve(&seen) else {
            return 0;
        };
        match catalog.set_package_pipeline(id, Some(p)) {
            Ok(()) => 1,
            Err(e) => {
                tracing::warn!(%e, package_id = id, "pipeline backfill: write failed");
                0
            }
        }
    };
    for (id, name, pathname) in rows {
        if current.as_ref().map(|(c, _)| *c) != Some(id) {
            written += flush(current.take());
            current = Some((id, lodestash_ingest::pipeline::from_path(&name)));
        }
        if let (Some((_, seen)), Some(p)) = (current.as_mut(), pathname.as_deref()) {
            seen.extend(lodestash_ingest::pipeline::from_path(p));
        }
    }
    written += flush(current.take());
    if written > 0 {
        tracing::info!(packs = written, "render pipeline backfilled");
    }
    written
}

// ── Job tray (B2b) ─────────────────────────────────────────────────────────────────────────

/// Shared tray state: active jobs (progress + cooperative cancel) and a session ring of
/// finished ones. The writer thread owns the mutations; commands read snapshots.
#[derive(Clone, Default)]
pub struct JobRegistry {
    inner: std::sync::Arc<std::sync::Mutex<JobState>>,
    /// Marked on every start and finish, and by workers as their progress moves — the tray is
    /// one of the two domains the shell is told about.
    changes: Changes,
}

#[derive(Default)]
struct JobState {
    next_id: u64,
    active: Vec<ActiveJob>,
    recent: VecDeque<RecentJob>,
}

/// Extract byte progress, read by the tray without taking the registry lock.
#[derive(Default)]
pub struct JobProgress {
    pub done: AtomicU64,
    pub total: AtomicU64,
}

struct ActiveJob {
    id: u64,
    label: String,
    kind: String,
    cancel: Arc<AtomicBool>,
    progress: Option<Arc<JobProgress>>,
}

/// Handles a caller holds after `start`, without touching the registry lock again.
pub struct JobHandles {
    pub id: u64,
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<Arc<JobProgress>>,
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
    /// Public: model downloads (which never touch the catalog) register through this.
    pub fn start(&self, label: String, kind: &str, cancellable: bool) -> JobHandles {
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
        drop(state);
        self.changes.jobs();
        JobHandles {
            id,
            cancel,
            progress,
        }
    }

    /// Move an active job to the session ring. `changed` is what the tray reports.
    /// Move an active job to the session ring. Public for the same reason as `start`.
    pub fn finish(&self, id: u64, ok: bool, cancelled: bool, changed: usize) {
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
        drop(state);
        self.changes.jobs();
    }

    fn has_active_extract(&self) -> bool {
        self.inner
            .lock()
            .expect("job registry lock")
            .active
            .iter()
            .any(|j| j.kind == "extract")
    }

    /// The process's change publication, which this registry marks whenever the tray changes.
    pub fn changes(&self) -> &Changes {
        &self.changes
    }

    /// A running job's progress moved. Coalesced by the publisher, so a worker can call this per
    /// chunk read without flooding the shell.
    pub fn progressed(&self) {
        self.changes.jobs();
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
                        (
                            p.done.load(Ordering::Relaxed),
                            p.total.load(Ordering::Relaxed),
                        )
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

/// Who caused a catalog change. The shell treats them differently (ADR-0016): the user's own
/// write refreshes what they are looking at; an agent's or the background lane's only says that
/// something moved, so a drain can never pull the grid out from under someone browsing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Agent,
    Background,
}

/// What the shell hears on `ls:changed`: which domains changed since the last publication, every
/// origin that touched the catalog in that window, and the per-domain generations. A refresh that
/// read the catalog at generation G clears only what was pending through G.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ChangedEvent {
    pub catalog: bool,
    pub jobs: bool,
    pub origins: Vec<&'static str>,
    pub catalog_gen: u64,
    pub jobs_gen: u64,
}

#[derive(Default)]
struct Pending {
    catalog: bool,
    jobs: bool,
    user: bool,
    agent: bool,
    background: bool,
    catalog_gen: u64,
    jobs_gen: u64,
}

impl Pending {
    fn dirty(&self) -> bool {
        self.catalog || self.jobs
    }

    /// Everything pending, as one event. The flags reset; the generations only ever climb.
    fn take(&mut self) -> ChangedEvent {
        let origins = [
            (self.user, "user"),
            (self.agent, "agent"),
            (self.background, "background"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        let event = ChangedEvent {
            catalog: self.catalog,
            jobs: self.jobs,
            origins,
            catalog_gen: self.catalog_gen,
            jobs_gen: self.jobs_gen,
        };
        self.catalog = false;
        self.jobs = false;
        self.user = false;
        self.agent = false;
        self.background = false;
        event
    }
}

/// The longest a committed change waits before the shell hears of it. A fixed window, not a
/// trailing debounce: the preview drain commits continuously, and a debounce waiting for quiet
/// would never fire while it ran. Fifty tag writes inside one window publish once; a longer burst
/// publishes once per window — never once per commit, which is what it used to be.
const PUBLISH_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);

/// The process's change publication (ADR-0016).
///
/// Marking is a mutex flip — the writer never waits on the shell, and nothing is emitted from
/// inside a transaction. A publisher thread, started once there is a window to tell, drains the
/// marks at most once per [`PUBLISH_WINDOW`]. Until then — and forever, headless — marks only
/// accumulate: no events and no growth, since a mark is a flag and a counter.
#[derive(Clone, Default)]
pub struct Changes {
    inner: Arc<(Mutex<Pending>, std::sync::Condvar)>,
}

impl Changes {
    pub fn catalog(&self, origin: Origin) {
        let (lock, cv) = &*self.inner;
        let mut p = lock.lock().expect("changes lock");
        p.catalog = true;
        p.catalog_gen += 1;
        match origin {
            Origin::User => p.user = true,
            Origin::Agent => p.agent = true,
            Origin::Background => p.background = true,
        }
        cv.notify_one();
    }

    pub fn jobs(&self) {
        let (lock, cv) = &*self.inner;
        let mut p = lock.lock().expect("changes lock");
        p.jobs = true;
        p.jobs_gen += 1;
        cv.notify_one();
    }

    /// Tell the shell, as `ls:changed`. Called once, when the window exists.
    pub fn start_publisher(&self, app: tauri::AppHandle) {
        self.spawn_publisher(PUBLISH_WINDOW, move |event| {
            use tauri::Emitter;
            let _ = app.emit("ls:changed", event);
        });
    }

    fn spawn_publisher(
        &self,
        window: std::time::Duration,
        mut sink: impl FnMut(ChangedEvent) + Send + 'static,
    ) {
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("lodestash-changes".into())
            .spawn(move || {
                let (lock, cv) = &*inner;
                loop {
                    let mut p = lock.lock().expect("changes lock");
                    while !p.dirty() {
                        p = cv.wait(p).expect("changes lock");
                    }
                    drop(p);
                    // The window opens at the first mark; everything landing inside it rides along.
                    std::thread::sleep(window);
                    let event = lock.lock().expect("changes lock").take();
                    sink(event);
                }
            })
            .expect("change publisher spawns");
    }
}

/// Publish a catalog change iff this turn committed rows. `total_changes` is SQLite's own count,
/// so a turn that ran and wrote nothing is not reported as a change.
fn mark_committed(changes: &Changes, catalog: &Catalog, before: u64, origin: Origin) {
    if catalog.total_changes() > before {
        changes.catalog(origin);
    }
}

/// The writer's channel, plus the one message the idle probe may have taken out of it.
///
/// The probe used to be `rx.recv_timeout(ZERO).is_ok()`: a receive whose message was then dropped
/// on the floor. A dropped `Op` never runs and never replies — its caller got "writer thread
/// dropped the job" for a write whose only fault was arriving at the idle boundary.
struct Inbox {
    rx: mpsc::Receiver<WriterMsg>,
    held: Option<WriterMsg>,
}

impl Inbox {
    fn next(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<WriterMsg, std::sync::mpsc::RecvTimeoutError> {
        match self.held.take() {
            Some(msg) => Ok(msg),
            None => self.rx.recv_timeout(timeout),
        }
    }

    /// Is a message waiting? One found here is held for [`Inbox::next`], never discarded.
    fn waiting(&mut self) -> bool {
        if self.held.is_none() {
            self.held = self.rx.try_recv().ok();
        }
        self.held.is_some()
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
        appraise: Arc<AppJobProgress>,
        // `focus` is where the user is looking (UI-SPEC §3). Shared with the app rather than sent
        // as an op: it changes on every scroll, and a browse must never queue behind a running
        // appraise pass.
        focus: Arc<Mutex<AppraiseFocus>>,
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
                // One-shot, before the loop: packs indexed before the harvest existed have no
                // pipeline, and only the writer thread may fix that (single-writer rule).
                backfill_package_pipelines(&catalog);
                // …and enforce the preview cap the settings screen has always shown and never
                // applied (UI-SPEC §11.4). Startup rather than per-write: the cache grows a
                // thumbnail at a time and a check on that path would stat the whole tree on every
                // preview.
                {
                    let cfg = AppConfig::load();
                    if let Some(thumbs) = default_cache_dirs().0 {
                        // Identity before pressure. A stale-budget proxy, an orphan staging
                        // mesh and a crash-leftover `.part` are dead the moment they become
                        // so, and the cap never reaches them: 359 MB against an 8 GB cap
                        // means the purge below had never once run while 145 MB of that was
                        // already garbage.
                        crate::storage::sweep_dead_mesh_cache(
                            &thumbs,
                            &cfg.preview_quality.proxy_budget().tag(),
                        );
                        if cfg.auto_purge_preview_cache {
                            crate::storage::purge_previews_over_cap(
                                &thumbs,
                                u64::from(cfg.preview_cache_cap_gb) * 1024 * 1024 * 1024,
                            );
                        }
                    }
                }
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
                // One embed slice in flight at a time. The worker owns the encode; this flag is
                // how the idle tick knows not to start a second one, and it is cleared when the
                // slice's `EmbeddingsDone` lands (or when its worker gives up).
                let embed_inflight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                let tag_inflight = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
                // DATA-MODEL §6 watch: live notify over the watched source roots, debounced by
                // the ingest helper. A signal means "something changed, rescan when convenient"
                // — the rescan is the incremental crawl (needs_rehash skips unchanged files), so
                // it is cheap. Skipped under the e2e escape hatch like every other idle pass.
                let mut watch = if drain_enabled && AppConfig::load().watch_sources {
                    rebuild_watch(&catalog)
                } else {
                    None
                };
                // The backlog lane works one tick in `IDLE_DUTY_PERIOD`, not every tick.
                //
                // Measured on the reference library: an idle app — window open, nothing asked of
                // it — burned **0.57 cores continuously**, forever, because each 1 s timeout
                // started a 1 s slice. This is an asset viewer; housekeeping does not get half the
                // machine. Duty-cycling here throttles all three lanes at once: the embed and tag
                // workers gate on their `*_inflight` flags, and only this thread clears them.
                //
                // Op latency is unaffected — a skipped tick still waits on the channel, so a user
                // write is picked up the moment it arrives (the point of 147d4b5).
                let mut idle_tick: u32 = 0;
                let changes = jobs.changes().clone();
                let mut inbox = Inbox { rx, held: None };
                loop {
                    match inbox.next(std::time::Duration::from_secs(1)) {
                        Ok(WriterMsg::Op(job)) => {
                            if matches!(job.op, Op::ExtractPackage { .. }) {
                                // Extract answers **"accepted"**, not "finished" — the unpack runs
                                // for minutes on a multi-GB pack, and its progress, outcome and
                                // cancel button live in the job tray. Both branches reply at the
                                // same point on purpose: the reply used to be sent at spawn when
                                // the queue was empty but only at *start* when it wasn't, so one
                                // command meant two different things depending on queue depth.
                                if extract_queue.is_empty() && !jobs.has_active_extract() {
                                    let result = run(
                                        &mut catalog,
                                        &db_path,
                                        job.op,
                                        &writer_tx,
                                        &jobs,
                                        &appraise,
                                        &focus,
                                    );
                                    let _ = job.reply.send(result);
                                } else {
                                    // A pack is unpacking (or queued) — join the queue, and say so
                                    // now rather than blocking the caller until this pack's turn.
                                    let _ = job.reply.send(Ok(WriteResult {
                                        changed: 0,
                                        ..Default::default()
                                    }));
                                    extract_queue.push_back(job);
                                }
                                continue;
                            }
                            // Only the appraise kicks are slow enough to count as "the pump is
                            // running" — everything else is a quick write the frontend already
                            // waits on synchronously.
                            let appraising =
                                matches!(job.op, Op::IdentifyAll | Op::IdentifyOne { .. });
                            if appraising {
                                pump.store(true, Ordering::Relaxed);
                            }
                            let source_set_changed = matches!(
                                job.op,
                                Op::PersistScan { .. } | Op::SetSourceWatch { .. }
                            );
                            let before = catalog.total_changes();
                            let result = run(
                                &mut catalog,
                                &db_path,
                                job.op,
                                &writer_tx,
                                &jobs,
                                &appraise,
                                &focus,
                            );
                            if appraising {
                                pump.store(false, Ordering::Relaxed);
                            }
                            // A failed op publishes nothing: its caller learns from the reply,
                            // and an event must never read as a success.
                            if result.is_ok() {
                                mark_committed(&changes, &catalog, before, job.origin);
                            }
                            let _ = job.reply.send(result);
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
                            outcome,
                            cancelled,
                        }) => {
                            // The payloads landed on disk off-thread; materialize them here —
                            // the catalog stays single-writer. Then the next queued pack gets
                            // its worker.
                            let before = catalog.total_changes();
                            let result =
                                materialize_extract(&mut catalog, package_id, outcome, cancelled);
                            let ok = result.is_ok();
                            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
                            jobs.finish(job_id, ok, cancelled, changed);
                            start_next_extract(
                                &mut catalog,
                                &mut extract_queue,
                                &db_path,
                                &writer_tx,
                                &jobs,
                                &appraise,
                                &focus,
                            );
                            // Background: the unpack was asked for minutes ago, and its outcome
                            // is in the tray — not a reason to re-read the page under the user.
                            mark_committed(&changes, &catalog, before, Origin::Background);
                        }
                        Ok(WriterMsg::EmbeddingsDone { model_id, done }) => {
                            // Cheap: N `upsert_embedding` calls, no inference. Clearing
                            // `embedding_dirty` here is what lets `promote_statuses` move the
                            // asset on, so the promote runs on the next idle tick.
                            let n = lodestash_app::drains::store_visual_embeddings(
                                &mut catalog,
                                &model_id,
                                &done,
                            );
                            embed_inflight.store(false, Ordering::Relaxed);
                            if n > 0 {
                                changes.catalog(Origin::Background);
                            }
                        }
                        Ok(WriterMsg::TagsDone { producers, done }) => {
                            let before = catalog.total_changes();
                            let n = lodestash_app::drains::store_tags(&mut catalog, &producers, &done);
                            tag_inflight.store(false, Ordering::Relaxed);
                            if n > 0 {
                                tracing::info!(tags = n, producer = %producers.tagger.0, "idle tagging slice stored");
                            }
                            // An empty result still finishes its assets (the queue flag clears), so
                            // the catalog changed even when no tag row did.
                            mark_committed(&changes, &catalog, before, Origin::Background);
                        }
                        Ok(WriterMsg::ManifestDone {
                            job_id,
                            package_id,
                            entries,
                            cancelled,
                        }) => {
                            // The archive's manifest is read; record the locators + mint the
                            // in-pack assets (surgical indexing, M14), then spawn the extract
                            // worker over the locators. The op's reply is answered by that
                            // phase's ExtractDone — except when the manifest failed, which is
                            // answered here so the caller never hangs.
                            // `true` = an extract worker is now running, so *its* `ExtractDone`
                            // drains the queue. Anything else — an error, or a cancel that stops
                            // after the analyze phase — has to settle here, or the queue behind it
                            // never moves again.
                            let before = catalog.total_changes();
                            let mut spawned = false;
                            // Annotated: nothing consumes this as a reply any more (the tray is
                            // the reporting surface), so the error type has no inference source.
                            let result: Result<WriteResult, String> = (|| {
                                let entries = entries?;
                                let minted = mint_in_pack(&mut catalog, package_id, &entries)?;
                                // Cancelled during analyze: keep the locators that were read (a
                                // partial manifest is still progress and the next run resumes from
                                // it) but do **not** roll on into the extract phase — the user
                                // asked it to stop.
                                if cancelled {
                                    return Ok(WriteResult {
                                        changed: minted,
                                        ..Default::default()
                                    });
                                }
                                let pkg = catalog
                                    .list_packages()
                                    .map_err(|e| e.to_string())?
                                    .into_iter()
                                    .find(|p| p.id == package_id)
                                    .ok_or_else(|| format!("package {package_id} not found"))?;
                                let workspace_dir = default_cache_dirs().1.ok_or_else(|| {
                                    "no workspace directory configured".to_string()
                                })?;
                                spawn_extract_worker(
                                    &pkg,
                                    &workspace_dir,
                                    &entries,
                                    &writer_tx,
                                    &jobs,
                                )?;
                                spawned = true;
                                Ok(WriteResult {
                                    changed: minted,
                                    ..Default::default()
                                })
                            })(
                            );
                            let ok = result.is_ok();
                            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
                            jobs.finish(job_id, ok, cancelled, changed);
                            if !spawned {
                                start_next_extract(
                                    &mut catalog,
                                    &mut extract_queue,
                                    &db_path,
                                    &writer_tx,
                                    &jobs,
                                    &appraise,
                                    &focus,
                                );
                            }
                            mark_committed(&changes, &catalog, before, Origin::Background);
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
                            if inbox.waiting() {
                                continue; // an op is waiting — held, and run on the next turn
                            }
                            // Watch signal (debounced upstream): rescan every watched source with
                            // the incremental crawl. The writer is idle here by construction.
                            if let Some(w) = &watch {
                                if w.rx.recv_timeout(std::time::Duration::ZERO).is_ok()
                                    && rescan_watched(&mut catalog, &db_path)
                                {
                                    changes.catalog(Origin::Background);
                                }
                            }
                            idle_tick = idle_tick.wrapping_add(1);
                            if idle_tick % IDLE_DUTY_PERIOD != 0 {
                                continue;
                            }
                            if idle_appraises {
                                // The encode goes to a worker; the writer keeps only the two
                                // cheap ends (read the queue slice here, persist the result when
                                // `EmbeddingsDone` arrives). Passing `None` for the embedders
                                // below is what keeps the pass's own embed drain out of this
                                // thread — the kick paths still run it inline, because there the
                                // user asked for it and is watching a progress bar.
                                if let Some(emb) = idle_embedder.clone() {
                                    if !embed_inflight.load(Ordering::Relaxed) {
                                        const SLICE: usize = 8;
                                        let (jobs, albedo) =
                                            lodestash_app::drains::visual_embed_batch(
                                                &catalog, SLICE,
                                            );
                                        if !jobs.is_empty() {
                                            embed_inflight.store(true, Ordering::Relaxed);
                                            let tx = writer_tx.clone();
                                            let flag = embed_inflight.clone();
                                            let (ws, cd) =
                                                (workspace_dir.clone(), cache_dir.clone());
                                            std::thread::spawn(move || {
                                                let done =
                                                    lodestash_app::drains::compute_visual_embeddings(
                                                        jobs,
                                                        &albedo,
                                                        emb.as_ref(),
                                                        ws.as_deref(),
                                                        cd.as_deref(),
                                                    );
                                                let model_id = emb.model_id().to_string();
                                                if tx
                                                    .send(WriterMsg::EmbeddingsDone {
                                                        model_id,
                                                        done,
                                                    })
                                                    .is_err()
                                                {
                                                    // The writer is gone (shutdown) — nothing to
                                                    // persist into, and nothing will clear the
                                                    // flag, so clear it here.
                                                    flag.store(false, Ordering::Relaxed);
                                                }
                                            });
                                        }
                                    }
                                }
                                // Tagging takes the same lane. The worker opens its own
                                // **read-only** catalog connection for the 3D zero-shot lookup —
                                // WAL admits any number of readers, and the single-writer rule is
                                // about writes (ADR-0002), which still all land right here.
                                if !tag_inflight.load(Ordering::Relaxed) {
                                    const TAG_SLICE: usize = 4;
                                    let jobs = lodestash_app::drains::tag_batch(
                                        &catalog, TAG_SLICE, None,
                                    );
                                    if !jobs.is_empty() {
                                        tag_inflight.store(true, Ordering::Relaxed);
                                        let tx = writer_tx.clone();
                                        let flag = tag_inflight.clone();
                                        let (ws, cd) = (workspace_dir.clone(), cache_dir.clone());
                                        let db = db_path.clone();
                                        let emb = idle_embedder.clone();
                                        std::thread::spawn(move || {
                                            let sent = tag_worker_slice(&db, jobs, emb, ws, cd, &tx);
                                            if !sent {
                                                flag.store(false, Ordering::Relaxed);
                                            }
                                        });
                                    }
                                }
                                pump.store(true, Ordering::Relaxed);
                                let r = appraise_pass(
                                    &mut catalog,
                                    None,
                                    idle_audio.clone(),
                                    std::time::Duration::from_secs(1),
                                    &appraise,
                                    &focus,
                                    // The idle pass no longer tags inline — the worker above does
                                    // it. An empty explicit set means "no assets", which is the
                                    // one thing it cannot be confused with "everything".
                                    Some(&[]),
                                );
                                pump.store(false, Ordering::Relaxed);
                                if r.map(|w| w.changed > 0).unwrap_or(false) {
                                    changes.catalog(Origin::Background);
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
                                    changes.catalog(Origin::Background);
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
        self.run_as(op, Origin::User)
    }

    /// [`Writer::run`] on someone else's behalf. The agent bridge says so, and the shell then
    /// shows a pending update rather than re-reading under the user (ADR-0016).
    pub fn run_as(&self, op: Op, origin: Origin) -> Result<WriteResult, String> {
        let (reply, wait) = mpsc::channel();
        self.tx
            .send(WriterMsg::Op(Job { op, reply, origin }))
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
fn rescan_watched(catalog: &mut Catalog, db_path: &Path) -> bool {
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
        let records = lodestash_app::persist::crawl_records(crawled.entries.iter().map(|(_, f)| f));
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
/// Compute one idle tagging slice off the writer thread and post the result back.
///
/// Returns whether a `TagsDone` was sent — the caller clears the in-flight flag when it was not,
/// or the lane would wedge shut for the life of the process.
fn tag_worker_slice(
    db_path: &std::path::Path,
    jobs: Vec<lodestash_core::catalog::VisualEmbedJob>,
    embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    workspace_dir: Option<std::path::PathBuf>,
    cache_dir: Option<std::path::PathBuf>,
    tx: &std::sync::mpsc::Sender<WriterMsg>,
) -> bool {
    // A read connection, only for the user's taught vocabulary — WAL admits any number of
    // readers, and every write still lands on the writer thread (ADR-0002).
    let Ok(cat) = Catalog::open(db_path) else {
        tracing::warn!("tag worker could not open a read connection; slice skipped");
        return false;
    };
    let teach = cat
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
    let zero_shot = embedder.and_then(|e| ImageZeroShot::new(e, &teach));
    let cfg = AppConfig::load();
    // **The idle lane never spends money.** Cloud tagging needs `drain_api_tags` (backoff, local
    // fallback, per-run cap) and that drain writes through `&mut Catalog`, so it belongs to the
    // writer's kick path — this slice runs off-writer and can only reach `compute_tags`, which
    // hands `OpenAiTagger` no image and would write empty tags. Beyond correctness it is the rule
    // AI-PIPELINE §5.1 wants anyway: a background lane must not quietly bill an API, so a cloud
    // pass is always an explicit, scoped, capped kick.
    if cfg.api_tagging_enabled && !cfg.api_key.is_empty() && cfg.api_model.is_some() {
        return false;
    }
    let tagger: Box<dyn Tagger> = if let Some(model) = cfg.tagger_model.clone() {
        let mut t = OllamaTagger::new(model);
        if let Some(vision) = cfg.tagger_vision_model.clone() {
            t = t.with_vision(vision);
        }
        Box::new(t.with_teaching(teach.clone()))
    } else {
        Box::new(FilenameTagger)
    };
    let done = lodestash_app::drains::compute_tags(
        jobs,
        tagger.as_ref(),
        zero_shot.as_ref(),
        workspace_dir.as_deref(),
        cache_dir.as_deref(),
    );
    tx.send(WriterMsg::TagsDone {
        producers: lodestash_app::drains::TagProducers {
            tagger: (tagger.producer_id(), tagger.model_id().to_string()),
            zero_shot: zero_shot
                .as_ref()
                .map(|z| (z.producer_id().to_string(), z.model_id().to_string())),
        },
        done,
    })
    .is_ok()
}

fn appraise_pass(
    catalog: &mut Catalog,
    embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    audio_embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    budget: std::time::Duration,
    progress: &AppJobProgress,
    focus: &Mutex<AppraiseFocus>,
    // The explicit set `identify_asset` was asked about, or `None` for the idle slice over
    // the whole queue. Without it the op flagged the requested assets and then appraised the
    // front of the global queue instead — 8,155 rows deep here, so the assets an agent asked
    // about came back untouched while the call reported success.
    only: Option<&[i64]>,
) -> Result<WriteResult, String> {
    // UI-SPEC §3: what the user is looking at goes first. Refreshed here rather than at each call
    // site because every pass — the idle slice, ✦ Identify all, a per-card kick — must read the
    // *current* focus: the one thing that makes the backlog follow the user is that it is re-read
    // after they have moved. A poisoned lock is not worth failing an appraisal over; the queue then
    // keeps whatever order it had, which is the pre-D3b behaviour and still correct.
    if let Ok(f) = focus.lock() {
        catalog.set_appraise_focus(f.clone());
    }
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
    // Nothing to tag → build no taggers. The block below encodes the **whole** zero-shot label
    // vocabulary (67 labels, one SigLIP text pass each) and it used to run on every pass,
    // including the once-a-second idle tick that usually has no work at all. That was free while
    // no embedder would load; the moment one did, a trivial user write — staging an asset —
    // went from 25 ms to **16.6 s**, because the single writer (ARCHITECTURE §5) was busy
    // re-encoding the same 67 labels when the op arrived. One `LIMIT 1` query is the whole fix.
    let has_tag_work = catalog
        .assets_needing_tags_within(1, only)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if has_tag_work {
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
        let zero_shot = embedder.clone().and_then(|e| ImageZeroShot::new(e, &teach));
        let audio_shot = audio_embedder.clone().and_then(AudioZeroShot::new);
        let cfg = AppConfig::load();
        let api_model = cfg
            .api_model
            .clone()
            .filter(|_| cfg.api_tagging_enabled && !cfg.api_key.is_empty());
        // The local tagger, built either way: on its own when no API model is configured, or as the
        // cloud drain's per-asset fallback (AI-PIPELINE §5.1 "Local fallback on failure").
        let local: Box<dyn Tagger> = if let Some(model) = cfg.tagger_model.clone() {
            let mut t = OllamaTagger::new(model);
            if let Some(vision) = cfg.tagger_vision_model.clone() {
                t = t.with_vision(vision);
            }
            Box::new(t.with_teaching(teach.clone()))
        } else {
            Box::new(FilenameTagger)
        };
        match api_model {
            // Cloud tagging goes through `drain_api_tags`, **not** the generic `drain_tags`.
            // `drain_tags` → `compute_tags` passes `image: None` (the two-phase shape expects
            // `caption()` to carry the picture) and `OpenAiTagger` has no `caption()` override, so
            // that path made no HTTP request at all and wrote an empty tag set that cleared
            // `tags_dirty` — assets marked done, never seen by any model. `drain_api_tags` is the
            // drain written for this: it passes the image, honours `Retry-After` with backoff, falls
            // back to the local model on a hard error, and caps the run.
            Some(model) => {
                let api = OpenAiTagger::new(cfg.api_base_url.clone(), cfg.api_key.clone(), model)
                    .with_teaching(teach.clone());
                lodestash_app::drains::drain_api_tags(
                    catalog,
                    &api,
                    Some(local.as_ref()),
                    zero_shot.as_ref(),
                    cfg.api_max_per_run,
                    workspace_dir.as_deref(),
                    cache_dir.as_deref(),
                    progress,
                    only,
                );
            }
            None => drain_tags(
                catalog,
                local.as_ref(),
                zero_shot.as_ref(),
                workspace_dir.as_deref(),
                cache_dir.as_deref(),
                progress,
                TAG_BUDGET_PER_PASS,
                only,
            ),
        }
        if let Some(az) = audio_shot.as_ref() {
            drain_audio_tags(
                catalog,
                az,
                workspace_dir.as_deref(),
                progress,
                AUDIO_TAG_BUDGET_PER_PASS,
                // Same scope the visual drain gets. A kick on 64 images used to drag 48 unrelated
                // audio clips through CLAP: the visual half took 328 ms and the slice took 20 min.
                only,
                Some(budget),
            );
        }
    }
    // Promotion and the vector/preview backfill still run every pass — they are the cheap,
    // budgeted half, and promotion is what a pass exists to do.
    let (counts, _assets, promoted) = drain_promote_resync(
        catalog,
        embedder,
        audio_embedder,
        workspace_dir.as_deref(),
        cache_dir.as_deref(),
        proxy,
        &roots,
        Some(budget),
        only,
    );
    // changed = the appraised backlog size the pass reconciled; when counts are None
    // (nothing moved) report 1 so the frontend toasts a real kick, not a silent no-op.
    Ok(WriteResult {
        changed: counts.map(|c| c.identified + c.unidentified).unwrap_or(1),
        promoted,
        ..Default::default()
    })
}

/// Start the next queued `ExtractPackage`, **skipping any that fail before spawning a worker**.
///
/// The queue is drained by whichever worker reports back (`ExtractDone` / a failed `ManifestDone`),
/// so exactly one thing must be true after this returns: either a worker is running, or the queue
/// is empty. An op that returns `Ok` spawned a worker and will report back. One that returns `Err`
/// — package row gone, no workspace configured, a pack with no extractable payloads — spawned
/// **nothing**, so it can never drain the queue itself.
///
/// Popping only once was a silent, session-long death: every pack behind the failed one sat in a
/// queue nobody would ever drain again, `extract_queue.is_empty()` stayed false from then on, and
/// every later extract request took the "join the queue" branch — no job in the tray, no error.
///
/// The caller was already told "accepted" when the op was queued, so a failure here has no channel
/// to travel back on. It is recorded as a **failed job** instead: the tray is where this feature
/// reports, and a pack that silently never unpacks is the exact failure this whole path exists to
/// stop being invisible.
fn start_next_extract(
    catalog: &mut Catalog,
    queue: &mut VecDeque<Job>,
    db_path: &Path,
    tx: &Sender<WriterMsg>,
    jobs: &JobRegistry,
    progress: &AppJobProgress,
    focus: &Mutex<AppraiseFocus>,
) {
    // Each failure is recorded as a failed job, and the registry publishes that itself.
    drain_to_first_spawn(queue, jobs, |op| {
        run(catalog, db_path, op, tx, jobs, progress, focus).map(|_| ())
    });
}

/// The queue invariant itself, with the writer's dependencies handed in.
///
/// Split out to be **testable**. Any test that reaches [`run`] links Tauri's window layer
/// (`comctl32`/`user32`/`gdi32`/`dwmapi`), and a test binary carries no application manifest — so
/// it binds to comctl32 v5, misses the v6 exports Tauri imports, and dies at load with
/// `STATUS_ENTRYPOINT_NOT_FOUND` before a single test runs. That is why this file had no tests at
/// all, over the only genuinely concurrent code in the repo. Taking `start` as a closure puts the
/// rule back within reach of one.
///
/// Returns how many ops failed to spawn — each is recorded as a failed job, because the caller was
/// already told "accepted" when it queued and has no other way to learn.
fn drain_to_first_spawn(
    queue: &mut VecDeque<Job>,
    jobs: &JobRegistry,
    mut start: impl FnMut(Op) -> Result<(), String>,
) -> usize {
    let mut failures = 0;
    while let Some(next) = queue.pop_front() {
        match start(next.op) {
            Ok(()) => return failures, // a worker is running; its report drains the rest
            Err(e) => {
                failures += 1;
                let h = jobs.start(format!("Extract failed — {e}"), "extract-failed", false);
                jobs.finish(h.id, false, false, 0);
            }
        }
    }
    failures
}

fn run(
    catalog: &mut Catalog,
    db_path: &Path,
    op: Op,
    tx: &Sender<WriterMsg>,
    jobs: &JobRegistry,
    progress: &AppJobProgress,
    focus: &Mutex<AppraiseFocus>,
) -> Result<WriteResult, String> {
    match op {
        Op::SetFavorite { ids, on } => catalog
            .set_favorite(&ids, on)
            .map(|changed| WriteResult {
                changed,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // Per asset, because `set_confirmed`'s un-confirm branch recomputes each row's earned
        // status from its own embedding/tag/metrics state — there is no set-wide answer to fold.
        Op::SetConfirmed { ids, on } => {
            let mut changed = 0usize;
            for id in ids {
                catalog.set_confirmed(id, on).map_err(|e| e.to_string())?;
                changed += 1;
            }
            Ok(WriteResult {
                changed,
                ..Default::default()
            })
        }

        Op::SetLinkIgnored { edge_id, ignored } => {
            let r = if ignored {
                catalog.ignore_relationship(edge_id)
            } else {
                catalog.restore_relationship(edge_id)
            };
            r.map(|did| WriteResult {
                changed: did as usize,
                ..Default::default()
            })
            .map_err(|e| e.to_string())
        }

        Op::SetPackKeepCached { package_id, keep } => catalog
            .set_package_keep_cached(package_id, keep)
            .map(|()| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::RetargetPackage {
            package_id,
            new_path,
        } => catalog
            .retarget_package(package_id, &new_path)
            .map(|()| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // `changed` is how many sources *moved* state, so a caller can tell "nothing to see" from
        // "a drive just came back" without diffing the whole list.
        Op::RefreshSourceHealth => catalog
            .refresh_source_health()
            .map(|moved| WriteResult {
                changed: moved.len(),
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // `changed` is the number of packs re-anchored with the root. The asset count the panel
        // promised comes from `relink_preview`, which ran before the user committed.
        // Files first, record second. A record dropped before its files would leave copies on
        // disk that nothing can name again — the same ordering `undo_import_batch` uses.
        Op::UndoExportBatch { batch_id } => undo_export_batch(catalog, &batch_id),

        Op::RelinkSource {
            source_id,
            new_root,
        } => catalog
            .relink_source(source_id, &new_root)
            .map(|moved| WriteResult {
                changed: moved,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // Undo (UI-SPEC §2 / DESIGN-GUIDELINES §7 L5): log the trash toggle so Ctrl/Cmd-Z can
        // flip it back. Only a toggle that actually changed rows is worth undoing — logging a
        // no-op would let a revert flip assets that were never touched.
        Op::SetTrashed { ids, trashed } => {
            let changed = catalog
                .set_trashed(&ids, trashed)
                .map_err(|e| e.to_string())?;
            if changed > 0 {
                catalog
                    .record_trash_op(&ids, trashed)
                    .map_err(|e| e.to_string())?;
            }
            Ok(WriteResult {
                changed,
                ..Default::default()
            })
        }

        Op::RemoveFromLibrary { targets, project } => {
            remove_from_library(catalog, targets, project)
        }

        Op::DeleteFromDisk {
            targets,
            roots,
            workspace,
        } => delete_from_disk(catalog, targets, &roots, workspace.as_deref()),

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
                    harvest_pack_metadata(
                        catalog,
                        id,
                        &pack.abs_path,
                        &pack.display_name,
                        None,
                        &pack.entries,
                    );
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

        // Import-packages dialog (FEATURES §5.2): the discovery scan already read each
        // archive's manifest, so this records + mints from those locators — no second inflate.
        // One `package` row per selected pack, pinned and in-pack minted: the Library and its
        // cards exist immediately; identify runs off the drains in the background.
        Op::ImportPacks { root, packs } => {
            let label = format!("Import {} packs", packs.len());
            let job = jobs.start(label, "import", false);
            let result = (|| {
                let name = lodestash_app::config::stem_name(std::path::Path::new(&root));
                let source_id = catalog
                    .upsert_import_source(&name, &root, "folder")
                    .map_err(|e| e.to_string())?;
                let mut inserted = 0usize;
                for pack in packs {
                    let id = catalog
                        .upsert_package(
                            source_id,
                            &pack.abs_path,
                            &name,
                            pack.publisher.as_deref(),
                            pack.category.as_deref(),
                        )
                        .map_err(|e| e.to_string())?;
                    catalog
                        .record_package_entries(id, &pack.entries)
                        .map_err(|e| e.to_string())?;
                    harvest_pack_metadata(
                        catalog,
                        id,
                        &pack.abs_path,
                        &pack.name,
                        pack.publisher.as_deref(),
                        &pack.entries,
                    );
                    inserted += mint_in_pack(catalog, id, &pack.entries)?;
                    catalog
                        .set_package_pinned(id, true)
                        .map_err(|e| e.to_string())?;
                }
                Ok(WriteResult {
                    changed: inserted,
                    ..Default::default()
                })
            })();
            let ok = result.is_ok();
            let inserted = result.as_ref().map(|r| r.changed).unwrap_or(0);
            jobs.finish(job.id, ok, false, inserted);
            result
        }

        Op::UpsertTrait {
            namespace,
            value,
            scope,
            usage_note,
            teach,
        } => catalog
            .upsert_trait(
                &namespace,
                &value,
                scope.as_deref(),
                usage_note.as_deref(),
                teach,
            )
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::UnpromoteTrait { tag_id } => catalog
            .unpromote_trait(tag_id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::SetTraitTeach { tag_id, teach } => catalog
            .set_trait_teach(tag_id, teach)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::SetTraitStyle {
            tag_id,
            color,
            icon,
        } => catalog
            .set_trait_style(tag_id, color.as_deref(), icon.as_deref())
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::SetSourceWatch { id, watch } => catalog
            .set_source_watch(id, watch)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // Log the create (id + name) after the row exists so undo can delete exactly the
        // collection this op created (UI-SPEC §2 / DESIGN-GUIDELINES §7 L5).
        Op::CreateCollection { name } => {
            let id = catalog
                .create_collection(&name)
                .map_err(|e| e.to_string())?;
            catalog
                .record_collection_create_op(id, &name)
                .map_err(|e| e.to_string())?;
            Ok(WriteResult {
                changed: 1,
                ..Default::default()
            })
        }

        Op::DeleteCollection { id } => catalog
            .delete_collection(id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::SetPackagePinned { id, pinned } => catalog
            .set_package_pinned(id, pinned)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::AddSourceExclude {
            source_id,
            kind,
            value,
        } => {
            catalog
                .add_source_exclude(source_id, &kind, &value)
                .map_err(|e| e.to_string())?;
            sync_source_exclude_to_config(catalog, source_id, &kind, &value, true)?;
            Ok(WriteResult {
                changed: 1,
                ..Default::default()
            })
        }

        Op::RemoveSourceExclude {
            source_id,
            kind,
            value,
        } => {
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
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::AddRule {
            taxonomy_id,
            category_id,
            match_type,
            pattern,
            priority,
        } => catalog
            .add_rule(taxonomy_id, category_id, &match_type, &pattern, priority)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::DeleteRule { rule_id } => catalog
            .delete_rule(rule_id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::DeleteTaxonomy { id } => catalog
            .delete_taxonomy(id)
            .map(|removed| WriteResult {
                changed: removed,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::AddCategory {
            taxonomy_id,
            parent_id,
            pref_label,
            path,
        } => catalog
            .add_category(taxonomy_id, parent_id, &pref_label, &path)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::ApplyMapping { taxonomy_id } => catalog
            .apply_mapping(taxonomy_id)
            .map(|changed| WriteResult {
                changed,
                ..Default::default()
            })
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
                focus,
                None,
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
            catalog
                .mark_for_identify(asset_id)
                .map_err(|e| e.to_string())?;
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
                focus,
                None,
            );
            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
            jobs.finish(job.id, result.is_ok(), false, changed);
            result
        }

        Op::IdentifyIds { asset_ids } => {
            // ENGINE-API §2 `identify_asset`: flag an explicit set, then ONE bounded pass so
            // a bulk agent call is a single reconcile, not N kicks.
            for id in &asset_ids {
                catalog.mark_for_identify(*id).map_err(|e| e.to_string())?;
            }
            let job = jobs.start(
                format!("Identify {} assets", asset_ids.len()),
                "appraise",
                false,
            );
            let embedder = resolve_embedder();
            let audio_embedder = resolve_audio_embedder();
            if embedder.is_none() && audio_embedder.is_none() {
                tracing::warn!(
                    "Identify {} assets: no local embedder models — flagged for a later pass",
                    asset_ids.len()
                );
            }
            let result = appraise_pass(
                catalog,
                embedder,
                audio_embedder,
                std::time::Duration::from_secs(20),
                progress,
                focus,
                Some(&asset_ids),
            );
            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
            jobs.finish(job.id, result.is_ok(), false, changed);
            result
        }

        // Staging no longer refuses without a registered editor. It used to answer *"no active
        // project registered — register one via the Connect wizard"*, which was a reasonable rule
        // for an agent and the wrong one for a person: the shell is where picks are collected, the
        // Unity plugin does not exist yet, and so that error was the answer for **every** user.
        // The shell dodged it by keeping its own in-memory list. One tray, created on demand, is
        // what removes the reason for the second one (M20, ADR-0010).
        Op::StageAssets { asset_ids } => {
            let tray = catalog
                .ensure_active_staging_tray()
                .map_err(|e| e.to_string())?;
            let mut changed = 0;
            for id in asset_ids {
                catalog
                    .collection_add_asset(tray, id)
                    .map_err(|e| e.to_string())?;
                changed += 1;
            }
            Ok(WriteResult {
                changed,
                ..Default::default()
            })
        }

        Op::UnstageAssets { asset_ids } => {
            // No tray means nothing was staged, so there is nothing to remove — and creating one
            // in order to remove from it would be an odd way to spend a write.
            let Some(tray) = catalog
                .active_staging_tray_id()
                .map_err(|e| e.to_string())?
            else {
                return Ok(WriteResult::default());
            };
            let mut changed = 0;
            for id in asset_ids {
                catalog
                    .collection_remove_asset(tray, id)
                    .map_err(|e| e.to_string())?;
                changed += 1;
            }
            Ok(WriteResult {
                changed,
                ..Default::default()
            })
        }

        Op::ClearStaged => catalog
            .clear_staging_tray()
            .map(|changed| WriteResult {
                changed,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::ExtractPackage { package_id } => {
            // Per-pack Extract (M14 surgical flow, INGESTION §2): if the manifest locators
            // aren't recorded yet (no browse persistence for this pack), a manifest worker
            // reads the archive once (progress + cancel through the tray) and the writer
            // records the locators + **mints the in-pack assets** (surgical indexing) when it
            // reports back. The extract worker then pulls only the wanted payloads with one
            // gzip skip-scan (`extract_at_offsets`) and the writer **materializes** them
            // (fills `workspace_rel`) — the rows hydrate from the workspace and render from
            // cache.
            //
            // **This command answers as soon as a worker is spawned, not when the pack is
            // unpacked.** `Writer::run` does a single `wait.recv()` and drops the receiver, so the
            // `reply.send` the workers make from `ExtractDone`/`ManifestDone` always lands on a
            // dead channel and is discarded. Outcome and progress reach the user through the **job
            // tray**, which is the right home for a multi-GB unpack — but the frontend's per-row
            // spinner clears early because of it, and the reply plumbing through the workers is
            // dead weight. Tracked on the board (extract lifecycle); do not read the worker's
            // `reply` argument as a promise to the caller.
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
                let wcancel = handles.cancel;
                let wprogress = handles.progress.expect("manifest jobs carry progress");
                let wpath = PathBuf::from(&pkg.package_path);
                let wjobs = jobs.clone();
                std::thread::Builder::new()
                    .name("manifest-worker".into())
                    .spawn(move || {
                        // Same guarantee as the extract worker: this send is the only thing that
                        // finishes the job and lets the queue move, so a panic must not eat it.
                        let work =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                                let report_cancel = wcancel.clone();
                                let report = move |done: u64, total: u64| {
                                    wprogress.done.store(done, Ordering::Relaxed);
                                    wprogress.total.store(total, Ordering::Relaxed);
                                    wjobs.progressed();
                                    !report_cancel.load(Ordering::Relaxed)
                                };
                                let read = read_manifest_with_progress(&wpath, &report)
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
                                    });
                                let cancelled = wcancel.load(Ordering::Relaxed);
                                // `read_manifest_with_progress` reports a cancel as an **error**, not as a
                                // partial `Ok` (`read_manifest_with_progress_reports_and_cancels` asserts
                                // exactly that) — so a `!cancelled` guard on the empty case alone was dead
                                // code, and pressing Cancel during Analyze still surfaced a raw
                                // "extraction cancelled" string as a failed job. Swallow the error here:
                                // cancelling is a decision, not a fault.
                                let outcome = if cancelled {
                                    Ok(read.unwrap_or_default())
                                } else {
                                    read.and_then(|entries| {
                                        if entries.is_empty() {
                                            Err("the pack has no manifest entries".to_string())
                                        } else {
                                            Ok(entries)
                                        }
                                    })
                                };
                                (outcome, cancelled)
                            }));
                        let (outcome, cancelled) =
                            work.unwrap_or_else(|_| {
                                (
                                Err("the manifest worker panicked — the archive is likely corrupt"
                                    .to_string()),
                                false,
                            )
                            });
                        let _ = wtx.send(WriterMsg::ManifestDone {
                            job_id: handles.id,
                            package_id,
                            entries: outcome,
                            cancelled,
                        });
                    })
                    .expect("manifest worker spawns");
            } else {
                // Locators already recorded (browse persistence) — mint + extract right away.
                mint_in_pack(catalog, package_id, &entries)?;
                spawn_extract_worker(&pkg, &workspace_dir, &entries, tx, jobs)?;
            }
            Ok(WriteResult {
                changed: 0, // queued — the tray reports the real changed count
                ..Default::default()
            })
        }
        Op::RenameCategory { id, label } => catalog
            .rename_category(id, &label)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::SetActiveProject { project_id } => catalog
            .set_active_project(project_id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // Batch undo opens its own connection on this thread (the same pattern as
        // persist_to_catalog): the single-writer rule is about which *thread* writes, and this
        // is still the writer — no other job can interleave with the delete.
        Op::UndoImportBatch { batch_id } => undo_import_batch(db_path, &batch_id),

        Op::DeleteCategory { id } => catalog
            .delete_category(id)
            .map(|removed| WriteResult {
                changed: removed,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // Undo: the writer thread is the one place a revert may run — it is a catalog mutation
        // and every catalog mutation belongs here, one order (ARCHITECTURE §5).
        Op::RevertOp { op_id } => catalog
            .revert_op(op_id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::RedoOp { op_id } => catalog
            .redo_op(op_id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::CollectionAddAsset {
            collection_id,
            asset_id,
        } => catalog
            .collection_add_asset(collection_id, asset_id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::CollectionRemoveAsset {
            collection_id,
            asset_id,
        } => catalog
            .collection_remove_asset(collection_id, asset_id)
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // Bulk-bar tag edit (KANBAN B9d). Tag writes replace per **source** (`set_tags`
        // re-writes everything that source contributed), so this adds/removes the tag under
        // the `manual` source — the established source for user-set tags (set_tags docs). Each
        // asset's prior `manual` set is snapshotted into its own undo-log entry first, which
        // is exactly the payload `revert_set_tags` restores on undo.
        Op::BulkTag {
            asset_ids,
            namespace,
            value,
            on,
        } => {
            // A person's edit (v1 plan S04). An add appends to their own tags; a removal is a
            // rejection of the term across every source — the bulk bar's and the inspector's
            // "remove" mean "this is wrong" — and its undo restores each source's row exactly.
            let tag = lodestash_core::catalog::TagAssignment {
                namespace,
                value,
                confidence: None,
            };
            let mode = if on {
                TagMode::Append
            } else {
                TagMode::RejectAllSources
            };
            let items = asset_ids
                .into_iter()
                .map(|asset_id| TagBatchItem {
                    asset_id,
                    source: "manual".to_string(),
                    model_id: None,
                    mode,
                    tags: vec![tag.clone()],
                    description: None,
                    expected_revision: None,
                })
                .collect();
            catalog
                .apply_tag_batch(&TagBatch {
                    items,
                    completes_queue: false,
                    history: Some(HistoryOrigin::User),
                    receipt: None,
                })
                .map(|out| WriteResult {
                    changed: out.changed_assets.len(),
                    ..Default::default()
                })
                .map_err(|e| e.to_string())
        }
        Op::TagBatch { batch } => catalog
            .apply_tag_batch(&batch)
            .map(|out| WriteResult {
                changed: out.changed_assets.len(),
                ..Default::default()
            })
            .map_err(|e| e.to_string()),
    }
}

/// Undo one import batch (UI-SPEC §16.1): delete the batch's copied files from the project,
/// then its import_record rows — catalog assets are never touched. Runs on the writer thread
/// over its own short-lived connection, so the single-writer rule holds (same pattern as
/// persist_to_catalog); the connection does not run migrations (the writer's own catalog already
/// did at spawn).
/// Delete what one export run wrote, then forget the run (FEATURES §7.1).
///
/// Every delete here is a file **this app created**, in a folder the user pointed it at, named in
/// the record the run wrote. That is a narrower promise than it sounds, so it is enforced rather
/// than assumed — four fences, each for a way this could reach a file it has no business touching:
///
/// - `file_name` must be a **bare name**: no separators, no `..`, no absolute path. A record with a
///   path in it could otherwise walk out of the destination folder.
/// - the resolved file must sit **directly inside** the recorded `dest_path`.
/// - the destination must still pass the export sandbox — re-checked **now**, not trusted from
///   export time, because a library source may have been added over that folder since.
/// - only files, never directories, and a missing file counts as already-undone rather than failed.
///
/// The originals are never in scope: an export copies, so undo deletes copies. The library is not
/// touched, and no asset row changes — the non-destructive invariant covers this the same way it
/// covers everything else.
fn undo_export_batch(catalog: &mut Catalog, batch_id: &str) -> Result<WriteResult, String> {
    let files = catalog
        .export_batch_files(batch_id)
        .map_err(|e| e.to_string())?;
    if files.is_empty() {
        return Err("that export is no longer in the history".into());
    }
    let roots = catalog.source_roots().unwrap_or_default();

    let mut removed = 0usize;
    let mut refused = 0usize;
    let mut failed = 0usize;
    for (dest_path, file_name) in &files {
        let bare = Path::new(file_name);
        let named_once = bare.components().count() == 1
            && !file_name.contains('/')
            && !file_name.contains('\\')
            && bare
                .file_name()
                .map(|f| f == bare.as_os_str())
                .unwrap_or(false);
        if !named_once {
            refused += 1;
            continue;
        }
        // The sandbox as it stands *now*: a folder that was legal to export into may since have
        // been added as a library source, and deleting inside one is what the invariant forbids.
        let Ok(dest) = lodestash_api::export::check_dest(dest_path, &roots) else {
            refused += 1;
            continue;
        };
        let abs = dest.join(bare);
        if !abs.starts_with(&dest) {
            refused += 1;
            continue;
        }
        match std::fs::metadata(&abs) {
            Ok(m) if m.is_file() => match std::fs::remove_file(&abs) {
                Ok(()) => removed += 1,
                Err(_) => failed += 1,
            },
            Ok(_) => refused += 1,
            // Already gone is the outcome undo wanted, not an error.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => removed += 1,
            Err(_) => failed += 1,
        }
    }

    // Forgotten only once the files are dealt with. A run left half-deleted keeps its record, so
    // the user can see it and try again rather than losing track of the copies still out there.
    if failed == 0 {
        catalog
            .forget_export_batch(batch_id)
            .map_err(|e| e.to_string())?;
    }
    Ok(WriteResult {
        changed: removed,
        failed,
        refused,
        ..Default::default()
    })
}

fn undo_import_batch(db_path: &Path, batch_id: &str) -> Result<WriteResult, String> {
    let conn = Connection::open(db_path).map_err(|e| format!("catalog unavailable: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT p.project_path, ir.dest_path
               FROM import_record ir
               JOIN project p ON p.id = ir.project_id
              WHERE ir.batch_id = ?1",
        )
        .map_err(|e| e.to_string())?;
    let files: Vec<(String, String)> = stmt
        .query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|e| e.to_string())?;

    // Delete the copied files first, while the paths are still known — the records go only
    // after the files, so a failed delete never strands a record pointing at nothing. A missing
    // file counts as refused (already gone), an error as failed.
    let mut failed = 0usize;
    let mut refused = 0usize;
    for (project_path, dest_path) in &files {
        let dest = Path::new(dest_path);
        // Fence the one destructive path (HANDOFF §6): `dest_path` is project-relative, so a
        // non-relative path or a `..` component must never resolve outside the project root.
        if dest.is_absolute()
            || dest
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            refused += 1;
            continue;
        }
        let abs = Path::new(project_path).join(dest);
        match std::fs::remove_file(&abs) {
            Ok(()) => {
                // Unity's import sidecar, if it exists — leaving it would half-undo the import.
                let _ = std::fs::remove_file(PathBuf::from(format!("{}.meta", abs.display())));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => refused += 1,
            Err(_) => failed += 1,
        }
    }

    let changed = conn
        .execute("DELETE FROM import_record WHERE batch_id = ?1", [batch_id])
        .map_err(|e| e.to_string())?;
    Ok(WriteResult {
        changed,
        failed,
        refused,
        ..Default::default()
    })
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
    jobs: &JobRegistry,
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
    let wcancel = handles.cancel;
    let wprogress = handles.progress.expect("extract jobs carry progress");
    let wpath = PathBuf::from(&pkg.package_path);
    let wworkspace = workspace_dir.to_path_buf();
    let wjobs = jobs.clone();
    let wpkg_id = pkg.id;
    std::thread::Builder::new()
        .name("extract-worker".into())
        .spawn(move || {
            // The send below is the **only** thing that runs `jobs.finish` and drains the queue.
            // A panic in the gzip/tar path (corrupt archive) would unwind a detached thread, the
            // send would never happen, `has_active_extract()` would stay true forever, and every
            // later extract would join a queue nobody drains — the same session-long jam
            // `start_next_extract` fixes for the error path, by a different road.
            let work = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let report_cancel = wcancel.clone();
                let report = move |done: u64, total: u64| {
                    wprogress.done.store(done, Ordering::Relaxed);
                    wprogress.total.store(total, Ordering::Relaxed);
                    wjobs.progressed();
                    !report_cancel.load(Ordering::Relaxed)
                };
                let placed = extract_at_offsets_with_progress(&wpath, &wworkspace, wanted, &report)
                    .map_err(|e| e.to_string());
                let cancelled = wcancel.load(Ordering::Relaxed);
                // "Extracted nothing" is a failure — unless the user cancelled before the first
                // payload landed, which is a cancel, not a fault, and must not surface as an error
                // toast for pressing Cancel.
                let outcome = placed.and_then(|a| {
                    if a.is_empty() && !cancelled {
                        Err("the pack extracted no assets".to_string())
                    } else {
                        Ok(a)
                    }
                });
                (outcome, cancelled)
            }));
            let (outcome, cancelled) = work.unwrap_or_else(|_| {
                (
                    Err("the extract worker panicked — the archive is likely corrupt".to_string()),
                    false,
                )
            });
            let _ = wtx.send(WriterMsg::ExtractDone {
                job_id: handles.id,
                package_id: wpkg_id,
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
    cancelled: bool,
) -> Result<WriteResult, String> {
    let extracted = outcome?;
    // `workspace_rel` is **relative to the workspace dir** — that is what the column's own doc
    // promises and what `crawl_records_in_workspace` writes. This path stored the extractor's
    // absolute `workspace_path` verbatim, so the column held two different formats depending on
    // which pass wrote the row. It survived only because `Path::join` with an absolute path
    // *replaces* on Windows, making the wrong form accidentally resolve — until the cache dir
    // moves, at which point every extracted row points at a directory that no longer exists.
    let workspace = default_cache_dirs().1;
    let pairs: Vec<(String, String)> = extracted
        .iter()
        .filter_map(|a| {
            let rel = workspace
                .as_deref()
                .and_then(|ws| a.workspace_path.strip_prefix(ws).ok())
                .unwrap_or(a.workspace_path.as_path());
            a.pathname
                .clone()
                .map(|p| (p, rel.to_string_lossy().replace('\\', "/")))
        })
        .collect();
    let matched = catalog
        .materialize_in_pack_assets(package_id, &pairs)
        .map_err(|e| e.to_string())?;
    let changed = matched.len();
    // Only a *finished* extraction may stamp the pack unpacked (HANDOFF §6): a cancelled one
    // still materializes its valid, resumable files, but `changed` is a truncated count and
    // recording it as the whole pack would make the catalog claim a full extraction.
    if !cancelled {
        catalog
            .mark_package_unpacked(package_id, changed)
            .map_err(|e| e.to_string())?;
    }
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
    Ok(WriteResult {
        changed,
        ..Default::default()
    })
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
    /// The backfill exists because the harvest runs at import: 87 packs in the reference library
    /// were indexed before it and would have stayed pipeline-less for good, so the §5.4 advisory
    /// could never fire for the library the user already has.
    #[test]
    fn existing_packs_get_their_pipeline_backfilled() {
        let mut c = Catalog::open_in_memory().expect("open");
        let source_id = c.upsert_import_source("s", "/s", "folder").expect("source");
        let named = c
            .upsert_package(
                source_id,
                "/s/altos.unitypackage",
                "Altos ... for Unity URP",
                None,
                None,
            )
            .expect("pkg");
        let by_entries = c
            .upsert_package(
                source_id,
                "/s/artificer.unitypackage",
                "Artificer",
                None,
                None,
            )
            .expect("pkg");
        let quiet = c
            .upsert_package(source_id, "/s/audio.unitypackage", "Sound Pack", None, None)
            .expect("pkg");
        let settled = c
            .upsert_package(
                source_id,
                "/s/settled.unitypackage",
                "Already Known URP",
                None,
                None,
            )
            .expect("pkg");
        c.set_package_pipeline(settled, Some("hdrp")).expect("set");
        let entry = |p: &str| PackageEntryRecord {
            guid: format!("g{p}"),
            pathname: Some(p.to_string()),
            tar_offset: Some(0),
            size_bytes: Some(16),
        };
        c.record_package_entries(
            by_entries,
            &[
                entry("Assets/Artificer/Other Bits/Artificer Double Sided HDRP.unitypackage"),
                entry("Assets/Artificer/Other Bits/Artificer Builtin Double Sided.unitypackage"),
            ],
        )
        .expect("entries");
        c.record_package_entries(quiet, &[entry("Assets/Sound Pack/Audio/Burp.wav")])
            .expect("entries");

        assert_eq!(
            backfill_package_pipelines(&c),
            2,
            "two packs name a pipeline"
        );
        let of = |id: i64| {
            c.list_packages()
                .unwrap()
                .into_iter()
                .find(|p| p.id == id)
                .unwrap()
                .pipeline
        };
        assert_eq!(
            of(named).as_deref(),
            Some("urp"),
            "the pack name alone names it"
        );
        assert_eq!(
            of(by_entries).as_deref(),
            Some("mixed"),
            "an HDRP variant beside a Built-in one is mixed"
        );
        assert_eq!(of(quiet), None, "a pack that names none stays unknown");
        assert_eq!(
            of(settled).as_deref(),
            Some("hdrp"),
            "a pack already classified is not revisited, even when its name says otherwise"
        );

        assert_eq!(
            backfill_package_pipelines(&c),
            0,
            "a second run has nothing left to classify"
        );
    }

    /// The desktop write path records what a pack says about itself. Before this, every pack the
    /// Tauri app imported recorded no licence and no pipeline at all: `read_package_meta` ran at
    /// discovery and everything but `version` was thrown away, and `set_package_metadata` was
    /// never called from this crate.
    ///
    /// Also pins the cost fence - a pack whose manifest declares no licence or UPM manifest must
    /// not reopen the archive, because `read_package_meta` streams until it finds candidates and
    /// would pay a full inflate to learn nothing. The archive path here does not exist, so a read
    /// attempt would surface as a missing licence rather than a hang.
    #[test]
    fn pack_metadata_is_harvested_on_import() {
        let c = Catalog::open_in_memory().expect("open");
        let source_id = c.upsert_import_source("s", "/s", "folder").expect("source");
        let entry = |p: &str| PackageEntryRecord {
            guid: format!("g{p}"),
            pathname: Some(p.to_string()),
            tar_offset: Some(0),
            size_bytes: Some(16),
        };

        // A store-cache pack (it has a publisher) shipping a URP variant and a Built-in one.
        let id = c
            .upsert_package(
                source_id,
                "/s/variants.unitypackage",
                "Variants",
                Some("SICS Games"),
                None,
            )
            .expect("pkg");
        harvest_pack_metadata(
            &c,
            id,
            "/s/does-not-exist.unitypackage",
            "Variants",
            Some("SICS Games"),
            &[
                entry("Assets/Variants/Upgrade/URP_10+.unitypackage"),
                entry("Assets/Variants/Upgrade/Built_in.unitypackage"),
                entry("Assets/Variants/Models/Rock.fbx"),
            ],
        );

        // A folder pack with no publisher that names one pipeline and nothing else.
        let urp = c
            .upsert_package(source_id, "/s/urp.unitypackage", "ToonFX URP", None, None)
            .expect("pkg");
        harvest_pack_metadata(
            &c,
            urp,
            "/s/does-not-exist.unitypackage",
            "ToonFX URP",
            None,
            &[entry("Assets/ToonFX/Shaders/Toon.shadergraph")],
        );

        // A pack that names no pipeline at all.
        let quiet = c
            .upsert_package(source_id, "/s/quiet.unitypackage", "Sound Pack", None, None)
            .expect("pkg");
        harvest_pack_metadata(
            &c,
            quiet,
            "/s/does-not-exist.unitypackage",
            "Sound Pack",
            None,
            &[entry("Assets/Sound Pack/Audio/Burp.wav")],
        );

        let by_name = |n: &str| {
            c.list_packages()
                .unwrap()
                .into_iter()
                .find(|p| p.name == n)
                .unwrap()
        };
        assert_eq!(
            by_name("Variants").license.as_deref(),
            Some("Unity Asset Store EULA"),
            "a pack with a publisher came from the store cache, which licenses it"
        );
        assert_eq!(
            by_name("Variants").pipeline.as_deref(),
            Some("mixed"),
            "a variant per pipeline is exactly the mixed case"
        );
        assert_eq!(
            by_name("ToonFX URP").pipeline.as_deref(),
            Some("urp"),
            "the pack name alone names it"
        );
        assert_eq!(
            by_name("ToonFX URP").license,
            None,
            "no publisher means nothing is known about the licence - not a default"
        );
        assert_eq!(
            by_name("Sound Pack").pipeline,
            None,
            "a pack that names no pipeline stays unknown, so no mismatch is ever advised for it"
        );
    }

    /// Undoing an export (FEATURES §7.1) deletes files the user chose the location of, which makes
    /// it the most dangerous delete in the app. Mutation-checked on the four fences that keep it
    /// pointed at its own copies:
    ///
    /// a record whose `file_name` carries a path must be **refused**, not followed out of the
    /// destination; a destination that is now inside a library source root must be refused even
    /// though it was legal when the export ran (the sandbox is re-checked, not trusted); a file
    /// that is already gone counts as undone rather than failed; and the record survives a run that
    /// could not finish, so copies still on disk stay findable.
    #[test]
    fn undoing_an_export_deletes_its_own_copies_and_refuses_anything_else() {
        let dir = tempfile::tempdir().expect("tmp");
        let out = dir.path().join("out");
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&out).expect("mkdir out");
        std::fs::create_dir_all(&lib).expect("mkdir lib");
        std::fs::write(out.join("rock.fbx"), b"copy").expect("w");
        std::fs::write(out.join("gone.fbx"), b"copy").expect("w");
        // The file a traversal would reach if `file_name` were followed as a path.
        std::fs::write(dir.path().join("outside.txt"), b"do not touch").expect("w");
        std::fs::remove_file(out.join("gone.fbx")).expect("rm");

        let mut c = Catalog::open_in_memory().expect("open");
        let out_s = out.to_string_lossy().to_string();
        c.record_exports(
            "batch-1",
            &out_s,
            &[
                (None, "rock.fbx".to_string()),
                (None, "gone.fbx".to_string()),
                // A record that tries to climb out. Nothing writes one — this is the fence's job.
                (None, "../outside.txt".to_string()),
            ],
        )
        .expect("record");

        let r = undo_export_batch(&mut c, "batch-1").expect("undo");
        assert_eq!(
            r.changed, 2,
            "the real copy and the already-missing one both count as undone"
        );
        assert_eq!(
            r.refused, 1,
            "the traversing record was followed instead of refused"
        );
        assert_eq!(r.failed, 0);
        assert!(!out.join("rock.fbx").exists(), "the copy was not deleted");
        assert!(
            dir.path().join("outside.txt").exists(),
            "undo escaped the destination folder"
        );
        assert!(
            c.recent_export_batches(10).expect("list").is_empty(),
            "a finished undo left its record behind"
        );

        // A destination that has since become a library source is refused outright, even though the
        // export into it was legal at the time. The sandbox protects the library, not the moment.
        c.upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
            .expect("src");
        std::fs::write(lib.join("inside.fbx"), b"library file").expect("w");
        c.record_exports(
            "batch-2",
            &lib.to_string_lossy(),
            &[(None, "inside.fbx".to_string())],
        )
        .expect("record");

        let r = undo_export_batch(&mut c, "batch-2").expect("undo");
        assert_eq!(r.refused, 1, "undo deleted inside a library source root");
        assert_eq!(r.changed, 0);
        assert!(
            lib.join("inside.fbx").exists(),
            "the non-destructive invariant was broken by an undo"
        );

        // An unknown batch is an error, not a silent no-op that reads as success.
        assert!(undo_export_batch(&mut c, "no-such-batch").is_err());
    }

    /// `n` queued `ExtractPackage` jobs. The reply receivers come back with them: dropping one
    /// makes its `send` fail, which would quietly mask a regression.
    fn queued_extracts(
        n: i64,
    ) -> (
        VecDeque<Job>,
        Vec<mpsc::Receiver<Result<WriteResult, String>>>,
    ) {
        let mut queue = VecDeque::new();
        let waits = (0..n)
            .map(|i| {
                let (reply, wait) = mpsc::channel();
                queue.push_back(Job {
                    op: Op::ExtractPackage {
                        package_id: 900 + i,
                    },
                    reply,
                    origin: Origin::User,
                });
                wait
            })
            .collect();
        (queue, waits)
    }

    /// The extract queue must never strand the packs behind a failing one.
    ///
    /// `Op::ExtractPackage` can fail *before* spawning anything — package row gone, no workspace
    /// configured, a pack with no extractable payloads — and only a spawned worker reports back to
    /// drain the queue. Draining one entry per report therefore left every pack behind a failure
    /// queued with nothing coming: `extract_queue.is_empty()` stayed false for the rest of the
    /// session, so every later extract joined a queue nobody drained — no job, no error, and the
    /// feature silently dead until restart.
    #[test]
    fn a_failing_extract_never_strands_the_queue_behind_it() {
        let jobs = JobRegistry::default();
        let (mut queue, _waits) = queued_extracts(3);

        // Every op fails to spawn, so nothing will ever report back on their behalf.
        let failures =
            drain_to_first_spawn(&mut queue, &jobs, |_| Err("no such package".to_string()));

        assert_eq!(failures, 3);
        assert!(
            queue.is_empty(),
            "{} pack(s) left queued with no worker coming for them",
            queue.len()
        );
        // The caller was told "accepted" when it queued, so the tray is the only place a spawn
        // failure can still be seen. Silence here is the same bug wearing a different hat.
        let failed = jobs.snapshot().recent.iter().filter(|j| !j.ok).count();
        assert_eq!(failed, 3, "every spawn failure must reach the job tray");
    }

    /// The other half of the rule: draining stops at the first op that *does* spawn, because that
    /// worker owns draining the rest. Going past it would unpack two packs at once, which is what
    /// UI-SPEC §5's one-at-a-time queue exists to prevent.
    #[test]
    fn the_queue_stops_at_the_first_op_that_spawns() {
        let jobs = JobRegistry::default();
        let (mut queue, _waits) = queued_extracts(3);

        let mut seen = 0;
        let failures = drain_to_first_spawn(&mut queue, &jobs, |_| {
            seen += 1;
            if seen == 2 {
                Ok(())
            } else {
                Err("no such package".to_string())
            }
        });

        assert_eq!(failures, 1, "only the first op failed");
        assert_eq!(seen, 2, "it must stop the moment one spawns");
        assert_eq!(
            queue.len(),
            1,
            "the pack behind the running one stays queued"
        );
    }

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

    /// Drain whatever is pending right now, as the publisher would at the end of a window.
    fn take(changes: &Changes) -> ChangedEvent {
        changes.inner.0.lock().expect("changes lock").take()
    }

    /// ADR-0016's headline: fifty writes landing inside one window reach the shell as one event.
    /// Before, every op emitted `catalog` *and* `jobs` — a hundred IPC events and fifty status
    /// re-reads for fifty tag writes, and a `jobs` event for ops that never touched the tray.
    #[test]
    fn fifty_writes_in_one_window_publish_once() {
        let changes = Changes::default();
        let (tx, rx) = mpsc::channel();
        changes.spawn_publisher(std::time::Duration::from_millis(60), move |e| {
            let _ = tx.send(e);
        });
        for _ in 0..50 {
            changes.catalog(Origin::User);
        }
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the window closes and publishes");
        assert_eq!(
            event,
            ChangedEvent {
                catalog: true,
                jobs: false,
                origins: vec!["user"],
                catalog_gen: 50,
                jobs_gen: 0,
            },
            "one event carrying every write's generation, and no jobs change nobody caused"
        );
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "nothing after it: the fifty were one publication"
        );
    }

    /// A fixed window, not a trailing debounce: a producer that never pauses (the preview drain)
    /// still publishes every window, where a debounce waiting for quiet would starve forever. And a
    /// window that mixes origins keeps all of them — the shell must not take an agent's write for
    /// the user's own because the two happened to share 250 ms.
    #[test]
    fn a_continuous_producer_publishes_every_window_and_loses_no_origin() {
        let changes = Changes::default();
        let (tx, rx) = mpsc::channel();
        changes.spawn_publisher(std::time::Duration::from_millis(20), move |e| {
            let _ = tx.send(e);
        });
        let origins = [Origin::User, Origin::Agent, Origin::Background];
        let started = std::time::Instant::now();
        let mut marks = 0u64;
        while started.elapsed() < std::time::Duration::from_millis(300) {
            changes.catalog(origins[(marks % 3) as usize]);
            marks += 1;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
            events.push(e);
        }
        assert!(
            events.len() >= 5,
            "300 ms of continuous writes over a 20 ms window published {} time(s) — starved",
            events.len()
        );
        assert!(
            events
                .windows(2)
                .all(|p| p[0].catalog_gen < p[1].catalog_gen),
            "generations only climb"
        );
        assert_eq!(
            events.last().map(|e| e.catalog_gen),
            Some(marks),
            "the last event accounts for every write — none lost, none deferred forever"
        );
        let seen: std::collections::HashSet<&str> = events
            .iter()
            .flat_map(|e| e.origins.iter().copied())
            .collect();
        assert_eq!(seen.len(), 3, "every origin survives coalescing: {seen:?}");
    }

    /// The tray is its own domain: a job starting, moving or finishing publishes `jobs`, never
    /// `catalog` — the grid has no reason to re-read because a download began.
    #[test]
    fn the_job_registry_publishes_its_own_changes_and_only_those() {
        let jobs = JobRegistry::default();
        let h = jobs.start("Extract X".into(), "extract", true);
        let started = take(jobs.changes());
        assert!(
            started.jobs && !started.catalog,
            "start is a jobs change: {started:?}"
        );
        jobs.progressed();
        jobs.finish(h.id, true, false, 3);
        let finished = take(jobs.changes());
        assert!(finished.jobs && !finished.catalog, "{finished:?}");
        assert_eq!(
            finished.jobs_gen, 3,
            "start, progress and finish each count"
        );
        assert!(
            finished.origins.is_empty(),
            "a tray change has no catalog origin"
        );
    }

    /// Only a turn that committed rows is a catalog change. `total_changes` is SQLite's own count,
    /// so a turn that only read stays silent and a write says so, under the origin that caused it.
    #[test]
    fn only_a_committed_write_marks_the_catalog() {
        let c = Catalog::open_in_memory().expect("open");
        let changes = Changes::default();

        let before = c.total_changes();
        let _ = c.source_infos().expect("read");
        mark_committed(&changes, &c, before, Origin::User);
        assert!(!take(&changes).catalog, "a read is not a change");

        let before = c.total_changes();
        c.upsert_import_source("s", "/s", "folder").expect("write");
        mark_committed(&changes, &c, before, Origin::Agent);
        let event = take(&changes);
        assert!(event.catalog, "a committed write is a change");
        assert_eq!(
            event.origins,
            vec!["agent"],
            "under the origin that caused it"
        );
    }

    /// The idle probe used to *receive and discard* whatever it found, so a write arriving at that
    /// instant never ran and never replied. It is held now, delivered next, and delivered once.
    #[test]
    fn a_write_found_by_the_idle_probe_is_run_next_not_dropped() {
        let (tx, rx) = mpsc::channel();
        let mut inbox = Inbox { rx, held: None };
        let (reply, wait) = mpsc::channel();
        tx.send(WriterMsg::Op(Job {
            op: Op::ClearStaged,
            reply,
            origin: Origin::User,
        }))
        .expect("send");
        assert!(inbox.waiting(), "the probe sees the queued write");
        assert!(
            inbox.waiting(),
            "probing again still holds exactly that one"
        );
        match inbox.next(std::time::Duration::ZERO) {
            Ok(WriterMsg::Op(job)) => job.reply.send(Ok(WriteResult::default())).expect("reply"),
            _ => panic!("the held write must come out next, ahead of any idle work"),
        }
        assert!(wait.recv().is_ok(), "its caller is answered");
        assert!(
            matches!(
                inbox.next(std::time::Duration::ZERO),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "and it is delivered once, not twice"
        );
    }
}
