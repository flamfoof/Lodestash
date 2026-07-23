//! `export_asset` (ENGINE-API §2): copy chosen assets to an arbitrary **sandboxed** path, no
//! editor involved. Reads the catalog and writes files — it never mutates the catalog, and never
//! touches the originals.

use std::path::{Component, Path, PathBuf};

use serde::Deserialize;

/// Why a destination was refused. These map to ENGINE-API §4 error codes.
#[derive(Debug, PartialEq, Eq)]
pub enum SandboxError {
    /// A relative path, or one containing `..`.
    Traversal,
    /// Inside a library source root — exporting there would write into the user's own asset
    /// folders, which the non-destructive invariant forbids.
    InsideLibrary,
}

impl SandboxError {
    pub fn code(&self) -> &'static str {
        match self {
            SandboxError::Traversal => "path_traversal",
            SandboxError::InsideLibrary => "forbidden_destination",
        }
    }

    pub fn message(&self) -> &'static str {
        match self {
            SandboxError::Traversal => "dest_path must be absolute and free of '..'.",
            SandboxError::InsideLibrary => {
                "dest_path is inside a library source root; exports must go elsewhere."
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportRequest {
    pub asset_ids: Vec<i64>,
    pub dest_path: String,
    #[serde(default)]
    pub with_dependencies: bool,
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

/// Copy `files` into `dest`, one per unit of progress. Returns the number written. Stops early
/// (leaving what it already wrote) when the job is cancelled.
pub fn copy_files(
    files: &[(PathBuf, String)],
    dest: &Path,
    job: &crate::jobs::Job,
) -> std::io::Result<u64> {
    std::fs::create_dir_all(dest)?;
    job.set_total(files.len() as u64);
    let mut written = 0u64;
    for (src, name) in files {
        if job.is_cancelled() {
            break;
        }
        // ponytail: name collisions overwrite. Exporting two same-named assets into one folder is
        // rare; add a numeric suffix if it stops being.
        std::fs::copy(src, dest.join(name))?;
        written += 1;
        job.advance();
    }
    Ok(written)
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

    #[test]
    fn copy_stops_when_the_job_is_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        let files: Vec<(PathBuf, String)> = (0..3)
            .map(|i| {
                let p = src_dir.join(format!("f{i}.bin"));
                std::fs::write(&p, b"x").unwrap();
                (p, format!("f{i}.bin"))
            })
            .collect();

        let jobs = crate::jobs::Jobs::default();
        let job = jobs.create("export", "3 files".into(), crate::jobs::JobState::Running);
        let out = dir.path().join("out");
        assert_eq!(copy_files(&files, &out, &job).unwrap(), 3);
        assert_eq!(job.progress(), 1.0);

        let cancelled = jobs.create("export", "3 files".into(), crate::jobs::JobState::Running);
        jobs.cancel(&cancelled.id);
        let out2 = dir.path().join("out2");
        assert_eq!(
            copy_files(&files, &out2, &cancelled).unwrap(),
            0,
            "a cancelled job writes nothing further"
        );
    }
}

#[cfg(test)]
mod flow_tests {
    use std::sync::Arc;

    use crate::{jobs::JobState, ServerState};

    /// `with_dependencies` copies the closure, not just the named asset: exporting a material
    /// brings its textures (DATA-MODEL §10 → ENGINE-API §4).
    #[test]
    fn export_with_dependencies_copies_the_closure() {
        use lodestash_core::catalog::{Catalog, CrawlRecord};

        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        for f in ["rock.mat", "rock_albedo.png", "unrelated.png"] {
            std::fs::write(lib.join(f), b"x").unwrap();
        }
        let db = dir.path().join("catalog.db");
        let (mat_id, tex_id) = {
            let mut cat = Catalog::open(&db).unwrap();
            cat.seed_media_types().unwrap();
            let src = cat
                .upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
                .unwrap();
            let rec = |rel: &str, mt: &str| CrawlRecord {
                workspace_rel: None,
                rel_path: rel.into(),
                name: rel.into(),
                media_type: mt.into(),
                size_bytes: 1,
                mtime: 1,
            };
            cat.record_crawl(
                src,
                None,
                &[
                    rec("rock.mat", "material"),
                    rec("rock_albedo.png", "texture"),
                    rec("unrelated.png", "texture"),
                ],
            )
            .unwrap();
            let id = |name: &str| -> i64 {
                cat.assets_for_source(src)
                    .unwrap()
                    .into_iter()
                    .find(|a| a.rel_path == name)
                    .unwrap()
                    .asset_id
            };
            let (mat, tex) = (id("rock.mat"), id("rock_albedo.png"));
            cat.record_dependencies(tex, Some("texguid"), "uses_texture", &[])
                .unwrap();
            cat.record_dependencies(mat, Some("matguid"), "uses_texture", &["texguid".into()])
                .unwrap();
            (mat, tex)
        };

        let state =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        assert_eq!(
            state.dependencies(mat_id).unwrap(),
            vec![tex_id],
            "the detail view names exactly what an export would add"
        );

        let dest = dir.path().join("out");
        let job = state
            .request_export(super::ExportRequest {
                asset_ids: vec![mat_id],
                dest_path: dest.to_string_lossy().into(),
                with_dependencies: true,
            })
            .unwrap();
        state.approve_job(&job.id);
        for _ in 0..200 {
            if job.state().is_terminal() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(job.state(), JobState::Done);
        assert!(dest.join("rock.mat").exists(), "the named asset");
        assert!(
            dest.join("rock_albedo.png").exists(),
            "its texture rode along"
        );
        assert!(
            !dest.join("unrelated.png").exists(),
            "closure, not the whole library"
        );
        let result = job.to_json();
        assert_eq!(result["result"]["dependencies_included"], true);
        assert_eq!(result["result"]["dependencies_added"], 1);
    }

    /// The whole gate: an export request writes nothing until it is approved, and denial is
    /// final. This is the invariant the desktop prompt exists to enforce.
    #[test]
    fn nothing_is_written_until_the_user_approves() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("a.png"), b"aaa").unwrap();
        std::fs::write(lib.join("b.png"), b"bb").unwrap();

        let db = dir.path().join("catalog.db");
        {
            use lodestash_core::catalog::{Catalog, CrawlRecord};
            let mut cat = Catalog::open(&db).unwrap();
            cat.seed_media_types().unwrap();
            let src = cat
                .upsert_import_source("Lib", &lib.to_string_lossy(), "folder")
                .unwrap();
            let rec = |rel: &str| CrawlRecord {
                workspace_rel: None,
                rel_path: rel.into(),
                name: rel.into(),
                media_type: "image".into(),
                size_bytes: 3,
                mtime: 1,
            };
            cat.record_crawl(src, None, &[rec("a.png"), rec("b.png")])
                .unwrap();
        }
        let state =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        let ids = state.catalog.lock().unwrap().recent_asset_ids(10).unwrap();
        let dest = dir.path().join("out");

        let job = state
            .request_export(super::ExportRequest {
                asset_ids: ids.clone(),
                dest_path: dest.to_string_lossy().into(),
                with_dependencies: false,
            })
            .expect("dest is outside the library");
        assert_eq!(job.state(), JobState::AwaitingApproval);
        assert!(!dest.exists(), "an unapproved export writes nothing");

        state.approve_job(&job.id);
        for _ in 0..200 {
            if job.state().is_terminal() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        assert_eq!(job.state(), JobState::Done, "approved export runs");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 2);
        assert_eq!(std::fs::read(dest.join("a.png")).unwrap(), b"aaa");
        // The originals are untouched (non-destructive invariant).
        assert!(lib.join("a.png").exists() && lib.join("b.png").exists());

        // Denial is final: the job is cancelled and re-approving does nothing.
        let denied = state
            .request_export(super::ExportRequest {
                asset_ids: ids,
                dest_path: dir.path().join("out2").to_string_lossy().into(),
                with_dependencies: false,
            })
            .unwrap();
        state.deny_job(&denied.id);
        assert_eq!(denied.state(), JobState::Cancelled);
        state.approve_job(&denied.id);
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(
            !dir.path().join("out2").exists(),
            "a denied export stays denied"
        );
    }
}
