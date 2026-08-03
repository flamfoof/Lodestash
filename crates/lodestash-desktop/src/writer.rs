//! The catalog write path: **one dedicated writer thread**, fed by a channel.
//!
//! `ARCHITECTURE.md` §5 is explicit — "all writes funnel through one dedicated writer (a
//! thread/actor owning the connection, fed by a channel); reads use short-lived per-task
//! connections". This is that thread. It is not an optimisation: `rusqlite::Connection` is
//! `!Sync`, Tauri runs every `#[tauri::command]` on a thread pool, and two commands writing at
//! once is exactly how `database is locked` came back last time (`HANDOFF.md` §6).
//!
//! Commands send a [`Job`] and block on its reply. Blocking is correct here: these are
//! single-row updates that take microseconds, the caller is a pool thread rather than the UI, and
//! the frontend needs the outcome before it can honestly redraw. A fire-and-forget write would
//! report success for something that had not happened yet.
//!
//! **The reads stay where they are.** `Library` keeps its own read-only connection (WAL makes a
//! reader and a writer concurrent), so browsing never waits behind a write.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};

use lodestash_core::catalog::Catalog;
use lodestash_core::types::{Exclude, ExcludeKind};

/// One asset the write path needs to act on, resolved from the library while its lock is held so
/// the writer thread never reaches back into it.
#[derive(Debug, Clone)]
pub struct Target {
    pub asset_id: i64,
    /// Absolute path. **Authoritative** — never recomposed from `source_root + rel_path`, which is
    /// wrong for workspace-extracted assets and can name a real, unrelated file (`app.rs:2003`).
    pub abs_path: Option<PathBuf>,
    /// Source root + the path relative to it, for the blacklist entry Remove writes.
    pub source_root: Option<PathBuf>,
    pub rel_path: String,
}

/// What the writer was asked to do. One variant per user-facing action, so the thread — not the
/// caller — owns the order of operations for anything that touches more than one thing.
pub enum Op {
    SetFavorite { ids: Vec<i64>, on: bool },
    /// Soft delete / restore. Catalog-only; the file on disk is never touched (DATA-MODEL §8).
    SetTrashed { ids: Vec<i64>, trashed: bool },
    /// **Remove from library** — the safe removal. Soft-deletes the row *and* blacklists the
    /// asset in its source, so the next scan does not bring it back. No file is touched.
    RemoveFromLibrary { targets: Vec<Target>, project: Option<String> },
    /// **Delete from disk** — the one irreversible action in this product.
    DeleteFromDisk { targets: Vec<Target>, roots: Vec<PathBuf>, workspace: Option<PathBuf> },
}

/// What a write did, in the terms the frontend reports to the user.
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct WriteResult {
    /// Rows (or files) the operation actually changed.
    pub changed: usize,
    /// Files refused by the delete fence, or paths that no longer exist.
    pub refused: usize,
    /// Files that were meant to go but could not (permissions, in use).
    pub failed: usize,
}

pub struct Job {
    pub op: Op,
    pub reply: Sender<Result<WriteResult, String>>,
}

/// Handle to the writer thread. Cloneable, `Send`, holds no connection of its own.
#[derive(Clone)]
pub struct Writer {
    tx: Sender<Job>,
}

impl Writer {
    /// Spawn the writer over `db_path`. The connection is opened **on the thread that owns it**,
    /// never handed across, because that is the whole point of the rule.
    pub fn spawn(db_path: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("lodestash-writer".into())
            .spawn(move || {
                // Opened once and held for the process's life: reopening per job would be a new
                // writer each time, which is the thing the rule forbids.
                let catalog = match Catalog::open(&db_path) {
                    Ok(c) => c,
                    Err(e) => {
                        tracing::error!(%e, "writer: catalog open failed; writes will be refused");
                        // Drain and refuse rather than hanging every caller forever.
                        for job in rx {
                            let _ = job.reply.send(Err(format!("catalog unavailable: {e}")));
                        }
                        return;
                    }
                };
                for job in rx {
                    let _ = job.reply.send(run(&catalog, job.op));
                }
                tracing::debug!("writer: channel closed, thread exiting");
            })
            .expect("writer thread spawns");
        Self { tx }
    }

    /// Run one op and wait for it. `Err` carries a message the frontend can show verbatim.
    pub fn run(&self, op: Op) -> Result<WriteResult, String> {
        let (reply, wait) = mpsc::channel();
        self.tx
            .send(Job { op, reply })
            .map_err(|_| "writer thread is gone".to_string())?;
        wait.recv()
            .map_err(|_| "writer thread dropped the job".to_string())?
    }
}

fn run(catalog: &Catalog, op: Op) -> Result<WriteResult, String> {
    match op {
        Op::SetFavorite { ids, on } => catalog
            .set_favorite(&ids, on)
            .map(|changed| WriteResult { changed, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::SetTrashed { ids, trashed } => catalog
            .set_trashed(&ids, trashed)
            .map(|changed| WriteResult { changed, ..Default::default() })
            .map_err(|e| e.to_string()),

        Op::RemoveFromLibrary { targets, project } => remove_from_library(catalog, targets, project),

        Op::DeleteFromDisk { targets, roots, workspace } => {
            delete_from_disk(catalog, targets, &roots, workspace.as_deref())
        }
    }
}

/// Soft-delete the rows **and** blacklist each asset in its source.
///
/// The blacklist is what makes this different from Trash: `crawler::crawl` takes the per-source
/// `Exclude` list, so an excluded asset is never re-offered by a scan. Without it a "removed"
/// asset reappears the next time the user indexes that folder, which reads as the app ignoring
/// them.
///
/// The exclude is written to the **app config**, not `import_source.blacklist`: the config is
/// what `crawl` is actually handed today (`app.rs:812`), and the catalog column is M5 storage
/// that nothing reads yet. Writing only the column would look right and change nothing.
fn remove_from_library(
    catalog: &Catalog,
    targets: Vec<Target>,
    project: Option<String>,
) -> Result<WriteResult, String> {
    let ids: Vec<i64> = targets.iter().map(|t| t.asset_id).collect();
    let changed = catalog.set_trashed(&ids, true).map_err(|e| e.to_string())?;

    let Some(project) = project else {
        // No project open: the row is hidden, but nothing owns a blacklist to write to. Say so
        // rather than reporting a removal that a scan will undo.
        return Err("no project is open, so the asset cannot be excluded from future scans".into());
    };
    let mut config = lodestash_app::config::AppConfig::load();
    let mut excluded = 0;
    for t in &targets {
        let Some(root) = t.source_root.as_deref() else {
            continue; // in-pack asset: no source-relative file for the crawler to skip
        };
        // `Asset` matches the full source-relative path or the bare file name
        // (`Exclude::matches`), so this excludes exactly the file the user removed.
        let exclude = Exclude {
            kind: ExcludeKind::Asset,
            value: t.rel_path.clone(),
        };
        if config.add_source_exclude(&project, root, exclude) {
            excluded += 1;
        }
    }
    tracing::info!(rows = changed, excluded, "removed from library");
    Ok(WriteResult { changed, ..Default::default() })
}

/// May this target's file be deleted?
///
/// `false` for an **in-pack** asset — it has no file of its own, and its `abs_path` would be the
/// `.unitypackage` holding hundreds of other assets — and `false` for anything outside every
/// source root the user opened (or the workspace this app owns).
fn deletable(t: &Target, roots: &[PathBuf], workspace: Option<&Path>) -> bool {
    let Some(path) = t.abs_path.as_deref() else {
        return false;
    };
    if !path.is_file() || !lodestash_app::thumbs::under_known_root(path, roots, workspace) {
        tracing::warn!(path = %path.display(), "refused to delete: outside every known root");
        return false;
    }
    true
}

/// Delete files from disk, then record only what actually left.
///
/// Two guards, in this order:
/// 1. `abs_path` is used as-is. Recomposing `source_root + rel_path` is wrong for extracted assets
///    and can name an unrelated real file — that is the primary protection.
/// 2. [`under_known_root`] bounds the blast radius if (1) is ever got wrong. It is fenced by
///    `only_paths_under_known_roots_may_be_deleted`, which is mutation-checked and must stay that way.
///
/// Trashing happens **after** the delete and only for files that succeeded: doing it first lets a
/// failed delete leave the catalog claiming a file is gone while it sits on disk.
fn delete_from_disk(
    catalog: &Catalog,
    targets: Vec<Target>,
    roots: &[PathBuf],
    workspace: Option<&Path>,
) -> Result<WriteResult, String> {
    let mut out = WriteResult::default();
    let mut deleted = Vec::new();
    for t in &targets {
        if !deletable(t, roots, workspace) {
            out.refused += 1;
            continue;
        }
        let path = t.abs_path.as_deref().expect("deletable implies a path");
        match std::fs::remove_file(path) {
            Ok(()) => {
                tracing::info!(path = %path.display(), "deleted file from disk");
                deleted.push(t.asset_id);
            }
            Err(e) => {
                tracing::warn!(path = %path.display(), %e, "delete from disk failed");
                out.failed += 1;
            }
        }
    }
    if !deleted.is_empty() {
        out.changed = catalog
            .set_trashed(&deleted, true)
            .map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(path: Option<&str>) -> Target {
        Target {
            asset_id: 1,
            abs_path: path.map(PathBuf::from),
            source_root: None,
            rel_path: "Assets/Rock.fbx".into(),
        }
    }

    /// The blast-radius guard, exercised through the code path a command actually takes rather
    /// than through `under_known_root` alone. A file outside every source root — and an in-pack
    /// asset, whose "path" is the archive holding hundreds of other assets — must be refused
    /// before `remove_file` is ever reached.
    ///
    /// `catalog` is `None` here on purpose: nothing should get far enough to need it.
    #[test]
    fn delete_refuses_paths_outside_every_known_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("library");
        std::fs::create_dir_all(&root).expect("mkdir");
        let outside = dir.path().join("elsewhere.fbx");
        std::fs::write(&outside, b"not yours").expect("write");

        let roots = vec![root];
        assert!(
            !deletable(&target(outside.to_str()), &roots, None),
            "a file outside every source root must be refused"
        );
        assert!(
            !deletable(&target(None), &roots, None),
            "an in-pack asset has no file of its own — deleting its archive would take every \
             other asset in that pack with it"
        );
        assert!(outside.exists(), "the file outside the root is still there");
    }

    /// The other half: a file that *is* under a known root passes the fence. Without this the
    /// test above would still pass if the fence refused everything.
    #[test]
    fn delete_allows_a_file_under_a_source_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("library");
        std::fs::create_dir_all(&root).expect("mkdir");
        let inside = root.join("Rock.fbx");
        std::fs::write(&inside, b"mine").expect("write");

        assert!(
            deletable(&target(inside.to_str()), std::slice::from_ref(&root), None),
            "a file under a source root must pass the fence"
        );
    }
}
