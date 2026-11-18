//! The engine's one `config.json` (v2 spec §4.5): every read and write of the app's configuration
//! goes through a [`ConfigStore`].
//!
//! - **One copy, one lock.** The store loads the file once and serves reads from memory, so a read
//!   never touches the disk — and never writes it (review U8: reading the settings used to save the
//!   file, API key and all). [`ConfigStore::update`] runs load-modify-save under the same lock, so
//!   two writers cannot lose each other's change.
//! - **Atomic writes.** A save writes a temp file beside the config, `sync_all`s it and renames it
//!   over the old one: a crash mid-save leaves the previous file, never a truncated one.
//! - **Corruption is reported, not overwritten** (review B12). A file that was read and does not
//!   parse is moved aside to `config.json.corrupt-<unix-ts>`, the store runs on defaults, and
//!   [`ConfigStore::recovered`] names the quarantined copy until the user dismisses it. The old
//!   loader read a truncated file as defaults and the next save made the loss permanent.
//! - **A file that was not read is never written over** (ADR-0024). A config that is there and
//!   cannot be read — held open by another program, access denied — is not corrupt, and one that
//!   is corrupt but cannot be moved aside is still the only copy. Either way the store is
//!   *blocked*: it holds a fallback that switches nothing on ([`ConfigStore::unreadable`] says
//!   why), and every [`ConfigStore::update`] is refused until the file loads.
//! - **Only the instance that owns the catalog writes.** A second Lodestash that was refused the
//!   catalog gets [`ConfigStore::open_read_only`]: it reads the config and cannot save its stale
//!   copy over the first instance's changes.
//! - **The cloud API key stays server-side.** In the app's own config it lives in the OS credential
//!   store and the file keeps it blank (AI-PIPELINE §5.1); a private config (a test's) keeps it in
//!   the file and never touches the credential store. [`ConfigStore::settings`] reports only whether
//!   a key is configured. A save that fails puts the credential store back as it was.
//! - **Changes apply live, in order.** [`ConfigStore::on_change`] observers are told of every
//!   committed update, in commit order: the agent planes follow the allow-list and byte release
//!   that way, and every pass reads the egress consent from the store when it starts.
//!
//! Tradeoff: the file is read once. An edit made to `config.json` by hand while the app runs is not
//! seen, and the next save replaces it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::ThreadId;
use std::time::Duration;

/// The Settings DTO (`lodestash-protocol`), which this store reads and writes by section.
pub use lodestash_protocol::dto::Settings;
use lodestash_protocol::dto::{
    AgentSettings, AppearanceSettings, CloudSettings, IdentifySettings, LibrarySettings,
    PreviewQuality as WireQuality, StorageSettings,
};

use crate::config::{AppConfig, PreviewQuality, SizeTiers};

/// Why an update did not land. The in-memory config is unchanged when one is returned — except
/// by [`ConfigError::Reloaded`], which says the store has just taken the file's config instead.
#[derive(Debug)]
pub enum ConfigError {
    /// The new config could not be written to disk.
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The config could not be encoded.
    Encode(serde_json::Error),
    /// The config file is there and could not be loaded: unreadable, or corrupt and impossible to
    /// move aside. Nothing is written over a file that was not read.
    Unreadable { path: PathBuf, reason: String },
    /// The file could not be loaded when the store opened, and now has been: the store holds its
    /// config from here on. This update was decided while looking at the fallback, so it was not
    /// applied — read the settings again and repeat it.
    Reloaded { path: PathBuf },
    /// This store never writes: another Lodestash owns the config.
    ReadOnly { reason: String },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Write { path, source } => {
                write!(f, "could not save {}: {source}", path.display())
            }
            ConfigError::Encode(e) => write!(f, "could not encode the config: {e}"),
            ConfigError::Unreadable { path, reason } => write!(
                f,
                "nothing was saved: {} is there but could not be loaded ({reason}), and it will \
                 not be written over",
                path.display()
            ),
            ConfigError::Reloaded { path } => write!(
                f,
                "that change was not saved: {} could not be loaded when Lodestash started and \
                 has only now been read — check the settings and make the change again",
                path.display()
            ),
            ConfigError::ReadOnly { reason } => write!(f, "nothing was saved: {reason}"),
        }
    }
}

impl std::error::Error for ConfigError {}

type Observer = Arc<dyn Fn(&AppConfig) + Send + Sync>;

/// Where the cloud API key rests outside the config file. The app's own config uses the OS
/// credential store ([`OsKeychain`]); a test hands in its own.
trait SecretStore: Send + Sync {
    fn load(&self) -> Option<String>;
    /// Store the key; `true` only when it was written.
    fn store(&self, key: &str) -> bool;
    fn clear(&self);
}

/// The config as last committed, and where its API key lives.
struct Current {
    config: AppConfig,
    /// The key is in the credential store and the file holds it blank.
    key_in_keychain: bool,
    /// Counts commits. Observers are told in this order.
    version: u64,
    /// Why the file could not be loaded. While set, `config` is the [`fallback`] and nothing is
    /// written to the file.
    blocked: Option<String>,
}

/// What observers have been told, and what they have yet to be.
#[derive(Default)]
struct Delivery {
    /// The newest committed config not yet told. An older one waiting here is replaced: an
    /// observer wants the current config, not every step to it.
    pending: Option<(u64, AppConfig)>,
    /// The newest version handed to observers.
    told: u64,
    /// The thread telling observers now. Only one does at a time, which is what keeps the order.
    telling: Option<ThreadId>,
}

/// The engine's configuration: loaded once, read from memory, written atomically under one lock.
pub struct ConfigStore {
    /// `None` for a store with nowhere to persist (an engine with no config location): updates
    /// apply in memory only.
    path: Option<PathBuf>,
    /// Where the cloud API key is kept; `None` keeps it in the file.
    secrets: Option<Box<dyn SecretStore>>,
    /// Set for a store that must never write, with the reason an update is answered with.
    read_only: Option<String>,
    current: Mutex<Current>,
    recovered: Mutex<Option<PathBuf>>,
    observers: Mutex<Vec<Observer>>,
    delivery: Mutex<Delivery>,
    /// Signalled when a thread has finished telling observers.
    told: Condvar,
}

impl ConfigStore {
    /// Open the config at `path`. Never fails: a missing file is the defaults; a file that does not
    /// parse is quarantined and reported ([`Self::recovered`]); a file that cannot be read leaves
    /// the store blocked ([`Self::unreadable`]). Nothing is written until the first
    /// [`Self::update`].
    pub fn open(path: impl Into<PathBuf>, keychain: bool) -> Self {
        let secrets = keychain.then(|| Box::new(OsKeychain) as Box<dyn SecretStore>);
        Self::open_with(path.into(), secrets, None)
    }

    /// Open the config at `path` for reading only: every [`Self::update`] answers
    /// [`ConfigError::ReadOnly`] with `reason`, and the file is never touched — not saved, and not
    /// moved aside if it does not parse. For a Lodestash that does not own the catalog (a second
    /// instance): its copy of the config goes stale as the owner changes settings, and saving it
    /// would undo them.
    pub fn open_read_only(
        path: impl Into<PathBuf>,
        keychain: bool,
        reason: impl Into<String>,
    ) -> Self {
        let secrets = keychain.then(|| Box::new(OsKeychain) as Box<dyn SecretStore>);
        Self::open_with(path.into(), secrets, Some(reason.into()))
    }

    fn open_with(
        path: PathBuf,
        secrets: Option<Box<dyn SecretStore>>,
        read_only: Option<String>,
    ) -> Self {
        let loaded = if read_only.is_some() {
            load(&path, false)
        } else {
            load_patiently(&path)
        };
        let (config, recovered, blocked) = match loaded {
            Loaded::Config(config) => (*config, None, None),
            Loaded::Quarantined(to) => (AppConfig::default(), Some(to), None),
            Loaded::Blocked(reason) => (fallback(), None, Some(reason)),
        };
        let (config, key_in_keychain) = match (&secrets, &blocked) {
            (Some(secrets), None) => with_stored_key(config, secrets.as_ref()),
            _ => (config, false),
        };
        Self {
            path: Some(path),
            secrets,
            read_only,
            current: Mutex::new(Current {
                config,
                key_in_keychain,
                version: 0,
                blocked,
            }),
            recovered: Mutex::new(recovered),
            observers: Mutex::new(Vec::new()),
            delivery: Mutex::new(Delivery::default()),
            told: Condvar::new(),
        }
    }

    /// A store with no file behind it: defaults, and updates that live as long as the store.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            secrets: None,
            read_only: None,
            current: Mutex::new(Current {
                config: AppConfig::default(),
                key_in_keychain: false,
                version: 0,
                blocked: None,
            }),
            recovered: Mutex::new(None),
            observers: Mutex::new(Vec::new()),
            delivery: Mutex::new(Delivery::default()),
            told: Condvar::new(),
        }
    }

    /// The file this store persists to, if any.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    fn lock(&self) -> MutexGuard<'_, Current> {
        // A panic inside an update leaves the last committed config in place (it is replaced only
        // after a successful write), so a poisoned lock still guards a consistent value.
        self.current.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// A copy of the whole config, API key included — for engine code that needs it. Never sent to
    /// a client: [`Self::settings`] is the client's view.
    pub fn get(&self) -> AppConfig {
        self.lock().config.clone()
    }

    /// Read part of the config without copying the rest — the hot paths (one thumbnail, one card)
    /// want one field.
    pub fn read<T>(&self, f: impl FnOnce(&AppConfig) -> T) -> T {
        f(&self.lock().config)
    }

    /// Change the config and save it, atomically, under the store's one lock. `f` works on a copy:
    /// when the save fails the change is dropped and the error returned, so memory, disk and the
    /// credential store never disagree. Observers ([`Self::on_change`]) are told after the lock is
    /// released, in commit order.
    ///
    /// Refused without calling `f` by a read-only store, and by one whose file could not be
    /// loaded: that one tries the file again first, and if it loads now takes its config and
    /// answers [`ConfigError::Reloaded`] — `f` was the caller's answer to a config they were not
    /// actually looking at.
    pub fn update<T>(&self, f: impl FnOnce(&mut AppConfig) -> T) -> Result<T, ConfigError> {
        if let Some(reason) = &self.read_only {
            return Err(ConfigError::ReadOnly {
                reason: reason.clone(),
            });
        }
        let mut current = self.lock();
        if current.blocked.is_some() {
            return Err(self.reload(current));
        }
        let mut next = current.config.clone();
        let out = f(&mut next);
        let key_in_keychain = match &self.path {
            Some(path) => {
                let placed = self.place_key(&current, &next);
                if let Err(e) = write_config(path, &next, placed.in_keychain) {
                    // The save did not happen, so neither did the key change.
                    if placed.changed {
                        self.restore_key(&current);
                    }
                    return Err(e);
                }
                placed.in_keychain
            }
            None => false,
        };
        current.config = next;
        current.key_in_keychain = key_in_keychain;
        current.version += 1;
        let (version, snapshot) = (current.version, current.config.clone());
        drop(current);
        self.deliver(version, snapshot);
        Ok(out)
    }

    /// Try the file of a blocked store again. If it loads, the store takes its config (and tells
    /// observers); either way the update that asked is refused.
    fn reload(&self, mut current: MutexGuard<'_, Current>) -> ConfigError {
        let Some(path) = self.path.clone() else {
            // A store with no file is never blocked.
            current.blocked = None;
            return ConfigError::ReadOnly {
                reason: "this config has no file".into(),
            };
        };
        let (config, recovered) = match load(&path, true) {
            Loaded::Blocked(reason) => {
                current.blocked = Some(reason.clone());
                return ConfigError::Unreadable { path, reason };
            }
            Loaded::Config(config) => (*config, None),
            Loaded::Quarantined(to) => (AppConfig::default(), Some(to)),
        };
        let (config, key_in_keychain) = match &self.secrets {
            Some(secrets) => with_stored_key(config, secrets.as_ref()),
            None => (config, false),
        };
        current.config = config;
        current.key_in_keychain = key_in_keychain;
        current.blocked = None;
        current.version += 1;
        let (version, snapshot) = (current.version, current.config.clone());
        drop(current);
        if recovered.is_some() {
            *self.recovered.lock().unwrap_or_else(|p| p.into_inner()) = recovered;
        }
        tracing::warn!(path = %path.display(), "config loaded after all; the store follows the file now");
        self.deliver(version, snapshot);
        ConfigError::Reloaded { path }
    }

    /// Put the API key where it belongs before a save, and say whether the credential store holds
    /// it (the file then keeps it blank) and whether this call changed that store. It is touched
    /// only when the key changed, or when a key still in the file can move into it.
    fn place_key(&self, current: &Current, next: &AppConfig) -> Placed {
        let Some(secrets) = &self.secrets else {
            return Placed {
                in_keychain: false,
                changed: false,
            };
        };
        let key = next.api_key.trim();
        if next.api_key == current.config.api_key && (current.key_in_keychain || key.is_empty()) {
            return Placed {
                in_keychain: current.key_in_keychain,
                changed: false,
            };
        }
        if key.is_empty() {
            secrets.clear();
            return Placed {
                in_keychain: false,
                changed: true,
            };
        }
        // The store unavailable: the key stays in the file so the app still works.
        Placed {
            in_keychain: secrets.store(&next.api_key),
            changed: true,
        }
    }

    /// Undo [`Self::place_key`] after the save it was for failed: the credential store goes back
    /// to holding what `current` says it held.
    fn restore_key(&self, current: &Current) {
        let Some(secrets) = &self.secrets else {
            return;
        };
        if !current.key_in_keychain {
            secrets.clear();
        } else if !secrets.store(&current.config.api_key) {
            tracing::error!(
                "a save failed and the credential store could not be put back: the stored API \
                 key may not match the config until the next successful save"
            );
        }
    }

    /// Tell observers about commit `version`, in commit order.
    ///
    /// One thread tells at a time. A commit that arrives meanwhile leaves its config for that
    /// thread and waits for it to finish, so when `update` returns its observers have heard this
    /// config or a newer one — never an older one after it. (Called from inside an observer, it
    /// leaves the config and returns: the telling already under way on this thread delivers it.)
    fn deliver(&self, version: u64, snapshot: AppConfig) {
        let me = std::thread::current().id();
        {
            let mut delivery = self.delivery.lock().unwrap_or_else(|p| p.into_inner());
            let newest = delivery.pending.as_ref().map_or(delivery.told, |(v, _)| *v);
            if version > newest {
                delivery.pending = Some((version, snapshot));
            }
            if delivery.telling == Some(me) {
                return;
            }
            while delivery.telling.is_some() {
                delivery = self.told.wait(delivery).unwrap_or_else(|p| p.into_inner());
            }
            delivery.telling = Some(me);
        }
        // Gives the turn back even if an observer panics.
        let _turn = Turn(self);
        loop {
            let next = {
                let mut delivery = self.delivery.lock().unwrap_or_else(|p| p.into_inner());
                let next = delivery.pending.take();
                if let Some((version, _)) = &next {
                    delivery.told = *version;
                }
                next
            };
            let Some((_, config)) = next else {
                break;
            };
            let observers: Vec<Observer> = self
                .observers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            for observer in observers {
                observer(&config);
            }
        }
    }

    /// Call `f` with the new config after every committed update — how a setting reaches code that
    /// holds its own copy of it (the agent planes' allow-list and byte release) without a restart.
    ///
    /// `f` runs on an updating thread, after the store's lock is released, and calls come in
    /// commit order: the last config `f` hears is the last one committed. When updates land
    /// together `f` may hear only the newest. `f` may read the store and may update it.
    pub fn on_change(&self, f: impl Fn(&AppConfig) + Send + Sync + 'static) {
        self.observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::new(f));
    }

    /// The quarantined copy of a config that failed to parse, until dismissed (B12).
    pub fn recovered(&self) -> Option<PathBuf> {
        self.recovered
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// The user has seen the recovery notice.
    pub fn dismiss_recovered(&self) {
        *self.recovered.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    /// Why the config file could not be loaded, while that is so. The store then holds a fallback
    /// that switches nothing on — no agent tool, no background identification, no source
    /// watching — and refuses every update; the file is left exactly as it is.
    pub fn unreadable(&self) -> Option<String> {
        self.lock().blocked.clone()
    }

    /// Whether this store refuses every write because another Lodestash owns the config.
    pub fn is_read_only(&self) -> bool {
        self.read_only.is_some()
    }

    /// The Settings screen's view (UI-SPEC §11): every section, the API key reduced to whether one
    /// is configured.
    pub fn settings(&self) -> Settings {
        self.read(settings_of)
    }

    /// Apply a Settings write (whole object) and save it. Returns the settings as stored — the size
    /// tiers normalised, the API key withheld.
    pub fn set_settings(&self, settings: Settings) -> Result<Settings, ConfigError> {
        self.update(|c| {
            apply_settings(settings, c);
            settings_of(c)
        })
    }
}

/// What [`ConfigStore::place_key`] did.
struct Placed {
    in_keychain: bool,
    changed: bool,
}

/// One thread's turn at telling observers; dropping it lets the next thread in.
struct Turn<'a>(&'a ConfigStore);

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.0
            .delivery
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .telling = None;
        self.0.told.notify_all();
    }
}

/// What reading the config file came to.
enum Loaded {
    /// The file's config — or the defaults, when there is no file.
    Config(Box<AppConfig>),
    /// The file did not parse and was moved aside, to here. The store runs on defaults.
    Quarantined(PathBuf),
    /// The file is there and was not loaded: it could not be read, or it does not parse and could
    /// not be moved aside. It is exactly as it was, and must not be written over.
    Blocked(String),
}

/// Read and parse `path`. Only a file that was read and does not parse is corrupt, and it is moved
/// aside (when `move_aside`) so the next save cannot destroy it. A read that fails is not
/// corruption: the file may be perfectly good.
fn load(path: &Path, move_aside: bool) -> Loaded {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Loaded::Config(Box::default()),
        Err(e) => {
            tracing::error!(%e, path = %path.display(), "config unreadable; it is left as it is");
            return Loaded::Blocked(format!("it could not be read: {e}"));
        }
    };
    let parse = match serde_json::from_slice::<AppConfig>(&bytes) {
        Ok(config) => return Loaded::Config(Box::new(config)),
        Err(e) => e,
    };
    tracing::error!(e = %parse, path = %path.display(), "config does not parse");
    if !move_aside {
        return Loaded::Blocked(format!("it does not parse: {parse}"));
    }
    match quarantine(path) {
        Ok(to) => {
            tracing::warn!(to = %to.display(), "config quarantined; running on defaults");
            Loaded::Quarantined(to)
        }
        Err(e) => {
            tracing::error!(%e, path = %path.display(), "config could not be quarantined");
            Loaded::Blocked(format!(
                "it does not parse ({parse}) and could not be moved aside ({e})"
            ))
        }
    }
}

/// [`load`], giving a file that cannot be read a moment: another program holding it — a scanner, a
/// backup, an editor saving — usually lets go within milliseconds.
fn load_patiently(path: &Path) -> Loaded {
    const TRIES: u32 = 5;
    const PAUSE: Duration = Duration::from_millis(40);
    let mut loaded = load(path, true);
    for _ in 1..TRIES {
        if !matches!(loaded, Loaded::Blocked(_)) {
            break;
        }
        std::thread::sleep(PAUSE);
        loaded = load(path, true);
    }
    loaded
}

/// What a blocked store holds in place of the config it could not read: the defaults, minus
/// everything a default would switch on that the user may have switched off. Every agent tool is
/// disabled, and nothing identifies or rescans in the background. It is never saved — a blocked
/// store refuses every update.
fn fallback() -> AppConfig {
    AppConfig {
        mcp_disabled_tools: crate::config::agent_tool_names()
            .map(str::to_string)
            .collect(),
        auto_identify: false,
        watch_sources: false,
        ..AppConfig::default()
    }
}

/// `config` with its API key taken from the credential store when the file holds none, and whether
/// that is where the key lives.
fn with_stored_key(mut config: AppConfig, secrets: &dyn SecretStore) -> (AppConfig, bool) {
    if config.api_key.is_empty() {
        if let Some(key) = secrets.load() {
            config.api_key = key;
            return (config, true);
        }
    }
    (config, false)
}

/// Move a bad config out of the way so the next save cannot overwrite what is left of it, and say
/// where it went.
fn quarantine(path: &Path) -> std::io::Result<PathBuf> {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".corrupt-{unix}"));
    let to = PathBuf::from(name);
    std::fs::rename(path, &to)?;
    Ok(to)
}

/// Encode `config` and replace `path` with it atomically: temp file, `sync_all`, rename.
fn write_config(path: &Path, config: &AppConfig, key_in_keychain: bool) -> Result<(), ConfigError> {
    let mut json = serde_json::to_value(config).map_err(ConfigError::Encode)?;
    if key_in_keychain {
        json["api_key"] = serde_json::Value::String(String::new());
    }
    let bytes = serde_json::to_vec_pretty(&json).map_err(ConfigError::Encode)?;
    write_atomic(path, &bytes).map_err(|source| ConfigError::Write {
        path: path.to_path_buf(),
        source,
    })
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp_name = path.as_os_str().to_owned();
    // The store's lock serialises this process; the pid keeps two processes' temps apart.
    tmp_name.push(format!(".tmp-{}", std::process::id()));
    let tmp = PathBuf::from(tmp_name);
    let written = (|| {
        let mut file = create_private(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()
    })();
    // `rename` replaces the destination on Windows too (MOVEFILE_REPLACE_EXISTING).
    let result = written.and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Create (or empty) `path` for writing, readable by its owner only where the OS has file modes.
///
/// The config can hold the cloud API key in plain text — always on Linux and macOS, where no
/// credential store is wired, and on Windows when the store is unavailable — so it is `0600`
/// there. On Windows the file takes its folder's ACL: the per-user profile's, which already keeps
/// other non-administrator users out.
fn create_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    {
        // `mode` applies only when the file is created; a temp left by a crashed save keeps the
        // mode it had.
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

/// The Settings DTO of a config — each section on its own. The API key is never copied out.
pub fn settings_of(c: &AppConfig) -> Settings {
    Settings {
        appearance: AppearanceSettings {
            m_kb: c.size_tiers.m_kb,
            l_kb: c.size_tiers.l_kb,
            xl_kb: c.size_tiers.xl_kb,
            cap_kb: c.size_tiers.cap_kb,
            preview_quality: match c.preview_quality {
                PreviewQuality::Low => WireQuality::Low,
                PreviewQuality::Medium => WireQuality::Medium,
                PreviewQuality::High => WireQuality::High,
            },
        },
        library: LibrarySettings {
            watch_sources: c.watch_sources,
            confirm_before_trash: c.confirm_before_trash,
            skip_onboarding: c.skip_onboarding,
        },
        identify: IdentifySettings {
            auto_identify: c.auto_identify,
            tagger_model: c.tagger_model.clone(),
            tagger_vision_model: c.tagger_vision_model.clone(),
        },
        cloud: CloudSettings {
            api_tagging_enabled: c.api_tagging_enabled,
            api_base_url: Some(c.api_base_url.clone()),
            api_model: c.api_model.clone(),
            api_key: None,
            clear_api_key: false,
            api_key_configured: !c.api_key.trim().is_empty(),
            api_max_per_run: Some(c.api_max_per_run),
            remote_image_bytes: c.remote_image_bytes,
        },
        agents: AgentSettings {
            mcp_disabled_tools: c.mcp_disabled_tools.clone(),
            mcp_bytes_enabled: c.mcp_bytes_enabled,
        },
        storage: StorageSettings {
            cache_extracted: c.cache_extracted,
            preview_cache_cap_gb: c.preview_cache_cap_gb,
            auto_purge_preview_cache: c.auto_purge_preview_cache,
        },
    }
}

/// Write every section of `s` into `c`. Each section touches only its own fields, so a section can
/// be applied alone.
pub fn apply_settings(s: Settings, c: &mut AppConfig) {
    apply_appearance(s.appearance, c);
    apply_library(s.library, c);
    apply_identify(s.identify, c);
    apply_cloud(s.cloud, c);
    apply_agents(s.agents, c);
    apply_storage(s.storage, c);
}

pub fn apply_appearance(s: AppearanceSettings, c: &mut AppConfig) {
    c.size_tiers = SizeTiers {
        m_kb: s.m_kb,
        l_kb: s.l_kb,
        xl_kb: s.xl_kb,
        cap_kb: s.cap_kb,
    }
    .normalized();
    c.preview_quality = match s.preview_quality {
        WireQuality::Low => PreviewQuality::Low,
        WireQuality::Medium => PreviewQuality::Medium,
        WireQuality::High => PreviewQuality::High,
    };
}

pub fn apply_library(s: LibrarySettings, c: &mut AppConfig) {
    c.watch_sources = s.watch_sources;
    c.confirm_before_trash = s.confirm_before_trash;
    c.skip_onboarding = s.skip_onboarding;
}

pub fn apply_identify(s: IdentifySettings, c: &mut AppConfig) {
    c.auto_identify = s.auto_identify;
    c.tagger_model = s.tagger_model;
    c.tagger_vision_model = s.tagger_vision_model;
}

/// What a write did not say changes nothing (ADR-0024):
///
/// - `api_key`: a value replaces the stored key; absent, empty or blank leaves it. Only
///   `clear_api_key` removes it, and it wins over a key sent with it.
/// - `api_base_url`, `api_max_per_run`: absent leaves the stored endpoint and cap.
pub fn apply_cloud(s: CloudSettings, c: &mut AppConfig) {
    c.api_tagging_enabled = s.api_tagging_enabled;
    if let Some(url) = s.api_base_url {
        c.api_base_url = url;
    }
    c.api_model = s.api_model;
    match s.api_key.as_deref().map(str::trim) {
        _ if s.clear_api_key => c.api_key.clear(),
        Some(key) if !key.is_empty() => c.api_key = key.to_string(),
        _ => {}
    }
    if let Some(cap) = s.api_max_per_run {
        c.api_max_per_run = cap;
    }
    c.remote_image_bytes = s.remote_image_bytes;
}

pub fn apply_agents(s: AgentSettings, c: &mut AppConfig) {
    c.mcp_disabled_tools = s.mcp_disabled_tools;
    c.mcp_bytes_enabled = s.mcp_bytes_enabled;
}

pub fn apply_storage(s: StorageSettings, c: &mut AppConfig) {
    c.cache_extracted = s.cache_extracted;
    c.preview_cache_cap_gb = s.preview_cache_cap_gb;
    c.auto_purge_preview_cache = s.auto_purge_preview_cache;
}

/// The OS credential store as a [`SecretStore`] (AI-PIPELINE §5.1): Windows Credential Manager via
/// `keyring`. Everywhere else nothing is stored and the caller keeps the plaintext-config
/// fallback. Every call is failure-tolerant — a locked or absent keychain never breaks a load or a
/// save.
struct OsKeychain;

impl SecretStore for OsKeychain {
    fn load(&self) -> Option<String> {
        secret::load_api_key()
    }

    fn store(&self, key: &str) -> bool {
        secret::store_api_key(key)
    }

    fn clear(&self) {
        secret::clear_api_key();
    }
}

mod secret {
    #[cfg(windows)]
    const SERVICE: &str = "Lodestash";
    #[cfg(windows)]
    const USER: &str = "api_key";

    #[cfg(windows)]
    fn entry() -> Option<keyring::Entry> {
        keyring::Entry::new(SERVICE, USER).ok()
    }

    /// Store the key; returns `true` only if it was actually written to the OS store.
    #[cfg(windows)]
    pub fn store_api_key(key: &str) -> bool {
        entry().is_some_and(|e| e.set_password(key).is_ok())
    }

    #[cfg(windows)]
    pub fn load_api_key() -> Option<String> {
        entry().and_then(|e| e.get_password().ok())
    }

    #[cfg(windows)]
    pub fn clear_api_key() {
        if let Some(e) = entry() {
            let _ = e.delete_credential();
        }
    }

    #[cfg(not(windows))]
    pub fn store_api_key(_key: &str) -> bool {
        false // no OS store wired for this platform → plaintext-config fallback
    }
    #[cfg(not(windows))]
    pub fn load_api_key() -> Option<String> {
        None
    }
    #[cfg(not(windows))]
    pub fn clear_api_key() {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::{agent_may_read_bytes, may_send, Modality};

    fn store_in(dir: &Path) -> (ConfigStore, PathBuf) {
        let path = dir.join("config.json");
        (ConfigStore::open(&path, false), path)
    }

    /// Review B12: a truncated config used to load as defaults, and the next save made the loss
    /// permanent. Now the bad file is moved aside intact, the store says so, and the next save
    /// writes a whole, valid file.
    #[test]
    fn a_corrupt_config_is_quarantined_reported_and_replaced_by_a_valid_one() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.json");
        let truncated = br#"{"last_project_name":"Game","projects":[{"name":"Ga"#;
        std::fs::write(&path, truncated).expect("seed");

        let store = ConfigStore::open(&path, false);
        let quarantined = store.recovered().expect("the recovery is reported");
        assert_eq!(
            std::fs::read(&quarantined).expect("the quarantined copy exists"),
            truncated,
            "the bad file is kept byte for byte"
        );
        assert!(
            quarantined
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("config.json.corrupt-"),
            "{quarantined:?}"
        );
        assert!(!path.exists(), "nothing was written in its place on open");
        assert!(store.get().auto_identify, "the store runs on defaults");

        store.update(|c| c.volume = 0.25).expect("save");
        let saved: AppConfig =
            serde_json::from_slice(&std::fs::read(&path).expect("saved")).expect("valid JSON");
        assert_eq!(saved.volume, 0.25);

        store.dismiss_recovered();
        assert!(store.recovered().is_none(), "dismissed");
        assert!(
            ConfigStore::open(&path, false).recovered().is_none(),
            "a valid file opens clean"
        );
    }

    /// The old store was load-modify-save per caller with a non-atomic write: two racing edits
    /// could each save over the other. Eight threads, fifty edits each, none lost — on disk.
    #[test]
    fn concurrent_updates_lose_nothing() {
        let dir = tempfile::tempdir().expect("tmp");
        let (store, path) = store_in(dir.path());
        let store = Arc::new(store);
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for i in 0..50 {
                        store
                            .update(|c| c.asset_store_roots.push(format!("t{t}/{i}").into()))
                            .expect("update");
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("thread");
        }
        assert_eq!(store.get().asset_store_roots.len(), 400);
        let reopened = ConfigStore::open(&path, false).get();
        assert_eq!(
            reopened.asset_store_roots.len(),
            400,
            "every edit reached the file"
        );
        let tmp_left = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().contains(".tmp-"));
        assert!(!tmp_left, "no temp file is left behind");
    }

    /// Review U8: reading the settings saved the file. A read — of the config or the settings —
    /// leaves the file exactly as it was, mtime included.
    #[test]
    fn reading_never_touches_the_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let (store, path) = store_in(dir.path());
        store
            .update(|c| c.api_key = "sk-test".into())
            .expect("seed");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .and_then(|f| f.set_modified(old))
            .expect("age the file");
        let bytes = std::fs::read(&path).unwrap();

        let reopened = ConfigStore::open(&path, false);
        let _ = reopened.get();
        let _ = reopened.settings();
        let _ = reopened.read(|c| c.preview_quality);

        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.modified().unwrap(), old, "a read moved the mtime");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "a read rewrote the file"
        );
    }

    /// Review U8, the other half: the key never leaves in a read, and a write that does not name
    /// it keeps it.
    #[test]
    fn the_api_key_is_write_only() {
        let dir = tempfile::tempdir().expect("tmp");
        let (store, _) = store_in(dir.path());
        let mut s = store.settings();
        assert!(!s.cloud.api_key_configured);
        s.cloud.api_key = Some("sk-secret".into());
        let stored = store.set_settings(s).expect("set");
        assert!(stored.cloud.api_key_configured);
        assert_eq!(stored.cloud.api_key, None);

        let read = serde_json::to_string(&store.settings()).unwrap();
        assert!(
            !read.contains("sk-secret"),
            "the key left in a read: {read}"
        );
        assert!(!read.contains("\"api_key\""), "{read}");
        assert!(read.contains("\"api_key_configured\":true"), "{read}");

        // A whole-object write from a client that never saw the key keeps it.
        let mut s = store.settings();
        s.library.confirm_before_trash = false;
        store.set_settings(s).expect("set");
        assert_eq!(store.get().api_key, "sk-secret");
    }

    /// What a client sends: the flat JSON object it read, edited by `edit`. Returns what the store
    /// answered.
    fn write_wire(
        store: &ConfigStore,
        edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
    ) -> Settings {
        let mut wire = serde_json::to_value(store.settings()).unwrap();
        edit(wire.as_object_mut().unwrap());
        let settings: Settings = serde_json::from_value(wire).expect("the DTO reads it");
        store.set_settings(settings).expect("set")
    }

    /// An empty `api_key` used to mean "clear the stored key", so a key field that was typed into
    /// and erased wiped the key on the next save of anything. Now nothing but an explicit
    /// `clear_api_key` removes it: absent, empty and blank all leave the key alone.
    #[test]
    fn an_empty_api_key_changes_nothing_and_clearing_is_explicit() {
        let dir = tempfile::tempdir().expect("tmp");
        let (store, _) = store_in(dir.path());
        store
            .update(|c| c.api_key = "sk-secret".into())
            .expect("seed");

        for blank in ["", "   "] {
            let stored = write_wire(&store, |o| {
                o.insert("api_key".into(), blank.into());
                o.insert("watch_sources".into(), false.into());
            });
            assert!(stored.cloud.api_key_configured, "{blank:?} cleared the key");
            assert_eq!(store.get().api_key, "sk-secret", "{blank:?}");
        }
        write_wire(&store, |o| {
            o.insert("clear_api_key".into(), false.into());
        });
        assert_eq!(store.get().api_key, "sk-secret", "false is not a request");

        let stored = write_wire(&store, |o| {
            o.insert("clear_api_key".into(), true.into());
        });
        assert!(!stored.cloud.api_key_configured);
        assert!(store.get().api_key.is_empty(), "the explicit clear");

        // Asked to clear and handed a key in one write: the key is cleared. A client's state can
        // still hold the key it sent last time; the explicit request is the newer intent.
        store
            .update(|c| c.api_key = "sk-secret".into())
            .expect("seed");
        write_wire(&store, |o| {
            o.insert("api_key".into(), "sk-stale".into());
            o.insert("clear_api_key".into(), true.into());
        });
        assert!(store.get().api_key.is_empty());

        // Neither write-only field ever appears in a read.
        let read = serde_json::to_string(&store.settings()).unwrap();
        assert!(!read.contains("clear_api_key"), "{read}");
        assert!(!read.contains("\"api_key\""), "{read}");
    }

    /// The Settings DTO read an omitted `api_max_per_run` as `0` — uncapped — while the config's
    /// own default is a cap, so a write that left the field out silently removed the cloud spend
    /// guard; one that left out `api_base_url` blanked the endpoint. Leaving either out now
    /// changes nothing: the DTO has no default of its own to disagree with the config's, and the
    /// stored cap and endpoint — the user's, or the config's default — stay.
    #[test]
    fn a_write_that_omits_the_cap_or_the_endpoint_changes_neither() {
        let dir = tempfile::tempdir().expect("tmp");
        let (store, _) = store_in(dir.path());
        let default = store.get();
        assert_ne!(default.api_max_per_run, 0, "the config starts capped");

        let omit = |store: &ConfigStore| {
            write_wire(store, |o| {
                o.remove("api_max_per_run");
                o.remove("api_base_url");
                o.insert("watch_sources".into(), false.into());
            })
        };
        // On a fresh config: its defaults, not "uncapped" and a blank endpoint.
        let stored = omit(&store);
        assert_eq!(store.get().api_max_per_run, default.api_max_per_run);
        assert_eq!(store.get().api_base_url, default.api_base_url);
        assert_eq!(
            serde_json::to_value(&stored).unwrap()["api_max_per_run"],
            default.api_max_per_run,
            "a read always carries the cap"
        );

        // On the user's own values: theirs — a tighter cap is not loosened, and a local endpoint
        // is not swapped for a remote one the stored key would then be sent to.
        store
            .update(|c| {
                c.api_max_per_run = 40;
                c.api_base_url = "http://127.0.0.1:1234/v1".into();
            })
            .expect("seed");
        omit(&store);
        assert_eq!(store.get().api_max_per_run, 40);
        assert_eq!(store.get().api_base_url, "http://127.0.0.1:1234/v1");

        // Saying so is still how to change them — `0` included.
        write_wire(&store, |o| {
            o.insert("api_max_per_run".into(), 0.into());
        });
        assert_eq!(store.get().api_max_per_run, 0, "an explicit 0 is uncapped");
    }

    /// Observers used to be told after the store's lock was released, so two updates landing
    /// together could be delivered in the wrong order — and whoever follows the config (the agent
    /// planes' allow-list and byte release) was left holding the older one: a consent just revoked
    /// stayed live. Deliveries now happen in commit order.
    ///
    /// The first observer stalls the first delivery until the second update has gone through — or
    /// for 300 ms, which is what it gets once the second update has to wait its turn.
    #[test]
    fn observers_hear_updates_in_commit_order() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Condvar;
        use std::time::Duration;

        let store = Arc::new(ConfigStore::in_memory());
        let stalled = Arc::new((Mutex::new(false), Condvar::new()));
        let revoked = Arc::new((Mutex::new(false), Condvar::new()));
        let first = Arc::new(AtomicBool::new(true));
        {
            let (stalled, revoked, first) = (stalled.clone(), revoked.clone(), first.clone());
            store.on_change(move |_| {
                if !first.swap(false, Ordering::SeqCst) {
                    return;
                }
                *stalled.0.lock().unwrap() = true;
                stalled.1.notify_all();
                let done = revoked.0.lock().unwrap();
                let _ = revoked
                    .1
                    .wait_timeout_while(done, Duration::from_millis(300), |done| !*done)
                    .unwrap();
            });
        }
        let heard = Arc::new(Mutex::new(Vec::new()));
        {
            let heard = heard.clone();
            store.on_change(move |c| heard.lock().unwrap().push(c.mcp_bytes_enabled));
        }

        let revoke = {
            let (store, stalled, revoked) = (store.clone(), stalled.clone(), revoked.clone());
            std::thread::spawn(move || {
                let waiting = stalled.0.lock().unwrap();
                drop(stalled.1.wait_while(waiting, |stalled| !*stalled).unwrap());
                store
                    .update(|c| c.mcp_bytes_enabled = false)
                    .expect("revoke");
                *revoked.0.lock().unwrap() = true;
                revoked.1.notify_all();
            })
        };
        store.update(|c| c.mcp_bytes_enabled = true).expect("grant");
        revoke.join().expect("thread");

        assert!(!store.get().mcp_bytes_enabled, "the revoke committed last");
        assert_eq!(
            *heard.lock().unwrap(),
            [true, false],
            "the last thing an observer heard must be the last thing committed"
        );
    }

    /// An observer may itself change the config (a migration of one setting into another, say):
    /// the change it makes is delivered after the one it was reacting to, and nothing deadlocks.
    #[test]
    fn an_observer_may_update_the_store() {
        let store = Arc::new(ConfigStore::in_memory());
        let heard = Arc::new(Mutex::new(Vec::new()));
        {
            let (inner, heard) = (Arc::downgrade(&store), heard.clone());
            store.on_change(move |c| {
                heard.lock().unwrap().push(c.volume);
                if c.volume == 0.5 {
                    let store = inner.upgrade().expect("the store");
                    store.update(|c| c.volume = 0.25).expect("nested");
                }
            });
        }
        store.update(|c| c.volume = 0.5).expect("outer");
        assert_eq!(*heard.lock().unwrap(), [0.5, 0.25]);
        assert_eq!(store.get().volume, 0.25);
    }

    fn corrupt_copies(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains(".corrupt-"))
            .collect()
    }

    /// A config that is there and cannot be read is not a corrupt config. It used to be
    /// quarantined like one — and the app then ran, and saved, on defaults. Here the "file" is a
    /// directory, which every platform refuses to read: it is left where it is, nothing is written
    /// over it, and once it can be read the store takes what it says.
    #[test]
    fn a_config_that_cannot_be_read_is_left_alone_and_never_overwritten() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.json");
        std::fs::create_dir(&path).unwrap();

        let store = ConfigStore::open(&path, false);
        assert!(
            store.recovered().is_none(),
            "a read error is not corruption"
        );
        assert!(path.is_dir(), "the unreadable config was moved");
        assert_eq!(corrupt_copies(dir.path()), Vec::<String>::new());
        assert!(store.unreadable().is_some(), "the state is reported");
        // Not knowing what the user chose, the store switches nothing on: no agent tool, no
        // background identification, no rescans.
        let held = store.get();
        assert_eq!(held.mcp_enabled_tools(), Some(Default::default()));
        assert!(!held.auto_identify && !held.watch_sources && !held.mcp_bytes_enabled);
        assert!(
            matches!(
                store.update(|c| c.volume = 0.1),
                Err(ConfigError::Unreadable { .. })
            ),
            "wrote a config over one that was never read"
        );
        assert!(path.is_dir());

        // The obstacle goes, and the user's real config is there.
        std::fs::remove_dir(&path).unwrap();
        let (seed, _) = store_in(dir.path());
        seed.update(|c| {
            c.mcp_disabled_tools = vec!["export_asset".into()];
            c.volume = 0.9;
        })
        .expect("seed");
        drop(seed);

        // The next write finds it readable. That write was decided while looking at the fallback,
        // so it is refused — and the file's config is what the store now holds, and what its
        // observers are told.
        let told = Arc::new(Mutex::new(None));
        {
            let told = told.clone();
            store.on_change(move |c| *told.lock().unwrap() = Some(c.mcp_disabled_tools.clone()));
        }
        assert!(matches!(
            store.update(|c| c.volume = 0.1),
            Err(ConfigError::Reloaded { .. })
        ));
        assert!(store.unreadable().is_none());
        assert_eq!(store.get().mcp_disabled_tools, ["export_asset"]);
        assert_eq!(store.get().volume, 0.9);
        assert_eq!(
            *told.lock().unwrap(),
            Some(vec!["export_asset".to_string()])
        );
        // From here it is an ordinary store.
        store.update(|c| c.volume = 0.2).expect("save");
        let saved = ConfigStore::open(&path, false).get();
        assert_eq!(saved.mcp_disabled_tools, ["export_asset"]);
        assert_eq!(saved.volume, 0.2);
    }

    /// The case from the field: another program — a backup, a scanner — holds `config.json` open
    /// without sharing it, so the read fails. The valid file used to be treated as corrupt; the
    /// rename failed too, and the next save replaced it with defaults.
    #[cfg(windows)]
    #[test]
    fn a_config_another_program_holds_open_is_left_alone() {
        use std::os::windows::fs::OpenOptionsExt;

        let dir = tempfile::tempdir().expect("tmp");
        let (seed, path) = store_in(dir.path());
        seed.update(|c| c.mcp_disabled_tools = vec!["export_asset".into()])
            .expect("seed");
        drop(seed);
        let before = std::fs::read(&path).unwrap();

        let held = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .expect("hold the file");
        let store = ConfigStore::open(&path, false);
        assert!(
            store.recovered().is_none(),
            "a read error is not corruption"
        );
        assert!(store.update(|c| c.volume = 0.1).is_err());
        drop(held);
        assert_eq!(std::fs::read(&path).unwrap(), before, "the file changed");
        assert_eq!(corrupt_copies(dir.path()), Vec::<String>::new());

        assert!(store.update(|c| c.volume = 0.1).is_err(), "refused once");
        store.update(|c| c.volume = 0.3).expect("save");
        let saved = ConfigStore::open(&path, false).get();
        assert_eq!(saved.mcp_disabled_tools, ["export_asset"]);
        assert_eq!(saved.volume, 0.3);
    }

    /// A config that does not parse is moved aside so the next save cannot destroy it. When the
    /// move itself fails, the save must not go ahead — it used to, over the only copy.
    #[test]
    fn a_corrupt_config_that_cannot_be_moved_aside_is_never_overwritten() {
        let dir = tempfile::tempdir().expect("tmp");
        let path = dir.path().join("config.json");
        let corrupt = br#"{"last_project_name":"Game","projects":[{"name":"Ga"#;
        std::fs::write(&path, corrupt).unwrap();
        // Every name the quarantine could pick for the next while is taken by a folder with
        // something in it, which a rename cannot replace.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let blockers: Vec<PathBuf> = (now..now + 30)
            .map(|ts| dir.path().join(format!("config.json.corrupt-{ts}")))
            .collect();
        for blocker in &blockers {
            std::fs::create_dir(blocker).unwrap();
            std::fs::write(blocker.join("in-the-way"), b"").unwrap();
        }

        let store = ConfigStore::open(&path, false);
        assert!(
            store.update(|c| c.volume = 0.1).is_err(),
            "saved over a corrupt config that was never moved aside"
        );
        assert_eq!(std::fs::read(&path).unwrap(), corrupt);

        // Once it can be moved aside, it is — and only then is a new file written.
        for blocker in &blockers {
            std::fs::remove_dir_all(blocker).unwrap();
        }
        assert!(store.update(|c| c.volume = 0.1).is_err(), "refused once");
        let moved = store.recovered().expect("the quarantine is reported");
        assert_eq!(std::fs::read(&moved).unwrap(), corrupt);
        store.update(|c| c.volume = 0.2).expect("save");
        assert_eq!(ConfigStore::open(&path, false).get().volume, 0.2);
    }

    /// Review S2: the two egress consent toggles were dropped by the settings view-model, so they
    /// never persisted or applied. They round-trip through the DTO and its JSON, persist, and move
    /// the live egress decision the passes and agent planes consult.
    #[test]
    fn consent_toggles_round_trip_and_change_the_live_egress_decision() {
        let dir = tempfile::tempdir().expect("tmp");
        let (store, path) = store_in(dir.path());
        let remote = "https://api.example.com/v1";
        assert!(may_send(&store.get(), remote, Modality::Image).is_err());
        assert!(agent_may_read_bytes(&store.get()).is_err());

        // What a client sends: the flat JSON object it read, two switches flipped.
        let mut wire = serde_json::to_value(store.settings()).unwrap();
        wire["remote_image_bytes"] = true.into();
        wire["mcp_bytes_enabled"] = true.into();
        let settings: Settings = serde_json::from_value(wire).expect("the DTO reads it");
        let seen = Arc::new(Mutex::new(None));
        let observed = seen.clone();
        store.on_change(move |c| *observed.lock().unwrap() = Some(c.mcp_bytes_enabled));
        store.set_settings(settings).expect("set");

        assert_eq!(may_send(&store.get(), remote, Modality::Image), Ok(()));
        assert_eq!(agent_may_read_bytes(&store.get()), Ok(()));
        assert_eq!(*seen.lock().unwrap(), Some(true), "observers heard it");
        let reread = ConfigStore::open(&path, false).settings();
        assert!(reread.cloud.remote_image_bytes && reread.agents.mcp_bytes_enabled);
    }

    /// A write that cannot reach the disk changes nothing — not even in memory.
    #[test]
    fn a_failed_save_leaves_the_config_as_it_was() {
        let dir = tempfile::tempdir().expect("tmp");
        // The config's folder is a file, so the save cannot create it.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"a file, not a folder").unwrap();
        let store = ConfigStore::open(blocker.join("config.json"), false);
        assert!(store.update(|c| c.volume = 0.1).is_err());
        assert_eq!(store.get().volume, AppConfig::default().volume);
    }

    /// A credential store a test owns: what it holds, and whether it may be written.
    #[derive(Clone, Default)]
    struct FakeSecrets {
        held: Arc<Mutex<Option<String>>>,
    }

    impl FakeSecrets {
        fn holding(key: &str) -> Self {
            let secrets = Self::default();
            *secrets.held.lock().unwrap() = Some(key.into());
            secrets
        }

        fn held(&self) -> Option<String> {
            self.held.lock().unwrap().clone()
        }
    }

    impl SecretStore for FakeSecrets {
        fn load(&self) -> Option<String> {
            self.held()
        }

        fn store(&self, key: &str) -> bool {
            *self.held.lock().unwrap() = Some(key.into());
            true
        }

        fn clear(&self) {
            *self.held.lock().unwrap() = None;
        }
    }

    /// A store at `<dir>/config/config.json` whose key rests in `secrets`, and a switch that makes
    /// every later save fail: the config folder is replaced by a file.
    fn store_with_secrets(dir: &Path, secrets: &FakeSecrets) -> (ConfigStore, impl Fn()) {
        let folder = dir.join("config");
        let store = ConfigStore::open_with(
            folder.join("config.json"),
            Some(Box::new(secrets.clone())),
            None,
        );
        let break_saves = move || {
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::write(&folder, b"a file, not a folder").unwrap();
        };
        (store, break_saves)
    }

    /// The key changed in the credential store before the config was written, so a save that
    /// failed reported failure and had already replaced — or removed — the stored key. A save and
    /// its key now succeed or fail together: after a failed save the credential store holds what it
    /// held before, and so does memory.
    #[test]
    fn a_failed_save_leaves_the_credential_store_as_it_was() {
        let dir = tempfile::tempdir().expect("tmp");
        let secrets = FakeSecrets::holding("sk-old");
        let (store, break_saves) = store_with_secrets(dir.path(), &secrets);
        assert_eq!(
            store.get().api_key,
            "sk-old",
            "the key is read from the store"
        );
        break_saves();

        assert!(store.update(|c| c.api_key = "sk-new".into()).is_err());
        assert_eq!(
            secrets.held().as_deref(),
            Some("sk-old"),
            "a failed save replaced the stored key"
        );
        assert!(store.update(|c| c.api_key.clear()).is_err());
        assert_eq!(
            secrets.held().as_deref(),
            Some("sk-old"),
            "a failed save removed the stored key"
        );
        assert_eq!(store.get().api_key, "sk-old");

        // With no key stored before: a failed first save of one leaves none behind.
        let dir = tempfile::tempdir().expect("tmp");
        let secrets = FakeSecrets::default();
        let (store, break_saves) = store_with_secrets(dir.path(), &secrets);
        break_saves();
        assert!(store.update(|c| c.api_key = "sk-new".into()).is_err());
        assert_eq!(secrets.held(), None, "a failed save stored a key");
        assert!(store.get().api_key.is_empty());
    }

    /// The other half: a save that works moves the key into the credential store, keeps the file
    /// blank, and an explicit clear empties both.
    #[test]
    fn a_saved_key_rests_in_the_credential_store_not_the_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let secrets = FakeSecrets::default();
        let (store, _) = store_with_secrets(dir.path(), &secrets);
        let path = store.path().unwrap().to_path_buf();

        store.update(|c| c.api_key = "sk-new".into()).expect("save");
        assert_eq!(secrets.held().as_deref(), Some("sk-new"));
        let file = std::fs::read_to_string(&path).unwrap();
        assert!(!file.contains("sk-new"), "the key is in the file: {file}");

        write_wire(&store, |o| {
            o.insert("clear_api_key".into(), true.into());
        });
        assert_eq!(secrets.held(), None);
        assert!(store.get().api_key.is_empty());
    }

    /// A read-only store — the one a Lodestash without the catalog gets — reads the file and
    /// never touches it: no save, and no quarantine of a file it cannot parse.
    #[test]
    fn a_read_only_store_reads_and_never_writes() {
        let dir = tempfile::tempdir().expect("tmp");
        let (seed, path) = store_in(dir.path());
        seed.update(|c| c.volume = 0.4).expect("seed");
        let before = std::fs::read(&path).unwrap();

        let store = ConfigStore::open_read_only(&path, false, "another Lodestash has the catalog");
        assert!(store.is_read_only());
        assert_eq!(store.get().volume, 0.4);
        let err = store.update(|c| c.volume = 0.1).unwrap_err();
        assert!(matches!(err, ConfigError::ReadOnly { .. }), "{err}");
        assert!(err.to_string().contains("another Lodestash"), "{err}");
        assert_eq!(store.get().volume, 0.4, "nor in memory");
        assert_eq!(std::fs::read(&path).unwrap(), before);

        std::fs::write(&path, b"{ not json").unwrap();
        let store = ConfigStore::open_read_only(&path, false, "another Lodestash has the catalog");
        assert!(store.recovered().is_none());
        assert_eq!(std::fs::read(&path).unwrap(), b"{ not json", "it was moved");
        assert_eq!(corrupt_copies(dir.path()), Vec::<String>::new());
    }

    /// The config can hold the API key in plain text wherever no credential store is wired, so
    /// where files have modes it is the owner's alone.
    #[cfg(unix)]
    #[test]
    fn the_config_file_is_readable_by_its_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().expect("tmp");
        let (store, path) = store_in(dir.path());
        store
            .update(|c| c.api_key = "sk-plain".into())
            .expect("save");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{mode:o}");
    }
}
