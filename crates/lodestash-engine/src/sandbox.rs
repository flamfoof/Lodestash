//! The export sandbox (ENGINE-API §2 `export_asset`, FEATURES §7.1): where the app may write
//! copies of a user's assets, and therefore where undoing an export may delete them.
//!
//! One rule, two callers: the API checks a destination before an export job exists, and the
//! writer re-checks it before undo deletes anything (the destination may since have become a
//! library source). It lives in the engine so the writer does not depend on the HTTP crate
//! (v2 spec §3); `lodestash_api::export` re-exports it.

use std::path::{Component, Path, PathBuf};

/// Why a destination was refused. These map to ENGINE-API §4 error codes.
#[derive(Debug, PartialEq, Eq)]
pub enum SandboxError {
    /// A relative path, or one containing `..`.
    Traversal,
    /// Inside a library source root — exporting there would write into the user's own asset
    /// folders, which the non-destructive invariant forbids.
    InsideLibrary,
    /// The source roots could not be read, so "inside a library source root" cannot be answered.
    /// Refused: [`check_dest`] with no roots admits every folder, the library's own included.
    RootsUnknown,
}

impl SandboxError {
    pub fn code(&self) -> &'static str {
        match self {
            SandboxError::Traversal => "path_traversal",
            SandboxError::InsideLibrary => "forbidden_destination",
            SandboxError::RootsUnknown => lodestash_protocol::codes::UNAVAILABLE,
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            SandboxError::Traversal => "dest_path must be absolute and free of '..'.",
            SandboxError::InsideLibrary => {
                "dest_path is inside a library source root; exports must go elsewhere."
            }
            SandboxError::RootsUnknown => {
                "the library's source roots could not be read, so the destination cannot be \
                 checked against them; nothing was exported."
            }
        }
    }
}

/// [`check_dest`] against roots that may not have been readable — what a caller holding a
/// `Result` from the catalog uses, so an unreadable root list can never become an empty one.
pub fn check_dest_against<E: std::fmt::Display>(
    dest: &str,
    source_roots: Result<Vec<String>, E>,
) -> Result<PathBuf, SandboxError> {
    let roots = source_roots.map_err(|e| {
        tracing::warn!(%e, "export sandbox: the source roots could not be read; refusing");
        SandboxError::RootsUnknown
    })?;
    check_dest(dest, &roots)
}

/// Validate a destination *before* any job exists. Lexical only — the path need not exist yet,
/// and we refuse `..` outright rather than trying to resolve it.
pub fn check_dest(dest: &str, source_roots: &[String]) -> Result<PathBuf, SandboxError> {
    let path = Path::new(dest);
    if !path.is_absolute() || path.components().any(|c| c == Component::ParentDir) {
        return Err(SandboxError::Traversal);
    }
    let normal = normalize(path);
    for root in source_roots {
        let root = normalize(Path::new(root));
        let root = root.trim_end_matches('/');
        // Compare on path boundaries: `C:/library_exports` is not inside `C:/lib`.
        if normal == root || normal.starts_with(&format!("{root}/")) {
            return Err(SandboxError::InsideLibrary);
        }
    }
    Ok(path.to_path_buf())
}

/// Lowercased, forward-slashed form for prefix comparison. Windows paths are case-insensitive and
/// arrive with either separator, so `G:\Lib` and `g:/lib/sub` must compare as the same tree.
fn normalize(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sandbox has exactly two jobs: no traversal, and never inside the library.
    #[test]
    fn dest_sandbox_admits_only_absolute_paths_outside_every_source_root() {
        let roots = vec!["G:/Game_Assets/Synty".to_string(), "C:\\lib".to_string()];

        assert!(check_dest("G:/exports/rocks", &roots).is_ok());
        assert!(check_dest("D:/anywhere", &roots).is_ok());

        assert_eq!(
            check_dest("exports/rocks", &roots).unwrap_err(),
            SandboxError::Traversal,
            "relative"
        );
        assert_eq!(
            check_dest("G:/exports/../../Windows", &roots).unwrap_err(),
            SandboxError::Traversal,
            "climbing out"
        );
        assert_eq!(
            check_dest("G:/Game_Assets/Synty/Models", &roots).unwrap_err(),
            SandboxError::InsideLibrary,
            "under a source root"
        );
        assert_eq!(
            check_dest("g:\\game_assets\\synty", &roots).unwrap_err(),
            SandboxError::InsideLibrary,
            "same root, other case and separator"
        );
        assert_eq!(
            check_dest("c:/lib/sub", &roots).unwrap_err(),
            SandboxError::InsideLibrary
        );
        // A sibling directory whose name merely starts with a root's name is fine.
        assert!(check_dest("C:/library_exports", &roots).is_ok());
    }
}
