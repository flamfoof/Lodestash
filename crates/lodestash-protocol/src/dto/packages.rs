//! Package and export inputs (ENGINE-API §2).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `unpack_package`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UnpackArgs {
    /// The pack to extract into the workspace.
    pub package_id: i64,
}

/// `export_asset` ↔ `POST /v1/export`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExportArgs {
    /// Assets to copy out.
    pub asset_ids: Vec<i64>,
    /// Absolute destination folder. Must be outside every library source root.
    pub dest_path: String,
    /// Also copy each asset's dependency closure — its materials and their textures (default
    /// false).
    #[serde(default)]
    pub with_dependencies: bool,
}
