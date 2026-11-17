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
use crate::config::ConfigFile;
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
        let (cache, workspace) = crate::paths::default_cache_dirs();
        Some(Self {
            catalog: crate::paths::catalog_db_path()?,
            config_dir: crate::config::config_file_path()?.parent()?.to_path_buf(),
            cache_dir: cache?,
            workspace_dir: workspace?,
            os_keychain: true,
        })
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
    pub fn config_file(&self) -> ConfigFile {
        ConfigFile::at(self.config_dir.join("config.json")).with_keychain(self.os_keychain)
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

struct Inner {
    cfg: EngineConfig,
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
        let jobs = JobRegistry::default();
        let pump = Arc::new(AtomicBool::new(false));
        let appraise = Arc::new(JobProgress::default());
        let focus = Arc::new(Mutex::new(AppraiseFocus::default()));
        let writer = match take_lock(&cfg) {
            Ok(lock) => crate::writer::spawn(WriterSetup {
                db_path: cfg.catalog.clone(),
                cache_dir: Some(cfg.cache_dir.clone()),
                workspace_dir: Some(cfg.workspace_dir.clone()),
                config: cfg.config_file(),
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
            cfg.config_file(),
        );
        let (events, _) = tokio::sync::broadcast::channel(64);
        Engine {
            inner: Arc::new(Inner {
                cfg,
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
    /// reports `Failed { code, message }`, refuses every write with the message and reads empty.
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
        let model = Library::new(PathBuf::new(), None, None, cfg.config_file());
        let (events, _) = tokio::sync::broadcast::channel(64);
        Engine {
            inner: Arc::new(Inner {
                cfg,
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

    /// This engine's `config.json`.
    pub fn config_file(&self) -> ConfigFile {
        self.inner.cfg.config_file()
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
    pub fn read(&self) -> ReadModelGuard<'_> {
        let mut slot = match self.inner.read.lock() {
            Ok(slot) => slot,
            // A panic mid-read may have left the snapshot half-built: rebuild it.
            Err(poisoned) => {
                let mut slot = poisoned.into_inner();
                slot.built_at = None;
                slot
            }
        };
        if !slot.model.is_attached() {
            self.attach_reader(&mut slot);
        }
        // Read before the rebuild, so a write landing during it makes the next read rebuild again
        // rather than be mistaken for already seen.
        let generation = self.catalog_generation();
        if slot.model.is_attached() && slot.built_at != Some(generation) {
            slot.model.reload();
            slot.built_at = Some(generation);
        }
        ReadModelGuard { slot }
    }

    /// Rebuild the read model now, whatever the generation says — for a change the catalog
    /// generation does not see (a settings edit that moves the size tiers).
    pub fn reload(&self) {
        if let Ok(mut slot) = self.inner.read.lock() {
            slot.built_at = None;
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

    /// Open the read model's connection once the writer has the catalog ready.
    fn attach_reader(&self, slot: &mut ReadSlot) {
        if self.status() != EngineStatus::Ready {
            return;
        }
        match Catalog::open_reader(&self.inner.cfg.catalog) {
            Ok(reader) => {
                let generation = self.catalog_generation();
                slot.model.attach(reader);
                slot.built_at = Some(generation);
            }
            Err(e) => {
                // The writer has the catalog open and current, so this is a real fault — say so
                // rather than serve an empty library.
                tracing::error!(%e, "read model: catalog reader failed to open");
                self.inner.status.set(EngineStatus::Failed {
                    code: "catalog_unreadable".into(),
                    message: format!("the catalog opened for writing but not for reading: {e}"),
                });
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
