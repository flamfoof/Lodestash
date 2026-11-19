//! The metrics stage: technical facets, perceptual hashes, and a model's vector preview while
//! its mesh is parsed anyway (DATA-MODEL §14).
use lodestash_core::catalog::Catalog;

use super::audio::audio_duration_ms;
use super::preview::store_vector_preview;
use super::resolve::resolve_job_path;

/// Extract technical facets (tri counts, dimensions, duration) for the rows `record_crawl` marked
/// `metrics_dirty` (DATA-MODEL §14). Runs on the persist thread's own connection like the embedding
/// drains, but unconditionally — facets are plain measurements and must not depend on the AI stack
/// being installed. A file we can't measure still gets its flag cleared (forward progress).
pub fn drain_metrics(
    cat: &mut Catalog,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    proxy: lodestash_ingest::decimate::Budget,
) {
    const BATCH: usize = 64;
    let mut total = 0usize;
    loop {
        let jobs = match cat.assets_needing_metrics(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "metrics queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            // In-pack payloads pull to a scratch file; keep the guard alive while `path` is read.
            let resolved = resolve_job_path(&job, workspace_dir);
            let path = resolved.as_ref().map(|(p, _)| p.clone());
            let metrics = path
                .as_deref()
                .map(|p| {
                    if job.media_code == "audio" {
                        lodestash_core::catalog::AssetMetrics {
                            duration_ms: audio_duration_ms(p),
                            ..Default::default()
                        }
                    } else {
                        // Passing cache_dir warms the parse-once mesh cache (TECH-STACK §7): a 3D
                        // asset's FBX parses here on scan, so its browse-time thumbnail skips it.
                        lodestash_ingest::metrics::extract(p, &job.media_code, cache_dir)
                    }
                })
                .unwrap_or_default();
            if let Err(e) = cat.set_metrics(job.asset_id, metrics) {
                tracing::warn!(%e, "set_metrics failed; stopping metrics drain");
                return;
            }
            // Perceptual hash (dHash, DATA-MODEL §5) for near-duplicate review: only raster
            // media get one, and it rides the same decode the metrics just did. In-pack scratch
            // pulls are fine — the hash is content-derived, not path-derived.
            if matches!(job.media_code.as_str(), "image" | "texture" | "sprite") {
                let ph = path
                    .as_deref()
                    .and_then(lodestash_ingest::metrics::perceptual_hash);
                if let Err(e) = cat.set_perceptual_hash(job.asset_id, ph) {
                    tracing::warn!(%e, "perceptual hash failed; retries next scan");
                }
            }
            // The mesh is parsed and cached right here, so this is the one place that can produce
            // the vector preview without a second parse — and it is already the persist thread, so
            // the write respects the single-writer rule (ARCHITECTURE §5).
            // An in-pack payload resolves to a *scratch* file with a random stem, so it has no
            // stable identity to key a preview on — see `store_vector_preview`.
            let is_scratch = resolved.as_ref().is_some_and(|(_, g)| g.is_some());
            if let (Some(p), Some(dir), false) = (path.as_deref(), cache_dir, is_scratch) {
                store_vector_preview(cat, job.asset_id, p, &job.media_code, dir, proxy);
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        tracing::info!(measured = total, "technical facets extracted");
    }
}
