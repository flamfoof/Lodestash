//! Engine-level tests (v2 plan Task 2.3): start, the catalog lock, writer supervision, and the
//! read model following the catalog generation. Every engine here lives in a temp dir with its
//! own config — none reads or writes the user's catalog, config, caches or credential store.

use std::path::Path;
use std::time::Duration;

use lodestash_core::catalog::{Catalog, CrawlRecord};

use crate::types::PersistBatch;
use crate::writer::{Op, Probe, ProbeEnd, WriteError};
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

    match engine.writer().run(Op::Probe(Probe {
        end: ProbeEnd::Panic,
        ..Default::default()
    })) {
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

/// Review findings 7, 11 and 13: a command that fails or panics **after** it has committed, with a
/// tray job open. The row it wrote is in the catalog, so the generation must move and the next
/// read must show it — it used to be marked only on success, and the read model went on serving
/// the snapshot from before. And the job it opened must end (as failed) rather than spin in the
/// tray for the rest of the session.
#[test]
fn a_command_that_fails_or_panics_after_committing_is_still_seen_and_ends_its_job() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, _) = started(dir.path());
    let (_, ids) = seed(&engine, dir.path(), &["a.png", "b.png"]);
    assert_eq!(engine.read().status().favorites, 0);
    let jobs_before = engine.jobs().snapshot().recent.len();

    for (end, id) in [(ProbeEnd::Fail, ids[0]), (ProbeEnd::Panic, ids[1])] {
        let before = engine.catalog_generation();
        let out = engine.writer().run(Op::Probe(Probe {
            job: true,
            favorite: Some(id),
            end,
            ..Default::default()
        }));
        match (end, out) {
            (ProbeEnd::Fail, Err(WriteError::Failed(_)))
            | (ProbeEnd::Panic, Err(WriteError::Internal(_))) => {}
            (_, other) => panic!("{end:?} answered {other:?}"),
        }
        assert!(
            engine.catalog_generation() > before,
            "{end:?}: a row was committed and the generation did not move"
        );
    }
    assert_eq!(
        engine.read().status().favorites,
        2,
        "the read model is serving the snapshot from before the commands that failed"
    );
    let tray = engine.jobs().snapshot();
    assert!(
        tray.active.is_empty(),
        "a job outlived the command that panicked: {:?}",
        tray.active.iter().map(|j| &j.label).collect::<Vec<_>>()
    );
    assert_eq!(
        tray.recent.len() - jobs_before,
        2,
        "both jobs reached the finished ring"
    );
    assert!(
        tray.recent.iter().take(2).all(|j| !j.ok),
        "a job whose command failed was recorded as a success"
    );
}

/// A probe that holds the writer; returns once the writer has started running it.
fn hold_the_writer(engine: &Engine, hold: Duration) -> std::thread::JoinHandle<()> {
    let (started, running) = std::sync::mpsc::channel();
    let writer = engine.writer().clone();
    let held = std::thread::spawn(move || {
        writer
            .run(Op::Probe(Probe {
                started: Some(started),
                hold,
                ..Default::default()
            }))
            .expect("the held command finishes");
    });
    running.recv_timeout(SETTLE).expect("the hold started");
    held
}

/// Review finding 2, first half: a command whose caller gave up **before it started** does not
/// run. "Busy, try again" used to be said of a command that was still queued — and it then ran
/// minutes later, after the user had been told it had not. For Delete from disk that is files
/// deleted behind a "nothing happened" message.
#[test]
fn a_command_withdrawn_before_it_started_never_runs() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = started(dir.path());
    let held = hold_the_writer(&engine, Duration::from_millis(500));

    let impatient = engine
        .writer()
        .clone()
        .with_reply_timeout(Duration::from_millis(50));
    let out = impatient.run(Op::CreateCollection {
        name: "abandoned".into(),
    });
    assert!(
        matches!(out, Err(WriteError::Busy)),
        "a command that never started answered {out:?}"
    );
    assert!(
        out.unwrap_err().to_string().starts_with("busy:"),
        "the `busy` code leads the text an agent is handed"
    );

    held.join().expect("hold");
    // Anything queued behind the hold has been reached by the time this one answers.
    engine
        .writer()
        .run(Op::ClearStaged)
        .expect("the writer serves again");
    let reader = Catalog::open_reader(&cfg.catalog).expect("separate read");
    assert!(
        reader.list_collections().expect("collections").is_empty(),
        "a command its caller was told had not run, ran"
    );
}

/// Review finding 2, second half: a command that **was** running when its caller stopped waiting
/// says so — "still running", not "try again" — and it does finish and commit.
#[test]
fn a_command_still_running_when_its_caller_stops_waiting_says_so_and_finishes() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, _) = started(dir.path());
    let (_, ids) = seed(&engine, dir.path(), &["a.png"]);

    let impatient = engine
        .writer()
        .clone()
        .with_reply_timeout(Duration::from_millis(50));
    let out = impatient.run(Op::Probe(Probe {
        favorite: Some(ids[0]),
        hold: Duration::from_millis(400),
        ..Default::default()
    }));
    assert!(
        matches!(out, Err(WriteError::StillRunning)),
        "a running command answered {out:?}"
    );
    let said = out.unwrap_err().to_string();
    assert!(
        said.starts_with("busy:") && said.contains("still running") && !said.contains("try again"),
        "{said}"
    );

    engine
        .writer()
        .run(Op::ClearStaged)
        .expect("the next command runs once the slow one is done");
    assert_eq!(
        engine.read().status().favorites,
        1,
        "the command its caller stopped waiting for still landed"
    );
}

/// Review finding 7, the extract branch: asking to extract a pack mints its in-pack rows on the
/// spot, before any worker runs. That turn never marked the catalog, so when the extraction then
/// failed (here: the archive is gone) nothing ever did — and the rows stayed out of the read
/// model until some unrelated write.
#[test]
fn rows_minted_by_an_extract_request_show_even_when_the_extraction_fails() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, _) = started(dir.path());
    let lib = dir.path().join("lib").to_string_lossy().into_owned();
    engine
        .writer()
        .run(Op::PersistScan {
            batches: vec![PersistBatch {
                root: lib.clone(),
                name: "lib".into(),
                excludes_json: None,
                records: Vec::new(),
            }],
            packs: vec![crate::writer::DiscoveredPack {
                root: lib.clone(),
                abs_path: dir
                    .path()
                    .join("lib")
                    .join("gone.unitypackage")
                    .to_string_lossy()
                    .into_owned(),
                display_name: "Gone".into(),
                entries: vec![lodestash_core::catalog::PackageEntryRecord {
                    guid: "aaaa1111bbbb2222cccc3333dddd4444".into(),
                    pathname: Some("Assets/Gone/Rock.png".into()),
                    tar_offset: Some(512),
                    size_bytes: Some(16),
                }],
            }],
        })
        .expect("the pack is browsable");
    assert_eq!(engine.read().status().in_pack, 0, "nothing minted yet");
    let package_id = engine.read().packages()[0].id;

    engine
        .writer()
        .run(Op::ExtractPackage { package_id })
        .expect("accepted");
    // The worker fails on the missing archive and reports back; wait for the tray to settle.
    let deadline = std::time::Instant::now() + SETTLE;
    while !engine.jobs().snapshot().active.is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "the extract never settled"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    engine.writer().run(Op::ClearStaged).expect("drained");
    assert_eq!(
        engine.read().status().in_pack,
        1,
        "the minted in-pack row is in the catalog and not in the read model"
    );
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
            .any(|e| e.kind == "asset" && e.value == "/drop.png"),
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
        excludes.iter().any(|e| e.value == "/drop.png"),
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

fn touch(path: &Path) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(path, b"bytes").expect("write");
}

/// Index one extracted asset — a row whose bytes live at `<workspace>/<workspace_rel>` — through
/// the writer, and put the file there. `bbb/Old.png` beside it is what nothing references.
fn seed_workspace(engine: &Engine, cfg: &EngineConfig, root: &Path) {
    engine
        .writer()
        .run(Op::PersistScan {
            batches: vec![PersistBatch {
                root: root.join("lib").to_string_lossy().into_owned(),
                name: "lib".into(),
                excludes_json: None,
                records: vec![CrawlRecord {
                    rel_path: "Rock.png".into(),
                    name: "Rock.png".into(),
                    media_type: "image".into(),
                    size_bytes: 1,
                    mtime: 1,
                    workspace_rel: Some("aaa/Rock.png".into()),
                    content_hash: None,
                }],
            }],
            packs: Vec::new(),
        })
        .expect("seed an extracted asset");
    touch(&cfg.workspace_dir.join("aaa").join("Rock.png"));
    touch(&cfg.workspace_dir.join("bbb").join("Old.png"));
}

/// Review finding 1. "What is referenced" came back **empty** from an engine with no catalog, and
/// the sweep read empty as "nothing is referenced": a second Lodestash refused the catalog's lock
/// would delete every extracted file of the one that holds it. Neither the dry run nor the sweep
/// answers unless the engine is Ready; the engine that owns the catalog still reclaims exactly
/// what no live asset resolves through.
#[test]
fn workspace_reclaim_refuses_unless_the_engine_is_ready() {
    let dir = tempfile::tempdir().expect("tmp");
    let (first, cfg) = started(dir.path());
    seed_workspace(&first, &cfg, dir.path());
    let (kept, loose) = (
        cfg.workspace_dir.join("aaa").join("Rock.png"),
        cfg.workspace_dir.join("bbb").join("Old.png"),
    );

    let second = Engine::start(cfg.clone());
    assert!(matches!(second.status(), EngineStatus::Failed { .. }));
    assert!(
        crate::storage::workspace_reclaim_plan(&second).is_err(),
        "the dry run answered from an engine that has no catalog"
    );
    assert!(
        crate::storage::reclaim_workspace(&second).is_err(),
        "the sweep ran from an engine that has no catalog"
    );
    assert!(
        kept.exists() && loose.exists(),
        "an engine that is not Ready deleted workspace folders"
    );

    let plan = crate::storage::workspace_reclaim_plan(&first).expect("dry run");
    assert_eq!(plan.kept_dirs, 1, "the referenced folder is protected");
    assert_eq!(plan.entries.len(), 1, "{plan:?}");
    assert!(plan.entries[0].path.ends_with("bbb"));
    assert!(loose.exists(), "a dry run deletes nothing");

    let out = crate::storage::reclaim_workspace(&first).expect("reclaim");
    assert_eq!(out.removed, 1);
    assert!(kept.exists(), "a referenced file was reclaimed");
    assert!(!loose.exists(), "the unreferenced folder stayed");
    let log = out.log.expect("the reclaim is logged");
    assert!(std::fs::read_to_string(log).expect("log").contains("bbb"));
}

/// The same refusal when the engine is Ready but the question cannot be answered: a failed query
/// used to read as an empty set (`unwrap_or_default`), and so as permission to delete everything.
#[test]
fn workspace_reclaim_refuses_when_what_is_referenced_cannot_be_read() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = started(dir.path());
    seed_workspace(&engine, &cfg, dir.path());
    rusqlite::Connection::open(&cfg.catalog)
        .expect("raw")
        .execute_batch("ALTER TABLE asset_file RENAME TO asset_file_gone;")
        .expect("break the query");

    assert!(crate::storage::workspace_reclaim_plan(&engine).is_err());
    assert!(crate::storage::reclaim_workspace(&engine).is_err());
    assert!(
        cfg.workspace_dir.join("aaa").join("Rock.png").exists()
            && cfg.workspace_dir.join("bbb").join("Old.png").exists(),
        "a failed query was read as nothing referenced"
    );
}

/// An extraction places its files first and records them when it finishes; until then they are
/// in the workspace and referenced by nothing. The sweep waits for it rather than deleting the
/// half of a pack that has landed (and the `.tmp` folder the rest is being written through).
#[test]
fn workspace_reclaim_waits_for_a_running_extraction() {
    let dir = tempfile::tempdir().expect("tmp");
    let (engine, cfg) = started(dir.path());
    let (landed, partial) = (
        cfg.workspace_dir.join("ccc").join("New.png"),
        cfg.workspace_dir.join(".tmp").join("0123"),
    );
    touch(&landed);
    touch(&partial);

    let job = engine.jobs().start("Extract Pack".into(), "extract", true);
    assert!(
        crate::storage::reclaim_workspace(&engine).is_err(),
        "the sweep ran under an extraction"
    );
    assert!(landed.exists() && partial.exists());

    engine.jobs().finish(job.id, true, false, 0);
    let out = crate::storage::reclaim_workspace(&engine).expect("reclaim after the extraction");
    assert_eq!(out.removed, 2);
    assert!(!landed.exists());
}

/// Review finding 5: removing a file at the source root wrote its bare name as the exclusion, and
/// a bare name matches every file of that name in every folder of the source — so the next scan
/// dropped them all. The exclusion names exactly the file removed, and a whole selection is one
/// config write, not one per file.
#[test]
fn removing_a_root_level_file_excludes_only_that_file() {
    use lodestash_core::types::is_excluded;
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
    // In path order: Models/rock.png, other.png, rock.png.
    let (lib_root, ids) = seed(
        &engine,
        dir.path(),
        &["rock.png", "Models/rock.png", "other.png"],
    );
    let (nested, other, root_level) = (ids[0], ids[1], ids[2]);

    let writes = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = writes.clone();
    engine.config_store().on_change(move |_| {
        seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let targets = engine.read().write_targets(&[root_level, other]);
    engine
        .writer()
        .run(Op::RemoveFromLibrary { targets })
        .expect("remove");
    assert_eq!(
        writes.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "one config write for the whole selection"
    );

    // The list the crawl is handed, read back from the file through a store of its own.
    let excludes = cfg.open_config().get().source_excludes("Game", &lib);
    assert!(is_excluded(Path::new("rock.png"), &excludes));
    assert!(is_excluded(Path::new("other.png"), &excludes));
    assert!(
        !is_excluded(Path::new("Models/rock.png"), &excludes),
        "a same-named file in a folder was excluded with it: {excludes:?}"
    );
    // The catalog's own list says the same.
    let reader = Catalog::open_reader(&cfg.catalog).expect("separate read");
    let source = reader.source_info_by_root(&lib_root).unwrap().unwrap();
    let stored: Vec<lodestash_core::types::Exclude> = reader
        .list_source_excludes(source.id)
        .expect("excludes")
        .into_iter()
        .map(|e| lodestash_core::types::Exclude {
            kind: lodestash_core::types::ExcludeKind::Asset,
            value: e.value,
        })
        .collect();
    assert_eq!(stored.len(), 2);
    assert!(is_excluded(Path::new("rock.png"), &stored));
    assert!(!is_excluded(Path::new("Models/rock.png"), &stored));
    assert!(
        !trashed_on_disk(&cfg, &lib_root, nested),
        "the nested file is untouched"
    );
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
