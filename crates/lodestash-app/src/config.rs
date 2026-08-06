//! Persistent app configuration — survives restarts.
//! Stored at: `{os_config_dir}/Lodestash/config.json`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lodestash_core::types::Exclude;
use serde::{Deserialize, Serialize};

pub use lodestash_core::types::ExcludeKind;

// ── Sort preference ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    #[default]
    Name,
    Type,
    Size,
    Modified,
    Rating,
    /// BM25 match quality for the active Tags query (IDENTIFICATION-AND-SEARCH §5.A).
    /// Falls back to name order when no query is active.
    Relevance,
}

impl SortBy {
    pub const OPTIONS: &'static [(SortBy, &'static str)] = &[
        (SortBy::Relevance, "Relevance"),
        (SortBy::Name, "Name"),
        (SortBy::Type, "Type"),
        (SortBy::Size, "Size"),
        (SortBy::Modified, "Date"),
        (SortBy::Rating, "Rating"),
    ];

    pub fn label(self) -> &'static str {
        Self::OPTIONS
            .iter()
            .find(|&&(s, _)| s == self)
            .map(|&(_, l)| l)
            .unwrap_or("Name")
    }
}

// ── Named library project ─────────────────────────────────────────────────────

/// A named, saved library that aggregates one or more source folders.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryProject {
    pub name: String,
    /// All source directories — may be on different drives or locations.
    pub paths: Vec<PathBuf>,
    /// Per-source indexing blacklist, keyed by source path (design §5.1, DATA-MODEL.md §1). A
    /// source with no entry (or an empty vec) indexes everything under it.
    #[serde(default)]
    pub blacklist: BTreeMap<PathBuf, Vec<Exclude>>,
    /// Sources imported as a **pack-only** container (FEATURES.md §5.2): the folder holds
    /// `.unitypackage` libraries, so it is tracked (a sidebar `⊞` row) but **not crawled** for
    /// loose files — its packs surface as in-pack libraries (DATA-MODEL §7 / M14) instead.
    #[serde(default)]
    pub pack_only: std::collections::BTreeSet<PathBuf>,
}

impl LibraryProject {
    pub fn live_paths(&self) -> Vec<&Path> {
        self.paths
            .iter()
            .filter(|p| p.is_dir())
            .map(|p| p.as_path())
            .collect()
    }

    /// Add a source path if not already present. Returns true if it was added.
    pub fn add_path(&mut self, path: PathBuf) -> bool {
        if self.paths.contains(&path) {
            return false;
        }
        self.paths.push(path);
        true
    }

    /// Remove a source path (and its blacklist). Returns true if it was present and removed.
    pub fn remove_path(&mut self, path: &Path) -> bool {
        let before = self.paths.len();
        self.paths.retain(|p| p != path);
        self.blacklist.remove(path);
        self.pack_only.remove(path);
        self.paths.len() != before
    }

    /// Is this source a pack-only container (imported packs, not crawled for loose files)?
    pub fn is_pack_only(&self, path: &Path) -> bool {
        self.pack_only.contains(path)
    }

    /// The exclusion list for a source (empty slice when none).
    pub fn excludes_for(&self, source: &Path) -> &[Exclude] {
        self.blacklist.get(source).map_or(&[], |v| v.as_slice())
    }

    /// Add an exclusion to a source. Returns true if it was added (deduped).
    pub fn add_exclude(&mut self, source: &Path, exclude: Exclude) -> bool {
        let list = self.blacklist.entry(source.to_path_buf()).or_default();
        if list.contains(&exclude) {
            return false;
        }
        list.push(exclude);
        true
    }

    /// Remove an exclusion from a source. Returns true if it was present.
    pub fn remove_exclude(&mut self, source: &Path, exclude: &Exclude) -> bool {
        let Some(list) = self.blacklist.get_mut(source) else {
            return false;
        };
        let before = list.len();
        list.retain(|e| e != exclude);
        let removed = list.len() != before;
        if list.is_empty() {
            self.blacklist.remove(source);
        }
        removed
    }
}

// ── View preferences ──────────────────────────────────────────────────────────

/// Browse view for the asset grid (DESIGN-GUIDELINES.md §7 "Browse views").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Flat,
    Folders,
    List,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewPrefs {
    pub thumb_size: f32,
    pub sort_by: SortBy,
    pub sort_asc: bool,
    #[serde(default)]
    pub mode: ViewMode,
    /// Thumbnail-grid assets per page (50/75/100 presets or a custom value).
    /// Persisted so a chosen page size survives restarts.
    #[serde(default = "default_page_size")]
    pub page_size: usize,
}

/// Default assets-per-page for the thumbnail grid.
pub const DEFAULT_PAGE_SIZE: usize = 50;
/// Custom page sizes are clamped to this range (0 would divide by zero; huge pages
/// defeat the point of paging).
pub const PAGE_SIZE_RANGE: std::ops::RangeInclusive<usize> = 10..=1000;

fn default_page_size() -> usize {
    DEFAULT_PAGE_SIZE
}

impl Default for ViewPrefs {
    fn default() -> Self {
        Self {
            thumb_size: 128.0,
            sort_by: SortBy::Name,
            sort_asc: true,
            mode: ViewMode::Flat,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }
}

// ── Svelte shell prefs ─────────────────────────────────────────────────────────

/// The Svelte shell's own persisted view state (FEATURES §10 / design README "State
/// Management"). Kept apart from the egui `ViewPrefs` until the B7 cutover retires it; the
/// egui build never reads this group.
///
/// B8c: the design keeps two independent axes the egui tri-state `ViewMode` collapsed —
/// **browse view** (Flat · Folders) × **row style** (Grid · List) — so both are stored here
/// as their own fields, alongside the density stop and the sort.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebPrefs {
    /// "flat" | "folders" — browse view (DESIGN-GUIDELINES §7 "Browse views").
    pub browse: String,
    /// "grid" | "list" — row style.
    pub row_style: String,
    /// "compact" | "cozy" | "large" — the 3-stop grid density (UI-SPEC §20.10).
    pub density: String,
    /// Sort key — `SortBy`'s serde name, which is what the frontend `SORTS` ids are.
    pub sort_by: SortBy,
    pub sort_asc: bool,
}

impl Default for WebPrefs {
    fn default() -> Self {
        Self {
            browse: "flat".into(),
            row_style: "grid".into(),
            density: "cozy".into(),
            sort_by: SortBy::Name,
            sort_asc: true,
        }
    }
}

// ── File-size tier labels ──────────────────────────────────────────────────────

/// The S / M / L / XL size badge tier shown on every thumbnail and used by the File-size filter.
/// Spec: design handoff §5.3, §3 "FILE SIZE".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeTier {
    S,
    M,
    L,
    Xl,
}

impl SizeTier {
    pub const ALL: [SizeTier; 4] = [SizeTier::S, SizeTier::M, SizeTier::L, SizeTier::Xl];

    pub fn letter(self) -> &'static str {
        match self {
            SizeTier::S => "S",
            SizeTier::M => "M",
            SizeTier::L => "L",
            SizeTier::Xl => "XL",
        }
    }
}

/// User-defined byte thresholds (in **KB**) behind the S/M/L/XL badge, plus the editable track cap.
/// Boundaries partition sizes as: `S < m_kb ≤ M < l_kb ≤ L < xl_kb ≤ XL`, with the track drawn up to
/// `cap_kb`. Spec: design handoff §5.3.
///
/// The invariant (ascending, ≥128 KB apart, within `[MIN_KB, MAX_CAP_KB]`) is enforced by
/// [`SizeTiers::normalized`] after any edit — pushing *larger* boundaries up, never smaller ones down.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeTiers {
    pub m_kb: u32,
    pub l_kb: u32,
    pub xl_kb: u32,
    pub cap_kb: u32,
}

impl SizeTiers {
    /// Smallest allowed boundary and minimum gap between adjacent boundaries (KB).
    pub const MIN_KB: u32 = 128;
    /// Minimum gap between adjacent boundaries and between XL and the cap (KB).
    pub const MIN_GAP_KB: u32 = 128;
    /// Hard upper bound for every boundary and the cap: 10 MB (design handoff §5.3, down from 50 MB).
    pub const MAX_CAP_KB: u32 = 10_240;

    /// Classify a byte size into its tier. Pure; the badge, the filter, and the legend all call it.
    pub fn tier_of(&self, bytes: u64) -> SizeTier {
        let kb = bytes / 1024;
        if kb < self.m_kb as u64 {
            SizeTier::S
        } else if kb < self.l_kb as u64 {
            SizeTier::M
        } else if kb < self.xl_kb as u64 {
            SizeTier::L
        } else {
            SizeTier::Xl
        }
    }

    /// Return a copy with the ascending / min-gap / bounds invariant enforced. Each boundary is
    /// clamped to at least `MIN_GAP_KB` above the previous one (pushing larger boundaries up), and
    /// the cap sits at least `MIN_GAP_KB` above XL — so ranges never overlap and the cap can never
    /// drop below the XL threshold. Idempotent.
    #[must_use]
    pub fn normalized(self) -> Self {
        let g = Self::MIN_GAP_KB;
        let max = Self::MAX_CAP_KB;
        // Leave headroom above each boundary for the ones that must sit above it (+cap).
        let m = self.m_kb.clamp(Self::MIN_KB, max - 3 * g);
        let l = self.l_kb.clamp(m + g, max - 2 * g);
        let xl = self.xl_kb.clamp(l + g, max - g);
        let cap = self.cap_kb.clamp(xl + g, max);
        Self {
            m_kb: m,
            l_kb: l,
            xl_kb: xl,
            cap_kb: cap,
        }
    }

    /// The resolved KB range `[lo, hi)` for a tier (hi = `cap_kb` for XL). For the legend.
    pub fn range_kb(&self, tier: SizeTier) -> (u32, u32) {
        match tier {
            SizeTier::S => (0, self.m_kb),
            SizeTier::M => (self.m_kb, self.l_kb),
            SizeTier::L => (self.l_kb, self.xl_kb),
            SizeTier::Xl => (self.xl_kb, self.cap_kb),
        }
    }
}

impl Default for SizeTiers {
    /// Design handoff §5.3 suggested defaults within 0–10 MB: S < 1 MB, M < 4 MB, L < 8 MB, XL ≥ 8 MB.
    fn default() -> Self {
        Self {
            m_kb: 1024,
            l_kb: 4096,
            xl_kb: 8192,
            cap_kb: 10_240,
        }
    }
}

// ── On-screen preview quality ───────────────────────────────────────────────────

/// Grid-thumbnail render sharpness — a GPU/perf tradeoff that affects **only the grid**. The
/// full-detail Quick Look preview always renders at full detail regardless of this. Spec: §5.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PreviewQuality {
    // Default to the low-fidelity grid the design shows (FEATURES §1.1/§5.2): grid cards are a
    // browse view, so render them cheap (128px) and reserve full detail for Quick Look. Raise it in
    // Settings ▸ Appearance for sharper cards at more VRAM / raster cost.
    #[default]
    Low,
    Medium,
    High,
}

impl PreviewQuality {
    pub const OPTIONS: &'static [(PreviewQuality, &'static str)] = &[
        (PreviewQuality::Low, "Low"),
        (PreviewQuality::Medium, "Medium"),
        (PreviewQuality::High, "High"),
    ];

    /// Grid thumbnail generation size (px) for this quality. Quick Look ignores this.
    pub fn thumb_px(self) -> u32 {
        match self {
            PreviewQuality::Low => 128,
            PreviewQuality::Medium => 256,
            PreviewQuality::High => 384,
        }
    }

    /// Triangle budget for the cached low-poly preview proxy (`mesh::load_preview_cached`). This is
    /// the half of "preview quality" that [`Self::thumb_px`] doesn't cover: resolution and polygon
    /// count are separate costs, and the mesh is the one that drives cache size and VRAM.
    pub fn proxy_budget(self) -> lodestash_ingest::decimate::Budget {
        use lodestash_ingest::decimate::Budget;
        match self {
            PreviewQuality::Low => Budget::new(0.02, 300, 1500),
            PreviewQuality::Medium => Budget::new(0.05, 600, 4000),
            PreviewQuality::High => Budget::new(0.12, 1200, 12000),
        }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

/// Default playback volume — a moderate level so audio previews aren't jarring.
fn default_true() -> bool {
    true
}

fn default_preview_cache_cap_gb() -> u32 {
    8
}

fn default_workspace_cache_cap_gb() -> u32 {
    10
}

fn default_rapid_index_max_gb() -> f32 {
    4.0
}

fn default_workspace_cache_retention_days() -> u32 {
    7
}

fn default_volume() -> f32 {
    0.5
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Name of the project that was open when the app last closed.
    pub last_project_name: Option<String>,
    /// Legacy single-root — kept for migration from older configs.
    #[serde(default)]
    pub last_root: Option<PathBuf>,
    /// Recently opened folders, newest first (capped at 10).
    #[serde(default)]
    pub recent_roots: Vec<PathBuf>,
    /// Named saved libraries the user can reopen from the empty state.
    pub projects: Vec<LibraryProject>,
    /// Persisted view preferences (zoom, sort).
    pub view: ViewPrefs,
    /// The Svelte shell's persisted view state (WebPrefs above). `#[serde(default)]` so a
    /// config.json written by the egui build loads unchanged.
    #[serde(default)]
    pub web: WebPrefs,
    /// File-size badge thresholds + editable cap (§5.3).
    #[serde(default)]
    pub size_tiers: SizeTiers,
    /// Grid-thumbnail render quality (§5.2). Grid-only; Quick Look is always full detail.
    #[serde(default)]
    pub preview_quality: PreviewQuality,
    /// Skip the first-run onboarding panel and open straight into the library (§1, §5.7).
    #[serde(default)]
    pub skip_onboarding: bool,
    /// Rehydrate extracted packages from the workspace on startup (UI-SPEC §11.6 Storage).
    /// Off (default): extractions are session-only; the workspace files still persist under
    /// the retention policy, so re-enabling brings them back without re-extracting.
    #[serde(default)]
    pub cache_extracted: bool,
    /// Preview (thumbnail/embedding) cache cap in gigabytes (UI-SPEC §5.5). Auto-purge drops
    /// least-recently-viewed previews when the cache exceeds this cap.
    #[serde(default = "default_preview_cache_cap_gb")]
    pub preview_cache_cap_gb: u32,
    /// Drop least-recently-viewed previews when the preview cache grows past the cap.
    #[serde(default = "default_true")]
    pub auto_purge_preview_cache: bool,
    /// Extracted-archive workspace cap in gigabytes (UI-SPEC §5.5).
    #[serde(default = "default_workspace_cache_cap_gb")]
    pub workspace_cache_cap_gb: u32,
    /// Max `.unitypackage` size (GB) that is indexed **in place** without asking. A bigger pack
    /// takes over a minute to inflate its manifest (DATA-MODEL §7 / M14), so opening it prompts to
    /// extract it to the temp cache instead. Default 4 GB.
    #[serde(default = "default_rapid_index_max_gb")]
    pub rapid_index_max_gb: f32,
    /// Age (days) after which extracted workspace files are eligible for purge.
    #[serde(default = "default_workspace_cache_retention_days")]
    pub workspace_cache_retention_days: u32,
    /// User-added Asset Store cache roots, checked after `ASSETSTORE_CACHE_PATH` and the
    /// per-OS default (INGESTION.md §2 discovery order; UI-SPEC §4).
    #[serde(default)]
    pub asset_store_roots: Vec<PathBuf>,
    /// Global audio-preview playback volume, `0.0..=1.0` (applies to every clip).
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// Watch the source roots and rescan when they change (DATA-MODEL §6). On by default;
    /// the rescan only fires while the library is idle, never mid-selection.
    #[serde(default = "default_true")]
    pub watch_sources: bool,
    /// Show a confirmation dialog before moving assets to Trash (UI-SPEC §11.6).
    /// On by default; trashed files are recoverable, but the shortcut is noisy.
    #[serde(default = "default_true")]
    pub confirm_before_trash: bool,
    /// Ollama model used for local tagging (AI-PIPELINE §2). `None` = tagging off — the drain is
    /// hours of CPU on a real library, so it never starts unasked.
    #[serde(default)]
    pub tagger_model: Option<String>,
    /// Optional vision model that captions the preview before the text model writes facets
    /// (AI-PIPELINE section 2). `None` = tag from the asset's name alone.
    #[serde(default)]
    pub tagger_vision_model: Option<String>,
    /// Run identification (SigLIP/CLAP embeddings + local-LLM tagging) automatically when new
    /// unidentified assets appear. **Default off** — identification is CPU-heavy (models run on
    /// CPU here), so it is a manual action unless the user opts into auto (AI-PIPELINE §2).
    #[serde(default)]
    pub auto_identify: bool,
    /// Opt-in cloud/remote tagging over the OpenAI-compatible vision shape (AI-PIPELINE §5). When
    /// on **and** a key + model are set, the visual tag pass uses the API instead of the local
    /// Ollama VLM. Off by default — the local path is free and private.
    #[serde(default)]
    pub api_tagging_enabled: bool,
    /// OpenAI-compatible base URL (default: Gemini's endpoint). Any provider or local server works.
    #[serde(default = "default_api_base_url")]
    pub api_base_url: String,
    /// The vision model id at that endpoint (e.g. a Gemini flash model). `None` = not configured.
    #[serde(default)]
    pub api_model: Option<String>,
    /// API key for the endpoint. A secret — never logged. Empty = not configured.
    #[serde(default)]
    pub api_key: String,
    /// Hard per-run cap on cloud-tagged assets (AI-PIPELINE §5.1 budget guard): a run stops after
    /// this many API calls and leaves the rest queued, so a mis-scoped Full Run on a paid endpoint
    /// can't become a surprise bill. Applies only to the API path; the local path is free.
    #[serde(default = "default_api_max_per_run")]
    pub api_max_per_run: u32,
    /// MCP/REST agent tools the user has switched **off** (UI-SPEC §16, card 178). Empty (the
    /// default) = every tool is live, so a fresh install behaves as before. A name here disables
    /// that tool on **both** agent planes at once (the gate is enforced in `lodestash-api`). Names
    /// are [`lodestash_api::AGENT_OPERATIONS`] tool ids.
    #[serde(default)]
    pub mcp_disabled_tools: Vec<String>,
}

fn default_api_max_per_run() -> u32 {
    500
}

/// The cloud API key at rest (AI-PIPELINE §5.1). On Windows the key lives in Credential Manager via
/// `keyring`; everywhere else these are no-ops and the caller keeps the plaintext-config fallback.
/// Every call is failure-tolerant — a locked or absent keychain never breaks config save/load.
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

fn default_api_base_url() -> String {
    lodestash_ai::openai::DEFAULT_BASE_URL.to_string()
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            last_project_name: None,
            last_root: None,
            recent_roots: Vec::new(),
            projects: Vec::new(),
            view: ViewPrefs::default(),
            web: WebPrefs::default(),
            size_tiers: SizeTiers::default(),
            preview_quality: PreviewQuality::default(),
            skip_onboarding: false,
            cache_extracted: false,
            preview_cache_cap_gb: default_preview_cache_cap_gb(),
            auto_purge_preview_cache: true,
            workspace_cache_cap_gb: default_workspace_cache_cap_gb(),
            rapid_index_max_gb: default_rapid_index_max_gb(),
            workspace_cache_retention_days: default_workspace_cache_retention_days(),
            asset_store_roots: Vec::new(),
            volume: default_volume(),
            watch_sources: true,
            confirm_before_trash: true,
            tagger_model: None,
            tagger_vision_model: None,
            auto_identify: false,
            api_tagging_enabled: false,
            api_base_url: default_api_base_url(),
            api_model: None,
            api_key: String::new(),
            api_max_per_run: default_api_max_per_run(),
            mcp_disabled_tools: Vec::new(),
        }
    }
}

impl AppConfig {
    const MAX_RECENT: usize = 10;

    /// The enabled agent tools as a set, or `None` when nothing is disabled (every tool live —
    /// the default). Feeds [`lodestash_api::ServerState::set_allowed`]; `None` leaves the state's
    /// construction default (allow-all) in place. A tool id in `mcp_disabled_tools` that isn't a
    /// real operation is simply ignored — it can only ever narrow the real inventory.
    pub fn mcp_enabled_tools(&self) -> Option<std::collections::HashSet<String>> {
        if self.mcp_disabled_tools.is_empty() {
            return None;
        }
        let disabled: std::collections::HashSet<&str> =
            self.mcp_disabled_tools.iter().map(String::as_str).collect();
        Some(
            lodestash_api::AGENT_OPERATIONS
                .iter()
                .map(|(tool, _)| *tool)
                .filter(|tool| !disabled.contains(tool))
                .map(str::to_string)
                .collect(),
        )
    }

    pub fn load() -> Self {
        let path = match config_path() {
            Some(p) => p,
            None => return Self::default(),
        };
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => return Self::default(),
        };
        let mut cfg: Self = serde_json::from_slice(&bytes).unwrap_or_default();
        // The API key lives in the OS credential store (AI-PIPELINE §5.1); the on-disk config keeps
        // it blank when that succeeded. If it's blank on disk, recover it from the keychain.
        if cfg.api_key.is_empty() {
            if let Some(key) = secret::load_api_key() {
                cfg.api_key = key;
            }
        }
        cfg
    }

    pub fn save(&self) {
        let path = match config_path() {
            Some(p) => p,
            None => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Key at rest (AI-PIPELINE §5.1): try the OS credential store first. If it took the key we
        // blank it in the on-disk JSON; if the store is unavailable, fall back to plaintext config so
        // the app still works. An empty key clears any stored secret.
        let stored_in_keychain = if self.api_key.trim().is_empty() {
            secret::clear_api_key();
            false
        } else {
            secret::store_api_key(&self.api_key)
        };
        let json = match serde_json::to_value(self) {
            Ok(mut v) => {
                if stored_in_keychain {
                    v["api_key"] = serde_json::Value::String(String::new());
                }
                v
            }
            Err(_) => return,
        };
        if let Ok(bytes) = serde_json::to_vec_pretty(&json) {
            let _ = std::fs::write(path, bytes);
        }
    }

    /// Return the project with this name, if any.
    pub fn project_by_name(&self, name: &str) -> Option<&LibraryProject> {
        self.projects.iter().find(|p| p.name == name)
    }

    /// Record that a single folder was opened as a new standalone project.
    /// Returns the project name it was saved under.
    pub fn record_open(&mut self, root: PathBuf) -> String {
        self.touch_recent(root.clone());
        let name = self.upsert_single_path_project(root);
        self.last_project_name = Some(name.clone());
        self.save();
        name
    }

    /// Record that a named multi-source project was opened.
    pub fn record_project_open(&mut self, name: &str) {
        self.last_project_name = Some(name.to_owned());
        if let Some(proj) = self.projects.iter().find(|p| p.name == name) {
            for path in proj.paths.clone() {
                self.touch_recent(path);
            }
        }
        self.save();
    }

    /// Add a source folder to an existing project by name.
    /// Returns false if the project wasn't found.
    pub fn add_source_to_project(&mut self, project_name: &str, path: PathBuf) -> bool {
        self.touch_recent(path.clone());
        if let Some(proj) = self.projects.iter_mut().find(|p| p.name == project_name) {
            proj.add_path(path);
            self.save();
            true
        } else {
            false
        }
    }

    /// Add a **pack-only** source to a project (FEATURES.md §5.2): tracked as a library folder
    /// but not crawled for loose files — its packs surface as in-pack libraries (M14). Persists.
    pub fn add_pack_only_source(&mut self, project_name: &str, path: PathBuf) -> bool {
        self.touch_recent(path.clone());
        if let Some(proj) = self.projects.iter_mut().find(|p| p.name == project_name) {
            proj.add_path(path.clone());
            proj.pack_only.insert(path);
            self.save();
            true
        } else {
            false
        }
    }

    /// Is `source` a pack-only container in `project_name`?
    pub fn is_pack_only(&self, project_name: &str, source: &Path) -> bool {
        self.project_by_name(project_name)
            .is_some_and(|p| p.is_pack_only(source))
    }

    /// The blacklist for a source in a project (cloned; empty when none). Design §5.1.
    pub fn source_excludes(&self, project_name: &str, source: &Path) -> Vec<Exclude> {
        self.project_by_name(project_name)
            .map(|p| p.excludes_for(source).to_vec())
            .unwrap_or_default()
    }

    /// Add a per-source exclusion (deduped) and persist. Returns true if added. Non-destructive —
    /// only affects cataloguing (DATA-MODEL.md §1, §6).
    pub fn add_source_exclude(
        &mut self,
        project_name: &str,
        source: &Path,
        exclude: Exclude,
    ) -> bool {
        if let Some(proj) = self.projects.iter_mut().find(|p| p.name == project_name) {
            if proj.add_exclude(source, exclude) {
                self.save();
                return true;
            }
        }
        false
    }

    /// Remove a per-source exclusion and persist. Returns true if it was present.
    pub fn remove_source_exclude(
        &mut self,
        project_name: &str,
        source: &Path,
        exclude: &Exclude,
    ) -> bool {
        if let Some(proj) = self.projects.iter_mut().find(|p| p.name == project_name) {
            if proj.remove_exclude(source, exclude) {
                self.save();
                return true;
            }
        }
        false
    }

    /// Remove a source folder from an existing project by name. Never touches the files on
    /// disk — only the catalog's reference to them (DATA-MODEL.md §6 non-destructive rule).
    /// Returns false if the project or the path wasn't found.
    pub fn remove_source_from_project(&mut self, project_name: &str, path: &Path) -> bool {
        if let Some(proj) = self.projects.iter_mut().find(|p| p.name == project_name) {
            let removed = proj.remove_path(path);
            if removed {
                self.save();
            }
            removed
        } else {
            false
        }
    }

    /// Add `path` to saved projects under its folder name if not already in any project.
    /// Returns the project name it was added to (or already existed under).
    fn upsert_single_path_project(&mut self, path: PathBuf) -> String {
        if let Some(proj) = self.projects.iter().find(|p| p.paths.contains(&path)) {
            return proj.name.clone();
        }
        let name = stem_name(&path);
        // Avoid duplicate project names by appending a counter.
        let unique_name = if self.projects.iter().any(|p| p.name == name) {
            format!("{name} ({})", self.projects.len())
        } else {
            name
        };
        self.projects.push(LibraryProject {
            name: unique_name.clone(),
            paths: vec![path],
            blacklist: BTreeMap::new(),
            pack_only: Default::default(),
        });
        unique_name
    }

    fn touch_recent(&mut self, path: PathBuf) {
        self.last_root = Some(path.clone());
        self.recent_roots.retain(|r| r != &path);
        self.recent_roots.insert(0, path);
        self.recent_roots.truncate(Self::MAX_RECENT);
    }
}

pub fn stem_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn config_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("Lodestash").join("config.json"))
}

/// Where `config.json` lives on this OS — surfaced in the UI so the user can find/back up their
/// project list without hunting through `dirs::config_dir()` conventions themselves.
pub fn config_file_path() -> Option<PathBuf> {
    config_path()
}

// ── Tests ─────────────────────────────────────────────────────────────────────
// Pure project/recent logic only. The public record_*/add_source_* wrappers call
// save() which writes to the real OS config dir, so tests exercise the private
// helpers (upsert_single_path_project, touch_recent) and add_path directly — never
// save() — to avoid clobbering a developer's actual config.
// Spec: TECH-STACK.md §5.1 (multi-source projects, this session's feature work).

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn upsert_names_project_from_folder_stem() {
        let mut cfg = AppConfig::default();
        let name = cfg.upsert_single_path_project(p("/assets/Synty"));
        assert_eq!(name, "Synty");
        assert_eq!(cfg.projects.len(), 1);
        assert_eq!(cfg.projects[0].paths, vec![p("/assets/Synty")]);
    }

    #[test]
    fn upsert_is_idempotent_for_same_path() {
        let mut cfg = AppConfig::default();
        let a = cfg.upsert_single_path_project(p("/assets/Synty"));
        let b = cfg.upsert_single_path_project(p("/assets/Synty"));
        assert_eq!(a, b);
        assert_eq!(
            cfg.projects.len(),
            1,
            "same path must not create a second project"
        );
    }

    #[test]
    fn upsert_disambiguates_same_stem_different_path() {
        let mut cfg = AppConfig::default();
        let a = cfg.upsert_single_path_project(p("/a/Synty"));
        let b = cfg.upsert_single_path_project(p("/b/Synty"));
        assert_eq!(a, "Synty");
        assert_ne!(
            a, b,
            "same folder name from a different path gets a unique label"
        );
        assert_eq!(cfg.projects.len(), 2);
    }

    #[test]
    fn mcp_enabled_tools_none_when_empty_and_narrows_the_real_inventory() {
        let mut cfg = AppConfig::default();
        assert!(
            cfg.mcp_enabled_tools().is_none(),
            "no disabled tools ⇒ allow-all (None)"
        );

        cfg.mcp_disabled_tools = vec!["export_asset".into(), "not_a_real_tool".into()];
        let enabled = cfg.mcp_enabled_tools().unwrap();
        assert!(!enabled.contains("export_asset"), "disabled tool dropped");
        assert!(enabled.contains("search_assets"), "others kept");
        assert!(
            !enabled.contains("not_a_real_tool"),
            "a bogus name can only narrow, never add"
        );
        assert_eq!(
            enabled.len(),
            lodestash_api::AGENT_OPERATIONS.len() - 1,
            "exactly one real tool removed"
        );
    }

    #[test]
    fn add_path_dedups() {
        let mut proj = LibraryProject {
            name: "Synty".into(),
            paths: vec![p("/a")],
            blacklist: BTreeMap::new(),
            pack_only: Default::default(),
        };
        assert!(proj.add_path(p("/b")), "new path is added");
        assert!(!proj.add_path(p("/a")), "existing path is not re-added");
        assert_eq!(proj.paths, vec![p("/a"), p("/b")]);
    }

    #[test]
    fn remove_path_removes_present_and_reports_absent() {
        let mut proj = LibraryProject {
            name: "Synty".into(),
            paths: vec![p("/a"), p("/b")],
            blacklist: BTreeMap::new(),
            pack_only: Default::default(),
        };
        assert!(proj.remove_path(&p("/a")));
        assert_eq!(proj.paths, vec![p("/b")]);
        assert!(
            !proj.remove_path(&p("/a")),
            "already removed, nothing to do"
        );
    }

    #[test]
    fn pack_only_source_is_tracked_but_flagged_and_cleared_on_remove() {
        let mut proj = LibraryProject {
            name: "Cache".into(),
            paths: vec![],
            blacklist: BTreeMap::new(),
            pack_only: Default::default(),
        };
        proj.add_path(p("/packs"));
        proj.pack_only.insert(p("/packs"));
        assert!(proj.is_pack_only(&p("/packs")), "flagged pack-only");
        assert!(!proj.is_pack_only(&p("/other")), "plain source is crawled");
        // Removing the source clears its pack-only flag too (no dangling marker).
        assert!(proj.remove_path(&p("/packs")));
        assert!(!proj.is_pack_only(&p("/packs")));
        assert!(proj.pack_only.is_empty());
    }

    #[test]
    fn touch_recent_is_newest_first_deduped_and_capped() {
        let mut cfg = AppConfig::default();
        for i in 0..15 {
            cfg.touch_recent(p(&format!("/r{i}")));
        }
        assert_eq!(cfg.recent_roots.len(), AppConfig::MAX_RECENT, "capped");
        assert_eq!(cfg.recent_roots[0], p("/r14"), "most recent is first");

        // Re-touching an existing path moves it to front without duplicating.
        cfg.touch_recent(p("/r10"));
        assert_eq!(cfg.recent_roots[0], p("/r10"));
        assert_eq!(
            cfg.recent_roots.iter().filter(|r| **r == p("/r10")).count(),
            1,
            "no duplicate entry"
        );
    }

    #[test]
    fn sortby_label_round_trips_options() {
        for &(s, label) in SortBy::OPTIONS {
            assert_eq!(s.label(), label);
        }
    }

    // ── Size tiers ──────────────────────────────────────────────────────────

    #[test]
    fn size_tier_of_classifies_at_default_thresholds() {
        let t = SizeTiers::default(); // S<1MB, M<4MB, L<8MB, XL≥8MB
        let kb = |n: u64| n * 1024;
        assert_eq!(t.tier_of(kb(512)), SizeTier::S, "512 KB is S");
        assert_eq!(
            t.tier_of(kb(1024)),
            SizeTier::M,
            "exactly 1 MB crosses into M"
        );
        assert_eq!(
            t.tier_of(kb(4096)),
            SizeTier::L,
            "exactly 4 MB crosses into L"
        );
        assert_eq!(
            t.tier_of(kb(8192)),
            SizeTier::Xl,
            "exactly 8 MB crosses into XL"
        );
        assert_eq!(
            t.tier_of(kb(50_000)),
            SizeTier::Xl,
            "way over cap is still XL"
        );
        assert_eq!(t.tier_of(0), SizeTier::S, "zero bytes is S");
    }

    #[test]
    fn normalized_enforces_ascending_min_gap_and_bounds() {
        // Deliberately inverted + out-of-bounds input.
        let t = SizeTiers {
            m_kb: 9000,
            l_kb: 10,
            xl_kb: 50,
            cap_kb: 20,
        }
        .normalized();
        let g = SizeTiers::MIN_GAP_KB;
        assert!(t.m_kb >= SizeTiers::MIN_KB);
        assert!(t.l_kb >= t.m_kb + g, "M|L gap held");
        assert!(t.xl_kb >= t.l_kb + g, "L|XL gap held");
        assert!(t.cap_kb >= t.xl_kb + g, "cap sits above XL");
        assert!(t.cap_kb <= SizeTiers::MAX_CAP_KB, "cap within 10 MB");
    }

    #[test]
    fn normalized_is_idempotent_and_noop_on_defaults() {
        let d = SizeTiers::default();
        assert_eq!(d.normalized(), d, "defaults already satisfy the invariant");
        assert_eq!(d.normalized().normalized(), d.normalized(), "idempotent");
    }

    #[test]
    fn normalized_pushes_larger_boundaries_up_not_smaller_down() {
        // Raise M above L: L, XL, cap must be pushed up; M stays where the user put it.
        let t = SizeTiers {
            m_kb: 5000,
            l_kb: 4096,
            xl_kb: 8192,
            cap_kb: 10_240,
        }
        .normalized();
        assert_eq!(t.m_kb, 5000, "the edited (smaller) boundary is honored");
        assert!(t.l_kb >= 5000 + SizeTiers::MIN_GAP_KB, "L pushed above M");
    }

    #[test]
    fn range_kb_xl_upper_bound_follows_the_cap() {
        let t = SizeTiers {
            cap_kb: 9000,
            ..SizeTiers::default()
        }
        .normalized();
        assert_eq!(t.range_kb(SizeTier::Xl), (t.xl_kb, t.cap_kb));
        assert_eq!(t.range_kb(SizeTier::S).0, 0);
    }

    #[test]
    fn per_source_blacklist_add_dedup_remove_and_isolation() {
        let mut proj = LibraryProject {
            name: "Synty".into(),
            paths: vec![p("/a"), p("/b")],
            blacklist: BTreeMap::new(),
            pack_only: Default::default(),
        };
        let ex = |v: &str| Exclude {
            kind: ExcludeKind::Folder,
            value: v.into(),
        };
        assert!(proj.add_exclude(&p("/a"), ex("Editor")));
        assert!(!proj.add_exclude(&p("/a"), ex("Editor")), "deduped");
        assert!(proj.add_exclude(&p("/a"), ex("Gizmos")));
        assert_eq!(proj.excludes_for(&p("/a")).len(), 2);
        assert!(
            proj.excludes_for(&p("/b")).is_empty(),
            "each source owns its own list"
        );

        assert!(proj.remove_exclude(&p("/a"), &ex("Editor")));
        assert_eq!(proj.excludes_for(&p("/a")).len(), 1);
        assert!(
            !proj.remove_exclude(&p("/a"), &ex("Editor")),
            "already gone"
        );

        // Removing the source drops its blacklist entry.
        proj.remove_path(&p("/a"));
        assert!(proj.excludes_for(&p("/a")).is_empty());
    }

    #[test]
    fn preview_quality_px_increases_with_quality() {
        assert!(PreviewQuality::Low.thumb_px() < PreviewQuality::Medium.thumb_px());
        assert!(PreviewQuality::Medium.thumb_px() < PreviewQuality::High.thumb_px());
        // The grid defaults to the low-fidelity preview the design shows (FEATURES §1.1/§5.2).
        assert_eq!(PreviewQuality::default(), PreviewQuality::Low);
    }
}
