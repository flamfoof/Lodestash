//! The catalog write path: **one dedicated writer thread**, fed by a channel.
//!
//! `ARCHITECTURE.md` §5 is explicit — "all writes funnel through one dedicated writer (a
//! thread/actor owning the connection, fed by a channel); reads use short-lived per-task
//! connections". This is that thread, owned by the engine (v2 spec §4.1, ADR-0021): it opens —
//! and migrates — the catalog, and nothing else in the process opens it for writing.
//!
//! Callers send an [`Op`] through a [`WriterHandle`] and block on its reply. The inbox is bounded
//! and the wait is too ([`INBOX_CAPACITY`], [`REPLY_TIMEOUT`]): a writer that cannot keep up
//! answers `busy` instead of hanging its callers. Every command runs under `catch_unwind`, so a
//! panic fails that command with `internal` and the thread keeps serving (review U9).
//!
//! **Reads do not come here.** The read model holds its own read-only connection
//! ([`crate::read_model`]; WAL makes a reader and a writer concurrent), so browsing never waits
//! behind a write.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender, SyncSender};
use std::sync::{Arc, Mutex};

use lodestash_ai::clap_tags::AudioZeroShot;
use lodestash_ai::image_tags::ImageZeroShot;
use lodestash_ai::ollama::OllamaTagger;
use lodestash_ai::openai::OpenAiTagger;
use lodestash_ai::tagger::{FilenameTagger, Tagger, TeachTrait};
use lodestash_core::catalog::{
    AppraiseFocus, Catalog, HistoryOrigin, PackageEntryRecord, TagBatch, TagBatchItem, TagMode,
};
use lodestash_core::types::{Exclude, ExcludeKind};
use lodestash_ingest::unitypackage::{
    extract_at_offsets_with_progress, read_manifest_with_progress, EntryLocator, ExtractedAsset,
};

use crate::config_store::ConfigStore;
use crate::drains::{
    drain_audio_tags, drain_previews, drain_promote_resync, drain_tags, resolve_audio_embedder,
    resolve_embedder,
};
use crate::engine::EngineStatus;
use crate::jobs::JobProgress as AppJobProgress;
use crate::types::PersistBatch;

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
    },
    /// **Delete from disk** — the one irreversible action in this product.
    DeleteFromDisk {
        targets: Vec<Target>,
        roots: Vec<PathBuf>,
        workspace: Option<PathBuf>,
    },
    /// Reclaim the workspace folders no live asset resolves through (UI-SPEC §11.4) — what it
    /// removed goes back on `outcome` ([`crate::storage::reclaim_workspace`]).
    ///
    /// On this thread because it deletes: "what is referenced" is read and acted on in one turn
    /// no extraction can interleave with, and an engine with no catalog has no writer to run it.
    ReclaimWorkspace {
        outcome: Sender<crate::storage::ReclaimOutcome>,
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
    /// Register or refresh an editor and make it active (ENGINE-API §5.1). Written here rather
    /// than through the engine's own connection, which is read-only (v1 plan S11).
    RegisterProject {
        project_path: String,
        unity_version: Option<String>,
        pipeline: Option<String>,
    },
    /// Durable import records for an acked request (ENGINE-API §5.3); `changed` counts the rows
    /// recorded, not the ones the dup-guard caught.
    RecordImports {
        project_id: i64,
        specs: Vec<lodestash_core::catalog::ImportSpec>,
        batch_id: String,
    },
    /// Export history for the files an export wrote (FEATURES §7.6).
    RecordExports {
        batch_id: String,
        dest_path: String,
        files: Vec<(Option<i64>, String)>,
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
    /// A scripted command — how the tests reach the writer's supervision (review U9, findings 2,
    /// 7 and 11): it can say it started, open a tray job, commit a real row, hold the writer, and
    /// then succeed, fail or panic.
    #[cfg(test)]
    Probe(Probe),
}

/// What an [`Op::Probe`] does, in this order.
#[cfg(test)]
#[derive(Default)]
pub struct Probe {
    /// Signalled when the writer starts running the command.
    pub started: Option<Sender<()>>,
    /// Open an `import` tray job first, the way a scan does.
    pub job: bool,
    /// Star this asset — a committed write the read model's snapshot shows.
    pub favorite: Option<i64>,
    /// Then hold the writer this long.
    pub hold: std::time::Duration,
    pub end: ProbeEnd,
}

#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ProbeEnd {
    #[default]
    Ok,
    Fail,
    Panic,
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
    pub reply: Sender<Result<WriteResult, WriteError>>,
    /// Who asked — decides how the shell hears about it (ADR-0016).
    pub origin: Origin,
    /// Whether the command is still wanted. The writer claims it before running it.
    pub(crate) claim: Arc<Claim>,
}

/// Where a queued command stands between its caller and the writer (review finding 2).
///
/// A caller that stops waiting has to be able to tell "it never ran" from "it is running", and a
/// command it was told never ran must then **not run** — "busy, try again" used to be said of a
/// Delete from disk that was merely queued, and the files went minutes later. Both sides race
/// for one atomic: the writer *starts* a command (`QUEUED → STARTED`) before running it, a caller
/// whose wait ran out *withdraws* it (`QUEUED → WITHDRAWN`). Exactly one of them wins.
#[derive(Default)]
pub(crate) struct Claim(std::sync::atomic::AtomicU8);

impl Claim {
    const QUEUED: u8 = 0;
    const STARTED: u8 = 1;
    const WITHDRAWN: u8 = 2;

    fn take(&self, to: u8) -> bool {
        self.0
            .compare_exchange(Self::QUEUED, to, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// The writer is about to run it. `false`: its caller withdrew it — do not run it.
    fn start(&self) -> bool {
        self.take(Self::STARTED)
    }

    /// The caller has stopped waiting. `false`: the writer already started it — it will finish.
    fn withdraw(&self) -> bool {
        self.take(Self::WITHDRAWN)
    }
}

/// What the writer thread's channel carries: an external op (with its reply), or a worker
/// reporting back so the catalog half runs on the writer thread (single-writer rule).
pub(crate) enum WriterMsg {
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
        done: Vec<crate::drains::ComputedEmbedding>,
    },
    /// A background tag worker finished a slice — same split, same reason: a caption is a whole
    /// VLM inference (~13 s here) and the writer only persists the result.
    TagsDone {
        producers: crate::drains::TagProducers,
        done: Vec<crate::drains::ComputedTags>,
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

/// Ends a tray job when dropped — as **failed**, unless [`JobGuard::finish`] reported how it
/// really ended (review finding 11).
///
/// A job used to be opened and closed by two separate calls with the work between them, so a
/// panic in that work — caught by the writer's supervision, which is the point of supervising —
/// left the job spinning in the tray for the rest of the session, and one of kind `extract`
/// blocked every later extraction. Open jobs through [`JobRegistry::open`] and the end is owned.
pub struct JobGuard {
    jobs: JobRegistry,
    id: u64,
    open: bool,
}

impl JobGuard {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Report how the job ended.
    pub fn finish(mut self, ok: bool, cancelled: bool, changed: usize) {
        self.open = false;
        self.jobs.finish(self.id, ok, cancelled, changed);
    }

    /// Someone else ends this job now: the writer, once a worker's report has reached it.
    fn hand_over(mut self) {
        self.open = false;
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        if self.open {
            self.jobs.finish(self.id, false, false, 0);
        }
    }
}

/// Holds an in-flight flag up for one off-writer slice and lowers it when dropped (review
/// finding 11). The embed and tag lanes each run one slice at a time behind such a flag, and
/// only the slice's result landing on the writer used to clear it — so a worker that panicked,
/// or a thread that never started, left the lane shut for the life of the process.
struct Inflight {
    flag: Arc<AtomicBool>,
    raised: bool,
}

impl Inflight {
    fn raise(flag: &Arc<AtomicBool>) -> Self {
        flag.store(true, Ordering::Relaxed);
        Self {
            flag: flag.clone(),
            raised: true,
        }
    }

    /// The slice's result is on its way to the writer, which lowers the flag when it lands.
    fn hand_over(mut self) {
        self.raised = false;
    }
}

impl Drop for Inflight {
    fn drop(&mut self) {
        if self.raised {
            self.flag.store(false, Ordering::Relaxed);
        }
    }
}

/// Run one off-writer slice on a thread of its own: `work` computes the message for the writer
/// (`None` when there is nothing to send). The flag stays up only if a message was handed over;
/// a panic in `work`, nothing to send, or a writer that has gone all lower it.
fn spawn_slice(
    slice: Inflight,
    tx: SyncSender<WriterMsg>,
    work: impl FnOnce() -> Option<WriterMsg> + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if work().is_some_and(|msg| tx.send(msg).is_ok()) {
            slice.hand_over();
        }
    })
}

/// A pack worker's promise to report back (review finding 11).
///
/// The report is the only thing that ends the worker's job and lets the one-at-a-time extract
/// queue move, so it is made whatever happens: by [`PackReport::manifest`] /
/// [`PackReport::extract`] when the work is done, and from `Drop` — as a failure — when the
/// worker unwinds or its thread never starts. With no writer left to hear it, the job is ended
/// here instead.
struct PackReport {
    tx: SyncSender<WriterMsg>,
    job: Option<JobGuard>,
    package_id: i64,
    analysing: bool,
}

impl PackReport {
    fn new(tx: &SyncSender<WriterMsg>, job: JobGuard, package_id: i64, analysing: bool) -> Self {
        Self {
            tx: tx.clone(),
            job: Some(job),
            package_id,
            analysing,
        }
    }

    fn manifest(mut self, entries: Result<Vec<PackageEntryRecord>, String>, cancelled: bool) {
        if let Some(job) = self.job.take() {
            let msg = WriterMsg::ManifestDone {
                job_id: job.id(),
                package_id: self.package_id,
                entries,
                cancelled,
            };
            if self.tx.send(msg).is_ok() {
                job.hand_over();
            }
        }
    }

    fn extract(mut self, outcome: Result<Vec<ExtractedAsset>, String>, cancelled: bool) {
        if let Some(job) = self.job.take() {
            let msg = WriterMsg::ExtractDone {
                job_id: job.id(),
                package_id: self.package_id,
                outcome,
                cancelled,
            };
            if self.tx.send(msg).is_ok() {
                job.hand_over();
            }
        }
    }
}

impl Drop for PackReport {
    fn drop(&mut self) {
        let Some(job) = self.job.take() else {
            return;
        };
        let why = "the worker stopped without reporting — it panicked or never started".to_string();
        let (job_id, package_id) = (job.id(), self.package_id);
        let msg = if self.analysing {
            WriterMsg::ManifestDone {
                job_id,
                package_id,
                entries: Err(why),
                cancelled: false,
            }
        } else {
            WriterMsg::ExtractDone {
                job_id,
                package_id,
                outcome: Err(why),
                cancelled: false,
            }
        };
        // `try_send`: this can run on the writer thread itself (a worker that failed to spawn),
        // which must not block on its own full inbox. Unsent, `job` drops and ends itself.
        if self.tx.try_send(msg).is_ok() {
            job.hand_over();
        }
    }
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

    /// [`Self::start`], with the job's end owned: the guard finishes it as failed if it is
    /// dropped without [`JobGuard::finish`] — on a panic, an early return, a thread that never
    /// ran. Prefer this to `start` + `finish` wherever work happens between the two.
    pub fn open(&self, label: String, kind: &str, cancellable: bool) -> (JobHandles, JobGuard) {
        let handles = self.start(label, kind, cancellable);
        let guard = JobGuard {
            jobs: self.clone(),
            id: handles.id,
            open: true,
        };
        (handles, guard)
    }

    /// Move an active job to the session ring. `changed` is what the tray reports. Public for
    /// the same reason as `start`.
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

    /// Is a pack being analysed or extracted right now? The one-at-a-time queue gates on it, and
    /// the workspace reclaim refuses under it.
    pub(crate) fn has_active_extract(&self) -> bool {
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

use crate::changes::{mark_committed, Stamp};
pub use crate::changes::{ChangedEvent, Changes, Origin};

/// The writer's inbox depth (v2 spec §4.1). A full inbox answers [`WriteError::Busy`] at once
/// rather than queueing a caller behind work it cannot see.
pub const INBOX_CAPACITY: usize = 1024;

/// How long a caller waits for its reply. What it is told then depends on where its command is
/// ([`Claim`]): still queued, it is withdrawn and the caller told [`WriteError::Busy`]; already
/// running, it finishes on its own and the caller is told [`WriteError::StillRunning`].
pub const REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Why a write did not happen (v2 spec §4.4 kinds, where the writer itself knows them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    /// The command ran and refused — the message is the one the caller shows. Its finer kind
    /// (`not_found`, `invalid`, `conflict`) arrives when the commands carry typed errors (P3).
    Failed(String),
    /// The command **did not run and will not**: the inbox was full, or it was still queued when
    /// its caller's wait ran out and was withdrawn. Nothing was written; asking again is safe.
    Busy,
    /// The command **is running** and its caller's wait ran out. It will finish and commit on its
    /// own, so asking again would do it twice. Its result shows when it lands.
    StillRunning,
    /// The command panicked. The writer caught it and kept serving; anything it had already
    /// committed stays committed, and the catalog generation says so.
    Internal(String),
    /// There is no writer to ask: the engine did not start, or it has stopped.
    Unavailable(String),
}

impl WriteError {
    /// The stable code the API maps (`lodestash_protocol::codes`); `failed` until commands are
    /// typed. Both "not run" and "still running" are `busy` — the kind an agent is told to
    /// expect — and the message, which the code leads, says which.
    pub fn code(&self) -> &'static str {
        match self {
            WriteError::Failed(_) => "failed",
            WriteError::Busy | WriteError::StillRunning => lodestash_protocol::codes::BUSY,
            WriteError::Internal(_) => lodestash_protocol::codes::INTERNAL,
            WriteError::Unavailable(_) => lodestash_protocol::codes::UNAVAILABLE,
        }
    }
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // Verbatim: the old shell showed these strings to the user, and still does.
            WriteError::Failed(m) | WriteError::Unavailable(m) => f.write_str(m),
            WriteError::Busy => f.write_str(
                "busy: the catalog writer is occupied and this command was not run — nothing \
                 was written; try again",
            ),
            WriteError::StillRunning => f.write_str(
                "busy: the command is still running — it will finish on its own, so do not send \
                 it again; its result shows when it lands",
            ),
            WriteError::Internal(m) => write!(
                f,
                "internal: the write failed unexpectedly ({m}); the writer is still running"
            ),
        }
    }
}

impl std::error::Error for WriteError {}

/// Handle to the writer thread. Cloneable, `Send`, holds no connection of its own.
#[derive(Clone)]
pub struct WriterHandle {
    tx: Option<SyncSender<WriterMsg>>,
    /// What every write answers when there is no writer (`tx` is `None`).
    absent: Arc<str>,
    reply_timeout: std::time::Duration,
    /// Set when the engine is dropped; the loop sees it within one idle tick and exits, closing
    /// the catalog and releasing its lock.
    stop: Arc<AtomicBool>,
}

impl WriterHandle {
    /// A handle with no writer behind it: every write answers `unavailable` with `why`.
    pub(crate) fn absent(why: impl Into<String>) -> Self {
        Self {
            tx: None,
            absent: why.into().into(),
            reply_timeout: REPLY_TIMEOUT,
            stop: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Run one command and wait for it, as the person at the machine.
    pub fn run(&self, op: Op) -> Result<WriteResult, WriteError> {
        self.run_as(op, Origin::User)
    }

    /// [`WriterHandle::run`] on someone else's behalf. The agent bridge says so, and the shell then
    /// shows a pending update rather than re-reading under the user (ADR-0016).
    pub fn run_as(&self, op: Op, origin: Origin) -> Result<WriteResult, WriteError> {
        let Some(tx) = &self.tx else {
            return Err(WriteError::Unavailable(self.absent.to_string()));
        };
        let (reply, wait) = mpsc::channel();
        let claim = Arc::new(Claim::default());
        let job = Job {
            op,
            reply,
            origin,
            claim: claim.clone(),
        };
        match tx.try_send(WriterMsg::Op(job)) {
            Ok(()) => {}
            // Never queued, so it will never run.
            Err(mpsc::TrySendError::Full(_)) => return Err(WriteError::Busy),
            Err(mpsc::TrySendError::Disconnected(_)) => {
                return Err(WriteError::Unavailable(
                    "the catalog writer has stopped".into(),
                ))
            }
        }
        match wait.recv_timeout(self.reply_timeout) {
            Ok(result) => result,
            // Still queued: withdrawn, so "it was not run" stays true.
            Err(mpsc::RecvTimeoutError::Timeout) if claim.withdraw() => Err(WriteError::Busy),
            // The writer has it. It may have answered in the instant the wait ran out.
            Err(mpsc::RecvTimeoutError::Timeout) => {
                wait.try_recv().unwrap_or(Err(WriteError::StillRunning))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(WriteError::Internal(
                "the writer dropped the command without answering".into(),
            )),
        }
    }

    /// The same writer, with a shorter wait — how a test reaches the timeout.
    #[cfg(test)]
    pub(crate) fn with_reply_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.reply_timeout = timeout;
        self
    }

    /// Ask the loop to exit. It finishes the command in hand first.
    pub(crate) fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Everything the writer thread is started with — the engine's paths and shared state
/// ([`crate::EngineConfig`]); the thread reads nothing from process-wide locations itself.
pub(crate) struct WriterSetup {
    pub db_path: PathBuf,
    pub cache_dir: Option<PathBuf>,
    pub workspace_dir: Option<PathBuf>,
    /// The engine's one config store — every read here is of the current, in-memory config.
    pub config: Arc<ConfigStore>,
    /// The catalog's `.lock` file, held for the thread's life (v2 spec §4.2).
    pub lock: std::fs::File,
    pub status: Arc<crate::engine::StatusCell>,
    pub jobs: JobRegistry,
    /// Raised while an appraise pass runs (B11d).
    pub pump: Arc<AtomicBool>,
    pub appraise: Arc<AppJobProgress>,
    /// Where the user is looking (UI-SPEC §3). Shared rather than sent as an op: it changes on
    /// every scroll, and a browse must never queue behind a running appraise pass.
    pub focus: Arc<Mutex<AppraiseFocus>>,
}

/// What every command and helper on the writer thread reads besides the catalog.
pub(crate) struct WriterCtx {
    db_path: PathBuf,
    tx: SyncSender<WriterMsg>,
    jobs: JobRegistry,
    appraise: Arc<AppJobProgress>,
    focus: Arc<Mutex<AppraiseFocus>>,
    cache_dir: Option<PathBuf>,
    workspace_dir: Option<PathBuf>,
    config: Arc<ConfigStore>,
}

/// Run one command under `catch_unwind` (v2 spec §4.1, review U9): a panic fails that command
/// with `internal` instead of ending the thread — and with it every write for the session.
fn supervised(f: impl FnOnce() -> Result<WriteResult, String>) -> Result<WriteResult, WriteError> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(e)) => Err(WriteError::Failed(e)),
        Err(payload) => {
            let msg = panic_text(payload.as_ref());
            tracing::error!(%msg, "a writer command panicked; the writer keeps serving");
            Err(WriteError::Internal(msg))
        }
    }
}

/// Background work on the writer thread (a worker's result landing, the idle pass) under the same
/// supervision. `None` means it panicked; that is logged and the loop carries on.
fn guarded<T>(what: &str, f: impl FnOnce() -> T) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(payload) => {
            let msg = panic_text(payload.as_ref());
            tracing::error!(what, %msg, "writer work panicked; the writer keeps serving");
            None
        }
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "a panic with no message".into())
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

/// Spawn the writer over `setup.db_path`. The connection is opened **on the thread that owns it**,
/// never handed across, because that is the whole point of the rule — and so is the migration:
/// the engine is the only code that opens the catalog for writing (v2 spec §4.2).
pub(crate) fn spawn(setup: WriterSetup) -> WriterHandle {
    let (tx, rx) = mpsc::sync_channel::<WriterMsg>(INBOX_CAPACITY);
    let stop = Arc::new(AtomicBool::new(false));
    let handle = WriterHandle {
        tx: Some(tx.clone()),
        absent: "".into(),
        reply_timeout: REPLY_TIMEOUT,
        stop: stop.clone(),
    };
    std::thread::Builder::new()
        .name("lodestash-writer".into())
        .spawn(move || writer_main(setup, tx, rx, stop))
        .expect("writer thread spawns");
    handle
}

/// Open (create, migrate) the catalog, reporting `Migrating` while a migration runs. A failure is
/// the status the shell shows — never an empty library (v2 spec §1.5).
fn open_catalog(path: &Path, status: &crate::engine::StatusCell) -> Result<Catalog, EngineStatus> {
    use lodestash_core::error::CoreError;
    let too_new = |found: u32, supported: u32| EngineStatus::Failed {
        code: "schema_too_new".into(),
        message: format!(
            "this catalog was written by a newer Lodestash (schema {found}; this build reads up to \
             {supported}). Update the app — the catalog is left untouched."
        ),
    };
    if path.exists() {
        // A read-only peek at the version, so the status can say a migration is running.
        match Catalog::open_reader(path) {
            Err(CoreError::SchemaTooOld { found, supported }) => {
                status.set(EngineStatus::Migrating {
                    from: found,
                    to: supported,
                });
            }
            Err(CoreError::SchemaTooNew { found, supported }) => {
                return Err(too_new(found, supported));
            }
            // Current, or unreadable — the writing open below says what is wrong.
            _ => {}
        }
    }
    Catalog::open(path).map_err(|e| match e {
        CoreError::SchemaTooNew { found, supported } => too_new(found, supported),
        other => EngineStatus::Failed {
            code: "catalog_open_failed".into(),
            message: format!(
                "the catalog at {} could not be opened: {other}",
                path.display()
            ),
        },
    })
}

/// No catalog to write to: answer every command with why, until the engine stops.
fn refuse_until_stopped(inbox: &mut Inbox, stop: &AtomicBool, why: &str) {
    while !stop.load(Ordering::Relaxed) {
        match inbox.next(std::time::Duration::from_millis(250)) {
            Ok(WriterMsg::Op(job)) => {
                let _ = job
                    .reply
                    .send(Err(WriteError::Unavailable(why.to_string())));
            }
            Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn writer_main(
    setup: WriterSetup,
    writer_tx: SyncSender<WriterMsg>,
    rx: mpsc::Receiver<WriterMsg>,
    stop: Arc<AtomicBool>,
) {
    // Declared first, so it drops last: the lock is released only once the connection is closed.
    let _lock = setup.lock;
    let mut inbox = Inbox { rx, held: None };
    // Opened once and held for the thread's life: reopening per job would be a new writer each
    // time, which is the thing the rule forbids.
    let mut catalog = match open_catalog(&setup.db_path, &setup.status) {
        Ok(c) => c,
        Err(failed) => {
            let why = match &failed {
                EngineStatus::Failed { message, .. } => message.clone(),
                _ => "the catalog could not be opened".to_string(),
            };
            tracing::error!(%why, "writer: catalog open failed; writes will be refused");
            setup.status.set(failed);
            refuse_until_stopped(&mut inbox, &stop, &why);
            return;
        }
    };
    setup.status.set(EngineStatus::Ready);

    let ctx = WriterCtx {
        db_path: setup.db_path.clone(),
        tx: writer_tx,
        jobs: setup.jobs.clone(),
        appraise: setup.appraise.clone(),
        focus: setup.focus.clone(),
        cache_dir: setup.cache_dir.clone(),
        workspace_dir: setup.workspace_dir.clone(),
        config: setup.config.clone(),
    };
    let pump = setup.pump;
    let jobs = &ctx.jobs;
    let changes = jobs.changes().clone();

    // One-shot chores before the loop. Packs indexed before the harvest existed have no pipeline,
    // and only the writer thread may fix that (single-writer rule)…
    let before = Stamp::of(&catalog);
    guarded("startup backfill", || backfill_package_pipelines(&catalog));
    // …and enforce the preview cap the settings screen has always shown and never applied
    // (UI-SPEC §11.4). Startup rather than per-write: the cache grows a thumbnail at a time and a
    // check on that path would stat the whole tree on every preview.
    let display = ctx.config.get();
    if let Some(thumbs) = ctx.cache_dir.as_deref() {
        guarded("preview cache sweep", || {
            // Identity before pressure. A stale-budget proxy, an orphan staging mesh and a
            // crash-leftover `.part` are dead the moment they become so, and the cap never reaches
            // them: 359 MB against an 8 GB cap means the purge below had never once run while
            // 145 MB of that was already garbage.
            crate::storage::sweep_dead_mesh_cache(
                thumbs,
                &display.preview_quality.proxy_budget().tag(),
            );
            if display.auto_purge_preview_cache {
                crate::storage::purge_previews_over_cap(
                    thumbs,
                    u64::from(display.preview_cache_cap_gb) * 1024 * 1024 * 1024,
                );
            }
        });
    }
    mark_committed(&changes, &catalog, before, Origin::Background);

    // Model preview backfill (M15) is time-boxed and durable, and this thread is the one place it
    // may write (single-writer rule). Ops preempt it within a few seconds; the queue is durable,
    // so it converges over idle moments.
    let proxy = display.preview_quality.proxy_budget();
    // B11c: when auto-identify is on and the local models are present, the idle pass appraises
    // (metrics → previews → deps → embeddings → promote) instead of only backfilling previews.
    // The embedders are resolved once here — model files load at startup only when the user
    // opted in — and borrowed per slice.
    let auto = display.auto_identify;
    let (idle_embedder, idle_audio) = if auto {
        (resolve_embedder(), resolve_audio_embedder())
    } else {
        (None, None)
    };
    let idle_appraises = auto && (idle_embedder.is_some() || idle_audio.is_some());
    // Test/CI escape hatch: the preview backfill is CPU-heavy (each model render is a decimate +
    // 4-view raster), and an e2e run that launches the app dozens of times would otherwise grind
    // the machine. LODESTASH_NO_PREVIEW_DRAIN=1 skips it; the grid then shows type marks for
    // models without previews.
    let drain_enabled = std::env::var("LODESTASH_NO_PREVIEW_DRAIN").is_err();
    // UI-SPEC §5: ticked packs unpack **one at a time**. An ExtractPackage op that arrives while a
    // worker is running (or already queued) waits here; each op's reply resolves to the real
    // outcome when its reconcile runs.
    let mut extract_queue: VecDeque<Job> = VecDeque::new();
    // One embed slice in flight at a time. The worker owns the encode; this flag is how the idle
    // tick knows not to start a second one, and it is cleared when the slice's `EmbeddingsDone`
    // lands (or when its worker gives up).
    let embed_inflight = Arc::new(AtomicBool::new(false));
    let tag_inflight = Arc::new(AtomicBool::new(false));
    // DATA-MODEL §6 watch: live notify over the watched source roots, debounced by the ingest
    // helper. A signal means "something changed, rescan when convenient" — the rescan is the
    // incremental crawl (needs_rehash skips unchanged files), so it is cheap. Skipped under the
    // e2e escape hatch like every other idle pass.
    let mut watch = if drain_enabled && display.watch_sources {
        rebuild_watch(&catalog)
    } else {
        None
    };
    // The backlog lane works one tick in `IDLE_DUTY_PERIOD`, not every tick.
    //
    // Measured on the reference library: an idle app — window open, nothing asked of it — burned
    // **0.57 cores continuously**, forever, because each 1 s timeout started a 1 s slice. This is
    // an asset viewer; housekeeping does not get half the machine. Duty-cycling here throttles all
    // three lanes at once: the embed and tag workers gate on their `*_inflight` flags, and only
    // this thread clears them.
    //
    // Op latency is unaffected — a skipped tick still waits on the channel, so a user write is
    // picked up the moment it arrives (the point of 147d4b5).
    let mut idle_tick: u32 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match inbox.next(std::time::Duration::from_secs(1)) {
            Ok(WriterMsg::Op(Job {
                op,
                reply,
                origin,
                claim,
            })) => {
                // Its caller stopped waiting while it was queued and was told it had not run
                // (`WriteError::Busy`). It must not run now — least of all a delete.
                if !claim.start() {
                    tracing::info!("skipped a command its caller withdrew while it was queued");
                    continue;
                }
                if matches!(op, Op::ExtractPackage { .. }) {
                    // Extract answers **"accepted"**, not "finished" — the unpack runs for minutes
                    // on a multi-GB pack, and its progress, outcome and cancel button live in the
                    // job tray. Both branches reply at the same point on purpose: the reply used
                    // to be sent at spawn when the queue was empty but only at *start* when it
                    // wasn't, so one command meant two different things depending on queue depth.
                    if extract_queue.is_empty() && !jobs.has_active_extract() {
                        // This turn mints the pack's in-pack rows before any worker runs, so it
                        // is marked like any other: it never was, and when the extraction then
                        // failed nothing told the read model the rows existed.
                        let before = Stamp::of(&catalog);
                        let result = supervised(|| run(&mut catalog, op, &ctx));
                        mark_committed(&changes, &catalog, before, origin);
                        let _ = reply.send(result);
                    } else {
                        // A pack is unpacking (or queued) — join the queue, and say so now rather
                        // than blocking the caller until this pack's turn.
                        let _ = reply.send(Ok(WriteResult::default()));
                        extract_queue.push_back(Job {
                            op,
                            reply,
                            origin,
                            claim,
                        });
                    }
                    continue;
                }
                // Only the appraise kicks are slow enough to count as "the pump is running" —
                // everything else is a quick write the frontend already waits on synchronously.
                let appraising = matches!(op, Op::IdentifyAll | Op::IdentifyOne { .. });
                if appraising {
                    pump.store(true, Ordering::Relaxed);
                }
                let source_set_changed =
                    matches!(op, Op::PersistScan { .. } | Op::SetSourceWatch { .. });
                let before = Stamp::of(&catalog);
                let result = supervised(|| run(&mut catalog, op, &ctx));
                if appraising {
                    pump.store(false, Ordering::Relaxed);
                }
                // Marked whenever SQLite's counters moved — also when the command then failed or
                // panicked. A command can commit and *then* fail (a scan whose packs step errors,
                // a selection that stops half-way), and marking only on success left the read
                // model serving the snapshot from before rows that are now in the catalog. The
                // mark says "the catalog changed", not "the command succeeded": the caller
                // learns that from the reply.
                mark_committed(&changes, &catalog, before, origin);
                let _ = reply.send(result);
                // A source set (or its watch flags) changed — re-arm the watch so new roots are
                // covered and unwatched ones stop firing.
                if source_set_changed {
                    watch = if drain_enabled && ctx.config.read(|c| c.watch_sources) {
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
                // The payloads landed on disk off-thread; materialize them here — the catalog
                // stays single-writer. Then the next queued pack gets its worker.
                let before = Stamp::of(&catalog);
                let result = guarded("extract materialize", || {
                    materialize_extract(&mut catalog, &ctx, package_id, outcome, cancelled)
                })
                .unwrap_or_else(|| Err("materializing the pack's files panicked".into()));
                let ok = result.is_ok();
                let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
                jobs.finish(job_id, ok, cancelled, changed);
                start_next_extract(&mut catalog, &mut extract_queue, &ctx);
                // Background: the unpack was asked for minutes ago, and its outcome is in the
                // tray — not a reason to re-read the page under the user.
                mark_committed(&changes, &catalog, before, Origin::Background);
            }
            Ok(WriterMsg::EmbeddingsDone { model_id, done }) => {
                // Cheap: N `upsert_embedding` calls, no inference. Clearing `embedding_dirty`
                // here is what lets `promote_statuses` move the asset on, so the promote runs on
                // the next idle tick.
                let before = Stamp::of(&catalog);
                guarded("embeddings store", || {
                    crate::drains::store_visual_embeddings(&mut catalog, &model_id, &done)
                });
                embed_inflight.store(false, Ordering::Relaxed);
                // By what was written, not by the count returned: a store that panicked after
                // its first rows returns nothing, and those rows are still in the catalog.
                mark_committed(&changes, &catalog, before, Origin::Background);
            }
            Ok(WriterMsg::TagsDone { producers, done }) => {
                let before = Stamp::of(&catalog);
                let n = guarded("tags store", || {
                    crate::drains::store_tags(&mut catalog, &producers, &done)
                })
                .unwrap_or(0);
                tag_inflight.store(false, Ordering::Relaxed);
                if n > 0 {
                    tracing::info!(tags = n, producer = %producers.tagger.0, "idle tagging slice stored");
                }
                // An empty result still finishes its assets (the queue flag clears), so the
                // catalog changed even when no tag row did.
                mark_committed(&changes, &catalog, before, Origin::Background);
            }
            Ok(WriterMsg::ManifestDone {
                job_id,
                package_id,
                entries,
                cancelled,
            }) => {
                // The archive's manifest is read; record the locators + mint the in-pack assets
                // (surgical indexing, M14), then spawn the extract worker over the locators. The
                // op's reply was "accepted"; this phase reports through the tray.
                // `spawned` = an extract worker is now running, so *its* `ExtractDone` drains the
                // queue. Anything else — an error, or a cancel that stops after the analyze phase
                // — has to settle here, or the queue behind it never moves again.
                let before = Stamp::of(&catalog);
                let mut spawned = false;
                let result: Result<WriteResult, String> = guarded("manifest record", || {
                    let entries = entries?;
                    let minted = mint_in_pack(&mut catalog, package_id, &entries)?;
                    // Cancelled during analyze: keep the locators that were read (a partial
                    // manifest is still progress and the next run resumes from it) but do
                    // **not** roll on into the extract phase — the user asked it to stop.
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
                    let workspace_dir = ctx
                        .workspace_dir
                        .clone()
                        .ok_or_else(|| "no workspace directory configured".to_string())?;
                    spawn_extract_worker(&pkg, &workspace_dir, &entries, &ctx.tx, &ctx.jobs)?;
                    spawned = true;
                    Ok(WriteResult {
                        changed: minted,
                        ..Default::default()
                    })
                })
                .unwrap_or_else(|| Err("recording the pack's manifest panicked".into()));
                let ok = result.is_ok();
                let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
                jobs.finish(job_id, ok, cancelled, changed);
                if !spawned {
                    start_next_extract(&mut catalog, &mut extract_queue, &ctx);
                }
                mark_committed(&changes, &catalog, before, Origin::Background);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Quiet for a second. Backfill previews in short slices, and only when nothing is
                // queued — a long pass would make every write wait for it (measured: a 5s slice
                // pushed a tag-create past the UI's own timeouts). 1s slices keep op latency under
                // ~1.2s while still filling most idle time; the queue is durable, so it converges.
                if !drain_enabled {
                    continue;
                }
                if inbox.waiting() {
                    continue; // an op is waiting — held, and run on the next turn
                }
                // The pass is marked once, after it, by what SQLite says it wrote — so a pass
                // that panics part-way still publishes the rows it had committed (it used to mark
                // only at the end of each branch, which a panic never reached).
                let before = Stamp::of(&catalog);
                guarded("idle pass", || {
                    // Watch signal (debounced upstream): rescan every watched source with the
                    // incremental crawl. The writer is idle here by construction.
                    if let Some(w) = &watch {
                        if w.rx.recv_timeout(std::time::Duration::ZERO).is_ok() {
                            rescan_watched(&mut catalog, &ctx);
                        }
                    }
                    idle_tick = idle_tick.wrapping_add(1);
                    if !idle_tick.is_multiple_of(IDLE_DUTY_PERIOD) {
                        return;
                    }
                    if idle_appraises {
                        // The encode goes to a worker; the writer keeps only the two cheap ends
                        // (read the queue slice here, persist the result when `EmbeddingsDone`
                        // arrives). Passing `None` for the embedders below is what keeps the
                        // pass's own embed drain out of this thread — the kick paths still run it
                        // inline, because there the user asked for it and is watching a bar.
                        if let Some(emb) = idle_embedder.clone() {
                            if !embed_inflight.load(Ordering::Relaxed) {
                                const SLICE: usize = 8;
                                let (jobs, albedo) =
                                    crate::drains::visual_embed_batch(&catalog, SLICE);
                                if !jobs.is_empty() {
                                    // Raised here and lowered by whoever ends the slice: the
                                    // writer when `EmbeddingsDone` lands, or the guard itself when
                                    // the worker panics, cannot send, or never starts.
                                    let slice = Inflight::raise(&embed_inflight);
                                    let (ws, cd) =
                                        (ctx.workspace_dir.clone(), ctx.cache_dir.clone());
                                    spawn_slice(slice, ctx.tx.clone(), move || {
                                        let done = crate::drains::compute_visual_embeddings(
                                            jobs,
                                            &albedo,
                                            emb.as_ref(),
                                            ws.as_deref(),
                                            cd.as_deref(),
                                        );
                                        let model_id = emb.model_id().to_string();
                                        Some(WriterMsg::EmbeddingsDone { model_id, done })
                                    });
                                }
                            }
                        }
                        // Tagging takes the same lane. The worker opens its own **read-only**
                        // catalog connection for the user's taught vocabulary — WAL admits any
                        // number of readers, and the single-writer rule is about writes
                        // (ADR-0002), which still all land right here.
                        if !tag_inflight.load(Ordering::Relaxed) {
                            const TAG_SLICE: usize = 4;
                            let jobs = crate::drains::tag_batch(&catalog, TAG_SLICE, None);
                            if !jobs.is_empty() {
                                let slice = Inflight::raise(&tag_inflight);
                                let (ws, cd) = (ctx.workspace_dir.clone(), ctx.cache_dir.clone());
                                let db = ctx.db_path.clone();
                                let emb = idle_embedder.clone();
                                let config = ctx.config.clone();
                                spawn_slice(slice, ctx.tx.clone(), move || {
                                    tag_worker_slice(&db, jobs, emb, ws, cd, &config)
                                });
                            }
                        }
                        pump.store(true, Ordering::Relaxed);
                        // Its outcome is the catalog's: the mark below reads what it wrote.
                        let _ = appraise_pass(
                            &mut catalog,
                            &ctx,
                            None,
                            idle_audio.clone(),
                            std::time::Duration::from_secs(1),
                            // The idle pass no longer tags inline — the worker above does it. An
                            // empty explicit set means "no assets", which is the one thing it
                            // cannot be confused with "everything".
                            Some(&[]),
                        );
                        pump.store(false, Ordering::Relaxed);
                    } else {
                        drain_previews(
                            &mut catalog,
                            ctx.workspace_dir.as_deref(),
                            ctx.cache_dir.as_deref(),
                            proxy,
                            std::time::Duration::from_secs(1),
                        );
                    }
                });
                // A panic inside the pass may have left the flag up; the next pass resets it.
                pump.store(false, Ordering::Relaxed);
                mark_committed(&changes, &catalog, before, Origin::Background);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    tracing::debug!("writer: stopped, thread exiting");
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
fn rescan_watched(catalog: &mut Catalog, ctx: &WriterCtx) -> bool {
    let cfg = ctx.config.get();
    let project = cfg.last_project_name.as_deref();
    let mut changed = false;
    let mut prefab_scopes = Vec::new();
    for info in catalog.source_infos().unwrap_or_default() {
        if !info.watch_enabled {
            continue;
        }
        let root = PathBuf::from(&info.root_path);
        prefab_scopes.push(crate::drains::PrefabScope {
            root: root.clone(),
            absent_is_empty: false,
        });
        let excludes = project
            .map(|p| cfg.source_excludes(p, &root))
            .unwrap_or_default();
        let known = catalog
            .known_sizes_for_source(&info.root_path)
            .unwrap_or_default();
        let crawled = crate::scan::crawl_source(0, &root, &excludes, &known);
        if crawled.entries.is_empty() && crawled.pending_packages.is_empty() {
            continue;
        }
        let records = crate::persist::crawl_records(crawled.entries.iter().map(|(_, f)| f));
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
        let batch = crate::types::PersistBatch {
            root: info.root_path.clone(),
            name: info.name.clone(),
            excludes_json: None,
            records,
        };
        match crate::persist::persist_to_catalog(&ctx.db_path, &[batch]) {
            Ok(_) => changed = true,
            Err(e) => tracing::warn!(%e, root = %info.root_path, "watch rescan failed"),
        }
    }
    // A prefab is not an asset, so the crawl above never sees one change — yet the watch fired.
    // Re-read the watched trees' prefab evidence; only files whose revision moved are parsed.
    if crate::drains::refresh_prefab_evidence(catalog, &prefab_scopes, false) > 0 {
        changed = true;
    }
    changed
}

/// One bounded appraise pass (metrics → previews → deps → embeddings → promote) on the/// writer thread. Shared by the ✦ kicks and the auto-identify idle pass (B11a–c): embedders
/// are resolved by the caller (the idle pass resolved them once at startup), everything else
/// is re-derived here because it can change between passes. `progress` is the shared
/// B11d surface the Identify screen polls — reset at pass start, filled by the tag/embed
/// drains as they work.
/// Compute one idle tagging slice off the writer thread: the `TagsDone` for the writer, or
/// `None` when the slice has nothing to store. [`spawn_slice`] sends it and keeps the in-flight
/// flag honest whichever it is — or if this panics.
fn tag_worker_slice(
    db_path: &std::path::Path,
    jobs: Vec<lodestash_core::catalog::VisualEmbedJob>,
    embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    workspace_dir: Option<std::path::PathBuf>,
    cache_dir: Option<std::path::PathBuf>,
    config: &ConfigStore,
) -> Option<WriterMsg> {
    // A read connection, only for the user's taught vocabulary — WAL admits any number of
    // readers, and every write still lands on the writer thread (ADR-0002). Read-only by type:
    // this used to be the migrating `Catalog::open` (review R5).
    let Ok(cat) = Catalog::open_reader(db_path) else {
        tracing::warn!("tag worker could not open a read connection; slice skipped");
        return None;
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
    let cfg = config.get();
    // **The idle lane never spends money.** Cloud tagging needs `drain_api_tags` (backoff, local
    // fallback, per-run cap) and that drain writes through `&mut Catalog`, so it belongs to the
    // writer's kick path — this slice runs off-writer and can only reach `compute_tags`, which
    // hands `OpenAiTagger` no image and would write empty tags. Beyond correctness it is the rule
    // AI-PIPELINE §5.1 wants anyway: a background lane must not quietly bill an API, so a cloud
    // pass is always an explicit, scoped, capped kick.
    if cfg.api_tagging_enabled && !cfg.api_key.is_empty() && cfg.api_model.is_some() {
        return None;
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
    // Resolved here, on the read connection: the compute phase below runs off the writer and has
    // no catalog to ask what a model wears (v1 plan S08).
    let slots = crate::drains::slot_textures_for(&jobs, |id| {
        cat.model_slot_texture_paths(id, workspace_dir.as_deref())
            .ok()
    });
    let done = crate::drains::compute_tags(
        jobs,
        tagger.as_ref(),
        zero_shot.as_ref(),
        workspace_dir.as_deref(),
        cache_dir.as_deref(),
        &slots,
    );
    Some(WriterMsg::TagsDone {
        producers: crate::drains::TagProducers {
            tagger: (tagger.producer_id(), tagger.model_id().to_string()),
            zero_shot: zero_shot
                .as_ref()
                .map(|z| (z.producer_id().to_string(), z.model_id().to_string())),
        },
        done,
    })
}

fn appraise_pass(
    catalog: &mut Catalog,
    ctx: &WriterCtx,
    embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    audio_embedder: Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    budget: std::time::Duration,
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
    if let Ok(f) = ctx.focus.lock() {
        catalog.set_appraise_focus(f.clone());
    }
    let progress = ctx.appraise.as_ref();
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(0, Ordering::Relaxed);
    progress.failed.store(0, Ordering::Relaxed);
    progress.cancelled.store(false, Ordering::Relaxed);
    let (cache_dir, workspace_dir) = (ctx.cache_dir.clone(), ctx.workspace_dir.clone());
    let proxy = ctx.config.read(|c| c.preview_quality.proxy_budget());
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
        let cfg = ctx.config.get();
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
                crate::drains::drain_api_tags(
                    catalog,
                    &cfg,
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
fn start_next_extract(catalog: &mut Catalog, queue: &mut VecDeque<Job>, ctx: &WriterCtx) {
    // Each failure is recorded as a failed job, and the registry publishes that itself. Each start
    // runs under the same supervision as any command, so a panicking one is a failed job too.
    drain_to_first_spawn(queue, &ctx.jobs, |op| {
        supervised(|| run(catalog, op, ctx))
            .map(|_| ())
            .map_err(|e| e.to_string())
    });
}

/// The queue invariant itself, with the writer's dependencies handed in.
///
/// Split out to be testable without a writer thread: taking `start` as a closure lets a test drive
/// the rule directly. (It was first split out because, in the desktop crate, any test that reached
/// [`run`] linked Tauri's window layer and died at load; in the engine the whole writer is
/// testable — see `tests_engine.rs`.)
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

fn run(catalog: &mut Catalog, op: Op, ctx: &WriterCtx) -> Result<WriteResult, String> {
    let jobs = &ctx.jobs;
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

        // Undo (UI-SPEC §2 / DESIGN-GUIDELINES §7 L5): apply the toggle and log it for Ctrl/Cmd-Z
        // in one transaction, recording only the ids whose state changed (review B13). Logging every
        // requested id made the undo of "trash {a, b}" restore an `a` that was already in the Trash.
        Op::SetTrashed { ids, trashed } => catalog
            .set_trashed_recorded(&ids, trashed)
            .map(|changed| WriteResult {
                changed,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::RemoveFromLibrary { targets } => remove_from_library(catalog, ctx, targets),

        Op::DeleteFromDisk {
            targets,
            roots,
            workspace,
        } => delete_from_disk(catalog, targets, &roots, workspace.as_deref()),

        Op::ReclaimWorkspace { outcome } => {
            let done =
                crate::storage::reclaim_on_writer(catalog, ctx.workspace_dir.as_deref(), jobs)?;
            let changed = done.removed;
            // The caller is waiting on this op's reply; a receiver that has gone changes nothing.
            let _ = outcome.send(done);
            Ok(WriteResult {
                changed,
                ..Default::default()
            })
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
            let (_, job) = jobs.open(label, "import", false);
            let result = (|| {
                crate::persist::persist_to_catalog(&ctx.db_path, &batches)
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
                // A rescan that found only a prefab edited changes no asset row, so nothing
                // queued would re-read it: re-read the scanned trees' prefab evidence here.
                let scopes: Vec<_> = batches
                    .iter()
                    .map(|b| crate::drains::PrefabScope {
                        root: PathBuf::from(&b.root),
                        absent_is_empty: false,
                    })
                    .collect();
                crate::drains::refresh_prefab_evidence(catalog, &scopes, false);
                Ok(WriteResult {
                    changed,
                    ..Default::default()
                })
            })();
            job.finish(result.is_ok(), false, changed);
            result
        }

        // Import-packages dialog (FEATURES §5.2): the discovery scan already read each
        // archive's manifest, so this records + mints from those locators — no second inflate.
        // One `package` row per selected pack, pinned and in-pack minted: the Library and its
        // cards exist immediately; identify runs off the drains in the background.
        Op::ImportPacks { root, packs } => {
            let label = format!("Import {} packs", packs.len());
            let (_, job) = jobs.open(label, "import", false);
            let result = (|| {
                let name = crate::config::stem_name(std::path::Path::new(&root));
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
            let inserted = result.as_ref().map(|r| r.changed).unwrap_or(0);
            job.finish(result.is_ok(), false, inserted);
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
            sync_source_exclude_to_config(catalog, ctx, source_id, &kind, &value, true)?;
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
            sync_source_exclude_to_config(catalog, ctx, source_id, &kind, &value, false)?;
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
            let (_, job) = jobs.open("Identify all".into(), "appraise", false);
            let embedder = resolve_embedder();
            let audio_embedder = resolve_audio_embedder();
            if embedder.is_none() && audio_embedder.is_none() {
                tracing::warn!("Identify all: no local embedder models — nothing can be appraised");
            }
            let result = appraise_pass(
                catalog,
                ctx,
                embedder,
                audio_embedder,
                std::time::Duration::from_secs(20),
                None,
            );
            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
            job.finish(result.is_ok(), false, changed);
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
            let (_, job) = jobs.open(format!("Identify asset {asset_id}"), "appraise", false);
            let embedder = resolve_embedder();
            let audio_embedder = resolve_audio_embedder();
            if embedder.is_none() && audio_embedder.is_none() {
                tracing::warn!("Identify asset {asset_id}: no local embedder models — flagged for a later pass");
            }
            let result = appraise_pass(
                catalog,
                ctx,
                embedder,
                audio_embedder,
                std::time::Duration::from_secs(20),
                None,
            );
            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
            job.finish(result.is_ok(), false, changed);
            result
        }

        Op::IdentifyIds { asset_ids } => {
            // ENGINE-API §2 `identify_asset`: flag an explicit set, then ONE bounded pass so
            // a bulk agent call is a single reconcile, not N kicks.
            for id in &asset_ids {
                catalog.mark_for_identify(*id).map_err(|e| e.to_string())?;
            }
            let (_, job) = jobs.open(
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
                ctx,
                embedder,
                audio_embedder,
                std::time::Duration::from_secs(20),
                Some(&asset_ids),
            );
            let changed = result.as_ref().map(|r| r.changed).unwrap_or(0);
            job.finish(result.is_ok(), false, changed);
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
            // unpacked.** `WriterHandle::run` waits once and drops the receiver, so the
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
            let workspace_dir = ctx
                .workspace_dir
                .clone()
                .ok_or_else(|| "no workspace directory configured".to_string())?;
            let entries = catalog
                .list_package_entries(package_id)
                .map_err(|e| e.to_string())?;
            if entries.is_empty() {
                // Phase 1: the manifest is unknown — read it on a worker (a big pack must
                // inflate its whole tar.gz; never on the writer thread). The kind is
                // "extract" so the one-at-a-time queue gates the whole flow.
                let (handles, job) = jobs.open(format!("Analyze {}", pkg.name), "extract", true);
                // Moved into the worker: it reports back when the work is done, and by itself —
                // as a failure — if the worker unwinds or the thread below never starts.
                let report_back = PackReport::new(&ctx.tx, job, package_id, true);
                let wcancel = handles.cancel;
                let wprogress = handles.progress.expect("manifest jobs carry progress");
                let wpath = PathBuf::from(&pkg.package_path);
                let wjobs = jobs.clone();
                let spawned = std::thread::Builder::new()
                    .name("manifest-worker".into())
                    .spawn(move || {
                        // Same guarantee as the extract worker: this report is the only thing that
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
                        report_back.manifest(outcome, cancelled);
                    });
                // A thread that could not start drops the closure, and with it the report —
                // which has already told the writer this job failed.
                spawned.map_err(|e| format!("the manifest worker could not start: {e}"))?;
            } else {
                // Locators already recorded (browse persistence) — mint + extract right away.
                mint_in_pack(catalog, package_id, &entries)?;
                spawn_extract_worker(&pkg, &workspace_dir, &entries, &ctx.tx, jobs)?;
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

        Op::RegisterProject {
            project_path,
            unity_version,
            pipeline,
        } => catalog
            .register_project(&project_path, unity_version.as_deref(), pipeline.as_deref())
            .map(|_| WriteResult {
                changed: 1,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::RecordImports {
            project_id,
            specs,
            batch_id,
        } => catalog
            .record_imports(project_id, &specs, &batch_id)
            .map(|outcomes| WriteResult {
                changed: outcomes
                    .iter()
                    .filter(|o| {
                        matches!(o, lodestash_core::catalog::ImportOutcome::Recorded { .. })
                    })
                    .count(),
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        Op::RecordExports {
            batch_id,
            dest_path,
            files,
        } => catalog
            .record_exports(&batch_id, &dest_path, &files)
            .map(|changed| WriteResult {
                changed,
                ..Default::default()
            })
            .map_err(|e| e.to_string()),

        // Batch undo opens its own connection on this thread (the same pattern as
        // persist_to_catalog): the single-writer rule is about which *thread* writes, and this
        // is still the writer — no other job can interleave with the delete.
        Op::UndoImportBatch { batch_id } => undo_import_batch(catalog, &batch_id),

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

        #[cfg(test)]
        Op::Probe(p) => {
            if let Some(started) = &p.started {
                let _ = started.send(());
            }
            let job = p.job.then(|| jobs.open("Probe".into(), "import", false).1);
            let mut changed = 0;
            if let Some(id) = p.favorite {
                changed = catalog
                    .set_favorite(&[id], true)
                    .map_err(|e| e.to_string())?;
            }
            std::thread::sleep(p.hold);
            let result = match p.end {
                ProbeEnd::Ok => Ok(WriteResult {
                    changed,
                    ..Default::default()
                }),
                ProbeEnd::Fail => Err("the probe failed on purpose".to_string()),
                ProbeEnd::Panic => panic!("a command panicked on purpose"),
            };
            if let Some(job) = job {
                job.finish(result.is_ok(), false, changed);
            }
            result
        }
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
    // The sandbox is only a fence while the roots are known. With none it admits every folder,
    // so a roots query that fails refuses the undo instead of running it unfenced.
    let roots = catalog.source_roots().map_err(|e| {
        format!("the library's source roots could not be read ({e}) — nothing was undone")
    })?;

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
        let Ok(dest) = crate::sandbox::check_dest(dest_path, &roots) else {
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

/// What batch undo may do about one recorded import file.
enum ProjectFile {
    /// Delete this — it resolves inside the project.
    Delete(PathBuf),
    /// Already gone: nothing to delete, and not a failure.
    Gone,
    /// Not a path inside the project. Never touched.
    Refused,
}

/// Fence one import record to its project (review finding 3): `dest_path` must be a plain relative
/// path, and the folder it names must **resolve** inside `project_root`.
///
/// The old fence asked `Path::is_absolute`, which on Windows is false for a root-relative
/// (`\Users\x`) or drive-relative (`C:f`) path — and `Path::join` then discards the base, so the
/// delete landed wherever the record pointed. Resolving the parent also stops a link inside the
/// project from carrying the delete out of it.
fn project_file(project_root: &Path, dest_path: &str) -> ProjectFile {
    if !lodestash_core::types::is_plain_relative_path(dest_path) {
        return ProjectFile::Refused;
    }
    // Component by component: `project_root` is canonical, and on Windows a canonical (`\\?\`)
    // path does not treat `/` as a separator, so a plain `join("Assets/x.png")` would name a
    // file that cannot exist.
    let mut abs = project_root.to_path_buf();
    for part in dest_path.split(['/', '\\']) {
        abs.push(part);
    }
    let (Some(parent), Some(name)) = (abs.parent(), abs.file_name()) else {
        return ProjectFile::Refused;
    };
    if !abs.starts_with(project_root) {
        return ProjectFile::Refused;
    }
    match parent.canonicalize() {
        Ok(real_parent) if real_parent.starts_with(project_root) => {
            ProjectFile::Delete(real_parent.join(name))
        }
        Ok(_) => ProjectFile::Refused,
        // The folder it was copied into is gone, so the file is.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectFile::Gone,
        Err(_) => ProjectFile::Refused,
    }
}

fn undo_import_batch(catalog: &Catalog, batch_id: &str) -> Result<WriteResult, String> {
    // Through the writer's own connection: this used to open a raw one, without the catalog's
    // pragmas or busy timeout (review R5).
    let files = catalog
        .import_batch_files(batch_id)
        .map_err(|e| e.to_string())?;

    // Delete the copied files first, while the paths are still known — the records go only
    // after the files, and only when none is left behind. A missing file counts as refused
    // (already gone), an error as failed.
    let mut failed = 0usize;
    let mut refused = 0usize;
    // Each project root resolved once. A root that does not resolve — the project moved, its
    // drive is unplugged — gives nothing to judge a path against: the undo is refused whole and
    // its records kept, rather than every file counted "already gone" and the batch forgotten.
    let mut roots: std::collections::HashMap<&str, PathBuf> = std::collections::HashMap::new();
    for (project_path, _) in &files {
        if !roots.contains_key(project_path.as_str()) {
            let real = Path::new(project_path).canonicalize().map_err(|e| {
                format!(
                    "the project folder {project_path} is not available ({e}) — nothing was undone"
                )
            })?;
            roots.insert(project_path, real);
        }
    }
    for (project_path, dest_path) in &files {
        match project_file(&roots[project_path.as_str()], dest_path) {
            ProjectFile::Refused => {
                tracing::warn!(%project_path, %dest_path, "import undo refused a path outside the project");
                refused += 1;
            }
            ProjectFile::Gone => refused += 1,
            ProjectFile::Delete(abs) => match std::fs::remove_file(&abs) {
                Ok(()) => {
                    // Unity's import sidecar, if it exists — leaving it would half-undo the import.
                    let _ = std::fs::remove_file(PathBuf::from(format!("{}.meta", abs.display())));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => refused += 1,
                Err(e) => {
                    tracing::warn!(path = %abs.display(), %e, "import undo could not delete a file");
                    failed += 1;
                }
            },
        }
    }

    // Forgotten only once the files are dealt with, like an export's: a batch left half-deleted
    // keeps its records, so it stays listed and can be undone again.
    let changed = if failed == 0 {
        catalog
            .forget_import_batch(batch_id)
            .map_err(|e| e.to_string())?
    } else {
        0
    };
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
    let classified = crate::persist::classify_in_pack_entries(entries);
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
    tx: &SyncSender<WriterMsg>,
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
    let (handles, job) = jobs.open(format!("Extract {}", pkg.name), "extract", true);
    // Moved into the worker: it reports back when the work is done, and by itself — as a
    // failure — if the worker unwinds or the thread below never starts.
    let report_back = PackReport::new(tx, job, pkg.id, false);
    let wcancel = handles.cancel;
    let wprogress = handles.progress.expect("extract jobs carry progress");
    let wpath = PathBuf::from(&pkg.package_path);
    let wworkspace = workspace_dir.to_path_buf();
    let wjobs = jobs.clone();
    std::thread::Builder::new()
        .name("extract-worker".into())
        .spawn(move || {
            // The report below is the **only** thing that runs `jobs.finish` and drains the
            // queue. A panic in the gzip/tar path (corrupt archive) would unwind a detached
            // thread, the report would never be made, `has_active_extract()` would stay true
            // forever, and every later extract would join a queue nobody drains — the same
            // session-long jam `start_next_extract` fixes for the error path, by a different road.
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
            report_back.extract(outcome, cancelled);
        })
        // A thread that could not start drops the closure, and with it the report — which has
        // already told the writer this job failed.
        .map_err(|e| format!("the extract worker could not start: {e}"))?;
    Ok(())
}

/// Materialize an extract worker's payloads (M14): fill `workspace_rel` on the minted rows
/// whose `package_entry.pathname` matched, then record the unpack. The rows hydrate from the
/// workspace from here on (extracted → in-pack → on-disk ladder, DATA-MODEL §7).
fn materialize_extract(
    catalog: &mut Catalog,
    ctx: &WriterCtx,
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
    let workspace = ctx.workspace_dir.clone();
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
    // The extraction landed prefabs beside the assets, and they are evidence about those assets.
    if let Some(ws) = workspace {
        crate::drains::refresh_prefab_evidence(
            catalog,
            &[crate::drains::PrefabScope {
                root: ws,
                absent_is_empty: true,
            }],
            false,
        );
    }
    Ok(WriteResult {
        changed,
        ..Default::default()
    })
}

/// Soft-delete the rows **and** blacklist each asset in its source.
///
/// The blacklist is what makes this different from Trash: the crawl takes the per-source
/// `Exclude` list, so an excluded asset is never re-offered by a scan. Without it a "removed"
/// asset reappears the next time the user indexes that folder, which reads as the app ignoring
/// them.
///
/// The exclude goes where the Settings blacklist editor writes one ([`Op::AddSourceExclude`]): the
/// source's own list in the catalog, mirrored into the open project's config, which is what the
/// crawl is handed today. It used to require an open project and fail without one — after the
/// rows were already trashed, so the error path skipped the reload and the asset stayed on screen
/// (review S3). Without a project the catalog list is written and the mirror has nothing to do.
fn remove_from_library(
    catalog: &Catalog,
    ctx: &WriterCtx,
    targets: Vec<Target>,
) -> Result<WriteResult, String> {
    let ids: Vec<i64> = targets.iter().map(|t| t.asset_id).collect();
    // Retired, not merely trashed: removing is not undoable, so the undo of an earlier trash
    // must not bring the asset back either (DATA-MODEL §9).
    let changed = catalog.retire_assets(&ids).map_err(|e| e.to_string())?;

    // What the open project's config is handed: `(source root, exclusion)` per removed file.
    let mut mirror: Vec<(String, Exclude)> = Vec::new();
    for t in &targets {
        let Some(root) = t.source_root.as_deref() else {
            continue; // in-pack asset: no source-relative file for the crawler to skip
        };
        let Some(source) = catalog
            .source_info_by_root(&root.to_string_lossy())
            .map_err(|e| e.to_string())?
        else {
            continue;
        };
        // Anchored, so it names this one path. A bare `rock.png` — what a file at the source
        // root used to be written as — also matches every `rock.png` in every folder
        // (`Exclude::matches`), and the next scan dropped them all (review finding 5).
        let value = Exclude::anchored(&t.rel_path);
        catalog
            .add_source_exclude(source.id, "asset", &value)
            .map_err(|e| e.to_string())?;
        mirror.push((
            source.root_path,
            Exclude {
                kind: ExcludeKind::Asset,
                value,
            },
        ));
    }
    // One config write for the whole selection (it is a whole-file save and a round of change
    // observers each time), and none when no project is open to mirror into.
    if !mirror.is_empty() && ctx.config.read(|c| c.last_project_name.is_some()) {
        ctx.config
            .update(|config| {
                let Some(project) = config.last_project_name.clone() else {
                    return;
                };
                for (root, exclude) in &mirror {
                    config.add_source_exclude(&project, Path::new(root), exclude.clone());
                }
            })
            .map_err(|e| e.to_string())?;
    }
    tracing::info!(
        rows = changed,
        excluded = mirror.len(),
        "removed from library"
    );
    Ok(WriteResult {
        changed,
        ..Default::default()
    })
}

/// Mirror a blacklist edit into the app config the crawler actually reads (UI-SPEC §11.1).
/// `import_source.blacklist` is catalog storage; `crawl` is fed from
/// `AppConfig.source_excludes(project, root)` — write both, or the Settings editor changes
/// nothing and the next scan resurrects every "removed" asset. The config write is a
/// [`ConfigStore::update`], so it cannot interleave with the web-prefs or settings writes; the
/// writer holds no other lock here, so there is no lock-order cycle.
fn sync_source_exclude_to_config(
    catalog: &Catalog,
    ctx: &WriterCtx,
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
    ctx.config
        .update(|config| {
            let Some(project) = config.last_project_name.clone() else {
                return;
            };
            if add {
                config.add_source_exclude(&project, std::path::Path::new(&root), exclude);
            } else {
                config.remove_source_exclude(&project, std::path::Path::new(&root), &exclude);
            }
        })
        .map_err(|e| e.to_string())
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
    if !path.is_file() || !crate::thumbs::under_known_root(path, roots, workspace) {
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
    // Retired, not merely trashed: the file is gone, so no earlier trash or restore of this
    // asset may be undone or redone back into a live row (DATA-MODEL §9).
    if !deleted.is_empty() {
        out.changed = catalog.retire_assets(&deleted).map_err(|e| e.to_string())?;
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

    /// The export undo's "never inside a library root" fence needs the roots. When they cannot
    /// be read, the undo refuses — it used to carry on with none, which is every folder allowed
    /// (review finding 1's audit).
    #[test]
    fn undoing_an_export_refuses_when_the_library_roots_cannot_be_read() {
        let dir = tempfile::tempdir().expect("tmp");
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).expect("mkdir lib");
        std::fs::write(lib.join("inside.fbx"), b"library file").expect("w");
        let db = dir.path().join("catalog.db");
        let mut c = Catalog::open(&db).expect("open");
        c.upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
            .expect("src");
        c.record_exports(
            "batch",
            &lib.to_string_lossy(),
            &[(None, "inside.fbx".to_string())],
        )
        .expect("record");
        // The roots query now fails; the export history still reads.
        rusqlite::Connection::open(&db)
            .expect("raw")
            .execute_batch("ALTER TABLE import_source RENAME TO import_source_gone;")
            .expect("break the roots query");
        assert!(
            c.source_roots().is_err(),
            "the fixture must break the query"
        );

        assert!(
            undo_export_batch(&mut c, "batch").is_err(),
            "the undo ran without knowing the library roots"
        );
        assert!(
            lib.join("inside.fbx").exists(),
            "a file inside a library root was deleted"
        );
        assert_eq!(
            c.export_batch_files("batch").expect("history").len(),
            1,
            "a refused undo keeps its record"
        );
    }

    /// A file catalog with one project registered at `project` and `dest_paths` recorded as the
    /// import batch `batch` — inserted raw, the way a build without the ack fence stored them.
    fn import_batch(dir: &Path, project: &Path, dest_paths: &[&str]) -> Catalog {
        let db = dir.join("catalog.db");
        let mut c = Catalog::open(&db).expect("open");
        c.seed_media_types().expect("seed");
        let src = c
            .upsert_import_source("Lib", &dir.join("lib").to_string_lossy(), "folder")
            .expect("source");
        let records: Vec<lodestash_core::catalog::CrawlRecord> = (0..dest_paths.len())
            .map(|i| lodestash_core::catalog::CrawlRecord {
                rel_path: format!("a{i}.png"),
                name: format!("a{i}.png"),
                media_type: "image".into(),
                size_bytes: 1,
                mtime: 1,
                workspace_rel: None,
                content_hash: None,
            })
            .collect();
        c.record_crawl(src, None, &records).expect("crawl");
        let project_id = c
            .register_project(&project.to_string_lossy(), None, None)
            .expect("project");
        let raw = rusqlite::Connection::open(&db).expect("raw");
        let assets: Vec<i64> = raw
            .prepare("SELECT id FROM asset ORDER BY id")
            .expect("prep")
            .query_map([], |r| r.get(0))
            .expect("ids")
            .collect::<Result<_, _>>()
            .expect("id rows");
        for (asset_id, dest) in assets.iter().zip(dest_paths) {
            raw.execute(
                "INSERT INTO import_record
                     (asset_id, project_id, dest_path, with_deps, imported_at, batch_id)
                     VALUES (?1, ?2, ?3, 1, 0, 'batch')",
                rusqlite::params![asset_id, project_id, dest],
            )
            .expect("import record");
        }
        c
    }

    /// `path` without its drive — `\Users\x\f` on Windows. `Path::is_absolute` is false for that
    /// there, yet `Path::join` lets it replace everything but the drive of what it is joined onto.
    fn root_relative(path: &Path) -> String {
        let s = path.to_string_lossy().into_owned();
        match s.split_once(':') {
            Some((_drive, rest)) if cfg!(windows) => rest.to_string(),
            _ => s,
        }
    }

    /// Batch undo deletes files, so every recorded path is fenced to the project (review finding
    /// 3). The old fence asked `Path::is_absolute`, which on Windows is false for a root-relative
    /// path — and the join then threw the project root away and deleted wherever it pointed.
    #[test]
    fn undoing_an_import_never_deletes_outside_the_project() {
        let dir = tempfile::tempdir().expect("tmp");
        let project = dir.path().join("proj");
        std::fs::create_dir_all(project.join("Assets")).expect("mkdir");
        std::fs::write(project.join("Assets").join("Good.png"), b"copy").expect("w");
        std::fs::write(project.join("Assets").join("Good.png.meta"), b"meta").expect("w");
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, b"do not touch").expect("w");

        let rooted = root_relative(&outside);
        let c = import_batch(
            dir.path(),
            &project,
            &[
                "Assets/Good.png",
                &rooted,
                "../outside.txt",
                "Assets/../../outside.txt",
            ],
        );
        let r = undo_import_batch(&c, "batch").expect("undo");
        assert!(
            outside.exists(),
            "undo deleted a file outside the project ({rooted:?})"
        );
        assert_eq!(r.refused, 3, "every path leaving the project is refused");
        assert_eq!(r.failed, 0);
        assert!(
            !project.join("Assets").join("Good.png").exists(),
            "the import's own copy was not deleted"
        );
        assert!(
            !project.join("Assets").join("Good.png.meta").exists(),
            "its Unity sidecar was left behind"
        );
        assert!(
            c.import_batch_files("batch").expect("records").is_empty(),
            "a finished undo left its records behind"
        );
        assert_eq!(r.changed, 4, "every record of the batch was forgotten");
    }

    /// A link inside the project must not carry the delete out of it: the folder a recorded file
    /// sits in has to *resolve* inside the project root, not merely be spelled under it.
    #[test]
    fn undoing_an_import_does_not_follow_a_link_out_of_the_project() {
        let dir = tempfile::tempdir().expect("tmp");
        let project = dir.path().join("proj");
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir_all(project.join("Assets")).expect("mkdir");
        std::fs::create_dir_all(&elsewhere).expect("mkdir");
        std::fs::write(elsewhere.join("keep.txt"), b"not the project's").expect("w");
        let link = project.join("Assets").join("Linked");
        #[cfg(windows)]
        {
            // A junction: a directory link that needs no privilege to create.
            let made = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(&link)
                .arg(&elsewhere)
                .output()
                .expect("mklink runs");
            assert!(made.status.success(), "junction: {made:?}");
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&elsewhere, &link).expect("symlink");
        assert!(link.join("keep.txt").exists(), "the link resolves");

        let c = import_batch(dir.path(), &project, &["Assets/Linked/keep.txt"]);
        let r = undo_import_batch(&c, "batch").expect("undo");
        assert!(
            elsewhere.join("keep.txt").exists(),
            "undo followed a link out of the project and deleted there"
        );
        assert_eq!((r.refused, r.failed), (1, 0));
    }

    /// The records go only once the files are dealt with. A batch with a file that could not be
    /// deleted keeps its records, so it stays listed and can be undone again — they used to be
    /// forgotten whatever happened, which the comment above the loop said could not happen.
    #[test]
    fn an_import_undo_that_could_not_finish_keeps_its_records() {
        let dir = tempfile::tempdir().expect("tmp");
        let project = dir.path().join("proj");
        // A directory where a file was recorded: `remove_file` fails on it, and not as "missing".
        std::fs::create_dir_all(project.join("Assets").join("Stuck.png")).expect("mkdir");
        std::fs::write(project.join("Assets").join("Good.png"), b"copy").expect("w");
        let c = import_batch(
            dir.path(),
            &project,
            &["Assets/Good.png", "Assets/Stuck.png"],
        );

        let r = undo_import_batch(&c, "batch").expect("undo");
        assert_eq!(r.failed, 1, "the stuck entry is reported as failed");
        assert!(!project.join("Assets").join("Good.png").exists());
        assert_eq!(r.changed, 0, "nothing is forgotten while a file is left");
        assert_eq!(
            c.import_batch_files("batch").expect("records").len(),
            2,
            "a half-finished undo lost track of what it left on disk"
        );

        // And with the project folder gone entirely there is nothing to judge the paths against:
        // refused as a whole, records kept — not "already gone, forget it".
        std::fs::create_dir_all(dir.path().join("second")).expect("mkdir");
        let gone = import_batch(
            &dir.path().join("second"),
            &dir.path().join("no-such-project"),
            &["Assets/Good.png"],
        );
        assert!(undo_import_batch(&gone, "batch").is_err());
        assert_eq!(gone.import_batch_files("batch").expect("records").len(), 1);
    }

    /// `n` queued `ExtractPackage` jobs. The reply receivers come back with them: dropping one
    /// makes its `send` fail, which would quietly mask a regression.
    fn queued_extracts(
        n: i64,
    ) -> (
        VecDeque<Job>,
        Vec<mpsc::Receiver<Result<WriteResult, WriteError>>>,
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
                    claim: Default::default(),
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

    /// The tray is its own domain: a job starting, moving or finishing publishes `jobs`, never
    /// `catalog` — the grid has no reason to re-read because a download began.
    #[test]
    fn the_job_registry_publishes_its_own_changes_and_only_those() {
        let jobs = JobRegistry::default();
        let h = jobs.start("Extract X".into(), "extract", true);
        let started = jobs.changes().take_pending();
        assert!(
            started.jobs && !started.catalog,
            "start is a jobs change: {started:?}"
        );
        jobs.progressed();
        jobs.finish(h.id, true, false, 3);
        let finished = jobs.changes().take_pending();
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

    /// A job opened through the registry ends whatever happens to the code that opened it
    /// (review finding 11): dropped without an outcome — a panic unwinding through it, an early
    /// `?` — it is finished as failed. It used to stay in the active list for the session, and
    /// one of kind `extract` made every later extraction queue behind a pack that would never
    /// finish.
    #[test]
    fn a_job_dropped_without_an_outcome_is_finished_as_failed() {
        let jobs = JobRegistry::default();
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (_handles, _job) = jobs.open("Extract X".into(), "extract", true);
            assert!(jobs.has_active_extract());
            panic!("between open and finish");
        }));
        assert!(unwound.is_err());
        assert!(
            !jobs.has_active_extract(),
            "a panicked extract job still blocks the queue"
        );
        let tray = jobs.snapshot();
        assert!(tray.active.is_empty());
        assert!(
            !tray.recent[0].ok,
            "an unwound job was recorded as a success"
        );

        // Reported, it ends as reported — and only once.
        let (_, job) = jobs.open("Import".into(), "import", false);
        job.finish(true, false, 7);
        let tray = jobs.snapshot();
        assert_eq!(tray.recent.len(), 2);
        assert!(tray.recent[0].ok && tray.recent[0].changed == 7);
    }

    /// A pack worker that dies still reports back: the report is what ends its job and lets the
    /// one-at-a-time queue move. Dropped unsent — the worker unwound, or its thread never
    /// started — it tells the writer the job failed; with no writer left, it ends the job itself.
    #[test]
    fn a_pack_worker_that_dies_still_reports_back() {
        let jobs = JobRegistry::default();
        let (tx, rx) = mpsc::sync_channel(4);

        let (_, job) = jobs.open("Extract X".into(), "extract", true);
        let job_id = job.id();
        let report = PackReport::new(&tx, job, 7, false);
        let died = std::thread::spawn(move || {
            let _report = report;
            panic!("the archive is corrupt in a way nobody caught");
        })
        .join();
        assert!(died.is_err());
        match rx.try_recv() {
            Ok(WriterMsg::ExtractDone {
                job_id: reported,
                package_id: 7,
                outcome: Err(_),
                cancelled: false,
            }) => assert_eq!(reported, job_id),
            _ => panic!("a dead extract worker reported nothing"),
        }
        assert!(
            jobs.has_active_extract(),
            "the writer ends the job when the report lands — not the worker"
        );

        // The analyse phase reports as a manifest failure.
        let (_, job) = jobs.open("Analyze Y".into(), "extract", true);
        drop(PackReport::new(&tx, job, 8, true));
        assert!(matches!(
            rx.try_recv(),
            Ok(WriterMsg::ManifestDone {
                package_id: 8,
                entries: Err(_),
                ..
            })
        ));

        // No writer left to hear it: the job is ended here, so nothing spins.
        let orphaned = JobRegistry::default();
        let (_, job) = orphaned.open("Extract Z".into(), "extract", true);
        let report = PackReport::new(&tx, job, 9, false);
        drop(rx);
        drop(report);
        assert!(!orphaned.has_active_extract());
        assert!(!orphaned.snapshot().recent[0].ok);
    }

    /// The watch rescan decides "did anything change?" by comparing what it crawls with what the
    /// catalog knows of the source (`known_sizes_for_source`), entry for entry. A file in the
    /// Trash is still on disk, so it is still crawled — and if the catalog side left it out, the
    /// two would never match and every rescan of a source with anything in the Trash would
    /// persist again, for ever. This drives the real rescan over a real folder: the consumer the
    /// core test `known_sizes_are_every_loose_file_of_the_source_trashed_or_not` exists for.
    #[test]
    fn a_watch_rescan_with_a_file_in_the_trash_is_not_a_change() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path().join("lib");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(root.join("a.png"), b"aaaa").expect("write a");
        std::fs::write(root.join("b.png"), b"bbbbbb").expect("write b");
        let root_str = root.to_string_lossy().into_owned();

        let db = dir.path().join("catalog.db");
        let mut catalog = Catalog::open(&db).expect("open");
        catalog
            .upsert_import_source("lib", &root_str, "folder")
            .expect("source");
        let (tx, _inbox) = mpsc::sync_channel(4);
        let ctx = WriterCtx {
            db_path: db.clone(),
            tx,
            jobs: JobRegistry::default(),
            appraise: Default::default(),
            focus: Default::default(),
            cache_dir: None,
            workspace_dir: None,
            config: Arc::new(ConfigStore::in_memory()),
        };

        assert!(
            rescan_watched(&mut catalog, &ctx),
            "the first sight of the folder is a change"
        );
        assert!(
            !rescan_watched(&mut catalog, &ctx),
            "nothing moved, so nothing is persisted"
        );

        let source = catalog
            .source_info_by_root(&root_str)
            .expect("query")
            .expect("source");
        let rows = catalog.entries_for_source(source.id).expect("rows");
        assert_eq!(rows.len(), 2, "both files were indexed");
        let a = rows
            .iter()
            .find(|r| r.rel_path == "a.png")
            .expect("a.png")
            .asset_id;
        catalog.set_trashed(&[a], true).expect("trash a");
        for pass in 0..2 {
            assert!(
                !rescan_watched(&mut catalog, &ctx),
                "pass {pass}: a file in the Trash made an unchanged folder look changed"
            );
        }

        std::fs::write(root.join("b.png"), b"a different size now").expect("edit b");
        assert!(
            rescan_watched(&mut catalog, &ctx),
            "a real edit is still a change"
        );
        assert!(
            !rescan_watched(&mut catalog, &ctx),
            "and it is recorded once"
        );
    }

    /// The embed and tag lanes run one slice at a time behind an in-flight flag that only the
    /// slice's result landing on the writer used to lower. A worker that panicked, had nothing
    /// to send, or found the writer gone must lower it too, or the lane never runs again.
    #[test]
    fn a_slice_that_never_reaches_the_writer_lowers_its_flag() {
        let flag = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(4);
        let up = || flag.load(Ordering::Relaxed);

        let panicked = spawn_slice(Inflight::raise(&flag), tx.clone(), || {
            panic!("the encoder blew up")
        })
        .join();
        assert!(panicked.is_err());
        assert!(!up(), "a panicked worker left its lane shut");

        spawn_slice(Inflight::raise(&flag), tx.clone(), || None)
            .join()
            .expect("join");
        assert!(!up(), "a slice with nothing to send left its lane shut");

        // Handed to the writer, the flag stays up: the writer lowers it when the result lands.
        let msg = || {
            Some(WriterMsg::EmbeddingsDone {
                model_id: "m".into(),
                done: Vec::new(),
            })
        };
        spawn_slice(Inflight::raise(&flag), tx.clone(), msg)
            .join()
            .expect("join");
        assert!(up(), "the flag fell before the writer stored the slice");
        assert!(matches!(
            rx.try_recv(),
            Ok(WriterMsg::EmbeddingsDone { .. })
        ));

        drop(rx);
        spawn_slice(Inflight::raise(&flag), tx, msg)
            .join()
            .expect("join");
        assert!(
            !up(),
            "a slice sent to a writer that has gone left its lane shut"
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
            claim: Default::default(),
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
