//! The MCP surface (v2 spec §7): one tool per `Agent` operation in the protocol catalog, and one
//! generic call path — input → [`OpSpec::request`] → HTTP → the app's answer. There is no
//! per-tool code: a new agent operation in the catalog is a new tool here, with nothing to write.
//!
//! **The app's REST answer is the tool's answer.** A JSON body is the structured content and the
//! same JSON as text; an image body is an image block; a non-2xx answer is `isError` carrying the
//! app's error envelope. One addition: `get_server_info` also names this server and the handshake
//! it used, so a host can debug its setup.

use std::sync::Arc;

use base64::Engine as _;
use lodestash_protocol::{find_op, Audience, Effect, OpSpec, OPERATIONS};
use lodestash_protocol::{ApiError, ErrorEnvelope};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, ListResourceTemplatesResult,
    ListToolsResult, PaginatedRequestParams, ProtocolVersion, ReadResourceRequestParams,
    ReadResourceResult, ResourceContents, ResourceTemplate, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler};
use serde_json::{json, Map, Value};

use crate::forward::{App, Reply};

/// Most bytes one resource read returns inline (ENGINE-API §8.2): bigger files are read from the
/// REST file route, not as base64 in a model's context.
const MAX_INLINE_RESOURCE_BYTES: usize = 4 << 20;

#[derive(Clone)]
pub struct Lodestash {
    app: Arc<App>,
}

impl Lodestash {
    pub fn new(app: App) -> Self {
        Self { app: Arc::new(app) }
    }
}

/// The catalog's agent operation called `name`. An `Engine` or `Ui` operation is not a tool.
fn agent_op(name: &str) -> Option<&'static OpSpec> {
    find_op(name).filter(|op| op.audience == Audience::Agent)
}

/// Every tool: the catalog's `Agent` operations, in declaration order.
pub fn tools() -> Vec<Tool> {
    OPERATIONS
        .iter()
        .filter(|op| op.audience == Audience::Agent)
        .map(tool)
        .collect()
}

fn tool(op: &'static OpSpec) -> Tool {
    let schema = match serde_json::to_value((op.input_schema)()) {
        Ok(Value::Object(schema)) => schema,
        // The catalog's own test holds every input schema to an object.
        _ => Map::from_iter([("type".to_string(), json!("object"))]),
    };
    Tool::new(op.name, op.summary, Arc::new(schema)).with_annotations(hints(op.effect))
}

/// MCP's behaviour hints from the operation's effect. Every tool acts on the local catalog only,
/// so none is open-world. A `Gated` operation is the one class that can replace or copy out what
/// is there (`write_tags` in `replace_producer` mode, `export_asset`, the imports), and it waits
/// for the person at the app to approve it.
fn hints(effect: Effect) -> ToolAnnotations {
    let hints = ToolAnnotations::new().open_world(false);
    match effect {
        Effect::Read => hints.read_only(true).idempotent(true),
        Effect::Write | Effect::Job => hints.read_only(false).destructive(false),
        Effect::Gated => hints.read_only(false).destructive(true),
    }
}

/// One tool call, start to finish. Blocking: it runs on the blocking pool.
fn run(app: &App, op: &OpSpec, input: &Value) -> CallToolResult {
    let request = match op.request(input) {
        Ok(request) => request,
        // Refused before anything is sent: the model reads why and can correct its input.
        Err(e) => return refusal(e),
    };
    match app.call(&request) {
        Ok((reply, port)) => {
            let mut result = relay(reply);
            if op.name == "get_server_info" {
                describe_this_server(&mut result, app, port);
            }
            result
        }
        Err(e) => refusal(e),
    }
}

/// The app's answer as a tool result.
fn relay(reply: Reply) -> CallToolResult {
    let b64 = || base64::engine::general_purpose::STANDARD.encode(&reply.body);
    if !(200..300).contains(&reply.status) {
        return CallToolResult::structured_error(envelope(&reply));
    }
    if reply.content_type.starts_with("image/") {
        return CallToolResult::success(vec![ContentBlock::image(b64(), &reply.content_type)]);
    }
    if reply.content_type.starts_with("audio/") {
        return CallToolResult::success(vec![ContentBlock::audio(b64(), &reply.content_type)]);
    }
    if reply.body.is_empty() {
        return CallToolResult::success(vec![]);
    }
    match serde_json::from_slice::<Value>(&reply.body) {
        // MCP's structured content is an object; the app's agent answers are objects.
        Ok(body @ Value::Object(_)) => CallToolResult::structured(body),
        Ok(body) => CallToolResult::success(vec![ContentBlock::text(body.to_string())]),
        Err(_) => match String::from_utf8(reply.body) {
            Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
            Err(raw) => CallToolResult::success(vec![ContentBlock::resource(
                ResourceContents::blob(
                    base64::engine::general_purpose::STANDARD.encode(raw.as_bytes()),
                    "lodestash:answer",
                )
                .with_mime_type(reply.content_type),
            )]),
        },
    }
}

/// A non-2xx answer's error envelope: the app's own when it sent one (ENGINE-API §4), otherwise one
/// built from the status — a route the app does not know, or a body the framework refused before
/// any handler saw it.
fn envelope(reply: &Reply) -> Value {
    if let Ok(body) = serde_json::from_slice::<Value>(&reply.body) {
        if body["error"]["code"].is_string() {
            return body;
        }
    }
    let code = match reply.status {
        400 | 405 | 415 | 422 => "invalid",
        401 => "unauthorized",
        403 => "denied",
        404 => "not_found",
        409 => "conflict",
        413 => "too_large",
        429 => "busy",
        503 => "unavailable",
        _ => "internal",
    };
    let text = String::from_utf8_lossy(&reply.body);
    let message = if text.trim().is_empty() {
        format!("the app answered {}", reply.status)
    } else {
        text.trim().to_string()
    };
    to_value(ApiError::new(code, message).with_detail(json!({ "status": reply.status })))
}

/// A refusal this server made itself — the input, or no app to send it to.
fn refusal(error: ApiError) -> CallToolResult {
    CallToolResult::structured_error(to_value(error))
}

fn to_value(error: ApiError) -> Value {
    serde_json::to_value(ErrorEnvelope { error })
        .unwrap_or_else(|e| json!({ "error": { "code": "internal", "message": e.to_string() } }))
}

/// `get_server_info`: add `mcp` — this server's version, the handshake file it read (the home
/// directory shown as `~`) and the port it reached — to the app's answer.
fn describe_this_server(result: &mut CallToolResult, app: &App, port: u16) {
    if result.is_error == Some(true) {
        return;
    }
    let Some(Value::Object(info)) = result.structured_content.as_mut() else {
        return;
    };
    info.insert(
        "mcp".into(),
        json!({
            "version": env!("CARGO_PKG_VERSION"),
            "transport": "stdio",
            "handshake": app.handshake_path().map(crate::launch::shown),
            "port": port,
        }),
    );
    let text = Value::Object(info.clone()).to_string();
    result.content = vec![ContentBlock::text(text)];
}

/// `asset://{id}/file` → the asset's id.
fn asset_file_id(uri: &str) -> Option<i64> {
    uri.strip_prefix("asset://")?
        .strip_suffix("/file")?
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
}

/// Read `asset://{id}/file` through the app's file route — the same resolver and byte policy as
/// REST (ENGINE-API §8.2) — inline up to [`MAX_INLINE_RESOURCE_BYTES`].
fn read_asset_file(app: &App, uri: &str) -> Result<ReadResourceResult, ErrorData> {
    let id = asset_file_id(uri).ok_or_else(|| {
        ErrorData::resource_not_found(format!("{uri} is not an asset://{{id}}/file URI"), None)
    })?;
    let op = find_op("asset_file").expect("the catalog declares asset_file");
    let refused = |e: ApiError| {
        let message = e.message.clone();
        ErrorData::invalid_request(message, Some(to_value(e)))
    };
    let request = op.request(&json!({ "id": id })).map_err(refused)?;
    let (reply, _) = app.call(&request).map_err(refused)?;
    if !(200..300).contains(&reply.status) {
        let body = envelope(&reply);
        let message = body["error"]["message"]
            .as_str()
            .unwrap_or("the app refused the file")
            .to_string();
        return Err(ErrorData::invalid_request(message, Some(body)));
    }
    if reply.body.len() > MAX_INLINE_RESOURCE_BYTES {
        return Err(refused(ApiError::new(
            "resource_too_large",
            format!(
                "the file is {} bytes; read it from /v1/assets/{id}/file instead",
                reply.body.len()
            ),
        )));
    }
    Ok(ReadResourceResult::new(vec![ResourceContents::blob(
        base64::engine::general_purpose::STANDARD.encode(&reply.body),
        uri,
    )
    .with_mime_type(reply.content_type)]))
}

impl ServerHandler for Lodestash {
    fn get_info(&self) -> ServerInfo {
        // `ServerInfo`/`Implementation` are #[non_exhaustive]: build from Default and set fields.
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
             images and audio the user already owns. Every tool is answered by the Lodestash \
             app, which this server starts when it is not running."
                .into(),
        );
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(tools()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        // An unknown name cannot be routed at all: a protocol error, not a tool result.
        let op = agent_op(&request.name).ok_or_else(|| {
            ErrorData::invalid_params(format!("no tool is named {:?}", request.name), None)
        })?;
        let input = request.arguments.map(Value::Object).unwrap_or(Value::Null);
        let app = self.app.clone();
        // Blocking HTTP, a launch that may wait 20 s: off the async threads, so a slow call does
        // not hold up `get_job_status` or `cancel_job`.
        tokio::task::spawn_blocking(move || run(&app, op, &input))
            .await
            .map_err(|e| ErrorData::internal_error(format!("the tool worker failed: {e}"), None))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new("asset://{id}/file", "asset_file").with_description(
                "An asset's source bytes, when this catalog releases bytes to agents. Large files \
                 are refused here; read them over the authenticated REST file route instead.",
            ),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let app = self.app.clone();
        tokio::task::spawn_blocking(move || read_asset_file(&app, &request.uri))
            .await
            .map_err(|e| {
                ErrorData::internal_error(format!("the resource worker failed: {e}"), None)
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(status: u16, content_type: &str, body: &[u8]) -> Reply {
        Reply {
            status,
            content_type: content_type.into(),
            body: body.to_vec(),
        }
    }

    /// A body that is not the app's envelope — axum refusing a body before any handler saw it —
    /// still reaches the model as an envelope, with the status it came with.
    #[test]
    fn a_refusal_without_an_envelope_gets_one() {
        let result = relay(reply(
            422,
            "text/plain",
            b"Failed to deserialize the JSON body",
        ));
        assert_eq!(result.is_error, Some(true));
        let body = result.structured_content.unwrap();
        assert_eq!(body["error"]["code"], "invalid");
        assert_eq!(body["error"]["detail"]["status"], 422);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("deserialize"));
    }

    /// The app's envelope passes through untouched, whatever else it carries.
    #[test]
    fn the_apps_envelope_is_relayed_as_it_came() {
        let sent = json!({"error": {"code": "tool_disabled", "message": "off", "detail": ""}});
        let result = relay(reply(403, "application/json", sent.to_string().as_bytes()));
        assert_eq!(result.is_error, Some(true));
        assert_eq!(result.structured_content.unwrap(), sent);
    }

    /// A JSON answer that is not an object stays text: MCP's structured content is an object.
    #[test]
    fn a_json_answer_that_is_not_an_object_is_text() {
        let result = relay(reply(200, "application/json", b"[1,2]"));
        assert_eq!(result.is_error, Some(false));
        assert!(result.structured_content.is_none());
        assert_eq!(result.content[0].as_text().unwrap().text, "[1,2]");
    }

    #[test]
    fn only_asset_file_uris_name_an_asset() {
        assert_eq!(asset_file_id("asset://42/file"), Some(42));
        for uri in [
            "asset://0/file",
            "asset://-1/file",
            "asset://x/file",
            "asset://42",
            "file:///etc/passwd",
        ] {
            assert_eq!(asset_file_id(uri), None, "{uri}");
        }
    }
}
