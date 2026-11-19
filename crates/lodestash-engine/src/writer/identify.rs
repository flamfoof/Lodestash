//! Appraisal on the writer thread: the kicks, and the idle lane.
//!
//! Both build an [`AppraisePlan`] and hand it to [`crate::appraise::run`], which orders the
//! stages. They differ in who does the inference. A kick runs every stage inline — the user asked
//! and is watching a bar. The idle lane keeps the writer for the cheap ends only: it reads a
//! slice of each model stage's queue, hands the compute to a worker thread, and stores the result
//! when it comes back as a [`WriterMsg`] (ADR-0013).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use lodestash_ai::clap_tags::AudioZeroShot;
use lodestash_ai::image_tags::ImageZeroShot;
use lodestash_core::catalog::Catalog;

use super::{spawn_slice, Inflight, WriteResult, WriterCtx, WriterMsg};
use crate::appraise::{
    self, AppraisePlan, Appraised, Env, Offload, SharedEmbedder, Stage, TagProducers,
};

/// How often the idle lane takes a turn: one tick in four, each tick a second (ADR-0014).
///
/// Every tick used to do a slice, which is a 100% duty cycle dressed up as "idle work" — an
/// app showing a grid and being asked for nothing sat at 0.57 cores indefinitely. Four is the
/// tradeoff: the backlog converges ~4× slower in wall-clock terms, and the machine stays the
/// user's. Raise it to be politer, lower it to drain faster.
pub(super) const IDLE_DUTY_PERIOD: u32 = 4;

/// How many assets one off-writer slice takes, per model stage. Small: a slice is computed
/// against the catalog as it was when the slice was read, and one unit is seconds of inference.
const EMBED_SLICE: usize = 8;
const TAG_SLICE: usize = 4;

/// One pass on the writer thread, under the settings and focus of this moment.
fn appraise(
    catalog: &mut Catalog,
    ctx: &WriterCtx,
    plan: &AppraisePlan,
    embedder: Option<SharedEmbedder>,
    audio_embedder: Option<SharedEmbedder>,
    offload: Option<&mut dyn Offload>,
) -> Appraised {
    // UI-SPEC §3: what the user is looking at goes first. Refreshed here rather than by each
    // caller because every pass — the idle slice, ✦ Identify all, a per-card kick — must read the
    // *current* focus: the one thing that makes the backlog follow the user is that it is re-read
    // after they have moved. A poisoned lock is not worth failing an appraisal over; the queue
    // then keeps whatever order it had.
    if let Ok(focus) = ctx.focus.lock() {
        catalog.set_appraise_focus(focus.clone());
    }
    // The B11d surface the Identify screen polls: reset when a pass starts, filled by the tag
    // stages as they work.
    let progress = ctx.appraise.as_ref();
    progress.done.store(0, Ordering::Relaxed);
    progress.total.store(0, Ordering::Relaxed);
    progress.failed.store(0, Ordering::Relaxed);
    progress.cancelled.store(false, Ordering::Relaxed);
    // Read once: a pass is judged by the settings — the consent — it started with.
    let config = ctx.config.get();
    let env = Env {
        workspace_dir: ctx.workspace_dir.as_deref(),
        cache_dir: ctx.cache_dir.as_deref(),
        config: &config,
        embedder,
        audio_embedder,
        progress,
        graph_due: &ctx.graph_due,
    };
    tracing::debug!(scope = ?plan.scope, stages = ?plan.stages, origin = ?plan.origin, "appraise pass");
    appraise::run(catalog, plan, &env, offload)
}

/// A kick (✦ Identify all, a per-card ✦, an agent's `identify_asset`): the whole plan inline.
///
/// The embedders are resolved here even when auto-identify is off — someone asked. With no local
/// model the pass still measures, scans and tags, and the model stages leave their assets queued
/// for a pass that has one.
pub(super) fn kick(
    catalog: &mut Catalog,
    ctx: &WriterCtx,
    plan: &AppraisePlan,
) -> Result<WriteResult, String> {
    let (embedder, audio_embedder) = (ctx.models.visual(), ctx.models.audio());
    if embedder.is_none() && audio_embedder.is_none() {
        tracing::warn!(
            scope = ?plan.scope,
            "identify: no local embedder models — nothing can be embedded in this pass"
        );
    }
    let done = appraise(catalog, ctx, plan, embedder, audio_embedder, None);
    // changed = the appraised backlog the pass reconciled; when nothing was promoted report 1 so
    // the frontend toasts a real kick, not a silent no-op.
    Ok(WriteResult {
        changed: done
            .counts
            .map(|c| c.identified + c.unidentified)
            .unwrap_or(1),
        promoted: done.promoted,
        ..Default::default()
    })
}

/// The backlog lane: what the writer does with a second in which nothing was asked of it.
///
/// It always measures — metrics, dependencies, previews and the status lifecycle are not
/// identification and need no model (AI-PIPELINE §2). It identifies when `auto_identify` is on
/// and a model is installed, and then every model stage runs off this thread, one slice of each
/// in flight at a time.
pub(super) struct IdleLane {
    /// Resolved once, when the writer starts, and only if auto-identify is on: model files load
    /// only for a user who opted in, and are borrowed by every slice after that.
    embedder: Option<SharedEmbedder>,
    audio_embedder: Option<SharedEmbedder>,
    /// Built by the first audio tag slice, on its worker, and kept: the label vocabulary is one
    /// text pass per label and does not change.
    audio_zero_shot: Arc<OnceLock<Option<Arc<AudioZeroShot>>>>,
    /// Up while a slice of that stage is out with a worker; lowered when its result lands here,
    /// or by the slice itself if it never will ([`Inflight`]).
    in_flight: [Arc<AtomicBool>; 4],
    tick: u32,
}

impl IdleLane {
    pub(super) fn new(ctx: &WriterCtx) -> Self {
        let (embedder, audio_embedder) = if ctx.config.read(|c| c.auto_identify) {
            (ctx.models.visual(), ctx.models.audio())
        } else {
            (None, None)
        };
        Self {
            embedder,
            audio_embedder,
            audio_zero_shot: Arc::default(),
            in_flight: Default::default(),
            tick: 0,
        }
    }

    /// Does this lane identify, or only measure?
    fn identifies(&self) -> bool {
        self.embedder.is_some() || self.audio_embedder.is_some()
    }

    fn flag(&self, stage: Stage) -> &Arc<AtomicBool> {
        &self.in_flight[match stage {
            Stage::VisualEmbed => 0,
            Stage::AudioEmbed => 1,
            Stage::Tags => 2,
            _ => 3,
        }]
    }

    /// A quiet second passed. The lane works one tick in [`IDLE_DUTY_PERIOD`]; a skipped tick
    /// costs nothing, and one counter throttles every stage at once — the off-writer slices are
    /// only ever started from a working turn.
    pub(super) fn tick(&mut self, catalog: &mut Catalog, ctx: &WriterCtx, pump: &AtomicBool) {
        self.tick = self.tick.wrapping_add(1);
        if self.tick.is_multiple_of(IDLE_DUTY_PERIOD) {
            self.work(catalog, ctx, pump);
        }
    }

    /// One working turn: a second of backfill on this thread, and a slice of each model stage
    /// sent to its worker.
    pub(super) fn work(&mut self, catalog: &mut Catalog, ctx: &WriterCtx, pump: &AtomicBool) {
        let identifies = self.identifies();
        let plan = AppraisePlan::idle(identifies);
        // The pump is "an identification pass is running" (B11d); measuring alone is not one.
        pump.store(identifies, Ordering::Relaxed);
        let (embedder, audio_embedder) = (self.embedder.clone(), self.audio_embedder.clone());
        let mut slices = Slices { lane: self, ctx };
        // Its outcome is the catalog's: the writer marks the turn by what it wrote.
        appraise(
            catalog,
            ctx,
            &plan,
            embedder,
            audio_embedder,
            Some(&mut slices),
        );
        pump.store(false, Ordering::Relaxed);
    }

    /// A slice of `stage` came back and was stored (or its store failed): the stage may send
    /// the next one.
    pub(super) fn landed(&self, stage: Stage) {
        self.flag(stage).store(false, Ordering::Relaxed);
    }
}

/// The idle lane as the plan's [`Offload`]: each model stage becomes one slice on a worker.
struct Slices<'a> {
    lane: &'a IdleLane,
    ctx: &'a WriterCtx,
}

impl Offload for Slices<'_> {
    fn offload(&mut self, stage: Stage, catalog: &Catalog, plan: &AppraisePlan, env: &Env<'_>) {
        let flag = self.lane.flag(stage);
        if flag.load(Ordering::Relaxed) {
            return; // the last slice of this stage is still out
        }
        let ctx = self.ctx;
        let (ws, cache) = (ctx.workspace_dir.clone(), ctx.cache_dir.clone());
        match stage {
            Stage::VisualEmbed => {
                let Some(emb) = env.embedder.clone() else {
                    return;
                };
                let (jobs, albedo) =
                    appraise::visual_embed_batch(catalog, EMBED_SLICE, &plan.scope);
                if jobs.is_empty() {
                    return;
                }
                spawn_slice(Inflight::raise(flag), ctx.tx.clone(), move || {
                    let done = appraise::compute_visual_embeddings(
                        jobs,
                        &albedo,
                        emb.as_ref(),
                        ws.as_deref(),
                        cache.as_deref(),
                    );
                    Some(WriterMsg::EmbeddingsDone {
                        stage,
                        model_id: emb.model_id().to_string(),
                        done,
                    })
                });
            }
            Stage::AudioEmbed => {
                let Some(emb) = env.audio_embedder.clone() else {
                    return;
                };
                let jobs = appraise::audio_embed_batch(catalog, EMBED_SLICE, &plan.scope);
                if jobs.is_empty() {
                    return;
                }
                spawn_slice(Inflight::raise(flag), ctx.tx.clone(), move || {
                    let done =
                        appraise::compute_audio_embeddings(jobs, emb.as_ref(), ws.as_deref());
                    Some(WriterMsg::EmbeddingsDone {
                        stage,
                        model_id: emb.model_id().to_string(),
                        done,
                    })
                });
            }
            Stage::AudioTags => {
                let Some(emb) = env.audio_embedder.clone() else {
                    return;
                };
                let jobs = appraise::audio_tag_batch(catalog, TAG_SLICE, &plan.scope);
                if jobs.is_empty() {
                    return;
                }
                let zero_shot = self.lane.audio_zero_shot.clone();
                spawn_slice(Inflight::raise(flag), ctx.tx.clone(), move || {
                    let zero_shot = zero_shot
                        .get_or_init(|| AudioZeroShot::new(emb).map(Arc::new))
                        .clone()?;
                    Some(WriterMsg::TagsDone {
                        stage,
                        producers: appraise::audio_tag_producers(&zero_shot),
                        done: appraise::compute_audio_tags(jobs, &zero_shot, ws.as_deref()),
                    })
                });
            }
            Stage::Tags => {
                // **The idle lane never spends money.** A cloud pass is always an explicit,
                // scoped, capped kick (AI-PIPELINE §5.1): a background lane must not quietly
                // bill an API, and the endpoint drain writes through the catalog, which a
                // worker does not have.
                if appraise::cloud_configured(env.config) {
                    return;
                }
                let jobs = appraise::tag_batch(catalog, TAG_SLICE, &plan.scope);
                if jobs.is_empty() {
                    return;
                }
                let db = ctx.db_path.clone();
                let emb = env.embedder.clone();
                let config = env.config.clone();
                spawn_slice(Inflight::raise(flag), ctx.tx.clone(), move || {
                    tag_slice(&db, jobs, emb, ws, cache, &config)
                });
            }
            // Not model stages: the plan runs them inline and never offers them here.
            Stage::Metrics | Stage::Dependencies | Stage::Previews | Stage::Promote => {}
        }
    }
}

/// Compute one idle slice of the generic tag stage off the writer thread: the [`WriterMsg`] for
/// the writer, or `None` when there is nothing to store. [`spawn_slice`] sends it and keeps the
/// in-flight flag honest whichever it is — or if this panics.
fn tag_slice(
    db_path: &std::path::Path,
    jobs: Vec<lodestash_core::catalog::VisualEmbedJob>,
    embedder: Option<SharedEmbedder>,
    workspace_dir: Option<std::path::PathBuf>,
    cache_dir: Option<std::path::PathBuf>,
    config: &crate::config::AppConfig,
) -> Option<WriterMsg> {
    // A read connection, for the user's taught vocabulary and what each model wears — WAL admits
    // any number of readers, and every write still lands on the writer thread (ADR-0002).
    // Read-only by type: this used to be the migrating `Catalog::open` (review R5).
    let Ok(cat) = Catalog::open_reader(db_path) else {
        tracing::warn!("tag worker could not open a read connection; slice skipped");
        return None;
    };
    let teach = appraise::teaching(cat.teaching_traits().unwrap_or_default());
    let zero_shot = embedder.and_then(|e| ImageZeroShot::new(e, &teach));
    let tagger = appraise::local_tagger(config, &teach);
    // Resolved here, on the read connection: the compute phase runs with no catalog to ask what
    // a model wears (v1 plan S08).
    let slots = appraise::slot_textures_for(&jobs, |id| {
        cat.model_slot_texture_paths(id, workspace_dir.as_deref())
            .ok()
    });
    let done = appraise::compute_tags(
        jobs,
        tagger.as_ref(),
        zero_shot.as_ref(),
        workspace_dir.as_deref(),
        cache_dir.as_deref(),
        &slots,
    );
    Some(WriterMsg::TagsDone {
        stage: Stage::Tags,
        producers: TagProducers {
            tagger: (tagger.producer_id(), tagger.model_id().to_string()),
            zero_shot: zero_shot
                .as_ref()
                .map(|z| (z.producer_id().to_string(), z.model_id().to_string())),
        },
        done,
    })
}
