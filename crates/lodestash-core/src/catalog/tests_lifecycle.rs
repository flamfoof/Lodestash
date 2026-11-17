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

#[test]
fn the_newest_two_backups_are_kept() {
    let dir = tempfile::tempdir().expect("tmp");
    let db = dir.path().join("catalog.db");
    for from in [SCHEMA_VERSION - 3, SCHEMA_VERSION - 2, SCHEMA_VERSION - 1] {
        remove_catalog(&db);
        file_catalog_at(&db, from);
        drop(Catalog::open(&db).expect("migrate"));
    }
    assert_eq!(
        backups(&db),
        vec![SCHEMA_VERSION - 2, SCHEMA_VERSION - 1],
        "the third backup prunes the oldest"
    );
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

/// `record_crawl` looks every crawled path up in its source. Without an index led by `rel_path`
/// that is a walk of the source per file — 2,000 lookups over a 20k-asset catalog took 4.5 s.
#[test]
fn the_crawl_lookup_searches_by_path() {
    let c = Catalog::open_in_memory().expect("open");
    let plan = query_plan(&c, CRAWL_LOOKUP_SQL, params![1, "a.png"]);
    assert!(
        plan.contains("idx_file_rel_path (rel_path=?)"),
        "the crawl lookup walks instead of searching:\n{plan}"
    );
}

/// Materializing an extracted pack finds each entry by `(package, pathname)`; without an index
/// over both, every file walks the whole pack's manifest.
#[test]
fn the_materialize_lookup_searches_by_package_and_path() {
    let c = Catalog::open_in_memory().expect("open");
    let plan = query_plan(&c, MATERIALIZE_SQL, params![1, "Assets/a.png", "ws/a.png"]);
    assert!(
        plan.contains("idx_pkgentry_path (package_id=? AND pathname=?)"),
        "the materialize lookup walks the pack:\n{plan}"
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
