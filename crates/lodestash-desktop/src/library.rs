//! Read-only view of the catalog for the desktop shell.
//!
//! This is deliberately **not** `Engine`. It opens its own connection and never writes:
//! the single-writer rule (ARCHITECTURE §5) says every catalog write goes through the persist
//! thread, and nothing here has one. That constraint is what lets the Svelte frontend show a
//! real library before the B1 extraction finishes — browsing is a read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lodestash_core::catalog::Catalog;
use serde::{Deserialize, Serialize};

/// One asset as the grid needs it. Carries a `preview_key`, never bytes — the frontend fetches
/// pixels over `ls://`, so a page of 100 cards is a few KB of JSON rather than a few MB.
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
    pub rating: Option<u8>,
    /// True when `asset_preview` holds a traced vector preview for this asset (M15).
    pub has_preview: bool,
    pub triangles: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GridQuery {
    /// Category tab id: `all | models | materials | sounds | images`.
    pub tab: String,
    /// Substring match over the name — the Tags-mode filter, before FTS is wired.
    pub search: String,
    /// STATUS facet (UI-SPEC §0.2): `all | unidentified | identified | confirmed`.
    /// `discovered` and `indexed` both fold into Unidentified (DATA-MODEL §8).
    pub status: String,
    /// `name | type | size | modified | rating` (config::SortBy, minus Relevance — that needs
    /// BM25 ranks, which arrive with FTS search).
    pub sort_by: String,
    pub sort_asc: bool,
    pub page: usize,
    pub page_size: usize,
}

impl Default for GridQuery {
    fn default() -> Self {
        Self {
            tab: "all".into(),
            search: String::new(),
            status: "all".into(),
            sort_by: "name".into(),
            sort_asc: true,
            page: 0,
            page_size: 100,
        }
    }
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
    pub rating: Option<u8>,
    pub has_preview: bool,
    pub triangles: Option<i64>,
    pub width_px: Option<i64>,
    pub height_px: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Which source root it came from.
    pub source: Option<String>,
    /// True when the file is not on disk — an in-pack asset whose bytes are still in the archive.
    pub in_pack: bool,
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
}

#[derive(Debug, Serialize)]
pub struct LibraryStatus {
    pub open: bool,
    pub catalog_path: Option<String>,
    pub sources: Vec<SourceSummary>,
    pub total_assets: usize,
    /// Assets carrying a stored vector preview — the honest measure of how much of the grid
    /// can draw without touching a mesh.
    pub with_preview: usize,
}

/// Everything the shell holds about the open catalog.
pub struct Library {
    catalog: Option<Catalog>,
    catalog_path: Option<PathBuf>,
    sources: Vec<SourceSummary>,
    /// All live rows, name-sorted once at open. The whole library is ~10k rows of small structs,
    /// so holding it beats a query per page — and it is what the egui build already does.
    /// ponytail: re-reads only on `reload`; a catalog written by the egui app while this is open
    /// will not show until then. Fine while this is a read-only viewer.
    rows: Vec<Row>,
    /// asset_id -> absolute path on disk, for `ls://file`.
    paths: HashMap<i64, PathBuf>,
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
                    catalog_path: Some(path),
                    sources: Vec::new(),
                    rows: Vec::new(),
                    paths: HashMap::new(),
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
            catalog_path: None,
            sources: Vec::new(),
            rows: Vec::new(),
            paths: HashMap::new(),
        }
    }

    /// Re-read sources and their assets from the catalog.
    pub fn reload(&mut self) {
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
                if e.trashed {
                    continue;
                }
                let name = Path::new(&e.rel_path)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| e.rel_path.clone());
                paths.insert(e.asset_id, root.join(&e.rel_path));
                rows.push(Row {
                    search_key: name.to_lowercase(),
                    mtime: e.mtime,
                    width_px: e.metrics.width_px,
                    height_px: e.metrics.height_px,
                    duration_ms: e.metrics.duration_ms,
                    source: Some(info.name.clone()),
                    item: GridItem {
                        asset_id: e.asset_id,
                        name,
                        rel_path: e.rel_path,
                        media_type: e.media_type,
                        size_bytes: e.size_bytes,
                        status: e.status,
                        rating: e.rating,
                        // Filled in below in one pass rather than a query per row.
                        has_preview: false,
                        triangles: e.metrics.tri_count,
                    },
                });
            }
            sources.push(SourceSummary {
                id: info.id,
                name: info.name.clone(),
                root_path: info.root_path.clone(),
                asset_count: info.asset_count,
            });
        }

        rows.sort_by(|a, b| a.search_key.cmp(&b.search_key));
        self.rows = rows;
        self.paths = paths;
        self.sources = sources;
        self.mark_previews();
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

    pub fn status(&self) -> LibraryStatus {
        LibraryStatus {
            open: self.catalog.is_some(),
            catalog_path: self
                .catalog_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            sources: self.sources.clone(),
            total_assets: self.rows.len(),
            with_preview: self.rows.iter().filter(|r| r.item.has_preview).count(),
        }
    }

    /// Filter, sort, page. Pure over the held rows, so it is testable without a catalog.
    pub fn page(&self, q: &GridQuery) -> GridPage {
        let needle = q.search.trim().to_lowercase();
        let mut matched: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| tab_matches(&q.tab, &r.item.media_type))
            .filter(|r| status_matches(&q.status, &r.item.status))
            .filter(|r| needle.is_empty() || r.search_key.contains(&needle))
            .collect();

        // Every key falls back to name for ties, which keeps paging stable when a key has many
        // equal values (thousands of assets share a rating, or have none). Name is sorted
        // explicitly rather than leaning on load order: relying on `reload` having sorted
        // `rows` makes the result depend on how the Library was built, not on the query.
        match q.sort_by.as_str() {
            "name" => matched.sort_by(|a, b| a.search_key.cmp(&b.search_key)),
            "size" => matched.sort_by(|a, b| {
                a.item
                    .size_bytes
                    .cmp(&b.item.size_bytes)
                    .then_with(|| a.search_key.cmp(&b.search_key))
            }),
            "modified" => matched.sort_by(|a, b| {
                a.mtime
                    .cmp(&b.mtime)
                    .then_with(|| a.search_key.cmp(&b.search_key))
            }),
            "rating" => matched.sort_by(|a, b| {
                a.item
                    .rating
                    .unwrap_or(0)
                    .cmp(&b.item.rating.unwrap_or(0))
                    .then_with(|| a.search_key.cmp(&b.search_key))
            }),
            "type" => matched.sort_by(|a, b| {
                a.item
                    .media_type
                    .cmp(&b.item.media_type)
                    .then_with(|| a.search_key.cmp(&b.search_key))
            }),
            _ => {}
        }
        if !q.sort_asc {
            matched.reverse();
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
            rating: row.item.rating,
            has_preview: row.item.has_preview,
            triangles: row.item.triangles,
            width_px: row.width_px,
            height_px: row.height_px,
            duration_ms: row.duration_ms,
            source: row.source.clone(),
            // The bytes live inside a `.unitypackage` until it is materialized; `ls://file`
            // answers 503 for these rather than extracting, so the UI must be able to say so.
            in_pack: abs.map_or(true, |p| !p.exists()),
        })
    }

    pub fn asset_path(&self, asset_id: i64) -> Option<&Path> {
        self.paths.get(&asset_id).map(|p| p.as_path())
    }

    pub fn preview_svg(&self, asset_id: i64) -> Option<String> {
        let cat = self.catalog.as_ref()?;
        cat.vector_preview(asset_id)
            .ok()
            .flatten()
            .map(|(svg, _)| svg)
    }
}

/// STATUS facet matching. `discovered` and `indexed` are both "Unidentified" to the user —
/// the lifecycle has four states but the UI names two (DATA-MODEL §8).
fn status_matches(filter: &str, status: &str) -> bool {
    match filter {
        "unidentified" => matches!(status, "discovered" | "indexed"),
        "identified" => status == "identified",
        "confirmed" => status == "confirmed",
        _ => true,
    }
}

/// Tab -> `media_type` mapping (FEATURES §0.2). A presentation grouping over the 7-value enum,
/// which is unchanged.
fn tab_matches(tab: &str, media_type: &str) -> bool {
    match tab {
        "models" => media_type == "model_3d",
        "materials" => media_type == "material",
        "sounds" => media_type == "audio",
        "images" => matches!(media_type, "image" | "texture" | "sprite" | "font"),
        // "all", and anything unrecognised, imposes no constraint.
        _ => true,
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
                mtime: 0,
                width_px: None,
                height_px: None,
                duration_ms: None,
                source: None,
                item: GridItem {
                    asset_id: 0,
                    name: (*n).into(),
                    rel_path: format!("{n}.x"),
                    media_type: (*mt).into(),
                    size_bytes: 1,
                    status: "indexed".into(),
                    rating: None,
                    has_preview: false,
                    triangles: None,
                },
            })
            .collect();
        Library {
            catalog: None,
            catalog_path: None,
            sources: Vec::new(),
            rows,
            paths: HashMap::new(),
        }
    }

    #[test]
    fn tab_buckets_cover_the_media_type_enum() {
        for mt in [
            "model_3d", "material", "audio", "image", "texture", "sprite", "font",
        ] {
            assert!(tab_matches("all", mt), "{mt} must pass All");
        }
        assert!(tab_matches("models", "model_3d"));
        assert!(
            !tab_matches("models", "material"),
            "materials are their own tab"
        );
        for mt in ["image", "texture", "sprite", "font"] {
            assert!(tab_matches("images", mt), "{mt} belongs to Images");
        }
        assert!(!tab_matches("sounds", "image"));
    }

    /// Descending must reverse the *whole* comparison, ties included. Reversing a stably-sorted
    /// list is what makes that true here — a naive `if !asc { cmp.reverse() }` inside the
    /// comparator leaves tie-breaks ascending, so page 2 of a descending sort can repeat or
    /// drop rows that page 1 already showed.
    #[test]
    fn sorting_is_total_and_descending_is_the_exact_reverse() {
        let l = lib(&[("b", "model_3d"), ("a", "model_3d"), ("c", "model_3d")]);
        let q = |asc| GridQuery {
            sort_by: "name".into(),
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

    #[test]
    fn size_sort_breaks_ties_by_name_so_paging_is_stable() {
        let mut l = lib(&[("z", "model_3d"), ("a", "model_3d"), ("m", "model_3d")]);
        for r in &mut l.rows {
            r.item.size_bytes = 100; // every row identical on the sort key
        }
        let names: Vec<String> = l
            .page(&GridQuery {
                sort_by: "size".into(),
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

    /// The lifecycle has four states but the UI names two: `discovered` and `indexed` are both
    /// "Unidentified" (DATA-MODEL §8). Getting this wrong hides the entire backlog the Identify
    /// screen exists to work through.
    #[test]
    fn unidentified_folds_discovered_and_indexed() {
        assert!(status_matches("unidentified", "discovered"));
        assert!(status_matches("unidentified", "indexed"));
        assert!(!status_matches("unidentified", "identified"));
        assert!(!status_matches("unidentified", "confirmed"));
        assert!(status_matches("identified", "identified"));
        assert!(status_matches("confirmed", "confirmed"));
        for s in ["discovered", "indexed", "identified", "confirmed"] {
            assert!(status_matches("all", s), "{s} must pass All");
        }
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
