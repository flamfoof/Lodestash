//! Identity follows content (DATA-MODEL §6.1, ADR-0025; v2 plan Task R.3).
//!
//! Every scenario is driven the way the watcher drives a rescan — real files in a temp dir, the
//! real incremental crawl (so a file at its known size and mtime carries no hash), the real
//! persist — and every assertion is read back through a reader of its own. No engine is started
//! and nothing outside the temp dir is touched.
//!
//! The files of one test differ in **size**: a rename keeps a file's mtime, so two files of one
//! size written in the same second and then swapped pass the crawl's `(size, mtime)` gate at both
//! paths. That blind spot is the gate's, it is stated in §6.1, and it is not what is tested here.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use lodestash_core::catalog::{
    Catalog, CatalogReader, HistoryOrigin, TagAssignment, TagBatch, TagBatchItem, TagMode,
};

/// One library folder and its catalog, under a temp dir.
struct Lib {
    _dir: tempfile::TempDir,
    root: PathBuf,
    db: PathBuf,
}

/// A row the grid would show: where it is, which asset it is, and its tags.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Shown {
    path: String,
    id: i64,
    tags: Vec<String>,
}

fn shown(path: &str, id: i64, tags: &[&str]) -> Shown {
    Shown {
        path: path.into(),
        id,
        tags: tags.iter().map(|t| t.to_string()).collect(),
    }
}

impl Lib {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path().join("lib");
        std::fs::create_dir_all(&root).expect("mkdir");
        Lib {
            db: dir.path().join("catalog.db"),
            root,
            _dir: dir,
        }
    }

    fn abs(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    fn write(&self, rel: &str, bytes: &[u8]) {
        let path = self.abs(rel);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        std::fs::write(path, bytes).expect("write");
    }

    /// Rename, replacing whatever is at `to` — what `mv -f` and Explorer's "Replace" do.
    fn rename(&self, from: &str, to: &str) {
        let to = self.abs(to);
        std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
        std::fs::rename(self.abs(from), to).expect("rename");
    }

    fn copy(&self, from: &str, to: &str) {
        let to = self.abs(to);
        std::fs::create_dir_all(to.parent().expect("parent")).expect("mkdir");
        std::fs::copy(self.abs(from), to).expect("copy");
    }

    fn remove(&self, rel: &str) {
        std::fs::remove_file(self.abs(rel)).expect("remove");
    }

    fn root_str(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }

    /// One rescan, through the function the watcher calls. Returns whether it persisted.
    fn rescan(&self) -> bool {
        // The writer's connection, as the engine opens it.
        let mut catalog = Catalog::open(&self.db).expect("open");
        catalog.seed_media_types().expect("seed");
        crate::persist::rescan_source(&mut catalog, &self.root_str(), "lib", &[]).expect("rescan")
    }

    /// A writing connection, for what a person does between scans.
    fn writer(&self) -> Catalog {
        Catalog::open(&self.db).expect("open")
    }

    fn reader(&self) -> CatalogReader {
        Catalog::open_reader(&self.db).expect("reader")
    }

    fn source(&self) -> i64 {
        self.reader()
            .source_info_by_root(&self.root_str())
            .expect("source")
            .expect("the source exists")
            .id
    }

    fn tags_of(&self, id: i64) -> Vec<String> {
        let mut tags: Vec<String> = self
            .reader()
            .tags_for_asset(id)
            .expect("tags")
            .into_iter()
            .map(|t| t.value)
            .collect();
        tags.sort();
        tags
    }

    /// What the grid shows: live rows whose file is on disk, by path.
    fn shown(&self) -> Vec<Shown> {
        let mut rows: Vec<Shown> = self
            .reader()
            .entries_for_source(self.source())
            .expect("entries")
            .into_iter()
            .filter(|e| !e.trashed)
            .map(|e| Shown {
                tags: self.tags_of(e.asset_id),
                path: e.rel_path,
                id: e.asset_id,
            })
            .collect();
        rows.sort();
        rows
    }

    /// Every row of the source — missing, trashed and in-pack ones too — as `(path, asset id)`.
    fn every_row(&self) -> Vec<(String, i64)> {
        let mut rows: Vec<(String, i64)> = Catalog::open(&self.db)
            .expect("open")
            .assets_for_source(self.source())
            .expect("rows")
            .into_iter()
            .map(|a| (a.rel_path, a.asset_id))
            .collect();
        rows.sort();
        rows
    }

    fn id_at(&self, rel: &str) -> i64 {
        self.every_row()
            .into_iter()
            .find(|(p, _)| p == rel)
            .unwrap_or_else(|| panic!("no row at {rel}: {:?}", self.every_row()))
            .1
    }

    /// A person tags the file at `rel`.
    fn tag(&self, rel: &str, value: &str) -> i64 {
        let id = self.id_at(rel);
        self.writer()
            .add_asset_tag(id, "manual", "subject", value)
            .expect("tag");
        id
    }

    /// No two rows of the source on one path — the invariant every scenario also checks.
    fn assert_one_row_per_path(&self) {
        let rows = self.every_row();
        let paths: HashSet<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths.len(), rows.len(), "two rows on one path: {rows:?}");
    }
}

/// `a.png` and `b.png` exchange names between two scans. Each row used to keep its path — and so
/// its tags — while holding the other file's bytes.
#[test]
fn a_swap_keeps_each_files_tags_with_its_bytes() {
    let lib = Lib::new();
    lib.write("a.png", b"alpha");
    lib.write("b.png", b"beta-beta");
    lib.rescan();
    let (a, b) = (lib.tag("a.png", "alpha"), lib.tag("b.png", "beta"));

    lib.rename("a.png", "tmp.png");
    lib.rename("b.png", "a.png");
    lib.rename("tmp.png", "b.png");
    assert!(lib.rescan(), "the swap was not seen");

    assert_eq!(
        lib.shown(),
        [shown("a.png", b, &["beta"]), shown("b.png", a, &["alpha"])],
        "each file's identity went with its bytes"
    );
    assert_eq!(
        lib.every_row().len(),
        2,
        "nothing went missing, nothing new"
    );
    lib.assert_one_row_per_path();
}

/// `a → b → c → a` in one scan: a permutation with no free path anywhere in it.
#[test]
fn a_three_way_rotation_keeps_each_files_tags_with_its_bytes() {
    let lib = Lib::new();
    lib.write("a.png", b"one");
    lib.write("b.png", b"two-two");
    lib.write("dir/c.png", b"three-three-three");
    lib.rescan();
    let (a, b, c) = (
        lib.tag("a.png", "one"),
        lib.tag("b.png", "two"),
        lib.tag("dir/c.png", "three"),
    );

    lib.rename("dir/c.png", "tmp.png");
    lib.rename("b.png", "dir/c.png");
    lib.rename("a.png", "b.png");
    lib.rename("tmp.png", "a.png");
    lib.rescan();

    assert_eq!(
        lib.shown(),
        [
            shown("a.png", c, &["three"]),
            shown("b.png", a, &["one"]),
            shown("dir/c.png", b, &["two"]),
        ]
    );
    assert_eq!(lib.every_row().len(), 3);
    lib.assert_one_row_per_path();
}

/// The review's example: `rock.png` is kept as `rock_old.png` and `rock_v2.png` takes its name.
/// The row at `rock.png` used to keep its tags over the new bytes, the `rock_v2` row went missing
/// with its own, and `rock_old.png` was a new, untagged asset.
#[test]
fn a_rename_onto_a_held_path_keeps_both_identities() {
    let lib = Lib::new();
    lib.write("rock.png", b"the first rock");
    lib.write("rock_v2.png", b"the second rock, larger");
    lib.rescan();
    let (v1, v2) = (
        lib.tag("rock.png", "mossy"),
        lib.tag("rock_v2.png", "sharp"),
    );

    lib.rename("rock.png", "rock_old.png");
    lib.rename("rock_v2.png", "rock.png");
    lib.rescan();

    assert_eq!(
        lib.shown(),
        [
            shown("rock.png", v2, &["sharp"]),
            shown("rock_old.png", v1, &["mossy"]),
        ]
    );
    assert_eq!(lib.every_row().len(), 2, "no row stranded, none invented");
    lib.assert_one_row_per_path();
}

/// A cross-volume move under the watcher: the copy lands (scan), then the original goes (scan).
/// The first scan rightly sees a second file. The second used to leave the tagged row missing
/// and the visible one bare. Everything keyed by the asset comes along: tags, favourite,
/// collection, import record, embedding, full-text index and undo.
#[test]
fn copy_then_delete_across_two_rescans_keeps_the_tagged_identity() {
    let lib = Lib::new();
    lib.write("rock.png", b"a rock");
    lib.write("tree.png", b"a tree, taller");
    lib.rescan();
    let rock = lib.id_at("rock.png");
    let (collection, undo) = {
        let mut w = lib.writer();
        // A person's tag, recorded for undo the way the shell records it.
        let undo = w
            .apply_tag_batch(&TagBatch {
                items: vec![TagBatchItem {
                    asset_id: rock,
                    source: "manual".into(),
                    model_id: None,
                    mode: TagMode::Append,
                    tags: vec![TagAssignment::exact("subject", "boulder")],
                    description: None,
                    expected_revision: None,
                }],
                completes_queue: false,
                history: Some(HistoryOrigin::User),
                receipt: None,
            })
            .expect("tag")
            .op_id
            .expect("logged");
        w.set_favorite(&[rock], true).expect("favourite");
        let collection = w.create_collection("Keepers").expect("collection");
        w.collection_add_asset(collection, rock).expect("member");
        let project = w
            .register_project("C:/dev/Game", None, None)
            .expect("project");
        w.record_imports(
            project,
            &[lodestash_core::catalog::ImportSpec {
                asset_id: rock,
                source_package_id: None,
                dest_path: "Assets/rock.png".into(),
                unity_guid: None,
                with_deps: false,
            }],
            "batch-1",
        )
        .expect("import record");
        let src = lib.source();
        w.upsert_embedding(rock, "test-model", "image", None, Some(src), &[0.5, 0.25])
            .expect("embedding");
        (collection, undo)
    };

    // Scan 1: the copy has landed; the original is still there. Two files, two identities.
    lib.copy("rock.png", "moved/rock.png");
    lib.rescan();
    let copy = lib.id_at("moved/rock.png");
    assert_ne!(
        copy, rock,
        "a copy beside its original is a file of its own"
    );
    assert_eq!(lib.tags_of(copy), Vec::<String>::new());

    // Scan 2: the original is deleted. No file in this scan is read — the copy is unchanged.
    lib.remove("rock.png");
    lib.rescan();

    assert_eq!(
        lib.shown(),
        [
            shown("moved/rock.png", rock, &["boulder"]),
            shown("tree.png", lib.id_at("tree.png"), &[]),
        ],
        "the tagged identity is at the path its bytes are at"
    );
    assert_eq!(
        lib.every_row().len(),
        2,
        "the empty duplicate was not removed: {:?}",
        lib.every_row()
    );
    lib.assert_one_row_per_path();

    let r = lib.reader();
    assert!(r.favorite_ids().expect("favourites").contains(&rock));
    assert_eq!(r.collection_members(collection).expect("members"), [rock]);
    assert!(
        r.import_states().expect("imports").contains_key(&rock),
        "the import record did not follow"
    );
    assert_eq!(
        r.embedding_of(rock).expect("embedding").map(|(m, _)| m),
        Some("test-model".to_string())
    );
    assert_eq!(
        r.search_assets("boulder", 10).expect("search"),
        Some(vec![rock]),
        "full-text search by tag finds the surviving row, and only it"
    );
    drop(r);

    // Undo of the tag edit made before the move still lands on the asset.
    lib.writer().revert_op(undo).expect("undo");
    assert_eq!(lib.tags_of(rock), Vec::<String>::new());
}

/// A copy that is kept beside its original is a second asset, however many scans pass: nothing
/// is merged while both files exist.
#[test]
fn a_copy_kept_beside_its_original_stays_a_second_identity() {
    let lib = Lib::new();
    lib.write("rock.png", b"a rock");
    lib.rescan();
    let rock = lib.tag("rock.png", "boulder");

    lib.copy("rock.png", "backup/rock.png");
    lib.rescan();
    let copy = lib.id_at("backup/rock.png");
    let both = [
        shown("backup/rock.png", copy, &[]),
        shown("rock.png", rock, &["boulder"]),
    ];
    assert_ne!(copy, rock);
    assert_eq!(lib.shown(), both);

    // A later scan that does persist (a new file arrived) still merges nothing.
    lib.write("tree.png", b"a tree, taller");
    lib.rescan();
    assert_eq!(lib.shown().len(), 3);
    assert_eq!(lib.id_at("rock.png"), rock);
    assert_eq!(lib.id_at("backup/rock.png"), copy);
    assert_eq!(lib.tags_of(copy), Vec::<String>::new());
    lib.assert_one_row_per_path();
}

/// Two places a vanished row's bytes could be, and nothing to tell them apart: neither is
/// chosen. The row goes missing with its tags; it is placed when one candidate is left.
#[test]
fn two_equal_candidates_are_left_unmatched() {
    // Across two scans: two untouched copies exist when the original goes.
    let lib = Lib::new();
    lib.write("rock.png", b"a rock");
    lib.rescan();
    let rock = lib.tag("rock.png", "boulder");
    lib.copy("rock.png", "x/one.png");
    lib.copy("rock.png", "y/two.png");
    lib.rescan();
    let (one, two) = (lib.id_at("x/one.png"), lib.id_at("y/two.png"));
    lib.remove("rock.png");
    lib.rescan();
    assert_eq!(
        lib.shown(),
        [shown("x/one.png", one, &[]), shown("y/two.png", two, &[])],
        "one of two equal candidates was chosen"
    );
    assert_eq!(lib.id_at("rock.png"), rock, "the row is kept, missing");
    assert_eq!(lib.tags_of(rock), ["boulder"], "with its tags");
    lib.assert_one_row_per_path();

    // One candidate goes: now there is exactly one place the row's bytes are.
    lib.remove("y/two.png");
    lib.rescan();
    assert_eq!(lib.shown(), [shown("x/one.png", rock, &["boulder"])]);
    lib.assert_one_row_per_path();

    // In one scan: the original is gone and its bytes are at two new paths.
    let lib = Lib::new();
    lib.write("rock.png", b"a rock");
    lib.rescan();
    let rock = lib.tag("rock.png", "boulder");
    lib.copy("rock.png", "x/one.png");
    lib.rename("rock.png", "y/two.png");
    lib.rescan();
    let rows = lib.shown();
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter().all(|r| r.id != rock && r.tags.is_empty()),
        "one of two equal candidates was chosen: {rows:?}"
    );
    assert_eq!(lib.tags_of(rock), ["boulder"]);
    lib.assert_one_row_per_path();
}

/// A duplicate somebody has touched is an asset in its own right: the vanished row does not
/// take it over, and its data is not merged into it.
#[test]
fn a_duplicate_with_user_data_is_never_absorbed() {
    let lib = Lib::new();
    lib.write("rock.png", b"a rock");
    lib.rescan();
    let rock = lib.tag("rock.png", "boulder");
    lib.copy("rock.png", "copy/rock.png");
    lib.rescan();
    let copy = lib.id_at("copy/rock.png");
    lib.writer().set_favorite(&[copy], true).expect("favourite");

    lib.remove("rock.png");
    lib.rescan();

    assert_eq!(
        lib.shown(),
        [shown("copy/rock.png", copy, &[])],
        "the favourited duplicate was absorbed, or took the other row's tags"
    );
    assert_eq!(
        lib.id_at("rock.png"),
        rock,
        "the vanished row is kept, missing"
    );
    assert_eq!(lib.tags_of(rock), ["boulder"]);
    assert!(lib
        .reader()
        .favorite_ids()
        .expect("favourites")
        .contains(&copy));
    lib.assert_one_row_per_path();
}

/// Trash is a decision about an asset and a pack's rows are not files of the folder: neither is
/// moved by a scan, neither is taken over, and neither gives up its path.
#[test]
fn trashed_and_in_pack_rows_are_never_moved_absorbed_or_displaced() {
    use lodestash_core::catalog::{InPackClass, PackageEntryRecord};

    let lib = Lib::new();
    lib.write("binned.png", b"binned");
    lib.write("kept.png", b"kept, longer");
    lib.write("spare.png", b"kept, longer"); // a duplicate of kept.png, about to be binned
    lib.write("mover.png", b"the mover, longest of all");
    lib.rescan();
    let binned = lib.tag("binned.png", "old");
    let kept = lib.tag("kept.png", "keeper");
    let spare = lib.id_at("spare.png");
    let mover = lib.tag("mover.png", "moving");
    lib.writer()
        .set_trashed(&[binned, spare], true)
        .expect("trash");

    // An in-pack asset of the same source whose recorded hash is kept.png's.
    let kept_hash: String = {
        let rows = lib.writer().assets_for_source(lib.source()).expect("rows");
        let row = rows.iter().find(|a| a.asset_id == kept).expect("kept");
        row.content_hash.clone().expect("hashed")
    };
    let in_pack = {
        let mut w = lib.writer();
        let pack_path = lib.abs("pack.unitypackage");
        let pkg = w
            .upsert_package(
                lib.source(),
                &pack_path.to_string_lossy(),
                "pack",
                None,
                None,
            )
            .expect("package");
        let entry = PackageEntryRecord {
            guid: "0123456789abcdef0123456789abcdef".into(),
            pathname: Some("Assets/kept.png".into()),
            tar_offset: Some(512),
            size_bytes: Some(12),
        };
        w.record_package_entries(pkg, std::slice::from_ref(&entry))
            .expect("entries");
        let class = InPackClass {
            name: "kept".into(),
            media_type: "image".into(),
        };
        w.mint_in_pack_assets(pkg, &[(entry.guid.clone(), class)].into_iter().collect())
            .expect("mint");
        let id = w
            .assets_for_source(lib.source())
            .expect("rows")
            .into_iter()
            .find(|a| a.rel_path == "Assets/kept.png")
            .expect("minted")
            .asset_id;
        rusqlite::Connection::open(&lib.db)
            .expect("raw")
            .execute(
                "UPDATE asset SET content_hash = ?2 WHERE id = ?1",
                rusqlite::params![id, kept_hash],
            )
            .expect("hash the in-pack row");
        id
    };

    // The trashed file moves; the tagged original of the trashed spare and of the in-pack row
    // is deleted; and a file is moved over the trashed row's old path.
    lib.rename("binned.png", "elsewhere/binned.png");
    lib.remove("kept.png");
    lib.rename("mover.png", "binned.png");
    lib.rescan();

    let rows = lib.every_row();
    let at = |id: i64| {
        let found = rows.iter().find(|(_, i)| *i == id);
        found.map(|(p, _)| p.as_str()).expect("the row exists")
    };
    assert_eq!(at(binned), "binned.png", "a trashed row was moved");
    assert_eq!(at(spare), "spare.png", "a trashed duplicate was absorbed");
    assert_eq!(at(in_pack), "Assets/kept.png", "an in-pack row was moved");
    assert_eq!(
        at(kept),
        "kept.png",
        "the vanished row took over a trashed or in-pack row"
    );
    assert_eq!(
        at(mover),
        "mover.png",
        "a row was moved onto a path a trashed row holds"
    );
    lib.assert_one_row_per_path();

    let trashed: HashSet<i64> = lib
        .writer()
        .assets_for_source(lib.source())
        .expect("rows")
        .into_iter()
        .filter(|a| a.trashed)
        .map(|a| a.asset_id)
        .collect();
    assert_eq!(trashed, HashSet::from([binned, spare]), "the Trash changed");
    // The moved trashed file is a new asset where it now is; nothing of the Trash went with it.
    let fresh = lib.id_at("elsewhere/binned.png");
    assert!(fresh != binned && lib.tags_of(fresh).is_empty());
    assert_eq!(lib.tags_of(binned), ["old"]);
    assert_eq!(lib.tags_of(kept), ["keeper"]);
    assert_eq!(lib.tags_of(mover), ["moving"]);
}

/// `mv -f rock_v2.png rock.png`: a file is moved over another, whose own bytes are then nowhere.
/// The bytes at `rock.png` are the `rock_v2` asset's, so that asset is what `rock.png` shows; the
/// old row is superseded — kept, hidden, at a path no scan can find — and comes back with its
/// tags if its bytes do.
#[test]
fn a_file_moved_over_another_takes_the_path_and_the_old_row_is_superseded() {
    let lib = Lib::new();
    let first = b"the first rock";
    lib.write("rock.png", first);
    lib.write("rock_v2.png", b"the second rock, larger");
    lib.rescan();
    let (v1, v2) = (
        lib.tag("rock.png", "mossy"),
        lib.tag("rock_v2.png", "sharp"),
    );

    lib.rename("rock_v2.png", "rock.png");
    lib.rescan();

    assert_eq!(lib.shown(), [shown("rock.png", v2, &["sharp"])]);
    let parked = format!(".lodestash-superseded/{v1}/rock.png");
    assert_eq!(
        lib.every_row(),
        [(parked.clone(), v1), ("rock.png".to_string(), v2)],
        "the superseded row is kept, off every real path"
    );
    assert_eq!(lib.tags_of(v1), ["mossy"]);

    // A new file of the mover's old name is a new asset — it revives nothing.
    lib.write("rock_v2.png", b"a third rock, the largest of them all");
    lib.rescan();
    let v3 = lib.id_at("rock_v2.png");
    assert!(v3 != v1 && v3 != v2);
    assert_eq!(lib.id_at(&parked), v1);

    // The first rock's bytes come back somewhere: so does its row, tags and all.
    lib.write("restored/first.png", first);
    lib.rescan();
    assert_eq!(
        lib.shown(),
        [
            shown("restored/first.png", v1, &["mossy"]),
            shown("rock.png", v2, &["sharp"]),
            shown("rock_v2.png", v3, &[]),
        ]
    );
    lib.assert_one_row_per_path();
}

/// A file copied aside and then edited — both between two scans — is still the same asset where
/// it was: the name keeps the identity when no other asset's bytes arrive there. (From the disk
/// this cannot be told from "renamed aside, then a new file of the old name", so it is not
/// guessed; the catalog did the same before.)
#[test]
fn a_file_copied_aside_and_then_edited_keeps_its_identity() {
    let lib = Lib::new();
    lib.write("rock.png", b"a rock");
    lib.rescan();
    let rock = lib.tag("rock.png", "boulder");

    lib.copy("rock.png", "rock_backup.png");
    lib.write("rock.png", b"a rock, reworked");
    lib.rescan();

    let backup = lib.id_at("rock_backup.png");
    assert_eq!(
        lib.shown(),
        [
            shown("rock.png", rock, &["boulder"]),
            shown("rock_backup.png", backup, &[]),
        ]
    );
    let rows = lib.writer().assets_for_source(lib.source()).expect("rows");
    let hash = |id: i64| {
        let row = rows.iter().find(|a| a.asset_id == id).expect("row");
        row.content_hash.clone().expect("hashed")
    };
    assert_ne!(
        hash(rock),
        hash(backup),
        "the edited file took its new hash"
    );
    lib.assert_one_row_per_path();
}

/// A small deterministic generator — the property below needs variety, not a dependency.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// **No sequence of renames within a source puts two rows on one path or loses a tag.**
///
/// Each case: a handful of files with distinct bytes, each tagged with its own tag, then a random
/// run of renames — onto free names and into folders, swaps, three-way rotations, and renames
/// over another file (which destroys that file) — with a rescan after some steps and after the
/// last. After every rescan:
///
/// - no path has two rows, and no row was created or deleted (renames make no new bytes);
/// - every file on disk is shown by the row that was tagged for **its bytes**, with that tag;
/// - every tag is still on the row it was put on, shown or not.
///
/// Declared budget: 20 s (about a hundred small rescans, each opening the catalog file).
#[test]
fn no_sequence_of_renames_puts_two_rows_on_one_path_or_loses_a_tag() {
    const POOL: [&str; 12] = [
        "a.png",
        "b.png",
        "c.png",
        "d.png",
        "e.png",
        "sub/a.png",
        "sub/b.png",
        "sub/f.png",
        "deep/er/a.png",
        "deep/er/g.png",
        "other/h.png",
        "other/c.png",
    ];
    for case in 0..24u64 {
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (case + 1).wrapping_mul(0xD134_2543_DE82_EF95));
        let lib = Lib::new();
        let count = 3 + rng.below(4);

        // `disk`: path → which file (by index) is there now.
        let mut disk: BTreeMap<String, usize> = BTreeMap::new();
        while disk.len() < count {
            let path = POOL[rng.below(POOL.len())].to_string();
            let index = disk.len();
            disk.entry(path).or_insert(index);
        }
        for (path, index) in &disk {
            // Distinct bytes and distinct sizes (the module doc says why).
            lib.write(path, &vec![b'A' + *index as u8; 8 + *index]);
        }
        lib.rescan();
        let mut ids = vec![0i64; count];
        for (path, index) in &disk {
            ids[*index] = lib.tag(path, &format!("tag{index}"));
        }

        let mut log: Vec<String> = Vec::new();
        let check = |disk: &BTreeMap<String, usize>, log: &[String]| {
            let context = format!("case {case} after {log:?}");
            let rows = lib.every_row();
            let paths: HashSet<&str> = rows.iter().map(|(p, _)| p.as_str()).collect();
            assert_eq!(
                paths.len(),
                rows.len(),
                "two rows on one path — {context}: {rows:?}"
            );
            let row_ids: HashSet<i64> = rows.iter().map(|(_, id)| *id).collect();
            assert_eq!(
                row_ids,
                ids.iter().copied().collect::<HashSet<i64>>(),
                "a row was created or deleted — {context}: {rows:?}"
            );
            let want: Vec<Shown> = disk
                .iter()
                .map(|(path, index)| shown(path, ids[*index], &[&format!("tag{index}")]))
                .collect();
            assert_eq!(
                lib.shown(),
                want,
                "a file is not shown by its own row — {context}"
            );
            for (index, id) in ids.iter().enumerate() {
                assert_eq!(
                    lib.tags_of(*id),
                    [format!("tag{index}")],
                    "a tag was lost or moved — {context}"
                );
            }
        };

        let steps = 2 + rng.below(5);
        for step in 0..steps {
            let files: Vec<String> = disk.keys().cloned().collect();
            let pick = |rng: &mut Rng| files[rng.below(files.len())].clone();
            match rng.below(5) {
                // A rename over another file: that file's bytes are gone.
                0 if files.len() >= 3 => {
                    let (from, to) = (pick(&mut rng), pick(&mut rng));
                    if from != to {
                        lib.rename(&from, &to);
                        let moved = disk.remove(&from).expect("from");
                        disk.insert(to.clone(), moved);
                        log.push(format!("{from} over {to}"));
                    }
                }
                1 if files.len() >= 2 => {
                    let (x, y) = (pick(&mut rng), pick(&mut rng));
                    if x != y {
                        lib.rename(&x, "swap.tmp");
                        lib.rename(&y, &x);
                        lib.rename("swap.tmp", &y);
                        let (fx, fy) = (disk[&x], disk[&y]);
                        disk.insert(x.clone(), fy);
                        disk.insert(y.clone(), fx);
                        log.push(format!("swap {x} {y}"));
                    }
                }
                2 if files.len() >= 3 => {
                    let (x, y, z) = (pick(&mut rng), pick(&mut rng), pick(&mut rng));
                    if x != y && y != z && x != z {
                        lib.rename(&z, "rot.tmp");
                        lib.rename(&y, &z);
                        lib.rename(&x, &y);
                        lib.rename("rot.tmp", &x);
                        let (fx, fy, fz) = (disk[&x], disk[&y], disk[&z]);
                        disk.insert(y.clone(), fx);
                        disk.insert(z.clone(), fy);
                        disk.insert(x.clone(), fz);
                        log.push(format!("rotate {x} {y} {z}"));
                    }
                }
                _ => {
                    let from = pick(&mut rng);
                    let to = POOL[rng.below(POOL.len())].to_string();
                    if !disk.contains_key(&to) {
                        lib.rename(&from, &to);
                        let moved = disk.remove(&from).expect("from");
                        disk.insert(to.clone(), moved);
                        log.push(format!("{from} -> {to}"));
                    }
                }
            }
            if step + 1 == steps || rng.below(2) == 0 {
                lib.rescan();
                log.push("scan".into());
                check(&disk, &log);
            }
        }
    }
}
