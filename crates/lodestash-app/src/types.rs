//! Domain types shared by every frontend.
//!
//! Extracted from `lodestash-ui/src/app.rs` (B1). None of these carry a rendering concern, so
//! the egui shell and the Tauri backend build on the same definitions. Types that genuinely
//! hold GPU or widget state — `AssetEntry`'s texture handle, `ImageViewer`, `ThumbReady`, the
//! card-layout constants — deliberately stayed behind in the UI crate.

use std::path::PathBuf;
use std::sync::Arc;

use lodestash_core::catalog::{CrawlRecord, HydratedAsset, PackageEntryRecord, StatusCounts};
use lodestash_core::types::{AssetStatus, MediaType};
use lodestash_ingest::crawler::CrawledFile;

use crate::config::{SizeTier, SizeTiers};
use crate::jobs::JobProgress;

/// The top-bar category tabs (UI-SPEC §0.3, prototype `catDefs`): four coarse buckets
/// grouping the 7-value `media_type` enum. The enum itself is unchanged (a data-model
/// invariant) — these are a presentation grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    All,
    Models,
    Materials,
    Sounds,
    Images,
}

impl Category {
    /// Tab order and label. The label carries no icon: egui drew a Unicode stand-in here, but the
    /// real mark is an `ls-type-*` sprite id the frontend resolves (DESIGN-GUIDELINES §5).
    pub const TABS: &'static [(Category, &'static str)] = &[
        (Category::All, "All"),
        (Category::Models, "Models"),
        (Category::Materials, "Materials"),
        (Category::Sounds, "Sounds"),
        (Category::Images, "Images"),
    ];

    /// Bucket → `media_type` mapping: Models = model_3d, Materials = material, Sounds = audio,
    /// Images = image+texture+sprite+font (UI-SPEC §0.3). Materials used to ride along with
    /// Models; they carry their own texture dependencies, so they are worth reaching directly.
    pub fn matches(self, mt: Option<MediaType>) -> bool {
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

    /// The CSS custom property naming this tab's colour (prototype `catColors`).
    ///
    /// Returning the token rather than a resolved colour is what lets this type live outside a
    /// renderer: `docs/design/tokens.css` is the source of truth, the webview uses the name
    /// directly, and the egui shell resolves it through `theme::token`.
    pub fn token(self) -> &'static str {
        match self {
            Category::All => "--ls-gold",
            Category::Models => "--ls-type-model",
            Category::Materials => "--ls-type-material",
            Category::Sounds => "--ls-type-sound",
            Category::Images => "--ls-type-image",
        }
    }
}

pub struct AssetEntry<T = ()> {
    /// Extracted-package origin (the sidebar "package library" facet). None for plain
    /// on-disk files. ponytail: session-only — rehydration re-derives it when the
    /// catalog packages model lands (P2 card).
    pub package: Option<String>,
    pub file: CrawledFile,
    /// Index into `scan_roots` — which source this asset came from (the Library Source).
    pub source_idx: usize,
    /// Catalog identity, attached when the write-behind persist reads back
    /// (None until then, or for files the catalog doesn't hold — unclassified).
    pub asset_id: Option<i64>,
    /// Catalog lifecycle status (DATA-MODEL §8); entries default to Discovered
    /// until the read-back lands.
    pub status: AssetStatus,
    /// In the Trash (catalog-only soft delete; files on disk untouched). Hidden from
    /// the grid, listed on the Trash screen (DATA-MODEL §8).
    pub trashed: bool,
    /// 1..=5 stars; None = unrated (DATA-MODEL §12).
    pub rating: Option<u8>,
    /// Technical facets extracted from the file (DATA-MODEL §14); all-None until the
    /// extraction drain has reached this asset.
    pub metrics: lodestash_core::catalog::AssetMetrics,
    /// BLAKE3 hash of the primary file; exact-dedup key for the Duplicates badge.
    pub content_hash: Option<String>,
    /// Frontend-owned preview handle. Generic so this type carries no renderer: the egui shell
    /// instantiates it with `egui::TextureHandle`, headless callers leave it as `()`.
    ///
    /// It stays a field rather than a side table keyed by entry index because `entries` is
    /// rebuilt from a filtered subset on a partial rescan — indices shift, and a side table
    /// would silently pair a card with another asset's thumbnail.
    pub texture: Option<T>,
    pub thumb_spawned: bool,
}

/// Build the whole folder hierarchy for one pack's assets, path-sorted, each node carrying its
/// direct and recursive asset counts (DATA-MODEL §7 pack browse). Pure over the entry list so it is
/// unit-testable; the UI caches it and rebuilds it with the grid cache. Complement of
/// [`folder_children`], which resolves a single level — this is the full tree the left pane draws.
pub fn build_pack_dir_tree<T>(entries: &[AssetEntry<T>], src: usize, pack: &str) -> Vec<DirNode> {
    use std::collections::BTreeMap;
    // dir path → (direct assets in it, recursive assets under it). BTreeMap keeps path order.
    let mut counts: BTreeMap<PathBuf, (usize, usize)> = BTreeMap::new();
    for e in entries {
        if e.source_idx != src || e.trashed || e.package.as_deref() != Some(pack) {
            continue;
        }
        let Some(parent) = e.file.rel_path.parent() else {
            continue;
        };
        // Every ancestor dir counts the file recursively; only its immediate parent counts it direct.
        let mut dir = PathBuf::new();
        for comp in parent.components() {
            dir.push(comp);
            let slot = counts.entry(dir.clone()).or_insert((0, 0));
            slot.1 += 1;
            if dir == parent {
                slot.0 += 1;
            }
        }
    }
    let paths: Vec<PathBuf> = counts.keys().cloned().collect();
    paths
        .iter()
        .map(|p| DirNode {
            name: p
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
            depth: p.components().count(),
            direct: counts[p].0,
            recursive: counts[p].1,
            // Component-wise prefix: `Assets/AB` is not a child of `Assets/A`.
            has_children: paths.iter().any(|q| q != p && q.starts_with(p)),
            path: p.clone(),
        })
        .collect()
}

/// Sort order for the Unity Assets package list (FEATURES.md §5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnitySort {
    /// Indexed / added packs first, then alphabetical (the default landing order).
    #[default]
    IndexedFirst,
    Name,
    Size,
    Date,
}

/// A pack mint deferred until the current scan's persist finishes, so the mint's writes don't race
/// the scan-persist ("database is locked", HANDOFF §6 — single-writer). Set by `pin_and_mint_now`
/// right before it kicks the hydrate crawl; consumed in `receive_catalog` once that persist lands.
pub struct PendingMint {
    pub package_id: i64,
    pub name: String,
    pub abs_path: PathBuf,
}

/// A pending confirm for opening an oversized pack (FEATURES.md §5.4): indexing it in place would
/// inflate the whole archive, so the user is asked whether to extract it to the temp cache instead.
pub struct LargePackPrompt {
    pub package_id: i64,
    pub name: String,
    pub abs_path: PathBuf,
    pub size_bytes: u64,
    /// Carried from the triggering action: Open → jumps into the folders after indexing.
    pub then_browse: bool,
}

/// A child-folder tile in Folders view: its name, the count of assets **directly** inside it, and
/// the **recursive** total including everything in its subfolders (shown as `direct (recursive)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTile {
    pub name: String,
    pub direct: usize,
    pub recursive: usize,
}

/// One folder in the pack directory tree — the persistent left pane of the pack browser (FEATURES.md
/// §5.4, the published nav redesign). `path` is source-root-relative (the browse `current_dir` key);
/// `depth` (component count) drives indentation; `direct`/`recursive` mirror [`FolderTile`]; and
/// `has_children` gates the expand chevron. The tree is path-sorted, so parents precede their children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirNode {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub direct: usize,
    pub recursive: usize,
    pub has_children: bool,
}

/// A tree row is visible only when every one of its ancestor folders is expanded. Root-level nodes
/// (whose only ancestor is the empty source root) are always visible.
pub fn ancestors_expanded(
    path: &std::path::Path,
    expanded: &std::collections::HashSet<PathBuf>,
) -> bool {
    let mut anc = path.parent();
    while let Some(a) = anc {
        if a.as_os_str().is_empty() {
            break;
        }
        if !expanded.contains(a) {
            return false;
        }
        anc = a.parent();
    }
    true
}

/// Seed the expanded set by auto-opening down the **linear spine** to where the content first fans
/// out: a folder opens iff every one of its ancestors is a single-child link. That opens the spine
/// **and** the first branching folder (so its children show), exactly like the mockup — deeper
/// branches stay closed until the user opens them.
pub fn seed_expanded(nodes: &[DirNode]) -> std::collections::HashSet<PathBuf> {
    let mut child_dirs: std::collections::HashMap<PathBuf, usize> =
        std::collections::HashMap::new();
    for n in nodes {
        if let Some(par) = n.path.parent() {
            *child_dirs.entry(par.to_path_buf()).or_default() += 1;
        }
    }
    nodes
        .iter()
        .filter(|n| {
            let mut anc = n.path.parent();
            while let Some(a) = anc {
                if a.as_os_str().is_empty() {
                    break;
                }
                if child_dirs.get(a).copied().unwrap_or(0) != 1 {
                    return false;
                }
                anc = a.parent();
            }
            true
        })
        .map(|n| n.path.clone())
        .collect()
}

/// Result of a background crawl. `generation` guards against a stale scan landing
/// after the user has already opened a different folder/project.
/// `partial` is `Some(source_idx)` when only one source was rescanned; the handler merges
/// those entries into the existing grid instead of replacing everything.
pub struct ScanDone {
    pub generation: u64,
    /// (source index into scan_roots, crawled file)
    pub entries: Vec<(usize, CrawledFile)>,
    pub pending_packages: Vec<PendingPackage>,
    pub partial: Option<usize>,
}

/// Sidebar LIBRARIES facet selection: a source root, or one extracted package.
/// Scope of a manual Identify pass (embeddings + tags, AI-PIPELINE §2). The user picks how much work
/// to (re)do; core just re-flags the matching rows dirty and the drains run over the dirty set.
#[derive(Clone, PartialEq)]
pub enum IdentifyScope {
    /// Only never-processed assets — the default; a mint leaves them dirty until first drained.
    New,
    /// Re-identify the whole live library (redo everything).
    Full,
    /// Re-identify only assets with no tags (failed / never tagged).
    Unidentified,
    /// A specific selection (right-click "Identify selected").
    Selected(Vec<i64>),
}

#[derive(Clone, PartialEq)]
pub enum LibraryFilter {
    Source(usize),
    Package(String),
}

/// A `.unitypackage` found while crawling, not yet extracted into the grid. Its manifest is
/// already resolved (read on the same background crawl thread) so the UI can offer an "Extract"
/// action immediately, with no separate loading step.
pub struct PendingPackage {
    pub source_idx: usize,
    pub abs_path: PathBuf,
    pub display_name: String,
    /// (display label, real `pathname`) for each nested sub-package — e.g. a URP/Built-in
    /// render-pipeline-variant wrapper (INGESTION.md §2 "Container nuance"). Empty when this
    /// package directly contains real assets (no wrapper).
    pub nested_candidates: Vec<(String, String)>,
    /// Index into `nested_candidates`, defaulted to the URP variant when one exists.
    pub chosen_variant: usize,
    pub extracting: bool,
    pub progress: Arc<JobProgress>,
    /// Manifest locators (guid/pathname/tar-offset/size) captured at browse time —
    /// persisted as `package_entry` rows on extraction so surgical re-extraction never
    /// needs a second full scan (DATA-MODEL §7, INGESTION §2).
    pub entries: Vec<PackageEntryRecord>,
    /// Tray row of the running extraction (full or surgical), if any.
    pub job: Option<u64>,
    /// Contents list expanded under the package row (pick-and-extract).
    pub browsing: bool,
    /// Indices into `entries` picked for surgical extraction.
    pub picked: std::collections::BTreeSet<usize>,
}

impl PendingPackage {
    /// The pickable entries grouped by their in-archive folder, folder-ordered
    /// (FEATURES.md §5.4). Indices point back into `entries`; a pack holds hundreds of
    /// them, and a flat list of hundreds is not a file tree.
    pub fn folders(&self) -> Vec<(String, Vec<usize>)> {
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
    pub fn extractable(&self) -> impl Iterator<Item = usize> + '_ {
        self.entries.iter().enumerate().filter_map(|(i, e)| {
            let p = e.pathname.as_deref()?;
            (!p.to_ascii_lowercase().ends_with(".meta")
                && e.tar_offset.is_some()
                && e.size_bytes.is_some())
            .then_some(i)
        })
    }
}

/// Startup hydrate: the catalog's view of the sources, shown instantly while the
/// filesystem crawl reconciles in the background. Superseded by that crawl's ScanDone.
pub struct HydrateDone {
    pub generation: u64,
    /// (source index, hydrated rows) per source.
    pub sources: Vec<(usize, Vec<HydratedAsset>)>,
    /// Package-extracted rows (only when "cache extracted packs" is on), with the
    /// workspace-existence check already done on the hydrate thread.
    pub extracted: Vec<(usize, Vec<lodestash_core::catalog::HydratedExtracted>)>,
    /// In-pack rows for pinned packs (DATA-MODEL §7 / M14) — bytes still inside the archive,
    /// carried with their locator for on-demand thumbnail/preview pulls.
    pub in_pack: Vec<(usize, Vec<lodestash_core::catalog::HydratedInPack>)>,
}

/// Per-root asset identity maps: root → (rel_path → (asset id, status, content_hash)).
pub type AssetIdentityMaps = Vec<(
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
pub struct CatalogSynced {
    pub generation: u64,
    pub counts: Option<StatusCounts>,
    /// True for a backlog drain, which computes no counts — `None` there means "nothing to
    /// report", not the persist failure it means everywhere else.
    pub drain_only: bool,
    /// The read path: per source root, `rel_path → (asset id, status)` for every live
    /// catalog row, joined onto the in-memory entries on receipt.
    pub assets: AssetIdentityMaps,
}

/// An extracted package's worth of persist work: the `package` row plus its
/// extracted assets (DATA-MODEL §7), handed to the catalog thread.
pub struct PackagePersist {
    /// Source root the package was found under (existing or to-be-created source row).
    pub root: String,
    pub root_name: String,
    pub package_path: String,
    pub name: String,
    pub publisher: Option<String>,
    pub category: Option<String>,
    pub records: Vec<CrawlRecord>,
    /// Manifest locators for `package_entry` (surgical re-extraction, DATA-MODEL §7).
    pub entries: Vec<PackageEntryRecord>,
    /// (name, root) of every project source, for the read-back status counts.
    pub all_roots: Vec<(String, String)>,
    /// False when the extraction was cancelled part-way. The rows that *were* placed are valid and
    /// resumable, so they still persist — but the pack must not be stamped `unpacked`, or a
    /// cancelled extract permanently records a truncated asset count as the whole pack.
    pub complete: bool,
}

/// One source's worth of persist work handed to the catalog thread.
pub struct PersistBatch {
    pub root: String,
    pub name: String,
    /// `import_source.blacklist` JSON for the one-time config adoption (DATA-MODEL §13).
    pub excludes_json: Option<String>,
    pub records: Vec<CrawlRecord>,
}

/// One pack payload pulled out for preview (FEATURES.md §5.4). The file lives in a scratch
/// directory, not the workspace: a peek is a look, so it must not leave catalog rows, grid
/// entries or cached extractions behind.
pub struct PackagePeek {
    pub name: String,
    pub media_type: Option<MediaType>,
    pub size_bytes: u64,
    pub path: PathBuf,
}

/// The Import-packages dialog (FEATURES.md §5.2): a folder scanned for `.unitypackage`
/// files, each of which can be imported as its own Library. Nothing is copied or unpacked —
/// importing writes a `package` row that points at the archive where it already lives.
pub struct ImportScan {
    pub root: PathBuf,
    pub scanning: bool,
    /// (in-folder group, archive path, display name, size) — group is the containing folder
    /// relative to the picked root, which is the publisher/category tree in an Asset Store cache.
    pub found: Vec<(String, PathBuf, String, u64)>,
    pub selected: std::collections::BTreeSet<usize>,
}

/// `PendingPackage.source_idx` for a pack opened from the catalog that lies outside every
/// library source: browse and preview work, extraction has nowhere to persist to.
pub const NO_SOURCE: usize = usize::MAX;

/// Sentinel `entry_idx` for the viewer stack while it is showing a peeked pack entry —
/// no grid entry backs it, and a real index must never collide with it.
pub const PEEK_ENTRY_IDX: usize = usize::MAX;

/// Result of a background package extraction.
pub struct ExtractDone {
    pub display_name: String,
    /// Identifies which `PendingPackage` this came from — matched by path rather than index
    /// since `pending_packages` may have been mutated while the extraction ran.
    pub abs_path: PathBuf,
    pub source_idx: usize,
    pub files: Vec<CrawledFile>,
    /// The package's manifest locators, carried through for `package_entry` persist.
    pub entries: Vec<PackageEntryRecord>,
    /// True for a surgical (picked-assets) extraction: the pending card stays so more
    /// can be pulled later; false for a full extraction, which retires it.
    pub partial: bool,
}

/// Triangle-count bucket for the Triangles facet (models only). Design §3 "TRIANGLES".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TriBucket {
    Le1k,
    K1to3,
    K3to6,
    Ge6k,
}

impl TriBucket {
    pub const ALL: [TriBucket; 4] = [
        TriBucket::Le1k,
        TriBucket::K1to3,
        TriBucket::K3to6,
        TriBucket::Ge6k,
    ];

    pub fn of(tris: u32) -> TriBucket {
        match tris {
            0..=1000 => TriBucket::Le1k,
            1001..=3000 => TriBucket::K1to3,
            3001..=6000 => TriBucket::K3to6,
            _ => TriBucket::Ge6k,
        }
    }

    pub fn label(self) -> &'static str {
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
pub struct ContentFilter {
    pub traits: std::collections::BTreeSet<String>,
    /// Canonical category paths (mapping library, DATA-MODEL §4) selected in the CATEGORY facet.
    pub categories: std::collections::BTreeSet<String>,
    pub tri_buckets: std::collections::BTreeSet<TriBucket>,
    pub sizes: std::collections::BTreeSet<SizeTier>,
    /// Minimum star rating (None = no constraint). DATA-MODEL §12.
    pub min_rating: Option<u8>,
    /// Dependency-graph constraint (DATA-MODEL §10 "Search"). None = no constraint.
    pub link: Option<LinkFilter>,
    /// Show only favourites (DATA-MODEL §12). A one-way switch: "not favourited" is not a thing
    /// anyone browses for.
    pub favorites_only: bool,
    pub trait_search: String,
}

/// Traits screen state (UI-SPEC §9), including the Mapping tab (card 172).
#[derive(Default)]
pub struct VocabState {
    pub tab: VocabTab,
    /// The vocabulary, reloaded from the catalog when `dirty`. Traits first (see `list_vocabulary`).
    pub rows: Vec<lodestash_core::catalog::VocabTrait>,
    pub dirty: bool,
    pub search: String,
    /// NEW TRAIT form.
    pub nt_name: String,
    pub nt_namespace: String,
    /// media_type code the trait applies to; empty = any.
    pub nt_scope: String,
    pub nt_usage: String,
    /// Mapping tab, reloaded when `map_dirty`.
    pub map_dirty: bool,
    /// Active taxonomy `(id, name, version)`, `None` until one is created.
    pub taxonomy: Option<(i64, String, i64)>,
    /// `(id, pref_label, path)` per category in the active taxonomy.
    pub categories: Vec<(i64, String, String)>,
    /// `(id, category_id, category_path, match_type, pattern, priority, enabled)` per rule.
    pub rules: Vec<(i64, i64, String, String, String, i64, bool)>,
    /// NEW CATEGORY + NEW RULE forms.
    pub nc_label: String,
    pub nr_category: Option<i64>,
    pub nr_match_type: String,
    pub nr_pattern: String,
    pub nr_priority: String,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum VocabTab {
    #[default]
    Traits,
    Mapping,
}

/// The `has_dependencies` / `has_dependents` chips (DATA-MODEL §10). Single-select: an asset that
/// is both ends of a chain (a material) satisfies either, so combining them would only ever
/// narrow to that intersection — not what the chips read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkFilter {
    /// Uses something: a material with textures, a mesh with a material.
    HasDependencies,
    /// Something uses it: a texture some material paints with.
    HasDependents,
}

impl LinkFilter {
    pub const ALL: [LinkFilter; 2] = [LinkFilter::HasDependencies, LinkFilter::HasDependents];

    pub fn label(self) -> &'static str {
        match self {
            LinkFilter::HasDependencies => "uses others",
            LinkFilter::HasDependents => "used by others",
        }
    }
}

impl ContentFilter {
    pub fn active_count(&self) -> usize {
        self.traits.len()
            + self.categories.len()
            + self.tri_buckets.len()
            + self.sizes.len()
            + usize::from(self.min_rating.is_some())
            + usize::from(self.link.is_some())
            + usize::from(self.favorites_only)
    }

    pub fn any_active(&self) -> bool {
        self.active_count() > 0
    }

    /// Empty every filter set (the popover's "Clear all", and part of the Library nav reset).
    pub fn clear(&mut self) {
        self.traits.clear();
        self.categories.clear();
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
    pub fn passes(
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

/// Which screen the sidebar has selected (UI-SPEC §0.2). Screens without a build yet
/// render the §12 stub — the nav row is never hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Screen {
    #[default]
    Library,
    /// Starred assets (FEATURES.md §4.7) — the library grid filtered to favourites; 2nd in nav.
    Favorites,
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
    /// Sidebar navigation order (UI-SPEC §0.2). Favorites sits 2nd, directly under Library.
    pub const NAV: [Screen; 11] = [
        Screen::Library,
        Screen::Favorites,
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

    pub fn label(self) -> &'static str {
        match self {
            Screen::Library => "Library",
            Screen::Favorites => "Favorites",
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
    pub fn glyph(self) -> &'static str {
        match self {
            Screen::Library => "\u{229E}",
            Screen::Favorites => "\u{2605}",
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

    /// One sentence of what the destination will be, for the §12 stub screen.
    pub fn stub_line(self) -> &'static str {
        match self {
            Screen::Library | Screen::Favorites | Screen::Settings => "", // built — never stubbed
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
pub enum SearchMode {
    #[default]
    Tags,
    Ai,
}

/// STATUS facet single-select (UI-SPEC §0.2). Maps to `status` (DATA-MODEL §8);
/// `discovered|indexed` fold into Unidentified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    All,
    Unidentified,
    Identified,
    Confirmed,
}

impl StatusFilter {
    /// Does an asset's catalog status pass this facet? Pure for testability.
    pub fn matches(self, status: AssetStatus) -> bool {
        match self {
            StatusFilter::All => true,
            StatusFilter::Unidentified => status.is_unidentified(),
            StatusFilter::Identified => status == AssetStatus::Identified,
            StatusFilter::Confirmed => status == AssetStatus::Confirmed,
        }
    }
}
