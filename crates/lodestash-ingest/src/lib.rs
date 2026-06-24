//! Ingestion: file crawling, magic-byte detection, Unity package parsing, thumbnailing.
//!
//! Spec: docs/INGESTION.md

pub mod crawler;
pub mod detect;
pub mod thumb;

pub use lodestash_core::types::MediaType;
