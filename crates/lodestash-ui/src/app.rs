//! Main application state and the eframe::App impl.
//! Spec: DESIGN-GUIDELINES.md §4, §7, §9; ARCHITECTURE.md §6.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use egui::{Key, RichText, ScrollArea, TextureHandle, Ui, Vec2};
use lodestash_core::catalog::{
    Catalog, CrawlRecord, HydratedAsset, PackageEntryRecord, StatusCounts,
};
use lodestash_core::types::{AssetStatus, MediaType};
use lodestash_ingest::asset_store::{self, AssetStorePackage};
use lodestash_ingest::crawler::{self, CrawledFile};
use lodestash_ingest::{detect, thumb, unitypackage};

use crate::audio::AudioPlayer;
use crate::config::{self, AppConfig, SizeTier, SizeTiers, SortBy, ViewMode};
use crate::jobs::{Intent, JobProgress, Jobs};
use crate::models;
use crate::theme;
use crate::viewport3d::{self, Viewport3d};

// ── Category tabs ─────────────────────────────────────────────────────────────

/// The top-bar category tabs (UI-SPEC §0.3, prototype `catDefs`): four coarse buckets
/// grouping the 7-value `media_type` enum. The enum itself is unchanged (a data-model
/// invariant) — these are a presentation grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    All,
    Models,
    Sounds,
    Images,
}

impl Category {
    const TABS: &'static [(Category, &'static str)] = &[
        (Category::All, "\u{25C6} All"),
        (Category::Models, "\u{2B21} Models"),
        (Category::Sounds, "\u{266A} Sounds"),
        (Category::Images, "\u{25A6} Images"),
    ];

    /// Bucket → `media_type` mapping: Models = model_3d+material, Sounds = audio,
    /// Images = image+texture+sprite+font (UI-SPEC §0.3).
    fn matches(self, mt: Option<MediaType>) -> bool {
        match self {
            Category::All => true,
            Category::Models => matches!(mt, Some(MediaType::Model3d | MediaType::Material)),
            Category::Sounds => mt == Some(MediaType::Audio),
            Category::Images => matches!(
                mt,
                Some(MediaType::Image | MediaType::Texture | MediaType::Sprite | MediaType::Font)
            ),
        }
    }

    /// Active-tab color (prototype `catColors`).
    fn color(self) -> egui::Color32 {
        match self {
            Category::All => theme::GOLD,
            Category::Models => theme::TYPE_MODEL,
            Category::Sounds => theme::TYPE_SOUND,
            Category::Images => theme::TYPE_IMAGE,
        }
    }
}

// ── Asset entry ───────────────────────────────────────────────────────────────

struct AssetEntry {
    /// Extracted-package origin (the sidebar "package library" facet). None for plain
    /// on-disk files. ponytail: session-only — rehydration re-derives it when the
    /// catalog packages model lands (P2 card).
    package: Option<String>,
    file: CrawledFile,
    /// Index into `scan_roots` — which source this asset came from (the Library Source).
    source_idx: usize,
    /// Catalog identity, attached when the write-behind persist reads back
    /// (None until then, or for files the catalog doesn't hold — unclassified).
    asset_id: Option<i64>,
    /// Catalog lifecycle status (DATA-MODEL §8); entries default to Discovered
    /// until the read-back lands.
    status: AssetStatus,
    /// In the Trash (catalog-only soft delete; files on disk untouched). Hidden from
    /// the grid, listed on the Trash screen (DATA-MODEL §8).
    trashed: bool,
    /// 1..=5 stars; None = unrated (DATA-MODEL §12).
    rating: Option<u8>,
    texture: Option<TextureHandle>,
    thumb_spawned: bool,
}

struct ThumbReady {
    index: usize,
    /// None when generate_texture fails; sent so pending_thumbs is always decremented.
    texture: Option<TextureHandle>,
}

/// Result of a background crawl. `generation` guards against a stale scan landing
/// after the user has already opened a different folder/project.
struct ScanDone {
    generation: u64,
    /// (source index into scan_roots, crawled file)
    entries: Vec<(usize, CrawledFile)>,
    pending_packages: Vec<PendingPackage>,
}

/// Sidebar LIBRARIES facet selection: a source root, or one extracted package.
#[derive(Clone, PartialEq)]
enum LibraryFilter {
    Source(usize),
    Package(String),
}

/// A `.unitypackage` found while crawling, not yet extracted into the grid. Its manifest is
/// already resolved (read on the same background crawl thread) so the UI can offer an "Extract"
/// action immediately, with no separate loading step.
struct PendingPackage {
    source_idx: usize,
    abs_path: PathBuf,
    display_name: String,
    /// (display label, real `pathname`) for each nested sub-package — e.g. a URP/Built-in
    /// render-pipeline-variant wrapper (INGESTION.md §2 "Container nuance"). Empty when this
    /// package directly contains real assets (no wrapper).
    nested_candidates: Vec<(String, String)>,
    /// Index into `nested_candidates`, defaulted to the URP variant when one exists.
    chosen_variant: usize,
    extracting: bool,
    progress: Arc<JobProgress>,
    /// Manifest locators (guid/pathname/tar-offset/size) captured at browse time —
    /// persisted as `package_entry` rows on extraction so surgical re-extraction never
    /// needs a second full scan (DATA-MODEL §7, INGESTION §2).
    entries: Vec<PackageEntryRecord>,
    /// Tray row of the running extraction (full or surgical), if any.
    job: Option<u64>,
    /// Contents list expanded under the package row (pick-and-extract).
    browsing: bool,
    /// Indices into `entries` picked for surgical extraction.
    picked: std::collections::BTreeSet<usize>,
}

impl PendingPackage {
    /// Indices of entries a surgical extraction can pull: a real (non-.meta) pathname
    /// plus the tar offset + size recorded at browse time.
    fn extractable(&self) -> impl Iterator<Item = usize> + '_ {
        self.entries.iter().enumerate().filter_map(|(i, e)| {
            let p = e.pathname.as_deref()?;
            (!p.to_ascii_lowercase().ends_with(".meta")
                && e.tar_offset.is_some()
                && e.size_bytes.is_some())
            .then_some(i)
        })
    }
}

/// Longest side a full-detail viewer texture may have; larger sources are
/// downscaled on the load thread (GPU texture limits, and 8k is beyond any zoom
/// level the overlay can usefully show).
const VIEWER_MAX_DIM: u32 = 8192;

/// Full-detail image loaded for the Quick Look viewer (issue #29 item 1).
struct ViewerLoaded {
    entry_idx: usize,
    /// (pixels, original dimensions before any downscale) or a decode error.
    result: Result<(egui::ColorImage, [u32; 2]), String>,
}

/// Quick Look's full-detail image state: what's loaded, and the zoom/pan/filter view.
#[derive(Default)]
struct ImageViewer {
    /// Entry the current texture (or in-flight load) belongs to.
    entry_idx: Option<usize>,
    texture: Option<TextureHandle>,
    /// Kept for re-upload when the NN toggle changes the sampler.
    image: Option<egui::ColorImage>,
    /// Original source dimensions (pre-downscale).
    source_dims: [u32; 2],
    loading: bool,
    error: Option<String>,
    /// None = fit-to-window; Some(z) = texels-per-point.
    zoom: Option<f32>,
    /// Pan offset from center, in points.
    offset: Vec2,
    /// Pixel-perfect nearest-neighbour sampling (design: image viewer NN toggle).
    nearest: bool,
}

impl ImageViewer {
    fn reset_for(&mut self, entry_idx: usize) {
        *self = ImageViewer {
            entry_idx: Some(entry_idx),
            nearest: self.nearest, // sampler preference survives navigation
            loading: true,
            ..Default::default()
        };
    }

    fn texture_options(&self) -> egui::TextureOptions {
        if self.nearest {
            egui::TextureOptions::NEAREST
        } else {
            egui::TextureOptions::LINEAR
        }
    }
}

/// Startup hydrate: the catalog's view of the sources, shown instantly while the
/// filesystem crawl reconciles in the background. Superseded by that crawl's ScanDone.
struct HydrateDone {
    generation: u64,
    /// (source index, hydrated rows) per source.
    sources: Vec<(usize, Vec<HydratedAsset>)>,
    /// Package-extracted rows (only when "cache extracted packs" is on), with the
    /// workspace-existence check already done on the hydrate thread.
    extracted: Vec<(usize, Vec<lodestash_core::catalog::HydratedExtracted>)>,
}

/// Per-root asset identity maps: root → (rel_path → (asset id, status)).
type AssetIdentityMaps = Vec<(
    String,
    std::collections::HashMap<String, (i64, AssetStatus, bool, Option<u8>)>,
)>;

/// Result of the write-behind catalog persist that follows every finished scan.
/// `counts` is None when the persist failed (logged; the UI falls back to in-memory).
struct CatalogSynced {
    generation: u64,
    counts: Option<StatusCounts>,
    /// The read path: per source root, `rel_path → (asset id, status)` for every live
    /// catalog row, joined onto the in-memory entries on receipt.
    assets: AssetIdentityMaps,
}

/// An extracted package's worth of persist work: the `package` row plus its
/// extracted assets (DATA-MODEL §7), handed to the catalog thread.
struct PackagePersist {
    /// Source root the package was found under (existing or to-be-created source row).
    root: String,
    root_name: String,
    package_path: String,
    name: String,
    publisher: Option<String>,
    category: Option<String>,
    records: Vec<CrawlRecord>,
    /// Manifest locators for `package_entry` (surgical re-extraction, DATA-MODEL §7).
    entries: Vec<PackageEntryRecord>,
    /// (name, root) of every project source, for the read-back status counts.
    all_roots: Vec<(String, String)>,
}

/// One source's worth of persist work handed to the catalog thread.
struct PersistBatch {
    root: String,
    name: String,
    /// `import_source.blacklist` JSON for the one-time config adoption (DATA-MODEL §13).
    excludes_json: Option<String>,
    records: Vec<CrawlRecord>,
}

/// Result of a background package extraction.
struct ExtractDone {
    display_name: String,
    /// Identifies which `PendingPackage` this came from — matched by path rather than index
    /// since `pending_packages` may have been mutated while the extraction ran.
    abs_path: PathBuf,
    source_idx: usize,
    files: Vec<CrawledFile>,
    /// The package's manifest locators, carried through for `package_entry` persist.
    entries: Vec<PackageEntryRecord>,
    /// True for a surgical (picked-assets) extraction: the pending card stays so more
    /// can be pulled later; false for a full extraction, which retires it.
    partial: bool,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const CARD_LABEL: f32 = 36.0;
const CARD_PAD: f32 = 8.0;
pub const THUMB_MIN: f32 = 64.0;
pub const THUMB_MAX: f32 = 256.0;
/// Floor for the proportional prefetch window (half the viewport each side),
/// so tiny windows still get some lookahead.
const LOOKAHEAD_ROWS_MIN: usize = 2;
/// Floor for the proportional resident window (1.5x the viewport each side).
/// ponytail: window-based eviction, not true LRU — swap for a budgeted LRU if thrashing shows.
const KEEP_ROWS_MIN: usize = 4;

/// Discrete grid-size stops for the segmented selector, all driving `thumb_size` (design §2).
const GRID_STOPS: [(f32, &str); 3] = [(96.0, "Compact"), (128.0, "Cozy"), (176.0, "Large")];

/// Preset assets-per-page choices; anything else is "Custom" (persisted either way).
const PAGE_SIZE_PRESETS: [usize; 3] = [50, 75, 100];

// ── Content filter (⚑ Filters popover) ─────────────────────────────────────────

/// Triangle-count bucket for the Triangles facet (models only). Design §3 "TRIANGLES".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum TriBucket {
    Le1k,
    K1to3,
    K3to6,
    Ge6k,
}

impl TriBucket {
    const ALL: [TriBucket; 4] = [
        TriBucket::Le1k,
        TriBucket::K1to3,
        TriBucket::K3to6,
        TriBucket::Ge6k,
    ];

    fn of(tris: u32) -> TriBucket {
        match tris {
            0..=1000 => TriBucket::Le1k,
            1001..=3000 => TriBucket::K1to3,
            3001..=6000 => TriBucket::K3to6,
            _ => TriBucket::Ge6k,
        }
    }

    fn label(self) -> &'static str {
        match self {
            TriBucket::Le1k => "\u{2264} 1k",
            TriBucket::K1to3 => "1k\u{2013}3k",
            TriBucket::K3to6 => "3k\u{2013}6k",
            TriBucket::Ge6k => "6k +",
        }
    }
}

/// The dedicated content filter (distinct from the sidebar facet nav): multi-select Trait,
/// Triangles, and File-size sets that AND together. Transient (not persisted). Design §3.
///
/// Trait and Triangle data don't exist in the pre-catalog build yet (traits arrive with P4 AI
/// tagging, tri counts with P1 3D metadata), so those sets are currently always empty and the
/// predicate below degenerates to the File-size filter — which is fully backed by `size_bytes`.
#[derive(Default)]
struct ContentFilter {
    traits: std::collections::BTreeSet<String>,
    tri_buckets: std::collections::BTreeSet<TriBucket>,
    sizes: std::collections::BTreeSet<SizeTier>,
    /// Minimum star rating (None = no constraint). DATA-MODEL §12.
    min_rating: Option<u8>,
    trait_search: String,
}

impl ContentFilter {
    fn active_count(&self) -> usize {
        self.traits.len()
            + self.tri_buckets.len()
            + self.sizes.len()
            + usize::from(self.min_rating.is_some())
    }

    fn any_active(&self) -> bool {
        self.active_count() > 0
    }

    /// Empty every filter set (the popover's "Clear all", and part of the Library nav reset).
    fn clear(&mut self) {
        self.traits.clear();
        self.tri_buckets.clear();
        self.sizes.clear();
        self.min_rating = None;
        self.trait_search.clear();
    }

    /// Pure predicate: does an asset pass the filter? An asset passes if it matches the selected
    /// traits (any) AND selected tri-buckets (any) AND selected size tiers (any); an empty section
    /// imposes no constraint. Assets lacking a tri count are excluded when any tri-bucket is
    /// selected (design §3). Kept pure over primitives so it's unit-testable without an
    /// `AssetEntry` (TECH-STACK.md §5.1).
    fn passes(
        &self,
        size: u64,
        tri: Option<u32>,
        rating: Option<u8>,
        traits: &[String],
        tiers: &SizeTiers,
    ) -> bool {
        if let Some(min) = self.min_rating {
            if rating.unwrap_or(0) < min {
                return false;
            }
        }
        if !self.traits.is_empty() && !traits.iter().any(|t| self.traits.contains(t)) {
            return false;
        }
        if !self.tri_buckets.is_empty() {
            match tri {
                Some(n) => {
                    if !self.tri_buckets.contains(&TriBucket::of(n)) {
                        return false;
                    }
                }
                None => return false,
            }
        }
        if !self.sizes.is_empty() && !self.sizes.contains(&tiers.tier_of(size)) {
            return false;
        }
        true
    }
}

// ── Top-level screen ────────────────────────────────────────────────────────────

/// Which screen the sidebar has selected (UI-SPEC §0.2). Screens without a build yet
/// render the §12 stub — the nav row is never hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Screen {
    #[default]
    Library,
    Identify,
    Collections,
    Packages,
    Unity,
    Vocabulary,
    Stats,
    Duplicates,
    Trash,
    Settings,
}

impl Screen {
    /// Sidebar navigation order (UI-SPEC §0.2 — 10 destinations).
    const NAV: [Screen; 10] = [
        Screen::Library,
        Screen::Identify,
        Screen::Collections,
        Screen::Packages,
        Screen::Unity,
        Screen::Vocabulary,
        Screen::Stats,
        Screen::Duplicates,
        Screen::Trash,
        Screen::Settings,
    ];

    fn label(self) -> &'static str {
        match self {
            Screen::Library => "Library",
            Screen::Identify => "Identify",
            Screen::Collections => "Collections",
            Screen::Packages => "Packages",
            Screen::Unity => "Unity Assets",
            // Prototype nav label is "Traits" (the Mapping tab lives inside this screen).
            Screen::Vocabulary => "Traits",
            Screen::Stats => "Stats",
            Screen::Duplicates => "Duplicates",
            Screen::Trash => "Trash",
            Screen::Settings => "Settings",
        }
    }

    /// Unicode stand-ins for the `ls-nav-*` glyphs (glyph catalog §01) until SVG icons load.
    fn glyph(self) -> &'static str {
        match self {
            Screen::Library => "\u{229E}",
            Screen::Identify => "\u{2726}",
            Screen::Collections => "\u{27E1}",
            Screen::Packages => "\u{25A4}",
            // ponytail: Unicode cube stands in for the `ls-unitypackage` SVG until the
            // sidebar renders vector icons (UI-SPEC §0.2).
            Screen::Unity => "\u{2B22}",
            Screen::Vocabulary => "\u{2756}",
            Screen::Stats => "\u{25C9}",
            Screen::Duplicates => "\u{29C9}",
            Screen::Trash => "\u{2298}",
            Screen::Settings => "\u{2699}",
        }
    }

    /// White-stroke SVG nav icon (tinted per row state in the sidebar). Design icons from
    /// `docs/design/icons/`, recolored to white so egui can tint them.
    fn icon(self) -> egui::ImageSource<'static> {
        match self {
            Screen::Library => egui::include_image!("../assets/icons/nav-library.svg"),
            Screen::Identify => egui::include_image!("../assets/icons/nav-identify.svg"),
            Screen::Collections => egui::include_image!("../assets/icons/nav-collections.svg"),
            Screen::Packages => egui::include_image!("../assets/icons/nav-packages.svg"),
            Screen::Unity => egui::include_image!("../assets/icons/unitypackage.svg"),
            Screen::Vocabulary => egui::include_image!("../assets/icons/nav-traits.svg"),
            Screen::Stats => egui::include_image!("../assets/icons/nav-stats.svg"),
            Screen::Duplicates => egui::include_image!("../assets/icons/nav-duplicates.svg"),
            Screen::Trash => egui::include_image!("../assets/icons/nav-trash.svg"),
            Screen::Settings => egui::include_image!("../assets/icons/nav-settings.svg"),
        }
    }

    fn glyph_color(self) -> egui::Color32 {
        match self {
            Screen::Library | Screen::Collections => theme::GOLD,
            Screen::Identify | Screen::Vocabulary => theme::ARCANE_BRIGHT,
            Screen::Duplicates => theme::WARNING,
            Screen::Trash => theme::DANGER,
            // SUCCESS green per the design's `ls-unitypackage` mark (FEATURES.md §11).
            Screen::Unity => theme::SUCCESS,
            Screen::Packages | Screen::Stats | Screen::Settings => theme::TEXT_MUTED,
        }
    }

    /// One sentence of what the destination will be, for the §12 stub screen.
    fn stub_line(self) -> &'static str {
        match self {
            Screen::Library | Screen::Settings => "", // built — never stubbed
            Screen::Identify => {
                "Work through the Unidentified backlog with batch AI + hand tagging."
            }
            Screen::Collections => "Pinned loadouts \u{2014} stash tabs for your best assets.",
            Screen::Packages => "Everything you own, by publisher and pack.",
            Screen::Unity => {
                ".unitypackage libraries \u{2014} indexed in place, surgically extracted on demand."
            }
            Screen::Vocabulary => {
                "Traits and the mapping library \u{2014} your canonical tag vocabulary."
            }
            Screen::Stats => "Library insights \u{2014} counts, sizes, identification coverage.",
            Screen::Duplicates => {
                "Review near-duplicate groups \u{2014} keep, merge, or trash; never auto-delete."
            }
            Screen::Trash => "Removed assets wait here for restore or explicit deletion.",
        }
    }
}

/// Top-bar search mode (UI-SPEC §0.1): Tags = live substring filter (live today),
/// AI = semantic search (stub until the P3/P4 embedding pipeline lands).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum SearchMode {
    #[default]
    Tags,
    Ai,
}

/// STATUS facet single-select (UI-SPEC §0.2). Maps to `status` (DATA-MODEL §8);
/// `discovered|indexed` fold into Unidentified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum StatusFilter {
    #[default]
    All,
    Unidentified,
    Identified,
    Confirmed,
}

impl StatusFilter {
    /// Does an asset's catalog status pass this facet? Pure for testability.
    fn matches(self, status: AssetStatus) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Unidentified => status.is_unidentified(),
            StatusFilter::Identified => status == AssetStatus::Identified,
            StatusFilter::Confirmed => status == AssetStatus::Confirmed,
        }
    }
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct LodestashApp {
    config: AppConfig,
    /// The source directories currently loaded (may be multiple for a multi-source project).
    scan_roots: Vec<PathBuf>,
    /// Name of the open LibraryProject (if any).
    project_name: Option<String>,
    search: String,
    /// Read-only catalog connection for Search-by-Tags (FTS5). Separate from the
    /// single writer (ARCHITECTURE §5) — WAL lets it read committed rows concurrently.
    /// `None` when no catalog exists yet (search falls back to path substring).
    search_catalog: Option<Catalog>,
    /// Asset id → BM25 rank (0 = best) for the current Tags query, or `None` when the
    /// query is empty (show everything). Drives both membership (Search by Tags) and the
    /// Relevance sort. Recomputed only when the query text changes.
    search_ranks: Option<std::collections::HashMap<i64, usize>>,
    /// The query the current `search_ranks` were computed for — the recompute guard.
    search_ranks_key: Option<String>,
    /// The local SigLIP joint image↔text embedder for Search-with-AI (IDENTIFICATION-AND-SEARCH
    /// §5.B), shared by the query side (this thread, text tower) and the persist-thread image
    /// drain (vision tower). `None` when the ONNX Runtime + model artifacts aren't present — AI
    /// search then stays a stub.
    ai_embedder: Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    /// Job-tray id of an in-flight SigLIP download (`Some` while downloading), and the channel
    /// its worker thread reports completion on. On success the UI reloads `ai_embedder` live.
    siglip_dl_job: Option<u64>,
    siglip_dl_rx: Receiver<Result<(), String>>,
    siglip_dl_tx: Sender<Result<(), String>>,
    active_tab: Category,
    sort_by: SortBy,
    sort_asc: bool,
    thumb_size: f32,
    view_mode: ViewMode,
    /// Flat mode: restrict the grid to one source (None = all libraries).
    source_filter: Option<LibraryFilter>,
    /// Folders mode location: None = top level (source tiles); Some = (source, dir rel to root).
    current_dir: Option<(usize, PathBuf)>,
    /// Child folders at `current_dir`: (name, contained-asset count). Rebuilt with the cache.
    folder_tiles: Vec<(String, usize)>,
    /// Bumped on every folder navigation so the grid ScrollArea gets fresh scroll state.
    nav_generation: u64,
    /// Current thumbnail-grid page (0-based; page size lives in `config.view.page_size`).
    /// Clamped to the filtered result on every cache rebuild.
    page: usize,
    /// Last (page, page_size) the grid rendered — a change resets the spawn window
    /// so the new page's thumbnails load.
    last_page_key: (usize, usize),
    /// The per-page selector is on "Custom…" (even while the value matches a preset).
    page_size_custom: bool,
    entries: Vec<AssetEntry>,
    /// Sorted+filtered indices into `entries`. Rebuilt only when `cache_dirty`.
    sorted_cache: Vec<usize>,
    /// Per-tab entry counts, parallel to Category::TABS. Rebuilt on folder load.
    tab_counts: Vec<usize>,
    cache_dirty: bool,
    /// Last computed column count and visible row window; used to skip spawn work when unchanged.
    last_cols: usize,
    last_row_start: usize,
    last_row_end: usize,
    thumb_rx: Receiver<ThumbReady>,
    thumb_tx: Sender<ThumbReady>,
    pending_thumbs: usize,
    scan_rx: Receiver<ScanDone>,
    scan_tx: Sender<ScanDone>,
    /// Bumped per load_paths; a ScanDone with an older generation is discarded.
    scan_generation: u64,
    scanning: bool,
    /// On-disk thumbnail cache dir (None if the OS cache dir is unavailable).
    thumb_cache_dir: Option<PathBuf>,
    /// Index into `sorted_cache` of the lead selection (inspector / Quick Look / anchor
    /// for keyboard nav).
    selected_cache_idx: Option<usize>,
    /// Multi-selection as entry indices (stable across sort changes); pruned to the
    /// visible filter on every cache rebuild (DESIGN-GUIDELINES §7 "Selection").
    selected: std::collections::BTreeSet<usize>,
    /// Shift-range anchor as a `sorted_cache` index.
    select_anchor: Option<usize>,
    /// When true, the Quick Look overlay is open.
    quick_look: bool,
    /// When true, the Sources panel (view/add/remove project sources + app-data locations) is open.
    sources_panel_open: bool,
    /// `.unitypackage` files found while crawling, awaiting an explicit Extract action (PR #8).
    pending_packages: Vec<PendingPackage>,
    extract_rx: Receiver<ExtractDone>,
    extract_tx: Sender<ExtractDone>,
    /// Where extracted package payloads are stored, content-addressed (None if the OS cache dir
    /// is unavailable).
    workspace_dir: Option<PathBuf>,
    /// The ⚑ Filters content filter (trait / triangles / file-size). Transient. Design §3.
    content_filter: ContentFilter,
    /// Top-bar search mode (UI-SPEC §0.1). AI is a stub until P3/P4.
    search_mode: SearchMode,
    /// Session dismissal for the Unidentified nudge banner (UI-SPEC §2.1).
    nudge_dismissed: bool,
    /// Asset Store cache browse state (UI-SPEC §4): validated cache roots and the
    /// packages under them, listed without unpacking. Scanned on first Packages open.
    store_roots: Vec<PathBuf>,
    store_packages: Vec<AssetStorePackage>,
    store_scanned: bool,
    /// Feedback when a user-picked cache folder fails validation.
    store_add_error: Option<String>,
    /// Unity Assets screen: catalog `package` rows (DATA-MODEL §7), loaded on first
    /// open and after a pin toggle. `None` until first read.
    unity_packages: Option<Vec<lodestash_core::catalog::PackageListing>>,
    /// Latest per-status counts read back from the catalog for the open project's
    /// sources (None until the first persist lands). Feeds the health meter + sidebar.
    catalog_counts: Option<StatusCounts>,
    /// A write-behind catalog persist is in flight.
    catalog_persisting: bool,
    catalog_rx: Receiver<CatalogSynced>,
    catalog_tx: Sender<CatalogSynced>,
    hydrate_rx: Receiver<HydrateDone>,
    hydrate_tx: Sender<HydrateDone>,
    /// Quick Look full-detail image viewer (issue #29 item 1).
    image_viewer: ImageViewer,
    viewer_rx: Receiver<ViewerLoaded>,
    viewer_tx: Sender<ViewerLoaded>,
    /// Quick Look 3D orbit viewport (issue #29 item 3) — None if GL context handoff failed.
    viewport3d: Option<Viewport3d>,
    /// Quick Look audio player — None if no audio output device is available.
    audio_player: Option<AudioPlayer>,
    /// Faint background radial gradients (UI-SPEC §0): gold top-right, arcane bottom-left.
    bg_gold: Option<TextureHandle>,
    bg_arcane: Option<TextureHandle>,
    /// Active top-level screen (UI-SPEC §0.2 sidebar).
    screen: Screen,
    /// STATUS facet selection (UI-SPEC §0.2). Pre-catalog every entry is `discovered`,
    /// which folds into Unidentified — so Identified/Confirmed currently match nothing.
    status_filter: StatusFilter,
    /// Index into `scan_roots` of the source whose blacklist panel is expanded in Settings. §5.1.
    expanded_source: Option<usize>,
    /// The in-progress blacklist exclusion text for the expanded source (transient). §5.1.
    blacklist_input: String,
    /// Show the first-run onboarding modal over the (dimmed) library. Design §1; suppressed by
    /// `config.skip_onboarding`.
    onboarding_open: bool,
    /// Job tray + toasts (DESIGN-GUIDELINES §7, UI-SPEC §18).
    jobs: Jobs,
    /// The running scan's tray row, finished/cancelled on ScanDone or generation bump.
    scan_job: Option<u64>,
}

impl LodestashApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::fonts::install(&cc.egui_ctx);
        // SVG/image loaders for the sidebar nav icons (egui_extras).
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);

        let (siglip_dl_tx, siglip_dl_rx) = mpsc::channel();
        let (thumb_tx, thumb_rx) = mpsc::channel();
        let (scan_tx, scan_rx) = mpsc::channel();
        let (extract_tx, extract_rx) = mpsc::channel();
        let (catalog_tx, catalog_rx) = mpsc::channel();
        let (hydrate_tx, hydrate_rx) = mpsc::channel();
        let (viewer_tx, viewer_rx) = mpsc::channel();
        let config = AppConfig::load();
        let sort_by = config.view.sort_by;
        let sort_asc = config.view.sort_asc;
        let thumb_size = config.view.thumb_size.clamp(THUMB_MIN, THUMB_MAX);
        let view_mode = config.view.mode;
        // First-run onboarding shows unless the user opted out (design §1, §5.7). It only actually
        // renders when no library is open, so a restored library naturally suppresses it.
        let show_onboarding = !config.skip_onboarding;

        let mut app = Self {
            scan_roots: Vec::new(),
            project_name: None,
            search: String::new(),
            search_catalog: catalog_db_path().and_then(|p| Catalog::open(&p).ok()),
            search_ranks: None,
            search_ranks_key: None,
            ai_embedder: resolve_embedder(),
            siglip_dl_job: None,
            siglip_dl_rx,
            siglip_dl_tx,
            active_tab: Category::All,
            sort_by,
            sort_asc,
            thumb_size,
            view_mode,
            source_filter: None,
            current_dir: None,
            folder_tiles: Vec::new(),
            nav_generation: 0,
            page: 0,
            last_page_key: (0, 0),
            page_size_custom: !PAGE_SIZE_PRESETS.contains(&config.view.page_size),
            entries: Vec::new(),
            sorted_cache: Vec::new(),
            tab_counts: vec![0; Category::TABS.len()],
            cache_dirty: false,
            last_cols: 0,
            last_row_start: usize::MAX,
            last_row_end: 0,
            thumb_rx,
            thumb_tx,
            pending_thumbs: 0,
            scan_rx,
            scan_tx,
            scan_generation: 0,
            scanning: false,
            thumb_cache_dir: dirs::cache_dir().map(|d| d.join("Lodestash").join("thumbs")),
            selected_cache_idx: None,
            selected: std::collections::BTreeSet::new(),
            select_anchor: None,
            quick_look: false,
            sources_panel_open: false,
            pending_packages: Vec::new(),
            extract_rx,
            extract_tx,
            workspace_dir: dirs::cache_dir().map(|d| d.join("Lodestash").join("workspace")),
            content_filter: ContentFilter::default(),
            search_mode: SearchMode::Tags,
            nudge_dismissed: false,
            store_roots: Vec::new(),
            store_packages: Vec::new(),
            unity_packages: None,
            store_scanned: false,
            store_add_error: None,
            catalog_counts: None,
            catalog_persisting: false,
            catalog_rx,
            catalog_tx,
            hydrate_rx,
            hydrate_tx,
            image_viewer: ImageViewer::default(),
            viewer_rx,
            viewer_tx,
            viewport3d: cc.gl.clone().and_then(Viewport3d::new),
            audio_player: AudioPlayer::new(config.volume),
            bg_gold: Some(cc.egui_ctx.load_texture(
                "bg_radial_gold",
                radial_image(theme::GOLD, 15), // rgba(231,183,101,.06)
                egui::TextureOptions::LINEAR,
            )),
            bg_arcane: Some(cc.egui_ctx.load_texture(
                "bg_radial_arcane",
                radial_image(theme::ARCANE, 13), // rgba(73,214,201,.05)
                egui::TextureOptions::LINEAR,
            )),
            screen: Screen::Library,
            status_filter: StatusFilter::All,
            expanded_source: None,
            blacklist_input: String::new(),
            onboarding_open: show_onboarding,
            jobs: Jobs::default(),
            scan_job: None,
            config,
        };

        // Restore the last open project, or fall back to last_root for old configs.
        if let Some(name) = app.config.last_project_name.clone() {
            if let Some(proj) = app.config.project_by_name(&name) {
                let paths: Vec<PathBuf> = proj.paths.clone();
                let live: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_dir()).collect();
                if !live.is_empty() {
                    app.load_paths(&cc.egui_ctx, live, Some(name));
                    return app;
                }
            }
        }
        if let Some(root) = app.config.last_root.clone() {
            if root.is_dir() {
                app.load_paths(&cc.egui_ctx, vec![root], None);
            }
        }

        app
    }

    // ── Folder / project loading ──────────────────────────────────────────────

    fn open_folder(&mut self, ctx: &egui::Context) {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            let name = self.config.record_open(path.clone());
            self.load_paths(ctx, vec![path], Some(name));
        }
    }

    fn add_source(&mut self, ctx: &egui::Context) {
        if self.project_name.is_none() {
            return;
        }
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.add_source_path(ctx, path);
        }
    }

    /// Add a known folder as a source: joins the open project when one exists, else
    /// opens it as a fresh library (same as "Open folder…" with this path).
    fn add_source_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        match self.project_name.clone() {
            Some(proj_name) => {
                self.config.add_source_to_project(&proj_name, path);
                // Reload all paths for this project from config (now includes the new one).
                let paths: Vec<PathBuf> = self
                    .config
                    .project_by_name(&proj_name)
                    .map(|p| p.paths.clone())
                    .unwrap_or_default();
                let live: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_dir()).collect();
                self.load_paths(ctx, live, Some(proj_name));
            }
            None => {
                let name = self.config.record_open(path.clone());
                self.load_paths(ctx, vec![path], Some(name));
            }
        }
    }

    /// Remove a source from the open project (config reference only — files are never touched)
    /// and reload the remaining sources.
    fn remove_source(&mut self, ctx: &egui::Context, path: &std::path::Path) {
        let Some(proj_name) = self.project_name.clone() else {
            return;
        };
        self.config.remove_source_from_project(&proj_name, path);
        let paths: Vec<PathBuf> = self
            .config
            .project_by_name(&proj_name)
            .map(|p| p.paths.clone())
            .unwrap_or_default();
        let live: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_dir()).collect();
        self.load_paths(ctx, live, Some(proj_name));
    }

    fn open_project(&mut self, ctx: &egui::Context, name: &str) {
        let paths: Vec<PathBuf> = self
            .config
            .project_by_name(name)
            .map(|p| p.paths.clone())
            .unwrap_or_default();
        let live: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_dir()).collect();
        if live.is_empty() {
            return;
        }
        self.config.record_project_open(name);
        self.load_paths(ctx, live, Some(name.to_owned()));
    }

    /// Core loader — clears the grid and crawls all `roots` on a background thread.
    /// Entries arrive via `scan_rx`; a 100k-file tree must not freeze the window.
    fn load_paths(&mut self, ctx: &egui::Context, roots: Vec<PathBuf>, name: Option<String>) {
        // Drop old channel to cancel in-flight thumbnail tasks.
        let (thumb_tx, thumb_rx) = mpsc::channel();
        self.thumb_tx = thumb_tx;
        self.thumb_rx = thumb_rx;
        self.pending_thumbs = 0;
        self.search.clear();
        self.active_tab = Category::All;

        self.entries = Vec::new();
        self.tab_counts = vec![0; Category::TABS.len()];
        self.cache_dirty = true;
        self.last_cols = 0;
        self.last_row_start = usize::MAX;
        self.last_row_end = 0;
        self.scan_roots = roots.clone();
        self.project_name = name;
        self.catalog_counts = None; // stale counts belong to the previous project
        self.selected_cache_idx = None;
        self.selected.clear();
        self.select_anchor = None;
        self.quick_look = false;
        self.source_filter = None;
        self.current_dir = None;
        self.folder_tiles = Vec::new();
        self.nav_generation += 1;
        self.page = 0;
        self.pending_packages = Vec::new();

        // Each source's blacklist (design §5.1) — resolved before the crawl thread so it honours
        // per-source excludes (INGESTION.md §1).
        let excludes: Vec<Vec<lodestash_core::types::Exclude>> = match self.project_name.as_deref()
        {
            Some(proj) => roots
                .iter()
                .map(|r| self.config.source_excludes(proj, r))
                .collect(),
            None => roots.iter().map(|_| Vec::new()).collect(),
        };

        self.scan_generation += 1;
        self.scanning = true;
        // Tray row (indeterminate — the crawl has no cheap total). A superseded scan's
        // row is finished here; its late ScanDone is dropped by the generation guard.
        if let Some(id) = self.scan_job.take() {
            self.jobs.finish(id);
        }
        let scan_label = match self.scan_roots.len() {
            1 => format!("Scanning {}", config::stem_name(&self.scan_roots[0])),
            n => format!("Scanning {n} sources"),
        };
        self.scan_job = Some(self.jobs.start(scan_label, Arc::default(), true));
        let generation = self.scan_generation;
        let tx = self.scan_tx.clone();
        std::thread::spawn(move || {
            let mut entries: Vec<(usize, CrawledFile)> = Vec::new();
            let mut pending_packages = Vec::new();
            for (source_idx, root) in roots.iter().enumerate() {
                for file in crawler::crawl(root, &excludes[source_idx]) {
                    if is_unitypackage(&file.abs_path) {
                        // Don't surface the raw archive; offer an Extract action instead (PR #8).
                        if let Some(pkg) = resolve_pending_package(source_idx, &file) {
                            pending_packages.push(pkg);
                        }
                        continue;
                    }
                    entries.push((source_idx, file));
                }
            }
            let _ = tx.send(ScanDone {
                generation,
                entries,
                pending_packages,
            });
        });

        // Startup hydrate: show the catalog's view of these sources instantly (no
        // disk walk) while the crawl above reconciles in the background. Whichever
        // lands first wins the frame; ScanDone always supersedes.
        if let Some(db_path) = catalog_db_path() {
            let roots: Vec<PathBuf> = self.scan_roots.clone();
            let tx = self.hydrate_tx.clone();
            let generation = self.scan_generation;
            let cache_extracted = self.config.cache_extracted;
            let workspace_dir = self.workspace_dir.clone();
            std::thread::spawn(move || {
                type Hydrated = (
                    Vec<(usize, Vec<HydratedAsset>)>,
                    Vec<(usize, Vec<lodestash_core::catalog::HydratedExtracted>)>,
                );
                let run = || -> Result<Hydrated, String> {
                    let catalog = Catalog::open(&db_path).map_err(|e| e.to_string())?;
                    let mut sources = Vec::new();
                    let mut extracted = Vec::new();
                    for (idx, root) in roots.iter().enumerate() {
                        let sid = catalog
                            .upsert_import_source(
                                &config::stem_name(root),
                                &root.to_string_lossy(),
                                "folder",
                            )
                            .map_err(|e| e.to_string())?;
                        let rows = catalog.entries_for_source(sid).map_err(|e| e.to_string())?;
                        if !rows.is_empty() {
                            sources.push((idx, rows));
                        }
                        // "Cache extracted packs" (UI-SPEC §11.6): rehydrate extracted
                        // entries whose workspace file still exists (retention may have
                        // evicted some — those simply don't come back).
                        if cache_extracted {
                            if let Some(ws) = &workspace_dir {
                                let rows = catalog
                                    .extracted_entries_for_source(sid)
                                    .map_err(|e| e.to_string())?;
                                let live: Vec<_> = rows
                                    .into_iter()
                                    .filter(|r| ws.join(&r.workspace_rel).is_file())
                                    .collect();
                                if !live.is_empty() {
                                    extracted.push((idx, live));
                                }
                            }
                        }
                    }
                    Ok((sources, extracted))
                };
                match run() {
                    Ok((sources, extracted)) if !sources.is_empty() || !extracted.is_empty() => {
                        let _ = tx.send(HydrateDone {
                            generation,
                            sources,
                            extracted,
                        });
                    }
                    Ok(_) => {} // nothing catalogued yet — first open of these sources
                    Err(err) => tracing::warn!(%err, "startup hydrate failed; waiting for crawl"),
                }
            });
        }

        ctx.request_repaint();
    }

    /// Apply a startup hydrate — only while the reconciling crawl is still running
    /// (a landed ScanDone is always fresher) and only for the current generation.
    fn receive_hydrate(&mut self) {
        while let Ok(msg) = self.hydrate_rx.try_recv() {
            if msg.generation != self.scan_generation || !self.scanning {
                continue;
            }
            self.entries = msg
                .sources
                .into_iter()
                .flat_map(|(source_idx, rows)| {
                    let root = self.scan_roots.get(source_idx).cloned().unwrap_or_default();
                    rows.into_iter().filter_map(move |row| {
                        // Legacy rows from before the .meta-sidecar filter existed.
                        if row.rel_path.to_ascii_lowercase().ends_with(".meta") {
                            return None;
                        }
                        let rel_path = PathBuf::from(&row.rel_path);
                        Some(AssetEntry {
                            package: None,
                            file: CrawledFile {
                                abs_path: root.join(&rel_path),
                                rel_path,
                                size_bytes: row.size_bytes.max(0) as u64,
                                mtime: row.mtime,
                                media_type: MediaType::from_code(&row.media_type),
                            },
                            source_idx,
                            asset_id: Some(row.asset_id),
                            status: AssetStatus::from_code(&row.status)?,
                            trashed: row.trashed,
                            rating: row.rating,
                            texture: None,
                            thumb_spawned: false,
                        })
                    })
                })
                .collect();
            if let Some(ws) = self.workspace_dir.clone() {
                for (source_idx, rows) in msg.extracted {
                    for row in rows {
                        let Some(status) = AssetStatus::from_code(&row.asset.status) else {
                            continue;
                        };
                        if row.asset.rel_path.to_ascii_lowercase().ends_with(".meta") {
                            continue; // legacy pre-filter rows
                        }
                        let rel_path = PathBuf::from(&row.asset.rel_path);
                        self.entries.push(AssetEntry {
                            package: Some(row.package_name),
                            file: CrawledFile {
                                abs_path: ws.join(&row.workspace_rel),
                                rel_path,
                                size_bytes: row.asset.size_bytes.max(0) as u64,
                                mtime: row.asset.mtime,
                                media_type: MediaType::from_code(&row.asset.media_type),
                            },
                            source_idx,
                            asset_id: Some(row.asset.asset_id),
                            status,
                            trashed: row.asset.trashed,
                            rating: row.asset.rating,
                            texture: None,
                            thumb_spawned: false,
                        });
                    }
                }
            }
            self.current_dir = self.default_dir();
            self.recompute_tab_counts();
            self.cache_dirty = true;
            self.last_cols = 0;
            self.last_row_start = usize::MAX;
            self.last_row_end = 0;
            tracing::info!(entries = self.entries.len(), "hydrated grid from catalog");
        }
    }

    // ── Per-frame helpers ─────────────────────────────────────────────────────

    fn receive_scan(&mut self) {
        while let Ok(msg) = self.scan_rx.try_recv() {
            if msg.generation != self.scan_generation {
                continue; // stale scan from a superseded load
            }
            self.scanning = false;
            if let Some(id) = self.scan_job.take() {
                self.jobs.finish(id);
            }
            self.jobs.toast(
                format!("Scan complete — {} assets", msg.entries.len()),
                Intent::Success,
            );
            // Carry catalog identity over from the outgoing entries (a hydrate or an
            // earlier read-back) so trashed/identified state doesn't flash back to
            // defaults between the crawl landing and the next persist read-back.
            // Extracted-package entries never appear in a folder crawl — they live in
            // the workspace. Keep them across rescans instead of silently dropping them.
            let (kept, rest): (Vec<AssetEntry>, Vec<AssetEntry>) =
                self.entries.drain(..).partition(|e| e.package.is_some());
            type Carried = (Option<i64>, AssetStatus, bool, Option<u8>);
            let carry: std::collections::HashMap<(usize, PathBuf), Carried> = rest
                .into_iter()
                .map(|e| {
                    (
                        (e.source_idx, e.file.rel_path),
                        (e.asset_id, e.status, e.trashed, e.rating),
                    )
                })
                .collect();
            self.entries = msg
                .entries
                .into_iter()
                .map(|(source_idx, f)| {
                    let (asset_id, status, trashed, rating) = carry
                        .get(&(source_idx, f.rel_path.clone()))
                        .copied()
                        .unwrap_or((None, AssetStatus::Discovered, false, None));
                    AssetEntry {
                        package: None,
                        file: f,
                        source_idx,
                        asset_id,
                        status,
                        trashed,
                        rating,
                        texture: None,
                        thumb_spawned: false,
                    }
                })
                .collect();
            self.entries.extend(kept);
            self.pending_packages = msg.pending_packages;
            // Entries were rebuilt — old entry indices are meaningless.
            self.selected.clear();
            self.select_anchor = None;
            self.current_dir = self.default_dir();
            self.recompute_tab_counts();
            self.cache_dirty = true;
            self.last_cols = 0;
            self.last_row_start = usize::MAX;
            self.last_row_end = 0;
            self.spawn_catalog_persist();
        }
    }

    /// Write-behind persist of the freshly-scanned entries into the SQLite catalog on
    /// a dedicated thread — the only catalog writer (single-writer rule, ARCHITECTURE
    /// §5). The grid keeps rendering from memory; the thread reports per-status counts
    /// back so the health meter and STATUS facet show catalog truth.
    fn spawn_catalog_persist(&mut self) {
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        let project = self
            .project_name
            .as_deref()
            .and_then(|n| self.config.project_by_name(n));
        let batches: Vec<PersistBatch> = self
            .scan_roots
            .iter()
            .enumerate()
            .map(|(i, root)| {
                // On-disk files only: extracted package payloads live in the workspace,
                // not under the source root — spawn_package_persist covers those.
                let records = crawl_records(
                    self.entries
                        .iter()
                        .filter(|e| e.source_idx == i && e.file.abs_path.starts_with(root))
                        .map(|e| &e.file),
                );
                let excludes_json = project
                    .map(|p| p.excludes_for(root))
                    .filter(|ex| !ex.is_empty())
                    .and_then(|ex| serde_json::to_string(ex).ok());
                PersistBatch {
                    root: root.to_string_lossy().into_owned(),
                    name: config::stem_name(root),
                    excludes_json,
                    records,
                }
            })
            .collect();

        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        let embedder = self.ai_embedder.clone();
        self.catalog_persisting = true;
        std::thread::spawn(move || {
            let (counts, assets, catalog) = match persist_to_catalog(&db_path, &batches) {
                Ok((counts, assets, cat)) => (Some(counts), assets, Some(cat)),
                Err(err) => {
                    tracing::warn!(%err, "catalog persist failed; UI stays on in-memory counts");
                    (None, Vec::new(), None)
                }
            };
            // Send even on failure so the in-flight flag always clears.
            let _ = tx.send(CatalogSynced {
                generation,
                counts,
                assets,
            });
            // Then embed the rows `record_crawl` just marked dirty, on this same connection
            // (still the sole writer). Runs *after* the grid gets its identity, so a large
            // first scan doesn't stall the UI on inference. Search-with-AI needs these.
            if let (Some(mut cat), Some(emb)) = (catalog, embedder) {
                drain_image_embeddings(&mut cat, emb.as_ref());
            }
        });
    }

    /// Write-behind persist of an extracted package: its `package` row (publisher and
    /// category taken from the Asset Store cache tree when it lives under one) plus
    /// asset rows carrying `package_id`. Same single-writer + generation rules as
    /// `spawn_catalog_persist`.
    fn spawn_package_persist(
        &mut self,
        source_idx: usize,
        package_path: &std::path::Path,
        files: &[CrawledFile],
        entries: Vec<PackageEntryRecord>,
    ) {
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        let Some(root) = self.scan_roots.get(source_idx) else {
            return;
        };
        let (publisher, category) = match self.store_root_of(package_path) {
            Some(store_root) => {
                let comps: Vec<String> = package_path
                    .strip_prefix(store_root)
                    .ok()
                    .map(|rel| {
                        rel.components()
                            .map(|c| c.as_os_str().to_string_lossy().into_owned())
                            .collect()
                    })
                    .unwrap_or_default();
                // comps: [publisher, category, ..., "Pack.unitypackage"]
                let publisher = (comps.len() >= 2).then(|| comps[0].clone());
                let category = (comps.len() >= 3).then(|| comps[1].clone());
                (publisher, category)
            }
            None => (None, None),
        };
        let job = PackagePersist {
            root: root.to_string_lossy().into_owned(),
            root_name: config::stem_name(root),
            package_path: package_path.to_string_lossy().into_owned(),
            name: package_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            publisher,
            category,
            records: {
                let mut records = crawl_records(files.iter());
                // Locate each extracted file inside the workspace so the catalog can
                // rehydrate it next session (M6 `asset_file.workspace_rel`).
                if let Some(ws) = &self.workspace_dir {
                    for (rec, f) in records.iter_mut().zip(files) {
                        rec.workspace_rel = f
                            .abs_path
                            .strip_prefix(ws)
                            .ok()
                            .map(|r| r.to_string_lossy().replace('\\', "/"));
                    }
                }
                records
            },
            entries,
            all_roots: self
                .scan_roots
                .iter()
                .map(|r| (config::stem_name(r), r.to_string_lossy().into_owned()))
                .collect(),
        };

        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        self.catalog_persisting = true;
        std::thread::spawn(move || {
            let (counts, assets) = match persist_package_to_catalog(&db_path, &job) {
                Ok((counts, assets)) => (Some(counts), assets),
                Err(err) => {
                    tracing::warn!(%err, "package persist failed; UI stays on in-memory counts");
                    (None, Vec::new())
                }
            };
            let _ = tx.send(CatalogSynced {
                generation,
                counts,
                assets,
            });
        });
    }

    /// Fire-and-forget browse-only persist (DATA-MODEL §7): each validated cache root
    /// becomes an `asset_store_library` source and every browsed pack a `package` row
    /// with `asset_count`/`unpacked_at` left NULL.
    fn spawn_store_browse_persist(&self) {
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        if self.store_packages.is_empty() {
            return;
        }
        let batches: Vec<(String, String, Vec<AssetStorePackage>)> = self
            .store_roots
            .iter()
            .map(|root| {
                let pkgs = self
                    .store_packages
                    .iter()
                    .filter(|p| p.path.starts_with(root))
                    .cloned()
                    .collect();
                (
                    config::stem_name(root),
                    root.to_string_lossy().into_owned(),
                    pkgs,
                )
            })
            .collect();
        std::thread::spawn(move || {
            let run = || -> Result<usize, String> {
                let catalog = Catalog::open(&db_path).map_err(|e| e.to_string())?;
                let mut n = 0;
                for (name, root, pkgs) in &batches {
                    let sid = catalog
                        .upsert_import_source(name, root, "asset_store_library")
                        .map_err(|e| e.to_string())?;
                    for pkg in pkgs {
                        catalog
                            .upsert_package(
                                sid,
                                &pkg.path.to_string_lossy(),
                                &pkg.name,
                                (!pkg.publisher.is_empty()).then_some(pkg.publisher.as_str()),
                                (!pkg.category.is_empty()).then_some(pkg.category.as_str()),
                            )
                            .map_err(|e| e.to_string())?;
                        n += 1;
                    }
                }
                Ok(n)
            };
            match run() {
                Ok(n) => tracing::info!(packages = n, "asset-store browse persisted"),
                Err(err) => tracing::warn!(%err, "asset-store browse persist failed"),
            }
        });
    }

    /// Tab-bar counts over the live (non-trashed) entries.
    fn recompute_tab_counts(&mut self) {
        self.tab_counts = Category::TABS
            .iter()
            .map(|&(cat, _)| {
                self.entries
                    .iter()
                    .filter(|e| !e.trashed && cat.matches(e.file.media_type))
                    .count()
            })
            .collect();
    }

    /// Move entries to / restore them from the Trash: optimistic local flip, then the
    /// soft delete on a writer thread (single-writer rule) with fresh project counts
    /// read back. Catalog-only — files on disk are never touched (DATA-MODEL §8).
    fn set_entries_trashed(&mut self, entry_idxs: &[usize], trashed: bool) {
        let mut ids = Vec::new();
        for &i in entry_idxs {
            if let Some(e) = self.entries.get_mut(i) {
                if let (Some(id), true) = (e.asset_id, e.trashed != trashed) {
                    e.trashed = trashed;
                    ids.push(id);
                }
            }
        }
        if ids.is_empty() {
            return;
        }
        if trashed {
            self.selected.retain(|i| !entry_idxs.contains(i));
            if self
                .selected_cache_idx
                .and_then(|ci| self.sorted_cache.get(ci))
                .is_some_and(|e| entry_idxs.contains(e))
            {
                self.selected_cache_idx = None;
                self.quick_look = false;
            }
        }
        self.recompute_tab_counts();
        self.cache_dirty = true;

        let Some(db_path) = catalog_db_path() else {
            return;
        };
        let roots: Vec<(String, String)> = self
            .scan_roots
            .iter()
            .map(|r| (config::stem_name(r), r.to_string_lossy().into_owned()))
            .collect();
        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        self.catalog_persisting = true;
        std::thread::spawn(move || {
            let run = || -> Result<StatusCounts, String> {
                let catalog = Catalog::open(&db_path).map_err(|e| e.to_string())?;
                catalog
                    .set_trashed(&ids, trashed)
                    .map_err(|e| e.to_string())?;
                let mut sids = Vec::with_capacity(roots.len());
                for (name, root) in &roots {
                    sids.push(
                        catalog
                            .upsert_import_source(name, root, "folder")
                            .map_err(|e| e.to_string())?,
                    );
                }
                catalog
                    .status_counts_for_sources(&sids)
                    .map_err(|e| e.to_string())
            };
            let counts = match run() {
                Ok(c) => Some(c),
                Err(err) => {
                    tracing::warn!(%err, "trash update failed; local flag may diverge until next sync");
                    None
                }
            };
            let _ = tx.send(CatalogSynced {
                generation,
                counts,
                assets: Vec::new(),
            });
        });
    }

    /// Set (or clear) the star rating on entries: optimistic local update, then the
    /// write on the single-writer thread. Keys 1-5 toggle: rating an already-equal
    /// selection clears it (design: ratings card, DATA-MODEL §12).
    fn set_entries_rating(&mut self, entry_idxs: &[usize], rating: Option<u8>) {
        let all_equal = !entry_idxs.is_empty()
            && entry_idxs
                .iter()
                .all(|&i| self.entries.get(i).is_some_and(|e| e.rating == rating));
        let rating = if all_equal { None } else { rating };

        let mut ids = Vec::new();
        for &i in entry_idxs {
            if let Some(e) = self.entries.get_mut(i) {
                if let Some(id) = e.asset_id {
                    e.rating = rating;
                    ids.push(id);
                }
            }
        }
        if ids.is_empty() {
            return;
        }
        self.cache_dirty = true; // rating filter/sort may change the view

        let Some(db_path) = catalog_db_path() else {
            return;
        };
        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        self.catalog_persisting = true;
        std::thread::spawn(move || {
            let result = Catalog::open(&db_path)
                .and_then(|c| c.set_rating(&ids, rating))
                .map_err(|e| e.to_string());
            if let Err(err) = result {
                tracing::warn!(%err, "rating update failed; local value may diverge until next sync");
            }
            let _ = tx.send(CatalogSynced {
                generation,
                counts: None,
                assets: Vec::new(),
            });
        });
    }

    /// Keep the viewer in sync with the Quick Look target: (re)load when the overlay
    /// is open on a raster asset it doesn't hold yet. Decode runs off-thread.
    fn ensure_viewer_loaded(&mut self, ctx: &egui::Context) {
        if !self.quick_look {
            // Drop the full-detail pixels + texture as soon as the overlay closes —
            // an 8k source holds ~128 MB across CPU+GPU copies.
            if self.image_viewer.entry_idx.is_some() {
                let nearest = self.image_viewer.nearest;
                self.image_viewer = ImageViewer {
                    nearest,
                    ..Default::default()
                };
            }
            // Same for the 3D viewport's mesh buffers.
            if let Some(vp) = &mut self.viewport3d {
                if vp.entry_idx.is_some() {
                    vp.clear();
                }
            }
            // Stop audio + free its waveform.
            if let Some(ap) = &mut self.audio_player {
                if ap.entry_idx.is_some() {
                    ap.clear();
                }
            }
            return;
        }
        let Some(&entry_idx) = self
            .selected_cache_idx
            .and_then(|ci| self.sorted_cache.get(ci))
        else {
            return;
        };
        let Some(entry) = self.entries.get(entry_idx) else {
            return;
        };
        // 3D branch mirrors the raster branch below.
        if entry.file.media_type == Some(MediaType::Model3d)
            && viewport3d::is_viewable_model(&entry.file.abs_path)
        {
            let path = entry.file.abs_path.clone();
            // Extracted packages place payloads in content-addressed dirs, so an FBX's
            // relative texture paths dangle — hand the loader a basename→file map built
            // from the same package's image entries. Loose files resolve on disk.
            let tex_files = match &entry.package {
                Some(pkg) => {
                    let pkg = pkg.clone();
                    self.entries
                        .iter()
                        .filter(|e| {
                            e.package.as_deref() == Some(pkg.as_str())
                                && matches!(
                                    e.file.media_type,
                                    Some(MediaType::Texture | MediaType::Image | MediaType::Sprite)
                                )
                        })
                        .filter_map(|e| {
                            let base = e
                                .file
                                .rel_path
                                .file_name()?
                                .to_string_lossy()
                                .to_lowercase();
                            Some((base, e.file.abs_path.clone()))
                        })
                        .collect()
                }
                None => std::collections::BTreeMap::new(),
            };
            if let Some(vp) = &mut self.viewport3d {
                if vp.entry_idx != Some(entry_idx) {
                    vp.load(entry_idx, path, tex_files, ctx);
                }
            }
            return;
        }
        // Audio branch — decode the waveform on a worker thread, like the image branch.
        if entry.file.media_type == Some(MediaType::Audio) {
            let path = entry.file.abs_path.clone();
            if let Some(ap) = &mut self.audio_player {
                if ap.entry_idx != Some(entry_idx) {
                    ap.load(entry_idx, path, ctx);
                }
            }
            return;
        }
        if !is_viewer_image(entry.file.media_type) || self.image_viewer.entry_idx == Some(entry_idx)
        {
            return;
        }
        self.image_viewer.reset_for(entry_idx);
        let path = entry.file.abs_path.clone();
        let tx = self.viewer_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = load_viewer_image(&path);
            let _ = tx.send(ViewerLoaded { entry_idx, result });
            ctx.request_repaint();
        });
    }

    fn receive_viewer(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.viewer_rx.try_recv() {
            if self.image_viewer.entry_idx != Some(msg.entry_idx) {
                continue; // stale load for a superseded target
            }
            self.image_viewer.loading = false;
            match msg.result {
                Ok((image, dims)) => {
                    self.image_viewer.texture = Some(ctx.load_texture(
                        "viewer_image",
                        image.clone(),
                        self.image_viewer.texture_options(),
                    ));
                    self.image_viewer.image = Some(image);
                    self.image_viewer.source_dims = dims;
                }
                Err(err) => self.image_viewer.error = Some(err),
            }
        }
    }

    fn receive_catalog(&mut self) {
        while let Ok(msg) = self.catalog_rx.try_recv() {
            self.catalog_persisting = false;
            if msg.generation != self.scan_generation {
                continue;
            }
            // Package rows may have changed (extraction persisted new ones) — let the
            // Unity Assets screen lazily reload its listing next open.
            self.unity_packages = None;
            match msg.counts {
                Some(counts) => self.catalog_counts = Some(counts),
                // The persist thread already logged the error; surface it honestly.
                None => self.jobs.toast(
                    "Catalog sync failed — changes not persisted",
                    Intent::Danger,
                ),
            }
            if msg.assets.is_empty() {
                continue;
            }
            // Attach catalog identity (asset id + status) to the in-memory entries,
            // joined by source root + rel_path — the grid's read path.
            let by_root: std::collections::HashMap<&str, _> = msg
                .assets
                .iter()
                .map(|(root, map)| (root.as_str(), map))
                .collect();
            let roots: Vec<String> = self
                .scan_roots
                .iter()
                .map(|r| r.to_string_lossy().into_owned())
                .collect();
            for entry in &mut self.entries {
                let Some(map) = roots
                    .get(entry.source_idx)
                    .and_then(|r| by_root.get(r.as_str()))
                else {
                    continue;
                };
                let rel = entry.file.rel_path.to_string_lossy().replace('\\', "/");
                if let Some(&(id, status, trashed, rating)) = map.get(&rel) {
                    entry.asset_id = Some(id);
                    entry.status = status;
                    entry.trashed = trashed;
                    entry.rating = rating;
                }
            }
            // Statuses/trash flags may have changed what the grid and tabs show.
            self.recompute_tab_counts();
            self.cache_dirty = true;
        }
    }

    /// Kick off a background extraction of `pending_packages[idx]` into the workspace dir.
    fn extract_package(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(workspace_dir) = self.workspace_dir.clone() else {
            return;
        };
        let Some(pkg) = self.pending_packages.get_mut(idx) else {
            return;
        };
        if pkg.extracting {
            return;
        }
        pkg.extracting = true;
        pkg.progress = Arc::default();
        let progress = pkg.progress.clone();
        let job_label = format!("Extracting {}", pkg.display_name);
        let abs_path = pkg.abs_path.clone();
        let source_idx = pkg.source_idx;
        let display_name = pkg.display_name.clone();
        let nested_pathname = pkg
            .nested_candidates
            .get(pkg.chosen_variant)
            .map(|(_, pathname)| pathname.clone());
        let entries = pkg.entries.clone();
        pkg.job = Some(self.jobs.start(job_label, progress.clone(), true));
        let tx = self.extract_tx.clone();

        std::thread::spawn(move || {
            let files = extract_package_contents(
                &abs_path,
                &workspace_dir,
                nested_pathname.as_deref(),
                &display_name,
                &progress,
            );
            let _ = tx.send(ExtractDone {
                display_name,
                abs_path,
                source_idx,
                files,
                entries,
                partial: false,
            });
        });
        ctx.request_repaint();
    }

    /// Surgical extraction of the picked entries of `pending_packages[idx]` — one
    /// skip-scan pulling only those payloads (INGESTION.md §2), never the whole pack.
    fn extract_picked(&mut self, ctx: &egui::Context, idx: usize) {
        let Some(workspace_dir) = self.workspace_dir.clone() else {
            return;
        };
        let Some(pkg) = self.pending_packages.get_mut(idx) else {
            return;
        };
        if pkg.extracting || pkg.picked.is_empty() {
            return;
        }
        pkg.extracting = true;

        let locators: Vec<unitypackage::EntryLocator> = pkg
            .picked
            .iter()
            .filter_map(|&i| {
                let e = pkg.entries.get(i)?;
                Some(unitypackage::EntryLocator {
                    guid: e.guid.clone(),
                    pathname: e.pathname.clone(),
                    // `extractable()` gates picking, so these are always Some.
                    offset: e.tar_offset? as u64,
                    size: e.size_bytes? as u64,
                })
            })
            .collect();
        let abs_path = pkg.abs_path.clone();
        let source_idx = pkg.source_idx;
        let display_name = pkg.display_name.clone();
        let entries = pkg.entries.clone();
        // Indeterminate row: the skip-scan reports no byte progress, and it can't stop
        // mid-payload — not cancellable.
        pkg.job = Some(self.jobs.start(
            format!(
                "Extracting {} assets from {}",
                locators.len(),
                pkg.display_name
            ),
            Arc::default(),
            false,
        ));
        let tx = self.extract_tx.clone();

        std::thread::spawn(move || {
            let files = match unitypackage::extract_at_offsets(&abs_path, &workspace_dir, locators)
            {
                Ok(assets) => extracted_to_crawled(assets, &display_name),
                Err(err) => {
                    tracing::warn!(%err, package = %display_name, "surgical extraction failed");
                    Vec::new()
                }
            };
            let _ = tx.send(ExtractDone {
                display_name,
                abs_path,
                source_idx,
                files,
                entries,
                partial: true,
            });
        });
        ctx.request_repaint();
    }

    fn receive_extracts(&mut self) {
        let mut done = Vec::new();
        while let Ok(msg) = self.extract_rx.try_recv() {
            done.push(msg);
        }
        for msg in done {
            let mut cancelled = false;
            if let Some(pkg) = self
                .pending_packages
                .iter_mut()
                .find(|p| p.abs_path == msg.abs_path)
            {
                cancelled = pkg.progress.is_cancelled();
                if let Some(id) = pkg.job.take() {
                    self.jobs.finish(id);
                }
            }
            if cancelled {
                self.jobs.toast(
                    format!("Extraction cancelled — {}", msg.display_name),
                    Intent::Info,
                );
            } else if msg.files.is_empty() {
                self.jobs.toast(
                    format!("Extraction produced no assets — {}", msg.display_name),
                    Intent::Warning,
                );
            } else {
                self.jobs.toast(
                    format!(
                        "Extracted {} assets from {}",
                        msg.files.len(),
                        msg.display_name
                    ),
                    Intent::Success,
                );
            }
            if msg.partial || cancelled {
                // Surgical pull (or an aborted full extract): the card stays armed.
                if let Some(pkg) = self
                    .pending_packages
                    .iter_mut()
                    .find(|p| p.abs_path == msg.abs_path)
                {
                    pkg.extracting = false;
                    pkg.picked.clear();
                }
            } else {
                self.pending_packages.retain(|p| p.abs_path != msg.abs_path);
            }
            // Write-behind: package row + extracted asset rows (DATA-MODEL §7).
            self.spawn_package_persist(msg.source_idx, &msg.abs_path, &msg.files, msg.entries);
            for file in msg.files {
                self.entries.push(AssetEntry {
                    package: Some(msg.display_name.clone()),
                    file,
                    source_idx: msg.source_idx,
                    asset_id: None,
                    status: AssetStatus::Discovered,
                    trashed: false,
                    rating: None,
                    texture: None,
                    thumb_spawned: false,
                });
            }
            self.recompute_tab_counts();
            self.cache_dirty = true;
        }
    }

    /// Bottom job tray (DESIGN-GUIDELINES §7 "Jobs / progress"): one row per running
    /// job — gold progress bar (or spinner when the total is unknown), ✕ where
    /// cooperative cancel is wired, a brief ✓ flash when done.
    fn jobs_tray(&mut self, ctx: &egui::Context) {
        if self.jobs.jobs.is_empty() {
            return;
        }
        egui::TopBottomPanel::bottom("jobs_tray")
            .frame(
                egui::Frame::none()
                    .fill(theme::SURFACE)
                    .inner_margin(egui::Margin::symmetric(12.0, 5.0))
                    .stroke(egui::Stroke::new(1.0, theme::BORDER)),
            )
            .show(ctx, |ui| {
                for job in &self.jobs.jobs {
                    ui.horizontal(|ui| {
                        if job.finished.is_some() {
                            ui.label(RichText::new("\u{2713}").color(theme::SUCCESS).size(11.0));
                            ui.label(RichText::new(&job.label).color(theme::TEXT_DIM).size(11.0));
                            return;
                        }
                        let total = job.progress.total.load(Ordering::Relaxed);
                        if total == 0 {
                            ui.add(egui::Spinner::new().size(12.0));
                        } else {
                            ui.add(
                                egui::ProgressBar::new(job.progress.fraction())
                                    .desired_width(120.0)
                                    .desired_height(10.0)
                                    .fill(theme::GOLD),
                            );
                        }
                        ui.label(
                            RichText::new(&job.label)
                                .color(theme::TEXT_MUTED)
                                .size(11.0),
                        );
                        if job.progress.is_cancelled() {
                            ui.label(
                                RichText::new("cancelling\u{2026}")
                                    .color(theme::TEXT_DIM)
                                    .size(10.0),
                            );
                        } else if job.cancellable
                            && ui
                                .add(
                                    egui::Button::new(RichText::new("\u{2715}").size(10.0)).small(),
                                )
                                .on_hover_text("Cancel")
                                .clicked()
                        {
                            job.progress.cancelled.store(true, Ordering::Relaxed);
                        }
                    });
                }
            });
    }

    /// Bottom-right toast stack (UI-SPEC §18): overlay fill, 3px intent accent, ~3.2s.
    fn toasts_overlay(&self, ctx: &egui::Context) {
        if self.jobs.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-16.0, -16.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                for toast in &self.jobs.toasts {
                    let accent = match toast.intent {
                        Intent::Success => theme::SUCCESS,
                        Intent::Warning => theme::WARNING,
                        Intent::Danger => theme::DANGER,
                        Intent::Info => theme::TYPE_IMAGE,
                    };
                    let resp = egui::Frame::none()
                        .fill(theme::OVERLAY)
                        .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG))
                        .rounding(4.0)
                        .inner_margin(egui::Margin {
                            left: 12.0,
                            right: 10.0,
                            top: 6.0,
                            bottom: 6.0,
                        })
                        .show(ui, |ui| {
                            ui.label(RichText::new(&toast.text).color(theme::TEXT).size(11.5));
                        })
                        .response;
                    // The 3px intent accent along the frame's left edge.
                    let r = resp.rect;
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(r.min, egui::pos2(r.min.x + 3.0, r.max.y)),
                        egui::Rounding {
                            nw: 4.0,
                            sw: 4.0,
                            ..Default::default()
                        },
                        accent,
                    );
                    ui.add_space(6.0);
                }
            });
    }

    fn receive_thumbnails(&mut self) {
        while let Ok(msg) = self.thumb_rx.try_recv() {
            self.pending_thumbs = self.pending_thumbs.saturating_sub(1);
            if let (Some(tex), Some(entry)) = (msg.texture, self.entries.get_mut(msg.index)) {
                entry.texture = Some(tex);
            }
        }
    }

    /// Rebuild the sorted+filtered index cache. Called only when `cache_dirty`.
    /// Selection is preserved by entry, not by grid slot — a filter change must not
    /// silently move the selection onto a different asset.
    /// Recompute the FTS5 ranks for the current Tags query (Search by Tags, Surface A).
    /// Guarded by the query text so it runs once per change, not once per rebuild. Falls
    /// back to `None` (path-substring only) when there's no catalog or the query is empty.
    /// The map is `asset_id → BM25 rank` (0 = best): membership for filtering, order for
    /// the Relevance sort.
    /// Search-with-AI (IDENTIFICATION-AND-SEARCH §5.B): embed the query locally, fuse BM25 ⊕
    /// vector KNN via `hybrid_search`, and drop the ranked result ids into `search_ranks` — the
    /// same map Tags search uses, so the grid filter/sort just works. No-op (leaves the stub)
    /// when no embedder is loaded. Runs inline: a query is one short forward pass (~ms on CPU).
    fn run_ai_search(&mut self) {
        const HIT_CAP: usize = 100_000;
        let query = self.search.trim().to_string();
        let Some(emb) = self.ai_embedder.clone() else {
            return; // no model → AI stays stubbed
        };
        if query.is_empty() {
            self.search_ranks = None;
            self.search_ranks_key = None;
            self.cache_dirty = true;
            return;
        }
        if self.search_catalog.is_none() {
            self.search_catalog = catalog_db_path().and_then(|p| Catalog::open(&p).ok());
        }
        let Some(cat) = &self.search_catalog else {
            return;
        };
        let qv = emb.embed_text(&query);
        // Bucket pre-filter is left to the grid's tab.matches (a Category spans several
        // media_types; hybrid_search takes a single one) — pass None here.
        let ids = cat
            .hybrid_search(&query, Some(&qv), emb.model_id(), None, HIT_CAP)
            .unwrap_or_default();
        self.search_ranks = Some(
            ids.into_iter()
                .enumerate()
                .map(|(rank, id)| (id, rank))
                .collect(),
        );
        self.search_ranks_key = Some(format!("\0ai:{query}"));
        self.sort_by = SortBy::Relevance; // fused order is the point of an AI search
        self.cache_dirty = true;
    }

    /// Kick off the SigLIP model download on a worker thread, tracked by a cancellable job-tray
    /// row. Idempotent-ish: no-op if one is already running.
    fn start_siglip_download(&mut self) {
        if self.siglip_dl_job.is_some() {
            return;
        }
        let progress = Arc::new(JobProgress::default());
        let id = self
            .jobs
            .start("Downloading SigLIP model (~412 MB)".into(), progress.clone(), true);
        self.siglip_dl_job = Some(id);
        let tx = self.siglip_dl_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(models::download_siglip(progress));
        });
    }

    /// Finish the download job when the worker reports back: on success reload the embedder live
    /// (Search-with-AI lights up without a restart); otherwise toast the reason. A tray ✕ sets
    /// `progress.cancelled`, which surfaces here as an `Err("cancelled")`.
    fn receive_model_download(&mut self) {
        while let Ok(res) = self.siglip_dl_rx.try_recv() {
            if let Some(id) = self.siglip_dl_job.take() {
                self.jobs.finish(id);
            }
            match res {
                Ok(()) => {
                    self.ai_embedder = resolve_embedder();
                    if self.ai_embedder.is_some() {
                        self.jobs
                            .toast("Vision model ready — Search with AI is live", Intent::Success);
                    } else {
                        // Files landed but the model won't load — almost always the ONNX Runtime.
                        self.jobs.toast(
                            "Model downloaded, but it won't load — check onnxruntime.dll (ORT 1.22)",
                            Intent::Danger,
                        );
                    }
                }
                Err(e) if e == "cancelled" => {
                    self.jobs.toast("Model download cancelled", Intent::Info)
                }
                Err(e) => self.jobs.toast(format!("Model download failed: {e}"), Intent::Danger),
            }
        }
    }

    fn refresh_search_ranks(&mut self) {
        // AI ranks are set explicitly by run_ai_search (on Enter), not recomputed per rebuild —
        // leave them untouched here so a rebuild doesn't wipe the last AI result set.
        if self.search_mode == SearchMode::Ai {
            return;
        }
        // Ranked retrieval with a high cap so a broad query keeps every match (the grid
        // re-scopes by source/tab/folder afterwards).
        const HIT_CAP: usize = 100_000;
        let key = match self.search_mode {
            SearchMode::Tags if !self.search.trim().is_empty() => Some(self.search.to_lowercase()),
            _ => None,
        };
        if key == self.search_ranks_key {
            return;
        }
        self.search_ranks_key = key.clone();
        // Lazily open the read connection: on a fresh install the catalog doesn't exist
        // at startup, but the first scan creates it — pick it up on the first query rather
        // than needing a restart.
        if key.is_some() && self.search_catalog.is_none() {
            self.search_catalog = catalog_db_path().and_then(|p| Catalog::open(&p).ok());
        }
        self.search_ranks = match (&key, &self.search_catalog) {
            (Some(q), Some(cat)) => cat.search_assets(q, HIT_CAP).ok().flatten().map(|ids| {
                ids.into_iter()
                    .enumerate()
                    .map(|(rank, id)| (id, rank))
                    .collect()
            }),
            _ => None,
        };
    }

    fn rebuild_cache(&mut self) {
        self.refresh_search_ranks();
        let selected_entry = self
            .selected_cache_idx
            .and_then(|ci| self.sorted_cache.get(ci).copied());
        // AI-mode queries never live-filter — results only replace the grid on Enter,
        // which is stubbed until the P3/P4 semantic pipeline (UI-SPEC §0.1).
        let q = match self.search_mode {
            SearchMode::Tags => self.search.to_lowercase(),
            SearchMode::Ai => String::new(),
        };
        let tab = self.active_tab;

        // Search by Tags (IDENTIFICATION-AND-SEARCH §5.A): catalogued entries match through
        // the FTS5 index (name/tags/description, prefix-tokenised); path substring is kept as
        // a union so uncatalogued entries (pre-persist / just-extracted) and mid-word matches
        // still work — nothing that matched before stops matching. The BM25 order feeds the
        // Relevance sort (sort_indices).
        let ranks = self.search_ranks.as_ref();
        // AI mode with a result set: the fused ids ARE the result — match iff the asset is in
        // it (no path-substring union; the semantic set is exact). Until Enter runs (ranks
        // None) nothing filters, so the grid is unchanged.
        let ai_active = self.search_mode == SearchMode::Ai && ranks.is_some();
        let name_matches = |e: &AssetEntry| {
            if ai_active {
                return e
                    .asset_id
                    .is_some_and(|id| ranks.unwrap().contains_key(&id));
            }
            if q.is_empty() {
                return true;
            }
            if let (Some(ranks), Some(id)) = (ranks, e.asset_id) {
                if ranks.contains_key(&id) {
                    return true;
                }
            }
            e.file
                .rel_path
                .to_string_lossy()
                .to_lowercase()
                .contains(&q)
        };

        let (mut idxs, tiles): (Vec<usize>, Vec<(String, usize)>) =
            match (self.view_mode, self.current_dir.clone()) {
                (ViewMode::Flat, _) => {
                    let filter = self.source_filter.clone();
                    let idxs = self
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| {
                            filter.as_ref().map_or(true, |f| match f {
                                LibraryFilter::Source(s) => e.source_idx == *s,
                                LibraryFilter::Package(p) => e.package.as_deref() == Some(p),
                            }) && tab.matches(e.file.media_type)
                                && name_matches(e)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    (idxs, Vec::new())
                }
                // Folders, top level: one tile per source; searching searches everything.
                (ViewMode::Folders, None) => {
                    if q.is_empty() && !ai_active {
                        let tiles = self
                            .scan_roots
                            .iter()
                            .enumerate()
                            .map(|(i, root)| {
                                let count = self
                                    .entries
                                    .iter()
                                    .filter(|e| {
                                        e.source_idx == i
                                            && !e.trashed
                                            && tab.matches(e.file.media_type)
                                    })
                                    .count();
                                (config::stem_name(root), count)
                            })
                            .collect();
                        (Vec::new(), tiles)
                    } else {
                        let idxs = self
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| tab.matches(e.file.media_type) && name_matches(e))
                            .map(|(i, _)| i)
                            .collect();
                        (idxs, Vec::new())
                    }
                }
                // Folders, inside a source: direct children + subfolder tiles;
                // searching searches the current subtree.
                (ViewMode::Folders, Some((src, dir))) => {
                    if q.is_empty() && !ai_active {
                        folder_children(&self.entries, src, &dir, tab)
                    } else {
                        let idxs = self
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| {
                                e.source_idx == src
                                    && e.file.rel_path.starts_with(&dir)
                                    && tab.matches(e.file.media_type)
                                    && name_matches(e)
                            })
                            .map(|(i, _)| i)
                            .collect();
                        (idxs, Vec::new())
                    }
                }
            };
        self.folder_tiles = tiles;

        // Content filter (⚑ Filters) composes on top of facet + search (design §3). Applied once
        // over the resolved asset indices; folder-tile counts are intentionally left unfiltered.
        if self.content_filter.any_active() {
            let cf = &self.content_filter;
            let tiers = &self.config.size_tiers;
            let entries = &self.entries;
            // No trait/tri data pre-catalog → None/&[]; wire real values when P1/P4 land.
            idxs.retain(|&i| {
                cf.passes(
                    entries[i].file.size_bytes,
                    None,
                    entries[i].rating,
                    &[],
                    tiers,
                )
            });
        }

        // Trashed assets leave the grid everywhere; they live on the Trash screen
        // (DATA-MODEL §8 — catalog-only, files untouched).
        {
            let entries = &self.entries;
            idxs.retain(|&i| !entries[i].trashed);
        }

        // STATUS facet (UI-SPEC §0.2): a real per-asset match now that entries carry
        // their catalog status (read back after each persist).
        if self.status_filter != StatusFilter::All {
            let sf = self.status_filter;
            let entries = &self.entries;
            idxs.retain(|&i| sf.matches(entries[i].status));
        }

        sort_indices(
            &mut idxs,
            &self.entries,
            self.sort_by,
            self.sort_asc,
            self.search_ranks.as_ref(),
        );

        self.sorted_cache = idxs;
        self.selected_cache_idx =
            selected_entry.and_then(|e| self.sorted_cache.iter().position(|&i| i == e));
        // Multi-selection follows the filter: hidden entries drop out (Ctrl-A is
        // "all within the current filter", design §7), and the range anchor resets.
        if !self.selected.is_empty() {
            let visible: std::collections::HashSet<usize> =
                self.sorted_cache.iter().copied().collect();
            self.selected.retain(|e| visible.contains(e));
        }
        self.select_anchor = self.selected_cache_idx;
        // Keep the page inside the (possibly shrunken) filtered result.
        self.page = self.page.min(self.page_count().saturating_sub(1));
        self.cache_dirty = false;
    }

    /// Number of grid pages for the current filtered result (always ≥ 1).
    fn page_count(&self) -> usize {
        self.sorted_cache
            .len()
            .div_ceil(self.config.view.page_size.max(1))
            .max(1)
    }

    /// The `sorted_cache` index range shown on the current page.
    fn page_range(&self) -> std::ops::Range<usize> {
        let size = self.config.view.page_size.max(1);
        let start = (self.page * size).min(self.sorted_cache.len());
        let end = (start + size).min(self.sorted_cache.len());
        start..end
    }

    /// Drop thumbnail textures far outside the visible window so VRAM stays bounded.
    /// In-flight loads (spawned, no texture yet) are left alone — their send must still
    /// decrement `pending_thumbs`; the texture lands and is evicted on a later pass.
    fn evict_far_thumbnails(&mut self, cols: usize, row_start: usize, row_end: usize) {
        // Rows are relative to the current page slice; entries on other pages fall
        // outside the keep set and evict too (a page flip frees the old page).
        let page = self.page_range();
        let (keep_start, keep_end) = keep_range(row_start, row_end, cols, page.len());
        let selected_entry = self
            .selected_cache_idx
            .and_then(|ci| self.sorted_cache.get(ci).copied());

        let keep: std::collections::HashSet<usize> = self.sorted_cache[page.clone()]
            [keep_start..keep_end]
            .iter()
            .copied()
            .collect();
        for (idx, entry) in self.entries.iter_mut().enumerate() {
            if entry.texture.is_some() && !keep.contains(&idx) && Some(idx) != selected_entry {
                entry.texture = None;
                entry.thumb_spawned = false;
            }
        }
    }

    fn save_view_prefs(&mut self) {
        self.config.view.sort_by = self.sort_by;
        self.config.view.sort_asc = self.sort_asc;
        self.config.view.thumb_size = self.thumb_size;
        self.config.view.mode = self.view_mode;
        self.config.save();
    }

    // ── Navigation rail (design §5, glyph catalog §01) ─────────────────────────

    /// The Library nav glyph: return to the Library screen and **reset to All libraries, clearing
    /// every active filter** — source filter, category tab, search, and the content-filter sets
    /// (design §4). Distinct from picking a specific source in the Library facet, which preserves
    /// the current filter.
    fn go_to_library(&mut self) {
        self.screen = Screen::Library;
        self.page = 0;
        self.status_filter = StatusFilter::All;
        self.source_filter = None;
        self.active_tab = Category::All;
        self.search.clear();
        self.content_filter.clear();
        self.current_dir = self.default_dir();
        self.selected_cache_idx = None;
        self.nav_generation += 1;
        self.cache_dirty = true;
    }

    /// The labeled sidebar (UI-SPEC §0.2): 9 nav destinations + STATUS and LIBRARIES
    /// facet groups. Replaces the earlier icon-only nav rail.
    fn sidebar(&mut self, ctx: &egui::Context) {
        let mut go_library = false;
        let mut new_screen: Option<Screen> = None;
        let mut new_status: Option<StatusFilter> = None;
        let mut new_source: Option<Option<LibraryFilter>> = None;
        let mut add_source_clicked = false;

        let trashed_count = self.entries.iter().filter(|e| e.trashed).count();
        let total = self.entries.len() - trashed_count;
        // STATUS rows show catalog truth when the persist has landed ("All" stays the
        // grid count — unclassified files render in the grid but aren't catalog rows).
        let (unidentified, identified_n, confirmed_n) = match &self.catalog_counts {
            Some(c) => (c.unidentified, c.identified, c.confirmed),
            None => (total, 0, 0),
        };
        let per_source: Vec<usize> = (0..self.scan_roots.len())
            .map(|i| self.entries.iter().filter(|e| e.source_idx == i).count())
            .collect();
        // Facet groups collapse to their labels on screens where they don't apply (§0.2).
        let facets_open = matches!(self.screen, Screen::Library | Screen::Identify);

        egui::SidePanel::left("sidebar")
            .exact_width(210.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .inner_margin(egui::Margin::symmetric(8.0, 10.0)),
            )
            .show(ctx, |ui| {
                ScrollArea::vertical().show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for screen in Screen::NAV {
                        let badge = match screen {
                            Screen::Identify => unidentified,
                            Screen::Trash => trashed_count,
                            // Dup-group counts need P2 dedup; 0 hides the pill.
                            _ => 0,
                        };
                        if sidebar_nav_row(
                            ui,
                            screen.icon(),
                            screen.glyph_color(),
                            screen.label(),
                            (badge > 0).then_some(badge),
                            self.screen == screen,
                        )
                        .clicked()
                        {
                            if screen == Screen::Library {
                                go_library = true;
                            } else {
                                new_screen = Some(screen);
                            }
                        }
                    }

                    sidebar_divider(ui);
                    facet_group_label(ui, "STATUS");
                    if facets_open {
                        for (sf, label, color, count) in [
                            (StatusFilter::All, "All", theme::TEXT_MUTED, total),
                            (
                                StatusFilter::Unidentified,
                                "Unidentified",
                                theme::STATUS_UNIDENTIFIED,
                                unidentified,
                            ),
                            (
                                StatusFilter::Identified,
                                "Identified",
                                theme::STATUS_IDENTIFIED,
                                identified_n,
                            ),
                            (
                                StatusFilter::Confirmed,
                                "Confirmed",
                                theme::STATUS_CONFIRMED,
                                confirmed_n,
                            ),
                        ] {
                            if facet_row(ui, Some(color), label, count, self.status_filter == sf)
                                .clicked()
                            {
                                new_status = Some(sf);
                            }
                        }
                    }

                    sidebar_divider(ui);
                    facet_group_label(ui, "LIBRARIES");
                    if facets_open {
                        if facet_row(
                            ui,
                            None,
                            "All libraries",
                            total,
                            self.source_filter.is_none(),
                        )
                        .clicked()
                        {
                            new_source = Some(None);
                        }
                        for (i, &count) in per_source.iter().enumerate() {
                            let name = config::stem_name(&self.scan_roots[i]);
                            let active = self.source_filter == Some(LibraryFilter::Source(i));
                            if facet_row(ui, None, &name, count, active).clicked() {
                                new_source = Some(Some(LibraryFilter::Source(i)));
                            }
                        }
                        // Extracted packages as their own library rows (session view of the
                        // P2 packages model).
                        let mut per_package: std::collections::BTreeMap<&str, usize> =
                            std::collections::BTreeMap::new();
                        for e in &self.entries {
                            if let Some(pkg) = e.package.as_deref() {
                                *per_package.entry(pkg).or_default() += 1;
                            }
                        }
                        let mut pick_package: Option<String> = None;
                        for (pkg, count) in per_package {
                            let active = matches!(
                                &self.source_filter,
                                Some(LibraryFilter::Package(p)) if p == pkg
                            );
                            let label = format!("\u{25A4} {pkg}");
                            if facet_row(ui, None, &label, count, active).clicked() {
                                pick_package = Some(pkg.to_owned());
                            }
                        }
                        if let Some(pkg) = pick_package {
                            new_source = Some(Some(LibraryFilter::Package(pkg)));
                        }
                        ui.add_space(4.0);
                        // ponytail: solid 1px border stands in for the spec's dashed one —
                        // egui strokes can't dash.
                        if self.project_name.is_some()
                            && ui
                                .add_sized(
                                    [ui.available_width(), 28.0],
                                    egui::Button::new(
                                        RichText::new("+ Add source\u{2026}")
                                            .color(theme::ARCANE)
                                            .size(12.0),
                                    )
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG)),
                                )
                                .clicked()
                        {
                            add_source_clicked = true;
                        }
                    }
                });
            });

        if go_library {
            self.go_to_library();
        }
        if let Some(s) = new_screen {
            self.screen = s;
            self.selected_cache_idx = None;
            self.quick_look = false;
        }
        if let Some(sf) = new_status {
            self.status_filter = sf;
            self.screen = Screen::Library;
            self.cache_dirty = true;
        }
        if let Some(src) = new_source {
            self.source_filter = src;
            self.screen = Screen::Library;
            self.cache_dirty = true;
        }
        if add_source_clicked {
            self.add_source(ctx);
        }
    }

    /// Drop every resident thumbnail texture so the grid re-generates them (e.g. after the
    /// on-screen quality changed). In-flight loads finish and are replaced next pass.
    fn invalidate_thumbnails(&mut self) {
        for e in &mut self.entries {
            e.texture = None;
            e.thumb_spawned = false;
        }
        self.last_cols = 0;
        self.last_row_start = usize::MAX;
        self.last_row_end = 0;
    }

    // ── Packages screen (UI-SPEC §4) ────────────────────────────────────────────

    /// Discover + validate Asset Store cache roots (`ASSETSTORE_CACHE_PATH` → per-OS
    /// default → user-added, INGESTION.md §2) and list their packages without unpacking.
    fn refresh_store_cache(&mut self) {
        let mut roots = asset_store::candidate_roots();
        for custom in &self.config.asset_store_roots {
            if !roots.contains(custom) {
                roots.push(custom.clone());
            }
        }
        self.store_roots = roots
            .into_iter()
            .filter(|r| asset_store::is_asset_store_cache(r))
            .collect();
        // ponytail: synchronous walk — the cache tree is metadata-only and modest;
        // move to a worker thread if a huge cache ever makes this stall.
        self.store_packages = self
            .store_roots
            .iter()
            .flat_map(|r| asset_store::browse(r))
            .collect();
        self.store_scanned = true;
        // Browse-only package rows (DATA-MODEL §7) — asset_count/unpacked_at stay NULL.
        self.spawn_store_browse_persist();
    }

    /// Which validated cache root a package lives under.
    fn store_root_of(&self, path: &std::path::Path) -> Option<&PathBuf> {
        self.store_roots.iter().find(|r| path.starts_with(r))
    }

    /// UI-SPEC §4: everything you own, publisher → pack, browsed straight from the
    /// download cache. Extraction goes through the existing pending-package flow, so a
    /// cache must be a library source (crawled) before its packs can be pulled in.
    fn packages_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        if !self.store_scanned {
            self.refresh_store_cache();
        }

        ui.label(
            RichText::new("Packages")
                .color(theme::TEXT)
                .font(crate::fonts::display(24.0)),
        );
        ui.label(
            RichText::new(
                "Everything you own, by publisher \u{2192} pack \u{2014} browsed straight \
                 from the Asset Store download cache. Nothing unpacks until you say so.",
            )
            .color(theme::TEXT_MUTED)
            .size(12.0),
        );
        ui.add_space(8.0);

        let mut add_clicked = false;
        let mut refresh_clicked = false;
        let mut source_to_add: Option<PathBuf> = None;
        let mut extract_idx: Option<usize> = None;

        ui.horizontal(|ui| {
            if ui
                .button(RichText::new("+ Add cache folder\u{2026}").size(11.0))
                .clicked()
            {
                add_clicked = true;
            }
            if ui
                .button(RichText::new("\u{27F3} Refresh").size(11.0))
                .clicked()
            {
                refresh_clicked = true;
            }
            if let Some(err) = &self.store_add_error {
                ui.label(RichText::new(err).color(theme::DANGER).size(11.0));
            }
        });
        for root in self.store_roots.clone() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(root.to_string_lossy())
                        .color(theme::TEXT_DIM)
                        .size(10.0)
                        .monospace(),
                );
                if self.scan_roots.contains(&root) {
                    ui.label(
                        RichText::new("\u{2713} library source")
                            .color(theme::SUCCESS)
                            .size(10.0),
                    );
                } else if ui
                    .button(
                        RichText::new("Use as library source")
                            .color(theme::GOLD)
                            .size(10.0),
                    )
                    .on_hover_text(
                        "Crawl this cache so its packages can be extracted into the library",
                    )
                    .clicked()
                {
                    source_to_add = Some(root.clone());
                }
            });
        }

        if self.store_roots.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("\u{25A4}").size(48.0).color(theme::TEXT_DIM));
                ui.label(
                    RichText::new("No Asset Store cache found")
                        .color(theme::TEXT)
                        .size(14.0),
                );
                let hint = asset_store::default_root()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "<no default on this platform>".into());
                ui.label(
                    RichText::new(format!(
                        "Looked at ASSETSTORE_CACHE_PATH and {hint} \u{2014} \
                         use \u{201C}+ Add cache folder\u{201D} if yours lives elsewhere."
                    ))
                    .color(theme::TEXT_DIM)
                    .size(11.0),
                );
            });
        } else {
            ui.add_space(4.0);
            ui.separator();
            ScrollArea::vertical().show(ui, |ui| {
                for run in publisher_runs(&self.store_packages) {
                    let pkgs = &self.store_packages[run];
                    let total: u64 = pkgs.iter().map(|p| p.size_bytes).sum();
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("\u{25A4}").color(theme::GOLD).size(13.0));
                        ui.label(
                            RichText::new(&pkgs[0].publisher)
                                .color(theme::TEXT)
                                .size(13.0)
                                .strong(),
                        );
                        ui.label(
                            RichText::new(format!(
                                "{} packs \u{B7} {}",
                                pkgs.len(),
                                format_bytes(total)
                            ))
                            .color(theme::TEXT_DIM)
                            .size(11.0)
                            .monospace(),
                        );
                    });
                    for pkg in pkgs {
                        ui.horizontal(|ui| {
                            ui.add_space(22.0);
                            ui.label(RichText::new(&pkg.name).color(theme::TEXT_MUTED).size(12.0));
                            if !pkg.category.is_empty() {
                                ui.label(
                                    RichText::new(&pkg.category)
                                        .color(theme::TEXT_DIM)
                                        .size(10.0),
                                );
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        RichText::new(format_bytes(pkg.size_bytes))
                                            .color(theme::TEXT_DIM)
                                            .size(10.0)
                                            .monospace(),
                                    );
                                    let pending = self
                                        .pending_packages
                                        .iter()
                                        .position(|p| p.abs_path == pkg.path);
                                    let root_is_source = self
                                        .store_root_of(&pkg.path)
                                        .is_some_and(|r| self.scan_roots.contains(r));
                                    match pending {
                                        Some(idx) if self.pending_packages[idx].extracting => {
                                            let frac =
                                                self.pending_packages[idx].progress.fraction();
                                            ui.add(
                                                egui::ProgressBar::new(frac)
                                                    .desired_width(120.0)
                                                    .desired_height(12.0)
                                                    .fill(theme::ARCANE)
                                                    .text(
                                                        RichText::new(format!(
                                                            "extracting {:.0}%",
                                                            frac * 100.0
                                                        ))
                                                        .size(9.0),
                                                    ),
                                            );
                                        }
                                        Some(idx) => {
                                            if ui
                                                .button(
                                                    RichText::new("Extract")
                                                        .color(theme::GOLD)
                                                        .size(11.0),
                                                )
                                                .clicked()
                                            {
                                                extract_idx = Some(idx);
                                            }
                                        }
                                        None if root_is_source => {
                                            ui.label(
                                                RichText::new(if self.scanning {
                                                    "scanning\u{2026}"
                                                } else {
                                                    "\u{2713} extracted"
                                                })
                                                .color(theme::TEXT_DIM)
                                                .size(10.0),
                                            );
                                        }
                                        None => {
                                            ui.label(
                                                RichText::new("needs the cache as a source")
                                                    .color(theme::TEXT_DIM)
                                                    .size(10.0),
                                            )
                                            .on_hover_text(
                                                "Click \u{201C}Use as library source\u{201D} \
                                                 above, then Extract",
                                            );
                                        }
                                    }
                                },
                            );
                        });
                    }
                }
            });
        }

        if add_clicked {
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                if asset_store::is_asset_store_cache(&path) {
                    self.store_add_error = None;
                    if !self.config.asset_store_roots.contains(&path) {
                        self.config.asset_store_roots.push(path);
                        self.config.save();
                    }
                    self.refresh_store_cache();
                } else {
                    self.store_add_error = Some(format!(
                        "\u{2298} {} has no .unitypackage files \u{2014} not an Asset Store cache",
                        path.display()
                    ));
                }
            }
        }
        if refresh_clicked {
            self.refresh_store_cache();
        }
        if let Some(root) = source_to_add {
            self.add_source_path(ctx, root);
        }
        if let Some(idx) = extract_idx {
            self.extract_package(ctx, idx);
        }
    }

    // ── Unity Assets screen (UI-SPEC §0.2, FEATURES.md §5) ──────────────────────

    /// The `.unitypackage` library manager: catalog `package` rows, each pinnable into
    /// the sidebar LIBRARIES facet (`package.pinned`, M7). Distinct from Packages (§4,
    /// the aggregate indexed-asset view). ponytail: this first slice is the listing +
    /// pin/unpin; the in-screen browse tree + per-asset preview (FEATURES §5.4) reuse
    /// the pending-packages bar's extraction for now and land as a follow-on.
    fn unity_screen(&mut self, ui: &mut Ui, _ctx: &egui::Context) {
        if self.unity_packages.is_none() {
            self.reload_unity_packages();
        }

        ui.horizontal(|ui| {
            ui.label(
                RichText::new("Unity Assets")
                    .color(theme::TEXT)
                    .font(crate::fonts::display(24.0)),
            );
            if ui
                .button(RichText::new("\u{27F3} Refresh").size(11.0))
                .clicked()
            {
                self.reload_unity_packages();
            }
        });
        ui.label(
            RichText::new(
                "Your .unitypackage libraries \u{2014} indexed in place, surgically extracted on \
                 demand. Pin a package to keep it as a sidebar library.",
            )
            .color(theme::TEXT_MUTED)
            .size(12.0),
        );
        ui.add_space(8.0);

        let packages = self.unity_packages.clone().unwrap_or_default();
        if packages.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("\u{2B22}").size(48.0).color(theme::TEXT_DIM));
                ui.label(
                    RichText::new("No packages indexed yet")
                        .color(theme::TEXT)
                        .size(14.0),
                );
                ui.label(
                    RichText::new(
                        "Extract a .unitypackage (Packages screen, or drop one in a source \
                         folder) and it shows up here.",
                    )
                    .color(theme::TEXT_DIM)
                    .size(11.0),
                );
            });
            return;
        }

        let mut toggle: Option<(i64, bool)> = None;
        ScrollArea::vertical().show(ui, |ui| {
            for pkg in &packages {
                egui::Frame::none()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .rounding(4.0)
                    .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("\u{2B22}").color(theme::SUCCESS).size(14.0));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&pkg.name).color(theme::TEXT).size(13.0));
                                let sub = match (&pkg.publisher, &pkg.category) {
                                    (Some(p), Some(c)) => format!("{p} \u{2022} {c}"),
                                    (Some(p), None) => p.clone(),
                                    _ => String::new(),
                                };
                                if !sub.is_empty() {
                                    ui.label(RichText::new(sub).color(theme::TEXT_DIM).size(10.0));
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let pinned = pkg.pinned;
                                    let (label, color) = if pinned {
                                        ("\u{2605} In Libraries \u{00B7} remove", theme::GOLD)
                                    } else {
                                        ("\u{2606} Add to Library", theme::TEXT_MUTED)
                                    };
                                    if ui
                                        .button(RichText::new(label).color(color).size(11.0))
                                        .clicked()
                                    {
                                        toggle = Some((pkg.id, !pinned));
                                    }
                                    if let Some(n) = pkg.asset_count {
                                        ui.label(
                                            RichText::new(format!("{n} assets"))
                                                .color(theme::TEXT_DIM)
                                                .size(10.0),
                                        );
                                    } else {
                                        ui.label(
                                            RichText::new("browsed \u{00B7} not extracted")
                                                .color(theme::TEXT_DIM)
                                                .size(10.0),
                                        );
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(4.0);
            }
        });

        if let Some((id, pin)) = toggle {
            self.set_package_pinned(id, pin);
        }
    }

    /// Read the catalog's `package` rows into `unity_packages` (cheap single SELECT).
    fn reload_unity_packages(&mut self) {
        let Some(db_path) = catalog_db_path() else {
            self.unity_packages = Some(Vec::new());
            return;
        };
        match Catalog::open(&db_path).and_then(|c| c.list_packages()) {
            Ok(list) => self.unity_packages = Some(list),
            Err(err) => {
                tracing::warn!(%err, "unity package listing failed");
                self.unity_packages = Some(Vec::new());
                self.jobs
                    .toast("Couldn't read packages from the catalog", Intent::Danger);
            }
        }
    }

    /// Pin/unpin a package (M7). Optimistic in-memory flip + a detached write (the pin
    /// is a single-row UPDATE; `busy_timeout` covers any overlap with a persist).
    fn set_package_pinned(&mut self, package_id: i64, pinned: bool) {
        if let Some(list) = &mut self.unity_packages {
            if let Some(p) = list.iter_mut().find(|p| p.id == package_id) {
                p.pinned = pinned;
                self.jobs.toast(
                    if pinned {
                        format!("Pinned {} to Libraries", p.name)
                    } else {
                        format!("Removed {} from Libraries", p.name)
                    },
                    Intent::Success,
                );
            }
        }
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        std::thread::spawn(move || {
            if let Err(err) = Catalog::open(&db_path).and_then(|c| {
                c.set_package_pinned(package_id, pinned)?;
                Ok(())
            }) {
                tracing::warn!(%err, package_id, "package pin write failed");
            }
        });
    }

    // ── Trash screen (UI-SPEC §7) ───────────────────────────────────────────────

    /// Catalog-only soft delete: zebra table rows with Restore. Files stay on disk;
    /// explicit delete-from-disk is a separate danger flow (not on this screen).
    fn trash_screen(&mut self, ui: &mut Ui) {
        ui.label(
            RichText::new("Trash")
                .color(theme::TEXT)
                .font(crate::fonts::display(24.0)),
        );
        ui.label(
            RichText::new(
                "Removed from the catalog only \u{2014} files stay on disk. Restore any time.",
            )
            .color(theme::TEXT_MUTED)
            .size(12.0),
        );
        ui.add_space(10.0);

        let trashed: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.trashed)
            .map(|(i, _)| i)
            .collect();
        if trashed.is_empty() {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("\u{2298}").size(48.0).color(theme::TEXT_DIM));
                ui.label(
                    RichText::new("Trash is empty.")
                        .color(theme::TEXT_MUTED)
                        .size(13.0),
                );
            });
            return;
        }

        let mut restore: Option<usize> = None;
        let many_sources = self.scan_roots.len() > 1;
        ScrollArea::vertical().show(ui, |ui| {
            for (row, &i) in trashed.iter().enumerate() {
                let entry = &self.entries[i];
                let bg = if row % 2 == 0 {
                    theme::SURFACE
                } else {
                    egui::Color32::TRANSPARENT
                };
                egui::Frame::none()
                    .fill(bg)
                    .inner_margin(egui::Margin::symmetric(8.0, 5.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let color = entry
                                .file
                                .media_type
                                .map(theme::media_type_color)
                                .unwrap_or(theme::TEXT_DIM);
                            ui.label(RichText::new("\u{25C6}").color(color).size(9.0));
                            let name = entry
                                .file
                                .rel_path
                                .file_stem()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            ui.add(
                                egui::Label::new(
                                    RichText::new(name).color(theme::TEXT_MUTED).size(12.0),
                                )
                                .truncate(),
                            );
                            let mut meta = Vec::new();
                            if many_sources {
                                meta.push(config::stem_name(&self.scan_roots[entry.source_idx]));
                            }
                            meta.push(format_bytes(entry.file.size_bytes));
                            ui.label(
                                RichText::new(meta.join(" \u{B7} "))
                                    .color(theme::TEXT_DIM)
                                    .size(10.0)
                                    .monospace(),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .button(
                                            RichText::new("\u{21A9} Restore")
                                                .color(theme::SUCCESS)
                                                .size(11.0),
                                        )
                                        .clicked()
                                    {
                                        restore = Some(i);
                                    }
                                },
                            );
                        });
                    });
            }
        });
        if let Some(i) = restore {
            self.set_entries_trashed(&[i], false);
        }
    }

    // ── Settings screen (design §5) ────────────────────────────────────────────

    fn settings_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut save = false;
        let mut quality_changed = false;
        let mut start_dl = false;
        // Blacklist actions collected during render, applied after (avoids borrowing self while
        // iterating scan_roots). Each is (source_idx, exclude).
        let mut toggle_expand: Option<usize> = None;
        let mut add_exclude: Option<(usize, lodestash_core::types::Exclude)> = None;
        let mut remove_exclude: Option<(usize, lodestash_core::types::Exclude)> = None;

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let side = ((ui.available_width() - 760.0) / 2.0).max(0.0);
                ui.horizontal(|ui| {
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_max_width(760.0);
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new("Settings")
                                .size(20.0)
                                .color(theme::TEXT)
                                .strong(),
                        );
                        ui.add_space(14.0);

                        // 5.1 Sources (Library) + per-source blacklist (§5.1).
                        settings_card(ui, "LIBRARY \u{00B7} SOURCES", |ui| {
                            if self.scan_roots.is_empty() {
                                ui.label(
                                    RichText::new("No sources yet \u{2014} open a folder to start.")
                                        .size(12.0)
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            for (i, root) in self.scan_roots.iter().enumerate() {
                                let expanded = self.expanded_source == Some(i);
                                let excludes = self
                                    .project_name
                                    .as_deref()
                                    .map(|p| self.config.source_excludes(p, root))
                                    .unwrap_or_default();
                                ui.horizontal(|ui| {
                                    let (dot, p) = ui
                                        .allocate_painter(Vec2::splat(10.0), egui::Sense::hover());
                                    p.circle_filled(dot.rect.center(), 4.0, theme::GOLD);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(config::stem_name(root))
                                                .size(13.0)
                                                .color(theme::TEXT),
                                        );
                                        ui.label(
                                            RichText::new(root.to_string_lossy())
                                                .size(10.0)
                                                .color(theme::TEXT_DIM)
                                                .monospace(),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            // Expander glyph (⌄ closed / ⌃ open) + count.
                                            let glyph = if expanded { "\u{2303}" } else { "\u{2304}" };
                                            let n = excludes.len();
                                            let btn = ui.add(
                                                egui::Button::new(
                                                    RichText::new(if n > 0 {
                                                        format!("{glyph}  {n} excluded")
                                                    } else {
                                                        format!("{glyph}  Exclusions")
                                                    })
                                                    .size(11.0)
                                                    .color(theme::TEXT_MUTED),
                                                )
                                                .frame(false),
                                            );
                                            if btn.clicked() {
                                                toggle_expand = Some(i);
                                            }
                                        },
                                    );
                                });

                                if expanded {
                                    // Blacklist panel — inset well.
                                    egui::Frame::none()
                                        .fill(theme::BG)
                                        .rounding(6.0)
                                        .inner_margin(egui::Margin::same(10.0))
                                        .show(ui, |ui| {
                                            if excludes.is_empty() {
                                                ui.label(
                                                    RichText::new(
                                                        "Nothing excluded \u{2014} everything \
                                                         under this source is indexed.",
                                                    )
                                                    .size(11.0)
                                                    .color(theme::TEXT_DIM),
                                                );
                                            }
                                            ui.horizontal_wrapped(|ui| {
                                                for ex in &excludes {
                                                    let color = match ex.kind {
                                                        config::ExcludeKind::Folder => theme::WARNING,
                                                        config::ExcludeKind::Asset => theme::DANGER,
                                                    };
                                                    if exclusion_chip(ui, &ex.value, color) {
                                                        remove_exclude = Some((i, ex.clone()));
                                                    }
                                                }
                                            });
                                            ui.add_space(6.0);
                                            ui.horizontal(|ui| {
                                                ui.add(
                                                    egui::TextEdit::singleline(
                                                        &mut self.blacklist_input,
                                                    )
                                                    .hint_text("folder or asset\u{2026}")
                                                    .desired_width(180.0),
                                                );
                                                let val = self.blacklist_input.trim().to_string();
                                                let enabled = !val.is_empty();
                                                if ui
                                                    .add_enabled(
                                                        enabled,
                                                        egui::Button::new(
                                                            RichText::new("\u{2298} folder")
                                                                .color(theme::WARNING)
                                                                .size(11.0),
                                                        ),
                                                    )
                                                    .clicked()
                                                {
                                                    add_exclude = Some((
                                                        i,
                                                        lodestash_core::types::Exclude {
                                                            kind: config::ExcludeKind::Folder,
                                                            value: val.clone(),
                                                        },
                                                    ));
                                                }
                                                if ui
                                                    .add_enabled(
                                                        enabled,
                                                        egui::Button::new(
                                                            RichText::new("\u{2298} asset")
                                                                .color(theme::DANGER)
                                                                .size(11.0),
                                                        ),
                                                    )
                                                    .clicked()
                                                {
                                                    add_exclude = Some((
                                                        i,
                                                        lodestash_core::types::Exclude {
                                                            kind: config::ExcludeKind::Asset,
                                                            value: val,
                                                        },
                                                    ));
                                                }
                                            });
                                        });
                                }
                                ui.add_space(4.0);
                            }
                            ui.add_space(2.0);
                            ui.label(
                                RichText::new(
                                    "Excluding only skips cataloguing \u{2014} files on disk are \
                                     never touched.",
                                )
                                .size(10.0)
                                .color(theme::TEXT_DIM),
                            );
                        });

                        // 5.2 Appearance — on-screen (grid) quality.
                        settings_card(ui, "APPEARANCE \u{00B7} ON-SCREEN QUALITY", |ui| {
                            ui.label(
                                RichText::new(
                                    "Grid thumbnail sharpness (a GPU/perf tradeoff). Quick Look \
                                     always renders full detail.",
                                )
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                            );
                            ui.add_space(6.0);
                            if segmented(
                                ui,
                                &mut self.config.preview_quality,
                                config::PreviewQuality::OPTIONS,
                            ) {
                                save = true;
                                quality_changed = true;
                            }
                        });

                        // 5.3 File-size labels.
                        settings_card(ui, "FILE-SIZE LABELS", |ui| {
                            ui.label(
                                RichText::new(
                                    "Thresholds behind the S / M / L / XL badge and the File-size \
                                     filter.",
                                )
                                .size(11.0)
                                .color(theme::TEXT_MUTED),
                            );
                            ui.add_space(8.0);
                            if size_tiers_editor(ui, &mut self.config.size_tiers) {
                                save = true;
                            }
                        });

                        // 5.7 First-run onboarding (Engine group in the design).
                        settings_card(ui, "FIRST-RUN", |ui| {
                            let mut show = !self.config.skip_onboarding;
                            if setting_toggle(
                                ui,
                                "Show the first-run setup screen",
                                "Off = open straight into the library on launch.",
                                &mut show,
                            ) {
                                self.config.skip_onboarding = !show;
                                save = true;
                            }
                        });

                        // Scaffold cards — controls shown; backends not built yet.
                        // AI · SEARCH MODEL — the SigLIP download that powers Search-with-AI.
                        let ai_loaded = self.ai_embedder.is_some();
                        let ai_downloading = self.siglip_dl_job.is_some();
                        let ai_files = models::siglip_installed();
                        settings_card(ui, "AI \u{00B7} SEARCH MODEL", |ui| {
                            if ai_loaded {
                                ui.label(
                                    RichText::new(
                                        "\u{2713} SigLIP 2 base loaded \u{2014} visual Search with AI is active.",
                                    )
                                    .size(12.0)
                                    .color(theme::SUCCESS),
                                );
                            } else if ai_downloading {
                                ui.label(
                                    RichText::new("Downloading\u{2026} see the job tray below.")
                                        .size(12.0)
                                        .color(theme::TEXT_DIM),
                                );
                            } else if ai_files {
                                ui.label(
                                    RichText::new(
                                        "Model files present but not loaded \u{2014} check the ONNX \
                                         Runtime (onnxruntime.dll, ORT 1.22) beside the app.",
                                    )
                                    .size(12.0)
                                    .color(theme::WARNING),
                                );
                            } else {
                                ui.label(
                                    RichText::new(
                                        "Describe an asset in plain language and find the ones that \
                                         look like it. Downloads SigLIP 2 base (~412 MB) for fully \
                                         offline, CPU-friendly visual search.",
                                    )
                                    .size(11.0)
                                    .color(theme::TEXT_DIM),
                                );
                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new("\u{2B73} Download model (~412 MB)")
                                                .size(12.0)
                                                .color(theme::ARCANE),
                                        )
                                        .stroke(egui::Stroke::new(1.0, theme::ARCANE))
                                        .rounding(6.0),
                                    )
                                    .clicked()
                                {
                                    start_dl = true;
                                }
                            }
                        });
                        settings_card(ui, "AI \u{00B7} IDENTIFY PROFILE", |ui| {
                            ui.label(
                                RichText::new(
                                    "Integrated / Balanced / Dedicated GPU \u{2014} model set + \
                                     preview quality. Planned (AI-MODELS-BY-GPU.md); no local AI \
                                     engine yet.",
                                )
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                            );
                        });
                        settings_card(ui, "MCP", |ui| {
                            ui.label(
                                RichText::new(
                                    "Enable the Lodestash MCP server (identify / tag / search) for \
                                     your agents. Planned; wires to the existing MCP plane \
                                     (MCP-AND-ENGINES.md \u{00A7}2), not a new one.",
                                )
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                            );
                        });
                        settings_card(ui, "STORAGE CAPS", |ui| {
                            let mut cache = self.config.cache_extracted;
                            if ui
                                .checkbox(
                                    &mut cache,
                                    RichText::new("Keep extracted packs cached").size(12.0),
                                )
                                .on_hover_text(
                                    "Extracted packages reload instantly next session from the \
                                     workspace instead of re-extracting. Uses workspace disk \
                                     (retention still applies). Off: extractions are session-only.",
                                )
                                .changed()
                            {
                                self.config.cache_extracted = cache;
                                save = true;
                            }
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "Preview-cache limit + auto-purge. Planned (DESIGN-GUIDELINES.md \
                                     \u{00A7}7 Storage).",
                                )
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                            );
                        });
                        settings_card(ui, "ENGINE BRIDGE", |ui| {
                            ui.label(
                                RichText::new(
                                    "Live-sync to the active Unity project + confirm-before-Trash. \
                                     Planned (ENGINE-API.md).",
                                )
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                            );
                        });
                    });
                });
            });

        if save {
            self.config.save();
            self.cache_dirty = true;
        }
        if quality_changed {
            self.invalidate_thumbnails();
        }
        if start_dl {
            self.start_siglip_download();
        }

        // Apply blacklist actions (collected during render to avoid borrowing self mid-iteration).
        if let Some(i) = toggle_expand {
            self.expanded_source = if self.expanded_source == Some(i) {
                None
            } else {
                self.blacklist_input.clear();
                Some(i)
            };
        }
        let proj = self.project_name.clone();
        let mut blacklist_changed = false;
        if let (Some(proj), Some((i, ex))) = (proj.as_deref(), add_exclude) {
            if let Some(root) = self.scan_roots.get(i).cloned() {
                if self.config.add_source_exclude(proj, &root, ex) {
                    self.blacklist_input.clear();
                    blacklist_changed = true;
                }
            }
        }
        if let (Some(proj), Some((i, ex))) = (proj.as_deref(), remove_exclude) {
            if let Some(root) = self.scan_roots.get(i).cloned() {
                if self.config.remove_source_exclude(proj, &root, &ex) {
                    blacklist_changed = true;
                }
            }
        }
        // A changed blacklist re-indexes the current library so the exclusion takes effect.
        if blacklist_changed {
            self.load_paths(ctx, self.scan_roots.clone(), self.project_name.clone());
        }
    }

    // ── Folder navigation (Folders view) ──────────────────────────────────────

    /// Where Folders view starts: single-source projects descend into the root
    /// directly; multi-source projects start at the source-tile top level.
    fn default_dir(&self) -> Option<(usize, PathBuf)> {
        (self.scan_roots.len() == 1).then(|| (0, PathBuf::new()))
    }

    fn nav_to(&mut self, dir: Option<(usize, PathBuf)>) {
        self.current_dir = dir;
        self.cache_dirty = true;
        self.nav_generation += 1;
        self.selected_cache_idx = None;
        self.quick_look = false;
    }

    /// Up one folder level; from a source root to the top level (multi-source only).
    fn nav_up(&mut self) {
        let Some((src, dir)) = self.current_dir.clone() else {
            return;
        };
        if dir.as_os_str().is_empty() {
            if self.scan_roots.len() > 1 {
                self.nav_to(None);
            }
        } else {
            let parent = dir.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            self.nav_to(Some((src, parent)));
        }
    }

    /// Breadcrumb + folder tiles for Folders view; handles navigation clicks.
    fn folders_nav_ui(&mut self, ui: &mut Ui) {
        let multi = self.scan_roots.len() > 1;
        let mut nav: Option<Option<(usize, PathBuf)>> = None;

        let crumb = |ui: &mut Ui, text: &str, active: bool| -> bool {
            let rt = RichText::new(text).size(12.0).color(if active {
                theme::GOLD
            } else {
                theme::TEXT_MUTED
            });
            ui.add(egui::Label::new(rt).sense(egui::Sense::click()))
                .clicked()
        };
        let sep = |ui: &mut Ui| {
            ui.label(RichText::new("/").color(theme::TEXT_DIM).size(12.0));
        };

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if multi && crumb(ui, "All libraries", self.current_dir.is_none()) {
                nav = Some(None);
            }
            if let Some((src, dir)) = self.current_dir.clone() {
                if multi {
                    sep(ui);
                }
                let at_root = dir.as_os_str().is_empty();
                if crumb(ui, &config::stem_name(&self.scan_roots[src]), at_root) && !at_root {
                    nav = Some(Some((src, PathBuf::new())));
                }
                let comps: Vec<String> = dir
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                let mut acc = PathBuf::new();
                for (ci, comp) in comps.iter().enumerate() {
                    acc.push(comp);
                    sep(ui);
                    let last = ci + 1 == comps.len();
                    if crumb(ui, comp, last) && !last {
                        nav = Some(Some((src, acc.clone())));
                    }
                }
            }
        });
        ui.add_space(6.0);

        // Folder tiles (hidden while searching — search shows subtree results flat).
        if !self.folder_tiles.is_empty() && self.search.is_empty() {
            let tiles = self.folder_tiles.clone();
            let mut clicked: Option<usize> = None;
            ScrollArea::vertical()
                .id_salt("folder_tiles")
                .max_height(150.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = Vec2::new(6.0, 6.0);
                        for (i, (name, count)) in tiles.iter().enumerate() {
                            let btn = egui::Button::new(
                                RichText::new(format!("\u{1F4C1} {name}   {count}"))
                                    .color(theme::TEXT)
                                    .size(12.0),
                            )
                            .fill(theme::SURFACE)
                            .stroke(egui::Stroke::new(1.0, theme::BORDER));
                            if ui.add(btn).clicked() {
                                clicked = Some(i);
                            }
                        }
                    });
                });
            ui.add_space(8.0);
            if let Some(i) = clicked {
                // Top level: tile index == source index (built parallel to scan_roots).
                match self.current_dir.clone() {
                    None => self.nav_to(Some((i, PathBuf::new()))),
                    Some((src, dir)) => self.nav_to(Some((src, dir.join(&tiles[i].0)))),
                }
            }
        }
        if let Some(n) = nav {
            self.nav_to(n);
        }
    }

    /// Spawn thumbnail tasks for entry indices that haven't been started yet.
    fn spawn_thumbnails(&mut self, ctx: &egui::Context, to_spawn: &[usize]) {
        for &idx in to_spawn {
            let entry = &self.entries[idx];
            if entry.thumb_spawned || !needs_thumbnail(entry.file.media_type) {
                continue;
            }
            self.entries[idx].thumb_spawned = true;
            self.pending_thumbs += 1;

            let abs_path = self.entries[idx].file.abs_path.clone();
            let rel_path = self.entries[idx]
                .file
                .rel_path
                .to_string_lossy()
                .into_owned();
            let size_bytes = self.entries[idx].file.size_bytes;
            let mtime = self.entries[idx].file.mtime;
            let cache_dir = self.thumb_cache_dir.clone();
            // On-screen quality drives the grid thumbnail render size (design §5.2). Quick Look is
            // unaffected — it decodes the original at full detail.
            let gen_px = self.config.preview_quality.thumb_px();
            let tx = self.thumb_tx.clone();
            let ctx2 = ctx.clone();

            rayon::spawn(move || {
                // Always send (even on failure) so pending_thumbs is always decremented.
                // Do NOT call request_repaint() here — stray repaints from rayon during resize
                // cause DWM jitter. The main thread polls via request_repaint_after() instead.
                let texture = generate_texture(
                    &ctx2,
                    &abs_path,
                    &rel_path,
                    size_bytes,
                    mtime,
                    cache_dir.as_deref(),
                    gen_px,
                );
                let _ = tx.send(ThumbReady {
                    index: idx,
                    texture,
                });
            });
        }
    }
}

fn name_key(e: &AssetEntry) -> String {
    e.file
        .rel_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn type_key(e: &AssetEntry) -> u8 {
    e.file.media_type.map(|m| m as u8).unwrap_or(255)
}

/// Sort `idxs` (indices into `entries`) in place per the view/sort toolbar's `sort_by`/`asc`.
/// Name pre-computes keys once (decorate-sort-undecorate) since `name_key` allocates —
/// worth avoiding at O(n log n) comparisons on 10k+ entry libraries.
fn sort_indices(
    idxs: &mut Vec<usize>,
    entries: &[AssetEntry],
    sort_by: SortBy,
    asc: bool,
    ranks: Option<&std::collections::HashMap<i64, usize>>,
) {
    match sort_by {
        // BM25 match quality (Search by Tags): FTS-ranked entries first in rank order,
        // then the path-substring-only matches (and everything, when no query is active)
        // in name order. Decorate once — `name_key` allocates.
        SortBy::Relevance => {
            let keys: Vec<(Option<usize>, String)> = idxs
                .iter()
                .map(|&i| {
                    let rank = entries[i]
                        .asset_id
                        .and_then(|id| ranks.and_then(|r| r.get(&id).copied()));
                    (rank, name_key(&entries[i]))
                })
                .collect();
            let mut paired: Vec<(usize, usize)> = idxs.iter().copied().zip(0..).collect();
            paired.sort_by(|&(_, ka), &(_, kb)| {
                let (ra, na) = &keys[ka];
                let (rb, nb) = &keys[kb];
                let o = match (ra, rb) {
                    (Some(x), Some(y)) => x.cmp(y).then_with(|| na.cmp(nb)),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => na.cmp(nb),
                };
                if asc {
                    o
                } else {
                    o.reverse()
                }
            });
            *idxs = paired.into_iter().map(|(i, _)| i).collect();
        }
        SortBy::Name => {
            let keys: Vec<String> = idxs.iter().map(|&i| name_key(&entries[i])).collect();
            let mut paired: Vec<(usize, usize)> = idxs.iter().copied().zip(0..).collect();
            paired.sort_by(|&(_, ka), &(_, kb)| {
                let o = keys[ka].cmp(&keys[kb]);
                if asc {
                    o
                } else {
                    o.reverse()
                }
            });
            *idxs = paired.into_iter().map(|(i, _)| i).collect();
        }
        SortBy::Type => idxs.sort_by(|&a, &b| {
            let o = type_key(&entries[a]).cmp(&type_key(&entries[b]));
            if asc {
                o
            } else {
                o.reverse()
            }
        }),
        SortBy::Size => idxs.sort_by(|&a, &b| {
            let o = entries[a].file.size_bytes.cmp(&entries[b].file.size_bytes);
            if asc {
                o
            } else {
                o.reverse()
            }
        }),
        SortBy::Modified => idxs.sort_by(|&a, &b| {
            let o = entries[a].file.mtime.cmp(&entries[b].file.mtime);
            if asc {
                o
            } else {
                o.reverse()
            }
        }),
        // Unrated sorts as 0 so rated assets lead when descending (the common use).
        SortBy::Rating => idxs.sort_by(|&a, &b| {
            let o = entries[a]
                .rating
                .unwrap_or(0)
                .cmp(&entries[b].rating.unwrap_or(0));
            if asc {
                o
            } else {
                o.reverse()
            }
        }),
    }
}

/// Folders view, inside a source: the assets living directly in `dir` plus one tile
/// per child directory with its contained-asset count (recursive), both `tab`-filtered.
/// Tiles come back name-sorted (BTreeMap).
fn folder_children(
    entries: &[AssetEntry],
    src: usize,
    dir: &std::path::Path,
    tab: Category,
) -> (Vec<usize>, Vec<(String, usize)>) {
    let mut tiles: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut idxs = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if e.source_idx != src || e.trashed || !tab.matches(e.file.media_type) {
            continue;
        }
        let Ok(rest) = e.file.rel_path.strip_prefix(dir) else {
            continue;
        };
        let mut comps = rest.components();
        let Some(first) = comps.next() else { continue };
        if comps.next().is_some() {
            // Deeper than one level: counts toward its top-level child folder.
            *tiles
                .entry(first.as_os_str().to_string_lossy().into_owned())
                .or_insert(0) += 1;
        } else {
            idxs.push(i);
        }
    }
    (idxs, tiles.into_iter().collect())
}

/// How many card columns fit in `available` width. Always ≥ 1 so the grid never divides by zero.
/// `available` is the content width *after* subtracting the scrollbar (callers do that).
fn grid_columns(available: f32, thumb_size: f32) -> usize {
    ((available + CARD_PAD) / (thumb_size + CARD_PAD)).max(1.0) as usize
}

/// The row window to prefetch thumbnails for: the visible `[start, end)` widened by
/// `LOOKAHEAD_ROWS` on each side and clamped to `[0, total_rows]`.
fn lookahead_window(start: usize, end: usize, total_rows: usize) -> (usize, usize) {
    // Half the visible window each side → ~2x the visible grid is loading at once.
    let buf = ((end - start) / 2).max(LOOKAHEAD_ROWS_MIN);
    let buf_start = start.saturating_sub(buf);
    let buf_end = (end + buf).min(total_rows);
    (buf_start, buf_end)
}

/// The `sorted_cache` index range `[start, end)` whose textures stay resident:
/// the visible row window widened by `KEEP_ROWS` on each side, in entries, clamped to `len`.
fn keep_range(row_start: usize, row_end: usize, cols: usize, len: usize) -> (usize, usize) {
    // 1.5x the visible window above and below stays resident; further rows evict
    // back to placeholders so VRAM stays bounded on large libraries.
    let keep = ((row_end - row_start) * 3 / 2).max(KEEP_ROWS_MIN);
    let keep_start = (row_start.saturating_sub(keep) * cols).min(len);
    let keep_end = ((row_end + keep) * cols).min(len);
    (keep_start, keep_end)
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for LodestashApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_hydrate();
        self.receive_scan();
        self.receive_thumbnails();
        self.receive_extracts();
        self.receive_catalog();
        self.receive_model_download();
        self.receive_viewer(ctx);
        // Tray ✕ on the scan row: the crawl thread finishes harmlessly (read-only) and
        // its ScanDone is dropped by the generation guard — semantically a cancel.
        if let Some(id) = self.scan_job {
            let cancelled = self
                .jobs
                .jobs
                .iter()
                .any(|j| j.id == id && j.progress.is_cancelled());
            if cancelled && self.scanning {
                self.scanning = false;
                self.scan_generation += 1;
                self.scan_job = None;
                self.jobs.finish(id);
                self.jobs.toast("Scan cancelled", Intent::Info);
            }
        }
        self.jobs.prune();
        if self.pending_packages.iter().any(|p| p.extracting) {
            // Keep the extraction progress bar moving without input events.
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(vp) = &mut self.viewport3d {
            vp.poll();
        }
        if let Some(ap) = &mut self.audio_player {
            ap.poll();
            // Keep the playhead moving while a clip plays (no input events otherwise).
            if ap.is_playing() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        self.ensure_viewer_loaded(ctx);

        // While a scan, thumbnails, or a catalog persist are in flight, wake up at
        // ~30 fps to drain the channels. Keeping this in the main thread (not the
        // worker threads) prevents stray request_repaint() calls from injecting
        // extra frames during resize events.
        if self.pending_thumbs > 0 || self.scanning || self.catalog_persisting {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
        // Job-tray rows and toast expiry need clock ticks, not input events.
        if self.jobs.any_active() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        if self.cache_dirty {
            self.rebuild_cache();
        }

        // ── Keyboard handling (when no text widget has focus) ─────────────────
        if !ctx.wants_keyboard_input()
            && self.view_mode == ViewMode::Folders
            && ctx.input(|i| i.key_pressed(Key::Backspace))
        {
            self.nav_up();
        }
        let n = self.sorted_cache.len();
        if !ctx.wants_keyboard_input() && n > 0 {
            let (left, right, up, down, space, esc, slash) = ctx.input(|i| {
                (
                    i.key_pressed(Key::ArrowLeft) && i.modifiers.is_none(),
                    i.key_pressed(Key::ArrowRight) && i.modifiers.is_none(),
                    i.key_pressed(Key::ArrowUp) && i.modifiers.is_none(),
                    i.key_pressed(Key::ArrowDown) && i.modifiers.is_none(),
                    i.key_pressed(Key::Space),
                    i.key_pressed(Key::Escape),
                    i.key_pressed(Key::Slash),
                )
            });

            if slash {
                // Request focus on the search field; egui picks it up via the id next frame.
                ctx.memory_mut(|m| m.request_focus(egui::Id::new("search_field")));
            }
            if esc {
                if self.quick_look {
                    self.quick_look = false;
                } else {
                    self.selected_cache_idx = None;
                    self.selected.clear();
                    self.select_anchor = None;
                }
            }
            if space && self.selected_cache_idx.is_some() {
                self.quick_look = !self.quick_look;
            }
            // Enter = open (§0.4). The full viewer lives behind Quick Look; dedicated
            // per-type viewers grow inside it (issue #29).
            if ctx.input(|i| i.key_pressed(Key::Enter)) && self.selected_cache_idx.is_some() {
                self.quick_look = true;
            }
            // Ctrl/Cmd-A: select all within the current filter (§0.4, design §7).
            if self.screen == Screen::Library
                && ctx.input(|i| i.modifiers.command && i.key_pressed(Key::A))
            {
                self.selected = self.sorted_cache.iter().copied().collect();
            }
            // Del: move the selection (or the lead card) to the Trash (design §9).
            // ponytail: no confirm modal — Trash is catalog-only and restorable; the
            // §11.6 confirm-before-Trash toggle arrives with the Settings backend.
            if self.screen == Screen::Library && ctx.input(|i| i.key_pressed(Key::Delete)) {
                let targets: Vec<usize> = if self.selected.is_empty() {
                    self.selected_cache_idx
                        .and_then(|ci| self.sorted_cache.get(ci).copied())
                        .into_iter()
                        .collect()
                } else {
                    self.selected.iter().copied().collect()
                };
                self.set_entries_trashed(&targets, true);
            }
            // Keys 1-5: rate the selection (or the lead card); same value again clears.
            if self.screen == Screen::Library {
                let pressed = ctx.input(|i| {
                    [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5]
                        .iter()
                        .position(|&k| i.key_pressed(k) && i.modifiers.is_none())
                });
                if let Some(n) = pressed {
                    let targets: Vec<usize> = if self.selected.is_empty() {
                        self.selected_cache_idx
                            .and_then(|ci| self.sorted_cache.get(ci).copied())
                            .into_iter()
                            .collect()
                    } else {
                        self.selected.iter().copied().collect()
                    };
                    self.set_entries_rating(&targets, Some(n as u8 + 1));
                }
            }

            // Arrow nav — we need the column count from the last frame.
            let cols = self.last_cols.max(1);
            if let Some(cur) = self.selected_cache_idx {
                let next = if right && cur + 1 < n {
                    Some(cur + 1)
                } else if left && cur > 0 {
                    Some(cur - 1)
                } else if down && cur + cols < n {
                    Some(cur + cols)
                } else if up && cur >= cols {
                    Some(cur - cols)
                } else {
                    None
                };
                if let Some(idx) = next {
                    self.selected_cache_idx = Some(idx);
                    self.select_anchor = Some(idx);
                    // Keyboard nav crosses page boundaries: follow the selection.
                    self.page = idx / self.config.view.page_size.max(1);
                    // Keyboard nav collapses a multi-selection to the lead card.
                    self.selected.clear();
                    if let Some(&e) = self.sorted_cache.get(idx) {
                        self.selected.insert(e);
                    }
                    // quick_look stays open; navigating moves the selected asset
                }
            } else if right || left || down || up {
                self.selected_cache_idx = Some(0);
                self.select_anchor = Some(0);
            }
        }

        // ── Top bar ──────────────────────────────────────────────────────────
        let mut add_source_clicked = false;
        let mut open_folder_clicked = false;
        let mut project_to_open: Option<String> = None;
        egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::none()
                    .fill(theme::SURFACE_RAISED)
                    .inner_margin(egui::Margin {
                        left: 12.0,
                        right: 12.0,
                        top: 8.0,
                        bottom: 8.0,
                    }),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    // Logo: rotated gold diamond + Cinzel wordmark (UI-SPEC §0.1).
                    logo_diamond(ui);
                    ui.label(
                        RichText::new("LODESTASH")
                            .color(theme::GOLD)
                            .font(crate::fonts::display(16.0)),
                    );
                    ui.separator();

                    ui.menu_button(RichText::new("File").color(theme::TEXT), |ui| {
                        if let Some(name) = &self.project_name {
                            let location = if self.scan_roots.len() == 1 {
                                self.scan_roots[0].to_string_lossy().into_owned()
                            } else {
                                self.scan_roots
                                    .iter()
                                    .map(|p| p.to_string_lossy().into_owned())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            };
                            ui.label(
                                RichText::new(format!("Current: {name}"))
                                    .color(theme::TEXT_DIM)
                                    .size(11.0),
                            );
                            ui.label(RichText::new(location).color(theme::TEXT_DIM).size(10.0));
                            ui.separator();
                        }

                        if ui.button("Open folder\u{2026}").clicked() {
                            open_folder_clicked = true;
                            ui.close_menu();
                        }

                        let switchable: Vec<&config::LibraryProject> = self
                            .config
                            .projects
                            .iter()
                            .filter(|p| !p.live_paths().is_empty())
                            .collect();
                        if !switchable.is_empty() {
                            ui.separator();
                            ui.label(RichText::new("Projects").color(theme::TEXT_DIM).size(11.0));
                            for proj in switchable {
                                let is_current = self.project_name.as_deref() == Some(&proj.name);
                                let prefix = if is_current { "\u{2713} " } else { "    " };
                                let resp = ui.button(
                                    RichText::new(format!("{prefix}{}", proj.name))
                                        .color(if is_current { theme::GOLD } else { theme::TEXT }),
                                );
                                if resp.clicked() {
                                    if !is_current {
                                        project_to_open = Some(proj.name.clone());
                                    }
                                    ui.close_menu();
                                }
                            }
                        }
                    });
                    // "Add source" only makes sense when a project is already open.
                    if self.project_name.is_some()
                        && ui
                            .button(RichText::new("+ Add source").color(theme::ARCANE))
                            .clicked()
                    {
                        add_source_clicked = true;
                    }

                    // Project / source info — a visible bordered button opening the Sources
                    // panel (frameless text read as a label, not an affordance).
                    if let Some(name) = &self.project_name {
                        let src_count = self.scan_roots.len();
                        let detail = if src_count == 1 {
                            self.scan_roots[0].to_string_lossy().into_owned()
                        } else {
                            format!("{src_count} sources")
                        };
                        let resp = ui
                            .add(
                                egui::Button::new(
                                    RichText::new(format!(
                                        "\u{25A4} {name}  \u{2014}  {detail}  \u{25BE}"
                                    ))
                                    .color(theme::TEXT_MUTED)
                                    .size(11.0),
                                )
                                .fill(theme::OVERLAY)
                                .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG))
                                .rounding(6.0),
                            )
                            .on_hover_text("View / manage sources")
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if resp.hovered() {
                            // Brightened border on hover (prototype's border-color:#6E7689).
                            ui.painter().rect_stroke(
                                resp.rect,
                                6.0,
                                egui::Stroke::new(1.0, theme::TEXT_DIM),
                            );
                        }
                        if resp.clicked() {
                            self.sources_panel_open = true;
                        }
                    }

                    // Search mode toggle + search box (UI-SPEC §0.1, prototype top bar).
                    ui.add_space(6.0);
                    let mode_color = match self.search_mode {
                        SearchMode::Tags => theme::GOLD,
                        SearchMode::Ai => theme::ARCANE,
                    };

                    // Segmented mode toggle in a container pill (prototype `#232C3D` / `#3A465C`).
                    egui::Frame::none()
                        .fill(theme::OVERLAY)
                        .stroke(egui::Stroke::new(1.0, theme::BORDER_STRONG))
                        .rounding(6.0)
                        .inner_margin(egui::Margin::same(2.0))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            for (mode, label) in [
                                (SearchMode::Tags, "\u{27E1} Tags"),
                                (SearchMode::Ai, "\u{2726} AI"),
                            ] {
                                let active = self.search_mode == mode;
                                let color = match mode {
                                    SearchMode::Tags => theme::GOLD,
                                    SearchMode::Ai => theme::ARCANE,
                                };
                                let seg = ui.add(
                                    egui::Button::new(RichText::new(label).size(11.0).color(
                                        if active { color } else { theme::TEXT_DIM },
                                    ))
                                    .fill(if active {
                                        color.linear_multiply(0.16)
                                    } else {
                                        egui::Color32::TRANSPARENT
                                    })
                                    .stroke(egui::Stroke::NONE)
                                    .rounding(4.0),
                                );
                                if seg.clicked() && !active {
                                    self.search_mode = mode;
                                    // Ranks belong to the old mode's query — drop them so the
                                    // new mode starts clean (AI won't inherit Tags' ids as a
                                    // result set, and vice-versa). Re-run recomputes them.
                                    self.search_ranks = None;
                                    self.search_ranks_key = None;
                                    self.cache_dirty = true;
                                }
                            }
                        });

                    ui.add_space(8.0);
                    // Contained search box: `#161B26` bg, mode-colored border, magnifier + input.
                    let hint = match self.search_mode {
                        SearchMode::Tags => "Search\u{2026}  /",
                        SearchMode::Ai => {
                            "describe what you need \u{2014} e.g. \u{BB}pickup sound for rocks\u{AB} \u{2014} then Enter"
                        }
                    };
                    let search_w = (ui.available_width() - 560.0).clamp(120.0, 340.0);
                    let mut search_changed = false;
                    let mut ai_submit = false;
                    egui::Frame::none()
                        .fill(theme::SURFACE)
                        .stroke(egui::Stroke::new(1.0, mode_color.linear_multiply(0.6)))
                        .rounding(6.0)
                        .inner_margin(egui::Margin::symmetric(10.0, 7.0))
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 7.0;
                            let (icon_rect, _) =
                                ui.allocate_exact_size(Vec2::splat(14.0), egui::Sense::hover());
                            egui::Image::new(egui::include_image!("../assets/icons/search.svg"))
                                .tint(theme::TEXT_DIM)
                                .paint_at(ui, icon_rect);
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.search)
                                    .id(egui::Id::new("search_field"))
                                    .frame(false)
                                    .hint_text(hint)
                                    .desired_width(search_w),
                            );
                            if resp.changed() && self.search_mode == SearchMode::Tags {
                                search_changed = true;
                            }
                            // AI mode is Enter-to-run (semantic search is not live-per-keystroke).
                            if self.search_mode == SearchMode::Ai
                                && resp.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                ai_submit = true;
                            }
                        });
                    if search_changed {
                        self.cache_dirty = true;
                    }
                    if ai_submit {
                        self.run_ai_search();
                    }
                    if self.search_mode == SearchMode::Ai {
                        let (msg, col) = if self.ai_embedder.is_some() {
                            ("\u{2726} visual search ready \u{2014} press Enter", theme::ARCANE)
                        } else {
                            // ponytail: engine is done; the gap is the model artifact (P4 manager).
                            ("\u{2726} add a vision model to enable (Settings \u{25B8} AI)", theme::TEXT_DIM)
                        };
                        ui.label(RichText::new(msg).color(col).size(10.0));
                    }

                    // Category buckets (prototype top-bar `cats`): bordered chips, active =
                    // bucket color border+text, inactive = muted. Library screen only.
                    if self.screen == Screen::Library {
                        ui.add_space(4.0);
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for &(cat, label) in Category::TABS {
                            let active = self.active_tab == cat;
                            let color = cat.color();
                            let resp = ui.add(
                                egui::Button::new(
                                    RichText::new(label).size(12.0).color(if active {
                                        color
                                    } else {
                                        theme::TEXT_MUTED
                                    }),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    if active { color } else { theme::BORDER },
                                ))
                                .rounding(6.0),
                            );
                            if resp.clicked() && !active {
                                self.active_tab = cat;
                                self.cache_dirty = true;
                            }
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // AI-profile pill (§0.1 #7) — arcane outline; links to Settings → AI.
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("\u{2726} local")
                                        .color(theme::ARCANE)
                                        .size(11.0),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::new(1.0, theme::ARCANE))
                                .rounding(999.0),
                            )
                            .on_hover_text("Active identify profile \u{2014} configure in Settings")
                            .clicked()
                        {
                            self.screen = Screen::Settings;
                        }

                        // Project chip (§0.1 #6) — engine projects arrive with the P5 bridge.
                        let chip = ui
                            .add(
                                egui::Button::new(
                                    RichText::new("\u{25CF} No project \u{B7} \u{2014}  \u{25BE}")
                                        .color(theme::TEXT_DIM)
                                        .size(11.0),
                                )
                                .fill(theme::SURFACE)
                                .stroke(egui::Stroke::new(1.0, theme::BORDER))
                                .rounding(999.0),
                            );
                        let chip_popup = ui.make_persistent_id("project_chip_popup");
                        if chip.clicked() {
                            ui.memory_mut(|m| m.toggle_popup(chip_popup));
                        }
                        egui::popup::popup_below_widget(
                            ui,
                            chip_popup,
                            &chip,
                            egui::PopupCloseBehavior::CloseOnClickOutside,
                            |ui| {
                                ui.set_min_width(220.0);
                                // ponytail: §16 switcher stub — real engine projects register
                                // through the P5 Unity bridge.
                                ui.label(
                                    RichText::new("No engine projects yet")
                                        .color(theme::TEXT)
                                        .size(12.0),
                                );
                                ui.label(
                                    RichText::new(
                                        "Unity projects appear here once the editor \
                                         bridge (P5) registers them.",
                                    )
                                    .color(theme::TEXT_DIM)
                                    .size(11.0),
                                );
                            },
                        );

                        // Health meter (§0.1 #5): mono label over a 6px gold bar. Reads
                        // catalog truth once the write-behind persist lands; "identified"
                        // is the non-Unidentified share (identified + confirmed).
                        let (identified, total) = match &self.catalog_counts {
                            Some(c) => (c.identified + c.confirmed, c.total()),
                            None => (0, self.entries.len()),
                        };
                        ui.vertical(|ui| {
                            ui.set_width(180.0);
                            ui.spacing_mut().item_spacing.y = 3.0;
                            ui.label(
                                RichText::new(format!("Identified {identified} / {total}"))
                                    .color(theme::TEXT_MUTED)
                                    .monospace()
                                    .size(10.0),
                            );
                            let (bar, _) = ui
                                .allocate_exact_size(egui::vec2(180.0, 6.0), egui::Sense::hover());
                            let painter = ui.painter();
                            painter.rect_filled(bar, 3.0, theme::BORDER);
                            if total > 0 && identified > 0 {
                                let frac = identified as f32 / total as f32;
                                let fill = egui::Rect::from_min_size(
                                    bar.min,
                                    egui::vec2(bar.width() * frac, bar.height()),
                                );
                                painter.rect_filled(fill, 3.0, theme::GOLD);
                            }
                        });

                        if self.pending_thumbs > 0 {
                            ui.label(
                                RichText::new(format!(
                                    "\u{25CC} {} loading\u{2026}",
                                    self.pending_thumbs
                                ))
                                .color(theme::ARCANE)
                                .size(11.0),
                            );
                        }
                        // Multi-selection count (design §7 "a selection count shows
                        // in the top bar").
                        if self.selected.len() > 1 {
                            ui.label(
                                RichText::new(format!("{} selected", self.selected.len()))
                                    .color(theme::GOLD)
                                    .size(11.0),
                            );
                        }
                    });
                });
            });

        if open_folder_clicked {
            self.open_folder(ctx);
        }
        if let Some(name) = project_to_open {
            self.open_project(ctx, &name);
        }
        if add_source_clicked {
            self.add_source(ctx);
        }

        // ── Sidebar (left, UI-SPEC §0.2) ──────────────────────────────────────
        self.sidebar(ctx);

        // ── View toolbar ──────────────────────────────────────────────────────
        // (Category buckets moved into the top bar next to search — prototype §top bar.)
        if self.screen == Screen::Library && !self.entries.is_empty() {
            egui::TopBottomPanel::top("toolbar")
                .frame(
                    egui::Frame::none()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin {
                            left: 12.0,
                            right: 12.0,
                            top: 5.0,
                            bottom: 5.0,
                        })
                        .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                )
                .show(ctx, |ui| {
                    // Result header row (UI-SPEC §2.2): result label · hint text · sort ·
                    // ⚑ Filters · grid density · view toggle. Pack/collection filter chips
                    // join once the P2 packages model / P3 collections can activate them.
                    ui.horizontal(|ui| {
                        let library_label = match &self.source_filter {
                            Some(LibraryFilter::Source(i)) => {
                                config::stem_name(&self.scan_roots[*i])
                            }
                            Some(LibraryFilter::Package(p)) => p.clone(),
                            None => self
                                .project_name
                                .clone()
                                .unwrap_or_else(|| "All libraries".into()),
                        };
                        ui.label(
                            RichText::new(format!(
                                "{} assets \u{B7} {library_label}",
                                self.sorted_cache.len()
                            ))
                            .color(theme::TEXT_MUTED)
                            .size(12.0),
                        );

                        // Hint text — dim, hidden on narrow windows (§2.2).
                        if ctx.screen_rect().width() >= 1200.0 {
                            ui.label(
                                RichText::new(
                                    "Space quick-looks \u{B7} \u{2922} full size \u{B7} \u{27E1} stage",
                                )
                                .color(theme::TEXT_DIM)
                                .size(11.0),
                            );
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            // Browse view toggle (DESIGN-GUIDELINES.md §7 "Browse views").
                            for (mode, label) in
                                [(ViewMode::Folders, "Folders"), (ViewMode::Flat, "Grid")]
                            {
                                let active = self.view_mode == mode;
                                if ui
                                    .selectable_label(active, RichText::new(label).size(11.0))
                                    .clicked()
                                    && !active
                                {
                                    self.view_mode = mode;
                                    self.nav_to(self.default_dir());
                                    self.save_view_prefs();
                                }
                            }

                            // Grid-size segmented selector (design §2) — grid view only;
                            // drives the persisted `thumb_size`, the single source of truth.
                            if self.view_mode == ViewMode::Flat {
                                ui.separator();
                                for (px, label) in GRID_STOPS.iter().rev() {
                                    let active = (self.thumb_size - px).abs() < 8.0;
                                    if ui
                                        .selectable_label(active, RichText::new(*label).size(11.0))
                                        .clicked()
                                        && !active
                                    {
                                        self.thumb_size = *px;
                                        self.save_view_prefs();
                                        self.cache_dirty = true;
                                    }
                                }
                            }

                            // ⚑ Filters button + content-filter popover (design §2, §3).
                            ui.separator();
                            let active = self.content_filter.any_active();
                            let ftext = if active {
                                RichText::new(format!(
                                    "\u{2691} Filters  {}",
                                    self.content_filter.active_count()
                                ))
                                .color(theme::BG)
                                .size(11.0)
                            } else {
                                RichText::new("\u{2691} Filters")
                                    .color(theme::TEXT)
                                    .size(11.0)
                            };
                            let mut fbtn = egui::Button::new(ftext);
                            if active {
                                fbtn = fbtn
                                    .fill(theme::GOLD)
                                    .stroke(egui::Stroke::new(1.0, theme::GOLD));
                            }
                            let fresp = ui.add(fbtn);
                            let popup_id = ui.make_persistent_id("filters_popup");
                            if fresp.clicked() {
                                ui.memory_mut(|m| m.toggle_popup(popup_id));
                            }
                            let cf = &mut self.content_filter;
                            let tiers = &self.config.size_tiers;
                            let mut filter_changed = false;
                            egui::popup::popup_below_widget(
                                ui,
                                popup_id,
                                &fresp,
                                egui::PopupCloseBehavior::CloseOnClickOutside,
                                |ui| {
                                    filter_changed = filter_popover(ui, cf, tiers);
                                },
                            );
                            if filter_changed {
                                self.cache_dirty = true;
                            }

                            // Sort select + direction.
                            ui.separator();
                            let sort_before = self.sort_by;
                            let asc_before = self.sort_asc;
                            let dir_label = if self.sort_asc {
                                "\u{2191}"
                            } else {
                                "\u{2193}"
                            };
                            if ui.small_button(dir_label).clicked() {
                                self.sort_asc = !self.sort_asc;
                            }
                            egui::ComboBox::from_id_salt("sort_combo")
                                .width(72.0)
                                .selected_text(
                                    RichText::new(self.sort_by.label())
                                        .color(theme::TEXT)
                                        .size(11.0),
                                )
                                .show_ui(ui, |ui| {
                                    for &(s, label) in SortBy::OPTIONS {
                                        ui.selectable_value(&mut self.sort_by, s, label);
                                    }
                                });
                            ui.label(RichText::new("Sort").color(theme::TEXT_DIM).size(11.0));
                            if self.sort_by != sort_before || self.sort_asc != asc_before {
                                self.cache_dirty = true;
                                self.save_view_prefs();
                            }
                        });
                    });
                });
        }

        // ── Job tray + toasts (DESIGN-GUIDELINES §7, UI-SPEC §18) ─────────────
        self.jobs_tray(ctx);
        self.toasts_overlay(ctx);

        // ── Pending packages bar (Unity Assets screen only) ─────────────────────
        // .unitypackage files found while crawling — the Extract/Browse affordance lives on
        // the dedicated Unity Assets screen, not cluttering the Library view.
        if !self.pending_packages.is_empty() && self.screen == Screen::Unity {
            let mut extract_clicked: Option<usize> = None;
            let mut picked_clicked: Option<usize> = None;
            egui::TopBottomPanel::top("packages_bar")
                .frame(
                    egui::Frame::none()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin::symmetric(12.0, 6.0))
                        .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                )
                .show(ctx, |ui| {
                    for i in 0..self.pending_packages.len() {
                        ui.horizontal(|ui| {
                            let pkg = &mut self.pending_packages[i];
                            ui.label(
                                RichText::new(format!("\u{1F4E6} {}", pkg.display_name))
                                    .color(theme::TEXT)
                                    .size(12.0),
                            );
                            if !pkg.nested_candidates.is_empty() {
                                let selected = pkg.nested_candidates[pkg.chosen_variant].0.clone();
                                egui::ComboBox::from_id_salt(("pkg_variant", i))
                                    .width(220.0)
                                    .selected_text(
                                        RichText::new(selected).color(theme::TEXT).size(11.0),
                                    )
                                    .show_ui(ui, |ui| {
                                        for (vi, (label, _)) in
                                            pkg.nested_candidates.iter().enumerate()
                                        {
                                            ui.selectable_value(
                                                &mut pkg.chosen_variant,
                                                vi,
                                                label.clone(),
                                            );
                                        }
                                    });
                            }
                            if pkg.extracting {
                                // ponytail: surgical extracts have no progress callback —
                                // a spinner is the honest indicator; full extracts keep
                                // the byte-accurate bar.
                                if pkg.browsing {
                                    ui.add(egui::Spinner::new().size(14.0));
                                    ui.label(
                                        RichText::new("extracting selection…")
                                            .color(theme::TEXT_DIM)
                                            .size(10.0),
                                    );
                                } else {
                                    let frac = pkg.progress.fraction();
                                    ui.add(
                                        egui::ProgressBar::new(frac)
                                            .desired_width(140.0)
                                            .desired_height(14.0)
                                            .fill(theme::ARCANE)
                                            .text(
                                                RichText::new(format!(
                                                    "extracting {:.0}%",
                                                    frac * 100.0
                                                ))
                                                .size(10.0),
                                            ),
                                    );
                                }
                            } else {
                                if ui.add(egui::Button::new("Extract")).clicked() {
                                    extract_clicked = Some(i);
                                }
                                // Pick-and-extract only makes sense where the manifest
                                // lists real assets — wrappers only contain sub-packages.
                                if pkg.nested_candidates.is_empty()
                                    && ui
                                        .add(egui::Button::new("Browse").selected(pkg.browsing))
                                        .clicked()
                                {
                                    pkg.browsing = !pkg.browsing;
                                }
                            }
                        });
                        let pkg = &mut self.pending_packages[i];
                        if pkg.browsing && !pkg.extracting {
                            let all: Vec<usize> = pkg.extractable().collect();
                            ui.horizontal(|ui| {
                                if ui.small_button("All").clicked() {
                                    pkg.picked = all.iter().copied().collect();
                                }
                                if ui.small_button("None").clicked() {
                                    pkg.picked.clear();
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "{} of {} picked",
                                        pkg.picked.len(),
                                        all.len()
                                    ))
                                    .color(theme::TEXT_DIM)
                                    .size(10.0),
                                );
                                if !pkg.picked.is_empty()
                                    && ui
                                        .add(egui::Button::new(format!(
                                            "Extract selected ({})",
                                            pkg.picked.len()
                                        )))
                                        .clicked()
                                {
                                    picked_clicked = Some(i);
                                }
                            });
                            egui::ScrollArea::vertical()
                                .id_salt(("pkg_browse", i))
                                .max_height(220.0)
                                .show(ui, |ui| {
                                    for ei in all {
                                        let e = &pkg.entries[ei];
                                        let (Some(pathname), Some(size)) =
                                            (e.pathname.as_deref(), e.size_bytes)
                                        else {
                                            continue;
                                        };
                                        ui.horizontal(|ui| {
                                            let mut on = pkg.picked.contains(&ei);
                                            if ui.checkbox(&mut on, "").changed() {
                                                if on {
                                                    pkg.picked.insert(ei);
                                                } else {
                                                    pkg.picked.remove(&ei);
                                                }
                                            }
                                            ui.label(
                                                RichText::new(pathname)
                                                    .color(theme::TEXT_DIM)
                                                    .size(11.0),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(format_bytes(size as u64))
                                                            .color(theme::TEXT_MUTED)
                                                            .size(10.0),
                                                    );
                                                },
                                            );
                                        });
                                    }
                                });
                        }
                    }
                });
            if let Some(idx) = extract_clicked {
                self.extract_package(ctx, idx);
            }
            if let Some(idx) = picked_clicked {
                self.extract_picked(ctx, idx);
            }
        }

        // ── Inspector pane (right side) ───────────────────────────────────────
        let mut inspector_rating: Option<(usize, Option<u8>)> = None;
        if self.screen == Screen::Library {
            if let Some(cache_idx) = self.selected_cache_idx {
                if let Some(&entry_idx) = self.sorted_cache.get(cache_idx) {
                    if let Some(entry) = self.entries.get(entry_idx) {
                        let file = &entry.file;
                        // Library Source label — only meaningful with >1 source (DATA-MODEL.md §10).
                        let library = (self.scan_roots.len() > 1)
                            .then(|| config::stem_name(&self.scan_roots[entry.source_idx]));
                        egui::SidePanel::right("inspector")
                            .resizable(true)
                            .min_width(200.0)
                            .default_width(260.0)
                            .frame(
                                egui::Frame::none()
                                    .fill(theme::SURFACE)
                                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                                    .inner_margin(egui::Margin::same(12.0)),
                            )
                            .show(ctx, |ui| {
                                if let Some(new_rating) = inspector_pane(
                                    ui,
                                    file,
                                    entry.status,
                                    entry.rating,
                                    entry.texture.as_ref(),
                                    library.as_deref(),
                                ) {
                                    inspector_rating = Some((entry_idx, new_rating));
                                }
                            });
                    }
                }
            }
        }
        if let Some((entry_idx, new_rating)) = inspector_rating {
            self.set_entries_rating(&[entry_idx], new_rating);
        }

        // A page flip (or page-size change) must re-check the spawn window even when
        // the visible row numbers happen to match the previous page's.
        let page_key = (self.page, self.config.view.page_size);
        if page_key != self.last_page_key {
            self.last_page_key = page_key;
            self.last_cols = 0;
            self.last_row_start = usize::MAX;
            self.last_row_end = 0;
        }

        // ── Pager bar (Library only): page controls + assets-per-page ─────────
        if self.screen == Screen::Library && !self.sorted_cache.is_empty() {
            egui::TopBottomPanel::bottom("pager")
                .frame(
                    egui::Frame::none()
                        .fill(theme::SURFACE)
                        .stroke(egui::Stroke::new(1.0, theme::BORDER))
                        .inner_margin(egui::Margin::symmetric(12.0, 4.0)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Per page").color(theme::TEXT_DIM).size(11.0));
                        let current = self.config.view.page_size;
                        let selected_label = if self.page_size_custom {
                            "Custom\u{2026}".to_owned()
                        } else {
                            current.to_string()
                        };
                        let mut new_size: Option<usize> = None;
                        egui::ComboBox::from_id_salt("page_size_combo")
                            .width(76.0)
                            .selected_text(RichText::new(selected_label).size(11.0))
                            .show_ui(ui, |ui| {
                                for preset in PAGE_SIZE_PRESETS {
                                    if ui
                                        .selectable_label(
                                            !self.page_size_custom && current == preset,
                                            preset.to_string(),
                                        )
                                        .clicked()
                                    {
                                        self.page_size_custom = false;
                                        new_size = Some(preset);
                                    }
                                }
                                if ui
                                    .selectable_label(self.page_size_custom, "Custom\u{2026}")
                                    .clicked()
                                {
                                    self.page_size_custom = true;
                                }
                            });
                        if self.page_size_custom {
                            let mut value = current;
                            let resp = ui.add(
                                egui::DragValue::new(&mut value)
                                    .range(config::PAGE_SIZE_RANGE)
                                    .speed(1),
                            );
                            if resp.changed() {
                                new_size = Some(value);
                            }
                        }
                        if let Some(size) = new_size {
                            let size = size.clamp(
                                *config::PAGE_SIZE_RANGE.start(),
                                *config::PAGE_SIZE_RANGE.end(),
                            );
                            if size != self.config.view.page_size {
                                self.config.view.page_size = size;
                                self.page = self.page.min(self.page_count().saturating_sub(1));
                                self.config.save();
                            }
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let pages = self.page_count();
                            if ui
                                .add_enabled(
                                    self.page + 1 < pages,
                                    egui::Button::new("Next \u{203A}"),
                                )
                                .clicked()
                            {
                                self.page += 1;
                            }
                            ui.label(
                                RichText::new(format!("Page {} / {}", self.page + 1, pages))
                                    .color(theme::TEXT_MUTED)
                                    .size(11.0)
                                    .monospace(),
                            );
                            if ui
                                .add_enabled(self.page > 0, egui::Button::new("\u{2039} Prev"))
                                .clicked()
                            {
                                self.page = self.page.saturating_sub(1);
                            }
                        });
                    });
                });
        }

        // ── Main content ─────────────────────────────────────────────────────
        let mut open_project: Option<String> = None;
        let mut open_single: Option<PathBuf> = None;
        let mut spawn_queue: Vec<usize> = Vec::new();
        let mut clicked_cache: Option<(usize, egui::Modifiers)> = None;
        let mut context_action: Option<(usize, ContextAction)> = None;
        let mut evict_window: Option<(usize, usize, usize)> = None;
        let mut back_to_library = false;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                // §0 ambience: faint radial gradients — gold top-right, arcane bottom-left.
                let panel = ui.max_rect();
                let d = panel.width() * 0.55;
                let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                if let Some(tex) = &self.bg_gold {
                    ui.painter().image(
                        tex.id(),
                        egui::Rect::from_center_size(panel.right_top(), Vec2::splat(d)),
                        uv,
                        egui::Color32::WHITE,
                    );
                }
                if let Some(tex) = &self.bg_arcane {
                    ui.painter().image(
                        tex.id(),
                        egui::Rect::from_center_size(panel.left_bottom(), Vec2::splat(d)),
                        uv,
                        egui::Color32::WHITE,
                    );
                }
                match self.screen {
                    Screen::Settings => {
                        self.settings_screen(ui, ctx);
                        return;
                    }
                    Screen::Packages => {
                        self.packages_screen(ui, ctx);
                        return;
                    }
                    Screen::Unity => {
                        self.unity_screen(ui, ctx);
                        return;
                    }
                    Screen::Trash => {
                        self.trash_screen(ui);
                        return;
                    }
                    Screen::Library => {}
                    // Unbuilt destinations render the §12 stub — never a dead click.
                    other => {
                        if stub_screen(ui, other) {
                            back_to_library = true;
                        }
                        return;
                    }
                }
                if self.entries.is_empty() {
                    if self.scanning {
                        scanning_state(ui);
                    } else {
                        empty_state(ui, &self.config, &mut open_project, &mut open_single);
                    }
                    return;
                }
                // Unidentified nudge (UI-SPEC §2.1) — pre-catalog, every asset is
                // Unidentified, so the count is the whole library until P3 status lands.
                if !self.nudge_dismissed && !self.entries.is_empty() {
                    let n = self.entries.len();
                    egui::Frame::none()
                        .fill(theme::WARNING.linear_multiply(0.10))
                        .stroke(egui::Stroke::new(1.0, theme::WARNING.linear_multiply(0.45)))
                        .rounding(4.0)
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{n} assets are Unidentified \u{2014} invisible to AI \
                                         search until appraised."
                                    ))
                                    .color(theme::WARNING)
                                    .size(12.0),
                                );
                                if ui
                                    .link(
                                        RichText::new("Open Identify mode \u{2192}")
                                            .color(theme::GOLD)
                                            .size(12.0),
                                    )
                                    .clicked()
                                {
                                    self.screen = Screen::Identify;
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .small_button(RichText::new("\u{2715}").size(10.0))
                                            .on_hover_text("Dismiss for this session")
                                            .clicked()
                                        {
                                            self.nudge_dismissed = true;
                                        }
                                    },
                                );
                            });
                        });
                    ui.add_space(6.0);
                }

                if self.view_mode == ViewMode::Folders {
                    self.folders_nav_ui(ui);
                }
                let tiles_shown = self.view_mode == ViewMode::Folders
                    && !self.folder_tiles.is_empty()
                    && self.search.is_empty();
                if self.sorted_cache.is_empty() {
                    if !tiles_shown {
                        no_results_state(ui, &self.search);
                    }
                } else {
                    // Thumbnail pages: only the current page's slice reaches the
                    // grid, so decode work and textures stay bounded per page.
                    let page_range = self.page_range();
                    let page_start = page_range.start;
                    let out = thumbnail_grid(
                        ui,
                        &self.entries,
                        &self.sorted_cache[page_range],
                        page_start,
                        self.thumb_size,
                        self.last_cols,
                        self.last_row_start,
                        self.last_row_end,
                        self.selected_cache_idx,
                        &self.selected,
                        self.nav_generation.wrapping_add(self.page as u64),
                        &self.config.size_tiers,
                    );
                    if out.cols != self.last_cols
                        || out.row_start != self.last_row_start
                        || out.row_end != self.last_row_end
                    {
                        evict_window = Some((out.cols, out.row_start, out.row_end));
                    }
                    self.last_cols = out.cols;
                    self.last_row_start = out.row_start;
                    self.last_row_end = out.row_end;
                    spawn_queue = out.to_spawn;
                    clicked_cache = out.clicked;
                    context_action = out.context;
                }
            });

        if back_to_library {
            self.go_to_library();
        }
        if let Some((cols, row_start, row_end)) = evict_window {
            self.evict_far_thumbnails(cols, row_start, row_end);
        }

        // Selection semantics (DESIGN-GUIDELINES §7): click selects; Ctrl toggles;
        // Shift extends a range from the anchor. Second plain click = Quick Look.
        if let Some((cache_idx, mods)) = clicked_cache {
            if let Some(&entry_idx) = self.sorted_cache.get(cache_idx) {
                if mods.command {
                    if !self.selected.insert(entry_idx) {
                        self.selected.remove(&entry_idx);
                    }
                    self.selected_cache_idx = Some(cache_idx);
                    self.select_anchor = Some(cache_idx);
                    self.quick_look = false;
                } else if mods.shift {
                    let anchor = self.select_anchor.unwrap_or(cache_idx);
                    let (a, b) = (anchor.min(cache_idx), anchor.max(cache_idx));
                    self.selected = self.sorted_cache[a..=b.min(self.sorted_cache.len() - 1)]
                        .iter()
                        .copied()
                        .collect();
                    self.selected_cache_idx = Some(cache_idx);
                    self.quick_look = false;
                } else if self.selected_cache_idx == Some(cache_idx) && self.selected.len() <= 1 {
                    // Second click on same card: open Quick Look.
                    self.quick_look = true;
                } else {
                    self.selected.clear();
                    self.selected.insert(entry_idx);
                    self.selected_cache_idx = Some(cache_idx);
                    self.select_anchor = Some(cache_idx);
                    self.quick_look = false;
                }
            }
        }

        // Context-menu actions operate on the whole selection where that makes sense
        // (Copy path); Reveal / open act on the clicked card.
        if let Some((cache_idx, action)) = context_action {
            if let Some(&entry_idx) = self.sorted_cache.get(cache_idx) {
                match action {
                    ContextAction::QuickLook => {
                        self.selected_cache_idx = Some(cache_idx);
                        self.quick_look = true;
                    }
                    ContextAction::Reveal => {
                        reveal_file(&self.entries[entry_idx].file.abs_path);
                    }
                    ContextAction::OpenWithDefault => {
                        // Opens the file with its default handler on every platform.
                        reveal_in_file_manager(&self.entries[entry_idx].file.abs_path);
                    }
                    ContextAction::Trash => {
                        let targets: Vec<usize> = if self.selected.contains(&entry_idx) {
                            self.selected.iter().copied().collect()
                        } else {
                            vec![entry_idx]
                        };
                        self.set_entries_trashed(&targets, true);
                    }
                    ContextAction::CopyPath => {
                        let set = if self.selected.contains(&entry_idx) {
                            self.selected.iter().copied().collect::<Vec<_>>()
                        } else {
                            vec![entry_idx]
                        };
                        let text = set
                            .iter()
                            .map(|&i| self.entries[i].file.abs_path.to_string_lossy().into_owned())
                            .collect::<Vec<_>>()
                            .join("\n");
                        ctx.output_mut(|o| o.copied_text = text);
                    }
                }
            }
        }

        if let Some(name) = open_project {
            self.open_project(ctx, &name.clone());
        } else if let Some(path) = open_single {
            let proj_name = self.config.record_open(path.clone());
            self.load_paths(ctx, vec![path], Some(proj_name));
        }
        if !spawn_queue.is_empty() {
            self.spawn_thumbnails(ctx, &spawn_queue);
        }

        // ── Quick Look overlay ────────────────────────────────────────────────
        if self.quick_look {
            if let Some(cache_idx) = self.selected_cache_idx {
                if let Some(&entry_idx) = self.sorted_cache.get(cache_idx) {
                    // Cheap clones so the closure can borrow the viewer mutably.
                    let peek = self.entries.get(entry_idx).map(|e| {
                        (
                            e.file
                                .rel_path
                                .file_stem()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_default(),
                            e.file.media_type,
                            e.file.size_bytes,
                            e.texture.clone(),
                            e.file.media_type == Some(MediaType::Model3d)
                                && viewport3d::is_viewable_model(&e.file.abs_path),
                        )
                    });
                    if let Some((name, media_type, size_bytes, thumb, direct_model)) = peek {
                        let mut open = self.quick_look;
                        let viewer = &mut self.image_viewer;
                        let viewport = self.viewport3d.as_mut();
                        let audio = self.audio_player.as_mut();
                        egui::Window::new("Quick Look")
                            .id(egui::Id::new("quick_look"))
                            .open(&mut open)
                            .collapsible(false)
                            .resizable(true)
                            .default_size([640.0, 620.0])
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .frame(
                                egui::Frame::window(&ctx.style())
                                    .fill(theme::SURFACE_RAISED)
                                    .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                            )
                            .show(ctx, |ui| {
                                quick_look_content(
                                    ui,
                                    &name,
                                    media_type,
                                    size_bytes,
                                    thumb.as_ref(),
                                    viewer,
                                    direct_model,
                                    viewport,
                                    audio,
                                );
                            });
                        self.quick_look = open;
                        // Persist a volume change from the audio player's slider (global).
                        if let Some(ap) = &self.audio_player {
                            if (ap.volume() - self.config.volume).abs() > 1e-4 {
                                self.config.volume = ap.volume();
                                self.config.save();
                            }
                        }
                    }
                }
            }
        }

        // ── First-run onboarding overlay (design §1) ──────────────────────────
        if self.screen == Screen::Library
            && self.onboarding_open
            && self.scan_roots.is_empty()
            && !self.scanning
        {
            let mut open_folder = false;
            let mut onb_project: Option<String> = None;
            let mut onb_single: Option<PathBuf> = None;
            let mut dismiss = false;
            let mut skip = self.config.skip_onboarding;

            // Dimming scrim over the whole window; catches clicks so the library behind stays inert.
            egui::Area::new(egui::Id::new("onboarding_scrim"))
                .order(egui::Order::Middle)
                .fixed_pos(egui::Pos2::ZERO)
                .interactable(true)
                .show(ctx, |ui| {
                    let screen = ctx.screen_rect();
                    ui.allocate_rect(screen, egui::Sense::click());
                    ui.painter()
                        .rect_filled(screen, 0.0, theme::BG.linear_multiply(0.6));
                });

            egui::Window::new("onboarding")
                .title_bar(false)
                .resizable(false)
                .collapsible(false)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .default_width(440.0)
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(theme::SURFACE_RAISED)
                        .stroke(egui::Stroke::new(1.0, theme::BORDER))
                        .inner_margin(egui::Margin::same(22.0)),
                )
                .show(ctx, |ui| {
                    onboarding_content(
                        ui,
                        &self.config,
                        &mut open_folder,
                        &mut onb_project,
                        &mut onb_single,
                        &mut dismiss,
                        &mut skip,
                    );
                });

            if skip != self.config.skip_onboarding {
                self.config.skip_onboarding = skip;
                self.config.save();
            }
            if dismiss {
                self.onboarding_open = false;
            }
            if open_folder {
                self.onboarding_open = false;
                self.open_folder(ctx);
            } else if let Some(name) = onb_project {
                self.onboarding_open = false;
                self.open_project(ctx, &name);
            } else if let Some(path) = onb_single {
                self.onboarding_open = false;
                let proj_name = self.config.record_open(path.clone());
                self.load_paths(ctx, vec![path], Some(proj_name));
            }
        }

        // ── Sources panel ──────────────────────────────────────────────────────
        if self.sources_panel_open {
            let mut open = self.sources_panel_open;
            let mut to_remove: Option<PathBuf> = None;
            let mut add_clicked = false;
            let mut rescan_clicked = false;
            egui::Window::new("Sources")
                .id(egui::Id::new("sources_panel"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_size([480.0, 320.0])
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(theme::SURFACE_RAISED)
                        .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                )
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Project sources")
                            .color(theme::TEXT_MUTED)
                            .size(12.0)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    for root in &self.scan_roots {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(root.to_string_lossy())
                                    .color(theme::TEXT)
                                    .size(12.0),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Remove").clicked() {
                                        to_remove = Some(root.clone());
                                    }
                                    if ui.small_button("Reveal").clicked() {
                                        reveal_in_file_manager(root);
                                    }
                                    // ponytail: re-crawls the whole project — per-source
                                    // incremental rescan arrives with the job engine (§11.1).
                                    if ui
                                        .small_button("\u{21BB} Rescan")
                                        .on_hover_text("Re-crawl sources for new/changed files")
                                        .clicked()
                                    {
                                        rescan_clicked = true;
                                    }
                                },
                            );
                        });
                    }
                    ui.add_space(8.0);
                    if ui
                        .button(RichText::new("+ Add source").color(theme::ARCANE))
                        .clicked()
                    {
                        add_clicked = true;
                    }

                    ui.separator();
                    ui.label(
                        RichText::new("App data")
                            .color(theme::TEXT_MUTED)
                            .size(12.0)
                            .strong(),
                    );
                    ui.add_space(4.0);
                    if let Some(config_path) = config::config_file_path() {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Config: {}", config_path.display()))
                                    .color(theme::TEXT_DIM)
                                    .size(11.0),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Reveal").clicked() {
                                        if let Some(parent) = config_path.parent() {
                                            reveal_in_file_manager(parent);
                                        }
                                    }
                                },
                            );
                        });
                    }
                    if let Some(thumb_dir) = &self.thumb_cache_dir {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!("Thumbnail cache: {}", thumb_dir.display()))
                                    .color(theme::TEXT_DIM)
                                    .size(11.0),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.small_button("Reveal").clicked() {
                                        reveal_in_file_manager(thumb_dir);
                                    }
                                },
                            );
                        });
                    }
                });
            self.sources_panel_open = open;
            if let Some(path) = to_remove {
                self.remove_source(ctx, &path);
            }
            if add_clicked {
                self.add_source(ctx);
            }
            if rescan_clicked {
                if let Some(name) = self.project_name.clone() {
                    self.open_project(ctx, &name);
                }
            }
        }
    }
}

/// Open `path` in the OS file manager (Explorer/Finder/whatever `xdg-open` resolves to).
/// Best-effort: a failure to launch is silently ignored, same as any other OS-integration action
/// that has no in-app fallback to show.
fn reveal_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

// ── Settings screen widgets (design §5) ────────────────────────────────────────

/// A settings section card: `SURFACE` fill, `BORDER`, 10px radius, letter-spaced title.
fn settings_card(ui: &mut Ui, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .rounding(10.0)
        .inner_margin(egui::Margin::same(14.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            section_label(ui, title);
            ui.add_space(10.0);
            add(ui);
        });
    ui.add_space(12.0);
}

/// A label + description on the left, a checkbox toggle on the right. Returns true if toggled.
fn setting_toggle(ui: &mut Ui, label: &str, desc: &str, value: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(RichText::new(label).size(13.0).color(theme::TEXT));
            if !desc.is_empty() {
                ui.label(RichText::new(desc).size(10.0).color(theme::TEXT_DIM));
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.checkbox(value, "").changed() {
                changed = true;
            }
        });
    });
    changed
}

/// A 3-way (or N-way) segmented selector. Returns true if the selection changed.
fn segmented<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, options: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        for &(val, label) in options {
            let active = *current == val;
            if ui
                .selectable_label(active, RichText::new(label).size(12.0))
                .clicked()
                && !active
            {
                *current = val;
                changed = true;
            }
        }
    });
    changed
}

/// The File-size labels editor (design §5.3): a painted preview track, editable MB boundaries +
/// cap, a reset, and a resolved-range legend. Enforces the ascending / 128 KB-gap / 10 MB-cap
/// invariant via [`SizeTiers::normalized`]. Returns true if the thresholds changed.
fn size_tiers_editor(ui: &mut Ui, tiers: &mut SizeTiers) -> bool {
    let mut t = *tiers;

    // Preview track: cap-relative segment fills, tinted per tier.
    let track_w = ui.available_width().min(700.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(track_w, 18.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, theme::BG);
    let cap = t.cap_kb.max(1) as f32;
    for tier in SizeTier::ALL {
        let (lo, hi) = t.range_kb(tier);
        let x0 = rect.left() + rect.width() * (lo as f32 / cap).clamp(0.0, 1.0);
        let x1 = rect.left() + rect.width() * (hi as f32 / cap).clamp(0.0, 1.0);
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x0, rect.top()), egui::pos2(x1, rect.bottom())),
            0.0,
            size_tier_color(tier).linear_multiply(0.55),
        );
    }
    ui.add_space(10.0);

    // Editable numeric boundaries (MB) — disjoint mutable field borrows.
    ui.horizontal(|ui| {
        for (label, kb) in [
            ("S|M", &mut t.m_kb),
            ("M|L", &mut t.l_kb),
            ("L|XL", &mut t.xl_kb),
            ("Cap", &mut t.cap_kb),
        ] {
            ui.label(RichText::new(label).size(11.0).color(theme::TEXT_MUTED));
            let mut mb = *kb as f32 / 1024.0;
            let max_mb = SizeTiers::MAX_CAP_KB as f32 / 1024.0;
            if ui
                .add(
                    egui::DragValue::new(&mut mb)
                        .speed(0.05)
                        .range(0.125..=max_mb)
                        .suffix(" MB")
                        .fixed_decimals(2),
                )
                .changed()
            {
                *kb = (mb * 1024.0).round() as u32;
            }
            ui.add_space(12.0);
        }
    });

    ui.add_space(8.0);
    if ui.button("Reset defaults").clicked() {
        t = SizeTiers::default();
    }

    // Normalize, then draw the resolved-range legend from the normalized values.
    let t = t.normalized();
    ui.add_space(8.0);
    for tier in SizeTier::ALL {
        let (lo, hi) = t.range_kb(tier);
        ui.horizontal(|ui| {
            let (dot, p) = ui.allocate_painter(Vec2::splat(10.0), egui::Sense::hover());
            p.circle_filled(dot.rect.center(), 4.0, size_tier_color(tier));
            ui.label(
                RichText::new(format!(
                    "{}   {} \u{2013} {}",
                    tier.letter(),
                    fmt_kb(lo),
                    fmt_kb(hi)
                ))
                .size(11.0)
                .color(theme::TEXT_MUTED),
            );
        });
    }

    if t != *tiers {
        *tiers = t;
        true
    } else {
        false
    }
}

// ── .unitypackage discovery & extraction ──────────────────────────────────────

fn is_unitypackage(path: &std::path::Path) -> bool {
    path.extension()
        .map(|e| e.eq_ignore_ascii_case("unitypackage"))
        .unwrap_or(false)
}

/// Display name for a package: the file stem of its path (extension and directories stripped).
fn package_display_name(path: &std::path::Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// Browse a freshly-crawled `.unitypackage` (cheap manifest read, no extraction) and turn it into
/// a `PendingPackage` ready for the user to act on. Runs on the background crawl thread, so a
/// large package's manifest read (still O(file size) to decompress, just O(1) memory) never
/// blocks the UI. Returns `None` if the archive can't be read (corrupt/unsupported) — silently
/// skipped rather than surfaced as an error card, matching INGESTION.md §8's "don't promise 100%."
fn resolve_pending_package(source_idx: usize, file: &CrawledFile) -> Option<PendingPackage> {
    let manifest = unitypackage::read_manifest(&file.abs_path).ok()?;
    let nested = unitypackage::nested_package_candidates(&manifest);
    let chosen_variant = unitypackage::preferred_variant_index(&nested);
    let nested_candidates = nested
        .iter()
        .filter_map(|e| {
            e.pathname
                .clone()
                .map(|p| (package_display_name(std::path::Path::new(&p)), p))
        })
        .collect();

    let entries = manifest
        .iter()
        .map(|e| PackageEntryRecord {
            guid: e.guid.clone(),
            pathname: e.pathname.clone(),
            tar_offset: e.asset_offset.map(|o| o as i64),
            size_bytes: e.asset_size.map(|s| s as i64),
        })
        .collect();

    Some(PendingPackage {
        source_idx,
        abs_path: file.abs_path.clone(),
        display_name: package_display_name(&file.rel_path),
        nested_candidates,
        chosen_variant,
        extracting: false,
        progress: Arc::default(),
        entries,
        job: None,
        browsing: false,
        picked: std::collections::BTreeSet::new(),
    })
}

/// Extract `path` into `workspace_dir` and convert the payloads into crawlable entries, virtually
/// namespaced under `display_name` (`rel_path` becomes `{display_name}/{original pathname}`) so
/// Folders view groups them under one tile showing where they came from.
///
/// When `nested_pathname` is set, the outer archive is a render-pipeline-variant wrapper
/// (INGESTION.md §2 "Container nuance"): the first extraction pass only recovers the wrapper's
/// direct payloads (including the nested `.unitypackage` file itself), so a second pass extracts
/// *that* file to reach the real assets.
fn extract_package_contents(
    path: &std::path::Path,
    workspace_dir: &std::path::Path,
    nested_pathname: Option<&str>,
    display_name: &str,
    progress: &JobProgress,
) -> Vec<CrawledFile> {
    let report = |done: u64, total: u64| {
        progress.done.store(done, Ordering::Relaxed);
        progress.total.store(total, Ordering::Relaxed);
        // false stops the extractor at the next read (tray ✕ — cooperative cancel).
        !progress.is_cancelled()
    };
    let Ok(outer) = unitypackage::extract_to_workspace_with_progress(path, workspace_dir, &report)
    else {
        return Vec::new();
    };

    let assets = match nested_pathname {
        Some(nested_pathname) => {
            let Some(nested) = outer
                .iter()
                .find(|a| a.pathname.as_deref() == Some(nested_pathname))
            else {
                return Vec::new();
            };
            // Phase 2 (the real inner package) restarts the bar; still honest activity.
            unitypackage::extract_to_workspace_with_progress(
                &nested.workspace_path,
                workspace_dir,
                &report,
            )
            .unwrap_or_default()
        }
        None => outer,
    };

    extracted_to_crawled(assets, display_name)
}

/// Convert extracted payloads into crawlable grid entries, virtually namespaced under
/// `display_name`. Shared by the full and surgical extraction paths.
fn extracted_to_crawled(
    assets: Vec<unitypackage::ExtractedAsset>,
    display_name: &str,
) -> Vec<CrawledFile> {
    assets
        .into_iter()
        // Unity .meta sidecars are bookkeeping, never grid assets.
        .filter(|a| {
            !a.pathname
                .as_deref()
                .is_some_and(|p| p.to_ascii_lowercase().ends_with(".meta"))
        })
        .map(|a| {
            let rel_suffix = a.pathname.clone().unwrap_or_else(|| a.guid.clone());
            let rel_path = PathBuf::from(display_name).join(rel_suffix);
            let media_type = detect::by_extension(&rel_path);
            let mtime = std::fs::metadata(&a.workspace_path)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            CrawledFile {
                abs_path: a.workspace_path,
                rel_path,
                size_bytes: a.size_bytes,
                mtime,
                media_type,
            }
        })
        .collect()
}

// ── Empty / no-results states ─────────────────────────────────────────────────

fn empty_state(
    ui: &mut Ui,
    config: &AppConfig,
    open_project: &mut Option<String>,
    open_single: &mut Option<PathBuf>,
) {
    let avail = ui.available_size();
    ui.add_space(avail.y * 0.18);
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("\u{25C8}")
                .size(48.0)
                .color(theme::GOLD.linear_multiply(0.4)),
        );
        ui.add_space(8.0);
        ui.label(RichText::new("Open a folder to browse your assets").color(theme::TEXT_MUTED));
        ui.add_space(4.0);
        ui.label(
            RichText::new("Use \"Open folder\u{2026}\" or \"+ Add source\" in the toolbar")
                .color(theme::TEXT_DIM)
                .size(11.0),
        );

        if !config.projects.is_empty() {
            ui.add_space(28.0);
            ui.label(
                RichText::new("Saved Libraries")
                    .color(theme::TEXT_DIM)
                    .size(11.0),
            );
            ui.add_space(8.0);
            for proj in &config.projects {
                let live = proj.live_paths();
                if live.is_empty() {
                    continue;
                }
                let src_label = if proj.paths.len() == 1 {
                    proj.paths[0].to_string_lossy().into_owned()
                } else {
                    format!("{} sources", proj.paths.len())
                };
                let btn = ui.add(
                    egui::Button::new(
                        RichText::new(format!("\u{1F4C1}  {}", proj.name))
                            .color(theme::TEXT)
                            .size(13.0),
                    )
                    .min_size(Vec2::new(240.0, 28.0))
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                );
                if btn.clicked() {
                    *open_project = Some(proj.name.clone());
                }
                ui.label(RichText::new(src_label).color(theme::TEXT_DIM).size(10.0));
                ui.add_space(4.0);
            }
        } else if !config.recent_roots.is_empty() {
            ui.add_space(24.0);
            ui.label(RichText::new("Recent").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(4.0);
            for path in config.recent_roots.iter().take(5) {
                if !path.is_dir() {
                    continue;
                }
                let label = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                if ui
                    .add(
                        egui::Button::new(RichText::new(label).color(theme::TEXT).size(12.0))
                            .fill(egui::Color32::TRANSPARENT),
                    )
                    .clicked()
                {
                    *open_single = Some(path.clone());
                }
            }
        }
    });
}

fn scanning_state(ui: &mut Ui) {
    let avail = ui.available_size();
    ui.add_space(avail.y * 0.3);
    ui.vertical_centered(|ui| {
        ui.add(egui::Spinner::new().size(28.0));
        ui.add_space(8.0);
        ui.label(RichText::new("Scanning\u{2026}").color(theme::TEXT_MUTED));
    });
}

/// Paint a dashed rounded-rectangle border (the gold drop affordance — design §1).
fn paint_dashed_rect(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let stroke = egui::Stroke::new(1.5, color);
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    for i in 0..4 {
        let seg = [corners[i], corners[(i + 1) % 4]];
        for shape in egui::Shape::dashed_line(&seg, stroke, 6.0, 4.0) {
            painter.add(shape);
        }
    }
}

/// First-run onboarding modal content (design §1): the "Point it at your hoard" picker with a
/// gold dashed drop affordance, quick-open of saved libraries/recents, and the
/// "Don't show this next time" checkbox bound to `skip_onboarding` (§5.7).
#[allow(clippy::too_many_arguments)]
fn onboarding_content(
    ui: &mut Ui,
    config: &AppConfig,
    open_folder: &mut bool,
    open_project: &mut Option<String>,
    open_single: &mut Option<PathBuf>,
    dismiss: &mut bool,
    skip: &mut bool,
) {
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("\u{25C8}").size(40.0).color(theme::GOLD));
        ui.add_space(6.0);
        ui.label(
            RichText::new("Point it at your hoard")
                .size(18.0)
                .color(theme::TEXT)
                .strong(),
        );
        ui.add_space(3.0);
        ui.label(
            RichText::new(
                "Choose a folder of assets \u{2014} models, textures, audio, packs \u{2014} and \
                 Lodestash will catalogue it.",
            )
            .size(11.0)
            .color(theme::TEXT_MUTED),
        );
    });
    ui.add_space(14.0);

    // Gold dashed drop well (drag-in ingest is a later QoL card; the button is the live action).
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 64.0), egui::Sense::hover());
    paint_dashed_rect(ui.painter(), rect, theme::GOLD);
    ui.painter().text(
        egui::pos2(rect.center().x, rect.top() + 20.0),
        egui::Align2::CENTER_CENTER,
        "Drop a folder here",
        egui::FontId::proportional(12.0),
        theme::GOLD,
    );
    ui.painter().text(
        egui::pos2(rect.center().x, rect.top() + 40.0),
        egui::Align2::CENTER_CENTER,
        "\u{2014} or \u{2014}",
        egui::FontId::proportional(10.0),
        theme::TEXT_DIM,
    );
    ui.add_space(10.0);
    ui.vertical_centered(|ui| {
        if ui
            .add(
                egui::Button::new(
                    RichText::new("Open folder\u{2026}")
                        .color(theme::BG)
                        .size(13.0)
                        .strong(),
                )
                .min_size(Vec2::new(160.0, 30.0))
                .fill(theme::GOLD),
            )
            .clicked()
        {
            *open_folder = true;
        }
    });

    // Quick-open saved libraries or recents.
    let has_saved = config.projects.iter().any(|p| !p.live_paths().is_empty());
    if has_saved {
        ui.add_space(14.0);
        section_label(ui, "SAVED LIBRARIES");
        ui.add_space(4.0);
        for proj in &config.projects {
            if proj.live_paths().is_empty() {
                continue;
            }
            let detail = if proj.paths.len() == 1 {
                proj.paths[0].to_string_lossy().into_owned()
            } else {
                format!("{} sources", proj.paths.len())
            };
            if ui
                .add(
                    egui::Button::new(
                        RichText::new(format!("\u{1F4C1}  {}", proj.name))
                            .color(theme::TEXT)
                            .size(12.0),
                    )
                    .min_size(Vec2::new(ui.available_width(), 26.0))
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                )
                .on_hover_text(detail)
                .clicked()
            {
                *open_project = Some(proj.name.clone());
            }
            ui.add_space(2.0);
        }
    } else {
        for path in config.recent_roots.iter().filter(|p| p.is_dir()).take(3) {
            let label = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            if ui
                .add(
                    egui::Button::new(RichText::new(label).color(theme::TEXT).size(12.0))
                        .fill(egui::Color32::TRANSPARENT),
                )
                .clicked()
            {
                *open_single = Some(path.clone());
            }
        }
    }

    ui.add_space(12.0);
    ui.separator();
    ui.horizontal(|ui| {
        let mut dont = *skip;
        if ui
            .checkbox(
                &mut dont,
                RichText::new("Don't show this next time")
                    .size(11.0)
                    .color(theme::TEXT_MUTED),
            )
            .changed()
        {
            *skip = dont;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("Skip for now")
                            .size(11.0)
                            .color(theme::TEXT_DIM),
                    )
                    .frame(false),
                )
                .clicked()
            {
                *dismiss = true;
            }
        });
    });
}

fn no_results_state(ui: &mut Ui, query: &str) {
    let avail = ui.available_size();
    ui.add_space(avail.y * 0.3);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("No assets match").color(theme::TEXT_MUTED));
        if !query.is_empty() {
            ui.label(
                RichText::new(format!("\"{query}\""))
                    .color(theme::TEXT_DIM)
                    .size(11.0),
            );
        }
    });
}

// ── Thumbnail grid ────────────────────────────────────────────────────────────

/// Render visible rows via `show_rows` (only O(visible) work per frame).
/// Returns `(to_spawn, cols, row_start, clicked_cache_idx)`.
/// The spawn-check loop only runs when cols or the visible row start changes — pure resize
/// events that don't move either value produce no per-frame allocation work.
// ponytail: threads viewport + selection state for one caller; a params struct would churn
// the render path without removing complexity. Allow until a second caller justifies it.
#[allow(clippy::too_many_arguments)]
/// A context-menu action on a card (DESIGN-GUIDELINES §7 "Selection & context menu").
/// The menu serves single and multi selection alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextAction {
    QuickLook,
    Reveal,
    OpenWithDefault,
    CopyPath,
    Trash,
}

/// What the grid reported back this frame.
struct GridOutput {
    to_spawn: Vec<usize>,
    cols: usize,
    row_start: usize,
    row_end: usize,
    /// (cache index, modifiers at click time)
    clicked: Option<(usize, egui::Modifiers)>,
    /// (cache index, chosen action)
    context: Option<(usize, ContextAction)>,
}

#[allow(clippy::too_many_arguments)] // ponytail: internal fn mirroring grid state; a struct would just rename the args
fn thumbnail_grid(
    ui: &mut Ui,
    entries: &[AssetEntry],
    indices: &[usize],
    /* `sorted_cache` offset of indices[0] — clicked/context report absolute cache idxs */
    index_offset: usize,
    thumb_size: f32,
    prev_cols: usize,
    prev_row_start: usize,
    prev_row_end: usize,
    selected_cache_idx: Option<usize>,
    selected_entries: &std::collections::BTreeSet<usize>,
    nav_salt: u64,
    tiers: &SizeTiers,
) -> GridOutput {
    let card_h = thumb_size + CARD_LABEL;
    let available = ui.available_width() - ui.spacing().scroll.bar_width - 2.0;
    let cols = grid_columns(available, thumb_size);
    let total_rows = indices.len().div_ceil(cols);

    let mut to_spawn: Vec<usize> = Vec::new();
    let mut row_start_out: usize = prev_row_start;
    let mut row_end_out: usize = prev_row_end;
    let mut clicked: Option<(usize, egui::Modifiers)> = None;
    let mut context: Option<(usize, ContextAction)> = None;

    ScrollArea::vertical()
        .id_salt(("asset_grid", nav_salt)) // fresh scroll state per folder navigation
        .auto_shrink([false; 2])
        .show_rows(ui, card_h, total_rows, |ui, row_range| {
            row_start_out = row_range.start;
            row_end_out = row_range.end;

            // Re-check the spawn buffer whenever the viewport window or column layout changes.
            // Includes height-only resize (row_range.end grows, revealing new bottom rows).
            if cols != prev_cols
                || row_range.start != prev_row_start
                || row_range.end != prev_row_end
            {
                let (buf_start, buf_end) =
                    lookahead_window(row_range.start, row_range.end, total_rows);
                for row in buf_start..buf_end {
                    let start = row * cols;
                    let end = (start + cols).min(indices.len());
                    for &idx in &indices[start..end] {
                        if !entries[idx].thumb_spawned
                            && needs_thumbnail(entries[idx].file.media_type)
                        {
                            to_spawn.push(idx);
                        }
                    }
                }
            }

            for row_idx in row_range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CARD_PAD;
                    let start = row_idx * cols;
                    let end = (start + cols).min(indices.len());
                    for (col, &idx) in indices[start..end].iter().enumerate() {
                        let cache_idx = index_offset + start + col;
                        let is_lead = selected_cache_idx == Some(cache_idx);
                        let in_selection = selected_entries.contains(&idx);
                        let resp = asset_card(
                            ui,
                            &entries[idx],
                            thumb_size,
                            is_lead || in_selection,
                            in_selection && selected_entries.len() > 1,
                            tiers,
                        );
                        if resp.clicked() {
                            clicked = Some((cache_idx, ui.input(|i| i.modifiers)));
                        }
                        // Right-click on an unselected card selects it before the menu.
                        if resp.secondary_clicked() && !in_selection {
                            clicked = Some((cache_idx, egui::Modifiers::NONE));
                        }
                        let can_trash = entries[idx].asset_id.is_some();
                        resp.context_menu(|ui| {
                            ui.set_min_width(190.0);
                            if let Some(action) = card_context_menu(ui, can_trash) {
                                context = Some((cache_idx, action));
                            }
                        });
                    }
                });
            }
        });

    GridOutput {
        to_spawn,
        cols,
        row_start: row_start_out,
        row_end: row_end_out,
        clicked,
        context,
    }
}

/// The right-click card menu (DESIGN-GUIDELINES §7). Catalog/engine actions render
/// disabled until their backends exist — grayed with a tooltip, never hidden.
fn card_context_menu(ui: &mut Ui, can_trash: bool) -> Option<ContextAction> {
    let mut action = None;
    let mut act = |ui: &mut Ui, a: ContextAction, label: &str| {
        if ui.button(label).clicked() {
            action = Some(a);
            ui.close_menu();
        }
    };
    // "Open" (in-app viewer) arrives with the P1 viewers.
    ui.add_enabled(false, egui::Button::new("Open"))
        .on_disabled_hover_text("In-app viewers land in P1");
    act(ui, ContextAction::QuickLook, "Quick Look        Space");
    ui.separator();
    act(ui, ContextAction::Reveal, "Reveal in Explorer");
    act(ui, ContextAction::OpenWithDefault, "Open with default app");
    act(ui, ContextAction::CopyPath, "Copy path");
    ui.separator();
    for label in [
        "\u{27E1} Stage",
        "Import\u{2026}",
        "Add to collection",
        "Edit tags",
        "Re-identify",
        "Find similar",
    ] {
        ui.add_enabled(false, egui::Button::new(label))
            .on_disabled_hover_text("Arrives with the catalog / engine bridge (P3\u{2013}P5)");
    }
    ui.separator();
    let trash = ui
        .add_enabled(
            can_trash,
            egui::Button::new(RichText::new("Move to Trash").color(theme::DANGER)),
        )
        .on_disabled_hover_text("Waiting for this asset's catalog row (syncs after scan)");
    if trash.clicked() {
        action = Some(ContextAction::Trash);
        ui.close_menu();
    }
    action
}

/// Reveal a file in the OS file manager, selecting it where the platform allows.
fn reveal_file(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
}

/// Colour for a file-size tier badge (design §3 "FILE SIZE" token mapping).
fn size_tier_color(tier: SizeTier) -> egui::Color32 {
    match tier {
        SizeTier::S => theme::SUCCESS,
        SizeTier::M => theme::TYPE_SOUND,
        SizeTier::L => theme::WARNING,
        SizeTier::Xl => theme::DANGER,
    }
}

/// Human-readable KB → "N KB" / "N MB".
fn fmt_kb(kb: u32) -> String {
    if kb >= 1024 {
        let mb = kb as f32 / 1024.0;
        if (mb.fract()).abs() < 0.05 {
            format!("{mb:.0} MB")
        } else {
            format!("{mb:.1} MB")
        }
    } else {
        format!("{kb} KB")
    }
}

/// A pill filter chip. Returns true if clicked.
fn filter_chip(ui: &mut Ui, label: &str, selected: bool, color: egui::Color32) -> bool {
    let (bg, fg, stroke) = if selected {
        (color.linear_multiply(0.25), color, color)
    } else {
        (theme::OVERLAY, theme::TEXT_MUTED, theme::BORDER_STRONG)
    };
    let resp = egui::Frame::none()
        .fill(bg)
        .rounding(999.0)
        .stroke(egui::Stroke::new(1.0, stroke))
        .inner_margin(egui::Margin::symmetric(9.0, 3.0))
        .show(ui, |ui| {
            ui.label(RichText::new(label).color(fg).size(11.0));
        })
        .response;
    resp.interact(egui::Sense::click()).clicked()
}

/// Contiguous publisher runs over a `browse()` result (already publisher-sorted),
/// as index ranges into the slice. Pure for testability (TECH-STACK §5.1).
fn publisher_runs(pkgs: &[AssetStorePackage]) -> Vec<std::ops::Range<usize>> {
    let mut runs = Vec::new();
    let mut start = 0;
    for i in 1..=pkgs.len() {
        if i == pkgs.len() || pkgs[i].publisher != pkgs[start].publisher {
            runs.push(start..i);
            start = i;
        }
    }
    runs
}

/// The top-bar logo: an 18px rotated gold diamond with an inner bright dot and a
/// faint glow (UI-SPEC §0.1).
fn logo_diamond(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::hover());
    let painter = ui.painter();
    let c = rect.center();
    let diamond = |r: f32| {
        vec![
            c + egui::vec2(0.0, -r),
            c + egui::vec2(r, 0.0),
            c + egui::vec2(0.0, r),
            c + egui::vec2(-r, 0.0),
        ]
    };
    // Faint glow: a slightly larger, mostly-transparent gold diamond behind.
    painter.add(egui::Shape::convex_polygon(
        diamond(11.0),
        theme::GOLD.linear_multiply(0.10),
        egui::Stroke::NONE,
    ));
    painter.add(egui::Shape::convex_polygon(
        diamond(9.0),
        egui::Color32::TRANSPARENT,
        egui::Stroke::new(2.0, theme::GOLD),
    ));
    painter.circle_filled(c, 2.2, theme::GOLD_BRIGHT);
}

/// The single on-disk catalog location: `<OS data dir>/Lodestash/catalog.db`
/// (DATA-MODEL §1 — one SQLite file). None if the platform has no data dir.
fn catalog_db_path() -> Option<PathBuf> {
    let dir = dirs::data_dir()?.join("Lodestash");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("catalog.db"))
}

/// Model id recorded with every vector; the query side reads back under the same id. Bump if
/// the encoder changes (old vectors then simply don't match — no collision).
const SIGLIP_MODEL_ID: &str = "siglip2-base-patch16-256";

/// Locate the SigLIP towers + tokenizer: `SIGLIP_{VISION,TEXT,TOKENIZER}` env override (dev),
/// else `<data>/Lodestash/models/siglip/{vision.onnx,text.onnx,tokenizer.json}` where the model
/// manager downloads them. `None` unless all three are present.
fn siglip_model_paths() -> Option<(PathBuf, PathBuf, PathBuf)> {
    if let (Ok(v), Ok(t), Ok(k)) = (
        std::env::var("SIGLIP_VISION"),
        std::env::var("SIGLIP_TEXT"),
        std::env::var("SIGLIP_TOKENIZER"),
    ) {
        return Some((PathBuf::from(v), PathBuf::from(t), PathBuf::from(k)));
    }
    let dir = dirs::data_dir()?.join("Lodestash").join("models").join("siglip");
    let (v, t, k) = (
        dir.join("vision.onnx"),
        dir.join("text.onnx"),
        dir.join("tokenizer.json"),
    );
    (v.exists() && t.exists() && k.exists()).then_some((v, t, k))
}

/// Load the SigLIP joint image↔text embedder if its artifacts (+ a matching ONNX Runtime,
/// discovered via `load-dynamic`) are present. Any failure is `None` — AI search falls back to
/// its stub, never crashes. On a stock machine with no model this returns `None` immediately.
fn resolve_embedder() -> Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>> {
    let (vision, text, tok) = siglip_model_paths()?;
    match lodestash_ai::siglip::SiglipEmbedder::load(&vision, &text, &tok, SIGLIP_MODEL_ID) {
        Ok(e) => {
            tracing::info!("siglip embedder loaded — Search-with-AI (visual) is live");
            Some(Arc::new(e))
        }
        Err(e) => {
            tracing::info!(%e, "no local siglip model; Search-with-AI stays stubbed");
            None
        }
    }
}

/// Embed the dirty **image** assets (SigLIP vision tower) in batches over the persist thread's
/// own sole-writer connection. Decode failures store a zero vector — which cosine-scores 0, so
/// it never matches — solely to clear `embedding_dirty` and guarantee forward progress (no
/// infinite re-query of an undecodable file). Non-image assets await their own encoder.
fn drain_image_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
) {
    const BATCH: usize = 64;
    let mut total = 0usize;
    loop {
        let jobs = match cat.image_assets_needing_embedding(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "image embed queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            let vector = match lodestash_ingest::raster::decode(std::path::Path::new(&job.abs_path))
            {
                Ok(img) => emb.embed_image(&img.to_rgba8()),
                Err(e) => {
                    // ponytail: undecodable → zero vec clears the flag (permanent-ish failure);
                    // no infinite loop, and it never surfaces in KNN.
                    tracing::warn!(%e, path = %job.abs_path, "image decode failed; storing zero vec");
                    vec![0.0; emb.dim()]
                }
            };
            if let Err(e) = cat.upsert_embedding(
                job.asset_id,
                emb.model_id(),
                "image",
                Some(job.media_type_id),
                Some(job.source_id),
                &vector,
            ) {
                tracing::warn!(%e, "upsert_embedding failed; stopping image drain");
                return;
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        tracing::info!(embedded = total, "siglip image embeddings written");
    }
}

/// Raster types the Quick Look viewer can decode via the shared raster seam
/// (`image` formats + DDS/PSD). HDR/EXR tone-map is the remaining #29 follow-up.
fn is_viewer_image(mt: Option<MediaType>) -> bool {
    matches!(
        mt,
        Some(MediaType::Image) | Some(MediaType::Texture) | Some(MediaType::Sprite)
    )
}

/// Decode a full-detail image on the load thread, downscaling anything past
/// `VIEWER_MAX_DIM` (GPU texture cap). Returns the pixels + original dimensions.
/// Routes through the shared raster seam, so DDS/PSD work here and in thumbnails.
fn load_viewer_image(path: &std::path::Path) -> Result<(egui::ColorImage, [u32; 2]), String> {
    let img = lodestash_ingest::raster::decode(path).map_err(|e| e.to_string())?;
    let dims = [img.width(), img.height()];
    let img = if img.width().max(img.height()) > VIEWER_MAX_DIM {
        // Triangle filter: fast and good enough for an already-oversized preview.
        img.resize(
            VIEWER_MAX_DIM,
            VIEWER_MAX_DIM,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Ok((
        egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()),
        dims,
    ))
}

/// Classified files → catalog crawl records; unclassified files stay grid-only
/// (`asset.media_type_id` is NOT NULL). Paths are stored `/`-separated.
fn crawl_records<'a>(files: impl Iterator<Item = &'a CrawledFile>) -> Vec<CrawlRecord> {
    files
        .filter(|f| f.media_type.is_some())
        .map(|f| CrawlRecord {
            rel_path: f.rel_path.to_string_lossy().replace('\\', "/"),
            name: f
                .rel_path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            media_type: f.media_type.expect("filtered Some").code().to_string(),
            size_bytes: f.size_bytes as i64,
            mtime: f.mtime,
            workspace_rel: None,
        })
        .collect()
}

/// Runs on the persist thread: the extracted package's `package` row, its asset rows
/// (carrying `package_id`), the unpack stamp, then project-wide status counts.
fn persist_package_to_catalog(
    db_path: &std::path::Path,
    job: &PackagePersist,
) -> Result<CatalogReadBack, String> {
    let mut catalog = Catalog::open(db_path).map_err(|e| e.to_string())?;
    catalog.seed_media_types().map_err(|e| e.to_string())?;

    let source_id = catalog
        .upsert_import_source(&job.root_name, &job.root, "folder")
        .map_err(|e| e.to_string())?;
    let package_id = catalog
        .upsert_package(
            source_id,
            &job.package_path,
            &job.name,
            job.publisher.as_deref(),
            job.category.as_deref(),
        )
        .map_err(|e| e.to_string())?;
    let stats = catalog
        .record_crawl(source_id, Some(package_id), &job.records)
        .map_err(|e| e.to_string())?;
    catalog
        .record_package_entries(package_id, &job.entries)
        .map_err(|e| e.to_string())?;
    catalog
        .mark_package_unpacked(package_id, job.records.len())
        .map_err(|e| e.to_string())?;
    tracing::info!(
        package = %job.name,
        inserted = stats.inserted,
        unchanged = stats.unchanged,
        "package sync"
    );

    let mut sources = Vec::with_capacity(job.all_roots.len());
    for (name, root) in &job.all_roots {
        let id = catalog
            .upsert_import_source(name, root, "folder")
            .map_err(|e| e.to_string())?;
        sources.push((root.clone(), id));
    }
    let ids: Vec<i64> = sources.iter().map(|(_, id)| *id).collect();
    let counts = catalog
        .status_counts_for_sources(&ids)
        .map_err(|e| e.to_string())?;
    let assets = read_back_assets(&catalog, &sources)?;
    Ok((counts, assets))
}

/// The persist threads' read-back payload: counts + per-root asset identity maps.
type CatalogReadBack = (StatusCounts, AssetIdentityMaps);

/// Read every live asset row back for the given `(root, source_id)` pairs, keyed for
/// the UI-side join (unknown status strings are dropped, not fatal).
fn read_back_assets(
    catalog: &Catalog,
    sources: &[(String, i64)],
) -> Result<AssetIdentityMaps, String> {
    sources
        .iter()
        .map(|(root, id)| {
            let map = catalog
                .assets_for_source(*id)
                .map_err(|e| e.to_string())?
                .into_iter()
                .filter_map(|a| {
                    AssetStatus::from_code(&a.status)
                        .map(|s| (a.rel_path, (a.asset_id, s, a.trashed, a.rating)))
                })
                .collect();
            Ok((root.clone(), map))
        })
        .collect()
}

/// Runs on the persist thread: open the catalog, upsert each source, record its crawl
/// incrementally, adopt any config blacklists (one-time M5 data step), and read back
/// the per-status counts + asset identities for exactly these sources.
fn persist_to_catalog(
    db_path: &std::path::Path,
    batches: &[PersistBatch],
) -> Result<(StatusCounts, AssetIdentityMaps, Catalog), String> {
    let mut catalog = Catalog::open(db_path).map_err(|e| e.to_string())?;
    catalog.seed_media_types().map_err(|e| e.to_string())?;

    let mut sources = Vec::with_capacity(batches.len());
    let mut blacklists = Vec::new();
    for batch in batches {
        let id = catalog
            .upsert_import_source(&batch.name, &batch.root, "folder")
            .map_err(|e| e.to_string())?;
        sources.push((batch.root.clone(), id));
        if let Some(json) = &batch.excludes_json {
            blacklists.push((batch.root.clone(), json.clone()));
        }
        let stats = catalog
            .record_crawl(id, None, &batch.records)
            .map_err(|e| e.to_string())?;
        // Files gone from disk are flagged missing (metadata preserved, hidden from
        // hydrate + grid); files that came back are unflagged.
        let present: std::collections::HashSet<String> =
            batch.records.iter().map(|r| r.rel_path.clone()).collect();
        let (gone, back) = catalog
            .reconcile_missing(id, &present)
            .map_err(|e| e.to_string())?;
        tracing::info!(
            root = %batch.root,
            inserted = stats.inserted,
            updated = stats.updated,
            unchanged = stats.unchanged,
            gone,
            back,
            "catalog sync"
        );
    }
    if !blacklists.is_empty() {
        catalog
            .adopt_config_blacklists(&blacklists)
            .map_err(|e| e.to_string())?;
    }
    let ids: Vec<i64> = sources.iter().map(|(_, id)| *id).collect();
    let counts = catalog
        .status_counts_for_sources(&ids)
        .map_err(|e| e.to_string())?;
    let assets = read_back_assets(&catalog, &sources)?;
    // Hand the open connection back so the caller can drain `embedding_dirty` on this same
    // thread — keeps the single-writer rule (ARCHITECTURE §5) with no extra writer thread.
    Ok((counts, assets, catalog))
}

/// A 128×128 radial-falloff sprite in `color`, peaking at `max_alpha` in the centre.
/// Stretched over the background for the §0 ambience gradients.
fn radial_image(color: egui::Color32, max_alpha: u8) -> egui::ColorImage {
    const N: usize = 128;
    let mut img = egui::ColorImage::new([N, N], egui::Color32::TRANSPARENT);
    let half = (N as f32 - 1.0) / 2.0;
    for y in 0..N {
        for x in 0..N {
            let dx = (x as f32 - half) / half;
            let dy = (y as f32 - half) / half;
            let d = (dx * dx + dy * dy).sqrt().min(1.0);
            let a = ((1.0 - d) * max_alpha as f32) as u8;
            img.pixels[y * N + x] =
                egui::Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a);
        }
    }
    img
}

/// One 32px sidebar navigation row: glyph + label + optional amber count badge.
/// Active shows an `--ls-overlay` fill + 2px gold left border (UI-SPEC §0.2).
fn sidebar_nav_row(
    ui: &mut Ui,
    icon: egui::ImageSource<'static>,
    icon_color: egui::Color32,
    label: &str,
    badge: Option<usize>,
    active: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
    // Row background first, so the icon draws on top of it.
    if active {
        ui.painter().rect_filled(rect, 4.0, theme::OVERLAY);
        ui.painter().rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
            0.0,
            theme::GOLD,
        );
    } else if resp.hovered() {
        ui.painter()
            .rect_filled(rect, 4.0, theme::OVERLAY.linear_multiply(0.5));
    }
    // The 16px SVG icon, tinted by row state (its own accent when active, muted otherwise).
    let tint = if active {
        icon_color
    } else {
        theme::TEXT_MUTED
    };
    let icon_rect = egui::Rect::from_center_size(
        rect.left_center() + egui::vec2(20.0, 0.0),
        Vec2::splat(16.0),
    );
    egui::Image::new(icon).tint(tint).paint_at(ui, icon_rect);
    let painter = ui.painter();
    painter.text(
        rect.left_center() + egui::vec2(36.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        if active {
            theme::TEXT
        } else {
            theme::TEXT_MUTED
        },
    );
    if let Some(n) = badge {
        let galley =
            painter.layout_no_wrap(n.to_string(), egui::FontId::monospace(10.0), theme::BG);
        let pad = egui::vec2(6.0, 2.0);
        let size = galley.size() + pad * 2.0;
        let pill = egui::Rect::from_min_size(
            egui::pos2(rect.max.x - size.x - 8.0, rect.center().y - size.y / 2.0),
            size,
        );
        painter.rect_filled(pill, size.y / 2.0, theme::WARNING);
        painter.galley(pill.min + pad, galley, theme::BG);
    }
    resp
}

/// A letter-spaced dim facet-group label (STATUS / LIBRARIES).
/// A 1px sidebar divider inset from the edges (prototype: `height:1px; background:#2C3547;
/// margin:12px 14px`), separating the nav list from the facet groups.
fn sidebar_divider(ui: &mut Ui) {
    ui.add_space(11.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        (rect.left() + 6.0)..=(rect.right() - 6.0),
        rect.center().y,
        egui::Stroke::new(1.0, theme::BORDER),
    );
    ui.add_space(9.0);
}

fn facet_group_label(ui: &mut Ui, text: &str) {
    // ponytail: egui has no letter-spacing; spaced chars approximate the design token.
    let spaced: String = text.chars().flat_map(|c| [c, ' ']).collect();
    ui.label(
        RichText::new(spaced.trim_end())
            .color(theme::TEXT_DIM)
            .size(10.0),
    );
    ui.add_space(2.0);
}

/// A 26px facet row: optional rotated status diamond + label + right-aligned mono count.
fn facet_row(
    ui: &mut Ui,
    dot: Option<egui::Color32>,
    label: &str,
    count: usize,
    active: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), egui::Sense::click());
    let painter = ui.painter();
    if active {
        painter.rect_filled(rect, 4.0, theme::OVERLAY);
        painter.rect_filled(
            egui::Rect::from_min_size(rect.left_top(), egui::vec2(2.0, rect.height())),
            0.0,
            theme::GOLD,
        );
    } else if resp.hovered() {
        painter.rect_filled(rect, 4.0, theme::OVERLAY.linear_multiply(0.5));
    }
    let mut x = rect.min.x + 12.0;
    if let Some(color) = dot {
        // "◆" is already the rotated status diamond (DESIGN-GUIDELINES §6).
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "\u{25C6}",
            egui::FontId::proportional(9.0),
            color,
        );
        x += 16.0;
    }
    painter.text(
        egui::pos2(x, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(12.0),
        if active {
            theme::TEXT
        } else {
            theme::TEXT_MUTED
        },
    );
    painter.text(
        egui::pos2(rect.max.x - 8.0, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        count.to_string(),
        egui::FontId::monospace(11.0),
        theme::TEXT_DIM,
    );
    resp
}

/// The §12 stub screen for navigable-but-unbuilt destinations. Returns true when
/// "← Back to Library" was clicked.
fn stub_screen(ui: &mut Ui, screen: Screen) -> bool {
    let mut back = false;
    ui.vertical_centered(|ui| {
        ui.add_space(ui.available_height() * 0.25);
        ui.label(
            RichText::new(screen.glyph())
                .size(52.0)
                .color(theme::TEXT_DIM),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new(screen.label())
                .color(theme::TEXT)
                .font(crate::fonts::display(24.0)),
        );
        ui.add_space(6.0);
        ui.label(
            RichText::new(screen.stub_line())
                .color(theme::TEXT_MUTED)
                .size(13.0),
        );
        ui.add_space(2.0);
        ui.label(
            RichText::new("Not wired yet.")
                .color(theme::TEXT_DIM)
                .size(11.0),
        );
        ui.add_space(12.0);
        if ui
            .button(RichText::new("\u{2190} Back to Library").color(theme::GOLD))
            .clicked()
        {
            back = true;
        }
    });
    back
}

/// A blacklist exclusion chip: tinted value + a ✕. Returns true if the ✕ was clicked (remove).
fn exclusion_chip(ui: &mut Ui, value: &str, color: egui::Color32) -> bool {
    let mut remove = false;
    egui::Frame::none()
        .fill(color.linear_multiply(0.18))
        .rounding(999.0)
        .stroke(egui::Stroke::new(1.0, color))
        .inner_margin(egui::Margin::symmetric(9.0, 3.0))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(RichText::new(value).color(color).size(11.0));
            if ui
                .add(
                    egui::Button::new(RichText::new("\u{2715}").color(color).size(10.0))
                        .frame(false),
                )
                .clicked()
            {
                remove = true;
            }
        });
    remove
}

/// Small letter-spaced section label used inside the filter popover and settings cards.
fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .size(10.0)
            .color(theme::TEXT_DIM)
            .strong(),
    );
}

/// The ⚑ Filters content-filter popover (design §3). Returns true if a filter set changed.
/// Only the FILE SIZE section is interactive today; TRAIT and TRIANGLES are shown but disabled
/// until their backing data exists (traits → P4 AI tagging, tri counts → P1 3D metadata), so a
/// user can never select a facet that would filter the grid to empty.
fn filter_popover(ui: &mut Ui, cf: &mut ContentFilter, tiers: &SizeTiers) -> bool {
    ui.set_min_width(320.0);
    let mut changed = false;

    ui.horizontal(|ui| {
        section_label(ui, "FILTER LIBRARY");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let clear = ui.add(
                egui::Button::new(
                    RichText::new("Clear all")
                        .size(10.0)
                        .color(theme::TEXT_MUTED),
                )
                .frame(false),
            );
            if clear.hovered() {
                ui.painter().rect_stroke(
                    clear.rect.expand(2.0),
                    3.0,
                    egui::Stroke::new(1.0, theme::DANGER),
                );
            }
            if clear.clicked() && cf.any_active() {
                cf.clear();
                changed = true;
            }
        });
    });
    ui.separator();

    // TRAIT — no tag data pre-catalog (P4).
    section_label(ui, "TRAIT");
    ui.add_enabled(
        false,
        egui::TextEdit::singleline(&mut cf.trait_search)
            .hint_text("search traits\u{2026}")
            .desired_width(140.0),
    );
    ui.label(
        RichText::new("No traits yet \u{2014} assets gain traits after AI identification.")
            .size(10.0)
            .color(theme::TEXT_DIM),
    );
    ui.add_space(6.0);

    // TRIANGLES — no tri-count data pre-catalog (P1); shown disabled.
    section_label(ui, "TRIANGLES");
    ui.add_enabled_ui(false, |ui| {
        ui.horizontal_wrapped(|ui| {
            for b in TriBucket::ALL {
                let sel = cf.tri_buckets.contains(&b);
                let _ = filter_chip(ui, b.label(), sel, theme::TYPE_MODEL);
            }
        });
    });
    ui.label(
        RichText::new("Available once 3D metadata is extracted.")
            .size(10.0)
            .color(theme::TEXT_DIM),
    );
    ui.add_space(6.0);

    // FILE SIZE — fully functional (every asset has a byte size).
    section_label(ui, "FILE SIZE");
    ui.horizontal_wrapped(|ui| {
        for tier in SizeTier::ALL {
            let (lo, hi) = tiers.range_kb(tier);
            let label = format!("{}  {}\u{2013}{}", tier.letter(), fmt_kb(lo), fmt_kb(hi));
            let sel = cf.sizes.contains(&tier);
            if filter_chip(ui, &label, sel, size_tier_color(tier)) {
                if sel {
                    cf.sizes.remove(&tier);
                } else {
                    cf.sizes.insert(tier);
                }
                changed = true;
            }
        }
    });
    ui.add_space(6.0);

    // RATING — minimum-stars single-select; same chip toggles off (DATA-MODEL §12).
    section_label(ui, "RATING");
    ui.horizontal_wrapped(|ui| {
        for min in 1..=5u8 {
            let label = format!("\u{2605}{min}+");
            let sel = cf.min_rating == Some(min);
            if filter_chip(ui, &label, sel, theme::GOLD) {
                cf.min_rating = if sel { None } else { Some(min) };
                changed = true;
            }
        }
    });

    changed
}

/// Returns true if the card was clicked.
fn asset_card(
    ui: &mut Ui,
    entry: &AssetEntry,
    thumb_size: f32,
    selected: bool,
    check_overlay: bool,
    tiers: &SizeTiers,
) -> egui::Response {
    let type_color = entry
        .file
        .media_type
        .map(theme::media_type_color)
        .unwrap_or(theme::TEXT_DIM);
    let border_color = if selected { theme::GOLD } else { theme::BORDER };
    let border_width = if selected { 2.0 } else { 1.0 };

    let resp = egui::Frame::none()
        .fill(theme::SURFACE)
        .rounding(6.0)
        .stroke(egui::Stroke::new(border_width, border_color))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_size(Vec2::new(thumb_size, thumb_size + CARD_LABEL));

                let thumb_rect = if let Some(tex) = &entry.texture {
                    ui.add(
                        egui::Image::new((tex.id(), Vec2::splat(thumb_size)))
                            .fit_to_exact_size(Vec2::splat(thumb_size)),
                    )
                    .rect
                } else {
                    let (resp, painter) =
                        ui.allocate_painter(Vec2::splat(thumb_size), egui::Sense::hover());
                    painter.rect_filled(resp.rect, 6.0, type_color.linear_multiply(0.12));
                    painter.text(
                        resp.rect.center(),
                        egui::Align2::CENTER_CENTER,
                        type_label_char(entry.file.media_type),
                        egui::FontId::proportional((thumb_size * 0.22).max(12.0)),
                        type_color.linear_multiply(0.6),
                    );
                    resp.rect
                };

                // S/M/L/XL size-tier badge, top-right of the thumbnail (design §3).
                let tier = tiers.tier_of(entry.file.size_bytes);
                let font = egui::FontId::proportional(9.0);
                let painter = ui.painter();
                let galley = painter.layout_no_wrap(tier.letter().into(), font.clone(), theme::BG);
                let bw = galley.size().x + 8.0;
                let top_right = thumb_rect.right_top() + egui::vec2(-3.0, 3.0);
                let badge = egui::Rect::from_min_max(
                    egui::pos2(top_right.x - bw, top_right.y),
                    egui::pos2(top_right.x, top_right.y + 14.0),
                );
                painter.rect_filled(badge, 3.0, size_tier_color(tier));
                painter.text(
                    badge.center(),
                    egui::Align2::CENTER_CENTER,
                    tier.letter(),
                    font,
                    theme::BG,
                );

                // Star rating, bottom-left of the thumbnail (DATA-MODEL §12).
                if let Some(r) = entry.rating {
                    painter.text(
                        thumb_rect.left_bottom() + egui::vec2(4.0, -3.0),
                        egui::Align2::LEFT_BOTTOM,
                        format!("\u{2605} {r}"),
                        egui::FontId::proportional(10.0),
                        theme::GOLD,
                    );
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (dot_resp, painter) =
                        ui.allocate_painter(Vec2::splat(8.0), egui::Sense::hover());
                    painter.circle_filled(dot_resp.rect.center(), 4.0, type_color);

                    let name = entry
                        .file
                        .rel_path
                        .file_stem()
                        .map(|s| s.to_string_lossy())
                        .unwrap_or_default();
                    ui.add(
                        egui::Label::new(
                            RichText::new(name.as_ref()).color(theme::TEXT).size(11.0),
                        )
                        .truncate(),
                    );
                });
            });
        });

    let resp = resp.response.interact(egui::Sense::click());
    if check_overlay {
        // Multi-select check badge, top-left (DESIGN-GUIDELINES §7 "Selection").
        let c = resp.rect.left_top() + egui::vec2(11.0, 11.0);
        let painter = ui.painter();
        painter.circle_filled(c, 8.0, theme::GOLD);
        painter.text(
            c,
            egui::Align2::CENTER_CENTER,
            "\u{2713}",
            egui::FontId::proportional(10.0),
            theme::BG,
        );
    }
    resp
}

// ── Inspector pane ────────────────────────────────────────────────────────────

/// Returns Some(new_rating) when the star row was clicked (Some(None) = cleared).
fn inspector_pane(
    ui: &mut Ui,
    file: &CrawledFile,
    status: AssetStatus,
    rating: Option<u8>,
    texture: Option<&TextureHandle>,
    library: Option<&str>,
) -> Option<Option<u8>> {
    let mut rating_clicked: Option<Option<u8>> = None;
    let type_color = file
        .media_type
        .map(theme::media_type_color)
        .unwrap_or(theme::TEXT_DIM);
    let type_name = media_type_label(file.media_type);

    // Header
    ui.horizontal(|ui| {
        let (dot, painter) = ui.allocate_painter(Vec2::splat(10.0), egui::Sense::hover());
        painter.circle_filled(dot.rect.center(), 5.0, type_color);
        let name = file
            .rel_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.add(
            egui::Label::new(RichText::new(&name).color(theme::TEXT).size(14.0).strong())
                .truncate(),
        );
    });
    ui.label(RichText::new(type_name).color(type_color).size(11.0));
    ui.add(egui::Separator::default().spacing(10.0));

    // Preview (image only for now; 3D/audio come in P1)
    if let Some(tex) = texture {
        let avail_w = ui.available_width();
        let size = Vec2::splat(avail_w.min(240.0));
        ui.add(
            egui::Image::new((tex.id(), size))
                .fit_to_exact_size(size)
                .rounding(4.0),
        );
        ui.add_space(8.0);
    }

    // Stat block
    egui::Grid::new("inspector_stats")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            stat_row(
                ui,
                "File",
                &file
                    .rel_path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
            stat_row(ui, "Size", &format_bytes(file.size_bytes));
            stat_row(ui, "Modified", &format_mtime(file.mtime));
            // Status row — user-facing terms fold discovered|indexed (DATA-MODEL §8).
            let (term, color) = if status.is_unidentified() {
                ("Unidentified", theme::STATUS_UNIDENTIFIED)
            } else if status == AssetStatus::Identified {
                ("Identified", theme::STATUS_IDENTIFIED)
            } else {
                ("Confirmed", theme::STATUS_CONFIRMED)
            };
            ui.label(RichText::new("Status").color(theme::TEXT_DIM).size(11.0));
            ui.label(
                RichText::new(format!("\u{25C6} {term}"))
                    .color(color)
                    .size(11.0),
            );
            ui.end_row();
            // Rating stars (keys 1-5 work too; clicking the current value clears).
            ui.label(RichText::new("Rating").color(theme::TEXT_DIM).size(11.0));
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 1.0;
                let current = rating.unwrap_or(0);
                for n in 1..=5u8 {
                    let (glyph, star_color) = if n <= current {
                        ("\u{2605}", theme::GOLD)
                    } else {
                        ("\u{2606}", theme::TEXT_DIM)
                    };
                    let resp = ui.add(
                        egui::Button::new(RichText::new(glyph).color(star_color).size(13.0))
                            .frame(false),
                    );
                    if resp.clicked() {
                        rating_clicked = Some(if current == n { None } else { Some(n) });
                    }
                }
            });
            ui.end_row();
            if let Some(lib) = library {
                stat_row(ui, "Library", lib);
            }
            stat_row(ui, "Path", &file.abs_path.to_string_lossy());
        });

    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Space \u{2192} Quick Look  \u{2022}  \u{2191}\u{2193}\u{2190}\u{2192} navigate  \u{2022}  1\u{2013}5 rate",
        )
        .color(theme::TEXT_DIM)
        .size(10.0),
    );
    rating_clicked
}

fn stat_row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).color(theme::TEXT_DIM).size(11.0));
    ui.add(egui::Label::new(RichText::new(value).color(theme::TEXT).size(11.0)).truncate());
    ui.end_row();
}

fn format_bytes(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} KB", b as f64 / (1 << 10) as f64),
        b => format!("{b} B"),
    }
}

fn format_mtime(mtime_secs: i64) -> String {
    // ponytail: no chrono dep — decompose epoch seconds into Y-M-D manually
    let secs = mtime_secs.max(0) as u64;
    // Days since 1970-01-01 using the proleptic Gregorian calendar.
    let days = secs / 86400;
    let mut y = 1970u64;
    let mut d = days;
    loop {
        let dy = if y % 400 == 0 || (y % 4 == 0 && y % 100 != 0) {
            366
        } else {
            365
        };
        if d < dy {
            break;
        }
        d -= dy;
        y += 1;
    }
    let months = [
        31u64,
        if y % 400 == 0 || (y % 4 == 0 && y % 100 != 0) {
            29
        } else {
            28
        },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 1u64;
    for &ml in &months {
        if d < ml {
            break;
        }
        d -= ml;
        m += 1;
    }
    format!("{y}-{m:02}-{:02}", d + 1)
}

fn media_type_label(mt: Option<MediaType>) -> &'static str {
    match mt {
        Some(MediaType::Model3d) => "3D Model",
        Some(MediaType::Texture) => "Texture",
        Some(MediaType::Audio) => "Audio",
        Some(MediaType::Image) => "Image",
        Some(MediaType::Sprite) => "Sprite",
        Some(MediaType::Material) => "Material",
        Some(MediaType::Font) => "Font",
        None => "Unknown",
    }
}

// ── Quick Look overlay ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn quick_look_content(
    ui: &mut Ui,
    name: &str,
    media_type: Option<MediaType>,
    size_bytes: u64,
    thumb: Option<&TextureHandle>,
    viewer: &mut ImageViewer,
    direct_model: bool,
    viewport: Option<&mut Viewport3d>,
    audio: Option<&mut AudioPlayer>,
) {
    let is_image = is_viewer_image(media_type);
    let is_model = direct_model && viewport.is_some();
    let is_audio = media_type == Some(MediaType::Audio) && audio.is_some();

    // Toolbar (image assets only): Fit / 1:1 / pixel-perfect toggle / zoom readout.
    if is_image {
        ui.horizontal(|ui| {
            if ui.selectable_label(viewer.zoom.is_none(), "Fit").clicked() {
                viewer.zoom = None;
                viewer.offset = Vec2::ZERO;
            }
            if ui
                .selectable_label(viewer.zoom == Some(1.0), "1:1")
                .clicked()
            {
                viewer.zoom = Some(1.0);
                viewer.offset = Vec2::ZERO;
            }
            let nn = ui
                .selectable_label(viewer.nearest, "\u{25A6} Pixel-perfect")
                .on_hover_text("Nearest-neighbour sampling \u{2014} no smoothing");
            if nn.clicked() {
                viewer.nearest = !viewer.nearest;
                // Re-upload with the new sampler; the pixels are retained for this.
                let opts = viewer.texture_options();
                if let Some(image) = viewer.image.clone() {
                    if let Some(tex) = &mut viewer.texture {
                        tex.set(image, opts);
                    }
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let [w, h] = viewer.source_dims;
                if w > 0 {
                    let pct = viewer.zoom.map(|z| format!("{:.0}%", z * 100.0));
                    ui.label(
                        RichText::new(format!(
                            "{w}\u{D7}{h}{}",
                            pct.map(|p| format!("  \u{B7}  {p}")).unwrap_or_default()
                        ))
                        .color(theme::TEXT_DIM)
                        .size(10.0)
                        .monospace(),
                    );
                }
            });
        });
        ui.add_space(4.0);
    }

    let avail = ui.available_size();
    let canvas = Vec2::new(avail.x, (avail.y - 64.0).max(96.0));

    if is_image && (viewer.texture.is_some() || viewer.loading || viewer.error.is_some()) {
        let (resp, mut painter) = ui.allocate_painter(canvas, egui::Sense::click_and_drag());
        painter.rect_filled(resp.rect, 4.0, theme::BG);

        if let Some(tex) = &viewer.texture {
            let tex_size = tex.size_vec2();
            let fit_zoom = (canvas.x / tex_size.x).min(canvas.y / tex_size.y).min(1.0);
            let zoom = viewer.zoom.unwrap_or(fit_zoom);

            // Scroll zooms around the cursor; drag pans; double-click refits.
            if resp.hovered() {
                let scroll = ui.input(|i| i.raw_scroll_delta.y);
                if scroll != 0.0 {
                    let factor = (scroll * 0.003).exp();
                    let new_zoom = (zoom * factor).clamp(0.02, 32.0);
                    if let Some(pointer) = resp.hover_pos() {
                        let rel = pointer - resp.rect.center() - viewer.offset;
                        viewer.offset += rel - rel * (new_zoom / zoom);
                    }
                    viewer.zoom = Some(new_zoom);
                }
            }
            if resp.dragged() {
                viewer.offset += resp.drag_delta();
            }
            if resp.double_clicked() {
                viewer.zoom = None;
                viewer.offset = Vec2::ZERO;
            }
            let zoom = viewer.zoom.unwrap_or(fit_zoom);

            let draw_size = tex_size * zoom;
            let rect = egui::Rect::from_center_size(resp.rect.center() + viewer.offset, draw_size);
            painter.set_clip_rect(resp.rect);
            painter.image(
                tex.id(),
                rect,
                egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else if viewer.loading {
            // Blurry-but-instant: the grid thumbnail as a stand-in while decoding.
            if let Some(tex) = thumb {
                let side = canvas.min_elem() * 0.6;
                painter.image(
                    tex.id(),
                    egui::Rect::from_center_size(resp.rect.center(), Vec2::splat(side)),
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
            painter.text(
                resp.rect.center_bottom() + egui::vec2(0.0, -10.0),
                egui::Align2::CENTER_BOTTOM,
                "\u{25CC} loading full detail\u{2026}",
                egui::FontId::proportional(11.0),
                theme::ARCANE,
            );
        } else if let Some(err) = &viewer.error {
            painter.text(
                resp.rect.center(),
                egui::Align2::CENTER_CENTER,
                format!("\u{2298} couldn't decode: {err}"),
                egui::FontId::proportional(11.0),
                theme::DANGER,
            );
        }
    } else if is_model {
        // 3D orbit viewport (issue #29 item 3) — three-d on the shared GL context.
        let vp = viewport.expect("is_model implies viewport");
        if vp.has_content() {
            vp.ui(ui, canvas);
        } else {
            let (resp, painter) = ui.allocate_painter(canvas, egui::Sense::hover());
            painter.rect_filled(resp.rect, 4.0, theme::BG);
            if vp.loading {
                if let Some(tex) = thumb {
                    let side = canvas.min_elem() * 0.6;
                    painter.image(
                        tex.id(),
                        egui::Rect::from_center_size(resp.rect.center(), Vec2::splat(side)),
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                painter.text(
                    resp.rect.center_bottom() + egui::vec2(0.0, -10.0),
                    egui::Align2::CENTER_BOTTOM,
                    "\u{25CC} loading mesh\u{2026}",
                    egui::FontId::proportional(11.0),
                    theme::ARCANE,
                );
            } else if let Some(err) = &vp.error {
                painter.text(
                    resp.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    format!("\u{2298} couldn't load model: {err}"),
                    egui::FontId::proportional(11.0),
                    theme::DANGER,
                );
            }
        }
    } else if is_audio {
        // Audio (prototype §"Quick Look → sound"): waveform + transport together in one
        // #141926 sound recess.
        let ap = audio.expect("is_audio implies audio");
        let dur = ap.meta.as_ref().map_or(0.0, |m| m.duration);

        egui::Frame::none()
            .fill(theme::RECESS)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .rounding(8.0)
            .inner_margin(egui::Margin::symmetric(16.0, 14.0))
            .show(ui, |ui| {
                // Waveform — fixed height (an available-derived height feeds the
                // auto-sizing window and creeps taller each frame).
                let wave = Vec2::new(ui.available_width(), 120.0);
                let (resp, painter) = ui.allocate_painter(wave, egui::Sense::click());
                if let Some(meta) = &ap.meta {
                    draw_waveform(
                        &painter,
                        resp.rect,
                        &meta.peaks,
                        ap.position(),
                        meta.duration,
                    );
                    if resp.clicked() {
                        if let Some(p) = resp.interact_pointer_pos() {
                            let frac =
                                ((p.x - resp.rect.left()) / resp.rect.width()).clamp(0.0, 1.0);
                            ap.seek(frac * meta.duration);
                        }
                    }
                } else {
                    let (msg, color) = if ap.loading {
                        (
                            "\u{25CC} decoding waveform\u{2026}".to_string(),
                            theme::ARCANE,
                        )
                    } else if let Some(err) = &ap.error {
                        (format!("\u{2298} couldn't decode: {err}"), theme::DANGER)
                    } else {
                        (String::new(), theme::TEXT_DIM)
                    };
                    if !msg.is_empty() {
                        painter.text(
                            resp.rect.center(),
                            egui::Align2::CENTER_CENTER,
                            msg,
                            egui::FontId::proportional(11.0),
                            color,
                        );
                    }
                }
                ui.add_space(14.0);
                // Row 1 — transport: play · timeline scrub · time (prototype's sound row).
                ui.horizontal(|ui| {
                    let glyph = if ap.is_playing() {
                        "\u{23F8}"
                    } else {
                        "\u{25B6}"
                    };
                    let play = ui.add(
                        egui::Button::new(
                            RichText::new(glyph).color(theme::ARCANE_BRIGHT).size(15.0),
                        )
                        .min_size(Vec2::splat(38.0))
                        .rounding(19.0)
                        .fill(theme::ARCANE.linear_multiply(0.14))
                        .stroke(egui::Stroke::new(1.0, theme::ARCANE)),
                    );
                    if play.clicked() {
                        ap.toggle();
                    }
                    ui.add_space(8.0);
                    // Fixed scrub width — sizing it from available_width feeds the
                    // auto-sizing window and creeps wider each frame (as the fixed
                    // waveform height fixed the vertical creep).
                    if let Some(t) = seek_bar(ui, 360.0, ap.position(), dur) {
                        ap.seek(t);
                    }
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!(
                            "{} / {}",
                            crate::audio::fmt_time(ap.position()),
                            crate::audio::fmt_time(dur)
                        ))
                        .monospace()
                        .color(theme::TEXT_MUTED)
                        .size(11.0),
                    );
                });
                ui.add_space(7.0);
                // Row 2 — volume control (left) + audio facts (right).
                ui.horizontal(|ui| {
                    let mut vol = ap.volume();
                    if volume_control(ui, &mut vol) {
                        ap.set_volume(vol);
                    }
                    if let Some(meta) = &ap.meta {
                        let chans = match meta.channels {
                            1 => "mono".to_string(),
                            2 => "stereo".to_string(),
                            n => format!("{n} ch"),
                        };
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{:.1} kHz \u{2022} {chans}",
                                    meta.sample_rate as f32 / 1000.0
                                ))
                                .color(theme::TEXT_DIM)
                                .size(10.0),
                            );
                        });
                    }
                });
            });
    } else {
        // Remaining non-previewable types keep the typed placeholder card
        // (sprite sheets / material spheres / font specimens arrive per issue #29).
        let preview_size = canvas.min_elem().max(64.0);
        ui.vertical_centered(|ui| {
            if let Some(tex) = thumb {
                ui.add(
                    egui::Image::new((tex.id(), Vec2::splat(preview_size)))
                        .fit_to_exact_size(Vec2::splat(preview_size))
                        .rounding(4.0),
                );
            } else {
                let type_color = media_type
                    .map(theme::media_type_color)
                    .unwrap_or(theme::TEXT_DIM);
                let (resp, painter) =
                    ui.allocate_painter(Vec2::splat(preview_size), egui::Sense::hover());
                painter.rect_filled(resp.rect, 8.0, type_color.linear_multiply(0.12));
                painter.text(
                    resp.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    type_label_char(media_type),
                    egui::FontId::proportional(preview_size * 0.18),
                    type_color.linear_multiply(0.7),
                );
            }
        });
    }

    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(name).color(theme::TEXT).size(15.0).strong());
        ui.label(
            RichText::new(format!(
                "{} \u{2022} {}",
                media_type_label(media_type),
                format_bytes(size_bytes),
            ))
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.label(
            RichText::new(if is_image {
                "scroll zoom \u{2022} drag pan \u{2022} double-click fit \u{2022} \u{2190}\u{2192} navigate \u{2022} Esc close"
            } else if is_model {
                "drag orbit \u{2022} right-drag pan \u{2022} scroll zoom \u{2022} double-click reframe \u{2022} Esc close"
            } else if is_audio {
                "\u{25B6} play/pause \u{2022} click waveform to seek \u{2022} \u{2190}\u{2192} navigate \u{2022} Esc close"
            } else {
                "Space / Esc \u{2192} close  \u{2022}  \u{2190}\u{2192} navigate"
            })
            .color(theme::TEXT_DIM)
            .size(10.0),
        );
    });
}

/// Paint a peak waveform in `rect`: centered vertical bars, the played portion in the
/// bright sound token and the rest dimmed, plus a playhead line at `pos/dur`.
/// Chunky rounded peak bars (prototype: 8px rounded bars, gap, played vs unplayed color).
/// The 256 source peaks are aggregated down to however many bars fit; played bars take the
/// bright sound token, the rest the base one — position is read from the color split, no
/// separate playhead line (matches the prototype).
fn draw_waveform(painter: &egui::Painter, rect: egui::Rect, peaks: &[f32], pos: f32, dur: f32) {
    if peaks.is_empty() {
        return;
    }
    let played = theme::ARCANE_BRIGHT; // #7BF0E5
    let ahead = theme::TYPE_SOUND; // #49C9E0
    let mid = rect.center().y;
    let half = rect.height() * 0.46;
    let (bar_w, gap) = (4.0f32, 3.0f32);
    let step = bar_w + gap;
    let n = (((rect.width() + gap) / step).floor() as usize).max(1);
    let play_frac = if dur > 0.0 {
        (pos / dur).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let src = peaks.len();
    for i in 0..n {
        // Aggregate the source peaks covering this display bar (peak of the window).
        let lo = i * src / n;
        let hi = (((i + 1) * src / n).max(lo + 1)).min(src);
        let p = peaks[lo..hi].iter().copied().fold(0.0f32, f32::max);
        let x = rect.left() + i as f32 * step + bar_w * 0.5;
        let h = (p * half).max(1.5);
        let frac = (i as f32 + 0.5) / n as f32;
        let color = if frac <= play_frac { played } else { ahead };
        let bar = egui::Rect::from_center_size(egui::pos2(x, mid), Vec2::new(bar_w, 2.0 * h));
        painter.rect_filled(bar, bar_w * 0.5, color);
    }
}

/// A contained volume control: a pill holding a speaker icon, a filled level bar, and a
/// percentage. Returns true when the value changed. The prototype has no volume control, so
/// this follows the design's rounded-track + contained-panel language (JRPG chrome §4).
fn volume_control(ui: &mut Ui, value: &mut f32) -> bool {
    let mut changed = false;
    egui::Frame::none()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .rounding(999.0)
        .inner_margin(egui::Margin::symmetric(9.0, 4.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                let (icon_rect, _) =
                    ui.allocate_exact_size(Vec2::new(13.0, 13.0), egui::Sense::hover());
                draw_speaker(ui.painter(), icon_rect, *value, theme::TEXT_MUTED);
                changed = volume_bar(ui, value);
                ui.label(
                    RichText::new(format!("{:.0}%", (*value * 100.0).round()))
                        .monospace()
                        .color(theme::TEXT_DIM)
                        .size(10.0),
                );
            });
        });
    changed
}

/// Timeline scrub bar (prototype's transport `<input type=range>`): a `width`-wide gold
/// progress track with a knob at `pos/dur`. Returns `Some(secs)` when clicked/dragged to
/// seek. Gold matches the waveform playhead so the two read as the same position.
fn seek_bar(ui: &mut Ui, width: f32, pos: f32, dur: f32) -> Option<f32> {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 8.0), egui::Sense::click_and_drag());
    let painter = ui.painter();
    let r = rect.height() / 2.0;
    painter.rect_filled(rect, r, theme::BORDER_STRONG);
    let frac = if dur > 0.0 {
        (pos / dur).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if frac > 0.0 {
        let filled =
            egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * frac, rect.height()));
        painter.rect_filled(filled, r, theme::GOLD);
    }
    let kx = rect.left() + rect.width() * frac;
    painter.circle_filled(
        egui::pos2(kx, rect.center().y),
        rect.height() * 0.85,
        theme::GOLD_BRIGHT,
    );
    if (resp.dragged() || resp.clicked()) && dur > 0.0 && rect.width() > 0.0 {
        if let Some(p) = resp.interact_pointer_pos() {
            let frac = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            return Some(frac * dur);
        }
    }
    None
}

/// A filled, draggable level bar (rounded track + arcane fill + a bright knob) — the
/// "more visible bar" the default egui slider wasn't. Click or drag anywhere to set.
fn volume_bar(ui: &mut Ui, value: &mut f32) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(84.0, 8.0), egui::Sense::click_and_drag());
    let painter = ui.painter();
    let r = rect.height() / 2.0;
    let v = value.clamp(0.0, 1.0);
    painter.rect_filled(rect, r, theme::BORDER_STRONG);
    if v > 0.0 {
        let filled =
            egui::Rect::from_min_size(rect.min, Vec2::new(rect.width() * v, rect.height()));
        painter.rect_filled(filled, r, theme::ARCANE);
    }
    let kx = rect.left() + rect.width() * v;
    painter.circle_filled(
        egui::pos2(kx, rect.center().y),
        rect.height() * 0.85,
        theme::ARCANE_BRIGHT,
    );
    let mut changed = false;
    if (resp.dragged() || resp.clicked()) && rect.width() > 0.0 {
        if let Some(p) = resp.interact_pointer_pos() {
            *value = ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            changed = true;
        }
    }
    changed
}

/// Draw a small speaker glyph (cone + level-dependent sound waves) filling `rect`.
fn draw_speaker(painter: &egui::Painter, rect: egui::Rect, level: f32, color: egui::Color32) {
    let s = rect.size().min_elem();
    let o = rect.min;
    let p = |fx: f32, fy: f32| egui::pos2(o.x + fx * s, o.y + fy * s);
    // Cone silhouette (back plate + flared front).
    let cone = vec![
        p(0.06, 0.38),
        p(0.26, 0.38),
        p(0.48, 0.16),
        p(0.48, 0.84),
        p(0.26, 0.62),
        p(0.06, 0.62),
    ];
    painter.add(egui::Shape::convex_polygon(cone, color, egui::Stroke::NONE));
    // Sound waves — one at low volume, two above half, none when muted.
    let stroke = egui::Stroke::new(1.4, color);
    if level > 0.02 {
        painter.line_segment([p(0.60, 0.34), p(0.60, 0.66)], stroke);
    }
    if level > 0.5 {
        painter.line_segment([p(0.74, 0.24), p(0.74, 0.76)], stroke);
    }
}

fn type_label_char(mt: Option<MediaType>) -> &'static str {
    match mt {
        Some(MediaType::Model3d) => "3D",
        Some(MediaType::Texture) => "TX",
        Some(MediaType::Audio) => "\u{266A}",
        Some(MediaType::Image) => "IMG",
        Some(MediaType::Sprite) => "SP",
        Some(MediaType::Material) => "MAT",
        Some(MediaType::Font) => "A",
        None => "?",
    }
}

// ── Background thumbnail helpers ──────────────────────────────────────────────

fn needs_thumbnail(mt: Option<MediaType>) -> bool {
    matches!(
        mt,
        Some(MediaType::Image | MediaType::Texture | MediaType::Sprite)
    )
}

fn generate_texture(
    ctx: &egui::Context,
    abs_path: &std::path::Path,
    tex_name: &str,
    size_bytes: u64,
    mtime: i64,
    cache_dir: Option<&std::path::Path>,
    gen_px: u32,
) -> Option<TextureHandle> {
    // Serve from / populate the on-disk thumbnail cache (INGESTION.md §6): restarts then
    // decode cached PNGs instead of full-size originals. The cache filename embeds WxH
    // (`thumb::cache_path`), so each on-screen-quality size gets its own cache entry. Falls back
    // to a direct decode when the cache dir is unavailable or generation fails (exotic format).
    let img = cache_dir
        .and_then(|dir| {
            let key = thumb::meta_key(abs_path, size_bytes, mtime);
            let params = thumb::ThumbParams {
                width: gen_px,
                height: gen_px,
            };
            thumb::generate(abs_path, dir, &key, params).ok()
        })
        .and_then(|cached| image::open(cached).ok())
        .or_else(|| image::open(abs_path).ok())?;
    let img = img.thumbnail(gen_px, gen_px).to_rgba8();
    let (w, h) = img.dimensions();
    let color_img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img);
    Some(ctx.load_texture(tex_name, color_img, egui::TextureOptions::LINEAR))
}

// ── Tests ─────────────────────────────────────────────────────────────────────
// Pure layout/index math only — the render loop and egui Ui are not unit-tested.
// Spec: TECH-STACK.md §5.1 (lodestash-ui row: "rebuild_cache order, thumbnail_grid column math").

#[cfg(test)]
mod tests {
    use super::*;

    // ── STATUS facet predicate (UI-SPEC §0.2, DATA-MODEL §8) ──────────────────

    #[test]
    fn status_filter_folds_discovered_and_indexed_into_unidentified() {
        use AssetStatus::*;
        for s in [Discovered, Indexed, Identified, Confirmed] {
            assert!(StatusFilter::All.matches(s));
        }
        assert!(StatusFilter::Unidentified.matches(Discovered));
        assert!(StatusFilter::Unidentified.matches(Indexed));
        assert!(!StatusFilter::Unidentified.matches(Identified));
        assert!(StatusFilter::Identified.matches(Identified));
        assert!(!StatusFilter::Identified.matches(Confirmed));
        assert!(StatusFilter::Confirmed.matches(Confirmed));
        assert!(!StatusFilter::Confirmed.matches(Discovered));
    }

    // ── Packages screen grouping (UI-SPEC §4) ─────────────────────────────────

    #[test]
    fn publisher_runs_groups_contiguous_publishers() {
        let pkg = |publisher: &str, name: &str| AssetStorePackage {
            publisher: publisher.into(),
            category: String::new(),
            name: name.into(),
            path: PathBuf::from(name),
            size_bytes: 1,
        };
        assert!(publisher_runs(&[]).is_empty());
        let pkgs = [
            pkg("SICS", "a"),
            pkg("SICS", "b"),
            pkg("Synty", "c"),
            pkg("Zerin", "d"),
        ];
        assert_eq!(publisher_runs(&pkgs), vec![0..2, 2..3, 3..4]);
    }

    // ── Content filter predicate (design §3) ──────────────────────────────────

    #[test]
    fn content_filter_empty_passes_everything() {
        let cf = ContentFilter::default();
        let tiers = SizeTiers::default();
        assert!(
            cf.passes(10, None, None, &[], &tiers),
            "no active facet = no constraint"
        );
        assert!(cf.passes(9_000_000, Some(5000), None, &["a".into()], &tiers));
    }

    #[test]
    fn content_filter_size_tier_is_and_over_the_selected_set() {
        let tiers = SizeTiers::default(); // S<1MB, M<4MB, L<8MB, XL≥8MB
        let mut cf = ContentFilter::default();
        cf.sizes.insert(SizeTier::S);
        cf.sizes.insert(SizeTier::L);
        let mb = |n: u64| n * 1024 * 1024;
        assert!(
            cf.passes(mb(0), None, None, &[], &tiers),
            "500 KB → S, selected"
        );
        assert!(
            !cf.passes(mb(2), None, None, &[], &tiers),
            "2 MB → M, not selected"
        );
        assert!(
            cf.passes(mb(5), None, None, &[], &tiers),
            "5 MB → L, selected"
        );
        assert!(
            !cf.passes(mb(9), None, None, &[], &tiers),
            "9 MB → XL, not selected"
        );
    }

    #[test]
    fn content_filter_tri_bucket_excludes_assets_without_a_count() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter::default();
        cf.tri_buckets.insert(TriBucket::K1to3);
        assert!(
            cf.passes(10, Some(2000), None, &[], &tiers),
            "2000 tris → 1k–3k, selected"
        );
        assert!(
            !cf.passes(10, Some(500), None, &[], &tiers),
            "500 tris → ≤1k, not selected"
        );
        assert!(
            !cf.passes(10, None, None, &[], &tiers),
            "no tri count is excluded when a tri facet is active"
        );
    }

    #[test]
    fn content_filter_trait_matches_any_selected() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter::default();
        cf.traits.insert("rusty".into());
        assert!(cf.passes(10, None, None, &["rusty".into(), "barrel".into()], &tiers));
        assert!(!cf.passes(10, None, None, &["barrel".into()], &tiers));
        assert!(
            !cf.passes(10, None, None, &[], &tiers),
            "an asset with no traits fails a trait filter"
        );
    }

    #[test]
    fn tri_bucket_boundaries() {
        assert_eq!(TriBucket::of(0), TriBucket::Le1k);
        assert_eq!(TriBucket::of(1000), TriBucket::Le1k);
        assert_eq!(TriBucket::of(1001), TriBucket::K1to3);
        assert_eq!(TriBucket::of(3000), TriBucket::K1to3);
        assert_eq!(TriBucket::of(6001), TriBucket::Ge6k);
    }

    #[test]
    fn content_filter_active_count_and_clear() {
        let mut cf = ContentFilter::default();
        cf.sizes.insert(SizeTier::S);
        cf.tri_buckets.insert(TriBucket::Le1k);
        assert_eq!(cf.active_count(), 2);
        assert!(cf.any_active());
        cf.clear();
        assert!(!cf.any_active());
    }

    #[test]
    fn grid_columns_is_never_zero() {
        // Even an absurdly narrow pane yields at least one column (no div-by-zero downstream).
        assert_eq!(grid_columns(0.0, 128.0), 1);
        assert_eq!(grid_columns(10.0, 128.0), 1);
        assert_eq!(grid_columns(-50.0, 128.0), 1);
    }

    #[test]
    fn grid_columns_scales_with_width() {
        // CARD_PAD is added to both available and the divisor: n cards of `size` + (n-1) pads fit.
        // width for exactly 4 columns at size 100, pad 8 = 4*100 + 3*8 = 424; formula uses +pad.
        assert_eq!(grid_columns(100.0, 100.0), 1);
        assert_eq!(grid_columns(208.0, 100.0), 2); // (208+8)/(108) = 2.0
        assert!(grid_columns(1000.0, 100.0) >= 9);
    }

    fn entry(src: usize, rel: &str, mt: Option<MediaType>) -> AssetEntry {
        AssetEntry {
            package: None,
            file: CrawledFile {
                abs_path: PathBuf::from(rel),
                rel_path: PathBuf::from(rel),
                size_bytes: 1,
                mtime: 0,
                media_type: mt,
            },
            source_idx: src,
            asset_id: None,
            status: AssetStatus::Discovered,
            trashed: false,
            rating: None,
            texture: None,
            thumb_spawned: false,
        }
    }

    #[test]
    fn folder_children_splits_dirs_and_files() {
        let entries = vec![
            entry(0, "a.png", Some(MediaType::Image)),
            entry(0, "PolygonCity/tex.png", Some(MediaType::Image)),
            entry(0, "PolygonCity/Models/m.fbx", Some(MediaType::Model3d)),
            entry(1, "other.png", Some(MediaType::Image)), // different source — excluded
        ];
        let (idxs, tiles) = folder_children(&entries, 0, std::path::Path::new(""), Category::All);
        assert_eq!(idxs, vec![0], "only the direct-child file at the root");
        assert_eq!(
            tiles,
            vec![("PolygonCity".to_owned(), 2)],
            "subtree count is recursive"
        );

        let (idxs, tiles) = folder_children(
            &entries,
            0,
            std::path::Path::new("PolygonCity"),
            Category::All,
        );
        assert_eq!(idxs, vec![1]);
        assert_eq!(tiles, vec![("Models".to_owned(), 1)]);
    }

    #[test]
    fn folder_children_respects_tab_filter() {
        let entries = vec![
            entry(0, "PolygonCity/tex.png", Some(MediaType::Image)),
            entry(0, "PolygonCity/m.fbx", Some(MediaType::Model3d)),
        ];
        let (idxs, tiles) =
            folder_children(&entries, 0, std::path::Path::new(""), Category::Models);
        assert!(idxs.is_empty());
        assert_eq!(
            tiles,
            vec![("PolygonCity".to_owned(), 1)],
            "tile counts are tab-filtered"
        );
    }

    #[test]
    fn keep_range_widens_and_clamps() {
        // Interior window: 1.5x the 6 visible rows (=9) kept each side, in entries.
        let (s, e) = keep_range(100, 106, 10, 10_000);
        assert_eq!(s, (100 - 9) * 10);
        assert_eq!(e, (106 + 9) * 10);
        // Tiny viewport falls back to the floor.
        let (s, e) = keep_range(100, 101, 10, 10_000);
        assert_eq!(s, (100 - KEEP_ROWS_MIN) * 10);
        assert_eq!(e, (101 + KEEP_ROWS_MIN) * 10);
        // Top of a short list: clamps to [0, len].
        assert_eq!(keep_range(0, 2, 10, 30), (0, 30));
        // Empty list never panics.
        assert_eq!(keep_range(0, 0, 1, 0), (0, 0));
    }

    #[test]
    fn lookahead_window_clamps_to_bounds() {
        // Top of list: cannot go below 0. 5 visible → buf 2 (floor).
        assert_eq!(lookahead_window(0, 5, 100), (0, 7));
        // Middle: half the 10 visible rows (=5) each side → ~2x visible loading.
        assert_eq!(lookahead_window(20, 30, 100), (15, 35));
        // Bottom: end cannot exceed total_rows.
        assert_eq!(lookahead_window(95, 100, 100), (93, 100));
    }

    fn entry_full(rel: &str, size: u64, mtime: i64, mt: Option<MediaType>) -> AssetEntry {
        AssetEntry {
            package: None,
            file: CrawledFile {
                abs_path: PathBuf::from(rel),
                rel_path: PathBuf::from(rel),
                size_bytes: size,
                mtime,
                media_type: mt,
            },
            source_idx: 0,
            asset_id: None,
            status: AssetStatus::Discovered,
            trashed: false,
            rating: None,
            texture: None,
            thumb_spawned: false,
        }
    }

    #[test]
    fn sort_indices_by_name_is_case_insensitive_and_reversible() {
        let entries = vec![
            entry_full("banana.png", 1, 1, Some(MediaType::Image)),
            entry_full("Apple.png", 1, 1, Some(MediaType::Image)),
            entry_full("cherry.png", 1, 1, Some(MediaType::Image)),
        ];
        let mut idxs = vec![0, 1, 2];
        sort_indices(&mut idxs, &entries, SortBy::Name, true, None);
        assert_eq!(
            idxs,
            vec![1, 0, 2],
            "Apple < banana < cherry, ignoring case"
        );

        sort_indices(&mut idxs, &entries, SortBy::Name, false, None);
        assert_eq!(idxs, vec![2, 0, 1], "descending reverses the same order");
    }

    #[test]
    fn sort_indices_by_size_and_modified() {
        let entries = vec![
            entry_full("a", 300, 3, None),
            entry_full("b", 100, 1, None),
            entry_full("c", 200, 2, None),
        ];
        let mut idxs = vec![0, 1, 2];
        sort_indices(&mut idxs, &entries, SortBy::Size, true, None);
        assert_eq!(idxs, vec![1, 2, 0], "ascending by size_bytes");

        let mut idxs = vec![0, 1, 2];
        sort_indices(&mut idxs, &entries, SortBy::Modified, true, None);
        assert_eq!(idxs, vec![1, 2, 0], "ascending by mtime");
    }

    #[test]
    fn sort_indices_by_type_groups_none_last() {
        // type_key maps None (unclassified) to 255 — sinks to the end ascending.
        let entries = vec![
            entry_full("a", 1, 1, None),
            entry_full("b", 1, 1, Some(MediaType::Audio)),
            entry_full("c", 1, 1, Some(MediaType::Model3d)),
        ];
        let mut idxs = vec![0, 1, 2];
        sort_indices(&mut idxs, &entries, SortBy::Type, true, None);
        assert_eq!(
            idxs.last(),
            Some(&0),
            "unclassified entries sink to the end"
        );
    }

    #[test]
    fn sort_indices_by_relevance_orders_by_bm25_rank() {
        // Three catalogued entries; ranks say #2 is the best match, #0 next, #1 unranked.
        let mut entries = vec![
            entry_full("alpha.png", 1, 1, Some(MediaType::Image)),
            entry_full("bravo.png", 1, 1, Some(MediaType::Image)),
            entry_full("charlie.png", 1, 1, Some(MediaType::Image)),
        ];
        entries[0].asset_id = Some(10);
        entries[1].asset_id = Some(20);
        entries[2].asset_id = Some(30);
        let ranks: std::collections::HashMap<i64, usize> = [(30, 0), (10, 1)].into_iter().collect(); // 20 is unranked

        let mut idxs = vec![0, 1, 2];
        sort_indices(&mut idxs, &entries, SortBy::Relevance, true, Some(&ranks));
        // charlie (rank 0) then alpha (rank 1) then bravo (unranked, name-last).
        assert_eq!(idxs, vec![2, 0, 1], "ranked entries lead in BM25 order");

        // No ranks → pure name order (the no-query fallback).
        let mut idxs = vec![2, 1, 0];
        sort_indices(&mut idxs, &entries, SortBy::Relevance, true, None);
        assert_eq!(idxs, vec![0, 1, 2], "no query → name order");
    }

    use proptest::prelude::*;

    proptest! {
        /// For arbitrary visible-row windows and library sizes, `keep_range` must always
        /// return a valid, in-bounds slice range — the eviction code indexes `sorted_cache`
        /// with it directly, so an out-of-bounds or inverted range would panic.
        #[test]
        fn keep_range_is_always_a_valid_slice_bound(
            row_start in 0usize..2000,
            extra in 0usize..50,
            cols in 1usize..64,
            len in 0usize..1_000_000,
        ) {
            let row_end = row_start + extra;
            let (start, end) = keep_range(row_start, row_end, cols, len);
            prop_assert!(start <= end);
            prop_assert!(end <= len);
        }
    }

    /// Build a minimal real `.unitypackage`: one GUID folder with `asset`/`pathname` (no
    /// `.meta`, not needed for this test).
    fn build_test_package(
        dir: &std::path::Path,
        name: &str,
        pathname: &str,
        asset_bytes: &[u8],
    ) -> PathBuf {
        let pkg_path = dir.join(name);
        let file = std::fs::File::create(&pkg_path).unwrap();
        let gz = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(gz);

        let mut append = |member: &str, bytes: &[u8]| {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, format!("guid0/{member}"), bytes)
                .unwrap();
        };
        append("asset", asset_bytes);
        append("pathname", pathname.as_bytes());

        builder.into_inner().unwrap().finish().unwrap();
        pkg_path
    }

    /// B1+B2 wiring: browse captures locators, `extractable` gates the pickable rows,
    /// and a surgical extraction from those locators yields a namespaced grid entry.
    #[test]
    fn pending_package_locators_drive_surgical_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let pkg_path = build_test_package(
            dir.path(),
            "Props.unitypackage",
            "Assets/Props/rock.fbx",
            b"FBX_DATA",
        );
        let file = CrawledFile {
            abs_path: pkg_path,
            rel_path: PathBuf::from("Props.unitypackage"),
            size_bytes: 0,
            mtime: 0,
            media_type: None,
        };
        let pkg = resolve_pending_package(0, &file).expect("manifest parses");
        assert_eq!(pkg.entries.len(), 1, "one locator captured at browse");
        let picks: Vec<usize> = pkg.extractable().collect();
        assert_eq!(picks, vec![0], "the fbx is pickable");

        let e = &pkg.entries[0];
        let assets = unitypackage::extract_at_offsets(
            &pkg.abs_path,
            &dir.path().join("ws"),
            vec![unitypackage::EntryLocator {
                guid: e.guid.clone(),
                pathname: e.pathname.clone(),
                offset: e.tar_offset.unwrap() as u64,
                size: e.size_bytes.unwrap() as u64,
            }],
        )
        .expect("surgical extraction succeeds");
        let files = extracted_to_crawled(assets, &pkg.display_name);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].rel_path,
            PathBuf::from("Props").join("Assets/Props/rock.fbx")
        );
        assert_eq!(std::fs::read(&files[0].abs_path).unwrap(), b"FBX_DATA");
    }

    #[test]
    fn resolve_and_extract_nested_wrapper_package_end_to_end() {
        // Two-level nesting, same shape as a real render-pipeline-variant wrapper: an outer
        // package whose only payload is itself a `.unitypackage` (the URP variant), which in
        // turn contains the real asset.
        let dir = tempfile::tempdir().unwrap();
        let inner_bytes = std::fs::read(build_test_package(
            dir.path(),
            "inner.unitypackage",
            "Assets/Pack/model.fbx",
            b"FBX_DATA",
        ))
        .unwrap();
        let outer_path = build_test_package(
            dir.path(),
            "outer.unitypackage",
            "Assets/Pack/URP_Pack_1.0.unitypackage",
            &inner_bytes,
        );

        let file = CrawledFile {
            abs_path: outer_path.clone(),
            rel_path: PathBuf::from("Pack.unitypackage"),
            size_bytes: std::fs::metadata(&outer_path).unwrap().len(),
            mtime: 0,
            media_type: None,
        };
        let pkg = resolve_pending_package(0, &file).expect("manifest should parse");
        assert_eq!(pkg.display_name, "Pack");
        assert_eq!(pkg.nested_candidates.len(), 1);
        assert_eq!(pkg.chosen_variant, 0, "sole candidate is auto-chosen");

        let workspace = dir.path().join("workspace");
        let nested_pathname = pkg.nested_candidates[pkg.chosen_variant].1.clone();
        let files = extract_package_contents(
            &pkg.abs_path,
            &workspace,
            Some(&nested_pathname),
            &pkg.display_name,
            &JobProgress::default(),
        );

        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].rel_path,
            PathBuf::from("Pack").join("Assets/Pack/model.fbx")
        );
        assert_eq!(files[0].media_type, Some(MediaType::Model3d));
        assert_eq!(files[0].size_bytes, b"FBX_DATA".len() as u64);
        assert_eq!(std::fs::read(&files[0].abs_path).unwrap(), b"FBX_DATA");
    }
}
