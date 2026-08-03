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
    pub rating: Option<u8>,
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
    /// Restrict to starred assets — what the Favorites screen is (FEATURES §4.7). A one-way
    /// switch: "not favourited" is not a thing anyone browses for.
    pub favorites_only: bool,
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
            favorites_only: false,
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
    pub rating: Option<u8>,
    pub has_preview: bool,
    pub triangles: Option<i64>,
    pub width_px: Option<i64>,
    pub height_px: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Which source root it came from.
    pub source: Option<String>,
    /// The albedo texture this model paints with, reached through the dependency graph
    /// (DATA-MODEL §10). `None` when the graph has no edge — the viewer then renders flat.
    pub texture_asset_id: Option<i64>,
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
    /// Assets whose bytes are still inside a `.unitypackage`. On a real Unity library this is
    /// the majority — 5,963 of 7,691 here — so a zero means a hydrate query is missing, not
    /// that the library has no packs.
    pub in_pack: usize,
    pub favorites: usize,
    /// Assets carrying a stored vector preview — the honest measure of how much of the grid
    /// can draw without touching a mesh.
    pub with_preview: usize,
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
                if e.trashed {
                    continue;
                }
                paths.insert(e.asset_id, root.join(&e.rel_path));
                rows.push(row_from(e, &info.name, None));
            }

            // In-pack rows are the bulk of a real library — on this machine 5,963 of 8,237 live
            // assets. Loading only `entries_for_source` showed ~21% of the catalog with nothing
            // saying so, which reads as a small library rather than a missing query.
            match cat.in_pack_entries_for_source(info.id) {
                Ok(v) => {
                    for e in v {
                        if e.asset.trashed {
                            continue;
                        }
                        // No local path: `lodestash://file` resolves nothing and answers 503 not_local,
                        // which is the correct read-only behaviour — extraction is a command.
                        let ext = Path::new(&e.asset.rel_path)
                            .extension()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let mut row = row_from(e.asset, &info.name, Some(e.package_name));
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
                        if e.asset.trashed {
                            continue;
                        }
                        if let Some(ws) = workspace_dir.as_ref() {
                            paths.insert(e.asset.asset_id, ws.join(&e.workspace_rel));
                        }
                        rows.push(row_from(e.asset, &info.name, Some(e.package_name)));
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
            });
        }

        rows.sort_by(|a, b| a.search_key.cmp(&b.search_key));
        self.rows = rows;
        self.paths = paths;
        self.sources = sources;
        self.mark_previews();
        self.mark_favorites();
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

    pub fn status(&self) -> LibraryStatus {
        LibraryStatus {
            open: self.catalog.is_some(),
            catalog_path: self
                .catalog_path
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned()),
            sources: self.sources.clone(),
            total_assets: self.rows.len(),
            in_pack: self.rows.iter().filter(|r| r.item.in_pack).count(),
            favorites: self.rows.iter().filter(|r| r.item.favorite).count(),
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
            .filter(|r| !q.favorites_only || r.item.favorite)
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
            texture_asset_id: self.model_texture(asset_id),
            in_pack: row.item.in_pack,
        })
    }

    pub fn thumb_cache_dir(&self) -> Option<&Path> {
        self.thumb_cache.as_deref()
    }

    pub fn asset_path(&self, asset_id: i64) -> Option<&Path> {
        self.paths.get(&asset_id).map(|p| p.as_path())
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

/// Build a grid row from a hydrated catalog entry, whatever storage class it came from.
///
/// `package` is `Some` for assets that live inside a pack — those have no local path, so the
/// row is browsable and searchable while `lodestash://file` correctly answers 503.
fn row_from(
    e: lodestash_core::catalog::HydratedAsset,
    source: &str,
    package: Option<String>,
) -> Row {
    let name = Path::new(&e.rel_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| e.rel_path.clone());
    Row {
        search_key: name.to_lowercase(),
        mtime: e.mtime,
        width_px: e.metrics.width_px,
        height_px: e.metrics.height_px,
        duration_ms: e.metrics.duration_ms,
        source: Some(source.to_string()),
        locator: None,
        item: GridItem {
            asset_id: e.asset_id,
            name,
            rel_path: e.rel_path,
            media_type: e.media_type,
            size_bytes: e.size_bytes,
            status: e.status,
            rating: e.rating,
            // Preview and favourite flags are filled in one pass each, not a query per row.
            has_preview: false,
            favorite: false,
            in_pack: package.is_some(),
            package,
            triangles: e.metrics.tri_count,
        },
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
                locator: None,
                item: GridItem {
                    asset_id: 0,
                    name: (*n).into(),
                    rel_path: format!("{n}.x"),
                    media_type: (*mt).into(),
                    size_bytes: 1,
                    status: "indexed".into(),
                    rating: None,
                    has_preview: false,
                    favorite: false,
                    in_pack: false,
                    package: None,
                    triangles: None,
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
            tab: "models".into(),
            ..Default::default()
        };
        assert_eq!(
            l.page(&q).total,
            1,
            "the audio favourite is excluded by the tab"
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
