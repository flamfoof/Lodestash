//! AI pipeline: local (ONNX/Ollama) and cloud (Claude) taggers + embedders.
//! Spec: docs/AI-PIPELINE.md, docs/IDENTIFICATION-AND-SEARCH.md §3-4

pub mod embedder;
pub mod onnx;
pub mod siglip;
pub mod worker;
