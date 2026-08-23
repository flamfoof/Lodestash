//! Read-only view of the catalog for the desktop shell.
//!
//! This is deliberately **not** `Engine`. It opens its own connection and never writes:
//! the single-writer rule (ARCHITECTURE §5) says every catalog write goes through the persist
//! thread, and nothing here has one. That constraint is what lets the Svelte frontend show a
//! real library before the B1 extraction finishes — browsing is a read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lodestash_app::config::{SizeTier, SizeTiers, SortBy};
use lodestash_app::grid;
use lodestash_app::types::{Category, ContentFilter, StatusFilter, TriBucket};
use lodestash_core::catalog::{AppraiseFocus, Catalog, TagAssignment};
use lodestash_core::types::{AssetStatus, MediaType};
use serde::{Deserialize, Serialize};

/// One asset as the grid needs it. Carries a `preview_key`, never bytes — the frontend fetches
/// pixels over `lodestash://`, so a page of 100 cards is a few KB of JSON rather than a few MB.
#[derive(Debug, Clone, Serialize)]
pub struct GridItem {
    pub asset_id: i64,
    /// File stem, which is what the card shows.
    pub name: String,
    pub rel_path: String,
    /// `media_type.code` — the frontend maps it to an `--ls-type-*` token and a sprite id.
    pub media_type: String,
    pub size_bytes: i64,
    pub status: String,
    /// True when `asset_preview` holds a traced vector preview for this asset (M15).
    pub has_preview: bool,
    /// Starred (DATA-MODEL §12). Carried on the item so a card can draw the pin without a
    /// second round-trip per asset.
    pub favorite: bool,
    /// The bytes are still inside a `.unitypackage` (DATA-MODEL §7 / M14). Browsable and
    /// searchable, but `lodestash://file` answers 503 rather than extracting — pulling is a command.
    pub in_pack: bool,
    /// Owning pack name, for in-pack assets.
    pub package: Option<String>,
    /// Already imported into the **active** project (`import_record`, DATA-MODEL §9) — the
    /// `✓ in <project>` badge. Filled in one query per reload by [`Library::mark_imported`], not
    /// per card: the data was always there, the bulk read was not (ALIGNMENT-AUDIT N1.5).
    pub in_project: bool,
    /// Imported here, but the source file has changed on disk since — `✓ older copy`, the one
    /// actionable state of the four (DESIGN-GUIDELINES §7).
    pub import_stale: bool,
    /// How many *other* projects hold it. Reads as `✓ in N others`: history, not a warning.
    pub in_other_projects: i64,
    pub triangles: Option<i64>,
    /// Clip length for audio, so a sound card can print `pos / dur` **before** the first play
    /// (UI-SPEC §2.5) and a list row can show its length in the stats column. `None` for
    /// everything else, and for audio the metrics drain has not reached yet.
    pub duration_ms: Option<i64>,
    /// Tag values on this asset — the chips under the card's meta line (FEATURES §1.1). Filled by
    /// [`Library::mark_traits`] in one query for the whole library, not one per card.
    pub traits: Vec<String>,
    /// The `import_source` this row belongs to. Carried so the frontend can look the source's
    /// availability up in the list it already has (`LibraryStatus.sources`) instead of the backend
    /// stamping a second copy of `state` onto 8k cards — the plate, the inert `⤴` and the Quick Look
    /// "isn't reachable" panel all read the same one fact (UI-SPEC §11.1).
    pub source_id: i64,
    /// `S` / `M` / `L` / `XL`, from the user's own thresholds rather than a fixed scale, so the
    /// badge means what Settings says it means (FEATURES §5.3).
    pub size_tier: String,
}

/// One grid request from the frontend.
///
/// The three facets deserialize **into the shared enums** (`all | models | …`,
/// `all | unidentified | …`, `name | type | …`) rather than arriving as strings this crate
/// re-matches. That is what keeps the two frontends answering a query the same way: the tab
/// grouping, the status fold and the sort vocabulary each have exactly one definition, in
/// `lodestash-app`. An id outside those sets is a frontend bug and fails the command loudly —
/// silently falling back to "All" is how a broken facet ships unnoticed.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GridQuery {
    pub tab: Category,
    /// Substring match over the name — the Tags-mode filter, before FTS is wired.
    pub search: String,
    /// ⟨Tags⟩/⟨AI⟩ (TopBar): "tags" = FTS ranks; "ai" = RRF-fused FTS + query embeddings
    /// (hybrid_search). Anything else behaves as tags.
    pub search_mode: String,
    /// Restrict to exactly these asset ids — the Identify screen's "just appraised" session
    /// set. Empty = no restriction.
    pub ids: Vec<i64>,
    /// `discovered` and `indexed` both fold into Unidentified (DATA-MODEL §8).
    pub status: StatusFilter,
    pub sort_by: SortBy,
    pub sort_asc: bool,
    /// Restrict to starred assets — what the Favorites screen is (FEATURES §4.7). A one-way
    /// switch: "not favourited" is not a thing anyone browses for.
    pub favorites_only: bool,
    /// Show the Trash instead of the library. Not a facet you combine with the others — a
    /// soft-deleted asset is absent from every normal view by definition (DATA-MODEL §8).
    pub trashed_only: bool,
    /// The ⚑ Filters popover (UI-SPEC §2.3): traits, triangle buckets, size tiers. The **shared**
    /// type, so `ContentFilter::passes` is the one definition of what passing means.
    pub filter: ContentFilter,
    /// Restrict to one pack — what the Packages screen's **Browse →** sets (UI-SPEC §4). Shown as
    /// a clearable chip above the grid: a filter you cannot see is a library that has quietly gone
    /// missing.
    pub package: Option<String>,
    /// Restrict to one **source library** — the sidebar's LIBRARIES facet (FEATURES §1.2).
    /// Distinct from `package`: a source is a watched root on disk, a package is one
    /// `.unitypackage` inside it, and a source usually holds many.
    pub source: Option<String>,
    /// Restrict to one collection's members — what the Collections screen's **Browse →** sets
    /// (UI-SPEC §6), shown as a clearable chip like the pack filter.
    pub collection: Option<i64>,
    /// Restrict to assets sharing an `asset_relationship` edge with this one — the ⚑ Filters
    /// LINKS section (FEATURES §1.4 / DATA-MODEL §10). Set to the selected asset's id; the grid
    /// then shows only its linked neighbours in both directions. `None` means no link filter.
    pub links: Option<i64>,
    pub page: usize,
    pub page_size: usize,
}

impl Default for GridQuery {
    fn default() -> Self {
        Self {
            tab: Category::All,
            search: String::new(),
            search_mode: "tags".to_string(),
            ids: Vec::new(),
            status: StatusFilter::All,
            sort_by: SortBy::Name,
            sort_asc: true,
            favorites_only: false,
            trashed_only: false,
            filter: ContentFilter::default(),
            package: None,
            source: None,
            collection: None,
            links: None,
            page: 0,
            page_size: 100,
        }
    }
}

/// Where one in-pack payload lives inside its `.unitypackage` — everything
/// `thumbs::ensure_in_pack_scratch` needs to pull it without going back to the catalog.
#[derive(Debug, Clone)]
pub struct InPackLocator {
    pub pack_path: String,
    pub offset: i64,
    pub size: i64,
    /// The asset's own extension, so the pulled scratch file decodes as what it is rather than
    /// as the archive it came out of.
    pub ext: String,
}

/// What the thumbnail handler needs, lifted out of the `Library` lock in one go.
pub struct ThumbJob {
    pub path: Option<PathBuf>,
    pub media_type: String,
    /// Only set for materials: the texture its shader ball wears.
    pub albedo: Option<PathBuf>,
    /// `Some` when the bytes are still inside a pack — the handler decides whether pulling them
    /// is worth it (see `protocol::in_pack_scratch`).
    pub in_pack: Option<InPackLocator>,
}

/// Everything the Inspector shows for one asset (UI-SPEC §13). Separate from `GridItem` so a
/// page of 100 cards never carries per-asset detail nobody is looking at.
#[derive(Debug, Clone, Serialize)]
pub struct AssetDetail {
    pub asset_id: i64,
    pub name: String,
    pub rel_path: String,
    /// Absolute path, for "reveal in explorer" and to show where it actually lives.
    pub abs_path: String,
    pub media_type: String,
    pub size_bytes: i64,
    pub status: String,
    pub has_preview: bool,
    pub triangles: Option<i64>,
    pub width_px: Option<i64>,
    pub height_px: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Which source root it came from.
    pub source: Option<String>,
    /// …and its id, so Quick Look can read that source's availability out of `sources` and say
    /// *when* it was last seen rather than only that the file will not open (UI-SPEC §11.1).
    pub source_id: i64,
    /// The albedo texture this model paints with, reached through the dependency graph
    /// (DATA-MODEL §10). `None` when the graph has no edge — the viewer then renders flat.
    pub texture_asset_id: Option<i64>,
    /// True when the file is not on disk — an in-pack asset whose bytes are still in the archive.
    pub in_pack: bool,
    /// This asset's tags (UI-SPEC §9): `confidence` is set for AI-tagged rows, so the Inspector
    /// can show which labels were guessed vs hand-added.
    pub tags: Vec<TagAssignment>,
    /// Starred, and soft-deleted. The Inspector draws its actions from these — a trashed asset
    /// offers Restore, not another Trash button.
    pub favorite: bool,
    pub trashed: bool,
}

/// One export run for the Recent-exports list (FEATURES §7.1).
#[derive(Debug, Clone, Serialize)]
pub struct ExportBatchRow {
    pub batch_id: String,
    pub dest_path: String,
    /// Files written — what undo would delete, not how many assets were picked.
    pub files: i64,
    pub exported_at: i64,
}

/// The relink panel's headline number (UI-SPEC §11.1) — *"4,198 of 4,206 files matched by relative
/// path — 8 would stay unavailable"*. Shown **before** the button commits.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RelinkCount {
    pub matched: i64,
    pub total: i64,
}

#[derive(Debug, Serialize)]
pub struct GridPage {
    pub items: Vec<GridItem>,
    /// Matches before paging, so the pager can size itself.
    pub total: usize,
    pub page: usize,
    pub pages: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceSummary {
    pub id: i64,
    pub name: String,
    pub root_path: String,
    pub asset_count: i64,
    /// Whether this source is being watched for changes (UI-SPEC §11.1) — drives the
    /// pause/resume toggle on the Settings screen.
    pub watch_enabled: bool,
    /// `available` | `paused` | `unavailable` (M18, UI-SPEC §11.1). An unavailable source keeps
    /// every row it owns: the catalog stays browsable, only the files are out of reach.
    pub state: String,
    /// Unix seconds the root last resolved — "unavailable since …".
    pub last_seen_at: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct LibraryStatus {
    pub open: bool,
    pub catalog_path: Option<String>,
    pub sources: Vec<SourceSummary>,
    pub total_assets: usize,
    /// Assets whose bytes are still inside a `.unitypackage`. On a real Unity library this is
    /// the majority — 5,963 of 7,691 here — so a zero means a hydrate query is missing, not
    /// that the library has no packs.
    pub in_pack: usize,
    pub favorites: usize,
    /// Assets carrying a stored vector preview — the honest measure of how much of the grid
    /// can draw without touching a mesh.
    pub with_preview: usize,
    /// Soft-deleted assets waiting in the Trash. Drives the nav count.
    pub trashed: usize,
    /// The STATUS facet group's counts (FEATURES §0.3 / §1.2) and the Identify rail badge.
    ///
    /// Folded through the **same** [`StatusFilter::matches`] the grid applies, so the number on a
    /// facet can never disagree with the number of cards clicking it produces.
    pub unidentified: usize,
    pub identified: usize,
    pub confirmed: usize,
    /// Groups, not assets: the Duplicates rail badge counts *sets* of matching files, so two
    /// copies of one texture is one badge, not two.
    pub duplicate_groups: usize,
    /// The type-tab row's counts (UI-SPEC §0.3), derived with the same [`Category::matches`] the
    /// grid applies — a tab's count is the number of cards clicking it produces. `All` equals
    /// `total_assets`; a bucket with zero hides its tab in the top bar.
    pub tab_counts: Vec<TabCount>,
}

/// One type-tab count (UI-SPEC §0.3). `id` is the frontend tab id — `Category::id()`.
#[derive(Debug, Serialize)]
pub struct TabCount {
    pub id: String,
    pub count: usize,
}

/// One level of the Folders view (DESIGN-GUIDELINES §7 "Browse views").
///
/// `source: None` is the top level, where the tiles are the **sources** themselves — "assets from
/// many sources must never blur into one anonymous pile". Below that, `dir` walks the source's own
/// directories. Search narrows the current **subtree** rather than the whole library, and it
/// narrows the tile counts with it: a tile promising 40 assets that opens onto 2 is worse than no
/// count at all.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FolderQuery {
    pub tab: Category,
    pub search: String,
    pub status: StatusFilter,
    pub filter: ContentFilter,
    /// Which source we are inside; `None` at the top level.
    pub source: Option<String>,
    /// Source-root-relative directory, `/`-separated. Empty is the source's root.
    pub dir: String,
    /// When set, the Folders tree is **rooted at this pack** (PACK-DIRECTORY-NAV): `dir` is then
    /// relative to the pack's top folder and only that pack's assets are browsed. The pack's own
    /// name never appears as a breadcrumb segment — the pack *is* the root.
    pub package: Option<String>,
}

impl Default for FolderQuery {
    fn default() -> Self {
        Self {
            tab: Category::All,
            search: String::new(),
            status: StatusFilter::All,
            filter: ContentFilter::default(),
            source: None,
            dir: String::new(),
            package: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct FolderTileOut {
    pub name: String,
    /// Assets directly inside it, and everything under it recursively — shown as `direct (recursive)`.
    pub direct: usize,
    pub recursive: usize,
}

/// The Settings screen's payload (UI-SPEC §11, design README §5). A whole-object snapshot: the
/// frontend edits it and writes it back through `set_settings`, mirroring how `WebPrefs` works.
/// Strings are used where the frontend's own ids are the vocabulary ("low"/"medium"/"high").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    /// Size-tier boundaries in KB (UI-SPEC §11.4): S < m < l < xl < cap, 128 KB minimum gap.
    pub m_kb: u32,
    pub l_kb: u32,
    pub xl_kb: u32,
    pub cap_kb: u32,
    /// Grid-only preview render quality (UI-SPEC §11.3). Quick Look is always full detail.
    pub preview_quality: String,
    /// Inverted by the UI: "Show first-run onboarding on launch" (§11.7/§1).
    pub skip_onboarding: bool,
    pub watch_sources: bool,
    pub auto_identify: bool,
    /// Keep extracted packs cached (UI-SPEC §11.6).
    pub cache_extracted: bool,
    pub preview_cache_cap_gb: u32,
    pub auto_purge_preview_cache: bool,
    /// Ask before trashing (UI-SPEC §11 "confirm before trash"): when on, the trash entry
    /// points (inspector, bulk bar) require a second confirming click.
    pub confirm_before_trash: bool,
    /// The MCP allow-list, stored as the disabled half (UI-SPEC §11.5, MCP↔REST 1:1).
    pub mcp_disabled_tools: Vec<String>,
    /// Ollama text model for local tagging (AI-PIPELINE §2). `None` = no local LLM.
    #[serde(default)]
    pub tagger_model: Option<String>,
    /// Optional Ollama vision model that captions the preview before the text model tags
    /// (AI-PIPELINE §2). `None` = tag from the asset's name alone.
    #[serde(default)]
    pub tagger_vision_model: Option<String>,
    /// Opt-in OpenAI-compatible tagging (AI-PIPELINE §5). On + key + model = the visual tag pass
    /// uses the API instead of the local Ollama VLM. Off by default — local is free and private.
    #[serde(default)]
    pub api_tagging_enabled: bool,
    /// OpenAI-compatible base URL (default: the provider's endpoint).
    #[serde(default)]
    pub api_base_url: String,
    /// The vision model id at that endpoint. `None` = not configured.
    #[serde(default)]
    pub api_model: Option<String>,
    /// API key for the endpoint — a secret, never logged. Empty = not configured.
    #[serde(default)]
    pub api_key: String,
}

impl Settings {
    pub fn from_config(c: &lodestash_app::config::AppConfig) -> Self {
        use lodestash_app::config::PreviewQuality;
        Self {
            m_kb: c.size_tiers.m_kb,
            l_kb: c.size_tiers.l_kb,
            xl_kb: c.size_tiers.xl_kb,
            cap_kb: c.size_tiers.cap_kb,
            preview_quality: match c.preview_quality {
                PreviewQuality::Low => "low",
                PreviewQuality::Medium => "medium",
                PreviewQuality::High => "high",
            }
            .into(),
            skip_onboarding: c.skip_onboarding,
            watch_sources: c.watch_sources,
            auto_identify: c.auto_identify,
            cache_extracted: c.cache_extracted,
            preview_cache_cap_gb: c.preview_cache_cap_gb,
            auto_purge_preview_cache: c.auto_purge_preview_cache,
            confirm_before_trash: c.confirm_before_trash,
            mcp_disabled_tools: c.mcp_disabled_tools.clone(),
            tagger_model: c.tagger_model.clone(),
            tagger_vision_model: c.tagger_vision_model.clone(),
            api_tagging_enabled: c.api_tagging_enabled,
            api_base_url: c.api_base_url.clone(),
            api_model: c.api_model.clone(),
            api_key: c.api_key.clone(),
        }
    }

    pub fn apply(self, c: &mut lodestash_app::config::AppConfig) {
        use lodestash_app::config::PreviewQuality;
        c.size_tiers = lodestash_app::config::SizeTiers {
            m_kb: self.m_kb,
            l_kb: self.l_kb,
            xl_kb: self.xl_kb,
            cap_kb: self.cap_kb,
        }
        .normalized();
        c.preview_quality = match self.preview_quality.as_str() {
            "medium" => PreviewQuality::Medium,
            "high" => PreviewQuality::High,
            _ => PreviewQuality::Low,
        };
        c.skip_onboarding = self.skip_onboarding;
        c.watch_sources = self.watch_sources;
        c.auto_identify = self.auto_identify;
        c.cache_extracted = self.cache_extracted;
        c.preview_cache_cap_gb = self.preview_cache_cap_gb;
        c.auto_purge_preview_cache = self.auto_purge_preview_cache;
        c.confirm_before_trash = self.confirm_before_trash;
        c.mcp_disabled_tools = self.mcp_disabled_tools;
        c.tagger_model = self.tagger_model;
        c.tagger_vision_model = self.tagger_vision_model;
        c.api_tagging_enabled = self.api_tagging_enabled;
        c.api_base_url = self.api_base_url;
        c.api_model = self.api_model;
        c.api_key = self.api_key;
    }
}

/// One folder in the pack browser's left tree (PACK-DIRECTORY-NAV §5).
#[derive(Debug, Clone, Serialize)]
pub struct PackDirNode {
    /// Pack-relative path, `/`-separated — the value the browse `dir` is set to on click.
    pub path: String,
    /// Last path segment — what the row reads.
    pub name: String,
    /// 1-based depth; the top folders of the pack are depth 1.
    pub depth: usize,
    /// Assets whose parent folder is this one.
    pub direct: usize,
    /// Everything at or under this folder.
    pub recursive: usize,
    /// True when some other folder nests under this one (shows the expand chevron).
    pub has_children: bool,
}

#[derive(Debug, Serialize)]
pub struct FolderPage {
    pub tiles: Vec<FolderTileOut>,
    /// The assets living **directly** in this folder. Not paged: a single directory holding more
    /// than a screenful is the exception, and the tiles above it are the navigation.
    pub items: Vec<GridItem>,
    /// The list view's third column per item (PACK-DIRECTORY-NAV §6): duration for audio,
    /// dimensions for images, `—` otherwise — aligned 1:1 with `items`.
    pub third: Vec<String>,
    /// Everything under this level, tiles included — the "N assets" the breadcrumb reports.
    pub total: usize,
}

/// One row of the trait vocabulary (UI-SPEC §9).
#[derive(Debug, Serialize)]
pub struct VocabRow {
    /// The `tag` row's id — what the authoring commands address (upsert_trait & co).
    pub id: i64,
    pub namespace: String,
    pub value: String,
    /// False for a plain tag — the row the "promote" strip shows (B8a: `is_trait = false` rows
    /// are already in `list_vocabulary`'s result).
    pub is_trait: bool,
    pub scope: Option<String>,
    pub usage_note: Option<String>,
    /// Assets carrying it, and the manual/AI split — which is the honest way to show how much of
    /// the vocabulary a human wrote versus how much a model guessed.
    pub usage_count: i64,
    pub manual_count: i64,
    pub ai_count: i64,
    /// Taught to the identifier (DATA-MODEL §4), rather than merely observed.
    pub teach: bool,
    /// The trait's chip style (set_trait_style): a CSS token or hex colour, and an icon id.
    pub color: Option<String>,
    pub icon: Option<String>,
}

/// One pack on the Packages screen (UI-SPEC §4), publisher → pack.
#[derive(Debug, Serialize)]
pub struct PackageRow {
    pub id: i64,
    pub name: String,
    pub publisher: String,
    pub category: Option<String>,
    /// Indexed assets **actually in the catalog** for this pack, not the manifest's claim.
    ///
    /// `package.asset_count` is populated for 1 of 87 packs here, so trusting it would show a
    /// library of blanks. Counting the rows we hold is the number the Browse → link will land on,
    /// which is the only count worth printing next to it.
    pub count: usize,
    pub unidentified: usize,
    pub bytes: i64,
    /// Sum of the package's model triangle counts — the stats sheet's triangles tile.
    pub triangles: i64,
    /// Per-media-type breakdown, same shape as the Stats screen's `TypeStat`.
    pub by_type: Vec<TypeStat>,
    pub pinned: bool,
    /// The pack has been unpacked to the workspace at some point (DATA-MODEL §7).
    pub extracted: bool,
}

/// The Stats overview (UI-SPEC §8.1).
///
/// Every number here is derived from the rows already in memory — there is no stats query. The
/// library is ~10k small structs the shell holds anyway, and a second set of `COUNT(*)`s over the
/// same data is how a summary screen comes to disagree with the grid it is summarising.
#[derive(Debug, Serialize)]
pub struct Stats {
    pub total_assets: usize,
    pub sources: usize,
    /// Distinct packs the live assets came out of.
    pub packs: usize,
    pub bytes_total: i64,
    pub triangles_total: i64,
    pub models: usize,
    pub unidentified: usize,
    pub by_type: Vec<TypeStat>,
    /// Heaviest packs first — the "disk weight by pack" bars.
    pub by_pack: Vec<PackStat>,
    /// Top 5 by size.
    pub heaviest: Vec<GridItem>,
    /// Exact-duplicate groups, for the insight row that links to the Duplicates screen.
    pub duplicate_groups: usize,
}

#[derive(Debug, Serialize)]
pub struct TypeStat {
    /// `media_type.code`, so the frontend reuses its existing colour and icon maps.
    pub media_type: String,
    pub count: usize,
    pub bytes: i64,
    pub triangles: i64,
    pub duration_ms: i64,
}

#[derive(Debug, Serialize)]
pub struct PackStat {
    pub name: String,
    pub count: usize,
    pub bytes: i64,
}

/// One exact-duplicate group (UI-SPEC §5). **Never auto-resolved** — this is a review queue.
#[derive(Debug, Serialize)]
pub struct DupGroup {
    /// BLAKE3 of the shared bytes. Shown truncated; it is also the group's identity.
    pub hash: String,
    pub size_bytes: i64,
    /// Members, keeper first.
    pub items: Vec<GridItem>,
    /// Which member is the keeper: highest status, ties broken by lowest asset id (earliest
    /// indexed). Always index 0 after sorting — carried explicitly so the frontend does not have
    /// to re-derive a rule that lives here.
    pub keeper: i64,
    /// `true` when the members come from different packs, which is the interesting case: the same
    /// rock shipped in three Synty packs is disk you can reclaim; two copies inside one pack is
    /// how that pack was built.
    pub cross_pack: bool,
}

/// What the ⚑ Filters popover needs to draw itself (UI-SPEC §2.3).
///
/// Sent from Rust rather than hardcoded in TypeScript because every id and label here is a wire
/// contract with `ContentFilter`: a bucket the frontend spells differently is a filter that
/// silently matches nothing, and the size ranges are user-editable (§11.4), so a TS copy would be
/// wrong for anyone who has touched them.
#[derive(Debug, Clone, Serialize)]
pub struct Facets {
    /// Traits present in the live library, most-used first, with the count that carries each.
    pub traits: Vec<TraitCount>,
    pub tri_buckets: Vec<FacetOption>,
    /// Size tiers, labelled with their resolved KB range so `L` reads as a size, not a letter.
    pub sizes: Vec<FacetOption>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TraitCount {
    pub name: String,
    pub count: usize,
}

/// One selectable chip: the id goes back in the `ContentFilter`, the label is what it reads as.
#[derive(Debug, Clone, Serialize)]
pub struct FacetOption {
    pub id: String,
    pub label: String,
}

/// Everything the shell holds about the open catalog.
pub struct Library {
    catalog: Option<Catalog>,
    /// Where generated thumbnails live — the same cache the egui build writes, so a thumbnail
    /// rendered by either frontend serves the other.
    thumb_cache: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
    sources: Vec<SourceSummary>,
    /// All live rows, name-sorted once at open. The whole library is ~10k rows of small structs,
    /// so holding it beats a query per page — and it is what the egui build already does.
    /// ponytail: re-reads only on `reload`; a catalog written by the egui app while this is open
    /// will not show until then. Fine while this is a read-only viewer.
    rows: Vec<Row>,
    /// asset_id -> absolute path on disk, for `lodestash://file`.
    paths: HashMap<i64, PathBuf>,
    /// The user's S/M/L/XL boundaries (§11.4), read from the same config the egui build writes —
    /// so the size badge on a card and the size facet in the popover agree with each other.
    tiers: SizeTiers,
    /// Assets staged in the tray (I6). In-memory only — the tray is transient design state;
    /// the commit-to-project half rides the engine build (B9e keeps the collect side).
    staged: Vec<i64>,
    /// The local text-capable embedder for AI-mode search (TopBar ⟨Tags⟩/⟨AI⟩), resolved once
    /// — model load is seconds, so per keystroke would be unusable. `None` until first use.
    ai_embedder: std::sync::Mutex<
        Option<std::sync::Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    >,
}

struct Row {
    item: GridItem,
    /// Lowercased name, precomputed — the search filter runs over every row per keystroke.
    search_key: String,
    /// Sort keys and inspector facets kept off `GridItem`, which is serialised per card.
    mtime: i64,
    width_px: Option<i64>,
    height_px: Option<i64>,
    duration_ms: Option<i64>,
    source: Option<String>,
    /// Set only for in-pack rows, so `lodestash://thumb` can pull the one payload it needs.
    locator: Option<InPackLocator>,
    /// Soft-deleted (DATA-MODEL §8). Held rather than dropped so the Trash screen can list it and
    /// `set_trashed(false)` can bring it back; every other view filters it out.
    trashed: bool,
    /// The catalog strings parsed once at hydrate, so filtering and sorting run over the shared
    /// enums instead of re-matching a string per row per keystroke. `media_type` is `None` for a
    /// code this build does not know (schema drift), which sorts last and matches no tab but All.
    media_type: Option<MediaType>,
    status: AssetStatus,
    /// Tag names, filled in one bulk pass (`mark_traits`) — the trait facet needs them per row.
    traits: Vec<String>,
    /// BLAKE3 of the primary file — the exact-duplicate key. `None` until the hash drain has
    /// reached this asset, which is most of a freshly scanned library.
    content_hash: Option<String>,
    /// dHash of the primary file (DATA-MODEL §5) — the near-duplicate key: same picture,
    /// different bytes (re-encoded / re-sized). `None` until the metrics drain decoded it.
    perceptual_hash: Option<[u8; 8]>,
    /// The path the Folders view browses by: `rel_path` for a loose file, but
    /// `<pack>/<rel_path>` for an in-pack asset.
    ///
    /// The synthetic first segment is load-bearing. Every `.unitypackage` lays its assets out
    /// under `Assets/…`, and one source holds dozens of packs — browsing by `rel_path` alone
    /// merges all of them into a single `Assets` folder containing thousands of files from
    /// unrelated products, which is exactly the "anonymous pile" the browse-views design exists to
    /// prevent. In-pack assets are 72% of a real library, so excluding them instead is not an option.
    browse_path: PathBuf,
}

impl Library {
    /// Open the catalog the egui app writes, read-only. Missing catalog is not an error: the
    /// shell shows an empty library and says so.
    pub fn open() -> Self {
        let Some(path) = lodestash_app::paths::catalog_db_path() else {
            return Self::empty();
        };
        if !path.exists() {
            return Self::empty();
        }
        match Catalog::open(&path) {
            Ok(cat) => {
                let mut lib = Self {
                    catalog: Some(cat),
                    thumb_cache: lodestash_app::paths::default_cache_dirs().0,
                    catalog_path: Some(path),
                    sources: Vec::new(),
                    rows: Vec::new(),
                    paths: HashMap::new(),
                    tiers: lodestash_app::config::AppConfig::load()
                        .size_tiers
                        .normalized(),
                    staged: Vec::new(),
                    ai_embedder: Default::default(),
                };
                lib.reload();
                lib
            }
            Err(e) => {
                tracing::warn!(%e, "catalog open failed; shell starts empty");
                Self::empty()
            }
        }
    }

    fn empty() -> Self {
        Self {
            catalog: None,
            thumb_cache: lodestash_app::paths::default_cache_dirs().0,
            catalog_path: None,
            sources: Vec::new(),
            rows: Vec::new(),
            paths: HashMap::new(),
            tiers: SizeTiers::default(),
            staged: Vec::new(),
            ai_embedder: Default::default(),
        }
    }

    /// Re-read sources and their assets from the catalog.
    pub fn reload(&mut self) {
        // Size tiers feed the S/M/L/XL badges and the ⚑ size facet; they come from config, which
        // Settings edits — re-read here so a change lands without a restart (the grid never holds
        // a second copy to go stale).
        self.tiers = lodestash_app::config::AppConfig::load()
            .size_tiers
            .normalized();
        let Some(cat) = self.catalog.as_ref() else {
            return;
        };
        let infos = match cat.source_infos() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "source_infos failed");
                return;
            }
        };

        let (_, workspace_dir) = lodestash_app::paths::default_cache_dirs();
        let mut rows = Vec::new();
        let mut paths = HashMap::new();
        let mut sources = Vec::new();
        for info in &infos {
            let root = PathBuf::from(&info.root_path);
            let entries = match cat.entries_for_source(info.id) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(%e, source = %info.name, "entries_for_source failed");
                    continue;
                }
            };
            for e in entries {
                // Trashed rows are **kept**, flagged: the Trash screen is the only way back from
                // a soft delete, and a row the shell never loaded cannot be restored.
                paths.insert(e.asset_id, root.join(&e.rel_path));
                rows.push(row_from(e, info.id, &info.name, None));
            }

            // In-pack rows are the bulk of a real library — on this machine 5,963 of 8,237 live
            // assets. Loading only `entries_for_source` showed ~21% of the catalog with nothing
            // saying so, which reads as a small library rather than a missing query.
            match cat.in_pack_entries_for_source(info.id) {
                Ok(v) => {
                    for e in v {
                        // No local path: `lodestash://file` resolves nothing and answers 503 not_local,
                        // which is the correct read-only behaviour — extraction is a command.
                        let ext = Path::new(&e.asset.rel_path)
                            .extension()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let mut row = row_from(e.asset, info.id, &info.name, Some(e.package_name));
                        // Carried per row so a thumbnail pull costs no catalog round-trip.
                        row.locator = Some(InPackLocator {
                            pack_path: e.pack_path,
                            offset: e.tar_offset,
                            size: e.size,
                            ext,
                        });
                        rows.push(row);
                    }
                }
                Err(e) => {
                    tracing::warn!(%e, source = %info.name, "in_pack_entries_for_source failed")
                }
            }

            // Extracted-package rows (workspace_rel). Empty on this catalog, but a library that
            // has unpacked anything would otherwise lose those assets the same way.
            match cat.extracted_entries_for_source(info.id) {
                Ok(v) => {
                    for e in v {
                        if let Some(ws) = workspace_dir.as_ref() {
                            paths.insert(e.asset.asset_id, ws.join(&e.workspace_rel));
                        }
                        rows.push(row_from(e.asset, info.id, &info.name, Some(e.package_name)));
                    }
                }
                Err(e) => {
                    tracing::warn!(%e, source = %info.name, "extracted_entries_for_source failed")
                }
            }
            sources.push(SourceSummary {
                id: info.id,
                name: info.name.clone(),
                root_path: info.root_path.clone(),
                asset_count: info.asset_count,
                watch_enabled: info.watch_enabled,
                state: info.state.clone(),
                last_seen_at: info.last_seen_at,
            });
        }

        rows.sort_by(|a, b| a.search_key.cmp(&b.search_key));
        // Near-duplicate review (DATA-MODEL §5) reads the perceptual hashes once per load.
        if let Ok(phashes) = cat.perceptual_hashes() {
            for row in &mut rows {
                row.perceptual_hash = phashes.get(&row.item.asset_id).copied();
            }
        }
        self.rows = rows;
        self.paths = paths;
        self.sources = sources;
        self.mark_previews();
        self.mark_favorites();
        self.mark_traits();
        self.mark_tiers();
        self.mark_imported();
    }

    /// The recent export runs (FEATURES §7.1). A thin pass-through — the grouping is the catalog's.
    pub fn recent_exports(&self, limit: usize) -> Result<Vec<ExportBatchRow>, String> {
        let cat = self.catalog.as_ref().ok_or("no catalog open")?;
        cat.recent_export_batches(limit)
            .map(|v| {
                v.into_iter()
                    .map(|b| ExportBatchRow {
                        batch_id: b.batch_id,
                        dest_path: b.dest_path,
                        files: b.files,
                        exported_at: b.exported_at,
                    })
                    .collect()
            })
            .map_err(|e| e.to_string())
    }

    /// What relinking this source to that folder would reach (UI-SPEC §11.1).
    ///
    /// A thin pass-through to the catalog: the count is the whole product here, and it is shown
    /// before the relink commits so a move that would strand half the library is visible first.
    pub fn relink_preview(&self, source_id: i64, new_root: &Path) -> Result<RelinkCount, String> {
        let cat = self.catalog.as_ref().ok_or("no catalog open")?;
        cat.relink_preview(source_id, new_root)
            .map(|p| RelinkCount {
                matched: p.matched,
                total: p.total,
            })
            .map_err(|e| e.to_string())
    }

    /// Turn the query the grid just ran into the appraisal queue's order (UI-SPEC §3).
    ///
    /// Three tiers, from the same filtered-and-sorted list the grid drew: the page on screen, then
    /// the rest of that filter **starting after the page** (scrolling forward is what people do),
    /// then whatever was above it. Everything outside the filter is the catalog's own newest-first
    /// fallback and needs no list.
    ///
    /// The whole thing is one `page()` over rows already in memory — no catalog round-trip — which
    /// is what makes it cheap enough to call on every browse.
    ///
    /// ponytail: `FILTER_CAP` bounds the second tier at 500 ids. The list is inlined into eight
    /// `ORDER BY` clauses; unbounded, a filter matching 8,000 assets would make the hint the most
    /// expensive part of every queue query. Raise it if a real backlog outruns the lookahead.
    pub fn focus_order(&self, q: &GridQuery) -> AppraiseFocus {
        const FILTER_CAP: usize = 500;
        let mut all = q.clone();
        all.page = 0;
        all.page_size = usize::MAX;
        let ordered: Vec<i64> = self
            .page(&all)
            .items
            .into_iter()
            .map(|i| i.asset_id)
            .collect();

        // `page()` clamps a stale page index to the last page rather than showing a blank grid;
        // clamp identically here or the queue prioritises a page nobody is looking at while the
        // grid shows another. The two must agree — they are answering the same question.
        let size = q.page_size.max(1);
        let pages = ordered.len().div_ceil(size);
        let page = q.page.min(pages.saturating_sub(1));
        let start = page.saturating_mul(size).min(ordered.len());
        let end = start.saturating_add(size).min(ordered.len());
        let visible = ordered[start..end].to_vec();
        let filtered = ordered[end..]
            .iter()
            .chain(ordered[..start].iter())
            .copied()
            .take(FILTER_CAP)
            .collect();
        AppraiseFocus { visible, filtered }
    }

    /// Stamp `in_project` onto every row from the active project's import set.
    ///
    /// One query for the whole library, like [`Self::mark_traits`]. Switching projects re-runs the
    /// reload, so the badges re-paint — which is correct and visible, and the reason this is not
    /// cached anywhere else.
    fn mark_imported(&mut self) {
        let Some(cat) = self.catalog.as_ref() else {
            return;
        };
        let states = match cat.import_states() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "import_states failed");
                return;
            }
        };
        for r in &mut self.rows {
            // Absent from the map is "never imported" — the common case, and it draws nothing.
            let Some(s) = states.get(&r.item.asset_id) else {
                continue;
            };
            r.item.in_project = s.in_active;
            r.item.import_stale = s.stale;
            r.item.in_other_projects = s.others;
        }
    }

    /// Stamp the S/M/L/XL badge onto every card.
    ///
    /// Done once here rather than in `grid_page`, because `folder_page`, `stats().heaviest` and the
    /// Inspector all clone the same `GridItem` — computing it per page would leave whichever screen
    /// someone forgot to touch showing a blank badge.
    fn mark_tiers(&mut self) {
        let tiers = self.tiers;
        for r in &mut self.rows {
            let bytes = r.item.size_bytes.max(0) as u64;
            r.item.size_tier = tiers.tier_of(bytes).letter().to_string();
        }
    }

    /// Attach tag names to rows, in one query rather than one per row.
    fn mark_traits(&mut self) {
        let Some(cat) = self.catalog.as_ref() else {
            return;
        };
        let mut by_asset = match cat.all_asset_tags() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "all_asset_tags failed");
                return;
            }
        };
        for r in &mut self.rows {
            if let Some(t) = by_asset.remove(&r.item.asset_id) {
                // The facet matches on the tag's **value**; its namespace is a grouping for the
                // Traits screen, not part of the chip's identity.
                r.traits = t.into_iter().map(|a| a.value).collect();
                // The card draws the same list, so it rides along on the serialised item rather
                // than costing a second round-trip per card.
                r.item.traits = r.traits.clone();
            }
        }
    }

    /// The chips the ⚑ popover draws, derived from what the library actually holds.
    ///
    /// Traits are counted over **live** rows and sorted by count: a cloud in catalog order puts
    /// whatever happens to sort first in front of the trait 900 assets share. Buckets and tiers
    /// are the full fixed sets, including empty ones — a facet that appears and disappears as you
    /// filter is a facet you cannot learn.
    pub fn facets(&self) -> Facets {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for r in self.live() {
            for t in &r.traits {
                *counts.entry(t.as_str()).or_default() += 1;
            }
        }
        let mut traits: Vec<TraitCount> = counts
            .into_iter()
            .map(|(name, count)| TraitCount {
                name: name.to_string(),
                count,
            })
            .collect();
        traits.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));

        Facets {
            traits,
            tri_buckets: TriBucket::ALL
                .iter()
                .map(|b| FacetOption {
                    id: serde_id(b),
                    label: b.label().to_string(),
                })
                .collect(),
            sizes: SizeTier::ALL
                .iter()
                .map(|t| {
                    let (lo, hi) = self.tiers.range_kb(*t);
                    FacetOption {
                        id: serde_id(t),
                        label: format!("{} · {}", t.letter(), kb_range(lo, hi)),
                    }
                })
                .collect(),
        }
    }

    /// Flag starred rows, in one query rather than one per row.
    fn mark_favorites(&mut self) {
        let Some(cat) = self.catalog.as_ref() else {
            return;
        };
        let ids = match cat.favorite_ids() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "favorite_ids failed");
                return;
            }
        };
        let set: std::collections::HashSet<i64> = ids.into_iter().collect();
        for r in &mut self.rows {
            r.item.favorite = set.contains(&r.item.asset_id);
        }
    }

    /// Flag which rows have a stored vector preview, in one query instead of one per row.
    fn mark_previews(&mut self) {
        let Some(cat) = self.catalog.as_ref() else {
            return;
        };
        let ids = match cat.assets_with_preview() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "assets_with_preview failed");
                return;
            }
        };
        let set: std::collections::HashSet<i64> = ids.into_iter().collect();
        for r in &mut self.rows {
            r.item.has_preview = set.contains(&r.item.asset_id);
        }
    }

    /// Rows that are actually in the library — everything except the Trash.
    fn live(&self) -> impl Iterator<Item = &Row> {
        self.rows.iter().filter(|r| !r.trashed)
    }

    pub fn status(&self) -> LibraryStatus {
        LibraryStatus {
            open: self.catalog.is_some(),
            catalog_path: self
                .catalog_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            sources: self.sources.clone(),
            // Counts are of the *live* library: a trashed asset is gone as far as every screen
            // except the Trash is concerned, so counting it would make the totals disagree with
            // the grid the user is looking at.
            total_assets: self.live().count(),
            in_pack: self.live().filter(|r| r.item.in_pack).count(),
            favorites: self.live().filter(|r| r.item.favorite).count(),
            with_preview: self.live().filter(|r| r.item.has_preview).count(),
            trashed: self.rows.iter().filter(|r| r.trashed).count(),
            unidentified: self.count_status(StatusFilter::Unidentified),
            identified: self.count_status(StatusFilter::Identified),
            confirmed: self.count_status(StatusFilter::Confirmed),
            duplicate_groups: self.duplicates().len(),
            tab_counts: Category::TABS
                .iter()
                .map(|(cat, _)| TabCount {
                    id: cat.id().to_string(),
                    count: self.live().filter(|r| cat.matches(r.media_type)).count(),
                })
                .collect(),
        }
    }

    /// Live rows passing one status facet. Shares the fold with the grid on purpose — see
    /// [`LibraryStatus::unidentified`].
    fn count_status(&self, f: StatusFilter) -> usize {
        self.live().filter(|r| f.matches(r.status)).count()
    }

    /// The facets both browse views share: tab, status and the ⚑ filter. Trash and favourites are
    /// **not** here — they scope the grid, not the Folders tree.
    ///
    /// Links are passed to `passes` as `(false, false)`: the dependency-graph chips are not on the
    /// popover (UI-SPEC §2.3 lists traits, triangles and size), so `filter.link` is always `None`
    /// here and the flags are never read. Wiring them means two more bulk queries at hydrate, and
    /// belongs with the chips that use them.
    fn keeps(&self, r: &Row, tab: Category, status: StatusFilter, filter: &ContentFilter) -> bool {
        tab.matches(r.media_type)
            && status.matches(r.status)
            && filter.passes(
                r.item.size_bytes.max(0) as u64,
                r.item.triangles.map(|t| t.max(0) as u32),
                None,
                &r.traits,
                (false, false),
                r.item.favorite,
                &self.tiers,
            )
    }

    /// Does this row match the active query?
    ///
    /// Union, exactly as the egui grid does it (`app.rs` `rebuild_cache`): FTS5 reaches tags and
    /// descriptions and ranks by BM25, but it matches whole tokens by prefix, so a mid-word query
    /// ("olygon") would stop matching things that matched yesterday. Keeping the substring pass
    /// means the search only ever gains results.
    fn hits(&self, r: &Row, needle: &str, ranks: Option<&HashMap<i64, usize>>) -> bool {
        ranks.is_some_and(|m| m.contains_key(&r.item.asset_id)) || r.search_key.contains(needle)
    }

    /// Filter, sort, page. Runs the search for the active mode, then hands the ranks to
    /// [`Self::page_with_ranks`], which is where the actual work — and the testability — lives.
    pub fn page(&self, q: &GridQuery) -> GridPage {
        let needle = q.search.trim().to_lowercase();
        // ⟨Tags⟩/⟨AI⟩ (TopBar): AI mode RRF-fuses FTS with query embeddings (hybrid_search);
        // without a local model it honestly degrades to the FTS ranks.
        let ranks = if q.search_mode == "ai" {
            self.ai_ranks(&needle)
                .or_else(|| self.search_ranks(&needle))
        } else {
            self.search_ranks(&needle)
        };
        self.page_with_ranks(q, ranks.as_ref())
    }

    /// Filter, sort, page against a **given** set of BM25 ranks. Pure over the held rows, so the
    /// union rule and the relevance order can be tested without standing up an FTS index.
    fn page_with_ranks(&self, q: &GridQuery, ranks: Option<&HashMap<i64, usize>>) -> GridPage {
        let needle = q.search.trim().to_lowercase();
        // Collection filter (UI-SPEC §6 Browse →): the grid matches the collection's members.
        // A missing catalog or unknown id filters to nothing rather than erroring — a deleted
        // collection is an empty grid until the chip is cleared.
        let members: Option<std::collections::HashSet<i64>> = match q.collection {
            Some(id) => self
                .catalog
                .as_ref()
                .and_then(|cat| cat.collection_members(id).ok())
                .map(|v| v.into_iter().collect()),
            None => None,
        };
        // LINKS filter (FEATURES §1.4 / DATA-MODEL §10): show only assets sharing an
        // `asset_relationship` edge with the selected id, computed once per query rather than
        // one edge query per row. An empty neighbour set — the selected asset has no edges, or
        // the catalog is missing — means **zero rows**: filtering by a link that doesn't exist
        // must show nothing, not everything.
        let linked: Option<std::collections::HashSet<i64>> = match q.links {
            Some(id) => Some(
                self.catalog
                    .as_ref()
                    .and_then(|cat| cat.asset_edges().ok())
                    .map(|edges| {
                        edges
                            .iter()
                            .filter(|e| e.from == id || e.to == id)
                            .flat_map(|e| [e.from, e.to])
                            .filter(|&x| x != id)
                            .collect::<std::collections::HashSet<_>>()
                    })
                    .unwrap_or_default(),
            ),
            None => None,
        };
        let mut matched: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| self.keeps(r, q.tab, q.status, &q.filter))
            .filter(|r| r.trashed == q.trashed_only)
            .filter(|r| !q.favorites_only || r.item.favorite)
            .filter(|r| match &q.package {
                Some(p) => r.item.package.as_deref() == Some(p.as_str()),
                None => true,
            })
            .filter(|r| match &q.source {
                Some(s) => r.source.as_deref() == Some(s.as_str()),
                None => true,
            })
            .filter(|r| match &members {
                Some(m) => m.contains(&r.item.asset_id),
                None => true,
            })
            .filter(|r| {
                if q.ids.is_empty() {
                    true
                } else {
                    q.ids.contains(&r.item.asset_id)
                }
            })
            .filter(|r| match &linked {
                Some(set) => set.contains(&r.item.asset_id),
                None => true,
            })
            .filter(|r| needle.is_empty() || self.hits(r, &needle, ranks))
            .collect();

        // Every key falls back to name for ties, which keeps paging stable when a key has many
        // equal values (thousands of assets share an mtime, or a size). Name is sorted
        // explicitly rather than leaning on load order: relying on `reload` having sorted
        // `rows` makes the result depend on how the Library was built, not on the query.
        //
        // Direction goes through `grid::dir` on the comparator rather than reversing the sorted
        // vector, which is the rule the egui grid applies. That swap is only safe because the
        // comparator below is a **total** order: reversing a stable sort keeps rows that compare
        // fully equal in load order *in both directions*, so without a unique final key a
        // descending page is not the reverse of the ascending one, and page boundaries can repeat
        // or drop the duplicates. `asset_id` is that key — this library really does hold assets
        // with the same name and the same size.
        fn key(r: &Row) -> (&str, i64) {
            (r.search_key.as_str(), r.item.asset_id)
        }
        let by = |o: std::cmp::Ordering, a: &Row, b: &Row| {
            grid::dir(o.then_with(|| key(a).cmp(&key(b))), q.sort_asc)
        };
        match q.sort_by {
            // Rating is not part of this product's surface (see the note in DATA-MODEL §12): a
            // personal library nobody else browses gets no value from ranking your own files, and
            // the toolbar no longer offers it. The variant still exists for the egui build, so it
            // is accepted and treated as Name rather than failing a query nobody can now send.
            SortBy::Name | SortBy::Rating => {
                matched.sort_by(|a, b| by(std::cmp::Ordering::Equal, a, b))
            }
            // BM25 order, hits before non-hits. With no query (or no catalog) every row is
            // unranked and this degrades to name order, which is why the toolbar can leave
            // Relevance selected while the user clears the search box.
            SortBy::Relevance => matched.sort_by(|a, b| {
                let k = |r: &Row| {
                    grid::relevance_key(
                        ranks
                            .as_ref()
                            .and_then(|m| m.get(&r.item.asset_id).copied()),
                    )
                };
                by(k(a).cmp(&k(b)), a, b)
            }),
            SortBy::Size => {
                matched.sort_by(|a, b| by(a.item.size_bytes.cmp(&b.item.size_bytes), a, b))
            }
            SortBy::Modified => matched.sort_by(|a, b| by(a.mtime.cmp(&b.mtime), a, b)),
            // Enum order, not alphabetical on the code string — `grid::media_key` owns that rule.
            SortBy::Type => matched.sort_by(|a, b| {
                by(
                    grid::media_key(a.media_type).cmp(&grid::media_key(b.media_type)),
                    a,
                    b,
                )
            }),
        }

        let page_size = q.page_size.max(1);
        let total = matched.len();
        let pages = total.div_ceil(page_size);
        // A stale page index (filter narrowed under the pager) clamps instead of showing blank.
        let page = q.page.min(pages.saturating_sub(1));
        let start = page * page_size;
        let items = matched
            .into_iter()
            .skip(start)
            .take(page_size)
            .map(|r| r.item.clone())
            .collect();

        GridPage {
            items,
            total,
            page,
            pages,
        }
    }

    /// The trait vocabulary, most-used first (UI-SPEC §9).
    ///
    /// One row per **tag id**, which is not one row per name: this catalog holds `free` twice,
    /// once in the `free` namespace and once in `mood`. The ⚑ trait cloud and
    /// `ContentFilter::passes` both key on the *name*, so those two rows are one chip there and
    /// two rows here. That split is right — the filter is what a user types, the vocabulary is
    /// what the taxonomy holds — but it means the counts only reconcile once summed by name.
    ///
    /// Read-only. Authoring — create, rename, merge, teach — is a write against `tag`, and the
    /// writer thread has no ops for it; adding them is its own slice with its own undo story.
    pub fn vocabulary(&self) -> Vec<VocabRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        let mut rows: Vec<VocabRow> = match cat.list_vocabulary() {
            Ok(v) => v
                .into_iter()
                .map(|t| VocabRow {
                    id: t.id,
                    namespace: t.namespace,
                    value: t.value,
                    is_trait: t.is_trait,
                    scope: t.scope,
                    usage_note: t.usage_note,
                    usage_count: t.usage_count,
                    manual_count: t.manual_count,
                    ai_count: t.ai_count,
                    teach: t.teach,
                    color: t.color,
                    icon: t.icon,
                })
                .collect(),
            Err(e) => {
                tracing::warn!(%e, "list_vocabulary failed");
                return Vec::new();
            }
        };
        rows.sort_by(|a, b| {
            b.usage_count
                .cmp(&a.usage_count)
                .then_with(|| a.namespace.cmp(&b.namespace))
                .then_with(|| a.value.cmp(&b.value))
        });
        rows
    }

    /// Every pack, publisher-then-name ordered, with the counts derived from the rows we hold
    /// (UI-SPEC §4).
    ///
    /// Registered Unity editors, active first (ENGINE-API §5.1, UI-SPEC §16.1). Read-only list
    /// for the Connect wizard's app half; switching the active editor is a write (writer thread).
    pub fn projects(&self) -> Vec<lodestash_core::catalog::ProjectRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.list_projects() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "list_projects failed");
                Vec::new()
            }
        }
    }

    /// Packs with **no indexed assets are kept**, showing `0`. Most of this catalog's 87 packs are
    /// still archives nobody has opened, and hiding them would make the screen claim the library
    /// holds a handful of packs when the Asset Store cache holds eighty-six.
    pub fn packages(&self) -> Vec<PackageRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        let listings = match cat.list_packages() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "list_packages failed");
                return Vec::new();
            }
        };
        // name -> (indexed, unidentified, bytes, triangles), over the live rows.
        let mut counts: HashMap<&str, (usize, usize, i64, i64)> = HashMap::new();
        let mut by_type: HashMap<&str, HashMap<String, (usize, i64, i64)>> = HashMap::new();
        for r in self.live() {
            if let Some(p) = r.item.package.as_deref() {
                let e = counts.entry(p).or_insert((0, 0, 0, 0));
                e.0 += 1;
                if !matches!(r.status, AssetStatus::Identified | AssetStatus::Confirmed) {
                    e.1 += 1;
                }
                e.2 += r.item.size_bytes.max(0);
                e.3 += r.item.triangles.unwrap_or(0);
                let t = by_type
                    .entry(p)
                    .or_default()
                    .entry(r.item.media_type.clone())
                    .or_insert((0, 0, 0));
                t.0 += 1;
                t.1 += r.item.size_bytes.max(0);
                t.2 += r.item.triangles.unwrap_or(0);
            }
        }
        listings
            .into_iter()
            .map(|p| {
                let (count, unidentified, bytes, triangles) =
                    counts.get(p.name.as_str()).copied().unwrap_or((0, 0, 0, 0));
                let mut types: Vec<TypeStat> = by_type
                    .remove(p.name.as_str())
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(media_type, (count, bytes, triangles))| TypeStat {
                        media_type,
                        count,
                        bytes,
                        triangles,
                        duration_ms: 0,
                    })
                    .collect();
                types.sort_by(|a, b| b.count.cmp(&a.count));
                PackageRow {
                    id: p.id,
                    name: p.name,
                    // Every pack in this catalog has one, but the column is nullable and an
                    // unlabelled pack still has to land somewhere the user can find it.
                    publisher: p.publisher.unwrap_or_else(|| "Unknown publisher".into()),
                    category: p.category,
                    count,
                    unidentified,
                    bytes,
                    triangles,
                    by_type: types,
                    pinned: p.pinned,
                    extracted: p.unpacked_at.is_some(),
                }
            })
            .collect()
    }

    /// The Stats overview, derived from the rows in memory (UI-SPEC §8.1).
    pub fn stats(&self) -> Stats {
        use std::collections::BTreeMap;
        let mut by_type: BTreeMap<u8, (String, TypeStat)> = BTreeMap::new();
        let mut by_pack: HashMap<&str, (usize, i64)> = HashMap::new();
        let mut bytes_total = 0i64;
        let mut triangles_total = 0i64;
        let mut models = 0usize;
        let mut unidentified = 0usize;

        for r in self.live() {
            bytes_total += r.item.size_bytes.max(0);
            triangles_total += r.item.triangles.unwrap_or(0).max(0);
            if r.media_type == Some(MediaType::Model3d) {
                models += 1;
            }
            if !matches!(r.status, AssetStatus::Identified | AssetStatus::Confirmed) {
                unidentified += 1;
            }
            if let Some(p) = r.item.package.as_deref() {
                let e = by_pack.entry(p).or_insert((0, 0));
                e.0 += 1;
                e.1 += r.item.size_bytes.max(0);
            }
            // Keyed on the enum's own order so the rows read in the same order as the Type sort.
            let e = by_type
                .entry(grid::media_key(r.media_type))
                .or_insert_with(|| {
                    (
                        r.item.media_type.clone(),
                        TypeStat {
                            media_type: r.item.media_type.clone(),
                            count: 0,
                            bytes: 0,
                            triangles: 0,
                            duration_ms: 0,
                        },
                    )
                });
            e.1.count += 1;
            e.1.bytes += r.item.size_bytes.max(0);
            e.1.triangles += r.item.triangles.unwrap_or(0).max(0);
            e.1.duration_ms += r.duration_ms.unwrap_or(0).max(0);
        }

        let mut packs: Vec<PackStat> = by_pack
            .into_iter()
            .map(|(name, (count, bytes))| PackStat {
                name: name.to_string(),
                count,
                bytes,
            })
            .collect();
        packs.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.name.cmp(&b.name)));

        let mut heaviest: Vec<&Row> = self.live().collect();
        heaviest.sort_by(|a, b| {
            b.item
                .size_bytes
                .cmp(&a.item.size_bytes)
                .then_with(|| a.item.asset_id.cmp(&b.item.asset_id))
        });

        Stats {
            total_assets: self.live().count(),
            sources: self.sources.len(),
            packs: packs.len(),
            bytes_total,
            triangles_total,
            models,
            unidentified,
            by_type: by_type.into_values().map(|(_, s)| s).collect(),
            by_pack: packs.into_iter().take(12).collect(),
            heaviest: heaviest.iter().take(5).map(|r| r.item.clone()).collect(),
            duplicate_groups: self.duplicates().len(),
        }
    }

    /// Exact-duplicate groups: live assets sharing a BLAKE3 (DATA-MODEL §5).
    ///
    /// **Exact, not perceptual.** UI-SPEC §5 describes near-duplicate review; the only similarity
    /// signal in the catalog today is the content hash, so this ships the half that is backed by
    /// data and says so on screen. Perceptual grouping arrives with the embeddings, and the
    /// screen's shape does not change when it does — only the reason line.
    ///
    /// ponytail: whole-library group-by on every call, over rows already in memory. It runs when a
    /// screen opens, not per keystroke; cache it behind the reload if a profile ever says to.
    pub fn duplicates(&self) -> Vec<DupGroup> {
        let mut by_hash: HashMap<&str, Vec<&Row>> = HashMap::new();
        for r in self.live() {
            // A missing hash is "not yet hashed", not "hashes to nothing" — grouping those
            // together would present the entire un-drained backlog as one giant duplicate set.
            if let Some(h) = r.content_hash.as_deref() {
                by_hash.entry(h).or_default().push(r);
            }
        }
        let mut groups: Vec<DupGroup> = by_hash
            .into_iter()
            .filter(|(_, rows)| rows.len() > 1)
            .map(|(hash, rows)| dup_group(hash.to_string(), rows))
            .collect();
        sort_groups(&mut groups);
        groups
    }

    /// Near-duplicate groups (DATA-MODEL §5): the same picture in different bytes — a re-encode
    /// or re-size — so BLAKE3 (byte-exact) misses it but the dHash does not. Members whose 64-bit
    /// perceptual hashes differ by ≤ 6 bits (Hamming) join one group, union-find style.
    ///
    /// Bounded to the first `PERCEPTUAL_COMPARE_CAP` hashed rows: the pairwise comparison is n²,
    /// and the review screen already pages — a cap is the honest shape, not a silent omission.
    pub fn near_duplicates(&self) -> Vec<DupGroup> {
        const HAMMING: u32 = 6;
        const CAP: usize = 4_000;
        let rows: Vec<&Row> = self
            .live()
            .filter(|r| r.perceptual_hash.is_some())
            .take(CAP)
            .collect();
        if rows.len() < 2 {
            return Vec::new();
        }
        let mut parent: Vec<usize> = (0..rows.len()).collect();
        for i in 0..rows.len() {
            let a = rows[i].perceptual_hash.unwrap();
            for j in (i + 1)..rows.len() {
                let b = rows[j].perceptual_hash.unwrap();
                let d = a
                    .iter()
                    .zip(b.iter())
                    .map(|(x, y)| (x ^ y).count_ones())
                    .sum::<u32>();
                if d <= HAMMING {
                    let (mut ri, mut rj) = (i, j);
                    while parent[ri] != ri {
                        ri = parent[ri];
                    }
                    while parent[rj] != rj {
                        rj = parent[rj];
                    }
                    parent[ri] = rj;
                }
            }
        }
        let mut by_root: HashMap<usize, Vec<&Row>> = HashMap::new();
        for (i, row) in rows.iter().enumerate() {
            let mut r = i;
            while parent[r] != r {
                r = parent[r];
            }
            by_root.entry(r).or_default().push(row);
        }
        let mut groups: Vec<DupGroup> = by_root
            .into_values()
            .filter(|members| members.len() > 1)
            .map(|members| {
                let h = members[0].perceptual_hash.unwrap();
                let hash = h.iter().map(|b| format!("{b:02x}")).collect::<String>();
                dup_group(hash, members)
            })
            .collect();
        sort_groups(&mut groups);
        groups
    }

    /// One level of the Folders view.
    ///
    /// Shares every non-folder facet with [`Self::page`] through [`Self::keeps`], so a tab or a
    /// filter chip means the same thing in both views — a Folders browser that quietly ignores the
    /// toolbar is a second, subtly different library.
    pub fn folder_page(&self, q: &FolderQuery) -> FolderPage {
        let needle = q.search.trim().to_lowercase();
        let ranks = self.search_ranks(&needle);
        let scoped: Vec<&Row> = self
            .rows
            .iter()
            // The Folders tree is the *live* library — the Trash is its own screen.
            .filter(|r| !r.trashed)
            .filter(|r| self.keeps(r, q.tab, q.status, &q.filter))
            .filter(|r| needle.is_empty() || self.hits(r, &needle, ranks.as_ref()))
            .filter(|r| match &q.source {
                Some(name) => r.source.as_deref() == Some(name.as_str()),
                None => true,
            })
            .collect();

        // The top level: one tile per source, not per folder. A single-source library is descended
        // automatically by the frontend, so nobody meets a screen with one tile on it.
        // A pack-rooted browse (PACK-DIRECTORY-NAV) never shows this level: the pack *is* the root.
        if q.source.is_none() && q.package.is_none() {
            let mut per_source: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for r in &scoped {
                *per_source
                    .entry(r.source.as_deref().unwrap_or("(unknown)"))
                    .or_default() += 1;
            }
            return FolderPage {
                total: scoped.len(),
                tiles: per_source
                    .into_iter()
                    .map(|(name, recursive)| FolderTileOut {
                        name: name.to_string(),
                        // A source is a root: nothing lives directly in it, so `direct` would
                        // always read 0 and the tile would say "0 (1,204)".
                        direct: recursive,
                        recursive,
                    })
                    .collect(),
                items: Vec::new(),
                third: Vec::new(),
            };
        }

        let dir = PathBuf::from(q.dir.replace('\\', "/"));
        // Pack-rooted browsing strips the leading pack segment from every path, so the pack's own
        // name is the tree root rather than a folder inside the source (PACK-DIRECTORY-NAV §4/§5).
        let (idxs, tiles) = if let Some(pkg) = q.package.as_deref() {
            let scoped: Vec<(usize, &Path, Option<MediaType>)> = scoped
                .iter()
                .enumerate()
                .filter(|(_, r)| r.item.package.as_deref() == Some(pkg))
                .filter_map(|(i, r)| {
                    r.browse_path
                        .strip_prefix(pkg)
                        .ok()
                        .map(|p| (i, p, r.media_type))
                })
                .collect();
            grid::folder_level(scoped.iter().map(|(i, p, mt)| (*i, *p, *mt)), &dir, q.tab)
        } else {
            let scoped: Vec<(usize, &Path, Option<MediaType>)> = scoped
                .iter()
                .enumerate()
                .map(|(i, r)| (i, r.browse_path.as_path(), r.media_type))
                .collect();
            grid::folder_level(scoped.iter().map(|(i, p, mt)| (*i, *p, *mt)), &dir, q.tab)
        };
        // Everything at or under this folder, which is what the header counts — the files here
        // plus everything the tiles lead to.
        let total = idxs.len() + tiles.iter().map(|t| t.recursive).sum::<usize>();
        // Sort once, then derive both the items and their third column from the same order so the
        // list view's columns can never disagree with each other (PACK-DIRECTORY-NAV §6).
        let mut idxs: Vec<usize> = idxs.into_iter().collect();
        idxs.sort_by_key(|&i| scoped[i].item.name.to_lowercase());
        let items: Vec<GridItem> = idxs.iter().map(|&i| scoped[i].item.clone()).collect();
        let third: Vec<String> = idxs
            .iter()
            .map(|&i| {
                let r = &scoped[i];
                grid::content_third_col(
                    r.media_type,
                    &lodestash_core::catalog::AssetMetrics {
                        tri_count: r.item.triangles,
                        width_px: r.width_px,
                        height_px: r.height_px,
                        duration_ms: r.duration_ms,
                    },
                )
            })
            .collect();
        FolderPage {
            tiles: tiles
                .into_iter()
                .map(|t| FolderTileOut {
                    name: t.name,
                    direct: t.direct,
                    recursive: t.recursive,
                })
                .collect(),
            items,
            third,
            total,
        }
    }

    /// Every non-staging collection with its member count (UI-SPEC §6).
    pub fn collections(&self) -> Vec<lodestash_core::catalog::CollectionRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.list_collections() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "list_collections failed");
                Vec::new()
            }
        }
    }

    /// The member ids of one collection — what a collection-filtered grid matches on (UI-SPEC §6).
    pub fn collection_members(&self, collection_id: i64) -> Vec<i64> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.collection_members(collection_id) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, collection = collection_id, "collection_members failed");
                Vec::new()
            }
        }
    }

    /// The Stats Tag Graph (UI-SPEC §8.2): tag nodes, tag↔tag co-occurrence edges and the
    /// `tags · links · assets` header counts. Empty graph when the catalog is missing or the
    /// query fails — the canvas draws nothing rather than breaking.
    pub fn tag_graph(&self) -> lodestash_core::catalog::TagGraph {
        let Some(cat) = self.catalog.as_ref() else {
            return empty_tag_graph();
        };
        match cat.tag_graph() {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!(%e, "tag_graph failed");
                empty_tag_graph()
            }
        }
    }

    /// Every resolved asset→asset edge (DATA-MODEL §10) — the set behind the LINKS filter and
    /// the tag graph's dashed dependency arcs. Empty when the catalog is missing.
    pub fn asset_edges(&self) -> Vec<lodestash_core::catalog::AssetEdge> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.asset_edges() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "asset_edges failed");
                Vec::new()
            }
        }
    }

    /// One source's blacklist (UI-SPEC §11.1 expanded panel): every `{kind, value}` exclusion
    /// the crawler will skip for this source (DATA-MODEL §10).
    pub fn source_excludes(&self, source_id: i64) -> Vec<lodestash_core::catalog::SourceExclude> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.list_source_excludes(source_id) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, source = source_id, "list_source_excludes failed");
                Vec::new()
            }
        }
    }

    /// Every taxonomy for the Mapping tab's switcher (UI-SPEC §9.2), active first, with the
    /// `v<version>` readout and `is_active` marker.
    pub fn taxonomies(&self) -> Vec<lodestash_core::catalog::TaxonomyRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.list_taxonomies() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "list_taxonomies failed");
                Vec::new()
            }
        }
    }

    /// The newest undo-log rows, newest first, capped at `limit` (UI-SPEC §2 / L5).
    pub fn recent_ops(&self, limit: usize) -> Vec<lodestash_core::catalog::OpLogRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.recent_ops(limit) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "recent_ops failed");
                Vec::new()
            }
        }
    }

    /// The newest undone ops — the redo stack (UI-SPEC §2 Shift-Z).
    pub fn redo_candidates(&self, limit: usize) -> Vec<lodestash_core::catalog::OpLogRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.redo_candidates(limit) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "redo_candidates failed");
                Vec::new()
            }
        }
    }

    /// The assets related to `asset_id` through dependency edges (DATA-MODEL §10) — the
    /// inspector's LINKED FILES / SIMILAR slices. `outbound` marks which way the edge points.
    pub fn related_assets(&self, asset_id: i64) -> Vec<lodestash_core::catalog::RelatedAsset> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.related_assets(asset_id) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "related_assets failed");
                Vec::new()
            }
        }
    }

    /// One taxonomy's mapping rules, priority-ordered (UI-SPEC §9.2).
    pub fn list_rules(&self, taxonomy_id: i64) -> Vec<lodestash_core::catalog::MappingRuleRow> {
        let Some(cat) = self.catalog.as_ref() else {
            return Vec::new();
        };
        match cat.list_rules(taxonomy_id) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(%e, "list_rules failed");
                Vec::new()
            }
        }
    }

    // ── Staging tray (I6) ──────────────────────────────────────────────────────

    /// Stage `ids` into the tray (dedup, order-preserving). Returns how many were new.
    pub fn stage(&mut self, ids: &[i64]) -> usize {
        let mut n = 0;
        for id in ids {
            if !self.staged.contains(id) {
                self.staged.push(*id);
                n += 1;
            }
        }
        n
    }

    /// Remove `ids` from the tray. Returns how many were present.
    pub fn unstage(&mut self, ids: &[i64]) -> usize {
        let before = self.staged.len();
        self.staged.retain(|id| !ids.contains(id));
        before - self.staged.len()
    }

    pub fn clear_staged(&mut self) {
        self.staged.clear();
    }

    /// The staged asset ids, in the order they were staged.
    pub fn staged_ids(&self) -> Vec<i64> {
        self.staged.clone()
    }

    /// The pack browser's left tree (PACK-DIRECTORY-NAV §5): every folder inside one package with
    /// its `direct (recursive)` counts, derived from the in-memory rows — no query, no new data.
    ///
    /// `path` is pack-relative (`Assets/Characters`), `depth` is 1-based, `direct` counts assets
    /// whose parent is this folder, `recursive` counts everything at or under it. The pack root
    /// itself is not a node: the tree renders the pack's top folders, and the breadcrumb's gold
    /// pill *is* the root.
    pub fn pack_tree(&self, package: &str) -> Vec<PackDirNode> {
        use std::collections::BTreeMap;
        let mut direct: BTreeMap<String, usize> = BTreeMap::new();
        let mut recursive: BTreeMap<String, usize> = BTreeMap::new();
        for r in self
            .rows
            .iter()
            .filter(|r| !r.trashed && r.item.package.as_deref() == Some(package))
        {
            let Ok(rel) = r.browse_path.strip_prefix(package) else {
                continue;
            };
            let rel = rel.to_string_lossy();
            // The file's own name is never a node — only the directories it lives under are.
            let Some(parent) = Path::new(&*rel).parent() else {
                continue;
            };
            if parent.as_os_str().is_empty() {
                continue; // a pack-root file (e.g. readme.txt) is shown by the contents list
            }
            // Every ancestor directory of this file — the dirs the tree can navigate to.
            let mut acc = String::new();
            for comp in parent.components() {
                let std::path::Component::Normal(c) = comp else {
                    continue;
                };
                if !acc.is_empty() {
                    acc.push('/');
                }
                acc.push_str(&c.to_string_lossy());
                *recursive.entry(acc.clone()).or_insert(0) += 1;
            }
            // Direct: this file's own parent directory.
            *direct
                .entry(parent.to_string_lossy().into_owned())
                .or_insert(0) += 1;
        }
        let mut paths: Vec<String> = direct.keys().chain(recursive.keys()).cloned().collect();
        paths.sort();
        paths.dedup();
        paths
            .into_iter()
            .map(|path| {
                let depth = path.matches('/').count() + 1;
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                let has_children = recursive.keys().any(|p| p.starts_with(&format!("{path}/")));
                let direct = *direct.get(&path).unwrap_or(&0);
                let recursive_count = *recursive.get(&path).unwrap_or(&0);
                PackDirNode {
                    path,
                    name,
                    depth,
                    direct,
                    recursive: recursive_count,
                    has_children,
                }
            })
            .collect()
    }

    /// Everything the Inspector needs for one asset.
    pub fn detail(&self, asset_id: i64) -> Option<AssetDetail> {
        let row = self.rows.iter().find(|r| r.item.asset_id == asset_id)?;
        let abs = self.paths.get(&asset_id);
        Some(AssetDetail {
            asset_id,
            name: row.item.name.clone(),
            rel_path: row.item.rel_path.clone(),
            abs_path: abs
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default(),
            media_type: row.item.media_type.clone(),
            size_bytes: row.item.size_bytes,
            status: row.item.status.clone(),
            has_preview: row.item.has_preview,
            triangles: row.item.triangles,
            width_px: row.width_px,
            height_px: row.height_px,
            duration_ms: row.duration_ms,
            source: row.source.clone(),
            source_id: row.item.source_id,
            texture_asset_id: self.albedo_texture(asset_id),
            favorite: row.item.favorite,
            trashed: row.trashed,
            in_pack: row.item.in_pack,
            tags: self
                .catalog
                .as_ref()
                .and_then(|c| c.tags_for_asset(asset_id).ok())
                .unwrap_or_default(),
        })
    }

    /// BM25 ranks for the active query: `asset_id -> position`, best first.
    ///
    /// This is Search by Tags (IDENTIFICATION-AND-SEARCH §5.A) — FTS5 over name, description and
    /// the denormalised tags, which is the whole point: the substring pass it unions with can
    /// only ever see the file name. `None` when there is no catalog, no query, or the query is
    /// punctuation-only; the caller then falls back to substring alone.
    ///
    /// ponytail: re-queried per keystroke rather than cached by query text (the egui build keys
    /// it on `search_ranks_key`). FTS5 over 8k rows is sub-millisecond and `page` takes `&self`;
    /// cache it behind a `Mutex` only if a profile says to.
    fn search_ranks(&self, needle: &str) -> Option<HashMap<i64, usize>> {
        if needle.is_empty() {
            return None;
        }
        // A high cap: the grid re-scopes by tab/status/favourites afterwards, so truncating here
        // would silently drop matches the user *is* filtering to.
        const HIT_CAP: usize = 100_000;
        let ids = self
            .catalog
            .as_ref()?
            .search_assets(needle, HIT_CAP)
            .ok()
            .flatten()?;
        Some(ids.into_iter().enumerate().map(|(r, id)| (id, r)).collect())
    }

    /// AI-mode search (TopBar ⟨Tags⟩/⟨AI⟩): RRF-fuse FTS with query embeddings via
    /// `catalog::hybrid_search` (AI-PIPELINE §3). The embedder resolves once (model load is
    /// seconds — per keystroke would be unusable) and the query embeds in its text space, which
    /// is the same joint space the image/audio embeddings were written into. Returns `None` when
    /// no local model is installed or the embed is a stub, so `page` falls back to FTS honestly.
    fn ai_ranks(&self, needle: &str) -> Option<HashMap<i64, usize>> {
        if needle.is_empty() {
            return None;
        }
        let embedder = {
            let mut slot = self.ai_embedder.lock().ok()?;
            if slot.is_none() {
                *slot = lodestash_app::drains::resolve_embedder();
            }
            slot.clone()?
        };
        let vec = embedder.embed_text(needle);
        if vec.is_empty() {
            return None; // a stub embedder — no usable text space
        }
        let model_id = embedder.model_id().to_string();
        const HIT_CAP: usize = 100_000;
        let ids = self
            .catalog
            .as_ref()?
            .hybrid_search(
                needle,
                &[(model_id.as_str(), vec.as_slice())],
                None,
                HIT_CAP,
            )
            .ok()?;
        Some(ids.into_iter().enumerate().map(|(r, id)| (id, r)).collect())
    }

    pub fn thumb_cache_dir(&self) -> Option<&Path> {
        self.thumb_cache.as_deref()
    }

    /// Bytes + file count under the preview cache root (Settings' CACHE card). The workspace —
    /// where extracted assets live — is a sibling directory and never counted here.
    pub fn preview_cache_usage(&self) -> (u64, usize) {
        let Some(root) = self.thumb_cache_dir() else {
            return (0, 0);
        };
        let mut bytes = 0u64;
        let mut files = 0usize;
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Ok(meta) = entry.metadata() {
                    bytes += meta.len();
                    files += 1;
                }
            }
        }
        (bytes, files)
    }

    /// Empty the preview cache: every loose thumb + the `mesh`/`inpack` scratch dirs. All of it
    /// regenerates on demand from the source assets, so this is non-destructive by design. The
    /// workspace (extracted assets) is never touched. Returns the number of files removed. A
    /// preview being written mid-clear tears — the next generate overwrites it, so it self-heals.
    pub fn clear_preview_cache(&self) -> usize {
        let Some(root) = self.thumb_cache_dir() else {
            return 0;
        };
        let mut removed = 0usize;
        if let Ok(entries) = std::fs::read_dir(root) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                if name == "mesh" || name == "inpack" {
                    if let Ok(len) = std::fs::remove_dir_all(&path) {
                        // count entries best-effort; a missing dir is fine
                        let _ = len;
                        removed += 1;
                    }
                    continue;
                }
                if path.is_file() && std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }

    /// The catalog's last-known (size, mtime) per loose-file rel_path for one source root — the
    /// crawl's incremental diff (`crawler::needs_rehash`, DATA-MODEL §6). Re-adding an
    /// already-indexed folder then skips BLAKE3 over every unchanged file. In-pack rows are
    /// exempt: their files live in the workspace cache, not the source.
    pub fn known_sizes(&self, root: &str) -> HashMap<String, (u64, i64)> {
        self.rows
            .iter()
            .filter(|r| !r.trashed && r.source.as_deref() == Some(root) && !r.item.in_pack)
            .map(|r| {
                (
                    r.browse_path.to_string_lossy().replace('\\', "/"),
                    (r.item.size_bytes as u64, r.mtime),
                )
            })
            .collect()
    }

    pub fn asset_path(&self, asset_id: i64) -> Option<&Path> {
        self.paths.get(&asset_id).map(|p| p.as_path())
    }

    /// Resolve ids into everything the writer thread needs, while the lock is held — so the
    /// writer never reaches back into the library and cannot deadlock against a read.
    pub fn write_targets(&self, ids: &[i64]) -> Vec<crate::writer::Target> {
        ids.iter()
            .filter_map(|id| {
                let row = self.rows.iter().find(|r| r.item.asset_id == *id)?;
                Some(crate::writer::Target {
                    asset_id: *id,
                    abs_path: self.paths.get(id).cloned(),
                    source_root: row
                        .source
                        .as_deref()
                        .and_then(|name| self.sources.iter().find(|s| s.name == name))
                        .map(|s| PathBuf::from(&s.root_path)),
                    rel_path: row.item.rel_path.clone(),
                })
            })
            .collect()
    }

    /// The source roots the user opened — the outer bound on what delete-from-disk may touch.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.sources
            .iter()
            .map(|s| PathBuf::from(&s.root_path))
            .collect()
    }

    /// The parsed `media_type` of a known asset, for routes that behave differently per type.
    pub fn media_type(&self, asset_id: i64) -> Option<MediaType> {
        self.rows
            .iter()
            .find(|r| r.item.asset_id == asset_id)?
            .media_type
    }

    /// The albedo texture this asset paints with, whatever kind of asset it is.
    ///
    /// A model is two hops away from its texture; a **material is one** — it *is* the middle of
    /// that chain. Sending a model down the one-hop path returns its material (not an image), and
    /// sending a material down the two-hop path returns nothing at all, so the viewer renders a
    /// grey ball with no error anywhere. The branch is the whole point.
    fn albedo_texture(&self, asset_id: i64) -> Option<i64> {
        let cat = self.catalog.as_ref()?;
        match self.media_type(asset_id) {
            Some(MediaType::Material) => cat.material_albedo_texture(asset_id).ok().flatten(),
            _ => cat.model_albedo_texture(asset_id).ok().flatten(),
        }
    }

    /// The albedo texture a model paints with: `model --uses_material--> material --uses_texture-->
    /// image`, in one query.
    ///
    /// **Two hops, not one.** A single hop lands on the *material*, which is not an image — the
    /// viewer would fetch it over `lodestash://file`, fail to decode it, and silently render untextured.
    /// The join is written so that mistake cannot compile away quietly.
    pub fn model_texture(&self, asset_id: i64) -> Option<i64> {
        let cat = self.catalog.as_ref()?;
        cat.model_albedo_texture(asset_id).ok().flatten()
    }

    /// Everything `lodestash://thumb` needs to render one asset, resolved under the lock so the handler
    /// does its file work without holding it.
    pub fn thumb_job(&self, asset_id: i64) -> Option<ThumbJob> {
        let row = self.rows.iter().find(|r| r.item.asset_id == asset_id)?;
        Some(ThumbJob {
            path: self.paths.get(&asset_id).cloned(),
            media_type: row.item.media_type.clone(),
            // A material has no pixels of its own — it wears a texture reached through the graph.
            albedo: (row.item.media_type == "material")
                .then(|| self.material_texture_path(asset_id))
                .flatten(),
            in_pack: row.locator.clone(),
        })
    }

    /// The on-disk texture a material paints its shader ball with — one hop, unlike a model's two.
    fn material_texture_path(&self, material_id: i64) -> Option<PathBuf> {
        let cat = self.catalog.as_ref()?;
        let tex = cat.material_albedo_texture(material_id).ok().flatten()?;
        self.paths.get(&tex).cloned()
    }

    /// Is this an asset we know about at all?
    ///
    /// Distinguishes "no such id" from "known, but its bytes are inside a pack" — the two need
    /// different answers over `lodestash://file` (404 vs 503 `not_local`), and conflating them tells a
    /// client the asset does not exist when it is sitting in the grid in front of the user.
    pub fn knows(&self, asset_id: i64) -> bool {
        self.rows.iter().any(|r| r.item.asset_id == asset_id)
    }

    pub fn preview_svg(&self, asset_id: i64) -> Option<String> {
        let cat = self.catalog.as_ref()?;
        cat.vector_preview(asset_id)
            .ok()
            .flatten()
            .map(|(svg, _)| svg)
    }
}

/// An empty Stats Tag Graph — what `Library::tag_graph` returns when there is no catalog or
/// the query failed. The canvas renders nothing instead of crashing or stalling on a half object.
fn empty_tag_graph() -> lodestash_core::catalog::TagGraph {
    lodestash_core::catalog::TagGraph {
        nodes: Vec::new(),
        edges: Vec::new(),
        asset_count: 0,
        link_count: 0,
    }
}

/// Lifecycle order, for picking a duplicate group's keeper: the most-appraised copy is the one
/// worth keeping, because it carries the work someone already did on it (DATA-MODEL §8).
fn status_rank(s: AssetStatus) -> u8 {
    match s {
        AssetStatus::Confirmed => 3,
        AssetStatus::Identified => 2,
        AssetStatus::Indexed => 1,
        AssetStatus::Discovered => 0,
    }
}

/// Build one duplicate group from its members: keeper first (highest status, ties to earliest
/// indexed), the reclaimable bytes, and whether the members span packs (the interesting case).
fn dup_group(hash: String, mut rows: Vec<&Row>) -> DupGroup {
    rows.sort_by_key(|r| (std::cmp::Reverse(status_rank(r.status)), r.item.asset_id));
    let first_pack = rows[0].item.package.as_deref();
    DupGroup {
        hash,
        size_bytes: rows[0].item.size_bytes,
        keeper: rows[0].item.asset_id,
        cross_pack: rows.iter().any(|r| r.item.package.as_deref() != first_pack),
        items: rows.iter().map(|r| r.item.clone()).collect(),
    }
}

/// Biggest reclaimable bytes first: a group of six 40 MB textures is the one worth opening.
fn sort_groups(groups: &mut Vec<DupGroup>) {
    groups.sort_by(|a, b| {
        let waste = |g: &DupGroup| g.size_bytes * (g.items.len() as i64 - 1);
        waste(b).cmp(&waste(a)).then_with(|| a.hash.cmp(&b.hash))
    });
}

/// A facet id, taken from the type's **own** serde name rather than a string written out again
/// here. The popover posts these ids straight back inside a `ContentFilter`, so deriving them from
/// the same impl that parses them is what makes a round-trip impossible to get wrong.
fn serde_id<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v)
        .ok()
        .and_then(|j| j.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// `128 KB–1.0 MB` style range for a size tier chip.
fn kb_range(lo_kb: u32, hi_kb: u32) -> String {
    let one = |kb: u32| {
        if kb >= 1024 {
            format!("{:.1} MB", kb as f64 / 1024.0)
        } else {
            format!("{kb} KB")
        }
    };
    format!("{}–{}", one(lo_kb), one(hi_kb))
}

/// Build a grid row from a hydrated catalog entry, whatever storage class it came from.
///
/// `package` is `Some` for assets that live inside a pack — those have no local path, so the
/// row is browsable and searchable while `lodestash://file` correctly answers 503.
fn row_from(
    e: lodestash_core::catalog::HydratedAsset,
    source_id: i64,
    source: &str,
    package: Option<String>,
) -> Row {
    let name = Path::new(&e.rel_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| e.rel_path.clone());
    let browse_path = match package.as_deref() {
        Some(p) => PathBuf::from(p).join(&e.rel_path),
        None => PathBuf::from(&e.rel_path),
    };
    Row {
        content_hash: e.content_hash.clone(),
        perceptual_hash: None,
        browse_path,
        search_key: name.to_lowercase(),
        trashed: e.trashed,
        media_type: MediaType::from_code(&e.media_type),
        // An unparseable status is treated as the earliest lifecycle state rather than dropped:
        // the asset is real and browsable, it just has not been identified as far as we can tell.
        status: AssetStatus::from_code(&e.status).unwrap_or(AssetStatus::Discovered),
        mtime: e.mtime,
        width_px: e.metrics.width_px,
        height_px: e.metrics.height_px,
        duration_ms: e.metrics.duration_ms,
        source: Some(source.to_string()),
        locator: None,
        traits: Vec::new(),
        item: GridItem {
            asset_id: e.asset_id,
            source_id,
            name,
            rel_path: e.rel_path,
            media_type: e.media_type,
            size_bytes: e.size_bytes,
            status: e.status,
            // Preview and favourite flags are filled in one pass each, not a query per row.
            has_preview: false,
            favorite: false,
            in_pack: package.is_some(),
            package,
            triangles: e.metrics.tri_count,
            duration_ms: e.metrics.duration_ms,
            // All three are filled by a whole-library pass after every row exists (`mark_traits`,
            // `mark_tiers`, `mark_imported`) — none is knowable from a single catalog row.
            traits: Vec::new(),
            size_tier: String::new(),
            in_project: false,
            import_stale: false,
            in_other_projects: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(names: &[(&str, &str)]) -> Library {
        let rows = names
            .iter()
            .map(|(n, mt)| Row {
                search_key: n.to_lowercase(),
                media_type: MediaType::from_code(mt),
                status: AssetStatus::Indexed,
                mtime: 0,
                width_px: None,
                height_px: None,
                duration_ms: None,
                source: None,
                locator: None,
                trashed: false,
                traits: Vec::new(),
                content_hash: None,
                perceptual_hash: None,
                browse_path: PathBuf::from(format!("{n}.x")),
                item: GridItem {
                    asset_id: 0,
                    source_id: 1,
                    name: (*n).into(),
                    rel_path: format!("{n}.x"),
                    media_type: (*mt).into(),
                    size_bytes: 1,
                    status: "indexed".into(),
                    has_preview: false,
                    favorite: false,
                    in_pack: false,
                    package: None,
                    triangles: None,
                    duration_ms: None,
                    traits: Vec::new(),
                    size_tier: String::new(),
                    in_project: false,
                    import_stale: false,
                    in_other_projects: 0,
                },
            })
            .collect();
        Library {
            catalog: None,
            thumb_cache: lodestash_app::paths::default_cache_dirs().0,
            catalog_path: None,
            sources: Vec::new(),
            rows,
            paths: HashMap::new(),
            tiers: SizeTiers::default(),
            staged: Vec::new(),
            ai_embedder: Default::default(),
        }
    }

    /// The three tiers the appraisal queue is ordered by (UI-SPEC §3), cut from the query the grid
    /// just ran.
    ///
    /// Mutation-checked on the three things that would quietly undo "the backlog follows you":
    /// `visible` must be the page **actually on screen** (page 0's ids while the user is on page 1
    /// is a queue politely appraising what they just scrolled past); the second tier must continue
    /// *forward* from that page, because scrolling forward is what people do and a tier that
    /// restarted at the top would prioritise what they have already seen; and the filter must
    /// narrow both tiers, or focusing inside a search would hand the queue the whole library.
    #[test]
    fn focus_order_cuts_the_page_then_the_rest_of_the_filter() {
        let mut l = lib(&[
            ("alpha", "image"),
            ("bravo", "image"),
            ("charlie", "image"),
            ("delta", "image"),
            ("echo", "audio"),
            ("foxtrot", "audio"),
        ]);
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
        }

        // Page 1 of 2 (page_size 2, name-sorted): charlie + delta are on screen.
        let q = GridQuery {
            page: 1,
            page_size: 2,
            ..Default::default()
        };
        let f = l.focus_order(&q);
        assert_eq!(f.visible, vec![3, 4], "the focus is not the page on screen");
        assert_eq!(
            f.filtered,
            vec![5, 6, 1, 2],
            "the second tier must run forward from the page, then wrap"
        );

        // Page 0: nothing precedes it, so the second tier is simply the remainder.
        let f = l.focus_order(&GridQuery {
            page: 0,
            page_size: 2,
            ..Default::default()
        });
        assert_eq!(f.visible, vec![1, 2]);
        assert_eq!(f.filtered, vec![3, 4, 5, 6]);

        // A filter narrows both tiers. Without this the queue would happily prioritise the 5,000
        // assets a search just excluded.
        let f = l.focus_order(&GridQuery {
            tab: Category::Sounds,
            page: 0,
            page_size: 1,
            ..Default::default()
        });
        assert_eq!(f.visible, vec![5], "the tab filter did not reach the focus");
        assert_eq!(
            f.filtered,
            vec![6],
            "the filter's remainder leaked other types"
        );

        // A page index past the end clamps rather than producing an empty focus — the grid clamps
        // it too, and the two must agree or the queue prioritises a page nobody is looking at.
        let f = l.focus_order(&GridQuery {
            page: 99,
            page_size: 2,
            ..Default::default()
        });
        assert_eq!(
            f.visible,
            vec![5, 6],
            "a stale page index emptied the focus"
        );
    }

    /// The badge is the user's thresholds applied to the card, not a fixed scale. Boundaries are
    /// exclusive-below (`S < m_kb ≤ M`), so the assertions sit *on* each boundary — an off-by-one
    /// in `tier_of`'s comparison shows up there and nowhere else, and a card mislabelled `M` when
    /// Settings says `L` is invisible until someone measures a file by hand.
    #[test]
    fn the_size_badge_follows_the_configured_thresholds() {
        // Defaults (FEATURES §5.3): S < 1 MB, M < 4 MB, L < 8 MB, XL ≥ 8 MB.
        let mut l = lib(&[
            ("a", "model_3d"),
            ("b", "model_3d"),
            ("c", "model_3d"),
            ("d", "model_3d"),
        ]);
        let kb = 1024;
        for (i, bytes) in [1023 * kb, 1024 * kb, 4096 * kb, 8192 * kb]
            .iter()
            .enumerate()
        {
            l.rows[i].item.size_bytes = *bytes;
        }
        l.mark_tiers();
        let letters: Vec<&str> = l.rows.iter().map(|r| r.item.size_tier.as_str()).collect();
        assert_eq!(letters, ["S", "M", "L", "XL"]);

        // Mutation check: the thresholds are read, not hardcoded. Push M's boundary past the
        // second row and it must fall back to S.
        l.tiers.m_kb = 2048;
        l.mark_tiers();
        assert_eq!(
            l.rows[1].item.size_tier, "S",
            "the badge must track SizeTiers, not a fixed scale"
        );
    }

    /// The sidebar's LIBRARIES facet is a button that claims to scope the grid. Before this the
    /// query had no `source` field at all, so the click set frontend state and the grid ignored
    /// it — the failure mode is a facet that looks selected and changes nothing.
    #[test]
    fn the_library_facet_scopes_the_grid_to_one_source() {
        let mut l = lib(&[("a", "model_3d"), ("b", "model_3d"), ("c", "model_3d")]);
        l.rows[0].source = Some("Synty".into());
        l.rows[1].source = Some("Synty".into());
        l.rows[2].source = Some("Humble".into());

        let q = |s: Option<&str>| GridQuery {
            source: s.map(str::to_string),
            ..Default::default()
        };
        assert_eq!(l.page(&q(None)).total, 3, "no source means every library");
        assert_eq!(l.page(&q(Some("Synty"))).total, 2);
        assert_eq!(l.page(&q(Some("Humble"))).total, 1);
        // Mutation check: the match is on the source, not a substring or a always-true fallback.
        assert_eq!(l.page(&q(Some("synty"))).total, 0, "source match is exact");
    }

    /// The bucket rule itself lives in `lodestash_app::types` and is tested there. What this
    /// asserts is the *wiring*: that a query's tab reaches that rule, over rows whose
    /// `media_type` came out of the catalog as a string. A parse that silently yields `None`
    /// would empty every tab but All while the grid still looks populated.
    #[test]
    fn the_tab_facet_filters_through_the_shared_bucket_rule() {
        let l = lib(&[
            ("m", "model_3d"),
            ("t", "material"),
            ("s", "audio"),
            ("i", "image"),
            ("f", "font"),
        ]);
        let on = |tab| {
            l.page(&GridQuery {
                tab,
                ..Default::default()
            })
            .total
        };
        assert_eq!(on(Category::All), 5);
        assert_eq!(on(Category::Models), 1);
        assert_eq!(on(Category::Materials), 1, "materials are their own tab");
        assert_eq!(on(Category::Sounds), 1);
        assert_eq!(on(Category::Images), 2, "image + font");
    }

    /// Sort by Type follows the `media_type` enum order, not the alphabet. Sorting the code
    /// strings instead puts `audio` before `model_3d`, so the two frontends disagree on the
    /// same query — the drift `grid::media_key` exists to prevent.
    #[test]
    fn type_sort_follows_enum_order_not_alphabetical() {
        let l = lib(&[("a", "audio"), ("m", "model_3d"), ("t", "texture")]);
        let names: Vec<String> = l
            .page(&GridQuery {
                sort_by: SortBy::Type,
                ..Default::default()
            })
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        assert_eq!(
            names,
            ["m", "t", "a"],
            "model_3d, texture, audio — declaration order"
        );
    }

    /// Descending must reverse the *whole* comparison, ties included. Reversing a stably-sorted
    /// list is what makes that true here — a naive `if !asc { cmp.reverse() }` inside the
    /// comparator leaves tie-breaks ascending, so page 2 of a descending sort can repeat or
    /// drop rows that page 1 already showed.
    #[test]
    fn sorting_is_total_and_descending_is_the_exact_reverse() {
        let l = lib(&[("b", "model_3d"), ("a", "model_3d"), ("c", "model_3d")]);
        let q = |asc| GridQuery {
            sort_by: SortBy::Name,
            sort_asc: asc,
            ..Default::default()
        };
        let up: Vec<String> = l
            .page(&q(true))
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        let mut down: Vec<String> = l
            .page(&q(false))
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        assert_eq!(up, ["a", "b", "c"]);
        down.reverse();
        assert_eq!(
            up, down,
            "descending must be the exact reverse of ascending"
        );
    }

    /// Three rows with ids 1..3, so a synthetic rank map can name them.
    fn searchable() -> Library {
        let mut l = lib(&[
            ("zeta", "model_3d"),
            ("alpha", "model_3d"),
            ("mid", "model_3d"),
        ]);
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
        }
        l
    }

    /// The point of wiring FTS in: an asset matches on its **tags and description**, which the
    /// substring pass cannot see because it only has the file name. Here "haunted" appears in no
    /// row's name, so the only way `alpha` comes back is through the rank map.
    #[test]
    fn a_rank_only_hit_is_returned_even_though_no_name_contains_the_query() {
        let l = searchable();
        let ranks: HashMap<i64, usize> = [(2, 0)].into_iter().collect();
        let q = GridQuery {
            search: "haunted".into(),
            ..Default::default()
        };
        assert_eq!(l.page_with_ranks(&q, None).total, 0, "no name matches it");
        let hits = l.page_with_ranks(&q, Some(&ranks));
        assert_eq!(hits.total, 1);
        assert_eq!(hits.items[0].name, "alpha", "matched through its tags");
    }

    /// The union, not a replacement: FTS matches whole tokens by prefix, so a mid-word query
    /// only the substring pass can satisfy must keep working. Dropping the substring arm is a
    /// silent regression — the grid just quietly stops finding things it used to.
    #[test]
    fn substring_matches_survive_alongside_the_ranked_hits() {
        let l = searchable();
        let ranks: HashMap<i64, usize> = [(2, 0)].into_iter().collect();
        let q = GridQuery {
            search: "et".into(), // mid-word in "zeta"; FTS prefix search would miss it
            ..Default::default()
        };
        let names: Vec<String> = l
            .page_with_ranks(&q, Some(&ranks))
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        assert!(names.contains(&"zeta".to_string()), "substring hit kept");
        assert!(names.contains(&"alpha".to_string()), "ranked hit kept");
    }

    /// Relevance puts BM25 hits first, in rank order, and everything else after — the reason
    /// `grid::relevance_key` exists. Sorting on the bare `Option<usize>` inverts it (`None` is
    /// less than `Some`), which buries the best match under every asset that did not match.
    #[test]
    fn relevance_puts_ranked_hits_first_then_the_rest_by_name() {
        let l = searchable();
        // "zeta" ranks best, "mid" second, "alpha" is not a hit but matches as a substring.
        let ranks: HashMap<i64, usize> = [(1, 0), (3, 1)].into_iter().collect();
        let names: Vec<String> = l
            .page_with_ranks(
                &GridQuery {
                    search: "a".into(), // substring of every name, so ordering is what is tested
                    sort_by: SortBy::Relevance,
                    ..Default::default()
                },
                Some(&ranks),
            )
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        assert_eq!(
            names,
            ["zeta", "mid", "alpha"],
            "rank 0, rank 1, then the unranked"
        );
    }

    /// A soft-deleted asset is absent from every normal view and present in exactly one. Getting
    /// this backwards either hides the Trash (nothing to restore from) or leaks deleted assets
    /// into the library, which is worse — the user thinks the delete did not work and tries again.
    #[test]
    fn trashed_rows_appear_only_in_the_trash_view() {
        let mut l = lib(&[("kept", "model_3d"), ("binned", "model_3d")]);
        l.rows[1].trashed = true;

        let live = l.page(&GridQuery::default());
        assert_eq!(live.total, 1);
        assert_eq!(live.items[0].name, "kept");

        let trash = l.page(&GridQuery {
            trashed_only: true,
            ..Default::default()
        });
        assert_eq!(trash.total, 1);
        assert_eq!(trash.items[0].name, "binned");

        // And the counts follow the same rule, or the nav disagrees with the grid.
        let s = l.status();
        assert_eq!(
            s.total_assets, 1,
            "the Trash is not part of the library count"
        );
        assert_eq!(s.trashed, 1);
    }

    /// Rows that compare **fully** equal — same name, same key — still need a deterministic
    /// order, or descending is not the reverse of ascending and a page boundary can show the
    /// same asset twice. A stable sort preserves load order in both directions, so the
    /// comparator ends in `asset_id`. Caught in e2e first: this library has duplicate names.
    #[test]
    fn rows_that_tie_on_every_key_still_reverse_cleanly() {
        let mut l = lib(&[
            ("dup", "model_3d"),
            ("dup", "model_3d"),
            ("dup", "model_3d"),
        ]);
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
            r.item.size_bytes = 100;
        }
        let ids = |asc| -> Vec<i64> {
            l.page(&GridQuery {
                sort_by: SortBy::Size,
                sort_asc: asc,
                ..Default::default()
            })
            .items
            .iter()
            .map(|i| i.asset_id)
            .collect()
        };
        let mut down = ids(false);
        down.reverse();
        assert_eq!(ids(true), down, "identical rows must still order totally");
    }

    #[test]
    fn size_sort_breaks_ties_by_name_so_paging_is_stable() {
        let mut l = lib(&[("z", "model_3d"), ("a", "model_3d"), ("m", "model_3d")]);
        for r in &mut l.rows {
            r.item.size_bytes = 100; // every row identical on the sort key
        }
        let names: Vec<String> = l
            .page(&GridQuery {
                sort_by: SortBy::Size,
                ..Default::default()
            })
            .items
            .iter()
            .map(|i| i.name.clone())
            .collect();
        assert_eq!(
            names,
            ["a", "m", "z"],
            "equal sizes fall back to name order"
        );
    }

    /// Wiring again: the fold itself is tested in `lodestash_app::types`. This asserts the
    /// catalog's status *string* reaches it — the four states arrive as text and are parsed once
    /// at hydrate, so a parse failure would silently file every asset under Unidentified.
    #[test]
    fn the_status_facet_filters_through_the_shared_fold() {
        let mut l = lib(&[("a", "model_3d"), ("b", "model_3d"), ("c", "model_3d")]);
        l.rows[0].status = AssetStatus::Discovered;
        l.rows[1].status = AssetStatus::Identified;
        l.rows[2].status = AssetStatus::Confirmed;
        let on = |status| {
            l.page(&GridQuery {
                status,
                ..Default::default()
            })
            .total
        };
        assert_eq!(on(StatusFilter::All), 3);
        assert_eq!(on(StatusFilter::Unidentified), 1);
        assert_eq!(on(StatusFilter::Identified), 1);
        assert_eq!(on(StatusFilter::Confirmed), 1);
    }

    /// A row hydrated from the catalog carries parsed enums, not just the JSON strings — the
    /// step both facet tests above depend on and neither exercises directly.
    #[test]
    fn hydrating_a_row_parses_its_media_type_and_status() {
        let e = lodestash_core::catalog::HydratedAsset {
            rel_path: "Assets/Rock.fbx".into(),
            asset_id: 7,
            media_type: "model_3d".into(),
            size_bytes: 10,
            mtime: 0,
            status: "identified".into(),
            trashed: false,
            rating: None,
            metrics: Default::default(),
            content_hash: None,
        };
        let row = row_from(e.clone(), 1, "Synty", None);
        assert_eq!(row.media_type, Some(MediaType::Model3d));
        assert_eq!(row.status, AssetStatus::Identified);
        assert_eq!(row.item.media_type, "model_3d", "the JSON keeps the code");
        assert_eq!(
            row.browse_path,
            PathBuf::from("Assets/Rock.fbx"),
            "a loose file browses at its own path"
        );

        // …and an in-pack asset browses under its pack, or every pack in the source merges into
        // one `Assets` folder (see the `browse_path` field doc).
        let packed = row_from(e, 1, "Synty", Some("PolygonCity".into()));
        assert_eq!(
            packed.browse_path,
            PathBuf::from("PolygonCity/Assets/Rock.fbx")
        );
    }

    /// Favorites is a *filter over the library*, not a separate screen with its own data. The
    /// bug this guards: routing Favorites at the same grid without passing the flag renders the
    /// entire library under a "Favorites" heading, which reads as "you have starred everything".
    #[test]
    fn favorites_only_narrows_to_starred_assets() {
        let mut l = lib(&[("a", "model_3d"), ("b", "model_3d"), ("c", "audio")]);
        l.rows[1].item.favorite = true;

        let all = l.page(&GridQuery::default());
        assert_eq!(all.total, 3, "the unfiltered grid still shows everything");

        let starred = l.page(&GridQuery {
            favorites_only: true,
            ..Default::default()
        });
        assert_eq!(starred.total, 1);
        assert_eq!(starred.items[0].name, "b");
        assert!(starred.items.iter().all(|i| i.favorite));
    }

    /// Favourites compose with the other facets rather than replacing them — a starred audio
    /// file must not appear under Favorites + Models.
    #[test]
    fn favorites_compose_with_tab_and_search() {
        let mut l = lib(&[("rock", "model_3d"), ("rain", "audio")]);
        for r in &mut l.rows {
            r.item.favorite = true;
        }
        let q = GridQuery {
            favorites_only: true,
            tab: Category::Models,
            ..Default::default()
        };
        assert_eq!(
            l.page(&q).total,
            1,
            "the audio favourite is excluded by the tab"
        );
    }

    /// The trait facet, which is the whole point of the popover: a chip narrows to the assets
    /// carrying that tag, and chips within a group OR together (UI-SPEC §2.3).
    #[test]
    fn the_trait_facet_narrows_to_assets_carrying_the_tag() {
        let mut l = lib(&[("a", "model_3d"), ("b", "model_3d"), ("c", "model_3d")]);
        l.rows[0].traits = vec!["stylized".into()];
        l.rows[1].traits = vec!["stylized".into(), "medieval".into()];
        let with = |names: &[&str]| GridQuery {
            filter: ContentFilter {
                traits: names.iter().map(|s| (*s).to_string()).collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(l.page(&with(&["stylized"])).total, 2);
        assert_eq!(l.page(&with(&["medieval"])).total, 1);
        assert_eq!(
            l.page(&with(&["stylized", "medieval"])).total,
            2,
            "chips inside a group OR together"
        );
    }

    /// Facets AND across groups. Getting this backwards (OR) turns every extra chip into a
    /// *wider* result, which reads as the filter being broken.
    #[test]
    fn facet_groups_and_together() {
        let mut l = lib(&[("a", "model_3d"), ("b", "model_3d")]);
        for r in &mut l.rows {
            r.traits = vec!["stylized".into()];
        }
        l.rows[0].item.triangles = Some(500); // ≤1k
        l.rows[1].item.triangles = Some(9_000); // 6k+
        let p = l.page(&GridQuery {
            filter: ContentFilter {
                traits: ["stylized".to_string()].into_iter().collect(),
                tri_buckets: [TriBucket::Le1k].into_iter().collect(),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(p.total, 1, "trait AND bucket, not trait OR bucket");
        assert_eq!(p.items[0].name, "a");
    }

    /// An asset with no triangle count is **excluded** when any triangle bucket is selected
    /// (design §3) — a texture is not a 0-triangle model, and filing it under `≤ 1k` would put
    /// every image in the library in front of someone filtering for low-poly meshes.
    #[test]
    fn assets_without_a_tri_count_are_excluded_by_a_tri_bucket() {
        let mut l = lib(&[("mesh", "model_3d"), ("png", "image")]);
        l.rows[0].item.triangles = Some(400);
        let p = l.page(&GridQuery {
            filter: ContentFilter {
                tri_buckets: [TriBucket::Le1k].into_iter().collect(),
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(p.total, 1);
        assert_eq!(p.items[0].name, "mesh");
    }

    /// The **wire contract**: the ids `facets()` publishes are exactly the ids a `ContentFilter`
    /// parses back. A popover posting `"le1k"` at a filter expecting `"Le1k"` silently matches
    /// nothing — the grid empties and nothing anywhere says why, so this round-trips every id
    /// through real JSON rather than comparing two Rust values.
    #[test]
    fn every_facet_id_round_trips_back_into_a_filter_that_matches() {
        let mut l = lib(&[("small", "model_3d"), ("big", "model_3d")]);
        l.rows[0].item.triangles = Some(500);
        l.rows[1].item.triangles = Some(9_000);
        l.rows[0].item.size_bytes = 1_024; // 1 KB — tier S under any legal boundary set
        l.rows[1].item.size_bytes = 64 * 1024 * 1024; // 64 MB — XL

        let f = l.facets();
        let parse = |json: String| -> ContentFilter {
            serde_json::from_str(&json).expect("facet ids must parse back into a ContentFilter")
        };

        let smallest = &f.tri_buckets[0].id;
        let p = l.page(&GridQuery {
            filter: parse(format!(r#"{{"tri_buckets":["{smallest}"]}}"#)),
            ..Default::default()
        });
        assert_eq!(p.total, 1, "the ≤1k chip must select the ≤1k asset");
        assert_eq!(p.items[0].name, "small");

        let largest = &f.sizes[3].id;
        let p = l.page(&GridQuery {
            filter: parse(format!(r#"{{"sizes":["{largest}"]}}"#)),
            ..Default::default()
        });
        assert_eq!(p.total, 1, "the XL chip must select the XL asset");
        assert_eq!(p.items[0].name, "big");
    }

    /// The cloud is ordered by how many assets carry each trait, and counts only the live library
    /// — a trait whose every asset is in the Trash is a chip that selects an empty grid.
    #[test]
    fn the_trait_cloud_counts_live_rows_most_used_first() {
        let mut l = lib(&[("a", "model_3d"), ("b", "model_3d"), ("c", "model_3d")]);
        l.rows[0].traits = vec!["common".into()];
        l.rows[1].traits = vec!["common".into()];
        l.rows[2].traits = vec!["binned".into()];
        l.rows[2].trashed = true;

        let f = l.facets();
        assert_eq!(f.traits.len(), 1, "the trashed row's trait is not offered");
        assert_eq!(f.traits[0].name, "common");
        assert_eq!(f.traits[0].count, 2);
    }

    /// Rows placed in a source, at a path, optionally inside a pack — what the Folders view walks.
    fn foldered(rows: &[(&str, &str, Option<&str>, &str)]) -> Library {
        let mut l = lib(&rows
            .iter()
            .map(|(name, mt, _, _)| (*name, *mt))
            .collect::<Vec<_>>());
        for (r, (_, _, pack, path)) in l.rows.iter_mut().zip(rows) {
            r.source = Some("Synty".into());
            r.item.rel_path = (*path).to_string();
            r.item.in_pack = pack.is_some();
            r.item.package = pack.map(|p| p.to_string());
            r.browse_path = match pack {
                Some(p) => PathBuf::from(p).join(path),
                None => PathBuf::from(path),
            };
        }
        l
    }

    /// The top level is the **sources**, not folders. Landing straight in a directory tree loses
    /// the answer to "where is this from?", which the browse-views design exists to keep.
    #[test]
    fn the_top_level_of_folders_lists_sources() {
        let l = foldered(&[
            ("a", "model_3d", None, "Assets/Rocks/a.fbx"),
            ("b", "model_3d", None, "Assets/b.fbx"),
        ]);
        let p = l.folder_page(&FolderQuery::default());
        assert_eq!(p.tiles.len(), 1);
        assert_eq!(p.tiles[0].name, "Synty");
        assert_eq!(p.tiles[0].recursive, 2);
        assert!(p.items.is_empty(), "no files hang off the top level");
    }

    /// Inside a source: subfolders become tiles, files at this level become items, and the counts
    /// distinguish "directly in here" from "somewhere under here".
    #[test]
    fn a_folder_level_splits_tiles_from_the_files_directly_in_it() {
        let l = foldered(&[
            ("deep", "model_3d", None, "Assets/Rocks/Big/deep.fbx"),
            ("near", "model_3d", None, "Assets/Rocks/near.fbx"),
            ("here", "model_3d", None, "Assets/here.fbx"),
        ]);
        let p = l.folder_page(&FolderQuery {
            source: Some("Synty".into()),
            dir: "Assets".into(),
            ..Default::default()
        });
        assert_eq!(p.items.len(), 1, "only the file directly in Assets/");
        assert_eq!(p.items[0].name, "here");
        assert_eq!(p.tiles.len(), 1);
        assert_eq!(p.tiles[0].name, "Rocks");
        assert_eq!(p.tiles[0].direct, 1, "near.fbx is directly in Rocks/");
        assert_eq!(p.tiles[0].recursive, 2, "…and deep.fbx is under it");
        assert_eq!(
            p.total, 3,
            "the header counts everything at or under this level"
        );
    }

    /// **Packs do not merge.** Every `.unitypackage` lays its assets out under `Assets/…`, and one
    /// source holds dozens of them, so browsing by `rel_path` alone collapses unrelated products
    /// into one `Assets` folder of thousands of files. In-pack assets are the majority of a real
    /// library, so this is the difference between a browsable tree and an anonymous pile.
    #[test]
    fn in_pack_assets_browse_under_their_pack_rather_than_merging() {
        let l = foldered(&[
            ("city", "model_3d", Some("PolygonCity"), "Assets/city.fbx"),
            ("farm", "model_3d", Some("PolygonFarm"), "Assets/farm.fbx"),
        ]);
        let top = l.folder_page(&FolderQuery {
            source: Some("Synty".into()),
            ..Default::default()
        });
        let names: Vec<&str> = top.tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["PolygonCity", "PolygonFarm"],
            "each pack is its own folder, not a shared Assets/"
        );

        let inside = l.folder_page(&FolderQuery {
            source: Some("Synty".into()),
            dir: "PolygonCity/Assets".into(),
            ..Default::default()
        });
        assert_eq!(inside.items.len(), 1);
        assert_eq!(inside.items[0].name, "city");
    }

    /// The pack browser's left tree (PACK-DIRECTORY-NAV §5): one node per folder, with the
    /// `direct (recursive)` counts the tree and the contents list both show, and the chevron flag.
    #[test]
    fn pack_tree_lists_folders_with_direct_and_recursive_counts() {
        let l = foldered(&[
            (
                "rock",
                "model_3d",
                Some("PolygonCity"),
                "Assets/Props/rock.fbx",
            ),
            (
                "tree",
                "model_3d",
                Some("PolygonCity"),
                "Assets/Props/tree.fbx",
            ),
            ("sky", "texture", Some("PolygonCity"), "Assets/Env/sky.png"),
            ("readme", "image", Some("PolygonCity"), "readme.txt"),
        ]);
        let tree = l.pack_tree("PolygonCity");
        let nodes: Vec<(&str, usize, usize, usize, bool)> = tree
            .iter()
            .map(|n| {
                (
                    n.name.as_str(),
                    n.depth,
                    n.direct,
                    n.recursive,
                    n.has_children,
                )
            })
            .collect();
        assert_eq!(
            nodes,
            [
                ("Assets", 1, 0, 3, true),
                ("Env", 2, 1, 1, false),
                ("Props", 2, 2, 2, false),
            ],
            "folders, their direct (recursive) counts, and whether a chevron shows"
        );
        assert_eq!(tree[0].path, "Assets", "the path is pack-relative");
        assert!(
            l.pack_tree("PolygonFarm").is_empty(),
            "another pack's tree is its own"
        );
    }

    /// Pack-rooted browsing (PACK-DIRECTORY-NAV): with `package` set, `dir` is relative to the
    /// pack, only that pack's assets are browsed, and the top level is the pack's folders — not
    /// the sources. The list's third column arrives with the items (duration for audio).
    #[test]
    fn a_pack_rooted_folder_page_browses_only_that_pack() {
        let mut l = foldered(&[
            (
                "rock",
                "model_3d",
                Some("PolygonCity"),
                "Assets/Props/rock.fbx",
            ),
            ("city", "model_3d", Some("PolygonCity"), "Assets/city.fbx"),
            ("hum", "audio", Some("PolygonCity"), "Assets/hum.wav"),
            ("farm", "model_3d", Some("PolygonFarm"), "Assets/farm.fbx"),
        ]);
        for r in &mut l.rows {
            if r.item.name == "hum" {
                r.duration_ms = Some(161_000);
            }
        }

        let root = l.folder_page(&FolderQuery {
            package: Some("PolygonCity".into()),
            ..Default::default()
        });
        let names: Vec<&str> = root.tiles.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names,
            ["Assets"],
            "the pack's own top folders, not every source's"
        );
        assert!(
            root.items.is_empty(),
            "nothing lives directly in the pack root"
        );

        let inside = l.folder_page(&FolderQuery {
            package: Some("PolygonCity".into()),
            dir: "Assets".into(),
            ..Default::default()
        });
        let item_names: Vec<&str> = inside.items.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(
            item_names,
            ["city", "hum"],
            "PolygonFarm's assets never appear"
        );
        assert_eq!(
            inside.third,
            ["—", "2:41"],
            "third column: dimensions/duration or —"
        );
        assert_eq!(inside.tiles.len(), 1);
        assert_eq!(inside.tiles[0].name, "Props");
        assert_eq!(inside.tiles[0].direct, 1);
        assert_eq!(inside.tiles[0].recursive, 1);
        assert_eq!(
            inside.total, 3,
            "the header counts everything under this pack folder"
        );
    }

    /// Search inside Folders narrows the **subtree**, tile counts included — a tile promising 40
    /// assets that opens onto 2 is worse than showing no count at all.
    #[test]
    fn searching_in_folders_narrows_the_tile_counts_too() {
        let l = foldered(&[
            ("rock_01", "model_3d", None, "Assets/Rocks/rock_01.fbx"),
            ("rock_02", "model_3d", None, "Assets/Rocks/rock_02.fbx"),
            ("tree_01", "model_3d", None, "Assets/Rocks/tree_01.fbx"),
        ]);
        let q = |search: &str| FolderQuery {
            source: Some("Synty".into()),
            dir: "Assets".into(),
            search: search.into(),
            ..Default::default()
        };
        assert_eq!(l.folder_page(&q("")).tiles[0].recursive, 3);
        assert_eq!(l.folder_page(&q("rock")).tiles[0].recursive, 2);
    }

    /// The toolbar means the same thing in both browse views. A Folders tree that ignores the tab
    /// is a second library that disagrees with the first one.
    #[test]
    fn the_tab_and_the_filter_reach_the_folders_view() {
        let mut l = foldered(&[
            ("mesh", "model_3d", None, "Assets/Art/mesh.fbx"),
            ("noise", "audio", None, "Assets/Art/noise.wav"),
        ]);
        l.rows[0].traits = vec!["stylized".into()];
        let at = |q: FolderQuery| l.folder_page(&q).tiles[0].recursive;
        assert_eq!(
            at(FolderQuery {
                source: Some("Synty".into()),
                dir: "Assets".into(),
                tab: Category::Models,
                ..Default::default()
            }),
            1,
            "the tab filters the tile counts"
        );
        assert_eq!(
            at(FolderQuery {
                source: Some("Synty".into()),
                dir: "Assets".into(),
                filter: ContentFilter {
                    traits: ["stylized".to_string()].into_iter().collect(),
                    ..Default::default()
                },
                ..Default::default()
            }),
            1,
            "and so does the ⚑ filter"
        );
    }

    /// A trashed asset is gone from every view but the Trash — including this one, which walks a
    /// different code path from the grid and so can lose the rule on its own.
    #[test]
    fn folders_never_show_a_trashed_asset() {
        let mut l = foldered(&[
            ("kept", "model_3d", None, "Assets/Art/kept.fbx"),
            ("binned", "model_3d", None, "Assets/Art/binned.fbx"),
        ]);
        l.rows[1].trashed = true;
        let p = l.folder_page(&FolderQuery {
            source: Some("Synty".into()),
            dir: "Assets/Art".into(),
            ..Default::default()
        });
        assert_eq!(p.items.len(), 1);
        assert_eq!(p.items[0].name, "kept");
    }

    /// Stats summarise the **live** library, or the overview disagrees with the grid it is
    /// summarising — a screen that says 8,237 assets over a grid showing 8,236 is a bug report
    /// nobody can act on.
    #[test]
    fn stats_count_the_live_library_and_bucket_by_type() {
        let mut l = lib(&[
            ("a", "model_3d"),
            ("b", "model_3d"),
            ("c", "audio"),
            ("gone", "model_3d"),
        ]);
        l.rows[3].trashed = true;
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
            r.item.size_bytes = 100;
        }
        l.rows[0].item.triangles = Some(900);
        l.rows[1].status = AssetStatus::Identified;

        let s = l.stats();
        assert_eq!(
            s.total_assets, 3,
            "the trashed asset is not part of the library"
        );
        assert_eq!(s.bytes_total, 300, "…nor of its size on disk");
        assert_eq!(s.models, 2);
        assert_eq!(s.triangles_total, 900);
        assert_eq!(
            s.unidentified, 2,
            "discovered and indexed both read Unidentified"
        );

        let models = s
            .by_type
            .iter()
            .find(|t| t.media_type == "model_3d")
            .expect("a model bucket");
        assert_eq!(models.count, 2);
        assert_eq!(models.bytes, 200);
    }

    /// Duplicates group on the content hash. **A missing hash is not a hash**: rows the hash drain
    /// has not reached yet would otherwise collect into one enormous "duplicate" group containing
    /// most of a freshly scanned library.
    #[test]
    fn unhashed_assets_never_group_as_duplicates() {
        let mut l = lib(&[("a", "model_3d"), ("b", "model_3d"), ("c", "model_3d")]);
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
        }
        assert!(
            l.duplicates().is_empty(),
            "three unhashed rows are not a group"
        );

        l.rows[0].content_hash = Some("beef".into());
        l.rows[1].content_hash = Some("beef".into());
        let g = l.duplicates();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].items.len(), 2);
    }

    /// The keeper is the copy carrying the most work: highest status, ties to the earliest
    /// indexed. Getting it backwards proposes trashing the identified copy and keeping the
    /// unappraised one — a destructive suggestion that looks reasonable on screen.
    #[test]
    fn the_keeper_is_the_most_appraised_copy() {
        let mut l = lib(&[
            ("first", "model_3d"),
            ("second", "model_3d"),
            ("third", "model_3d"),
        ]);
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
            r.content_hash = Some("same".into());
        }
        l.rows[2].status = AssetStatus::Confirmed;

        let g = l.duplicates();
        assert_eq!(g[0].keeper, 3, "the confirmed copy keeps, not the earliest");
        assert_eq!(g[0].items[0].asset_id, 3, "keeper leads the member list");

        // With no status to separate them, the earliest indexed wins.
        l.rows[2].status = AssetStatus::Indexed;
        assert_eq!(l.duplicates()[0].keeper, 1);
    }

    /// Groups lead with the most reclaimable bytes — six copies of a 40 MB texture before two
    /// copies of a 2 KB one, whatever order the hash map happened to yield.
    #[test]
    fn duplicate_groups_lead_with_the_most_reclaimable_bytes() {
        let mut l = lib(&[
            ("small_a", "texture"),
            ("small_b", "texture"),
            ("big_a", "texture"),
            ("big_b", "texture"),
        ]);
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
        }
        l.rows[0].content_hash = Some("small".into());
        l.rows[1].content_hash = Some("small".into());
        l.rows[2].content_hash = Some("big".into());
        l.rows[3].content_hash = Some("big".into());
        l.rows[0].item.size_bytes = 1_000;
        l.rows[1].item.size_bytes = 1_000;
        l.rows[2].item.size_bytes = 5_000_000;
        l.rows[3].item.size_bytes = 5_000_000;

        let g = l.duplicates();
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].hash, "big", "the heavy group leads");
    }

    /// Same bytes in two different packs is disk you can reclaim; twice inside one pack is how
    /// that pack was built. The screen leads with the first, so the flag has to be right.
    #[test]
    fn cross_pack_duplicates_are_flagged_as_such() {
        let mut l = lib(&[("a", "texture"), ("b", "texture"), ("c", "texture")]);
        for (i, r) in l.rows.iter_mut().enumerate() {
            r.item.asset_id = i as i64 + 1;
            r.content_hash = Some("h".into());
            r.item.package = Some("PolygonCity".into());
        }
        assert!(
            !l.duplicates()[0].cross_pack,
            "all three came from one pack"
        );
        l.rows[2].item.package = Some("PolygonFarm".into());
        assert!(l.duplicates()[0].cross_pack);
    }

    /// Browse → is only honest if the pack filter composes with everything else; a pack view that
    /// silently ignored the tab would show a count the toolbar disagrees with.
    #[test]
    fn the_pack_filter_narrows_and_composes() {
        let mut l = lib(&[("a", "model_3d"), ("b", "audio"), ("c", "model_3d")]);
        l.rows[0].item.package = Some("PolygonCity".into());
        l.rows[1].item.package = Some("PolygonCity".into());
        l.rows[2].item.package = Some("PolygonFarm".into());

        let q = |pack: &str, tab| GridQuery {
            package: Some(pack.into()),
            tab,
            ..Default::default()
        };
        assert_eq!(l.page(&q("PolygonCity", Category::All)).total, 2);
        assert_eq!(l.page(&q("PolygonFarm", Category::All)).total, 1);
        assert_eq!(
            l.page(&q("PolygonCity", Category::Models)).total,
            1,
            "the tab still applies inside a pack"
        );
        assert_eq!(
            l.page(&GridQuery::default()).total,
            3,
            "no pack means no constraint"
        );
    }

    #[test]
    fn search_is_case_insensitive_substring() {
        let l = lib(&[("Rock_01", "model_3d"), ("tree", "model_3d")]);
        let q = GridQuery {
            search: "ROCK".into(),
            ..Default::default()
        };
        let p = l.page(&q);
        assert_eq!(p.total, 1);
        assert_eq!(p.items[0].name, "Rock_01");
    }

    #[test]
    fn paging_splits_and_reports_totals() {
        let names: Vec<(String, &str)> =
            (0..25).map(|i| (format!("a{i:02}"), "model_3d")).collect();
        let l = lib(&names
            .iter()
            .map(|(n, m)| (n.as_str(), *m))
            .collect::<Vec<_>>());
        let q = |page| GridQuery {
            page,
            page_size: 10,
            ..Default::default()
        };
        assert_eq!(l.page(&q(0)).items.len(), 10);
        assert_eq!(l.page(&q(2)).items.len(), 5, "last page is partial");
        assert_eq!(l.page(&q(0)).pages, 3);
    }

    #[test]
    fn a_page_index_past_the_end_clamps_rather_than_blanking() {
        // The filter narrowing under a deep pager is the real case: the user is on page 9,
        // types a query matching two assets, and must see those two — not an empty grid.
        let l = lib(&[("rock", "model_3d"), ("rocket", "model_3d")]);
        let p = l.page(&GridQuery {
            page: 9,
            page_size: 10,
            search: "rock".into(),
            ..Default::default()
        });
        assert_eq!(p.page, 0);
        assert_eq!(p.items.len(), 2);
    }
}
