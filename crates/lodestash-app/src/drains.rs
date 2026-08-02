//! Identify/persist backend: embedder loaders + the metrics/deps/embedding/tag drains, split
//! out of app.rs to shrink it. Moved to `lodestash-app` (B1); carries no renderer.
#![allow(clippy::too_many_arguments)]
use std::sync::Arc;

use lodestash_core::catalog::{Catalog, StatusCounts};

use crate::models;
use crate::paths::*;
use crate::persist::read_back_assets;
use crate::types::*;

/// Load the SigLIP joint image↔text embedder if its artifacts (+ a matching ONNX Runtime,
/// discovered via `load-dynamic`) are present. Any failure is `None` — AI search falls back to
/// its stub, never crashes. On a stock machine with no model this returns `None` immediately.
pub fn resolve_embedder() -> Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>> {
    let (vision, text, tok) = siglip_model_paths()?;
    match lodestash_ai::siglip::SiglipEmbedder::load(&vision, &text, &tok, SIGLIP_MODEL_ID) {
        Ok(e) => {
            tracing::info!("siglip embedder loaded — Search-with-AI (visual) is live");
            Some(Arc::new(e))
        }
        Err(e) => {
            tracing::info!(%e, "no local siglip model; Search-with-AI stays stubbed");
            None
        }
    }
}

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

/// Model id for the audio space. Kept distinct from the visual one so the two never mix in
/// `asset_embedding` — `embeddings_for_knn` shards on `model_id`.
pub const CLAP_MODEL_ID: &str = "clap-htsat-unfused";

/// Load the CLAP audio embedder if its artifacts are present, else `None` (audio simply gets no
/// vectors — visual search is unaffected). Mirrors `resolve_embedder`.
pub fn resolve_audio_embedder() -> Option<Arc<dyn lodestash_ai::embedder::Embedder + Send + Sync>> {
    let dir = models::clap_dir()?;
    let (a, t, k) = (
        dir.join("audio.onnx"),
        dir.join("text.onnx"),
        dir.join("tokenizer.json"),
    );
    if !(a.exists() && t.exists() && k.exists()) {
        return None;
    }
    match lodestash_ai::clap::ClapEmbedder::load(&a, &t, &k, CLAP_MODEL_ID) {
        Ok(e) => {
            tracing::info!("clap embedder loaded — audio is searchable");
            Some(Arc::new(e))
        }
        Err(e) => {
            tracing::info!(%e, "no local clap model; audio stays unsearchable");
            None
        }
    }
}

/// Decode an audio file to mono f32 at CLAP's 48 kHz. `rodio` bundles symphonia, so this needs
/// no new dependency. Capped at a little over the 10 s the encoder consumes.
/// ponytail: linear resampling — inaudible at mel resolution; swap in a windowed-sinc if audio
/// retrieval quality ever demands it.
pub fn decode_audio_48k_mono(path: &std::path::Path) -> Result<Vec<f32>, String> {
    use rodio::Source;
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let dec = rodio::Decoder::new(std::io::BufReader::new(file)).map_err(|e| e.to_string())?;
    let (sr, ch) = (dec.sample_rate(), dec.channels() as usize);
    if sr == 0 || ch == 0 {
        return Err("bad audio header".into());
    }
    // Only the first ~12s can matter (CLAP consumes 10s); bounds work on long music beds.
    let cap = sr as usize * ch * 12;
    let raw: Vec<f32> = dec.take(cap).map(|s| s as f32 / 32768.0).collect();
    if raw.is_empty() {
        return Err("no samples decoded".into());
    }
    let mono: Vec<f32> = if ch <= 1 {
        raw
    } else {
        raw.chunks(ch)
            .map(|c| c.iter().sum::<f32>() / ch as f32)
            .collect()
    };
    if sr == lodestash_ingest::melspec::SAMPLE_RATE {
        return Ok(mono);
    }
    let ratio = lodestash_ingest::melspec::SAMPLE_RATE as f64 / sr as f64;
    let out_len = (mono.len() as f64 * ratio) as usize;
    Ok((0..out_len)
        .map(|i| {
            let src = i as f64 / ratio;
            let i0 = src.floor() as usize;
            let frac = (src - i0 as f64) as f32;
            let a = mono.get(i0).copied().unwrap_or(0.0);
            let b = mono.get(i0 + 1).copied().unwrap_or(a);
            a + (b - a) * frac
        })
        .collect())
}

/// Embed dirty **audio** assets with CLAP: decode → 48 kHz mono → log-mel → audio tower.
/// Same sole-writer connection and same forward-progress rule as the visual drain (an
/// undecodable file stores a zero vector so it's never re-queried forever).
pub fn drain_audio_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    workspace_dir: Option<&std::path::Path>,
) {
    const BATCH: usize = 32;
    let mut total = 0usize;
    loop {
        let jobs = match cat.audio_assets_needing_embedding(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "audio embed queue query failed; retries next scan");
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
            let vector = path
                .as_deref()
                .and_then(|p| {
                    decode_audio_48k_mono(p)
                        .map_err(|e| tracing::warn!(%e, "audio decode failed"))
                        .ok()
                })
                .map(|samples| {
                    let (frames, mel) = lodestash_ingest::melspec::log_mel_spectrogram(&samples);
                    emb.embed_audio(&mel, frames)
                })
                .unwrap_or_else(|| vec![0.0; emb.dim()]);
            if let Err(e) = cat.upsert_embedding(
                job.asset_id,
                emb.model_id(),
                "audio",
                Some(job.media_type_id),
                Some(job.source_id),
                &vector,
            ) {
                tracing::warn!(%e, "upsert_embedding failed; stopping audio drain");
                return;
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        tracing::info!(embedded = total, "clap audio embeddings written");
    }
}

/// Tag dirty **visual** assets through an OpenAI-compatible cloud VLM (AI-PIPELINE §5). One call per
/// asset — a capable cloud model holds the schema, so no local caption pre-step. Single pass (no
/// weight-residency concern), keyed on `tags_dirty`, audio excluded (`tagger.supports`). Forward
/// progress preserved: a request that fails or a preview that won't render still clears the flag.
/// Zero-shot tags for one asset (AI-PIPELINE §2.1/§3). For a `model_3d` asset it scores the stored
/// **multiview** embedding (four orbit views pooled by the embedding drain) rather than the single
/// rendered thumbnail — a better basis, at no extra render cost — when that vector exists and came
/// from this tagger's own embedder. Every other case scores the decoded preview.
fn zeroshot_tags(
    zs: &lodestash_ai::image_tags::ImageZeroShot,
    cat: &Catalog,
    asset_id: i64,
    media_code: &str,
    img: Option<&image::RgbaImage>,
) -> Vec<lodestash_core::catalog::TagAssignment> {
    if media_code == "model_3d" {
        if let Ok(Some((mid, v))) = cat.embedding_of(asset_id) {
            if mid == zs.base_model_id() {
                return zs.tag_vector(&v);
            }
        }
    }
    img.map(|i| zs.tag(i)).unwrap_or_default()
}

pub fn drain_api_tags(
    cat: &mut Catalog,
    tagger: &lodestash_ai::openai::OpenAiTagger,
    fallback: Option<&dyn lodestash_ai::tagger::Tagger>,
    zero_shot: Option<&lodestash_ai::image_tags::ImageZeroShot>,
    max_assets: u32,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    progress: &crate::jobs::JobProgress,
) {
    use lodestash_ai::openai::ApiError;
    use lodestash_ai::tagger::{TagInput, Tagger};
    use std::sync::atomic::Ordering;
    const BATCH: usize = 16;
    const TAG_PX: u32 = 384;
    let model_id = tagger.model_id().to_string();
    // Budget guard (AI-PIPELINE §5.1): stop after `max_assets` API calls and leave the rest queued,
    // so a mis-scoped run on a paid endpoint can't run away. `0` = uncapped (a local/free endpoint).
    let mut tagged: u32 = 0;
    // Once a *hard* error proves the endpoint is unusable (bad key/quota/non-vision model), stop
    // hammering it for the rest of the pass — every remaining asset would fail the same way. If a
    // local fallback exists we use it for the remainder; if not, we've already stopped (see below).
    let mut api_dead = false;
    loop {
        if progress.is_cancelled() {
            return;
        }
        let jobs = match cat.assets_needing_tags(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "tag queue query failed; retries next run");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            if progress.is_cancelled() {
                return;
            }
            // Budget cap reached — stop and leave the remaining assets queued for the next run.
            if max_assets != 0 && tagged >= max_assets {
                tracing::info!(
                    max_assets,
                    "api tag budget cap reached; remaining assets stay queued"
                );
                return;
            }
            // Audio has no cloud path. If the CLAP pass already tagged it, it isn't in this queue;
            // if it couldn't run, clear the flag with an empty set (as the local drain does) rather
            // than skipping — skipping would return the same audio-only batch forever.
            if !tagger.supports(&job.media_code) {
                if let Err(e) = cat.set_tags(job.asset_id, "ai", Some(&model_id), &[]) {
                    tracing::warn!(%e, "api audio clear failed; stopping api tag drain");
                    return;
                }
                progress.done.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            let resolved = resolve_job_path(&job, workspace_dir);
            let path = resolved.as_ref().map(|(p, _)| p.clone());
            let name = path
                .as_deref()
                .filter(|_| resolved.as_ref().is_some_and(|(_, g)| g.is_none()))
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Some(img) = path
                .as_deref()
                .zip(cache_dir)
                .and_then(|(p, cache)| tag_preview(p, cache, TAG_PX))
            else {
                // No preview to send. Nothing to look at ⇒ clear (as the local drain does), don't
                // leave it dirty to re-loop forever.
                if let Err(e) = cat.set_tags(job.asset_id, "ai", Some(&model_id), &[]) {
                    tracing::warn!(%e, "api set_tags(empty) failed; stopping api tag drain");
                    return;
                }
                progress.done.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            let input = TagInput {
                media_code: &job.media_code,
                name: &name,
                image: Some(&img),
            };

            // The endpoint already proved dead this pass: skip it and use the local model directly.
            // (`api_dead` is only ever set when a fallback exists — see the error arm.)
            if api_dead {
                let mut tags = fallback
                    .map(|fb| fb.tag(&input, None).tags)
                    .unwrap_or_default();
                if let Some(z) = zero_shot {
                    tags.extend(zeroshot_tags(
                        z,
                        cat,
                        job.asset_id,
                        &job.media_code,
                        Some(&img),
                    ));
                }
                let tags = lodestash_ai::tagger::validate(tags);
                if let Err(e) = cat.set_tags(job.asset_id, "ai", Some(&model_id), &tags) {
                    tracing::warn!(%e, "api(fallback) set_tags failed; stopping api tag drain");
                    return;
                }
                progress.done.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            tagged += 1; // one asset went to the API (Ok or fell back after a real attempt)
            let tags = match attempt_api_tag(tagger, &input, progress) {
                AttemptOutcome::Ok(tags) => tags,
                AttemptOutcome::Cancelled => return,
                AttemptOutcome::Failed(err) => {
                    progress.failed.fetch_add(1, Ordering::Relaxed);
                    match fallback {
                        // Substitute the local model for this asset (AI-PIPELINE §5.1). A hard error
                        // also flips `api_dead` so we stop retrying a dead endpoint; a throttle does
                        // not — it may clear by the next asset.
                        Some(fb) if fb.supports(&job.media_code) => {
                            if matches!(err, ApiError::Hard(_)) {
                                tracing::warn!(%err, "api tagger failed hard; using local model for the rest of this run");
                                api_dead = true;
                            }
                            fb.tag(&input, None).tags
                        }
                        // No local model to fall back to: leave this asset **dirty** (never mark it
                        // done-with-no-tags) and stop the pass. The next run resumes here.
                        _ => {
                            tracing::warn!(%err, "api tagger failed and no local fallback; asset stays queued, stopping run");
                            return;
                        }
                    }
                }
            };
            // Union the API tags with the SigLIP zero-shot proposals over the same preview.
            let mut tags = tags;
            if let Some(z) = zero_shot {
                tags.extend(zeroshot_tags(
                    z,
                    cat,
                    job.asset_id,
                    &job.media_code,
                    Some(&img),
                ));
            }
            let tags = lodestash_ai::tagger::validate(tags);
            if let Err(e) = cat.set_tags(job.asset_id, "ai", Some(&model_id), &tags) {
                tracing::warn!(%e, "api set_tags failed; stopping api tag drain");
                return;
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Outcome of one asset's API tagging after backoff/retry.
enum AttemptOutcome {
    Ok(Vec<lodestash_core::catalog::TagAssignment>),
    /// The user cancelled during a backoff sleep — the caller must stop the drain.
    Cancelled,
    /// Exhausted retries (throttle) or a hard error — the caller decides fallback vs. leave-dirty.
    Failed(lodestash_ai::openai::ApiError),
}

/// One asset, with rate-limit backoff + resume (AI-PIPELINE §5.1). Retries only a `Throttled` call
/// (honouring `Retry-After`, else exponential backoff + jitter); a `Hard` error returns immediately.
/// Cancellation is polled during the sleep so a paused run stops promptly.
fn attempt_api_tag(
    tagger: &lodestash_ai::openai::OpenAiTagger,
    input: &lodestash_ai::tagger::TagInput<'_>,
    progress: &crate::jobs::JobProgress,
) -> AttemptOutcome {
    use lodestash_ai::openai::ApiError;
    const MAX_RETRIES: u32 = 4;
    let mut attempt: u32 = 0;
    loop {
        match tagger.try_tag(input) {
            Ok(res) => return AttemptOutcome::Ok(res.tags),
            Err(ApiError::Throttled { retry_after }) => {
                if attempt >= MAX_RETRIES {
                    return AttemptOutcome::Failed(ApiError::Throttled { retry_after });
                }
                // Server's Retry-After wins; otherwise 2s, 4s, 8s, 16s (capped 30s) + up-to-1s jitter.
                let base = retry_after.unwrap_or_else(|| {
                    let secs = (2u64 << attempt).min(30);
                    std::time::Duration::from_secs(secs)
                });
                let wait = base + std::time::Duration::from_millis(jitter_ms());
                if sleep_cancellable(wait, progress) {
                    return AttemptOutcome::Cancelled;
                }
                attempt += 1;
            }
            Err(hard) => return AttemptOutcome::Failed(hard),
        }
    }
}

/// Cheap jitter (0–999 ms) from the wall clock — avoids a `rand` dependency for backoff spread,
/// which only needs to be uncorrelated between callers, not cryptographic.
fn jitter_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % 1000)
        .unwrap_or(0)
}

/// Sleep `dur`, waking every 250 ms to check for cancel. Returns `true` if cancelled mid-sleep.
fn sleep_cancellable(dur: std::time::Duration, progress: &crate::jobs::JobProgress) -> bool {
    let step = std::time::Duration::from_millis(250);
    let mut left = dur;
    while left > std::time::Duration::ZERO {
        if progress.is_cancelled() {
            return true;
        }
        let nap = left.min(step);
        std::thread::sleep(nap);
        left = left.saturating_sub(nap);
    }
    progress.is_cancelled()
}

/// Tag dirty **audio** assets by CLAP zero-shot (AI-PIPELINE §6/§110): decode → log-mel → score
/// against the label vocabulary in the same joint space the embeddings use. Its own `tags_dirty`
/// queue, run before the VLM tag drain (which can't see audio anyway), so an audio re-tag pass works
/// without a re-embed. Same forward-progress rule: a clip that yields nothing still clears its flag
/// (`set_tags` with an empty set), so it is never re-queued forever.
pub fn drain_audio_tags(
    cat: &mut Catalog,
    zero_shot: &lodestash_ai::clap_tags::AudioZeroShot,
    workspace_dir: Option<&std::path::Path>,
    progress: &crate::jobs::JobProgress,
) {
    use std::sync::atomic::Ordering;
    const BATCH: usize = 32;
    let model_id = zero_shot.model_id().to_string();
    loop {
        if progress.is_cancelled() {
            return;
        }
        let jobs = match cat.audio_assets_needing_tags(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "audio tag queue query failed; retries next run");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            if progress.is_cancelled() {
                return;
            }
            let resolved = resolve_job_path(&job, workspace_dir);
            let path = resolved.as_ref().map(|(p, _)| p.clone());
            let tags = path
                .as_deref()
                .and_then(|p| {
                    decode_audio_48k_mono(p)
                        .map_err(|e| tracing::warn!(%e, "audio decode failed"))
                        .ok()
                })
                .map(|samples| {
                    let (frames, mel) = lodestash_ingest::melspec::log_mel_spectrogram(&samples);
                    zero_shot.tag(&mel, frames)
                })
                .unwrap_or_default();
            if let Err(e) = cat.set_tags(job.asset_id, "ai", Some(&model_id), &tags) {
                tracing::warn!(%e, "audio set_tags failed; stopping audio tag drain");
                return;
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
        if n < BATCH {
            break;
        }
    }
}

/// Embed the dirty **visual** assets (SigLIP vision tower) in batches over the persist thread's
/// own sole-writer connection: 2D files decode directly, 3D models are multiview-rendered first.
/// Failures store a zero vector — which cosine-scores 0, so it never matches — solely to clear
/// `embedding_dirty` and guarantee forward progress (no infinite re-query of a bad file).
/// Audio still awaits its own encoder (CLAP).
/// Harvest Unity's own bookkeeping for the rows marked `deps_dirty` (DATA-MODEL §10): each asset's
/// GUID from its `.meta` sidecar, and — for materials — the texture GUIDs it declares, which become
/// `asset_relationship` edges. Same sole-writer connection and forward-progress rule as the other
/// drains: an asset with no `.meta` still clears its flag.
///
/// Order matters within a drain but not across them: an edge whose target isn't catalogued yet is
/// stored `missing` and late-bound by `resolve_pending_relationships`, so a texture indexed after
/// its material still connects.
pub fn drain_dependencies(cat: &mut Catalog, workspace_dir: Option<&std::path::Path>) {
    const BATCH: usize = 256;
    let mut total = 0usize;
    let mut edges = 0usize;
    loop {
        let jobs = match cat.assets_needing_dep_scan(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "dependency queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            let path = match (&job.disk_path, &job.workspace_rel) {
                (Some(p), _) => Some(std::path::PathBuf::from(p)),
                (None, Some(rel)) => workspace_dir.map(|w| w.join(rel)),
                _ => None,
            };
            let guid = path.as_deref().and_then(lodestash_ingest::deps::guid_of);
            // Materials are the only catalogued type that declares GUIDs today: meshes embed
            // their materials (`externalObjects` is empty on a stock FBX import), and prefabs
            // aren't a `media_type`.
            let refs = match (job.media_code.as_str(), path.as_deref()) {
                ("material", Some(p)) => std::fs::read_to_string(p)
                    .map(|yaml| lodestash_ingest::deps::material_texture_guids(&yaml))
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            edges += refs.len();
            if let Err(e) =
                cat.record_dependencies(job.asset_id, guid.as_deref(), "uses_texture", &refs)
            {
                tracing::warn!(%e, "record_dependencies failed; stopping dependency drain");
                return;
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        drain_prefab_edges(cat);
        drain_inferred_texture_edges(cat);
    }
    match cat.resolve_pending_relationships() {
        Ok(late) if late > 0 => tracing::info!(late, "dependency edges late-bound"),
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "resolving pending dependency edges failed"),
    }
    if total > 0 {
        tracing::info!(scanned = total, edges, "dependency graph harvested");
    }
}

/// Walk every source root for `.prefab` files and rebuild the **mesh → material** edges from what
/// they bind (DATA-MODEL §10). Prefabs are not assets — they aren't a `media_type` — so they can't
/// ride the `deps_dirty` queue; they are read as pure evidence about assets that *are* catalogued.
///
/// Runs only when the dependency drain actually scanned something, so a steady-state library
/// re-walks nothing. ponytail: on-disk sources only — a prefab extracted into the package
/// workspace is skipped until someone wants that.
pub fn drain_prefab_edges(cat: &mut Catalog) {
    let roots = match cat.source_roots() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(%e, "prefab pass: source roots unavailable");
            return;
        }
    };
    let mut pairs: Vec<(String, String)> = roots
        .iter()
        .flat_map(|root| lodestash_ingest::deps::prefab_pairs_under(std::path::Path::new(root)))
        .collect();
    pairs.sort();
    pairs.dedup();
    match cat.replace_prefab_relationships(&pairs) {
        Ok(written) => tracing::info!(pairs = pairs.len(), written, "prefab edges rebuilt"),
        Err(e) => tracing::warn!(%e, "rebuilding prefab edges failed"),
    }
}

/// The §10 step-2 naming heuristic: connect a raw-export model (no `.meta`, so no GUID and no edge)
/// to sibling textures in the same source folder whose file stem name-matches the model's — e.g.
/// `Human3.fbx` → `Human3_Albedo.png`. The matching (`deps::texture_matches_model`) runs here in the
/// ingest-aware layer; the catalog only supplies the rows and takes the resolved edges. Applies only
/// to edge-less models, so it never overrides the deterministic GUID/prefab graph.
pub fn drain_inferred_texture_edges(cat: &mut Catalog) {
    use std::path::{Path, PathBuf};
    let models = match cat.edgeless_models() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(%e, "naming heuristic: edge-less models query failed");
            return;
        }
    };
    if models.is_empty() {
        return;
    }
    let textures = match cat.inference_textures() {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(%e, "naming heuristic: textures query failed");
            return;
        }
    };
    // Index textures by (source, parent folder) → (stem, id, rel_path).
    type DirIndex = std::collections::HashMap<(i64, PathBuf), Vec<(String, i64, String)>>;
    let mut by_dir: DirIndex = DirIndex::new();
    for (id, src, rel) in &textures {
        let p = Path::new(rel);
        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(stem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) {
            by_dir
                .entry((*src, dir))
                .or_default()
                .push((stem, *id, rel.clone()));
        }
    }
    let mut edges: Vec<(i64, i64, String)> = Vec::new();
    for (mid, src, rel) in &models {
        let p = Path::new(rel);
        let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
        let Some(mstem) = p.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
            continue;
        };
        if let Some(sibs) = by_dir.get(&(*src, dir)) {
            for (tstem, tid, trel) in sibs {
                if lodestash_ingest::deps::texture_matches_model(&mstem, tstem) {
                    edges.push((*mid, *tid, trel.clone()));
                }
            }
        }
    }
    if edges.is_empty() {
        return;
    }
    let borrowed: Vec<(i64, i64, &str)> =
        edges.iter().map(|(f, t, d)| (*f, *t, d.as_str())).collect();
    match cat.insert_inferred_texture_edges(&borrowed) {
        Ok(n) if n > 0 => tracing::info!(inferred = n, "naming-heuristic texture edges added"),
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "inserting inferred texture edges failed"),
    }
}

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
/// How long one pass may spend backfilling previews. Four 256px CPU renders per model is tens of
/// milliseconds each, so a few thousand models is minutes — too long to hold the persist thread
/// away from scans and mutations in one go.
pub const PREVIEW_BACKFILL_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

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
    let mut cursor = 0i64;
    loop {
        // Time-boxed rather than run-to-completion: four CPU renders per model over a few
        // thousand models is minutes of work, and it must not hold the persist thread away from
        // scans and mutations. The queue is durable, so the next pass picks up where this left off.
        if started.elapsed() >= budget {
            break;
        }
        let jobs = match cat.assets_needing_preview(cursor, BATCH) {
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
            // Advance past every id we look at, successful or not, so a model that fails to
            // render cannot wedge the queue ahead of the ones behind it.
            cursor = cursor.max(job.asset_id);
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
fn store_vector_preview(
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

/// Largest in-pack payload we will pull into memory. `read_payload_at` buffers the whole thing —
/// its own doc says the caller decides whether a payload is small enough — and nothing checked, so
/// a single multi-GB asset inside a `.unitypackage` allocated multi-GB in the identify drain.
///
/// Above the cap the asset simply isn't identified from inside the archive; extracting the pack
/// once materializes it on disk and every drain then reads it normally. That is the same trade the
/// in-pack *thumbnail* cap already makes, and gzip has no random access to make streaming cheaper
/// (HANDOFF §3).
const IN_PACK_PULL_MAX_BYTES: i64 = 256 * 1024 * 1024;

fn in_pack_pull_ok(size_bytes: i64, pack: &str) -> bool {
    if size_bytes > IN_PACK_PULL_MAX_BYTES {
        tracing::warn!(
            size_bytes,
            cap = IN_PACK_PULL_MAX_BYTES,
            pack,
            "in-pack payload too large to pull into memory; extract the pack to identify it"
        );
        return false;
    }
    true
}

/// Clip length via the decoder the player already uses. `total_duration` answers from the header
/// for WAV/FLAC; formats that can't (some MP3s) report `None`, which stays "unknown" rather than
/// paying for a full decode just to fill a stat row.
pub fn audio_duration_ms(path: &std::path::Path) -> Option<i64> {
    use rodio::Source;
    let file = std::fs::File::open(path).ok()?;
    let dec = rodio::Decoder::new(std::io::BufReader::new(file)).ok()?;
    Some(dec.total_duration()?.as_millis() as i64)
}

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
) -> (Option<StatusCounts>, AssetIdentityMaps) {
    drain_metrics(cat, workspace_dir, cache_dir, proxy);
    // Backfill previews for models the metrics drain will never revisit — anything indexed before
    // previews existed is already clean, so without this it stays blank forever. Time-boxed so a
    // few thousand models don't hold the persist thread; the queue is durable and converges over
    // successive passes.
    drain_previews(
        cat,
        workspace_dir,
        cache_dir,
        proxy,
        PREVIEW_BACKFILL_BUDGET,
    );
    drain_dependencies(cat, workspace_dir);
    if let Some(emb) = embedder {
        drain_image_embeddings(cat, emb.as_ref(), workspace_dir, cache_dir);
    }
    if let Some(emb) = audio_embedder {
        drain_audio_embeddings(cat, emb.as_ref(), workspace_dir);
    }
    match cat.promote_statuses() {
        Ok(0) => return (None, Vec::new()), // nothing moved — nothing to re-read
        Ok(n) => tracing::info!(writes = n, "status lifecycle reconciled"),
        Err(e) => {
            tracing::warn!(%e, "status promote failed; statuses stale until next drain");
            return (None, Vec::new());
        }
    }
    resync_after_drain(cat, roots)
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

/// Tag the `tags_dirty` backlog, one asset at a time, on the caller's sole-writer connection.
///
/// The preview handed to a vision model is the **thumbnail cache's own PNG** — the same pixels the
/// grid shows, produced by the same `thumb::generate` (decode for 2D, hero render for 3D), so the
/// model sees what the user sees and no second rendering path exists to drift.
///
/// Forward progress is unconditional: an asset the tagger refuses (audio), one whose preview can't
/// be made, and one the model answers nothing for all still clear `tags_dirty` via an empty write.
/// Otherwise the queue would hand back the same rows forever.
pub fn drain_tags(
    cat: &mut Catalog,
    tagger: &dyn lodestash_ai::tagger::Tagger,
    zero_shot: Option<&lodestash_ai::image_tags::ImageZeroShot>,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    progress: &crate::jobs::JobProgress,
) {
    use std::sync::atomic::Ordering;
    const BATCH: usize = 16;
    /// Fixed preview size, independent of the user's grid-quality setting: tagging results should
    /// not change because someone moved a slider.
    const TAG_PX: u32 = 384;

    if let Ok(total) = cat.count_needing_tags() {
        progress.total.store(total as u64, Ordering::Relaxed);
    }
    let mut tagged = 0usize;
    let model_id = tagger.model_id().to_string();
    loop {
        if progress.is_cancelled() {
            break;
        }
        let jobs = match cat.assets_needing_tags(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "tag queue query failed; retries next run");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        // Two passes over the batch, not one pass doing both stages. Interleaving the vision and
        // text models makes Ollama evict and reload multi-GB weights on every asset — measured
        // ~50 s/asset that way, against a few seconds when each model stays resident for a batch.
        let mut prepared: Vec<(
            lodestash_core::catalog::VisualEmbedJob,
            String,
            Option<String>,
            Vec<lodestash_core::catalog::TagAssignment>,
        )> = Vec::with_capacity(jobs.len());
        for job in jobs {
            if progress.is_cancelled() {
                return;
            }
            // In-pack payloads pull to a scratch file; keep the guard alive while `path` is read
            // (only this first pass touches the file — the caption image is decoded into memory here).
            let resolved = resolve_job_path(&job, workspace_dir);
            let path = resolved.as_ref().map(|(p, _)| p.clone());
            // A scratch pull (in-pack) has a random temp stem — don't feed it as the asset name; the
            // caption carries the signal. ponytail: real in-pack name needs the job to carry it.
            let is_scratch = resolved.as_ref().is_some_and(|(_, g)| g.is_some());
            let name = if is_scratch {
                String::new()
            } else {
                path.as_deref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            };
            // Decode the preview once, then use it for both the VLM caption and the SigLIP zero-shot
            // pass (AI-PIPELINE §2.1: the two are a union, run over the same pixels). Only visual
            // assets get here; audio has no preview and no zero-shot path.
            let img = if tagger.supports(&job.media_code) {
                path.as_deref()
                    .zip(cache_dir)
                    .and_then(|(p, cache)| tag_preview(p, cache, TAG_PX))
            } else {
                None
            };
            let caption = img.as_ref().and_then(|img| {
                tagger.caption(&lodestash_ai::tagger::TagInput {
                    media_code: &job.media_code,
                    name: &name,
                    image: Some(img),
                })
            });
            let zs_tags = zero_shot
                .map(|z| zeroshot_tags(z, cat, job.asset_id, &job.media_code, img.as_ref()))
                .unwrap_or_default();
            prepared.push((job, name, caption, zs_tags));
        }

        for (job, name, caption, zs_tags) in prepared {
            if progress.is_cancelled() {
                return;
            }
            let result = if tagger.supports(&job.media_code) {
                tagger.tag(
                    &lodestash_ai::tagger::TagInput {
                        media_code: &job.media_code,
                        name: &name,
                        image: None,
                    },
                    caption.as_deref(),
                )
            } else {
                lodestash_ai::tagger::TagResult::default()
            };
            // Union the VLM/name tags with the zero-shot proposals; validate dedupes + caps per facet.
            let mut merged = result.tags;
            merged.extend(zs_tags);
            let tags = lodestash_ai::tagger::validate(merged);
            // The caption is the model's own words about the asset; it lands in `description`,
            // which FTS indexes, so it is searchable rather than decorative.
            if let Some(caption) = &result.caption {
                if let Err(e) = cat.set_description(job.asset_id, Some(caption)) {
                    tracing::warn!(%e, "caption write failed");
                }
            }
            match cat.set_tags(job.asset_id, "ai", Some(&model_id), &tags) {
                Ok(n) => tagged += n,
                Err(e) => {
                    tracing::warn!(%e, "set_tags failed; stopping tag drain");
                    return;
                }
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
        }
    }
    tracing::info!(tags = tagged, model = %model_id, "tagging pass finished");
}

/// The cached preview PNG for `source`, decoded. `None` when the file can't be previewed at all
/// (an audio clip, an unreadable model) — the caller then tags from the name alone.
pub fn tag_preview(
    source: &std::path::Path,
    cache_dir: &std::path::Path,
    px: u32,
) -> Option<image::RgbaImage> {
    let meta = std::fs::metadata(source).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let key = lodestash_ingest::thumb::meta_key(source, meta.len(), mtime);
    let params = lodestash_ingest::thumb::ThumbParams {
        width: px,
        height: px,
        ..Default::default()
    };
    let png = lodestash_ingest::thumb::generate(source, cache_dir, &key, params).ok()?;
    Some(image::open(png).ok()?.to_rgba8())
}

/// Decode a visual drain job's image from whichever of the three §7 byte-sources applies:
/// an on-disk file, a package-extracted workspace file, or — for an **in-pack** asset — one
/// payload pulled into memory and decoded from bytes (DATA-MODEL §7 / M14). The in-pack path
/// never writes to disk: identify surgically reads a single payload, one asset at a time.
/// `None` = no source resolved / undecodable (the caller stores a zero vector to advance).
pub fn resolve_job_image(
    job: &lodestash_core::catalog::VisualEmbedJob,
    workspace_dir: Option<&std::path::Path>,
) -> Option<image::DynamicImage> {
    if let Some(p) = &job.disk_path {
        return lodestash_ingest::raster::decode(std::path::Path::new(p))
            .map_err(|e| tracing::warn!(%e, "image decode failed"))
            .ok();
    }
    if let (Some(rel), Some(ws)) = (&job.workspace_rel, workspace_dir) {
        return lodestash_ingest::raster::decode(&ws.join(rel))
            .map_err(|e| tracing::warn!(%e, "extracted image decode failed"))
            .ok();
    }
    if let (Some(pack), Some(off), Some(sz)) = (&job.pack_path, job.pack_offset, job.pack_size) {
        if !in_pack_pull_ok(sz, pack) {
            return None;
        }
        let bytes = lodestash_ingest::unitypackage::read_payload_at(
            std::path::Path::new(pack),
            off as u64,
            sz as u64,
        )
        .map_err(|e| tracing::warn!(%e, "in-pack payload pull failed"))
        .ok()?;
        return lodestash_ingest::raster::decode_bytes(&bytes, job.ext.as_deref())
            .map_err(|e| tracing::warn!(%e, "in-pack image decode failed"))
            .ok();
    }
    None
}

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

/// Resolve a drain job to a readable on-disk path from whichever §7 byte-source applies. On-disk and
/// workspace-extracted files resolve to their real path with no guard; an **in-pack** asset has its
/// one payload surgically pulled to an OS-temp scratch file (DATA-MODEL §7 / M14), returned with a
/// `TempPath` guard that deletes it on drop — so the path-based metrics/audio/tag extractors can
/// identify an in-pack asset without a workspace extraction. Serial + manual by design (INGESTION
/// §2), so the pull-per-file cost is fine. `None` when nothing resolves (evicted workspace file, no
/// locator). Keep the returned guard alive for as long as the path is used.
pub fn resolve_job_path(
    job: &lodestash_core::catalog::VisualEmbedJob,
    workspace_dir: Option<&std::path::Path>,
) -> Option<(std::path::PathBuf, Option<tempfile::TempPath>)> {
    if let Some(p) = &job.disk_path {
        return Some((std::path::PathBuf::from(p), None));
    }
    if let (Some(rel), Some(ws)) = (&job.workspace_rel, workspace_dir) {
        return Some((ws.join(rel), None));
    }
    if let (Some(pack), Some(off), Some(sz)) = (&job.pack_path, job.pack_offset, job.pack_size) {
        if !in_pack_pull_ok(sz, pack) {
            return None;
        }
        let bytes = lodestash_ingest::unitypackage::read_payload_at(
            std::path::Path::new(pack),
            off as u64,
            sz as u64,
        )
        .map_err(|e| tracing::warn!(%e, "in-pack payload pull failed"))
        .ok()?;
        // Keep the extension so the path-based decoders dispatch on it exactly as for a real file.
        let suffix = job
            .ext
            .as_deref()
            .map(|e| format!(".{e}"))
            .unwrap_or_default();
        let mut tmp = tempfile::Builder::new()
            .suffix(&suffix)
            .tempfile()
            .map_err(|e| tracing::warn!(%e, "scratch temp create failed"))
            .ok()?;
        std::io::Write::write_all(&mut tmp, &bytes)
            .map_err(|e| tracing::warn!(%e, "scratch temp write failed"))
            .ok()?;
        let temp_path = tmp.into_temp_path();
        let pathbuf = temp_path.to_path_buf();
        return Some((pathbuf, Some(temp_path)));
    }
    None
}

pub fn drain_image_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
) {
    const BATCH: usize = 64;
    let mut total = 0usize;
    // A material's albedo lives on the `uses_texture` edges, not on the job row, so it takes one
    // whole-library read up front — trivial next to a single encoder pass, and materials without a
    // resolved edge simply render flat.
    let albedo = cat.material_albedo_paths().unwrap_or_else(|e| {
        tracing::warn!(%e, "material albedo lookup failed; shader balls render flat");
        Default::default()
    });
    loop {
        let jobs = match cat.visual_assets_needing_embedding(BATCH) {
            Ok(j) => j,
            Err(e) => {
                tracing::warn!(%e, "visual embed queue query failed; retries next scan");
                break;
            }
        };
        if jobs.is_empty() {
            break;
        }
        let n = jobs.len();
        for job in jobs {
            let modality = if job.media_code == "model_3d" {
                "model_3d"
            } else {
                "image"
            };
            let vector = if job.media_code == "model_3d" {
                // On-disk, workspace-extracted, or an in-pack model pulled to a scratch file — the
                // multiview renderer is geometry-only, so an isolated (untextured) temp .fbx/.glb
                // embeds by shape exactly as an on-disk one would. Guard held while the path is read.
                let resolved = resolve_job_path(&job, workspace_dir);
                resolved
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
            if let Err(e) = cat.upsert_embedding(
                job.asset_id,
                emb.model_id(),
                modality,
                Some(job.media_type_id),
                Some(job.source_id),
                &vector,
            ) {
                tracing::warn!(%e, "upsert_embedding failed; stopping visual drain");
                return;
            }
            total += 1;
        }
        if n < BATCH {
            break;
        }
    }
    if total > 0 {
        tracing::info!(embedded = total, "siglip image embeddings written");
    }
}
