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
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use lodestash_core::catalog::{AssetSummaryRow, Catalog, ImportSpec, PackageListing, ProjectRow};

/// ENGINE-API §2 `list_tags` row — `(namespace, value, asset count)`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TagRow {
    pub namespace: String,
    pub value: String,
    pub count: i64,
}

impl From<(String, String, i64)> for TagRow {
    fn from((namespace, value, count): (String, String, i64)) -> Self {
        Self {
            namespace,
            value,
            count,
        }
    }
}

/// ENGINE-API §2 `list_collections` row — the core row plus the staging flag, so a client can
/// render the active project's staging tray apart from the user's named collections.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CollectionRowApi {
    pub id: i64,
    pub name: String,
    pub is_staging: bool,
    pub asset_count: usize,
}

impl From<lodestash_core::catalog::CollectionRow> for CollectionRowApi {
    fn from(r: lodestash_core::catalog::CollectionRow) -> Self {
        Self {
            id: r.id,
            name: r.name,
            is_staging: false,
            asset_count: r.asset_count as usize,
        }
    }
}

/// ENGINE-API §2 `suggest_tags` row — a tag with its AI confidence (None = hand-added).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TagProposal {
    pub namespace: String,
    pub value: String,
    pub confidence: Option<f64>,
}

pub use handshake::{handshake_path, write_handshake, Handshake};
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
    ("list_tags", "GET /v1/tags"),
    ("add_tag", "POST /v1/assets/{id}/tags"),
    ("list_collections", "GET /v1/collections"),
    ("apply_mapping", "POST /v1/mapping/apply"),
    ("identify_asset", "POST /v1/identify"),
    ("suggest_tags", "GET /v1/assets/{id}/suggest-tags"),
    ("unpack_package", "POST /v1/packages/unpack"),
    ("stage_asset", "POST /v1/stage"),
    ("list_staged", "GET /v1/stage"),
    ("import_staged", "POST /v1/stage/import"),
    ("write_tags", "POST /v1/tags/batch"),
];

/// Everything a preview renderer needs for one asset, resolved from the catalog by
/// [`ServerState::preview_subject`] so the renderer needs no database access of its own.
pub struct PreviewSubject {
    pub media_type: String,
    /// The asset's own file, **verified to exist**. `None` for an asset still inside a pack.
    pub disk_path: Option<PathBuf>,
    /// `(package path, tar offset, byte size, extension)` for an in-pack asset — the locator a
    /// surgical pull needs (INGESTION §2).
    pub in_pack: Option<(String, i64, i64, String)>,
    /// For a material: the on-disk texture its shader ball wears (DATA-MODEL §10).
    pub albedo: Option<PathBuf>,
    /// For a material whose albedo is *also* still inside a pack: that texture's locator, so the
    /// renderer can pull it the same way it pulls an asset's own payload.
    pub albedo_in_pack: Option<(String, i64, i64, String)>,
    /// For a model: the texture each material slot is painted with (DATA-MODEL §10, v1 plan S08).
    /// Empty when the graph paints none of it — the renderer then falls back to the untextured
    /// hero view.
    pub slot_textures: std::collections::BTreeMap<u32, PathBuf>,
}

/// Resolves one asset to a cached preview PNG, generating it on a miss.
pub type PreviewFn = Box<dyn Fn(&PreviewSubject) -> Option<PathBuf> + Send + Sync>;

/// Encodes a natural-language query into every vector space the library has, as
/// `(model_id, vector)` pairs — SigLIP for visuals, CLAP for audio. Supplied by the caller so
/// this crate stays model-free; empty (or absent) means semantic search is unavailable.
pub type QueryEmbedFn = Box<dyn Fn(&str) -> Vec<(String, Vec<f32>)> + Send + Sync>;

/// The catalog mutations the agent planes may request (ENGINE-API §2 "mutating" rows). The
/// desktop maps each variant to its single-writer `Op` — this crate never writes the catalog
/// itself (ADR-0002), it only forwards the request and reports how many rows changed.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AgentWrite {
    AddTag {
        asset_id: i64,
        namespace: String,
        value: String,
    },
    RemoveTag {
        asset_id: i64,
        namespace: String,
        value: String,
    },
    ApplyMapping {
        taxonomy_id: i64,
    },
    /// Flag + appraise an explicit set (blocking: the writer runs one bounded pass).
    Identify {
        asset_ids: Vec<i64>,
    },
    /// Extract one pack into the workspace (blocking: the whole M14 flow completes first).
    UnpackPackage {
        package_id: i64,
    },
    StageAsset {
        asset_id: i64,
    },
    UnstageAsset {
        asset_id: i64,
    },
    /// An approved `write_tags` request (ENGINE-API §8.3) and its body digest — applied as one
    /// batch with its durable receipt. The desktop turns it into the core batch.
    TagBatch {
        request: TagBatchRequest,
        digest: String,
    },
    /// Register or refresh a Unity editor and make it the active project (ENGINE-API §5.1).
    RegisterProject {
        project_path: String,
        unity_version: Option<String>,
        pipeline: Option<String>,
    },
    /// The durable `import_record` rows for an acked request (ENGINE-API §5.3). Reports how many
    /// were recorded; the rest were already imported.
    RecordImports {
        project_id: i64,
        specs: Vec<ImportSpec>,
        batch_id: String,
    },
    /// Export history for the files an export wrote (FEATURES §7.6).
    RecordExports {
        batch_id: String,
        dest_path: String,
        files: Vec<(Option<i64>, String)>,
    },
}

/// Most bytes one `write_tags` request may carry (ENGINE-API §8.3).
pub const MAX_TAG_BATCH_BYTES: usize = 1 << 20;

/// One assignment as an agent submits it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagIn {
    pub namespace: String,
    pub value: String,
    #[serde(default)]
    pub confidence: Option<f64>,
}

/// One asset's part of a `write_tags` request.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagBatchAsset {
    pub asset_id: i64,
    /// The `annotation_revision` `get_asset` reported — anything changed since refuses the batch.
    pub expected_revision: String,
    pub producer_id: String,
    #[serde(default)]
    pub model_id: Option<String>,
    /// `append` | `replace_producer`.
    pub mode: String,
    #[serde(default)]
    pub tags: Vec<TagIn>,
    /// Omitted preserves the caption.
    #[serde(default)]
    pub description: Option<String>,
}

/// `write_tags` ↔ `POST /v1/tags/batch` (ENGINE-API §8.3). One type for both planes: the MCP tool
/// parses into it too, so the two cannot validate differently.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TagBatchRequest {
    pub request_id: String,
    /// Accepted and recorded; enforced once vocabulary review (§8.5) lands.
    #[serde(default)]
    pub vocabulary_revision: Option<String>,
    pub assets: Vec<TagBatchAsset>,
}

impl TagBatchRequest {
    /// The core batch an approved request becomes: every item under `ai:<producer>`, the
    /// revisions the agent read, the agent's history, and the durable receipt.
    pub fn to_core_batch(&self, digest: &str) -> lodestash_core::catalog::TagBatch {
        use lodestash_core::catalog::{
            ai_source, HistoryOrigin, TagAssignment, TagBatch, TagBatchItem, TagMode, TagReceiptKey,
        };
        TagBatch {
            items: self
                .assets
                .iter()
                .map(|a| TagBatchItem {
                    asset_id: a.asset_id,
                    source: ai_source(&a.producer_id),
                    model_id: a.model_id.clone(),
                    mode: if a.mode == "replace_producer" {
                        TagMode::ReplaceSource
                    } else {
                        TagMode::Append
                    },
                    tags: a
                        .tags
                        .iter()
                        .map(|t| TagAssignment {
                            namespace: t.namespace.clone(),
                            value: t.value.clone(),
                            confidence: t.confidence,
                        })
                        .collect(),
                    description: a.description.clone(),
                    expected_revision: Some(a.expected_revision.clone()),
                })
                .collect(),
            completes_queue: false,
            history: Some(HistoryOrigin::Agent),
            receipt: Some(TagReceiptKey {
                request_id: self.request_id.clone(),
                digest: digest.to_string(),
            }),
        }
    }
}

/// Why a `write_tags` request was refused, with the stable code and HTTP status both planes
/// report (ENGINE-API §8.4).
#[derive(Debug, Clone, PartialEq)]
pub enum TagBatchError {
    Invalid(String),
    TooLarge(usize),
    ProducerNotConfigured,
    ProducerMismatch(String),
    AssetNotFound(i64),
    RequestConflict(String),
    Catalog(String),
}

impl TagBatchError {
    /// `(HTTP status, code, message)`.
    pub fn parts(&self) -> (u16, &'static str, String) {
        match self {
            Self::Invalid(m) => (400, "invalid_argument", m.clone()),
            Self::TooLarge(n) => (
                413,
                "resource_too_large",
                format!("{n} bytes; a tag batch holds at most {MAX_TAG_BATCH_BYTES}"),
            ),
            Self::ProducerNotConfigured => (
                409,
                "producer_not_configured",
                "no agent producer is configured in Lodestash (agent_producer_id), so there is no \
                 producer to attribute these tags to"
                    .into(),
            ),
            Self::ProducerMismatch(p) => (
                403,
                "producer_mismatch",
                format!("producer {p:?} is not the one configured for this host"),
            ),
            Self::AssetNotFound(id) => {
                (404, "asset_not_found", format!("asset {id} does not exist"))
            }
            Self::RequestConflict(id) => (
                409,
                "request_conflict",
                format!("request {id} was already used with a different body"),
            ),
            Self::Catalog(m) => (500, "job_failed", m.clone()),
        }
    }
}

/// What [`ServerState::request_tag_batch`] decided.
pub enum TagBatchKickoff {
    /// Parked on the approval gate (→ `202`).
    Awaiting(std::sync::Arc<jobs::Job>),
    /// The same request already committed: its recorded result (→ `200`).
    Replayed {
        request_id: String,
        result: serde_json::Value,
    },
}

impl TagBatchKickoff {
    /// `(HTTP status, body)` — one builder for both planes.
    pub fn to_response(&self) -> (u16, serde_json::Value) {
        match self {
            Self::Awaiting(job) => (202, job.to_json()),
            Self::Replayed { request_id, result } => (
                200,
                serde_json::json!({
                    "request_id": request_id,
                    "state": "done",
                    "replayed": true,
                    "result": result,
                }),
            ),
        }
    }
}

/// The desktop's write bridge — [`AgentWrite`] → writer `Op` → changed rows.
pub type AgentWriteFn = Box<dyn Fn(AgentWrite) -> Result<usize, String> + Send + Sync>;

/// A headless server's line to the running app (v1 plan S11): one REST request — method, `/v1/`
/// path, JSON body — answered with the app's status and JSON body, or `Err` when no app answered.
///
/// Operations the app owns — writes, jobs, approvals — are forwarded whole rather than run a
/// second time here: a job this process created would be invisible to the app, and an approval
/// parked here would wait for a window that does not exist.
pub type AppProxyFn = Box<
    dyn Fn(&str, &str, Option<&serde_json::Value>) -> Result<(u16, serde_json::Value), String>
        + Send
        + Sync,
>;

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
    Awaiting {
        job: std::sync::Arc<jobs::Job>,
        pipeline_mismatch: Vec<lodestash_core::catalog::PipelineAdvisory>,
    },
    /// Every requested asset is already in the project (dup-guard) — nothing to do (→ REST `200`
    /// with `job_id: null`).
    AllAlreadyImported {
        imported_existing: Vec<i64>,
        pipeline_mismatch: Vec<lodestash_core::catalog::PipelineAdvisory>,
    },
}

impl ImportKickoff {
    /// The agent-facing response body (ENGINE-API §5.2, advisories §5.4).
    ///
    /// **One builder for both planes.** MCP and REST are required to be 1:1 on the agent-facing
    /// surface, and two hand-copied `json!` blocks is exactly how that stops being true — the
    /// `pipeline_mismatch` list would have landed on one plane and not the other. The status code
    /// is still the transport's business (`202` awaiting vs `200` nothing-to-do).
    ///
    /// The advisory key is **omitted when empty** rather than sent as `[]`: a client that never
    /// sees it behaves exactly as it did before the advisory existed.
    pub fn to_json(&self) -> serde_json::Value {
        let (mut body, advisories) = match self {
            ImportKickoff::Awaiting {
                job,
                pipeline_mismatch,
            } => (job.to_json(), pipeline_mismatch),
            ImportKickoff::AllAlreadyImported {
                imported_existing,
                pipeline_mismatch,
            } => (
                serde_json::json!({ "imported_existing": imported_existing, "job_id": null }),
                pipeline_mismatch,
            ),
        };
        if !advisories.is_empty() {
            if let Some(obj) = body.as_object_mut() {
                obj.insert(
                    "pipeline_mismatch".into(),
                    serde_json::to_value(advisories).unwrap_or(serde_json::Value::Null),
                );
            }
        }
        body
    }
}

/// Why an import could not even be queued.
pub enum ImportError {
    /// `asset_ids` was empty (→ `400`).
    Empty,
    /// No `project_id` given and no editor registered (→ `409 project_not_registered`). The caller
    /// falls back to `export_asset` to a path (ENGINE-API §5.2).
    NoProject,
}

/// One asset the Unity plugin reports it imported (ENGINE-API §5.3 ack). Carries the *real*
/// in-project path and GUID Unity assigned — the durable `import_record` is written from these.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct AckAsset {
    pub asset_id: i64,
    pub dest_path: String,
    #[serde(default)]
    pub unity_guid: Option<String>,
}

/// A pending import request the plugin drains on Unity's main thread (ENGINE-API §5.3). The
/// `assets` carry `file_url`s so the plugin can fetch the bytes.
pub struct ImportQueuePayload {
    pub request_id: String,
    pub job_id: String,
    pub dest_subfolder: Option<String>,
    pub assets: Vec<AssetSummaryRow>,
}

/// Why an ack could not be applied.
#[derive(Debug)]
pub enum AckError {
    /// No queued request with that id (already acked, cancelled, or never existed).
    UnknownRequest,
    Db(lodestash_core::error::CoreError),
}

/// One approved import waiting for the plugin to pick it up (ENGINE-API §5.3). In-memory, like the
/// job registry — a crash loses the queue, never the catalog (§5 "Transient vs durable").
struct ImportQueueEntry {
    request_id: String,
    job_id: String,
    project_id: i64,
    /// The resolved dependency closure — what the plugin will actually import.
    asset_ids: Vec<i64>,
    with_deps: bool,
    dest_subfolder: Option<String>,
    /// Visibility timeout: while this is in the future the entry is considered in flight and is
    /// **not** handed out again. Set by [`ServerState::poll_import_queue`], cleared by removal in
    /// [`ServerState::ack_import`].
    ///
    /// Without it, a plugin that polls twice before acking — its own retry, a Unity domain reload,
    /// a dropped response — receives the same request again and copies the files into `Assets/` a
    /// second time. The `UNIQUE(asset_id, project_id)` guard only stops the duplicate *record*, not
    /// the duplicate copy.
    leased_until: Option<std::time::Instant>,
}

/// How long a polled import stays invisible to further polls. Long enough to cover a Unity import
/// plus a domain reload; short enough that a crashed plugin's work is redelivered in the same
/// session rather than stranding the job.
const IMPORT_LEASE: std::time::Duration = std::time::Duration::from_secs(60);

/// What a handler needs to answer a request. The API owns its **own** catalog connection
/// (read-only queries) so it never contends with the UI's single writer — DATA-MODEL §1.
pub struct ServerState {
    catalog: Mutex<Catalog>,
    pub jobs: jobs::Jobs,
    workspace_dir: PathBuf,
    /// Given everything known about one asset, the cached preview PNG for it (generating on a
    /// miss). Supplied by the caller because the thumbnail cache layout — and, for audio, the
    /// decoder — live outside this crate.
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
    /// May an agent receive asset **bytes** — previews and originals — or only metadata and links
    /// (v1 plan S10)? Off until the GUI says otherwise: a local transport says nothing about where
    /// the model behind it runs, and reading metadata never implied permission to upload files.
    byte_release: AtomicBool,
    /// Approved-or-not export requests, keyed by job id. Held apart from the job itself so the
    /// job record stays a pure status object.
    pending_exports: Mutex<std::collections::HashMap<String, (export::ExportRequest, PathBuf)>>,
    /// Approved-or-not import requests, keyed by job id — the import counterpart of
    /// `pending_exports`. Stores the request and its resolved target project.
    pending_imports: Mutex<std::collections::HashMap<String, (ImportRequest, i64)>>,
    /// Approved imports waiting for a Unity plugin to drain and ack (ENGINE-API §5.3).
    import_queue: Mutex<Vec<ImportQueueEntry>>,
    /// Called when a job starts waiting on the user. The desktop app repaints on it — without
    /// this the approval prompt only appears the next time the window happens to redraw.
    wake: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    /// The desktop's single-writer bridge (ADR-0002): `None` until `set_write` is called at
    /// startup. Mutating agent ops error honestly when it is missing.
    write: Mutex<Option<Arc<AgentWriteFn>>>,
    /// Parked `write_tags` requests and their body digests, keyed by job id. In memory like the
    /// rest of the queue: a restart loses an unapproved request visibly, never a committed one —
    /// committed requests have their receipt in the catalog.
    pending_tag_batches: Mutex<std::collections::HashMap<String, (TagBatchRequest, String)>>,
    /// The producer this host's agent writes tags under (`agent_producer_id`). `None` refuses
    /// `write_tags` rather than attribute an agent's tags to a producer nobody chose.
    agent_producer: Mutex<Option<String>>,
    /// Set only by a headless server: where app-owned operations go ([`AppProxyFn`]).
    app_proxy: Mutex<Option<Arc<AppProxyFn>>>,
}

impl ServerState {
    pub fn new(
        catalog_path: &Path,
        workspace_dir: PathBuf,
        preview: PreviewFn,
    ) -> lodestash_core::error::Result<Self> {
        Ok(Self {
            // Read-only, and refused at any other schema version (v1 plan S11): every write this
            // state makes goes through the bridge to the one writer, and migrating is the writer's
            // job, not a side effect of an agent connecting.
            catalog: Mutex::new(Catalog::open_reader(catalog_path)?),
            jobs: jobs::Jobs::default(),
            workspace_dir,
            preview,
            query_embed: Mutex::new(None),
            token: new_token(),
            allowed: Mutex::new(None),
            byte_release: AtomicBool::new(false),
            pending_exports: Mutex::new(std::collections::HashMap::new()),
            pending_imports: Mutex::new(std::collections::HashMap::new()),
            import_queue: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            write: Mutex::new(None),
            pending_tag_batches: Mutex::new(std::collections::HashMap::new()),
            agent_producer: Mutex::new(None),
            app_proxy: Mutex::new(None),
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

    /// Whether the agent planes may serve asset bytes (v1 plan S10). The GUI sets it from
    /// `mcp_bytes_enabled`; a headless server without a GUI to ask keeps the default, which is no.
    pub fn set_byte_release(&self, on: bool) {
        self.byte_release
            .store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// May bytes be served to an agent right now?
    pub fn byte_release(&self) -> bool {
        self.byte_release.load(std::sync::atomic::Ordering::Relaxed)
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

    /// The catalog this state reads (M31).
    pub fn catalog_id(&self) -> lodestash_core::error::Result<String> {
        self.catalog.lock().unwrap().catalog_id()
    }

    /// `get_server_info` for both planes (ENGINE-API §4, §8.4): the versions, plus the facts a
    /// client should check rather than assume from a saved example — which catalog this is, who
    /// can write it, whether asset bytes are released, and which tools are switched on.
    ///
    /// `writer` is `in_process` inside the app; for a headless server it is the answer of the app
    /// it would forward to, asked now — `app` when that app has this catalog open, otherwise
    /// `app_unavailable` with the reason. `none` is a state with no way to write at all.
    pub fn server_info_json(&self) -> lodestash_core::error::Result<serde_json::Value> {
        let (app, schema) = self.server_info()?;
        let catalog_id = self.catalog_id()?;
        let (writer, writer_detail) = if self.write.lock().unwrap().is_some() {
            ("in_process", None)
        } else if let Some(proxy) = self.app_proxy() {
            match proxy("GET", "/v1/server-info", None) {
                Ok((200, _)) => ("app", None),
                Ok((status, body)) => (
                    "app_unavailable",
                    Some(format!("the app answered {status}: {body}")),
                ),
                Err(e) => ("app_unavailable", Some(e)),
            }
        } else {
            ("none", None)
        };
        let enabled: Vec<&str> = AGENT_OPERATIONS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| self.allows(name))
            .collect();
        Ok(serde_json::json!({
            "app": app,
            "schema": schema,
            "api": "v1",
            "catalog_id": catalog_id,
            "writer": writer,
            "writer_detail": writer_detail,
            "bytes_released": self.byte_release(),
            "enabled_tools": enabled,
        }))
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
        // Resident per embedding generation rather than rebuilt per request (v1 plan S09), and
        // scored over the assets that are eligible *now* — a trashed asset must not spend a slot.
        let snapshot = cat.embedding_snapshot(&model_id)?;
        let eligible = cat.eligible_for_search(None)?;
        let ids: Vec<i64> = snapshot
            .index
            .knn_eligible(&query, limit + 1, &eligible)
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
        self.write(AgentWrite::RegisterProject {
            project_path: project_path.to_string(),
            unity_version: unity_version.map(str::to_string),
            pipeline: pipeline.map(str::to_string),
        })
        .map_err(lodestash_core::error::CoreError::Catalog)?;
        // The writer committed; this connection reads the row back rather than trusting a number
        // passed through a bridge built to count changed rows.
        let id = self
            .catalog
            .lock()
            .unwrap()
            .list_projects()?
            .into_iter()
            .find(|p| p.project_path == project_path)
            .map(|p| p.id)
            .ok_or_else(|| {
                lodestash_core::error::CoreError::Catalog(
                    "the project was registered but cannot be read back".into(),
                )
            })?;
        Ok((id, true))
    }

    /// Registered editors, active first (ENGINE-API §5.1 `list_projects`).
    pub fn list_projects(&self) -> lodestash_core::error::Result<Vec<ProjectRow>> {
        self.catalog.lock().unwrap().list_projects()
    }

    /// Route app-owned operations to the running app (v1 plan S11). A headless server calls this
    /// at startup; the GUI never does, because it *is* the app.
    pub fn set_app_proxy(&self, f: AppProxyFn) {
        *self.app_proxy.lock().unwrap() = Some(Arc::new(f));
    }

    /// The line to the running app, when this state belongs to a headless server.
    pub fn app_proxy(&self) -> Option<Arc<AppProxyFn>> {
        self.app_proxy.lock().unwrap().clone()
    }

    /// The desktop's single-writer bridge. Called once at startup, before any mutating op is
    /// served; the API errors honestly until then (a headless server without the desktop is
    /// read-only, which is the correct default).
    pub fn set_write(&self, f: AgentWriteFn) {
        *self.write.lock().unwrap() = Some(Arc::new(f));
    }

    /// The producer this host's agent writes under (ENGINE-API §8.3). A blank id is no id.
    pub fn set_agent_producer(&self, producer: Option<String>) {
        *self.agent_producer.lock().unwrap() = producer.filter(|p| !p.trim().is_empty());
    }

    /// ENGINE-API §8.3 `write_tags`: validate the whole request, answer a replay from its durable
    /// receipt or its pending job, and otherwise park it for the person at the machine to approve.
    /// Nothing is written here — the write happens on approval, in the app's single writer.
    pub fn request_tag_batch(
        &self,
        req: TagBatchRequest,
        body_len: usize,
    ) -> Result<TagBatchKickoff, TagBatchError> {
        use lodestash_core::catalog::{
            ai_source, validate_assignments, TagAssignment, MAX_BATCH_ASSETS, MAX_DESCRIPTION_CHARS,
        };
        if body_len > MAX_TAG_BATCH_BYTES {
            return Err(TagBatchError::TooLarge(body_len));
        }
        if uuid::Uuid::parse_str(&req.request_id).is_err() {
            return Err(TagBatchError::Invalid(format!(
                "request_id {:?} is not a UUID",
                req.request_id
            )));
        }
        let producer = self
            .agent_producer
            .lock()
            .unwrap()
            .clone()
            .ok_or(TagBatchError::ProducerNotConfigured)?;
        if req.assets.is_empty() || req.assets.len() > MAX_BATCH_ASSETS {
            return Err(TagBatchError::Invalid(format!(
                "{} assets; a batch holds 1 to {MAX_BATCH_ASSETS}",
                req.assets.len()
            )));
        }
        let source = ai_source(&producer);
        let mut seen = HashSet::new();
        for a in &req.assets {
            if !seen.insert(a.asset_id) {
                return Err(TagBatchError::Invalid(format!(
                    "asset {} appears twice",
                    a.asset_id
                )));
            }
            if a.producer_id != producer {
                return Err(TagBatchError::ProducerMismatch(a.producer_id.clone()));
            }
            if a.mode != "append" && a.mode != "replace_producer" {
                return Err(TagBatchError::Invalid(format!(
                    "mode {:?} is not append or replace_producer",
                    a.mode
                )));
            }
            if a.expected_revision.trim().is_empty() {
                return Err(TagBatchError::Invalid(
                    "expected_revision is required — read it from get_asset".into(),
                ));
            }
            if let Some(d) = &a.description {
                let text = d.trim();
                if text.is_empty() || text.chars().count() > MAX_DESCRIPTION_CHARS {
                    return Err(TagBatchError::Invalid(format!(
                        "asset {}: a description is 1 to {MAX_DESCRIPTION_CHARS} characters",
                        a.asset_id
                    )));
                }
            }
            let tags: Vec<TagAssignment> = a
                .tags
                .iter()
                .map(|t| TagAssignment {
                    namespace: t.namespace.clone(),
                    value: t.value.clone(),
                    confidence: t.confidence,
                })
                .collect();
            validate_assignments(&source, &tags)
                .map_err(|e| TagBatchError::Invalid(format!("asset {}: {e}", a.asset_id)))?;
        }
        for a in &req.assets {
            if self
                .asset(a.asset_id)
                .map_err(|e| TagBatchError::Catalog(e.to_string()))?
                .is_none()
            {
                return Err(TagBatchError::AssetNotFound(a.asset_id));
            }
        }
        let digest = blake3::hash(
            serde_json::to_string(&req)
                .expect("a parsed request serializes")
                .as_bytes(),
        )
        .to_hex()
        .to_string();
        // A committed request answers from its receipt, however long ago it committed.
        if let Some(r) = self
            .catalog
            .lock()
            .unwrap()
            .tag_request_receipt(&req.request_id)
            .map_err(|e| TagBatchError::Catalog(e.to_string()))?
        {
            return if r.digest == digest {
                Ok(TagBatchKickoff::Replayed {
                    request_id: req.request_id,
                    result: r.result,
                })
            } else {
                Err(TagBatchError::RequestConflict(req.request_id))
            };
        }
        // A parked one answers with its job.
        {
            let pending = self.pending_tag_batches.lock().unwrap();
            if let Some((job_id, (_, d))) = pending
                .iter()
                .find(|(_, (p, _))| p.request_id == req.request_id)
            {
                return if *d == digest {
                    self.jobs
                        .get(job_id)
                        .map(TagBatchKickoff::Awaiting)
                        .ok_or_else(|| TagBatchError::Catalog("the parked job is gone".into()))
                } else {
                    Err(TagBatchError::RequestConflict(req.request_id))
                };
            }
        }
        let detail = self.tag_batch_detail(&req, &source);
        let job = self.jobs.create(
            "write_tags",
            format!("Write tags on {} asset(s) as {source}", req.assets.len()),
            jobs::JobState::AwaitingApproval,
        );
        job.set_detail(detail);
        self.pending_tag_batches
            .lock()
            .unwrap()
            .insert(job.id.clone(), (req, digest));
        self.wake();
        Ok(TagBatchKickoff::Awaiting(job))
    }

    /// What the approval prompt shows: per asset, this producer's current tags and the proposal,
    /// and the proposed caption.
    fn tag_batch_detail(&self, req: &TagBatchRequest, source: &str) -> serde_json::Value {
        let cat = self.catalog.lock().unwrap();
        let assets: Vec<serde_json::Value> = req
            .assets
            .iter()
            .map(|a| {
                let current: Vec<String> = cat
                    .asset_tag_rows(a.asset_id)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|r| r.source == source)
                    .map(|r| format!("{}:{}", r.namespace, r.value))
                    .collect();
                let proposed: Vec<String> = a
                    .tags
                    .iter()
                    .map(|t| format!("{}:{}", t.namespace, t.value))
                    .collect();
                serde_json::json!({
                    "asset_id": a.asset_id,
                    "mode": a.mode,
                    "current": current,
                    "proposed": proposed,
                    "description": a.description,
                })
            })
            .collect();
        serde_json::json!({ "producer": source, "assets": assets })
    }

    /// Run an approved request through the app's writer, then answer from its receipt — the
    /// catalog's own record of what committed, not this process's memory of it.
    fn run_tag_batch(&self, job: Arc<jobs::Job>, req: TagBatchRequest, digest: String) {
        job.set_state(jobs::JobState::Running);
        let request_id = req.request_id.clone();
        if let Err(e) = self.write(AgentWrite::TagBatch {
            request: req,
            digest,
        }) {
            return job.fail(e);
        }
        match self
            .catalog
            .lock()
            .unwrap()
            .tag_request_receipt(&request_id)
        {
            Ok(Some(r)) => job.finish(r.result),
            Ok(None) => job.fail("the write reported success but left no receipt"),
            Err(e) => job.fail(e.to_string()),
        }
    }

    /// ENGINE-API §3 asset detail — one builder for both planes, carrying each tag's provenance
    /// and the `annotation_revision` a `write_tags` item must name.
    pub fn asset_detail_json(
        &self,
        id: i64,
    ) -> lodestash_core::error::Result<Option<serde_json::Value>> {
        let Some(row) = self.asset(id)? else {
            return Ok(None);
        };
        let deps = self.dependencies(id).unwrap_or_default();
        let mut body = routes::summary_json(&row);
        body["size_bytes"] = serde_json::json!(row.size_bytes);
        body["stats"] = serde_json::json!({
            "tris": row.metrics.tri_count, "width_px": row.metrics.width_px,
            "height_px": row.metrics.height_px, "duration_ms": row.metrics.duration_ms,
        });
        body["dependencies"] = serde_json::json!({ "count": deps.len(), "asset_ids": deps });
        let cat = self.catalog.lock().unwrap();
        body["annotation_revision"] = serde_json::json!(cat.annotation_revision(id)?);
        body["tag_provenance"] = serde_json::to_value(cat.asset_tag_rows(id)?)?;
        Ok(Some(body))
    }

    fn write(&self, w: AgentWrite) -> Result<usize, String> {
        self.write
            .lock()
            .unwrap()
            .as_ref()
            .map(|f| f(w))
            .unwrap_or_else(|| Err("the engine has no writer bridge — read-only mode".into()))
    }

    /// ENGINE-API §2 `list_tags` — the tag vocabulary with usage counts, most-used first.
    pub fn list_tags(
        &self,
        prefix: &str,
        limit: usize,
    ) -> lodestash_core::error::Result<Vec<TagRow>> {
        let cat = self.catalog.lock().unwrap();
        let all = cat.tag_usage(limit)?;
        Ok(if prefix.is_empty() {
            all.into_iter().map(Into::into).collect()
        } else {
            all.into_iter()
                .filter(|(ns, v, _)| ns.starts_with(prefix) || v.starts_with(prefix))
                .map(Into::into)
                .collect()
        })
    }

    /// ENGINE-API §2 `list_collections` — the named collections plus (per the doc) the active
    /// project's staging tray, flagged so a client can render it apart.
    pub fn list_collections(&self) -> lodestash_core::error::Result<Vec<CollectionRowApi>> {
        let cat = self.catalog.lock().unwrap();
        let mut rows: Vec<CollectionRowApi> = cat
            .list_collections()?
            .into_iter()
            .map(Into::into)
            .collect();
        // The tray is listed whether or not an editor is registered — same resolver as
        // `list_staged`, so the two cannot disagree about whether a tray exists (ADR-0010).
        if let Some(tray) = cat.active_staging_tray_id()? {
            let count = cat.collection_members(tray)?.len();
            rows.push(CollectionRowApi {
                id: tray,
                name: "Staging".into(),
                is_staging: true,
                asset_count: count,
            });
        }
        Ok(rows)
    }

    /// ENGINE-API §2 `suggest_tags` — the asset's current tags with confidences. The zero-shot
    /// tagger's output IS the suggestion; an un-tagged asset answers `[]` honestly.
    pub fn suggest_tags(&self, asset_id: i64) -> lodestash_core::error::Result<Vec<TagProposal>> {
        let cat = self.catalog.lock().unwrap();
        Ok(cat
            .tags_for_asset(asset_id)?
            .into_iter()
            .map(|t| TagProposal {
                namespace: t.namespace,
                value: t.value,
                confidence: t.confidence,
            })
            .collect())
    }

    /// ENGINE-API §2 `add_tag` — one `(namespace, value)` on one asset, through the writer.
    pub fn add_tag(&self, asset_id: i64, namespace: &str, value: &str) -> Result<usize, String> {
        self.write(AgentWrite::AddTag {
            asset_id,
            namespace: namespace.to_string(),
            value: value.to_string(),
        })
    }

    /// ENGINE-API §2 `apply_mapping` — run the deterministic mapping engine over one taxonomy.
    pub fn apply_mapping(&self, taxonomy_id: i64) -> Result<usize, String> {
        self.write(AgentWrite::ApplyMapping { taxonomy_id })
    }

    /// ENGINE-API §2 `identify_asset` — long-running: parks a job and appraises the set on a
    /// worker thread through the writer bridge. `get_job_status` reports progress/result.
    pub fn identify_asset(&self, asset_ids: Vec<i64>) -> Result<Arc<jobs::Job>, String> {
        if asset_ids.is_empty() {
            return Err("asset_ids must not be empty".into());
        }
        let job = self.jobs.create(
            "identify",
            format!("Identify {} assets", asset_ids.len()),
            jobs::JobState::Running,
        );
        let job2 = job.clone();
        let write = self.write.lock().unwrap().clone();
        std::thread::spawn(move || match write {
            Some(f) => match f(AgentWrite::Identify { asset_ids }) {
                Ok(n) => job2.finish(serde_json::json!({ "changed": n })),
                Err(e) => job2.fail(e),
            },
            None => job2.fail("no writer bridge — read-only mode"),
        });
        Ok(job)
    }

    /// ENGINE-API §2 `unpack_package` — long-running: extracts one pack through the writer
    /// bridge on a worker thread.
    pub fn unpack_package(&self, package_id: i64) -> Result<Arc<jobs::Job>, String> {
        let job = self.jobs.create(
            "unpack",
            format!("Unpack package {package_id}"),
            jobs::JobState::Running,
        );
        let job2 = job.clone();
        let write = self.write.lock().unwrap().clone();
        std::thread::spawn(move || match write {
            Some(f) => match f(AgentWrite::UnpackPackage { package_id }) {
                Ok(n) => job2.finish(serde_json::json!({ "changed": n })),
                Err(e) => job2.fail(e),
            },
            None => job2.fail("no writer bridge — read-only mode"),
        });
        Ok(job)
    }

    /// ENGINE-API §6 `stage_asset` / `unstage` — the active project's staging shortlist.
    pub fn stage_asset(&self, asset_id: i64) -> Result<usize, String> {
        self.write(AgentWrite::StageAsset { asset_id })
    }

    pub fn unstage_asset(&self, asset_id: i64) -> Result<usize, String> {
        self.write(AgentWrite::UnstageAsset { asset_id })
    }

    /// ENGINE-API §6 `list_staged` — the current staging tray as asset summaries.
    ///
    /// "Current" is the active project's tray, or the project-less one when no editor is
    /// registered (M20). It used to answer `[]` without a project on the reading that *"there is
    /// nothing staged anywhere without an editor"* — which stopped being true the moment the
    /// shell started staging into a tray of its own. Both planes resolve through
    /// `active_staging_tray_id` now, so an agent asking what is staged gets the picks the person
    /// can see (ADR-0010). `None` still means nothing has ever been staged.
    pub fn list_staged(&self) -> lodestash_core::error::Result<Vec<AssetSummaryRow>> {
        let cat = self.catalog.lock().unwrap();
        let Some(tray) = cat.active_staging_tray_id()? else {
            return Ok(Vec::new());
        };
        let ids = cat.collection_members(tray)?;
        cat.asset_summaries(&ids)
    }

    /// ENGINE-API §6 `import_staged` — queue the staged shortlist for import into the active
    /// project (the same approval + queue path as `import_to_project`).
    pub fn import_staged(&self) -> Result<ImportKickoff, ImportError> {
        let ids = self.list_staged().map_err(|_| ImportError::Empty)?;
        self.request_import(ImportRequest {
            project_id: None,
            asset_ids: ids.into_iter().map(|a| a.id).collect(),
            dest_subfolder: None,
            with_dependencies: true,
        })
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
        // §5.4 advisory: warn where the pack and the project target different render pipelines.
        // Computed over what the caller *asked for*, not only what is new — a re-import of an
        // already-present asset is still worth the warning.
        let pipeline_mismatch = self
            .catalog
            .lock()
            .unwrap()
            .pipeline_mismatches(&req.asset_ids, project_id)
            .unwrap_or_default();
        if fresh.is_empty() {
            return Ok(ImportKickoff::AllAlreadyImported {
                imported_existing: already,
                pipeline_mismatch,
            });
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
        Ok(ImportKickoff::Awaiting {
            job,
            pipeline_mismatch,
        })
    }

    /// The approved import is **enqueued** for the plugin here (ENGINE-API §5.3) — not recorded. It
    /// resolves the dependency closure (a mesh drags its materials + textures — same resolver as
    /// export) and appends a queue entry; the job stays `running` until a plugin drains it via
    /// [`Self::poll_import_queue`] and acks via [`Self::ack_import`], which writes the durable
    /// `import_record` with Unity's real path + GUID. With no editor connected the job simply stays
    /// in flight — the import is pending, not failed.
    fn enqueue_import(&self, job: std::sync::Arc<jobs::Job>, req: ImportRequest, project_id: i64) {
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
        self.import_queue.lock().unwrap().push(ImportQueueEntry {
            request_id: format!("req_{}", uuid::Uuid::new_v4().simple()),
            job_id: job.id.clone(),
            project_id,
            asset_ids: ids,
            leased_until: None,
            with_deps: req.with_dependencies,
            dest_subfolder: req.dest_subfolder,
        });
    }

    /// The plugin drains this project's pending imports (ENGINE-API §5.3 `GET /v1/import-queue`).
    /// Each payload carries the assets' summaries (with `file_url`s) so the plugin can fetch bytes.
    ///
    /// ponytail: returns immediately with whatever is pending (`[]` when nothing is). The spec's
    /// ~25s long-poll is a busy-poll optimisation, not a correctness requirement — add a
    /// `tokio::sync::Notify` wait here if a real plugin's poll rate ever matters.
    pub fn poll_import_queue(
        &self,
        project_id: i64,
    ) -> lodestash_core::error::Result<Vec<ImportQueuePayload>> {
        let pending: Vec<(String, String, Option<String>, Vec<i64>)> = {
            let mut q = self.import_queue.lock().unwrap();
            let now = std::time::Instant::now();
            q.iter_mut()
                // Skip anything already handed out and still inside its lease.
                .filter(|e| {
                    // ponytail: map_or, not is_none_or — the workspace MSRV is 1.80.
                    e.project_id == project_id && e.leased_until.map_or(true, |until| until <= now)
                })
                .map(|e| {
                    e.leased_until = Some(now + IMPORT_LEASE);
                    (
                        e.request_id.clone(),
                        e.job_id.clone(),
                        e.dest_subfolder.clone(),
                        e.asset_ids.clone(),
                    )
                })
                .collect()
        };
        let cat = self.catalog.lock().unwrap();
        pending
            .into_iter()
            .map(|(request_id, job_id, dest_subfolder, ids)| {
                Ok(ImportQueuePayload {
                    request_id,
                    job_id,
                    dest_subfolder,
                    assets: cat.asset_summaries(&ids)?,
                })
            })
            .collect()
    }

    /// The plugin reports an import finished (ENGINE-API §5.3 `POST /v1/import-queue/{req}/ack`):
    /// remove the queued request, write one durable `import_record` per acked asset (with the real
    /// `dest_path`/`guid` Unity assigned — the dup-guard still applies), and flip the job `done`.
    pub fn ack_import(&self, request_id: &str, imported: Vec<AckAsset>) -> Result<(), AckError> {
        let entry = {
            let mut q = self.import_queue.lock().unwrap();
            let pos = q
                .iter()
                .position(|e| e.request_id == request_id)
                .ok_or(AckError::UnknownRequest)?;
            // The ack must only claim assets this request actually queued. Without the check, any
            // holder of the session token could write durable `import_record` rows for arbitrary
            // assets — and that record is the part that survives a crash. Reject the whole request
            // rather than filtering: a client acking ids it was never given is malformed, and
            // quietly dropping them would hide it.
            if let Some(bad) = imported
                .iter()
                .find(|a| !q[pos].asset_ids.contains(&a.asset_id))
            {
                tracing::warn!(
                    request_id,
                    asset_id = bad.asset_id,
                    "ack claimed an asset that was not queued for this request"
                );
                return Err(AckError::UnknownRequest);
            }
            q.remove(pos)
        };
        let ids: Vec<i64> = imported.iter().map(|a| a.asset_id).collect();
        let rows = self
            .catalog
            .lock()
            .unwrap()
            .asset_summaries(&ids)
            .map_err(AckError::Db)?;
        let specs: Vec<ImportSpec> = imported
            .iter()
            .map(|a| ImportSpec {
                asset_id: a.asset_id,
                source_package_id: rows
                    .iter()
                    .find(|r| r.id == a.asset_id)
                    .and_then(|r| r.package_id),
                dest_path: a.dest_path.clone(),
                unity_guid: a.unity_guid.clone(),
                with_deps: entry.with_deps,
            })
            .collect();
        let batch_id = format!("imp-{}", entry.job_id);
        let requested = specs.len();
        let recorded = self
            .write(AgentWrite::RecordImports {
                project_id: entry.project_id,
                specs,
                batch_id: batch_id.clone(),
            })
            .map_err(|e| AckError::Db(lodestash_core::error::CoreError::Catalog(e)))?;
        if let Some(job) = self.jobs.get(&entry.job_id) {
            job.finish(serde_json::json!({
                "project_id": entry.project_id,
                "recorded": recorded,
                "already_imported": requested.saturating_sub(recorded),
                "batch_id": batch_id,
            }));
        }
        Ok(())
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
            let state = self.clone();
            std::thread::spawn(move || state.enqueue_import(job, req, project_id));
            return;
        }
        if let Some((req, digest)) = self.pending_tag_batches.lock().unwrap().remove(job_id) {
            let state = self.clone();
            std::thread::spawn(move || state.run_tag_batch(job, req, digest));
        }
    }

    /// The desktop app said no.
    pub fn deny_job(&self, job_id: &str) {
        self.pending_exports.lock().unwrap().remove(job_id);
        self.pending_imports.lock().unwrap().remove(job_id);
        self.pending_tag_batches.lock().unwrap().remove(job_id);
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
        let report = match self.copy_export(&req, &dest, &job) {
            Ok(r) => r,
            Err(e) => return job.fail(e),
        };
        if job.is_cancelled() {
            job.set_state(jobs::JobState::Cancelled);
            tracing::info!(%job.id, written = report.written, "export cancelled part-way");
            return;
        }
        job.finish(report.to_json(req.with_dependencies));
    }

    /// The export itself: resolve the closure, resolve the files, copy them.
    ///
    /// One body, two entry points — the approval-gated agent path ([`Self::run_export`]) and the
    /// desktop’s own [`Self::export_now`]. They must not drift, because "what does an export
    /// actually copy" is the question both of them are answering.
    fn copy_export(
        &self,
        req: &export::ExportRequest,
        dest: &Path,
        job: &jobs::Job,
    ) -> Result<export::ExportReport, String> {
        // With dependencies, the copy set is the transitive closure over the P2 relationship
        // graph (a material drags its textures). Edges that resolve to nothing in the library
        // add nothing — the closure only walks `resolved` ones.
        let ids = if req.with_dependencies {
            self.catalog
                .lock()
                .unwrap()
                .dependency_closure(&req.asset_ids)
                .map_err(|e| e.to_string())?
        } else {
            req.asset_ids.clone()
        };
        let rows = self
            .catalog
            .lock()
            .unwrap()
            .asset_summaries(&ids)
            .map_err(|e| e.to_string())?;

        // Two reasons a pick contributes no file, told apart because they have different fixes:
        // an in-pack asset needs its pack extracted, a missing one needs its drive back.
        let mut files: Vec<(PathBuf, String)> = Vec::new();
        // Which asset each written file came from, so the history can point back at it.
        let mut owner: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
        let mut in_pack = 0usize;
        let mut unreachable = 0usize;
        for r in &rows {
            if let Some(p) = self.source_path(r) {
                if p.exists() {
                    let name = file_name_for(r);
                    owner.insert(name.clone(), r.id);
                    files.push((p, name));
                    continue;
                }
            }
            // `disk_path` is **synthesised** as `root_path/rel_path` for every row without a
            // `workspace_rel` — including an in-pack asset, whose bytes are inside the archive and
            // never at that path. So the path existing is not what tells the two apart: a row that
            // belongs to a package and has not been extracted is in-pack, and everything else that
            // failed to resolve is genuinely out of reach.
            if r.package_id.is_some() && r.workspace_rel.is_none() {
                in_pack += 1;
            } else {
                unreachable += 1;
            }
        }
        // Ids the catalog could not describe at all are counted too, or the numbers would not add
        // up to what the caller asked for.
        unreachable += ids.len().saturating_sub(rows.len());

        let names = export::copy_files(&files, dest, job).map_err(|e| e.to_string())?;

        // Export history (FEATURES §7.6) — recorded from what was **written**, not what was asked
        // for, and recorded for the agent plane too: an export an agent ran still put files on the
        // user's disk, and they are entitled to see it and undo it.
        //
        // A failure to record is not a failure to export. The files exist either way, and reporting
        // a successful copy as an error would be the worse lie of the two.
        if !names.is_empty() {
            let rows: Vec<(Option<i64>, String)> = names
                .iter()
                .map(|n| (owner.get(n).copied(), n.clone()))
                .collect();
            if let Err(e) = self.write(AgentWrite::RecordExports {
                batch_id: job.id.clone(),
                dest_path: dest.display().to_string(),
                files: rows,
            }) {
                tracing::warn!(%e, "export history not recorded; the files were still written");
            }
        }

        Ok(export::ExportReport {
            written: names.len() as u64,
            in_pack,
            unreachable,
            dependencies_added: ids.len().saturating_sub(req.asset_ids.len()),
            dest_path: dest.display().to_string(),
        })
    }

    /// Export **now**, without the approval gate (UI-SPEC §16.2).
    ///
    /// The gate exists because an *agent* asked (ENGINE-API §4: nothing is copied until the person
    /// at the desktop app approves). Here that person is the one asking — they picked the
    /// destination in a native folder dialog, which is a more deliberate act than a prompt they
    /// would approve after the fact. Asking them to confirm their own click would be theatre, and
    /// would train them to dismiss the prompt that does matter.
    ///
    /// The **sandbox still applies in full**: absolute, no `..`, and never inside a library source
    /// root. That rule protects the non-destructive invariant, not the agent boundary, so it holds
    /// for every caller.
    ///
    /// Synchronous on purpose. The caller reports what happened, and it cannot do that honestly
    /// from a thread it never waited for.
    pub fn export_now(&self, req: export::ExportRequest) -> Result<export::ExportReport, String> {
        let roots = self
            .catalog
            .lock()
            .unwrap()
            .source_roots()
            .unwrap_or_default();
        let dest =
            export::check_dest(&req.dest_path, &roots).map_err(|e| e.message().to_string())?;
        let summary = format!(
            "Export {} asset(s) to {}",
            req.asset_ids.len(),
            dest.display()
        );
        let job = self.jobs.create("export", summary, jobs::JobState::Running);
        match self.copy_export(&req, &dest, &job) {
            Ok(report) => {
                job.finish(report.to_json(req.with_dependencies));
                Ok(report)
            }
            // A copy failure is not a sandbox refusal and must not read like one.
            Err(e) => {
                job.fail(e.clone());
                Err(e)
            }
        }
    }

    /// The asset's preview PNG on disk, generated on a cache miss.
    pub fn preview_path(&self, id: i64) -> Option<PathBuf> {
        (self.preview)(&self.preview_subject(id)?)
    }

    /// Resolve everything a preview needs for one asset, from the catalog, so the renderer needs
    /// no database access of its own.
    ///
    /// Built because the old shape — "give the resolver the source path" — could not describe the
    /// majority of a real library: an **in-pack** asset has no file at that path (the summary
    /// query fabricates `root_path/rel_path` for it), so every preview call for one failed, and
    /// a **material** has no pixels of its own at all. The agent plane could not see a single
    /// asset of any type as a result.
    pub fn preview_subject(&self, id: i64) -> Option<PreviewSubject> {
        let row = self.asset(id).ok().flatten()?;
        // Only a path that is actually there. `disk_path` is fabricated for in-pack rows.
        let disk_path = self.source_path(&row).filter(|p| p.is_file());
        let cat = self.catalog.lock().unwrap();
        let in_pack = if disk_path.is_none() {
            cat.in_pack_locator(id).ok().flatten()
        } else {
            None
        };
        // A material wears a texture reached through the dependency graph (DATA-MODEL §10). That
        // texture is as often still in a pack as the material is, so resolve *both* shapes — a
        // locator the renderer can pull, not only a path that happens to already exist.
        let texture = if row.media_type == "material" {
            cat.material_albedo_texture(id).ok().flatten()
        } else {
            None
        };
        let albedo = texture
            .and_then(|tex| cat.asset_summaries(&[tex]).ok())
            .and_then(|rows| rows.into_iter().next())
            .and_then(|t| t.disk_path.map(PathBuf::from))
            .filter(|p| p.is_file());
        let albedo_in_pack = match (texture, &albedo) {
            (Some(tex), None) => cat.in_pack_locator(tex).ok().flatten(),
            _ => None,
        };
        // A model wears one texture per material slot; the loader prefers whatever the model file
        // declares itself, so this is only what the graph resolved for the rest (v1 plan S08).
        let slot_textures = if row.media_type == "model_3d" {
            cat.model_slot_texture_paths(id, Some(&self.workspace_dir))
                .unwrap_or_default()
        } else {
            Default::default()
        };
        drop(cat);
        Some(PreviewSubject {
            media_type: row.media_type,
            disk_path,
            in_pack,
            albedo,
            albedo_in_pack,
            slot_textures,
        })
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

/// A stand-in for the desktop's writer in this crate's tests: its own writable connection, doing
/// the writes the engine's read-only connection hands to the bridge (v1 plan S11). Not the writer
/// thread — just enough to prove the state asks for them instead of writing itself.
#[cfg(test)]
pub(crate) fn test_writes(db: &Path) -> AgentWriteFn {
    let cat = Mutex::new(Catalog::open(db).expect("test writer opens"));
    Box::new(move |w| {
        let cat = cat.lock().unwrap();
        match w {
            AgentWrite::RegisterProject {
                project_path,
                unity_version,
                pipeline,
            } => cat
                .register_project(&project_path, unity_version.as_deref(), pipeline.as_deref())
                .map(|_| 1),
            AgentWrite::RecordImports {
                project_id,
                specs,
                batch_id,
            } => cat.record_imports(project_id, &specs, &batch_id).map(|o| {
                o.iter()
                    .filter(|o| {
                        matches!(o, lodestash_core::catalog::ImportOutcome::Recorded { .. })
                    })
                    .count()
            }),
            AgentWrite::RecordExports {
                batch_id,
                dest_path,
                files,
            } => cat.record_exports(&batch_id, &dest_path, &files),
            other => return Err(format!("the test writer does not do {other:?}")),
        }
        .map_err(|e| e.to_string())
    })
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
    let mut handshake = Handshake::new(port, state.token());
    handshake.catalog_id = state.catalog_id().ok();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's state reads through a connection that cannot migrate or write (v1 plan S11):
    /// a catalog the app has not upgraded yet is refused with the instruction, not upgraded by
    /// whichever agent host happened to connect first — and the project an agent registers is
    /// written by the writer it hands the request to.
    #[test]
    fn the_engine_state_neither_migrates_nor_writes_the_catalog() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        Catalog::open(&db).unwrap();
        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch("PRAGMA user_version = 29;")
            .unwrap();
        let refused = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None))
            .err()
            .expect("an old catalog must be refused");
        assert!(
            refused.to_string().contains("open the Lodestash app once"),
            "{refused}"
        );
        let version: u32 = rusqlite::Connection::open(&db)
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(version, 29, "connecting an agent upgraded the catalog");

        rusqlite::Connection::open(&db)
            .unwrap()
            .execute_batch(&format!(
                "PRAGMA user_version = {};",
                lodestash_core::catalog::SCHEMA_VERSION
            ))
            .unwrap();
        let state = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap();
        assert!(
            state.register_project("C:/dev/Game", None, None).is_err(),
            "with no writer bridge, registering must fail rather than write through the reader"
        );
        state.set_write(test_writes(&db));
        let (id, _) = state
            .register_project("C:/dev/Game", None, None)
            .expect("through the bridge");
        assert_eq!(
            state.list_projects().unwrap()[0].id,
            id,
            "the reader sees the writer's row"
        );
    }

    /// An in-pack material must still resolve to something renderable.
    ///
    /// `preview_subject` only ever offered an albedo it could find *on disk*, so a material inside
    /// an un-extracted pack — whose texture is in the same pack — handed the renderer nothing to
    /// key off and nothing to paint, and the shader ball was never drawn at all.
    #[test]
    fn an_in_pack_material_carries_both_its_own_locator_and_its_albedos() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        {
            // Create the schema, then seed through a second connection: `Catalog::conn` is
            // crate-private, and these rows are fixture shapes no public setter builds.
            lodestash_core::catalog::Catalog::open(&db).unwrap();
            rusqlite::Connection::open(&db)
                .unwrap()
                .execute_batch(
                    "INSERT INTO import_source (id, uuid, name, root_path, kind)
                         VALUES (1, 'src-1', 's', '/s', 'folder');
                     INSERT INTO media_type (id, code, label)
                         VALUES (1, 'material', 'Material'), (2, 'image', 'Image');
                     INSERT INTO package (id, uuid, source_id, package_path, name)
                         VALUES (10, 'p1', 1, 'G:/cache/Pack.unitypackage', 'Pack');
                     INSERT INTO package_entry (id, package_id, guid, pathname, tar_offset, size_bytes) VALUES
                         (100, 10, 'g1', 'Assets/Stone.mat', 4096, 512),
                         (101, 10, 'g2', 'Assets/Stone_albedo.png', 8192, 2048);
                     INSERT INTO asset (id, uuid, source_id, media_type_id, package_id, name, created_at, updated_at) VALUES
                         (1, 'a1', 1, 1, 10, 'Stone', 0, 0),
                         (2, 'a2', 1, 2, 10, 'Stone_albedo', 0, 0);
                     INSERT INTO asset_file (id, asset_id, rel_path, ext, size_bytes, mtime, package_entry_id, workspace_rel) VALUES
                         (1, 1, 'Stone.mat', 'mat', 512, 0, 100, NULL),
                         (2, 2, 'Stone_albedo.png', 'png', 2048, 0, 101, NULL);
                     UPDATE asset SET primary_file_id = 1 WHERE id = 1;
                     UPDATE asset SET primary_file_id = 2 WHERE id = 2;
                     INSERT INTO asset_relationship (from_asset_id, to_asset_id, kind, state)
                         VALUES (1, 2, 'uses_texture', 'resolved');",
                )
                .expect("fixture");
        }
        let state = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap();
        let s = state.preview_subject(1).expect("the material resolves");

        assert_eq!(s.media_type, "material");
        assert!(s.disk_path.is_none(), "an in-pack material has no file");
        assert_eq!(
            s.in_pack,
            Some((
                "G:/cache/Pack.unitypackage".to_string(),
                4096,
                512,
                "mat".to_string()
            )),
            "the ball is keyed off the material's own locator when there is no file to stat"
        );
        assert!(s.albedo.is_none(), "the texture is not on disk either");
        assert_eq!(
            s.albedo_in_pack,
            Some((
                "G:/cache/Pack.unitypackage".to_string(),
                8192,
                2048,
                "png".to_string()
            )),
            "the albedo is reachable through the graph and pullable through the same scratch"
        );
    }
}
