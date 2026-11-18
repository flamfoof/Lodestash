//! Engine-level tests (v2 plan Task 2.3): start, the catalog lock, writer supervision, and the
//! read model following the catalog generation. Every engine here lives in a temp dir with its
//! own config — none reads or writes the user's catalog, config, caches or credential store.

use std::path::Path;
use std::time::Duration;

use lodestash_core::catalog::{Catalog, CrawlRecord};

use crate::types::PersistBatch;
use crate::writer::{Op, WriteError};
use crate::{Engine, EngineConfig, EngineStatus, Origin};

const SETTLE: Duration = Duration::from_secs(5);

/// An engine config under `root` whose config file turns off the work a test does not want on
/// the writer's idle ticks: auto-identify (it would load whatever models the machine has
/// installed) and the source watch.
fn quiet(root: &Path) -> EngineConfig {
    let cfg = EngineConfig::under(root);
    cfg.open_config()
        .update(|c| {
            c.auto_identify = false;
            c.watch_sources = false;
        })
        .expect("seed the config");
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
    cfg.open_config()
        .update(|c| {
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

    // Read back from the file, through a store of its own: the engine's update reached the disk.
    let excludes = cfg.open_config().get().source_excludes("Game", &lib);
    assert!(
        excludes.iter().any(|e| e.value == "drop.png"),
        "the crawl's list did not get the exclusion: {excludes:?}"
    );
}

/// Review B12, at the engine: a config that does not parse is quarantined, the engine starts on
/// defaults, and its report carries `config_recovered` until the user dismisses it. Changing a
/// setting then writes a valid file.
#[test]
fn a_corrupt_config_is_reported_by_the_engine_until_dismissed() {
    let dir = tempfile::tempdir().expect("tmp");
    let cfg = EngineConfig::under(dir.path());
    std::fs::create_dir_all(&cfg.config_dir).unwrap();
    std::fs::write(cfg.config_path(), b"{\"auto_identify\": fal").unwrap();

    let engine = Engine::start(cfg.clone());
    assert_eq!(engine.wait_settled(SETTLE), EngineStatus::Ready);
    let report = serde_json::to_value(engine.report()).unwrap();
    assert_eq!(report["state"], "ready");
    let moved = engine.config_recovered().expect("the recovery is reported");
    assert_eq!(report["config_recovered"], moved.to_string_lossy().as_ref());
    assert!(moved.is_file(), "the bad file is kept");

    // Quiet the idle lane before it can load models, then prove the write path is whole again.
    engine
        .config_store()
        .update(|c| {
            c.auto_identify = false;
            c.watch_sources = false;
        })
        .expect("save");
    let reread = cfg.open_config();
    assert!(reread.recovered().is_none(), "the saved file parses");
    assert!(!reread.get().auto_identify);

    engine.dismiss_config_recovered();
    assert!(engine.report().config_recovered.is_none());
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

/// Is `asset_id` in the Trash, read through a connection of its own?
fn trashed_on_disk(cfg: &EngineConfig, lib: &str, asset_id: i64) -> bool {
    let reader = Catalog::open_reader(&cfg.catalog).expect("separate read");
    let source = reader.source_info_by_root(lib).unwrap().unwrap();
    reader
        .entries_for_source(source.id)
        .unwrap()
        .into_iter()
        .find(|r| r.asset_id == asset_id)
        .expect("the row exists")
        .trashed
}

/// Review finding 6: Delete from disk and Remove from library trashed through an unlogged write,
/// so the undo of an *earlier* "move to Trash" flipped the row back — resurrecting an asset whose
/// file the app had just deleted, or one the user had removed from the library. Driven through
/// the writer, read back through a separate connection.
#[test]
fn undo_never_restores_a_deleted_or_removed_asset() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = started(dir.path());
    let (lib, ids) = seed(&engine, dir.path(), &["a.png", "b.png", "c.png"]);
    let (a, b, c) = (ids[0], ids[1], ids[2]);
    std::fs::create_dir_all(&lib).expect("lib");
    let file_a = Path::new(&lib).join("a.png");
    std::fs::write(&file_a, b"bytes").expect("a on disk");
    let w = engine.writer();

    // Trash a (undoable), then delete its file from disk.
    w.run(Op::SetTrashed {
        ids: vec![a],
        trashed: true,
    })
    .expect("trash a");
    let trash_a = engine.read().recent_ops(1)[0].id;
    let targets = engine.read().write_targets(&[a]);
    let roots = engine.read().roots();
    let out = w
        .run(Op::DeleteFromDisk {
            targets,
            roots,
            workspace: Some(cfg.workspace_dir.clone()),
        })
        .expect("delete from disk");
    assert_eq!((out.refused, out.failed), (0, 0), "the delete went through");
    assert!(!file_a.exists(), "the file left the disk");
    assert!(
        w.run(Op::RevertOp { op_id: trash_a }).is_err(),
        "the trash of a deleted file is no longer an undoable op"
    );
    assert!(
        trashed_on_disk(&cfg, &lib, a),
        "undo un-trashed an asset whose file was deleted"
    );

    // Trash {b, c} (undoable), then remove b from the library: undo restores c only.
    w.run(Op::SetTrashed {
        ids: vec![b, c],
        trashed: true,
    })
    .expect("trash b and c");
    let trash_bc = engine.read().recent_ops(1)[0].id;
    let targets = engine.read().write_targets(&[b]);
    w.run(Op::RemoveFromLibrary { targets })
        .expect("remove b from the library");
    w.run(Op::RevertOp { op_id: trash_bc }).expect("undo");
    assert!(
        trashed_on_disk(&cfg, &lib, b),
        "undo brought back an asset the user removed from the library"
    );
    assert!(
        !trashed_on_disk(&cfg, &lib, c),
        "the rest of that trash still undoes"
    );
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
