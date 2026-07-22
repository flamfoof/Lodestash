//! Loopback HTTP/JSON API (/v1/) over axum. 1:1 with the MCP tool surface.
//! Spec: docs/ENGINE-API.md
//!
//! This slice is the **read-only** half: handshake + auth + `server-info`, `search`,
//! `assets/{id}`, `preview`, `file`. Mutating routes need the job engine (§4 "Async") and land
//! with it.

pub mod handshake;
mod routes;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lodestash_core::catalog::Catalog;

pub use handshake::{write_handshake, Handshake};
pub use routes::router;

/// Every agent-facing operation in ENGINE-API §2, by MCP tool name. The MCP crate asserts its
/// own tool list against this, which is how the 1:1 invariant stays enforced rather than hoped
/// for. The list grows as routes land — parity is about *presence in both planes*, so a row is
/// added only when the route exists.
pub const AGENT_OPERATIONS: &[(&str, &str)] = &[
    ("get_server_info", "GET /v1/server-info"),
    ("search_assets", "GET /v1/search"),
    ("get_asset", "GET /v1/assets/{id}"),
    ("get_asset_preview", "GET /v1/assets/{id}/preview"),
];

/// What a handler needs to answer a request. The API owns its **own** catalog connection
/// (read-only queries) so it never contends with the UI's single writer — DATA-MODEL §1.
pub struct ServerState {
    catalog: Mutex<Catalog>,
    workspace_dir: PathBuf,
    /// Absolute path of an asset's cached preview image, if one exists. Supplied by the caller
    /// because the thumbnail cache layout belongs to `lodestash-ingest`, which this crate
    /// deliberately doesn't depend on.
    preview: Box<dyn Fn(i64) -> Option<PathBuf> + Send + Sync>,
    token: String,
}

impl ServerState {
    pub fn new(
        catalog_path: &Path,
        workspace_dir: PathBuf,
        preview: Box<dyn Fn(i64) -> Option<PathBuf> + Send + Sync>,
    ) -> lodestash_core::error::Result<Self> {
        Ok(Self {
            catalog: Mutex::new(Catalog::open(catalog_path)?),
            workspace_dir,
            preview,
            token: new_token(),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }
}

/// Per-session bearer token (ENGINE-API §1). A v4 UUID: 122 bits from the OS RNG.
pub fn new_token() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}
