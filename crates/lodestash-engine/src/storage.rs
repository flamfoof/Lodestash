//! Disk the app is holding: the extracted-pack **workspace** and the **preview cache**.
//!
//! Two stores with two policies (UI-SPEC §11.4). Previews are disposable and capped — everything
//! in there regenerates from the source asset. The workspace holds payloads pulled out of a
//! `.unitypackage`, which regenerate only while that archive is still on disk; a pack whose
//! archive has gone is holding the *only* copy, and nothing here may take it.
//!
//! **Both settings were inert until 2026-08-29.** `cache_extracted`, `preview_cache_cap_gb` and
//! `auto_purge_preview_cache` were read when loading config and written when saving it, and
//! consumed nowhere — so the workspace grew without bound (3.6 GB across 1,178 folders on the
//! reference machine, none of it referenced by the catalog) and the cap never capped anything.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// One reclaimable workspace directory: the content-addressed `<hash>/` folder and what it holds.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReclaimEntry {
    pub path: String,
    pub bytes: u64,
    pub files: usize,
}

/// What a reclaim would remove, computed without deleting anything (UI-SPEC §11.4 dry run).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ReclaimPlan {
    pub entries: Vec<ReclaimEntry>,
    pub bytes: u64,
    pub files: usize,
    /// Directories left alone because a live asset still resolves through them.
    pub kept_dirs: usize,
}

/// Every path segment any live asset resolves through.
///
/// Segments, not whole paths, because `workspace_rel` is written in **two formats**: the extract
/// path stores the absolute `workspace_path` verbatim, while `crawl_records_in_workspace` stores
/// a workspace-relative path (which is what the column's own doc comment promises). Both contain
/// the content-hash directory as a segment, and that is the unit the workspace is organised in,
/// so keying on segments is correct under either — and over-protective rather than under- if a
/// third format ever appears.
fn protected_segments(referenced: &HashSet<String>) -> HashSet<String> {
    referenced
        .iter()
        .flat_map(|rel| {
            rel.split(['/', '\\'])
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect()
}

/// Recursive size of a directory: `(bytes, files)`. Unreadable entries are skipped rather than
/// failing the plan — a locked file makes one folder unreclaimable, not the whole sweep.
fn dir_size(dir: &Path) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut files = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(meta) = entry.metadata() {
                bytes += meta.len();
                files += 1;
            }
        }
    }
    (bytes, files)
}

/// Which workspace folders no live asset resolves through any more.
///
/// The workspace is `<workspace>/<content hash>/<basename>`, so a top-level folder whose name
/// appears in no `workspace_rel` is unreachable: nothing in the catalog can name it, and nothing
/// ever will again — re-extraction writes the same hash but re-records the row, so a returning
/// pack is not harmed by this having run.
pub fn plan_reclaim(workspace: &Path, referenced: &HashSet<String>) -> ReclaimPlan {
    let protected = protected_segments(referenced);
    let mut plan = ReclaimPlan::default();
    let Ok(entries) = std::fs::read_dir(workspace) else {
        return plan;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if protected.contains(&name) {
            plan.kept_dirs += 1;
            continue;
        }
        let (bytes, files) = dir_size(&path);
        plan.bytes += bytes;
        plan.files += files;
        plan.entries.push(ReclaimEntry {
            path: path.to_string_lossy().into_owned(),
            bytes,
            files,
        });
    }
    // Biggest first: the list is read to answer "what is costing me the most".
    plan.entries.sort_by_key(|e| std::cmp::Reverse(e.bytes));
    plan
}

/// Write the plan beside the workspace before anything is deleted, so a reclaim is auditable
/// after the fact. Best-effort: a log that cannot be written must not stop the reclaim, but it is
/// reported, because "I deleted 3.6 GB and kept no record" is worth saying out loud.
pub fn write_reclaim_log(workspace: &Path, plan: &ReclaimPlan) -> Option<PathBuf> {
    let log = workspace.with_file_name("reclaimed.log");
    let mut out = format!(
        "# lodestash workspace reclaim — {}\n# {} folders, {} files, {} bytes\n",
        chrono_stamp(),
        plan.entries.len(),
        plan.files,
        plan.bytes
    );
    for e in &plan.entries {
        out.push_str(&format!("{}\t{}\t{}\n", e.bytes, e.files, e.path));
    }
    // Append: a second reclaim must not erase the record of the first.
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .ok()?;
    f.write_all(out.as_bytes()).ok()?;
    Some(log)
}

/// Seconds since the epoch, as a stamp for the log. Deliberately not a formatted date: the app
/// carries no date library, and an epoch second is unambiguous in a machine-read log.
fn chrono_stamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

/// Delete what the plan lists. Returns `(folders removed, bytes freed)` — recomputed from what
/// actually went, never from the plan, so a folder that could not be removed is not counted as
/// reclaimed.
pub fn execute_reclaim(plan: &ReclaimPlan) -> (usize, u64) {
    let mut removed = 0usize;
    let mut freed = 0u64;
    for e in &plan.entries {
        if std::fs::remove_dir_all(&e.path).is_ok() {
            removed += 1;
            freed += e.bytes;
        } else {
            tracing::warn!(path = %e.path, "reclaim: folder could not be removed");
        }
    }
    (removed, freed)
}

/// Outcome of a reclaim — recomputed from what actually went, plus where the record was written.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ReclaimOutcome {
    pub removed: usize,
    pub freed: u64,
    /// The append-only log listing every folder removed. `None` means it could not be written,
    /// which the caller says out loud: deleting gigabytes and keeping no record is worth knowing.
    pub log: Option<String>,
}

/// What a workspace reclaim would remove, without removing it (UI-SPEC §11.4) — the dry run.
///
/// Answered only by an engine that is **Ready** and whose catalog can say what is still
/// referenced. "Nothing is referenced" and "I could not find out" used to be the same empty set
/// (review finding 1): an engine with no catalog — a second Lodestash refused the lock — planned
/// to delete every folder in a workspace it shares with the one that holds it.
pub fn workspace_reclaim_plan(engine: &crate::Engine) -> Result<ReclaimPlan, String> {
    // Not Ready, or a snapshot that could not be rebuilt, is an error here — never a library
    // that happens to reference nothing.
    let library = engine
        .read()
        .map_err(|e| format!("{e} — nothing in the workspace was measured"))?;
    let workspace = engine.config().workspace_dir.clone();
    if workspace.as_os_str().is_empty() {
        return Err("no workspace folder is configured".into());
    }
    let referenced = library.referenced_workspace_paths()?;
    Ok(plan_reclaim(&workspace, &referenced))
}

/// Reclaim the workspace folders no live asset resolves through (UI-SPEC §11.4).
///
/// Runs **on the writer thread** ([`crate::writer::Op::ReclaimWorkspace`]), like every other
/// delete, for three reasons: an engine with no catalog has no writer, so it cannot run at all;
/// "what is referenced" is read through the writer's own connection in the same turn that
/// deletes, so no extraction can record a folder between the plan and the sweep; and a caller
/// that gave up waiting before the sweep started does not have its files deleted later.
pub fn reclaim_workspace(engine: &crate::Engine) -> Result<ReclaimOutcome, String> {
    let (outcome, done) = std::sync::mpsc::channel();
    engine
        .writer()
        .run(crate::writer::Op::ReclaimWorkspace { outcome })
        .map_err(|e| e.to_string())?;
    done.try_recv()
        .map_err(|_| "the reclaim finished without saying what it removed".to_string())
}

/// The sweep itself — [`reclaim_workspace`]'s half on the writer thread.
///
/// Refuses, deleting nothing, when the workspace is unset, when the referenced set cannot be
/// read, and while a pack is being analysed or extracted: an extraction places its files first
/// and records them when it finishes, so until then they are referenced by nothing — as is the
/// `.tmp` folder the rest is being written through.
pub(crate) fn reclaim_on_writer(
    catalog: &lodestash_core::catalog::Catalog,
    workspace: Option<&Path>,
    jobs: &crate::writer::JobRegistry,
) -> Result<ReclaimOutcome, String> {
    let workspace = workspace
        .filter(|w| !w.as_os_str().is_empty())
        .ok_or("no workspace folder is configured")?;
    if jobs.has_active_extract() {
        return Err(
            "a pack is being extracted — reclaim the workspace once it has finished".into(),
        );
    }
    let referenced = catalog.referenced_workspace_paths().map_err(|e| {
        format!("what the library still uses could not be read ({e}) — nothing was reclaimed")
    })?;
    let plan = plan_reclaim(workspace, &referenced);
    let log = write_reclaim_log(workspace, &plan);
    let (removed, freed) = execute_reclaim(&plan);
    tracing::info!(removed, freed, "workspace reclaimed");
    Ok(ReclaimOutcome {
        removed,
        freed,
        log: log.map(|p| p.to_string_lossy().into_owned()),
    })
}

/// Delete the mesh-cache files that are dead by **identity**, before any cap is consulted.
///
/// The cap cannot do this job. Three classes of file stop being useful the moment they do, and
/// gating their removal on cap pressure means they simply accumulate: the reference machine held
/// 359 MB against an 8 GB cap, so [`purge_previews_over_cap`] had never once fired, and 145 MB of
/// it was already garbage:
///
/// - **Stale-budget proxies** — `<key>-<tag>.lod` whose tag is not the current quality's. A
///   setting change forks the cache and nothing reaps the loser; the machine held 967 proxies at
///   Low's budget *and* 1,307 at Medium's.
/// - **Orphan staging geometry** — `<key>.msh` with no `<key>-*.lod` sibling. The metrics drain
///   parses every model into one to read the source triangle count, and
///   `mesh::load_preview_cached` deletes it when it decimates — but only if a preview is ever
///   generated for that same key. 600 were left, 123 MB, largest 48.6 MB.
/// - **Crash leftovers** — `*.part` from an interrupted temp-then-rename.
///
/// Deliberately **not** catalog-driven: every relation needed is in the filenames, so one
/// `read_dir` answers all three. A race with a drain mid-write costs one re-parse, which is the
/// same price a cache miss already pays.
///
/// Returns `(files removed, bytes freed)`.
pub fn sweep_dead_mesh_cache(thumbs: &Path, current_tag: &str) -> (usize, u64) {
    let mesh = thumbs.join("mesh");
    let Ok(entries) = std::fs::read_dir(&mesh) else {
        return (0, 0);
    };
    let mut names: Vec<(PathBuf, String, u64)> = Vec::new();
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        let name = e.file_name().to_string_lossy().into_owned();
        names.push((path, name, meta.len()));
    }
    // Which stems still have a proxy the current setting would serve. Built before anything is
    // deleted, so the `.msh` decision cannot depend on the order the directory happened to list.
    let live_stems: HashSet<&str> = names
        .iter()
        .filter_map(|(_, n, _)| n.strip_suffix(".lod"))
        .filter(|stem| stem.ends_with(current_tag))
        .filter_map(|stem| stem.rsplit_once('-').map(|(key, _)| key))
        .collect();

    let mut removed = 0usize;
    let mut freed = 0u64;
    for (path, name, len) in &names {
        let dead = if let Some(stem) = name.strip_suffix(".lod") {
            !stem.ends_with(current_tag)
        } else if let Some(key) = name.strip_suffix(".msh") {
            !live_stems.contains(key)
        } else {
            name.ends_with(".part")
        };
        if dead && std::fs::remove_file(path).is_ok() {
            removed += 1;
            freed += len;
        }
    }
    if removed > 0 {
        tracing::info!(removed, freed, current_tag, "dead mesh-cache files swept");
    }
    (removed, freed)
}

/// Trim the preview cache to `cap_bytes`, oldest first (UI-SPEC §11.4 "Auto-purge over cap").
///
/// Oldest by mtime rather than true least-recently-*viewed*: a read does not touch mtime, so the
/// cache records no access times and inventing them would mean writing on every preview read.
/// For a regenerable cache the two orderings differ only in which cheap-to-rebuild file goes
/// first. Returns `(files removed, bytes freed)`; a cache already under the cap is untouched.
pub fn purge_previews_over_cap(thumbs: &Path, cap_bytes: u64) -> (usize, u64) {
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![thumbs.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let Ok(meta) = entry.metadata() else { continue };
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            total += meta.len();
            files.push((path, meta.len(), mtime));
        }
    }
    if total <= cap_bytes {
        return (0, 0);
    }
    files.sort_by_key(|(_, _, mtime)| *mtime);
    let mut removed = 0usize;
    let mut freed = 0u64;
    for (path, len, _) in files {
        if total - freed <= cap_bytes {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            removed += 1;
            freed += len;
        }
    }
    if removed > 0 {
        tracing::info!(removed, freed, cap_bytes, "preview cache trimmed to cap");
    }
    (removed, freed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: usize) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![b'x'; bytes]).unwrap();
    }

    /// The sweep's whole job: take what nothing references, leave what something does. Keying on
    /// the hash **segment** is what makes it correct for both `workspace_rel` formats in the
    /// catalog — an absolute path from the extract path, a relative one from the crawl.
    #[test]
    fn reclaim_takes_only_unreferenced_folders() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        touch(&ws.join("aaa").join("Rock.fbx"), 1000);
        touch(&ws.join("bbb").join("Tree.fbx"), 2000);
        touch(&ws.join("ccc").join("Sky.png"), 4000);

        let referenced: HashSet<String> = [
            // relative form (crawl)
            "aaa/Rock.fbx".to_string(),
            // absolute form (extract) — same folder, different spelling
            ws.join("bbb")
                .join("Tree.fbx")
                .to_string_lossy()
                .into_owned(),
        ]
        .into_iter()
        .collect();

        let plan = plan_reclaim(ws, &referenced);
        assert_eq!(plan.kept_dirs, 2, "both spellings protect their folder");
        assert_eq!(plan.entries.len(), 1, "only ccc is unreachable: {plan:?}");
        assert!(plan.entries[0].path.ends_with("ccc"));
        assert_eq!(plan.bytes, 4000);
        assert_eq!(plan.files, 1);

        let (removed, freed) = execute_reclaim(&plan);
        assert_eq!((removed, freed), (1, 4000));
        assert!(ws.join("aaa").exists(), "referenced folder survived");
        assert!(ws.join("bbb").exists(), "referenced folder survived");
        assert!(!ws.join("ccc").exists(), "unreferenced folder went");
    }

    #[test]
    fn nothing_referenced_means_everything_is_reclaimable() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("aaa").join("a.fbx"), 10);
        touch(&dir.path().join("bbb").join("b.fbx"), 20);
        let plan = plan_reclaim(dir.path(), &HashSet::new());
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.bytes, 30);
        // Biggest first — the list answers "what is costing me the most".
        assert!(plan.entries[0].bytes >= plan.entries[1].bytes);
    }

    #[test]
    fn a_missing_workspace_is_not_an_error() {
        let plan = plan_reclaim(Path::new("G:/no/such/workspace"), &HashSet::new());
        assert_eq!(plan.entries.len(), 0);
        assert_eq!(plan.bytes, 0);
    }

    #[test]
    fn purge_trims_oldest_until_under_cap() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old.png");
        let mid = dir.path().join("mid.png");
        let new = dir.path().join("new.png");
        // Written in order with a gap, so the mtimes genuinely differ. Setting them directly
        // would need a file-time crate, and this cache's whole ordering question is worth less
        // than a dependency (TECH-STACK §1 keeps the list short on purpose).
        for p in [&old, &mid, &new] {
            touch(p, 400);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let (removed, freed) = purge_previews_over_cap(dir.path(), 900);
        assert_eq!(
            (removed, freed),
            (1, 400),
            "one file takes it under the cap"
        );
        assert!(!old.exists(), "the oldest went first");
        assert!(mid.exists() && new.exists());

        assert_eq!(
            purge_previews_over_cap(dir.path(), 900),
            (0, 0),
            "already under the cap — nothing to do"
        );
    }

    /// The sweep decides by identity, not by pressure — the cap never reaches these.
    ///
    /// Reproduces the three classes found on the reference machine: proxies left behind by a
    /// quality change, staging meshes for models that were parsed for their triangle count but
    /// never thumbnailed, and `.part` leftovers from an interrupted write.
    #[test]
    fn the_sweep_takes_dead_mesh_files_and_leaves_the_live_ones() {
        let dir = tempfile::tempdir().unwrap();
        let mesh = dir.path().join("mesh");
        std::fs::create_dir_all(&mesh).unwrap();
        let now = "r50f600c4000";
        let then = "r20f300c1500";

        // Two models: `keepme` has a current proxy, `stale` only an old one.
        touch(&mesh.join(format!("keepme-{now}.lod")), 10);
        touch(&mesh.join(format!("keepme-{then}.lod")), 20);
        touch(&mesh.join("keepme.msh"), 40);
        touch(&mesh.join(format!("stale-{then}.lod")), 80);
        touch(&mesh.join("stale.msh"), 160);
        touch(&mesh.join("orphan.msh"), 320);
        touch(&mesh.join("half.lod.1234-0.part"), 640);
        // A thumbnail outside `mesh/` is not this sweep's business at any size.
        touch(&dir.path().join("a_128x128_x.png"), 1280);

        let (removed, freed) = sweep_dead_mesh_cache(dir.path(), now);

        let gone = |n: &str| !mesh.join(n).exists();
        assert!(
            gone(&format!("keepme-{then}.lod")),
            "a stale-budget proxy goes"
        );
        assert!(
            gone(&format!("stale-{then}.lod")),
            "even when it is the only proxy"
        );
        assert!(
            gone("orphan.msh"),
            "staging geometry with no proxy at all goes"
        );
        assert!(
            gone("stale.msh"),
            "its proxy was stale, so the staging mesh is not protected by it either"
        );
        assert!(gone("half.lod.1234-0.part"), "a crash leftover goes");

        assert!(
            mesh.join(format!("keepme-{now}.lod")).exists(),
            "the current proxy stays"
        );
        assert!(
            mesh.join("keepme.msh").exists(),
            "staging geometry backing a current proxy stays — the metrics facet reads it"
        );
        assert!(
            dir.path().join("a_128x128_x.png").exists(),
            "thumbnails are the cap's job"
        );
        assert_eq!((removed, freed), (5, 20 + 80 + 160 + 320 + 640));
    }
}
