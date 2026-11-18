//! The Lodestash engine — the one service that owns the catalog (v2 spec §3–§4, ADR-0021).
//!
//! [`Engine`] is the handle: it takes the catalog's lock, runs the one writer thread (which
//! creates and migrates the catalog), keeps the read model current with the catalog generation,
//! and publishes changes. Everything else the application *does* that is not drawing or serving
//! lives here too: configuration, the model manager, the scan/persist loop, the identify drains
//! (absorbed from `lodestash-app`, v2 plan Task 2.2) and media generation. The desktop shell and
//! the HTTP API are adapters over it; nothing here may depend on a UI framework or an HTTP server.

pub mod audio;
pub mod changes;
pub mod config;
pub mod config_store;
pub mod drains;
pub mod egress;
mod engine;
pub mod grid;
pub mod jobs;
pub mod media;
pub mod models;
pub mod paths;
pub mod persist;
pub mod read_model;
pub mod sandbox;
pub mod scan;
pub mod storage;
pub mod thumbs;
pub mod types;
pub mod writer;

pub use changes::{ChangedEvent, Origin};
pub use config_store::ConfigStore;
pub use engine::{Engine, EngineConfig, EngineReport, EngineStatus, ReadError, ReadModelGuard};
/// What a [`Engine::subscribe`] receiver answers when it fell behind or the engine is gone.
pub use tokio::sync::broadcast::error::RecvError as ChangeRecvError;
pub use writer::{Op, WriteError, WriteResult, WriterHandle};

#[cfg(test)]
mod tests_engine;
