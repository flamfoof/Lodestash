//! Loopback HTTP/JSON API (/v1/) over axum. 1:1 with the MCP tool surface.
//! Spec: docs/ENGINE-API.md
//!
//! This slice is the **read-only** half: handshake + auth + `server-info`, `search`,
//! `assets/{id}`, `preview`, `file`. Mutating routes need the job engine (§4 "Async") and land
//! with it.

pub mod handshake;
pub mod routes;

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lodestash_core::catalog::{AssetSummaryRow, Catalog, PackageListing};
use lodestash_core::vector::{BruteForceIndex, VectorIndex};

pub use handshake::{write_handshake, Handshake};
pub use routes::{package_json, router, summary_json};

/// Every agent-facing operation in ENGINE-API §2, by MCP tool name. The MCP crate asserts its
/// own tool list against this, which is how the 1:1 invariant stays enforced rather than hoped
/// for. The list grows as routes land — parity is about *presence in both planes*, so a row is
/// added only when the route exists.
pub const AGENT_OPERATIONS: &[(&str, &str)] = &[
    ("get_server_info", "GET /v1/server-info"),
    ("search_assets", "GET /v1/search"),
    ("get_asset", "GET /v1/assets/{id}"),
    ("get_asset_preview", "GET /v1/assets/{id}/preview"),
    ("find_similar", "GET /v1/assets/{id}/similar"),
    ("list_packages", "GET /v1/packages"),
];

/// Resolves an asset's source file to a cached preview PNG, generating it on a miss.
pub type PreviewFn = Box<dyn Fn(&Path) -> Option<PathBuf> + Send + Sync>;

/// What a handler needs to answer a request. The API owns its **own** catalog connection
/// (read-only queries) so it never contends with the UI's single writer — DATA-MODEL §1.
pub struct ServerState {
    catalog: Mutex<Catalog>,
    workspace_dir: PathBuf,
    /// Given an asset's source file, the cached preview PNG for it (generating on a miss).
    /// Supplied by the caller because the thumbnail cache layout — and, for audio, the decoder —
    /// live outside this crate.
    preview: PreviewFn,
    token: String,
}

impl ServerState {
    pub fn new(
        catalog_path: &Path,
        workspace_dir: PathBuf,
        preview: PreviewFn,
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

    // ── The agent operations. Both planes call *these*, never the catalog directly — that's
    //    what makes the MCP↔REST 1:1 rule structural instead of aspirational.

    /// `{app, schema, api}` versions (ENGINE-API §4 "Versioning").
    pub fn server_info(&self) -> lodestash_core::error::Result<(String, u32)> {
        let schema = self.catalog.lock().unwrap().schema_version()?;
        Ok((env!("CARGO_PKG_VERSION").to_string(), schema))
    }

    /// Structural search. An empty/punctuation-only query falls back to the most recent assets so
    /// a client that just connected sees the library rather than nothing.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        media_type: Option<&str>,
    ) -> lodestash_core::error::Result<Vec<AssetSummaryRow>> {
        let cat = self.catalog.lock().unwrap();
        let ids = match cat.search_assets(query, limit)? {
            Some(ids) => ids,
            None => cat.recent_asset_ids(limit)?,
        };
        let rows = cat.asset_summaries(&ids)?;
        Ok(match media_type {
            Some(m) => rows.into_iter().filter(|r| r.media_type == m).collect(),
            None => rows,
        })
    }

    pub fn asset(&self, id: i64) -> lodestash_core::error::Result<Option<AssetSummaryRow>> {
        let cat = self.catalog.lock().unwrap();
        Ok(cat.asset_summaries(&[id])?.into_iter().next())
    }

    /// Where an asset's bytes live: an on-disk source file, or a payload extracted into the
    /// workspace.
    pub fn source_path(&self, row: &AssetSummaryRow) -> Option<PathBuf> {
        if let Some(p) = &row.disk_path {
            return Some(PathBuf::from(p));
        }
        row.workspace_rel
            .as_ref()
            .map(|rel| self.workspace_dir.join(rel))
    }

    /// "More like this" (ENGINE-API §2 `find_similar`): cosine KNN in the asset's own embedding
    /// space, the asset itself dropped from its own results. Empty when the asset has no
    /// embedding yet — a not-yet-encoded library isn't an error, it just has no neighbours.
    pub fn similar(
        &self,
        id: i64,
        limit: usize,
    ) -> lodestash_core::error::Result<Vec<AssetSummaryRow>> {
        let cat = self.catalog.lock().unwrap();
        let Some((model_id, query)) = cat.embedding_of(id)? else {
            return Ok(Vec::new());
        };
        let index = BruteForceIndex::new(cat.embeddings_for_knn(&model_id, None)?);
        let ids: Vec<i64> = index
            .knn(&query, limit + 1)
            .into_iter()
            .map(|(id, _)| id)
            .filter(|&other| other != id)
            .take(limit)
            .collect();
        cat.asset_summaries(&ids)
    }

    /// Downloaded Asset Store packages (ENGINE-API §3 `Package`).
    pub fn packages(&self) -> lodestash_core::error::Result<Vec<PackageListing>> {
        self.catalog.lock().unwrap().list_packages()
    }

    /// The asset's preview PNG on disk, generated on a cache miss.
    pub fn preview_path(&self, id: i64) -> Option<PathBuf> {
        let row = self.asset(id).ok().flatten()?;
        (self.preview)(&self.source_path(&row)?)
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
