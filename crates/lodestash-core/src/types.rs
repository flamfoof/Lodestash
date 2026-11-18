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
    /// A single asset — matched by exact relative path or by file name. A value that starts with
    /// `/` is **anchored** at the source root and matches that one path only.
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
    /// The `Asset` value that names exactly one file: its source-relative path behind a leading
    /// `/` (the `.gitignore` convention). What **Remove from library** writes — a bare `rock.png`
    /// would also match every `rock.png` in every folder of the source.
    pub fn anchored(rel_path: &str) -> String {
        format!("/{}", rel_path.replace('\\', "/").trim_matches('/'))
    }

    /// Does this exclusion match a source-relative path?
    /// - `Folder`: `rel` equals the value or lies under it (any path prefix of components).
    /// - `Asset`: `rel` equals the value, or — unless the value is anchored with a leading `/` —
    ///   its file name equals the value.
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
                let anchored = self.value.trim_start().starts_with(['/', '\\']);
                rel_s == target
                    || (!anchored
                        && rel
                            .file_name()
                            .map(|n| norm(&n.to_string_lossy()) == target)
                            .unwrap_or(false))
            }
        }
    }
}

/// Is `path` a plain relative path — one or more ordinary components and nothing else?
///
/// The fence for any stored path that is later joined onto a folder and **deleted** (an import
/// record's `dest_path`, DATA-MODEL §9). Refused: an empty path, a root (`/x`, `\x`, `\\srv\x`), a
/// drive or stream (`C:x`, `a:b`), and any `.`/`..`/empty component. Judged on the text with both
/// separators, because `Path::is_absolute` is false on Windows for `\x` and `C:x` while
/// `Path::join` still lets either one replace the base it is joined onto.
pub fn is_plain_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains([':', '\0'])
        && path
            .split(['/', '\\'])
            .all(|part| !part.is_empty() && part != "." && part != "..")
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

    /// What **Remove from library** writes for a file at the source root. A bare `drop.png`
    /// matches by file name, so removing one root-level file used to exclude every `drop.png` in
    /// the source; the anchored form names that one path.
    #[test]
    fn an_anchored_asset_exclude_matches_only_that_path() {
        let e = asset(&Exclude::anchored("drop.png"));
        assert!(e.matches(Path::new("drop.png")), "the file itself");
        assert!(e.matches(Path::new("DROP.PNG")), "still case-insensitive");
        assert!(
            !e.matches(Path::new("Models/drop.png")),
            "a same-named file in a folder is a different file"
        );
        assert!(!e.matches(Path::new("a/b/drop.png")));

        let nested = asset(&Exclude::anchored("Models\\chair.fbx"));
        assert!(nested.matches(Path::new("Models/chair.fbx")));
        assert!(!nested.matches(Path::new("Other/Models/chair.fbx")));
        assert!(!nested.matches(Path::new("chair.fbx")));
        // Anchoring is idempotent, so a value read back and re-anchored is the same entry.
        assert_eq!(
            Exclude::anchored(&Exclude::anchored("drop.png")),
            "/drop.png"
        );
    }

    /// The fence `undo_import_batch` and the import ack share (review finding 3): a path that
    /// can leave — or replace — the folder it is joined onto is not project-relative. On Windows
    /// `Path::is_absolute` is false for `\Users\x` and `C:f`, and `Path::join` then discards the
    /// base, so the judgement is made on the text, the same on every OS.
    #[test]
    fn a_plain_relative_path_has_no_root_prefix_or_parent() {
        for ok in [
            "Assets/Wildlife/Beaver.fbx",
            "Assets\\Wildlife\\Beaver.fbx",
            "a",
        ] {
            assert!(is_plain_relative_path(ok), "{ok:?} is project-relative");
        }
        for bad in [
            "",
            "/etc/passwd",
            "\\Users\\x\\file.txt",
            "C:file.txt",
            "C:\\Users\\x",
            "c:/users/x",
            "\\\\server\\share\\x",
            "//server/share/x",
            "..",
            "../x",
            "Assets/../../x",
            "Assets\\..\\..\\x",
            "./Assets/x",
            "Assets/./x",
            "Assets//x",
            "Assets/x/",
            "Assets/x:stream",
            "Assets/x\0y",
        ] {
            assert!(!is_plain_relative_path(bad), "{bad:?} must be refused");
        }
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
