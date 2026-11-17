//! Engine-level tests (v2 plan Task 2.3): start, the catalog lock, writer supervision, and the
//! read model following the catalog generation. Every engine here lives in a temp dir with its
//! own config — none reads or writes the user's catalog, config, caches or credential store.

use std::path::Path;
use std::time::Duration;

use lodestash_core::catalog::{Catalog, CrawlRecord};

use crate::config::AppConfig;
use crate::types::PersistBatch;
use crate::writer::{Op, WriteError};
use crate::{Engine, EngineConfig, EngineStatus, Origin};

const SETTLE: Duration = Duration::from_secs(5);

/// An engine config under `root` whose config file turns off the work a test does not want on
/// the writer's idle ticks: auto-identify (it would load whatever models the machine has
/// installed) and the source watch.
fn quiet(root: &Path) -> EngineConfig {
    let cfg = EngineConfig::under(root);
    cfg.config_file().save(&AppConfig {
        auto_identify: false,
        watch_sources: false,
        ..AppConfig::default()
    });
    cfg
}

fn started(root: &Path) -> (Engine, EngineConfig) {
    let cfg = quiet(root);
    let engine = Engine::start(cfg.clone());
    assert_eq!(engine.wait_settled(SETTLE), EngineStatus::Ready);
    (engine, cfg)
}

/// Index `names` as image files of one source rooted at `<root>/lib`, through the writer — the
/// same op a crawl sends. Returns the source root and the asset ids in name order.
fn seed(engine: &Engine, root: &Path, names: &[&str]) -> (String, Vec<i64>) {
    let lib = root.join("lib").to_string_lossy().into_owned();
    let records = names
        .iter()
        .map(|n| CrawlRecord {
            rel_path: (*n).into(),
            name: (*n).into(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 1,
            workspace_rel: None,
            content_hash: None,
        })
        .collect();
    engine
        .writer()
        .run(Op::PersistScan {
            batches: vec![PersistBatch {
                root: lib.clone(),
                name: "lib".into(),
                excludes_json: None,
                records,
            }],
            packs: Vec::new(),
        })
        .expect("seed");
    let mut items = engine
        .read()
        .page(&crate::read_model::GridQuery {
            page_size: 1000,
            ..Default::default()
        })
        .items;
    items.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    (lib, items.into_iter().map(|i| i.asset_id).collect())
}

/// Review S1: a fresh install has no catalog, and every write used to answer "no catalog is open".
/// The engine creates it — folder and all — reports `Ready`, and the first write lands.
#[test]
fn a_fresh_install_creates_its_catalog_and_takes_writes() {
    let dir = tempfile::tempdir().expect("tmp");
    let root = dir.path().join("never").join("existed");
    let (engine, cfg) = started(&root);
    assert!(cfg.catalog.is_file(), "the catalog was created");

    engine
        .writer()
        .run(Op::CreateCollection {
            name: "Rocks".into(),
        })
        .expect("the first write lands");
    let names: Vec<String> = engine
        .read()
        .collections()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names, ["Rocks"]);
}

/// v2 spec §4.2: one engine per catalog. The second is refused with `catalog_locked` before it
/// opens anything — two engines used to be able to migrate one file at once — and the first keeps
/// working. The lock goes when the first engine does.
#[test]
fn a_second_engine_on_the_same_catalog_is_refused() {
    let dir = tempfile::tempdir().expect("tmp");
    let (first, cfg) = started(dir.path());

    let second = Engine::start(cfg.clone());
    match second.status() {
        EngineStatus::Failed { code, .. } => assert_eq!(code, "catalog_locked"),
        other => panic!("the second engine started: {other:?}"),
    }
    assert!(matches!(
        second.writer().run(Op::ClearStaged),
        Err(WriteError::Unavailable(_))
    ));
    first
        .writer()
        .run(Op::ClearStaged)
        .expect("the first engine still writes");

    drop(first);
    let deadline = std::time::Instant::now() + SETTLE;
    loop {
        let third = Engine::start(cfg.clone());
        if third.wait_settled(SETTLE) == EngineStatus::Ready {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the lock outlived its engine"
        );
        drop(third);
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Review U9: a panic on the writer thread used to end every write for the session. Now the
/// command that panicked answers `internal`, and the next one runs.
#[test]
fn a_panicking_command_answers_internal_and_the_writer_keeps_serving() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, _) = started(dir.path());

    match engine.writer().run(Op::Panic) {
        Err(WriteError::Internal(msg)) => assert!(msg.contains("on purpose"), "{msg}"),
        other => panic!("a panic answered {other:?}"),
    }
    engine
        .writer()
        .run(Op::CreateCollection {
            name: "after".into(),
        })
        .expect("the writer survived the panic");
    assert_eq!(engine.read().collections().len(), 1);
}

/// A reply that does not come within the timeout answers `busy`; the command still finishes, and
/// the writer goes on serving.
#[test]
fn a_reply_that_takes_too_long_answers_busy() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, _) = started(dir.path());

    let impatient = engine
        .writer()
        .clone()
        .with_reply_timeout(Duration::from_millis(50));
    assert!(matches!(
        impatient.run(Op::Hold(Duration::from_millis(400))),
        Err(WriteError::Busy)
    ));
    engine
        .writer()
        .run(Op::ClearStaged)
        .expect("the next command runs once the slow one is done");
}

/// v2 spec §4.3: the read model follows the catalog generation. An agent's write — which the old
/// shell never reloaded for, hence its "Library updated · Refresh" banner — is visible on the next
/// read with no reload. So is a crawl's persist, which commits through a connection of its own.
#[test]
fn an_agent_write_moves_the_read_model_without_a_reload() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, _) = started(dir.path());
    let (_, ids) = seed(&engine, dir.path(), &["a.png", "b.png"]);
    assert_eq!(ids.len(), 2, "the persist's own connection was seen");
    let before = engine.read().generation();
    assert_eq!(engine.read().status().favorites, 0);

    engine
        .writer()
        .run_as(
            Op::SetFavorite {
                ids: vec![ids[0]],
                on: true,
            },
            Origin::Agent,
        )
        .expect("agent write");
    let read = engine.read();
    assert_eq!(read.status().favorites, 1, "the agent's write is visible");
    assert!(read.generation() > before, "at a newer generation");
}

/// Review S3: Remove from Library trashed the rows, then failed for want of an open project — so
/// the error path skipped the reload and the asset stayed on screen. Without a project it now
/// succeeds: the row is trashed and the exclusion is on the source's own blacklist, read back
/// through a connection of its own.
#[test]
fn remove_from_library_needs_no_open_project() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = started(dir.path());
    let (lib, ids) = seed(&engine, dir.path(), &["keep.png", "drop.png"]);
    let drop_id = ids[0]; // "drop.png" sorts first
    let targets = engine.read().write_targets(&[drop_id]);
    assert_eq!(targets.len(), 1);

    let out = engine
        .writer()
        .run(Op::RemoveFromLibrary { targets })
        .expect("Remove from Library succeeds without a project");
    assert_eq!(out.changed, 1);
    assert_eq!(
        engine.read().status().total_assets,
        1,
        "the removed asset left the library view"
    );

    let reader = Catalog::open_reader(&cfg.catalog).expect("separate read");
    let source = reader
        .source_info_by_root(&lib)
        .expect("query")
        .expect("source");
    let trashed: Vec<(String, bool)> = reader
        .entries_for_source(source.id)
        .expect("rows")
        .into_iter()
        .map(|a| (a.rel_path, a.trashed))
        .collect();
    assert!(trashed.contains(&("drop.png".into(), true)), "{trashed:?}");
    assert!(trashed.contains(&("keep.png".into(), false)), "{trashed:?}");
    let excludes = reader.list_source_excludes(source.id).expect("excludes");
    assert!(
        excludes
            .iter()
            .any(|e| e.kind == "asset" && e.value == "drop.png"),
        "the next scan would bring it back: {excludes:?}"
    );
}

/// With a project open, Remove from Library also mirrors the exclusion into that project's config —
/// the list the crawl is handed — and into **this engine's** config file: the helper used to save
/// the OS-default config whichever file the edit had loaded.
#[test]
fn remove_from_library_mirrors_into_the_open_projects_config() {
    let dir = tempfile::tempdir().expect("tmp");
    let cfg = quiet(dir.path());
    let lib = dir.path().join("lib");
    let file = cfg.config_file();
    file.edit(|c| {
        c.projects.push(crate::config::LibraryProject {
            name: "Game".into(),
            paths: vec![lib.clone()],
            blacklist: Default::default(),
            pack_only: Default::default(),
        });
        c.last_project_name = Some("Game".into());
    })
    .expect("project");
    let engine = Engine::start(cfg.clone());
    assert_eq!(engine.wait_settled(SETTLE), EngineStatus::Ready);
    let (_, ids) = seed(&engine, dir.path(), &["drop.png"]);
    let targets = engine.read().write_targets(&ids);
    engine
        .writer()
        .run(Op::RemoveFromLibrary { targets })
        .expect("remove");

    let excludes = file.load_display_only().source_excludes("Game", &lib);
    assert!(
        excludes.iter().any(|e| e.value == "drop.png"),
        "the crawl's list did not get the exclusion: {excludes:?}"
    );
}

/// Review B13, through the writer: trash {a, b} while a is already in the Trash, then undo — a
/// stays trashed. The op used to log every requested id, so the undo restored an asset the trash
/// never moved.
#[test]
fn undoing_a_trash_restores_only_what_it_changed() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = started(dir.path());
    let (lib, ids) = seed(&engine, dir.path(), &["a.png", "b.png"]);
    let (a, b) = (ids[0], ids[1]);
    let w = engine.writer();
    w.run(Op::SetTrashed {
        ids: vec![a],
        trashed: true,
    })
    .expect("trash a");
    let both = w
        .run(Op::SetTrashed {
            ids: vec![a, b],
            trashed: true,
        })
        .expect("trash both");
    assert_eq!(both.changed, 1, "only b changed");
    let op = engine.read().recent_ops(1)[0].id;
    w.run(Op::RevertOp { op_id: op }).expect("undo");

    let reader = Catalog::open_reader(&cfg.catalog).expect("separate read");
    let source = reader.source_info_by_root(&lib).unwrap().unwrap();
    let trashed = |id: i64| {
        reader
            .entries_for_source(source.id)
            .unwrap()
            .into_iter()
            .find(|r| r.asset_id == id)
            .unwrap()
            .trashed
    };
    assert!(trashed(a), "undo restored an asset this trash never moved");
    assert!(!trashed(b), "undo restored the one it did");
}

/// A catalog written by a newer build is refused — reported, not opened, not changed.
#[test]
fn a_catalog_newer_than_this_build_is_refused_untouched() {
    let dir = tempfile::tempdir().expect("tmp");
    let cfg = quiet(dir.path());
    drop(Catalog::open(&cfg.catalog).expect("create"));
    rusqlite::Connection::open(&cfg.catalog)
        .expect("raw")
        .pragma_update(None, "user_version", 9_999)
        .expect("future schema");

    let engine = Engine::start(cfg.clone());
    match engine.wait_settled(SETTLE) {
        EngineStatus::Failed { code, .. } => assert_eq!(code, "schema_too_new"),
        other => panic!("a newer catalog was opened: {other:?}"),
    }
    assert!(matches!(
        engine.writer().run(Op::ClearStaged),
        Err(WriteError::Unavailable(_))
    ));
    let version: i64 = rusqlite::Connection::open(&cfg.catalog)
        .expect("raw")
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .expect("version");
    assert_eq!(version, 9_999, "the newer catalog was migrated");
}

/// The change feed moved into the engine with the writer (ADR-0016): a subscriber hears a
/// committed write, under the origin that made it.
#[test]
fn a_subscriber_hears_a_committed_write() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, _) = started(dir.path());
    let mut events = engine.subscribe();

    engine
        .writer()
        .run(Op::CreateCollection { name: "x".into() })
        .expect("write");
    let deadline = std::time::Instant::now() + SETTLE;
    loop {
        match events.try_recv() {
            Ok(e) if e.catalog => {
                assert_eq!(e.origins, ["user"]);
                break;
            }
            Ok(_) | Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                assert!(std::time::Instant::now() < deadline, "no event arrived");
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(e) => panic!("the feed failed: {e}"),
        }
    }
}
