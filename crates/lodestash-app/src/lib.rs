//! Headless desktop orchestration — everything the application *does* that is not drawing.
//!
//! This crate exists so the frontend is replaceable. It holds the job registry, model-artifact
//! downloads, configuration, the catalog scan/persist loop and the identify drains; the UI crate
//! holds only the widgets that render them. Nothing here may depend on a UI framework.
//!
//! Spec: ARCHITECTURE.md §6. It is being filled incrementally — see the extraction card in
//! `docs/KANBAN.md` for what has moved and what is still parked in `lodestash-ui`.

pub mod config;
pub mod jobs;
pub mod models;
pub mod types;
