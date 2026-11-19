//! The change bus (ADR-0016, v2 spec §4.1): what changed, who caused it, and the generations a
//! reader compares against.
//!
//! Marking is a mutex flip — the writer never waits on a listener, and nothing is emitted from
//! inside a transaction. A publisher thread, started once something subscribes
//! ([`crate::Engine::subscribe`]), drains the marks at most once per [`PUBLISH_WINDOW`]. Until
//! then — and forever, headless — marks only accumulate: no events and no growth, since a mark is
//! a flag and a counter. The catalog generation is readable without a publisher; the read model
//! rebuilds when it moves (v2 spec §4.3).

use std::sync::{Arc, Condvar, Mutex};

use lodestash_core::catalog::Catalog;

/// Who caused a catalog change. The shell treats them differently (ADR-0016): the user's own
/// write refreshes what they are looking at; an agent's or the background lane's only says that
/// something moved, so a drain can never pull the grid out from under someone browsing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Agent,
    Background,
}

/// What a listener hears: which domains changed since the last publication, every origin that
/// touched the catalog in that window, and the per-domain generations. A refresh that read the
/// catalog at generation G clears only what was pending through G.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ChangedEvent {
    pub catalog: bool,
    pub jobs: bool,
    pub origins: Vec<&'static str>,
    pub catalog_gen: u64,
    pub jobs_gen: u64,
}

#[derive(Default)]
struct Pending {
    catalog: bool,
    jobs: bool,
    user: bool,
    agent: bool,
    background: bool,
    catalog_gen: u64,
    jobs_gen: u64,
}

impl Pending {
    fn dirty(&self) -> bool {
        self.catalog || self.jobs
    }

    /// Everything pending, as one event. The flags reset; the generations only ever climb.
    fn take(&mut self) -> ChangedEvent {
        let origins = [
            (self.user, "user"),
            (self.agent, "agent"),
            (self.background, "background"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        let event = ChangedEvent {
            catalog: self.catalog,
            jobs: self.jobs,
            origins,
            catalog_gen: self.catalog_gen,
            jobs_gen: self.jobs_gen,
        };
        self.catalog = false;
        self.jobs = false;
        self.user = false;
        self.agent = false;
        self.background = false;
        event
    }
}

/// The longest a committed change waits before a listener hears of it. A fixed window, not a
/// trailing debounce: the preview drain commits continuously, and a debounce waiting for quiet
/// would never fire while it ran. Fifty tag writes inside one window publish once; a longer burst
/// publishes once per window — never once per commit, which is what it used to be.
pub const PUBLISH_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);

/// The process's change publication (ADR-0016).
#[derive(Clone, Default)]
pub struct Changes {
    inner: Arc<(Mutex<Pending>, Condvar)>,
}

impl Changes {
    pub fn catalog(&self, origin: Origin) {
        let (lock, cv) = &*self.inner;
        let mut p = lock.lock().expect("changes lock");
        p.catalog = true;
        p.catalog_gen += 1;
        match origin {
            Origin::User => p.user = true,
            Origin::Agent => p.agent = true,
            Origin::Background => p.background = true,
        }
        cv.notify_one();
    }

    pub fn jobs(&self) {
        let (lock, cv) = &*self.inner;
        let mut p = lock.lock().expect("changes lock");
        p.jobs = true;
        p.jobs_gen += 1;
        cv.notify_one();
    }

    /// The catalog generation now: it climbs once per committed change, whoever made it. A reader
    /// that built its view at generation G is stale exactly when this is no longer G.
    pub fn catalog_generation(&self) -> u64 {
        self.inner.0.lock().expect("changes lock").catalog_gen
    }

    /// Drain the marks into `sink`, at most once per `window`, on a thread of its own. Started
    /// once, by whatever first wants events.
    pub(crate) fn spawn_publisher(
        &self,
        window: std::time::Duration,
        mut sink: impl FnMut(ChangedEvent) + Send + 'static,
    ) {
        let inner = self.inner.clone();
        std::thread::Builder::new()
            .name("lodestash-changes".into())
            .spawn(move || {
                let (lock, cv) = &*inner;
                loop {
                    let mut p = lock.lock().expect("changes lock");
                    while !p.dirty() {
                        p = cv.wait(p).expect("changes lock");
                    }
                    drop(p);
                    // The window opens at the first mark; everything landing inside it rides along.
                    std::thread::sleep(window);
                    let event = lock.lock().expect("changes lock").take();
                    sink(event);
                }
            })
            .expect("change publisher spawns");
    }

    /// Drain whatever is pending right now, as the publisher would at the end of a window.
    #[cfg(test)]
    pub(crate) fn take_pending(&self) -> ChangedEvent {
        self.inner.0.lock().expect("changes lock").take()
    }
}

/// Where the catalog stood before a writer turn: this connection's own change count, and SQLite's
/// `data_version`, which moves when **another** connection commits. The engine opens no second
/// writing connection (a scan's persist was the last one, v2 plan Task 2.5); the check stays so
/// that a commit from outside the writer — another tool on the file — is still a change.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Stamp {
    total: u64,
    data_version: Option<i64>,
}

impl Stamp {
    pub(crate) fn of(catalog: &Catalog) -> Self {
        Self {
            total: catalog.total_changes(),
            data_version: catalog.data_version().ok(),
        }
    }
}

/// Publish a catalog change iff this turn committed rows — through the writer's connection, or
/// through one it opened. SQLite's own counters decide, so a turn that ran and wrote nothing is not
/// reported as a change.
pub(crate) fn mark_committed(changes: &Changes, catalog: &Catalog, before: Stamp, origin: Origin) {
    let after = Stamp::of(catalog);
    let elsewhere = match (before.data_version, after.data_version) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    };
    if after.total > before.total || elsewhere {
        changes.catalog(origin);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// ADR-0016's headline: fifty writes landing inside one window reach a listener as one event.
    /// Before, every op emitted `catalog` *and* `jobs` — a hundred IPC events and fifty status
    /// re-reads for fifty tag writes, and a `jobs` event for ops that never touched the tray.
    #[test]
    fn fifty_writes_in_one_window_publish_once() {
        let changes = Changes::default();
        let (tx, rx) = mpsc::channel();
        changes.spawn_publisher(std::time::Duration::from_millis(60), move |e| {
            let _ = tx.send(e);
        });
        for _ in 0..50 {
            changes.catalog(Origin::User);
        }
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("the window closes and publishes");
        assert_eq!(
            event,
            ChangedEvent {
                catalog: true,
                jobs: false,
                origins: vec!["user"],
                catalog_gen: 50,
                jobs_gen: 0,
            },
            "one event carrying every write's generation, and no jobs change nobody caused"
        );
        assert!(
            rx.recv_timeout(std::time::Duration::from_millis(200))
                .is_err(),
            "nothing after it: the fifty were one publication"
        );
    }

    /// A fixed window, not a trailing debounce: a producer that never pauses (the preview drain)
    /// still publishes every window, where a debounce waiting for quiet would starve forever. And a
    /// window that mixes origins keeps all of them — the shell must not take an agent's write for
    /// the user's own because the two happened to share 250 ms.
    #[test]
    fn a_continuous_producer_publishes_every_window_and_loses_no_origin() {
        let changes = Changes::default();
        let (tx, rx) = mpsc::channel();
        changes.spawn_publisher(std::time::Duration::from_millis(20), move |e| {
            let _ = tx.send(e);
        });
        let origins = [Origin::User, Origin::Agent, Origin::Background];
        let started = std::time::Instant::now();
        let mut marks = 0u64;
        while started.elapsed() < std::time::Duration::from_millis(300) {
            changes.catalog(origins[(marks % 3) as usize]);
            marks += 1;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut events = Vec::new();
        while let Ok(e) = rx.recv_timeout(std::time::Duration::from_millis(200)) {
            events.push(e);
        }
        assert!(
            events.len() >= 5,
            "300 ms of continuous writes over a 20 ms window published {} time(s) — starved",
            events.len()
        );
        assert!(
            events
                .windows(2)
                .all(|p| p[0].catalog_gen < p[1].catalog_gen),
            "generations only climb"
        );
        assert_eq!(
            events.last().map(|e| e.catalog_gen),
            Some(marks),
            "the last event accounts for every write — none lost, none deferred forever"
        );
        let seen: std::collections::HashSet<&str> = events
            .iter()
            .flat_map(|e| e.origins.iter().copied())
            .collect();
        assert_eq!(seen.len(), 3, "every origin survives coalescing: {seen:?}");
    }

    /// Only a turn that committed rows is a catalog change. `total_changes` is SQLite's own count,
    /// so a turn that only read stays silent and a write says so, under the origin that caused it.
    #[test]
    fn only_a_committed_write_marks_the_catalog() {
        let c = Catalog::open_in_memory().expect("open");
        let changes = Changes::default();

        let before = Stamp::of(&c);
        let _ = c.source_infos().expect("read");
        mark_committed(&changes, &c, before, Origin::User);
        assert!(!changes.take_pending().catalog, "a read is not a change");

        let before = Stamp::of(&c);
        c.upsert_import_source("s", "/s", "folder").expect("write");
        mark_committed(&changes, &c, before, Origin::Agent);
        let event = changes.take_pending();
        assert!(event.catalog, "a committed write is a change");
        assert_eq!(
            event.origins,
            vec!["agent"],
            "under the origin that caused it"
        );
        assert_eq!(changes.catalog_generation(), 1);
    }

    /// A commit through a second connection during a writer turn is still a change: the writer's
    /// own `total_changes` cannot see it, `data_version` can. (A scan's persist used to be such a
    /// connection; nothing in the engine is any more.)
    /// Before the read model rebuilt on the generation, an added source showed up only because the
    /// shell reloaded after every command.
    #[test]
    fn a_commit_through_another_connection_marks_the_catalog() {
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("catalog.db");
        let writer = Catalog::open(&db).expect("open");
        let changes = Changes::default();

        let before = Stamp::of(&writer);
        Catalog::open(&db)
            .expect("second connection")
            .upsert_import_source("s", "/s", "folder")
            .expect("write elsewhere");
        mark_committed(&changes, &writer, before, Origin::User);
        assert!(changes.take_pending().catalog);
    }
}
