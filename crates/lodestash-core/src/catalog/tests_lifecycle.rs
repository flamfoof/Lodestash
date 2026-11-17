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
