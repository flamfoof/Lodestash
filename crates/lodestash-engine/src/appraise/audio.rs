//! The audio stages: CLAP embeddings and CLAP zero-shot tags over decoded clips, and the clip
//! length the metrics stage records.
use lodestash_core::catalog::Catalog;

use super::resolve::resolve_job_path;
use super::tag::write_groups;

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

/// Clip length via the decoder the player already uses. `total_duration` answers from the header
/// for WAV/FLAC; formats that can't (some MP3s) report `None`, which stays "unknown" rather than
/// paying for a full decode just to fill a stat row.
pub fn audio_duration_ms(path: &std::path::Path) -> Option<i64> {
    use rodio::Source;
    let file = std::fs::File::open(path).ok()?;
    let dec = rodio::Decoder::new(std::io::BufReader::new(file)).ok()?;
    Some(dec.total_duration()?.as_millis() as i64)
}
