//! The cloud tag stage: one OpenAI-compatible request per asset, behind the egress gate, with
//! backoff, a per-run cap and a local fallback (AI-PIPELINE §5).
use lodestash_core::catalog::Catalog;

use super::plan::Scope;
use super::resolve::resolve_job_path;
use super::tag::{tag_preview_with, write_groups, zeroshot_tags};

/// Tag dirty **visual** assets through an OpenAI-compatible cloud VLM (AI-PIPELINE §5). One call per
/// asset — a capable cloud model holds the schema, so no local caption pre-step. Single pass (no
/// weight-residency concern), over the generic tag queue — which never holds audio (review B5).
/// Forward progress preserved: a preview that won't render still clears the flag.
pub fn drain_api_tags(
    cat: &mut Catalog,
    // The consent this pass runs under. Taken from the caller rather than re-read here: the gate
    // below decides whether any bytes leave, and a pass must be judged by the settings it was
    // started with, not by whatever the file says by the time it reaches this line.
    cfg: &crate::config::AppConfig,
    tagger: &lodestash_ai::openai::OpenAiTagger,
    fallback: Option<&dyn lodestash_ai::tagger::Tagger>,
    zero_shot: Option<&lodestash_ai::image_tags::ImageZeroShot>,
    max_assets: u32,
    workspace_dir: Option<&std::path::Path>,
    cache_dir: Option<&std::path::Path>,
    progress: &crate::jobs::JobProgress,
    // Confined to the pass's scope, exactly as [`super::drain_tags`] is. Without it an
    // `identify_asset(ids)` routed to the cloud would take the front of the **global** queue
    // instead of the ids asked about — the same defect already fixed once on the local path, and
    // far more expensive here because every wrong asset is a paid API call.
    scope: &Scope,
) {
    use lodestash_ai::openai::ApiError;
    use lodestash_ai::tagger::{TagInput, Tagger};
    use std::sync::atomic::Ordering;
    const BATCH: usize = 16;
    const TAG_PX: u32 = 384;
    // Asked once, before a single preview is rendered or a request built (v1 plan S10): a remote
    // endpoint without consent for image bytes means this pass makes no outbound request at all,
    // rather than sending and reporting an error afterwards.
    let endpoint = tagger.base_url().to_string();
    if let Err(denied) = crate::egress::may_send(cfg, &endpoint, crate::egress::Modality::Image) {
        tracing::warn!(%denied, "cloud tagging pass refused before any request");
        return;
    }
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
        let jobs = match cat.assets_needing_tags_within(BATCH, scope) {
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
            // A kind this endpoint cannot look at (audio never reaches here — it is not in this
            // queue): clear the flag with an empty set, as the local drain does, rather than
            // skipping — skipping would return the same batch forever.
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
            let slots = cat
                .model_slot_texture_paths(job.asset_id, workspace_dir)
                .unwrap_or_default();
            let Some(img) = path
                .as_deref()
                .zip(cache_dir)
                .and_then(|(p, cache)| tag_preview_with(p, cache, TAG_PX, &slots))
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
