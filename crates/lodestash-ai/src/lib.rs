//! AI pipeline: local (ONNX/Ollama) and cloud (Claude) taggers + embedders.
//! Spec: docs/AI-PIPELINE.md, docs/IDENTIFICATION-AND-SEARCH.md §3-4

pub mod clap;
pub mod clap_tags;
pub mod embedder;
pub mod ollama;
pub mod onnx;
pub mod siglip;
pub mod tagger;
pub mod worker;
