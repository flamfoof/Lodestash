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
        let Some(row) = self.state.asset(args.id).map_err(internal)? else {
            return Err(ErrorData::invalid_params("no such asset", None));
        };
        let mut body = lodestash_api::summary_json(&row);
        body["size_bytes"] = serde_json::json!(row.size_bytes);
        body["stats"] = serde_json::json!({
            "tris": row.metrics.tri_count, "width_px": row.metrics.width_px,
            "height_px": row.metrics.height_px, "duration_ms": row.metrics.duration_ms,
        });
        let deps = self.state.dependencies(args.id).unwrap_or_default();
        body["dependencies"] = serde_json::json!({ "count": deps.len(), "asset_ids": deps });
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
            Ok(lodestash_api::ImportKickoff::Awaiting(job)) => {
                Ok(CallToolResult::success(vec![ContentBlock::json(
                    job.to_json(),
                )?]))
            }
            Ok(lodestash_api::ImportKickoff::AllAlreadyImported(ids)) => {
                Ok(CallToolResult::success(vec![ContentBlock::json(
                    serde_json::json!({ "imported_existing": ids, "job_id": null }),
                )?]))
            }
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
        let Some(png) = self.state.preview_path(args.id) else {
            return Err(ErrorData::internal_error("no preview for this asset", None));
        };
        let bytes = std::fs::read(&png)
            .map_err(|e| ErrorData::internal_error(format!("preview unreadable: {e}"), None))?;

        // MCP-AND-ENGINES "Returning previews — the rule": always pair a viewable image with a
        // resource_link. The model reasons over the image; the consumer fetches the bytes.
        let mut content = vec![ContentBlock::image(b64(&bytes), "image/png")];
        if let Some(src) = self.state.source_path(&row) {
            content.push(ContentBlock::resource_link(
                Resource::new(file_uri(&src), row.name.clone())
                    .with_description(format!("{} source file", row.media_type)),
            ));
        }
        Ok(CallToolResult::success(content))
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
}
