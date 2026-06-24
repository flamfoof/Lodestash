//! Main application state and the eframe::App impl.
//! Spec: DESIGN-GUIDELINES.md §4, §7, §9; ARCHITECTURE.md §6.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{RichText, ScrollArea, TextureHandle, Ui, Vec2};
use lodestash_core::types::MediaType;
use lodestash_ingest::crawler::{self, CrawledFile};

use crate::config::{AppConfig, SortBy};
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
    file:          CrawledFile,
    texture:       Option<TextureHandle>,
    thumb_spawned: bool,
}

struct ThumbReady {
    index:   usize,
    texture: TextureHandle,
}

// ── Constants ─────────────────────────────────────────────────────────────────

const THUMB_GEN_SIZE: u32  = 256;
const CARD_LABEL:     f32  = 36.0;
const CARD_PAD:       f32  = 8.0;
pub const THUMB_MIN:  f32  = 64.0;
pub const THUMB_MAX:  f32  = 256.0;
/// Extra rows above/below the visible viewport to prefetch thumbnails for.
const LOOKAHEAD_ROWS: usize = 4;

// ── App ───────────────────────────────────────────────────────────────────────

pub struct LodestashApp {
    config:         AppConfig,
    /// The source directories currently loaded (may be multiple for a multi-source project).
    scan_roots:     Vec<PathBuf>,
    /// Name of the open LibraryProject (if any).
    project_name:   Option<String>,
    search:         String,
    active_tab:     Category,
    sort_by:        SortBy,
    sort_asc:       bool,
    thumb_size:     f32,
    entries:        Vec<AssetEntry>,
    /// Sorted+filtered indices into `entries`. Rebuilt only when `cache_dirty`.
    sorted_cache:   Vec<usize>,
    /// Per-tab entry counts, parallel to Category::TABS. Rebuilt on folder load.
    tab_counts:     Vec<usize>,
    cache_dirty:    bool,
    thumb_rx:       Receiver<ThumbReady>,
    thumb_tx:       Sender<ThumbReady>,
    pending_thumbs: usize,
}

impl LodestashApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::apply(&cc.egui_ctx);

        let (thumb_tx, thumb_rx) = mpsc::channel();
        let config     = AppConfig::load();
        let sort_by    = config.view.sort_by;
        let sort_asc   = config.view.sort_asc;
        let thumb_size = config.view.thumb_size.clamp(THUMB_MIN, THUMB_MAX);

        let mut app = Self {
            scan_roots:     Vec::new(),
            project_name:   None,
            search:         String::new(),
            active_tab:     Category::All,
            sort_by,
            sort_asc,
            thumb_size,
            entries:        Vec::new(),
            sorted_cache:   Vec::new(),
            tab_counts:     vec![0; Category::TABS.len()],
            cache_dirty:    false,
            thumb_rx,
            thumb_tx,
            pending_thumbs: 0,
            config,
        };

        // Restore the last open project, or fall back to last_root for old configs.
        if let Some(name) = app.config.last_project_name.clone() {
            if let Some(proj) = app.config.project_by_name(&name) {
                let paths: Vec<PathBuf> = proj.paths.clone();
                let live: Vec<PathBuf>  = paths.into_iter().filter(|p| p.is_dir()).collect();
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
        let Some(proj_name) = self.project_name.clone() else { return; };
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            self.config.add_source_to_project(&proj_name, path);
            // Reload all paths for this project from config (now includes the new one).
            let paths: Vec<PathBuf> = self.config
                .project_by_name(&proj_name)
                .map(|p| p.paths.clone())
                .unwrap_or_default();
            let live: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_dir()).collect();
            self.load_paths(ctx, live, Some(proj_name));
        }
    }

    fn open_project(&mut self, ctx: &egui::Context, name: &str) {
        let paths: Vec<PathBuf> = self.config
            .project_by_name(name)
            .map(|p| p.paths.clone())
            .unwrap_or_default();
        let live: Vec<PathBuf> = paths.into_iter().filter(|p| p.is_dir()).collect();
        if live.is_empty() { return; }
        self.config.record_project_open(name);
        self.load_paths(ctx, live, Some(name.to_owned()));
    }

    /// Core loader — crawls all `roots` and populates `entries`.
    fn load_paths(&mut self, ctx: &egui::Context, roots: Vec<PathBuf>, name: Option<String>) {
        // Drop old channel to cancel in-flight thumbnail tasks.
        let (thumb_tx, thumb_rx) = mpsc::channel();
        self.thumb_tx       = thumb_tx;
        self.thumb_rx       = thumb_rx;
        self.pending_thumbs = 0;
        self.search.clear();
        self.active_tab = Category::All;

        self.entries = roots.iter()
            .flat_map(|root| crawler::crawl(root))
            .map(|f| AssetEntry { file: f, texture: None, thumb_spawned: false })
            .collect();

        self.tab_counts = Category::TABS
            .iter()
            .map(|&(cat, _)| self.entries.iter().filter(|e| cat.matches(e.file.media_type)).count())
            .collect();

        self.cache_dirty  = true;
        self.scan_roots   = roots;
        self.project_name = name;

        ctx.request_repaint();
    }

    // ── Per-frame helpers ─────────────────────────────────────────────────────

    fn receive_thumbnails(&mut self) {
        while let Ok(msg) = self.thumb_rx.try_recv() {
            if let Some(entry) = self.entries.get_mut(msg.index) {
                entry.texture = Some(msg.texture);
                self.pending_thumbs = self.pending_thumbs.saturating_sub(1);
            }
        }
    }

    /// Rebuild the sorted+filtered index cache. Called only when `cache_dirty`.
    fn rebuild_cache(&mut self) {
        let q   = self.search.to_lowercase();
        let tab = self.active_tab;

        let mut idxs: Vec<usize> = self.entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                tab.matches(e.file.media_type)
                    && (q.is_empty()
                        || e.file.rel_path.to_string_lossy().to_lowercase().contains(&q))
            })
            .map(|(i, _)| i)
            .collect();

        let asc     = self.sort_asc;
        let entries = &self.entries;
        match self.sort_by {
            SortBy::Name => {
                let keys: Vec<String> = idxs.iter().map(|&i| name_key(&entries[i])).collect();
                let mut paired: Vec<(usize, usize)> = idxs.iter().copied().zip(0..).collect();
                paired.sort_by(|&(_, ka), &(_, kb)| {
                    let o = keys[ka].cmp(&keys[kb]);
                    if asc { o } else { o.reverse() }
                });
                idxs = paired.into_iter().map(|(i, _)| i).collect();
            }
            SortBy::Type => idxs.sort_by(|&a, &b| {
                let o = type_key(&entries[a]).cmp(&type_key(&entries[b]));
                if asc { o } else { o.reverse() }
            }),
            SortBy::Size => idxs.sort_by(|&a, &b| {
                let o = entries[a].file.size_bytes.cmp(&entries[b].file.size_bytes);
                if asc { o } else { o.reverse() }
            }),
            SortBy::Modified => idxs.sort_by(|&a, &b| {
                let o = entries[a].file.mtime.cmp(&entries[b].file.mtime);
                if asc { o } else { o.reverse() }
            }),
        }

        self.sorted_cache = idxs;
        self.cache_dirty  = false;
    }

    fn save_view_prefs(&mut self) {
        self.config.view.sort_by    = self.sort_by;
        self.config.view.sort_asc   = self.sort_asc;
        self.config.view.thumb_size = self.thumb_size;
        self.config.save();
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
            let rel_path = self.entries[idx].file.rel_path.to_string_lossy().into_owned();
            let tx   = self.thumb_tx.clone();
            let ctx2 = ctx.clone();

            rayon::spawn(move || {
                if let Some(tex) = generate_texture(&ctx2, &abs_path, &rel_path) {
                    let _ = tx.send(ThumbReady { index: idx, texture: tex });
                    ctx2.request_repaint();
                }
            });
        }
    }
}

fn name_key(e: &AssetEntry) -> String {
    e.file.rel_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn type_key(e: &AssetEntry) -> u8 {
    e.file.media_type.map(|m| m as u8).unwrap_or(255)
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for LodestashApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_thumbnails();

        if self.cache_dirty {
            self.rebuild_cache();
        }

        // ── Top bar ──────────────────────────────────────────────────────────
        let mut add_source_clicked = false;
        egui::TopBottomPanel::top("topbar")
            .frame(
                egui::Frame::none()
                    .fill(theme::SURFACE_RAISED)
                    .inner_margin(egui::Margin { left: 12.0, right: 12.0, top: 8.0, bottom: 8.0 }),
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
                    if ui.button(RichText::new("Open folder\u{2026}").color(theme::TEXT)).clicked() {
                        self.open_folder(ctx);
                    }
                    // "Add source" only makes sense when a project is already open.
                    if self.project_name.is_some() {
                        if ui.button(RichText::new("+ Add source").color(theme::ARCANE)).clicked() {
                            add_source_clicked = true;
                        }
                    }

                    // Project / source info.
                    if let Some(name) = &self.project_name {
                        let src_count = self.scan_roots.len();
                        let detail = if src_count == 1 {
                            self.scan_roots[0].to_string_lossy().into_owned()
                        } else {
                            format!("{src_count} sources")
                        };
                        ui.label(
                            RichText::new(format!("{name}  \u{2014}  {detail}"))
                                .color(theme::TEXT_DIM)
                                .size(11.0),
                        );
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

        if add_source_clicked {
            self.add_source(ctx);
        }

        // ── Category tab bar + view toolbar ──────────────────────────────────
        if !self.entries.is_empty() {
            egui::TopBottomPanel::top("tabbar")
                .frame(
                    egui::Frame::none()
                        .fill(theme::SURFACE)
                        .inner_margin(egui::Margin { left: 12.0, right: 12.0, top: 4.0, bottom: 0.0 })
                        .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (i, &(cat, label)) in Category::TABS.iter().enumerate() {
                            let n = self.tab_counts[i];
                            if n == 0 && cat != Category::All { continue; }
                            let is_active = self.active_tab == cat;
                            let tab_color = cat.color();
                            let rich = RichText::new(format!("{label}  {n}"))
                                .size(12.0)
                                .color(if is_active { tab_color } else { theme::TEXT_DIM });
                            let resp = ui.add(
                                egui::Button::new(rich)
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::NONE)
                                    .frame(false),
                            );
                            if resp.clicked() {
                                self.active_tab  = cat;
                                self.cache_dirty = true;
                            }
                            if is_active {
                                let rect = resp.rect;
                                ui.painter().line_segment(
                                    [egui::pos2(rect.min.x, rect.max.y),
                                     egui::pos2(rect.max.x, rect.max.y)],
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
                        .inner_margin(egui::Margin { left: 12.0, right: 12.0, top: 5.0, bottom: 5.0 })
                        .stroke(egui::Stroke::new(1.0, theme::BORDER)),
                )
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        let search_resp = ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("Search\u{2026}")
                                .desired_width(180.0),
                        );
                        if search_resp.changed() {
                            self.cache_dirty = true;
                        }

                        ui.separator();
                        ui.label(RichText::new("Sort").color(theme::TEXT_DIM).size(11.0));

                        let sort_before = self.sort_by;
                        egui::ComboBox::from_id_salt("sort_combo")
                            .width(72.0)
                            .selected_text(
                                RichText::new(self.sort_by.label()).color(theme::TEXT).size(11.0),
                            )
                            .show_ui(ui, |ui| {
                                for &(s, label) in SortBy::OPTIONS {
                                    ui.selectable_value(&mut self.sort_by, s, label);
                                }
                            });

                        let asc_before = self.sort_asc;
                        let dir_label  = if self.sort_asc { "\u{2191}" } else { "\u{2193}" };
                        if ui.small_button(dir_label).clicked() {
                            self.sort_asc = !self.sort_asc;
                        }

                        if self.sort_by != sort_before || self.sort_asc != asc_before {
                            self.cache_dirty = true;
                            self.save_view_prefs();
                        }

                        ui.separator();
                        ui.label(RichText::new("Size").color(theme::TEXT_DIM).size(11.0));
                        let size_resp = ui.add(
                            egui::Slider::new(&mut self.thumb_size, THUMB_MIN..=THUMB_MAX)
                                .show_value(false)
                                .step_by(8.0),
                        );
                        if size_resp.drag_stopped() {
                            self.save_view_prefs();
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

        // ── Main content ─────────────────────────────────────────────────────
        let mut open_project: Option<String> = None;
        let mut open_single:  Option<PathBuf> = None;
        let mut spawn_queue:  Vec<usize>      = Vec::new();

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::same(12.0)),
            )
            .show(ctx, |ui| {
                if self.entries.is_empty() {
                    empty_state(ui, &self.config, &mut open_project, &mut open_single);
                } else if self.sorted_cache.is_empty() {
                    no_results_state(ui, &self.search);
                } else {
                    spawn_queue = thumbnail_grid(
                        ui, &self.entries, &self.sorted_cache, self.thumb_size,
                    );
                }
            });

        if let Some(name) = open_project {
            self.open_project(ctx, &name.clone());
        } else if let Some(path) = open_single {
            let proj_name = self.config.record_open(path.clone());
            self.load_paths(ctx, vec![path], Some(proj_name));
        }
        if !spawn_queue.is_empty() {
            self.spawn_thumbnails(ctx, &spawn_queue);
        }
    }
}

// ── Empty / no-results states ─────────────────────────────────────────────────

fn empty_state(
    ui:           &mut Ui,
    config:       &AppConfig,
    open_project: &mut Option<String>,
    open_single:  &mut Option<PathBuf>,
) {
    let avail = ui.available_size();
    ui.add_space(avail.y * 0.18);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("\u{25C8}").size(48.0).color(theme::GOLD.linear_multiply(0.4)));
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
            ui.label(RichText::new("Saved Libraries").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(8.0);
            for proj in &config.projects {
                let live = proj.live_paths();
                if live.is_empty() { continue; }
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
                ui.label(
                    RichText::new(src_label).color(theme::TEXT_DIM).size(10.0),
                );
                ui.add_space(4.0);
            }
        } else if !config.recent_roots.is_empty() {
            ui.add_space(24.0);
            ui.label(RichText::new("Recent").color(theme::TEXT_DIM).size(11.0));
            ui.add_space(4.0);
            for path in config.recent_roots.iter().take(5) {
                if !path.is_dir() { continue; }
                let label = path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                if ui
                    .add(egui::Button::new(RichText::new(label).color(theme::TEXT).size(12.0))
                        .fill(egui::Color32::TRANSPARENT))
                    .clicked()
                {
                    *open_single = Some(path.clone());
                }
            }
        }
    });
}

fn no_results_state(ui: &mut Ui, query: &str) {
    let avail = ui.available_size();
    ui.add_space(avail.y * 0.3);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("No assets match").color(theme::TEXT_MUTED));
        if !query.is_empty() {
            ui.label(
                RichText::new(format!("\"{query}\"")).color(theme::TEXT_DIM).size(11.0),
            );
        }
    });
}

// ── Thumbnail grid ────────────────────────────────────────────────────────────

/// Render visible rows via `show_rows` (only O(visible) work per frame).
/// Returns entry indices in the visible+lookahead window that need thumbnails spawned.
fn thumbnail_grid(
    ui:         &mut Ui,
    entries:    &[AssetEntry],
    indices:    &[usize],
    thumb_size: f32,
) -> Vec<usize> {
    let card_h     = thumb_size + CARD_LABEL;
    let available  = ui.available_width() - ui.spacing().scroll.bar_width - 2.0;
    let cols       = ((available + CARD_PAD) / (thumb_size + CARD_PAD)).max(1.0) as usize;
    let total_rows = indices.len().div_ceil(cols);

    let mut to_spawn: Vec<usize> = Vec::new();

    ScrollArea::vertical()
        .auto_shrink([false; 2])
        .show_rows(ui, card_h, total_rows, |ui, row_range| {
            // Expand range with lookahead for demand thumbnail prefetching.
            let buf_start = row_range.start.saturating_sub(LOOKAHEAD_ROWS);
            let buf_end   = (row_range.end + LOOKAHEAD_ROWS).min(total_rows);
            for row in buf_start..buf_end {
                let start = row * cols;
                let end   = (start + cols).min(indices.len());
                for &idx in &indices[start..end] {
                    if !entries[idx].thumb_spawned && needs_thumbnail(entries[idx].file.media_type) {
                        to_spawn.push(idx);
                    }
                }
            }

            // Render only the truly visible rows.
            for row_idx in row_range {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CARD_PAD;
                    let start = row_idx * cols;
                    let end   = (start + cols).min(indices.len());
                    for &idx in &indices[start..end] {
                        asset_card(ui, &entries[idx], thumb_size);
                    }
                });
            }
        });

    to_spawn
}

fn asset_card(ui: &mut Ui, entry: &AssetEntry, thumb_size: f32) {
    let type_color = entry.file.media_type
        .map(theme::media_type_color)
        .unwrap_or(theme::TEXT_DIM);

    egui::Frame::none()
        .fill(theme::SURFACE)
        .rounding(6.0)
        .stroke(egui::Stroke::new(1.0, theme::BORDER))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_max_size(Vec2::new(thumb_size, thumb_size + CARD_LABEL));

                if let Some(tex) = &entry.texture {
                    ui.add(
                        egui::Image::new((tex.id(), Vec2::splat(thumb_size)))
                            .fit_to_exact_size(Vec2::splat(thumb_size)),
                    );
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
                }

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let (dot_resp, painter) =
                        ui.allocate_painter(Vec2::splat(8.0), egui::Sense::hover());
                    painter.circle_filled(dot_resp.rect.center(), 4.0, type_color);

                    let name = entry.file.rel_path.file_stem()
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
}

fn type_label_char(mt: Option<MediaType>) -> &'static str {
    match mt {
        Some(MediaType::Model3d)  => "3D",
        Some(MediaType::Texture)  => "TX",
        Some(MediaType::Audio)    => "\u{266A}",
        Some(MediaType::Image)    => "IMG",
        Some(MediaType::Sprite)   => "SP",
        Some(MediaType::Material) => "MAT",
        Some(MediaType::Font)     => "A",
        None                      => "?",
    }
}

// ── Background thumbnail helpers ──────────────────────────────────────────────

fn needs_thumbnail(mt: Option<MediaType>) -> bool {
    matches!(mt, Some(MediaType::Image | MediaType::Texture | MediaType::Sprite))
}

fn generate_texture(
    ctx:      &egui::Context,
    abs_path: &std::path::Path,
    tex_name: &str,
) -> Option<TextureHandle> {
    let img = image::open(abs_path).ok()?;
    let img = img.thumbnail(THUMB_GEN_SIZE, THUMB_GEN_SIZE).to_rgba8();
    let (w, h) = img.dimensions();
    let color_img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &img);
    Some(ctx.load_texture(tex_name, color_img, egui::TextureOptions::LINEAR))
}
