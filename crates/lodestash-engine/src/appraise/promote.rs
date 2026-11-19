//! One pass over the queues, then the status lifecycle and the read-back the grid needs.
use std::sync::Arc;

use lodestash_core::catalog::{Catalog, StatusCounts};

use super::audio::drain_audio_embeddings;
use super::deps::drain_dependencies;
use super::embed::drain_image_embeddings;
use super::metrics::drain_metrics;
use super::preview::{drain_previews, PREVIEW_BACKFILL_BUDGET};
use crate::persist::read_back_assets;
use crate::types::*;

/// Drain everything the catalog still has queued (facets, then vectors), reconcile the
/// status lifecycle (DATA-MODEL §8), and read the fresh counts + asset identities back for
/// `roots` (`(name, root_path)`). A drain now *moves* statuses, so it is no longer
/// count-neutral: the health meter and STATUS facet need the re-read.
///
/// Runs on the persist thread's own connection — the sole writer (ARCHITECTURE §5).
pub fn drain_promote_resync(
    cat: &mut Catalog,
    embedder: Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    audio_embedder: Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>>,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    proxy: lodestash_ingest::decimate::Budget,
    roots: &[(String, String)],
    // Bounds one embedding pass (`None` = drain until the queue is empty — the egui pump's
    // behavior). The desktop writer passes a cap so a long Identify-all never blocks writes.
    budget: Option<std::time::Duration>,
    // The explicit set an `identify_asset` kick was asked about, or `None` for a general pass.
    //
    // When it is set, the **library-wide backfills below are skipped**. Metrics, previews and
    // dependency scans are opportunistic catch-up over the whole catalog; they have nothing to do
    // with the assets the caller named, and running them made a kick on 64 images spend 20 s
    // rendering previews for unrelated models (measured 2026-09-03: the requested work itself took
    // 342 ms). The idle lane owns that catch-up, and it converges there.
    only: Option<&[i64]>,
) -> (Option<StatusCounts>, AssetIdentityMaps, Vec<i64>) {
    let scoped = only.is_some();
    if !scoped {
        drain_metrics(cat, workspace_dir, cache_dir, proxy);
    }
    // Backfill previews for models the metrics drain will never revisit — anything indexed before
    // previews existed is already clean, so without this it stays blank forever. Time-boxed so a
    // few thousand models don't hold the persist thread; the queue is durable and converges over
    // successive passes.
    //
    // **The caller's budget wins when it is smaller.** `PREVIEW_BACKFILL_BUDGET` is 90 s, and it
    // was applied verbatim on the writer's once-a-second idle tick — the pass's own 1 s budget
    // only ever reached the embedding drains below. That is most of why a trivial user write
    // measured ~16 s on an idle app: not the model inference it looked like, but a preview
    // backfill happily rendering for a minute and a half between ops.
    if !scoped {
        drain_previews(
            cat,
            workspace_dir,
            cache_dir,
            proxy,
            budget.map_or(PREVIEW_BACKFILL_BUDGET, |b| b.min(PREVIEW_BACKFILL_BUDGET)),
        );
    }
    if !scoped {
        drain_dependencies(cat, workspace_dir);
    }
    if let Some(emb) = embedder {
        drain_image_embeddings(cat, emb.as_ref(), workspace_dir, cache_dir, budget, only);
    }
    if let Some(emb) = audio_embedder {
        drain_audio_embeddings(cat, emb.as_ref(), workspace_dir, budget, only);
    }
    let promoted = match cat.promote_statuses() {
        Ok(p) if p.is_empty() => return (None, Vec::new(), Vec::new()), // nothing moved
        Ok(p) => {
            tracing::info!(promoted = p.len(), "status lifecycle reconciled");
            p
        }
        Err(e) => {
            tracing::warn!(%e, "status promote failed; statuses stale until next drain");
            return (None, Vec::new(), Vec::new());
        }
    };
    let (counts, maps) = resync_after_drain(cat, roots);
    (counts, maps, promoted)
}

/// Re-read per-status counts + asset identities for `roots` after a drain has changed them.
/// Shared by every drain path, so what the grid learns never depends on which one ran.
pub fn resync_after_drain(
    cat: &mut Catalog,
    roots: &[(String, String)],
) -> (Option<StatusCounts>, AssetIdentityMaps) {
    let read = || -> Result<(StatusCounts, AssetIdentityMaps), String> {
        let mut sources = Vec::with_capacity(roots.len());
        for (name, root) in roots {
            let id = cat
                .upsert_import_source(name, root, "folder")
                .map_err(|e| e.to_string())?;
            sources.push((root.clone(), id));
        }
        let ids: Vec<i64> = sources.iter().map(|(_, id)| *id).collect();
        let counts = cat
            .status_counts_for_sources(&ids)
            .map_err(|e| e.to_string())?;
        Ok((counts, read_back_assets(cat, &sources)?))
    };
    match read() {
        Ok((counts, assets)) => (Some(counts), assets),
        Err(err) => {
            tracing::warn!(%err, "post-drain read-back failed; UI keeps the pre-drain counts");
            (None, Vec::new())
        }
    }
}
