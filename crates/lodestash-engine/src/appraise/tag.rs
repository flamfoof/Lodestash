//! The generic tag stage: read a slice of the queue, caption and tag it, store each producer's
//! tags under its own source (ADR-0017). Three phases, so the inference can run off the writer
//! (ADR-0013).
use lodestash_core::catalog::Catalog;

use super::resolve::resolve_job_path;

/// Zero-shot tags for one asset (AI-PIPELINE §2.1/§3). For a `model_3d` asset it scores the stored
/// **multiview** embedding (four orbit views pooled by the embedding drain) rather than the single
/// rendered thumbnail — a better basis, at no extra render cost — when that vector exists and came
/// from this tagger's own embedder. Every other case scores the decoded preview.
pub(super) fn zeroshot_tags(
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
pub(super) fn write_groups(
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
            expected_revision: None,
        })
        .collect();
    match cat.apply_tag_batch(&TagBatch {
        items,
        completes_queue: true,
        history: Some(HistoryOrigin::Background),
        receipt: None,
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
    // What each model wears, resolved by the read phase (v1 plan S08): this phase never sees a
    // catalog, which is the point of the split.
    slot_textures: &SlotTextures,
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
            let empty = std::collections::BTreeMap::new();
            let slots = slot_textures.get(&job.asset_id).unwrap_or(&empty);
            path.as_deref()
                .zip(cache_dir)
                .and_then(|(p, cache)| tag_preview_with(p, cache, TAG_PX, slots))
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
            expected_revision: None,
        });
        if let Some((producer, model)) = &producers.zero_shot {
            items.push(TagBatchItem {
                asset_id: r.asset_id,
                source: ai_source(producer),
                model_id: Some(model.clone()),
                mode: TagMode::ReplaceSource,
                tags: r.zero_shot.clone(),
                description: None,
                expected_revision: None,
            });
        }
    }
    match cat.apply_tag_batch(&TagBatch {
        items,
        completes_queue: true,
        history: Some(HistoryOrigin::Background),
        receipt: None,
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

/// Tag the `tags_dirty` backlog, one asset at a time, on the caller's sole-writer connection.
///
/// The preview handed to a vision model is the **thumbnail cache's own PNG** — the same pixels the
/// grid shows, produced by the same `thumb::generate` (decode for 2D, hero render for 3D), so the
/// model sees what the user sees and no second rendering path exists to drift.
///
/// Forward progress is unconditional: an asset the tagger refuses (audio), one whose preview can't
/// be made, and one the model answers nothing for all still clear `tags_dirty` via an empty write.
/// Otherwise the queue would hand back the same rows forever.
///
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
        let slots = slot_textures_for(&jobs, |id| {
            cat.model_slot_texture_paths(id, workspace_dir).ok()
        });
        let done = compute_tags(jobs, tagger, zero_shot, workspace_dir, cache_dir, &slots);
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
    tag_preview_with(source, cache_dir, px, &std::collections::BTreeMap::new())
}

/// [`tag_preview`] painted with what the model wears (v1 plan S08): the tagger is looking for
/// "rusty barrel", and the rust is in the texture. A model nothing paints, or one too big to
/// rasterise whole, falls back to the untextured render rather than losing its preview.
pub fn tag_preview_with(
    source: &std::path::Path,
    cache_dir: &std::path::Path,
    px: u32,
    slot_textures: &std::collections::BTreeMap<u32, std::path::PathBuf>,
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
    let png = lodestash_ingest::thumb::generate_model_textured(
        source,
        cache_dir,
        &key,
        params,
        slot_textures,
    )
    .or_else(|_| lodestash_ingest::thumb::generate(source, cache_dir, &key, params))
    .ok()?;
    Some(image::open(png).ok()?.to_rgba8())
}

/// What each asset's model wears, by asset id: the read phase resolves it, the compute phase paints
/// with it (v1 plan S08).
pub type SlotTextures =
    std::collections::HashMap<i64, std::collections::BTreeMap<u32, std::path::PathBuf>>;

/// The textures the models in `jobs` wear, for the compute phase that has no catalog.
pub fn slot_textures_for(
    jobs: &[lodestash_core::catalog::VisualEmbedJob],
    slots_of: impl Fn(i64) -> Option<std::collections::BTreeMap<u32, std::path::PathBuf>>,
) -> SlotTextures {
    jobs.iter()
        .filter(|j| j.media_code == "model_3d")
        .filter_map(|j| {
            let slots = slots_of(j.asset_id).filter(|s| !s.is_empty())?;
            Some((j.asset_id, slots))
        })
        .collect()
}
