//! The Settings screen's state (UI-SPEC §11, v2 spec §4.5), one type for reading and writing it.
//!
//! **Sections in Rust, one flat object on the wire.** Each section is its own struct, so the engine
//! reads and applies them independently; every section is `#[serde(flatten)]`ed, so the JSON is
//! the flat object the old shell's `get_settings`/`set_settings` already exchange. A write sends
//! the whole object.
//!
//! **The cloud API key is write-only** (review U8). A read never carries it: `api_key` is absent
//! and `api_key_configured` says whether one is stored. On a write, an absent, `null`, empty or
//! blank `api_key` leaves the stored key alone and any other value replaces it; only
//! `clear_api_key: true` removes it (ADR-0024). `api_key_configured` is ignored.
//!
//! **What a write leaves out must not open anything** (ADR-0024). Most fields are required. The
//! ones a write may omit either turn something *off* when absent (`api_tagging_enabled`,
//! `remote_image_bytes`, `mcp_bytes_enabled`, the model ids) or change nothing (`api_key`,
//! `api_max_per_run`, `api_base_url`). None has a default of its own that could disagree with the
//! config's — an omitted cap used to read as `0`, uncapped. `tests/dto_wire.rs` lists every
//! optional field and what its absence means; a new one fails that test until it is listed.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Everything the Settings screen edits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Settings {
    #[serde(flatten)]
    pub appearance: AppearanceSettings,
    #[serde(flatten)]
    pub library: LibrarySettings,
    #[serde(flatten)]
    pub identify: IdentifySettings,
    #[serde(flatten)]
    pub cloud: CloudSettings,
    #[serde(flatten)]
    pub agents: AgentSettings,
    #[serde(flatten)]
    pub storage: StorageSettings,
}

/// Grid-only preview render quality (UI-SPEC §11.3). Quick Look always renders full detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewQuality {
    Low,
    Medium,
    High,
}

/// ON-SCREEN QUALITY and FILE-SIZE LABELS (UI-SPEC §11.3, §11.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AppearanceSettings {
    /// Size-tier boundaries in KB: S < m < l < xl < cap, 128 KB minimum gap. A write is normalised
    /// to that invariant, so what is read back can differ from what was sent.
    pub m_kb: u32,
    pub l_kb: u32,
    pub xl_kb: u32,
    pub cap_kb: u32,
    pub preview_quality: PreviewQuality,
}

/// WATCHED SOURCES and the shell's own switches (UI-SPEC §11.1, §11.7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LibrarySettings {
    /// Rescan the watched sources when their files change (DATA-MODEL §6).
    pub watch_sources: bool,
    /// Ask before moving assets to Trash.
    pub confirm_before_trash: bool,
    /// Skip the first-run onboarding panel (the UI shows the inverse, "Show first-run setup").
    pub skip_onboarding: bool,
}

/// IDENTIFY PROFILE (UI-SPEC §11.2, AI-PIPELINE §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IdentifySettings {
    /// Appraise new and changed assets in the background.
    pub auto_identify: bool,
    /// Ollama text model for local tagging; `null` = no local LLM.
    #[serde(default)]
    pub tagger_model: Option<String>,
    /// Ollama vision model that captions the preview first; `null` = tag from the name alone.
    #[serde(default)]
    pub tagger_vision_model: Option<String>,
}

/// Opt-in cloud tagging over an OpenAI-compatible endpoint (AI-PIPELINE §5) and the consent it
/// needs to send pixels anywhere remote (ADR-0020).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CloudSettings {
    #[serde(default)]
    pub api_tagging_enabled: bool,
    /// The OpenAI-compatible endpoint. Always present on a read. On a write, absent (or `null`)
    /// keeps the stored endpoint — it is never swapped for another the stored key would then be
    /// sent to.
    #[serde(default)]
    pub api_base_url: Option<String>,
    /// The vision model id at that endpoint; `null` = not configured.
    #[serde(default)]
    pub api_model: Option<String>,
    /// Write-only: never present on a read. A value replaces the stored key; absent, `null`,
    /// empty or blank leaves it as it is. To remove the key, send `clear_api_key`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Write-only: `true` removes the stored key — the only thing that does. It wins over an
    /// `api_key` sent in the same write. Never present on a read.
    #[serde(default, skip_serializing_if = "is_false")]
    pub clear_api_key: bool,
    /// Read-only: whether a key is stored. Ignored on a write.
    #[serde(default)]
    pub api_key_configured: bool,
    /// Hard per-run cap on cloud-tagged assets (AI-PIPELINE §5.1); `0` = uncapped. Always present
    /// on a read. On a write, absent (or `null`) keeps the stored cap: leaving it out never
    /// uncaps a run.
    #[serde(default)]
    pub api_max_per_run: Option<u32>,
    /// May image bytes, and pixels rendered from models, materials and sprites, go to a remote
    /// endpoint? Off by default; a loopback endpoint never needs it.
    #[serde(default)]
    pub remote_image_bytes: bool,
}

fn is_false(flag: &bool) -> bool {
    !*flag
}

/// MODEL CONTEXT PROTOCOL (UI-SPEC §11.5): what an agent plane may call and receive.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSettings {
    /// Agent tools switched off — the disabled half of the allow-list. Empty = every tool live.
    pub mcp_disabled_tools: Vec<String>,
    /// May an agent plane receive asset bytes (previews, originals), not only metadata and links?
    #[serde(default)]
    pub mcp_bytes_enabled: bool,
}

/// PREVIEW CACHE and EXTRACTED PACKS (UI-SPEC §11.6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StorageSettings {
    /// Keep every extracted pack cached.
    pub cache_extracted: bool,
    pub preview_cache_cap_gb: u32,
    pub auto_purge_preview_cache: bool,
}
