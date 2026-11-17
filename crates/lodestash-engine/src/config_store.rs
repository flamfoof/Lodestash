//! The engine's one `config.json` (v2 spec §4.5): every read and write of the app's configuration
//! goes through a [`ConfigStore`].
//!
//! - **One copy, one lock.** The store loads the file once and serves reads from memory, so a read
//!   never touches the disk — and never writes it (review U8: reading the settings used to save the
//!   file, API key and all). [`ConfigStore::update`] runs load-modify-save under the same lock, so
//!   two writers cannot lose each other's change.
//! - **Atomic writes.** A save writes a temp file beside the config, `sync_all`s it and renames it
//!   over the old one: a crash mid-save leaves the previous file, never a truncated one.
//! - **Corruption is reported, not overwritten** (review B12). A file that does not parse is moved
//!   aside to `config.json.corrupt-<unix-ts>`, the store runs on defaults, and
//!   [`ConfigStore::recovered`] names the quarantined copy until the user dismisses it. The old
//!   loader read a truncated file as defaults and the next save made the loss permanent.
//! - **The cloud API key stays server-side.** In the app's own config it lives in the OS credential
//!   store and the file keeps it blank (AI-PIPELINE §5.1); a private config (a test's) keeps it in
//!   the file and never touches the credential store. [`ConfigStore::settings`] reports only whether
//!   a key is configured.
//! - **Changes apply live.** [`ConfigStore::on_change`] observers run after every committed update:
//!   the agent planes follow the allow-list and byte release that way, and every pass reads the
//!   egress consent from the store when it starts.
//!
//! Tradeoff: the file is read once. An edit made to `config.json` by hand while the app runs is not
//! seen, and the next save replaces it.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// The Settings DTO (`lodestash-protocol`), which this store reads and writes by section.
pub use lodestash_protocol::dto::Settings;
use lodestash_protocol::dto::{
    AgentSettings, AppearanceSettings, CloudSettings, IdentifySettings, LibrarySettings,
    PreviewQuality as WireQuality, StorageSettings,
};

use crate::config::{AppConfig, PreviewQuality, SizeTiers};

/// Why an update did not land. The in-memory config is unchanged when it is returned.
#[derive(Debug)]
pub enum ConfigError {
    /// The new config could not be written to disk.
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The config could not be encoded.
    Encode(serde_json::Error),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Write { path, source } => {
                write!(f, "could not save {}: {source}", path.display())
            }
            ConfigError::Encode(e) => write!(f, "could not encode the config: {e}"),
        }
    }
}

impl std::error::Error for ConfigError {}

type Observer = Arc<dyn Fn(&AppConfig) + Send + Sync>;

/// The config as last committed, and where its API key lives.
struct Current {
    config: AppConfig,
    /// The key is in the OS credential store and the file holds it blank.
    key_in_keychain: bool,
}

/// The engine's configuration: loaded once, read from memory, written atomically under one lock.
pub struct ConfigStore {
    /// `None` for a store with nowhere to persist (an engine with no config location): updates
    /// apply in memory only.
    path: Option<PathBuf>,
    /// Keep the cloud API key in the OS credential store.
    keychain: bool,
    current: Mutex<Current>,
    recovered: Mutex<Option<PathBuf>>,
    observers: Mutex<Vec<Observer>>,
}

impl ConfigStore {
    /// Open the config at `path`. Never fails: a missing file is the defaults; a file that does not
    /// parse is quarantined and reported ([`Self::recovered`]). Nothing is written until the first
    /// [`Self::update`].
    pub fn open(path: impl Into<PathBuf>, keychain: bool) -> Self {
        let path = path.into();
        let (mut config, recovered) = load(&path);
        let mut key_in_keychain = false;
        if keychain && config.api_key.is_empty() {
            if let Some(key) = secret::load_api_key() {
                config.api_key = key;
                key_in_keychain = true;
            }
        }
        Self {
            path: Some(path),
            keychain,
            current: Mutex::new(Current {
                config,
                key_in_keychain,
            }),
            recovered: Mutex::new(recovered),
            observers: Mutex::new(Vec::new()),
        }
    }

    /// A store with no file behind it: defaults, and updates that live as long as the store.
    pub fn in_memory() -> Self {
        Self {
            path: None,
            keychain: false,
            current: Mutex::new(Current {
                config: AppConfig::default(),
                key_in_keychain: false,
            }),
            recovered: Mutex::new(None),
            observers: Mutex::new(Vec::new()),
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
    /// when the save fails the change is dropped and the error returned, so memory and disk never
    /// disagree. Observers ([`Self::on_change`]) run after the lock is released.
    pub fn update<T>(&self, f: impl FnOnce(&mut AppConfig) -> T) -> Result<T, ConfigError> {
        let mut current = self.lock();
        let mut next = current.config.clone();
        let out = f(&mut next);
        let key_in_keychain = match &self.path {
            Some(path) => {
                let in_keychain = self.place_key(&current, &next);
                write_config(path, &next, in_keychain)?;
                in_keychain
            }
            None => false,
        };
        current.config = next;
        current.key_in_keychain = key_in_keychain;
        let snapshot = current.config.clone();
        drop(current);
        self.notify(&snapshot);
        Ok(out)
    }

    /// Put the API key where it belongs before a save; `true` when the credential store holds it
    /// (the file then keeps it blank). Touches the credential store only when the key changed, or
    /// when a key still in the file can move into the store.
    fn place_key(&self, current: &Current, next: &AppConfig) -> bool {
        if !self.keychain {
            return false;
        }
        let key = next.api_key.trim();
        if next.api_key == current.config.api_key && (current.key_in_keychain || key.is_empty()) {
            return current.key_in_keychain;
        }
        if key.is_empty() {
            secret::clear_api_key();
            return false;
        }
        // The store unavailable: the key stays in the file so the app still works.
        secret::store_api_key(&next.api_key)
    }

    fn notify(&self, config: &AppConfig) {
        let observers: Vec<Observer> = self
            .observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        for observer in observers {
            observer(config);
        }
    }

    /// Call `f` with the new config after every committed update — how a setting reaches code that
    /// holds its own copy of it (the agent planes' allow-list and byte release) without a restart.
    /// `f` runs on the updating thread, after the store's lock is released.
    pub fn on_change(&self, f: impl Fn(&AppConfig) + Send + Sync + 'static) {
        self.observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(Arc::new(f));
    }

    /// The quarantined copy of a config that failed to parse at open, until dismissed (B12).
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

/// Read and parse `path`. A missing file is the defaults; anything else that does not yield a
/// config is quarantined beside it and named in the second value.
fn load(path: &Path) -> (AppConfig, Option<PathBuf>) {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (AppConfig::default(), None),
        Err(e) => {
            tracing::error!(%e, path = %path.display(), "config unreadable; running on defaults");
            return (AppConfig::default(), quarantine(path));
        }
    };
    match serde_json::from_slice::<AppConfig>(&bytes) {
        Ok(config) => (config, None),
        Err(e) => {
            tracing::error!(%e, path = %path.display(), "config does not parse; running on defaults");
            (AppConfig::default(), quarantine(path))
        }
    }
}

/// Move a bad config out of the way so the next save cannot overwrite what is left of it. Returns
/// where it went — or, when even the move failed, the file itself, so the user is still told.
fn quarantine(path: &Path) -> Option<PathBuf> {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".corrupt-{unix}"));
    let to = PathBuf::from(name);
    match std::fs::rename(path, &to) {
        Ok(()) => {
            tracing::warn!(to = %to.display(), "config quarantined");
            Some(to)
        }
        Err(e) => {
            tracing::error!(%e, path = %path.display(), "config could not be quarantined");
            Some(path.to_path_buf())
        }
    }
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
        let mut file = std::fs::File::create(&tmp)?;
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
            api_base_url: c.api_base_url.clone(),
            api_model: c.api_model.clone(),
            api_key: None,
            api_key_configured: !c.api_key.trim().is_empty(),
            api_max_per_run: c.api_max_per_run,
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

/// `api_key`: absent keeps the stored key, `""` clears it, anything else replaces it.
pub fn apply_cloud(s: CloudSettings, c: &mut AppConfig) {
    c.api_tagging_enabled = s.api_tagging_enabled;
    c.api_base_url = s.api_base_url;
    c.api_model = s.api_model;
    if let Some(key) = s.api_key {
        c.api_key = key.trim().to_string();
    }
    c.api_max_per_run = s.api_max_per_run;
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

/// The cloud API key at rest (AI-PIPELINE §5.1). On Windows the key lives in Credential Manager via
/// `keyring`; everywhere else these are no-ops and the caller keeps the plaintext-config fallback.
/// Every call is failure-tolerant — a locked or absent keychain never breaks a load or a save.
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

        // A whole-object write from a client that never saw the key keeps it…
        let mut s = store.settings();
        s.library.confirm_before_trash = false;
        store.set_settings(s).expect("set");
        assert_eq!(store.get().api_key, "sk-secret");
        // …and an empty one clears it.
        let mut s = store.settings();
        s.cloud.api_key = Some(String::new());
        assert!(!store.set_settings(s).expect("set").cloud.api_key_configured);
        assert!(store.get().api_key.is_empty());
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
}
