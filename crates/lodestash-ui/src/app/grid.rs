//! Grid layout + sort math (pure functions), split out of app.rs. Child module of `app`,
//! reaches app's private items via `use super::*`.
use super::*;

pub(super) fn name_key(e: &AssetEntry) -> String {
    e.file
        .rel_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub(super) fn type_key(e: &AssetEntry) -> u8 {
    e.file.media_type.map(|m| m as u8).unwrap_or(255)
}

/// Apply the sort direction. Every arm of [`sort_indices`] ends in this.
pub(super) fn dir(o: std::cmp::Ordering, asc: bool) -> std::cmp::Ordering {
    if asc {
        o
    } else {
        o.reverse()
    }
}

/// Sort `idxs` (indices into `entries`) in place per the view/sort toolbar's `sort_by`/`asc`.
/// Name pre-computes keys once (decorate-sort-undecorate) since `name_key` allocates —
/// worth avoiding at O(n log n) comparisons on 10k+ entry libraries.
pub(super) fn sort_indices(
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
pub(super) fn folder_children(
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

/// Max thumbnails generating at once — half the cores (min 2). A 3D/material thumbnail is a
/// CPU-bound parse+render, so leaving cores free keeps the UI responsive while a folder of them
/// generates (INGESTION §6). rayon already caps at core count; this caps below it.
pub(super) fn thumb_inflight_cap() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(2))
        .unwrap_or(2)
}

/// How many card columns fit in `available` width. Always ≥ 1 so the grid never divides by zero.
/// `available` is the content width *after* subtracting the scrollbar (callers do that).
pub(super) fn grid_columns(available: f32, thumb_size: f32) -> usize {
    ((available + CARD_PAD) / (thumb_size + CARD_PAD)).max(1.0) as usize
}

/// The row window to prefetch thumbnails for: the visible `[start, end)` widened by
/// `LOOKAHEAD_ROWS` on each side and clamped to `[0, total_rows]`.
pub(super) fn lookahead_window(start: usize, end: usize, total_rows: usize) -> (usize, usize) {
    // Half the visible window each side → ~2x the visible grid is loading at once.
    let buf = ((end - start) / 2).max(LOOKAHEAD_ROWS_MIN);
    let buf_start = start.saturating_sub(buf);
    let buf_end = (end + buf).min(total_rows);
    (buf_start, buf_end)
}

/// The `sorted_cache` index range `[start, end)` whose textures stay resident:
/// the visible row window widened by `KEEP_ROWS` on each side, in entries, clamped to `len`.
pub(super) fn keep_range(
    row_start: usize,
    row_end: usize,
    cols: usize,
    len: usize,
) -> (usize, usize) {
    // 1.5x the visible window above and below stays resident; further rows evict
    // back to placeholders so VRAM stays bounded on large libraries.
    let keep = ((row_end - row_start) * 3 / 2).max(KEEP_ROWS_MIN);
    let keep_start = (row_start.saturating_sub(keep) * cols).min(len);
    let keep_end = ((row_end + keep) * cols).min(len);
    (keep_start, keep_end)
}

/// The pack-browser contents list's third column: duration for audio, dimensions for an image, `—`
/// otherwise or when the metric hasn't been extracted yet (PACK-DIRECTORY-NAV §6). Pure/testable;
/// the widget renders folders (from `folder_tiles`) then files (from `sorted_cache`) using this.
pub(super) fn content_third_col(
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
