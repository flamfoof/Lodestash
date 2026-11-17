//! Domain types the engine's read model, crawl and persist share.
//!
//! Extracted from the egui `app.rs` (B1). The egui shell's own view-models (screens, tree state,
//! scan/hydrate messages, dialogs) were deleted with it (v2 plan Task 2.2); what is left carries
//! no rendering concern.

use std::path::PathBuf;

use lodestash_core::catalog::{CrawlRecord, PackageEntryRecord};
use lodestash_core::types::{AssetStatus, MediaType};
use serde::{Deserialize, Serialize};

use crate::config::{SizeTier, SizeTiers};

/// The top-bar category tabs (UI-SPEC §0.3, prototype `catDefs`): four coarse buckets
/// grouping the 7-value `media_type` enum. The enum itself is unchanged (a data-model
/// invariant) — these are a presentation grouping.
///
/// The serde names **are** the tab ids the frontends use (`all`, `models`, …), so a query
/// arriving as JSON deserializes straight into this type rather than being re-matched against
/// string literals on the other side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    #[default]
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

    /// The frontend's tab id — must match the serde snake_case name above, which is what
    /// `GridQuery.tab` round-trips through. One literal, two callers, so a rename cannot drift.
    pub fn id(self) -> &'static str {
        match self {
            Category::All => "all",
            Category::Models => "models",
            Category::Materials => "materials",
            Category::Sounds => "sounds",
            Category::Images => "images",
        }
    }
}

/// `--ls-type-model` as CSS hex. Vector previews carry it on the SVG root so the stored artifact
/// is correct in a plain SVG renderer as well as under a stylesheet (DESIGN-GUIDELINES §2).
pub const MODEL_HEX: &str = "#A98BFF";

/// A child-folder tile in Folders view: its name, the count of assets **directly** inside it, and
/// the **recursive** total including everything in its subfolders (shown as `direct (recursive)`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FolderTile {
    pub name: String,
    pub direct: usize,
    pub recursive: usize,
}

/// A `.unitypackage` a crawl surfaced, with its manifest already read (on the crawl's own thread),
/// so the writer can record it as a browsable pack without opening the archive again.
pub struct PendingPackage {
    pub source_idx: usize,
    pub abs_path: PathBuf,
    pub display_name: String,
    /// Manifest locators (guid/pathname/tar-offset/size) captured at browse time —
    /// persisted as `package_entry` rows so surgical extraction never needs a second full scan
    /// (DATA-MODEL §7, INGESTION §2).
    pub entries: Vec<PackageEntryRecord>,
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

/// One source's worth of persist work handed to the catalog thread.
pub struct PersistBatch {
    pub root: String,
    pub name: String,
    /// `import_source.blacklist` JSON for the one-time config adoption (DATA-MODEL §13).
    pub excludes_json: Option<String>,
    pub records: Vec<CrawlRecord>,
}

/// Triangle-count bucket for the Triangles facet (models only). Design §3 "TRIANGLES".
///
/// The serde names are the **wire ids** the Svelte popover sends back — see the note on
/// [`ContentFilter`].
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
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
///
/// **This is also the wire type.** The Svelte popover posts one of these inside `GridQuery`
/// rather than three parallel arrays the desktop crate re-assembles: the filter and its
/// [`Self::passes`] predicate then have exactly one definition, shared by both frontends, and a
/// facet that means one thing in egui cannot come to mean another in the webview. `#[serde(default)]`
/// so a frontend that knows about fewer facets than this build still sends a valid filter.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
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

/// The `has_dependencies` / `has_dependents` chips (DATA-MODEL §10). Single-select: an asset that
/// is both ends of a chain (a material) satisfies either, so combining them would only ever
/// narrow to that intersection — not what the chips read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
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

/// STATUS facet single-select (UI-SPEC §0.2). Maps to `status` (DATA-MODEL §8);
/// `discovered|indexed` fold into Unidentified. Serde names are the facet ids the frontends
/// send, as with [`Category`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[cfg(test)]
mod facet_tests {
    use super::*;

    /// Every `media_type` reaches a tab. A type that falls through every arm is invisible in the
    /// grid unless the user is on All — which reads as a missing asset, not a missing tab.
    #[test]
    fn tab_buckets_cover_the_media_type_enum() {
        for mt in [
            MediaType::Model3d,
            MediaType::Material,
            MediaType::Audio,
            MediaType::Image,
            MediaType::Texture,
            MediaType::Sprite,
            MediaType::Font,
        ] {
            assert!(Category::All.matches(Some(mt)), "{mt:?} must pass All");
            let bucketed = Category::TABS
                .iter()
                .filter(|(c, _)| *c != Category::All)
                .any(|(c, _)| c.matches(Some(mt)));
            assert!(bucketed, "{mt:?} belongs to no tab");
        }
        assert!(Category::Models.matches(Some(MediaType::Model3d)));
        assert!(
            !Category::Models.matches(Some(MediaType::Material)),
            "materials are their own tab, not part of Models"
        );
        for mt in [
            MediaType::Image,
            MediaType::Texture,
            MediaType::Sprite,
            MediaType::Font,
        ] {
            assert!(Category::Images.matches(Some(mt)), "{mt:?} is an Image");
        }
        assert!(!Category::Sounds.matches(Some(MediaType::Image)));
        // Unclassified: only All takes it, so it stays browsable without polluting a bucket.
        assert!(Category::All.matches(None));
        assert!(!Category::Models.matches(None));
    }

    /// The lifecycle has four states but the UI names two: `discovered` and `indexed` are both
    /// "Unidentified" (DATA-MODEL §8). Getting this wrong hides the entire backlog the Identify
    /// screen exists to work through.
    #[test]
    fn unidentified_folds_discovered_and_indexed() {
        use AssetStatus::*;
        assert!(StatusFilter::Unidentified.matches(Discovered));
        assert!(StatusFilter::Unidentified.matches(Indexed));
        assert!(!StatusFilter::Unidentified.matches(Identified));
        assert!(!StatusFilter::Unidentified.matches(Confirmed));
        assert!(StatusFilter::Identified.matches(Identified));
        assert!(StatusFilter::Confirmed.matches(Confirmed));
        for s in [Discovered, Indexed, Identified, Confirmed] {
            assert!(StatusFilter::All.matches(s), "{s:?} must pass All");
        }
    }

    /// The serde names are a **wire contract**: the Svelte toolbar sends these ids and the query
    /// deserializes straight into these enums. Renaming a variant silently breaks the facet, so
    /// the ids are pinned here rather than left to the derive.
    #[test]
    fn facet_ids_are_the_strings_the_frontends_send() {
        for (c, expect) in [
            (Category::All, "\"all\""),
            (Category::Models, "\"models\""),
            (Category::Materials, "\"materials\""),
            (Category::Sounds, "\"sounds\""),
            (Category::Images, "\"images\""),
        ] {
            assert_eq!(serde_json::to_string(&c).unwrap(), expect);
            assert_eq!(serde_json::from_str::<Category>(expect).unwrap(), c);
        }
        for (s, expect) in [
            (StatusFilter::All, "\"all\""),
            (StatusFilter::Unidentified, "\"unidentified\""),
            (StatusFilter::Identified, "\"identified\""),
            (StatusFilter::Confirmed, "\"confirmed\""),
        ] {
            assert_eq!(serde_json::to_string(&s).unwrap(), expect);
            assert_eq!(serde_json::from_str::<StatusFilter>(expect).unwrap(), s);
        }
    }
}
