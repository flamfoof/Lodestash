//! Main application state and the eframe::App impl.
//! Spec: DESIGN-GUIDELINES.md §4, §7, §9; ARCHITECTURE.md §6.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use egui::{Key, RichText, ScrollArea, TextureHandle, Ui, Vec2};
use lodestash_core::catalog::{
    Catalog, CrawlRecord, HydratedAsset, InPackClass, PackageEntryRecord, StatusCounts,
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
    Materials,
    Sounds,
    Images,
}

impl Category {
    const TABS: &'static [(Category, &'static str)] = &[
        (Category::All, "\u{25C6} All"),
        (Category::Models, "\u{2B21} Models"),
        (Category::Materials, "\u{25C7} Materials"),
        (Category::Sounds, "\u{266A} Sounds"),
        (Category::Images, "\u{25A6} Images"),
    ];

    /// Bucket → `media_type` mapping: Models = model_3d, Materials = material, Sounds = audio,
    /// Images = image+texture+sprite+font (UI-SPEC §0.3). Materials used to ride along with
    /// Models; they carry their own texture dependencies, so they are worth reaching directly.
    fn matches(self, mt: Option<MediaType>) -> bool {
        match self {
            Category::All => true,
            Category::Models => mt == Some(MediaType::Model3d),
            Category::Materials => mt == Some(MediaType::Material),
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
            Category::Materials => theme::TYPE_MATERIAL,
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
    /// Technical facets extracted from the file (DATA-MODEL §14); all-None until the
    /// extraction drain has reached this asset.
    metrics: lodestash_core::catalog::AssetMetrics,
    /// BLAKE3 hash of the primary file; exact-dedup key for the Duplicates badge.
    content_hash: Option<String>,
    texture: Option<TextureHandle>,
    thumb_spawned: bool,
}

/// A child-folder tile in Folders view: its name, the count of assets **directly** inside it, and
/// the **recursive** total including everything in its subfolders (shown as `direct (recursive)`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct FolderTile {
    name: String,
    direct: usize,
    recursive: usize,
}

struct ThumbReady {
    index: usize,
    /// None when generate_texture fails; sent so pending_thumbs is always decremented.
    texture: Option<TextureHandle>,
}

/// Result of a background crawl. `generation` guards against a stale scan landing
/// after the user has already opened a different folder/project.
/// `partial` is `Some(source_idx)` when only one source was rescanned; the handler merges
/// those entries into the existing grid instead of replacing everything.
struct ScanDone {
    generation: u64,
    /// (source index into scan_roots, crawled file)
    entries: Vec<(usize, CrawledFile)>,
    pending_packages: Vec<PendingPackage>,
    partial: Option<usize>,
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
    /// The pickable entries grouped by their in-archive folder, folder-ordered
    /// (FEATURES.md §5.4). Indices point back into `entries`; a pack holds hundreds of
    /// them, and a flat list of hundreds is not a file tree.
    fn folders(&self) -> Vec<(String, Vec<usize>)> {
        let mut by_folder: std::collections::BTreeMap<String, Vec<usize>> =
            std::collections::BTreeMap::new();
        for i in self.extractable() {
            // `extractable()` already proved the pathname is Some.
            let path = self.entries[i].pathname.as_deref().unwrap_or_default();
            let folder = path.rsplit_once('/').map_or("", |(dir, _)| dir);
            by_folder.entry(folder.to_string()).or_default().push(i);
        }
        by_folder.into_iter().collect()
    }

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
    /// In-pack rows for pinned packs (DATA-MODEL §7 / M14) — bytes still inside the archive,
    /// carried with their locator for on-demand thumbnail/preview pulls.
    in_pack: Vec<(usize, Vec<lodestash_core::catalog::HydratedInPack>)>,
}

/// Per-root asset identity maps: root → (rel_path → (asset id, status, content_hash)).
type AssetIdentityMaps = Vec<(
    String,
    std::collections::HashMap<
        String,
        (
            i64,
            AssetStatus,
            bool,
            Option<u8>,
            lodestash_core::catalog::AssetMetrics,
            Option<String>,
        ),
    >,
)>;

/// Result of the write-behind catalog persist that follows every finished scan.
/// `counts` is None when the persist failed (logged; the UI falls back to in-memory).
struct CatalogSynced {
    generation: u64,
    counts: Option<StatusCounts>,
    /// True for a backlog drain, which computes no counts — `None` there means "nothing to
    /// report", not the persist failure it means everywhere else.
    drain_only: bool,
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

/// One pack payload pulled out for preview (FEATURES.md §5.4). The file lives in a scratch
/// directory, not the workspace: a peek is a look, so it must not leave catalog rows, grid
/// entries or cached extractions behind.
struct PackagePeek {
    name: String,
    media_type: Option<MediaType>,
    size_bytes: u64,
    path: PathBuf,
}

/// The Import-packages dialog (FEATURES.md §5.2): a folder scanned for `.unitypackage`
/// files, each of which can be imported as its own Library. Nothing is copied or unpacked —
/// importing writes a `package` row that points at the archive where it already lives.
struct ImportScan {
    root: PathBuf,
    scanning: bool,
    /// (in-folder group, archive path, display name, size) — group is the containing folder
    /// relative to the picked root, which is the publisher/category tree in an Asset Store cache.
    found: Vec<(String, PathBuf, String, u64)>,
    selected: std::collections::BTreeSet<usize>,
}

/// `PendingPackage.source_idx` for a pack opened from the catalog that lies outside every
/// library source: browse and preview work, extraction has nowhere to persist to.
const NO_SOURCE: usize = usize::MAX;

/// Sentinel `entry_idx` for the viewer stack while it is showing a peeked pack entry —
/// no grid entry backs it, and a real index must never collide with it.
const PEEK_ENTRY_IDX: usize = usize::MAX;

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
/// Row height and thumbnail size for the list/table view.
const LIST_ROW_HEIGHT: f32 = 38.0;
const LIST_THUMB_SIZE: f32 = 28.0;
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
    /// Dependency-graph constraint (DATA-MODEL §10 "Search"). None = no constraint.
    link: Option<LinkFilter>,
    /// Show only favourites (DATA-MODEL §12). A one-way switch: "not favourited" is not a thing
    /// anyone browses for.
    favorites_only: bool,
    trait_search: String,
}

/// The `has_dependencies` / `has_dependents` chips (DATA-MODEL §10). Single-select: an asset that
/// is both ends of a chain (a material) satisfies either, so combining them would only ever
/// narrow to that intersection — not what the chips read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LinkFilter {
    /// Uses something: a material with textures, a mesh with a material.
    HasDependencies,
    /// Something uses it: a texture some material paints with.
    HasDependents,
}

impl LinkFilter {
    const ALL: [LinkFilter; 2] = [LinkFilter::HasDependencies, LinkFilter::HasDependents];

    fn label(self) -> &'static str {
        match self {
            LinkFilter::HasDependencies => "uses others",
            LinkFilter::HasDependents => "used by others",
        }
    }
}

impl ContentFilter {
    fn active_count(&self) -> usize {
        self.traits.len()
            + self.tri_buckets.len()
            + self.sizes.len()
            + usize::from(self.min_rating.is_some())
            + usize::from(self.link.is_some())
            + usize::from(self.favorites_only)
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
        self.link = None;
        self.favorites_only = false;
        self.trait_search.clear();
    }

    /// Pure predicate: does an asset pass the filter? An asset passes if it matches the selected
    /// traits (any) AND selected tri-buckets (any) AND selected size tiers (any); an empty section
    /// imposes no constraint. Assets lacking a tri count are excluded when any tri-bucket is
    /// selected (design §3). Kept pure over primitives so it's unit-testable without an
    /// `AssetEntry` (TECH-STACK.md §5.1).
    // ponytail: a predicate over primitives, kept free of an AssetEntry so it stays unit-testable.
    #[allow(clippy::too_many_arguments)]
    fn passes(
        &self,
        size: u64,
        tri: Option<u32>,
        rating: Option<u8>,
        traits: &[String],
        links: (bool, bool),
        favorite: bool,
        tiers: &SizeTiers,
    ) -> bool {
        if self.favorites_only && !favorite {
            return false;
        }
        let (has_dependencies, has_dependents) = links;
        match self.link {
            Some(LinkFilter::HasDependencies) if !has_dependencies => return false,
            Some(LinkFilter::HasDependents) if !has_dependents => return false,
            _ => {}
        }
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
    /// Every asset's tags, refreshed per catalog sync (IDENTIFICATION-AND-SEARCH §2). Held beside
    /// the entries rather than inside them for the same reason as the edge sets: entries outlive
    /// any single catalog read, and a tagging pass changes tags without rebuilding the grid.
    asset_tags: std::collections::HashMap<i64, Vec<lodestash_core::catalog::TagAssignment>>,
    /// Tag vocabulary with usage counts, most-used first — the ⚑ Filters TRAIT chips.
    tag_usage: Vec<(String, String, i64)>,
    /// Material asset id → the absolute path of the texture its preview sphere wears, resolved
    /// from the dependency graph (DATA-MODEL §10). Refreshed per catalog sync with the tag maps.
    material_albedo: std::collections::HashMap<i64, String>,
    /// Favourited asset ids (DATA-MODEL §12). A set beside the entries, like the tag and edge
    /// maps: entries outlive any single catalog read, and a favourite toggles far more often than
    /// the grid rebuilds.
    favorites: std::collections::HashSet<i64>,
    /// Asset ids with at least one resolved outbound / inbound dependency edge (DATA-MODEL §10).
    /// Reloaded on every catalog sync; drives the grid's link badge and the LINKS filter chips.
    /// Sets rather than a per-entry flag — the grid's entries outlive any single catalog read.
    edges_out: std::collections::HashSet<i64>,
    edges_in: std::collections::HashSet<i64>,
    /// LINKED FILES rows for the selected asset (UI-SPEC §13.6), and the asset they belong to —
    /// the reload guard, since the panel re-renders every frame but the graph changes rarely.
    related: Vec<lodestash_core::catalog::RelatedAsset>,
    related_for: Option<i64>,
    /// The local SigLIP joint image↔text embedder for Search-with-AI (IDENTIFICATION-AND-SEARCH
    /// §5.B), shared by the query side (this thread, text tower) and the persist-thread image
    /// drain (vision tower). `None` when the ONNX Runtime + model artifacts aren't present — AI
    /// search then stays a stub.
    ai_embedder: Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    /// The local CLAP joint audio↔text embedder — a **separate** 512-d space from SigLIP's
    /// 768-d visual one. Queries are embedded into both and the two KNN result lists are fused
    /// by RRF; the vectors themselves are never compared across spaces.
    audio_embedder: Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    /// Structural constraints parsed out of the last AI query (§5.B step 1) — applied to the
    /// grid on top of the fused vector/BM25 ranking.
    ai_filters: Option<lodestash_core::query::ParsedQuery>,
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
    /// Folder-navigation history for the ← / → buttons: each visited `current_dir`, newest at the
    /// back, with `nav_cursor` pointing at the current position. Navigating forward from the middle
    /// truncates the tail (standard browser history). Reset when the library reloads.
    nav_history: Vec<Option<(usize, PathBuf)>>,
    nav_cursor: usize,
    /// Child folders at `current_dir` (name + direct/recursive counts). Rebuilt with the cache.
    folder_tiles: Vec<FolderTile>,
    /// In Folders view, render the current directory's assets as a **file list** (folders still
    /// first, as tiles) instead of a thumbnail grid — the pack browser's list/thumbnails toggle
    /// (FEATURES.md §5.4). Session state.
    folders_as_list: bool,
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
    /// The pack entry currently peeked (FEATURES.md §5.4 right pane): one payload pulled to
    /// a scratch file and shown in the viewer stack, with no catalog row and no grid entry.
    peek: Option<PackagePeek>,
    peek_rx: Receiver<Result<PackagePeek, String>>,
    peek_tx: Sender<Result<PackagePeek, String>>,
    /// In-pack asset locators (DATA-MODEL §7 / M14): `asset_id → (pack path, tar offset, size)`.
    /// Lives beside the grid entries (refreshed per hydrate) rather than on `AssetEntry`, so the
    /// grid plumbing stays unchanged; the thumbnail and preview paths look a payload up here to
    /// pull it from the archive on demand.
    in_pack_locators: std::collections::HashMap<i64, (String, i64, i64)>,
    /// A pack the user pressed **Open →** on: once its in-pack assets land in the grid (via the
    /// background mint + hydrate), the browser jumps into it in Folders view (FEATURES.md §5.4).
    pending_pack_browse: Option<String>,
    /// Signalled by a background in-pack mint (import / Add-to-Library) once its assets exist,
    /// so the grid re-hydrates and the new pack library appears (M14).
    rehydrate_rx: Receiver<()>,
    rehydrate_tx: Sender<()>,
    /// The open Import-packages dialog (§5.2), if any.
    import_scan: Option<ImportScan>,
    import_rx: Receiver<Vec<(String, PathBuf, String, u64)>>,
    import_tx: Sender<Vec<(String, PathBuf, String, u64)>>,
    /// Where extracted package payloads are stored, content-addressed (None if the OS cache dir
    /// is unavailable).
    workspace_dir: Option<PathBuf>,
    /// The loopback engine API's handshake (port + token), once it's listening (ENGINE-API §1).
    /// `None` when the port or the handshake file couldn't be taken — the GUI works regardless.
    engine: Option<lodestash_api::Handshake>,
    /// The agent planes' shared read state. Kept so the GUI can answer their approval prompts
    /// (MCP-AND-ENGINES §5: mutating ops confirm server-side, not in the client).
    engine_state: Option<std::sync::Arc<lodestash_api::ServerState>>,
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
    /// Packs ticked on the Packages screen, by cache path (survives a refresh, which
    /// rebuilds `store_packages`).
    store_selected: std::collections::BTreeSet<PathBuf>,
    /// Live watch over the source roots (DATA-MODEL §6). `None` when disabled or unavailable.
    watcher: Option<lodestash_ingest::watch::RootWatcher>,
    /// Pack whose stats modal is open (UI-SPEC §15), by pack name.
    pack_stats_open: Option<String>,
    /// A watched root changed; the rescan waits for the library to be idle.
    watch_dirty: bool,
    /// When the last watch-triggered rescan ran — a busy tree (a Unity project writing its
    /// Library folder) must not put the app in a rescan loop.
    last_watch_rescan: Option<std::time::Instant>,
    /// Packs waiting their turn to unpack. ponytail: one extraction at a time — each is a
    /// gzip stream-through plus a catalog persist, so running them in parallel mostly
    /// contends on disk; widen this to N workers only if a measurement says so.
    extract_queue: std::collections::VecDeque<PathBuf>,
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
    /// STATUS facet selection (UI-SPEC §0.2). Entries start `discovered` (folding into
    /// Unidentified) and move as `promote_statuses` reconciles them (DATA-MODEL §8).
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
    /// Tray row id for a running tagging pass, so it can be finished when the drain reports back.
    tag_job: Option<u64>,
    /// Models Ollama reports as installed, fetched once when the Settings AI card first opens
    /// (the daemon may not be running, and asking every frame would block the UI thread).
    ollama_models: Option<Vec<String>>,
    /// Catalog `import_source` metadata keyed by root path, cached for the Settings panel.
    /// Refreshed after catalog sync and after source-related mutations (watch toggle/rescan).
    source_infos: std::collections::HashMap<String, lodestash_core::catalog::SourceInfo>,
    /// In-flight source-info refresh.
    source_info_rx: Receiver<Vec<lodestash_core::catalog::SourceInfo>>,
    source_info_tx: Sender<Vec<lodestash_core::catalog::SourceInfo>>,
    source_info_pending: bool,
    /// Cached (thumbnail, workspace) cache sizes in bytes for the Storage caps card.
    storage_size: Option<(u64, u64)>,
    storage_size_rx: Option<Receiver<(u64, u64)>>,
    /// Pending destructive-action confirmations (entry indices).
    pending_confirm_trash: Option<Vec<usize>>,
    pending_delete_from_disk: Option<Vec<usize>>,
    /// Number of exact content_hash duplicate groups (size >= 2) among live entries;
    /// drives the Duplicates nav badge.
    duplicate_groups: usize,
    /// Drag-select state for marquee selection over the grid/list.
    drag_select_start: Option<egui::Pos2>,
    drag_select_current: Option<egui::Pos2>,
    drag_select_active: bool,
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
        let (peek_tx, peek_rx) = mpsc::channel();
        let (rehydrate_tx, rehydrate_rx) = mpsc::channel::<()>();
        let (import_tx, import_rx) = mpsc::channel();
        let (catalog_tx, catalog_rx) = mpsc::channel();
        let (hydrate_tx, hydrate_rx) = mpsc::channel();
        let (viewer_tx, viewer_rx) = mpsc::channel();
        let (source_info_tx, source_info_rx) = mpsc::channel();
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
            favorites: std::collections::HashSet::new(),
            asset_tags: std::collections::HashMap::new(),
            tag_usage: Vec::new(),
            material_albedo: std::collections::HashMap::new(),
            edges_out: std::collections::HashSet::new(),
            edges_in: std::collections::HashSet::new(),
            related: Vec::new(),
            related_for: None,
            search_ranks: None,
            search_ranks_key: None,
            ai_embedder: resolve_embedder(),
            audio_embedder: resolve_audio_embedder(),
            ai_filters: None,
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
            nav_history: vec![None],
            nav_cursor: 0,
            folder_tiles: Vec::new(),
            folders_as_list: false,
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
            peek: None,
            peek_rx,
            peek_tx,
            in_pack_locators: std::collections::HashMap::new(),
            pending_pack_browse: None,
            rehydrate_rx,
            rehydrate_tx,
            import_scan: None,
            import_rx,
            import_tx,
            workspace_dir: dirs::cache_dir().map(|d| d.join("Lodestash").join("workspace")),
            content_filter: ContentFilter::default(),
            search_mode: SearchMode::Tags,
            nudge_dismissed: false,
            store_roots: Vec::new(),
            store_packages: Vec::new(),
            store_selected: Default::default(),
            watcher: None,
            pack_stats_open: None,
            watch_dirty: false,
            last_watch_rescan: None,
            extract_queue: Default::default(),
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
            tag_job: None,
            ollama_models: None,
            engine: None,
            engine_state: None,
            source_infos: std::collections::HashMap::new(),
            source_info_rx,
            source_info_tx,
            source_info_pending: false,
            storage_size: None,
            storage_size_rx: None,
            pending_confirm_trash: None,
            pending_delete_from_disk: None,
            duplicate_groups: 0,
            drag_select_start: None,
            drag_select_current: None,
            drag_select_active: false,
            config,
        };

        if let Some((handshake, state)) = {
            let (thumbs, ws) = default_cache_dirs();
            start_engine_api(thumbs, ws)
        } {
            // An agent's request arrives on a server thread; the window must wake to show the
            // approval prompt rather than wait for the user to jiggle the mouse.
            let repaint_ctx = cc.egui_ctx.clone();
            state.set_wake(Box::new(move || repaint_ctx.request_repaint()));
            // Same embedders the grid's Search-with-AI uses, so the agent plane ranks
            // identically instead of running a second, subtly different pipeline.
            let (visual, audio) = (app.ai_embedder.clone(), app.audio_embedder.clone());
            if visual.is_some() || audio.is_some() {
                state.set_query_embedder(Box::new(move |q: &str| {
                    [visual.as_ref(), audio.as_ref()]
                        .into_iter()
                        .flatten()
                        .map(|e| (e.model_id().to_string(), e.embed_text(q)))
                        .collect()
                }));
            }
            app.engine = Some(handshake);
            app.engine_state = Some(state);
        }

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
        self.reset_nav_history();
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
        // Pack-only sources (FEATURES.md §5.2) are tracked but not crawled for loose files — their
        // packs surface as in-pack libraries via the hydrate instead (M14).
        let pack_only: Vec<bool> = match self.project_name.as_deref() {
            Some(proj) => roots
                .iter()
                .map(|r| self.config.is_pack_only(proj, r))
                .collect(),
            None => roots.iter().map(|_| false).collect(),
        };

        self.start_watching();
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
                if pack_only[source_idx] {
                    continue; // tracked container; its packs hydrate as in-pack libraries
                }
                for file in crawler::crawl(root, &excludes[source_idx]) {
                    if is_unitypackage(&file.abs_path) {
                        // Don't surface the raw archive; offer an Extract action instead (PR #8).
                        if let Some(pkg) = resolve_pending_package(source_idx, &file) {
                            pending_packages.push(pkg);
                        }
                        continue;
                    }
                    if !is_browseable(&file) {
                        continue;
                    }
                    entries.push((source_idx, file));
                }
            }
            let _ = tx.send(ScanDone {
                generation,
                entries,
                pending_packages,
                partial: None,
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
                    Vec<(usize, Vec<lodestash_core::catalog::HydratedInPack>)>,
                );
                let run = || -> Result<Hydrated, String> {
                    let catalog = Catalog::open(&db_path).map_err(|e| e.to_string())?;
                    let mut sources = Vec::new();
                    let mut extracted = Vec::new();
                    let mut in_pack = Vec::new();
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
                        // A **pinned** pack comes back either way (FEATURES.md §5.5, M7): the
                        // pin is the user saying this is a library, and a library that
                        // vanishes on restart isn't one.
                        if let Some(ws) = &workspace_dir {
                            let rows = catalog
                                .extracted_entries_for_source(sid)
                                .map_err(|e| e.to_string())?;
                            let live: Vec<_> = rows
                                .into_iter()
                                .filter(|r| {
                                    (cache_extracted || r.pinned)
                                        && ws.join(&r.workspace_rel).is_file()
                                })
                                .collect();
                            if !live.is_empty() {
                                extracted.push((idx, live));
                            }
                        }
                        // In-pack assets (M14): a pinned pack's contents surface as grid cards
                        // whose bytes are pulled from the archive on demand — no workspace copy.
                        let pack_rows: Vec<_> = catalog
                            .in_pack_entries_for_source(sid)
                            .map_err(|e| e.to_string())?
                            .into_iter()
                            .filter(|r| r.pinned)
                            .collect();
                        if !pack_rows.is_empty() {
                            in_pack.push((idx, pack_rows));
                        }
                    }
                    Ok((sources, extracted, in_pack))
                };
                match run() {
                    Ok((sources, extracted, in_pack))
                        if !sources.is_empty() || !extracted.is_empty() || !in_pack.is_empty() =>
                    {
                        let _ = tx.send(HydrateDone {
                            generation,
                            sources,
                            extracted,
                            in_pack,
                        });
                    }
                    Ok(_) => {} // nothing catalogued yet — first open of these sources
                    Err(err) => tracing::warn!(%err, "startup hydrate failed; waiting for crawl"),
                }
            });
        }

        ctx.request_repaint();
    }

    /// Rescan a single source root in the background, merging its results into the existing
    /// grid instead of replacing everything (ScanDone::partial).
    fn rescan_source(&mut self, ctx: &egui::Context, source_idx: usize) {
        let Some(root) = self.scan_roots.get(source_idx).cloned() else {
            return;
        };
        let excludes = self
            .project_name
            .as_deref()
            .map(|p| self.config.source_excludes(p, &root))
            .unwrap_or_default();

        self.start_watching();
        self.scan_generation += 1;
        self.scanning = true;
        if let Some(id) = self.scan_job.take() {
            self.jobs.finish(id);
        }
        self.scan_job = Some(self.jobs.start(
            format!("Rescanning {}", config::stem_name(&root)),
            Arc::default(),
            true,
        ));
        let generation = self.scan_generation;
        let tx = self.scan_tx.clone();
        std::thread::spawn(move || {
            let mut entries = Vec::new();
            let mut pending_packages = Vec::new();
            for file in crawler::crawl(&root, &excludes) {
                if is_unitypackage(&file.abs_path) {
                    if let Some(pkg) = resolve_pending_package(source_idx, &file) {
                        pending_packages.push(pkg);
                    }
                    continue;
                }
                if !is_browseable(&file) {
                    continue;
                }
                entries.push((source_idx, file));
            }
            let _ = tx.send(ScanDone {
                generation,
                entries,
                pending_packages,
                partial: Some(source_idx),
            });
        });
        ctx.request_repaint();
    }

    /// Toggle filesystem watching for one source and persist it in the catalog. Optimistically
    /// updates the in-memory cache so the Settings row reflects the change immediately.
    fn apply_source_watch_toggle(&mut self, ctx: &egui::Context, source_idx: usize, enabled: bool) {
        let Some(root) = self.scan_roots.get(source_idx).cloned() else {
            return;
        };
        let root_str = root.to_string_lossy().to_string();
        if let Some(info) = self.source_infos.get_mut(&root_str) {
            info.watch_enabled = enabled;
        }
        // Persist in the background and refresh the cache once the write lands.
        let db_path = match catalog_db_path() {
            Some(p) => p,
            None => {
                self.start_watching();
                self.refresh_source_infos();
                return;
            }
        };
        std::thread::spawn(move || {
            let _ = Catalog::open(&db_path).and_then(|c| {
                c.source_info_by_root(&root_str).and_then(|info| {
                    if let Some(i) = info {
                        c.set_source_watch(i.id, enabled)?;
                    }
                    Ok(())
                })
            });
        });
        self.start_watching();
        self.refresh_source_infos();
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
                            metrics: row.metrics,
                            content_hash: row.content_hash.clone(),
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
                            metrics: row.asset.metrics,
                            content_hash: row.asset.content_hash.clone(),
                            texture: None,
                            thumb_spawned: false,
                        });
                    }
                }
            }
            // In-pack assets (M14): grid cards whose bytes stay inside the archive. `abs_path`
            // points at the `.unitypackage` itself (provenance + Reveal), and the locator lands
            // in `in_pack_locators` so the thumbnail/preview paths pull the payload on demand.
            self.in_pack_locators
                .retain(|id, _| self.entries.iter().any(|e| e.asset_id == Some(*id)));
            for (source_idx, rows) in msg.in_pack {
                for row in rows {
                    let Some(status) = AssetStatus::from_code(&row.asset.status) else {
                        continue;
                    };
                    let pack_path = PathBuf::from(&row.pack_path);
                    self.in_pack_locators.insert(
                        row.asset.asset_id,
                        (row.pack_path, row.tar_offset, row.size),
                    );
                    self.entries.push(AssetEntry {
                        package: Some(row.package_name),
                        file: CrawledFile {
                            abs_path: pack_path,
                            rel_path: PathBuf::from(&row.asset.rel_path),
                            size_bytes: row.asset.size_bytes.max(0) as u64,
                            mtime: row.asset.mtime,
                            media_type: MediaType::from_code(&row.asset.media_type),
                        },
                        source_idx,
                        asset_id: Some(row.asset.asset_id),
                        status,
                        trashed: row.asset.trashed,
                        rating: row.asset.rating,
                        metrics: row.asset.metrics,
                        content_hash: row.asset.content_hash.clone(),
                        texture: None,
                        thumb_spawned: false,
                    });
                }
            }
            self.current_dir = self.default_dir();
            self.reset_nav_history();
            self.recompute_tab_counts();
            self.cache_dirty = true;
            self.last_cols = 0;
            self.last_row_start = usize::MAX;
            self.last_row_end = 0;
            // A pending Open → jumps into the pack's folder explorer once its assets have landed.
            if let Some(name) = self.pending_pack_browse.clone() {
                if self
                    .entries
                    .iter()
                    .any(|e| e.package.as_deref() == Some(name.as_str()))
                {
                    self.pending_pack_browse = None;
                    self.browse_package_folders(name);
                }
            }
            tracing::info!(entries = self.entries.len(), "hydrated grid from catalog");
            // A hydrate skips the persist thread, and the embed/metric drains live inside it —
            // so a library that was catalogued before the AI models arrived would never encode.
            // Drain the backlog directly instead of waiting for the next rescan.
            self.spawn_backlog_drain();
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
            let partial = msg.partial;
            let (kept, rest): (Vec<AssetEntry>, Vec<AssetEntry>) = if let Some(sidx) = partial {
                self.entries
                    .drain(..)
                    .partition(|e| e.package.is_some() || e.source_idx != sidx)
            } else {
                self.entries.drain(..).partition(|e| e.package.is_some())
            };
            type Carried = (
                Option<i64>,
                AssetStatus,
                bool,
                Option<u8>,
                lodestash_core::catalog::AssetMetrics,
                Option<String>,
            );
            let carry: std::collections::HashMap<(usize, PathBuf), Carried> = rest
                .into_iter()
                .map(|e| {
                    (
                        (e.source_idx, e.file.rel_path),
                        (
                            e.asset_id,
                            e.status,
                            e.trashed,
                            e.rating,
                            e.metrics,
                            e.content_hash,
                        ),
                    )
                })
                .collect();
            let new_entries: Vec<AssetEntry> = msg
                .entries
                .into_iter()
                .map(|(source_idx, f)| {
                    let (asset_id, status, trashed, rating, metrics, content_hash) = carry
                        .get(&(source_idx, f.rel_path.clone()))
                        .cloned()
                        .unwrap_or((
                            None,
                            AssetStatus::Discovered,
                            false,
                            None,
                            Default::default(),
                            None,
                        ));
                    AssetEntry {
                        package: None,
                        file: f,
                        source_idx,
                        asset_id,
                        status,
                        trashed,
                        rating,
                        metrics,
                        content_hash,
                        texture: None,
                        thumb_spawned: false,
                    }
                })
                .collect();
            self.entries = kept;
            self.entries.extend(new_entries);
            if let Some(sidx) = partial {
                self.pending_packages.retain(|p| p.source_idx != sidx);
                self.pending_packages.extend(msg.pending_packages);
            } else {
                self.pending_packages = msg.pending_packages;
            }
            // Entry indices changed; reset selection.
            self.selected.clear();
            self.select_anchor = None;
            self.current_dir = self.default_dir();
            self.reset_nav_history();
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
    /// Encode whatever the catalog still has queued (facets + embeddings), without a scan.
    /// Takes the same writer slot as the persist thread — never both at once (DATA-MODEL §1
    /// single writer), so this no-ops while a persist is in flight and retries on the next
    /// hydrate.
    /// Reload the two dependency-edge id sets from the catalog (DATA-MODEL §10). Called after
    /// every catalog sync — a drain can add edges — and cheap enough to do wholesale: two
    /// `SELECT DISTINCT`s over a table with one row per declared reference.
    fn reload_edge_sets(&mut self) {
        if self.search_catalog.is_none() {
            self.search_catalog = catalog_db_path().and_then(|p| Catalog::open(&p).ok());
        }
        let Some(cat) = &self.search_catalog else {
            return;
        };
        match cat.assets_with_edges() {
            Ok((out, inbound)) => {
                self.edges_out = out.into_iter().collect();
                self.edges_in = inbound.into_iter().collect();
                self.related_for = None; // force the inspector panel to re-read
            }
            Err(e) => tracing::warn!(%e, "dependency edge sets unavailable"),
        }
        // Tags move on the same beat: a tagging pass reports back through the same message.
        match cat.all_asset_tags() {
            Ok(map) => self.asset_tags = map,
            Err(e) => tracing::warn!(%e, "asset tags unavailable"),
        }
        match cat.tag_usage(60) {
            Ok(v) => self.tag_usage = v,
            Err(e) => tracing::warn!(%e, "tag vocabulary unavailable"),
        }
        match cat.favorite_ids() {
            Ok(ids) => self.favorites = ids.into_iter().collect(),
            Err(e) => tracing::warn!(%e, "favorites unavailable"),
        }
        match cat.material_albedo_paths() {
            Ok(map) => {
                // A material whose texture only just resolved needs its placeholder thumbnail
                // thrown away, or it keeps the flat card until the next scan.
                for entry in &mut self.entries {
                    if entry.file.media_type == Some(MediaType::Material)
                        && entry
                            .asset_id
                            .is_some_and(|id| map.get(&id) != self.material_albedo.get(&id))
                    {
                        entry.texture = None;
                        entry.thumb_spawned = false;
                    }
                }
                self.material_albedo = map;
            }
            Err(e) => tracing::warn!(%e, "material albedo paths unavailable"),
        }
    }

    /// LINKED FILES rows for `asset_id`, reloaded only when the selection changes (UI-SPEC §13.6).
    fn ensure_related_loaded(&mut self, asset_id: i64) {
        if self.related_for == Some(asset_id) {
            return;
        }
        self.related_for = Some(asset_id);
        self.related = self
            .search_catalog
            .as_ref()
            .and_then(|cat| cat.related_assets(asset_id).ok())
            .unwrap_or_default();
    }

    /// Select the grid entry carrying `asset_id`, if it's in the current view — the LINKED FILES
    /// rows' click target. Returns false when the asset exists but is filtered out of the grid.
    fn select_by_asset_id(&mut self, asset_id: i64) -> bool {
        let Some(entry_idx) = self
            .entries
            .iter()
            .position(|e| e.asset_id == Some(asset_id))
        else {
            return false;
        };
        let Some(cache_idx) = self.sorted_cache.iter().position(|&i| i == entry_idx) else {
            return false;
        };
        self.selected_cache_idx = Some(cache_idx);
        self.selected.clear();
        self.selected.insert(entry_idx);
        self.page = cache_idx / self.config.view.page_size.max(1);
        true
    }

    /// Tag the `tags_dirty` backlog with the configured Ollama model (AI-PIPELINE §2), on its own
    /// thread holding the writer slot.
    ///
    /// Manual, not automatic: measured on this box a name-only pass is ~4.7 s/asset and a vision pass
    /// several times that, so a full library is hours of CPU — something the user asks for, never
    /// something a scan starts. It is resumable for the same reason: `tags_dirty` is per asset, so a
    /// cancelled run simply leaves the rest queued.
    fn spawn_tagging(&mut self) {
        if self.catalog_persisting {
            self.jobs
                .toast("Catalog is busy — try again in a moment", Intent::Info);
            return;
        }
        let (Some(db_path), Some(model)) = (catalog_db_path(), self.config.tagger_model.clone())
        else {
            return;
        };
        let vision_model = self.config.tagger_vision_model.clone();
        let workspace_dir = self.workspace_dir.clone();
        let cache_dir = self.thumb_cache_dir.clone();
        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        let roots = self.scan_root_names();
        // A manual Identify pass does the full job: embeddings first (so search works), then tags.
        let embedder = self.ai_embedder.clone();
        let audio_embedder = self.audio_embedder.clone();
        let progress = std::sync::Arc::new(crate::jobs::JobProgress::default());
        let job_progress = progress.clone();
        self.tag_job = Some(self.jobs.start(
            format!("Identifying with {model}"),
            progress.clone(),
            true,
        ));
        self.catalog_persisting = true;

        std::thread::spawn(move || {
            let tagger = match vision_model {
                Some(v) => lodestash_ai::ollama::OllamaTagger::new(model).with_vision(v),
                None => lodestash_ai::ollama::OllamaTagger::new(model),
            };
            let (counts, assets) = match Catalog::open(&db_path) {
                Ok(mut cat) => {
                    if let Some(emb) = &embedder {
                        drain_image_embeddings(&mut cat, emb.as_ref(), workspace_dir.as_deref());
                    }
                    if let Some(emb) = &audio_embedder {
                        drain_audio_embeddings(&mut cat, emb.as_ref(), workspace_dir.as_deref());
                    }
                    drain_tags(
                        &mut cat,
                        &tagger,
                        workspace_dir.as_deref(),
                        cache_dir.as_deref(),
                        &job_progress,
                    );
                    // Tags change what search returns and (with P4's status rule) what "identified"
                    // means, so report fresh counts + identities like any other drain.
                    resync_after_drain(&mut cat, &roots)
                }
                Err(e) => {
                    tracing::warn!(%e, "tagging: catalog unavailable");
                    (None, Vec::new())
                }
            };
            let _ = tx.send(CatalogSynced {
                generation,
                drain_only: true,
                counts,
                assets,
            });
        });
    }

    /// `(source name, root path)` per scan root — what `upsert_import_source` wants on the
    /// persist thread.
    fn scan_root_names(&self) -> Vec<(String, String)> {
        self.scan_roots
            .iter()
            .map(|r| (config::stem_name(r), r.to_string_lossy().into_owned()))
            .collect()
    }

    fn spawn_backlog_drain(&mut self) {
        if self.catalog_persisting {
            return;
        }
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        // Identification (embeddings) is CPU-heavy and manual by default (AI-PIPELINE §2): a hydrate
        // only auto-embeds when the user opted into auto-identify. Metrics/deps/promote still run —
        // they are cheap and drive the grid's facets and status.
        let embedder = self
            .config
            .auto_identify
            .then(|| self.ai_embedder.clone())
            .flatten();
        let audio_embedder = self
            .config
            .auto_identify
            .then(|| self.audio_embedder.clone())
            .flatten();
        let workspace_dir = self.workspace_dir.clone();
        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        let roots = self.scan_root_names();
        self.catalog_persisting = true;

        std::thread::spawn(move || {
            let (counts, assets) = match Catalog::open(&db_path) {
                Ok(mut cat) => drain_promote_resync(
                    &mut cat,
                    embedder,
                    audio_embedder,
                    workspace_dir.as_deref(),
                    &roots,
                ),
                Err(e) => {
                    tracing::warn!(%e, "backlog drain: catalog unavailable");
                    (None, Vec::new())
                }
            };
            // `drain_only` keeps a `None` here quiet: it means "nothing moved", not a failure.
            let _ = tx.send(CatalogSynced {
                generation,
                drain_only: true,
                counts,
                assets,
            });
        });
    }

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
        let audio_embedder = self.audio_embedder.clone();
        let workspace_dir = self.workspace_dir.clone();
        let roots = self.scan_root_names();
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
                drain_only: false,
                counts,
                assets,
            });
            // Then embed the rows `record_crawl` just marked dirty, on this same connection
            // (still the sole writer). Runs *after* the grid gets its identity, so a large
            // first scan doesn't stall the UI on inference. Search-with-AI needs these.
            if let Some(mut cat) = catalog {
                let (counts, assets) = drain_promote_resync(
                    &mut cat,
                    embedder,
                    audio_embedder,
                    workspace_dir.as_deref(),
                    &roots,
                );
                // Second message: the drain promoted rows, so the meter and STATUS facet
                // are now stale. `drain_only` keeps a no-op quiet.
                if counts.is_some() {
                    let _ = tx.send(CatalogSynced {
                        generation,
                        drain_only: true,
                        counts,
                        assets,
                    });
                }
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
            records: crawl_records_in_workspace(files.iter(), self.workspace_dir.as_deref()),
            entries,
            all_roots: self
                .scan_roots
                .iter()
                .map(|r| (config::stem_name(r), r.to_string_lossy().into_owned()))
                .collect(),
        };

        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        let embedder = self.ai_embedder.clone();
        let audio_embedder = self.audio_embedder.clone();
        let workspace_dir = self.workspace_dir.clone();
        let roots = job.all_roots.clone();
        self.catalog_persisting = true;
        std::thread::spawn(move || {
            let (counts, assets, catalog) = match persist_package_to_catalog(&db_path, &job) {
                Ok((counts, assets, cat)) => (Some(counts), assets, Some(cat)),
                Err(err) => {
                    tracing::warn!(%err, "package persist failed; UI stays on in-memory counts");
                    (None, Vec::new(), None)
                }
            };
            let _ = tx.send(CatalogSynced {
                generation,
                drain_only: false,
                counts,
                assets,
            });
            // Embed the freshly-extracted images now, while their files are guaranteed present in
            // the workspace (same sole-writer connection).
            if let Some(mut cat) = catalog {
                let (counts, assets) = drain_promote_resync(
                    &mut cat,
                    embedder,
                    audio_embedder,
                    workspace_dir.as_deref(),
                    &roots,
                );
                if counts.is_some() {
                    let _ = tx.send(CatalogSynced {
                        generation,
                        drain_only: true,
                        counts,
                        assets,
                    });
                }
            }
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
    /// Also recomputes exact content_hash duplicate groups for the Duplicates nav badge.
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

        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for e in self.entries.iter().filter(|e| !e.trashed) {
            if let Some(hash) = e.content_hash.as_deref().filter(|h| !h.is_empty()) {
                *counts.entry(hash.to_string()).or_insert(0) += 1;
            }
        }
        self.duplicate_groups = counts.values().filter(|&&n| n >= 2).count();
    }

    /// Delete the original files for the given entries from disk, then keep the catalog
    /// rows in the Trash as a record. This is the explicit danger flow; it cannot be undone.
    /// (DESIGN-GUIDELINES.md §Settings).
    fn delete_files_from_disk(&mut self, entry_idxs: &[usize]) {
        let mut deleted = Vec::new();
        let mut failed = 0usize;
        let mut refused = 0usize;
        for &i in entry_idxs {
            let Some(entry) = self.entries.get(i) else {
                continue;
            };
            // `abs_path` is the authoritative location every other action uses (Reveal, Open,
            // thumbnails). Rebuilding it as `scan_root.join(rel_path)` is wrong for
            // package-extracted entries, whose `rel_path` is relative to the **workspace** — that
            // reconstruction can name a real, unrelated file under the source root.
            let path = entry.file.abs_path.clone();
            if !self.is_deletable_path(&path) {
                tracing::warn!(path = %path.display(), "refused to delete: outside every known root");
                refused += 1;
                continue;
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    tracing::info!(path = %path.display(), "deleted file from disk");
                    deleted.push(i);
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), %e, "delete from disk failed");
                    failed += 1;
                }
            }
        }
        // Only what actually left the disk is recorded as gone. Trashing first and deleting
        // afterwards let a failed delete leave the catalog claiming the file was removed.
        if !deleted.is_empty() {
            self.set_entries_trashed(&deleted, true);
            self.jobs.toast(
                format!("Deleted {} file(s) from disk", deleted.len()),
                Intent::Warning,
            );
        }
        if failed + refused > 0 {
            self.jobs.toast(
                format!(
                    "{} file(s) could not be deleted — they are still on disk",
                    failed + refused
                ),
                Intent::Danger,
            );
        }
    }

    /// Guard for the one destructive path in the app: a file may only be deleted when it sits
    /// under a source root the user opened, or inside the extraction workspace this app owns.
    ///
    /// Defence in depth, not the primary check — the primary check is using `abs_path` rather than
    /// recomposing a path. This is what bounds the blast radius if a future change gets that wrong.
    fn is_deletable_path(&self, path: &std::path::Path) -> bool {
        path.is_file() && under_known_root(path, &self.scan_roots, self.workspace_dir.as_deref())
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
        let roots = self.scan_root_names();
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
                drain_only: false,
                counts,
                assets: Vec::new(),
            });
        });
    }

    /// Toggle favourite on a selection (DATA-MODEL §12). Optimistic locally, then written on the
    /// writer thread like every other catalog mutation. If the selection is mixed, favouriting
    /// wins: the user asked for a pin, and pinning what is already pinned is a no-op.
    fn toggle_favorite(&mut self, entry_idxs: &[usize]) {
        let ids: Vec<i64> = entry_idxs
            .iter()
            .filter_map(|&i| self.entries.get(i).and_then(|e| e.asset_id))
            .collect();
        if ids.is_empty() {
            // No catalog row yet: the scan hasn't persisted this asset, so there is nothing to
            // pin *to*. Say so rather than silently doing nothing.
            self.jobs.toast(
                "Not catalogued yet — favourites need the scan to finish",
                Intent::Info,
            );
            return;
        }
        let make_favorite = !ids.iter().all(|id| self.favorites.contains(id));
        for id in &ids {
            if make_favorite {
                self.favorites.insert(*id);
            } else {
                self.favorites.remove(id);
            }
        }
        self.cache_dirty = true; // the favourites filter may include/exclude these now

        let Some(db_path) = catalog_db_path() else {
            return;
        };
        let generation = self.scan_generation;
        let tx = self.catalog_tx.clone();
        std::thread::spawn(move || {
            let result = Catalog::open(&db_path)
                .and_then(|c| c.set_favorite(&ids, make_favorite))
                .map_err(|e| e.to_string());
            if let Err(err) = result {
                tracing::warn!(%err, "favorite update failed; local flag may diverge until next sync");
            }
            let _ = tx.send(CatalogSynced {
                generation,
                drain_only: true,
                counts: None,
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
                drain_only: false,
                counts: None,
                assets: Vec::new(),
            });
        });
    }

    /// Keep the viewer in sync with the Quick Look target: (re)load when the overlay
    /// is open on a raster asset it doesn't hold yet. Decode runs off-thread.
    fn ensure_viewer_loaded(&mut self, ctx: &egui::Context) {
        // A pack peek owns the viewer stack while it is open — otherwise this would free its
        // texture (and drop its in-flight load as stale) on the very next frame.
        if self.peek.is_some() {
            return;
        }
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
            self.reload_edge_sets();
            if let Some(id) = self.tag_job.take() {
                self.jobs.finish(id);
                self.jobs
                    .toast("Identification pass finished", Intent::Success);
            }
            match msg.counts {
                Some(counts) => self.catalog_counts = Some(counts),
                None if msg.drain_only => {}
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
                if let Some(&(id, status, trashed, rating, metrics, ref content_hash)) =
                    map.get(&rel)
                {
                    entry.asset_id = Some(id);
                    entry.status = status;
                    entry.trashed = trashed;
                    entry.rating = rating;
                    entry.metrics = metrics;
                    entry.content_hash = content_hash.clone();
                }
            }
            // Statuses/trash flags may have changed what the grid and tabs show.
            self.recompute_tab_counts();
            self.cache_dirty = true;
            self.refresh_source_infos();
        }
    }

    /// Load `import_source` metadata off the main thread so the Settings Sources panel can show
    /// live counts, watch state, and last-scanned without stalling the UI.
    fn refresh_source_infos(&mut self) {
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        if self.source_info_pending {
            return;
        }
        self.source_info_pending = true;
        let tx = self.source_info_tx.clone();
        std::thread::spawn(move || {
            let infos = match Catalog::open(&db_path).and_then(|c| c.source_infos()) {
                Ok(v) => v,
                Err(err) => {
                    tracing::warn!(%err, "source info load failed");
                    Vec::new()
                }
            };
            let _ = tx.send(infos);
        });
    }

    /// Merge loaded source metadata into the cache and restart watching so per-source watch
    /// toggles take effect immediately.
    fn receive_source_infos(&mut self) {
        let mut dirty = false;
        while let Ok(infos) = self.source_info_rx.try_recv() {
            self.source_info_pending = false;
            let old: std::collections::HashSet<_> =
                self.source_infos.values().map(|i| i.id).collect();
            self.source_infos.clear();
            for info in infos {
                self.source_infos.insert(info.root_path.clone(), info);
            }
            let new: std::collections::HashSet<_> =
                self.source_infos.values().map(|i| i.id).collect();
            dirty = old != new
                || self
                    .scan_roots
                    .iter()
                    // `contains_key` over a `str` — the previous `get(&…to_string())` allocated
                    // twice per root on a path that runs every frame.
                    .any(|r| self.source_infos.contains_key(r.to_string_lossy().as_ref()));
        }
        if dirty {
            self.start_watching();
        }
    }

    /// Kick off a background measurement of the thumbnail and workspace cache directories.
    fn refresh_storage_size(&mut self) {
        if self.storage_size_rx.is_some() {
            return;
        }
        let (thumb_dir, ws_dir) = default_cache_dirs();
        let (tx, rx) = mpsc::channel();
        self.storage_size_rx = Some(rx);
        std::thread::spawn(move || {
            let thumb = thumb_dir.as_deref().map_or(0, dir_size);
            let ws = ws_dir.as_deref().map_or(0, dir_size);
            let _ = tx.send((thumb, ws));
        });
    }

    /// Receive the latest cache-size measurement and cache it for the Settings Storage card.
    fn receive_storage_size(&mut self) {
        if let Some(rx) = &self.storage_size_rx {
            if let Ok((thumb, ws)) = rx.try_recv() {
                self.storage_size = Some((thumb, ws));
                self.storage_size_rx = None;
            }
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
        if pkg.source_idx == NO_SOURCE {
            self.jobs.toast(
                "Add the pack's folder as a library source before extracting from it",
                Intent::Warning,
            );
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

    /// (Re)start the filesystem watch over the current source roots. Dropping the old watcher
    /// unregisters its handles, so this is also how watching stops when a project closes.
    /// Watches only roots whose `import_source.watch_enabled` is true (UI-SPEC §11.1).
    fn start_watching(&mut self) {
        self.watcher = None;
        self.watch_dirty = false;
        if !self.config.watch_sources || self.scan_roots.is_empty() {
            return;
        }
        let watched: Vec<PathBuf> = self
            .scan_roots
            .iter()
            .filter(|r| {
                self.source_infos
                    .get(&r.to_string_lossy().to_string())
                    .map_or(true, |info| info.watch_enabled)
            })
            .cloned()
            .collect();
        if watched.is_empty() {
            return;
        }
        // A pack landing in a cache root is hundreds of events; one second of quiet is plenty
        // to let a copy finish before we look.
        self.watcher =
            lodestash_ingest::watch::watch_roots(&watched, std::time::Duration::from_secs(1));
    }

    /// Drain watch signals and rescan when the library is idle. Deliberately *not* immediate: a
    /// rescan rebuilds the entry list, so doing it under an active selection or an open Quick
    /// Look would yank the ground out from under the user. The flag just waits.
    fn poll_watch(&mut self, ctx: &egui::Context) {
        if let Some(w) = &self.watcher {
            while w.rx.try_recv().is_ok() {
                self.watch_dirty = true;
            }
        }
        let busy = self.scanning
            || self.catalog_persisting
            || self.pending_packages.iter().any(|p| p.extracting)
            || !self.selected.is_empty()
            || self.quick_look;
        // ponytail: fixed 30 s floor between watch rescans. Cheap guard against a chatty tree;
        // if a crawl ever costs more than that, scale it off the last scan's duration instead.
        let too_soon = self
            .last_watch_rescan
            .is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(30));
        if !self.watch_dirty || busy || too_soon {
            return;
        }
        self.watch_dirty = false;
        self.last_watch_rescan = Some(std::time::Instant::now());
        self.jobs
            .toast("Sources changed \u{2014} rescanning", Intent::Info);
        let roots = self.scan_roots.clone();
        let name = self.project_name.clone();
        self.load_paths(ctx, roots, name);
    }

    // ponytail: the spec's ◉ prefix is dropped — the bundled text fonts have no glyph for it and
    // it renders as tofu; it comes back with the icon sprite (DESIGN-GUIDELINES §5).
    /// Pack stats modal (UI-SPEC §15): tiles, per-type contents, heaviest assets, and a jump
    /// into the Library. Tags are P4 — the spec's empty state stands in until tagging exists.
    /// The server-side confirmation gate (MCP-AND-ENGINES §5). An agent can *ask* to write files
    /// to disk; nothing happens until the person at the machine approves it here. One prompt at a
    /// time, oldest first.
    fn agent_approval_modal(&mut self, ctx: &egui::Context) {
        let Some(state) = self.engine_state.clone() else {
            return;
        };
        let Some(job) = state.jobs.awaiting_approval().into_iter().next() else {
            return;
        };

        egui::Area::new(egui::Id::new("agent_approval_scrim"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                // No click-to-dismiss: this decision is deliberate, not incidental.
                ui.allocate_rect(screen, egui::Sense::hover());
                ui.painter()
                    .rect_filled(screen, 0.0, theme::BG.linear_multiply(0.6));
            });

        let mut decision: Option<bool> = None;
        egui::Window::new("agent_approval")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(460.0)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(theme::SURFACE_RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::WARNING))
                    .inner_margin(egui::Margin::same(20.0)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new("An agent is asking to write files")
                        .color(theme::TEXT)
                        .font(crate::fonts::display(18.0)),
                );
                ui.add_space(8.0);
                ui.label(RichText::new(&job.summary).color(theme::TEXT).size(13.0));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Originals are never moved or modified - this copies them.")
                        .color(theme::TEXT_DIM)
                        .size(11.0),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("Deny").size(13.0)).clicked() {
                        decision = Some(false);
                    }
                    if ui
                        .button(RichText::new("Approve").size(13.0).color(theme::GOLD))
                        .clicked()
                    {
                        decision = Some(true);
                    }
                });
            });

        match decision {
            Some(true) => {
                state.approve_job(&job.id);
                self.jobs
                    .toast(format!("Approved: {}", job.summary), Intent::Info);
            }
            Some(false) => {
                state.deny_job(&job.id);
                self.jobs
                    .toast("Denied the agent's export", Intent::Warning);
            }
            None => {}
        }
    }

    /// Render destructive-action confirmation modals (Trash and delete-from-disk). These are
    /// simple centered windows; the dangerous primary action is colored DANGER.
    fn render_modals(&mut self, ctx: &egui::Context) {
        // Dim the rest of the UI while any destructive confirmation is open.
        let any_open =
            self.pending_confirm_trash.is_some() || self.pending_delete_from_disk.is_some();
        if any_open {
            egui::Area::new(egui::Id::new("destructive_modal_scrim"))
                .order(egui::Order::Middle)
                .fixed_pos(egui::Pos2::ZERO)
                .show(ctx, |ui| {
                    let screen = ctx.screen_rect();
                    ui.allocate_rect(screen, egui::Sense::hover());
                    ui.painter()
                        .rect_filled(screen, 0.0, theme::BG.linear_multiply(0.6));
                });
        }

        let mut confirm_trash: Option<bool> = None;
        if let Some(targets) = &self.pending_confirm_trash {
            egui::Window::new("confirm_trash")
                .title_bar(false)
                .resizable(false)
                .collapsible(false)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .default_width(420.0)
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(theme::SURFACE_RAISED)
                        .stroke(egui::Stroke::new(1.0, theme::WARNING))
                        .inner_margin(egui::Margin::same(20.0)),
                )
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Move to Trash?")
                            .color(theme::TEXT)
                            .font(crate::fonts::display(18.0)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!(
                            "{} asset(s) will be moved to Trash. Files stay on disk and can be \
                             restored any time.",
                            targets.len()
                        ))
                        .color(theme::TEXT)
                        .size(13.0),
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Cancel").size(13.0)).clicked() {
                            confirm_trash = Some(false);
                        }
                        if ui
                            .button(
                                RichText::new("Move to Trash")
                                    .size(13.0)
                                    .color(theme::WARNING),
                            )
                            .clicked()
                        {
                            confirm_trash = Some(true);
                        }
                    });
                });
        }
        match confirm_trash {
            Some(true) => {
                let targets = self.pending_confirm_trash.take().unwrap_or_default();
                self.set_entries_trashed(&targets, true);
            }
            Some(false) => {
                self.pending_confirm_trash = None;
            }
            None => {}
        }

        let mut confirm_delete: Option<bool> = None;
        if let Some(targets) = &self.pending_delete_from_disk {
            egui::Window::new("confirm_delete_from_disk")
                .title_bar(false)
                .resizable(false)
                .collapsible(false)
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .default_width(460.0)
                .frame(
                    egui::Frame::window(&ctx.style())
                        .fill(theme::SURFACE_RAISED)
                        .stroke(egui::Stroke::new(2.0, theme::DANGER))
                        .inner_margin(egui::Margin::same(20.0)),
                )
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Delete file from disk?")
                            .color(theme::DANGER)
                            .font(crate::fonts::display(18.0)),
                    );
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!(
                            "{} file(s) will be permanently deleted from your disk. This cannot be \
                             undone and breaks the non-destructive default.",
                            targets.len()
                        ))
                        .color(theme::TEXT)
                        .size(13.0),
                    );
                    ui.add_space(4.0);
                    ui.label(
                        RichText::new(
                            "The catalog entry will remain in Trash as a record, but restoring it \
                             will not bring the file back.",
                        )
                        .color(theme::TEXT_DIM)
                        .size(11.0),
                    );
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Cancel").size(13.0)).clicked() {
                            confirm_delete = Some(false);
                        }
                        if ui
                            .button(
                                RichText::new("Delete from disk")
                                    .size(13.0)
                                    .color(theme::DANGER),
                            )
                            .clicked()
                        {
                            confirm_delete = Some(true);
                        }
                    });
                });
        }
        match confirm_delete {
            Some(true) => {
                let targets = self.pending_delete_from_disk.take().unwrap_or_default();
                self.delete_files_from_disk(&targets);
            }
            Some(false) => {
                self.pending_delete_from_disk = None;
            }
            None => {}
        }
    }

    fn pack_stats_modal(&mut self, ctx: &egui::Context) {
        let Some(pack) = self.pack_stats_open.clone() else {
            return;
        };
        let stats = pack_stats(&self.entries, &pack);
        let mut close = false;
        let mut browse = false;

        egui::Area::new(egui::Id::new("pack_stats_scrim"))
            .order(egui::Order::Middle)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                if ui.allocate_rect(screen, egui::Sense::click()).clicked() {
                    close = true;
                }
                ui.painter()
                    .rect_filled(screen, 0.0, theme::BG.linear_multiply(0.6));
            });

        egui::Window::new("pack_stats")
            .title_bar(false)
            .resizable(false)
            .collapsible(false)
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .default_width(620.0)
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(theme::SURFACE_RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .inner_margin(egui::Margin::same(20.0)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(&pack)
                            .color(theme::TEXT)
                            .font(crate::fonts::display(20.0)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(RichText::new("\u{2715}").size(13.0)).clicked() {
                            close = true;
                        }
                    });
                });
                ui.label(
                    RichText::new("extracted pack \u{00B7} license unknown")
                        .color(theme::TEXT_DIM)
                        .size(11.0)
                        .monospace(),
                );
                ui.add_space(10.0);

                // Four stat tiles.
                ui.horizontal(|ui| {
                    stat_tile(ui, "ASSETS", &stats.assets.to_string(), theme::TEXT);
                    stat_tile(ui, "ON DISK", &format_bytes(stats.bytes), theme::TEXT);
                    stat_tile(
                        ui,
                        "TRIANGLES",
                        &stats
                            .tris
                            .map(format_thousands)
                            .unwrap_or_else(|| "\u{2014}".into()),
                        theme::TEXT,
                    );
                    let (unid, color) = if stats.unidentified > 0 {
                        (stats.unidentified.to_string(), theme::WARNING)
                    } else {
                        ("0".to_string(), theme::TEXT_DIM)
                    };
                    stat_tile(ui, "UNIDENTIFIED", &unid, color);
                });
                ui.add_space(12.0);

                section_label(ui, "CONTENTS");
                for (mt, count, bytes) in &stats.contents {
                    ui.horizontal(|ui| {
                        let color = mt.map(theme::media_type_color).unwrap_or(theme::TEXT_DIM);
                        ui.label(RichText::new("\u{25C6}").color(color).size(10.0));
                        ui.label(
                            RichText::new(media_type_label(*mt))
                                .color(theme::TEXT_MUTED)
                                .size(11.0),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{count} \u{00B7} {}", format_bytes(*bytes)))
                                    .color(theme::TEXT_DIM)
                                    .size(11.0)
                                    .monospace(),
                            );
                        });
                    });
                }
                ui.add_space(10.0);

                section_label(ui, "TAGS IN THIS PACK");
                ui.label(
                    RichText::new("no tags yet \u{2014} assets in this pack are Unidentified")
                        .color(theme::TEXT_DIM)
                        .size(11.0),
                );
                ui.add_space(10.0);

                section_label(ui, "HEAVIEST IN PACK");
                for (name, bytes) in &stats.heaviest {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(name).color(theme::TEXT_MUTED).size(11.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format_bytes(*bytes))
                                    .color(theme::TEXT_DIM)
                                    .size(11.0)
                                    .monospace(),
                            );
                        });
                    });
                }
                ui.add_space(14.0);

                if ui
                    .button(
                        RichText::new(format!("Browse {pack} in Library \u{2192}"))
                            .color(theme::GOLD)
                            .size(12.0),
                    )
                    .clicked()
                {
                    browse = true;
                }
            });

        if browse {
            self.pack_stats_open = None;
            self.browse_package(pack);
        } else if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.pack_stats_open = None;
        }
    }

    /// Assets in the library per extracted pack: `(total, unidentified)` (UI-SPEC §4 pack row).
    /// Counted off the in-memory entries — they already carry their pack name, so this needs no
    /// catalog round-trip and stays correct for packs extracted this session.
    fn package_asset_counts(&self) -> std::collections::HashMap<String, (usize, usize)> {
        package_asset_counts(&self.entries)
    }

    /// Jump to the Library filtered to one pack (UI-SPEC §4 "Browse →").
    fn browse_package(&mut self, name: String) {
        self.source_filter = Some(LibraryFilter::Package(name));
        self.screen = Screen::Library;
        self.nav_to(None);
    }

    /// Open a pack in the **folder-explorer** (FEATURES.md §5.4): the library grid filtered to the
    /// pack, in Folders view rooted at the pack's top folder, with a fresh back/forward history.
    fn browse_package_folders(&mut self, name: String) {
        self.source_filter = Some(LibraryFilter::Package(name));
        self.screen = Screen::Library;
        // Switch to Folders for this browse without clobbering the saved default view mode.
        self.view_mode = ViewMode::Folders;
        // Root the explorer at the pack's own source; folder tiles below are its in-archive dirs.
        let src = self
            .entries
            .iter()
            .find(|e| matches!(&self.source_filter, Some(LibraryFilter::Package(p)) if e.package.as_deref() == Some(p.as_str())))
            .map(|e| e.source_idx)
            .unwrap_or(0);
        self.apply_dir(Some((src, PathBuf::new())));
        self.reset_nav_history();
    }

    /// Leave the pack folder explorer and return to the Unity Assets landing (FEATURES.md §5.3).
    fn return_to_unity(&mut self) {
        self.source_filter = None;
        self.screen = Screen::Unity;
        self.apply_dir(None);
        self.reset_nav_history();
    }

    /// Queue every selected pack that can actually be unpacked, then start the first one.
    /// Selections whose cache root isn't a library source yet are left ticked and reported —
    /// silently dropping them would look like the button did nothing.
    fn enqueue_selected_extracts(&mut self, ctx: &egui::Context) {
        let pending: Vec<PathBuf> = self
            .pending_packages
            .iter()
            .map(|p| p.abs_path.clone())
            .collect();
        let (ready, blocked) = split_extractable(&self.store_selected, &pending);
        for path in &ready {
            if !self.extract_queue.contains(path) {
                self.extract_queue.push_back(path.clone());
            }
            self.store_selected.remove(path);
        }
        if !blocked.is_empty() {
            self.jobs.toast(
                format!(
                    "{} pack(s) need their cache added as a library source first",
                    blocked.len()
                ),
                Intent::Warning,
            );
        }
        if !ready.is_empty() {
            self.jobs.toast(
                format!("Queued {} pack(s) to unpack", ready.len()),
                Intent::Info,
            );
        }
        self.pump_extract_queue(ctx);
    }

    /// Start the next queued pack once nothing is extracting. Called after enqueuing and after
    /// every finished extraction, so the queue drains itself.
    fn pump_extract_queue(&mut self, ctx: &egui::Context) {
        if self.pending_packages.iter().any(|p| p.extracting) {
            return;
        }
        while let Some(path) = self.extract_queue.pop_front() {
            if let Some(idx) = self
                .pending_packages
                .iter()
                .position(|p| p.abs_path == path && !p.extracting)
            {
                self.extract_package(ctx, idx);
                return;
            }
            // Dropped from pending (already extracted, or its source was removed) — skip it.
        }
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
        if pkg.source_idx == NO_SOURCE {
            self.jobs.toast(
                "Add the pack's folder as a library source before extracting from it",
                Intent::Warning,
            );
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

    /// Pull one entry of `pending_packages[idx]` to a scratch file and show it (§5.4).
    /// Same skip-scan as a surgical extraction, one locator wide.
    fn peek_entry(&mut self, ctx: &egui::Context, idx: usize, entry_idx: usize) {
        let Some(pkg) = self.pending_packages.get(idx) else {
            return;
        };
        let Some(e) = pkg.entries.get(entry_idx) else {
            return;
        };
        // `extractable()` gates the rows that can be clicked, so these are Some.
        let (Some(pathname), Some(offset), Some(size)) =
            (e.pathname.clone(), e.tar_offset, e.size_bytes)
        else {
            return;
        };
        let locator = unitypackage::EntryLocator {
            guid: e.guid.clone(),
            pathname: Some(pathname.clone()),
            offset: offset as u64,
            size: size as u64,
        };
        let abs_path = pkg.abs_path.clone();
        // ponytail: peeked files accumulate in the OS temp dir until the OS clears it —
        // content-addressed, so a re-peek costs nothing. Sweep on exit if it ever matters.
        let dest = std::env::temp_dir().join("lodestash-peek");
        let tx = self.peek_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match unitypackage::extract_at_offsets(&abs_path, &dest, vec![locator]) {
                Ok(assets) => match assets.into_iter().next() {
                    Some(a) => Ok(PackagePeek {
                        name: pathname
                            .rsplit_once('/')
                            .map_or(pathname.as_str(), |(_, f)| f)
                            .to_string(),
                        media_type: detect::by_extension(std::path::Path::new(&pathname)),
                        size_bytes: size as u64,
                        path: a.workspace_path,
                    }),
                    None => Err("the archive had no payload at that offset".into()),
                },
                Err(err) => Err(err.to_string()),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Hand a finished peek to whichever viewer owns its media type — the same viewers the
    /// grid's Quick Look drives, keyed on `PEEK_ENTRY_IDX` instead of a grid row.
    fn receive_peek(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.peek_rx.try_recv() {
            let peek = match msg {
                Ok(p) => p,
                Err(err) => {
                    self.jobs.toast(
                        format!("Couldn't preview that file \u{2014} {err}"),
                        Intent::Warning,
                    );
                    continue;
                }
            };
            match peek.media_type {
                Some(MediaType::Model3d) if viewport3d::is_viewable_model(&peek.path) => {
                    if let Some(vp) = &mut self.viewport3d {
                        vp.load(
                            PEEK_ENTRY_IDX,
                            peek.path.clone(),
                            std::collections::BTreeMap::new(),
                            ctx,
                        );
                    }
                }
                Some(MediaType::Audio) => {
                    if let Some(ap) = &mut self.audio_player {
                        ap.load(PEEK_ENTRY_IDX, peek.path.clone(), ctx);
                    }
                }
                mt if is_viewer_image(mt) => {
                    self.image_viewer.reset_for(PEEK_ENTRY_IDX);
                    let path = peek.path.clone();
                    let tx = self.viewer_tx.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || {
                        let result = load_viewer_image(&path);
                        let _ = tx.send(ViewerLoaded {
                            entry_idx: PEEK_ENTRY_IDX,
                            result,
                        });
                        ctx.request_repaint();
                    });
                }
                // Scripts, prefabs, materials: no viewer yet — the panel states the type.
                _ => {}
            }
            self.peek = Some(peek);
        }
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
            for file in msg.files.into_iter().filter(is_browseable) {
                self.entries.push(AssetEntry {
                    package: Some(msg.display_name.clone()),
                    file,
                    source_idx: msg.source_idx,
                    asset_id: None,
                    status: AssetStatus::Discovered,
                    trashed: false,
                    rating: None,
                    metrics: Default::default(),
                    content_hash: None,
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
            self.ai_filters = None;
            self.cache_dirty = true;
            return;
        }
        if self.search_catalog.is_none() {
            self.search_catalog = catalog_db_path().and_then(|p| Catalog::open(&p).ok());
        }
        let Some(cat) = &self.search_catalog else {
            return;
        };
        // §5.B step 1: split structural constraints out first. "rocks under 5mb" embeds
        // "rocks" — the size words would otherwise pollute the query vector as if they
        // described appearance — and becomes an exact filter instead.
        let parsed = lodestash_core::query::parse_query(&query);
        let semantic = parsed.semantic_text.clone();
        self.ai_filters = Some(parsed);

        // A filters-only query ("audio under 1mb") has nothing to embed: skip the vector arms
        // and let the structural filters do the work.
        if semantic.is_empty() {
            self.search_ranks = None;
            self.search_ranks_key = None;
            self.sort_by = SortBy::Name;
            self.cache_dirty = true;
            return;
        }

        // Embed the query into every loaded space: SigLIP (visual, 768-d) and CLAP (audio,
        // 512-d) are separate and non-comparable, so each is searched on its own and the
        // results fused on *rank* by RRF (§5.B "joint SigLIP/CLAP space").
        let visual = emb.embed_text(&semantic);
        let audio = self
            .audio_embedder
            .as_ref()
            .map(|a| (a.clone(), a.embed_text(&semantic)));
        let mut spaces: Vec<(&str, &[f32])> = vec![(emb.model_id(), &visual)];
        if let Some((a, v)) = &audio {
            spaces.push((a.model_id(), v));
        }
        // Bucket pre-filter is left to the grid's tab.matches (a Category spans several
        // media_types; hybrid_search takes a single one) — pass None here.
        let ids = cat
            .hybrid_search(&semantic, &spaces, None, HIT_CAP)
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

    /// Kick off the AI download on a worker thread (ONNX Runtime if missing, then the SigLIP
    /// model), tracked by a cancellable job-tray row. No-op if one is already running.
    fn start_siglip_download(&mut self) {
        if self.siglip_dl_job.is_some() {
            return;
        }
        let need_ort = !models::ort_installed();
        let label = if need_ort {
            "Downloading AI runtime + models (~645 MB)"
        } else {
            "Downloading AI models (~575 MB)"
        };
        let progress = Arc::new(JobProgress::default());
        let id = self.jobs.start(label.into(), progress.clone(), true);
        self.siglip_dl_job = Some(id);
        let tx = self.siglip_dl_tx.clone();
        std::thread::spawn(move || {
            let res = (|| {
                if need_ort {
                    models::download_ort(progress.clone())?;
                }
                models::download_siglip(progress.clone())?;
                models::download_clap(progress)
            })();
            let _ = tx.send(res);
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
                    // Point ort at the just-downloaded runtime before the first load (ort reads
                    // ORT_DYLIB_PATH when it dlopens). Fresh installs never touched ort at
                    // startup, so this first load picks it up live — no restart.
                    models::apply_ort_dylib_env();
                    self.ai_embedder = resolve_embedder();
                    self.audio_embedder = resolve_audio_embedder();
                    if self.ai_embedder.is_some() {
                        self.jobs.toast(
                            "Vision model ready — Search with AI is live",
                            Intent::Success,
                        );
                    } else {
                        // Files landed but the model won't load. If ort was already initialised
                        // this session (e.g. a stale runtime), a restart picks up the new one.
                        self.jobs.toast(
                            "Downloaded — restart Lodestash to finish enabling Search with AI",
                            Intent::Warning,
                        );
                    }
                }
                Err(e) if e == "cancelled" => {
                    self.jobs.toast("Model download cancelled", Intent::Info)
                }
                Err(e) => self
                    .jobs
                    .toast(format!("Model download failed: {e}"), Intent::Danger),
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

        // When a pack is the active filter, the folder explorer is scoped to it (its own tree).
        let pack: Option<String> = match &self.source_filter {
            Some(LibraryFilter::Package(p)) => Some(p.clone()),
            _ => None,
        };
        let (mut idxs, tiles): (Vec<usize>, Vec<FolderTile>) =
            match (self.view_mode, self.current_dir.clone()) {
                (ViewMode::Flat, _) | (ViewMode::List, _) => {
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
                                FolderTile {
                                    name: config::stem_name(root),
                                    direct: count,
                                    recursive: count,
                                }
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
                        folder_children(&self.entries, src, &dir, tab, pack.as_deref())
                    } else {
                        let idxs = self
                            .entries
                            .iter()
                            .enumerate()
                            .filter(|(_, e)| {
                                e.source_idx == src
                                    && e.file.rel_path.starts_with(&dir)
                                    && tab.matches(e.file.media_type)
                                    && pack
                                        .as_deref()
                                        .map_or(true, |p| e.package.as_deref() == Some(p))
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
            let (edges_out, edges_in) = (&self.edges_out, &self.edges_in);
            let asset_tags = &self.asset_tags;
            let favorites = &self.favorites;
            idxs.retain(|&i| {
                let id = entries[i].asset_id;
                let links = (
                    id.is_some_and(|id| edges_out.contains(&id)),
                    id.is_some_and(|id| edges_in.contains(&id)),
                );
                let traits: Vec<String> = id
                    .and_then(|id| asset_tags.get(&id))
                    .map(|tags| tags.iter().map(|t| t.value.clone()).collect())
                    .unwrap_or_default();
                cf.passes(
                    entries[i].file.size_bytes,
                    entries[i].metrics.tri_count.map(|t| t.max(0) as u32),
                    entries[i].rating,
                    &traits,
                    links,
                    id.is_some_and(|id| favorites.contains(&id)),
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

        // Structural constraints parsed out of the AI query (§5.B step 1) compose on top of
        // the fused ranking — "audio under 1mb" is a vector search AND two exact filters.
        if self.search_mode == SearchMode::Ai {
            if let Some(f) = self.ai_filters.as_ref().filter(|f| f.has_filters()) {
                let entries = &self.entries;
                idxs.retain(|&i| {
                    let e = &entries[i];
                    f.media_type.map_or(true, |mt| e.file.media_type == Some(mt))
                        // Unrated counts as 0, so it never satisfies a "4 stars" filter.
                        && f.min_rating.map_or(true, |r| e.rating.unwrap_or(0) >= r)
                        && f.max_size_bytes.map_or(true, |b| e.file.size_bytes as i64 <= b)
                        && f.min_size_bytes.map_or(true, |b| e.file.size_bytes as i64 >= b)
                        // Unmeasured facets can't satisfy a numeric range — an unknown tri
                        // count is not a small one (DATA-MODEL §14).
                        && f.max_tris.map_or(true, |t| e.metrics.tri_count.is_some_and(|n| n <= t))
                        && f.min_tris.map_or(true, |t| e.metrics.tri_count.is_some_and(|n| n >= t))
                        && f.max_duration_ms
                            .map_or(true, |d| e.metrics.duration_ms.is_some_and(|n| n <= d))
                        && f.min_duration_ms
                            .map_or(true, |d| e.metrics.duration_ms.is_some_and(|n| n >= d))
                        && f.status.map_or(true, |st| e.status.code() == st)
                });
            }
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

    /// Track pointer drag state for marquee selection over the grid/list.
    /// On release after a real drag, selects all visible cards whose rectangles intersect
    /// the marquee. Returns `true` if a marquee selection was just finalised, so the caller
    /// can suppress the ordinary click for this frame.
    fn update_drag_select(&mut self, ui: &egui::Ui, rects: &[(usize, usize, egui::Rect)]) -> bool {
        let (pos, down, released, mods) = ui.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.button_down(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Primary),
                i.modifiers,
            )
        });

        if down {
            if let Some(p) = pos {
                if self.drag_select_start.is_none() {
                    self.drag_select_start = Some(p);
                    self.drag_select_current = Some(p);
                    self.drag_select_active = false;
                } else {
                    self.drag_select_current = Some(p);
                    if let Some(start) = self.drag_select_start {
                        if p.distance(start) > 4.0 {
                            self.drag_select_active = true;
                        }
                    }
                }
            }
        } else if released {
            if self.drag_select_active {
                if let (Some(start), Some(current)) =
                    (self.drag_select_start, self.drag_select_current)
                {
                    let select_rect = egui::Rect::from_two_pos(start, current);
                    let add = mods.command || mods.shift;
                    if !add {
                        self.selected.clear();
                    }
                    let mut last_cache: Option<usize> = None;
                    for (cache_idx, entry_idx, rect) in rects {
                        if rect.intersects(select_rect) {
                            if add {
                                if !self.selected.insert(*entry_idx) {
                                    self.selected.remove(entry_idx);
                                }
                            } else {
                                self.selected.insert(*entry_idx);
                            }
                            last_cache = Some(*cache_idx);
                        }
                    }
                    if let Some(cache_idx) = last_cache {
                        self.selected_cache_idx = Some(cache_idx);
                        self.select_anchor = Some(cache_idx);
                    }
                }
                self.drag_select_start = None;
                self.drag_select_current = None;
                self.drag_select_active = false;
                return true;
            }
            self.drag_select_start = None;
            self.drag_select_current = None;
            self.drag_select_active = false;
        } else if !self.drag_select_active {
            self.drag_select_start = None;
            self.drag_select_current = None;
        }

        false
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
        self.reset_nav_history();
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
                            Screen::Duplicates => self.duplicate_groups,
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
        // A cache the user moved off the default drive is invisible to the env var and the per-OS
        // default — but if they added a pack folder from it as a library source, they have already
        // told us where it is. Infer it rather than making them configure the same path twice.
        for root in &self.scan_roots {
            if let Some(inferred) = asset_store::infer_root_from(root) {
                if !roots.contains(&inferred) {
                    tracing::info!(root = %inferred.display(), "inferred Asset Store cache root");
                    roots.push(inferred);
                }
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
        // Packs extracted in an earlier session have catalog rows but no entries yet; the
        // listing is how their counts survive a restart. Same lazily-loaded list the Unity
        // Assets screen uses.
        if self.unity_packages.is_none() {
            self.reload_unity_packages();
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
        let mut browse_pkg: Option<String> = None;
        let mut stats_pkg: Option<String> = None;

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

        // Batch bar (UI-SPEC §4 "batch unpack"): only meaningful once something is ticked.
        let mut enqueue_clicked = false;
        if !self.store_selected.is_empty() || !self.extract_queue.is_empty() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if !self.store_selected.is_empty() {
                    if ui
                        .button(
                            RichText::new(format!(
                                "\u{25B6} Extract selected ({})",
                                self.store_selected.len()
                            ))
                            .color(theme::GOLD)
                            .size(11.0),
                        )
                        .clicked()
                    {
                        enqueue_clicked = true;
                    }
                    if ui
                        .button(RichText::new("Clear selection").size(11.0))
                        .clicked()
                    {
                        self.store_selected.clear();
                    }
                }
                if !self.extract_queue.is_empty() {
                    ui.label(
                        RichText::new(format!("{} queued", self.extract_queue.len()))
                            .color(theme::TEXT_DIM)
                            .size(11.0),
                    );
                    if ui
                        .button(RichText::new("Cancel queue").size(11.0))
                        .clicked()
                    {
                        // Only the waiting packs — the one already unpacking has its own ✕ in
                        // the job tray.
                        self.extract_queue.clear();
                    }
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
            // Taken out of `self` for the closure, which also borrows `self.store_packages`.
            let mut selected = std::mem::take(&mut self.store_selected);
            let counts = self.package_asset_counts();
            let catalog_counts: std::collections::HashMap<&str, i64> = self
                .unity_packages
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter_map(|p| Some((p.package_path.as_str(), p.asset_count?)))
                .collect();
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
                        let all_ticked = pkgs.iter().all(|p| selected.contains(&p.path));
                        let label = if all_ticked { "none" } else { "all" };
                        if ui
                            .button(RichText::new(label).size(10.0))
                            .on_hover_text("Tick or untick every pack by this publisher")
                            .clicked()
                        {
                            for p in pkgs {
                                if all_ticked {
                                    selected.remove(&p.path);
                                } else {
                                    selected.insert(p.path.clone());
                                }
                            }
                        }
                    });
                    for pkg in pkgs {
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            let mut ticked = selected.contains(&pkg.path);
                            if ui.checkbox(&mut ticked, "").changed() {
                                if ticked {
                                    selected.insert(pkg.path.clone());
                                } else {
                                    selected.remove(&pkg.path);
                                }
                            }
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
                                        None if root_is_source => match counts.get(&pkg.name) {
                                            Some(&(total, unid)) => {
                                                if ui
                                                    .button(
                                                        RichText::new("Browse \u{2192}")
                                                            .color(theme::GOLD)
                                                            .size(11.0),
                                                    )
                                                    .clicked()
                                                {
                                                    browse_pkg = Some(pkg.name.clone());
                                                }
                                                if ui
                                                    .button(RichText::new("Stats").size(11.0))
                                                    .clicked()
                                                {
                                                    stats_pkg = Some(pkg.name.clone());
                                                }
                                                if unid > 0 {
                                                    ui.label(
                                                        RichText::new(format!("{unid} unid"))
                                                            .color(theme::WARNING)
                                                            .size(10.0),
                                                    );
                                                }
                                                ui.label(
                                                    RichText::new(format!("{total} assets"))
                                                        .color(theme::TEXT_DIM)
                                                        .size(10.0),
                                                );
                                            }
                                            None => {
                                                // No entries in memory: the pack was extracted
                                                // in an earlier session, so fall back to the
                                                // count the catalog recorded then.
                                                let stored = catalog_counts
                                                    .get(pkg.path.to_string_lossy().as_ref())
                                                    .copied();
                                                let label = match (self.scanning, stored) {
                                                    (true, _) => "scanning\u{2026}".to_string(),
                                                    (false, Some(n)) => {
                                                        format!("\u{2713} {n} assets")
                                                    }
                                                    (false, None) => {
                                                        "\u{2713} extracted".to_string()
                                                    }
                                                };
                                                ui.label(
                                                    RichText::new(label)
                                                        .color(theme::TEXT_DIM)
                                                        .size(10.0),
                                                );
                                            }
                                        },
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
            self.store_selected = selected;
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
        if enqueue_clicked {
            self.enqueue_selected_extracts(ctx);
        }
        if let Some(name) = browse_pkg {
            self.browse_package(name);
        }
        if let Some(name) = stats_pkg {
            self.pack_stats_open = Some(name);
        }
    }

    // ── Unity Assets screen (UI-SPEC §0.2, FEATURES.md §5) ──────────────────────

    /// The `.unitypackage` library manager: catalog `package` rows, each pinnable into
    /// the sidebar LIBRARIES facet (`package.pinned`, M7). Distinct from Packages (§4,
    /// the aggregate indexed-asset view). ponytail: this first slice is the listing +
    /// pin/unpin; the in-screen browse tree + per-asset preview (FEATURES §5.4) reuse
    /// the pending-packages bar's extraction for now and land as a follow-on.
    fn unity_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        if self.unity_packages.is_none() {
            self.reload_unity_packages();
        }
        let mut import_clicked = false;
        self.import_dialog(ctx);

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
            if ui
                .button(
                    RichText::new("+ Import packages\u{2026}")
                        .color(theme::ARCANE)
                        .size(11.0),
                )
                .on_hover_text("Scan a folder for .unitypackage files and import them as libraries")
                .clicked()
            {
                import_clicked = true;
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
                        "Import a folder of .unitypackage files, or extract one from the \
                         Packages screen, and it shows up here.",
                    )
                    .color(theme::TEXT_DIM)
                    .size(11.0),
                );
            });
            if import_clicked {
                self.open_import_dialog();
            }
            return;
        }

        let mut toggle: Option<(i64, bool)> = None;
        let mut open_pkg: Option<usize> = None;
        let mut browse_pkg: Option<String> = None;
        let mut stats_pkg: Option<String> = None;
        let counts = self.package_asset_counts();
        ScrollArea::vertical().show(ui, |ui| {
            for (pkg_idx, pkg) in packages.iter().enumerate() {
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
                                    // §5.3 "Open →": the pick-and-extract contents list,
                                    // rebuilt from the persisted manifest — works in a
                                    // session that never crawled this pack.
                                    if ui
                                        .button(RichText::new("Open \u{2192}").size(11.0))
                                        .clicked()
                                    {
                                        open_pkg = Some(pkg_idx);
                                    }
                                    if let Some(n) = pkg.asset_count {
                                        // Extracted this session ⇒ its assets are in the grid,
                                        // so the row can jump straight there (UI-SPEC §4).
                                        // Both need this pack's assets in memory: Browse filters
                                        // the grid, and the stats are computed off the entries.
                                        if counts.contains_key(&pkg.name) {
                                            if ui
                                                .button(
                                                    RichText::new("Browse \u{2192}")
                                                        .color(theme::GOLD)
                                                        .size(11.0),
                                                )
                                                .clicked()
                                            {
                                                browse_pkg = Some(pkg.name.clone());
                                            }
                                            if ui
                                                .button(RichText::new("Stats").size(11.0))
                                                .clicked()
                                            {
                                                stats_pkg = Some(pkg.name.clone());
                                            }
                                        }
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
        if let Some(i) = open_pkg {
            if let Some(pkg) = packages.get(i) {
                self.open_package_browse(ctx, pkg);
            }
        }
        if import_clicked {
            self.open_import_dialog();
        }
        if let Some(name) = browse_pkg {
            self.browse_package(name);
        }
        if let Some(name) = stats_pkg {
            self.pack_stats_open = Some(name);
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
        let mut unpinned_name = None;
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
                if !pinned {
                    unpinned_name = Some(p.name.clone());
                }
            }
        }
        // Removing drops the facet, pulls the pack's assets out of the grid, and resets the
        // filter to All if that library was the active one (FEATURES.md §5.5). The catalog
        // rows stay — this is a library membership change, not a delete.
        if let Some(name) = unpinned_name {
            self.entries
                .retain(|e| e.package.as_deref() != Some(name.as_str()));
            if matches!(&self.source_filter, Some(LibraryFilter::Package(p)) if *p == name) {
                self.source_filter = None;
            }
            self.selected_cache_idx = None;
            self.selected.clear();
            self.cache_dirty = true;
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

    /// Open a catalog package for browse/pick-and-extract (FEATURES.md §5.4). The manifest
    /// locators were persisted when the pack was first indexed, so this needs no second scan
    /// of the archive — which is what makes a pack's contents retrievable in a later session.
    /// **Open →** a pack (FEATURES.md §5.4): add it as a library and browse it in the folder
    /// explorer. Opening pins the pack, tracks its source folder as a pack-only scan root, mints
    /// its in-pack assets in the background (no extraction — bytes are pulled on demand), and jumps
    /// into its folders once they land. Re-opening an already-surfaced pack is instant.
    fn open_package_browse(
        &mut self,
        ctx: &egui::Context,
        pkg: &lodestash_core::catalog::PackageListing,
    ) {
        let name = pkg.name.clone();
        // Already surfaced in the grid → straight into its folders.
        if self
            .entries
            .iter()
            .any(|e| e.package.as_deref() == Some(name.as_str()))
        {
            self.browse_package_folders(name);
            return;
        }
        let abs_path = PathBuf::from(&pkg.package_path);
        if !abs_path.exists() {
            self.jobs.toast(
                format!("Archive is gone from {}", pkg.package_path),
                Intent::Warning,
            );
            return;
        }
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        let package_id = pkg.id;

        // Track the pack's source folder as a pack-only library so the hydrate surfaces its
        // in-pack assets (they carry that source's id). Then load that project view.
        let proj = self.project_name.clone().unwrap_or_else(|| {
            let root = abs_path
                .parent()
                .map(PathBuf::from)
                .unwrap_or(abs_path.clone());
            self.config.record_open(root)
        });
        if let Ok(Some(root)) =
            Catalog::open(&db_path).and_then(|c| c.package_source_root(package_id))
        {
            self.config.add_pack_only_source(&proj, PathBuf::from(root));
        }

        // Browse into the pack once its assets land (set before the background mint kicks off).
        self.pending_pack_browse = Some(name.clone());

        // Background: pin + ensure minted, then signal a re-hydrate.
        let tx = self.rehydrate_tx.clone();
        let ctx2 = ctx.clone();
        let path = abs_path.clone();
        std::thread::spawn(move || {
            if let Ok(c) = Catalog::open(&db_path) {
                if let Err(err) = c.set_package_pinned(package_id, true) {
                    tracing::warn!(%err, package_id, "open: pin failed");
                }
            }
            if let Err(err) = ensure_in_pack_minted(&db_path, package_id, &path) {
                tracing::warn!(%err, package_id, "open: in-pack mint failed");
            }
            let _ = tx.send(());
            ctx2.request_repaint();
        });

        self.jobs
            .toast(format!("Opening {name}\u{2026}"), Intent::Success);
        // Bring the pack's source folder into the active project view; the crawl skips it
        // (pack-only) and the hydrate surfaces its packs once the background mint signals.
        let live: Vec<PathBuf> = self
            .config
            .project_by_name(&proj)
            .map(|p| p.live_paths().iter().map(|p| p.to_path_buf()).collect())
            .unwrap_or_default();
        self.load_paths(ctx, live, Some(proj));
    }

    /// The §5.2 dialog: the packs found under the picked folder, grouped by the folder that
    /// holds them (which is the publisher/category tree in an Asset Store cache), each
    /// tickable, importing as one Library apiece.
    fn import_dialog(&mut self, ctx: &egui::Context) {
        let Some(scan) = &mut self.import_scan else {
            return;
        };
        let mut open = true;
        let mut import_now = false;
        let mut cancel = false;
        egui::Window::new("Import packages")
            .id(egui::Id::new("import_packages"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size([620.0, 480.0])
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::window(&ctx.style())
                    .fill(theme::SURFACE_RAISED)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(scan.root.to_string_lossy())
                        .color(theme::TEXT_MUTED)
                        .size(11.0),
                );
                ui.add_space(6.0);
                if scan.scanning {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(14.0));
                        ui.label(
                            RichText::new("looking for .unitypackage files\u{2026}")
                                .color(theme::TEXT_DIM)
                                .size(11.0),
                        );
                    });
                    return;
                }
                if scan.found.is_empty() {
                    ui.label(
                        RichText::new("No .unitypackage files in that folder.")
                            .color(theme::TEXT_DIM)
                            .size(12.0),
                    );
                    return;
                }

                // Contiguous runs of the sorted list — the scan sorted by (group, name).
                let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
                for (i, (group, _, _, _)) in scan.found.iter().enumerate() {
                    match groups.last_mut() {
                        Some((g, members)) if g == group => members.push(i),
                        _ => groups.push((group.clone(), vec![i])),
                    }
                }
                egui::ScrollArea::vertical()
                    .max_height(330.0)
                    .show(ui, |ui| {
                        for (group, members) in &groups {
                            let label = if group.is_empty() {
                                "(this folder)"
                            } else {
                                group.as_str()
                            };
                            let picked =
                                members.iter().filter(|m| scan.selected.contains(m)).count();
                            egui::CollapsingHeader::new(
                                RichText::new(format!(
                                    "{label}  \u{00B7}  {picked}/{}",
                                    members.len()
                                ))
                                .color(if picked > 0 {
                                    theme::GOLD
                                } else {
                                    theme::TEXT_MUTED
                                })
                                .size(11.0),
                            )
                            .id_salt(("import_group", group))
                            .default_open(groups.len() <= 8)
                            .show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    if ui.small_button("All").clicked() {
                                        scan.selected.extend(members.iter().copied());
                                    }
                                    if ui.small_button("None").clicked() {
                                        scan.selected.retain(|s| !members.contains(s));
                                    }
                                });
                                for &i in members {
                                    let (_, _, name, size) = &scan.found[i];
                                    ui.horizontal(|ui| {
                                        let mut on = scan.selected.contains(&i);
                                        if ui.checkbox(&mut on, "").changed() {
                                            if on {
                                                scan.selected.insert(i);
                                            } else {
                                                scan.selected.remove(&i);
                                            }
                                        }
                                        ui.label(RichText::new(name).color(theme::TEXT).size(11.0));
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                ui.label(
                                                    RichText::new(format_bytes(*size))
                                                        .color(theme::TEXT_MUTED)
                                                        .size(10.0),
                                                );
                                            },
                                        );
                                    });
                                }
                            });
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("indexed in place \u{2014} no files are copied")
                            .color(theme::TEXT_DIM)
                            .size(10.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let n = scan.selected.len();
                        if ui
                            .add_enabled(
                                n > 0,
                                egui::Button::new(
                                    RichText::new(format!("Import as libraries ({n})"))
                                        .color(theme::GOLD)
                                        .size(11.0),
                                ),
                            )
                            .clicked()
                        {
                            import_now = true;
                        }
                        if ui.button(RichText::new("Cancel").size(11.0)).clicked() {
                            cancel = true;
                        }
                        ui.label(
                            RichText::new(format!("{n} selected"))
                                .color(theme::TEXT_MUTED)
                                .size(10.0),
                        );
                    });
                });
            });
        if import_now {
            self.import_selected_packages(ctx);
        } else if cancel || !open {
            self.import_scan = None;
        }
    }

    /// §5.2 step 1: pick a folder and scan it for `.unitypackage` files, off-thread (an
    /// Asset Store cache is thousands of files deep).
    fn open_import_dialog(&mut self) {
        let Some(root) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.import_scan = Some(ImportScan {
            root: root.clone(),
            scanning: true,
            found: Vec::new(),
            selected: std::collections::BTreeSet::new(),
        });
        let tx = self.import_tx.clone();
        std::thread::spawn(move || {
            let mut found: Vec<_> = lodestash_ingest::crawler::crawl(&root, &[])
                .into_iter()
                .filter(|f| is_unitypackage(&f.abs_path))
                .map(|f| {
                    let group = f
                        .rel_path
                        .parent()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_default();
                    let name = package_display_name(&f.rel_path);
                    (group, f.abs_path, name, f.size_bytes)
                })
                .collect();
            found.sort_by(|a, b| (&a.0, &a.2).cmp(&(&b.0, &b.2)));
            let _ = tx.send(found);
        });
    }

    fn receive_import_scan(&mut self) {
        while let Ok(found) = self.import_rx.try_recv() {
            if let Some(scan) = &mut self.import_scan {
                scan.found = found;
                scan.scanning = false;
            }
        }
    }

    /// A background in-pack mint (import / Add-to-Library) finished — rebuild the grid so the new
    /// pack libraries appear (M14). ponytail: reuses `load_paths` (the crawl skips the pack-only
    /// folder, the hydrate inside surfaces the minted assets); a hydrate-only path is the
    /// optimisation if a re-crawl per import ever shows.
    fn receive_rehydrate(&mut self, ctx: &egui::Context) {
        let mut got = false;
        while self.rehydrate_rx.try_recv().is_ok() {
            got = true;
        }
        if got && !self.scan_roots.is_empty() {
            self.load_paths(ctx, self.scan_roots.clone(), self.project_name.clone());
        }
    }

    /// §5.2 step 2: each selected archive becomes one Library — an `import_source` for the
    /// picked folder plus a **pinned** `package` row per pack. Nothing is copied or unpacked;
    /// the row points at the archive where it already lives, and its contents are read on the
    /// first Open →.
    fn import_selected_packages(&mut self, ctx: &egui::Context) {
        let Some(scan) = &self.import_scan else {
            return;
        };
        let picks: Vec<(PathBuf, String)> = scan
            .selected
            .iter()
            .filter_map(|&i| scan.found.get(i))
            .map(|(_, path, name, _)| (path.clone(), name.clone()))
            .collect();
        if picks.is_empty() {
            return;
        }
        let Some(db_path) = catalog_db_path() else {
            return;
        };
        let root = scan.root.clone();
        let n = picks.len();
        self.import_scan = None;

        // Sync: pin each pack (fast — no archive read) and collect (id, path) for the background
        // mint. The manifest read + mint is deferred because it streams the whole gzip per pack.
        let result = (|| -> Result<Vec<(i64, PathBuf)>, String> {
            let catalog = Catalog::open(&db_path).map_err(|e| e.to_string())?;
            let sid = catalog
                .upsert_import_source(&config::stem_name(&root), &root.to_string_lossy(), "folder")
                .map_err(|e| e.to_string())?;
            let mut ids = Vec::with_capacity(picks.len());
            for (path, name) in &picks {
                // Publisher/category are the Asset Store cache's own two levels above the
                // pack; a plain folder of packs simply has neither.
                let (publisher, category) = store_publisher_category(&root, path);
                let id = catalog
                    .upsert_package(
                        sid,
                        &path.to_string_lossy(),
                        name,
                        publisher.as_deref(),
                        category.as_deref(),
                    )
                    .map_err(|e| e.to_string())?;
                catalog
                    .set_package_pinned(id, true)
                    .map_err(|e| e.to_string())?;
                ids.push((id, path.clone()));
            }
            Ok(ids)
        })();

        match result {
            Ok(ids) => {
                // The folder is a **pack-only** container (FEATURES.md §5.2): tracked as a library
                // source but not crawled for loose files — its packs surface as in-pack libraries.
                let proj = self
                    .project_name
                    .clone()
                    .unwrap_or_else(|| self.config.record_open(root.clone()));
                self.config.add_pack_only_source(&proj, root.clone());

                // Background: read each pack's manifest (if needed) + mint its in-pack assets,
                // then signal a re-hydrate so the new libraries appear without a restart (M14).
                let db2 = db_path.clone();
                let tx = self.rehydrate_tx.clone();
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    let mut minted = 0usize;
                    for (id, path) in ids {
                        match ensure_in_pack_minted(&db2, id, &path) {
                            Ok(k) => minted += k,
                            Err(err) => {
                                tracing::warn!(%err, package_id = id, "in-pack mint failed")
                            }
                        }
                    }
                    tracing::info!(minted, "import minted in-pack assets");
                    let _ = tx.send(());
                    ctx2.request_repaint();
                });

                self.jobs.toast(
                    format!("Imported {n} package(s) as libraries"),
                    Intent::Success,
                );
                // Bring the folder into the active project view; the crawl skips it (pack-only)
                // and the hydrate surfaces its packs once the background mint signals.
                let live: Vec<PathBuf> = self
                    .config
                    .project_by_name(&proj)
                    .map(|p| p.live_paths().iter().map(|p| p.to_path_buf()).collect())
                    .unwrap_or_default();
                self.load_paths(ctx, live, Some(proj));
                self.reload_unity_packages();
            }
            Err(err) => {
                tracing::warn!(%err, "package import failed");
                self.jobs
                    .toast(format!("Import failed \u{2014} {err}"), Intent::Danger);
            }
        }
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
        let mut delete_from_disk: Option<usize> = None;
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
                                    if ui
                                        .button(
                                            RichText::new("\u{2716} Delete from disk")
                                                .color(theme::DANGER)
                                                .size(11.0),
                                        )
                                        .clicked()
                                    {
                                        delete_from_disk = Some(i);
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
        if let Some(i) = delete_from_disk {
            self.pending_delete_from_disk = Some(vec![i]);
        }
    }

    // ── Settings screen (design §5) ────────────────────────────────────────────

    fn settings_screen(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut save = false;
        let mut watch_changed = false;
        let mut quality_changed = false;
        let mut start_dl = false;
        // Per-source actions collected during render, applied after.
        let mut toggle_expand: Option<usize> = None;
        let mut add_exclude: Option<(usize, lodestash_core::types::Exclude)> = None;
        let mut remove_exclude: Option<(usize, lodestash_core::types::Exclude)> = None;
        let mut rescan: Option<usize> = None;
        let mut toggle_watch: Option<(usize, bool)> = None;

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
                            let mut master_watch = self.config.watch_sources;
                            if ui
                                .checkbox(
                                    &mut master_watch,
                                    RichText::new("Watch sources for changes").size(12.0),
                                )
                                .on_hover_text(
                                    "Rescan automatically when files appear or change under a \
                                     source root (DATA-MODEL \u{00A7}6). Per-source pause toggles \
                                     below override this for each source.",
                                )
                                .changed()
                            {
                                self.config.watch_sources = master_watch;
                                save = true;
                                watch_changed = true;
                            }
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
                                let root_str = root.to_string_lossy().to_string();
                                let info = self.source_infos.get(&root_str);
                                let watch = info.map_or(true, |s| s.watch_enabled);
                                let master_watch = self.config.watch_sources;
                                ui.horizontal(|ui| {
                                    let (dot, p) = ui
                                        .allocate_painter(Vec2::splat(10.0), egui::Sense::hover());
                                    let dot_color = if watch && master_watch {
                                        theme::GOLD
                                    } else {
                                        theme::TEXT_DIM
                                    };
                                    p.circle_filled(dot.rect.center(), 4.0, dot_color);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            RichText::new(config::stem_name(root))
                                                .size(13.0)
                                                .color(theme::TEXT),
                                        );
                                        let mut meta_parts = Vec::new();
                                        if let Some(info) = info {
                                            meta_parts.push(format!("{} assets", info.asset_count));
                                            meta_parts.push(info.kind.replace('_', " "));
                                            if let Some(ts) = info.last_scanned_at {
                                                meta_parts.push(format!(
                                                    "scanned {}",
                                                    format_relative_time(ts)
                                                ));
                                            } else {
                                                meta_parts.push("not scanned".into());
                                            }
                                        }
                                        if !meta_parts.is_empty() {
                                            ui.label(
                                                RichText::new(meta_parts.join(" \u{B7} "))
                                                    .size(10.0)
                                                    .color(theme::TEXT_DIM)
                                                    .monospace(),
                                            );
                                        }
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
                                            // Rescan this source.
                                            if ui
                                                .add(
                                                    egui::Button::new(
                                                        RichText::new("\u{21BB} rescan")
                                                            .size(11.0)
                                                            .color(theme::ARCANE),
                                                    )
                                                    .frame(false),
                                                )
                                                .clicked()
                                            {
                                                rescan = Some(i);
                                            }
                                            // Watch toggle per source.
                                            let mut watch_value = watch;
                                            if ui
                                                .checkbox(
                                                    &mut watch_value,
                                                    RichText::new("watch").size(11.0),
                                                )
                                                .changed()
                                            {
                                                toggle_watch = Some((i, watch_value));
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
                        let audio_loaded = self.audio_embedder.is_some();
                        let ai_downloading = self.siglip_dl_job.is_some();
                        let ai_files = models::siglip_installed() && models::clap_installed();
                        settings_card(ui, "AI \u{00B7} SEARCH MODEL", |ui| {
                            if ai_loaded {
                                ui.label(
                                    RichText::new(if audio_loaded {
                                        "\u{2713} SigLIP 2 + CLAP loaded \u{2014} Search with AI covers \
                                         images, 3D models and audio."
                                    } else {
                                        "\u{2713} SigLIP 2 loaded \u{2014} visual Search with AI is active \
                                         (audio model not installed)."
                                    })
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
                                        "Model files present but not loaded \u{2014} the ONNX \
                                         Runtime is missing. Click below to fetch it, or restart if \
                                         you just installed it.",
                                    )
                                    .size(12.0)
                                    .color(theme::WARNING),
                                );
                            } else {
                                let need_ort = !models::ort_installed();
                                let (blurb, label) = if need_ort {
                                    (
                                        "Describe an asset in plain language and find the ones that \
                                         look like it, and audio by how it sounds. Downloads the ONNX \
                                         Runtime (1.22) + SigLIP 2 base + CLAP (~645 MB total) for fully \
                                         offline, CPU-friendly search.",
                                        "\u{2B73} Download AI runtime + models (~645 MB)",
                                    )
                                } else {
                                    (
                                        "Downloads SigLIP 2 base + CLAP (~575 MB) for fully offline, \
                                         CPU-friendly visual and audio search. Runtime already installed.",
                                        "\u{2B73} Download AI models (~575 MB)",
                                    )
                                };
                                ui.label(RichText::new(blurb).size(11.0).color(theme::TEXT_DIM));
                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            RichText::new(label).size(12.0).color(theme::ARCANE),
                                        )
                                        .stroke(egui::Stroke::new(1.0, theme::ARCANE))
                                        .rounding(6.0),
                                    )
                                    .clicked()
                                {
                                    start_dl = true;
                                }
                                // Offline / manual fallback (the version is strict — ORT 1.22.x).
                                if let (Some(md), Some(rd)) =
                                    (models::siglip_dir(), models::runtime_dir())
                                {
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(
                                            "Offline install: put onnxruntime.dll (ORT 1.22, win-x64) in",
                                        )
                                        .size(10.0)
                                        .color(theme::TEXT_DIM),
                                    );
                                    ui.label(
                                        RichText::new(rd.to_string_lossy())
                                            .size(10.0)
                                            .monospace()
                                            .color(theme::TEXT_MUTED),
                                    );
                                    ui.label(
                                        RichText::new(
                                            "and the SigLIP vision.onnx / text.onnx / tokenizer.json in",
                                        )
                                        .size(10.0)
                                        .color(theme::TEXT_DIM),
                                    );
                                    ui.label(
                                        RichText::new(md.to_string_lossy())
                                            .size(10.0)
                                            .monospace()
                                            .color(theme::TEXT_MUTED),
                                    );
                                }
                            }
                        });
                        // AI · IDENTIFY — local tagging through an Ollama model already on the
                        // machine (AI-PIPELINE §2). Manual: a full pass is hours of CPU.
                        let mut start_tagging = false;
                        let tagging_running = self.tag_job.is_some();
                        settings_card(ui, "AI \u{00B7} IDENTIFY", |ui| {
                            let models = self.ollama_models.get_or_insert_with(|| {
                                lodestash_ai::ollama::OllamaTagger::installed_models(
                                    lodestash_ai::ollama::DEFAULT_HOST,
                                )
                                .unwrap_or_default()
                            });
                            let vision_candidates: Vec<String> = models
                                .iter()
                                .filter(|m| is_vision_model(m))
                                .cloned()
                                .collect();
                            if models.is_empty() {
                                ui.label(
                                    RichText::new(
                                        "No local model found. Install Ollama and pull a vision \
                                         model (moondream ~1.7 GB, or qwen2.5vl:3b ~3 GB), then \
                                         reopen Settings.",
                                    )
                                    .size(11.0)
                                    .color(theme::TEXT_DIM),
                                );
                                return;
                            }
                            ui.label(
                                RichText::new(
                                    "Tags assets with a model already on this machine \u{2014} \
                                     nothing leaves the box. Vision models describe the preview; \
                                     text-only models read the asset's name.",
                                )
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                            );
                            ui.add_space(6.0);
                            // Manual by default (AI-PIPELINE §2): identification is CPU-heavy, so it
                            // runs only when the user asks — unless auto is enabled, which drains new
                            // unidentified assets in the background as they appear.
                            if ui
                                .checkbox(
                                    &mut self.config.auto_identify,
                                    "Auto-identify new assets as they appear",
                                )
                                .on_hover_text(
                                    "Off (default): identify runs only when you press the button \
                                     below. On: embeddings + tags run automatically in the \
                                     background — heavier on CPU.",
                                )
                                .changed()
                            {
                                save = true;
                            }
                            ui.add_space(6.0);
                            let current = self
                                .config
                                .tagger_model
                                .clone()
                                .unwrap_or_else(|| "\u{2014} off \u{2014}".into());
                            egui::ComboBox::from_id_salt("tagger_model")
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    if ui
                                        .selectable_label(
                                            self.config.tagger_model.is_none(),
                                            "\u{2014} off \u{2014}",
                                        )
                                        .clicked()
                                    {
                                        self.config.tagger_model = None;
                                        save = true;
                                    }
                                    for m in models.iter() {
                                        if ui
                                            .selectable_label(
                                                self.config.tagger_model.as_deref() == Some(m),
                                                m,
                                            )
                                            .clicked()
                                        {
                                            self.config.tagger_model = Some(m.to_string());
                                            save = true;
                                        }
                                    }
                                });
                            if self.config.tagger_model.is_some() {
                                // Optional captioning stage. Two models, because a small VLM
                                // captions well but cannot hold a JSON schema (see OllamaTagger).
                                let vision_label = self
                                    .config
                                    .tagger_vision_model
                                    .clone()
                                    .unwrap_or_else(|| "\u{2014} name only \u{2014}".into());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new("Preview captions")
                                            .size(11.0)
                                            .color(theme::TEXT_DIM),
                                    );
                                    egui::ComboBox::from_id_salt("tagger_vision")
                                        .selected_text(vision_label)
                                        .show_ui(ui, |ui| {
                                            if ui
                                                .selectable_label(
                                                    self.config.tagger_vision_model.is_none(),
                                                    "\u{2014} name only \u{2014}",
                                                )
                                                .clicked()
                                            {
                                                self.config.tagger_vision_model = None;
                                                save = true;
                                            }
                                            for m in vision_candidates.iter() {
                                                if ui
                                                    .selectable_label(
                                                        self.config.tagger_vision_model.as_deref()
                                                            == Some(m),
                                                        m,
                                                    )
                                                    .clicked()
                                                {
                                                    self.config.tagger_vision_model =
                                                        Some(m.to_string());
                                                    save = true;
                                                }
                                            }
                                        });
                                });
                                ui.add_space(6.0);
                                let label = if tagging_running {
                                    "Identifying\u{2026} see the job tray".to_string()
                                } else {
                                    "\u{2726} Identify unidentified assets".to_string()
                                };
                                if ui
                                    .add_enabled(
                                        !tagging_running,
                                        egui::Button::new(
                                            RichText::new(label).size(12.0).color(theme::ARCANE),
                                        )
                                        .stroke(egui::Stroke::new(1.0, theme::ARCANE))
                                        .rounding(6.0),
                                    )
                                    .clicked()
                                {
                                    start_tagging = true;
                                }
                                ui.label(
                                    RichText::new(
                                        "Runs on CPU: seconds per asset, so a large library takes \
                                         hours. Cancel from the tray \u{2014} it resumes where it \
                                         stopped.",
                                    )
                                    .size(10.0)
                                    .color(theme::TEXT_DIM),
                                );
                            }
                        });
                        if start_tagging {
                            self.spawn_tagging();
                        }
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
                            // Start measuring cache directories once the Settings screen is open.
                            if self.storage_size.is_none() && self.storage_size_rx.is_none() {
                                self.refresh_storage_size();
                            }
                            let (thumb_used, ws_used) = self.storage_size.unwrap_or((0, 0));

                            // Keep extracted packs cached toggle.
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
                            ui.add_space(10.0);

                            // Preview cache cap.
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Preview cache limit").size(12.0).color(theme::TEXT),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{} GB",
                                                self.config.preview_cache_cap_gb
                                            ))
                                            .size(12.0)
                                            .color(theme::TEXT_DIM)
                                            .monospace(),
                                        );
                                    },
                                );
                            });
                            let mut preview_cap = self.config.preview_cache_cap_gb;
                            if ui
                                .add(
                                    egui::Slider::new(&mut preview_cap, 1..=16)
                                        .show_value(false)
                                        .suffix(" GB"),
                                )
                                .changed()
                            {
                                self.config.preview_cache_cap_gb = preview_cap;
                                save = true;
                            }
                            // Preview-cache usage bar scaled by on-screen quality.
                            let quality_scale = match self.config.preview_quality {
                                config::PreviewQuality::Low => 0.5,
                                config::PreviewQuality::Medium => 1.0,
                                config::PreviewQuality::High => 2.0,
                            };
                            let preview_used = (thumb_used as f64 * quality_scale) as u64;
                            let preview_cap_bytes =
                                self.config.preview_cache_cap_gb as u64 * (1u64 << 30);
                            let preview_frac = if preview_cap_bytes > 0 {
                                ((preview_used as f64 / preview_cap_bytes as f64) as f32)
                                    .clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let preview_color = if preview_used > preview_cap_bytes {
                                theme::DANGER
                            } else if preview_used as f64
                                > preview_cap_bytes as f64 * 0.8
                            {
                                theme::WARNING
                            } else {
                                theme::GOLD
                            };
                            let preview_text = if preview_frac > 0.0 || thumb_used > 0 {
                                format!(
                                    "{} of {} used",
                                    format_bytes(preview_used),
                                    format_bytes(preview_cap_bytes)
                                )
                            } else {
                                "No preview cache yet".to_string()
                            };
                            ui.add(
                                egui::ProgressBar::new(preview_frac)
                                    .fill(preview_color)
                                    .text(RichText::new(preview_text).size(11.0)),
                            );
                            ui.add_space(6.0);

                            // Auto-purge toggle.
                            let mut auto_purge = self.config.auto_purge_preview_cache;
                            if ui
                                .checkbox(
                                    &mut auto_purge,
                                    RichText::new("Auto-purge over cap").size(12.0),
                                )
                                .on_hover_text(
                                    "Drop the least-recently-viewed previews when the cache \
                                     exceeds the cap.",
                                )
                                .changed()
                            {
                                self.config.auto_purge_preview_cache = auto_purge;
                                save = true;
                            }

                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("Extracted archive workspace")
                                    .size(12.0)
                                    .color(theme::TEXT),
                            );
                            // Workspace cache cap.
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Workspace cap").size(11.0).color(theme::TEXT_MUTED),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(format!(
                                                "{} GB",
                                                self.config.workspace_cache_cap_gb
                                            ))
                                            .size(11.0)
                                            .color(theme::TEXT_DIM)
                                            .monospace(),
                                        );
                                    },
                                );
                            });
                            let mut ws_cap = self.config.workspace_cache_cap_gb;
                            if ui
                                .add(
                                    egui::Slider::new(&mut ws_cap, 1..=128)
                                        .show_value(false)
                                        .suffix(" GB"),
                                )
                                .changed()
                            {
                                self.config.workspace_cache_cap_gb = ws_cap;
                                save = true;
                            }
                            let ws_cap_bytes =
                                self.config.workspace_cache_cap_gb as u64 * (1u64 << 30);
                            let ws_frac = if ws_cap_bytes > 0 {
                                ((ws_used as f64 / ws_cap_bytes as f64) as f32).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            let ws_color = if ws_used > ws_cap_bytes {
                                theme::DANGER
                            } else if ws_used as f64 > ws_cap_bytes as f64 * 0.8 {
                                theme::WARNING
                            } else {
                                theme::ARCANE
                            };
                            let ws_text = if ws_used > 0 {
                                format!(
                                    "{} of {} used",
                                    format_bytes(ws_used),
                                    format_bytes(ws_cap_bytes)
                                )
                            } else {
                                "No extracted workspace yet".to_string()
                            };
                            ui.add(
                                egui::ProgressBar::new(ws_frac)
                                    .fill(ws_color)
                                    .text(RichText::new(ws_text).size(11.0)),
                            );
                            // Retention days.
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Retention").size(11.0).color(theme::TEXT_MUTED),
                                );
                                let mut retention = self.config.workspace_cache_retention_days;
                                if ui
                                    .add(
                                        egui::Slider::new(&mut retention, 1..=90)
                                            .suffix(" days"),
                                    )
                                    .changed()
                                {
                                    self.config.workspace_cache_retention_days = retention;
                                    save = true;
                                }
                            });
                        });
                        let engine = self.engine.clone();
                        settings_card(ui, "ENGINE BRIDGE", |ui| {
                            match &engine {
                                // ENGINE-API §1: the GUI shows port/token for the manual paste
                                // flow, for editors that can't read engine.json themselves.
                                Some(h) => {
                                    ui.label(
                                        RichText::new(format!(
                                            "Listening on 127.0.0.1:{}  -  API {}",
                                            h.port, h.api_version
                                        ))
                                        .size(11.0)
                                        .color(theme::TEXT),
                                    );
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new("Token").size(11.0).color(theme::TEXT_DIM),
                                        );
                                        if ui.small_button("Copy").clicked() {
                                            ui.output_mut(|o| o.copied_text = h.token.clone());
                                        }
                                    });
                                }
                                None => {
                                    ui.label(
                                        RichText::new("Not listening (no catalog or port).")
                                            .size(11.0)
                                            .color(theme::TEXT_DIM),
                                    );
                                }
                            }
                            ui.label(
                                RichText::new(
                                    "Read routes only; import into the open Unity project is \
                                     planned (ENGINE-API.md).",
                                )
                                .size(11.0)
                                .color(theme::TEXT_DIM),
                            );
                            ui.add_space(8.0);
                            let mut confirm = self.config.confirm_before_trash;
                            if setting_toggle(
                                ui,
                                "Confirm before moving to Trash",
                                "Show a confirmation dialog before the Delete key moves assets \
                                 to Trash. Trashed files are recoverable and stay on disk.",
                                &mut confirm,
                            ) {
                                self.config.confirm_before_trash = confirm;
                                save = true;
                            }
                        });
                    });
                });
            });

        if save {
            self.config.save();
            self.cache_dirty = true;
        }
        if watch_changed {
            // Takes effect now: either start watching the open roots, or drop the handles.
            self.start_watching();
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
        if let Some(i) = rescan {
            self.rescan_source(ctx, i);
        }
        if let Some((i, enabled)) = toggle_watch {
            self.apply_source_watch_toggle(ctx, i, enabled);
        }
    }

    // ── Folder navigation (Folders view) ──────────────────────────────────────

    /// Where Folders view starts: single-source projects descend into the root
    /// directly; multi-source projects start at the source-tile top level.
    fn default_dir(&self) -> Option<(usize, PathBuf)> {
        (self.scan_roots.len() == 1).then(|| (0, PathBuf::new()))
    }

    /// Apply a folder location (shared by nav_to / back / forward) without touching history.
    fn apply_dir(&mut self, dir: Option<(usize, PathBuf)>) {
        self.current_dir = dir;
        self.cache_dirty = true;
        self.nav_generation += 1;
        self.selected_cache_idx = None;
        self.quick_look = false;
    }

    fn nav_to(&mut self, dir: Option<(usize, PathBuf)>) {
        if self.nav_history.get(self.nav_cursor) == Some(&dir) {
            return; // already at this location — don't stack a duplicate
        }
        self.apply_dir(dir.clone());
        self.nav_history.truncate(self.nav_cursor + 1);
        self.nav_history.push(dir);
        self.nav_cursor = self.nav_history.len() - 1;
    }

    /// Reset navigation history to the current location (on library reload / package open).
    fn reset_nav_history(&mut self) {
        self.nav_history = vec![self.current_dir.clone()];
        self.nav_cursor = 0;
    }

    fn can_go_back(&self) -> bool {
        self.nav_cursor > 0
    }

    fn can_go_forward(&self) -> bool {
        self.nav_cursor + 1 < self.nav_history.len()
    }

    fn nav_back(&mut self) {
        if self.can_go_back() {
            self.nav_cursor -= 1;
            let dir = self.nav_history[self.nav_cursor].clone();
            self.apply_dir(dir);
        }
    }

    fn nav_forward(&mut self) {
        if self.can_go_forward() {
            self.nav_cursor += 1;
            let dir = self.nav_history[self.nav_cursor].clone();
            self.apply_dir(dir);
        }
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

        let (can_back, can_fwd) = (self.can_go_back(), self.can_go_forward());
        let active_pack: Option<String> = match &self.source_filter {
            Some(LibraryFilter::Package(p)) => Some(p.clone()),
            _ => None,
        };
        let mut go_back = false;
        let mut go_fwd = false;
        let mut go_unity = false;

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let arrow = |ui: &mut Ui, glyph: &str, enabled: bool| -> bool {
                ui.add_enabled(
                    enabled,
                    egui::Button::new(RichText::new(glyph).size(14.0)).frame(false),
                )
                .on_hover_text(if glyph == "\u{2190}" {
                    "Back"
                } else {
                    "Forward"
                })
                .clicked()
            };
            go_back |= arrow(ui, "\u{2190}", can_back);
            go_fwd |= arrow(ui, "\u{2192}", can_fwd);

            let (root_src, at_root) = match self.current_dir.clone() {
                Some((src, dir)) => (Some(src), dir.as_os_str().is_empty()),
                None => (None, true),
            };

            if let Some(pack) = &active_pack {
                // Pack browse: a return-to-Unity button, then the pack name as the explorer root
                // (not "All libraries / <source>") — the tree is that pack's own contents.
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("\u{27F5} Unity Assets")
                                .size(11.0)
                                .color(theme::ARCANE),
                        )
                        .frame(false),
                    )
                    .on_hover_text("Back to Unity Assets")
                    .clicked()
                {
                    go_unity = true;
                }
                sep(ui);
                if crumb(ui, pack, at_root) && !at_root {
                    if let Some(src) = root_src {
                        nav = Some(Some((src, PathBuf::new())));
                    }
                }
            } else {
                if multi && crumb(ui, "All libraries", self.current_dir.is_none()) {
                    nav = Some(None);
                }
                if let Some(src) = root_src {
                    if multi {
                        sep(ui);
                    }
                    if crumb(ui, &config::stem_name(&self.scan_roots[src]), at_root) && !at_root {
                        nav = Some(Some((src, PathBuf::new())));
                    }
                }
            }

            // Shared: the current directory's path components.
            if let Some((src, dir)) = self.current_dir.clone() {
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
        if go_unity {
            self.return_to_unity();
        }
        if go_back {
            self.nav_back();
        } else if go_fwd {
            self.nav_forward();
        }

        // Thumbnails / file-list content toggle for the folder explorer (FEATURES.md §5.4).
        let list_now = self.folders_as_list;
        let mut flip = false;
        ui.horizontal(|ui| {
            let (glyph, hint) = if list_now {
                ("\u{25A6} Thumbnails", "Show this folder as thumbnails")
            } else {
                ("\u{2630} List", "Show this folder as a file list")
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(glyph).size(11.0).color(theme::TEXT_MUTED))
                        .frame(false),
                )
                .on_hover_text(hint)
                .clicked()
            {
                flip = true;
            }
        });
        if flip {
            self.folders_as_list = !self.folders_as_list;
            self.cache_dirty = true;
        }
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
                        ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                        for (i, tile) in tiles.iter().enumerate() {
                            // `direct (recursive)` — e.g. `3 (50)`; drop the parenthetical when the
                            // folder has no deeper assets (direct == recursive).
                            let count = if tile.recursive > tile.direct {
                                format!("{} ({})", tile.direct, tile.recursive)
                            } else {
                                tile.direct.to_string()
                            };
                            let btn = egui::Button::new(
                                RichText::new(format!("\u{1F4C1} {}   {count}", tile.name))
                                    .color(theme::TEXT)
                                    .size(13.5),
                            )
                            .fill(theme::SURFACE)
                            .stroke(egui::Stroke::new(1.0, theme::BORDER))
                            .min_size(Vec2::new(0.0, 34.0));
                            if ui
                                .add(btn)
                                .on_hover_text(format!(
                                    "{} \u{2014} {} here, {} in total",
                                    tile.name, tile.direct, tile.recursive
                                ))
                                .clicked()
                            {
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
                    Some((src, dir)) => self.nav_to(Some((src, dir.join(&tiles[i].name)))),
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
            // In-pack assets have no file on disk (abs_path is the .unitypackage) — their
            // thumbnail needs a surgical payload pull, which lands in the step-4 slice. Mark as
            // handled so the grid stops retrying and shows the typed placeholder (M14).
            if entry
                .asset_id
                .is_some_and(|id| self.in_pack_locators.contains_key(&id))
            {
                self.entries[idx].thumb_spawned = true;
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
            let kind = match self.entries[idx].file.media_type {
                Some(MediaType::Audio) => ThumbKind::Audio,
                // A material has no pixels of its own: paint its albedo on a sphere.
                Some(MediaType::Material) => ThumbKind::Material {
                    albedo: self.entries[idx]
                        .asset_id
                        .and_then(|id| self.material_albedo.get(&id))
                        .map(PathBuf::from),
                },
                _ => ThumbKind::Raster,
            };
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
                    kind,
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

/// Apply the sort direction. Every arm of [`sort_indices`] ends in this.
fn dir(o: std::cmp::Ordering, asc: bool) -> std::cmp::Ordering {
    if asc {
        o
    } else {
        o.reverse()
    }
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
                dir(o, asc)
            });
            *idxs = paired.into_iter().map(|(i, _)| i).collect();
        }
        SortBy::Name => {
            let keys: Vec<String> = idxs.iter().map(|&i| name_key(&entries[i])).collect();
            let mut paired: Vec<(usize, usize)> = idxs.iter().copied().zip(0..).collect();
            paired.sort_by(|&(_, ka), &(_, kb)| dir(keys[ka].cmp(&keys[kb]), asc));
            *idxs = paired.into_iter().map(|(i, _)| i).collect();
        }
        SortBy::Type => {
            idxs.sort_by(|&a, &b| dir(type_key(&entries[a]).cmp(&type_key(&entries[b])), asc))
        }
        SortBy::Size => idxs.sort_by(|&a, &b| {
            dir(
                entries[a].file.size_bytes.cmp(&entries[b].file.size_bytes),
                asc,
            )
        }),
        SortBy::Modified => {
            idxs.sort_by(|&a, &b| dir(entries[a].file.mtime.cmp(&entries[b].file.mtime), asc))
        }
        // Unrated sorts as 0 so rated assets lead when descending (the common use).
        SortBy::Rating => idxs.sort_by(|&a, &b| {
            dir(
                entries[a]
                    .rating
                    .unwrap_or(0)
                    .cmp(&entries[b].rating.unwrap_or(0)),
                asc,
            )
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
    pack: Option<&str>,
) -> (Vec<usize>, Vec<FolderTile>) {
    // name → (direct children in that folder, total recursive under it)
    let mut tiles: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    let mut idxs = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        if e.source_idx != src || e.trashed || !tab.matches(e.file.media_type) {
            continue;
        }
        // When browsing one pack, its folder tree is only that pack's assets — several packs can
        // share a source (and all use `Assets/…` paths), so without this they would merge.
        if let Some(p) = pack {
            if e.package.as_deref() != Some(p) {
                continue;
            }
        }
        let Ok(rest) = e.file.rel_path.strip_prefix(dir) else {
            continue;
        };
        let comps: Vec<_> = rest.components().collect();
        match comps.len() {
            0 => continue,
            1 => idxs.push(i), // a file directly in the current folder
            _ => {
                // Under subfolder comps[0]: always recursive; direct only when it's exactly
                // one level below (comps == [subfolder, file]).
                let name = comps[0].as_os_str().to_string_lossy().into_owned();
                let e = tiles.entry(name).or_insert((0, 0));
                e.1 += 1;
                if comps.len() == 2 {
                    e.0 += 1;
                }
            }
        }
    }
    let tiles = tiles
        .into_iter()
        .map(|(name, (direct, recursive))| FolderTile {
            name,
            direct,
            recursive,
        })
        .collect();
    (idxs, tiles)
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
    /// Clean shutdown removes the handshake file (ENGINE-API §1) — the port dies with the
    /// process, so leaving `engine.json` behind makes every client probe a dead pid first.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.engine.is_some() {
            lodestash_api::handshake::remove_handshake();
        }
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_hydrate();
        self.receive_scan();
        self.receive_thumbnails();
        self.receive_extracts();
        self.receive_peek(ctx);
        self.receive_import_scan();
        self.receive_rehydrate(ctx);
        // A finished extraction frees the slot the batch queue waits on.
        self.pump_extract_queue(ctx);
        self.poll_watch(ctx);
        self.pack_stats_modal(ctx);
        self.agent_approval_modal(ctx);
        self.render_modals(ctx);
        self.receive_catalog();
        self.receive_source_infos();
        self.receive_storage_size();
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
            if self.screen == Screen::Library && ctx.input(|i| i.key_pressed(Key::Delete)) {
                let targets: Vec<usize> = if self.selected.is_empty() {
                    self.selected_cache_idx
                        .and_then(|ci| self.sorted_cache.get(ci).copied())
                        .into_iter()
                        .collect()
                } else {
                    self.selected.iter().copied().collect()
                };
                if self.config.confirm_before_trash {
                    self.pending_confirm_trash = Some(targets);
                } else {
                    self.set_entries_trashed(&targets, true);
                }
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
                // F: pin the selection as a favourite (DATA-MODEL §12). Same target rule as
                // rating — the selection, or the lead card when nothing is multi-selected.
                if ctx.input(|i| i.key_pressed(Key::F) && i.modifiers.is_none()) {
                    let targets: Vec<usize> = if self.selected.is_empty() {
                        self.selected_cache_idx
                            .and_then(|ci| self.sorted_cache.get(ci).copied())
                            .into_iter()
                            .collect()
                    } else {
                        self.selected.iter().copied().collect()
                    };
                    self.toggle_favorite(&targets);
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
                                    self.ai_filters = None;
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
                        // Show what was parsed out, so a shrunken result set is never a mystery.
                        let parsed_hint = self
                            .ai_filters
                            .as_ref()
                            .filter(|f| f.has_filters())
                            .map(|f| {
                                let mut parts: Vec<String> = Vec::new();
                                if let Some(mt) = f.media_type {
                                    parts.push(mt.code().to_string());
                                }
                                if let Some(r) = f.min_rating {
                                    parts.push(format!("\u{2605}{r}+"));
                                }
                                if let Some(b) = f.max_size_bytes {
                                    parts.push(format!("< {}", format_bytes(b as u64)));
                                }
                                if let Some(b) = f.min_size_bytes {
                                    parts.push(format!("> {}", format_bytes(b as u64)));
                                }
                                if let Some(st) = f.status {
                                    parts.push(st.to_string());
                                }
                                format!("\u{2726} {}", parts.join(" \u{00B7} "))
                            });
                        let (msg, col) = if let Some(h) = &parsed_hint {
                            (h.as_str(), theme::GOLD)
                        } else if self.ai_embedder.is_some() {
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
                            for (mode, label) in [
                                (ViewMode::Folders, "Folders"),
                                (ViewMode::Flat, "Grid"),
                                (ViewMode::List, "List"),
                            ] {
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
                            let tag_usage = &self.tag_usage;
                            let mut filter_changed = false;
                            egui::popup::popup_below_widget(
                                ui,
                                popup_id,
                                &fresp,
                                egui::PopupCloseBehavior::CloseOnClickOutside,
                                |ui| {
                                    filter_changed =
                                        filter_popover(ui, cf, tag_usage, tiers);
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
            let mut peek_clicked: Option<(usize, usize)> = None;
            // An open contents tree needs room; the bar stays a thin strip otherwise.
            let browsing_any = self
                .pending_packages
                .iter()
                .any(|p| p.browsing && !p.extracting);
            egui::TopBottomPanel::top("packages_bar")
                .resizable(browsing_any)
                .default_height(if browsing_any { 360.0 } else { 0.0 })
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
                            let folders = pkg.folders();
                            egui::ScrollArea::vertical()
                                .id_salt(("pkg_browse", i))
                                .max_height(260.0)
                                .show(ui, |ui| {
                                    for (folder, members) in &folders {
                                        let label = if folder.is_empty() {
                                            "(archive root)"
                                        } else {
                                            folder.as_str()
                                        };
                                        let picked_here = members
                                            .iter()
                                            .filter(|m| pkg.picked.contains(m))
                                            .count();
                                        egui::CollapsingHeader::new(
                                            RichText::new(format!(
                                                "{label}  \u{00B7}  {}/{}",
                                                picked_here,
                                                members.len()
                                            ))
                                            .color(if picked_here > 0 {
                                                theme::GOLD
                                            } else {
                                                theme::TEXT_MUTED
                                            })
                                            .size(11.0),
                                        )
                                        .id_salt(("pkg_folder", i, folder))
                                        // A handful of folders reads as a tree; forty reads
                                        // as the flat list this replaced.
                                        .default_open(folders.len() <= 8)
                                        .show(ui, |ui| {
                                            ui.horizontal(|ui| {
                                                if ui.small_button("All").clicked() {
                                                    pkg.picked.extend(members.iter().copied());
                                                }
                                                if ui.small_button("None").clicked() {
                                                    pkg.picked.retain(|p| !members.contains(p));
                                                }
                                            });
                                            for &ei in members {
                                                let e = &pkg.entries[ei];
                                                let (Some(pathname), Some(size)) =
                                                    (e.pathname.as_deref(), e.size_bytes)
                                                else {
                                                    continue;
                                                };
                                                let leaf = pathname
                                                    .rsplit_once('/')
                                                    .map_or(pathname, |(_, f)| f);
                                                let mt = detect::by_extension(
                                                    std::path::Path::new(pathname),
                                                );
                                                ui.horizontal(|ui| {
                                                    let mut on = pkg.picked.contains(&ei);
                                                    if ui.checkbox(&mut on, "").changed() {
                                                        if on {
                                                            pkg.picked.insert(ei);
                                                        } else {
                                                            pkg.picked.remove(&ei);
                                                        }
                                                    }
                                                    // Typed dot per §5.4 — the bundled fonts
                                                    // render most per-type glyphs as tofu, so
                                                    // the type is carried by colour on a dot.
                                                    ui.label(
                                                        RichText::new("\u{25CF}")
                                                            .color(mt.map_or(
                                                                theme::TEXT_DIM,
                                                                theme::media_type_color,
                                                            ))
                                                            .size(9.0),
                                                    )
                                                    .on_hover_text(media_type_label(mt));
                                                    if ui
                                                        .add(
                                                            egui::Label::new(
                                                                RichText::new(leaf)
                                                                    .color(theme::TEXT_DIM)
                                                                    .size(11.0),
                                                            )
                                                            .sense(egui::Sense::click()),
                                                        )
                                                        .on_hover_text(
                                                            "Preview \u{2014} pulls just this \
                                                             file out of the archive",
                                                        )
                                                        .clicked()
                                                    {
                                                        peek_clicked = Some((i, ei));
                                                    }
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.label(
                                                                RichText::new(format_bytes(
                                                                    size as u64,
                                                                ))
                                                                .color(theme::TEXT_MUTED)
                                                                .size(10.0),
                                                            );
                                                        },
                                                    );
                                                });
                                            }
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
            if let Some((idx, entry_idx)) = peek_clicked {
                self.peek_entry(ctx, idx, entry_idx);
            }
        }

        // ── Peek window (FEATURES.md §5.4 right pane) ─────────────────────────
        if let Some(peek) = &self.peek {
            let mut open = true;
            let direct_model = peek.media_type == Some(MediaType::Model3d)
                && viewport3d::is_viewable_model(&peek.path);
            let (name, media_type, size_bytes) =
                (peek.name.clone(), peek.media_type, peek.size_bytes);
            let viewer = &mut self.image_viewer;
            let viewport = self.viewport3d.as_mut();
            let audio = self.audio_player.as_mut();
            egui::Window::new(format!("Preview \u{2014} {name}"))
                .id(egui::Id::new("package_peek"))
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
                        None,
                        viewer,
                        direct_model,
                        viewport,
                        audio,
                    );
                    ui.label(
                        RichText::new(
                            "From the archive \u{2014} nothing has been added to your library",
                        )
                        .color(theme::TEXT_DIM)
                        .size(10.0),
                    );
                });
            if !open {
                self.peek = None;
            }
        }

        // ── Inspector pane (right side) ───────────────────────────────────────
        let mut inspector_rating: Option<(usize, Option<u8>)> = None;
        let mut inspector_nav: Option<i64> = None;
        if self.screen == Screen::Library {
            // Load the LINKED FILES rows before borrowing the entry (the load needs &mut self).
            if let Some(id) = self
                .selected_cache_idx
                .and_then(|c| self.sorted_cache.get(c).copied())
                .and_then(|i| self.entries.get(i))
                .and_then(|e| e.asset_id)
            {
                self.ensure_related_loaded(id);
            }
            let related = &self.related;
            let entry_tags = &self.asset_tags;
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
                                    entry.metrics,
                                    entry.texture.as_ref(),
                                    library.as_deref(),
                                    related,
                                    entry
                                        .asset_id
                                        .and_then(|id| entry_tags.get(&id))
                                        .map(Vec::as_slice)
                                        .unwrap_or(&[]),
                                    &mut inspector_nav,
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
        // A LINKED FILES row was clicked: jump to that asset, or say why we can't.
        if let Some(id) = inspector_nav {
            if !self.select_by_asset_id(id) {
                self.jobs.toast(
                    "That linked asset is filtered out of the current view",
                    Intent::Info,
                );
            }
            self.cache_dirty = true;
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
                // Unidentified nudge (UI-SPEC §2.1) — the real backlog now that
                // `promote_statuses` moves rows (DATA-MODEL §8); nothing left ⇒ no banner.
                let n = self
                    .entries
                    .iter()
                    .filter(|e| !e.trashed && e.status.is_unidentified())
                    .count();
                if !self.nudge_dismissed && n > 0 {
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
                    let as_list = self.view_mode == ViewMode::List
                        || (self.view_mode == ViewMode::Folders && self.folders_as_list);
                    let out = if as_list {
                        asset_list(
                            ui,
                            &self.entries,
                            &self.sorted_cache[page_range],
                            page_start,
                            self.last_row_start,
                            self.last_row_end,
                            self.selected_cache_idx,
                            &self.selected,
                            self.nav_generation.wrapping_add(self.page as u64),
                            &self.edges_out,
                        )
                    } else {
                        thumbnail_grid(
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
                            &self.edges_out,
                            &self.favorites,
                            &self.config.size_tiers,
                        )
                    };
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

                    // Marquee drag-select: track pointer drag, draw overlay, and on release
                    // select all visible cards/rows that intersect the rubber-band rectangle.
                    let suppress_click = self.update_drag_select(ui, &out.rects);
                    if suppress_click {
                        clicked_cache = None;
                    }
                    if self.drag_select_active {
                        if let (Some(start), Some(current)) =
                            (self.drag_select_start, self.drag_select_current)
                        {
                            let rect = egui::Rect::from_two_pos(start, current);
                            ui.painter()
                                .rect_filled(rect, 0.0, theme::GOLD.linear_multiply(0.15));
                            ui.painter().rect_stroke(
                                rect,
                                0.0,
                                egui::Stroke::new(1.0, theme::GOLD),
                            );
                        }
                    }
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
                    ContextAction::ToggleFavorite => {
                        let targets: Vec<usize> = if self.selected.contains(&entry_idx) {
                            self.selected.iter().copied().collect()
                        } else {
                            vec![entry_idx]
                        };
                        self.toggle_favorite(&targets);
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
/// Chip colour per facet (IDENTIFICATION-AND-SEARCH §2). Facets are the vocabulary's structure,
/// so they get the type palette rather than one flat "tag" colour.
fn facet_color(namespace: &str) -> egui::Color32 {
    match namespace {
        "subject" => theme::TYPE_MODEL,
        "style" => theme::ARCANE,
        "material" => theme::TYPE_MATERIAL,
        "mood" => theme::GOLD,
        "usage" => theme::TYPE_IMAGE,
        "audio" => theme::TYPE_SOUND,
        _ => theme::TEXT_MUTED,
    }
}

/// Whether an Ollama model name looks like a vision model. Ollama's `/api/tags` doesn't report
/// capability, so the name is the only signal available — and handing pixels to a text-only model
/// wastes the call, while withholding them from a vision model wastes the model.
/// ponytail: a name list. Read `/api/show`'s `families` if this ever guesses wrong on a model
/// someone actually uses.
fn is_vision_model(name: &str) -> bool {
    const VISION_MARKERS: &[&str] = &[
        "moondream",
        "llava",
        "bakllava",
        "vl",
        "vision",
        "gemma3",
        "minicpm-v",
    ];
    let lower = name.to_lowercase();
    VISION_MARKERS.iter().any(|m| lower.contains(m))
}

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

/// Is this file a game asset the library should show?
///
/// The catalog only ever stored classified files (`asset.media_type_id` is NOT NULL), so an
/// unclassified one — a `.url` shortcut, a `.pdf` readme, a `.prefab`, a `.cs` script — could
/// only ever be a grid-only ghost: no status, no facets, no embedding, nothing an agent could
/// retrieve. Dropping them at the crawl makes the grid and the catalog agree
/// (INGESTION §3 "non-content", DATA-MODEL §2 media_type).
///
/// ponytail: extension-only, matching `detect::by_extension`. A real asset with an unknown
/// extension is dropped too — wire the magic-byte fallback in if that shows up in practice.
fn is_browseable(file: &CrawledFile) -> bool {
    file.media_type.is_some()
}

/// The Asset Store cache lays packs out as `<root>/<publisher>/<category>/<pack>.unitypackage`
/// (INGESTION §2). Read those two levels off the path when they're there; a plain folder of
/// packs has neither, and inventing them would be a claim the layout doesn't make.
fn store_publisher_category(
    root: &std::path::Path,
    package_path: &std::path::Path,
) -> (Option<String>, Option<String>) {
    let Ok(rel) = package_path.strip_prefix(root) else {
        return (None, None);
    };
    let comps: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    // comps: [publisher, category, …, "Pack.unitypackage"]
    (
        (comps.len() >= 2).then(|| comps[0].clone()),
        (comps.len() >= 3).then(|| comps[1].clone()),
    )
}

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
    ToggleFavorite,
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
    /// Visible rows/cards with their screen-space rectangles for marquee drag-select.
    /// (cache_idx, entry_idx, rect).
    rects: Vec<(usize, usize, egui::Rect)>,
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
    // Asset ids with outbound dependency edges — drives the card's link badge (DATA-MODEL §10).
    linked_ids: &std::collections::HashSet<i64>,
    // Favourited asset ids — drives the card's heart (DATA-MODEL §12).
    favorite_ids: &std::collections::HashSet<i64>,
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
    let mut rects: Vec<(usize, usize, egui::Rect)> = Vec::new();

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
                            entries[idx]
                                .asset_id
                                .is_some_and(|id| linked_ids.contains(&id)),
                            entries[idx]
                                .asset_id
                                .is_some_and(|id| favorite_ids.contains(&id)),
                            tiers,
                        );
                        rects.push((cache_idx, idx, resp.rect));
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
        rects,
    }
}

/// List/table view: one asset per row with a small thumbnail, name, type, size and rating.
/// Returns a `GridOutput` with `cols == 1` so the caller can reuse the same post-render plumbing.
#[allow(clippy::too_many_arguments)] // ponytail: mirrors grid state; an args struct would just rename them
fn asset_list(
    ui: &mut Ui,
    entries: &[AssetEntry],
    indices: &[usize],
    index_offset: usize,
    prev_row_start: usize,
    prev_row_end: usize,
    selected_cache_idx: Option<usize>,
    selected_entries: &std::collections::BTreeSet<usize>,
    nav_salt: u64,
    linked_ids: &std::collections::HashSet<i64>,
) -> GridOutput {
    let total_rows = indices.len();
    let mut to_spawn: Vec<usize> = Vec::new();
    let mut row_start_out: usize = prev_row_start;
    let mut row_end_out: usize = prev_row_end;
    let mut clicked: Option<(usize, egui::Modifiers)> = None;
    let mut context: Option<(usize, ContextAction)> = None;
    let mut rects: Vec<(usize, usize, egui::Rect)> = Vec::new();

    ScrollArea::vertical()
        .id_salt(("asset_list", nav_salt))
        .auto_shrink([false; 2])
        .show_rows(ui, LIST_ROW_HEIGHT, total_rows, |ui, row_range| {
            row_start_out = row_range.start;
            row_end_out = row_range.end;

            if row_range.start != prev_row_start || row_range.end != prev_row_end {
                let (buf_start, buf_end) =
                    lookahead_window(row_range.start, row_range.end, total_rows);
                for &idx in &indices[buf_start..buf_end] {
                    if !entries[idx].thumb_spawned && needs_thumbnail(entries[idx].file.media_type)
                    {
                        to_spawn.push(idx);
                    }
                }
            }

            for row_idx in row_range {
                let idx = indices[row_idx];
                let cache_idx = index_offset + row_idx;
                let entry = &entries[idx];
                let is_lead = selected_cache_idx == Some(cache_idx);
                let in_selection = selected_entries.contains(&idx);
                let selected = is_lead || in_selection;

                let type_color = entry
                    .file
                    .media_type
                    .map(theme::media_type_color)
                    .unwrap_or(theme::TEXT_DIM);
                let border_color = if selected { theme::GOLD } else { theme::BORDER };
                let border_width = if selected { 2.0 } else { 1.0 };

                let frame = egui::Frame::none()
                    .fill(theme::SURFACE)
                    .rounding(4.0)
                    .stroke(egui::Stroke::new(border_width, border_color))
                    .inner_margin(egui::Margin::symmetric(6.0, 4.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.set_min_size(egui::Vec2::new(0.0, LIST_THUMB_SIZE));
                            // Thumbnail or placeholder
                            if let Some(tex) = &entry.texture {
                                ui.add(
                                    egui::Image::new((
                                        tex.id(),
                                        egui::Vec2::splat(LIST_THUMB_SIZE),
                                    ))
                                    .fit_to_exact_size(egui::Vec2::splat(LIST_THUMB_SIZE))
                                    .rounding(3.0),
                                );
                            } else {
                                let (resp, painter) = ui.allocate_painter(
                                    egui::Vec2::splat(LIST_THUMB_SIZE),
                                    egui::Sense::hover(),
                                );
                                painter.rect_filled(
                                    resp.rect,
                                    3.0,
                                    type_color.linear_multiply(0.12),
                                );
                                painter.text(
                                    resp.rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    type_label_char(entry.file.media_type),
                                    egui::FontId::proportional((LIST_THUMB_SIZE * 0.35).max(10.0)),
                                    type_color.linear_multiply(0.6),
                                );
                            }

                            ui.add_space(6.0);
                            let name = entry
                                .file
                                .rel_path
                                .file_stem()
                                .map(|s| s.to_string_lossy())
                                .unwrap_or_default();
                            ui.label(
                                egui::RichText::new(name.as_ref())
                                    .color(theme::TEXT)
                                    .size(12.0),
                            );

                            // Metadata flushed to the right
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if entry.asset_id.is_some_and(|id| linked_ids.contains(&id)) {
                                        ui.label(
                                            egui::RichText::new("\u{26AD}")
                                                .color(theme::TYPE_MATERIAL)
                                                .size(11.0),
                                        );
                                    }
                                    if let Some(r) = entry.rating {
                                        ui.label(
                                            egui::RichText::new(format!("\u{2605} {r}"))
                                                .color(theme::GOLD)
                                                .size(11.0),
                                        );
                                    }
                                    ui.label(
                                        egui::RichText::new(format_bytes(entry.file.size_bytes))
                                            .color(theme::TEXT_DIM)
                                            .size(11.0),
                                    );
                                    ui.label(
                                        egui::RichText::new(media_type_label(
                                            entry.file.media_type,
                                        ))
                                        .color(type_color)
                                        .size(11.0),
                                    );
                                },
                            );
                        });
                    });

                let resp = frame.response.interact(egui::Sense::click());
                rects.push((cache_idx, idx, resp.rect));
                if resp.clicked() {
                    clicked = Some((cache_idx, ui.input(|i| i.modifiers)));
                }
                if resp.secondary_clicked() && !in_selection {
                    clicked = Some((cache_idx, egui::Modifiers::NONE));
                }
                let can_trash = entry.asset_id.is_some();
                resp.context_menu(|ui| {
                    ui.set_min_width(190.0);
                    if let Some(action) = card_context_menu(ui, can_trash) {
                        context = Some((cache_idx, action));
                    }
                });

                // Add a tiny gap between rows
                ui.add_space(2.0);
            }
        });

    GridOutput {
        to_spawn,
        cols: 1,
        row_start: row_start_out,
        row_end: row_end_out,
        clicked,
        context,
        rects,
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
    act(
        ui,
        ContextAction::ToggleFavorite,
        "\u{2665} Favourite          F",
    );
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
/// Start the loopback `/v1/` API (ENGINE-API §1). Best-effort: a failure here costs the plugin
/// bridge, not the app, so it logs and returns `None`.
type EngineHandles = (
    lodestash_api::Handshake,
    std::sync::Arc<lodestash_api::ServerState>,
);

fn start_engine_api(
    thumb_cache_dir: Option<PathBuf>,
    workspace_dir: Option<PathBuf>,
) -> Option<EngineHandles> {
    let state = std::sync::Arc::new(agent_state(thumb_cache_dir, workspace_dir)?);
    let handshake = lodestash_api::serve(state.clone())
        .map_err(|e| tracing::warn!(%e, "engine API did not start"))
        .ok()?;
    Some((handshake, state))
}

/// The shared read state both agent planes run on (REST here, MCP in `--mcp` mode). The preview
/// closure reuses the grid's thumbnail cache, so an agent asking for a preview gets the same
/// rendered image the user sees — generated on a miss at a fixed 256 px.
pub fn agent_state(
    thumb_cache_dir: Option<PathBuf>,
    workspace_dir: Option<PathBuf>,
) -> Option<lodestash_api::ServerState> {
    let db = catalog_db_path()?;
    let ws = workspace_dir.unwrap_or_default();
    let preview = move |path: &std::path::Path| -> Option<PathBuf> {
        let dir = thumb_cache_dir.as_deref()?;
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let key = thumb::meta_key(path, meta.len(), mtime);
        let params = thumb::ThumbParams {
            width: 256,
            height: 256,
        };
        if detect::by_extension(path) == Some(MediaType::Audio) {
            thumb::generate_waveform(dir, &key, params, |bars| {
                crate::audio::load_waveform(path, bars).map(|m| m.peaks)
            })
            .ok()
        } else {
            thumb::generate(path, dir, &key, params).ok()
        }
    };
    lodestash_api::ServerState::new(&db, ws, Box::new(preview))
        .map_err(|e| tracing::warn!(%e, "agent plane: catalog unavailable"))
        .ok()
}

/// Default cache locations, shared by the GUI and the `--mcp` entry point.
pub fn default_cache_dirs() -> (Option<PathBuf>, Option<PathBuf>) {
    (
        dirs::cache_dir().map(|d| d.join("Lodestash").join("thumbs")),
        dirs::cache_dir().map(|d| d.join("Lodestash").join("workspace")),
    )
}

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
    let dir = dirs::data_dir()?
        .join("Lodestash")
        .join("models")
        .join("siglip");
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

/// Encode one 3D model by rendering it from a few orbit views and mean-pooling the per-view
/// SigLIP embeddings (IDENTIFICATION-AND-SEARCH §3 Tier 1, "SigLIP 2 image / 3D-multiview").
/// `None` when the mesh can't be loaded or carries no geometry.
fn embed_model_multiview(
    path: &std::path::Path,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
) -> Option<Vec<f32>> {
    // FBX/OBJ via ufbx, glTF/GLB via three-d-asset — one entry point, world-space MeshData.
    let meshes = lodestash_ingest::mesh::load_any(path, &std::collections::BTreeMap::new())
        .map_err(|e| tracing::warn!(%e, "mesh load failed"))
        .ok()?;
    let views = lodestash_ingest::render3d::render_views(
        &meshes,
        RENDER_SIZE,
        lodestash_ingest::render3d::DEFAULT_VIEWS,
    );
    if views.is_empty() {
        return None;
    }
    // Mean-pool then L2-normalise: the standard multiview aggregation, and it keeps the result
    // in the same unit-sphere space as the single-image and text vectors.
    let mut acc = vec![0.0f32; emb.dim()];
    for v in &views {
        for (a, x) in acc.iter_mut().zip(emb.embed_image(v)) {
            *a += x;
        }
    }
    let norm: f32 = acc.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= 0.0 {
        return None;
    }
    for a in &mut acc {
        *a /= norm;
    }
    Some(acc)
}

/// Offscreen render resolution per view — SigLIP resizes to 256² anyway, so rendering larger
/// only costs rasterisation time.
const RENDER_SIZE: u32 = 256;

/// Model id for the audio space. Kept distinct from the visual one so the two never mix in
/// `asset_embedding` — `embeddings_for_knn` shards on `model_id`.
const CLAP_MODEL_ID: &str = "clap-htsat-unfused";

/// Load the CLAP audio embedder if its artifacts are present, else `None` (audio simply gets no
/// vectors — visual search is unaffected). Mirrors `resolve_embedder`.
fn resolve_audio_embedder() -> Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>> {
    let dir = models::clap_dir()?;
    let (a, t, k) = (
        dir.join("audio.onnx"),
        dir.join("text.onnx"),
        dir.join("tokenizer.json"),
    );
    if !(a.exists() && t.exists() && k.exists()) {
        return None;
    }
    match lodestash_ai::clap::ClapEmbedder::load(&a, &t, &k, CLAP_MODEL_ID) {
        Ok(e) => {
            tracing::info!("clap embedder loaded — audio is searchable");
            Some(Arc::new(e))
        }
        Err(e) => {
            tracing::info!(%e, "no local clap model; audio stays unsearchable");
            None
        }
    }
}

/// Decode an audio file to mono f32 at CLAP's 48 kHz. `rodio` bundles symphonia, so this needs
/// no new dependency. Capped at a little over the 10 s the encoder consumes.
/// ponytail: linear resampling — inaudible at mel resolution; swap in a windowed-sinc if audio
/// retrieval quality ever demands it.
fn decode_audio_48k_mono(path: &std::path::Path) -> Result<Vec<f32>, String> {
    use rodio::Source;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec = rodio::Decoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let (sr, ch) = (dec.sample_rate(), dec.channels() as usize);
    if sr == 0 || ch == 0 {
        return Err("bad audio header".into());
    }
    // Only the first ~12s can matter (CLAP consumes 10s); bounds work on long music beds.
    let cap = sr as usize * ch * 12;
    let raw: Vec<f32> = dec.take(cap).map(|s| s as f32 / 32768.0).collect();
    if raw.is_empty() {
        return Err("no samples decoded".into());
    }
    let mono: Vec<f32> = if ch <= 1 {
        raw
    } else {
        raw.chunks(ch)
            .map(|c| c.iter().sum::<f32>() / ch as f32)
            .collect()
    };
    if sr == lodestash_ingest::melspec::SAMPLE_RATE {
        return Ok(mono);
    }
    let ratio = lodestash_ingest::melspec::SAMPLE_RATE as f64 / sr as f64;
    let out_len = (mono.len() as f64 * ratio) as usize;
    Ok((0..out_len)
        .map(|i| {
            let src = i as f64 / ratio;
            let i0 = src.floor() as usize;
            let frac = (src - i0 as f64) as f32;
            let a = mono.get(i0).copied().unwrap_or(0.0);
            let b = mono.get(i0 + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect())
}

/// Embed dirty **audio** assets with CLAP: decode → 48 kHz mono → log-mel → audio tower.
/// Same sole-writer connection and same forward-progress rule as the visual drain (an
/// undecodable file stores a zero vector so it's never re-queried forever).
fn drain_audio_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    workspace_dir: Option<&std::path::Path>,
) {
    const BATCH: usize = 32;
    let mut total = 0usize;
    loop {
        let jobs = match cat.audio_assets_needing_embedding(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "audio embed queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            let path = match (&job.disk_path, &job.workspace_rel) {
                (Some(p), _) => Some(std::path::PathBuf::from(p)),
                (None, Some(rel)) => workspace_dir.map(|w| w.join(rel)),
                _ => None,
            };
            let vector = path
                .as_deref()
                .and_then(|p| {
                    decode_audio_48k_mono(p)
                        .map_err(|e| tracing::warn!(%e, "audio decode failed"))
                        .ok()
                })
                .map(|samples| {
                    let (frames, mel) = lodestash_ingest::melspec::log_mel_spectrogram(&samples);
                    emb.embed_audio(&mel, frames)
                })
                .unwrap_or_else(|| vec![0.0; emb.dim()]);
            if let Err(e) = cat.upsert_embedding(
                job.asset_id,
                emb.model_id(),
                "audio",
                Some(job.media_type_id),
                Some(job.source_id),
                &vector,
            ) {
                tracing::warn!(%e, "upsert_embedding failed; stopping audio drain");
                return;
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        tracing::info!(embedded = total, "clap audio embeddings written");
    }
}

/// Embed the dirty **visual** assets (SigLIP vision tower) in batches over the persist thread's
/// own sole-writer connection: 2D files decode directly, 3D models are multiview-rendered first.
/// Failures store a zero vector — which cosine-scores 0, so it never matches — solely to clear
/// `embedding_dirty` and guarantee forward progress (no infinite re-query of a bad file).
/// Audio still awaits its own encoder (CLAP).
/// Harvest Unity's own bookkeeping for the rows marked `deps_dirty` (DATA-MODEL §10): each asset's
/// GUID from its `.meta` sidecar, and — for materials — the texture GUIDs it declares, which become
/// `asset_relationship` edges. Same sole-writer connection and forward-progress rule as the other
/// drains: an asset with no `.meta` still clears its flag.
///
/// Order matters within a drain but not across them: an edge whose target isn't catalogued yet is
/// stored `missing` and late-bound by `resolve_pending_relationships`, so a texture indexed after
/// its material still connects.
fn drain_dependencies(cat: &mut Catalog, workspace_dir: Option<&std::path::Path>) {
    const BATCH: usize = 256;
    let mut total = 0usize;
    let mut edges = 0usize;
    loop {
        let jobs = match cat.assets_needing_dep_scan(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "dependency queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            let path = match (&job.disk_path, &job.workspace_rel) {
                (Some(p), _) => Some(std::path::PathBuf::from(p)),
                (None, Some(rel)) => workspace_dir.map(|w| w.join(rel)),
                _ => None,
            };
            let guid = path.as_deref().and_then(lodestash_ingest::deps::guid_of);
            // Materials are the only catalogued type that declares GUIDs today: meshes embed
            // their materials (`externalObjects` is empty on a stock FBX import), and prefabs
            // aren't a `media_type`.
            let refs = match (job.media_code.as_str(), path.as_deref()) {
                ("material", Some(p)) => std::fs::read_to_string(p)
                    .map(|yaml| lodestash_ingest::deps::material_texture_guids(&yaml))
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            edges += refs.len();
            if let Err(e) =
                cat.record_dependencies(job.asset_id, guid.as_deref(), "uses_texture", &refs)
            {
                tracing::warn!(%e, "record_dependencies failed; stopping dependency drain");
                return;
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        drain_prefab_edges(cat);
    }
    match cat.resolve_pending_relationships() {
        Ok(late) if late > 0 => tracing::info!(late, "dependency edges late-bound"),
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "resolving pending dependency edges failed"),
    }
    if total > 0 {
        tracing::info!(scanned = total, edges, "dependency graph harvested");
    }
}

/// Walk every source root for `.prefab` files and rebuild the **mesh → material** edges from what
/// they bind (DATA-MODEL §10). Prefabs are not assets — they aren't a `media_type` — so they can't
/// ride the `deps_dirty` queue; they are read as pure evidence about assets that *are* catalogued.
///
/// Runs only when the dependency drain actually scanned something, so a steady-state library
/// re-walks nothing. ponytail: on-disk sources only — a prefab extracted into the package
/// workspace is skipped until someone wants that.
fn drain_prefab_edges(cat: &mut Catalog) {
    let roots = match cat.source_roots() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "prefab pass: source roots unavailable");
            return;
        }
    };
    let mut pairs: Vec<(String, String)> = roots
        .iter()
        .flat_map(|root| lodestash_ingest::deps::prefab_pairs_under(std::path::Path::new(root)))
        .collect();
    pairs.sort();
    pairs.dedup();
    match cat.replace_prefab_relationships(&pairs) {
        Ok(written) => tracing::info!(pairs = pairs.len(), written, "prefab edges rebuilt"),
        Err(e) => tracing::warn!(%e, "rebuilding prefab edges failed"),
    }
}

/// Extract technical facets (tri counts, dimensions, duration) for the rows `record_crawl` marked
/// `metrics_dirty` (DATA-MODEL §14). Runs on the persist thread's own connection like the embedding
/// drains, but unconditionally — facets are plain measurements and must not depend on the AI stack
/// being installed. A file we can't measure still gets its flag cleared (forward progress).
fn drain_metrics(cat: &mut Catalog, workspace_dir: Option<&std::path::Path>) {
    const BATCH: usize = 64;
    let mut total = 0usize;
    loop {
        let jobs = match cat.assets_needing_metrics(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "metrics queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            let path = match (&job.disk_path, &job.workspace_rel) {
                (Some(p), _) => Some(std::path::PathBuf::from(p)),
                (None, Some(rel)) => workspace_dir.map(|w| w.join(rel)),
                _ => None,
            };
            let metrics = path
                .as_deref()
                .map(|p| {
                    if job.media_code == "audio" {
                        lodestash_core::catalog::AssetMetrics {
                            duration_ms: audio_duration_ms(p),
                            ..Default::default()
                        }
                    } else {
                        lodestash_ingest::metrics::extract(p, &job.media_code)
                    }
                })
                .unwrap_or_default();
            if let Err(e) = cat.set_metrics(job.asset_id, metrics) {
                tracing::warn!(%e, "set_metrics failed; stopping metrics drain");
                return;
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        tracing::info!(measured = total, "technical facets extracted");
    }
}

/// Clip length via the decoder the player already uses. `total_duration` answers from the header
/// for WAV/FLAC; formats that can't (some MP3s) report `None`, which stays "unknown" rather than
/// paying for a full decode just to fill a stat row.
fn audio_duration_ms(path: &std::path::Path) -> Option<i64> {
    use rodio::Source;
    let file = std::fs::File::open(path).ok()?;
    let dec = rodio::Decoder::new(std::io::BufReader::new(file)).ok()?;
    Some(dec.total_duration()?.as_millis() as i64)
}

/// Drain everything the catalog still has queued (facets, then vectors), reconcile the
/// status lifecycle (DATA-MODEL §8), and read the fresh counts + asset identities back for
/// `roots` (`(name, root_path)`). A drain now *moves* statuses, so it is no longer
/// count-neutral: the health meter and STATUS facet need the re-read.
///
/// Runs on the persist thread's own connection — the sole writer (ARCHITECTURE §5).
fn drain_promote_resync(
    cat: &mut Catalog,
    embedder: Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    audio_embedder: Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    workspace_dir: Option<&std::path::Path>,
    roots: &[(String, String)],
) -> (Option<StatusCounts>, AssetIdentityMaps) {
    drain_metrics(cat, workspace_dir);
    drain_dependencies(cat, workspace_dir);
    if let Some(emb) = embedder {
        drain_image_embeddings(cat, emb.as_ref(), workspace_dir);
    }
    if let Some(emb) = audio_embedder {
        drain_audio_embeddings(cat, emb.as_ref(), workspace_dir);
    }
    match cat.promote_statuses() {
        Ok(0) => return (None, Vec::new()), // nothing moved — nothing to re-read
        Ok(n) => tracing::info!(writes = n, "status lifecycle reconciled"),
        Err(e) => {
            tracing::warn!(%e, "status promote failed; statuses stale until next drain");
            return (None, Vec::new());
        }
    }
    resync_after_drain(cat, roots)
}

/// Re-read per-status counts + asset identities for `roots` after a drain has changed them.
/// Shared by every drain path, so what the grid learns never depends on which one ran.
fn resync_after_drain(
    cat: &mut Catalog,
    roots: &[(String, String)],
) -> (Option<StatusCounts>, AssetIdentityMaps) {
    let read = || -> Result<(StatusCounts, AssetIdentityMaps), String> {
        let mut sources = Vec::with_capacity(roots.len());
        for (name, root) in roots {
            let id = cat
                .upsert_import_source(name, root, "folder")
                .map_err(|e| e.to_string())?;
            sources.push((root.clone(), id));
        }
        let ids: Vec<i64> = sources.iter().map(|(_, id)| *id).collect();
        let counts = cat
            .status_counts_for_sources(&ids)
            .map_err(|e| e.to_string())?;
        Ok((counts, read_back_assets(cat, &sources)?))
    };
    match read() {
        Ok((counts, assets)) => (Some(counts), assets),
        Err(err) => {
            tracing::warn!(%err, "post-drain read-back failed; UI keeps the pre-drain counts");
            (None, Vec::new())
        }
    }
}

/// Tag the `tags_dirty` backlog, one asset at a time, on the caller's sole-writer connection.
///
/// The preview handed to a vision model is the **thumbnail cache's own PNG** — the same pixels the
/// grid shows, produced by the same `thumb::generate` (decode for 2D, hero render for 3D), so the
/// model sees what the user sees and no second rendering path exists to drift.
///
/// Forward progress is unconditional: an asset the tagger refuses (audio), one whose preview can't
/// be made, and one the model answers nothing for all still clear `tags_dirty` via an empty write.
/// Otherwise the queue would hand back the same rows forever.
fn drain_tags(
    cat: &mut Catalog,
    tagger: &dyn lodestash_ai::tagger::Tagger,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    progress: &crate::jobs::JobProgress,
) {
    use std::sync::atomic::Ordering;
    const BATCH: usize = 16;
    /// Fixed preview size, independent of the user's grid-quality setting: tagging results should
    /// not change because someone moved a slider.
    const TAG_PX: u32 = 384;

    if let Ok(total) = cat.count_needing_tags() {
        progress.total.store(total as u64, Ordering::Relaxed);
    }
    let mut tagged = 0usize;
    let model_id = tagger.model_id().to_string();
    loop {
        if progress.is_cancelled() {
            break;
        }
        let jobs = match cat.assets_needing_tags(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "tag queue query failed; retries next run");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        // Two passes over the batch, not one pass doing both stages. Interleaving the vision and
        // text models makes Ollama evict and reload multi-GB weights on every asset — measured
        // ~50 s/asset that way, against a few seconds when each model stays resident for a batch.
        let mut prepared: Vec<(
            lodestash_core::catalog::VisualEmbedJob,
            String,
            Option<String>,
        )> = Vec::with_capacity(jobs.len());
        for job in jobs {
            if progress.is_cancelled() {
                return;
            }
            let path = match (&job.disk_path, &job.workspace_rel) {
                (Some(p), _) => Some(std::path::PathBuf::from(p)),
                (None, Some(rel)) => workspace_dir.map(|w| w.join(rel)),
                _ => None,
            };
            let name = path
                .as_deref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let caption = if tagger.supports(&job.media_code) {
                path.as_deref()
                    .zip(cache_dir)
                    .and_then(|(p, cache)| tag_preview(p, cache, TAG_PX))
                    .and_then(|img| {
                        tagger.caption(&lodestash_ai::tagger::TagInput {
                            media_code: &job.media_code,
                            name: &name,
                            image: Some(&img),
                        })
                    })
            } else {
                None
            };
            prepared.push((job, name, caption));
        }

        for (job, name, caption) in prepared {
            if progress.is_cancelled() {
                return;
            }
            let result = if tagger.supports(&job.media_code) {
                tagger.tag(
                    &lodestash_ai::tagger::TagInput {
                        media_code: &job.media_code,
                        name: &name,
                        image: None,
                    },
                    caption.as_deref(),
                )
            } else {
                lodestash_ai::tagger::TagResult::default()
            };
            let tags = lodestash_ai::tagger::validate(result.tags);
            // The caption is the model's own words about the asset; it lands in `description`,
            // which FTS indexes, so it is searchable rather than decorative.
            if let Some(caption) = &result.caption {
                if let Err(e) = cat.set_description(job.asset_id, Some(caption)) {
                    tracing::warn!(%e, "caption write failed");
                }
            }
            match cat.set_tags(job.asset_id, "ai", Some(&model_id), &tags) {
                Ok(n) => tagged += n,
                Err(e) => {
                    tracing::warn!(%e, "set_tags failed; stopping tag drain");
                    return;
                }
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
    }
    tracing::info!(tags = tagged, model = %model_id, "tagging pass finished");
}

/// The cached preview PNG for `source`, decoded. `None` when the file can't be previewed at all
/// (an audio clip, an unreadable model) — the caller then tags from the name alone.
fn tag_preview(
    source: &std::path::Path,
    cache_dir: &std::path::Path,
    px: u32,
) -> Option<image::RgbaImage> {
    let meta = std::fs::metadata(source).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let key = lodestash_ingest::thumb::meta_key(source, meta.len(), mtime);
    let params = lodestash_ingest::thumb::ThumbParams {
        width: px,
        height: px,
    };
    let png = lodestash_ingest::thumb::generate(source, cache_dir, &key, params).ok()?;
    Some(image::open(png).ok()?.to_rgba8())
}

/// Decode a visual drain job's image from whichever of the three §7 byte-sources applies:
/// an on-disk file, a package-extracted workspace file, or — for an **in-pack** asset — one
/// payload pulled into memory and decoded from bytes (DATA-MODEL §7 / M14). The in-pack path
/// never writes to disk: identify surgically reads a single payload, one asset at a time.
/// `None` = no source resolved / undecodable (the caller stores a zero vector to advance).
fn resolve_job_image(
    job: &lodestash_core::catalog::VisualEmbedJob,
    workspace_dir: Option<&std::path::Path>,
) -> Option<image::DynamicImage> {
    if let Some(p) = &job.disk_path {
        return lodestash_ingest::raster::decode(std::path::Path::new(p))
            .map_err(|e| tracing::warn!(%e, "image decode failed"))
            .ok();
    }
    if let (Some(rel), Some(ws)) = (&job.workspace_rel, workspace_dir) {
        return lodestash_ingest::raster::decode(&ws.join(rel))
            .map_err(|e| tracing::warn!(%e, "extracted image decode failed"))
            .ok();
    }
    if let (Some(pack), Some(off), Some(sz)) = (&job.pack_path, job.pack_offset, job.pack_size) {
        let bytes = lodestash_ingest::unitypackage::read_payload_at(
            std::path::Path::new(pack),
            off as u64,
            sz as u64,
        )
        .map_err(|e| tracing::warn!(%e, "in-pack payload pull failed"))
        .ok()?;
        return lodestash_ingest::raster::decode_bytes(&bytes, job.ext.as_deref())
            .map_err(|e| tracing::warn!(%e, "in-pack image decode failed"))
            .ok();
    }
    None
}

fn drain_image_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    workspace_dir: Option<&std::path::Path>,
) {
    const BATCH: usize = 64;
    let mut total = 0usize;
    loop {
        let jobs = match cat.visual_assets_needing_embedding(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "visual embed queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            let modality = if job.media_code == "model_3d" {
                "model_3d"
            } else {
                "image"
            };
            let vector = if job.media_code == "model_3d" {
                // On-disk source file, or a package-extracted file resolved against the workspace.
                // ponytail: an in-pack model needs mesh-from-bytes render (no path to give the
                // renderer) — not yet; in-pack models fall to the zero vec below until it lands.
                let path = match (&job.disk_path, &job.workspace_rel) {
                    (Some(p), _) => Some(std::path::PathBuf::from(p)),
                    (None, Some(rel)) => workspace_dir.map(|w| w.join(rel)),
                    _ => None,
                };
                path.as_deref().and_then(|p| embed_model_multiview(p, emb))
            } else {
                resolve_job_image(&job, workspace_dir).map(|img| emb.embed_image(&img.to_rgba8()))
            }
            // ponytail: unusable (undecodable / unrenderable / evicted from the workspace) →
            // zero vec clears the flag so the drain always advances and never re-queries the
            // same row forever; it scores 0 in KNN so it never surfaces.
            .unwrap_or_else(|| vec![0.0; emb.dim()]);
            if let Err(e) = cat.upsert_embedding(
                job.asset_id,
                emb.model_id(),
                modality,
                Some(job.media_type_id),
                Some(job.source_id),
                &vector,
            ) {
                tracing::warn!(%e, "upsert_embedding failed; stopping visual drain");
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
        .map(crawl_record)
        .collect()
}

/// One classified file as a catalog record. Callers that know where the file lives fill in
/// `workspace_rel` afterwards; an on-disk crawl leaves it None.
fn crawl_record(f: &CrawledFile) -> CrawlRecord {
    CrawlRecord {
        rel_path: f.rel_path.to_string_lossy().replace('\\', "/"),
        name: f
            .rel_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        media_type: f
            .media_type
            .expect("caller filtered Some")
            .code()
            .to_string(),
        size_bytes: f.size_bytes as i64,
        mtime: f.mtime,
        workspace_rel: None,
    }
}

/// Ensure a pack's in-pack assets exist (M14): record its manifest locators if the catalog
/// doesn't have them yet (reading the archive once), then mint an in-pack asset per classified
/// entry. Idempotent — mint skips duplicates and extracted packs. Background-only: a manifest
/// read streams the whole gzip. Returns how many assets were newly minted.
fn ensure_in_pack_minted(
    db: &std::path::Path,
    package_id: i64,
    pack_path: &std::path::Path,
) -> Result<usize, String> {
    let mut catalog = Catalog::open(db).map_err(|e| e.to_string())?;
    let mut entries = catalog
        .list_package_entries(package_id)
        .map_err(|e| e.to_string())?;
    if entries.is_empty() {
        let manifest = unitypackage::read_manifest(pack_path).map_err(|e| e.to_string())?;
        entries = manifest
            .iter()
            .map(|e| PackageEntryRecord {
                guid: e.guid.clone(),
                pathname: e.pathname.clone(),
                tar_offset: e.asset_offset.map(|o| o as i64),
                size_bytes: e.asset_size.map(|s| s as i64),
            })
            .collect();
        catalog
            .record_package_entries(package_id, &entries)
            .map_err(|e| e.to_string())?;
    }
    let classified = classify_in_pack_entries(&entries);
    let stats = catalog
        .mint_in_pack_assets(package_id, &classified)
        .map_err(|e| e.to_string())?;
    Ok(stats.inserted)
}

/// Classify a package's manifest entries for minting (M14): map each entry's GUID to the media
/// type + display name the in-pack asset should carry. Entries whose extension classifies to no
/// `media_type` (scripts, prefabs, manifests) are dropped — the same "unclassified never reaches
/// the grid" rule the crawl applies, enforced here by their absence from the returned map.
fn classify_in_pack_entries(
    entries: &[PackageEntryRecord],
) -> std::collections::HashMap<String, InPackClass> {
    entries
        .iter()
        .filter_map(|e| {
            let pathname = e.pathname.as_deref()?;
            let path = std::path::Path::new(pathname);
            let media_type = detect::by_extension(path)?;
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| e.guid.clone());
            Some((
                e.guid.clone(),
                InPackClass {
                    name,
                    media_type: media_type.code().to_string(),
                },
            ))
        })
        .collect()
}

/// [`crawl_records`] for package-extracted files, locating each payload inside the extraction
/// workspace (M6 `asset_file.workspace_rel`) so the catalog can rehydrate it in a later session.
///
/// The location has to be resolved in the same pass that builds the record: `crawl_records` drops
/// unclassified files, so a record list and the file list it came from are different lengths and
/// pairing them positionally mis-assigns every path after the first dropped file.
fn crawl_records_in_workspace<'a>(
    files: impl Iterator<Item = &'a CrawledFile>,
    workspace_dir: Option<&std::path::Path>,
) -> Vec<CrawlRecord> {
    files
        .filter(|f| f.media_type.is_some())
        .map(|f| {
            let mut rec = crawl_record(f);
            rec.workspace_rel = workspace_dir.and_then(|ws| {
                f.abs_path
                    .strip_prefix(ws)
                    .ok()
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
            });
            rec
        })
        .collect()
}

/// Runs on the persist thread: the extracted package's `package` row, its asset rows
/// (carrying `package_id`), the unpack stamp, then project-wide status counts.
fn persist_package_to_catalog(
    db_path: &std::path::Path,
    job: &PackagePersist,
) -> Result<(StatusCounts, AssetIdentityMaps, Catalog), String> {
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
    harvest_package_metadata(&catalog, package_id, job);
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
    Ok((counts, assets, catalog))
}

/// Harvest what a pack says about itself and store it (DATA-MODEL §7, INGESTION §2).
///
/// Runs at **extraction** time rather than at browse time: the harvest re-reads the archive, and a
/// 20 GB pack should not be inflated twice just to fill in a version string the user hasn't asked
/// to see. The entries the extraction already produced tell it which GUIDs are worth reading.
///
/// A pack with a publisher came from the Asset Store cache tree, and that is enough to know its
/// licence — the EULA is a fact about where the pack came from, so it fills in only when the pack
/// itself said nothing.
fn harvest_package_metadata(catalog: &Catalog, package_id: i64, job: &PackagePersist) {
    let entries: Vec<unitypackage::PackageEntry> = job
        .entries
        .iter()
        .map(|e| unitypackage::PackageEntry {
            guid: e.guid.clone(),
            pathname: e.pathname.clone(),
            asset_size: e.size_bytes.map(|n| n as u64),
            ..Default::default()
        })
        .collect();
    let path = std::path::Path::new(&job.package_path);
    let meta = match unitypackage::read_package_meta(path, &entries) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(%e, "package metadata harvest failed");
            return;
        }
    };
    if let (Some(png), Some(cache)) = (&meta.icon_png, default_cache_dirs().0) {
        if let Err(e) = thumb::write_package_icon(&cache, &job.package_path, png) {
            tracing::warn!(%e, "pack icon write failed");
        }
    }
    let store_pack = job.publisher.is_some();
    let license = meta
        .license
        .clone()
        .or_else(|| store_pack.then(|| "Unity Asset Store EULA".to_string()));
    let license_url = meta
        .license_url
        .clone()
        .or_else(|| store_pack.then(|| "https://unity.com/legal/as-terms".to_string()));
    if let Err(e) = catalog.set_package_metadata(
        package_id,
        meta.version.as_deref(),
        license.as_deref(),
        job.publisher.as_deref(),
        license_url.as_deref(),
    ) {
        tracing::warn!(%e, "package metadata write failed");
    } else {
        tracing::info!(
            package = %job.name,
            version = ?meta.version,
            license = ?license,
            icon = meta.icon_png.as_ref().map_or(0, |b| b.len()),
            "pack metadata harvested"
        );
    }
}

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
                    AssetStatus::from_code(&a.status).map(|s| {
                        (
                            a.rel_path,
                            (
                                a.asset_id,
                                s,
                                a.trashed,
                                a.rating,
                                a.metrics,
                                a.content_hash,
                            ),
                        )
                    })
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
fn filter_popover(
    ui: &mut Ui,
    cf: &mut ContentFilter,
    tag_usage: &[(String, String, i64)],
    tiers: &SizeTiers,
) -> bool {
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

    // TRAIT — the tag vocabulary, most-used first (IDENTIFICATION-AND-SEARCH §2).
    section_label(ui, "TRAIT");
    if tag_usage.is_empty() {
        ui.label(
            RichText::new(
                "No traits yet \u{2014} run Settings \u{25b8} AI \u{00b7} Identify to tag the library.",
            )
            .size(10.0)
            .color(theme::TEXT_DIM),
        );
    } else {
        ui.add(
            egui::TextEdit::singleline(&mut cf.trait_search)
                .hint_text("search traits\u{2026}")
                .desired_width(140.0),
        );
        let needle = cf.trait_search.trim().to_lowercase();
        ui.horizontal_wrapped(|ui| {
            let mut shown = 0usize;
            for (namespace, value, count) in tag_usage {
                // Selected chips always stay visible, or a search would hide the filter that is
                // currently narrowing the grid.
                let selected = cf.traits.contains(value);
                if !selected && !needle.is_empty() && !value.to_lowercase().contains(&needle) {
                    continue;
                }
                if !selected && shown >= 18 {
                    continue;
                }
                shown += 1;
                let label = format!("{value}  {count}");
                if filter_chip(ui, &label, selected, facet_color(namespace)) {
                    if selected {
                        cf.traits.remove(value);
                    } else {
                        cf.traits.insert(value.clone());
                    }
                    changed = true;
                }
            }
        });
    }
    ui.add_space(6.0);

    // TRIANGLES — backed by asset.tri_count (DATA-MODEL §14), extracted after each scan.
    section_label(ui, "TRIANGLES");
    ui.horizontal_wrapped(|ui| {
        for b in TriBucket::ALL {
            let sel = cf.tri_buckets.contains(&b);
            if filter_chip(ui, b.label(), sel, theme::TYPE_MODEL) {
                if sel {
                    cf.tri_buckets.remove(&b);
                } else {
                    cf.tri_buckets.insert(b);
                }
            }
        }
    });
    ui.label(
        RichText::new("Models only \u{2014} counts appear as assets are measured.")
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

    // FAVOURITES — a pin, not a rating (DATA-MODEL §12).
    section_label(ui, "FAVOURITES");
    if filter_chip(
        ui,
        "\u{2665} Favourites only",
        cf.favorites_only,
        theme::DANGER,
    ) {
        cf.favorites_only = !cf.favorites_only;
        changed = true;
    }
    ui.add_space(6.0);

    // LINKS — the dependency graph (DATA-MODEL §10). Single-select; same chip toggles off.
    section_label(ui, "LINKS");
    ui.horizontal_wrapped(|ui| {
        for lf in LinkFilter::ALL {
            let sel = cf.link == Some(lf);
            if filter_chip(ui, lf.label(), sel, theme::TYPE_MATERIAL) {
                cf.link = if sel { None } else { Some(lf) };
                changed = true;
            }
        }
    });
    ui.label(
        RichText::new(
            "From Unity's own GUID refs \u{2014} assets outside a Unity project have none.",
        )
        .size(10.0)
        .color(theme::TEXT_DIM),
    );
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
#[allow(clippy::too_many_arguments)] // ponytail: mirrors grid state; an args struct would just rename them
fn asset_card(
    ui: &mut Ui,
    entry: &AssetEntry,
    thumb_size: f32,
    selected: bool,
    check_overlay: bool,
    // Has at least one resolved outbound dependency edge (DATA-MODEL §10).
    linked: bool,
    // Favourited (DATA-MODEL §12).
    favorite: bool,
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

                // Link-chain badge, bottom-right: this asset pulls others along on export
                // (DATA-MODEL §10). Outbound edges only — "used by" costs an export nothing.
                // Drawn, not typed: the bundled text fonts have no chain glyph (U+26AD renders as
                // tofu — same trap as the dropped ◉ in UI-SPEC §15), so two interlocking rings
                // are painted instead.
                if linked {
                    let c = thumb_rect.right_bottom() + egui::vec2(-11.0, -8.0);
                    let stroke = egui::Stroke::new(1.4, theme::TYPE_MATERIAL);
                    painter.circle_stroke(c + egui::vec2(-2.5, 0.0), 3.0, stroke);
                    painter.circle_stroke(c + egui::vec2(2.5, 0.0), 3.0, stroke);
                }

                // Favourite heart, top-left of the thumbnail — opposite the size tier so the two
                // never collide, and above the multi-select check which only shows while selecting.
                if favorite {
                    painter.text(
                        thumb_rect.left_top() + egui::vec2(4.0, 2.0),
                        egui::Align2::LEFT_TOP,
                        "\u{2665}",
                        egui::FontId::proportional(12.0),
                        theme::DANGER,
                    );
                }

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

/// Returns Some(new_rating) when the star row was clicked (Some(None) = cleared); `nav_to` is set
/// to a linked asset's id when one of its LINKED FILES rows is clicked.
// ponytail: a wide parameter list beats an args struct used exactly once — the panel reads its
// inputs straight off the selected entry. Group them if a third caller ever appears.
#[allow(clippy::too_many_arguments)]
fn inspector_pane(
    ui: &mut Ui,
    file: &CrawledFile,
    status: AssetStatus,
    rating: Option<u8>,
    metrics: lodestash_core::catalog::AssetMetrics,
    texture: Option<&TextureHandle>,
    library: Option<&str>,
    related: &[lodestash_core::catalog::RelatedAsset],
    tags: &[lodestash_core::catalog::TagAssignment],
    nav_to: &mut Option<i64>,
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
            // Technical facets (DATA-MODEL §14) — a row only appears once measured; NULL is
            // "unknown", so an absent row never reads as zero.
            if let (Some(w), Some(h)) = (metrics.width_px, metrics.height_px) {
                stat_row(ui, "Dimensions", &format!("{w} \u{00D7} {h}"));
            }
            if let Some(tris) = metrics.tri_count {
                stat_row(ui, "Triangles", &format_thousands(tris));
            }
            if let Some(ms) = metrics.duration_ms {
                stat_row(ui, "Duration", &crate::audio::fmt_time(ms as f32 / 1000.0));
            }
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

    // TRAITS (UI-SPEC §13.5) — the facet tags identification produced. AI tags carry a confidence
    // underbar; the value itself is never hidden, since the threshold is a display rule and not a
    // deletion (IDENTIFICATION-AND-SEARCH §2).
    ui.add_space(10.0);
    section_label(ui, "TRAITS");
    if tags.is_empty() {
        ui.label(
            RichText::new(if status.is_unidentified() {
                "No traits yet \u{2014} run Identify."
            } else {
                "No traits on this asset."
            })
            .size(10.0)
            .color(theme::TEXT_DIM),
        );
    } else {
        ui.horizontal_wrapped(|ui| {
            for tag in tags {
                let color = facet_color(&tag.namespace);
                let resp = ui
                    .add(
                        egui::Button::new(RichText::new(&tag.value).size(10.0).color(color))
                            .fill(color.linear_multiply(0.10))
                            .stroke(egui::Stroke::new(1.0, color.linear_multiply(0.55)))
                            .rounding(8.0),
                    )
                    .on_hover_text(match tag.confidence {
                        Some(c) => {
                            format!("{} \u{00b7} {:.0}% confident", tag.namespace, c * 100.0)
                        }
                        None => format!("{} \u{00b7} exact", tag.namespace),
                    });
                // Confidence underbar: a model's self-estimate, drawn rather than written so a
                // chip stays a chip.
                if let Some(c) = tag.confidence {
                    let r = resp.rect;
                    let w = (r.width() - 8.0) * (c as f32).clamp(0.0, 1.0);
                    ui.painter().line_segment(
                        [
                            egui::pos2(r.left() + 4.0, r.bottom() - 1.5),
                            egui::pos2(r.left() + 4.0 + w, r.bottom() - 1.5),
                        ],
                        egui::Stroke::new(1.5, color),
                    );
                }
            }
        });
    }

    // LINKED FILES (UI-SPEC §13.6) — what this asset drags along, and what would drag it.
    if !related.is_empty() {
        ui.add_space(10.0);
        section_label(ui, "LINKED FILES");
        ui.label(
            RichText::new("parsed from .meta GUID refs \u{00B7} prefab bindings")
                .color(theme::TEXT_DIM)
                .size(9.0),
        );
        let mut missing = 0usize;
        for rel in related {
            // Only glyphs the bundled fonts actually carry (◆/◇/•) — see the card badge note.
            let (glyph, color) = match rel.media_code.as_deref() {
                Some("material") => ("\u{25C7}", theme::TYPE_MATERIAL),
                Some("model_3d") => ("\u{25C6}", theme::TYPE_MODEL),
                _ => ("\u{2022}", theme::TYPE_IMAGE),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(glyph).color(color).size(11.0));
                match (&rel.name, rel.asset_id) {
                    (Some(name), Some(id)) => {
                        // Inbound rows read "used by", so the arrow direction carries the meaning
                        // the row's position alone can't.
                        let arrow = if rel.outbound { "\u{2192}" } else { "\u{2190}" };
                        ui.label(RichText::new(arrow).color(theme::TEXT_DIM).size(10.0));
                        let link = ui.add(
                            egui::Button::new(RichText::new(name).color(theme::TEXT).size(10.0))
                                .frame(false),
                        );
                        if link.clicked() {
                            *nav_to = Some(id);
                        }
                    }
                    _ => {
                        missing += 1;
                        ui.label(
                            RichText::new(format!(
                                "\u{2715} missing \u{00B7} {}",
                                rel.declared_path.as_deref().unwrap_or("unknown")
                            ))
                            .color(theme::DANGER)
                            .size(10.0),
                        );
                    }
                }
            });
        }
        if missing > 0 {
            ui.label(
                RichText::new("missing links are skipped on export")
                    .color(theme::WARNING)
                    .size(9.0),
            );
        }
    }

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

/// Assets per extracted pack as `(total, unidentified)`, trashed rows excluded (they left the
/// grid, so counting them would contradict what Browse → shows). Free function so the rule is
/// testable without an egui context.
fn package_asset_counts(
    entries: &[AssetEntry],
) -> std::collections::HashMap<String, (usize, usize)> {
    let mut out: std::collections::HashMap<String, (usize, usize)> = Default::default();
    for e in entries.iter().filter(|e| !e.trashed) {
        let Some(pkg) = e.package.as_deref() else {
            continue;
        };
        let slot = out.entry(pkg.to_string()).or_default();
        slot.0 += 1;
        slot.1 += usize::from(e.status.is_unidentified());
    }
    out
}

/// What the pack-stats modal shows (UI-SPEC §15), computed off the in-memory entries.
#[derive(Debug, Default, PartialEq)]
struct PackStats {
    assets: usize,
    bytes: u64,
    /// Summed triangles across the pack's measured models; `None` when nothing is measured yet.
    tris: Option<i64>,
    unidentified: usize,
    /// Per media type: (type, count, bytes), heaviest type first.
    contents: Vec<(Option<MediaType>, usize, u64)>,
    /// Biggest three assets: (name, bytes).
    heaviest: Vec<(String, u64)>,
}

/// Summarise one pack. Trashed assets are excluded, matching the counts on the pack row.
fn pack_stats(entries: &[AssetEntry], pack: &str) -> PackStats {
    let mine: Vec<&AssetEntry> = entries
        .iter()
        .filter(|e| !e.trashed && e.package.as_deref() == Some(pack))
        .collect();
    let mut out = PackStats {
        assets: mine.len(),
        bytes: mine.iter().map(|e| e.file.size_bytes).sum(),
        unidentified: mine.iter().filter(|e| e.status.is_unidentified()).count(),
        ..Default::default()
    };
    let measured: i64 = mine.iter().filter_map(|e| e.metrics.tri_count).sum();
    // Nothing measured ⇒ unknown, not "0 triangles" (DATA-MODEL §14).
    out.tris = mine
        .iter()
        .any(|e| e.metrics.tri_count.is_some())
        .then_some(measured);

    let mut by_type: std::collections::HashMap<Option<MediaType>, (usize, u64)> =
        Default::default();
    for e in &mine {
        let slot = by_type.entry(e.file.media_type).or_default();
        slot.0 += 1;
        slot.1 += e.file.size_bytes;
    }
    out.contents = by_type.into_iter().map(|(mt, (n, b))| (mt, n, b)).collect();
    out.contents.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| media_type_label(a.0).cmp(media_type_label(b.0)))
    });

    let mut biggest: Vec<&&AssetEntry> = mine.iter().collect();
    biggest.sort_by_key(|e| std::cmp::Reverse(e.file.size_bytes));
    out.heaviest = biggest
        .iter()
        .take(3)
        .map(|e| {
            let name = e
                .file
                .rel_path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (name, e.file.size_bytes)
        })
        .collect();
    out
}

/// Split ticked packs into the ones the extractor can act on now (their cache root is a library
/// source, so they have a `pending_packages` row) and the ones still blocked on that. Pure so the
/// batch rule is testable without an egui context.
fn split_extractable(
    selected: &std::collections::BTreeSet<PathBuf>,
    pending: &[PathBuf],
) -> (Vec<PathBuf>, Vec<PathBuf>) {
    selected
        .iter()
        .cloned()
        .partition(|path| pending.contains(path))
}

/// One stat tile in the pack-stats modal (UI-SPEC §15).
fn stat_tile(ui: &mut Ui, label: &str, value: &str, color: egui::Color32) {
    egui::Frame::none()
        .fill(theme::SURFACE)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .rounding(4.0)
        .inner_margin(egui::Margin::symmetric(14.0, 8.0))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(label).color(theme::TEXT_DIM).size(9.0));
                ui.label(RichText::new(value).color(color).size(15.0));
            });
        });
}

fn stat_row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).color(theme::TEXT_DIM).size(11.0));
    ui.add(egui::Label::new(RichText::new(value).color(theme::TEXT).size(11.0)).truncate());
    ui.end_row();
}

/// `12345` -> `12,345`. Triangle counts are the only place the UI shows a raw count big enough
/// to need grouping.
fn format_thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        return format!("-{out}");
    }
    out
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

/// Human-readable relative time from a Unix timestamp (e.g. "just now", "5m ago").
fn format_relative_time(secs: i64) -> String {
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else {
        return "unknown".to_string();
    };
    let elapsed = now.as_secs().saturating_sub(secs.max(0) as u64);
    match elapsed {
        0..=59 => "just now".to_string(),
        60..=3_599 => format!("{}m ago", elapsed / 60),
        3_600..=86_399 => format!("{}h ago", elapsed / 3_600),
        86_400..=2_592_000 => format!("{}d ago", elapsed / 86_400),
        _ => format_mtime(secs),
    }
}

/// Recursively sum the file sizes under `dir`. Skips unreadable entries rather than failing.
fn dir_size(dir: &std::path::Path) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_file() => {
                    total += entry.metadata().map(|m| m.len()).unwrap_or(0);
                }
                Ok(t) if t.is_dir() => {
                    total += dir_size(&path);
                }
                _ => {}
            }
        }
    }
    total
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

/// What kind of thumbnail an asset needs. Materials and audio have no pixels of their own, so
/// each gets a rendered stand-in rather than a decode.
#[derive(Debug, Clone)]
enum ThumbKind {
    Raster,
    Audio,
    /// A shader ball wearing the material's albedo, when the dependency graph resolved one.
    Material {
        albedo: Option<PathBuf>,
    },
}

/// Is `path` inside a source root the user opened, or the extraction workspace this app owns?
/// Pure so the one destructive path in the app has a testable guard (TECH-STACK §5.1).
fn under_known_root(
    path: &std::path::Path,
    roots: &[PathBuf],
    workspace: Option<&std::path::Path>,
) -> bool {
    roots.iter().any(|r| path.starts_with(r)) || workspace.is_some_and(|w| path.starts_with(w))
}

fn needs_thumbnail(mt: Option<MediaType>) -> bool {
    matches!(
        mt,
        // Model3d thumbnails are rendered offscreen by the CPU rasteriser in `thumb::generate`
        // (no GPU context needed, so it runs on the same worker threads as image decoding).
        // Audio takes the waveform branch in `generate_texture`.
        Some(
            MediaType::Image
                | MediaType::Texture
                | MediaType::Sprite
                | MediaType::Model3d
                | MediaType::Audio
                | MediaType::Material
        )
    )
}

// ponytail: 8 flat args instead of a params struct — one call site, all of them primitives.
#[allow(clippy::too_many_arguments)]
fn generate_texture(
    ctx: &egui::Context,
    abs_path: &std::path::Path,
    tex_name: &str,
    size_bytes: u64,
    mtime: i64,
    cache_dir: Option<&std::path::Path>,
    gen_px: u32,
    kind: ThumbKind,
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
            match &kind {
                // Audio has no pixels: draw the peak waveform the Quick Look player already
                // decodes (rodio lives in this crate, so the decode stays here and `thumb` only
                // owns the cache key + drawing).
                ThumbKind::Audio => thumb::generate_waveform(dir, &key, params, |bars| {
                    crate::audio::load_waveform(abs_path, bars).map(|m| m.peaks)
                })
                .ok(),
                ThumbKind::Material { albedo } => {
                    // The texture is part of the picture, so it is part of the cache key: swap the
                    // material's albedo and the old ball must not be served.
                    let key = match albedo {
                        Some(a) => format!("{key}-{}", thumb::meta_key(a, 0, 0)),
                        None => format!("{key}-flat"),
                    };
                    thumb::generate_material(dir, &key, params, albedo.as_deref()).ok()
                }
                ThumbKind::Raster => thumb::generate(abs_path, dir, &key, params).ok(),
            }
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

    /// The category buckets partition the 7-value `media_type` enum — every type lands in exactly
    /// one tab, and Materials is its own now that materials carry texture dependencies.
    #[test]
    fn category_tabs_partition_the_media_types() {
        use MediaType::*;
        for mt in [Model3d, Texture, Audio, Image, Sprite, Material, Font] {
            let hits: Vec<Category> = Category::TABS
                .iter()
                .map(|&(c, _)| c)
                .filter(|c| *c != Category::All && c.matches(Some(mt)))
                .collect();
            assert_eq!(
                hits.len(),
                1,
                "{mt:?} landed in {hits:?}, want exactly one tab"
            );
        }
        assert!(Category::Materials.matches(Some(Material)));
        assert!(
            !Category::Models.matches(Some(Material)),
            "materials left the Models bucket"
        );
        assert!(
            Category::All.matches(None),
            "All still shows anything the grid holds"
        );
    }

    /// Only real game assets reach the grid: a `.url` shortcut or a `.pdf` readme has no
    /// `media_type`, so the catalog would never hold it either.
    #[test]
    fn unclassified_files_are_not_browseable() {
        let f = |rel: &str| CrawledFile {
            abs_path: PathBuf::from(rel),
            rel_path: PathBuf::from(rel),
            size_bytes: 1,
            mtime: 1,
            media_type: lodestash_ingest::detect::by_extension(std::path::Path::new(rel)),
        };
        assert!(is_browseable(&f("Models/rock.fbx")));
        assert!(is_browseable(&f("Materials/rock.mat")));
        for junk in [
            "Docs/readme.pdf",
            "Asset Store.url",
            "Prefabs/rock.prefab",
            "Scripts/Player.cs",
            "notes.txt",
        ] {
            assert!(!is_browseable(&f(junk)), "{junk} is not a game asset");
        }
    }

    /// End-to-end on **real files with a real model**: crawl two assets whose names say nothing
    /// useful, run the drain with a vision model, and require that the tags come from the pixels.
    /// `#[ignore]` + env-gated like the other model-backed tests.
    ///
    /// ```bash
    /// OLLAMA_MODEL=moondream:latest cargo test -p lodestash-ui drain_tags_end_to_end -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "needs Ollama with a vision model"]
    fn drain_tags_end_to_end() {
        let Ok(model) = std::env::var("OLLAMA_MODEL") else {
            panic!("set OLLAMA_MODEL");
        };
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        // Neutral names, so a name-reading tagger can't pass: only the pixels distinguish these.
        let red = image::RgbaImage::from_pixel(256, 256, image::Rgba([200, 30, 30, 255]));
        red.save(lib.join("asset_0001.png")).unwrap();
        let mut checker = image::RgbaImage::new(256, 256);
        for (x, y, px) in checker.enumerate_pixels_mut() {
            let on = ((x / 32) + (y / 32)) % 2 == 0;
            *px = image::Rgba(if on {
                [20, 20, 20, 255]
            } else {
                [235, 235, 235, 255]
            });
        }
        checker.save(lib.join("asset_0002.png")).unwrap();

        let db = dir.path().join("catalog.db");
        {
            use lodestash_core::catalog::{Catalog, CrawlRecord};
            let mut cat = Catalog::open(&db).unwrap();
            cat.seed_media_types().unwrap();
            let src = cat
                .upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
                .unwrap();
            let recs: Vec<CrawlRecord> = ["asset_0001.png", "asset_0002.png"]
                .iter()
                .map(|rel| CrawlRecord {
                    workspace_rel: None,
                    rel_path: (*rel).into(),
                    name: (*rel).into(),
                    media_type: "image".into(),
                    size_bytes: std::fs::metadata(lib.join(rel)).unwrap().len() as i64,
                    mtime: 1,
                })
                .collect();
            cat.record_crawl(src, None, &recs).unwrap();
        }

        let mut cat = Catalog::open(&db).unwrap();
        let vision = std::env::var("OLLAMA_VISION").unwrap_or_else(|_| "moondream:latest".into());
        let tagger = lodestash_ai::ollama::OllamaTagger::new(model).with_vision(vision);
        let progress = crate::jobs::JobProgress::default();
        let cache = dir.path().join("thumbs");
        drain_tags(&mut cat, &tagger, None, Some(&cache), &progress);

        use std::sync::atomic::Ordering;
        assert_eq!(progress.done.load(Ordering::Relaxed), 2, "both assets ran");
        assert_eq!(
            cat.count_needing_tags().unwrap(),
            0,
            "the queue drains even if the model says nothing"
        );
        let mut any = false;
        for id in 1..=2i64 {
            let tags = cat.tags_for_asset(id).unwrap();
            println!(
                "asset {id}: {}",
                tags.iter()
                    .map(|t| format!("{}={}", t.namespace, t.value))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            any |= !tags.is_empty();
        }
        assert!(any, "a vision model saw two plain images and said nothing");
        // The point of tagging: a word that appears nowhere in the filename now finds the asset.
        let hits = cat.search_assets("red", 10).unwrap().unwrap_or_default();
        println!("search 'red' -> {hits:?}");
    }

    /// Tag a slice of the **real** library, then cancel — proving the pass is interruptible and
    /// resumable (`tags_dirty` is per asset) and giving the UI something real to draw.
    ///
    /// ```bash
    /// LODESTASH_DB=".../catalog.db" OLLAMA_MODEL=qwen3:8b OLLAMA_VISION=moondream:latest     ///   cargo test -p lodestash-ui tag_real_library_slice -- --ignored --nocapture
    /// ```
    #[test]
    #[ignore = "writes tags into a real catalog; needs Ollama"]
    fn tag_real_library_slice() {
        let db = std::env::var("LODESTASH_DB").expect("set LODESTASH_DB");
        let model = std::env::var("OLLAMA_MODEL").expect("set OLLAMA_MODEL");
        let seconds: u64 = std::env::var("TAG_SECONDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(120);
        let mut cat = Catalog::open(std::path::Path::new(&db)).expect("open real catalog");
        let before = cat.count_needing_tags().expect("count");

        let tagger = match std::env::var("OLLAMA_VISION") {
            Ok(v) => lodestash_ai::ollama::OllamaTagger::new(model).with_vision(v),
            Err(_) => lodestash_ai::ollama::OllamaTagger::new(model),
        };
        let progress = std::sync::Arc::new(crate::jobs::JobProgress::default());
        let stop = progress.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(seconds));
            stop.cancelled
                .store(true, std::sync::atomic::Ordering::Relaxed);
        });
        let cache = dirs::cache_dir()
            .map(|d| d.join("Lodestash").join("thumbs"))
            .expect("cache dir");
        drain_tags(&mut cat, &tagger, None, Some(&cache), &progress);

        let after = cat.count_needing_tags().expect("count after");
        let done = progress.done.load(std::sync::atomic::Ordering::Relaxed);
        println!("tagged {done} assets; queue {before} -> {after}");
        assert!(done > 0, "nothing was tagged");
        assert_eq!(
            after,
            before - done as i64,
            "the queue shrank by exactly what was tagged — the rest stays queued"
        );
        for (ns, value, n) in cat.tag_usage(15).expect("usage") {
            println!("  {ns:9} {value:24} {n}");
        }
    }

    /// Vision detection is name-based (Ollama reports no capability flag), so it has to catch the
    /// families people actually pull and not fire on a text model that merely contains a letter
    /// pair.
    #[test]
    fn vision_models_are_recognised_by_name() {
        for m in [
            "moondream:latest",
            "llava:13b",
            "qwen2.5vl:3b",
            "gemma3:4b",
            "minicpm-v",
        ] {
            assert!(is_vision_model(m), "{m} is a vision model");
        }
        for m in ["qwen3:8b", "llama3.2:3b", "mistral", "phi4", "deepseek-r1"] {
            assert!(!is_vision_model(m), "{m} is text-only");
        }
    }

    // ── Content filter predicate (design §3) ──────────────────────────────────

    #[test]
    fn content_filter_empty_passes_everything() {
        let cf = ContentFilter::default();
        let tiers = SizeTiers::default();
        assert!(
            cf.passes(10, None, None, &[], (false, false), false, &tiers),
            "no active facet = no constraint"
        );
        assert!(cf.passes(
            9_000_000,
            Some(5000),
            None,
            &["a".into()],
            (false, false),
            false,
            &tiers
        ));
    }

    /// The TRAIT chips filter on tag *values*, and an asset with no tags must fail the filter
    /// rather than pass it — otherwise selecting a trait would show the untagged backlog too.
    #[test]
    fn content_filter_traits_match_tag_values() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter::default();
        cf.traits.insert("rock".to_string());
        let t = |v: &[&str]| -> Vec<String> { v.iter().map(|s| s.to_string()).collect() };

        assert!(cf.passes(
            10,
            None,
            None,
            &t(&["rock", "grey"]),
            (false, false),
            false,
            &tiers
        ));
        assert!(
            cf.passes(
                10,
                None,
                None,
                &t(&["stone", "rock"]),
                (false, false),
                false,
                &tiers
            ),
            "any selected trait matches"
        );
        assert!(!cf.passes(
            10,
            None,
            None,
            &t(&["sword"]),
            (false, false),
            false,
            &tiers
        ));
        assert!(
            !cf.passes(10, None, None, &[], (false, false), false, &tiers),
            "an untagged asset is not a match"
        );
    }

    /// The delete-from-disk guard: only files under a source root the user opened, or inside the
    /// workspace this app owns. Everything else is refused, whatever path composition produced it.
    #[test]
    fn only_paths_under_known_roots_may_be_deleted() {
        let roots = vec![
            PathBuf::from("G:/Game_Assets/Synty"),
            PathBuf::from("C:/lib"),
        ];
        let ws = PathBuf::from("C:/cache/Lodestash/workspace");
        let ok = |p: &str| under_known_root(std::path::Path::new(p), &roots, Some(&ws));

        assert!(ok("G:/Game_Assets/Synty/Models/rock.fbx"));
        assert!(
            ok("C:/cache/Lodestash/workspace/abc/tex.png"),
            "workspace is ours"
        );
        assert!(
            !ok("C:/Windows/System32/kernel32.dll"),
            "outside every root"
        );
        assert!(
            !ok("G:/Game_Assets/Other/rock.fbx"),
            "sibling of a root is not in it"
        );
        assert!(
            !under_known_root(std::path::Path::new("C:/anything"), &roots, None),
            "no workspace configured means no workspace deletions"
        );
    }

    /// The favourites filter is a pin filter: it must not care about rating, tags or links, and an
    /// asset with no catalog row (hence no favourite) must fail it rather than sneak through.
    #[test]
    fn content_filter_favorites_only_selects_pins() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter {
            favorites_only: true,
            ..Default::default()
        };
        assert!(cf.passes(10, None, None, &[], (false, false), true, &tiers));
        assert!(!cf.passes(10, None, None, &[], (false, false), false, &tiers));
        // A 5-star asset is not a favourite unless it was pinned.
        assert!(!cf.passes(10, None, Some(5), &[], (false, false), false, &tiers));
        assert_eq!(cf.active_count(), 1, "the chip counts as an active facet");

        cf.favorites_only = false;
        assert!(
            cf.passes(10, None, None, &[], (false, false), false, &tiers),
            "off, it constrains nothing"
        );
    }

    /// The LINKS chips read the dependency graph, not the file (DATA-MODEL §10): an asset with no
    /// edges fails either chip, and the two directions select different assets.
    #[test]
    fn content_filter_links_select_by_edge_direction() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter {
            link: Some(LinkFilter::HasDependencies),
            ..Default::default()
        };
        assert!(
            cf.passes(10, None, None, &[], (true, false), false, &tiers),
            "uses others"
        );
        assert!(
            !cf.passes(10, None, None, &[], (false, true), false, &tiers),
            "only used by others"
        );
        assert!(
            !cf.passes(10, None, None, &[], (false, false), false, &tiers),
            "no edges at all"
        );

        cf.link = Some(LinkFilter::HasDependents);
        assert!(cf.passes(10, None, None, &[], (false, true), false, &tiers));
        assert!(!cf.passes(10, None, None, &[], (true, false), false, &tiers));
        // A material sits in the middle of a chain and satisfies both.
        assert!(cf.passes(10, None, None, &[], (true, true), false, &tiers));
        assert_eq!(cf.active_count(), 1, "the chip counts as an active facet");
    }

    #[test]
    fn content_filter_size_tier_is_and_over_the_selected_set() {
        let tiers = SizeTiers::default(); // S<1MB, M<4MB, L<8MB, XL≥8MB
        let mut cf = ContentFilter::default();
        cf.sizes.insert(SizeTier::S);
        cf.sizes.insert(SizeTier::L);
        let mb = |n: u64| n * 1024 * 1024;
        assert!(
            cf.passes(mb(0), None, None, &[], (false, false), false, &tiers),
            "500 KB → S, selected"
        );
        assert!(
            !cf.passes(mb(2), None, None, &[], (false, false), false, &tiers),
            "2 MB → M, not selected"
        );
        assert!(
            cf.passes(mb(5), None, None, &[], (false, false), false, &tiers),
            "5 MB → L, selected"
        );
        assert!(
            !cf.passes(mb(9), None, None, &[], (false, false), false, &tiers),
            "9 MB → XL, not selected"
        );
    }

    #[test]
    fn content_filter_tri_bucket_excludes_assets_without_a_count() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter::default();
        cf.tri_buckets.insert(TriBucket::K1to3);
        assert!(
            cf.passes(10, Some(2000), None, &[], (false, false), false, &tiers),
            "2000 tris → 1k–3k, selected"
        );
        assert!(
            !cf.passes(10, Some(500), None, &[], (false, false), false, &tiers),
            "500 tris → ≤1k, not selected"
        );
        assert!(
            !cf.passes(10, None, None, &[], (false, false), false, &tiers),
            "no tri count is excluded when a tri facet is active"
        );
    }

    #[test]
    fn content_filter_trait_matches_any_selected() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter::default();
        cf.traits.insert("rusty".into());
        assert!(cf.passes(
            10,
            None,
            None,
            &["rusty".into(), "barrel".into()],
            (false, false),
            false,
            &tiers
        ));
        assert!(!cf.passes(
            10,
            None,
            None,
            &["barrel".into()],
            (false, false),
            false,
            &tiers
        ));
        assert!(
            !cf.passes(10, None, None, &[], (false, false), false, &tiers),
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
            metrics: Default::default(),
            content_hash: None,
            texture: None,
            thumb_spawned: false,
        }
    }

    #[test]
    fn pack_stats_summarise_only_that_pack() {
        let mk = |rel: &str, size: u64, mt: Option<MediaType>, pkg: Option<&str>, tris| {
            let mut e = entry_full(rel, size, 0, mt);
            e.package = pkg.map(str::to_string);
            e.metrics.tri_count = tris;
            e
        };
        let entries = vec![
            mk(
                "rock.fbx",
                900,
                Some(MediaType::Model3d),
                Some("Pack"),
                Some(1200),
            ),
            mk(
                "rock_d.png",
                400,
                Some(MediaType::Texture),
                Some("Pack"),
                None,
            ),
            mk(
                "other.fbx",
                5000,
                Some(MediaType::Model3d),
                Some("Other"),
                Some(99),
            ),
        ];

        let st = pack_stats(&entries, "Pack");
        assert_eq!(st.assets, 2);
        assert_eq!(st.bytes, 1300);
        assert_eq!(st.tris, Some(1200), "only this pack's measured models");
        assert_eq!(st.unidentified, 2, "fresh entries are Discovered");
        assert_eq!(st.contents.len(), 2);
        assert_eq!(st.contents[0].1, 1, "heaviest type first");
        assert_eq!(st.heaviest[0].0, "rock.fbx");

        // Nothing measured ⇒ unknown, not zero.
        let unmeasured = vec![mk("a.png", 1, Some(MediaType::Image), Some("P"), None)];
        assert_eq!(pack_stats(&unmeasured, "P").tris, None);
    }

    #[test]
    fn pack_counts_exclude_trashed_and_count_unidentified() {
        let mut a = entry(0, "a.png", Some(MediaType::Image));
        a.package = Some("Pack".into());
        let mut b = entry(0, "b.png", Some(MediaType::Image));
        b.package = Some("Pack".into());
        b.status = AssetStatus::Identified;
        let mut trashed = entry(0, "c.png", Some(MediaType::Image));
        trashed.package = Some("Pack".into());
        trashed.trashed = true;
        let loose = entry(0, "d.png", Some(MediaType::Image));

        let counts = package_asset_counts(&[a, b, trashed, loose]);
        assert_eq!(counts.get("Pack"), Some(&(2, 1)));
        assert_eq!(counts.len(), 1, "assets outside a pack are not counted");
    }

    #[test]
    fn batch_extract_splits_ready_from_blocked_packs() {
        let sel: std::collections::BTreeSet<PathBuf> = ["a.unitypackage", "b.unitypackage"]
            .iter()
            .map(PathBuf::from)
            .collect();
        let (ready, blocked) = split_extractable(&sel, &[PathBuf::from("b.unitypackage")]);
        assert_eq!(ready, vec![PathBuf::from("b.unitypackage")]);
        assert_eq!(
            blocked,
            vec![PathBuf::from("a.unitypackage")],
            "a pack whose cache isn't a source yet stays blocked, not silently dropped"
        );
    }

    #[test]
    fn triangle_counts_are_grouped_for_display() {
        assert_eq!(format_thousands(7), "7");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1000), "1,000");
        assert_eq!(format_thousands(1234567), "1,234,567");
    }

    #[test]
    fn folder_children_splits_dirs_and_files() {
        let entries = vec![
            entry(0, "a.png", Some(MediaType::Image)),
            entry(0, "PolygonCity/tex.png", Some(MediaType::Image)),
            entry(0, "PolygonCity/Models/m.fbx", Some(MediaType::Model3d)),
            entry(1, "other.png", Some(MediaType::Image)), // different source — excluded
        ];
        let (idxs, tiles) =
            folder_children(&entries, 0, std::path::Path::new(""), Category::All, None);
        assert_eq!(idxs, vec![0], "only the direct-child file at the root");
        // PolygonCity has 1 file directly in it (tex.png) and 2 total (incl. Models/m.fbx).
        assert_eq!(
            tiles,
            vec![FolderTile {
                name: "PolygonCity".to_owned(),
                direct: 1,
                recursive: 2,
            }]
        );

        let (idxs, tiles) = folder_children(
            &entries,
            0,
            std::path::Path::new("PolygonCity"),
            Category::All,
            None,
        );
        assert_eq!(idxs, vec![1]);
        assert_eq!(
            tiles,
            vec![FolderTile {
                name: "Models".to_owned(),
                direct: 1,
                recursive: 1,
            }]
        );
    }

    #[test]
    fn folder_children_respects_tab_filter() {
        let entries = vec![
            entry(0, "PolygonCity/tex.png", Some(MediaType::Image)),
            entry(0, "PolygonCity/m.fbx", Some(MediaType::Model3d)),
        ];
        let (idxs, tiles) = folder_children(
            &entries,
            0,
            std::path::Path::new(""),
            Category::Models,
            None,
        );
        assert!(idxs.is_empty());
        assert_eq!(
            tiles,
            vec![FolderTile {
                name: "PolygonCity".to_owned(),
                direct: 1,
                recursive: 1,
            }],
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
            metrics: Default::default(),
            content_hash: None,
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

    /// §5.2 import: publisher/category come from the Asset Store's own two levels, and a
    /// plain folder of packs must not be given invented ones.
    #[test]
    fn import_reads_publisher_and_category_only_where_the_layout_has_them() {
        let root = PathBuf::from("C:/cache");
        let at = |rel: &str| root.join(rel);
        assert_eq!(
            store_publisher_category(&root, &at("SICS Games/3D Models/Isles.unitypackage")),
            (Some("SICS Games".into()), Some("3D Models".into()))
        );
        assert_eq!(
            store_publisher_category(&root, &at("SomePublisher/Pack.unitypackage")),
            (Some("SomePublisher".into()), None),
            "one level deep names a publisher, not a category"
        );
        assert_eq!(
            store_publisher_category(&root, &at("Pack.unitypackage")),
            (None, None),
            "a flat folder of packs has neither"
        );
        assert_eq!(
            store_publisher_category(
                &root,
                std::path::Path::new("D:/elsewhere/Pack.unitypackage")
            ),
            (None, None),
            "a path outside the picked root tells us nothing"
        );
    }

    /// An extracted payload's workspace location must belong to *its own* record. An
    /// unclassified file between two classified ones is dropped from the records, so anything
    /// pairing the two lists positionally shifts every location after it.
    #[test]
    fn workspace_locations_follow_their_own_record() {
        let ws = PathBuf::from("C:/ws");
        let file = |rel: &str, mt: Option<MediaType>| CrawledFile {
            abs_path: ws.join(rel),
            rel_path: PathBuf::from(rel),
            size_bytes: 1,
            mtime: 0,
            media_type: mt,
        };
        let files = [
            file("aa/rock.fbx", Some(MediaType::Model3d)),
            file("bb/notes.txt", None), // unclassified — never persisted
            file("cc/rock.png", Some(MediaType::Texture)),
        ];
        let records = crawl_records_in_workspace(files.iter(), Some(&ws));
        let got: Vec<(&str, Option<&str>)> = records
            .iter()
            .map(|r| (r.rel_path.as_str(), r.workspace_rel.as_deref()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("aa/rock.fbx", Some("aa/rock.fbx")),
                ("cc/rock.png", Some("cc/rock.png")),
            ]
        );
        // A file outside the workspace (or no workspace at all) simply has no location.
        assert!(crawl_records_in_workspace(files.iter(), None)
            .iter()
            .all(|r| r.workspace_rel.is_none()));
    }

    /// §5.4: the browse list is a folder tree, so `.meta` sidecars stay out, entries land
    /// under their in-archive folder, and a payload at the archive root gets its own group.
    #[test]
    fn browse_entries_group_by_in_archive_folder() {
        let entry = |p: &str| PackageEntryRecord {
            guid: p.to_string(),
            pathname: Some(p.to_string()),
            tar_offset: Some(0),
            size_bytes: Some(1),
        };
        let pkg = PendingPackage {
            source_idx: 0,
            abs_path: PathBuf::from("Pack.unitypackage"),
            display_name: "Pack".into(),
            nested_candidates: Vec::new(),
            chosen_variant: 0,
            extracting: false,
            progress: Arc::default(),
            entries: vec![
                entry("Assets/Props/rock.fbx"),
                entry("Assets/Props/rock.fbx.meta"),
                entry("Assets/Textures/rock.png"),
                entry("Assets/Props/log.fbx"),
                entry("readme.txt"),
            ],
            job: None,
            browsing: true,
            picked: std::collections::BTreeSet::new(),
        };
        let folders = pkg.folders();
        let shape: Vec<(&str, usize)> =
            folders.iter().map(|(f, m)| (f.as_str(), m.len())).collect();
        assert_eq!(
            shape,
            vec![("", 1), ("Assets/Props", 2), ("Assets/Textures", 1)],
            "folder-ordered groups, root first, .meta excluded"
        );
        let props = &folders[1].1;
        assert_eq!(props, &vec![0, 3], "group members index back into entries");
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
