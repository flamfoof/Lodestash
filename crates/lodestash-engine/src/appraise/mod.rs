//! Appraisal: the stages that turn an indexed file into an identified asset (AI-PIPELINE,
//! v2 spec §4.6).
//!
//! A caller builds an [`AppraisePlan`] — which assets, which stages, what budget, who asked — and
//! [`run`] runs it. The order of the stages is [`Stage::ORDER`], fixed in [`plan`]; no caller
//! orders them. One file per stage, and everything public is re-exported here, so callers name
//! `appraise::drain_metrics`, not the file it lives in.
//!
//! | stage | file | what it writes |
//! |---|---|---|
//! | [`Stage::Metrics`] | [`metrics`] | technical facets, perceptual hash |
//! | [`Stage::Dependencies`] | [`deps`] | GUIDs, declared references, prefab evidence |
//! | [`Stage::Previews`] | [`preview`] | vector previews for models |
//! | [`Stage::AudioEmbed`], [`Stage::AudioTags`] | [`audio`] | CLAP embeddings, CLAP zero-shot tags |
//! | [`Stage::VisualEmbed`] | [`embed`] | SigLIP embeddings for pixels, models and materials |
//! | [`Stage::Tags`] | [`tag`], [`cloud`] | the generic tag stage, locally or through an endpoint |
//! | [`Stage::Promote`] | [`plan`] | the status lifecycle |
//!
//! [`embedders`] loads the models and [`resolve`] finds a job's bytes.
#![allow(clippy::too_many_arguments)]

mod audio;
mod cloud;
mod deps;
mod embed;
mod embedders;
mod metrics;
mod plan;
mod preview;
mod resolve;
mod tag;

pub use audio::*;
pub use cloud::*;
pub use deps::*;
pub use embed::*;
pub use embedders::*;
pub use metrics::*;
pub use plan::{
    run, AppraisePlan, Appraised, Budget, Env, Offload, Scope, Stage, Stages, TimeBudget,
};
pub use preview::*;
pub use resolve::*;
pub use tag::*;

#[cfg(test)]
pub(crate) use resolve::pulls;

#[cfg(test)]
pub(crate) mod testkit;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_plan;
