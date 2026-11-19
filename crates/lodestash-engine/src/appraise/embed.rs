//! The visual embedding stage: read a slice of the queue, encode it (decode for 2D, multiview
//! render for 3D, a shader ball for a material), store the vectors. Three phases, so the encode
//! can run off the writer (ADR-0013).
use std::time::Instant;

use lodestash_core::catalog::Catalog;

use super::plan::{past, Scope};
use super::resolve::{resolve_job_image, resolve_job_path};

/// Encode one 3D model by rendering it from a few orbit views and mean-pooling the per-view
/// SigLIP embeddings (IDENTIFICATION-AND-SEARCH §3 Tier 1, "SigLIP 2 image / 3D-multiview").
/// `None` when the mesh can't be loaded or carries no geometry.
pub fn embed_model_multiview(
    path: &std::path::Path,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    cache_dir: Option<&std::path::Path>,
) -> Option<Vec<f32>> {
    // Untextured multiview, so it shares the parse-once mesh cache with thumbnails/metrics
    // (TECH-STACK §7): the FBX parses once across all three consumers. It embeds the same low-poly
    // proxy the grid thumbnails render — SigLIP reads silhouette, which decimation preserves, and
    // sharing the artifact keeps one cached mesh per file rather than two.
    let meshes = match cache_dir {
        Some(dir) => lodestash_ingest::mesh::load_preview_cached(
            path,
            dir,
            lodestash_ingest::decimate::PREVIEW_BUDGET,
        ),
        None => lodestash_ingest::mesh::load_any(path, &std::collections::BTreeMap::new()),
    }
    .map_err(|e| tracing::warn!(%e, "mesh load failed"))
    .ok()?;
    let views = lodestash_ingest::render3d::render_views(
        &meshes,
        RENDER_SIZE,
        lodestash_ingest::render3d::DEFAULT_VIEWS,
    );
    if views.is_empty() {
        return None;
    }
    // Mean-pool then L2-normalise: the standard multiview aggregation, and it keeps the result
    // in the same unit-sphere space as the single-image and text vectors.
    let mut acc = vec![0.0f32; emb.dim()];
    for v in &views {
        for (a, x) in acc.iter_mut().zip(emb.embed_image(v)) {
            *a += x;
        }
    }
    let norm: f32 = acc.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm <= 0.0 {
        return None;
    }
    for a in &mut acc {
        *a /= norm;
    }
    Some(acc)
}

/// Offscreen render resolution per view — SigLIP resizes to 256² anyway, so rendering larger
/// only costs rasterisation time.
pub const RENDER_SIZE: u32 = 256;

/// Render a **material** job as a shader ball for the encoder: the albedo painted on a sphere, or —
/// when the dependency graph resolved no texture — the flat base colour, which still encodes as a
/// material rather than leaving the row `embedding_dirty` forever. Same shape as
/// `thumb::generate_material`, minus the cache: the vector is the artifact here, not a PNG.
///
/// Opaque backdrop, unlike the thumbnail's transparent one — SigLIP reads RGB, so an alpha
/// background composites to black and eats the ball's silhouette.
pub fn render_job_material(albedo: Option<&std::path::Path>) -> Option<image::RgbaImage> {
    // A texture we can't decode is not a failure: the ball still shows the material's colour.
    let tex = albedo
        .and_then(|p| lodestash_ingest::raster::decode(p).ok())
        // Downscale first: a 4k atlas costs far more to sample per pixel than 256² is worth.
        .map(|img| img.thumbnail(512, 512).to_rgba8());
    lodestash_ingest::material::render_sphere(
        tex.as_ref(),
        [210, 210, 210, 255],
        RENDER_SIZE,
        [32, 32, 32, 255],
    )
}

/// `asset_id` → albedo texture path, read once per batch (materials only).
pub type AlbedoPaths = std::collections::HashMap<i64, String>;

/// One finished embedding, computed away from the catalog.
pub struct ComputedEmbedding {
    pub asset_id: i64,
    pub media_type_id: i64,
    pub source_id: i64,
    pub modality: &'static str,
    pub vector: Vec<f32>,
}

/// Phase 1 of the visual embed drain: **read** a slice of the queue. Fast, catalog-only.
///
/// The three phases exist because the compute is not fast and the catalog is single-writer
/// (ADR-0002). Interleaved read → encode → write meant the writer thread held the catalog for the
/// whole encode, and one unit is a four-view render plus four SigLIP passes — ~17 s for a 3D model
/// on this CPU. Every user write queued behind it: a `POST /v1/stage` measured 25 ms on an idle
/// app and ~18 s once an embedder was actually loaded. Splitting read / compute / write lets the
/// heavy middle run anywhere, and the writer only ever does the two cheap ends.
///
/// Confined to `scope`, so an `identify_asset` kick embeds the assets it was asked about rather
/// than the front of the global queue.
pub fn visual_embed_batch(
    cat: &Catalog,
    limit: usize,
    scope: &Scope,
) -> (Vec<lodestash_core::catalog::VisualEmbedJob>, AlbedoPaths) {
    let jobs = cat
        .visual_assets_needing_embedding_within(limit, scope)
        .unwrap_or_else(|e| {
            tracing::warn!(%e, "visual embed queue query failed; retries next pass");
            Vec::new()
        });
    if jobs.is_empty() {
        return (jobs, Default::default());
    }
    // A material's albedo lives on the `uses_texture` edges, not on the job row, so it is read
    // here — with the jobs — and carried into the compute phase, which never sees a catalog.
    let albedo = cat.material_albedo_paths().unwrap_or_else(|e| {
        tracing::warn!(%e, "material albedo lookup failed; shader balls render flat");
        Default::default()
    });
    (jobs, albedo)
}

/// Phase 2: **compute**. No catalog, so this can run on any thread — that is the whole point.
pub fn compute_visual_embeddings(
    jobs: Vec<lodestash_core::catalog::VisualEmbedJob>,
    albedo: &AlbedoPaths,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
) -> Vec<ComputedEmbedding> {
    jobs.into_iter()
        .map(|job| {
            let modality = if job.media_code == "model_3d" {
                "model_3d"
            } else {
                "image"
            };
            let vector = if job.media_code == "model_3d" {
                // On-disk, workspace-extracted, or an in-pack model pulled to a scratch file — the
                // multiview renderer is geometry-only, so an isolated (untextured) temp .fbx/.glb
                // embeds by shape exactly as an on-disk one would.
                resolve_job_path(&job, workspace_dir)
                    .as_ref()
                    .and_then(|(p, _)| embed_model_multiview(p, emb, cache_dir))
            } else if job.media_code == "material" {
                // A `.mat` has no decodable pixels of its own — it encodes as the shader ball the
                // grid shows for it (UI-SPEC §13.1), so search matches on its look, not its name.
                render_job_material(albedo.get(&job.asset_id).map(std::path::Path::new))
                    .map(|img| emb.embed_image(&img))
            } else {
                resolve_job_image(&job, workspace_dir).map(|img| emb.embed_image(&img.to_rgba8()))
            }
            // ponytail: unusable (undecodable / unrenderable / evicted from the workspace) →
            // zero vec clears the flag so the drain always advances and never re-queries the
            // same row forever; it scores 0 in KNN so it never surfaces.
            .unwrap_or_else(|| vec![0.0; emb.dim()]);
            ComputedEmbedding {
                asset_id: job.asset_id,
                media_type_id: job.media_type_id,
                source_id: job.source_id,
                modality,
                vector,
            }
        })
        .collect()
}

/// Phase 3: **write**. Fast, catalog-only. Returns how many landed. Serves the audio stage too:
/// a computed embedding names its own modality.
pub fn store_embeddings(cat: &mut Catalog, model_id: &str, done: &[ComputedEmbedding]) -> usize {
    let mut n = 0;
    for d in done {
        if let Err(e) = cat.upsert_embedding(
            d.asset_id,
            model_id,
            d.modality,
            Some(d.media_type_id),
            Some(d.source_id),
            &d.vector,
        ) {
            tracing::warn!(%e, "upsert_embedding failed; stopping the store");
            break;
        }
        n += 1;
    }
    if n > 0 {
        tracing::debug!(embedded = n, model = model_id, "embeddings written");
    }
    n
}

/// The three phases run back-to-back on one thread — the kick path, where the user asked for it
/// and expects the app to be busy. The idle path drives the phases separately so the compute
/// happens off the writer (see [`visual_embed_batch`]).
pub fn drain_image_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    scope: &Scope,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    deadline: Option<Instant>,
) {
    // **Slice size follows the budget.** One unit is a four-view render plus four SigLIP passes
    // (~17 s here), so a fixed batch of 64 overshoots a 20 s budget by minutes no matter where the
    // deadline is tested — a batch is atomic once started. Measured 2026-09-03: an `identify` kick
    // on 64 images finished its own tagging in 481 ms and then spent **>3 minutes** in here.
    // A pass with no deadline drains to empty and keeps the big batch.
    let batch = if deadline.is_some() { 4 } else { 64 };
    loop {
        if past(deadline) {
            break;
        }
        let (jobs, albedo) = visual_embed_batch(cat, batch, scope);
        let n = jobs.len();
        if n == 0 {
            break;
        }
        let done = compute_visual_embeddings(jobs, &albedo, emb, workspace_dir, cache_dir);
        if store_embeddings(cat, emb.model_id(), &done) < done.len() {
            break; // a write failed — it logged, and the queue is durable
        }
        if past(deadline) {
            tracing::debug!("visual embed pass hit its time budget");
            break;
        }
        if n < batch {
            break;
        }
    }
}
