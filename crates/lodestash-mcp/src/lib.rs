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
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, ListResourceTemplatesResult, PaginatedRequestParams,
    ProtocolVersion, ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents,
    ResourceTemplate, ServerCapabilities, ServerInfo,
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
    /// Restrict to one library (`list_libraries` ids).
    pub source_id: Option<i64>,
    /// Restrict to one package; it must belong to `source_id` when both are given.
    pub package_id: Option<i64>,
    /// Canonical `namespace:value` tags, all of which must be present.
    pub tags: Option<Vec<String>>,
}

/// `list_libraries` (ENGINE-API §8.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListLibrariesArgs {
    /// Page size (default 25, maximum 200).
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page.
    pub cursor: Option<String>,
}

/// `list_assets` (ENGINE-API §8.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListAssetsArgs {
    /// The library to walk (`list_libraries` ids). Required.
    pub source_id: i64,
    /// Only this package's assets; it must belong to the library.
    pub package_id: Option<i64>,
    /// model_3d | texture | audio | image | sprite | material | font.
    pub media_type: Option<String>,
    /// discovered | indexed | identified | confirmed.
    pub status: Option<String>,
    /// any (default) | tagged | untagged.
    pub tag_state: Option<String>,
    /// ordered (default): an id-order walk you page with `cursor`. random: a fresh sample; no cursor.
    pub selection: Option<String>,
    /// Page size (default 25, maximum 200).
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page of the same walk.
    pub cursor: Option<String>,
}

/// `get_assets` (ENGINE-API §8.1).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetAssetsArgs {
    /// 1 to 100 distinct asset ids.
    pub asset_ids: Vec<i64>,
    /// When given, assets outside this library are reported unavailable.
    pub source_id: Option<i64>,
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

/// `get_asset_evidence` (ENGINE-API §8.2).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceArgs {
    /// The asset's catalog id.
    pub id: i64,
    /// auto (default), thumbnail, model_views, or text.
    pub kind: Option<String>,
    /// Longest representation edge; defaults to 512, maximum 1024.
    pub max_dimension: Option<usize>,
    /// Requested 3D views; defaults to 1, maximum 4.
    pub views: Option<usize>,
}

/// `get_vocabulary` (ENGINE-API §8.5).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct VocabularyArgs {
    /// Restrict traits to this media type.
    pub media_type: Option<String>,
    /// Taxonomy id; omitted selects the active taxonomy when there is one.
    pub taxonomy_id: Option<i64>,
    /// Trait namespace or value prefix.
    pub prefix: Option<String>,
    /// Page size (default 25, maximum 200 across all returned entry types).
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page with the same filters.
    pub cursor: Option<String>,
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
pub struct ReviewDecisionArgs {
    pub namespace: String,
    pub value: String,
    pub source: String,
    pub action: String,
    pub reason: String,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReviewAssetArgs {
    pub asset_id: i64,
    pub expected_revision: String,
    pub decisions: Vec<ReviewDecisionArgs>,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ReviewTagsArgs {
    pub request_id: String,
    pub vocabulary_revision: String,
    pub assets: Vec<ReviewAssetArgs>,
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

    /// Headless: hand an app-owned operation to the running app and relay its answer (v1 plan S11).
    ///
    /// `None` when this server *is* in the app, and the caller runs the operation itself. The REST
    /// answer is this tool's answer — the planes are 1:1, so the app's body is exactly what the
    /// tool would have returned in-process — which is why the job id an agent gets back is the
    /// app's own, and `get_job_status` on it reaches the job that is actually running.
    ///
    /// Nothing is retried. A request that timed out after it was sent may still land, and only a
    /// request id makes a repeat safe; that decision belongs to the caller.
    fn forward(
        &self,
        method: &str,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Option<Result<CallToolResult, ErrorData>> {
        let proxy = self.state.app_proxy()?;
        Some(relay(proxy(method, path, body.as_ref())))
    }

    /// App, schema and API versions of the running Lodestash.
    #[tool(
        name = "get_server_info",
        description = "App, schema and API versions."
    )]
    pub async fn get_server_info(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("get_server_info")?;
        let body = self.state.server_info_json().map_err(internal)?;
        Ok(CallToolResult::success(vec![ContentBlock::json(body)?]))
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
        let scope = match self.state.search_scope(&scope_of(&args)) {
            Ok(scope) => scope,
            Err(e) => return refused(e),
        };
        let rows = self
            .state
            .search(&args.query, args.limit.unwrap_or(25).clamp(1, 200), &scope)
            .map_err(internal)?;
        let json: Vec<_> = rows.iter().map(|r| self.state.summary(r)).collect();
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
        let scope = match self.state.search_scope(&scope_of(&args)) {
            Ok(scope) => scope,
            Err(e) => return refused(e),
        };
        match self.state.semantic_search(&args.query, limit, &scope) {
            Some(Ok(rows)) => {
                let json: Vec<_> = rows.iter().map(|r| self.state.summary(r)).collect();
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
        let json: Vec<_> = rows.iter().map(|r| self.state.summary(r)).collect();
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
        if let Some(r) = self.forward(
            "POST",
            "/v1/projects/register",
            Some(serde_json::json!({
                "project_path": args.project_path,
                "unity_version": args.unity_version,
                "pipeline": args.pipeline,
            })),
        ) {
            return r;
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
        if let Some(r) = self.forward(
            "POST",
            "/v1/import",
            Some(serde_json::json!({
                "asset_ids": args.asset_ids,
                "project_id": args.project_id,
                "dest_subfolder": args.dest_subfolder,
                "with_dependencies": args.with_dependencies.unwrap_or(true),
            })),
        ) {
            return r;
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
        if let Some(r) = self.forward(
            "POST",
            "/v1/export",
            Some(serde_json::json!({
                "asset_ids": args.asset_ids,
                "dest_path": args.dest_path,
                "with_dependencies": args.with_dependencies.unwrap_or(false),
            })),
        ) {
            return r;
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
        if let Some(r) = self.forward("GET", &job_path(&args.job_id, "")?, None) {
            return r;
        }
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
        if let Some(r) = self.forward("POST", &job_path(&args.job_id, "/cancel")?, None) {
            return r;
        }
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
        // Bytes are a separate permission from metadata (v1 plan S10): without it the tool still
        // answers — name, type, links — and simply carries no image.
        if !self.state.byte_release() {
            content.push(ContentBlock::text(
                "preview bytes withheld: this agent may read metadata and links; serving asset \
                 bytes needs `mcp_bytes_enabled` in Settings",
            ));
        } else if let Some(png) = self.state.preview_path(args.id) {
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

    /// A bounded evidence manifest plus the available raster preview for a vision-capable host.
    #[tool(
        name = "get_asset_evidence",
        description = "Return an asset-analysis manifest and available preview image blocks. Byte release must be enabled in Lodestash Settings; a denied or unavailable manifest never claims visual evidence."
    )]
    pub async fn get_asset_evidence(
        &self,
        Parameters(args): Parameters<EvidenceArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("get_asset_evidence")?;
        let manifest = match self.state.get_asset_evidence(
            args.id,
            &lodestash_api::EvidenceRequest {
                kind: args.kind,
                max_dimension: args.max_dimension,
                views: args.views,
            },
        ) {
            Ok(manifest) => manifest,
            Err(error) => return refused(error),
        };
        let mut content = vec![ContentBlock::json(manifest.clone())?];
        if manifest["representations"]
            .as_array()
            .is_some_and(|representations| !representations.is_empty())
        {
            if let Some(path) = self.state.preview_path(args.id) {
                if let Ok(bytes) = std::fs::read(path) {
                    content.push(ContentBlock::image(b64(&bytes), "image/png"));
                }
            }
        }
        content.push(ContentBlock::resource_link(
            Resource::new(format!("asset://{}/file", args.id), "asset_file")
                .with_description("The asset source bytes, subject to the same byte policy."),
        ));
        Ok(CallToolResult::success(content))
    }

    /// The canonical traits and selected taxonomy used to review agent assignments.
    #[tool(
        name = "get_vocabulary",
        description = "Return the revisioned, app-configured vocabulary policy, applicable canonical traits, and the selected taxonomy. Keep revision and cursor when proposing or reviewing tags."
    )]
    pub async fn get_vocabulary(
        &self,
        Parameters(args): Parameters<VocabularyArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("get_vocabulary")?;
        answer(
            self.state
                .get_vocabulary(&lodestash_api::VocabularyRequest {
                    media_type: args.media_type,
                    taxonomy_id: args.taxonomy_id,
                    prefix: args.prefix,
                    limit: args.limit,
                    cursor: args.cursor,
                }),
        )
    }

    /// ENGINE-API §8.1 `list_libraries`.
    #[tool(
        name = "list_libraries",
        description = "The libraries in this catalog (id, name, kind, asset_count), paged by cursor. \
                       Use an id as source_id for list_assets and the search tools."
    )]
    pub async fn list_libraries(
        &self,
        Parameters(args): Parameters<ListLibrariesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("list_libraries")?;
        answer(self.state.list_libraries(&lodestash_api::LibrariesRequest {
            limit: args.limit,
            cursor: args.cursor,
        }))
    }

    /// ENGINE-API §8.1 `list_assets`.
    #[tool(
        name = "list_assets",
        description = "Walk one library completely: selection 'ordered' pages in id order — pass \
                       next_cursor back until it is null; assets added after the walk began wait for \
                       the next walk. selection 'random' returns a fresh sample without replacement. \
                       Filter by package, media_type, status and tag_state. Search ranks; this visits."
    )]
    pub async fn list_assets(
        &self,
        Parameters(args): Parameters<ListAssetsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("list_assets")?;
        answer(self.state.list_assets(&lodestash_api::ListAssetsRequest {
            source_id: Some(args.source_id),
            package_id: args.package_id,
            media_type: args.media_type,
            status: args.status,
            tag_state: args.tag_state,
            selection: args.selection,
            limit: args.limit,
            cursor: args.cursor,
        }))
    }

    /// ENGINE-API §8.1 `get_assets`.
    #[tool(
        name = "get_assets",
        description = "Full detail for 1-100 assets in the order given, with the ids that could not \
                       be returned listed under 'unavailable'. Read each asset's revisions here \
                       before writing tags."
    )]
    pub async fn get_assets(
        &self,
        Parameters(args): Parameters<GetAssetsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("get_assets")?;
        answer(self.state.read_assets(&lodestash_api::ReadAssetsRequest {
            asset_ids: args.asset_ids,
            source_id: args.source_id,
        }))
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
        if let Some(r) = self.forward(
            "POST",
            &format!("/v1/assets/{}/tags", args.asset_id),
            Some(serde_json::json!({ "namespace": ns, "value": args.value })),
        ) {
            return r;
        }
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
        // Parked on the app's gate, where the person at the machine can see it (closes S05's
        // headless gap): the body goes as the agent wrote it, and the app validates it.
        if let Some(r) = self.forward(
            "POST",
            "/v1/tags/batch",
            Some(
                serde_json::to_value(&args)
                    .map_err(|e| ErrorData::internal_error(e.to_string(), None))?,
            ),
        ) {
            return r;
        }
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

    #[tool(
        name = "review_tags",
        description = "Accept or reject exact existing AI tag associations after local approval. Manual and imported tags are protected."
    )]
    pub async fn review_tags(
        &self,
        Parameters(args): Parameters<ReviewTagsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.gate("review_tags")?;
        let value = serde_json::to_value(&args)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        if let Some(result) = self.forward("POST", "/v1/tags/review", Some(value.clone())) {
            return result;
        }
        let bytes = serde_json::to_vec(&value)
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let request: lodestash_api::TagReviewRequest = serde_json::from_slice(&bytes)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        match self.state.request_tag_review(request, bytes.len()) {
            Ok(kickoff) => Ok(CallToolResult::success(vec![ContentBlock::json(
                kickoff.to_response().1,
            )?])),
            Err(error) => {
                let (status, code, message) = error.parts();
                Ok(CallToolResult::error(vec![ContentBlock::json(
                    serde_json::json!({"error":{"code":code,"message":message,"status":status}}),
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
        if let Some(r) = self.forward(
            "POST",
            "/v1/mapping/apply",
            Some(serde_json::json!({ "taxonomy_id": args.taxonomy_id })),
        ) {
            return r;
        }
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
        if let Some(r) = self.forward(
            "POST",
            "/v1/identify",
            Some(serde_json::json!({ "asset_ids": args.asset_ids })),
        ) {
            return r;
        }
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
        if let Some(r) = self.forward(
            "POST",
            "/v1/packages/unpack",
            Some(serde_json::json!({ "package_id": args.package_id })),
        ) {
            return r;
        }
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
        if let Some(r) = self.forward(
            "POST",
            "/v1/stage",
            Some(serde_json::json!({ "asset_id": args.asset_id })),
        ) {
            return r;
        }
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
        let json: Vec<_> = rows.iter().map(|r| self.state.summary(r)).collect();
        Ok(CallToolResult::success(vec![ContentBlock::json(json)?]))
    }

    /// ENGINE-API §6 `import_staged` — queue the staged shortlist for import (approval gate).
    #[tool(
        name = "import_staged",
        description = "Queue every staged asset for import into the active project. Parks on the human approval gate; returns a job id."
    )]
    pub async fn import_staged(&self) -> Result<CallToolResult, ErrorData> {
        self.gate("import_staged")?;
        if let Some(r) = self.forward("POST", "/v1/stage/import", None) {
            return r;
        }
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
    /// Every tool runs on the blocking pool (v1 plan S11).
    ///
    /// rmcp gives each request its own task, but the headless server is a current-thread runtime
    /// and every tool body calls synchronous state — the catalog mutex, a preview render, a query
    /// embedding that loads a model, a forward that waits for the app. One slow call held the only
    /// thread, so `get_job_status` and `cancel_job` could not answer while the job they asked
    /// about was the thing being slow. `Handle::block_on` from a pool thread is sound here because
    /// `serve_stdio`'s own `block_on` keeps driving the runtime.
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let me = self.clone();
        let rt = tokio::runtime::Handle::current();
        tokio::task::spawn_blocking(move || {
            rt.block_on(async move {
                let tcc = ToolCallContext::new(&me, request, context);
                me.tool_router.call(tcc).await
            })
        })
        .await
        .map_err(|e| ErrorData::internal_error(format!("the tool worker failed: {e}"), None))?
    }

    /// `asset://{id}/file` — the one resource shape this server has (ENGINE-API §8.2).
    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("asset://{id}/file", "asset_file").with_description(
                "An asset's source bytes, when this catalog releases bytes to agents. Large files \
                 are refused here; read them over the authenticated REST file route instead.",
            ),
        ]))
    }

    /// Read `asset://{id}/file` through the same resolver and byte policy as REST's file route —
    /// never unrestricted filesystem access, and never more than [`MAX_INLINE_RESOURCE_BYTES`]
    /// inline.
    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let state = self.state.clone();
        tokio::task::spawn_blocking(move || read_asset_file(&state, &request.uri))
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("the resource worker failed: {e}"), None)
            })?
    }

    fn get_info(&self) -> ServerInfo {
        // `ServerInfo`/`Implementation` are #[non_exhaustive] — build from Default and set fields.
        let mut info = ServerInfo::default();
        info.protocol_version = ProtocolVersion::LATEST;
        info.capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
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

/// The app's REST answer as this tool's answer. A refusal stays a tool result carrying the app's
/// error body and status — the shape `write_tags` already uses — so an agent reads the same code
/// whichever plane refused. No app to answer is a domain failure too, `app_unavailable` (ENGINE-API
/// §8.4): the request was well-formed, and reads still work.
fn relay(answer: Result<(u16, serde_json::Value), String>) -> Result<CallToolResult, ErrorData> {
    match answer {
        Ok((status, body)) if status < 400 => {
            Ok(CallToolResult::success(vec![ContentBlock::json(body)?]))
        }
        Ok((status, mut body)) => {
            match body
                .get_mut("error")
                .and_then(serde_json::Value::as_object_mut)
            {
                Some(error) => {
                    error.insert("status".into(), status.into());
                }
                None => {
                    body = serde_json::json!({ "error": { "status": status, "body": body } });
                }
            }
            Ok(CallToolResult::error(vec![ContentBlock::json(body)?]))
        }
        Err(e) => Ok(CallToolResult::error(vec![ContentBlock::json(
            serde_json::json!({
                "error": { "code": "app_unavailable", "message": e, "status": 503 }
            }),
        )?])),
    }
}

/// `/v1/jobs/{id}{suffix}`, for a job id an agent supplied. Ids are `kind_hex`; anything else is
/// refused rather than spliced into a path the app would route somewhere else.
fn job_path(id: &str, suffix: &str) -> Result<String, ErrorData> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(ErrorData::invalid_params("no such job", None));
    }
    Ok(format!("/v1/jobs/{id}{suffix}"))
}

/// Most bytes one resource read returns inline: the evidence ceiling (ENGINE-API §8.2). Bigger files
/// are a link to the REST route, not base64 in a model's context.
pub const MAX_INLINE_RESOURCE_BYTES: u64 = 4 * 1024 * 1024;

fn read_asset_file(state: &ServerState, uri: &str) -> Result<ReadResourceResult, ErrorData> {
    let refuse = |e: lodestash_api::AgentError| {
        ErrorData::invalid_request(e.message.clone(), Some(e.to_json()))
    };
    let id = uri
        .strip_prefix("asset://")
        .and_then(|rest| rest.strip_suffix("/file"))
        .and_then(|id| id.parse::<i64>().ok())
        .filter(|id| *id > 0)
        .ok_or_else(|| {
            ErrorData::resource_not_found(format!("{uri} is not an asset://{{id}}/file URI"), None)
        })?;
    let path = state.resolve_file(id).map_err(refuse)?;
    let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    if len > MAX_INLINE_RESOURCE_BYTES {
        return Err(refuse(lodestash_api::AgentError::resource_too_large(
            format!("the file is {len} bytes; read it from /v1/assets/{id}/file instead"),
        )));
    }
    let bytes = std::fs::read(&path)
        .map_err(|e| ErrorData::internal_error(format!("reading the file failed: {e}"), None))?;
    Ok(ReadResourceResult::new(vec![ResourceContents::blob(
        b64(&bytes),
        uri,
    )
    .with_mime_type(mime_for(&path))]))
}

/// A content type from the extension, for the kinds a model host can use; anything else is bytes.
fn mime_for(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("tga") => "image/x-tga",
        Some("wav") => "audio/wav",
        Some("ogg") => "audio/ogg",
        Some("mp3") => "audio/mpeg",
        Some("glb") => "model/gltf-binary",
        Some("gltf") => "model/gltf+json",
        _ => "application/octet-stream",
    }
}

/// A search's optional scope, as the shared request type.
fn scope_of(args: &SearchArgs) -> lodestash_api::SearchScope {
    lodestash_api::SearchScope {
        source_id: args.source_id,
        package_id: args.package_id,
        media_type: args.media_type.clone(),
        tags: args.tags.clone().unwrap_or_default(),
    }
}

/// A refused request as a tool result carrying the same code and status REST would send
/// (ENGINE-API §8.4) — a domain failure, not a protocol error.
fn refused(e: lodestash_api::AgentError) -> Result<CallToolResult, ErrorData> {
    Ok(CallToolResult::error(vec![ContentBlock::json(
        e.to_json(),
    )?]))
}

fn answer(
    r: Result<serde_json::Value, lodestash_api::AgentError>,
) -> Result<CallToolResult, ErrorData> {
    match r {
        Ok(body) => Ok(CallToolResult::success(vec![ContentBlock::json(body)?])),
        Err(e) => refused(e),
    }
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

    /// A current catalog on disk and a state reading it — the engine's connection is read-only and
    /// never creates one (v1 plan S11).
    fn fixture() -> (tempfile::TempDir, Arc<ServerState>) {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        lodestash_core::catalog::Catalog::open(&db).unwrap();
        let state = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap();
        (dir, Arc::new(state))
    }

    /// `asset://{id}/file` reads through the shared resolver and byte policy (v1 plan S12b): the
    /// bytes come back inline under the ceiling, and a refusal says why — bytes not released, too
    /// large to inline, or not an asset URI at all.
    #[test]
    fn an_asset_resource_is_read_through_the_resolver() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        {
            let mut c = lodestash_core::catalog::Catalog::open(&db).unwrap();
            c.seed_media_types().unwrap();
            let src = c
                .upsert_import_source("Lib", dir.path().to_str().unwrap(), "folder")
                .unwrap();
            std::fs::write(dir.path().join("rock.png"), b"png bytes").unwrap();
            std::fs::write(
                dir.path().join("huge.png"),
                vec![0u8; (MAX_INLINE_RESOURCE_BYTES + 1) as usize],
            )
            .unwrap();
            let rec = |n: &str| lodestash_core::catalog::CrawlRecord {
                workspace_rel: None,
                content_hash: None,
                rel_path: n.into(),
                name: n.into(),
                media_type: "image".into(),
                size_bytes: 1,
                mtime: 1,
            };
            c.record_crawl(src, None, &[rec("rock.png"), rec("huge.png")])
                .unwrap();
        }
        let state = ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap();
        let id = |name: &str| {
            state
                .search("", 10, &Default::default())
                .unwrap()
                .into_iter()
                .find(|r| r.name == name)
                .unwrap()
                .id
        };
        let (rock, huge) = (id("rock.png"), id("huge.png"));

        let denied = read_asset_file(&state, &format!("asset://{rock}/file")).unwrap_err();
        assert!(serde_json::to_string(&denied)
            .unwrap()
            .contains("bytes_not_released"));

        state.set_byte_release(true);
        let read = read_asset_file(&state, &format!("asset://{rock}/file")).unwrap();
        let body = serde_json::to_value(&read).unwrap();
        assert_eq!(body["contents"][0]["blob"], b64(b"png bytes"));
        assert_eq!(body["contents"][0]["mimeType"], "image/png");

        let big = read_asset_file(&state, &format!("asset://{huge}/file")).unwrap_err();
        assert!(serde_json::to_string(&big)
            .unwrap()
            .contains("resource_too_large"));
        for uri in ["asset://x/file", "file:///etc/passwd", "asset://0/file"] {
            assert!(read_asset_file(&state, uri).is_err(), "{uri}");
        }
    }

    /// The discovery tools are the shared operations (v1 plan S12): a tool's JSON is what the
    /// state returns — the value REST sends — and a refusal is an `isError` result with the code
    /// REST would send, not a protocol error.
    #[test]
    fn discovery_tools_are_the_shared_operations() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("catalog.db");
        let source = {
            let mut c = lodestash_core::catalog::Catalog::open(&db).unwrap();
            c.seed_media_types().unwrap();
            let src = c.upsert_import_source("Lib", "C:/lib", "folder").unwrap();
            let rec = |n: &str| lodestash_core::catalog::CrawlRecord {
                workspace_rel: None,
                content_hash: None,
                rel_path: n.into(),
                name: n.into(),
                media_type: "image".into(),
                size_bytes: 1,
                mtime: 1,
            };
            c.record_crawl(src, None, &[rec("a.png"), rec("b.png"), rec("c.png")])
                .unwrap();
            src
        };
        let state =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        let mcp = Lodestash::new(state.clone());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let body = |r: &CallToolResult| -> serde_json::Value {
            let text = serde_json::to_value(r).unwrap()["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string();
            serde_json::from_str(&text).unwrap()
        };

        let tool = rt
            .block_on(mcp.list_assets(Parameters(ListAssetsArgs {
                source_id: source,
                package_id: None,
                media_type: None,
                status: None,
                tag_state: None,
                selection: None,
                limit: Some(2),
                cursor: None,
            })))
            .unwrap();
        let shared = state
            .list_assets(&lodestash_api::ListAssetsRequest {
                source_id: Some(source),
                limit: Some(2),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(body(&tool), shared);

        let vocabulary = rt
            .block_on(mcp.get_vocabulary(Parameters(VocabularyArgs {
                media_type: None,
                taxonomy_id: None,
                prefix: None,
                limit: Some(2),
                cursor: None,
            })))
            .unwrap();
        assert_eq!(
            body(&vocabulary),
            state
                .get_vocabulary(&lodestash_api::VocabularyRequest {
                    limit: Some(2),
                    ..Default::default()
                })
                .unwrap()
        );

        let refused = rt
            .block_on(mcp.list_assets(Parameters(ListAssetsArgs {
                source_id: source,
                package_id: None,
                media_type: None,
                status: None,
                tag_state: None,
                selection: Some("random".into()),
                limit: None,
                cursor: Some("w1.1.2.x.y".into()),
            })))
            .expect("a refusal is a tool result");
        assert_eq!(refused.is_error, Some(true));
        assert_eq!(body(&refused)["error"]["code"], "invalid_cursor");

        let searched = rt
            .block_on(mcp.search_assets(Parameters(SearchArgs {
                query: "png".into(),
                limit: None,
                media_type: Some("mesh".into()),
                source_id: None,
                package_id: None,
                tags: None,
            })))
            .expect("a refusal is a tool result");
        assert_eq!(body(&searched)["error"]["code"], "invalid_argument");

        let read = rt
            .block_on(mcp.get_assets(Parameters(GetAssetsArgs {
                asset_ids: vec![1, 999],
                source_id: None,
            })))
            .unwrap();
        assert_eq!(
            body(&read),
            state
                .read_assets(&lodestash_api::ReadAssetsRequest {
                    asset_ids: vec![1, 999],
                    source_id: None
                })
                .unwrap()
        );

        let evidence = rt
            .block_on(mcp.get_asset_evidence(Parameters(EvidenceArgs {
                id: 1,
                kind: None,
                max_dimension: None,
                views: None,
            })))
            .unwrap();
        assert_eq!(
            body(&evidence),
            state
                .get_asset_evidence(1, &lodestash_api::EvidenceRequest::default())
                .unwrap()
        );
    }

    /// Headless (v1 plan S11): an app-owned tool is the app's REST operation, forwarded whole. The
    /// agent gets the app's own job back and polls that job through the app; nothing runs here —
    /// no local job, no approval parked where no window can show it. A refusal is relayed with its
    /// code, "no app" is a plain error, and neither a disabled tool nor a malformed job id ever
    /// reaches the app.
    #[test]
    fn a_headless_server_hands_app_owned_tools_to_the_app() {
        let (_dir, state) = fixture();
        type Call = (String, String, Option<serde_json::Value>);
        let calls = Arc::new(std::sync::Mutex::new(Vec::<Call>::new()));
        let seen = calls.clone();
        state.set_app_proxy(Box::new(move |method, path, body| {
            seen.lock()
                .unwrap()
                .push((method.into(), path.into(), body.cloned()));
            match path {
                "/v1/identify" => Ok((202, serde_json::json!({"id": "identify_abc", "state": "running"}))),
                "/v1/jobs/identify_abc" => Ok((200, serde_json::json!({"id": "identify_abc", "state": "done"}))),
                "/v1/stage/import" => Ok((
                    409,
                    serde_json::json!({"error": {"code": "project_not_registered", "message": "No Unity project is registered."}}),
                )),
                "/v1/mapping/apply" => Err("no running Lodestash app answered".into()),
                other => panic!("forwarded somewhere unexpected: {other}"),
            }
        }));
        let mcp = Lodestash::new(state.clone());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let text = |r: &CallToolResult| serde_json::to_string(r).unwrap();

        let started = rt
            .block_on(mcp.identify_asset(Parameters(IdentifyArgs { asset_ids: vec![7] })))
            .expect("forwarded");
        assert!(
            text(&started).contains("identify_abc"),
            "{}",
            text(&started)
        );
        assert!(
            state.jobs.get("identify_abc").is_none() && state.jobs.awaiting_approval().is_empty(),
            "the headless server made a job of its own"
        );

        let status = rt
            .block_on(mcp.get_job_status(Parameters(JobArgs {
                job_id: "identify_abc".into(),
            })))
            .expect("forwarded");
        assert!(text(&status).contains("done"), "{}", text(&status));

        let refused = rt
            .block_on(mcp.import_staged())
            .expect("a refusal is a tool result");
        assert_eq!(refused.is_error, Some(true));
        assert!(
            text(&refused).contains("project_not_registered") && text(&refused).contains("409"),
            "the app's code and status must survive the relay: {}",
            text(&refused)
        );

        let absent = rt
            .block_on(mcp.apply_mapping(Parameters(MappingArgs { taxonomy_id: 1 })))
            .expect("no app is a domain failure, not a protocol error");
        assert_eq!(absent.is_error, Some(true));
        assert!(
            text(&absent).contains("app_unavailable")
                && text(&absent).contains("no running Lodestash"),
            "{}",
            text(&absent)
        );

        let before = calls.lock().unwrap().len();
        assert!(rt
            .block_on(mcp.cancel_job(Parameters(JobArgs {
                job_id: "../export".into()
            })))
            .is_err());
        state.set_allowed(["search_assets".to_string()].into());
        assert!(rt
            .block_on(mcp.identify_asset(Parameters(IdentifyArgs { asset_ids: vec![7] })))
            .is_err());
        assert_eq!(
            calls.lock().unwrap().len(),
            before,
            "a malformed job id or a disabled tool reached the app"
        );

        let calls = calls.lock().unwrap();
        assert_eq!(
            calls[0],
            (
                "POST".to_string(),
                "/v1/identify".to_string(),
                Some(serde_json::json!({"asset_ids": [7]}))
            )
        );
    }

    /// The MCP↔REST 1:1 invariant (ENGINE-API §2): every agent operation exists on **both**
    /// planes. Adding a route without its tool — or a tool without its route — fails here.
    /// ENGINE-API §2 "1:1 planes" — every AGENT_OPERATION exists as an MCP tool name. The REST
    /// plane's names come from the same list (engine-api §2), so this one test pins both.
    #[test]
    fn tool_set_matches_the_rest_surface() {
        let (_dir, state) = fixture();

        let mut tools: Vec<String> = Lodestash::new(state)
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

    #[test]
    fn evidence_tool_is_registered() {
        let (_dir, state) = fixture();
        assert!(Lodestash::new(state)
            .tool_router
            .list_all()
            .iter()
            .any(|tool| tool.name == "get_asset_evidence"));
    }

    /// The per-tool allow-list (UI-SPEC §16, card 178) gates the MCP plane: default allows
    /// everything, a set restricts to its members, and a disabled tool errors.
    #[test]
    fn allow_list_gates_the_mcp_plane() {
        let (_dir, state) = fixture();

        // Construction default: no allow-list, every tool live.
        assert!(state.allows("export_asset"), "allow-all by default");

        let enabled: std::collections::HashSet<String> = ["search_assets".to_string()].into();
        state.set_allowed(enabled);

        let lode = Lodestash::new(state);
        assert!(lode.gate("search_assets").is_ok(), "enabled tool passes");
        assert!(lode.gate("export_asset").is_err(), "disabled tool refused");
    }

    /// Bytes are a separate permission from metadata (v1 plan S10, ADR-0020). Without byte release
    /// the preview tool still answers — the asset's identity, and a link when there is a file — and
    /// says why it carries no image. It is not an error: an agent that can read metadata has not
    /// done anything wrong by asking for the picture.
    #[test]
    fn a_preview_withholds_bytes_until_they_are_released() {
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
                    rel_path: "rock.png".into(),
                    name: "rock.png".into(),
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
        let mcp = Lodestash::new(state.clone());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let call = || {
            let out = rt
                .block_on(mcp.get_asset_preview(Parameters(
                    serde_json::from_value(serde_json::json!({ "id": id })).unwrap(),
                )))
                .expect("withholding bytes is not a protocol error");
            assert_ne!(
                out.is_error,
                Some(true),
                "asking for a preview is not an error"
            );
            serde_json::to_string(&out).unwrap()
        };

        let withheld = call();
        assert!(withheld.contains("withheld"), "MCP said {withheld}");
        assert!(
            withheld.contains("rock.png"),
            "the tool must still answer with what the asset is: {withheld}"
        );

        state.set_byte_release(true);
        let released = call();
        assert!(
            !released.contains("withheld"),
            "bytes released and the tool still refuses: {released}"
        );
    }

    #[test]
    fn evidence_tool_delivers_the_manifest_and_preview_image() {
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
                    rel_path: "arch.fbx".into(),
                    name: "arch.fbx".into(),
                    media_type: "model_3d".into(),
                    size_bytes: 1,
                    mtime: 1,
                }],
            )
            .unwrap();
            c.assets_needing_tags_within(1, None).unwrap()[0].asset_id
        };
        let preview = dir.path().join("arch.png");
        let png = vec![
            0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13, b'I', b'H', b'D', b'R', 0, 0, 0,
            64, 0, 0, 0, 32,
        ];
        std::fs::write(&preview, &png).unwrap();
        let preview_for_renderer = preview.clone();
        let state = Arc::new(
            ServerState::new(
                &db,
                dir.path().to_path_buf(),
                Box::new(move |_| Some(preview_for_renderer.clone())),
            )
            .unwrap(),
        );
        state.set_byte_release(true);
        let mcp = Lodestash::new(state);
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let output = rt
            .block_on(mcp.get_asset_evidence(Parameters(EvidenceArgs {
                id,
                kind: None,
                max_dimension: None,
                views: None,
            })))
            .unwrap();
        let text = serde_json::to_string(&output).unwrap();
        assert!(text.contains("model_views"), "MCP said {text}");
        assert!(
            text.contains(&b64(&png)),
            "MCP omitted the preview image: {text}"
        );
        assert!(
            text.contains(&format!("asset://{id}/file")),
            "MCP said {text}"
        );
    }

    /// Every tool calls synchronous state on a one-thread runtime (v1 plan S11). A slow tool — a
    /// preview that takes a second and a half to render — must not stop `get_server_info` from
    /// answering meanwhile. Driven through the real JSON-RPC service, because the fix is in
    /// `call_tool` and a direct method call would never reach it.
    #[test]
    fn a_slow_tool_does_not_stall_the_others() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
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
                    rel_path: "rock.png".into(),
                    name: "rock.png".into(),
                    media_type: "image".into(),
                    size_bytes: 1,
                    mtime: 1,
                }],
            )
            .unwrap();
            c.assets_needing_tags_within(1, None).unwrap()[0].asset_id
        };
        let state = Arc::new(
            ServerState::new(
                &db,
                dir.path().to_path_buf(),
                Box::new(|_| {
                    std::thread::sleep(std::time::Duration::from_millis(1500));
                    None
                }),
            )
            .unwrap(),
        );
        state.set_byte_release(true);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let (client, server) = tokio::io::duplex(1 << 16);
            let _service = tokio::spawn(async move {
                use rmcp::ServiceExt;
                let running = Lodestash::new(state)
                    .serve(tokio::io::split(server))
                    .await
                    .expect("serve");
                let _ = running.waiting().await;
            });
            let (read, mut write) = tokio::io::split(client);
            let mut lines = BufReader::new(read).lines();
            let send = |v: serde_json::Value| {
                let mut line = v.to_string();
                line.push('\n');
                line
            };
            let next_id = |line: Option<String>| -> i64 {
                let v: serde_json::Value = serde_json::from_str(&line.expect("a reply")).unwrap();
                v["id"].as_i64().unwrap_or(-1)
            };
            let wait = std::time::Duration::from_secs(10);

            write
                .write_all(send(serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                    "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                               "clientInfo": {"name": "test", "version": "0"}}})).as_bytes())
                .await
                .unwrap();
            let init = tokio::time::timeout(wait, lines.next_line()).await.unwrap().unwrap();
            assert_eq!(next_id(init), 1);
            write
                .write_all(send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).as_bytes())
                .await
                .unwrap();

            let started = std::time::Instant::now();
            write
                .write_all(send(serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
                    "params": {"name": "get_asset_preview", "arguments": {"id": id}}})).as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            write
                .write_all(send(serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
                    "params": {"name": "get_server_info", "arguments": {}}})).as_bytes())
                .await
                .unwrap();

            let first = tokio::time::timeout(wait, lines.next_line()).await.unwrap().unwrap();
            let first_at = started.elapsed();
            assert_eq!(next_id(first), 3, "get_server_info answered after the slow preview");
            assert!(
                first_at < std::time::Duration::from_millis(1000),
                "get_server_info took {first_at:?} behind a preview render"
            );
            let second = tokio::time::timeout(wait, lines.next_line()).await.unwrap().unwrap();
            assert_eq!(next_id(second), 2, "the preview itself still answers");
        });
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
        state.set_vocabulary_policy("open".into()).unwrap();
        let mcp = Lodestash::new(state.clone());
        let rev = state.asset_detail_json(id).unwrap().unwrap()["annotation_revision"]
            .as_str()
            .unwrap()
            .to_string();
        let body = |rid: &str, producer: &str, value: &str| {
            serde_json::json!({
                "request_id": rid,
                "vocabulary_revision": state.get_vocabulary(&lodestash_api::VocabularyRequest::default()).unwrap()["revision"],
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
