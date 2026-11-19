//! Appraisal: the stages that turn an indexed file into an identified asset (AI-PIPELINE,
//! v2 spec §4.6). One file per stage; everything public is re-exported here, so callers name
//! `appraise::drain_metrics`, not the file it lives in.
//!
//! | file | stage |
//! |---|---|
//! | [`metrics`] | technical facets, perceptual hash |
//! | [`deps`] | GUIDs, declared references, prefab evidence |
//! | [`preview`] | vector previews for models |
//! | [`audio`] | CLAP embeddings and CLAP zero-shot tags |
//! | [`embed`] | SigLIP embeddings for pixels, models and materials |
//! | [`tag`], [`cloud`] | the generic tag stage, locally or through an endpoint |
//! | [`promote`] | one pass over all of them, then the status lifecycle |
//!
//! [`embedders`] loads the models and [`resolve`] finds a job's bytes.
#![allow(clippy::too_many_arguments)]

mod audio;
mod cloud;
mod deps;
mod embed;
mod embedders;
mod metrics;
mod preview;
mod promote;
mod resolve;
mod tag;

pub use audio::*;
pub use cloud::*;
pub use deps::*;
pub use embed::*;
pub use embedders::*;
pub use metrics::*;
pub use preview::*;
pub use promote::*;
pub use resolve::*;
pub use tag::*;

#[cfg(test)]
mod tests;
