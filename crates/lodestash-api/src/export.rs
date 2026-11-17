//! `export_asset` (ENGINE-API §2): copy chosen assets to an arbitrary **sandboxed** path, no
//! editor involved. Reads the catalog and writes files — it never mutates the catalog, and never
//! touches the originals.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The destination rule lives in the engine — its writer re-checks it before undoing an export —
/// and is re-exported here under the path the API has always used.
pub use lodestash_engine::sandbox::{check_dest, SandboxError};

/// What an export actually did (UI-SPEC §16.2 / ENGINE-API §2).
///
/// The two skip counts are kept apart because they are different problems with different fixes,
/// and one number covering both would tell the user nothing they can act on: an **in-pack** asset
/// needs its pack extracted first, an **unreachable** one needs its drive plugged back in.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExportReport {
    /// Files written into the destination.
    pub written: u64,
    /// Picks whose bytes are still inside a `.unitypackage`. Extraction is a separate, explicit
    /// action — an export never unpacks an archive behind the user's back.
    pub in_pack: usize,
    /// Picks whose file the catalog knows about but the disk does not: a source that went away,
    /// or a file moved out from under the index (UI-SPEC §11.1).
    pub unreachable: usize,
    /// How many extra files the dependency closure pulled in — the materials and textures the
    /// user did not name but would have missed.
    pub dependencies_added: usize,
    pub dest_path: String,
}

impl ExportReport {
    /// The job payload ENGINE-API §2 clients read. `missing` is kept beside the split halves for
    /// clients written before the split — dropping it would break them for a rename.
    pub fn to_json(&self, requested_dependencies: bool) -> serde_json::Value {
        serde_json::json!({
            "written": self.written,
            "missing": self.in_pack + self.unreachable,
            "in_pack": self.in_pack,
            "unreachable": self.unreachable,
            "dest_path": self.dest_path,
            "dependencies_included": requested_dependencies,
            "requested_dependencies": requested_dependencies,
            "dependencies_added": self.dependencies_added,
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportRequest {
    pub asset_ids: Vec<i64>,
    pub dest_path: String,
    #[serde(default)]
    pub with_dependencies: bool,
}

/// Copy `files` into `dest`, one per unit of progress. Returns the **names actually written**, in
/// order. Stops early (leaving what it already wrote) when the job is cancelled.
///
/// Names rather than a count, because the export history records what undo would have to delete
/// (FEATURES §7.6). A cancelled run writes fewer files than it was asked for, and a history built
/// from the request instead of the result would point undo at files it never created.
pub fn copy_files(
    files: &[(PathBuf, String)],
    dest: &Path,
    job: &crate::jobs::Job,
) -> std::io::Result<Vec<String>> {
    std::fs::create_dir_all(dest)?;
    job.set_total(files.len() as u64);
    let mut written = Vec::with_capacity(files.len());
    for (src, name) in files {
        if job.is_cancelled() {
            break;
        }
        // ponytail: name collisions overwrite. Exporting two same-named assets into one folder is
        // rare; add a numeric suffix if it stops being.
        std::fs::copy(src, dest.join(name))?;
        written.push(name.clone());
        job.advance();
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(copy_files(&files, &out, &job).unwrap().len(), 3);
        assert_eq!(job.progress(), 1.0);

        let cancelled = jobs.create("export", "3 files".into(), crate::jobs::JobState::Running);
        jobs.cancel(&cancelled.id);
        let out2 = dir.path().join("out2");
        assert!(
            copy_files(&files, &out2, &cancelled).unwrap().is_empty(),
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
                content_hash: None,
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

    /// **Export to folder** from the desktop (UI-SPEC §16.2): the person clicking *is* the
    /// approval, so there is no gate — but every other rule still holds.
    ///
    /// Mutation-checked on the four things that would make it dangerous rather than merely wrong:
    /// the sandbox must still refuse a destination inside a library source root (an export that
    /// writes into the user's own asset folders breaks the non-destructive invariant, and skipping
    /// the gate must not skip that); the originals must survive untouched; a pick that cannot be
    /// copied must be **counted and told apart** — in-pack needs an extract, unreachable needs the
    /// drive — because one merged "missing" number tells the user nothing they can act on; and a
    /// refusal must write nothing at all.
    #[test]
    fn export_now_skips_the_gate_but_not_the_sandbox() {
        use lodestash_core::catalog::{Catalog, CrawlRecord};

        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("lib");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join("here.png"), b"aaa").unwrap();
        // Catalogued, then gone from disk: the D7b case, from the export's point of view.
        std::fs::write(lib.join("gone.png"), b"bb").unwrap();

        let db = dir.path().join("catalog.db");
        {
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
                content_hash: None,
            };
            cat.record_crawl(src, None, &[rec("here.png"), rec("gone.png")])
                .unwrap();
        }
        std::fs::remove_file(lib.join("gone.png")).unwrap();

        let state =
            Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
        let ids: Vec<i64> = {
            let cat = state.catalog.lock().unwrap();
            let mut v = cat.asset_summaries(&[1, 2]).unwrap();
            v.sort_by(|a, b| a.name.cmp(&b.name));
            v.iter().map(|r| r.id).collect()
        };
        assert_eq!(ids.len(), 2, "fixture");

        // Refused: inside the library. The gate is gone; the sandbox is not.
        let refusal = state
            .export_now(super::ExportRequest {
                asset_ids: ids.clone(),
                dest_path: lib.join("out").to_string_lossy().into(),
                with_dependencies: false,
            })
            .expect_err("exporting into the library was allowed");
        assert!(
            refusal.contains("inside a library source root"),
            "the refusal does not say why: {refusal}"
        );
        assert!(
            !lib.join("out").exists(),
            "a refused export still created its destination"
        );

        // Allowed: outside every root. One file lands; the catalogued-but-absent one is reported
        // as unreachable rather than counted as written.
        let dest = dir.path().join("out");
        let report = state
            .export_now(super::ExportRequest {
                asset_ids: ids.clone(),
                dest_path: dest.to_string_lossy().into(),
                with_dependencies: false,
            })
            .expect("export");
        assert_eq!(report.written, 1, "expected exactly the file that exists");
        assert_eq!(
            report.unreachable, 1,
            "a catalogued file missing from disk must be reported, not silently dropped"
        );
        assert_eq!(report.in_pack, 0, "nothing here is in a pack");
        assert!(dest.join("here.png").exists());
        assert!(!dest.join("gone.png").exists());

        // The originals are untouched — an export copies, it never moves (standing invariant).
        assert_eq!(
            std::fs::read(lib.join("here.png")).unwrap(),
            b"aaa",
            "the original was modified or moved"
        );
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
                content_hash: None,
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
