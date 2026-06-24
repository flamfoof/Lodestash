//! Shared enums and value types. Keep every variant in sync with DATA-MODEL.md §3
//! and the media_type invariant in CLAUDE.md.

use serde::{Deserialize, Serialize};

/// Canonical asset media type.
/// Invariant: keep in sync with DATA-MODEL.md §3, ENGINE-API.md, IDENTIFICATION-AND-SEARCH.md.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaType {
    Model3d,
    Texture,
    Audio,
    Image,
    Sprite,
    Material,
    Font,
}

/// Asset lifecycle status. Invariant: discovered → indexed → identified → confirmed.
/// See DATA-MODEL.md §8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetStatus {
    Discovered,
    Indexed,
    Identified,
    Confirmed,
}

/// Unique identifier for an asset in the catalog.
pub type AssetId = uuid::Uuid;
