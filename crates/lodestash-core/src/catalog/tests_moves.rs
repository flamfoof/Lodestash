//! Identity follows content (DATA-MODEL §6.1, ADR-0025) — the parts of the rule a scenario with
//! real files cannot reach one at a time: the pairing rule, every kind of user data, what the
//! `(size, mtime)` gate lets the matching know, and what a take-over leaves behind. The
//! scenarios themselves run against real files in `lodestash-engine` (`tests_identity.rs`).

use super::moves::pair_by_evidence;
use super::*;

fn rec(rel: &str, size: i64, hash: Option<&str>) -> CrawlRecord {
    CrawlRecord {
        workspace_rel: None,
        content_hash: hash.map(str::to_string),
        rel_path: rel.to_string(),
        name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
        media_type: "image".to_string(),
        size_bytes: size,
        mtime: 1,
    }
}

/// One full-source crawl in the order `persist_to_catalog` runs it.
fn scan(c: &mut Catalog, src: i64, records: &[CrawlRecord]) -> Reconciled {
    let matched = c.reconcile_moves(src, records).expect("moves");
    let rest: Vec<CrawlRecord> = records
        .iter()
        .filter(|r| !matched.consumed.contains(&r.rel_path))
        .cloned()
        .collect();
    c.record_crawl(src, None, &rest).expect("crawl");
    let present: HashSet<String> = records.iter().map(|r| r.rel_path.clone()).collect();
    c.reconcile_missing(src, &present).expect("missing");
    matched
}

fn library(records: &[CrawlRecord]) -> (Catalog, i64) {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    scan(&mut c, src, records);
    (c, src)
}

/// `(rel_path, asset_id)` for every row of the source, sorted by path.
fn rows(c: &Catalog, src: i64) -> Vec<(String, i64)> {
    let mut rows: Vec<(String, i64)> = c
        .assets_for_source(src)
        .expect("read")
        .into_iter()
        .map(|a| (a.rel_path, a.asset_id))
        .collect();
    rows.sort();
    rows
}

fn id_at(c: &Catalog, src: i64, rel: &str) -> i64 {
    rows(c, src)
        .into_iter()
        .find(|(p, _)| p == rel)
        .unwrap_or_else(|| panic!("no row at {rel}"))
        .1
}

/// The pairing rule, on its own: evidence pairs, order never does. Each case is given both ways
/// round, so an answer that depended on the order of either list would differ.
#[test]
fn rows_and_places_pair_by_evidence_and_never_by_order() {
    /// The pairs as `(row path, place path)`, for the lists as given and both reversed.
    fn pairs<'a>(rows: &[&'a str], places: &[&'a str]) -> Vec<(&'a str, &'a str)> {
        let named = |rows: &[&'a str], places: &[&'a str]| {
            let mut out: Vec<(&str, &str)> = pair_by_evidence(rows, places)
                .into_iter()
                .map(|(r, p)| (rows[r], places[p]))
                .collect();
            out.sort();
            out
        };
        let forward = named(rows, places);
        let (mut rows_back, mut places_back) = (rows.to_vec(), places.to_vec());
        rows_back.reverse();
        places_back.reverse();
        assert_eq!(
            forward,
            named(&rows_back, &places_back),
            "the pairing depends on the order of the lists"
        );
        forward
    }

    assert_eq!(
        pairs(&["a.png"], &["moved/z.png"]),
        [("a.png", "moved/z.png")],
        "one row and one place"
    );
    assert_eq!(
        pairs(
            &["old/sub1/a.png", "old/sub2/a.png"],
            &["new/sub2/a.png", "new/sub1/a.png"]
        ),
        [
            ("old/sub1/a.png", "new/sub1/a.png"),
            ("old/sub2/a.png", "new/sub2/a.png")
        ],
        "a folder moved rigidly keeps its same-named duplicates apart"
    );
    assert_eq!(
        pairs(&["old/a.png", "old/b.png"], &["new/c.png", "new/a.png"]),
        [("old/a.png", "new/a.png"), ("old/b.png", "new/c.png")],
        "the file name, then the one row and one place that are left"
    );
    assert_eq!(
        pairs(&["x/a.png"], &["y/a.png", "y/b.png"]),
        [("x/a.png", "y/a.png")],
        "the same-named place is the move; the other is a copy"
    );

    // Nothing to go by: nothing is paired. Sort order used to decide each of these.
    let none: [(&str, &str); 0] = [];
    assert_eq!(
        pairs(&["x/a.png", "y/a.png"], &["p/a.png", "q/a.png"]),
        none,
        "a name both rows and both places share singles nobody out"
    );
    assert_eq!(
        pairs(&["x/a.png", "y/a.png"], &["z/a.png"]),
        none,
        "the first same-named row in path order was chosen"
    );
    assert_eq!(pairs(&["a.png"], &["b.png", "c.png"]), none);
    assert_eq!(
        pairs(
            &["tile_1.png", "tile_2.png"],
            &["floor_a.png", "floor_b.png"]
        ),
        none,
        "leftovers of equal count were paired in path order"
    );
}

/// What a person — or an agent they approved — can do to an asset, each of which makes it an
/// asset in its own right: a vanished row never takes its place. Then the control: a duplicate
/// only the machine has touched **is** taken over, and what the machine made goes with it.
#[test]
fn every_kind_of_user_data_keeps_a_duplicate_from_being_taken_over() {
    type Touch = fn(&mut Catalog, i64);
    let touches: [(&str, Touch); 14] = [
        ("a favourite", |c, d| {
            c.set_favorite(&[d], true).expect("favourite");
        }),
        ("a rating", |c, d| {
            c.set_rating(&[d], Some(4)).expect("rating");
        }),
        ("confirmed", |c, d| {
            c.set_confirmed(d, true).expect("confirm")
        }),
        ("a manual tag", |c, d| {
            c.add_asset_tag(d, "manual", "subject", "mine")
                .expect("tag");
        }),
        ("an imported tag", |c, d| {
            c.add_asset_tag(d, "imported", "subject", "theirs")
                .expect("tag");
        }),
        ("a description", |c, d| {
            c.set_description(d, Some("a grey rock")).expect("caption")
        }),
        ("a licence override", |c, d| {
            c.conn
                .execute("UPDATE asset SET license = 'CC0' WHERE id = ?1", params![d])
                .expect("licence");
        }),
        ("a collection", |c, d| {
            let id = c.create_collection("Keepers").expect("collection");
            c.collection_add_asset(id, d).expect("member");
        }),
        ("an import record", |c, d| {
            let project = c
                .register_project("C:/dev/Game", None, None)
                .expect("project");
            let spec = ImportSpec {
                asset_id: d,
                source_package_id: None,
                dest_path: "Assets/a.png".into(),
                unity_guid: None,
                with_deps: false,
            };
            c.record_imports(project, &[spec], "batch").expect("import");
        }),
        ("an export record", |c, d| {
            c.record_exports("batch", "C:/out", &[(Some(d), "a.png".to_string())])
                .expect("export");
        }),
        ("a manual category", |c, d| {
            c.conn
                .execute_batch(
                    "INSERT INTO taxonomy (id, uuid, name) VALUES (1, 't', 'T');
                     INSERT INTO category (id, taxonomy_id, pref_label, path)
                          VALUES (1, 1, 'Props', 'Props');",
                )
                .expect("taxonomy");
            c.assign_category_manual(d, 1, 1).expect("category");
        }),
        ("an ignored link", |c, d| {
            c.conn
                .execute(
                    "INSERT INTO asset_relationship (from_asset_id, kind, source, state)
                          VALUES (?1, 'uses_texture', 'unity_meta', 'ignored')",
                    params![d],
                )
                .expect("edge");
        }),
        // Nothing of these two is left in the asset's own rows: only the history says so.
        ("a trash that was undone", |c, d| {
            c.set_trashed_recorded(&[d], true).expect("trash");
            c.set_trashed_recorded(&[d], false).expect("restore");
        }),
        ("an agent's approved write", |c, d| {
            let result = serde_json::json!({ "asset_ids": [d], "unchanged_assets": [d] });
            c.conn
                .execute(
                    "INSERT INTO tag_request (request_id, body_digest, result_json, created_at)
                          VALUES ('req', 'digest', ?1, 0)",
                    params![result.to_string()],
                )
                .expect("receipt");
        }),
    ];

    // A tagged original and its copy; then the original's file goes. No file is read in that
    // scan: the copy passes the gate.
    let fixture = || {
        let (mut c, src) = library(&[rec("a.png", 5, Some("H")), rec("copy/a.png", 5, Some("H"))]);
        let (original, copy) = (id_at(&c, src, "a.png"), id_at(&c, src, "copy/a.png"));
        c.add_asset_tag(original, "manual", "subject", "boulder")
            .expect("tag");
        (c, src, original, copy)
    };
    for (what, touch) in touches {
        let (mut c, src, original, copy) = fixture();
        touch(&mut c, copy);
        let matched = scan(&mut c, src, &[rec("copy/a.png", 5, None)]);
        assert_eq!(
            matched.absorbed, 0,
            "a duplicate with {what} was taken over"
        );
        assert_eq!(
            rows(&c, src),
            [
                ("a.png".to_string(), original),
                ("copy/a.png".to_string(), copy)
            ],
            "a duplicate with {what} was taken over"
        );
        assert_eq!(
            c.tags_for_asset(original).expect("tags")[0].value,
            "boulder"
        );
    }

    // The control. What the machine made is not user data: an AI tag with its background
    // history row, an embedding, a rule-assigned category, a harvested link.
    let (mut c, src, original, copy) = fixture();
    c.apply_tag_batch(&TagBatch {
        items: vec![TagBatchItem {
            asset_id: copy,
            source: "ai:test".into(),
            model_id: Some("test".into()),
            mode: TagMode::ReplaceSource,
            tags: vec![TagAssignment {
                namespace: "subject".into(),
                value: "stone".into(),
                confidence: Some(0.5),
            }],
            description: None,
            expected_revision: None,
        }],
        completes_queue: true,
        history: Some(HistoryOrigin::Background),
        receipt: None,
    })
    .expect("ai tags");
    c.upsert_embedding(copy, "model", "image", None, Some(src), &[1.0, 0.0])
        .expect("embedding");
    c.conn
        .execute_batch(&format!(
            "INSERT INTO taxonomy (id, uuid, name) VALUES (1, 't', 'T');
             INSERT INTO category (id, taxonomy_id, pref_label, path) VALUES (1, 1, 'P', 'P');
             INSERT INTO asset_category (asset_id, category_id, taxonomy_id, assigned_by)
                  VALUES ({copy}, 1, 1, 'rule');
             INSERT INTO asset_relationship (from_asset_id, kind, source, state)
                  VALUES ({copy}, 'uses_texture', 'unity_meta', 'missing');"
        ))
        .expect("machine rows");
    let generation = c.embedding_generation("model").expect("generation");

    let matched = scan(&mut c, src, &[rec("copy/a.png", 5, None)]);
    assert_eq!(
        (matched.moved, matched.absorbed, matched.superseded),
        (0, 1, 0)
    );
    assert_eq!(
        rows(&c, src),
        [("copy/a.png".to_string(), original)],
        "the tagged row is where its bytes are, and the empty duplicate is gone"
    );
    let tags: Vec<String> = c
        .tags_for_asset(original)
        .expect("tags")
        .into_iter()
        .map(|t| t.value)
        .collect();
    assert_eq!(tags, ["boulder"], "the duplicate's AI tags were merged in");
    let left = |table: &str, column: &str| -> i64 {
        c.conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = ?1"),
                params![copy],
                |r| r.get(0),
            )
            .expect("count")
    };
    for (table, column) in [
        ("asset", "id"),
        ("asset_file", "asset_id"),
        ("asset_tag", "asset_id"),
        ("asset_embedding", "asset_id"),
        ("asset_category", "asset_id"),
        ("asset_relationship", "from_asset_id"),
        ("asset_fts", "rowid"),
    ] {
        assert_eq!(left(table, column), 0, "the duplicate left rows in {table}");
    }
    assert_eq!(
        c.embedding_generation("model").expect("generation"),
        generation + 1,
        "a resident snapshot would still hold the deleted row's vector"
    );
    assert_eq!(
        c.search_assets("stone", 10).expect("search"),
        Some(Vec::new()),
        "the duplicate's tag is still in the full-text index"
    );
}

/// History that cannot be read is not history that is not there: nothing is taken over.
#[test]
fn a_history_that_cannot_be_read_takes_over_nothing() {
    let (mut c, src) = library(&[rec("a.png", 5, Some("H")), rec("copy/a.png", 5, Some("H"))]);
    let (original, copy) = (id_at(&c, src, "a.png"), id_at(&c, src, "copy/a.png"));
    c.add_asset_tag(original, "manual", "subject", "boulder")
        .expect("tag");
    c.conn
        .execute(
            "INSERT INTO op_log (op_kind, payload_json, created_at, origin)
                  VALUES ('set_trashed', 'not json', 0, 'user')",
            [],
        )
        .expect("a damaged history row");

    let matched = scan(&mut c, src, &[rec("copy/a.png", 5, None)]);
    assert_eq!(matched, Reconciled::default());
    assert_eq!(
        rows(&c, src),
        [
            ("a.png".to_string(), original),
            ("copy/a.png".to_string(), copy)
        ]
    );
}

/// The matching knows only what the crawl read. A file that passed the `(size, mtime)` gate is
/// taken to hold what its row recorded; one that changed and could not be read is taken to hold
/// nothing known — its row keeps the path, and nothing moves onto it or away from it.
#[test]
fn a_file_the_crawl_did_not_read_is_never_a_place_to_move_to() {
    let (mut c, src) = library(&[
        rec("gate.png", 5, Some("H1")),
        rec("locked.png", 6, Some("H2")),
        rec("mover.png", 7, Some("H3")),
    ]);
    let (gate, locked, mover) = (
        id_at(&c, src, "gate.png"),
        id_at(&c, src, "locked.png"),
        id_at(&c, src, "mover.png"),
    );

    // gate.png: unchanged, not read. locked.png: changed (size), unreadable. mover.png: gone,
    // and its bytes are at a new path.
    let matched = scan(
        &mut c,
        src,
        &[
            rec("gate.png", 5, None),
            rec("locked.png", 9, None),
            rec("moved/mover.png", 7, Some("H3")),
        ],
    );
    assert_eq!(
        (matched.moved, matched.absorbed, matched.superseded),
        (1, 0, 0)
    );
    assert_eq!(
        rows(&c, src),
        [
            ("gate.png".to_string(), gate),
            ("locked.png".to_string(), locked),
            ("moved/mover.png".to_string(), mover),
        ]
    );
    let hash_of = |id: i64| -> Option<String> {
        c.conn
            .query_row(
                "SELECT content_hash FROM asset WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .expect("hash")
    };
    assert_eq!(
        hash_of(gate).as_deref(),
        Some("H1"),
        "the gate kept its hash"
    );
    assert_eq!(hash_of(mover).as_deref(), Some("H3"));
}

/// `record_crawl` is what makes "an in-place edit takes the new hash" true. A record whose hash
/// differs from the row's is a changed file even at the same size and mtime; a hash read for a
/// row that never had one is filled in without re-queuing the asset.
#[test]
fn a_read_hash_decides_over_the_size_and_mtime_gate() {
    let (mut c, src) = library(&[rec("edited.png", 5, Some("H1")), rec("legacy.png", 6, None)]);
    let (edited, legacy) = (id_at(&c, src, "edited.png"), id_at(&c, src, "legacy.png"));
    c.conn
        .execute("UPDATE asset SET embedding_dirty = 0, tags_dirty = 0", [])
        .expect("processed");

    let stats = c
        .record_crawl(
            src,
            None,
            &[
                rec("edited.png", 5, Some("H9")),
                rec("legacy.png", 6, Some("H2")),
            ],
        )
        .expect("crawl");
    assert_eq!((stats.updated, stats.unchanged, stats.inserted), (1, 1, 0));
    let state = |id: i64| -> (Option<String>, bool) {
        c.conn
            .query_row(
                "SELECT content_hash, embedding_dirty FROM asset WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("state")
    };
    assert_eq!(
        state(edited),
        (Some("H9".to_string()), true),
        "same size and mtime, other bytes: the row kept its stale hash"
    );
    assert_eq!(
        state(legacy),
        (Some("H2".to_string()), false),
        "a first hash is filled in, and nothing is re-queued for it"
    );
}

/// Two rows on one path is damage from before the rule. Nothing is moved onto that path and
/// neither row is moved off it — and nothing panics.
#[test]
fn two_rows_already_on_one_path_are_left_alone() {
    let (mut c, src) = library(&[
        rec("a.png", 5, Some("H1")),
        rec("b.png", 6, Some("H2")),
        rec("c.png", 7, Some("H3")),
    ]);
    let c_id = id_at(&c, src, "c.png");
    c.conn
        .execute(
            "UPDATE asset_file SET rel_path = 'a.png' WHERE rel_path = 'b.png'",
            [],
        )
        .expect("the damage");

    // c.png was moved over a.png.
    let matched = c
        .reconcile_moves(src, &[rec("a.png", 7, Some("H3"))])
        .expect("moves");
    assert_eq!(matched, Reconciled::default());
    let on_a = rows(&c, src).iter().filter(|(p, _)| p == "a.png").count();
    assert_eq!(on_a, 2, "a third row was put on the path");
    assert_eq!(id_at(&c, src, "c.png"), c_id);
}
