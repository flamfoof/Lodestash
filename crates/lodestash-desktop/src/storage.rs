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
}
