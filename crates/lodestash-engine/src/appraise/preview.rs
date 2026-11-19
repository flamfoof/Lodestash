//! The preview stage: vector previews for models that have none (M15).
use std::time::Instant;

use lodestash_core::catalog::Catalog;

use super::plan::{past, Scope};
use super::resolve::local_job_path;

/// Generate vector previews for the models in `scope` that have none.
///
/// The metrics stage also produces previews, because it is the one place that already has the
/// mesh parsed — but it only visits `metrics_dirty` rows. That leaves every model indexed *before*
/// previews existed with no preview and no path to ever getting one, since nothing re-dirties an
/// unchanged file. This stage closes that hole: it asks the catalog which models lack a preview
/// and fills them in, so an existing library converges instead of staying blank forever.
///
/// **It never pulls a payload out of an archive** (review B3). A model still inside its package
/// has no stable path to key a preview on, so the queue does not list it — and should one reach
/// here anyway, it is skipped by where its bytes are, before anything is read. This stage used to
/// resolve every job first, which for an in-pack model means inflating its archive up to the
/// payload, and then skip the result: on every pass, for ever, because a model that gets no
/// preview never left the queue.
///
/// Returns how many previews it attempted.
pub fn drain_previews(
    cat: &mut Catalog,
    scope: &Scope,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    proxy: lodestash_ingest::decimate::Budget,
    deadline: Option<Instant>,
) -> usize {
    const BATCH: usize = 16;
    let Some(dir) = cache_dir else {
        return 0;
    };
    let mut total = 0usize;
    // Every id this pass has looked at, so a model that fails to render cannot be handed back
    // forever. A set rather than a high-water id: since D3b the queue is ordered by where the user
    // is looking, and "greater than the last id" no longer means "not yet tried" (UI-SPEC §3).
    let mut attempted: Vec<i64> = Vec::new();
    'queue: loop {
        // Time-boxed rather than run-to-completion: four CPU renders per model over a few
        // thousand models is minutes of work, and it must not hold the writer thread away from
        // scans and mutations. The queue is durable, so the next pass picks up where this left off.
        if past(deadline) {
            break;
        }
        let jobs = match cat.assets_needing_preview_within(&attempted, BATCH, scope) {
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
            if past(deadline) {
                break 'queue;
            }
            // Record every id we look at, successful or not, so a model that fails to render
            // cannot wedge the queue ahead of the ones behind it.
            attempted.push(job.asset_id);
            let Some(path) = local_job_path(&job, workspace_dir) else {
                continue;
            };
            store_vector_preview(cat, job.asset_id, &path, &job.media_code, dir, proxy);
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
