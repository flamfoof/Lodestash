//! Loopback HTTP/JSON API (/v1/) over axum. 1:1 with the MCP tool surface.
//! Spec: docs/ENGINE-API.md
//!
//! This slice is the **read-only** half: handshake + auth + `server-info`, `search`,
//! `assets/{id}`, `preview`, `file`. Mutating routes need the job engine (§4 "Async") and land
//! with it.

pub mod export;
pub mod handshake;
pub mod jobs;
pub mod routes;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use lodestash_core::catalog::{
    AssetSummaryRow, Catalog, ImportOutcome, ImportSpec, PackageListing, ProjectRow,
};
use lodestash_core::vector::{BruteForceIndex, VectorIndex};

pub use handshake::{write_handshake, Handshake};
pub use routes::{package_json, project_json, router, summary_json};

/// Every agent-facing operation in ENGINE-API §2, by MCP tool name. The MCP crate asserts its
/// own tool list against this, which is how the 1:1 invariant stays enforced rather than hoped
/// for. The list grows as routes land — parity is about *presence in both planes*, so a row is
/// added only when the route exists.
pub const AGENT_OPERATIONS: &[(&str, &str)] = &[
    ("get_server_info", "GET /v1/server-info"),
    ("search_assets", "GET /v1/search"),
    ("semantic_search", "GET /v1/search?mode=ai"),
    ("get_asset", "GET /v1/assets/{id}"),
    ("get_asset_preview", "GET /v1/assets/{id}/preview"),
    ("find_similar", "GET /v1/assets/{id}/similar"),
    ("list_packages", "GET /v1/packages"),
    ("register_project", "POST /v1/projects/register"),
    ("list_projects", "GET /v1/projects"),
    ("import_to_project", "POST /v1/import"),
    ("export_asset", "POST /v1/export"),
    ("get_job_status", "GET /v1/jobs/{id}"),
    ("cancel_job", "POST /v1/jobs/{id}/cancel"),
];

/// Resolves an asset's source file to a cached preview PNG, generating it on a miss.
pub type PreviewFn = Box<dyn Fn(&Path) -> Option<PathBuf> + Send + Sync>;

/// Encodes a natural-language query into every vector space the library has, as
/// `(model_id, vector)` pairs — SigLIP for visuals, CLAP for audio. Supplied by the caller so
/// this crate stays model-free; empty (or absent) means semantic search is unavailable.
pub type QueryEmbedFn = Box<dyn Fn(&str) -> Vec<(String, Vec<f32>)> + Send + Sync>;

/// Body of `import_to_project` (ENGINE-API §5.2). `project_id` omitted ⇒ the active editor.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ImportRequest {
    pub asset_ids: Vec<i64>,
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub dest_subfolder: Option<String>,
    #[serde(default)]
    pub with_dependencies: bool,
}

/// What [`ServerState::request_import`] decided (ENGINE-API §5.2/§5.4).
pub enum ImportKickoff {
    /// Parked on the human approval gate — the returned job is `awaiting_approval` (→ REST `202`).
    Awaiting(std::sync::Arc<jobs::Job>),
    /// Every requested asset is already in the project (dup-guard) — nothing to do (→ REST `200`
    /// with `job_id: null`).
    AllAlreadyImported(Vec<i64>),
}

/// Why an import could not even be queued.
pub enum ImportError {
    /// `asset_ids` was empty (→ `400`).
    Empty,
    /// No `project_id` given and no editor registered (→ `409 project_not_registered`). The caller
    /// falls back to `export_asset` to a path (ENGINE-API §5.2).
    NoProject,
}

/// What a handler needs to answer a request. The API owns its **own** catalog connection
/// (read-only queries) so it never contends with the UI's single writer — DATA-MODEL §1.
pub struct ServerState {
    catalog: Mutex<Catalog>,
    pub jobs: jobs::Jobs,
    workspace_dir: PathBuf,
    /// Given an asset's source file, the cached preview PNG for it (generating on a miss).
    /// Supplied by the caller because the thumbnail cache layout — and, for audio, the decoder —
    /// live outside this crate.
    preview: PreviewFn,
    /// `None` until the desktop app has an embedder loaded — `search(.., ai)` then answers
    /// `ai_unavailable` rather than silently degrading to keyword matching.
    query_embed: Mutex<Option<QueryEmbedFn>>,
    token: String,
    /// The per-tool allow-list (UI-SPEC §16, card 178). `None` = every agent operation is live
    /// (the default, so a fresh install behaves exactly as before); `Some(set)` restricts the
    /// agent surface to the named tools. Enforced on **both** planes — REST checks it in its
    /// `gate` middleware, MCP checks it per tool — so a disabled tool is disabled 1:1.
    allowed: Mutex<Option<HashSet<String>>>,
    /// Approved-or-not export requests, keyed by job id. Held apart from the job itself so the
    /// job record stays a pure status object.
    pending_exports: Mutex<std::collections::HashMap<String, (export::ExportRequest, PathBuf)>>,
    /// Approved-or-not import requests, keyed by job id — the import counterpart of
    /// `pending_exports`. Stores the request and its resolved target project.
    pending_imports: Mutex<std::collections::HashMap<String, (ImportRequest, i64)>>,
    /// Called when a job starts waiting on the user. The desktop app repaints on it — without
    /// this the approval prompt only appears the next time the window happens to redraw.
    wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
}

impl ServerState {
    pub fn new(
        catalog_path: &Path,
        workspace_dir: PathBuf,
        preview: PreviewFn,
    ) -> lodestash_core::error::Result<Self> {
        Ok(Self {
            catalog: Mutex::new(Catalog::open(catalog_path)?),
            jobs: jobs::Jobs::default(),
            workspace_dir,
            preview,
            query_embed: Mutex::new(None),
            token: new_token(),
            allowed: Mutex::new(None),
            pending_exports: Mutex::new(std::collections::HashMap::new()),
            pending_imports: Mutex::new(std::collections::HashMap::new()),
            wake: Mutex::new(None),
        })
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    /// Restrict the agent surface to `enabled` tool names (UI-SPEC §16). The GUI computes this
    /// from config — every [`AGENT_OPERATIONS`] name the user hasn't switched off — and calls it
    /// once at wiring time and again whenever the toggles change. An empty set disables every
    /// tool; to re-open everything, call [`Self::allow_all`].
    pub fn set_allowed(&self, enabled: HashSet<String>) {
        *self.allowed.lock().unwrap() = Some(enabled);
    }

    /// Drop the allow-list — every agent operation is live again (the construction default).
    pub fn allow_all(&self) {
        *self.allowed.lock().unwrap() = None;
    }

    /// Is `tool` callable? `true` when no allow-list is set (default) or the tool is in it. Both
    /// planes consult this before dispatch, which is what keeps the gate MCP↔REST 1:1.
    pub fn allows(&self, tool: &str) -> bool {
        match &*self.allowed.lock().unwrap() {
            None => true,
            Some(set) => set.contains(tool),
        }
    }

    /// Wire the query encoder (the GUI does this once its embedders load).
    pub fn set_query_embedder(&self, f: QueryEmbedFn) {
        *self.query_embed.lock().unwrap() = Some(f);
    }

    pub fn semantic_available(&self) -> bool {
        self.query_embed.lock().unwrap().is_some()
    }

    /// Semantic search (ENGINE-API §2 `semantic_search`, `GET /v1/search?mode=ai`): embed the
    /// query into each space and fuse BM25 + per-space KNN with RRF — the same
    /// `Catalog::hybrid_search` the desktop grid runs, so both surfaces rank identically.
    /// `None` when no embedder is loaded.
    pub fn semantic_search(
        &self,
        query: &str,
        limit: usize,
        media_type: Option<&str>,
    ) -> Option<lodestash_core::error::Result<Vec<AssetSummaryRow>>> {
        let spaces = (self.query_embed.lock().unwrap().as_ref()?)(query);
        if spaces.is_empty() {
            return None;
        }
        Some((|| {
            let cat = self.catalog.lock().unwrap();
            let media_id = match media_type {
                Some(code) => cat.media_type_id(code)?,
                None => None,
            };
            let borrowed: Vec<(&str, &[f32])> = spaces
                .iter()
                .map(|(m, v)| (m.as_str(), v.as_slice()))
                .collect();
            let ids = cat.hybrid_search(query, &borrowed, media_id, limit)?;
            cat.asset_summaries(&ids)
        })())
    }

    /// Register the "something needs the user's attention" callback (the GUI's repaint).
    pub fn set_wake(&self, f: Box<dyn Fn() + Send + Sync>) {
        *self.wake.lock().unwrap() = Some(f);
    }

    fn wake(&self) {
        if let Some(f) = self.wake.lock().unwrap().as_ref() {
            f();
        }
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

    /// What an asset drags along (ENGINE-API §2 `AssetDetail.dependencies`): the transitive
    /// closure over resolved dependency edges, the asset itself removed. Deliberately the same
    /// set `export_asset` copies with `with_dependencies: true` — a client that shows this list
    /// is showing exactly what an export would write.
    pub fn dependencies(&self, id: i64) -> lodestash_core::error::Result<Vec<i64>> {
        let mut ids = self.catalog.lock().unwrap().dependency_closure(&[id])?;
        ids.retain(|&d| d != id);
        Ok(ids)
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

    /// Register/refresh the open Unity editor (ENGINE-API §5.1 `register_project`). Registration
    /// always makes it the active project, so `is_active` is always `true`.
    pub fn register_project(
        &self,
        project_path: &str,
        unity_version: Option<&str>,
        pipeline: Option<&str>,
    ) -> lodestash_core::error::Result<(i64, bool)> {
        let id =
            self.catalog
                .lock()
                .unwrap()
                .register_project(project_path, unity_version, pipeline)?;
        Ok((id, true))
    }

    /// Registered editors, active first (ENGINE-API §5.1 `list_projects`).
    pub fn list_projects(&self) -> lodestash_core::error::Result<Vec<ProjectRow>> {
        self.catalog.lock().unwrap().list_projects()
    }

    /// Queue an import into a Unity project (ENGINE-API §5.2 `import_to_project`). Resolves the
    /// target (given `project_id`, else the active editor), applies the §5.4 dup-guard, and — when
    /// anything is new to import — parks a job on the approval gate exactly like `export_asset`:
    /// nothing is written until [`Self::approve_job`]. This is the first *mutating* agent op after
    /// export, so it obeys the same "confirm server-side" rule (MCP-AND-ENGINES §5).
    pub fn request_import(&self, req: ImportRequest) -> Result<ImportKickoff, ImportError> {
        if req.asset_ids.is_empty() {
            return Err(ImportError::Empty);
        }
        let project_id = match req.project_id {
            Some(id) => id,
            None => self
                .catalog
                .lock()
                .unwrap()
                .list_projects()
                .ok()
                .and_then(|ps| ps.into_iter().find(|p| p.is_active).map(|p| p.id))
                .ok_or(ImportError::NoProject)?,
        };
        // §5.4 dup-guard: assets already recorded for this project are dropped from the request.
        let already = self
            .catalog
            .lock()
            .unwrap()
            .already_imported(&req.asset_ids, project_id)
            .unwrap_or_default();
        let fresh: Vec<i64> = req
            .asset_ids
            .iter()
            .copied()
            .filter(|id| !already.contains(id))
            .collect();
        if fresh.is_empty() {
            return Ok(ImportKickoff::AllAlreadyImported(already));
        }
        let summary = format!(
            "Import {} asset(s) into project {}",
            fresh.len(),
            project_id
        );
        let job = self
            .jobs
            .create("import", summary, jobs::JobState::AwaitingApproval);
        let mut stored = req;
        stored.asset_ids = fresh; // approve imports exactly what was advertised as new
        self.pending_imports
            .lock()
            .unwrap()
            .insert(job.id.clone(), (stored, project_id));
        self.wake();
        Ok(ImportKickoff::Awaiting(job))
    }

    /// The approved import runs here (the import counterpart of [`Self::run_export`]). Resolves the
    /// dependency closure (a mesh drags its materials + textures — same resolver as export), then
    /// writes one durable `import_record` per asset.
    ///
    /// **Interim divergence from ENGINE-API §5.3:** the spec writes `import_record` on the plugin's
    /// *ack* (with Unity's real `dest_path`/`guid`). Until the UPM plugin + import-queue land, an
    /// approved import records here with the computed `Assets/<sub>/<name>` path and a NULL guid;
    /// the plugin ack will refine those. Recorded here in ENGINE-API §5, same change.
    fn run_import(&self, job: std::sync::Arc<jobs::Job>, req: ImportRequest, project_id: i64) {
        job.set_state(jobs::JobState::Running);
        let ids = if req.with_dependencies {
            match self
                .catalog
                .lock()
                .unwrap()
                .dependency_closure(&req.asset_ids)
            {
                Ok(ids) => ids,
                Err(e) => return job.fail(e.to_string()),
            }
        } else {
            req.asset_ids.clone()
        };
        let rows = match self.catalog.lock().unwrap().asset_summaries(&ids) {
            Ok(rows) => rows,
            Err(e) => return job.fail(e.to_string()),
        };
        let sub = req.dest_subfolder.as_deref().unwrap_or("Lodestash");
        let specs: Vec<ImportSpec> = rows
            .iter()
            .map(|r| ImportSpec {
                asset_id: r.id,
                source_package_id: r.package_id,
                dest_path: format!("Assets/{sub}/{}", file_name_for(r)),
                unity_guid: None,
                with_deps: req.with_dependencies,
            })
            .collect();
        let batch_id = format!("imp-{}", job.id);
        match self
            .catalog
            .lock()
            .unwrap()
            .record_imports(project_id, &specs, &batch_id)
        {
            Ok(outcomes) => {
                let recorded = outcomes
                    .iter()
                    .filter(|o| matches!(o, ImportOutcome::Recorded { .. }))
                    .count();
                job.finish(serde_json::json!({
                    "project_id": project_id,
                    "recorded": recorded,
                    "already_imported": outcomes.len() - recorded,
                    "dependencies_included": req.with_dependencies,
                    "dependencies_added": ids.len().saturating_sub(req.asset_ids.len()),
                    "batch_id": batch_id,
                }));
            }
            Err(e) => job.fail(e.to_string()),
        }
    }

    /// Queue an export (ENGINE-API §2 `export_asset`). Validates the destination against the
    /// sandbox *now* — a bad path is an error, not a job that fails later — then parks the job
    /// on the approval gate. Nothing is copied until [`Self::approve_job`].
    pub fn request_export(
        &self,
        req: export::ExportRequest,
    ) -> Result<std::sync::Arc<jobs::Job>, export::SandboxError> {
        let roots = self
            .catalog
            .lock()
            .unwrap()
            .source_roots()
            .unwrap_or_default();
        let dest = export::check_dest(&req.dest_path, &roots)?;
        let summary = format!(
            "Export {} asset(s) to {}",
            req.asset_ids.len(),
            dest.display()
        );
        let job = self
            .jobs
            .create("export", summary, jobs::JobState::AwaitingApproval);
        self.pending_exports
            .lock()
            .unwrap()
            .insert(job.id.clone(), (req, dest));
        self.wake();
        Ok(job)
    }

    /// The desktop app said yes: run the export on a worker thread. Unknown/already-approved job
    /// ids are a no-op.
    pub fn approve_job(self: &std::sync::Arc<Self>, job_id: &str) {
        let Some(job) = self.jobs.get(job_id) else {
            return;
        };
        if job.state() != jobs::JobState::AwaitingApproval {
            return;
        }
        // The job is either a pending export or a pending import — run whichever it is.
        if let Some((req, dest)) = self.pending_exports.lock().unwrap().remove(job_id) {
            job.set_state(jobs::JobState::Queued);
            let state = self.clone();
            std::thread::spawn(move || state.run_export(job, req, dest));
            return;
        }
        if let Some((req, project_id)) = self.pending_imports.lock().unwrap().remove(job_id) {
            job.set_state(jobs::JobState::Queued);
            let state = self.clone();
            std::thread::spawn(move || state.run_import(job, req, project_id));
        }
    }

    /// The desktop app said no.
    pub fn deny_job(&self, job_id: &str) {
        self.pending_exports.lock().unwrap().remove(job_id);
        self.pending_imports.lock().unwrap().remove(job_id);
        if let Some(job) = self.jobs.get(job_id) {
            if job.state() == jobs::JobState::AwaitingApproval {
                job.set_state(jobs::JobState::Cancelled);
            }
        }
    }

    fn run_export(
        &self,
        job: std::sync::Arc<jobs::Job>,
        req: export::ExportRequest,
        dest: PathBuf,
    ) {
        job.set_state(jobs::JobState::Running);
        // With dependencies, the copy set is the transitive closure over the P2 relationship
        // graph (a material drags its textures). Edges that resolve to nothing in the library
        // add nothing — the closure only walks `resolved` ones.
        let ids = if req.with_dependencies {
            match self
                .catalog
                .lock()
                .unwrap()
                .dependency_closure(&req.asset_ids)
            {
                Ok(ids) => ids,
                Err(e) => return job.fail(e.to_string()),
            }
        } else {
            req.asset_ids.clone()
        };
        let rows = match self.catalog.lock().unwrap().asset_summaries(&ids) {
            Ok(rows) => rows,
            Err(e) => return job.fail(e.to_string()),
        };
        let files: Vec<(PathBuf, String)> = rows
            .iter()
            .filter_map(|r| self.source_path(r).map(|p| (p, file_name_for(r))))
            .collect();
        let missing = ids.len().saturating_sub(files.len());
        match export::copy_files(&files, &dest, &job) {
            Ok(written) if job.is_cancelled() => {
                job.set_state(jobs::JobState::Cancelled);
                tracing::info!(%job.id, written, "export cancelled part-way");
            }
            Ok(written) => job.finish(serde_json::json!({
                "written": written,
                "missing": missing,
                "dest_path": dest.display().to_string(),
                "dependencies_included": req.with_dependencies,
                "requested_dependencies": req.with_dependencies,
                "dependencies_added": ids.len().saturating_sub(req.asset_ids.len()),
            })),
            Err(e) => job.fail(e.to_string()),
        }
    }

    /// The asset's preview PNG on disk, generated on a cache miss.
    pub fn preview_path(&self, id: i64) -> Option<PathBuf> {
        let row = self.asset(id).ok().flatten()?;
        (self.preview)(&self.source_path(&row)?)
    }
}

/// What the exported copy is called on disk: the asset's name, with the primary file's extension
/// when the name doesn't already carry one.
fn file_name_for(r: &AssetSummaryRow) -> String {
    match &r.format {
        Some(ext) if !r.name.to_lowercase().ends_with(&format!(".{ext}")) => {
            format!("{}.{ext}", r.name)
        }
        _ => r.name.clone(),
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
