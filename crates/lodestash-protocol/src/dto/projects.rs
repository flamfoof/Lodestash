//! Unity project, import and staging inputs (ENGINE-API §5, §6).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `register_project` ↔ `POST /v1/projects/register` (ENGINE-API §5.1).
///
/// The plugin also sends `can_execute_main_thread`; it is read and ignored, as v1 does, until the
/// import-queue drain uses it — so it is not offered to agents as an argument.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegisterProjectArgs {
    /// Absolute path to the Unity project root.
    pub project_path: String,
    /// Editor version, e.g. "6000.0.23f1".
    pub unity_version: Option<String>,
    /// Render pipeline: "built_in" | "urp" | "hdrp".
    pub pipeline: Option<String>,
}

/// `import_to_project` ↔ `POST /v1/import` (ENGINE-API §5.2). `project_id` omitted ⇒ the active
/// editor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportRequest {
    /// Assets to import into the project.
    pub asset_ids: Vec<i64>,
    /// Target project id; omit to use the active editor.
    pub project_id: Option<i64>,
    /// Subfolder under `Assets/` to import into (default "Lodestash").
    pub dest_subfolder: Option<String>,
    /// Bring each asset's dependency closure — materials, textures (default true).
    // v1 disagreed with itself: the MCP tool, the staged import and the desktop all meant true,
    // the REST body alone read an absent field as false. The operation means true.
    #[serde(default = "yes")]
    pub with_dependencies: bool,
}

fn yes() -> bool {
    true
}

/// `stage_asset` ↔ `POST /v1/stage` (ENGINE-API §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StageArgs {
    /// The asset to add to the active project's staging tray.
    pub asset_id: i64,
}

/// `import_queue` ↔ `GET /v1/import-queue` — the Unity plugin's poll (ENGINE-API §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ImportQueueArgs {
    /// The registered project whose pending imports to hand out.
    pub project_id: i64,
}

/// One asset the Unity plugin reports it imported (ENGINE-API §5.3 ack) — the real in-project path
/// and GUID Unity assigned, from which the durable `import_record` is written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AckAsset {
    pub asset_id: i64,
    pub dest_path: String,
    #[serde(default)]
    pub unity_guid: Option<String>,
}

/// `import_queue_ack` ↔ `POST /v1/import-queue/{request_id}/ack` (ENGINE-API §5.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AckRequest {
    /// The `request_id` the poll handed out (a path field).
    pub request_id: String,
    /// What was imported; empty when nothing was.
    pub imported: Vec<AckAsset>,
}
