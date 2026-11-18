//! The operation catalog: what each `/v1` operation is called, where it lives, who may call it,
//! what it does to the catalog, and the types it takes and answers (v2 spec §5).

use schemars::{generate::SchemaSettings, JsonSchema, Schema};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

use crate::dto::*;
use crate::error::{codes, ApiError};
use crate::wire::{self, WireRequest};

/// An operation's HTTP method.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl HttpMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// Who an operation is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Audience {
    /// An MCP tool *and* an HTTP route — the 1:1 invariant, by construction. The UI may call these
    /// too.
    Agent,
    /// HTTP only: plumbing for the Unity plugin (ENGINE-API §5.3, §8.2), not an operation an LLM
    /// should call.
    Engine,
    /// HTTP only, and only from the app's UI session.
    Ui,
}

/// What an operation does to the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Effect {
    /// Changes nothing a later read would see.
    Read,
    /// Changes the catalog (or the import queue) before it answers.
    Write,
    /// Starts a job and answers with it (`202`); poll `get_job_status`.
    Job,
    /// Parks a job on the person's approval gate when an agent calls it (ENGINE-API §2,
    /// "Server-side confirmation"); nothing happens until they approve.
    Gated,
}

/// Who is calling, as the HTTP adapter established it (v2 spec §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Caller {
    /// A bearer-token client — the MCP server, the Unity plugin, a script. Agent approval gates
    /// and the per-tool allow-list apply.
    Agent,
    /// The app's own UI session: the person at the machine. Agent gates do not apply; the UI's
    /// own confirmations do.
    Ui,
}

/// One operation, as declared in [`OPERATIONS`].
#[derive(Debug, Clone, Copy)]
pub struct OpSpec {
    /// The MCP tool name for an `Agent` operation; the operation's name everywhere.
    pub name: &'static str,
    pub method: HttpMethod,
    /// The path template under ENGINE-API §2's conventions. `{field}` segments are filled from the
    /// input field of that name. A trailing `?k=v` is a fixed query that selects a variant of a
    /// route another operation shares (`semantic_search` is `/v1/search?mode=ai`).
    pub path: &'static str,
    pub audience: Audience,
    pub effect: Effect,
    /// One line on what the operation does — an `Agent` operation's MCP tool description.
    pub summary: &'static str,
    /// JSON Schema (2020-12) of the input; an `Agent` operation's MCP tool input schema.
    pub input_schema: fn() -> Schema,
    /// JSON Schema of a successful answer's body.
    pub output_schema: fn() -> Schema,
    /// The input decoded into the operation's input type and encoded again: checks it (`invalid`
    /// when the type refuses it) and puts it in wire form — defaults filled, v1 spellings used.
    pub canonical_input: fn(&Value) -> Result<Value, ApiError>,
}

impl OpSpec {
    /// The path template without its fixed query: the route an HTTP router registers.
    pub fn route_path(&self) -> &'static str {
        self.path
            .split_once('?')
            .map_or(self.path, |(route, _)| route)
    }

    /// The fixed query (`mode=ai`), if the path carries one.
    pub fn fixed_query(&self) -> Option<&'static str> {
        self.path.split_once('?').map(|(_, query)| query)
    }

    /// The input fields the path is filled from, in path order.
    pub fn path_params(&self) -> impl Iterator<Item = &'static str> {
        self.route_path().split('/').filter_map(wire::param_name)
    }

    /// The HTTP request that performs this operation for `input` — what a generic client (the MCP
    /// forwarder) sends. See the crate docs, "From input to request".
    pub fn request(&self, input: &Value) -> Result<WireRequest, ApiError> {
        wire::request(self, input)
    }
}

/// The operation called `name`.
pub fn find_op(name: &str) -> Option<&'static OpSpec> {
    OPERATIONS.iter().find(|op| op.name == name)
}

/// The schema of `T` as MCP hosts expect it: JSON Schema 2020-12 (the generator settings rmcp
/// uses for tool schemas).
fn schema_for<T: JsonSchema>() -> Schema {
    SchemaSettings::draft2020_12()
        .into_generator()
        .into_root_schema_for::<T>()
}

/// Decode an operation's input: a JSON object (or nothing), where a null field is an absent one.
/// A refusal is `invalid`, its message prefixed with the operation's name by the caller.
fn decode_input<T: DeserializeOwned>(input: Value) -> Result<T, ApiError> {
    let fields = match input {
        Value::Null => Map::new(),
        Value::Object(mut fields) => {
            fields.retain(|_, v| !v.is_null());
            fields
        }
        other => {
            return Err(ApiError::invalid(format!(
                "takes a JSON object, not {}",
                wire::kind(&other)
            )))
        }
    };
    serde_json::from_value(Value::Object(fields)).map_err(|e| ApiError::invalid(e.to_string()))
}

/// `error`, said of operation `op`.
pub(crate) fn of_op(op: &str, mut error: ApiError) -> ApiError {
    error.message = format!("{op}: {}", error.message);
    error
}

fn canonical_input<T: DeserializeOwned + Serialize>(input: &Value) -> Result<Value, ApiError> {
    encode_output(decode_input::<T>(input.clone())?)
}

fn encode_output<T: Serialize>(output: T) -> Result<Value, ApiError> {
    serde_json::to_value(output).map_err(|e| ApiError::new(codes::INTERNAL, e.to_string()))
}

operations! {
    get_server_info: GET "/v1/server-info" (NoInput) -> Value, Agent, Read,
        "App, schema and API versions.";
    search_assets: GET "/v1/search" (SearchArgs) -> Value, Agent, Read,
        "Search the asset catalog by text; returns asset summaries.";
    semantic_search: GET "/v1/search?mode=ai" (SearchArgs) -> Value, Agent, Read,
        "Find assets by description (\"mossy stone wall\", \"metallic clang\"). Ranks by meaning, \
         not filename. Errors if no embedding model is loaded.";
    list_libraries: GET "/v1/libraries" (ListLibrariesArgs) -> Value, Agent, Read,
        "The libraries in this catalog (id, name, kind, asset_count), paged by cursor. Use an id \
         as source_id for list_assets and the search tools.";
    list_assets: GET "/v1/assets" (ListAssetsArgs) -> Value, Agent, Read,
        "Walk one library completely: selection 'ordered' pages in id order — pass next_cursor \
         back until it is null; assets added after the walk began wait for the next walk. \
         selection 'random' returns a fresh sample without replacement. Filter by package, \
         media_type, status and tag_state. Search ranks; this visits.";
    get_assets: POST "/v1/assets/read" (GetAssetsArgs) -> Value, Agent, Read,
        "Full detail for 1-100 assets in the order given, with the ids that could not be returned \
         listed under 'unavailable'. Read each asset's revisions here before writing tags.";
    get_asset: GET "/v1/assets/{id}" (AssetArgs) -> Value, Agent, Read,
        "Full metadata for one asset, including technical facets.";
    get_asset_preview: GET "/v1/assets/{id}/preview" (AssetArgs) -> Value, Agent, Read,
        "A rendered preview image of the asset (3D turntable, texture, or audio waveform). The \
         answer is the image alone; the source file's link is file_resource_uri in get_asset.";
    find_similar: GET "/v1/assets/{id}/similar" (SimilarArgs) -> Value, Agent, Read,
        "Assets most visually (or sonically) similar to the given one. Empty if the library has \
         not been embedded yet.";
    suggest_tags: GET "/v1/assets/{id}/suggest-tags" (AssetArgs) -> Value, Agent, Read,
        "Read the tags proposed for an asset, with AI confidences. Empty for an untagged asset.";
    list_tags: GET "/v1/tags" (TagsArgs) -> Value, Agent, Read,
        "List the tag vocabulary with asset counts, most-used first.";
    add_tag: POST "/v1/assets/{asset_id}/tags" (AddTagArgs) -> Value, Agent, Write,
        "Add a tag to an asset. Mutating — the catalog write goes through the app's single writer.";
    write_tags: POST "/v1/tags/batch" (TagBatchRequest) -> Value, Agent, Gated,
        "Write tags (and optionally a short description) on 1-100 assets as this host's \
         configured producer. Read each asset's annotation_revision with get_asset first. Returns \
         a job in state 'awaiting_approval' until the desktop owner approves (poll \
         get_job_status); retrying the same request_id and body returns the original result. \
         Never confirms an asset.";
    list_collections: GET "/v1/collections" (NoInput) -> Value, Agent, Read,
        "List the user's collections (and the active project's staging tray, flagged is_staging).";
    apply_mapping: POST "/v1/mapping/apply" (MappingArgs) -> Value, Agent, Write,
        "Run the deterministic mapping engine over one taxonomy, writing category assignments. \
         Mutating.";
    identify_asset: POST "/v1/identify" (IdentifyArgs) -> Value, Agent, Job,
        "Flag and appraise specific assets (embeddings, tags, promote). Long-running: returns a \
         job id to poll with get_job_status.";
    list_packages: GET "/v1/packages" (NoInput) -> Value, Agent, Read,
        "Asset Store packages in the library (publisher, category, asset count).";
    unpack_package: POST "/v1/packages/unpack" (UnpackArgs) -> Value, Agent, Job,
        "Extract a .unitypackage into the workspace so its assets become local files. \
         Long-running: returns a job id.";
    export_asset: POST "/v1/export" (ExportArgs) -> Value, Agent, Gated,
        "Copy assets to a folder outside the library. Returns a job that stays in state \
         'awaiting_approval' until the desktop app's owner approves it — poll get_job_status.";
    register_project: POST "/v1/projects/register" (RegisterProjectArgs) -> Value, Agent, Write,
        "Register the open Unity editor as the active import target. Idempotent per project path; \
         returns { project_id, is_active }.";
    list_projects: GET "/v1/projects" (NoInput) -> Value, Agent, Read,
        "Registered Unity editors (path, version, pipeline) and which is active.";
    import_to_project: POST "/v1/import" (ImportRequest) -> Value, Agent, Gated,
        "Import assets — with their dependency closure — into the open Unity project. Returns a \
         job in state 'awaiting_approval' until the desktop owner approves (poll get_job_status), \
         reports assets already imported, or errors if no editor is registered.";
    stage_asset: POST "/v1/stage" (StageArgs) -> Value, Agent, Write,
        "Add an asset to the active project's staging tray (per-project shortlist). Requires a \
         registered editor.";
    list_staged: GET "/v1/stage" (NoInput) -> Value, Agent, Read,
        "List the active project's staged assets (the shortlist before import).";
    import_staged: POST "/v1/stage/import" (NoInput) -> Value, Agent, Gated,
        "Queue every staged asset for import into the active project. Parks on the human approval \
         gate; returns a job id.";
    get_job_status: GET "/v1/jobs/{job_id}" (JobArgs) -> Value, Agent, Read,
        "State and progress of a job (awaiting_approval | queued | running | done | error | \
         cancelled).";
    cancel_job: POST "/v1/jobs/{job_id}/cancel" (JobArgs) -> Value, Agent, Write,
        "Request cancellation of a job. Idempotent; finished jobs are unaffected.";

    import_queue: GET "/v1/import-queue" (ImportQueueArgs) -> Value, Engine, Read,
        "The Unity plugin's poll: one project's approved imports, each with its assets' file \
         URLs. A polled request is not handed out again until its lease lapses or it is acked.";
    import_queue_ack: POST "/v1/import-queue/{request_id}/ack" (AckRequest) -> Value, Engine, Write,
        "The Unity plugin reports what it imported for one polled request: the import records \
         are written with Unity's real paths and GUIDs, and the import job finishes.";
    asset_file: GET "/v1/assets/{id}/file" (AssetArgs) -> Value, Engine, Read,
        "An asset's source bytes — an in-pack payload pulled and verified — when this catalog \
         releases bytes. The target of an MCP resource link and of the plugin's file URLs.";
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// `dispatch` routes by name to the operation's own input type: defaults are filled, v1
    /// spellings read, null treated as absent — the same decoding `OpSpec::request` checks with.
    #[test]
    fn dispatch_decodes_into_the_operations_input_type() {
        let echo = |name: &str, input: Value| {
            let err = dispatch(&Echo, name, &Caller::Agent, input).unwrap_err();
            assert_eq!((err.code.as_str(), err.message.as_str()), ("echo", name));
            err.detail.unwrap()
        };
        assert_eq!(
            echo(
                "add_tag",
                json!({ "asset_id": 7, "value": "rusty", "namespace": null })
            ),
            json!({ "asset_id": 7, "namespace": "free", "value": "rusty" })
        );
        assert_eq!(
            echo("search_assets", json!({ "q": "stone" }))["q"],
            "stone",
            "the v1 REST spelling is read"
        );
        assert_eq!(echo("list_packages", Value::Null), json!({}));
        assert_eq!(
            echo(
                "import_queue_ack",
                json!({ "request_id": "req_1", "imported": [] })
            ),
            json!({ "request_id": "req_1", "imported": [] })
        );
    }

    #[test]
    fn dispatch_refuses_an_unknown_name_and_input_its_type_refuses() {
        let err = dispatch(&Echo, "drop_tables", &Caller::Agent, json!({})).unwrap_err();
        assert_eq!(err.code, codes::NOT_FOUND);
        for (name, input) in [
            ("get_asset", json!({})),
            ("get_asset", json!({ "id": "seven" })),
            (
                "write_tags",
                json!({ "request_id": "r", "assets": [], "extra": 1 }),
            ),
            ("list_tags", json!([1, 2])),
        ] {
            let err = dispatch(&Echo, name, &Caller::Agent, input.clone()).unwrap_err();
            assert_eq!(err.code, codes::INVALID, "{name} {input}");
            assert!(err.message.starts_with(name), "{}", err.message);
        }
    }

    /// The generated trait and the catalog are one list: every declared operation dispatches.
    #[test]
    fn every_declared_operation_dispatches() {
        for op in OPERATIONS {
            let err = dispatch(&Echo, op.name, &Caller::Ui, json!({})).unwrap_err();
            assert!(
                (err.code == "echo" && err.message == op.name) || err.code == codes::INVALID,
                "{}: {err}",
                op.name
            );
        }
    }
}
