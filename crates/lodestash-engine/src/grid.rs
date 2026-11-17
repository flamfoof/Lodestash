//! Grid sort and folder math (pure functions), split out of the egui `app.rs` (B1). The read
//! model sorts and folds with these; the egui card-layout helpers were deleted with that shell
//! (v2 plan Task 2.2).
use lodestash_core::types::MediaType;

use crate::types::*;

/// Sort-by-Type ordering: **enum declaration order**, unclassified last.
///
/// Not alphabetical on the code string — that would put `audio` before `model_3d` and give the
/// two frontends different grids for the same query. This is the one home for that rule.
pub fn media_key(mt: Option<MediaType>) -> u8 {
    mt.map(|m| m as u8).unwrap_or(255)
}

/// Apply the sort direction.
pub fn dir(o: std::cmp::Ordering, asc: bool) -> std::cmp::Ordering {
    if asc {
        o
    } else {
        o.reverse()
    }
}

/// Relevance ordering key: **ranked hits first, in rank order; unranked after.**
///
/// `Option`'s own ordering is the wrong way round for this — `None < Some` would put every
/// non-matching asset *above* the best BM25 hit — so relevance never sorts on the bare
/// `Option<usize>`. Callers break ties on name.
pub fn relevance_key(rank: Option<usize>) -> (u8, usize) {
    match rank {
        Some(r) => (0, r),
        None => (1, 0),
    }
}

/// One level of the Folders view, over **already-scoped** paths: which of them sit directly in
/// `dir`, and one tile per child directory with its direct and recursive counts.
///
/// Takes `(caller's index, path)` pairs rather than a slice of anything, because the two frontends
/// hold different row types and this rule — what counts as "in this folder" versus "under it" — is
/// the part that must not diverge between them. Scoping (which source, which pack, trashed,
/// search) belongs to the caller; only the tab filter is applied here, because tile counts have to
/// agree with the grid the tab produces. Tiles come back name-sorted (BTreeMap).
pub fn folder_level<'a, I>(
    items: I,
    dir: &std::path::Path,
    tab: Category,
) -> (Vec<usize>, Vec<FolderTile>)
where
    I: IntoIterator<Item = (usize, &'a std::path::Path, Option<MediaType>)>,
{
    // name → (direct children in that folder, total recursive under it)
    let mut tiles: std::collections::BTreeMap<String, (usize, usize)> =
        std::collections::BTreeMap::new();
    let mut idxs = Vec::new();
    for (i, rel_path, media_type) in items {
        if !tab.matches(media_type) {
            continue;
        }
        let Ok(rest) = rel_path.strip_prefix(dir) else {
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

/// The pack-browser contents list's third column: duration for audio, dimensions for an image, `—`
/// otherwise or when the metric hasn't been extracted yet (PACK-DIRECTORY-NAV §6). Pure/testable;
/// the widget renders folders (from `folder_tiles`) then files (from `sorted_cache`) using this.
pub fn content_third_col(
    mt: Option<MediaType>,
    m: &lodestash_core::catalog::AssetMetrics,
) -> String {
    match mt {
        Some(MediaType::Audio) => m
            .duration_ms
            .map(|d| crate::audio::fmt_time(d.max(0) as f32 / 1000.0))
            .unwrap_or_else(|| "\u{2014}".into()),
        Some(MediaType::Image | MediaType::Texture | MediaType::Sprite) => {
            match (m.width_px, m.height_px) {
                (Some(w), Some(h)) if w == h => format!("{w}\u{00B2}"),
                (Some(w), Some(h)) => format!("{w}\u{00D7}{h}"),
                _ => "\u{2014}".into(),
            }
        }
        _ => "\u{2014}".into(),
    }
}
