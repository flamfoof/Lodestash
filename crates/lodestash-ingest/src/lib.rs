//! Ingestion: file crawling, magic-byte detection, Unity package parsing, thumbnailing.
//!
//! Spec: docs/INGESTION.md

pub mod asset_store;
pub mod crawler;
pub mod detect;
pub mod mesh;
pub mod raster;
pub mod render3d;
pub mod thumb;
pub mod unitypackage;
pub mod workspace;

pub use lodestash_core::types::MediaType;
