//! The audio stages: CLAP embeddings and CLAP zero-shot tags over decoded clips, and the clip
//! length the metrics stage records.
//!
//! Both stages have the three phases the visual ones have — read a slice, compute it, store it
//! (ADR-0013) — so the idle lane can run the model off the writer. A kick runs them back to back.
use std::time::Instant;

use lodestash_ai::clap_tags::AudioZeroShot;
use lodestash_core::catalog::{Catalog, VisualEmbedJob};

use super::embed::{store_embeddings, ComputedEmbedding};
use super::plan::{past, Budget, Env, Scope};
use super::resolve::resolve_job_path;
use super::tag::{store_tags, ComputedTags, TagProducers};

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

/// Phase 1 of the audio embed stage: **read** a slice of the queue.
pub fn audio_embed_batch(cat: &Catalog, limit: usize, scope: &Scope) -> Vec<VisualEmbedJob> {
    cat.audio_assets_needing_embedding_within(limit, scope)
        .unwrap_or_else(|e| {
            tracing::warn!(%e, "audio embed queue query failed; retries next pass");
            Vec::new()
        })
}

/// Phase 2: **compute** — decode → 48 kHz mono → log-mel → the audio tower. No catalog, so it runs
/// on any thread. An undecodable clip gets a zero vector, which scores 0 and never surfaces, so
/// the queue always advances (the visual stage's rule).
pub fn compute_audio_embeddings(
    jobs: Vec<VisualEmbedJob>,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    workspace_dir: Option<&std::path::Path>,
) -> Vec<ComputedEmbedding> {
    jobs.into_iter()
        .map(|job| {
            // In-pack payloads pull to a scratch file; keep the guard alive while `path` is read.
            let resolved = resolve_job_path(&job, workspace_dir);
            let vector = resolved
                .as_ref()
                .and_then(|(p, _)| {
                    decode_audio_48k_mono(p)
                        .map_err(|e| tracing::warn!(%e, "audio decode failed"))
                        .ok()
                })
                .map(|samples| {
                    let (frames, mel) = lodestash_ingest::melspec::log_mel_spectrogram(&samples);
                    emb.embed_audio(&mel, frames)
                })
                .unwrap_or_else(|| vec![0.0; emb.dim()]);
            ComputedEmbedding {
                asset_id: job.asset_id,
                media_type_id: job.media_type_id,
                source_id: job.source_id,
                modality: "audio",
                vector,
            }
        })
        .collect()
}

/// The audio embed stage inline — the kick path. One clip at a time, so `deadline` is tested per
/// clip: a decode plus a CLAP pass is seconds, and a 32-clip batch overruns any budget worth
/// setting.
pub fn drain_audio_embeddings(
    cat: &mut Catalog,
    emb: &(dyn lodestash_ai::embedder::Embedder + Send + Sync),
    scope: &Scope,
    workspace_dir: Option<&std::path::Path>,
    deadline: Option<Instant>,
) {
    const BATCH: usize = 32;
    let mut total = 0usize;
    'queue: loop {
        let jobs = audio_embed_batch(cat, BATCH, scope);
        let n = jobs.len();
        for job in jobs {
            if past(deadline) {
                tracing::debug!(embedded = total, "audio embed pass hit its time budget");
                break 'queue;
            }
            let done = compute_audio_embeddings(vec![job], emb, workspace_dir);
            if store_embeddings(cat, emb.model_id(), &done) < done.len() {
                break 'queue; // a write failed — it logged, and the queue is durable
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

/// Phase 1 of the audio tag stage: **read** a slice of the queue.
pub fn audio_tag_batch(cat: &Catalog, limit: usize, scope: &Scope) -> Vec<VisualEmbedJob> {
    cat.audio_assets_needing_tags_within(limit, scope)
        .unwrap_or_else(|e| {
            tracing::warn!(%e, "audio tag queue query failed; retries next pass");
            Vec::new()
        })
}

/// Phase 2: **compute** — decode → log-mel → score against the label vocabulary in the joint
/// space the embeddings use (AI-PIPELINE §6). A clip that yields nothing is still a result: its
/// empty set finishes it, so it is never re-queued for ever.
pub fn compute_audio_tags(
    jobs: Vec<VisualEmbedJob>,
    zero_shot: &AudioZeroShot,
    workspace_dir: Option<&std::path::Path>,
) -> Vec<ComputedTags> {
    jobs.into_iter()
        .map(|job| {
            let resolved = resolve_job_path(&job, workspace_dir);
            let tags = resolved
                .as_ref()
                .and_then(|(p, _)| {
                    decode_audio_48k_mono(p)
                        .map_err(|e| tracing::warn!(%e, "audio decode failed"))
                        .ok()
                })
                .map(|samples| {
                    let (frames, mel) = lodestash_ingest::melspec::log_mel_spectrogram(&samples);
                    zero_shot.tag(&mel, frames)
                })
                .unwrap_or_default();
            ComputedTags {
                asset_id: job.asset_id,
                caption: None,
                tags,
                zero_shot: Vec::new(),
            }
        })
        .collect()
}

/// Who an audio tag slice is stored under: the CLAP zero-shot producer, alone.
pub fn audio_tag_producers(zero_shot: &AudioZeroShot) -> TagProducers {
    TagProducers {
        tagger: (
            zero_shot.producer_id().to_string(),
            zero_shot.model_id().to_string(),
        ),
        zero_shot: None,
    }
}

/// The audio tag stage inline — the kick path. Its own queue, and the **only** stage that
/// finishes an audio asset's tags (review B5). One clip at a time: `max_assets` is exact and
/// `deadline` is tested per clip, because a count says nothing about how long 48 decodes take.
pub fn drain_audio_tags(
    cat: &mut Catalog,
    zero_shot: &AudioZeroShot,
    scope: &Scope,
    workspace_dir: Option<&std::path::Path>,
    progress: &crate::jobs::JobProgress,
    // `0` = no cap.
    max_assets: u32,
    deadline: Option<Instant>,
) {
    use std::sync::atomic::Ordering;
    const BATCH: usize = 32;
    let producers = audio_tag_producers(zero_shot);
    let mut tagged = 0usize;
    loop {
        let jobs = audio_tag_batch(cat, BATCH, scope);
        let n = jobs.len();
        for job in jobs {
            if progress.is_cancelled() || (max_assets != 0 && tagged >= max_assets as usize) {
                return;
            }
            if past(deadline) {
                tracing::debug!("audio tag pass hit its time budget");
                return;
            }
            let done = compute_audio_tags(vec![job], zero_shot, workspace_dir);
            // Finished by the write, not by a tag count: a clip the model said nothing about
            // stores an empty set and is done.
            if store_tags(cat, &producers, &done).is_none() {
                return;
            }
            progress.done.fetch_add(1, Ordering::Relaxed);
            tagged += 1;
        }
        if n < BATCH {
            break;
        }
    }
}

/// The audio tag stage of a pass. The label vocabulary is encoded only when a clip is queued:
/// building the tagger is one text pass per label, and most passes have no audio to tag.
pub(super) fn tag_stage(
    cat: &mut Catalog,
    scope: &Scope,
    env: &Env<'_>,
    budget: Budget,
    deadline: Option<Instant>,
) {
    let Some(emb) = env.audio_embedder.clone() else {
        return;
    };
    if audio_tag_batch(cat, 1, scope).is_empty() {
        return;
    }
    let Some(zero_shot) = AudioZeroShot::new(emb) else {
        return;
    };
    drain_audio_tags(
        cat,
        &zero_shot,
        scope,
        env.workspace_dir,
        env.progress,
        budget.audio_tagged,
        deadline,
    );
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
