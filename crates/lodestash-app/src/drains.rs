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
    budget: Option<std::time::Duration>,
    // The explicit set an `identify_asset` kick named, or `None` for the whole queue.
    only: Option<&[i64]>,
) {
    const BATCH: usize = 32;
    let mut total = 0usize;
    let deadline = budget.map(|b| std::time::Instant::now() + b);
    loop {
        let jobs = match cat.audio_assets_needing_embedding_within(BATCH, only) {
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
            // Per clip, not per batch: a decode plus a CLAP pass is seconds, so a 32-unit batch
            // overruns any budget worth setting. Same defect as the preview and visual-embed
            // drains had — a deadline tested outside the loop that does the work.
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                tracing::debug!(embedded = total, "audio embed pass hit its time budget");
                return;
            }
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
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            tracing::debug!(embedded = total, "audio embed pass hit its time budget");
            break;
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
    media_code: &str,
    img: Option<&image::RgbaImage>,
) -> Vec<lodestash_core::catalog::TagAssignment> {
    // **Not `model_3d`.** A model's stored vector is a mean-pooled multiview embedding of an
    // untextured, flat-shaded orthographic render — far outside what SigLIP was trained on, and it
    // shows. Measured over 20 models (2026-08-30): only ~20% produced any label at all, and half of
    // those were wrong — `usage:tileable texture` on a wall *mesh*, `material:metal` and
    // `subject:icon` on a giant mushroom. The same pass over real texture pixels is confident and
    // right (`usage:tileable texture` 0.96, `material:brick` 0.69 on a Synty dungeon atlas).
    //
    // So the labels ride on pixels the artist actually authored, never on our own render. A model
    // still gets tags — from the captioning tagger, which reads the same preview as prose and does
    // far better with it (`SM_Env_Pillar_Broken_02` → `subject:pillar, material:stone`). Silence
    // beats a guess is this module's own rule; a coin-flip tag is a guess.
    if media_code == "model_3d" {
        return Vec::new();
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
    // Restrict the pass to these assets, exactly as [`drain_tags`] does. `None` = the whole queue.
    // Without this an `identify_asset(ids)` routed to the cloud would take the front of the
    // **global** queue instead of the ids asked about — the same defect already fixed once on the
    // local path, and far more expensive here because every wrong asset is a paid API call.
    only: Option<&[i64]>,
) {
    use lodestash_ai::openai::ApiError;
    use lodestash_ai::tagger::{TagInput, Tagger};
    use std::sync::atomic::Ordering;
    const BATCH: usize = 16;
    const TAG_PX: u32 = 384;
    let model_id = tagger.model_id().to_string();
    // Each producer's tags go under its own source (v1 plan S04): the endpoint's, a local
    // fallback's, and the zero-shot pass's — never one merged under the endpoint's model.
    let api_source = lodestash_core::catalog::ai_source(&tagger.producer_id());
    let zs_source = zero_shot.map(|z| {
        (
            lodestash_core::catalog::ai_source(z.producer_id()),
            z.model_id().to_string(),
        )
    });
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
        let jobs = match cat.assets_needing_tags_within(BATCH, only) {
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
                if !write_groups(
                    cat,
                    job.asset_id,
                    vec![(api_source.clone(), Some(model_id.clone()), Vec::new())],
                ) {
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
                if !write_groups(
                    cat,
                    job.asset_id,
                    vec![(api_source.clone(), Some(model_id.clone()), Vec::new())],
                ) {
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
                // The dead endpoint wrote nothing: the fallback's tags go under the fallback's own
                // producer and model — never the failed remote model's.
                let mut groups = Vec::new();
                if let Some(fb) = fallback {
                    groups.push((
                        lodestash_core::catalog::ai_source(&fb.producer_id()),
                        Some(fb.model_id().to_string()),
                        lodestash_ai::tagger::validate(fb.tag(&input, None).tags),
                    ));
                }
                if let (Some(z), Some((src, zm))) = (zero_shot, &zs_source) {
                    groups.push((
                        src.clone(),
                        Some(zm.clone()),
                        lodestash_ai::tagger::validate(zeroshot_tags(
                            z,
                            &job.media_code,
                            Some(&img),
                        )),
                    ));
                }
                if !write_groups(cat, job.asset_id, groups) {
                    return;
                }
                progress.done.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            tagged += 1; // one asset went to the API (Ok or fell back after a real attempt)
            let (source, model, tags) = match attempt_api_tag(tagger, &input, progress) {
                AttemptOutcome::Ok(tags) => (api_source.clone(), model_id.clone(), tags),
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
                            (
                                lodestash_core::catalog::ai_source(&fb.producer_id()),
                                fb.model_id().to_string(),
                                fb.tag(&input, None).tags,
                            )
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
            // The SigLIP zero-shot proposals over the same preview stay under their own producer.
            let mut groups = vec![(source, Some(model), lodestash_ai::tagger::validate(tags))];
            if let (Some(z), Some((src, zm))) = (zero_shot, &zs_source) {
                groups.push((
                    src.clone(),
                    Some(zm.clone()),
                    lodestash_ai::tagger::validate(zeroshot_tags(z, &job.media_code, Some(&img))),
                ));
            }
            if !write_groups(cat, job.asset_id, groups) {
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
    max_assets: u32,
    // The explicit set an `identify_asset` kick was asked about, or `None` for the whole queue.
    only: Option<&[i64]>,
    // Wall-clock ceiling for the whole drain, checked **per asset**. A count budget alone is not
    // a latency bound when one unit is a decode plus a CLAP pass.
    budget: Option<std::time::Duration>,
) {
    use std::sync::atomic::Ordering;
    const BATCH: usize = 32;
    let deadline = budget.map(|b| std::time::Instant::now() + b);
    let model_id = zero_shot.model_id().to_string();
    let mut tagged_assets = 0usize;
    loop {
        if progress.is_cancelled() {
            return;
        }
        // Budget guard (AI-PIPELINE §5.1): stop after `max_assets` assets and leave the rest
        // queued, so a writer-thread pass (bounded by design) never blocks writes on a long
        // backlog. `0` = uncapped.
        if max_assets != 0 && tagged_assets >= max_assets as usize {
            return;
        }
        let jobs = match cat.audio_assets_needing_tags_within(BATCH, only) {
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
            // Hard per-asset cap inside the batch too, so `max_assets` is exact, not +BATCH.
            if max_assets != 0 && tagged_assets >= max_assets as usize {
                return;
            }
            // Per asset, not per batch: one unit here is a full decode plus a CLAP pass, and a
            // count budget says nothing about how long 48 of those take.
            if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                tracing::debug!("audio tag pass hit its time budget");
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
            let source = lodestash_core::catalog::ai_source(zero_shot.producer_id());
            if !write_groups(
                cat,
                job.asset_id,
                vec![(source, Some(model_id.clone()), tags)],
            ) {
                return;
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
            tagged_assets += 1;
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

/// Tag the `tags_dirty` backlog, one asset at a time, on the caller's sole-writer connection.
///
/// The preview handed to a vision model is the **thumbnail cache's own PNG** — the same pixels the
/// grid shows, produced by the same `thumb::generate` (decode for 2D, hero render for 3D), so the
/// model sees what the user sees and no second rendering path exists to drift.
///
/// Forward progress is unconditional: an asset the tagger refuses (audio), one whose preview can't
/// be made, and one the model answers nothing for all still clear `tags_dirty` via an empty write.
/// Otherwise the queue would hand back the same rows forever.
/// One asset's finished tag proposal, computed away from the writer.
pub struct ComputedTags {
    pub asset_id: i64,
    pub caption: Option<String>,
    pub tags: Vec<lodestash_core::catalog::TagAssignment>,
    /// The zero-shot pass's proposals, kept apart: it is its own producer, and storing them under
    /// the VLM's model id is the attribution M26 had to mark unrecoverable.
    pub zero_shot: Vec<lodestash_core::catalog::TagAssignment>,
}

/// Who wrote a slice's tags: `(producer id, model id)` of the tagger, and of the zero-shot pass
/// when one ran beside it.
#[derive(Debug, Clone)]
pub struct TagProducers {
    pub tagger: (String, String),
    pub zero_shot: Option<(String, String)>,
}

/// One asset's result as one batch: each producer's tags under its own source, and the queue
/// cleared — a result, a fallback's result or "nothing to look at" all finish the asset. `false`
/// when the catalog refused it; the caller stops, and the asset stays queued.
fn write_groups(
    cat: &mut Catalog,
    asset_id: i64,
    groups: Vec<(
        String,
        Option<String>,
        Vec<lodestash_core::catalog::TagAssignment>,
    )>,
) -> bool {
    use lodestash_core::catalog::{HistoryOrigin, TagBatch, TagBatchItem, TagMode};
    let items = groups
        .into_iter()
        .map(|(source, model_id, tags)| TagBatchItem {
            asset_id,
            source,
            model_id,
            mode: TagMode::ReplaceSource,
            tags,
            description: None,
        })
        .collect();
    match cat.apply_tag_batch(&TagBatch {
        items,
        completes_queue: true,
        history: Some(HistoryOrigin::Background),
    }) {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(%e, asset_id, "tag batch refused; the asset stays queued");
            false
        }
    }
}

/// Phase 1 of tagging: **read** a slice of the queue. Same split, and for the same reason, as
/// [`visual_embed_batch`] — a caption is a whole VLM inference (~13 s here) and the writer must
/// not be inside it.
pub fn tag_batch(
    cat: &Catalog,
    limit: usize,
    only: Option<&[i64]>,
) -> Vec<lodestash_core::catalog::VisualEmbedJob> {
    cat.assets_needing_tags_within(limit, only)
        .unwrap_or_else(|e| {
            tracing::warn!(%e, "tag queue query failed; retries next run");
            Vec::new()
        })
}

/// Phase 2: **compute**. Needs no catalog at all — pixels, a tagger and a label set.
///
/// Two passes over the batch, not one pass doing both stages: interleaving the vision and text
/// models makes Ollama evict and reload multi-GB weights per asset (~50 s/asset, against a few
/// seconds when each model stays resident for a batch).
pub fn compute_tags(
    jobs: Vec<lodestash_core::catalog::VisualEmbedJob>,
    tagger: &dyn lodestash_ai::tagger::Tagger,
    zero_shot: Option<&lodestash_ai::image_tags::ImageZeroShot>,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
) -> Vec<ComputedTags> {
    /// Fixed preview size, independent of the user's grid-quality setting: tagging results should
    /// not change because someone moved a slider.
    const TAG_PX: u32 = 384;
    // This path never fills `TagInput::image` — the caption phase is what carries the picture. A
    // tagger that reads the pixels directly therefore sees nothing here and answers "no tags",
    // which the caller stores as a finished result, clearing `tags_dirty` on an asset no model ever
    // looked at. Refuse instead: an empty return writes nothing, so the assets stay queued for the
    // drain that can actually serve them.
    if tagger.needs_image() {
        tracing::warn!(
            model = %tagger.model_id(),
            "this tagger reads the image directly and cannot run on the batch path; assets stay queued"
        );
        return Vec::new();
    }
    let mut prepared = Vec::with_capacity(jobs.len());
    for job in jobs {
        // In-pack payloads pull to a scratch file; keep the guard alive while `path` is read.
        let resolved = resolve_job_path(&job, workspace_dir);
        let path = resolved.as_ref().map(|(p, _)| p.clone());
        // A scratch pull (in-pack) has a random temp stem — don't feed it as the asset name; the
        // caption carries the signal.
        let is_scratch = resolved.as_ref().is_some_and(|(_, g)| g.is_some());
        let name = if is_scratch {
            String::new()
        } else {
            path.as_deref()
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
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
            .map(|z| zeroshot_tags(z, &job.media_code, img.as_ref()))
            .unwrap_or_default();
        prepared.push((job, name, caption, zs_tags));
    }

    prepared
        .into_iter()
        .map(|(job, name, caption, zs_tags)| {
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
            ComputedTags {
                asset_id: job.asset_id,
                caption: result.caption,
                tags: lodestash_ai::tagger::validate(result.tags),
                zero_shot: lodestash_ai::tagger::validate(zs_tags),
            }
        })
        .collect()
}

/// Phase 3: **write** — the whole slice as one atomic batch, each producer's tags under its own
/// source, the configured captioner's caption with them. Returns the number of tag rows proposed.
pub fn store_tags(cat: &mut Catalog, producers: &TagProducers, results: &[ComputedTags]) -> usize {
    use lodestash_core::catalog::{
        ai_source, HistoryOrigin, TagBatch, TagBatchItem, TagMode, MAX_DESCRIPTION_CHARS,
    };
    if results.is_empty() {
        return 0;
    }
    let mut items = Vec::with_capacity(results.len() * 2);
    for r in results {
        // The caption is the model's own words about the asset; it lands in `description`, which
        // FTS indexes, so it is searchable rather than decorative.
        let caption = r
            .caption
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(|c| c.chars().take(MAX_DESCRIPTION_CHARS).collect::<String>());
        items.push(TagBatchItem {
            asset_id: r.asset_id,
            source: ai_source(&producers.tagger.0),
            model_id: Some(producers.tagger.1.clone()),
            mode: TagMode::ReplaceSource,
            tags: r.tags.clone(),
            description: caption,
        });
        if let Some((producer, model)) = &producers.zero_shot {
            items.push(TagBatchItem {
                asset_id: r.asset_id,
                source: ai_source(producer),
                model_id: Some(model.clone()),
                mode: TagMode::ReplaceSource,
                tags: r.zero_shot.clone(),
                description: None,
            });
        }
    }
    match cat.apply_tag_batch(&TagBatch {
        items,
        completes_queue: true,
        history: Some(HistoryOrigin::Background),
    }) {
        Ok(_) => results
            .iter()
            .map(|r| r.tags.len() + r.zero_shot.len())
            .sum(),
        Err(e) => {
            tracing::warn!(%e, "tag slice refused; its assets stay queued");
            0
        }
    }
}

/// The three phases back-to-back on one thread — the **kick** path, where the user asked for it
/// and is watching a progress bar. The idle path drives the phases separately so the inference
/// happens off the writer (see [`tag_batch`]).
pub fn drain_tags(
    cat: &mut Catalog,
    tagger: &dyn lodestash_ai::tagger::Tagger,
    zero_shot: Option<&lodestash_ai::image_tags::ImageZeroShot>,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    progress: &crate::jobs::JobProgress,
    max_assets: u32,
    // Restrict the pass to these assets. `None` = the whole queue (the idle slice);
    // `Some(ids)` = exactly what `identify_asset` was asked about.
    only: Option<&[i64]>,
) {
    use std::sync::atomic::Ordering;
    const BATCH: usize = 16;

    if let Ok(total) = cat.count_needing_tags() {
        progress.total.store(total as u64, Ordering::Relaxed);
    }
    let mut tagged = 0usize;
    let mut tagged_assets = 0usize;
    let model_id = tagger.model_id().to_string();
    let producers = TagProducers {
        tagger: (tagger.producer_id(), model_id.clone()),
        zero_shot: zero_shot.map(|z| (z.producer_id().to_string(), z.model_id().to_string())),
    };
    loop {
        if progress.is_cancelled() {
            break;
        }
        // Budget guard (AI-PIPELINE §5.1): stop after `max_assets` assets and leave the rest
        // queued. `0` = uncapped.
        let room = if max_assets == 0 {
            BATCH
        } else {
            BATCH.min((max_assets as usize).saturating_sub(tagged_assets))
        };
        if room == 0 {
            break;
        }
        let jobs = tag_batch(cat, room, only);
        let n = jobs.len();
        if n == 0 {
            break;
        }
        let done = compute_tags(jobs, tagger, zero_shot, workspace_dir, cache_dir);
        if progress.is_cancelled() {
            return;
        }
        tagged += store_tags(cat, &producers, &done);
        tagged_assets += done.len();
        progress
            .done
            .fetch_add(done.len() as u64, Ordering::Relaxed);
        if n < room {
            break;
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
pub fn visual_embed_batch(
    cat: &Catalog,
    limit: usize,
) -> (Vec<lodestash_core::catalog::VisualEmbedJob>, AlbedoPaths) {
    visual_embed_batch_within(cat, limit, None)
}

/// [`visual_embed_batch`], restricted to an explicit set — what an `identify_asset` kick needs so
/// it embeds the assets it was asked about rather than the front of the global queue.
pub fn visual_embed_batch_within(
    cat: &Catalog,
    limit: usize,
    only: Option<&[i64]>,
) -> (Vec<lodestash_core::catalog::VisualEmbedJob>, AlbedoPaths) {
    let jobs = cat
        .visual_assets_needing_embedding_within(limit, only)
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

/// Phase 3: **write**. Fast, catalog-only. Returns how many landed.
pub fn store_visual_embeddings(
    cat: &mut Catalog,
    model_id: &str,
    done: &[ComputedEmbedding],
) -> usize {
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
            tracing::warn!(%e, "upsert_embedding failed; stopping visual store");
            break;
        }
        n += 1;
    }
    if n > 0 {
        tracing::info!(embedded = n, "siglip image embeddings written");
    }
    n
}

/// The three phases run back-to-back on one thread — the kick path, where the user asked for it
/// and expects the app to be busy. The idle path drives the phases separately so the compute
/// happens off the writer (see [`visual_embed_batch`]).
pub fn drain_image_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    budget: Option<std::time::Duration>,
    // The explicit set an `identify_asset` kick named, or `None` for the whole queue.
    only: Option<&[i64]>,
) {
    // **Slice size follows the budget.** One unit is a four-view render plus four SigLIP passes
    // (~17 s here), so a fixed batch of 64 overshoots a 20 s budget by minutes no matter where the
    // deadline is tested — a batch is atomic once started. Measured 2026-09-03: an `identify` kick
    // on 64 images finished its own tagging in 481 ms and then spent **>3 minutes** in here.
    // Unbudgeted callers (the old pump, which drains to empty) keep the big batch.
    let batch = if budget.is_some() { 4 } else { 64 };
    let deadline = budget.map(|b| std::time::Instant::now() + b);
    loop {
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            break;
        }
        let (jobs, albedo) = visual_embed_batch_within(cat, batch, only);
        let n = jobs.len();
        if n == 0 {
            break;
        }
        let done = compute_visual_embeddings(jobs, &albedo, emb, workspace_dir, cache_dir);
        if store_visual_embeddings(cat, emb.model_id(), &done) < done.len() {
            break; // a write failed — it logged, and the queue is durable
        }
        if deadline.is_some_and(|d| std::time::Instant::now() >= d) {
            tracing::debug!("visual embed pass hit its time budget");
            break;
        }
        if n < batch {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodestash_core::catalog::VisualEmbedJob;

    fn in_pack_job(pack_size: i64) -> VisualEmbedJob {
        VisualEmbedJob {
            asset_id: 1,
            disk_path: None,
            workspace_rel: None,
            media_type_id: 0,
            source_id: 0,
            media_code: "image".into(),
            pack_path: Some("C:/packs/P.unitypackage".into()),
            pack_offset: Some(0),
            pack_size: Some(pack_size),
            ext: Some("png".into()),
        }
    }

    /// The defect that made cloud tagging a data-destroying no-op: `OpenAiTagger` reads
    /// `TagInput::image`, `compute_tags` never fills it, so the tagger short-circuited to an empty
    /// result with no HTTP request and the caller stored that as a finished answer — clearing
    /// `tags_dirty` on assets no model had seen. Returning **no rows** is the point: `store_tags`
    /// then writes nothing and the assets stay queued for `drain_api_tags`.
    #[test]
    fn compute_tags_refuses_a_tagger_that_reads_the_image_itself() {
        let api = lodestash_ai::openai::OpenAiTagger::new("http://127.0.0.1:0", "k", "m");
        assert!(
            lodestash_ai::tagger::Tagger::needs_image(&api),
            "the API tagger sends pixels itself"
        );
        let out = compute_tags(vec![in_pack_job(1024)], &api, None, None, None);
        assert!(
            out.is_empty(),
            "an image-reading tagger must produce no writes on the batch path, \
             or its emptiness is stored as a finished result"
        );
    }

    /// The local tagger is the one this path was built for and must stay usable on it.
    #[test]
    fn the_local_tagger_still_runs_on_the_batch_path() {
        assert!(!lodestash_ai::tagger::Tagger::needs_image(
            &lodestash_ai::tagger::FilenameTagger
        ));
    }

    /// An in-pack payload over the pull cap must be refused *before* `read_payload_at` buffers it —
    /// a multi-GB asset must never be pulled wholesale into RAM by the identify drain. The guard
    /// runs ahead of any file IO, so the archive itself need not exist for the refusal.
    #[test]
    fn resolve_job_path_refuses_oversized_in_pack_payloads() {
        assert!(in_pack_pull_ok(IN_PACK_PULL_MAX_BYTES, "P.unitypackage"));
        assert!(!in_pack_pull_ok(
            IN_PACK_PULL_MAX_BYTES + 1,
            "P.unitypackage"
        ));
        assert!(
            resolve_job_path(&in_pack_job(IN_PACK_PULL_MAX_BYTES + 1), None).is_none(),
            "an over-cap payload resolves to None rather than being pulled into memory"
        );
    }

    fn subject(value: &str, confidence: f64) -> lodestash_core::catalog::TagAssignment {
        lodestash_core::catalog::TagAssignment {
            namespace: "subject".into(),
            value: value.into(),
            confidence: Some(confidence),
        }
    }

    /// A catalog holding one real PNG under a real source root, queued for tagging.
    fn catalog_with_image(dir: &std::path::Path, name: &str) -> (Catalog, i64) {
        image::RgbaImage::from_pixel(16, 16, image::Rgba([120, 110, 100, 255]))
            .save(dir.join(name))
            .expect("png");
        let mut cat = Catalog::open_in_memory().expect("open");
        cat.seed_media_types().expect("seed");
        let src = cat
            .upsert_import_source("Lib", dir.to_str().expect("utf-8 path"), "folder")
            .expect("source");
        cat.record_crawl(
            src,
            None,
            &[lodestash_core::catalog::CrawlRecord {
                workspace_rel: None,
                content_hash: None,
                rel_path: name.into(),
                name: name.into(),
                media_type: "image".into(),
                size_bytes: 16,
                mtime: 1,
            }],
        )
        .expect("crawl");
        let id = cat.assets_needing_tags_within(16, None).expect("queue")[0].asset_id;
        (cat, id)
    }

    /// The attribution defect S04 exists for: a cloud pass that fell back to the local model stored
    /// the local model's tags under the **cloud** model's id. The endpoint here is a closed local
    /// port, so the attempt fails hard at once and the filename tagger substitutes — and its tags
    /// land under its own producer and model, with nothing claimed for the endpoint.
    #[test]
    fn a_fallback_is_credited_to_the_fallback_not_the_failed_endpoint() {
        let dir = tempfile::tempdir().expect("tmp");
        let cache = tempfile::tempdir().expect("cache");
        let (mut cat, id) = catalog_with_image(dir.path(), "rock.png");
        let api = lodestash_ai::openai::OpenAiTagger::new("http://127.0.0.1:1", "k", "cloud-model");
        let progress = crate::jobs::JobProgress::default();
        drain_api_tags(
            &mut cat,
            &api,
            Some(&lodestash_ai::tagger::FilenameTagger),
            None,
            0,
            None,
            Some(cache.path()),
            &progress,
            None,
        );
        let rows = cat.asset_tag_rows(id).expect("rows");
        assert!(!rows.is_empty(), "the fallback wrote nothing");
        assert!(
            rows.iter()
                .all(|r| r.source == "ai:filename"
                    && r.model_id.as_deref() == Some("filename-stub-v1")),
            "the fallback's tags are credited to someone else: {rows:?}"
        );
        assert_eq!(
            progress.failed.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the endpoint's failure is still counted"
        );
        assert_eq!(
            cat.count_needing_tags().expect("queue"),
            0,
            "the fallback's result finishes the asset"
        );
    }

    /// A slice's VLM tags and zero-shot proposals are two producers, stored as two sources — not the
    /// zero-shot merged under the VLM's model id, which is what M26 had to mark unrecoverable.
    #[test]
    fn a_slice_stores_each_producer_under_its_own_source() {
        let dir = tempfile::tempdir().expect("tmp");
        let (mut cat, id) = catalog_with_image(dir.path(), "wall.png");
        let producers = TagProducers {
            tagger: ("ollama@localhost:11434".into(), "qwen3:8b".into()),
            zero_shot: Some(("siglip-zeroshot".into(), "siglip2-base".into())),
        };
        let stored = store_tags(
            &mut cat,
            &producers,
            &[ComputedTags {
                asset_id: id,
                caption: Some("A mossy wall.".into()),
                tags: vec![subject("wall", 0.9)],
                zero_shot: vec![subject("brick", 0.6)],
            }],
        );
        assert_eq!(stored, 2);
        let rows: Vec<_> = cat
            .asset_tag_rows(id)
            .expect("rows")
            .into_iter()
            .map(|r| (r.source, r.value, r.model_id))
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "ai:ollama@localhost:11434".to_string(),
                    "wall".to_string(),
                    Some("qwen3:8b".to_string())
                ),
                (
                    "ai:siglip-zeroshot".to_string(),
                    "brick".to_string(),
                    Some("siglip2-base".to_string())
                ),
            ]
        );
        assert_eq!(cat.count_needing_tags().expect("queue"), 0);
    }

    /// A producer id names the endpoint's host so two endpoints stay apart — and never carries the
    /// key a URL can embed.
    #[test]
    fn producer_ids_name_the_host_and_never_the_credentials() {
        assert_eq!(
            lodestash_ai::tagger::endpoint_host(
                "https://user:sk-secret@API.example.com:8443/v1/chat?k=1"
            ),
            "api.example.com:8443"
        );
        let api = lodestash_ai::openai::OpenAiTagger::new(
            "https://key:secret@api.example.com/v1",
            "k",
            "m",
        );
        let id = lodestash_ai::tagger::Tagger::producer_id(&api);
        assert_eq!(id, "openai-compatible@api.example.com");
        assert!(!id.contains("secret"));
        assert!(
            lodestash_core::catalog::source_problem(&lodestash_core::catalog::ai_source(&id))
                .is_none()
        );
    }
}
