//! Ingestion: file crawling, magic-byte detection, Unity package parsing, thumbnailing.
//!
//! Spec: docs/INGESTION.md

pub mod asset_store;
pub mod crawler;
pub mod deps;
pub mod detect;
pub mod material;
pub mod melspec;
pub mod mesh;
pub mod metrics;
pub mod raster;
pub mod render3d;
pub mod thumb;
pub mod unitypackage;
pub mod watch;
pub mod workspace;

pub use lodestash_core::types::MediaType;
