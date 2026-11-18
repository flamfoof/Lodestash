//! The [`Engine`] handle: one per catalog, the only code that opens it for writing (v2 spec §4,
//! ADR-0021).
//!
//! [`Engine::start`] never fails. It takes the catalog's lock, spawns the writer — which creates
//! the catalog when it is missing and migrates it when it is old — and returns at once; what
//! happened is [`Engine::status`]. A failure is a status the shell shows, never an empty library
//! (v2 spec §1.5, review S1).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use lodestash_core::catalog::{AppraiseFocus, Catalog};

use crate::changes::{ChangedEvent, PUBLISH_WINDOW};
use crate::config_store::ConfigStore;
use crate::jobs::JobProgress;
use crate::read_model::Library;
use crate::writer::{JobRegistry, WriterHandle, WriterSetup};

/// Every path one engine uses. The engine reads no process-wide location itself: the desktop
/// passes the OS defaults ([`Self::os_default`]), a test passes a temp dir ([`Self::under`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    /// The catalog file. Created when missing; its parent directory too.
    pub catalog: PathBuf,
    /// Where `config.json` lives.
    pub config_dir: PathBuf,
    /// The preview cache: thumbnails, viewer proxies, in-pack scratch.
    pub cache_dir: PathBuf,
    /// Extracted pack payloads (`asset_file.workspace_rel` resolves against it).
    pub workspace_dir: PathBuf,
    /// Keep the cloud API key in the OS credential store. Only the app's own config does; a
    /// private config keeps it in its file, so a test never reads or clears the user's key.
    pub os_keychain: bool,
}

impl EngineConfig {
    /// The app's own locations: `LODESTASH_CATALOG` or the data dir's catalog, the OS config dir,
    /// the OS cache dir. `None` when the OS has no such directories.
    pub fn os_default() -> Option<Self> {
        let dirs = crate::paths::AppDirs::os()?;
        Some(Self::for_catalog(crate::paths::catalog_db_path()?, &dirs))
    }

    /// `catalog` laid out against the app's directories: the shared config and preview cache,
    /// and a workspace that is **this catalog's alone** ([`crate::paths::workspace_for`]) — the
    /// default catalog's is where it has always been, any other catalog's sits beside it.
    pub fn for_catalog(catalog: PathBuf, dirs: &crate::paths::AppDirs) -> Self {
        Self {
            workspace_dir: crate::paths::workspace_for(&catalog, dirs),
            catalog,
            config_dir: dirs.config_dir.clone(),
            cache_dir: dirs.cache_dir.clone(),
            os_keychain: true,
        }
    }

    /// Everything under one directory, the credential store untouched — a test, or a library kept
    /// apart from the app's own.
    pub fn under(root: &Path) -> Self {
        Self {
            catalog: root.join("catalog.db"),
            config_dir: root.join("config"),
            cache_dir: root.join("cache").join("thumbs"),
            workspace_dir: root.join("cache").join("workspace"),
            os_keychain: false,
        }
    }

    /// This engine's `config.json`.
    pub fn config_path(&self) -> PathBuf {
        self.config_dir.join("config.json")
    }

    /// Open this engine's config. The engine opens it once, at start, and every read and write of
    /// it goes through that store ([`Engine::config_store`]); a test opens one here to seed the file
    /// before the engine starts.
    pub fn open_config(&self) -> ConfigStore {
        ConfigStore::open(self.config_path(), self.os_keychain)
    }

    /// The exclusive lock file beside the catalog (v2 spec §4.2).
    pub fn lock_path(&self) -> PathBuf {
        let mut name = self.catalog.as_os_str().to_owned();
        name.push(".lock");
        PathBuf::from(name)
    }
}

/// What the engine is doing (v2 spec §4.2) — what `GET /v1/status` will answer and what the shell
/// shows instead of a library while it is not `Ready`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EngineStatus {
    Starting,
    Migrating {
        from: u32,
        to: u32,
    },
    Ready,
    /// `code` is stable (`catalog_locked`, `schema_too_new`, `catalog_open_failed`,
    /// `catalog_unreadable`, `no_catalog_location`); `message` says what to do.
    Failed {
        code: String,
        message: String,
    },
}

impl EngineStatus {
    /// `Ready` or `Failed` — nothing more will happen on its own.
    pub fn is_settled(&self) -> bool {
        matches!(self, EngineStatus::Ready | EngineStatus::Failed { .. })
    }
}

/// The status and what the user should be told about it — the body `GET /v1/status` will answer
/// (v2 spec §4.2, §4.5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct EngineReport {
    #[serde(flatten)]
    pub status: EngineStatus,
    /// Set when `config.json` did not parse at start: where the bad file was moved. The engine ran
    /// on defaults; reported until [`Engine::dismiss_config_recovered`] (review B12).
    pub config_recovered: Option<PathBuf>,
    /// Set while `config.json` is there and could not be loaded, with why. The engine runs on a
    /// fallback that switches nothing on, the file is left as it is, and settings are not saved
    /// until it loads ([`crate::ConfigStore::unreadable`]).
    pub config_unreadable: Option<String>,
}

/// The status, shared with the writer thread that moves it.
pub(crate) struct StatusCell {
    status: Mutex<EngineStatus>,
    changed: Condvar,
}

impl StatusCell {
    fn new(status: EngineStatus) -> Self {
        Self {
            status: Mutex::new(status),
            changed: Condvar::new(),
        }
    }

    pub(crate) fn set(&self, status: EngineStatus) {
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = status;
        self.changed.notify_all();
    }

    fn get(&self) -> EngineStatus {
        self.status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
}

/// The read model and the catalog generation it was built at.
struct ReadSlot {
    model: Library,
    built_at: Option<u64>,
}

/// A read of the snapshot, current as of the catalog generation it carries (v2 spec §4.3).
pub struct ReadModelGuard<'a> {
    slot: MutexGuard<'a, ReadSlot>,
}

impl ReadModelGuard<'_> {
    /// The catalog generation this read reflects — what `X-Lodestash-Gen` will carry.
    pub fn generation(&self) -> u64 {
        self.slot.built_at.unwrap_or(0)
    }
}

impl std::ops::Deref for ReadModelGuard<'_> {
    type Target = Library;
    fn deref(&self) -> &Library {
        &self.slot.model
    }
}

/// Why the library cannot be read right now (v2 spec §4.2–§4.3).
///
/// A read used to answer in every state — an empty library while the engine was starting or had
/// failed, the last snapshot when a rebuild failed — so "no assets" and "I could not look" were
/// the same answer, and the callers that *delete by what a read says is unreferenced* could not
/// tell them apart (integrity review, findings 1, 8 and 9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The engine is not `Ready`: still starting or migrating, or it failed. The status says
    /// which, and for a failure carries the code and what to do.
    NotReady(EngineStatus),
    /// The catalog is open but its snapshot could not be rebuilt. Nothing stale is served, the
    /// failed build is not stamped current, and the next read tries again.
    Rebuild(String),
}

impl ReadError {
    /// The stable kind an adapter maps (`lodestash_protocol::codes`).
    pub fn code(&self) -> &'static str {
        match self {
            ReadError::NotReady(_) => lodestash_protocol::codes::UNAVAILABLE,
            ReadError::Rebuild(_) => lodestash_protocol::codes::INTERNAL,
        }
    }
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::NotReady(EngineStatus::Failed { code, message }) => {
                write!(f, "the library is not open ({code}): {message}")
            }
            ReadError::NotReady(EngineStatus::Migrating { from, to }) => write!(
                f,
                "the catalog is being upgraded (schema {from} to {to}) — the library opens when \
                 that finishes"
            ),
            ReadError::NotReady(_) => f.write_str("the library is still opening"),
            ReadError::Rebuild(why) => write!(f, "the library could not be read: {why}"),
        }
    }
}

impl std::error::Error for ReadError {}

struct Inner {
    cfg: EngineConfig,
    config: Arc<ConfigStore>,
    status: Arc<StatusCell>,
    writer: WriterHandle,
    read: Mutex<ReadSlot>,
    jobs: JobRegistry,
    pump: Arc<AtomicBool>,
    appraise: Arc<JobProgress>,
    focus: Arc<Mutex<AppraiseFocus>>,
    events: tokio::sync::broadcast::Sender<ChangedEvent>,
    publisher: Once,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // The writer finishes the command in hand, closes the catalog and releases the lock.
        self.writer.stop();
    }
}

/// The engine: the catalog's one owner. Cheap to clone; every clone is the same engine.
#[derive(Clone)]
pub struct Engine {
    inner: Arc<Inner>,
}

impl Engine {
    /// Start an engine on `cfg`. Never fails: a failure is [`EngineStatus::Failed`].
    ///
    /// Synchronously: create the catalog's directory and take `<catalog>.lock`
    /// (`File::try_lock`), so a second engine on the same catalog fails fast with
    /// `catalog_locked` instead of migrating it concurrently. Then the writer thread opens —
    /// creating or migrating — the catalog and reports `Ready`; [`Self::wait_settled`] waits for it.
    pub fn start(cfg: EngineConfig) -> Engine {
        let status = Arc::new(StatusCell::new(EngineStatus::Starting));
        // The lock first: only the engine that owns the catalog may write the config. A second
        // instance that was refused reads `config.json` but cannot save it — its copy goes stale
        // as the owner changes settings, and one save from it would put the stale copy back
        // (ADR-0024).
        let lock = take_lock(&cfg);
        // Opened once: the writer, the read model and the shell all read this one.
        let config = Arc::new(match &lock {
            Ok(_) => cfg.open_config(),
            Err(_) => ConfigStore::open_read_only(
                cfg.config_path(),
                cfg.os_keychain,
                "this Lodestash did not get the catalog, so it does not save settings — the \
                 Lodestash that has it does",
            ),
        });
        if let Some(moved) = config.recovered() {
            tracing::error!(to = %moved.display(), "config.json did not parse; running on defaults");
        }
        if let Some(why) = config.unreadable() {
            tracing::error!(%why, "config.json could not be loaded; it is left as it is and nothing is saved over it");
        }
        let jobs = JobRegistry::default();
        let pump = Arc::new(AtomicBool::new(false));
        let appraise = Arc::new(JobProgress::default());
        let focus = Arc::new(Mutex::new(AppraiseFocus::default()));
        let writer = match lock {
            Ok(lock) => crate::writer::spawn(WriterSetup {
                db_path: cfg.catalog.clone(),
                cache_dir: Some(cfg.cache_dir.clone()),
                workspace_dir: Some(cfg.workspace_dir.clone()),
                config: config.clone(),
                lock,
                status: status.clone(),
                jobs: jobs.clone(),
                pump: pump.clone(),
                appraise: appraise.clone(),
                focus: focus.clone(),
            }),
            Err(failed) => {
                let why = match &failed {
                    EngineStatus::Failed { message, .. } => message.clone(),
                    _ => String::new(),
                };
                tracing::error!(%why, "engine not started");
                status.set(failed);
                WriterHandle::absent(why)
            }
        };
        let model = Library::new(
            cfg.catalog.clone(),
            Some(cfg.cache_dir.clone()),
            Some(cfg.workspace_dir.clone()),
            config.clone(),
        );
        let (events, _) = tokio::sync::broadcast::channel(64);
        Engine {
            inner: Arc::new(Inner {
                cfg,
                config,
                status,
                writer,
                read: Mutex::new(ReadSlot {
                    model,
                    built_at: None,
                }),
                jobs,
                pump,
                appraise,
                focus,
                events,
                publisher: Once::new(),
            }),
        }
    }

    /// An engine that could not even be configured — no catalog location on this system. It
    /// reports `Failed { code, message }`, and refuses every write and every read with it.
    pub fn failed(code: &str, message: &str) -> Engine {
        let status = Arc::new(StatusCell::new(EngineStatus::Failed {
            code: code.into(),
            message: message.into(),
        }));
        let cfg = EngineConfig {
            catalog: PathBuf::new(),
            config_dir: PathBuf::new(),
            cache_dir: PathBuf::new(),
            workspace_dir: PathBuf::new(),
            os_keychain: false,
        };
        // Nowhere to keep a config either: defaults, held in memory.
        let config = Arc::new(ConfigStore::in_memory());
        let model = Library::new(PathBuf::new(), None, None, config.clone());
        let (events, _) = tokio::sync::broadcast::channel(64);
        Engine {
            inner: Arc::new(Inner {
                cfg,
                config,
                status,
                writer: WriterHandle::absent(message),
                read: Mutex::new(ReadSlot {
                    model,
                    built_at: None,
                }),
                jobs: JobRegistry::default(),
                pump: Arc::new(AtomicBool::new(false)),
                appraise: Arc::new(JobProgress::default()),
                focus: Arc::new(Mutex::new(AppraiseFocus::default())),
                events,
                publisher: Once::new(),
            }),
        }
    }

    /// What the engine is doing now.
    pub fn status(&self) -> EngineStatus {
        self.inner.status.get()
    }

    /// The status, with what the user still has to be told (a recovered config).
    pub fn report(&self) -> EngineReport {
        EngineReport {
            status: self.status(),
            config_recovered: self.config_recovered(),
            config_unreadable: self.inner.config.unreadable(),
        }
    }

    /// Where a `config.json` that failed to parse at start was moved, until dismissed (B12).
    pub fn config_recovered(&self) -> Option<PathBuf> {
        self.inner.config.recovered()
    }

    /// The user has seen that their config was reset.
    pub fn dismiss_config_recovered(&self) {
        self.inner.config.dismiss_recovered();
    }

    /// Wait until the engine is `Ready` or `Failed`, at most `timeout`, and say which.
    pub fn wait_settled(&self, timeout: Duration) -> EngineStatus {
        let deadline = Instant::now() + timeout;
        let cell = &self.inner.status;
        let mut status = cell.status.lock().unwrap_or_else(|p| p.into_inner());
        while !status.is_settled() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            status = cell
                .changed
                .wait_timeout(status, left)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        status.clone()
    }

    /// The paths this engine was started with.
    pub fn config(&self) -> &EngineConfig {
        &self.inner.cfg
    }

    /// This engine's configuration — the only way to read or write its `config.json`.
    pub fn config_store(&self) -> &Arc<ConfigStore> {
        &self.inner.config
    }

    /// Every committed change, coalesced (ADR-0016). The publisher starts with the first
    /// subscriber; until then marks only accumulate, so a headless engine emits nothing.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<ChangedEvent> {
        let rx = self.inner.events.subscribe();
        self.inner.publisher.call_once(|| {
            let tx = self.inner.events.clone();
            self.inner
                .jobs
                .changes()
                .spawn_publisher(PUBLISH_WINDOW, move |event| {
                    // No subscriber left is not an error: the event is simply not wanted.
                    let _ = tx.send(event);
                });
        });
        rx
    }

    /// The writer: typed commands, replies bounded by a timeout.
    pub fn writer(&self) -> &WriterHandle {
        &self.inner.writer
    }

    /// The read model, rebuilt first if the catalog generation moved since it was built — so a
    /// write by anyone (the user, an agent, the background lane) shows on the next read without a
    /// refresh (v2 spec §4.3).
    ///
    /// An `Err` — never an empty or stale library — when the engine is not `Ready` or the
    /// snapshot could not be rebuilt ([`ReadError`]). A rebuild that fails is **not stamped**:
    /// the generation it was for stays unseen, so the next read tries again.
    pub fn read(&self) -> Result<ReadModelGuard<'_>, ReadError> {
        let mut slot = self.read_slot();
        let status = self.status();
        if status != EngineStatus::Ready {
            return Err(ReadError::NotReady(status));
        }
        if !slot.model.is_attached() {
            self.attach_reader(&mut slot)?;
        }
        // Read before the rebuild, so a write landing during it makes the next read rebuild again
        // rather than be mistaken for already seen.
        let generation = self.catalog_generation();
        if slot.built_at != Some(generation) {
            if let Err(why) = slot.model.reload() {
                tracing::error!(%why, generation, "read model: rebuild failed; not serving it");
                slot.built_at = None;
                return Err(ReadError::Rebuild(why));
            }
            slot.built_at = Some(generation);
        }
        Ok(ReadModelGuard { slot })
    }

    /// Rebuild the read model on the next read, whatever the generation says — for a change the
    /// catalog generation does not see (a settings edit that moves the size tiers).
    pub fn reload(&self) {
        self.read_slot().built_at = None;
    }

    /// The read slot, whatever happened to the last thread that held it.
    ///
    /// A panic while the slot was held poisons its mutex, and may have left the snapshot
    /// half-built — so it is rebuilt **once**, and the poison is cleared. It used to stay set:
    /// every later read then took this path and rebuilt the whole library again, and
    /// [`Self::reload`], which gave up on a poisoned lock, never ran at all (review finding 10).
    fn read_slot(&self) -> MutexGuard<'_, ReadSlot> {
        match self.inner.read.lock() {
            Ok(slot) => slot,
            Err(poisoned) => {
                let mut slot = poisoned.into_inner();
                slot.built_at = None;
                self.inner.read.clear_poison();
                slot
            }
        }
    }

    /// The catalog generation now (ADR-0016): it climbs once per committed change.
    pub fn catalog_generation(&self) -> u64 {
        self.inner.jobs.changes().catalog_generation()
    }

    /// The job tray (B2b).
    pub fn jobs(&self) -> &JobRegistry {
        &self.inner.jobs
    }

    /// True while the writer runs an appraise pass (B11d).
    pub fn pump_active(&self) -> bool {
        self.inner.pump.load(Ordering::Relaxed)
    }

    /// The running appraise pass's counters (B11d).
    pub fn appraise_progress(&self) -> &JobProgress {
        &self.inner.appraise
    }

    /// Where the user is looking (UI-SPEC §3): the next appraise pass takes this order. Never a
    /// writer command — browsing must not queue behind a running pass.
    pub fn set_appraise_focus(&self, focus: AppraiseFocus) {
        if let Ok(mut f) = self.inner.focus.lock() {
            *f = focus;
        }
    }

    /// Open the read model's connection once the writer has the catalog ready. The snapshot is
    /// built — and stamped — by the read that asked for this.
    fn attach_reader(&self, slot: &mut ReadSlot) -> Result<(), ReadError> {
        match Catalog::open_reader(&self.inner.cfg.catalog) {
            Ok(reader) => {
                slot.model.attach(reader);
                slot.built_at = None;
                Ok(())
            }
            Err(e) => {
                // The writer has the catalog open and current, so this is a real fault — say so
                // rather than serve an empty library.
                tracing::error!(%e, "read model: catalog reader failed to open");
                let failed = EngineStatus::Failed {
                    code: "catalog_unreadable".into(),
                    message: format!("the catalog opened for writing but not for reading: {e}"),
                };
                self.inner.status.set(failed.clone());
                Err(ReadError::NotReady(failed))
            }
        }
    }
}

/// Create the catalog's directory and take its exclusive lock. The lock is held by the writer
/// thread for its life: a second engine on this catalog is told so instead of migrating it.
fn take_lock(cfg: &EngineConfig) -> Result<std::fs::File, EngineStatus> {
    let fail = |code: &str, message: String| EngineStatus::Failed {
        code: code.into(),
        message,
    };
    if let Some(dir) = cfg.catalog.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| {
            fail(
                "catalog_open_failed",
                format!("could not create the catalog folder {}: {e}", dir.display()),
            )
        })?;
    }
    let path = cfg.lock_path();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| {
            fail(
                "catalog_open_failed",
                format!("could not open the catalog lock {}: {e}", path.display()),
            )
        })?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(fail(
            "catalog_locked",
            format!(
                "another Lodestash is already using the catalog at {} — close it, or point this \
                 one at a different catalog",
                cfg.catalog.display()
            ),
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(fail(
            "catalog_open_failed",
            format!(
                "could not lock the catalog at {}: {e}",
                cfg.catalog.display()
            ),
        )),
    }
}
