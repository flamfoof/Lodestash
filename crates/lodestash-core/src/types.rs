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

impl MediaType {
    /// Parse a catalog `media_type.code`; unknown codes are None (schema drift, not a panic).
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "model_3d" => Some(MediaType::Model3d),
            "texture" => Some(MediaType::Texture),
            "audio" => Some(MediaType::Audio),
            "image" => Some(MediaType::Image),
            "sprite" => Some(MediaType::Sprite),
            "material" => Some(MediaType::Material),
            "font" => Some(MediaType::Font),
            _ => None,
        }
    }

    /// The catalog `media_type.code` string (DATA-MODEL §2). Must match the serde
    /// snake_case names and the `seed_media_types` rows.
    pub fn code(self) -> &'static str {
        match self {
            MediaType::Model3d => "model_3d",
            MediaType::Texture => "texture",
            MediaType::Audio => "audio",
            MediaType::Image => "image",
            MediaType::Sprite => "sprite",
            MediaType::Material => "material",
            MediaType::Font => "font",
        }
    }
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

impl AssetStatus {
    /// The catalog `asset.status` string (DATA-MODEL §8). Matches the serde names.
    pub fn code(self) -> &'static str {
        match self {
            AssetStatus::Discovered => "discovered",
            AssetStatus::Indexed => "indexed",
            AssetStatus::Identified => "identified",
            AssetStatus::Confirmed => "confirmed",
        }
    }

    /// Parse a catalog status string; unknown values are None (schema drift, not a panic).
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "discovered" => Some(AssetStatus::Discovered),
            "indexed" => Some(AssetStatus::Indexed),
            "identified" => Some(AssetStatus::Identified),
            "confirmed" => Some(AssetStatus::Confirmed),
            _ => None,
        }
    }

    /// The user-facing fold (DATA-MODEL §8): `discovered|indexed` are Unidentified.
    pub fn is_unidentified(self) -> bool {
        matches!(self, AssetStatus::Discovered | AssetStatus::Indexed)
    }
}

/// Unique identifier for an asset in the catalog.
pub type AssetId = uuid::Uuid;

/// What a per-source blacklist entry excludes. See DATA-MODEL.md §1 (`import_source.blacklist`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExcludeKind {
    /// A folder (and everything under it), relative to the source root.
    Folder,
    /// A single asset — matched by exact relative path or by file name.
    Asset,
}

/// A per-source indexing exclusion: "skip this folder/asset under this source when indexing."
/// Non-destructive — only affects cataloguing, never files on disk (DATA-MODEL.md §6). `value` is
/// a source-relative path (folder) or a relative path / file name (asset), compared with `/` and
/// `\` treated alike so entries typed on any OS still match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exclude {
    pub kind: ExcludeKind,
    pub value: String,
}

impl Exclude {
    /// Does this exclusion match a source-relative path?
    /// - `Folder`: `rel` equals the value or lies under it (any path prefix of components).
    /// - `Asset`: `rel` equals the value, or its file name equals the value.
    pub fn matches(&self, rel: &std::path::Path) -> bool {
        let norm = |s: &str| s.replace('\\', "/").trim_matches('/').to_lowercase();
        let target = norm(&self.value);
        if target.is_empty() {
            return false;
        }
        let rel_s = norm(&rel.to_string_lossy());
        match self.kind {
            ExcludeKind::Folder => rel_s == target || rel_s.starts_with(&format!("{target}/")),
            ExcludeKind::Asset => {
                rel_s == target
                    || rel
                        .file_name()
                        .map(|n| norm(&n.to_string_lossy()) == target)
                        .unwrap_or(false)
            }
        }
    }
}

/// Is a source-relative path excluded by any entry in the blacklist?
pub fn is_excluded(rel: &std::path::Path, excludes: &[Exclude]) -> bool {
    excludes.iter().any(|e| e.matches(rel))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn folder(v: &str) -> Exclude {
        Exclude {
            kind: ExcludeKind::Folder,
            value: v.into(),
        }
    }
    fn asset(v: &str) -> Exclude {
        Exclude {
            kind: ExcludeKind::Asset,
            value: v.into(),
        }
    }

    #[test]
    fn folder_exclude_matches_the_dir_and_its_subtree_only() {
        let e = folder("Editor");
        assert!(e.matches(Path::new("Editor")), "the folder itself");
        assert!(e.matches(Path::new("Editor/tool.cs")), "a file under it");
        assert!(e.matches(Path::new("Editor/sub/deep.png")), "nested");
        assert!(
            !e.matches(Path::new("Editors/x.png")),
            "prefix but not a path boundary"
        );
        assert!(
            !e.matches(Path::new("Models/Editor")),
            "same name deeper is not the excluded root"
        );
    }

    #[test]
    fn asset_exclude_matches_exact_path_or_file_name() {
        assert!(
            asset("Models/chair.fbx").matches(Path::new("Models/chair.fbx")),
            "exact rel path"
        );
        assert!(
            asset("chair.fbx").matches(Path::new("Models/chair.fbx")),
            "bare file name anywhere"
        );
        assert!(!asset("chair.fbx").matches(Path::new("Models/table.fbx")));
    }

    #[test]
    fn matching_is_slash_and_case_insensitive() {
        assert!(
            folder("Editor").matches(Path::new("editor/x.cs")),
            "case-insensitive"
        );
        assert!(
            folder("a/b").matches(Path::new("a\\b\\c.png")),
            "backslash paths match"
        );
    }

    #[test]
    fn empty_value_never_matches_and_is_excluded_helper() {
        assert!(!folder("").matches(Path::new("anything")));
        assert!(is_excluded(Path::new("Editor/x"), &[folder("Editor")]));
        assert!(!is_excluded(Path::new("Models/x"), &[folder("Editor")]));
    }
}
