//! Discovery and search inputs (ENGINE-API §2, §8.1).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// `search_assets` and `semantic_search` — the text, a limit and the §8.1 scope.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Words to match against asset names, descriptions and tags. Empty returns the most
    /// recently catalogued assets.
    // v1 REST reads this as `q` (ENGINE-API §2) and the MCP tool as `query`: both are read, and it
    // is sent as `q` so a request built from this type reaches a v1 server unchanged.
    #[serde(default, rename(serialize = "q"), alias = "q")]
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ListLibrariesArgs {
    /// Page size (default 25, maximum 200).
    pub limit: Option<usize>,
    /// `next_cursor` from the previous page.
    pub cursor: Option<String>,
}

/// `list_assets` (ENGINE-API §8.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
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

/// `get_assets` (ENGINE-API §8.1). Read-only, though it travels as a POST body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetAssetsArgs {
    /// 1 to 100 distinct asset ids.
    pub asset_ids: Vec<i64>,
    /// When given, assets outside this library are reported unavailable.
    pub source_id: Option<i64>,
}

/// `find_similar` (ENGINE-API §2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SimilarArgs {
    /// The asset to find neighbours for.
    pub id: i64,
    /// Maximum neighbours (default 10).
    pub limit: Option<usize>,
}
