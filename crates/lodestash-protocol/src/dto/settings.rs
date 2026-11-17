//! The Settings screen's state (UI-SPEC §11, v2 spec §4.5), one type for reading and writing it.
//!
//! **Sections in Rust, one flat object on the wire.** Each section is its own struct, so the engine
//! reads and applies them independently; every section is `#[serde(flatten)]`ed, so the JSON is
//! the flat object the old shell's `get_settings`/`set_settings` already exchange. A write sends
//! the whole object.
//!
//! **The cloud API key is write-only** (review U8). A read never carries it: `api_key` is absent
//! and `api_key_configured` says whether one is stored. On a write, an absent (or `null`)
//! `api_key` keeps the stored key, `""` clears it, and any other value replaces it;
//! `api_key_configured` is ignored.

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
    #[serde(default)]
    pub api_base_url: String,
    /// The vision model id at that endpoint; `null` = not configured.
    #[serde(default)]
    pub api_model: Option<String>,
    /// Write-only: never present on a read. Absent or `null` keeps the stored key, `""` clears it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// Read-only: whether a key is stored. Ignored on a write.
    #[serde(default)]
    pub api_key_configured: bool,
    /// Hard per-run cap on cloud-tagged assets (AI-PIPELINE §5.1); `0` = uncapped.
    #[serde(default)]
    pub api_max_per_run: u32,
    /// May image bytes, and pixels rendered from models, materials and sprites, go to a remote
    /// endpoint? Off by default; a loopback endpoint never needs it.
    #[serde(default)]
    pub remote_image_bytes: bool,
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
