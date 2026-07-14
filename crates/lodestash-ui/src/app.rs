//! Main application state and the eframe::App impl.
//! Spec: DESIGN-GUIDELINES.md §4, §7, §9; ARCHITECTURE.md §6.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use egui::{Key, RichText, ScrollArea, TextureHandle, Ui, Vec2};
use lodestash_core::types::MediaType;
use lodestash_ingest::crawler::{self, CrawledFile};
use lodestash_ingest::thumb;

use crate::config::{self, AppConfig, SizeTier, SizeTiers, SortBy, ViewMode};
use crate::theme;

// ── Category tabs ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    All,
    Type(MediaType),
}

impl Category {
    const TABS: &'static [(Category, &'static str)] = &[
        (Category::All, "All"),
        (Category::Type(MediaType::Model3d), "3D Models"),
        (Category::Type(MediaType::Audio), "Audio"),
        (Category::Type(MediaType::Image), "Images"),
        (Category::Type(MediaType::Texture), "Textures"),
        (Category::Type(MediaType::Sprite), "Sprites"),
        (Category::Type(MediaType::Material), "Materials"),
        (Category::Type(MediaType::Font), "Fonts"),
    ];

    fn matches(self, mt: Option<MediaType>) -> bool {
        match self {
            Category::All => true,
            Category::Type(t) => mt == Some(t),
        }
    }

    fn color(self) -> egui::Color32 {
        match self {
            Category::All => theme::TEXT_MUTED,
            Category::Type(mt) => theme::media_type_color(mt),
        }
    }
}

// ── Asset entry ───────────────────────────────────────────────────────────────

struct AssetEntry {
    file: CrawledFile,
    /// Index into `scan_roots` — which source this asset came from (the Library Source).
    source_idx: usize,
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
}

// ── Constants ─────────────────────────────────────────────────────────────────

const CARD_LABEL: f32 = 36.0;
const CARD_PAD: f32 = 8.0;
pub const THUMB_MIN: f32 = 64.0;
pub const THUMB_MAX: f32 = 256.0;
/// Extra rows above/below the visible viewport to prefetch thumbnails for.
const LOOKAHEAD_ROWS: usize = 4;
/// Rows beyond the visible window whose textures are kept resident; anything further
/// is evicted so VRAM stays bounded while scrolling a large library.
/// ponytail: window-based eviction, not true LRU — swap for a budgeted LRU if thrashing shows.
const KEEP_ROWS: usize = 32;

/// Discrete grid-size stops for the segmented selector, all driving `thumb_size` (design §2).
const GRID_STOPS: [(f32, &str); 3] = [(96.0, "Compact"), (128.0, "Cozy"), (176.0, "Large")];

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
    trait_search: String,
}

impl ContentFilter {
    fn active_count(&self) -> usize {
        self.traits.len() + self.tri_buckets.len() + self.sizes.len()
    }

    fn any_active(&self) -> bool {
        self.active_count() > 0
    }

    /// Empty every filter set (the popover's "Clear all", and part of the Library nav reset).
    fn clear(&mut self) {
        self.traits.clear();
        self.tri_buckets.clear();
        self.sizes.clear();
        self.trait_search.clear();
    }

    /// Pure predicate: does an asset pass the filter? An asset passes if it matches the selected
    /// traits (any) AND selected tri-buckets (any) AND selected size tiers (any); an empty section
    /// imposes no constraint. Assets lacking a tri count are excluded when any tri-bucket is
    /// selected (design §3). Kept pure over primitives so it's unit-testable without an
    /// `AssetEntry` (TECH-STACK.md §5.1).
    fn passes(&self, size: u64, tri: Option<u32>, traits: &[String], tiers: &SizeTiers) -> bool {
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

/// Which screen the nav rail has selected. Design §5 adds Settings alongside the Library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Screen {
    #[default]
    Library,
    Settings,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct LodestashApp {
    config: AppConfig,
    /// The source directories currently loaded (may be multiple for a multi-source project).
    scan_roots: Vec<PathBuf>,
    /// Name of the open LibraryProject (if any).
    project_name: Option<String>,
    search: String,
    active_tab: Category,
    sort_by: SortBy,
    sort_asc: bool,
    thumb_size: f32,
    view_mode: ViewMode,
    /// Flat mode: restrict the grid to one source (None = all libraries).
    source_filter: Option<usize>,
    /// Folders mode location: None = top level (source tiles); Some = (source, dir rel to root).
    current_dir: Option<(usize, PathBuf)>,
    /// Child folders at `current_dir`: (name, contained-asset count). Rebuilt with the cache.
    folder_tiles: Vec<(String, usize)>,
    /// Bumped on every folder navigation so the grid ScrollArea gets fresh scroll state.
    nav_generation: u64,
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
    /// Index into `sorted_cache` of the currently selected asset.
    selected_cache_idx: Option<usize>,
    /// When true, the Quick Look overlay is open.
    quick_look: bool,
    /// When true, the Sources panel (view/add/remove project sources + app-data locations) is open.
    sources_panel_open: bool,
    /// The ⚑ Filters content filter (trait / triangles / file-size). Transient. Design §3.
    content_filter: ContentFilter,
    /// Active top-level screen (Library grid vs Settings). Design §5.
    screen: Screen,
}

impl LodestashApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);

        let (thumb_tx, thumb_rx) = mpsc::channel();
        let (scan_tx, scan_rx) = mpsc::channel();
        let config = AppConfig::load();
        let sort_by = config.view.sort_by;
        let sort_asc = config.view.sort_asc;
        let thumb_size = config.view.thumb_size.clamp(THUMB_MIN, THUMB_MAX);
        let view_mode = config.view.mode;

        let mut app = Self {
            scan_roots: Vec::new(),
            project_name: None,
            search: String::new(),
            active_tab: Category::All,
            sort_by,
            sort_asc,
            thumb_size,
            view_mode,
            source_filter: None,
            current_dir: None,
            folder_tiles: Vec::new(),
            nav_generation: 0,
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
            quick_look: false,
            sources_panel_open: false,
            content_filter: ContentFilter::default(),
            screen: Screen::Library,
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
        let Some(proj_name) = self.project_name.clone() else {
            return;
        };
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
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
        self.selected_cache_idx = None;
        self.quick_look = false;
        self.source_filter = None;
        self.current_dir = None;
        self.folder_tiles = Vec::new();
        self.nav_generation += 1;

        self.scan_generation += 1;
        self.scanning = true;
        let generation = self.scan_generation;
        let tx = self.scan_tx.clone();
        std::thread::spawn(move || {
            let entries: Vec<(usize, CrawledFile)> = roots
                .iter()
                .enumerate()
                .flat_map(|(i, root)| crawler::crawl(root).into_iter().map(move |f| (i, f)))
                .collect();
            let _ = tx.send(ScanDone {
                generation,
                entries,
            });
        });

        ctx.request_repaint();
    }

    // ── Per-frame helpers ─────────────────────────────────────────────────────

    fn receive_scan(&mut self) {
        while let Ok(msg) = self.scan_rx.try_recv() {
            if msg.generation != self.scan_generation {
                continue; // stale scan from a superseded load
            }
            self.scanning = false;
            self.entries = msg
                .entries
                .into_iter()
                .map(|(source_idx, f)| AssetEntry {
                    file: f,
                    source_idx,
                    texture: None,
                    thumb_spawned: false,
                })
                .collect();
            self.current_dir = self.default_dir();
            self.tab_counts = Category::TABS
                .iter()
                .map(|&(cat, _)| {
                    self.entries
                        .iter()
                        .filter(|e| cat.matches(e.file.media_type))
                        .count()
                })
                .collect();
            self.cache_dirty = true;
            self.last_cols = 0;
            self.last_row_start = usize::MAX;
            self.last_row_end = 0;
        }
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
    fn rebuild_cache(&mut self) {
        let selected_entry = self
            .selected_cache_idx
            .and_then(|ci| self.sorted_cache.get(ci).copied());
        let q = self.search.to_lowercase();
        let tab = self.active_tab;

        let name_matches = |e: &AssetEntry| {
            q.is_empty()
                || e.file
                    .rel_path
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&q)
        };

        let (mut idxs, tiles): (Vec<usize>, Vec<(String, usize)>) =
            match (self.view_mode, self.current_dir.clone()) {
                (ViewMode::Flat, _) => {
                    let filter = self.source_filter;
                    let idxs = self
                        .entries
                        .iter()
                        .enumerate()
                        .filter(|(_, e)| {
                            filter.map_or(true, |s| e.source_idx == s)
                                && tab.matches(e.file.media_type)
                                && name_matches(e)
                        })
                        .map(|(i, _)| i)
                        .collect();
                    (idxs, Vec::new())
                }
                // Folders, top level: one tile per source; searching searches everything.
                (ViewMode::Folders, None) => {
                    if q.is_empty() {
                        let tiles = self
                            .scan_roots
                            .iter()
                            .enumerate()
                            .map(|(i, root)| {
                                let count = self
                                    .entries
                                    .iter()
                                    .filter(|e| e.source_idx == i && tab.matches(e.file.media_type))
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
                    if q.is_empty() {
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
            idxs.retain(|&i| cf.passes(entries[i].file.size_bytes, None, &[], tiers));
        }

        sort_indices(&mut idxs, &self.entries, self.sort_by, self.sort_asc);

        self.sorted_cache = idxs;
        self.selected_cache_idx =
            selected_entry.and_then(|e| self.sorted_cache.iter().position(|&i| i == e));
        self.cache_dirty = false;
    }

    /// Drop thumbnail textures far outside the visible window so VRAM stays bounded.
    /// In-flight loads (spawned, no texture yet) are left alone — their send must still
    /// decrement `pending_thumbs`; the texture lands and is evicted on a later pass.
    fn evict_far_thumbnails(&mut self, cols: usize, row_start: usize, row_end: usize) {
        let (keep_start, keep_end) = keep_range(row_start, row_end, cols, self.sorted_cache.len());
        let selected_entry = self
            .selected_cache_idx
            .and_then(|ci| self.sorted_cache.get(ci).copied());

        let keep: std::collections::HashSet<usize> = self.sorted_cache[keep_start..keep_end]
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
        self.source_filter = None;
        self.active_tab = Category::All;
        self.search.clear();
        self.content_filter.clear();
        self.current_dir = self.default_dir();
        self.selected_cache_idx = None;
        self.nav_generation += 1;
        self.cache_dirty = true;
    }

    fn nav_rail(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav_rail")
            .exact_width(52.0)
            .resizable(false)
            .frame(
                egui::Frame::none()
                    .fill(theme::SURFACE)
                    .stroke(egui::Stroke::new(1.0, theme::BORDER))
                    .inner_margin(egui::Margin::symmetric(7.0, 10.0)),
            )
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    // Library — the reset-to-everything affordance (design §4).
                    if nav_button(
                        ui,
                        "\u{229E}",
                        theme::GOLD,
                        self.screen == Screen::Library,
                        true,
                    )
                    .on_hover_text("Library")
                    .clicked()
                    {
                        self.go_to_library();
                    }
                    ui.add_space(8.0);
                    // Forward-declared destinations (glyph catalog §01) — disabled until their
                    // screens exist. Shown so the rail reads as the designed structure.
                    for (glyph, color, name) in [
                        ("\u{2726}", theme::ARCANE_BRIGHT, "Identify"),
                        ("\u{27E1}", theme::GOLD, "Collections"),
                        ("\u{25A4}", theme::TEXT_MUTED, "Packages"),
                        ("\u{2756}", theme::ARCANE_BRIGHT, "Traits"),
                        ("\u{25C9}", theme::TEXT_MUTED, "Stats"),
                        ("\u{29C9}", theme::WARNING, "Duplicates"),
                        ("\u{2298}", theme::DANGER, "Trash"),
                    ] {
                        nav_button(ui, glyph, color, false, false).on_hover_text(name);
                        ui.add_space(5.0);
                    }
                });
                // Settings pinned to the bottom of the rail.
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    if nav_button(
                        ui,
                        "\u{2699}",
                        theme::TEXT_MUTED,
                        self.screen == Screen::Settings,
                        true,
                    )
                    .on_hover_text("Settings")
                    .clicked()
                    {
                        self.screen = Screen::Settings;
                        self.selected_cache_idx = None;
                    }
                });
            });
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

    // ── Settings screen (design §5) ────────────────────────────────────────────

    fn settings_screen(&mut self, ui: &mut Ui) {
        let mut save = false;
        let mut quality_changed = false;

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

                        // 5.1 Sources (Library).
                        settings_card(ui, "LIBRARY \u{00B7} SOURCES", |ui| {
                            if self.scan_roots.is_empty() {
                                ui.label(
                                    RichText::new("No sources yet \u{2014} open a folder to start.")
                                        .size(12.0)
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            for root in &self.scan_roots {
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
                                });
                                ui.add_space(2.0);
                            }
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new(
                                    "Per-source exclusions (blacklist) \u{2014} planned; needs the \
                                     import_source catalog (DATA-MODEL.md \u{00A7}1).",
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
fn sort_indices(idxs: &mut Vec<usize>, entries: &[AssetEntry], sort_by: SortBy, asc: bool) {
    match sort_by {
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
        if e.source_idx != src || !tab.matches(e.file.media_type) {
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
    let buf_start = start.saturating_sub(LOOKAHEAD_ROWS);
    let buf_end = (end + LOOKAHEAD_ROWS).min(total_rows);
    (buf_start, buf_end)
}

/// The `sorted_cache` index range `[start, end)` whose textures stay resident:
/// the visible row window widened by `KEEP_ROWS` on each side, in entries, clamped to `len`.
fn keep_range(row_start: usize, row_end: usize, cols: usize, len: usize) -> (usize, usize) {
    let keep_start = (row_start.saturating_sub(KEEP_ROWS) * cols).min(len);
    let keep_end = ((row_end + KEEP_ROWS) * cols).min(len);
    (keep_start, keep_end)
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for LodestashApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_scan();
        self.receive_thumbnails();

        // While a scan or thumbnails are in flight, wake up at ~30 fps to drain the
        // channels. Keeping this in the main thread (not the worker threads) prevents
        // stray request_repaint() calls from injecting extra frames during resize events.
        if self.pending_thumbs > 0 || self.scanning {
            ctx.request_repaint_after(Duration::from_millis(33));
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
                }
            }
            if space && self.selected_cache_idx.is_some() {
                self.quick_look = !self.quick_look;
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
                    // quick_look stays open; navigating moves the selected asset
                }
            } else if right || left || down || up {
                self.selected_cache_idx = Some(0);
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
                    ui.label(
                        RichText::new("\u{25C8} Lodestash")
                            .color(theme::GOLD)
                            .size(16.0)
                            .strong(),
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

                    // Project / source info — click to open the Sources panel.
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
                                    RichText::new(format!("{name}  \u{2014}  {detail}"))
                                        .color(theme::TEXT_DIM)
                                        .size(11.0),
                                )
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::NONE)
                                .frame(false),
                            )
                            .on_hover_text("View / manage sources");
                        if resp.clicked() {
                            self.sources_panel_open = true;
                        }
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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

        // ── Navigation rail (left) ────────────────────────────────────────────
        self.nav_rail(ctx);

        // ── Category tab bar + view toolbar ──────────────────────────────────
        if self.screen == Screen::Library && !self.entries.is_empty() {
            egui::TopBottomPanel::top("tabbar")
                .frame(
                    egui::Frame::none()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin {
                            left: 12.0,
                            right: 12.0,
                            top: 4.0,
                            bottom: 0.0,
                        })
                        .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (i, &(cat, label)) in Category::TABS.iter().enumerate() {
                            let n = self.tab_counts[i];
                            if n == 0 && cat != Category::All {
                                continue;
                            }
                            let is_active = self.active_tab == cat;
                            let tab_color = cat.color();
                            let rich = RichText::new(format!("{label}  {n}")).size(12.0).color(
                                if is_active {
                                    tab_color
                                } else {
                                    theme::TEXT_DIM
                                },
                            );
                            let resp = ui.add(
                                egui::Button::new(rich)
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::NONE)
                                    .frame(false),
                            );
                            if resp.clicked() {
                                self.active_tab = cat;
                                self.cache_dirty = true;
                            }
                            if is_active {
                                let rect = resp.rect;
                                ui.painter().line_segment(
                                    [
                                        egui::pos2(rect.min.x, rect.max.y),
                                        egui::pos2(rect.max.x, rect.max.y),
                                    ],
                                    egui::Stroke::new(2.0, tab_color),
                                );
                            }
                            ui.add_space(8.0);
                        }
                    });
                });

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
                    ui.horizontal(|ui| {
                        let search_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .id(egui::Id::new("search_field"))
                                .hint_text("Search\u{2026}  /")
                                .desired_width(180.0),
                        );
                        if search_resp.changed() {
                            self.cache_dirty = true;
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

                        // Browse view toggle (DESIGN-GUIDELINES.md §7 "Browse views").
                        ui.separator();
                        for (mode, label) in
                            [(ViewMode::Flat, "Grid"), (ViewMode::Folders, "Folders")]
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

                        // Library filter — flat mode, multi-source projects only.
                        if self.view_mode == ViewMode::Flat && self.scan_roots.len() > 1 {
                            let selected = match self.source_filter {
                                None => "All libraries".to_owned(),
                                Some(i) => config::stem_name(&self.scan_roots[i]),
                            };
                            let before = self.source_filter;
                            egui::ComboBox::from_id_salt("lib_filter")
                                .width(110.0)
                                .selected_text(
                                    RichText::new(selected).color(theme::TEXT).size(11.0),
                                )
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut self.source_filter,
                                        None,
                                        "All libraries",
                                    );
                                    for i in 0..self.scan_roots.len() {
                                        let name = config::stem_name(&self.scan_roots[i]);
                                        ui.selectable_value(&mut self.source_filter, Some(i), name);
                                    }
                                });
                            if self.source_filter != before {
                                self.cache_dirty = true;
                            }
                        }

                        ui.separator();
                        ui.label(RichText::new("Sort").color(theme::TEXT_DIM).size(11.0));

                        let sort_before = self.sort_by;
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

                        let asc_before = self.sort_asc;
                        let dir_label = if self.sort_asc {
                            "\u{2191}"
                        } else {
                            "\u{2193}"
                        };
                        if ui.small_button(dir_label).clicked() {
                            self.sort_asc = !self.sort_asc;
                        }

                        if self.sort_by != sort_before || self.sort_asc != asc_before {
                            self.cache_dirty = true;
                            self.save_view_prefs();
                        }

                        // Grid-size segmented selector (design §2) — grid view only; drives the
                        // persisted `thumb_size`, the single source of truth.
                        if self.view_mode == ViewMode::Flat {
                            ui.separator();
                            ui.label(RichText::new("Grid").color(theme::TEXT_DIM).size(11.0));
                            for (px, label) in GRID_STOPS {
                                let active = (self.thumb_size - px).abs() < 8.0;
                                if ui
                                    .selectable_label(active, RichText::new(label).size(11.0))
                                    .clicked()
                                    && !active
                                {
                                    self.thumb_size = px;
                                    self.save_view_prefs();
                                    self.cache_dirty = true;
                                }
                            }
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{} assets", self.sorted_cache.len()))
                                    .color(theme::TEXT_MUTED)
                                    .size(11.0),
                            );
                        });
                    });
                });
        }

        // ── Inspector pane (right side) ───────────────────────────────────────
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
                                inspector_pane(
                                    ui,
                                    file,
                                    entry.texture.as_ref(),
                                    library.as_deref(),
                                );
                            });
                    }
                }
            }
        }

        // ── Main content ─────────────────────────────────────────────────────
        let mut open_project: Option<String> = None;
        let mut open_single: Option<PathBuf> = None;
        let mut spawn_queue: Vec<usize> = Vec::new();
        let mut clicked_cache: Option<usize> = None;
        let mut evict_window: Option<(usize, usize, usize)> = None;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                if self.screen == Screen::Settings {
                    self.settings_screen(ui);
                    return;
                }
                if self.entries.is_empty() {
                    if self.scanning {
                        scanning_state(ui);
                    } else {
                        empty_state(ui, &self.config, &mut open_project, &mut open_single);
                    }
                    return;
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
                    let (sq, new_cols, new_row_start, new_row_end, clicked) = thumbnail_grid(
                        ui,
                        &self.entries,
                        &self.sorted_cache,
                        self.thumb_size,
                        self.last_cols,
                        self.last_row_start,
                        self.last_row_end,
                        self.selected_cache_idx,
                        self.nav_generation,
                        &self.config.size_tiers,
                    );
                    if new_cols != self.last_cols
                        || new_row_start != self.last_row_start
                        || new_row_end != self.last_row_end
                    {
                        evict_window = Some((new_cols, new_row_start, new_row_end));
                    }
                    self.last_cols = new_cols;
                    self.last_row_start = new_row_start;
                    self.last_row_end = new_row_end;
                    spawn_queue = sq;
                    clicked_cache = clicked;
                }
            });

        if let Some((cols, row_start, row_end)) = evict_window {
            self.evict_far_thumbnails(cols, row_start, row_end);
        }

        if let Some(cache_idx) = clicked_cache {
            if self.selected_cache_idx == Some(cache_idx) {
                // Second click on same card: open Quick Look.
                self.quick_look = true;
            } else {
                self.selected_cache_idx = Some(cache_idx);
                self.quick_look = false;
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
                    if let Some(entry) = self.entries.get(entry_idx) {
                        let mut open = self.quick_look;
                        egui::Window::new("Quick Look")
                            .id(egui::Id::new("quick_look"))
                            .open(&mut open)
                            .collapsible(false)
                            .resizable(true)
                            .default_size([520.0, 560.0])
                            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                            .frame(
                                egui::Frame::window(&ctx.style())
                                    .fill(theme::SURFACE_RAISED)
                                    .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                            )
                            .show(ctx, |ui| {
                                quick_look_content(ui, entry);
                            });
                        self.quick_look = open;
                    }
                }
            }
        }

        // ── Sources panel ──────────────────────────────────────────────────────
        if self.sources_panel_open {
            let mut open = self.sources_panel_open;
            let mut to_remove: Option<PathBuf> = None;
            let mut add_clicked = false;
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
fn thumbnail_grid(
    ui: &mut Ui,
    entries: &[AssetEntry],
    indices: &[usize],
    thumb_size: f32,
    prev_cols: usize,
    prev_row_start: usize,
    prev_row_end: usize,
    selected_cache_idx: Option<usize>,
    nav_salt: u64,
    tiers: &SizeTiers,
) -> (Vec<usize>, usize, usize, usize, Option<usize>) {
    let card_h = thumb_size + CARD_LABEL;
    let available = ui.available_width() - ui.spacing().scroll.bar_width - 2.0;
    let cols = grid_columns(available, thumb_size);
    let total_rows = indices.len().div_ceil(cols);

    let mut to_spawn: Vec<usize> = Vec::new();
    let mut row_start_out: usize = prev_row_start;
    let mut row_end_out: usize = prev_row_end;
    let mut clicked: Option<usize> = None;

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
                        let cache_idx = start + col;
                        let selected = selected_cache_idx == Some(cache_idx);
                        if asset_card(ui, &entries[idx], thumb_size, selected, tiers) {
                            clicked = Some(cache_idx);
                        }
                    }
                });
            }
        });

    (to_spawn, cols, row_start_out, row_end_out, clicked)
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

/// A navigation-rail glyph button. Active shows a gold left-accent; disabled dims the glyph.
fn nav_button(
    ui: &mut Ui,
    glyph: &str,
    color: egui::Color32,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(38.0), sense);
    let painter = ui.painter();
    let bg = if active {
        theme::OVERLAY
    } else if enabled && resp.hovered() {
        theme::OVERLAY.linear_multiply(0.5)
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 7.0, bg);
    if active {
        painter.rect_filled(
            egui::Rect::from_min_size(
                rect.left_top() + egui::vec2(-5.0, 7.0),
                egui::vec2(3.0, rect.height() - 14.0),
            ),
            2.0,
            theme::GOLD,
        );
    }
    let gc = if enabled {
        color
    } else {
        color.linear_multiply(0.32)
    };
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(19.0),
        gc,
    );
    resp
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

    changed
}

/// Returns true if the card was clicked.
fn asset_card(
    ui: &mut Ui,
    entry: &AssetEntry,
    thumb_size: f32,
    selected: bool,
    tiers: &SizeTiers,
) -> bool {
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

    resp.response.interact(egui::Sense::click()).clicked()
}

// ── Inspector pane ────────────────────────────────────────────────────────────

fn inspector_pane(
    ui: &mut Ui,
    file: &CrawledFile,
    texture: Option<&TextureHandle>,
    library: Option<&str>,
) {
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
            if let Some(lib) = library {
                stat_row(ui, "Library", lib);
            }
            stat_row(ui, "Path", &file.abs_path.to_string_lossy());
        });

    ui.add_space(8.0);
    ui.label(
        RichText::new(
            "Space \u{2192} Quick Look  \u{2022}  \u{2191}\u{2193}\u{2190}\u{2192} navigate",
        )
        .color(theme::TEXT_DIM)
        .size(10.0),
    );
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

fn quick_look_content(ui: &mut Ui, entry: &AssetEntry) {
    let avail = ui.available_size();
    let preview_size = (avail.x.min(avail.y - 80.0)).max(64.0);

    ui.vertical_centered(|ui| {
        if let Some(tex) = &entry.texture {
            ui.add(
                egui::Image::new((tex.id(), Vec2::splat(preview_size)))
                    .fit_to_exact_size(Vec2::splat(preview_size))
                    .rounding(4.0),
            );
        } else {
            let type_color = entry
                .file
                .media_type
                .map(theme::media_type_color)
                .unwrap_or(theme::TEXT_DIM);
            let (resp, painter) =
                ui.allocate_painter(Vec2::splat(preview_size), egui::Sense::hover());
            painter.rect_filled(resp.rect, 8.0, type_color.linear_multiply(0.12));
            painter.text(
                resp.rect.center(),
                egui::Align2::CENTER_CENTER,
                type_label_char(entry.file.media_type),
                egui::FontId::proportional(preview_size * 0.18),
                type_color.linear_multiply(0.7),
            );
        }

        ui.add_space(12.0);
        let name = entry
            .file
            .rel_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        ui.label(RichText::new(&name).color(theme::TEXT).size(15.0).strong());
        ui.label(
            RichText::new(format!(
                "{} \u{2022} {}",
                media_type_label(entry.file.media_type),
                format_bytes(entry.file.size_bytes),
            ))
            .color(theme::TEXT_DIM)
            .size(11.0),
        );
        ui.add_space(8.0);
        ui.label(
            RichText::new("Space / Esc \u{2192} close  \u{2022}  \u{2190}\u{2192} navigate")
                .color(theme::TEXT_DIM)
                .size(10.0),
        );
    });
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

    // ── Content filter predicate (design §3) ──────────────────────────────────

    #[test]
    fn content_filter_empty_passes_everything() {
        let cf = ContentFilter::default();
        let tiers = SizeTiers::default();
        assert!(
            cf.passes(10, None, &[], &tiers),
            "no active facet = no constraint"
        );
        assert!(cf.passes(9_000_000, Some(5000), &["a".into()], &tiers));
    }

    #[test]
    fn content_filter_size_tier_is_and_over_the_selected_set() {
        let tiers = SizeTiers::default(); // S<1MB, M<4MB, L<8MB, XL≥8MB
        let mut cf = ContentFilter::default();
        cf.sizes.insert(SizeTier::S);
        cf.sizes.insert(SizeTier::L);
        let mb = |n: u64| n * 1024 * 1024;
        assert!(cf.passes(mb(0), None, &[], &tiers), "500 KB → S, selected");
        assert!(
            !cf.passes(mb(2), None, &[], &tiers),
            "2 MB → M, not selected"
        );
        assert!(cf.passes(mb(5), None, &[], &tiers), "5 MB → L, selected");
        assert!(
            !cf.passes(mb(9), None, &[], &tiers),
            "9 MB → XL, not selected"
        );
    }

    #[test]
    fn content_filter_tri_bucket_excludes_assets_without_a_count() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter::default();
        cf.tri_buckets.insert(TriBucket::K1to3);
        assert!(
            cf.passes(10, Some(2000), &[], &tiers),
            "2000 tris → 1k–3k, selected"
        );
        assert!(
            !cf.passes(10, Some(500), &[], &tiers),
            "500 tris → ≤1k, not selected"
        );
        assert!(
            !cf.passes(10, None, &[], &tiers),
            "no tri count is excluded when a tri facet is active"
        );
    }

    #[test]
    fn content_filter_trait_matches_any_selected() {
        let tiers = SizeTiers::default();
        let mut cf = ContentFilter::default();
        cf.traits.insert("rusty".into());
        assert!(cf.passes(10, None, &["rusty".into(), "barrel".into()], &tiers));
        assert!(!cf.passes(10, None, &["barrel".into()], &tiers));
        assert!(
            !cf.passes(10, None, &[], &tiers),
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
            file: CrawledFile {
                abs_path: PathBuf::from(rel),
                rel_path: PathBuf::from(rel),
                size_bytes: 1,
                mtime: 0,
                media_type: mt,
            },
            source_idx: src,
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
        let (idxs, tiles) = folder_children(
            &entries,
            0,
            std::path::Path::new(""),
            Category::Type(MediaType::Model3d),
        );
        assert!(idxs.is_empty());
        assert_eq!(
            tiles,
            vec![("PolygonCity".to_owned(), 1)],
            "tile counts are tab-filtered"
        );
    }

    #[test]
    fn keep_range_widens_and_clamps() {
        // Interior window: widened KEEP_ROWS each side, in entries.
        let (s, e) = keep_range(100, 105, 10, 10_000);
        assert_eq!(s, (100 - KEEP_ROWS) * 10);
        assert_eq!(e, (105 + KEEP_ROWS) * 10);
        // Top of a short list: clamps to [0, len].
        assert_eq!(keep_range(0, 2, 10, 100), (0, 100));
        // Empty list never panics.
        assert_eq!(keep_range(0, 0, 1, 0), (0, 0));
    }

    #[test]
    fn lookahead_window_clamps_to_bounds() {
        // Top of list: cannot go below 0.
        assert_eq!(lookahead_window(0, 5, 100), (0, 5 + LOOKAHEAD_ROWS));
        // Middle: widened both sides.
        assert_eq!(
            lookahead_window(20, 30, 100),
            (20 - LOOKAHEAD_ROWS, 30 + LOOKAHEAD_ROWS)
        );
        // Bottom: end cannot exceed total_rows.
        assert_eq!(lookahead_window(95, 100, 100), (95 - LOOKAHEAD_ROWS, 100));
    }

    fn entry_full(rel: &str, size: u64, mtime: i64, mt: Option<MediaType>) -> AssetEntry {
        AssetEntry {
            file: CrawledFile {
                abs_path: PathBuf::from(rel),
                rel_path: PathBuf::from(rel),
                size_bytes: size,
                mtime,
                media_type: mt,
            },
            source_idx: 0,
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
        sort_indices(&mut idxs, &entries, SortBy::Name, true);
        assert_eq!(
            idxs,
            vec![1, 0, 2],
            "Apple < banana < cherry, ignoring case"
        );

        sort_indices(&mut idxs, &entries, SortBy::Name, false);
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
        sort_indices(&mut idxs, &entries, SortBy::Size, true);
        assert_eq!(idxs, vec![1, 2, 0], "ascending by size_bytes");

        let mut idxs = vec![0, 1, 2];
        sort_indices(&mut idxs, &entries, SortBy::Modified, true);
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
        sort_indices(&mut idxs, &entries, SortBy::Type, true);
        assert_eq!(
            idxs.last(),
            Some(&0),
            "unclassified entries sink to the end"
        );
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
}
