//! Tag, mapping and identification inputs (ENGINE-API §2, §8.3).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `list_tags` ↔ `GET /v1/tags`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TagsArgs {
    /// Only tags whose namespace or value starts with this (optional).
    pub prefix: Option<String>,
    /// Maximum rows (default 500).
    pub limit: Option<usize>,
}

/// `add_tag` ↔ `POST /v1/assets/{asset_id}/tags`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AddTagArgs {
    /// The asset to tag.
    pub asset_id: i64,
    /// Namespace (default "free").
    #[serde(default = "free")]
    pub namespace: String,
    /// The tag value, e.g. "rusty".
    pub value: String,
}

fn free() -> String {
    "free".into()
}

// `write_tags` (ENGINE-API §8.3). These three serialize byte-for-byte as v1's `lodestash-api`
// types do — same fields, same order, `None` as `null` — because a committed request's receipt is
// a digest of that serialization: a different byte would turn every retry into `request_conflict`.
// `lodestash-api/tests/protocol_types.rs` holds them to it.

/// One proposed assignment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagIn {
    /// A facet a model may write: subject | style | material | mood | usage | free.
    pub namespace: String,
    pub value: String,
    /// Your confidence in [0, 1], or null.
    #[serde(default)]
    pub confidence: Option<f64>,
}

/// One asset's part of a `write_tags` request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagBatchAsset {
    pub asset_id: i64,
    /// The `annotation_revision` get_asset reported. If anything changed since, the batch is refused.
    pub expected_revision: String,
    /// The producer id configured for this host in Lodestash.
    pub producer_id: String,
    /// The model you are, when you know it; null otherwise. Never guess.
    #[serde(default)]
    pub model_id: Option<String>,
    /// `append` (add to this producer's tags) or `replace_producer` (replace them).
    pub mode: String,
    #[serde(default)]
    pub tags: Vec<TagIn>,
    /// A short description; omit to leave the caption as it is.
    #[serde(default)]
    pub description: Option<String>,
}

/// `write_tags` ↔ `POST /v1/tags/batch`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TagBatchRequest {
    /// A fresh UUID per request. Reuse it only to retry the same body after a lost response.
    pub request_id: String,
    /// The vocabulary revision the proposals were checked against (enforced once review lands).
    #[serde(default)]
    pub vocabulary_revision: Option<String>,
    /// 1 to 100 assets.
    pub assets: Vec<TagBatchAsset>,
}

/// `apply_mapping` ↔ `POST /v1/mapping/apply`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct MappingArgs {
    /// Taxonomy to run the deterministic mapping engine over.
    pub taxonomy_id: i64,
}

/// `identify_asset` ↔ `POST /v1/identify`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IdentifyArgs {
    /// Assets to flag + appraise.
    pub asset_ids: Vec<i64>,
}
