//! The Lodestash engine — the one service that owns the catalog (v2 spec §3–§4, ADR-0021).
//!
//! Everything the application *does* that is not drawing or serving lives here: configuration,
//! the model manager, the catalog scan/persist loop and the identify drains (absorbed from
//! `lodestash-app`, v2 plan Task 2.2). The desktop shell and the HTTP API are adapters over it;
//! nothing here may depend on a UI framework or an HTTP server.

pub mod audio;
pub mod config;
pub mod drains;
pub mod egress;
pub mod grid;
pub mod jobs;
pub mod models;
pub mod paths;
pub mod persist;
pub mod scan;
pub mod thumbs;
pub mod types;
