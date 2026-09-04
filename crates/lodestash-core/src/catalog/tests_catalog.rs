//! The main catalog tests.
//!
//! Split out of `catalog.rs` on 2026-08-22 — a **verbatim move of whole `impl` blocks**, no
//! behaviour change. That file had reached 10,312 lines behind eight `impl Catalog` blocks that
//! already marked these seams. Types, free helpers and constants deliberately stayed in `mod.rs`:
//! they are shared, and the first attempt at this split moved them with whichever block they
//! happened to sit under, which put `CollectionRow` at the bottom of the test module.
//!
//! Privacy still works — a child module can see its ancestors' private items, so `self.conn` and
//! the crate-private helpers resolve exactly as they did when this was one file.

use super::*;

#[test]
fn open_in_memory_applies_migrations() {
    let catalog = Catalog::open_in_memory().expect("catalog open");
    let version: u32 = catalog
        .conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .expect("user_version");
    assert_eq!(version, 23, "all 23 migrations should have run");
}

/// Every queue an appraise pass drains must respect the set the caller named.
///
/// This bug was found four separate times, one queue at a time, and each discovery cost a
/// measurement: an `identify_asset` kick on 64 images ran **20 minutes** while its own tagging
/// took 342 ms, because the audio-tag, visual-embed and audio-embed queues all ignored the scope
/// and happily worked the rest of the library. One test over all four, so a fifth queue added
/// later has an obvious place to be checked.
#[test]
fn every_appraise_queue_respects_an_explicit_scope() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    let mk = |c: &mut Catalog, rel: &str, code: &str| {
        c.record_crawl(
            src,
            None,
            &[CrawlRecord {
                media_type: code.to_string(),
                ..rec(rel, 1, 1)
            }],
        )
        .expect("crawl");
        c.conn
            .query_row("SELECT id FROM asset ORDER BY id DESC LIMIT 1", [], |r| {
                r.get::<_, i64>(0)
            })
            .expect("id")
    };
    let img = mk(&mut c, "a.png", "image");
    let other_img = mk(&mut c, "b.png", "image");
    let snd = mk(&mut c, "a.wav", "audio");
    let other_snd = mk(&mut c, "b.wav", "audio");
    c.conn
        .execute("UPDATE asset SET tags_dirty = 1, embedding_dirty = 1", [])
        .expect("dirty");

    let ids = |v: Vec<VisualEmbedJob>| -> Vec<i64> {
        let mut out: Vec<i64> = v.into_iter().map(|j| j.asset_id).collect();
        out.sort();
        out
    };
    // Scoped to one asset of each kind: the sibling of the same kind must not appear.
    assert_eq!(
        ids(c.assets_needing_tags_within(10, Some(&[img])).unwrap()),
        vec![img]
    );
    assert_eq!(
        ids(c
            .audio_assets_needing_tags_within(10, Some(&[snd]))
            .unwrap()),
        vec![snd]
    );
    assert_eq!(
        ids(c
            .visual_assets_needing_embedding_within(10, Some(&[img]))
            .unwrap()),
        vec![img]
    );
    assert_eq!(
        ids(c
            .audio_assets_needing_embedding_within(10, Some(&[snd]))
            .unwrap()),
        vec![snd]
    );

    // Unscoped still sees everything — the scope must not leak into the general pass. The visual
    // tag queue is deliberately *not* media-filtered (audio falls out later, on `Tagger::supports`),
    // so it returns all four; the audio queues are filtered by `mt.code`.
    assert_eq!(
        ids(c.assets_needing_tags(10).unwrap()),
        vec![img, other_img, snd, other_snd]
    );
    assert_eq!(
        ids(c.audio_assets_needing_tags(10).unwrap()),
        vec![snd, other_snd]
    );

    // And an explicit *empty* set means nothing, never everything.
    for q in [
        c.assets_needing_tags_within(10, Some(&[])).unwrap(),
        c.audio_assets_needing_tags_within(10, Some(&[])).unwrap(),
        c.visual_assets_needing_embedding_within(10, Some(&[]))
            .unwrap(),
        c.audio_assets_needing_embedding_within(10, Some(&[]))
            .unwrap(),
    ] {
        assert!(q.is_empty(), "an empty scope asked for nothing");
    }
}

/// An **Identify** kick must not invalidate a vector it cannot replace.
///
/// `embedding_dirty = 1` means "the bytes changed" (DATA-MODEL §8) and promotion to `identified`
/// needs the flag clear. Raising it on an asset that already has a good embedding made a
/// re-identify *reduce* how identified an asset could be — permanently, on any machine with no
/// working embedder. An asset with **no** vector must still be flagged, or the embed queue
/// (`assets_needing_embedding`, which selects on the flag) would never see it.
#[test]
fn identify_flags_keep_a_good_vector_and_still_queue_a_missing_one() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 2, 2)])
        .expect("crawl");
    let ids: Vec<i64> = c
        .conn
        .prepare("SELECT id FROM asset ORDER BY id")
        .and_then(|mut s| s.query_map([], |r| r.get(0)).and_then(Iterator::collect))
        .expect("ids");
    let (embedded, bare) = (ids[0], ids[1]);
    c.upsert_embedding(embedded, "m", "image", None, None, &[0.1, 0.2])
        .expect("embed");
    c.conn
        .execute("UPDATE asset SET embedding_dirty = 0", [])
        .expect("settle");

    let dirty = |c: &Catalog, id: i64| -> i64 {
        c.conn
            .query_row(
                "SELECT embedding_dirty FROM asset WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .expect("read")
    };

    for id in [embedded, bare] {
        c.mark_for_identify(id).expect("mark");
    }
    assert_eq!(dirty(&c, embedded), 0, "a good vector survives the kick");
    assert_eq!(dirty(&c, bare), 1, "a missing vector still gets queued");

    // Every flag entry point shares the rule — the bulk ones are how a library-wide re-identify
    // would have dirtied thousands of usable vectors at once.
    c.conn
        .execute("UPDATE asset SET embedding_dirty = 0", [])
        .expect("settle");
    c.flag_identify_assets(&[embedded, bare]).expect("flag set");
    assert_eq!(dirty(&c, embedded), 0);
    assert_eq!(dirty(&c, bare), 1);

    c.conn
        .execute("UPDATE asset SET embedding_dirty = 0", [])
        .expect("settle");
    c.flag_identify_all().expect("flag all");
    assert_eq!(dirty(&c, embedded), 0);
    assert_eq!(dirty(&c, bare), 1);

    c.conn
        .execute("UPDATE asset SET embedding_dirty = 0", [])
        .expect("settle");
    c.flag_identify_untagged().expect("flag untagged");
    assert_eq!(dirty(&c, embedded), 0);
    assert_eq!(dirty(&c, bare), 1);
}

/// The viewer's texture lookup must walk **both** hops. Stopping at the first lands on the
/// material — not an image — which the viewer would hand to an image loader and render blank,
/// with no error anywhere. Also checks that an unresolved edge is not followed.
#[test]
fn model_albedo_texture_walks_both_hops_and_ignores_unresolved_edges() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    fn mk(c: &mut Catalog, src: i64, rel: &str, code: &str) -> i64 {
        c.record_crawl(
            src,
            None,
            &[CrawlRecord {
                media_type: code.to_string(),
                ..rec(rel, 1, 1)
            }],
        )
        .expect("crawl");
        c.conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.id=a.primary_file_id
                      WHERE af.rel_path = ?1",
                params![rel],
                |r| r.get::<_, i64>(0),
            )
            .expect("id")
    }
    let model = mk(&mut c, src, "rock.fbx", "model_3d");
    let material = mk(&mut c, src, "rock.mat", "material");
    let texture = mk(&mut c, src, "rock.png", "image");
    let lonely = mk(&mut c, src, "bare.fbx", "model_3d");
    let dead_mat = mk(&mut c, src, "bare.mat", "material");

    let edge = |c: &Catalog, from: i64, to: i64, kind: &str, state: &str| {
        c.conn
            .execute(
                "INSERT INTO asset_relationship
                        (from_asset_id, to_asset_id, kind, source, confidence, state)
                     VALUES (?1, ?2, ?3, 'test', 1.0, ?4)",
                params![from, to, kind, state],
            )
            .expect("edge");
    };
    edge(&c, model, material, "uses_material", "resolved");
    edge(&c, material, texture, "uses_texture", "resolved");
    // `lonely` reaches a material, but that material's texture edge never resolved.
    edge(&c, lonely, dead_mat, "uses_material", "resolved");
    edge(&c, dead_mat, texture, "uses_texture", "missing");

    assert_eq!(
        c.model_albedo_texture(model).expect("query"),
        Some(texture),
        "two resolved hops must reach the image"
    );
    assert_ne!(
        c.model_albedo_texture(model).expect("query"),
        Some(material),
        "must not stop at the material — it is not an image"
    );
    assert_eq!(
        c.model_albedo_texture(lonely).expect("query"),
        None,
        "an unresolved second hop yields no texture"
    );
    assert_eq!(c.model_albedo_texture(texture).expect("query"), None);
}

/// The regression this queue exists for: previews used to be generated only inside the metrics
/// drain, which visits `metrics_dirty = 1` rows. On a library indexed before previews existed
/// every asset is already clean, so nothing was ever revisited and the grid stayed blank
/// forever. A model with no preview must surface here regardless of `metrics_dirty`.
#[test]
fn preview_queue_finds_clean_models_and_walks_past_failures() {
    let mut catalog = Catalog::open_in_memory().expect("catalog open");
    catalog.seed_media_types().expect("seed");
    let source = catalog
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    let model = |rel: &str| CrawlRecord {
        media_type: "model_3d".to_string(),
        ..rec(rel, 1, 1)
    };
    catalog
        .record_crawl(
            source,
            None,
            &[model("a.fbx"), model("b.fbx"), rec("tex.png", 1, 1)],
        )
        .expect("crawl");
    // Exactly what the real catalog looked like: everything already measured.
    catalog
        .conn
        .execute("UPDATE asset SET metrics_dirty = 0", [])
        .expect("clean");

    let ids: Vec<i64> = catalog
        .assets_needing_preview(&[], 10)
        .expect("queue")
        .iter()
        .map(|j| j.asset_id)
        .collect();
    assert_eq!(
        ids.len(),
        2,
        "both clean models queue; the texture does not"
    );

    // A model whose render fails writes no row, so it stays queued. The cursor is what stops
    // it from wedging the queue ahead of everything behind it.
    let after_first = catalog
        .assets_needing_preview(&ids[..1], 10)
        .expect("queue")
        .iter()
        .map(|j| j.asset_id)
        .collect::<Vec<_>>();
    assert_eq!(
        after_first,
        vec![ids[1]],
        "cursor advances past the failure"
    );

    catalog
        .set_vector_preview(ids[0], "<svg/>", 4, "k")
        .expect("store");
    let left: Vec<i64> = catalog
        .assets_needing_preview(&[], 10)
        .expect("queue")
        .iter()
        .map(|j| j.asset_id)
        .collect();
    assert_eq!(left, vec![ids[1]], "a stored preview leaves the queue");
}

#[test]
fn vector_preview_round_trips_and_tracks_staleness() {
    let mut catalog = Catalog::open_in_memory().expect("catalog open");
    catalog.seed_media_types().expect("seed");
    let source = catalog
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    catalog
        .record_crawl(source, None, &[rec("Rock.png", 1, 1)])
        .expect("crawl");
    let asset: i64 = catalog
        .conn
        .query_row("SELECT id FROM asset LIMIT 1", [], |r| r.get(0))
        .expect("asset");

    assert_eq!(catalog.vector_preview(asset).unwrap(), None);
    assert!(!catalog.has_current_preview(asset, "key-a").unwrap());

    let svg = r#"<svg viewBox="0 0 256 256"><g id="v0"/></svg>"#;
    catalog
        .set_vector_preview(asset, svg, 4, "key-a")
        .expect("store");
    assert_eq!(
        catalog.vector_preview(asset).unwrap(),
        Some((svg.to_string(), "key-a".into()))
    );
    assert!(catalog.has_current_preview(asset, "key-a").unwrap());
    // A changed file (or a raised quality) yields a different key, so the drain regenerates
    // rather than serving a preview of the old geometry.
    assert!(!catalog.has_current_preview(asset, "key-b").unwrap());
    assert_eq!(
        catalog.vector_preview_bytes().unwrap(),
        svg.len() as i64,
        "the Storage readout counts catalog preview bytes"
    );

    // Upsert replaces rather than duplicating — one preview per asset, always the current one.
    catalog
        .set_vector_preview(asset, "<svg/>", 1, "key-b")
        .expect("replace");
    assert_eq!(
        catalog.vector_preview(asset).unwrap(),
        Some(("<svg/>".to_string(), "key-b".into()))
    );
    assert_eq!(catalog.vector_preview_bytes().unwrap(), 6);

    // Previews are derived data: deleting the asset must not leave an orphan row behind.
    catalog
        .conn
        .execute("DELETE FROM asset WHERE id = ?1", params![asset])
        .expect("delete asset");
    assert_eq!(catalog.vector_preview(asset).unwrap(), None);
    assert_eq!(catalog.vector_preview_bytes().unwrap(), 0);
}

/// A catalog stopped at a given schema version, for exercising one migration's data steps
/// against rows that predate it.
///
/// This, not rewinding `user_version` on a fully-migrated database: a rewind replays **every**
/// later migration, so the first one carrying DDL fails on the second pass (`ALTER TABLE … ADD
/// COLUMN` is not idempotent) and the test breaks the day an unrelated migration lands.
fn catalog_at(version: u32) -> Catalog {
    let conn = Connection::open_in_memory().expect("open");
    configure_connection(&conn).expect("pragmas");
    let mut c = Catalog {
        conn,
        appraise_focus: AppraiseFocus::default(),
    };
    c.apply_migrations_up_to(version)
        .unwrap_or_else(|e| panic!("migrations to v{version}: {e}"));
    c
}

/// A catalog stopped at schema v4, for exercising M5's data steps.
fn catalog_at_v4() -> Catalog {
    catalog_at(4)
}

#[test]
fn m5_reassigns_legacy_tray_to_active_project() {
    let mut c = catalog_at_v4();
    c.conn
        .execute_batch(
            "INSERT INTO project (id, uuid, name, project_path, is_active)
                 VALUES (1, 'p1', 'Game', 'C:/game', 1);
                 INSERT INTO collection (id, uuid, name, is_staging)
                 VALUES (10, 'c1', 'Staging', 1);",
        )
        .expect("fixture");
    c.apply_migrations().expect("M5");

    let project_id: Option<i64> = c
        .conn
        .query_row("SELECT project_id FROM collection WHERE id=10", [], |r| {
            r.get(0)
        })
        .expect("tray survives");
    assert_eq!(
        project_id,
        Some(1),
        "legacy tray reassigned to active project"
    );
}

#[test]
fn m5_deletes_orphan_legacy_tray_when_no_active_project() {
    let mut c = catalog_at_v4();
    c.conn
        .execute_batch(
            "INSERT INTO collection (id, uuid, name, is_staging)
                 VALUES (10, 'c1', 'Staging', 1);",
        )
        .expect("fixture");
    c.apply_migrations().expect("M5");

    let n: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM collection WHERE is_staging=1",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(n, 0, "orphan legacy tray deleted");
}

#[test]
fn m5_enforces_one_tray_per_project() {
    let c = Catalog::open_in_memory().expect("open");
    c.conn
            .execute_batch(
                "INSERT INTO project (id, uuid, name, project_path) VALUES (1, 'p1', 'A', '/a');
                 INSERT INTO collection (uuid, name, is_staging, project_id) VALUES ('c1', 'S', 1, 1);",
            )
            .expect("first tray");
    let dup = c.conn.execute(
        "INSERT INTO collection (uuid, name, is_staging, project_id) VALUES ('c2', 'S2', 1, 1)",
        [],
    );
    assert!(
        dup.is_err(),
        "second staging tray for the same project rejected"
    );
}

#[test]
fn register_project_upserts_activates_and_dup_guards_imports() {
    let c = Catalog::open_in_memory().expect("open");
    c.conn
            .execute_batch(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                     VALUES (1, 's1', 'Src', 'C:/lib', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at)
                     VALUES (1, 'a1', 1, 1, 'rock', 0, 0), (2, 'a2', 1, 1, 'tree', 0, 0);",
            )
            .expect("fixture");

    // Re-register the same path: an upsert, not a second row; refreshes fields, stays active.
    let p1 = c
        .register_project("C:/dev/MyGame", Some("6000.0.23f1"), Some("urp"))
        .unwrap();
    let p1b = c
        .register_project("C:/dev/MyGame", Some("6000.0.30f1"), Some("urp"))
        .unwrap();
    assert_eq!(p1, p1b, "same path upserts the same row");

    // A different project registering becomes the sole active editor.
    let p2 = c.register_project("D:/other", None, None).unwrap();
    let projects = c.list_projects().unwrap();
    assert_eq!(projects.len(), 2, "two distinct projects");
    let active: Vec<i64> = projects
        .iter()
        .filter(|p| p.is_active)
        .map(|p| p.id)
        .collect();
    assert_eq!(
        active,
        vec![p2],
        "exactly one active — the most recent register"
    );
    assert_eq!(projects[0].name, "other", "name is the last path segment");
    assert_eq!(
        projects
            .iter()
            .find(|p| p.id == p1)
            .unwrap()
            .unity_version
            .as_deref(),
        Some("6000.0.30f1"),
        "upsert refreshed the version"
    );

    let trays: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM collection WHERE is_staging=1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(trays, 2, "one staging tray per registered project");

    let specs = |ids: &[i64]| {
        ids.iter()
            .map(|&asset_id| ImportSpec {
                asset_id,
                source_package_id: None,
                dest_path: format!("Assets/{asset_id}.png"),
                unity_guid: None,
                with_deps: true,
            })
            .collect::<Vec<_>>()
    };

    let first = c.record_imports(p1, &specs(&[1, 2]), "batch-1").unwrap();
    assert!(matches!(
        first[0],
        ImportOutcome::Recorded { asset_id: 1, .. }
    ));
    assert!(matches!(
        first[1],
        ImportOutcome::Recorded { asset_id: 2, .. }
    ));

    // Advisory: both imported for p1, neither for p2.
    assert_eq!(c.already_imported(&[1, 2], p1).unwrap(), vec![1, 2]);
    assert!(c.already_imported(&[1, 2], p2).unwrap().is_empty());

    // Re-import asset 1 into p1: dup-guarded, no second row.
    let again = c.record_imports(p1, &specs(&[1]), "batch-2").unwrap();
    assert_eq!(again, vec![ImportOutcome::AlreadyImported { asset_id: 1 }]);
    let rows: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM import_record WHERE asset_id=1 AND project_id=?1",
            [p1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1, "dup-guard kept it at one row");

    // Same asset into a *different* project is a fresh import, not a dup.
    let cross = c.record_imports(p2, &specs(&[1]), "batch-3").unwrap();
    assert!(matches!(
        cross[0],
        ImportOutcome::Recorded { asset_id: 1, .. }
    ));
}

/// Source availability (UI-SPEC §11.1, M18).
///
/// The three things that would make this dangerous rather than merely wrong, all asserted: a
/// missing root must **not** delete or trash anything, a root that comes back must clear the
/// state again, and `last_seen_at` must keep the last time the root *was* there rather than
/// being stamped while it is gone — "unavailable since" is only honest if it does.
#[test]
fn source_health_marks_a_missing_root_without_touching_its_rows() {
    let dir = tempfile::tempdir().expect("tmp");
    let root = dir.path().join("lib");
    std::fs::create_dir_all(&root).expect("mkdir");
    let root_s = root.to_string_lossy().to_string();

    let c = Catalog::open_in_memory().expect("open");
    c.conn
            .execute_batch(&format!(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                     VALUES (1, 's1', 'Src', '{}', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at)
                     VALUES (1, 'a1', 1, 1, 'one', 0, 0), (2, 'a2', 1, 1, 'two', 0, 0);",
                root_s.replace('\\', "/")
            ))
            .expect("fixture");

    // Present: available, and a first sighting recorded.
    assert!(c.refresh_source_health().unwrap().is_empty());
    let info = c.source_infos().unwrap().remove(0);
    assert_eq!(info.state, "available");
    let seen = info
        .last_seen_at
        .expect("a present root records when it was seen");

    // Gone: unavailable, and the id is reported as moved.
    std::fs::remove_dir_all(&root).expect("rm");
    assert_eq!(c.refresh_source_health().unwrap(), vec![1]);
    let info = c.source_infos().unwrap().remove(0);
    assert_eq!(info.state, "unavailable");
    assert_eq!(
        info.last_seen_at,
        Some(seen),
        "last_seen_at moved while the root was gone — 'unavailable since' would lie"
    );
    assert_eq!(
        info.asset_count, 2,
        "a missing root removed rows from the catalog — the index must survive the drive"
    );
    let live: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live, 2, "a missing root trashed assets");

    // Back again: available, and reported as moved a second time.
    std::fs::create_dir_all(&root).expect("mkdir");
    assert_eq!(c.refresh_source_health().unwrap(), vec![1]);
    assert_eq!(c.source_infos().unwrap().remove(0).state, "available");
}

/// Appraisal queue order (UI-SPEC §3): on screen → rest of the current filter → everything
/// else, newest first.
///
/// Mutation-checked on the four things that would make the queue lie about following the user:
/// the three tiers must come back in that order (a queue that merely *contains* the visible
/// asset has not prioritised it); the fallback must be **newest first**, not the `ORDER BY id`
/// this replaced; setting a focus must not change which rows are eligible, only their order (a
/// focus that filtered would strand the rest of the backlog forever); and every queue must move
/// together, because an asset that gets tagged and then waits behind 3,000 others for its
/// embedding is never promoted and never flips on screen.
#[test]
fn appraise_focus_orders_every_queue_without_narrowing_it() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.conn
            .execute_batch(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                     VALUES (1, 's1', 'Src', 'C:/lib', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at,
                                    tags_dirty, embedding_dirty, metrics_dirty, deps_dirty)
                     VALUES (1, 'a1', 1, 1, 'one',   0, 0, 1, 1, 1, 1),
                            (2, 'a2', 1, 1, 'two',   0, 0, 1, 1, 1, 1),
                            (3, 'a3', 1, 1, 'three', 0, 0, 1, 1, 1, 1),
                            (4, 'a4', 1, 1, 'four',  0, 0, 1, 1, 1, 1);
                 INSERT INTO asset_file (id, asset_id, rel_path, ext, missing)
                     VALUES (1, 1, 'one.png', 'png', 0),
                            (2, 2, 'two.png', 'png', 0),
                            (3, 3, 'three.png', 'png', 0),
                            (4, 4, 'four.png', 'png', 0);
                 UPDATE asset SET primary_file_id = id;",
            )
            .expect("fixture");

    let tags = |c: &Catalog| {
        c.assets_needing_tags(10)
            .expect("tags")
            .into_iter()
            .map(|j| j.asset_id)
            .collect::<Vec<_>>()
    };
    let embeds = |c: &Catalog| {
        c.assets_needing_embedding(10)
            .expect("embed")
            .into_iter()
            .map(|j| j.asset_id)
            .collect::<Vec<_>>()
    };
    let metrics = |c: &Catalog| {
        c.assets_needing_metrics(10)
            .expect("metrics")
            .into_iter()
            .map(|j| j.asset_id)
            .collect::<Vec<_>>()
    };

    // No focus: newest first. This is the spec's third tier, and it is a deliberate reversal of
    // the `ORDER BY a.id` that came before — a fresh scan should not start at the oldest asset
    // in the library.
    assert_eq!(
        tags(&c),
        vec![4, 3, 2, 1],
        "the unfocused queue is not newest-first"
    );
    assert_eq!(embeds(&c), vec![4, 3, 2, 1]);
    assert_eq!(metrics(&c), vec![4, 3, 2, 1]);

    // Focus: asset 1 is on screen, 2 is elsewhere in the same filter, 3 and 4 are neither.
    c.set_appraise_focus(AppraiseFocus {
        visible: vec![1],
        filtered: vec![2],
    });
    for (name, got) in [
        ("tags", tags(&c)),
        ("embeddings", embeds(&c)),
        ("metrics", metrics(&c)),
    ] {
        assert_eq!(
            got,
            vec![1, 2, 4, 3],
            "the {name} queue did not follow the user — on screen, then the filter, then newest"
        );
    }

    // Order, not eligibility: every dirty row is still in the queue. A focus that narrowed
    // would leave the rest of the backlog unreachable for as long as the user stayed put.
    assert_eq!(tags(&c).len(), 4, "focusing dropped rows out of the queue");

    // The visible tier keeps its own order, so the first card on screen is appraised first
    // rather than whichever of the visible ones happens to have the highest id.
    c.set_appraise_focus(AppraiseFocus {
        visible: vec![2, 4],
        filtered: Vec::new(),
    });
    let got = tags(&c);
    assert_eq!(
        &got[..2],
        &[4, 2],
        "within one tier the queue must still be newest-first, not id-ascending"
    );

    // Clearing it goes back to newest-first rather than keeping a stale focus alive.
    c.set_appraise_focus(AppraiseFocus::default());
    assert_eq!(
        tags(&c),
        vec![4, 3, 2, 1],
        "a cleared focus still ordered the queue"
    );
}

/// Relink (UI-SPEC §11.1) — the count shown before committing, and the rewrite itself.
///
/// Mutation-checked on the four things that would make a relink dangerous rather than merely
/// wrong: the preview must count the **pack** for an in-pack asset and not its rel_path (every
/// in-pack row would read as unmatched, and the panel would tell the user a good drive strands
/// their library); workspace-extracted rows must be outside both numbers; a pack that does not
/// sit under the old root must be left alone rather than re-anchored to a guess; and no asset
/// row may be deleted or trashed by any of it.
#[test]
fn relink_counts_before_it_commits_and_rewrites_only_paths() {
    let dir = tempfile::tempdir().expect("tmp");
    let old_root = dir.path().join("old");
    let new_root = dir.path().join("new");
    std::fs::create_dir_all(old_root.join("sub")).expect("mkdir old");
    std::fs::create_dir_all(new_root.join("sub")).expect("mkdir new");
    // The new drive holds one of the two loose files, and the pack.
    std::fs::write(new_root.join("sub/here.png"), b"x").expect("w");
    std::fs::write(new_root.join("kit.unitypackage"), b"x").expect("w");
    let old_pack = old_root.join("kit.unitypackage");
    let outsider = dir.path().join("elsewhere.unitypackage");

    let c = Catalog::open_in_memory().expect("open");
    c.conn
            .execute_batch(&format!(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                     VALUES (1, 's1', 'Src', '{old}', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO package (id, uuid, source_id, name, package_path)
                     VALUES (1, 'p1', 1, 'Kit', '{pack}'),
                            (2, 'p2', 1, 'Stray', '{stray}');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at, package_id)
                     VALUES (1, 'a1', 1, 1, 'here', 0, 0, NULL),
                            (2, 'a2', 1, 1, 'gone', 0, 0, NULL),
                            (3, 'a3', 1, 1, 'inpack', 0, 0, 1),
                            (4, 'a4', 1, 1, 'unpacked', 0, 0, 1);
                 INSERT INTO package_entry (id, package_id, guid, pathname)
                     VALUES (1, 1, 'g1', 'Assets/inpack.png');
                 INSERT INTO asset_file (id, asset_id, rel_path, package_entry_id, workspace_rel)
                     VALUES (1, 1, 'sub/here.png', NULL, NULL),
                            (2, 2, 'sub/gone.png', NULL, NULL),
                            (3, 3, 'Assets/inpack.png', 1, NULL),
                            (4, 4, 'Assets/unpacked.png', NULL, 'ws/unpacked.png');
                 UPDATE asset SET primary_file_id = id;",
                old = old_root.to_string_lossy().replace('\\', "/"),
                pack = old_pack.to_string_lossy().replace('\\', "/"),
                stray = outsider.to_string_lossy().replace('\\', "/"),
            ))
            .expect("fixture");

    // Three assets are the source's to speak for; the workspace-extracted one is not, because
    // moving the root does not move it.
    let p = c.relink_preview(1, &new_root).expect("preview");
    assert_eq!(p.total, 3, "workspace-extracted rows leaked into the count");
    assert_eq!(
        p.matched, 2,
        "expected here.png + the in-pack asset via its archive"
    );

    // The same folder with the pack removed drops exactly the in-pack asset — which is the
    // proof the pack, not `Assets/inpack.png`, is what was checked.
    std::fs::remove_file(new_root.join("kit.unitypackage")).expect("rm");
    assert_eq!(c.relink_preview(1, &new_root).expect("preview").matched, 1);
    std::fs::write(new_root.join("kit.unitypackage"), b"x").expect("w");

    assert_eq!(c.relink_source(1, &new_root).expect("relink"), 1);
    let info = c.source_infos().unwrap().remove(0);
    assert_eq!(info.root_path, new_root.to_string_lossy());
    assert_eq!(info.state, "available");
    assert!(info.last_seen_at.is_some());

    let packs: Vec<String> = {
        let mut s = c
            .conn
            .prepare("SELECT package_path FROM package ORDER BY id")
            .unwrap();
        let v = s
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        v
    };
    assert_eq!(
        PathBuf::from(&packs[0]),
        new_root.join("kit.unitypackage"),
        "the pack under the old root was not re-anchored"
    );
    assert_eq!(
        PathBuf::from(&packs[1]),
        outsider,
        "a pack outside the old root was rewritten to a guess"
    );

    let live: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset WHERE deleted_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(live, 4, "a relink deleted or trashed asset rows");
}

/// The bulk read behind the `✓ in <project>` badge (UI-SPEC §2.5).
///
/// Mutation-checked on the two things that would make the badge lie: it must key on the
/// **active** project (a set that ignores `is_active` would light the badge for assets imported
/// into some other project), and it must come back empty rather than error when no project is
/// active (a library with nothing to compare against draws no badge).
#[test]
fn import_states_distinguishes_active_other_and_stale() {
    let c = Catalog::open_in_memory().expect("open");
    // `import_record.asset_id` is a real foreign key — the assets have to exist first.
    c.conn
            .execute_batch(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                     VALUES (1, 's1', 'Src', 'C:/lib', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at)
                     VALUES (1, 'a1', 1, 1, 'one', 0, 0),
                            (2, 'a2', 1, 1, 'two', 0, 0),
                            (3, 'a3', 1, 1, 'three', 0, 0);",
            )
            .expect("fixture");
    let p1 = c.register_project("C:/dev/GameA", None, None).unwrap();
    let p2 = c.register_project("C:/dev/GameB", None, None).unwrap();
    let specs = |ids: &[i64]| {
        ids.iter()
            .map(|&asset_id| ImportSpec {
                asset_id,
                source_package_id: None,
                dest_path: format!("Assets/{asset_id}.png"),
                unity_guid: None,
                with_deps: true,
            })
            .collect::<Vec<_>>()
    };
    c.record_imports(p1, &specs(&[1, 2]), "b1").unwrap();
    c.record_imports(p2, &specs(&[3]), "b2").unwrap();

    c.set_active_project(p1).unwrap();
    let a = c.import_states().unwrap();
    assert!(
        a[&1].in_active && a[&2].in_active,
        "active project's imports missing"
    );
    assert!(
        !a.get(&3).map(|s| s.in_active).unwrap_or(false),
        "an import into another project lit the badge"
    );
    assert_eq!(a[&3].others, 1, "GameB's import should read as in 1 other");
    assert!(!a[&1].stale, "nothing has changed on disk yet");

    // "Older copy": the source file's mtime moves past the import. This is the only actionable
    // state of the four, so it must come from the two timestamps and nothing softer.
    c.conn
        .execute(
            "INSERT INTO asset_file (asset_id, rel_path, mtime) VALUES (1, 'one.png', ?1)",
            params![i64::MAX],
        )
        .unwrap();
    assert!(
        c.import_states().unwrap()[&1].stale,
        "a newer source file did not read as an older copy"
    );

    // Switching projects re-answers the question — this is why the badge re-paints on switch.
    c.set_active_project(p2).unwrap();
    let b = c.import_states().unwrap();
    assert!(
        b[&3].in_active,
        "GameB's import is not active after the switch"
    );
    assert!(
        !b[&1].in_active && b[&1].others == 1,
        "GameA's import should now count as another project's"
    );

    // No active project: nothing is `in_active`, and it is not an error.
    c.conn
        .execute("UPDATE project SET is_active = 0", [])
        .unwrap();
    let none = c.import_states().unwrap();
    assert!(
        none.values().all(|s| !s.in_active),
        "no active project must read as nothing to compare against"
    );
}

#[test]
fn set_active_project_switches_the_single_active_editor() {
    let c = Catalog::open_in_memory().expect("open");
    let p1 = c.register_project("C:/dev/GameA", None, None).unwrap();
    let p2 = c.register_project("C:/dev/GameB", None, None).unwrap();
    assert!(
        c.list_projects()
            .unwrap()
            .iter()
            .find(|p| p.id == p2)
            .unwrap()
            .is_active
    );

    c.set_active_project(p1).unwrap();
    let projects = c.list_projects().unwrap();
    let active: Vec<i64> = projects
        .iter()
        .filter(|p| p.is_active)
        .map(|p| p.id)
        .collect();
    assert_eq!(active, vec![p1], "exactly one active, switched back to p1");

    assert!(c.set_active_project(999).is_err(), "unknown editor");
}

#[test]
fn m5_backfills_batch_ids_per_project_and_time() {
    let mut c = catalog_at_v4();
    c.conn
            .execute_batch(
                "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 's1', 'Src', 'C:/lib', 'folder');
                 INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
                 INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at)
                 VALUES (1, 'a1', 1, 1, 'rock', 0, 0), (2, 'a2', 1, 1, 'tree', 0, 0);
                 INSERT INTO project (id, uuid, name, project_path) VALUES (1, 'p1', 'Game', '/g');
                 INSERT INTO import_record (asset_id, project_id, dest_path, imported_at)
                 VALUES (1, 1, 'Assets/rock.png', 1000), (2, 1, 'Assets/tree.png', 1000);",
            )
            .expect("fixture");
    c.apply_migrations().expect("M5");

    let ids: Vec<String> = c
        .conn
        .prepare("SELECT batch_id FROM import_record ORDER BY asset_id")
        .expect("prep")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<std::result::Result<_, _>>()
        .expect("rows");
    assert_eq!(ids.len(), 2);
    assert_eq!(
        ids[0], ids[1],
        "same (project, imported_at) group → same batch id"
    );
    assert_eq!(ids[0], "legacy-1-1000");
}

#[test]
fn adopt_config_blacklists_fills_only_empty_matching_sources() {
    let c = Catalog::open_in_memory().expect("open");
    c.conn
        .execute_batch(
            "INSERT INTO import_source (uuid, name, root_path, kind, blacklist) VALUES
                 ('s1', 'A', 'C:/a', 'folder', NULL),
                 ('s2', 'B', 'C:/b', 'folder', '[{\"kind\":\"folder\",\"value\":\"Editor\"}]');",
        )
        .expect("fixture");

    let updated = c
        .adopt_config_blacklists(&[
            (
                "C:/a".into(),
                "[{\"kind\":\"asset\",\"value\":\"x.dll\"}]".into(),
            ),
            (
                "C:/b".into(),
                "[{\"kind\":\"folder\",\"value\":\"Gizmos\"}]".into(),
            ),
            ("C:/missing".into(), "[]".into()),
        ])
        .expect("adopt");
    assert_eq!(updated, 1, "only the empty, matching source is filled");

    let b: String = c
        .conn
        .query_row(
            "SELECT blacklist FROM import_source WHERE root_path='C:/b'",
            [],
            |r| r.get(0),
        )
        .expect("b");
    assert!(b.contains("Editor"), "existing catalog blacklist untouched");
}

fn rec(rel: &str, size: i64, mtime: i64) -> CrawlRecord {
    CrawlRecord {
        workspace_rel: None,
        content_hash: None,
        rel_path: rel.to_string(),
        name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
        media_type: "image".to_string(),
        size_bytes: size,
        mtime,
    }
}

fn rec_h(rel: &str, size: i64, mtime: i64, hash: &str) -> CrawlRecord {
    CrawlRecord {
        workspace_rel: None,
        content_hash: Some(hash.to_string()),
        rel_path: rel.to_string(),
        name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
        media_type: "image".to_string(),
        size_bytes: size,
        mtime,
    }
}

/// `apply_mapping` is deterministic: rules run in priority order, the lowest-priority match wins,
/// and the result never depends on rule *insertion* order.
#[test]
fn mapping_rule_priority_lower_wins_and_is_order_independent() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("barrel.png", 1, 1)])
        .expect("crawl");
    let asset: i64 = c
        .conn
        .query_row("SELECT id FROM asset LIMIT 1", [], |r| r.get(0))
        .expect("asset");
    c.set_tags(
        asset,
        "manual",
        None,
        &[TagAssignment::exact("subject", "barrel")],
    )
    .expect("tag");

    let tax = c.create_taxonomy("Default", true).expect("tax");
    let props = c.add_category(tax, None, "Props", "Props").expect("props");
    let junk = c.add_category(tax, None, "Junk", "Junk").expect("junk");
    // Insert the HIGH-priority-number (loses) rule FIRST, so a correct engine must sort by
    // priority, not insertion/id order.
    c.add_rule(tax, junk, "exact_tag", "subject:barrel", 200)
        .expect("r1");
    c.add_rule(tax, props, "exact_tag", "subject:barrel", 50)
        .expect("r2");

    let n = c.apply_mapping(tax).expect("apply");
    assert_eq!(n, 1, "exactly one winning assignment");
    let cats = c.categories_of(asset, tax).expect("cats");
    assert_eq!(cats.len(), 1, "one category per asset");
    assert_eq!(
        cats[0].1, "Props",
        "priority 50 beats 200 regardless of insert order"
    );
    assert_eq!(cats[0].2, "rule");
}

/// The starter pack creates a usable library and its rules categorize real assets end to end.
#[test]
fn starter_taxonomy_applies_to_real_assets() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    let music = CrawlRecord {
        workspace_rel: None,
        content_hash: None,
        rel_path: "Audio/theme.wav".into(),
        name: "theme.wav".into(),
        media_type: "audio".into(),
        size_bytes: 1,
        mtime: 1,
    };
    c.record_crawl(src, None, &[rec("hero.png", 1, 1), music])
        .expect("crawl");
    let hero: i64 = c
            .conn
            .query_row("SELECT a.id FROM asset a JOIN asset_file af ON af.asset_id=a.id WHERE af.rel_path GLOB '*hero*'", [], |r| r.get(0))
            .expect("hero");
    let theme: i64 = c
            .conn
            .query_row("SELECT a.id FROM asset a JOIN asset_file af ON af.asset_id=a.id WHERE af.rel_path GLOB '*theme*'", [], |r| r.get(0))
            .expect("theme");
    c.set_tags(
        hero,
        "ai",
        Some("m"),
        &[TagAssignment::exact("usage", "character")],
    )
    .expect("tag hero");
    c.set_tags(
        theme,
        "ai",
        Some("m"),
        &[TagAssignment::exact("usage", "music")],
    )
    .expect("tag theme");

    let tax = c.create_starter_taxonomy().expect("starter");
    assert_eq!(c.active_taxonomy().expect("active"), Some(tax));
    assert!(
        c.list_categories(tax).expect("cats").len() >= 10,
        "a real category tree"
    );

    let n = c.apply_mapping(tax).expect("apply");
    assert!(n >= 2, "both assets categorized");
    assert_eq!(c.categories_of(hero, tax).expect("cats")[0].1, "Characters");
    // The music clip lands in Audio/Music (alias, prio 100), not the coarse Audio (media_type 200).
    assert_eq!(
        c.categories_of(theme, tax).expect("cats")[0].1,
        "Audio/Music"
    );
}

/// A pinned manual assignment survives a re-apply; non-pinned rule rows regenerate. Also proves
/// `alias`, `media_type`, and `glob_path` match, and that a pinned asset is not re-claimed by a rule.
#[test]
fn mapping_pinned_survives_reapply_and_match_types_work() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    let sfx = CrawlRecord {
        workspace_rel: None,
        content_hash: None,
        rel_path: "Audio/step.wav".into(),
        name: "step.wav".into(),
        media_type: "audio".into(),
        size_bytes: 1,
        mtime: 1,
    };
    c.record_crawl(src, None, &[rec("props/barrel.png", 1, 1), sfx])
        .expect("crawl");
    let by_name = |c: &Catalog, like: &str| -> i64 {
        c.conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.asset_id=a.id
                     WHERE af.rel_path GLOB ?1 LIMIT 1",
                params![like],
                |r| r.get(0),
            )
            .expect("asset by path")
    };
    let barrel = by_name(&c, "*barrel*");
    let step = by_name(&c, "*step*");
    c.set_tags(
        barrel,
        "manual",
        None,
        &[TagAssignment::exact("subject", "barrel")],
    )
    .expect("tag");

    let tax = c.create_taxonomy("Default", true).expect("tax");
    let props = c.add_category(tax, None, "Props", "Props").expect("props");
    let sfx_cat = c.add_category(tax, None, "SFX", "SFX").expect("sfx");
    let manual_cat = c.add_category(tax, None, "Hero", "Hero").expect("hero");
    // alias (bare value) matches the namespaced tag; media_type matches the wav.
    c.add_rule(tax, props, "alias", "barrel", 100)
        .expect("r-alias");
    c.add_rule(tax, sfx_cat, "media_type", "audio", 100)
        .expect("r-media");

    // Pin the barrel to a manual category *before* applying — the rule must not override it.
    c.assign_category_manual(barrel, manual_cat, tax)
        .expect("pin");

    c.apply_mapping(tax).expect("apply 1");
    let barrel_cats = c.categories_of(barrel, tax).expect("cats");
    assert_eq!(
        barrel_cats.len(),
        1,
        "pinned asset keeps only its manual row"
    );
    assert_eq!(barrel_cats[0].1, "Hero");
    assert!(barrel_cats[0].3, "row is pinned");
    assert_eq!(
        c.categories_of(step, tax).expect("cats")[0].1,
        "SFX",
        "media_type rule matched"
    );

    // Re-map: bump version, re-apply. Pinned row survives; the SFX rule row regenerates.
    c.bump_taxonomy_version(tax).expect("bump");
    c.apply_mapping(tax).expect("apply 2");
    let barrel_cats = c.categories_of(barrel, tax).expect("cats");
    assert_eq!(barrel_cats.len(), 1);
    assert_eq!(
        barrel_cats[0].1, "Hero",
        "pinned override survived the re-apply"
    );
    assert_eq!(
        c.categories_of(step, tax).expect("cats").len(),
        1,
        "non-pinned regenerated once"
    );

    // The Library facet map reflects the assignments: barrel→Hero, step→SFX.
    let by_asset = c.all_asset_categories(tax).expect("facet map");
    assert_eq!(
        by_asset.get(&barrel).map(|v| v.as_slice()),
        Some(["Hero".to_string()].as_slice())
    );
    assert_eq!(
        by_asset.get(&step).map(|v| v.as_slice()),
        Some(["SFX".to_string()].as_slice())
    );
}

/// New assets queue for extraction; storing facets clears the flag and the values reach the
/// read paths; a changed file re-queues.
#[test]
fn metrics_queue_clears_on_write_and_re_dirties_on_change() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("a.png", 1, 1)])
        .expect("crawl");

    let job = c.assets_needing_metrics(10).expect("queue");
    assert_eq!(job.len(), 1, "a fresh asset needs extraction");
    assert_eq!(job[0].disk_path.as_deref(), Some("C:/lib/a.png"));

    let m = AssetMetrics {
        width_px: Some(1024),
        height_px: Some(512),
        ..Default::default()
    };
    c.set_metrics(job[0].asset_id, m).expect("set");
    assert!(c.assets_needing_metrics(10).expect("queue").is_empty());
    assert_eq!(c.entries_for_source(src).expect("hydrate")[0].metrics, m);
    assert_eq!(c.assets_for_source(src).expect("identity")[0].metrics, m);

    // Extraction that finds nothing still clears the flag (forward progress).
    c.record_crawl(src, None, &[rec("a.png", 9, 9)])
        .expect("rescan");
    assert_eq!(c.assets_needing_metrics(10).expect("queue").len(), 1);
    c.set_metrics(job[0].asset_id, AssetMetrics::default())
        .expect("set empty");
    assert!(c.assets_needing_metrics(10).expect("queue").is_empty());
}

#[test]
fn asset_summaries_keep_request_order_and_drop_misses() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("a.png", 10, 1), rec("b.png", 20, 1)])
        .expect("crawl");
    let ids = c.recent_asset_ids(10).expect("recent");
    assert_eq!(ids.len(), 2);

    // Reversed order, plus an id that doesn't exist.
    let want = vec![ids[1], 9999, ids[0]];
    let rows = c.asset_summaries(&want).expect("summaries");
    assert_eq!(rows.len(), 2, "the unknown id is a miss, not an error");
    assert_eq!(rows[0].id, ids[1], "request order is preserved");
    assert_eq!(rows[1].id, ids[0]);
    assert_eq!(rows[0].media_type, "image");
    assert_eq!(rows[0].format.as_deref(), Some("png"));
    assert_eq!(
        rows[0].disk_path.as_deref(),
        Some(format!("C:/lib/{}", rows[0].name).as_str()),
        "the summary carries the primary file's absolute path"
    );
    assert!(rows[0].uuid.len() >= 32, "uuid travels to the agent planes");

    assert!(c.media_type_id("audio").expect("lookup").is_some());
    assert_eq!(c.media_type_id("nope").expect("lookup"), None);
}

#[test]
fn embedding_round_trips_and_prefilters() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 2, 1)])
        .expect("crawl");
    let ids: Vec<i64> = c
        .conn
        .prepare("SELECT id FROM asset ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    let img: i64 = c
        .conn
        .query_row("SELECT id FROM media_type WHERE code='image'", [], |r| {
            r.get(0)
        })
        .expect("image media type");

    c.upsert_embedding(
        ids[0],
        "siglip2",
        "image",
        Some(img),
        Some(src),
        &[1.0, 0.0, 0.0],
    )
    .expect("embed 0");
    c.upsert_embedding(
        ids[1],
        "siglip2",
        "image",
        Some(img),
        Some(src),
        &[0.0, 1.0, 0.0],
    )
    .expect("embed 1");

    // Round-trip: vectors decode exactly, and embedding_dirty was cleared.
    let cands = c.embeddings_for_knn("siglip2", None).expect("knn load");
    assert_eq!(cands.len(), 2);
    let v0 = &cands.iter().find(|(id, _)| *id == ids[0]).unwrap().1;
    assert_eq!(v0, &vec![1.0, 0.0, 0.0]);
    let dirty: i64 = c
        .conn
        .query_row(
            "SELECT embedding_dirty FROM asset WHERE id=?1",
            [ids[0]],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dirty, 0, "upsert clears embedding_dirty");

    // Upsert replaces (same asset+model), doesn't duplicate.
    c.upsert_embedding(
        ids[0],
        "siglip2",
        "image",
        Some(img),
        Some(src),
        &[0.5, 0.5, 0.0],
    )
    .expect("re-embed");
    assert_eq!(c.embeddings_for_knn("siglip2", None).unwrap().len(), 2);

    // Media-type pre-filter: a bogus type yields nothing.
    assert!(c
        .embeddings_for_knn("siglip2", Some(9999))
        .unwrap()
        .is_empty());
    // Soft-deleted assets drop out.
    c.conn
        .execute("UPDATE asset SET deleted_at=1 WHERE id=?1", [ids[0]])
        .unwrap();
    assert_eq!(c.embeddings_for_knn("siglip2", None).unwrap().len(), 1);
}

#[test]
fn hybrid_search_fuses_bm25_and_knn() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(
        src,
        None,
        &[
            rec("rock_wall.png", 1, 1), // BM25-matches "rock"; vector identical to query
            rec("rock_floor.png", 2, 1), // BM25-matches "rock"; vector orthogonal
            rec("tree_oak.png", 3, 1),  // no BM25 match; vector close to query
        ],
    )
    .expect("crawl");
    // Map name → id so assertions don't depend on insertion order.
    let id_of = |name: &str| -> i64 {
        c.conn
            .query_row("SELECT id FROM asset WHERE name=?1", [name], |r| r.get(0))
            .unwrap()
    };
    let (wall, floor, oak) = (
        id_of("rock_wall.png"),
        id_of("rock_floor.png"),
        id_of("tree_oak.png"),
    );
    let img: i64 = c
        .conn
        .query_row("SELECT id FROM media_type WHERE code='image'", [], |r| {
            r.get(0)
        })
        .unwrap();
    c.upsert_embedding(wall, "m", "image", Some(img), Some(src), &[1.0, 0.0, 0.0])
        .unwrap();
    c.upsert_embedding(floor, "m", "image", Some(img), Some(src), &[0.0, 1.0, 0.0])
        .unwrap();
    c.upsert_embedding(oak, "m", "image", Some(img), Some(src), &[0.9, 0.1, 0.0])
        .unwrap();

    // Query "rock" (BM25 → wall, floor) + vector [1,0,0] (KNN → wall, oak, floor).
    let fused = c
        .hybrid_search("rock", &[("m", &[1.0, 0.0, 0.0][..])], None, 10)
        .expect("hybrid");
    assert_eq!(fused[0], wall, "wall wins — high in both retrievers");
    assert!(
        fused.contains(&oak),
        "oak enters via the vector retriever alone"
    );
    assert!(fused.contains(&floor));

    // query_vec = None → pure structural (BM25 only): oak (no name match) is absent.
    let structural = c.hybrid_search("rock", &[], None, 10).expect("structural");
    assert!(!structural.contains(&oak), "no vector → tree not found");
    assert!(structural.contains(&wall) && structural.contains(&floor));
    assert!(c.hybrid_search("rock", &[], None, 0).unwrap().is_empty());
}

#[test]
fn fts_match_query_is_injection_safe() {
    // Every term is prefix-quoted; FTS5 operators in the body are neutralised.
    assert_eq!(fts_match_query("rock"), Some("\"rock\"*".into()));
    assert_eq!(
        fts_match_query("low poly"),
        Some("\"low\"* \"poly\"*".into())
    );
    // Quotes / operators / stars never reach MATCH as syntax.
    assert_eq!(
        fts_match_query("a\" OR b*"),
        Some("\"a\"* \"or\"* \"b\"*".into())
    );
    assert_eq!(fts_match_query(""), None);
    assert_eq!(fts_match_query("  -–—  "), None, "punctuation only → None");
}

#[test]
fn search_assets_ranks_and_prefix_matches() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(
        src,
        None,
        &[
            rec("rock_wall.png", 1, 1),
            rec("rocky_path.png", 2, 1),
            rec("tree_oak.png", 3, 1),
        ],
    )
    .expect("crawl");

    // Prefix match: "rock" finds rock_wall AND rocky_path, not the tree.
    let hits = c.search_assets("rock", 50).expect("search").expect("some");
    assert_eq!(hits.len(), 2, "rock* matches both rock names");
    let tree = c.search_assets("tree", 50).expect("s").expect("some");
    assert_eq!(tree.len(), 1);

    // Empty / punctuation-only → None (caller shows everything).
    assert!(c.search_assets("", 50).expect("s").is_none());
    assert!(c.search_assets("***", 50).expect("s").is_none());

    // Soft-deleted rows drop out of results.
    c.conn
        .execute(
            "UPDATE asset SET deleted_at = 1 WHERE name = 'rock_wall.png'",
            [],
        )
        .expect("trash");
    let hits = c.search_assets("rock", 50).expect("s").expect("some");
    assert_eq!(hits.len(), 1, "trashed asset excluded");
}

/// Perf probe (not an assertion of speed — a measurement): how does the brute-force KNN
/// path scale? Splits the two costs so we optimise the real one instead of guessing —
/// reloading+decoding every BLOB per query vs. the cosine scan + full sort.
/// `cargo test -p lodestash-core perf_probe -- --ignored --nocapture`
#[test]
#[ignore]
fn perf_probe_knn_scaling() {
    use std::time::Instant;
    const DIM: usize = 768;

    for n in [10_000usize, 50_000] {
        let dir = tempfile::tempdir().expect("tmp");
        let mut c = Catalog::open(&dir.path().join("perf.db")).expect("open");
        c.seed_media_types().expect("seed");
        let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");

        let recs: Vec<CrawlRecord> = (0..n)
            .map(|i| {
                let mut r = rec(&format!("a{i}.png"), 1, 1);
                r.media_type = "image".into();
                r
            })
            .collect();
        c.record_crawl(sid, None, &recs).expect("crawl");

        // Deterministic pseudo-random unit-ish vectors.
        let mk = |seed: usize| -> Vec<f32> {
            let mut h = seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
            (0..DIM)
                .map(|_| {
                    h ^= h << 13;
                    h ^= h >> 7;
                    h ^= h << 17;
                    (h as u32 as f32 / u32::MAX as f32) - 0.5
                })
                .collect()
        };
        let t = Instant::now();
        for i in 0..n {
            c.upsert_embedding(i as i64 + 1, "m", "image", Some(1), Some(sid), &mk(i))
                .expect("upsert");
        }
        let seed_time = t.elapsed();

        let t = Instant::now();
        let cands = c.embeddings_for_knn("m", None).expect("load");
        let load = t.elapsed();
        assert_eq!(cands.len(), n);

        let q = mk(12345);
        let t = Instant::now();
        let idx = crate::vector::BruteForceIndex::new(cands);
        let hits =
            <crate::vector::BruteForceIndex as crate::vector::VectorIndex>::knn(&idx, &q, 20);
        let scan = t.elapsed();
        assert_eq!(hits.len(), 20);

        println!(
                "n={n:>6} dim={DIM}  seed={seed_time:>10.2?}  LOAD={load:>10.2?}  SCAN+SORT={scan:>10.2?}  total_query={:?}",
                load + scan
            );
    }
}

#[test]
fn visual_embed_queue_covers_images_models_on_disk_and_extracted() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("src", "G:/src", "folder")
        .expect("sid");
    let pid = c
        .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
        .expect("pid");

    // On-disk image, on-disk 3D model, extracted-package sprite, and audio (NOT visual).
    let mut on_disk = rec("tex/brick.png", 5, 1);
    on_disk.media_type = "image".into();
    let mut model = rec("models/sword.fbx", 8, 1);
    model.media_type = "model_3d".into();
    let mut extracted = rec("Pack/Sprites/hero.png", 6, 1);
    extracted.media_type = "sprite".into();
    extracted.workspace_rel = Some("hash7/hero.png".into());
    let mut audio = rec("sfx/boom.wav", 7, 1);
    audio.media_type = "audio".into();
    c.record_crawl(sid, None, &[on_disk, model, audio])
        .expect("crawl1");
    c.record_crawl(sid, Some(pid), &[extracted])
        .expect("crawl2");

    let jobs = c.visual_assets_needing_embedding(100).expect("queue");
    assert_eq!(jobs.len(), 3, "2 images + 1 model queued; audio excluded");

    let img = jobs
        .iter()
        .find(|j| j.media_code == "image")
        .expect("image job");
    assert_eq!(img.disk_path.as_deref(), Some("G:/src/tex/brick.png"));
    assert!(img.workspace_rel.is_none());

    let m = jobs
        .iter()
        .find(|j| j.media_code == "model_3d")
        .expect("model job");
    assert_eq!(m.disk_path.as_deref(), Some("G:/src/models/sword.fbx"));

    let ext = jobs
        .iter()
        .find(|j| j.workspace_rel.is_some())
        .expect("extracted job");
    assert_eq!(ext.media_code, "sprite");
    assert_eq!(ext.workspace_rel.as_deref(), Some("hash7/hero.png"));
    assert!(
        ext.disk_path.is_none(),
        "extracted files resolve via the workspace, not root"
    );
}

#[test]
fn m6_extracted_rehydration_and_package_entries_round_trip() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("src", "G:/src", "folder")
        .expect("sid");
    let pid = c
        .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
        .expect("pid");

    // Extracted asset: recorded with a workspace_rel, hidden from the normal
    // hydrate, visible to the extracted hydrate with its package name.
    let mut extracted = rec("Pack/Models/Rock.fbx", 10, 1);
    extracted.media_type = "model_3d".into();
    extracted.workspace_rel = Some("abc123/Rock.fbx".into());
    c.record_crawl(sid, Some(pid), &[extracted])
        .expect("record");
    c.record_crawl(sid, None, &[rec("plain.png", 5, 1)])
        .expect("record2");

    let normal = c.entries_for_source(sid).expect("normal");
    assert_eq!(
        normal.len(),
        1,
        "extracted rows stay out of the folder hydrate"
    );
    let ex = c.extracted_entries_for_source(sid).expect("extracted");
    assert_eq!(ex.len(), 1);
    assert_eq!(ex[0].package_name, "Pack");
    assert_eq!(ex[0].workspace_rel, "abc123/Rock.fbx");
    assert_eq!(ex[0].asset.rel_path, "Pack/Models/Rock.fbx");
    assert!(!ex[0].pinned, "packages start unpinned (M7)");

    // M7 pin round-trip: listing + the extracted hydrate both see the flag.
    c.set_package_pinned(pid, true).expect("pin");
    let listing = c.list_packages().expect("list");
    assert_eq!(listing.len(), 1);
    assert!(listing[0].pinned);
    assert_eq!(listing[0].name, "Pack");
    assert!(
        c.extracted_entries_for_source(sid).expect("extracted")[0].pinned,
        "pin visible on rehydrated entries"
    );
    c.set_package_pinned(pid, false).expect("unpin");
    assert!(!c.list_packages().expect("list")[0].pinned);

    // package_entry offsets round-trip + upsert semantics.
    let entries = vec![
        PackageEntryRecord {
            guid: "g1".into(),
            pathname: Some("Assets/Rock.fbx".into()),
            tar_offset: Some(1536),
            size_bytes: Some(4096),
        },
        PackageEntryRecord {
            guid: "g2".into(),
            pathname: None,
            tar_offset: Some(8192),
            size_bytes: Some(100),
        },
    ];
    c.record_package_entries(pid, &entries).expect("entries");
    let mut updated = entries.clone();
    updated[1].tar_offset = Some(9000);
    c.record_package_entries(pid, &updated).expect("upsert");
    // Path-ordered read: NULL pathnames (the .meta-less locators) sort first in SQLite.
    let got = c.list_package_entries(pid).expect("entries read");
    assert_eq!(
        got,
        vec![updated[1].clone(), updated[0].clone()],
        "guid-keyed upsert, path-ordered read"
    );
}

#[test]
fn mint_in_pack_assets_links_locators_and_skips_unclassified() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("src", "G:/src", "asset_store_library")
        .expect("sid");
    let pid = c
        .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
        .expect("pid");

    // Three manifest locators; the caller classifies only two (the third is a .cs script
    // with no media_type — it must never become a grid asset).
    c.record_package_entries(
        pid,
        &[
            PackageEntryRecord {
                guid: "g_rock".into(),
                pathname: Some("Assets/Models/Rock.fbx".into()),
                tar_offset: Some(1536),
                size_bytes: Some(4096),
            },
            PackageEntryRecord {
                guid: "g_tex".into(),
                pathname: Some("Assets/Textures/Rock_Albedo.png".into()),
                tar_offset: Some(8192),
                size_bytes: Some(2048),
            },
            PackageEntryRecord {
                guid: "g_script".into(),
                pathname: Some("Assets/Scripts/Spin.cs".into()),
                tar_offset: Some(16384),
                size_bytes: Some(512),
            },
        ],
    )
    .expect("entries");

    let classified = HashMap::from([
        (
            "g_rock".to_string(),
            InPackClass {
                name: "Rock".into(),
                media_type: "model_3d".into(),
            },
        ),
        (
            "g_tex".to_string(),
            InPackClass {
                name: "Rock_Albedo".into(),
                media_type: "texture".into(),
            },
        ),
    ]);

    let stats = c.mint_in_pack_assets(pid, &classified).expect("mint");
    assert_eq!(stats.inserted, 2, "two classified entries minted");
    // The owning source_id is derived from the package row, not passed in.
    let minted_source: i64 = c
        .conn
        .query_row(
            "SELECT DISTINCT source_id FROM asset WHERE package_id = ?1",
            params![pid],
            |r| r.get(0),
        )
        .expect("minted source");
    assert_eq!(
        minted_source, sid,
        "minted assets belong to the pack's source"
    );

    // Every minted file links its locator and has NO workspace_rel (bytes stay in-pack).
    let linked: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset_file af JOIN package_entry pe
                   ON pe.id = af.package_entry_id
                 WHERE pe.package_id = ?1 AND af.workspace_rel IS NULL",
            params![pid],
            |r| r.get(0),
        )
        .expect("linked count");
    assert_eq!(
        linked, 2,
        "both in-pack files carry a locator, none extracted"
    );

    // The unclassified script produced no asset.
    let script_assets: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset_file WHERE package_entry_id =
                   (SELECT id FROM package_entry WHERE guid = 'g_script')",
            [],
            |r| r.get(0),
        )
        .expect("script count");
    assert_eq!(
        script_assets, 0,
        "unclassified entry never becomes an asset"
    );

    // A minted asset is queued for every drain (default dirty flags).
    let dirty: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset WHERE package_id = ?1
                   AND embedding_dirty = 1 AND tags_dirty = 1 AND deps_dirty = 1",
            params![pid],
            |r| r.get(0),
        )
        .expect("dirty count");
    assert_eq!(dirty, 2, "minted in-pack assets await identify");

    // Idempotent: re-minting the same classification inserts nothing new.
    let again = c.mint_in_pack_assets(pid, &classified).expect("re-mint");
    assert_eq!(again.inserted, 0, "re-import mints nothing twice");
    assert_eq!(again.unchanged, 2);

    // Guard: once the pack has any extracted (workspace_rel) row, minting is skipped wholesale
    // so an extracted pack can't grow duplicate in-pack rows. Add a new classified locator and
    // an extracted row, then confirm the new locator is NOT minted.
    c.record_package_entries(
        pid,
        &[PackageEntryRecord {
            guid: "g_new".into(),
            pathname: Some("Assets/Audio/Hit.wav".into()),
            tar_offset: Some(32768),
            size_bytes: Some(1024),
        }],
    )
    .expect("new entry");
    c.conn
        .execute(
            "INSERT INTO asset_file (asset_id, rel_path, workspace_rel)
                 VALUES ((SELECT id FROM asset WHERE package_id = ?1 LIMIT 1),
                         'Pack/extracted.fbx', 'hash/extracted.fbx')",
            params![pid],
        )
        .expect("extracted row");
    let with_extracted = HashMap::from([(
        "g_new".to_string(),
        InPackClass {
            name: "Hit".into(),
            media_type: "audio".into(),
        },
    )]);
    let skipped = c
        .mint_in_pack_assets(pid, &with_extracted)
        .expect("guarded mint");
    assert_eq!(
        skipped.inserted, 0,
        "an extracted pack does not grow in-pack duplicates"
    );
}

#[test]
fn flag_identify_scopes_untagged_and_selected() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");
    let rec = |p: &str| CrawlRecord {
        rel_path: p.into(),
        name: p.into(),
        media_type: "image".into(),
        size_bytes: 1,
        mtime: 0,
        workspace_rel: None,
        content_hash: None,
    };
    c.record_crawl(sid, None, &[rec("a.png"), rec("b.png"), rec("c.png")])
        .unwrap();
    // Simulate an already-identified library, then tag only 'a'.
    c.conn
        .execute("UPDATE asset SET embedding_dirty = 0, tags_dirty = 0", [])
        .unwrap();
    let a_id: i64 = c
        .conn
        .query_row(
            "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE af.rel_path = 'a.png'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.set_tags(
        a_id,
        "ai",
        None,
        &[TagAssignment {
            namespace: "style".into(),
            value: "foo".into(),
            confidence: None,
        }],
    )
    .unwrap();

    // Unidentified scope: b + c (no tags) re-flagged; a (tagged) left clean.
    assert_eq!(c.flag_identify_untagged().unwrap(), 2);
    let a_dirty: i64 = c
        .conn
        .query_row(
            "SELECT tags_dirty FROM asset WHERE id = ?1",
            params![a_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        a_dirty, 0,
        "a tagged asset is not re-flagged by Unidentified"
    );

    // Selected scope: exactly the given ids.
    c.conn
        .execute("UPDATE asset SET embedding_dirty = 0, tags_dirty = 0", [])
        .unwrap();
    assert_eq!(c.flag_identify_assets(&[a_id]).unwrap(), 1);
    let total_dirty: i64 = c
        .conn
        .query_row("SELECT COUNT(*) FROM asset WHERE tags_dirty = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(total_dirty, 1, "only the selected asset is flagged");
}

#[test]
fn confirm_flow_sets_and_reverts_status_and_reject_stops_matching() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");
    c.record_crawl(
        sid,
        None,
        &[CrawlRecord {
            rel_path: "a.png".into(),
            name: "a.png".into(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 0,
            workspace_rel: None,
            content_hash: None,
        }],
    )
    .unwrap();
    let id: i64 = c
        .conn
        .query_row(
            "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE af.rel_path = 'a.png'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    // An identified asset: embedding present + clean, one tag, facets extracted.
    c.conn
        .execute(
            "UPDATE asset SET embedding_dirty = 0, metrics_dirty = 0, status = 'identified'
                 WHERE id = ?1",
            params![id],
        )
        .unwrap();
    c.conn
        .execute(
            "INSERT INTO asset_embedding
                    (asset_id, model_id, modality, dim, embedding, created_at)
                 VALUES (?1, 'm', 'image', 1, X'00000000', 0)",
            params![id],
        )
        .unwrap();
    c.set_tags(
        id,
        "ai",
        None,
        &[
            TagAssignment {
                namespace: "subject".into(),
                value: "dragon".into(),
                confidence: Some(0.8),
            },
            TagAssignment {
                namespace: "material".into(),
                value: "scales".into(),
                confidence: Some(0.6),
            },
        ],
    )
    .unwrap();

    let status = |c: &Catalog| -> String {
        c.conn
            .query_row("SELECT status FROM asset WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .unwrap()
    };

    // Confirm → confirmed; un-confirm recomputes the earned status (embedding + tag ⇒ identified).
    c.set_confirmed(id, true).unwrap();
    assert_eq!(status(&c), "confirmed");
    c.set_confirmed(id, false).unwrap();
    assert_eq!(status(&c), "identified");

    // Reject a tag: it is gone from the asset and no longer matches FTS.
    assert_eq!(c.remove_asset_tag(id, "subject", "dragon").unwrap(), 1);
    let left = c.tags_for_asset(id).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].value, "scales");
    let hits: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset_fts WHERE asset_fts MATCH 'dragon'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(hits, 0, "rejected term stops matching FTS");
}

#[test]
fn trait_promote_teach_and_unpromote_keeps_assignments() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c.upsert_import_source("s", "G:/s", "folder").expect("sid");
    c.record_crawl(
        sid,
        None,
        &[CrawlRecord {
            rel_path: "a.png".into(),
            name: "a.png".into(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 0,
            workspace_rel: None,
            content_hash: None,
        }],
    )
    .unwrap();
    let id: i64 = c
        .conn
        .query_row(
            "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                 WHERE af.rel_path = 'a.png'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.set_tags(
        id,
        "ai",
        None,
        &[TagAssignment {
            namespace: "material".into(),
            value: "metal".into(),
            confidence: Some(0.7),
        }],
    )
    .unwrap();

    // Promote the in-use tag to a teaching trait scoped to models.
    let tid = c
        .upsert_trait(
            "material",
            "metal",
            Some("model_3d"),
            Some("shiny hard surface"),
            true,
        )
        .unwrap();
    let vocab = c.list_vocabulary().unwrap();
    let row = vocab.iter().find(|v| v.id == tid).unwrap();
    assert!(row.is_trait && row.teach);
    assert_eq!(
        row.usage_count, 1,
        "promotion keeps the existing assignment"
    );
    assert_eq!(row.ai_count, 1);

    // Teaching vocabulary now carries it, scope + usage note intact.
    let teach = c.teaching_traits().unwrap();
    assert_eq!(
        teach,
        vec![(
            "material".to_string(),
            "metal".to_string(),
            Some("model_3d".to_string()),
            Some("shiny hard surface".to_string())
        )]
    );
    // Pause it: excluded from teaching, still a trait.
    c.set_trait_teach(tid, false).unwrap();
    assert!(c.teaching_traits().unwrap().is_empty());

    // Un-promote: trait status gone, but the tag and its assignment survive.
    c.unpromote_trait(tid).unwrap();
    let vocab = c.list_vocabulary().unwrap();
    let row = vocab.iter().find(|v| v.id == tid).unwrap();
    assert!(!row.is_trait);
    assert_eq!(row.usage_count, 1, "un-promote never drops assignments");
}

/// `set_trait_style` interpolates `color` into CSS `var()` on the card — the server must
/// reject anything that is not a palette token or a strict hex, so a hand-edited payload
/// cannot smuggle CSS through it.
#[test]
fn trait_style_rejects_anything_but_the_palette_or_strict_hex() {
    let c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let tid = c
        .upsert_trait("trait", "shiny", None, None, true)
        .expect("trait");

    // The six palette tokens pass.
    for tok in [
        "--ls-type-model",
        "--ls-type-sound",
        "--ls-type-image",
        "--ls-type-material",
        "--ls-type-sprite",
        "--ls-type-font",
    ] {
        c.set_trait_style(tid, Some(tok), None).expect("token ok");
    }
    // Strict hex passes (the legacy egui build sends it).
    c.set_trait_style(tid, Some("#AABBCC"), None)
        .expect("hex ok");
    // Clearing passes.
    c.set_trait_style(tid, None, None).expect("clear ok");

    // Everything else is rejected, not stored — injection attempts included.
    for bad in [
        "red; } body { display: none }",
        "url(javascript:alert(1))",
        "--ls-gold; color: red",
        "RED",
        "#GGHHII",
        "#fff",
        "--ls-type",
    ] {
        assert!(
            c.set_trait_style(tid, Some(bad), None).is_err(),
            "must reject {bad:?}"
        );
    }
}

/// An `alias` rule pattern is user text, so LIKE metacharacters must be literals. Note `_`
/// can't reach `norm_name` — `normalize_tag` turns it into a space — but `%` survives, so that
/// is the case that actually bites.
#[test]
fn alias_rule_treats_like_metacharacters_literally() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 1, 1)])
        .unwrap();
    let ids: Vec<i64> = c
        .conn
        .prepare("SELECT id FROM asset ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    // `50zzz off` must NOT match the rule `50% off` — unescaped, `%` matches any run.
    c.set_tags(
        ids[0],
        "manual",
        None,
        &[TagAssignment::exact("style", "50% off")],
    )
    .unwrap();
    c.set_tags(
        ids[1],
        "manual",
        None,
        &[TagAssignment::exact("style", "50zzz off")],
    )
    .unwrap();

    let tax = c.create_taxonomy("Default", true).expect("tax");
    let cat = c
        .add_category(tax, None, "Low Poly", "Low Poly")
        .expect("cat");
    c.add_rule(tax, cat, "alias", "50% off", 10).expect("rule");
    c.apply_mapping(tax).expect("apply");

    assert_eq!(
        c.categories_of(ids[0], tax).expect("cats").len(),
        1,
        "the literal alias still matches"
    );
    assert!(
        c.categories_of(ids[1], tax).expect("cats").is_empty(),
        "`%` must be a literal percent sign, not a wildcard"
    );
}

/// The alias arm escapes `_` too, not just `%`. `_` can't normally reach `norm_name`
/// (`normalize_tag` turns it into a space), so inject a literal-underscore tag directly to
/// pin the escape: the rule must match the literal `_` and NOT the space variant.
#[test]
fn alias_rule_treats_underscore_literally() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 1, 1)])
        .unwrap();
    let ids: Vec<i64> = c
        .conn
        .prepare("SELECT id FROM asset ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();

    // Asset 0: a tag whose norm_name carries a literal underscore, bypassing normalize_tag
    // (which is exactly why `_` in the wildcard arm is only defensive in real data).
    c.conn
        .execute(
            "INSERT INTO tag (name, norm_name, namespace) VALUES (?1, ?2, ?3)",
            params!["low_poly", "style:low_poly", "style"],
        )
        .unwrap();
    let tag_id: i64 = c
        .conn
        .query_row(
            "SELECT id FROM tag WHERE norm_name = 'style:low_poly'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    c.conn
        .execute(
            "INSERT INTO asset_tag (asset_id, tag_id, source, created_at)
                 VALUES (?1, ?2, 'manual', 0)",
            params![ids[0], tag_id],
        )
        .unwrap();
    // Asset 1: the same word with a space — what normalize_tag produces for "low_poly".
    c.set_tags(
        ids[1],
        "manual",
        None,
        &[TagAssignment::exact("style", "low poly")],
    )
    .unwrap();

    let tax = c.create_taxonomy("Default", true).expect("tax");
    let cat = c
        .add_category(tax, None, "Low Poly", "Low Poly")
        .expect("cat");
    c.add_rule(tax, cat, "alias", "low_poly", 10).expect("rule");
    c.apply_mapping(tax).expect("apply");

    assert_eq!(
        c.categories_of(ids[0], tax).expect("cats").len(),
        1,
        "the literal underscore alias still matches its own tag"
    );
    assert!(
        c.categories_of(ids[1], tax).expect("cats").is_empty(),
        "`_` must be a literal underscore, not a wildcard matching the space variant"
    );
}

#[test]
fn unmaterialize_returns_only_unshared_files_and_reverts_rows() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("s", "G:/s", "asset_store_library")
        .expect("sid");
    let p1 = c
        .upsert_package(sid, "G:/s/P1.unitypackage", "P1", None, None)
        .expect("p1");
    let p2 = c
        .upsert_package(sid, "G:/s/P2.unitypackage", "P2", None, None)
        .expect("p2");
    let entry = |guid: &str, path: &str| PackageEntryRecord {
        guid: guid.into(),
        pathname: Some(path.into()),
        tar_offset: Some(0),
        size_bytes: Some(1),
    };
    let tex = |n: &str| InPackClass {
        name: n.into(),
        media_type: "texture".into(),
    };
    // P1 has A + B; P2 has A too (same content → same workspace_rel).
    c.record_package_entries(
        p1,
        &[entry("g_a", "Assets/A.png"), entry("g_b", "Assets/B.png")],
    )
    .unwrap();
    c.record_package_entries(p2, &[entry("g_c", "Assets/A.png")])
        .unwrap();
    c.mint_in_pack_assets(
        p1,
        &HashMap::from([("g_a".into(), tex("A")), ("g_b".into(), tex("B"))]),
    )
    .unwrap();
    c.mint_in_pack_assets(p2, &HashMap::from([("g_c".into(), tex("A"))]))
        .unwrap();
    // Materialize: A.png in both packs points at the *same* content-addressed file.
    c.materialize_in_pack_assets(
        p1,
        &[
            ("Assets/A.png".into(), "hashA/A.png".into()),
            ("Assets/B.png".into(), "hashB/B.png".into()),
        ],
    )
    .unwrap();
    c.materialize_in_pack_assets(p2, &[("Assets/A.png".into(), "hashA/A.png".into())])
        .unwrap();

    let freed = c.unmaterialize_package(p1).expect("unmaterialize");
    assert_eq!(
        freed,
        vec!["hashB/B.png".to_string()],
        "only B is deletable — A is shared with P2 and must survive"
    );

    // Two assets *inside one pack* that deduped to the same file must still be reclaimed. The
    // sharer has to be outside the pack being unmaterialized; counting a sibling row meant each
    // saw the other as a live user, so neither was returned — and the update cleared both
    // `workspace_rel`s in the next statement, leaking those bytes permanently.
    let p3 = c
        .upsert_package(sid, "G:/s/P3.unitypackage", "P3", None, None)
        .expect("p3");
    c.record_package_entries(
        p3,
        &[entry("g_d", "Assets/D.png"), entry("g_e", "Assets/E.png")],
    )
    .unwrap();
    c.mint_in_pack_assets(
        p3,
        &HashMap::from([("g_d".into(), tex("D")), ("g_e".into(), tex("E"))]),
    )
    .unwrap();
    c.materialize_in_pack_assets(
        p3,
        &[
            ("Assets/D.png".into(), "hashD/D.png".into()),
            ("Assets/E.png".into(), "hashD/D.png".into()),
        ],
    )
    .unwrap();
    assert_eq!(
        c.unmaterialize_package(p3).expect("unmaterialize p3"),
        vec!["hashD/D.png".to_string()],
        "an intra-pack duplicate is still reclaimed, exactly once"
    );
    // P1's rows reverted to in-pack (workspace_rel NULL, locator kept); P2 untouched.
    let p1_extracted: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset a JOIN asset_file af ON af.asset_id=a.id
                 WHERE a.package_id=?1 AND af.workspace_rel IS NOT NULL",
            params![p1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(p1_extracted, 0, "P1 fully un-materialized");
    let p2_extracted: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset a JOIN asset_file af ON af.asset_id=a.id
                 WHERE a.package_id=?1 AND af.workspace_rel IS NOT NULL",
            params![p2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(p2_extracted, 1, "P2's materialized row is untouched");
}

#[test]
fn materialize_fills_workspace_rel_on_minted_rows_no_shadow() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("src", "G:/src", "asset_store_library")
        .expect("sid");
    let pid = c
        .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
        .expect("pid");
    c.set_package_pinned(pid, true).expect("pin");

    c.record_package_entries(
        pid,
        &[
            PackageEntryRecord {
                guid: "g_rock".into(),
                pathname: Some("Assets/Models/Rock.fbx".into()),
                tar_offset: Some(1536),
                size_bytes: Some(4096),
            },
            PackageEntryRecord {
                guid: "g_tex".into(),
                pathname: Some("Assets/Textures/Rock_Albedo.png".into()),
                tar_offset: Some(8192),
                size_bytes: Some(2048),
            },
        ],
    )
    .expect("entries");
    let classified = HashMap::from([
        (
            "g_rock".to_string(),
            InPackClass {
                name: "Rock".into(),
                media_type: "model_3d".into(),
            },
        ),
        (
            "g_tex".to_string(),
            InPackClass {
                name: "Rock_Albedo".into(),
                media_type: "texture".into(),
            },
        ),
    ]);
    c.mint_in_pack_assets(pid, &classified).expect("mint");

    // Both start in-pack (workspace_rel NULL), neither extracted.
    assert_eq!(c.in_pack_entries_for_source(sid).unwrap().len(), 2);
    assert_eq!(c.extracted_entries_for_source(sid).unwrap().len(), 0);

    // Extract only Rock: materialize fills its workspace_rel in place. The pathname with no
    // minted row (a stray extracted file) is reported unmatched so the caller can record_crawl it.
    let matched = c
        .materialize_in_pack_assets(
            pid,
            &[
                ("Assets/Models/Rock.fbx".into(), "h0/Rock.fbx".into()),
                ("Assets/Nope.txt".into(), "h0/Nope.txt".into()),
            ],
        )
        .expect("materialize");
    assert_eq!(
        matched,
        HashSet::from(["Assets/Models/Rock.fbx".to_string()]),
        "only the minted pathname is materialized"
    );

    // Rock flipped in-pack → extracted (no parallel row); the texture stays in-pack.
    let in_pack = c.in_pack_entries_for_source(sid).unwrap();
    let extracted = c.extracted_entries_for_source(sid).unwrap();
    assert_eq!(in_pack.len(), 1, "texture still in-pack");
    assert_eq!(in_pack[0].asset.rel_path, "Assets/Textures/Rock_Albedo.png");
    assert_eq!(extracted.len(), 1, "rock now resolves on disk");
    assert_eq!(extracted[0].workspace_rel, "h0/Rock.fbx");
    assert_eq!(extracted[0].asset.rel_path, "Assets/Models/Rock.fbx");

    // The materialized row keeps its package_entry link (still an in-pack asset, just on disk).
    let still_linked: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset_file
                 WHERE package_entry_id IS NOT NULL AND workspace_rel = 'h0/Rock.fbx'",
            [],
            |r| r.get(0),
        )
        .expect("link count");
    assert_eq!(still_linked, 1, "materialize does not sever the locator");

    // Total rows unchanged: 2 minted, 0 added — the whole point of step 8.
    let total: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset WHERE package_id = ?1",
            params![pid],
            |r| r.get(0),
        )
        .expect("total");
    assert_eq!(total, 2, "no parallel set — one row per file");
}

#[test]
fn in_pack_entries_surface_with_pack_and_locator() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let sid = c
        .upsert_import_source("src", "G:/src", "asset_store_library")
        .expect("sid");
    let pid = c
        .upsert_package(sid, "G:/src/Pack.unitypackage", "Pack", None, None)
        .expect("pid");
    c.record_package_entries(
        pid,
        &[PackageEntryRecord {
            guid: "g_rock".into(),
            pathname: Some("Assets/Models/Rock.fbx".into()),
            tar_offset: Some(1536),
            size_bytes: Some(4096),
        }],
    )
    .expect("entries");
    c.set_package_pinned(pid, true).expect("pin");
    c.mint_in_pack_assets(
        pid,
        &HashMap::from([(
            "g_rock".to_string(),
            InPackClass {
                name: "Rock".into(),
                media_type: "model_3d".into(),
            },
        )]),
    )
    .expect("mint");

    let rows = c.in_pack_entries_for_source(sid).expect("in-pack hydrate");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.package_name, "Pack");
    assert!(
        r.pinned,
        "the pack's pin rides through so the grid can gate on it"
    );
    assert_eq!(r.pack_path, "G:/src/Pack.unitypackage");
    assert_eq!(
        (r.tar_offset, r.size),
        (1536, 4096),
        "locator for on-demand pull"
    );
    assert_eq!(r.asset.rel_path, "Assets/Models/Rock.fbx");
    assert_eq!(r.asset.media_type, "model_3d");

    // An in-pack asset is NOT returned by the plain folder hydrate (package_id excludes it),
    // so it can't become a broken on-disk card.
    assert!(
        c.entries_for_source(sid)
            .expect("folder hydrate")
            .is_empty(),
        "in-pack assets stay out of the folder hydrate"
    );
}

#[test]
fn upsert_import_source_is_idempotent_by_root_path() {
    let c = Catalog::open_in_memory().expect("open");
    let a = c
        .upsert_import_source("Synty", "C:/lib", "folder")
        .expect("insert");
    let b = c
        .upsert_import_source("Renamed", "C:/lib", "folder")
        .expect("reuse");
    assert_eq!(a, b, "same root_path resolves to the same source");
    let name: String = c
        .conn
        .query_row("SELECT name FROM import_source WHERE id=?1", [a], |r| {
            r.get(0)
        })
        .expect("name");
    assert_eq!(name, "Synty", "name only set on insert");
}

#[test]
fn record_crawl_inserts_then_skips_then_updates() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");

    // First crawl: everything new.
    let stats = c
        .record_crawl(
            src,
            None,
            &[rec("rocks/a.png", 10, 100), rec("rocks/b.png", 20, 100)],
        )
        .expect("crawl 1");
    assert_eq!(
        stats,
        CrawlStats {
            inserted: 2,
            updated: 0,
            unchanged: 0
        }
    );

    // Second crawl: one untouched, one changed (new mtime).
    let stats = c
        .record_crawl(
            src,
            None,
            &[rec("rocks/a.png", 10, 100), rec("rocks/b.png", 22, 200)],
        )
        .expect("crawl 2");
    assert_eq!(
        stats,
        CrawlStats {
            inserted: 0,
            updated: 1,
            unchanged: 1
        }
    );

    // The changed file's asset was marked dirty; sizes stored per file row.
    let (size, dirty): (i64, i64) = c
        .conn
        .query_row(
            "SELECT af.size_bytes, a.embedding_dirty
                 FROM asset_file af JOIN asset a ON a.id = af.asset_id
                 WHERE af.rel_path = 'rocks/b.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(size, 22);
    assert_eq!(dirty, 1);

    // Assets carry a primary file and 'discovered' status.
    let n: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset
                 WHERE status='discovered' AND primary_file_id IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(n, 2);
}

#[test]
fn reconcile_moves_repoints_vanished_path_keeping_asset_id() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");

    // First crawl: one file at its old path, hashed.
    c.record_crawl(src, None, &[rec_h("rocks/a.png", 10, 100, "H1")])
        .expect("crawl 1");
    let before: i64 = c
        .conn
        .query_row(
            "SELECT a.id FROM asset a
                 JOIN asset_file af ON af.asset_id = a.id
                 WHERE af.rel_path = 'rocks/a.png'",
            [],
            |r| r.get(0),
        )
        .expect("old row");

    // The file moved to a subfolder with the same bytes: the vanished path + same hash
    // ⇒ same file. The record is consumed (returned), so the caller drops it and no
    // duplicate row is inserted.
    let moved = c
        .reconcile_moves(src, &[rec_h("assets/rocks/a.png", 10, 100, "H1")])
        .expect("moves");
    assert_eq!(moved, vec!["assets/rocks/a.png".to_string()]);
    let stats = c.record_crawl(src, None, &[]).expect("crawl 2");
    assert_eq!(
        stats.inserted, 0,
        "moved record must not insert a duplicate"
    );

    // The SAME asset row was repointed — id, hash and name preserved, path updated.
    let (after, hash, name): (i64, String, String) = c
        .conn
        .query_row(
            "SELECT a.id, a.content_hash, a.name FROM asset a
                 JOIN asset_file af ON af.asset_id = a.id
                 WHERE af.rel_path = 'assets/rocks/a.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .expect("moved row");
    assert_eq!(after, before, "move must preserve asset_id");
    assert_eq!(hash, "H1", "hash survives the move");
    assert_eq!(name, "a.png");
    let gone: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM asset_file WHERE rel_path = 'rocks/a.png'",
            [],
            |r| r.get(0),
        )
        .expect("old gone");
    assert_eq!(gone, 0, "the old path must no longer exist");
}

#[test]
fn assets_for_source_flags_trashed_and_set_trashed_round_trips() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("a.png", 1, 1), rec("b.png", 2, 2)])
        .expect("crawl");
    c.conn
        .execute(
            "UPDATE asset SET status='identified' WHERE name='a.png';",
            [],
        )
        .expect("promote");

    let rows = c.assets_for_source(src).expect("read");
    assert_eq!(rows.len(), 2);
    let a = rows.iter().find(|r| r.rel_path == "a.png").expect("a");
    assert_eq!(a.status, "identified");
    assert!(!a.trashed, "not trashed");

    let b_id = rows
        .iter()
        .find(|r| r.rel_path == "b.png")
        .expect("b")
        .asset_id;
    assert_eq!(c.set_trashed(&[b_id], true).expect("trash"), 1);
    let rows = c.assets_for_source(src).expect("read 2");
    assert!(
        rows.iter()
            .find(|r| r.rel_path == "b.png")
            .expect("b")
            .trashed,
        "flagged"
    );
    // Trashed rows leave the status counts (search, dup groups, staging).
    assert_eq!(
        c.status_counts_for_sources(&[src]).expect("counts").total(),
        1
    );
    // Restore round-trips.
    assert_eq!(c.set_trashed(&[b_id], false).expect("restore"), 1);
    let rows = c.assets_for_source(src).expect("read 3");
    assert!(
        !rows
            .iter()
            .find(|r| r.rel_path == "b.png")
            .expect("b")
            .trashed
    );
    assert_eq!(c.set_trashed(&[], true).expect("noop"), 0);
}

#[test]
fn hydrate_and_missing_reconcile_round_trip() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    c.record_crawl(src, None, &[rec("a.png", 1, 10), rec("sub/b.png", 2, 20)])
        .expect("crawl");

    let rows = c.entries_for_source(src).expect("hydrate");
    assert_eq!(rows.len(), 2);
    let a = rows.iter().find(|r| r.rel_path == "a.png").expect("a");
    assert_eq!(
        (a.media_type.as_str(), a.size_bytes, a.mtime),
        ("image", 1, 10)
    );
    assert_eq!(a.status, "discovered");
    assert!(!a.trashed);
    assert_eq!(a.rating, None);

    // Rating round-trips through both read paths.
    assert_eq!(c.set_rating(&[a.asset_id], Some(4)).expect("rate"), 1);
    let a2 = c
        .entries_for_source(src)
        .expect("hydrate rated")
        .into_iter()
        .find(|r| r.rel_path == "a.png")
        .expect("a");
    assert_eq!(a2.rating, Some(4));
    let read = c.assets_for_source(src).expect("read");
    assert_eq!(
        read.iter()
            .find(|r| r.rel_path == "a.png")
            .expect("a")
            .rating,
        Some(4)
    );
    assert_eq!(c.set_rating(&[a.asset_id], None).expect("clear"), 1);

    // b vanishes from disk → flagged missing, hidden from hydrate, metadata kept.
    let present: std::collections::HashSet<String> = ["a.png".to_string()].into();
    assert_eq!(c.reconcile_missing(src, &present).expect("gone"), (1, 0));
    assert_eq!(c.entries_for_source(src).expect("hydrate 2").len(), 1);

    // b reappears → unflagged, hydrates again.
    let present: std::collections::HashSet<String> =
        ["a.png".to_string(), "sub/b.png".to_string()].into();
    assert_eq!(c.reconcile_missing(src, &present).expect("back"), (0, 1));
    assert_eq!(c.entries_for_source(src).expect("hydrate 3").len(), 2);

    // Extracted package assets are exempt from missing-reconcile and hydrate.
    let pkg = c
        .upsert_package(src, "C:/lib/p.unitypackage", "p", None, None)
        .expect("pkg");
    c.record_crawl(src, Some(pkg), &[rec("p/x.png", 3, 30)])
        .expect("pkg crawl");
    assert_eq!(
        c.reconcile_missing(src, &present).expect("exempt"),
        (0, 0),
        "package asset not flagged despite being absent from the crawl"
    );
    assert_eq!(
        c.entries_for_source(src).expect("hydrate 4").len(),
        2,
        "package assets stay out of the folder hydrate"
    );
}

#[test]
fn upsert_package_and_unpack_stamp() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Store", "C:/cache", "asset_store_library")
        .expect("src");

    // Browse-only: row exists, unpack fields NULL, idempotent by (source, path).
    let p1 = c
        .upsert_package(
            src,
            "C:/cache/Synty/3D/POLYGON City.unitypackage",
            "POLYGON City",
            Some("Synty"),
            Some("3D"),
        )
        .expect("insert");
    let p2 = c
        .upsert_package(
            src,
            "C:/cache/Synty/3D/POLYGON City.unitypackage",
            "POLYGON City",
            Some("Synty"),
            Some("3D"),
        )
        .expect("reuse");
    assert_eq!(p1, p2);
    let unpacked: Option<i64> = c
        .conn
        .query_row("SELECT unpacked_at FROM package WHERE id=?1", [p1], |r| {
            r.get(0)
        })
        .expect("row");
    assert_eq!(unpacked, None, "browse-only leaves unpacked_at NULL");

    // Ingest: extracted assets carry package_id; unpack stamp fills.
    let stats = c
        .record_crawl(src, Some(p1), &[rec("POLYGON City/rock.png", 5, 50)])
        .expect("extract crawl");
    assert_eq!(stats.inserted, 1);
    c.mark_package_unpacked(p1, 1).expect("stamp");
    let (pkg_of_asset, count): (i64, i64) = c
        .conn
        .query_row(
            "SELECT a.package_id, p.asset_count FROM asset a
                 JOIN package p ON p.id = a.package_id WHERE a.name='rock.png'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("joined");
    assert_eq!(pkg_of_asset, p1);
    assert_eq!(count, 1);

    // M13: the same archive found through a *second* source is the same pack, and the
    // second sighting fills in what the first didn't know rather than making a new row.
    let folder = c
        .upsert_import_source("Synty", "C:/cache/Synty", "folder")
        .expect("second source");
    let bare = c
        .upsert_package(
            folder,
            "C:/cache/Synty/3D/Bare.unitypackage",
            "Bare",
            None,
            None,
        )
        .expect("bare insert");
    let again = c
        .upsert_package(
            src,
            "C:/cache/Synty/3D/POLYGON City.unitypackage",
            "POLYGON City",
            Some("Synty"),
            Some("3D"),
        )
        .expect("same archive, other source");
    assert_eq!(again, p1, "one row per archive, whichever source found it");
    c.upsert_package(
        src,
        "C:/cache/Synty/3D/Bare.unitypackage",
        "Bare",
        Some("Synty"),
        Some("3D"),
    )
    .expect("fill");
    let filled: (Option<String>, Option<String>) = c
        .conn
        .query_row(
            "SELECT publisher, category FROM package WHERE id=?1",
            [bare],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("row");
    assert_eq!(
        filled,
        (Some("Synty".into()), Some("3D".into())),
        "a later sighting fills blanks"
    );
    assert_eq!(
        c.list_packages().expect("list").len(),
        2,
        "two archives, two rows"
    );
}

#[test]
fn status_counts_scope_to_sources_and_fold_discovered() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let a = c.upsert_import_source("A", "C:/a", "folder").expect("a");
    let b = c.upsert_import_source("B", "C:/b", "folder").expect("b");
    c.record_crawl(a, None, &[rec("x.png", 1, 1), rec("y.png", 2, 2)])
        .expect("a crawl");
    c.record_crawl(b, None, &[rec("z.png", 3, 3)])
        .expect("b crawl");
    c.conn
        .execute(
            "UPDATE asset SET status='identified' WHERE name='y.png'",
            [],
        )
        .expect("promote");

    let counts = c.status_counts_for_sources(&[a]).expect("counts a");
    assert_eq!(
        counts,
        StatusCounts {
            unidentified: 1,
            identified: 1,
            confirmed: 0
        }
    );
    assert_eq!(counts.total(), 2, "source B excluded");
    assert_eq!(
        c.status_counts_for_sources(&[]).expect("empty"),
        StatusCounts::default()
    );
}

#[test]
fn promote_statuses_walks_the_lifecycle() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
    c.record_crawl(
        s,
        None,
        &[rec("x.png", 1, 1), rec("y.png", 2, 2), rec("z.png", 3, 3)],
    )
    .expect("crawl");
    let id_of = |c: &Catalog, name: &str| -> i64 {
        c.conn
            .query_row("SELECT id FROM asset WHERE name=?1", params![name], |r| {
                r.get(0)
            })
            .expect("id")
    };
    let status_of = |c: &Catalog, name: &str| -> String {
        c.conn
            .query_row(
                "SELECT status FROM asset WHERE name=?1",
                params![name],
                |r| r.get(0),
            )
            .expect("status")
    };
    let (x, y, z) = (id_of(&c, "x.png"), id_of(&c, "y.png"), id_of(&c, "z.png"));

    // Nothing drained yet: everything stays `discovered`.
    assert_eq!(c.promote_statuses().expect("noop").len(), 0);
    assert_eq!(status_of(&c, "x.png"), "discovered");

    // Facets drained ⇒ indexed. An embedding alone is *not* enough: the asset is findable
    // but nothing has been said about it, so it stays in the Identify backlog.
    c.conn
        .execute("UPDATE asset SET metrics_dirty = 0", [])
        .expect("metrics drained");
    c.upsert_embedding(y, "m", "image", None, None, &[0.5, 0.5])
        .expect("embed y");
    assert_eq!(
        c.promote_statuses().expect("promote").len(),
        0,
        "metrics-drained but untagged: the return is promoted ids only, and nothing promoted"
    );
    assert_eq!(status_of(&c, "x.png"), "indexed");
    assert_eq!(
        status_of(&c, "y.png"),
        "indexed",
        "an embedding without tags is not identification"
    );
    assert_eq!(status_of(&c, "z.png"), "indexed");

    // Tags on top of the embedding complete it.
    c.set_tags(
        y,
        "ai",
        Some("m"),
        &[TagAssignment::exact("subject", "rock")],
    )
    .expect("tag y");
    assert_eq!(c.promote_statuses().expect("promote with tags").len(), 1);
    assert_eq!(status_of(&c, "y.png"), "identified");
    assert_eq!(c.promote_statuses().expect("idempotent").len(), 0);

    // A confirmed row is human-set and never demoted or re-promoted.
    c.conn
        .execute(
            "UPDATE asset SET status='confirmed', embedding_dirty=1 WHERE id=?1",
            params![z],
        )
        .expect("confirm z");
    // y's bytes changed: dirty embedding drops it back to indexed. Demotions are not
    // promotions — the return stays empty, the status is the signal.
    c.conn
        .execute("UPDATE asset SET embedding_dirty=1 WHERE id=?1", params![y])
        .expect("dirty y");
    assert_eq!(c.promote_statuses().expect("demote").len(), 0);
    assert_eq!(status_of(&c, "y.png"), "indexed");
    assert_eq!(status_of(&c, "z.png"), "confirmed");
    let _ = x;

    assert_eq!(
        c.status_counts_for_sources(&[s]).expect("counts"),
        StatusCounts {
            unidentified: 2,
            identified: 0,
            confirmed: 1
        }
    );
}

#[test]
fn naming_heuristic_candidates_and_idempotent_insert() {
    // §10 step-2 catalog contract: an edge-less model is a candidate, the insert is idempotent,
    // and inserting an edge drops the model out of the candidate set. (The name matching itself
    // lives in the ingest layer and is tested there — `deps::texture_matches_model`.)
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
    let mk = |rel: &str, mt: &str| CrawlRecord {
        workspace_rel: None,
        content_hash: None,
        rel_path: rel.to_string(),
        name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
        media_type: mt.to_string(),
        size_bytes: 1,
        mtime: 1,
    };
    c.record_crawl(
        s,
        None,
        &[
            mk("Chars/Human3.fbx", "model_3d"),
            mk("Chars/Human3_Albedo.png", "image"),
            mk("Chars/Human3_Normal.png", "image"),
            mk("Chars/Rock_Albedo.png", "image"), // unrelated stem
        ],
    )
    .expect("crawl");
    let id = |rel: &str| -> i64 {
        c.conn
            .query_row(
                "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                     WHERE af.rel_path = ?1",
                params![rel],
                |r| r.get(0),
            )
            .expect("id")
    };
    let model = id("Chars/Human3.fbx");
    assert_eq!(
        c.edgeless_models()
            .expect("models")
            .iter()
            .map(|(m, _, _)| *m)
            .collect::<Vec<_>>(),
        vec![model],
        "the model with no edges is the sole candidate"
    );
    assert_eq!(c.inference_textures().expect("tex").len(), 3);

    let (alb, nrm) = (id("Chars/Human3_Albedo.png"), id("Chars/Human3_Normal.png"));
    assert_eq!(
        c.insert_inferred_texture_edges(&[
            (model, alb, "Chars/Human3_Albedo.png"),
            (model, nrm, "Chars/Human3_Normal.png"),
        ])
        .expect("insert"),
        2
    );
    assert!(
        c.edgeless_models().expect("m2").is_empty(),
        "a model that gained edges is no longer a candidate (idempotent re-run)"
    );
    assert_eq!(
        c.insert_inferred_texture_edges(&[(model, alb, "x")])
            .expect("reinsert"),
        0,
        "the UNIQUE conflict makes a re-insert a no-op"
    );

    let mut got: Vec<i64> = c
        .conn
        .prepare(
            "SELECT to_asset_id FROM asset_relationship
                 WHERE from_asset_id = ?1 AND source = 'inferred'",
        )
        .unwrap()
        .query_map(params![model], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    got.sort();
    let mut want = vec![alb, nrm];
    want.sort();
    assert_eq!(
        got, want,
        "exactly the resolved edges, confidence-tagged inferred"
    );
}

#[test]
fn dependency_edges_resolve_now_or_later() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
    c.record_crawl(s, None, &[rec("rock.mat", 1, 1), rec("albedo.png", 2, 2)])
        .expect("crawl");
    let id_of = |c: &Catalog, name: &str| -> i64 {
        c.conn
            .query_row("SELECT id FROM asset WHERE name=?1", params![name], |r| {
                r.get(0)
            })
            .expect("id")
    };
    let (mat, tex) = (id_of(&c, "rock.mat"), id_of(&c, "albedo.png"));
    let tex_guid = "b24b57e0cfed05643be34b0256df875e".to_string();
    let gone_guid = "ffffffffffffffffffffffffffffffff".to_string();

    // The material is scanned first — its target has no GUID yet, so the edge is `missing`.
    c.record_dependencies(
        mat,
        Some("1111"),
        "uses_texture",
        std::slice::from_ref(&tex_guid),
    )
    .expect("mat deps");
    let edge = |c: &Catalog| -> (Option<i64>, String) {
        c.conn
            .query_row(
                "SELECT to_asset_id, state FROM asset_relationship WHERE from_asset_id=?1",
                params![mat],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("edge")
    };
    assert_eq!(edge(&c), (None, "missing".into()));
    assert_eq!(c.resolve_pending_relationships().expect("early"), 0);

    // The texture lands later; one statement late-binds the edge.
    c.record_dependencies(tex, Some(&tex_guid), "uses_texture", &[])
        .expect("tex deps");
    assert_eq!(c.resolve_pending_relationships().expect("late"), 1);
    assert_eq!(edge(&c), (Some(tex), "resolved".into()));

    // A rescan replaces the harvest wholesale rather than duplicating it, and a reference to
    // something that isn't in the library stays honestly unresolved.
    c.record_dependencies(mat, Some("1111"), "uses_texture", &[tex_guid, gone_guid])
        .expect("rescan");
    let counts: (i64, i64) = c
        .conn
        .query_row(
            "SELECT COUNT(*), SUM(to_asset_id IS NULL) FROM asset_relationship
                 WHERE from_asset_id=?1",
            params![mat],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .expect("counts");
    assert_eq!(counts, (2, 1), "two edges, one still unresolved");

    // Every scanned asset is off the queue.
    let dirty: i64 = c
        .conn
        .query_row("SELECT COUNT(*) FROM asset WHERE deps_dirty=1", [], |r| {
            r.get(0)
        })
        .expect("dirty");
    assert_eq!(dirty, 0);
}

#[test]
fn prefab_edges_rebuild_and_extend_the_closure() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
    c.record_crawl(
        s,
        None,
        &[
            rec("hero.fbx", 1, 1),
            rec("hero.mat", 2, 2),
            rec("t.png", 3, 3),
        ],
    )
    .expect("crawl");
    let id_of = |c: &Catalog, name: &str| -> i64 {
        c.conn
            .query_row("SELECT id FROM asset WHERE name=?1", params![name], |r| {
                r.get(0)
            })
            .expect("id")
    };
    let (mesh, mat, tex) = (
        id_of(&c, "hero.fbx"),
        id_of(&c, "hero.mat"),
        id_of(&c, "t.png"),
    );
    c.record_dependencies(mesh, Some("meshguid"), "uses_texture", &[])
        .expect("mesh");
    c.record_dependencies(tex, Some("texguid"), "uses_texture", &[])
        .expect("tex");
    c.record_dependencies(mat, Some("matguid"), "uses_texture", &["texguid".into()])
        .expect("mat");

    // A prefab binds the mesh to the material; the mesh's closure now reaches the texture
    // through it — which is the whole point of the pass.
    let written = c
        .replace_prefab_relationships(&[
            ("meshguid".into(), "matguid".into()),
            ("nosuchmesh".into(), "matguid".into()),
        ])
        .expect("prefab edges");
    assert_eq!(written, 1, "a pair whose mesh isn't catalogued is dropped");
    assert_eq!(
        c.dependency_closure(&[mesh]).expect("closure"),
        vec![mesh, mat, tex],
        "mesh → material → texture, transitively"
    );

    // Rebuilding replaces rather than accumulates, and the `unity_meta` edges survive it.
    c.replace_prefab_relationships(&[("meshguid".into(), "matguid".into())])
        .expect("rebuild");
    let by_source: Vec<(String, i64)> = c
        .conn
        .prepare("SELECT source, COUNT(*) FROM asset_relationship GROUP BY 1 ORDER BY 1")
        .and_then(|mut st| {
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<std::result::Result<Vec<_>, _>>()
        })
        .expect("by source");
    assert_eq!(
        by_source,
        vec![
            ("unity_meta".to_string(), 1),
            ("unity_prefab".to_string(), 1)
        ]
    );
}

/// The decisive one: a tag makes an asset findable by a word that appears nowhere in its
/// name or path — which is what `tags_concat` was silently failing to do while the trigger
/// wrote `''`. Re-tagging must also *remove* the old term, or search starts lying.
#[test]
fn tags_are_searchable_and_replaced_per_source() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
    c.record_crawl(s, None, &[rec("asset_0192.fbx", 1, 1)])
        .expect("crawl");
    let id = c
        .conn
        .query_row("SELECT id FROM asset", [], |r| r.get::<_, i64>(0))
        .expect("id");
    let found = |c: &Catalog, q: &str| -> bool {
        !c.search_assets(q, 10)
            .expect("search")
            .unwrap_or_default()
            .is_empty()
    };

    assert!(!found(&c, "beaver"), "nothing to match yet");
    c.set_tags(
        id,
        "ai",
        Some("stub-v1"),
        &[
            TagAssignment {
                namespace: "subject".into(),
                value: "Beaver".into(),
                confidence: Some(1.7), // clamped
            },
            TagAssignment::exact("style", "low_poly"),
        ],
    )
    .expect("ai tags");
    assert!(found(&c, "beaver"), "tag term reaches FTS");
    assert!(found(&c, "low poly"), "underscores fold to spaces");
    assert_eq!(
        c.conn
            .query_row(
                "SELECT confidence FROM asset_tag WHERE confidence IS NOT NULL",
                [],
                |r| r.get::<_, f64>(0)
            )
            .expect("confidence"),
        1.0,
        "confidence clamped into [0,1]"
    );

    // A user tag lands beside the model's, not on top of it.
    c.set_tags(id, "manual", None, &[TagAssignment::exact("mood", "cosy")])
        .expect("manual tag");
    assert!(found(&c, "beaver") && found(&c, "cosy"), "sources coexist");

    // Re-running the tagger replaces only its own tags — and the retired term stops matching.
    c.set_tags(
        id,
        "ai",
        Some("stub-v2"),
        &[TagAssignment::exact("subject", "otter")],
    )
    .expect("retag");
    assert!(found(&c, "otter"), "new term indexed");
    assert!(!found(&c, "beaver"), "old term removed from the index");
    assert!(found(&c, "cosy"), "the manual tag survived the re-run");

    let tags = c.tags_for_asset(id).expect("read back");
    assert_eq!(
        tags.iter()
            .map(|t| (t.namespace.as_str(), t.value.as_str()))
            .collect::<Vec<_>>(),
        vec![("mood", "cosy"), ("subject", "otter")]
    );
    let dirty: i64 = c
        .conn
        .query_row(
            "SELECT tags_dirty FROM asset WHERE id=?1",
            params![id],
            |r| r.get(0),
        )
        .expect("flag");
    assert_eq!(dirty, 0, "tagging clears the queue flag");
}

/// The grid reads tags in bulk, and the filter chips read the vocabulary with counts; both
/// must exclude trashed assets, or the UI offers a facet that filters to nothing.
/// A favourite is a pin, independent of rating and status, and a trashed asset stops being
/// listed as one — it left the grid, so offering it as a favourite would be a dead link.
#[test]
fn favorites_are_independent_of_rating_and_hide_when_trashed() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
    c.record_crawl(s, None, &[rec("a.png", 1, 1), rec("b.png", 2, 2)])
        .expect("crawl");
    let ids: Vec<i64> = c
        .conn
        .prepare("SELECT id FROM asset ORDER BY id")
        .and_then(|mut st| {
            st.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()
        })
        .expect("ids");

    assert!(c.favorite_ids().expect("none yet").is_empty());
    assert_eq!(c.set_favorite(&ids[..1], true).expect("fav"), 1);
    assert_eq!(c.favorite_ids().expect("one"), vec![ids[0]]);

    // Rating the *other* asset must not make it a favourite, and vice versa.
    c.set_rating(&ids[1..], Some(5)).expect("rate");
    assert_eq!(c.favorite_ids().expect("still one"), vec![ids[0]]);

    c.set_trashed(&ids[..1], true).expect("trash");
    assert!(
        c.favorite_ids().expect("after trash").is_empty(),
        "a trashed favourite is not listed"
    );
    c.set_trashed(&ids[..1], false).expect("restore");
    assert_eq!(
        c.favorite_ids().expect("after restore"),
        vec![ids[0]],
        "restoring brings the pin back — it was never cleared"
    );
    assert_eq!(c.set_favorite(&ids[..1], false).expect("unfav"), 1);
    assert!(c.favorite_ids().expect("cleared").is_empty());
}

#[test]
fn bulk_tag_reads_skip_trashed_assets() {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let s = c.upsert_import_source("A", "C:/a", "folder").expect("src");
    c.record_crawl(s, None, &[rec("a.png", 1, 1), rec("b.png", 2, 2)])
        .expect("crawl");
    let ids: Vec<i64> = c
        .conn
        .prepare("SELECT id FROM asset ORDER BY id")
        .and_then(|mut st| {
            st.query_map([], |r| r.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()
        })
        .expect("ids");
    for id in &ids {
        c.set_tags(
            *id,
            "ai",
            Some("m"),
            &[TagAssignment::exact("subject", "rock")],
        )
        .expect("tag");
    }
    assert_eq!(c.all_asset_tags().expect("bulk").len(), 2);
    assert_eq!(
        c.tag_usage(10).expect("usage"),
        vec![("subject".to_string(), "rock".to_string(), 2)]
    );

    c.set_trashed(&[ids[0]], true).expect("trash");
    assert_eq!(
        c.all_asset_tags().expect("bulk after trash").len(),
        1,
        "a trashed asset leaves the grid, so its tags do too"
    );
    assert_eq!(
        c.tag_usage(10).expect("usage after trash")[0].2,
        1,
        "the chip count follows"
    );
}

/// Namespaces share the `tag` table, and `norm_name` is globally UNIQUE — so the same word in
/// two facets must not collapse onto one row.
#[test]
fn the_same_word_in_two_facets_stays_two_tags() {
    assert_eq!(
        normalize_tag("material", "Metal"),
        Some(("material:metal".into(), "Metal".into()))
    );
    assert_eq!(
        normalize_tag("mood", "metal").map(|(n, _)| n),
        Some("mood:metal".into())
    );
    assert_eq!(
        normalize_tag("", "  spaced   out  "),
        Some(("free:spaced out".into(), "spaced out".into())),
        "empty namespace falls back to free; whitespace collapses"
    );
    assert_eq!(normalize_tag("subject", "   "), None);
}

#[test]
fn seed_media_types_idempotent() {
    let catalog = Catalog::open_in_memory().expect("catalog open");
    catalog.seed_media_types().expect("first seed");
    catalog
        .seed_media_types()
        .expect("second seed (idempotent)");

    let count: i64 = catalog
        .conn
        .query_row("SELECT COUNT(*) FROM media_type", [], |r| r.get(0))
        .expect("count");
    assert_eq!(count, 7, "all 7 media types seeded");
}

#[test]
fn foreign_keys_enforced() {
    let catalog = Catalog::open_in_memory().expect("catalog open");
    let result = catalog.conn.execute(
        "INSERT INTO asset (uuid, source_id, media_type_id, name, status, created_at, updated_at)
             VALUES ('abc', 9999, 9999, 'test', 'discovered', 0, 0)",
        [],
    );
    assert!(result.is_err(), "FK violation should be rejected");
}

/// ENGINE-API 5.4 - the advisory fires on a genuine disagreement and stays quiet on every
/// "we don't actually know" case. A false mismatch tells a user their materials will be pink
/// when they will not, which is worse than saying nothing.
#[test]
fn pipeline_mismatch_advisory() {
    let c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind) VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
             INSERT INTO package (id, uuid, source_id, package_path, name, pipeline) VALUES
                 (10, 'p-builtin', 1, '/s/a.unitypackage', 'BuiltInPack', 'built_in'),
                 (11, 'p-urp',     1, '/s/b.unitypackage', 'UrpPack',     'urp'),
                 (12, 'p-mixed',   1, '/s/c.unitypackage', 'MixedPack',   'mixed'),
                 (13, 'p-unknown', 1, '/s/d.unitypackage', 'QuietPack',   NULL);
             INSERT INTO asset (id, uuid, source_id, media_type_id, package_id, name, created_at, updated_at)
                 VALUES (1, 'a1', 1, 1, 10, 'rock',  0, 0),
                        (2, 'a2', 1, 1, 10, 'moss',  0, 0),
                        (3, 'a3', 1, 1, 11, 'tree',  0, 0),
                        (4, 'a4', 1, 1, 12, 'sword', 0, 0),
                        (5, 'a5', 1, 1, 13, 'shard', 0, 0),
                        (6, 'a6', 1, 1, NULL, 'loose', 0, 0);",
        )
        .expect("fixture");
    let urp_project = c
        .register_project("C:/dev/UrpGame", Some("6000.0.30f1"), Some("urp"))
        .unwrap();

    let all = [1, 2, 3, 4, 5, 6];
    let advisories = c.pipeline_mismatches(&all, urp_project).unwrap();
    assert_eq!(
        advisories.len(),
        1,
        "only the Built-in pack disagrees with a URP project: {advisories:?}"
    );
    let a = &advisories[0];
    assert_eq!(a.package, "BuiltInPack");
    assert_eq!(a.package_pipeline, "built_in");
    assert_eq!(a.project_pipeline, "urp");
    assert_eq!(
        a.asset_ids,
        vec![1, 2],
        "grouped per pack - one cause, one sentence, every asset it covers"
    );

    // A project whose pipeline was never reported cannot disagree with anything.
    let unknown_project = c.register_project("C:/dev/Unknown", None, None).unwrap();
    assert!(
        c.pipeline_mismatches(&all, unknown_project)
            .unwrap()
            .is_empty(),
        "unknown project pipeline must not advise"
    );

    // And the matching pack is silent from the other side too.
    let builtin_project = c
        .register_project("C:/dev/BuiltInGame", None, Some("built_in"))
        .unwrap();
    let from_builtin = c.pipeline_mismatches(&all, builtin_project).unwrap();
    assert_eq!(from_builtin.len(), 1);
    assert_eq!(from_builtin[0].package, "UrpPack");

    assert!(
        c.pipeline_mismatches(&[], urp_project).unwrap().is_empty(),
        "no assets, no advisory"
    );
}

/// DATA-MODEL 9 - the licence an asset actually carries, and the reason the URL falls back
/// separately: pairing an asset's own licence with its pack's URL cites the wrong document.
#[test]
fn effective_license_falls_back_to_the_pack() {
    let c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind) VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
             INSERT INTO package (id, uuid, source_id, package_path, name, license, license_url)
                 VALUES (10, 'p1', 1, '/s/a.unitypackage', 'Pack',
                         'Unity Asset Store EULA', 'https://unity.com/legal/as-terms');
             INSERT INTO asset (id, uuid, source_id, media_type_id, package_id, name, license, license_url, created_at, updated_at)
                 VALUES (1, 'a1', 1, 1, 10, 'inherits', NULL,      NULL, 0, 0),
                        (2, 'a2', 1, 1, 10, 'override', 'CC0-1.0', NULL, 0, 0),
                        (3, 'a3', 1, 1, NULL, 'loose',   NULL,     NULL, 0, 0);",
        )
        .expect("fixture");

    assert_eq!(
        c.effective_license(1).unwrap(),
        Some((
            "Unity Asset Store EULA".into(),
            Some("https://unity.com/legal/as-terms".into())
        )),
        "an asset that states nothing carries its pack's licence"
    );
    assert_eq!(
        c.effective_license(2).unwrap(),
        Some((
            "CC0-1.0".into(),
            Some("https://unity.com/legal/as-terms".into())
        )),
        "the asset's own licence wins; the URL falls back independently"
    );
    assert_eq!(
        c.effective_license(3).unwrap(),
        None,
        "no asset licence and no pack means nothing to show, not the word unknown"
    );
    assert_eq!(c.effective_license(999).unwrap(), None, "no such asset");
}

/// M21 - the Asset Store EULA backfill. The harvest could always state this (a pack with a
/// publisher came out of the store cache tree); only the desktop write path never called the
/// setter, leaving 87 packs in the reference library licence-less - and no re-harvest reaches
/// them without re-inflating every archive to learn what `publisher` already says.
#[test]
fn store_pack_license_backfill() {
    let mut c = catalog_at(20);
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO package (id, uuid, source_id, package_path, name, publisher, license) VALUES
                 (1, 'p1', 1, '/s/a.unitypackage', 'FromStore',   'SICS Games', NULL),
                 (2, 'p2', 1, '/s/b.unitypackage', 'StatedOwn',   'SICS Games', 'CC0-1.0'),
                 (3, 'p3', 1, '/s/c.unitypackage', 'LooseFolder', NULL,         NULL);",
        )
        .expect("fixture");
    // These rows predate M21, which is the situation on every catalog that existed before it.
    c.apply_migrations_up_to(u32::MAX).expect("migrate");
    let by_name = |n: &str| {
        c.list_packages()
            .unwrap()
            .into_iter()
            .find(|p| p.name == n)
            .unwrap()
    };
    assert_eq!(
        by_name("FromStore").license.as_deref(),
        Some("Unity Asset Store EULA"),
        "a store-cache pack that said nothing is licensed by where it came from"
    );
    assert_eq!(
        by_name("FromStore").license_url.as_deref(),
        Some("https://unity.com/legal/as-terms")
    );
    assert_eq!(
        by_name("StatedOwn").license.as_deref(),
        Some("CC0-1.0"),
        "a pack that stated its own licence keeps it"
    );
    assert_eq!(
        by_name("LooseFolder").license,
        None,
        "no publisher means not from the store, so nothing is known about its licence"
    );
}

/// UI-SPEC 13.6 - the row states. `in pack` vs `in library` is not decoration: a dependency
/// inside the pack travels with it, while one that resolves across packs is what ENGINE-API 5.5
/// flags, because Unity reassigns GUIDs on import.
#[test]
fn related_assets_tells_in_pack_from_in_library() {
    let mut c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind) VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'material', 'Material'), (2, 'texture', 'Texture');
             INSERT INTO package (id, uuid, source_id, package_path, name) VALUES
                 (10, 'p1', 1, '/s/a.unitypackage', 'PackA'),
                 (11, 'p2', 1, '/s/b.unitypackage', 'PackB');
             INSERT INTO asset (id, uuid, source_id, media_type_id, package_id, name, unity_guid, created_at, updated_at) VALUES
                 (1, 'a1', 1, 1, 10, 'rock_mat',  'g1', 0, 0),
                 (2, 'a2', 1, 2, 10, 'rock_albedo', 'g2', 0, 0),
                 (3, 'a3', 1, 2, 11, 'shared_noise', 'g3', 0, 0);",
        )
        .expect("fixture");

    c.record_dependencies(
        1,
        Some("g1"),
        "uses_texture",
        &["g2".to_string(), "g3".to_string(), "gMISSING".to_string()],
    )
    .expect("deps");

    let rows = c.related_assets(1).unwrap();
    let by_name = |n: &str| {
        rows.iter()
            .find(|r| r.name.as_deref() == Some(n))
            .unwrap_or_else(|| panic!("no row for {n}: {rows:?}"))
    };
    assert_eq!(
        by_name("rock_albedo").same_pack,
        Some(true),
        "same pack - the dependency travels with it"
    );
    assert_eq!(
        by_name("shared_noise").same_pack,
        Some(false),
        "another pack - the reference will not survive an import"
    );
    let missing = rows
        .iter()
        .find(|r| r.asset_id.is_none())
        .expect("missing row");
    assert_eq!(missing.same_pack, None, "nothing to be in the same pack as");
    assert_eq!(missing.declared_path.as_deref(), Some("gMISSING"));
    assert!(missing.edge_id > 0, "the row carries the id Ignore acts on");
}

/// An ignore has to survive the next dependency scan. `record_dependencies` re-derives an
/// asset's edges wholesale, so without the two guards here every muted link would come back the
/// moment the file's bytes changed - and SQLite cannot dedupe the re-insert, because a `missing`
/// edge has a NULL `to_asset_id` and NULLs are distinct under UNIQUE.
#[test]
fn ignored_edges_survive_a_reharvest() {
    let mut c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind) VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'material', 'Material'), (2, 'texture', 'Texture');
             INSERT INTO asset (id, uuid, source_id, media_type_id, name, unity_guid, created_at, updated_at) VALUES
                 (1, 'a1', 1, 1, 'rock_mat', 'g1', 0, 0);",
        )
        .expect("fixture");
    let refs = ["gGONE".to_string()];
    c.record_dependencies(1, Some("g1"), "uses_texture", &refs)
        .expect("deps");

    let edge = c.related_assets(1).unwrap()[0].edge_id;
    assert!(c.ignore_relationship(edge).unwrap(), "first ignore takes");
    assert!(
        !c.ignore_relationship(edge).unwrap(),
        "ignoring twice changes nothing"
    );

    // The scan that would otherwise resurrect it.
    c.record_dependencies(1, Some("g1"), "uses_texture", &refs)
        .expect("re-harvest");
    let rows = c.related_assets(1).unwrap();
    assert_eq!(rows.len(), 1, "no duplicate edge was minted: {rows:?}");
    assert_eq!(rows[0].state, "ignored", "the mute survived the re-harvest");

    // And it stays out of the late-binding pass even once the target exists.
    c.conn
        .execute_batch(
            "INSERT INTO asset (id, uuid, source_id, media_type_id, name, unity_guid, created_at, updated_at)
                 VALUES (2, 'a2', 1, 2, 'found_it', 'gGONE', 0, 0);",
        )
        .expect("target appears");
    c.resolve_pending_relationships().expect("resolve");
    assert_eq!(
        c.related_assets(1).unwrap()[0].state,
        "ignored",
        "an ignored edge is not silently re-resolved"
    );

    // Restore hands it back, and the ordinary resolve then binds it.
    assert!(c.restore_relationship(edge).unwrap(), "restore takes");
    let rows = c.related_assets(1).unwrap();
    assert_eq!(
        rows[0].state, "resolved",
        "restore re-binds through the normal path"
    );
    assert_eq!(rows[0].name.as_deref(), Some("found_it"));
    assert!(
        !c.restore_relationship(edge).unwrap(),
        "restoring a live edge changes nothing"
    );
}

/// M22 - edges pointing at Unity's own built-in resources. Nothing in any catalog will ever be
/// that file, so the edge is `missing` for good: the Inspector warned that links "are skipped on
/// export" over a link that is not broken, and offered a relink with nothing to point at.
#[test]
fn builtin_resource_edges_are_dropped() {
    let mut c = catalog_at(21);
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'material', 'Material');
             INSERT INTO asset (id, uuid, source_id, media_type_id, name, unity_guid, created_at, updated_at)
                 VALUES (1, 'a1', 1, 1, 'prefab_mat', 'g1', 0, 0),
                        (2, 'a2', 1, 1, 'real_mat',   'g2', 0, 0);
             INSERT INTO asset_relationship (id, from_asset_id, to_asset_id, kind, source, declared_path, state) VALUES
                 (1, 1, NULL, 'uses_material', 'unity_prefab', '0000000000000000f000000000000000', 'missing'),
                 (2, 1, NULL, 'uses_texture',  'unity_meta',   'gSOMETHINGREAL',                   'missing'),
                 (3, 1, 2,    'uses_material', 'unity_prefab', 'g2',                               'resolved');",
        )
        .expect("fixture");
    c.apply_migrations_up_to(u32::MAX).expect("migrate");
    let rows = c.related_assets(1).unwrap();
    let declared: Vec<Option<&str>> = rows.iter().map(|r| r.declared_path.as_deref()).collect();
    assert!(
        !declared.contains(&Some("0000000000000000f000000000000000")),
        "the built-in resource edge is gone: {rows:?}"
    );
    assert!(
        declared.contains(&Some("gSOMETHINGREAL")),
        "a genuinely broken link is left alone"
    );
    assert_eq!(
        rows.len(),
        2,
        "only the built-in edge was dropped: {rows:?}"
    );
}

/// UI-SPEC 13.9 - where an in-pack asset lives *inside* its archive.
///
/// The `workspace_rel IS NULL` guard is the whole point and cannot be exercised against the
/// reference library, which currently holds no extracted assets at all: an asset that has been
/// materialized has a real file on disk, so it must report an `abs_path` and NOT an in-archive
/// path, or the Inspector tells you to look somewhere the bytes are no longer read from.
#[test]
fn in_pack_location_is_only_for_assets_still_in_the_archive() {
    let c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
             INSERT INTO package (id, uuid, source_id, package_path, name)
                 VALUES (10, 'p1', 1, 'G:/cache/Pack.unitypackage', 'Pack');
             INSERT INTO package_entry (id, package_id, guid, pathname, tar_offset, size_bytes) VALUES
                 (100, 10, 'g-in',  'Assets/Pack/Textures/Rock.png', 0, 10),
                 (101, 10, 'g-out', 'Assets/Pack/Textures/Moss.png', 0, 10),
                 (102, 10, 'g-nop', NULL,                            0, 10);
             INSERT INTO asset (id, uuid, source_id, media_type_id, package_id, name, created_at, updated_at) VALUES
                 (1, 'a1', 1, 1, 10, 'Rock', 0, 0),
                 (2, 'a2', 1, 1, 10, 'Moss', 0, 0),
                 (3, 'a3', 1, 1, 10, 'Nameless', 0, 0),
                 (4, 'a4', 1, 1, NULL, 'Loose', 0, 0);
             INSERT INTO asset_file (id, asset_id, rel_path, ext, size_bytes, mtime, package_entry_id, workspace_rel) VALUES
                 (1, 1, 'Rock.png', 'png', 10, 0, 100, NULL),
                 (2, 2, 'Moss.png', 'png', 10, 0, 101, 'abc123/Moss.png'),
                 (3, 3, 'Nameless.png', 'png', 10, 0, 102, NULL),
                 (4, 4, 'Loose.png', 'png', 10, 0, NULL, NULL);
             UPDATE asset SET primary_file_id = 1 WHERE id = 1;
             UPDATE asset SET primary_file_id = 2 WHERE id = 2;
             UPDATE asset SET primary_file_id = 3 WHERE id = 3;
             UPDATE asset SET primary_file_id = 4 WHERE id = 4;",
        )
        .expect("fixture");

    assert_eq!(
        c.in_pack_location(1).unwrap(),
        Some((
            "Assets/Pack/Textures/Rock.png".to_string(),
            "G:/cache/Pack.unitypackage".to_string()
        )),
        "an asset still inside the archive reports where it sits and which archive holds it"
    );
    assert_eq!(
        c.in_pack_location(2).unwrap(),
        None,
        "an extracted asset has a real file on disk - pointing at the archive would send the user \
         somewhere the bytes are no longer read from"
    );
    assert_eq!(
        c.in_pack_location(3).unwrap(),
        None,
        "an entry with no declared pathname has no location to show"
    );
    assert_eq!(
        c.in_pack_location(4).unwrap(),
        None,
        "a loose file is not in any pack"
    );
    assert_eq!(c.in_pack_location(999).unwrap(), None, "no such asset");
}

/// The locator behind an in-pack preview (INGESTION 2 "Surgical pull"). It must answer only for
/// an asset genuinely still in its archive: a materialized one has a real file to render from,
/// and an entry with no recorded offset has nothing to seek to.
#[test]
fn in_pack_locator_answers_only_for_pullable_assets() {
    let c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
             INSERT INTO package (id, uuid, source_id, package_path, name)
                 VALUES (10, 'p1', 1, 'G:/cache/Pack.unitypackage', 'Pack');
             INSERT INTO package_entry (id, package_id, guid, pathname, tar_offset, size_bytes) VALUES
                 (100, 10, 'g1', 'Assets/Rock.png', 4096, 2048),
                 (101, 10, 'g2', 'Assets/Moss.png', 8192, 1024),
                 (102, 10, 'g3', 'Assets/Nope.png', NULL, 512);
             INSERT INTO asset (id, uuid, source_id, media_type_id, package_id, name, created_at, updated_at) VALUES
                 (1, 'a1', 1, 1, 10, 'Rock', 0, 0),
                 (2, 'a2', 1, 1, 10, 'Moss', 0, 0),
                 (3, 'a3', 1, 1, 10, 'Nope', 0, 0),
                 (4, 'a4', 1, 1, NULL, 'Loose', 0, 0);
             INSERT INTO asset_file (id, asset_id, rel_path, ext, size_bytes, mtime, package_entry_id, workspace_rel) VALUES
                 (1, 1, 'Rock.png', 'png', 2048, 0, 100, NULL),
                 (2, 2, 'Moss.png', 'png', 1024, 0, 101, 'abc/Moss.png'),
                 (3, 3, 'Nope.png', 'png', 512, 0, 102, NULL),
                 (4, 4, 'Loose.png', 'png', 10, 0, NULL, NULL);
             UPDATE asset SET primary_file_id = 1 WHERE id = 1;
             UPDATE asset SET primary_file_id = 2 WHERE id = 2;
             UPDATE asset SET primary_file_id = 3 WHERE id = 3;
             UPDATE asset SET primary_file_id = 4 WHERE id = 4;",
        )
        .expect("fixture");

    assert_eq!(
        c.in_pack_locator(1).unwrap(),
        Some((
            "G:/cache/Pack.unitypackage".to_string(),
            4096,
            2048,
            "png".to_string()
        )),
        "an asset still in the archive hands back the offset a surgical pull needs"
    );
    assert_eq!(
        c.in_pack_locator(2).unwrap(),
        None,
        "an extracted asset renders from its real file, not by re-pulling the archive"
    );
    assert_eq!(
        c.in_pack_locator(3).unwrap(),
        None,
        "no offset, nothing to seek to"
    );
    assert_eq!(
        c.in_pack_locator(4).unwrap(),
        None,
        "a loose file is in no pack"
    );
}

/// `identify_asset(ids)` must appraise *those* ids. The op flagged them and then ran a generic
/// bounded pass, which takes the front of the global queue - 8,155 rows deep on the reference
/// library - so an agent's three assets came back untouched while the call reported success.
#[test]
fn the_tag_queue_can_be_restricted_to_an_explicit_set() {
    let c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
             INSERT INTO asset (id, uuid, source_id, media_type_id, name, tags_dirty, created_at, updated_at) VALUES
                 (1, 'a1', 1, 1, 'one',   1, 0, 0),
                 (2, 'a2', 1, 1, 'two',   1, 0, 0),
                 (3, 'a3', 1, 1, 'three', 1, 0, 0),
                 (4, 'a4', 1, 1, 'clean', 0, 0, 0);
             INSERT INTO asset_file (id, asset_id, rel_path, ext, size_bytes, mtime) VALUES
                 (1, 1, 'one.png',   'png', 1, 0),
                 (2, 2, 'two.png',   'png', 1, 0),
                 (3, 3, 'three.png', 'png', 1, 0),
                 (4, 4, 'clean.png', 'png', 1, 0);
             UPDATE asset SET primary_file_id = 1 WHERE id = 1;
             UPDATE asset SET primary_file_id = 2 WHERE id = 2;
             UPDATE asset SET primary_file_id = 3 WHERE id = 3;
             UPDATE asset SET primary_file_id = 4 WHERE id = 4;",
        )
        .expect("fixture");

    let all: Vec<i64> = c
        .assets_needing_tags_within(10, None)
        .unwrap()
        .iter()
        .map(|j| j.asset_id)
        .collect();
    assert_eq!(all.len(), 3, "the whole dirty queue: {all:?}");

    let some: Vec<i64> = c
        .assets_needing_tags_within(10, Some(&[3, 1]))
        .unwrap()
        .iter()
        .map(|j| j.asset_id)
        .collect();
    assert_eq!(some.len(), 2, "only what was asked for: {some:?}");
    assert!(some.contains(&1) && some.contains(&3));

    assert!(
        c.assets_needing_tags_within(10, Some(&[4]))
            .unwrap()
            .is_empty(),
        "an asset that is not dirty is not appraised just because it was named"
    );
    assert!(
        c.assets_needing_tags_within(10, Some(&[]))
            .unwrap()
            .is_empty(),
        "an explicit empty set asks for nothing - it must not mean everything"
    );
}

/// The Traits screen's three numbers must be about *assets*, and the split must fit inside the
/// usage it splits.
///
/// `asset_tag` is keyed on `(asset_id, tag_id, source)`, so one asset can hold one tag twice — a
/// person set it, and the AI later proposed it too. Summing rows for the split while counting
/// distinct assets for the usage made `manual + ai` exceed `usage_count` on exactly that asset,
/// which is what the e2e vocabulary invariant caught on the real catalog.
#[test]
fn the_manual_ai_split_counts_assets_and_never_exceeds_the_usage() {
    let c = Catalog::open_in_memory().unwrap();
    c.conn
        .execute_batch(
            "INSERT INTO import_source (id, uuid, name, root_path, kind)
                 VALUES (1, 'src-1', 's', '/s', 'folder');
             INSERT INTO media_type (id, code, label) VALUES (1, 'image', 'Image');
             INSERT INTO asset (id, uuid, source_id, media_type_id, name, created_at, updated_at)
                 VALUES (1, 'a1', 1, 1, 'One', 0, 0), (2, 'a2', 1, 1, 'Two', 0, 0);
             INSERT INTO tag (id, namespace, name, norm_name, is_trait)
                 VALUES (7, 'subject', 'rock', 'rock', 1);
             INSERT INTO asset_tag (asset_id, tag_id, source, created_at) VALUES
                 (1, 7, 'manual', 0),
                 (1, 7, 'ai', 0),
                 (2, 7, 'ai', 0);",
        )
        .expect("fixture");

    let v = c.list_vocabulary().unwrap();
    let rock = v.iter().find(|t| t.value == "rock").expect("the tag lists");
    assert_eq!(rock.usage_count, 2, "two assets carry it, not three rows");
    assert_eq!(rock.manual_count, 1, "one asset was tagged by hand");
    assert_eq!(
        rock.ai_count, 1,
        "the asset a person already tagged is not also an AI tag — manual wins the tie"
    );
    assert!(
        rock.manual_count + rock.ai_count <= rock.usage_count,
        "the split cannot exceed the usage it splits"
    );
}
