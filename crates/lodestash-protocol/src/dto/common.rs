use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The input of an operation that takes none. An object, so a host may send `{}` (or nothing).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NoInput {}

/// One asset by id — `get_asset`, `get_asset_preview`, `suggest_tags`, `asset_file`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssetArgs {
    /// The asset's catalog id.
    pub id: i64,
}

/// One job by id — `get_job_status`, `cancel_job`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct JobArgs {
    /// Job id returned by a long-running tool.
    pub job_id: String,
}
