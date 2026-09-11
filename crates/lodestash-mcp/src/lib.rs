//! MCP server (rmcp, stdio transport). 1:1 with the loopback REST surface.
//! Spec: docs/MCP-AND-ENGINES.md, docs/ENGINE-API.md
//!
//! Every tool here calls a [`lodestash_api::ServerState`] operation — the same method the REST
//! handler calls. That's what makes "MCP↔REST 1:1" structural: an operation is written once and
//! both planes expose it, and the parity test fails the build if one plane grows a name the
//! other lacks.

use std::sync::Arc;

use lodestash_api::ServerState;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ProtocolVersion, Resource, ServerCapabilities, ServerInfo,
};
use rmcp::{tool, tool_handler, tool_router, ErrorData, ServerHandler};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct Lodestash {
    state: Arc<ServerState>,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Words to match against asset names, descriptions and tags. Empty returns the most
    /// recently catalogued assets.
    #[serde(default)]
    pub query: String,
    /// Maximum results (default 25).
    pub limit: Option<usize>,
    /// Restrict to one media type: model_3d | texture | audio | image | sprite | material | font.
    pub media_type: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SimilarArgs {
    /// The asset to find neighbours for.
    pub id: i64,
    /// Maximum neighbours (default 10).
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ExportArgs {
    /// Assets to copy out.
    pub asset_ids: Vec<i64>,
    /// Absolute destination folder. Must be outside every library source root.
    pub dest_path: String,
    /// Accepted but not yet honoured — the dependency graph arrives with package relationships.
    pub with_dependencies: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct JobArgs {
    /// Job id returned by a long-running tool.
    pub job_id: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AssetArgs {
    /// The asset's catalog id.
    pub id: i64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RegisterProjectArgs {
    /// Absolute path to the Unity project root.
    pub project_path: String,
    /// Editor version, e.g. "6000.0.23f1".
    pub unity_version: Option<String>,
    /// Render pipeline: "built_in" | "urp" | "hdrp".
    pub pipeline: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ImportArgs {
    /// Assets to import into the project.
    pub asset_ids: Vec<i64>,
    /// Target project id; omit to use the active editor.
    pub project_id: Option<i64>,
    /// Subfolder under `Assets/` to import into (default "Lodestash").
    pub dest_subfolder: Option<String>,
    /// Bring each asset's dependency closure — materials, textures (default true).
    pub with_dependencies: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct TagsArgs {
    /// Only tags whose namespace or value starts with this (optional).
    pub prefix: Option<String>,
    /// Maximum rows (default 500).
    pub limit: Option<usize>,
}

/// One proposed assignment (ENGINE-API §8.3).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteTagsTag {
    /// A facet a model may write: subject | style | material | mood | usage | free.
    pub namespace: String,
    pub value: String,
    /// Your confidence in [0, 1], or null.
    pub confidence: Option<f64>,
}

/// One asset's part of a `write_tags` request.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteTagsAsset {
    pub asset_id: i64,
    /// The `annotation_revision` get_asset reported. If anything changed since, the batch is refused.
    pub expected_revision: String,
    /// The producer id configured for this host in Lodestash.
    pub producer_id: String,
    /// The model you are, when you know it; null otherwise. Never guess.
    pub model_id: Option<String>,
    /// `append` (add to this producer's tags) or `replace_producer` (replace them).
    pub mode: String,
    pub tags: Vec<WriteTagsTag>,
    /// A short description; omit to leave the caption as it is.
    pub description: Option<String>,
}

/// `write_tags` — the same request `POST /v1/tags/batch` takes.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteTagsArgs {
    /// A fresh UUID per request. Reuse it only to retry the same body after a lost response.
    pub request_id: String,
    /// The vocabulary revision the proposals were checked against (enforced once review lands).
    pub vocabulary_revision: Option<String>,
    /// 1 to 100 assets.
    pub assets: Vec<WriteTagsAsset>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddTagArgs {
    /// The asset to tag.
    pub asset_id: i64,
    /// Namespace (default "free").
    pub namespace: Option<String>,
    /// The tag value, e.g. "rusty".
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MappingArgs {
    /// Taxonomy to run the deterministic mapping engine over.
    pub taxonomy_id: i64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct IdentifyArgs {
    /// Assets to flag + appraise.
    pub asset_ids: Vec<i64>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UnpackArgs {
    /// The pack to extract into the workspace.
    pub package_id: i64,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct StageArgs {
    /// The asset to add to (or remove from) the active project's staging tray.
    pub asset_id: i64,
}

#[tool_router(router = tool_router)]
impl Lodestash {
    pub fn new(state: Arc<ServerState>) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }

    /// Per-tool allow-list check (UI-SPEC §16, card 178). The REST plane refuses a disabled tool
    /// with `403 tool_disabled`; here it's a tool error. Same [`ServerState::allows`] gate on both
    /// planes, so a tool the user switched off is off 1:1.
    fn gate(&self, tool: &str) -> Result<(), ErrorData> {
        if self.state.allows(tool) {
            Ok(())
        } else {
            Err(ErrorData::invalid_params(
                format!("tool '{tool}' is disabled in Lodestash's MCP settings"),
                None,
            ))
        }
    }

    /// App, schema and API versions of the running Lodestash.
    #[tool(
        name = "get_server_info",
        description = "App, schema and API versions."
    )]
    pub async fn get_server_info(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("get_server_info")?;
        let (app, schema) = self.state.server_info().map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::json(
            serde_json::json!({ "app": app, "schema": schema, "api": "v1" }),
        )?]))
    }

    /// Search the catalog by text (name, description, tags).
    #[tool(
        name = "search_assets",
        description = "Search the asset catalog by text; returns asset summaries."
    )]
    pub async fn search_assets(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("search_assets")?;
        let rows = self
            .state
            .search(
                &args.query,
                args.limit.unwrap_or(25).clamp(1, 200),
                args.media_type.as_deref(),
            )
            .map_err(internal)?;
        let json: Vec<_> = rows.iter().map(lodestash_api::summary_json).collect();
        Ok(CallToolResult::success(vec![ContentBlock::json(json)?]))
    }

    /// Natural-language search over the visual/audio embedding index.
    #[tool(
        name = "semantic_search",
        description = "Find assets by description (\"mossy stone wall\", \"metallic clang\").                        Ranks by meaning, not filename. Errors if no embedding model is loaded."
    )]
    pub async fn semantic_search(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("semantic_search")?;
        let limit = args.limit.unwrap_or(25).clamp(1, 200);
        match self
            .state
            .semantic_search(&args.query, limit, args.media_type.as_deref())
        {
            Some(Ok(rows)) => {
                let json: Vec<_> = rows.iter().map(lodestash_api::summary_json).collect();
                Ok(CallToolResult::success(vec![ContentBlock::json(json)?]))
            }
            Some(Err(e)) => Err(internal(e)),
            None => Err(ErrorData::internal_error(
                "no embedding model is loaded; download the AI models in Lodestash's settings",
                None,
            )),
        }
    }

    /// One asset's full record.
    #[tool(
        name = "get_asset",
        description = "Full metadata for one asset, including technical facets."
    )]
    pub async fn get_asset(
        &self,
        Parameters(args): Parameters<AssetArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("get_asset")?;
        // The same builder as REST's `GET /v1/assets/{id}`: provenance and revision included, and
        // no second copy of the response to drift.
        let Some(body) = self.state.asset_detail_json(args.id).map_err(internal)? else {
            return Err(ErrorData::invalid_params("no such asset", None));
        };
        Ok(CallToolResult::success(vec![ContentBlock::json(body)?]))
    }

    /// Assets most like this one.
    #[tool(
        name = "find_similar",
        description = "Assets most visually (or sonically) similar to the given one. Empty if \
                       the library has not been embedded yet."
    )]
    pub async fn find_similar(
        &self,
        Parameters(args): Parameters<SimilarArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("find_similar")?;
        let rows = self
            .state
            .similar(args.id, args.limit.unwrap_or(10).clamp(1, 200))
            .map_err(internal)?;
        let json: Vec<_> = rows.iter().map(lodestash_api::summary_json).collect();
        Ok(CallToolResult::success(vec![ContentBlock::json(json)?]))
    }

    /// The downloaded Asset Store packages.
    #[tool(
        name = "list_packages",
        description = "Asset Store packages in the library (publisher, category, asset count)."
    )]
    pub async fn list_packages(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("list_packages")?;
        let rows = self.state.packages().map_err(internal)?;
        let json: Vec<_> = rows.iter().map(lodestash_api::package_json).collect();
        Ok(CallToolResult::success(vec![ContentBlock::json(json)?]))
    }

    /// Register the open Unity editor as the active import target.
    #[tool(
        name = "register_project",
        description = "Register the open Unity editor as the active import target. Idempotent per \
                       project path; returns { project_id, is_active }."
    )]
    pub async fn register_project(
        &self,
        Parameters(args): Parameters<RegisterProjectArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("register_project")?;
        if args.project_path.trim().is_empty() {
            return Err(ErrorData::invalid_params(
                "project_path must not be empty",
                None,
            ));
        }
        let (id, is_active) = self
            .state
            .register_project(
                &args.project_path,
                args.unity_version.as_deref(),
                args.pipeline.as_deref(),
            )
            .map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::json(
            serde_json::json!({ "project_id": id, "is_active": is_active }),
        )?]))
    }

    /// Registered Unity editors and which is active.
    #[tool(
        name = "list_projects",
        description = "Registered Unity editors (path, version, pipeline) and which is active."
    )]
    pub async fn list_projects(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("list_projects")?;
        let rows = self.state.list_projects().map_err(internal)?;
        let json: Vec<_> = rows.iter().map(lodestash_api::project_json).collect();
        Ok(CallToolResult::success(vec![ContentBlock::json(json)?]))
    }

    /// Import assets (with dependencies) into the open Unity project. Approval-gated.
    #[tool(
        name = "import_to_project",
        description = "Import assets — with their dependency closure — into the open Unity project. \
                       Returns a job in state 'awaiting_approval' until the desktop owner approves \
                       (poll get_job_status), reports assets already imported, or errors if no \
                       editor is registered."
    )]
    pub async fn import_to_project(
        &self,
        Parameters(args): Parameters<ImportArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("import_to_project")?;
        if args.asset_ids.is_empty() {
            return Err(ErrorData::invalid_params(
                "asset_ids must not be empty",
                None,
            ));
        }
        let req = lodestash_api::ImportRequest {
            asset_ids: args.asset_ids,
            project_id: args.project_id,
            dest_subfolder: args.dest_subfolder,
            with_dependencies: args.with_dependencies.unwrap_or(true),
        };
        match self.state.request_import(req) {
            Ok(k) => Ok(CallToolResult::success(vec![ContentBlock::json(
                k.to_json(),
            )?])),
            Err(lodestash_api::ImportError::Empty) => Err(ErrorData::invalid_params(
                "asset_ids must not be empty",
                None,
            )),
            Err(lodestash_api::ImportError::NoProject) => Err(ErrorData::invalid_params(
                "no Unity project is registered; register one or export to a folder instead",
                None,
            )),
        }
    }

    /// Copy assets out to a folder. Waits for the person at the machine to approve.
    #[tool(
        name = "export_asset",
        description = "Copy assets to a folder outside the library. Returns a job that stays in \
                       state 'awaiting_approval' until the desktop app's owner approves it — \
                       poll get_job_status."
    )]
    pub async fn export_asset(
        &self,
        Parameters(args): Parameters<ExportArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("export_asset")?;
        if args.asset_ids.is_empty() {
            return Err(ErrorData::invalid_params(
                "asset_ids must not be empty",
                None,
            ));
        }
        let req = lodestash_api::export::ExportRequest {
            asset_ids: args.asset_ids,
            dest_path: args.dest_path,
            with_dependencies: args.with_dependencies.unwrap_or(false),
        };
        match self.state.request_export(req) {
            Ok(job) => Ok(CallToolResult::success(vec![ContentBlock::json(
                job.to_json(),
            )?])),
            Err(e) => Err(ErrorData::invalid_params(e.message(), None)),
        }
    }

    /// Progress of a long-running operation.
    #[tool(
        name = "get_job_status",
        description = "State and progress of a job (awaiting_approval | queued | running | \
                       done | error | cancelled)."
    )]
    pub async fn get_job_status(
        &self,
        Parameters(args): Parameters<JobArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("get_job_status")?;
        match self.state.jobs.get(&args.job_id) {
            Some(job) => Ok(CallToolResult::success(vec![ContentBlock::json(
                job.to_json(),
            )?])),
            None => Err(ErrorData::invalid_params("no such job", None)),
        }
    }

    /// Ask a job to stop. Idempotent.
    #[tool(
        name = "cancel_job",
        description = "Request cancellation of a job. Idempotent; finished jobs are unaffected."
    )]
    pub async fn cancel_job(
        &self,
        Parameters(args): Parameters<JobArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("cancel_job")?;
        match self.state.jobs.cancel(&args.job_id) {
            Some(job) => Ok(CallToolResult::success(vec![ContentBlock::json(
                job.to_json(),
            )?])),
            None => Err(ErrorData::invalid_params("no such job", None)),
        }
    }

    /// A viewable preview plus a link to the real bytes.
    #[tool(
        name = "get_asset_preview",
        description = "A rendered preview image of the asset (3D turntable, texture, or audio \
                       waveform), plus a resource link to the source file."
    )]
    pub async fn get_asset_preview(
        &self,
        Parameters(args): Parameters<AssetArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("get_asset_preview")?;
        let Some(row) = self.state.asset(args.id).map_err(internal)? else {
            return Err(ErrorData::invalid_params("no such asset", None));
        };
        // A missing preview is **not** an error. This used to return `Err` before it added
        // anything, so an asset with no cached render answered with no image, no file link and no
        // metadata — and since previews resolved for nothing (in-pack assets have no file at the
        // path the summary fabricates, and materials have no pixels of their own), that was every
        // asset in a real library. The tool now returns what it has.
        let mut content = Vec::new();
        if let Some(png) = self.state.preview_path(args.id) {
            // An unreadable cache entry costs the image, not the call.
            if let Ok(bytes) = std::fs::read(&png) {
                content.push(ContentBlock::image(b64(&bytes), "image/png"));
            }
        }
        // MCP-AND-ENGINES "Returning previews — the rule": always pair a viewable image with a
        // resource_link. The model reasons over the image; the consumer fetches the bytes.
        // Only a file that is really there — an in-pack asset has no path to hand out.
        if let Some(src) = self.state.source_path(&row).filter(|p| p.is_file()) {
            content.push(ContentBlock::resource_link(
                Resource::new(file_uri(&src), row.name.clone())
                    .with_description(format!("{} source file", row.media_type)),
            ));
        }
        // Always say *what* it is, so a caller with no image still has something to reason from.
        content.push(ContentBlock::json(serde_json::json!({
            "id": row.id,
            "name": row.name,
            "media_type": row.media_type,
            "format": row.format,
            "package": row.package_name,
            "preview": if content.iter().any(|c| matches!(c, ContentBlock::Image(_))) {
                "rendered"
            } else {
                "unavailable"
            },
        }))?);
        Ok(CallToolResult::success(content))
    }

    /// ENGINE-API §2 `list_tags` — the tag vocabulary with usage counts.
    #[tool(
        name = "list_tags",
        description = "List the tag vocabulary with asset counts, most-used first."
    )]
    pub async fn list_tags(
        &self,
        Parameters(args): Parameters<TagsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("list_tags")?;
        let rows = self
            .state
            .list_tags(&args.prefix.unwrap_or_default(), args.limit.unwrap_or(500))
            .map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::json(rows)?]))
    }

    /// ENGINE-API §2 `add_tag` — one `(namespace, value)` on one asset (mutating).
    #[tool(
        name = "add_tag",
        description = "Add a tag to an asset. Mutating — the catalog write goes through the app's single writer."
    )]
    pub async fn add_tag(
        &self,
        Parameters(args): Parameters<AddTagArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("add_tag")?;
        let ns = args.namespace.unwrap_or_else(|| "free".into());
        let changed = self
            .state
            .add_tag(args.asset_id, &ns, &args.value)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![ContentBlock::json(
            serde_json::json!({ "changed": changed }),
        )?]))
    }

    /// ENGINE-API §8.3 `write_tags` — an attributed batch of tags and captions, parked for the
    /// person at the machine to approve. Parsed into the very request type REST uses, so the two
    /// planes validate identically; a refusal is a tool error carrying the same code.
    #[tool(
        name = "write_tags",
        description = "Write tags (and optionally a short description) on 1-100 assets as this host's \
                       configured producer. Read each asset's annotation_revision with get_asset \
                       first. Returns a job in state 'awaiting_approval' until the desktop owner \
                       approves (poll get_job_status); retrying the same request_id and body returns \
                       the original result. Never confirms an asset."
    )]
    pub async fn write_tags(
        &self,
        Parameters(args): Parameters<WriteTagsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("write_tags")?;
        let bytes = serde_json::to_vec(&args)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let req: lodestash_api::TagBatchRequest = serde_json::from_slice(&bytes)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        match self.state.request_tag_batch(req, bytes.len()) {
            Ok(k) => Ok(CallToolResult::success(vec![ContentBlock::json(
                k.to_response().1,
            )?])),
            Err(e) => {
                let (status, code, message) = e.parts();
                Ok(CallToolResult::error(vec![ContentBlock::json(
                    serde_json::json!({
                        "error": { "code": code, "message": message, "status": status }
                    }),
                )?]))
            }
        }
    }

    /// ENGINE-API §2 `list_collections` — named collections + the active project's staging tray.
    #[tool(
        name = "list_collections",
        description = "List the user's collections (and the active project's staging tray, flagged is_staging)."
    )]
    pub async fn list_collections(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("list_collections")?;
        let rows = self.state.list_collections().map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::json(rows)?]))
    }

    /// ENGINE-API §2 `apply_mapping` — run the deterministic mapping engine (mutating).
    #[tool(
        name = "apply_mapping",
        description = "Run the deterministic mapping engine over one taxonomy, writing category assignments. Mutating."
    )]
    pub async fn apply_mapping(
        &self,
        Parameters(args): Parameters<MappingArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("apply_mapping")?;
        let changed = self
            .state
            .apply_mapping(args.taxonomy_id)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![ContentBlock::json(
            serde_json::json!({ "changed": changed }),
        )?]))
    }

    /// ENGINE-API §2 `identify_asset` — long-running appraise of an explicit set.
    #[tool(
        name = "identify_asset",
        description = "Flag and appraise specific assets (embeddings, tags, promote). Long-running: returns a job id to poll with get_job_status."
    )]
    pub async fn identify_asset(
        &self,
        Parameters(args): Parameters<IdentifyArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("identify_asset")?;
        let job = self
            .state
            .identify_asset(args.asset_ids)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![ContentBlock::json(
            job.to_json(),
        )?]))
    }

    /// ENGINE-API §2 `suggest_tags` — the asset's tags with confidences (read-only).
    #[tool(
        name = "suggest_tags",
        description = "Read the tags proposed for an asset, with AI confidences. Empty for an untagged asset."
    )]
    pub async fn suggest_tags(
        &self,
        Parameters(args): Parameters<AssetArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("suggest_tags")?;
        let rows = self.state.suggest_tags(args.id).map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::json(rows)?]))
    }

    /// ENGINE-API §2 `unpack_package` — extract one pack into the workspace (long-running).
    #[tool(
        name = "unpack_package",
        description = "Extract a .unitypackage into the workspace so its assets become local files. Long-running: returns a job id."
    )]
    pub async fn unpack_package(
        &self,
        Parameters(args): Parameters<UnpackArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("unpack_package")?;
        let job = self
            .state
            .unpack_package(args.package_id)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![ContentBlock::json(
            job.to_json(),
        )?]))
    }

    /// ENGINE-API §6 `stage_asset` — add one asset to the active project's staging shortlist.
    #[tool(
        name = "stage_asset",
        description = "Add an asset to the active project's staging tray (per-project shortlist). Requires a registered editor."
    )]
    pub async fn stage_asset(
        &self,
        Parameters(args): Parameters<StageArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("stage_asset")?;
        let changed = self
            .state
            .stage_asset(args.asset_id)
            .map_err(|e| ErrorData::invalid_params(e, None))?;
        Ok(CallToolResult::success(vec![ContentBlock::json(
            serde_json::json!({ "changed": changed }),
        )?]))
    }

    /// ENGINE-API §6 `list_staged` — the active project's staging shortlist as summaries.
    #[tool(
        name = "list_staged",
        description = "List the active project's staged assets (the shortlist before import)."
    )]
    pub async fn list_staged(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("list_staged")?;
        let rows = self.state.list_staged().map_err(internal)?;
        let json: Vec<_> = rows.iter().map(lodestash_api::summary_json).collect();
        Ok(CallToolResult::success(vec![ContentBlock::json(json)?]))
    }

    /// ENGINE-API §6 `import_staged` — queue the staged shortlist for import (approval gate).
    #[tool(
        name = "import_staged",
        description = "Queue every staged asset for import into the active project. Parks on the human approval gate; returns a job id."
    )]
    pub async fn import_staged(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("import_staged")?;
        match self.state.import_staged() {
            Ok(k) => Ok(CallToolResult::success(vec![ContentBlock::json(
                k.to_json(),
            )?])),
            Err(lodestash_api::ImportError::Empty) => Err(ErrorData::invalid_params(
                "The staging tray is empty — stage_asset first.",
                None,
            )),
            Err(lodestash_api::ImportError::NoProject) => Err(ErrorData::invalid_params(
                "No Unity project is registered — open the Lodestash panel in Unity first.",
                None,
            )),
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for Lodestash {
    fn get_info(&self) -> ServerInfo {
        // `ServerInfo`/`Implementation` are #[non_exhaustive] — build from Default and set fields.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info.name = "lodestash".into();
        info.server_info.version = env!("CARGO_PKG_VERSION").into();
        info.instructions = Some(
            "Local game-asset catalog. Search, inspect and preview the 3D models, textures, \
             images and audio the user already owns."
                .into(),
        );
        info
    }
}

/// Serve MCP over stdio (the transport Claude Desktop/Code expect). Blocks until the client
/// disconnects.
pub async fn serve_stdio(state: Arc<ServerState>) -> std::io::Result<()> {
    use rmcp::ServiceExt;
    let service = Lodestash::new(state)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(std::io::Error::other)?;
    service.waiting().await.map_err(std::io::Error::other)?;
    Ok(())
}

fn internal(e: lodestash_core::error::CoreError) -> ErrorData {
    ErrorData::internal_error(e.to_string(), None)
}

/// `file://` URI for a local path. Windows paths arrive with backslashes and a drive letter;
/// a URI wants forward slashes throughout, or clients read `G:\a\b` as one opaque segment.
/// ponytail: no percent-encoding — spaces and `#` in asset paths would need it; add an encoder
/// if a client trips over one.
fn file_uri(path: &std::path::Path) -> String {
    format!("file:///{}", path.display().to_string().replace('\\', "/"))
}

/// ponytail: `base64` is already in the tree for rmcp's image blocks — no reason to hand-roll.
fn b64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The MCP↔REST 1:1 invariant (ENGINE-API §2): every agent operation exists on **both**
    /// planes. Adding a route without its tool — or a tool without its route — fails here.
    /// ENGINE-API §2 "1:1 planes" — every AGENT_OPERATION exists as an MCP tool name. The REST
    /// plane's names come from the same list (engine-api §2), so this one test pins both.
    #[test]
    fn tool_set_matches_the_rest_surface() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let state = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap();

        let mut tools: Vec<String> = Lodestash::new(Arc::new(state))
            .tool_router
            .list_all()
            .into_iter()
            .map(|t| t.name.to_string())
            .collect();
        let mut expected: Vec<String> = lodestash_api::AGENT_OPERATIONS
            .iter()
            .map(|(tool, _)| tool.to_string())
            .collect();
        tools.sort();
        expected.sort();
        assert_eq!(tools, expected, "MCP tools must mirror the REST operations");
    }

    /// The per-tool allow-list (UI-SPEC §16, card 178) gates the MCP plane: default allows
    /// everything, a set restricts to its members, and a disabled tool errors.
    #[test]
    fn allow_list_gates_the_mcp_plane() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let state =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());

        // Construction default: no allow-list, every tool live.
        assert!(state.allows("export_asset"), "allow-all by default");

        let enabled: std::collections::HashSet<String> = ["search_assets".to_string()].into();
        state.set_allowed(enabled);

        let lode = Lodestash::new(state);
        assert!(lode.gate("search_assets").is_ok(), "enabled tool passes");
        assert!(lode.gate("export_asset").is_err(), "disabled tool refused");
    }

    /// `write_tags` is one contract on both planes (ENGINE-API §8.4): the tool parses into the REST
    /// request type and runs the same validation, so a refusal carries the same code and an
    /// accepted request parks the same kind of job on the same gate.
    #[test]
    fn write_tags_is_one_contract_on_both_planes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let id = {
            let mut c = lodestash_core::catalog::Catalog::open(&db).unwrap();
            c.seed_media_types().unwrap();
            let src = c.upsert_import_source("Lib", "C:/lib", "folder").unwrap();
            c.record_crawl(
                src,
                None,
                &[lodestash_core::catalog::CrawlRecord {
                    workspace_rel: None,
                    content_hash: None,
                    rel_path: "a.png".into(),
                    name: "a.png".into(),
                    media_type: "image".into(),
                    size_bytes: 1,
                    mtime: 1,
                }],
            )
            .unwrap();
            c.assets_needing_tags_within(1, None).unwrap()[0].asset_id
        };
        let state =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        state.set_agent_producer(Some("codex@test".into()));
        let mcp = Lodestash::new(state.clone());
        let rev = state.asset_detail_json(id).unwrap().unwrap()["annotation_revision"]
            .as_str()
            .unwrap()
            .to_string();
        let body = |rid: &str, producer: &str, value: &str| {
            serde_json::json!({
                "request_id": rid,
                "assets": [{ "asset_id": id, "expected_revision": rev, "producer_id": producer,
                             "mode": "append",
                             "tags": [{ "namespace": "subject", "value": value, "confidence": 0.5 }] }]
            })
        };
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for (b, code) in [
            (
                body(
                    "6f1c1e0e-1111-4111-8111-111111111111",
                    "someone-else",
                    "stone",
                ),
                "producer_mismatch",
            ),
            (
                body("6f1c1e0e-2222-4222-8222-222222222222", "codex@test", "0.9"),
                "invalid_argument",
            ),
        ] {
            let rest = state
                .request_tag_batch(serde_json::from_value(b.clone()).unwrap(), 100)
                .err()
                .map(|e| e.parts().1);
            let out = rt
                .block_on(mcp.write_tags(Parameters(serde_json::from_value(b).unwrap())))
                .expect("a domain refusal is a tool result, not a protocol error");
            let text = serde_json::to_string(&out).unwrap();
            assert_eq!(rest, Some(code));
            assert_eq!(out.is_error, Some(true), "{text}");
            assert!(text.contains(code), "MCP said {text}");
        }
        let ok = body(
            "6f1c1e0e-3333-4333-8333-333333333333",
            "codex@test",
            "stone",
        );
        let out = rt
            .block_on(mcp.write_tags(Parameters(serde_json::from_value(ok).unwrap())))
            .expect("tool call");
        let text = serde_json::to_string(&out).unwrap();
        assert!(
            text.contains("awaiting_approval") && text.contains("write_tags"),
            "{text}"
        );
        assert_eq!(
            state.jobs.awaiting_approval().len(),
            1,
            "parked on the one gate"
        );
    }
}
