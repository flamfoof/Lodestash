//! The appraisal plan (v2 spec §4.6): which assets a pass works on, which stages it runs, how
//! long it may take and who asked. Callers build a plan; [`run`] is the one place that decides
//! the order stages run in.
//!
//! It replaces one parameter that meant three things. `only: Option<&[i64]>` was the *scope* on a
//! kick, *"skip the library-wide backfills"* inside the pass, and — as `Some(&[])` on the idle
//! tick — *"do not tag inline"*. The last two were side effects of the first, which is how the
//! idle lane came to run no backfill at all (review B4).

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

pub use lodestash_core::catalog::AppraiseScope as Scope;
use lodestash_core::catalog::{Catalog, StatusCounts};

use super::{
    audio, deps::drain_dependencies, embed::drain_image_embeddings, metrics::drain_metrics,
    preview::drain_previews, tag, SharedEmbedder,
};
use crate::changes::Origin;
use crate::config::AppConfig;
use crate::jobs::JobProgress;

/// One step of appraisal. A plan runs the ones it names, always in [`Stage::ORDER`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Technical facets and the perceptual hash — and, for a model, its vector preview while the
    /// mesh is parsed anyway.
    Metrics,
    /// GUIDs, declared references, prefab evidence.
    Dependencies,
    /// Vector previews for the models the metrics stage will not revisit.
    Previews,
    /// CLAP embeddings.
    AudioEmbed,
    /// CLAP zero-shot tags. The only stage that tags audio.
    AudioTags,
    /// SigLIP embeddings: pictures, models, materials.
    VisualEmbed,
    /// The generic tag stage — a local model or an endpoint. Never audio.
    Tags,
    /// The status lifecycle (DATA-MODEL §8).
    Promote,
}

impl Stage {
    /// The order every pass runs its stages in (v2 spec §4.6). No caller orders stages.
    ///
    /// Measurements first: they need no model, and metrics is what moves an asset from
    /// `discovered` to `indexed`. Dependencies before previews and embeddings, so a material's
    /// textures are known by the time something paints with them. The audio stages before the
    /// generic tag stage (review B5), and promotion last, because it reads what the others wrote.
    pub const ORDER: [Stage; 8] = [
        Stage::Metrics,
        Stage::Dependencies,
        Stage::Previews,
        Stage::AudioEmbed,
        Stage::AudioTags,
        Stage::VisualEmbed,
        Stage::Tags,
        Stage::Promote,
    ];

    /// Does this stage run a model? In the idle lane these leave the writer thread (ADR-0013).
    pub fn is_inference(self) -> bool {
        matches!(
            self,
            Stage::AudioEmbed | Stage::AudioTags | Stage::VisualEmbed | Stage::Tags
        )
    }

    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// A set of stages. It has no order of its own: [`Stages::in_order`] is [`Stage::ORDER`].
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Stages(u8);

impl Stages {
    /// What needs no model and says nothing about what an asset *is*: metrics, dependencies,
    /// previews. These run whether or not identification is automatic (AI-PIPELINE §2).
    pub const BACKFILL: Stages =
        Stages::of(&[Stage::Metrics, Stage::Dependencies, Stage::Previews]);
    /// Embeddings and tags — identification proper, what `auto_identify` gates.
    pub const IDENTIFY: Stages = Stages::of(&[
        Stage::AudioEmbed,
        Stage::AudioTags,
        Stage::VisualEmbed,
        Stage::Tags,
    ]);
    pub const ALL: Stages = Stages::of(&Stage::ORDER);

    pub const fn of(stages: &[Stage]) -> Stages {
        let mut bits = 0;
        let mut i = 0;
        while i < stages.len() {
            bits |= stages[i].bit();
            i += 1;
        }
        Stages(bits)
    }

    pub const fn union(self, other: Stages) -> Stages {
        Stages(self.0 | other.0)
    }

    pub const fn with(self, stage: Stage) -> Stages {
        Stages(self.0 | stage.bit())
    }

    pub fn contains(self, stage: Stage) -> bool {
        self.0 & stage.bit() != 0
    }

    /// The stages of this set, in the one order.
    pub fn in_order(self) -> impl Iterator<Item = Stage> {
        Stage::ORDER.into_iter().filter(move |s| self.contains(*s))
    }
}

impl std::fmt::Debug for Stages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.in_order()).finish()
    }
}

/// How long the timed stages may run. Checked **per unit** of work, never per batch (ADR-0013):
/// a deadline tested once per sixteen renders is not a deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeBudget {
    /// Until every queue in scope is empty.
    Unbounded,
    /// Each stage gets this long: a kick. Someone asked and is watching a bar, and one slow
    /// stage must not take the others' turn.
    PerStage(Duration),
    /// The whole pass gets this long: an idle slice. The writer is only borrowed, so the bound
    /// is on the pass — the stages take it in order, and what is left waits for the next slice.
    Pass(Duration),
}

/// What a pass may spend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Wall clock for every stage but the generic tag stage, whose unit is a whole model call —
    /// seconds each — and whose bound is therefore the count below (AI-PIPELINE §5.1).
    pub time: TimeBudget,
    /// Assets the generic tag stage may finish in one pass. `0` = no cap. A cloud pass is capped
    /// by the user's own `api_max_per_run` instead.
    pub tagged: u32,
    /// Clips the audio tag stage may finish in one pass. `0` = no cap.
    pub audio_tagged: u32,
}

impl Budget {
    /// Drain every queue in scope.
    pub const UNBOUNDED: Budget = Budget {
        time: TimeBudget::Unbounded,
        tagged: 0,
        audio_tagged: 0,
    };

    /// A kick on the writer thread: bounded, so a long backlog never holds every other write for
    /// the length of the library. What is left stays queued for the next kick or the idle lane.
    pub const KICK: Budget = Budget {
        time: TimeBudget::PerStage(Duration::from_secs(20)),
        tagged: 64,
        audio_tagged: 48,
    };

    /// One idle slice: a second of the writer, all stages together.
    pub const IDLE: Budget = Budget {
        time: TimeBudget::Pass(Duration::from_secs(1)),
        tagged: 0,
        audio_tagged: 0,
    };
}

/// One appraisal pass, as its caller wants it (v2 spec §4.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppraisePlan {
    /// The assets every stage is confined to.
    pub scope: Scope,
    pub stages: Stages,
    pub budget: Budget,
    /// Who asked.
    pub origin: Origin,
}

impl AppraisePlan {
    /// A kick: every stage, on what the caller named.
    pub fn kick(scope: Scope, origin: Origin) -> Self {
        Self {
            scope,
            stages: Stages::ALL,
            budget: Budget::KICK,
            origin,
        }
    }

    /// The idle lane's slice of the whole library: every backfill stage and the status
    /// lifecycle always (review B4), identification when it is automatic.
    pub fn idle(identify: bool) -> Self {
        let stages = Stages::BACKFILL.with(Stage::Promote);
        Self {
            scope: Scope::All,
            stages: if identify {
                stages.union(Stages::IDENTIFY)
            } else {
                stages
            },
            budget: Budget::IDLE,
            origin: Origin::Background,
        }
    }
}

/// What a pass reads besides the catalog.
pub struct Env<'a> {
    pub workspace_dir: Option<&'a Path>,
    pub cache_dir: Option<&'a Path>,
    /// The settings this pass runs under, read once when it starts. A pass is judged by the
    /// consent it was started with (ADR-0020), not by what the file says when it reaches a line.
    pub config: &'a AppConfig,
    /// The pixel space, or `None` when no model is installed: the visual stages then do nothing
    /// and their assets stay queued.
    pub embedder: Option<SharedEmbedder>,
    /// The audio space, likewise.
    pub audio_embedder: Option<SharedEmbedder>,
    pub progress: &'a JobProgress,
    /// Raised by a pass that harvested dependencies without reconciling the library's graph —
    /// a scoped pass, or one whose time ran out — and lowered by the pass that does.
    pub graph_due: &'a AtomicBool,
}

/// Takes a model stage off the calling thread (ADR-0013): read a slice here, compute it on a
/// worker, store it when the result comes back. The idle lane has one; a kick has none and runs
/// every stage inline, because there the user asked and is watching a bar.
pub trait Offload {
    fn offload(&mut self, stage: Stage, catalog: &Catalog, plan: &AppraisePlan, env: &Env<'_>);
}

/// What a pass did.
#[derive(Debug, Default)]
pub struct Appraised {
    /// The stages that ran, in the order they ran.
    pub ran: Vec<Stage>,
    /// The assets this pass promoted to `identified`.
    pub promoted: Vec<i64>,
    /// The library's status counts after a pass that promoted something.
    pub counts: Option<StatusCounts>,
}

/// Run `plan`: its stages, in [`Stage::ORDER`], each confined to its scope.
///
/// On the catalog's one writing connection, so on the writer thread. With an `offload` the
/// model stages are handed to it instead of run here.
pub fn run(
    cat: &mut Catalog,
    plan: &AppraisePlan,
    env: &Env<'_>,
    mut offload: Option<&mut dyn Offload>,
) -> Appraised {
    let mut out = Appraised::default();
    // An explicit empty set is no assets — not one query is worth running for it.
    if plan.scope.is_empty() {
        return out;
    }
    let scope = &plan.scope;
    let (ws, cache) = (env.workspace_dir, env.cache_dir);
    let pass_deadline = match plan.budget.time {
        TimeBudget::Pass(d) => Some(Instant::now() + d),
        _ => None,
    };
    for stage in plan.stages.in_order() {
        let deadline = match plan.budget.time {
            TimeBudget::Unbounded => None,
            TimeBudget::PerStage(d) => Some(Instant::now() + d),
            TimeBudget::Pass(_) => pass_deadline,
        };
        out.ran.push(stage);
        if stage.is_inference() {
            if let Some(lane) = offload.as_deref_mut() {
                lane.offload(stage, cat, plan, env);
                continue;
            }
        }
        match stage {
            Stage::Metrics => {
                let proxy = env.config.preview_quality.proxy_budget();
                drain_metrics(cat, scope, ws, cache, proxy, deadline);
            }
            Stage::Dependencies => drain_dependencies(cat, scope, ws, deadline, env.graph_due),
            Stage::Previews => {
                let proxy = env.config.preview_quality.proxy_budget();
                drain_previews(cat, scope, ws, cache, proxy, deadline);
            }
            Stage::AudioEmbed => {
                if let Some(emb) = &env.audio_embedder {
                    audio::drain_audio_embeddings(cat, emb.as_ref(), scope, ws, deadline);
                }
            }
            Stage::AudioTags => audio::tag_stage(cat, scope, env, plan.budget, deadline),
            Stage::VisualEmbed => {
                if let Some(emb) = &env.embedder {
                    drain_image_embeddings(cat, emb.as_ref(), scope, ws, cache, deadline);
                }
            }
            Stage::Tags => tag::tag_stage(cat, scope, env, plan.budget),
            Stage::Promote => match cat.promote_statuses_within(scope) {
                Ok(promoted) if promoted.is_empty() => {}
                Ok(promoted) => {
                    tracing::info!(promoted = promoted.len(), "status lifecycle reconciled");
                    out.counts = library_counts(cat);
                    out.promoted = promoted;
                }
                Err(e) => {
                    tracing::warn!(%e, "status promote failed; statuses stale until the next pass")
                }
            },
        }
    }
    out
}

/// Per-status counts over every source, for the caller's report.
fn library_counts(cat: &Catalog) -> Option<StatusCounts> {
    let ids: Vec<i64> = cat.source_infos().ok()?.into_iter().map(|s| s.id).collect();
    cat.status_counts_for_sources(&ids)
        .map_err(|e| tracing::warn!(%e, "post-pass status counts unavailable"))
        .ok()
}

/// Has `deadline` passed? `None` never does.
pub(super) fn past(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|d| Instant::now() >= d)
}
