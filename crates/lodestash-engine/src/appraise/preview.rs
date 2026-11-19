//! The preview stage: vector previews for models that have none (M15).
use lodestash_core::catalog::Catalog;

use super::resolve::resolve_job_path;

/// How long one pass may spend backfilling previews.
///
/// Four 256px CPU renders per model is tens of milliseconds each, so the whole backlog is minutes
/// and cannot run in one go without starving scans and mutations. At 20s a pass a 3,400-model
/// library needed dozens of launches to converge, which in practice meant it never did — the grid
/// sat at 6% coverage. 90s still yields the thread promptly but actually finishes a library over a
/// handful of sessions.
///
/// ponytail: a fixed slice, not adaptive. If this ever competes with a scan, make it a Settings
/// action rather than growing the constant.
pub const PREVIEW_BACKFILL_BUDGET: std::time::Duration = std::time::Duration::from_secs(90);

/// Generate vector previews for models that have none.
///
/// The metrics drain also produces previews, because it is the one place that already has the mesh
/// parsed — but it only visits `metrics_dirty` rows. That leaves every model indexed *before*
/// previews existed with no preview and no path to ever getting one, since nothing re-dirties an
/// unchanged file. This drain closes that hole: it asks the catalog which models lack a preview
/// and fills them in, so an existing library converges instead of staying blank forever.
///
/// Runs on the persist thread with the writer connection, like every other drain (ARCHITECTURE §5).
/// In-pack assets are skipped for the same reason as in the metrics path — a scratch file has no
/// stable identity to key a preview on.
pub fn drain_previews(
    cat: &mut Catalog,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    proxy: lodestash_ingest::decimate::Budget,
    budget: std::time::Duration,
) -> usize {
    const BATCH: usize = 16;
    let Some(dir) = cache_dir else {
        return 0;
    };
    let started = std::time::Instant::now();
    let mut total = 0usize;
    // Every id this pass has looked at, so a model that fails to render cannot be handed back
    // forever. A set rather than a high-water id: since D3b the queue is ordered by where the user
    // is looking, and "greater than the last id" no longer means "not yet tried" (UI-SPEC §3).
    let mut attempted: Vec<i64> = Vec::new();
    loop {
        // Time-boxed rather than run-to-completion: four CPU renders per model over a few
        // thousand models is minutes of work, and it must not hold the persist thread away from
        // scans and mutations. The queue is durable, so the next pass picks up where this left off.
        if started.elapsed() >= budget {
            break;
        }
        let jobs = match cat.assets_needing_preview(&attempted, BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "preview queue query failed; retries next pass");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            // **Per render, not per batch.** The check above only ran between batches of 16, and
            // one render is ~1.1 s here — so a "1 second" idle slice actually held the writer for
            // ~17 s, and every user write queued behind it (a `POST /v1/stage` measured 16 s on an
            // idle app, against 2 ms when the drain had not started). The budget is a promise about
            // latency; checking it once per sixteen units is not keeping it.
            if started.elapsed() >= budget {
                break;
            }
            // Record every id we look at, successful or not, so a model that fails to render
            // cannot wedge the queue ahead of the ones behind it.
            attempted.push(job.asset_id);
            let resolved = resolve_job_path(&job, workspace_dir);
            let is_scratch = resolved.as_ref().is_some_and(|(_, g)| g.is_some());
            let Some((p, _)) = resolved.as_ref() else {
                continue;
            };
            if is_scratch {
                continue;
            }
            store_vector_preview(cat, job.asset_id, p, &job.media_code, dir, proxy);
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        tracing::info!(previews = total, "vector previews generated");
    }
    total
}

/// Render `path`'s four orbit views from its low-poly proxy, trace them into one contour SVG and
/// store it on the asset (M15). Only 3D models — a texture has pixels of its own and audio has a
/// waveform, so neither gains anything from being vectorised.
///
/// Skipped when the asset already has a preview built from the same mesh cache key, so a re-scan
/// that finds nothing changed doesn't spend four CPU renders per model. Every failure is silent:
/// a missing preview falls back to the raster thumbnail, which is not worth failing a scan over.
///
/// **Never called for an in-pack asset** (the caller checks the scratch guard). Its bytes resolve to
/// a temp file with a random stem, so `mesh::cache_key` — which is `(path, size, mtime)` — is unique
/// on every run: the staleness check could never hit, every metrics pass would re-render four views,
/// and each one would leave an orphan `.lod` in the cache that nothing reads or evicts. An in-pack
/// model keeps its raster thumbnail until the pack is materialized, at which point it has a real
/// path and picks up a preview like anything else. Giving in-pack assets previews needs the job to
/// carry a stable identity (the `package_entry` locator) rather than the scratch path.
pub(super) fn store_vector_preview(
    cat: &Catalog,
    asset_id: i64,
    path: &std::path::Path,
    media_code: &str,
    cache_dir: &std::path::Path,
    budget: lodestash_ingest::decimate::Budget,
) {
    use lodestash_ingest::{mesh, render3d, vectorize};

    if media_code != "model_3d" {
        return;
    }
    let key = format!("{}-{}", mesh::cache_key(path), budget.tag());
    match cat.has_current_preview(asset_id, &key) {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            tracing::debug!(%e, "preview staleness check failed; regenerating");
        }
    }
    let Ok(meshes) = mesh::load_preview_cached(path, cache_dir, budget) else {
        return;
    };
    let views =
        render3d::render_views_on(&meshes, PREVIEW_RENDER_PX, render3d::DEFAULT_VIEWS, [0; 4]);
    if views.is_empty() {
        return;
    }
    let traced: Vec<vectorize::ViewBands> = views
        .iter()
        .map(|v| vectorize::trace(v, vectorize::Bands::default()))
        .collect();
    let svg = vectorize::to_svg(&traced, PREVIEW_RENDER_PX, crate::types::MODEL_HEX);
    if let Err(e) = cat.set_vector_preview(asset_id, &svg, traced.len() as i64, &key) {
        tracing::debug!(%e, "storing vector preview failed");
    }
}

/// Trace resolution. The SVG is resolution-independent once traced, so this only sets how finely
/// the contour follows the silhouette — 256 px matches the multiview encoder's render size.
const PREVIEW_RENDER_PX: u32 = 256;
