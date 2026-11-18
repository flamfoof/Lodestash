//! The catalog file's lifecycle (v2 spec §4.2, plan Task 1.2): created when missing, backed up
//! before a migration touches it, migrated once however many connections race to do it, and
//! refused — untouched — when a newer build wrote it.

use super::*;

/// A file catalog stopped at `version`, closed — what an older build leaves on disk.
fn file_catalog_at(path: &Path, version: u32) {
    let conn = Connection::open(path).expect("open");
    configure_connection(&conn).expect("pragmas");
    let mut c = Catalog {
        conn,
        db_path: Some(path.to_path_buf()),
        appraise_focus: AppraiseFocus::default(),
    };
    c.apply_migrations_up_to(version)
        .unwrap_or_else(|e| panic!("migrations to v{version}: {e}"));
    c.upsert_import_source("Lib", "C:/lib", "folder")
        .expect("a row worth keeping");
}

fn user_version(path: &Path) -> u32 {
    Connection::open(path)
        .expect("raw")
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("user_version")
}

/// The `<catalog>.v<N>.bak` files beside `db`, by version, oldest first.
fn backups(db: &Path) -> Vec<u32> {
    let prefix = format!("{}.v", db.file_name().unwrap().to_string_lossy());
    let mut found: Vec<u32> = std::fs::read_dir(db.parent().unwrap())
        .expect("dir")
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.strip_prefix(&prefix)?
                .strip_suffix(".bak")?
                .parse()
                .ok()
        })
        .collect();
    found.sort_unstable();
    found
}

fn remove_catalog(db: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let p = PathBuf::from(format!("{}{suffix}", db.display()));
        if p.exists() {
            std::fs::remove_file(&p).expect("remove");
        }
    }
}

#[test]
fn a_missing_catalog_is_created_at_the_current_schema_without_a_backup() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    let c = Catalog::open(&db).expect("create");
    assert_eq!(c.schema_version().expect("version"), SCHEMA_VERSION);
    drop(c);
    assert!(db.exists(), "the file was created");
    assert_eq!(user_version(&db), SCHEMA_VERSION);
    assert!(backups(&db).is_empty(), "nothing to back up in a new file");
}

/// A backup is taken when, and only when, a migration is about to change an existing catalog —
/// and it is the catalog as it was: the old version, with its rows.
#[test]
fn an_existing_catalog_is_backed_up_before_it_is_migrated_and_only_then() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    file_catalog_at(&db, SCHEMA_VERSION - 1);

    let c = Catalog::open(&db).expect("migrate");
    assert_eq!(c.schema_version().expect("version"), SCHEMA_VERSION);
    assert_eq!(c.source_infos().expect("sources").len(), 1, "rows kept");
    drop(c);
    assert_eq!(backups(&db), vec![SCHEMA_VERSION - 1]);
    let bak = PathBuf::from(format!("{}.v{}.bak", db.display(), SCHEMA_VERSION - 1));
    assert_eq!(
        user_version(&bak),
        SCHEMA_VERSION - 1,
        "the backup is the old file"
    );
    let rows: i64 = Connection::open(&bak)
        .expect("backup opens")
        .query_row("SELECT COUNT(*) FROM import_source", [], |r| r.get(0))
        .expect("rows");
    assert_eq!(rows, 1, "the backup holds the old rows");

    // Opening a current catalog migrates nothing, so it backs up nothing.
    drop(Catalog::open(&db).expect("reopen"));
    assert_eq!(backups(&db), vec![SCHEMA_VERSION - 1], "no second backup");
}

fn backup_path(db: &Path, version: u32) -> PathBuf {
    PathBuf::from(format!("{}.v{version}.bak", db.display()))
}

/// Stamp a backup as written `age_secs` ago (negative: ahead of the clock), so "newest" is stated
/// rather than raced.
fn set_age(path: &Path, age_secs: i64) {
    let span = std::time::Duration::from_secs(age_secs.unsigned_abs());
    let now = std::time::SystemTime::now();
    let when = if age_secs >= 0 {
        now - span
    } else {
        now + span
    };
    std::fs::File::options()
        .write(true)
        .open(path)
        .expect("open backup")
        .set_modified(when)
        .expect("set mtime");
}

/// "Newest" is by when a backup was written, not by its schema number: the versions arrive out of
/// order here (a catalog restored from an older copy migrates again), and the two written last
/// are the two kept. Keeping the two highest versions kept `v(N-1)` — the oldest file of the
/// three — and dropped the one before last.
#[test]
fn the_newest_two_backups_are_kept() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    let order = [SCHEMA_VERSION - 1, SCHEMA_VERSION - 3, SCHEMA_VERSION - 2];
    for (i, from) in order.into_iter().enumerate() {
        remove_catalog(&db);
        file_catalog_at(&db, from);
        drop(Catalog::open(&db).expect("migrate"));
        // Written in this order, a minute apart.
        set_age(&backup_path(&db, from), 600 - 60 * i as i64);
    }
    assert_eq!(
        backups(&db),
        vec![SCHEMA_VERSION - 3, SCHEMA_VERSION - 2],
        "the two most recently written are kept, whatever their versions"
    );
}

/// The backup a migration has just taken is the one copy of the catalog as it was — it is never
/// the one pruned (review finding 4). With two higher-versioned backups already beside it, keeping
/// "the two highest versions" deleted the file it had written a moment before.
#[test]
fn the_backup_just_written_is_never_pruned() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    let (older, newer, restored) = (SCHEMA_VERSION - 2, SCHEMA_VERSION - 1, SCHEMA_VERSION - 3);
    for (version, age) in [(older, 7200), (newer, 3600)] {
        std::fs::write(backup_path(&db, version), b"an earlier backup").expect("backup");
        set_age(&backup_path(&db, version), age);
    }
    // The user put an older copy of the catalog back; opening it migrates it again.
    file_catalog_at(&db, restored);
    drop(Catalog::open(&db).expect("migrate"));

    assert_eq!(
        backups(&db),
        vec![restored, newer],
        "the new backup and the newest earlier one are kept"
    );
    assert_eq!(
        user_version(&backup_path(&db, restored)),
        restored,
        "and it is the catalog as it was"
    );
}

/// Pruning is housekeeping: a backup that cannot be removed (here a directory wearing a backup's
/// name), or a folder that cannot be listed, is logged and the catalog still opens.
#[test]
fn a_backup_that_cannot_be_pruned_does_not_fail_the_open() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    let stuck = backup_path(&db, 1);
    std::fs::create_dir(&stuck).expect("a directory where a backup would be");
    std::fs::write(stuck.join("keeps-it-non-empty"), b"x").expect("fill");
    // Newer than the directory, so the directory is the one pruning reaches for.
    std::fs::write(backup_path(&db, 2), b"an earlier backup").expect("backup");
    set_age(&backup_path(&db, 2), -60);
    file_catalog_at(&db, SCHEMA_VERSION - 1);

    let c = Catalog::open(&db).expect("a failed prune must not fail the open");
    assert_eq!(c.schema_version().expect("version"), SCHEMA_VERSION);
    assert_eq!(
        user_version(&backup_path(&db, SCHEMA_VERSION - 1)),
        SCHEMA_VERSION - 1,
        "the backup was still taken"
    );
    assert!(stuck.is_dir(), "the unremovable entry is left as it was");
}

/// `Path::parent` of a bare `catalog.db` is `Some("")`, not `None` — and listing `""` fails, which
/// used to fail the open of any catalog named by a bare relative path.
#[test]
fn a_bare_file_name_has_the_current_directory_as_its_folder() {
    assert_eq!(folder_of(Path::new("catalog.db")), Path::new("."));
    assert_eq!(folder_of(Path::new("data/catalog.db")), Path::new("data"));
    assert!(std::fs::read_dir(folder_of(Path::new("catalog.db"))).is_ok());
}

/// A catalog a newer build wrote is refused by both constructors, with what to do, and left
/// exactly as it was — no migration, no backup.
#[test]
fn a_newer_schema_is_refused_by_both_constructors_and_left_alone() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    drop(Catalog::open(&db).expect("create"));
    let future = SCHEMA_VERSION + 1;
    Connection::open(&db)
        .expect("raw")
        .execute_batch(&format!("PRAGMA user_version = {future};"))
        .expect("future marker");

    let refused = |r: Result<()>| match r {
        Err(CoreError::SchemaTooNew { found, supported }) => {
            assert_eq!((found, supported), (future, SCHEMA_VERSION));
        }
        other => panic!("not refused as too new: {other:?}"),
    };
    refused(Catalog::open(&db).map(drop));
    refused(Catalog::open_reader(&db).map(drop));
    assert_eq!(user_version(&db), future, "the refused file was changed");
    assert!(backups(&db).is_empty(), "a refusal backed up the file");
}

/// Two connections opening the same new file at the same moment both succeed, and every
/// migration ran exactly once: one schema version, one catalog identity.
#[test]
fn two_connections_racing_to_create_one_catalog_both_succeed() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    let barrier = std::sync::Barrier::new(2);
    let ids: Vec<String> = std::thread::scope(|s| {
        let racers: Vec<_> = (0..2)
            .map(|_| {
                s.spawn(|| {
                    barrier.wait();
                    let c = Catalog::open(&db).expect("open while another opens");
                    assert_eq!(c.schema_version().expect("version"), SCHEMA_VERSION);
                    c.catalog_id().expect("identity")
                })
            })
            .collect();
        racers.into_iter().map(|h| h.join().unwrap()).collect()
    });
    assert_eq!(ids[0], ids[1], "the identity was minted twice");
    assert_eq!(user_version(&db), SCHEMA_VERSION);
}

/// The same race over a catalog that **needs migrating**, so both connections also back it up.
/// Both succeed, the step ran once, and the one backup beside the catalog is whole and is the
/// catalog as it was. They used to share one temporary name: the second `VACUUM INTO` found the
/// first's file there and the open failed.
#[test]
fn two_connections_racing_to_migrate_one_catalog_both_succeed() {
    for round in 0..12 {
        let dir = tempfile::tempdir().expect("tmp");
        let db = dir.path().join("catalog.db");
        file_catalog_at(&db, SCHEMA_VERSION - 1);
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|s| {
            let racers: Vec<_> = (0..2)
                .map(|_| {
                    s.spawn(|| {
                        barrier.wait();
                        let c = Catalog::open(&db)
                            .unwrap_or_else(|e| panic!("round {round}: a racing open failed: {e}"));
                        assert_eq!(c.schema_version().expect("version"), SCHEMA_VERSION);
                        assert_eq!(c.source_infos().expect("sources").len(), 1, "rows kept");
                    })
                })
                .collect();
            for h in racers {
                h.join().unwrap();
            }
        });
        assert_eq!(user_version(&db), SCHEMA_VERSION);
        assert_eq!(backups(&db), vec![SCHEMA_VERSION - 1], "round {round}");
        let bak = backup_path(&db, SCHEMA_VERSION - 1);
        assert_eq!(
            user_version(&bak),
            SCHEMA_VERSION - 1,
            "round {round}: the backup is not the catalog as it was"
        );
        let rows: i64 = Connection::open(&bak)
            .expect("backup opens")
            .query_row("SELECT COUNT(*) FROM import_source", [], |r| r.get(0))
            .expect("rows");
        assert_eq!(rows, 1, "round {round}: the backup lost its rows");
        let strays: Vec<String> = std::fs::read_dir(dir.path())
            .expect("dir")
            .filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
            .filter(|n| n.contains(".partial"))
            .collect();
        assert!(strays.is_empty(), "round {round}: left {strays:?} behind");
    }
}

// ---- M32: the crawl and in-pack lookups are index searches (v2 plan Task 1.4) ----------------

/// How SQLite would run `sql` here, one plan line per step.
fn query_plan(c: &Catalog, sql: &str, args: impl rusqlite::Params) -> String {
    let mut stmt = c
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .expect("explain");
    let lines = stmt
        .query_map(args, |r| r.get::<_, String>(3))
        .expect("plan")
        .collect::<std::result::Result<Vec<_>, _>>()
        .expect("plan rows");
    lines.join("\n")
}

/// How many loose files and how many pack entries [`populated`] holds.
const PLAN_ROWS: usize = 600;

/// A catalog **file** with real rows in it and fresh statistics: two sources of [`PLAN_ROWS`]
/// loose files each (the same relative paths in both, so `rel_path` alone is not unique), and one
/// pack of [`PLAN_ROWS`] minted entries.
///
/// The plan tests used to explain their query against an empty in-memory database, where the
/// planner has nothing to weigh and the answer is the same whatever the data would make it do.
/// Here it plans as it does for a library: with rows, on a file, after `ANALYZE`.
fn populated(dir: &Path) -> (Catalog, i64, i64) {
    let mut c = Catalog::open(&dir.join("catalog.db")).expect("open");
    c.seed_media_types().expect("seed");
    let records: Vec<CrawlRecord> = (0..PLAN_ROWS)
        .map(|i| CrawlRecord {
            rel_path: format!("dir{}/file{i}.png", i % 20),
            name: format!("file{i}.png"),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 1,
            workspace_rel: None,
            content_hash: None,
        })
        .collect();
    let mut first = 0;
    for root in ["C:/lib-a", "C:/lib-b"] {
        let src = c.upsert_import_source(root, root, "folder").expect("src");
        c.record_crawl(src, None, &records).expect("crawl");
        if first == 0 {
            first = src;
        }
    }
    let pkg = c
        .upsert_package(first, "C:/lib-a/p.unitypackage", "p", None, None)
        .expect("package");
    let entries: Vec<PackageEntryRecord> = (0..PLAN_ROWS)
        .map(|i| PackageEntryRecord {
            guid: format!("{i:032x}"),
            pathname: Some(format!("Assets/dir{}/in{i}.png", i % 20)),
            tar_offset: Some(512 * (i as i64 + 1)),
            size_bytes: Some(10),
        })
        .collect();
    c.record_package_entries(pkg, &entries).expect("entries");
    let classified: HashMap<String, InPackClass> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let class = InPackClass {
                name: format!("in{i}"),
                media_type: "image".into(),
            };
            (e.guid.clone(), class)
        })
        .collect();
    c.mint_in_pack_assets(pkg, &classified).expect("mint");
    c.conn.execute_batch("ANALYZE;").expect("analyze");
    (c, first, pkg)
}

/// `record_crawl` looks every crawled path up in its source. Without an index led by `rel_path`
/// that is a walk of the source per file — 2,000 lookups over a 20k-asset catalog took 4.5 s.
///
/// Asserted where it matters: on a populated file, where the planner could choose to walk the
/// source's assets instead — and does, the moment the index is gone, which is what proves the
/// assertion can fail.
#[test]
fn the_crawl_lookup_searches_by_path() {
    let dir = tempfile::tempdir().expect("tmp");
    let (c, src, _) = populated(dir.path());
    let args = || (src, "dir7/file7.png");
    let found: i64 = c
        .conn
        .query_row(
            &format!("SELECT COUNT(*) FROM ({CRAWL_LOOKUP_SQL})"),
            args(),
            |r| r.get(0),
        )
        .expect("the lookup runs");
    assert_eq!(
        found, 1,
        "one row of this source, though two sources hold the path"
    );

    let plan = query_plan(&c, CRAWL_LOOKUP_SQL, args());
    assert!(
        plan.contains("USING INDEX idx_file_rel_path (rel_path=?)") && !plan.contains("SCAN"),
        "the crawl lookup walks instead of searching:\n{plan}"
    );

    c.conn
        .execute_batch("DROP INDEX idx_file_rel_path;")
        .expect("drop");
    let without = query_plan(&c, CRAWL_LOOKUP_SQL, args());
    assert!(
        !without.contains("rel_path=?"),
        "without its index the lookup cannot search by path, so the assertion above is real:\n{without}"
    );
}

/// Materializing an extracted pack finds each entry by `(package, pathname)`; without an index
/// over both, every file walks the whole pack's manifest. Same shape as the test above.
#[test]
fn the_materialize_lookup_searches_by_package_and_path() {
    let dir = tempfile::tempdir().expect("tmp");
    let (c, _, pkg) = populated(dir.path());
    let args = || (pkg, "Assets/dir7/in7.png", "ws/in7.png");
    let plan = query_plan(&c, MATERIALIZE_SQL, args());
    assert!(
        plan.contains("idx_pkgentry_path (package_id=? AND pathname=?)") && !plan.contains("SCAN"),
        "the materialize lookup walks the pack:\n{plan}"
    );
    assert_eq!(
        c.conn.execute(MATERIALIZE_SQL, args()).expect("it runs"),
        1,
        "and it updates exactly the one file of that entry"
    );

    c.conn
        .execute_batch("DROP INDEX idx_pkgentry_path;")
        .expect("drop");
    let without = query_plan(&c, MATERIALIZE_SQL, args());
    assert!(
        !without.contains("pathname=?"),
        "without its index the lookup cannot search by pathname:\n{without}"
    );
}

/// M32 adds indexes and nothing else: a v31 catalog keeps every row.
#[test]
fn migrating_a_v31_catalog_to_v32_keeps_its_rows() {
    let mut c = Catalog {
        conn: Connection::open_in_memory().expect("open"),
        db_path: None,
        appraise_focus: AppraiseFocus::default(),
    };
    configure_connection(&c.conn).expect("pragmas");
    c.apply_migrations_up_to(31).expect("v31");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    let rec = |rel: &str| CrawlRecord {
        rel_path: rel.into(),
        name: rel.into(),
        media_type: "image".into(),
        size_bytes: 1,
        mtime: 1,
        workspace_rel: None,
        content_hash: Some(format!("h-{rel}")),
    };
    c.record_crawl(src, None, &[rec("a.png"), rec("b/c.png")])
        .expect("crawl");
    let pkg = c
        .upsert_package(src, "C:/lib/p.unitypackage", "p", None, None)
        .expect("package");
    c.record_package_entries(
        pkg,
        &[PackageEntryRecord {
            guid: "0123456789abcdef0123456789abcdef".into(),
            pathname: Some("Assets/x.png".into()),
            tar_offset: Some(512),
            size_bytes: Some(10),
        }],
    )
    .expect("entries");

    const TABLES: [&str; 4] = ["import_source", "asset", "asset_file", "package_entry"];
    let counts = |c: &Catalog| -> Vec<i64> {
        TABLES
            .iter()
            .map(|t| {
                c.conn
                    .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                    .expect("count")
            })
            .collect()
    };
    let before = counts(&c);
    assert_eq!(before, vec![1, 2, 2, 1], "the fixture holds what it should");

    c.apply_migrations_up_to(32).expect("v32");
    assert_eq!(c.schema_version().expect("version"), 32);
    assert_eq!(counts(&c), before, "M32 changed rows");
    let indexes: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index'
                AND name IN ('idx_file_rel_path', 'idx_pkgentry_path')",
            [],
            |r| r.get(0),
        )
        .expect("indexes");
    assert_eq!(indexes, 2);
}
