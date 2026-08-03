//! Read-only view of the catalog for the desktop shell.
//!
//! This is deliberately **not** `Engine`. It opens its own connection and never writes:
//! the single-writer rule (ARCHITECTURE §5) says every catalog write goes through the persist
//! thread, and nothing here has one. That constraint is what lets the Svelte frontend show a
//! real library before the B1 extraction finishes — browsing is a read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lodestash_app::config::SortBy;
use lodestash_app::grid;
use lodestash_app::types::{Category, StatusFilter};
use lodestash_core::catalog::Catalog;
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
    /// `discovered` and `indexed` both fold into Unidentified (DATA-MODEL §8).
    pub status: StatusFilter,
    pub sort_by: SortBy,
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
            tab: Category::All,
            search: String::new(),
            status: StatusFilter::All,
            sort_by: SortBy::Name,
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
    /// The catalog strings parsed once at hydrate, so filtering and sorting run over the shared
    /// enums instead of re-matching a string per row per keystroke. `media_type` is `None` for a
    /// code this build does not know (schema drift), which sorts last and matches no tab but All.
    media_type: Option<MediaType>,
    status: AssetStatus,
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

    /// Filter, sort, page. Runs the FTS query for the active search, then hands the ranks to
    /// [`Self::page_with_ranks`], which is where the actual work — and the testability — lives.
    pub fn page(&self, q: &GridQuery) -> GridPage {
        let needle = q.search.trim().to_lowercase();
        let ranks = self.search_ranks(&needle);
        self.page_with_ranks(q, ranks.as_ref())
    }

    /// Filter, sort, page against a **given** set of BM25 ranks. Pure over the held rows, so the
    /// union rule and the relevance order can be tested without standing up an FTS index.
    fn page_with_ranks(&self, q: &GridQuery, ranks: Option<&HashMap<i64, usize>>) -> GridPage {
        let needle = q.search.trim().to_lowercase();
        let hit = |r: &Row| {
            // Union, exactly as the egui grid does it (`app.rs` `rebuild_cache`): FTS5 reaches
            // tags and descriptions and ranks by BM25, but it matches whole tokens by prefix, so
            // a mid-word query ("olygon") would stop matching things that matched yesterday.
            // Keeping the substring pass means the search only ever gains results.
            ranks
                .as_ref()
                .is_some_and(|m| m.contains_key(&r.item.asset_id))
                || r.search_key.contains(&needle)
        };
        let mut matched: Vec<&Row> = self
            .rows
            .iter()
            .filter(|r| q.tab.matches(r.media_type))
            .filter(|r| q.status.matches(r.status))
            .filter(|r| !q.favorites_only || r.item.favorite)
            .filter(|r| needle.is_empty() || hit(r))
            .collect();

        // Every key falls back to name for ties, which keeps paging stable when a key has many
        // equal values (thousands of assets share a rating, or have none). Name is sorted
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
            SortBy::Name => matched.sort_by(|a, b| by(std::cmp::Ordering::Equal, a, b)),
            // BM25 order, hits before non-hits. With no query (or no catalog) every row is
            // unranked and this degrades to name order, which is why the toolbar can leave
            // Relevance selected while the user clears the search box.
            SortBy::Relevance => matched.sort_by(|a, b| {
                let k = |r: &Row| {
                    grid::relevance_key(ranks.as_ref().and_then(|m| m.get(&r.item.asset_id).copied()))
                };
                by(k(a).cmp(&k(b)), a, b)
            }),
            SortBy::Size => {
                matched.sort_by(|a, b| by(a.item.size_bytes.cmp(&b.item.size_bytes), a, b))
            }
            SortBy::Modified => matched.sort_by(|a, b| by(a.mtime.cmp(&b.mtime), a, b)),
            SortBy::Rating => matched.sort_by(|a, b| {
                by(
                    a.item.rating.unwrap_or(0).cmp(&b.item.rating.unwrap_or(0)),
                    a,
                    b,
                )
            }),
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
            texture_asset_id: self.albedo_texture(asset_id),
            in_pack: row.item.in_pack,
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

    pub fn thumb_cache_dir(&self) -> Option<&Path> {
        self.thumb_cache.as_deref()
    }

    pub fn asset_path(&self, asset_id: i64) -> Option<&Path> {
        self.paths.get(&asset_id).map(|p| p.as_path())
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

    /// Rows that compare **fully** equal — same name, same key — still need a deterministic
    /// order, or descending is not the reverse of ascending and a page boundary can show the
    /// same asset twice. A stable sort preserves load order in both directions, so the
    /// comparator ends in `asset_id`. Caught in e2e first: this library has duplicate names.
    #[test]
    fn rows_that_tie_on_every_key_still_reverse_cleanly() {
        let mut l = lib(&[("dup", "model_3d"), ("dup", "model_3d"), ("dup", "model_3d")]);
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
        let row = row_from(e, "Synty", None);
        assert_eq!(row.media_type, Some(MediaType::Model3d));
        assert_eq!(row.status, AssetStatus::Identified);
        assert_eq!(row.item.media_type, "model_3d", "the JSON keeps the code");
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
