//! Filesystem watch over the library's source roots (DATA-MODEL §6 "Watch", INGESTION §2).
//!
//! `notify` fires per-event and fires *a lot* — a pack landing in a cache root is hundreds of
//! events. This wraps it in a debounce: after the first change, wait until the tree has been
//! quiet for `quiet`, then emit one signal. Callers treat that signal as "something changed,
//! rescan when convenient", never as a description of what changed — watchers drop events under
//! bursts, which is exactly why the spec pairs them with a full reconcile.

use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// A live watch over a set of roots. Dropping it stops watching.
pub struct RootWatcher {
    // Held for its lifetime — dropping the watcher unregisters the OS handles.
    _watcher: RecommendedWatcher,
    /// One message per quiet period in which at least one change happened.
    pub rx: Receiver<()>,
}

/// Watch `roots` recursively. `None` if the platform watcher can't start (no permissions, too
/// many watches) — watching is an optimisation, so a failure degrades to manual rescan rather
/// than an error the user has to dismiss.
pub fn watch_roots(roots: &[std::path::PathBuf], quiet: Duration) -> Option<RootWatcher> {
    let (raw_tx, raw_rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        // Errors here are per-event (a vanished path mid-scan); the drop is the signal.
        if res.is_ok() {
            let _ = raw_tx.send(());
        }
    })
    .map_err(|e| tracing::warn!(%e, "filesystem watch unavailable"))
    .ok()?;

    let mut watched = 0usize;
    for root in roots {
        match watcher.watch(Path::new(root), RecursiveMode::Recursive) {
            Ok(()) => watched += 1,
            Err(e) => tracing::warn!(%e, root = %root.display(), "watch failed for root"),
        }
    }
    if watched == 0 {
        return None;
    }

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || debounce(raw_rx, tx, quiet));
    Some(RootWatcher {
        _watcher: watcher,
        rx,
    })
}

/// Collapse a burst of raw events into one signal per quiet period. Exits when either channel
/// closes (watcher dropped, or the receiver went away).
fn debounce(raw: Receiver<()>, out: mpsc::Sender<()>, quiet: Duration) {
    while raw.recv().is_ok() {
        // Keep swallowing until the tree goes quiet for a full period.
        loop {
            match raw.recv_timeout(quiet) {
                Ok(()) => continue,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        if out.send(()).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A burst of changes must produce exactly one signal, and a later change a second one.
    #[test]
    fn debounce_collapses_a_burst_into_one_signal() {
        let (raw_tx, raw_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let quiet = Duration::from_millis(60);
        let h = std::thread::spawn(move || debounce(raw_rx, tx, quiet));

        for _ in 0..50 {
            raw_tx.send(()).unwrap();
        }
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "one signal"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(150)).is_err(),
            "the burst must not produce a second signal"
        );

        raw_tx.send(()).unwrap();
        assert!(
            rx.recv_timeout(Duration::from_secs(2)).is_ok(),
            "a later change signals again"
        );

        drop(raw_tx);
        h.join().unwrap();
    }

    #[test]
    fn watching_a_real_directory_reports_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let Some(w) = watch_roots(&[dir.path().to_path_buf()], Duration::from_millis(80)) else {
            return; // no watcher on this machine — nothing to assert
        };
        std::fs::write(dir.path().join("new.png"), b"x").unwrap();
        assert!(
            w.rx.recv_timeout(Duration::from_secs(5)).is_ok(),
            "a created file should reach the debounced channel"
        );
    }
}
