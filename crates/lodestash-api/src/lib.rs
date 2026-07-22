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
    /// Given an asset's source file, the cached preview PNG for it (generating on a miss).
    /// Supplied by the caller because the thumbnail cache layout — and, for audio, the decoder —
    /// live outside this crate.
    preview: Box<dyn Fn(&Path) -> Option<PathBuf> + Send + Sync>,
    token: String,
}

impl ServerState {
    pub fn new(
        catalog_path: &Path,
        workspace_dir: PathBuf,
        preview: Box<dyn Fn(&Path) -> Option<PathBuf> + Send + Sync>,
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

/// Bind an ephemeral loopback port, write `engine.json`, and serve on a background thread.
/// Binding happens *here*, synchronously, so the returned handshake carries the real port.
///
/// ponytail: the server thread runs until the process exits — no shutdown handle. The GUI owns
/// the API's whole lifetime; add one if the API ever becomes toggleable at runtime.
pub fn serve(state: std::sync::Arc<ServerState>) -> std::io::Result<Handshake> {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
    listener.set_nonblocking(true)?;
    let port = listener.local_addr()?.port();
    let handshake = Handshake::new(port, state.token());
    let path = write_handshake(&handshake)?;
    tracing::info!(port, path = %path.display(), "engine API listening");

    std::thread::Builder::new()
        .name("lodestash-api".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => return tracing::error!(%e, "engine API runtime"),
            };
            rt.block_on(async move {
                match tokio::net::TcpListener::from_std(listener) {
                    Ok(l) => {
                        if let Err(e) = axum::serve(l, router(state)).await {
                            tracing::error!(%e, "engine API stopped");
                        }
                    }
                    Err(e) => tracing::error!(%e, "engine API listener"),
                }
            });
        })?;
    Ok(handshake)
}
