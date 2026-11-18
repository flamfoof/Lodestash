//! The L5-slice tests.
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

fn rec(rel: &str, mt: &str) -> CrawlRecord {
    CrawlRecord {
        workspace_rel: None,
        content_hash: None,
        rel_path: rel.to_string(),
        name: rel.rsplit('/').next().unwrap_or(rel).to_string(),
        media_type: mt.to_string(),
        size_bytes: 1,
        mtime: 1,
    }
}

fn asset_id(c: &Catalog, rel: &str) -> i64 {
    c.conn
        .query_row(
            "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
                  WHERE af.rel_path = ?1",
            params![rel],
            |r| r.get::<_, i64>(0),
        )
        .expect("asset id")
}

fn is_trashed(c: &Catalog, id: i64) -> bool {
    c.conn
        .query_row(
            "SELECT deleted_at IS NOT NULL FROM asset WHERE id = ?1",
            params![id],
            |r| r.get::<_, bool>(0),
        )
        .expect("trash flag")
}

fn tagged_catalog(rels: &[(&str, &str)]) -> (Catalog, Vec<i64>) {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    let records: Vec<CrawlRecord> = rels.iter().map(|(r, m)| rec(r, m)).collect();
    c.record_crawl(src, None, &records).expect("crawl");
    let ids = rels.iter().map(|(r, _)| asset_id(&c, r)).collect();
    (c, ids)
}

#[test]
fn tag_graph_reports_nodes_edges_and_header_counts() {
    let (mut c, ids) = tagged_catalog(&[
        ("a.fbx", "model_3d"),
        ("b.png", "image"),
        ("c.wav", "audio"),
    ]);
    let (a, b, cc) = (ids[0], ids[1], ids[2]);
    // rock on two assets; rpg on one; music+ambient co-occur on the third.
    c.set_tags(
        a,
        "manual",
        None,
        &[
            TagAssignment::exact("subject", "rock"),
            TagAssignment::exact("style", "rpg"),
        ],
    )
    .expect("tags a");
    c.set_tags(
        b,
        "manual",
        None,
        &[TagAssignment::exact("subject", "rock")],
    )
    .expect("tags b");
    c.set_tags(
        cc,
        "manual",
        None,
        &[
            TagAssignment::exact("usage", "music"),
            TagAssignment::exact("mood", "ambient"),
        ],
    )
    .expect("tags c");

    let g = c.tag_graph().expect("graph");
    assert_eq!(g.asset_count, 3, "all three live tagged assets");
    assert_eq!(g.link_count, 0, "no relationship edges yet");
    assert_eq!(g.nodes.len(), 4);
    let rock = g
        .nodes
        .iter()
        .find(|n| n.value == "rock")
        .expect("rock node");
    assert_eq!(rock.usage_count, 2, "rock is on two assets");
    assert_eq!(g.edges.len(), 2, "rock↔rpg on a, music↔ambient on c");
    let pair = |x: i64, y: i64| (x.min(y), x.max(y));
    let edges: HashSet<(i64, i64)> = g.edges.iter().map(|e| pair(e.tag_a, e.tag_b)).collect();
    let rock_id = rock.tag_id;
    let rpg_id = g.nodes.iter().find(|n| n.value == "rpg").unwrap().tag_id;
    let music_id = g.nodes.iter().find(|n| n.value == "music").unwrap().tag_id;
    let ambient_id = g
        .nodes
        .iter()
        .find(|n| n.value == "ambient")
        .unwrap()
        .tag_id;
    assert!(edges.contains(&pair(rock_id, rpg_id)));
    assert!(edges.contains(&pair(music_id, ambient_id)));
    for e in &g.edges {
        assert_eq!(e.weight, 1);
    }

    // A resolved relationship edge feeds link_count; an unresolved one does not.
    c.conn
        .execute(
            "INSERT INTO asset_relationship (from_asset_id, to_asset_id, kind, source, state)
                 VALUES (?1, ?2, 'uses_texture', 'inferred', 'resolved')",
            params![a, b],
        )
        .expect("edge");
    assert_eq!(c.tag_graph().expect("graph").link_count, 1);
    c.conn
        .execute(
            "INSERT INTO asset_relationship (from_asset_id, to_asset_id, kind, source, state)
                 VALUES (?1, NULL, 'uses_material', 'unity_meta', 'missing')",
            params![a],
        )
        .expect("missing edge");
    assert_eq!(c.tag_graph().expect("graph").link_count, 1);

    // Trashed assets leave the graph: rock drops to one asset, rpg disappears entirely.
    c.set_trashed(&[a], true).expect("trash");
    let g = c.tag_graph().expect("graph");
    assert_eq!(g.asset_count, 2);
    let rock = g
        .nodes
        .iter()
        .find(|n| n.value == "rock")
        .expect("rock node");
    assert_eq!(rock.usage_count, 1);
    assert!(g.nodes.iter().all(|n| n.value != "rpg"));
    assert_eq!(g.edges.len(), 1, "only music↔ambient remains");
}

#[test]
fn asset_edges_lists_only_resolved_edges_between_live_assets() {
    let (c, ids) = tagged_catalog(&[
        ("a.fbx", "model_3d"),
        ("b.png", "image"),
        ("c.wav", "audio"),
    ]);
    let (a, b, cc) = (ids[0], ids[1], ids[2]);
    let edge = |from: i64, to: Option<i64>| {
        c.conn
            .execute(
                "INSERT INTO asset_relationship (from_asset_id, to_asset_id, kind, source, state)
                     VALUES (?1, ?2, 'uses_texture', 'inferred', ?3)",
                params![from, to, if to.is_some() { "resolved" } else { "missing" }],
            )
            .expect("edge");
    };
    edge(a, Some(b));
    edge(a, Some(cc));
    edge(b, None); // unresolved — excluded

    let edges = c.asset_edges().expect("edges");
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|e| e.kind == "uses_texture"));
    assert!(edges.iter().any(|e| e.from == a && e.to == b));
    assert!(edges.iter().any(|e| e.from == a && e.to == cc));

    // Trashing one endpoint removes the edge.
    c.set_trashed(&[b], true).expect("trash");
    let edges = c.asset_edges().expect("edges");
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].to, cc);
}

#[test]
fn source_blacklist_add_list_remove_round_trip() {
    let c = Catalog::open_in_memory().expect("open");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    assert!(c.list_source_excludes(src).expect("empty").is_empty());

    c.add_source_exclude(src, "folder", "Assets/Editor")
        .expect("add folder");
    c.add_source_exclude(src, "asset", "junk.fbx")
        .expect("add asset");
    let list = c.list_source_excludes(src).expect("list");
    assert_eq!(list.len(), 2);
    assert!(list.contains(&SourceExclude {
        kind: "folder".into(),
        value: "Assets/Editor".into()
    }));
    assert!(list.contains(&SourceExclude {
        kind: "asset".into(),
        value: "junk.fbx".into()
    }));

    // An exact duplicate is a no-op.
    c.add_source_exclude(src, "folder", "Assets/Editor")
        .expect("dup");
    assert_eq!(c.list_source_excludes(src).expect("list").len(), 2);

    assert!(c
        .remove_source_exclude(src, "folder", "Assets/Editor")
        .expect("removed"));
    assert!(
        !c.remove_source_exclude(src, "folder", "Assets/Editor")
            .expect("second"),
        "already gone"
    );
    let list = c.list_source_excludes(src).expect("list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].kind, "asset");

    // Sources are independent — a second source has its own (empty) blacklist.
    let src2 = c
        .upsert_import_source("Other", "D:/other", "folder")
        .expect("src2");
    assert!(c.list_source_excludes(src2).expect("empty").is_empty());
}

#[test]
fn source_blacklist_rejects_unknown_kind_and_empty_value() {
    let c = Catalog::open_in_memory().expect("open");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    assert!(
        c.add_source_exclude(src, "file", "x").is_err(),
        "kind must be folder|asset"
    );
    assert!(
        c.add_source_exclude(src, "folder", "   ").is_err(),
        "empty value rejected"
    );
    assert!(c.list_source_excludes(src).expect("list").is_empty());
}

#[test]
fn taxonomy_switcher_list_activate_and_delete() {
    let mut c = Catalog::open_in_memory().expect("open");
    let t1 = c.create_taxonomy("Game", false).expect("t1");
    let t2 = c.create_taxonomy("Worldbuilding", true).expect("t2");
    let rows = c.list_taxonomies().expect("list");
    assert_eq!(rows.len(), 2);
    let active = rows.iter().find(|r| r.is_active).expect("active row");
    assert_eq!(active.id, t2);
    assert_eq!(active.name, "Worldbuilding");
    assert_eq!(active.version, 1);

    c.set_active_taxonomy(t1).expect("activate");
    let rows = c.list_taxonomies().expect("list");
    assert_eq!(rows.iter().filter(|r| r.is_active).count(), 1, "one active");
    assert!(rows.iter().find(|r| r.id == t1).unwrap().is_active);
    assert!(!rows.iter().find(|r| r.id == t2).unwrap().is_active);

    // Deleting a taxonomy takes its categories, rules and aliases with it.
    let cat = c.add_category(t1, None, "Props", "Props").expect("cat");
    c.add_rule(t1, cat, "exact_tag", "subject:barrel", 10)
        .expect("rule");
    c.add_category_alias(cat, "junk").expect("alias");
    let n = c.delete_taxonomy(t1).expect("delete");
    assert_eq!(n, 2, "category + taxonomy row");
    assert!(c.list_categories(t1).expect("cats").is_empty());
    assert!(c.list_category_aliases(cat).expect("aliases").is_empty());
    assert_eq!(c.list_taxonomies().expect("list").len(), 1);

    assert!(c.set_active_taxonomy(999).is_err(), "unknown taxonomy");
    assert!(c.delete_taxonomy(999).is_err(), "unknown taxonomy");
}

#[test]
fn rename_category_rewrites_descendant_paths() {
    let mut c = Catalog::open_in_memory().expect("open");
    let tax = c.create_taxonomy("Default", true).expect("tax");
    let env = c
        .add_category(tax, None, "Environment", "Environment")
        .expect("env");
    let foil = c
        .add_category(tax, Some(env), "Foliage", "Environment/Foliage")
        .expect("foil");
    c.add_category(tax, Some(foil), "Trees", "Environment/Foliage/Trees")
        .expect("trees");

    c.rename_category(env, "World").expect("rename");
    let paths: HashSet<String> = c
        .list_categories(tax)
        .expect("cats")
        .into_iter()
        .map(|(_, _, p)| p)
        .collect();
    assert_eq!(
        paths,
        ["World", "World/Foliage", "World/Foliage/Trees"]
            .into_iter()
            .map(String::from)
            .collect()
    );
    assert!(c
        .list_categories(tax)
        .expect("cats")
        .iter()
        .any(|(_, label, _)| label == "World"));

    // Same label is a no-op, empty label is rejected.
    c.rename_category(env, "World").expect("no-op rename");
    assert!(
        c.rename_category(env, "   ").is_err(),
        "empty label rejected"
    );
}

#[test]
fn delete_category_removes_subtree_and_cascades_assignments() {
    let (mut c, ids) = tagged_catalog(&[("barrel.png", "image")]);
    let asset = ids[0];
    let tax = c.create_taxonomy("Default", true).expect("tax");
    let root = c.add_category(tax, None, "Root", "Root").expect("root");
    let child = c
        .add_category(tax, Some(root), "Child", "Root/Child")
        .expect("child");
    c.add_rule(tax, child, "exact_tag", "subject:barrel", 10)
        .expect("rule");
    c.add_category_alias(child, "crate").expect("alias");
    c.assign_category_manual(asset, child, tax).expect("assign");

    assert_eq!(c.delete_category(root).expect("delete"), 2, "root + child");
    assert!(c.list_categories(tax).expect("cats").is_empty());
    assert!(
        c.categories_of(asset, tax).expect("cats").is_empty(),
        "assignments cascade"
    );
    let rules: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM mapping_rule WHERE taxonomy_id = ?1",
            params![tax],
            |r| r.get(0),
        )
        .expect("rules");
    assert_eq!(rules, 0, "rules cascade");
    assert!(c.delete_category(999).is_err(), "unknown category");
}

#[test]
fn aliases_round_trip_normalize_and_remove() {
    let c = Catalog::open_in_memory().expect("open");
    let tax = c.create_taxonomy("Default", true).expect("tax");
    let cat = c.add_category(tax, None, "Props", "Props").expect("cat");
    let id1 = c.add_category_alias(cat, "Barrel").expect("add");
    let id2 = c.add_category_alias(cat, "barrel").expect("add again");
    assert_eq!(id1, id2, "case-insensitive norm makes them the same alias");
    assert!(
        c.add_category_alias(cat, "  ").is_err(),
        "empty alias rejected"
    );

    let rows = c.list_category_aliases(cat).expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].alias, "Barrel");

    c.remove_category_alias(rows[0].id).expect("remove");
    assert!(c.list_category_aliases(cat).expect("list").is_empty());
}

#[test]
fn disabled_rule_is_skipped_by_apply_mapping() {
    let (mut c, ids) = tagged_catalog(&[("barrel.png", "image")]);
    let asset = ids[0];
    c.set_tags(
        asset,
        "manual",
        None,
        &[TagAssignment::exact("subject", "barrel")],
    )
    .expect("tag");
    let tax = c.create_taxonomy("Default", true).expect("tax");
    let cat = c.add_category(tax, None, "Props", "Props").expect("cat");
    let rule = c
        .add_rule(tax, cat, "exact_tag", "subject:barrel", 10)
        .expect("rule");

    assert_eq!(c.apply_mapping(tax).expect("apply"), 1);
    assert_eq!(c.categories_of(asset, tax).expect("cats").len(), 1);

    c.set_rule_enabled(rule, false).expect("disable");
    // Re-apply drops the non-pinned rows; the disabled rule produces nothing.
    assert_eq!(c.apply_mapping(tax).expect("apply"), 0);
    assert!(c.categories_of(asset, tax).expect("cats").is_empty());
}

#[test]
fn op_log_record_recent_delete_manage_the_tail() {
    let c = Catalog::open_in_memory().expect("open");
    let a = c
        .record_op("set_trashed", r#"{"asset_ids":[1],"trashed":true}"#, None)
        .expect("op1");
    let b = c
        .record_op(
            "create_collection",
            r#"{"collection_id":9,"name":"x"}"#,
            None,
        )
        .expect("op2");
    let cc = c.record_op("set_tags", "{}", None).expect("op3");

    let recent = c.recent_ops(2).expect("recent");
    assert_eq!(recent.len(), 2);
    assert_eq!(recent[0].id, cc, "newest first");
    assert_eq!(recent[0].op_kind, "set_tags");
    assert_eq!(recent[1].id, b);

    let full = c.recent_ops(100).expect("full");
    assert_eq!(full.len(), 3);
    assert!(
        full[0].id > full[1].id && full[1].id > full[2].id,
        "append order"
    );

    assert_eq!(c.delete_ops(&[a, b]).expect("delete"), 2);
    let remaining = c.recent_ops(10).expect("remaining");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id, cc);
    assert_eq!(c.delete_ops(&[]).expect("no-op"), 0);
}

/// `add_asset_tag` appends — adding one manual tag must not delete the asset's other
/// manual tags (the set_tags replace-per-source trap the bulk bar used to fall into).
#[test]
fn add_asset_tag_merges_without_touching_the_existing_set() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d")]);
    let asset = ids[0];
    c.set_tags(
        asset,
        "manual",
        None,
        &[
            TagAssignment::exact("subject", "rock"),
            TagAssignment::exact("style", "rpg"),
        ],
    )
    .expect("initial");

    let n = c
        .add_asset_tag(asset, "manual", "subject", "shiny")
        .expect("add");
    assert_eq!(n, 1, "one row inserted");
    let tags = c.tags_for_asset(asset).expect("tags");
    assert_eq!(tags.len(), 3, "rock + rpg survive, shiny joins");
    assert!(tags.contains(&TagAssignment::exact("subject", "shiny")));

    // Adding the same tag again is a no-op — and records nothing (the writer only logs
    // affected ids, so a revert can never yank a pre-existing tag).
    let again = c
        .add_asset_tag(asset, "manual", "subject", "shiny")
        .expect("again");
    assert_eq!(again, 0, "duplicate insert is a no-op");
}

#[test]
fn revert_set_trashed_toggles_direction_in_both_ways() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);

    // Trash a, then undo the trash: the asset comes back.
    let op = c.record_trash_op(&[a], true).expect("record trash");
    assert_eq!(c.set_trashed(&[a], true).expect("trash"), 1);
    assert!(is_trashed(&c, a));
    c.revert_set_trashed(op).expect("revert");
    assert!(!is_trashed(&c, a), "undo of a trash restores the asset");
    assert!(c.recent_ops(10).expect("recent").is_empty());

    // Untrash b (trashed first), then undo: it is trashed again.
    c.set_trashed(&[b], true).expect("trash b");
    let op = c.record_trash_op(&[b], false).expect("record untrash");
    assert_eq!(c.set_trashed(&[b], false).expect("untrash"), 1);
    assert!(!is_trashed(&c, b));
    c.revert_op(op).expect("revert");
    assert!(is_trashed(&c, b), "undo of an untrash re-trashes the asset");
}

/// Undoing a trash puts back only what that trash changed (review B13): an asset that was already
/// in the Trash stays there. Same for the other direction, and for redo.
#[test]
fn undoing_a_trash_restores_only_what_it_changed() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    c.set_trashed(&[a], true).expect("a is already trashed");

    assert_eq!(
        c.set_trashed_recorded(&[a, b], true).expect("trash both"),
        1,
        "only b changed"
    );
    let op = c.recent_ops(1).expect("ops")[0].id;
    c.revert_op(op).expect("undo");
    assert!(
        is_trashed(&c, a),
        "undo restored an asset this trash never moved"
    );
    assert!(!is_trashed(&c, b), "undo restored the one it did");
    c.redo_op(op).expect("redo");
    assert!(is_trashed(&c, a) && is_trashed(&c, b));

    // Untrash {a, b} while only a is trashed... first put b back so the sides differ.
    c.set_trashed(&[b], false).expect("b out");
    assert_eq!(
        c.set_trashed_recorded(&[a, b], false)
            .expect("untrash both"),
        1,
        "only a changed"
    );
    let op = c.recent_ops(1).expect("ops")[0].id;
    c.revert_op(op).expect("undo untrash");
    assert!(is_trashed(&c, a), "undo re-trashed the one it restored");
    assert!(
        !is_trashed(&c, b),
        "undo trashed an asset this untrash never moved"
    );

    // A toggle that changes nothing leaves nothing to undo.
    let before = c.recent_ops(10).expect("ops").len();
    assert_eq!(c.set_trashed_recorded(&[a], true).expect("no-op"), 0);
    assert_eq!(c.recent_ops(10).expect("ops").len(), before);
}

/// Restoring counts only rows that were in the Trash: the untrash used to match every requested id,
/// so restoring a live asset reported a change that never happened.
#[test]
fn restoring_counts_only_rows_that_were_trashed() {
    let (c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    c.set_trashed(&[a], true).expect("trash a");
    assert_eq!(
        c.set_trashed(&[a, b], false).expect("restore"),
        1,
        "b was never trashed, so restoring it changed nothing"
    );
    assert!(!is_trashed(&c, a) && !is_trashed(&c, b));
    assert_eq!(
        c.set_trashed(&[a, b], false).expect("restore again"),
        0,
        "nothing left in the Trash"
    );
}

/// Registering a project is one transaction (review B13): a failure after "deactivate all"
/// leaves the previously active project active and no half-registered row behind.
#[test]
fn a_failed_project_registration_changes_nothing() {
    let c = Catalog::open_in_memory().expect("open");
    let first = c
        .register_project("C:/dev/First", None, None)
        .expect("register");
    // The failure lands on the step right after every other project was deactivated.
    c.conn
        .execute_batch(
            "CREATE TEMP TRIGGER inject_failure BEFORE INSERT ON project
             BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .expect("inject");
    assert!(c.register_project("C:/dev/Second", None, None).is_err());

    let projects = c.list_projects().expect("list");
    assert_eq!(
        projects.len(),
        1,
        "a half-registered project was left behind"
    );
    assert_eq!(c.active_project_id().expect("active"), Some(first));
}

/// Delete from disk and Remove from library take an asset out for good, and the undo history must
/// not be a way back (review finding 6): the trash that came before them is no longer undoable for
/// that asset, in either direction, and an op left with nothing to flip is gone.
#[test]
fn undo_and_redo_never_restore_a_retired_asset() {
    let (mut c, ids) = tagged_catalog(&[
        ("a.fbx", "model_3d"),
        ("b.png", "image"),
        ("c.png", "image"),
    ]);
    let (a, b, keep) = (ids[0], ids[1], ids[2]);

    // Trash {a, keep} (logged), then the file behind `a` is deleted: undo restores only `keep`.
    assert_eq!(c.set_trashed_recorded(&[a, keep], true).expect("trash"), 2);
    let trash = c.recent_ops(1).expect("ops")[0].id;
    assert_eq!(
        c.retire_assets(&[a]).expect("retire"),
        0,
        "already in the Trash"
    );
    c.revert_op(trash).expect("undo");
    assert!(
        is_trashed(&c, a),
        "undo un-trashed an asset whose file the app deleted"
    );
    assert!(!is_trashed(&c, keep), "the rest of that trash still undoes");
    c.redo_op(trash).expect("redo");
    assert!(is_trashed(&c, a) && is_trashed(&c, keep));

    // Restore `b` from the Trash, undo that restore (so it sits on the redo stack), then remove
    // `b` from the library: the redo must not bring it back, and has nothing left to do.
    c.set_trashed(&[b], true).expect("b in the Trash");
    assert_eq!(c.set_trashed_recorded(&[b], false).expect("restore"), 1);
    let restore = c.recent_ops(1).expect("ops")[0].id;
    c.revert_op(restore).expect("undo the restore");
    assert!(is_trashed(&c, b));
    c.retire_assets(&[b]).expect("retire b");
    assert!(
        c.redo_candidates(10)
            .expect("redo")
            .iter()
            .all(|r| r.id != restore),
        "an op with nothing left to flip stayed on the redo stack"
    );
    assert!(
        c.redo_op(restore).is_err(),
        "the emptied op is gone, not replayable"
    );
    assert!(is_trashed(&c, b), "redo resurrected a retired asset");

    // Retiring a live asset trashes it and reports the change.
    c.set_trashed(&[keep], false).expect("keep is live");
    assert_eq!(c.retire_assets(&[keep]).expect("retire live"), 1);
    assert!(is_trashed(&c, keep));
    assert_eq!(c.retire_assets(&[]).expect("no-op"), 0);
}

/// Undo → redo → undo acts on the **same** op (review finding 6). Redo used to delete the row, so
/// the second undo reached past it and reverted whatever the user had done before.
#[test]
fn undo_after_a_redo_reverts_the_op_that_was_redone() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    c.set_trashed_recorded(&[a], true).expect("earlier op");
    let earlier = c.recent_ops(1).expect("ops")[0].id;
    c.set_trashed_recorded(&[b], true).expect("later op");
    let later = c.recent_ops(1).expect("ops")[0].id;

    c.revert_op(later).expect("undo");
    c.redo_op(later).expect("redo");
    assert_eq!(
        c.recent_ops(1).expect("ops")[0].id,
        later,
        "after a redo the next undo is the op just redone"
    );
    c.revert_op(c.recent_ops(1).expect("ops")[0].id)
        .expect("undo again");
    assert!(!is_trashed(&c, b), "the redone op was not undone");
    assert!(
        is_trashed(&c, a),
        "undo → redo → undo reverted an unrelated earlier op"
    );
    assert_eq!(c.recent_ops(1).expect("ops")[0].id, earlier);
}

/// The redo stack is a stack: the op undone last is redone first. It was read newest-id-first, so
/// after undoing two ops the redo button replayed them in the wrong order.
#[test]
fn redo_offers_the_most_recently_undone_op_first() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    c.set_trashed_recorded(&[ids[0]], true).expect("first");
    let first = c.recent_ops(1).expect("ops")[0].id;
    c.set_trashed_recorded(&[ids[1]], true).expect("second");
    let second = c.recent_ops(1).expect("ops")[0].id;
    c.revert_op(second).expect("undo second");
    c.revert_op(first).expect("undo first");
    let stack: Vec<i64> = c
        .redo_candidates(10)
        .expect("redo")
        .iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(
        stack,
        vec![first, second],
        "the last undone is the first redone"
    );
}

/// Redoing a collection create makes a new row with a new id; the op must follow it, or the next
/// undo would try to delete the id that no longer exists.
#[test]
fn a_redone_collection_create_can_be_undone_again() {
    let mut c = Catalog::open_in_memory().expect("open");
    let id = c.create_collection("Temp").expect("create");
    let op = c.record_collection_create_op(id, "Temp").expect("record");
    c.revert_op(op).expect("undo");
    c.redo_op(op).expect("redo");
    assert_eq!(c.list_collections().expect("list").len(), 1);
    c.revert_op(op).expect("undo the redone create");
    assert!(
        c.list_collections().expect("list").is_empty(),
        "the re-created collection was left behind"
    );
}

#[test]
fn redo_applies_the_forward_payload_and_re_arms_the_op() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d")]);
    let a = ids[0];

    // Trash → undo → redo: the trash lands twice, and the op is back on the undo stack — it is
    // the last thing that happened, so it is what the next undo reverts.
    let op = c.record_trash_op(&[a], true).expect("record trash");
    assert_eq!(c.set_trashed(&[a], true).expect("trash"), 1);
    c.revert_op(op).expect("undo");
    assert!(!is_trashed(&c, a));
    assert!(
        c.redo_candidates(10)
            .expect("redo stack")
            .iter()
            .any(|r| r.id == op),
        "the undone op sits on the redo stack"
    );
    c.redo_op(op).expect("redo");
    assert!(is_trashed(&c, a), "redo re-applies the trashed state");
    assert_eq!(
        c.recent_ops(10)
            .expect("recent")
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>(),
        vec![op],
        "redo puts the op back on the undo stack"
    );
    assert!(
        c.redo_candidates(10).expect("redo stack").is_empty(),
        "nothing left to redo"
    );
    assert!(
        c.redo_op(op).is_err(),
        "an op that is not undone is not redone"
    );

    // New work clears the redo stack.
    let op2 = c.record_trash_op(&[a], false).expect("record untrash 2");
    assert_eq!(c.set_trashed(&[a], false).expect("untrash 2"), 1);
    c.revert_op(op2).expect("undo 2");
    assert_eq!(c.redo_candidates(10).expect("redo stack").len(), 1);
    c.record_op("set_trashed", r#"{"asset_ids":[1],"trashed":true}"#, None)
        .expect("new work");
    assert!(
        c.redo_candidates(10).expect("redo stack").is_empty(),
        "new work invalidates the redo stack"
    );
}

#[test]
fn revert_create_collection_deletes_the_created_collection() {
    let c = Catalog::open_in_memory().expect("open");
    let id = c.create_collection("Temp").expect("create");
    let op = c.record_collection_create_op(id, "Temp").expect("record");
    assert_eq!(c.list_collections().expect("list").len(), 1);

    c.revert_create_collection(op).expect("revert");
    assert!(c.list_collections().expect("list").is_empty());
    assert!(c.delete_collection(id).is_err(), "already deleted");
}

#[test]
fn revert_dispatches_and_rejects_unknown_kinds() {
    let mut c = Catalog::open_in_memory().expect("open");
    let op = c.record_op("mystery", "{}", None).expect("record");
    assert!(c.revert_op(op).is_err(), "no revert for unknown kind");
    assert_eq!(
        c.recent_ops(10).expect("recent").len(),
        1,
        "failed revert keeps the op"
    );
    assert!(c.revert_op(999).is_err(), "no such op");
}

/// Naming something that is not there is `NotFound` (v2 plan Task 1.1) — an op to undo or redo, a
/// collection or taxonomy to delete — so an adapter answers 404, not 500.
#[test]
fn naming_a_missing_op_collection_or_taxonomy_is_not_found() {
    use crate::error::ErrorKind;
    let mut c = Catalog::open_in_memory().expect("open");
    let errors = [
        ("undo", c.revert_op(999).expect_err("undo")),
        (
            "delete collection",
            c.delete_collection(999).expect_err("del"),
        ),
        (
            "activate taxonomy",
            c.set_active_taxonomy(999).expect_err("activate"),
        ),
        (
            "delete taxonomy",
            c.delete_taxonomy(999).expect_err("delete"),
        ),
    ];
    for (what, err) in errors {
        assert!(matches!(err, CoreError::NotFound(_)), "{what}: {err:?}");
        assert_eq!(err.kind(), ErrorKind::NotFound, "{what}");
    }

    // The staging tray exists; it is refused for what it is, not reported missing.
    let tray = c.ensure_active_staging_tray().expect("tray");
    let err = c
        .delete_collection(tray)
        .expect_err("the tray is protected");
    assert_eq!(err.kind(), ErrorKind::Invalid, "{err:?}");
}

#[test]
fn collection_membership_add_remove_is_idempotent() {
    let (c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    let col = c.create_collection("Terrain").expect("create");

    c.collection_add_asset(col, a).expect("add a");
    c.collection_add_asset(col, b).expect("add b");
    assert_eq!(c.collection_members(col).expect("members").len(), 2);

    // Adding the same asset again is a no-op — membership is a set.
    c.collection_add_asset(col, a).expect("add a again");
    let members = c.collection_members(col).expect("members");
    assert_eq!(members.len(), 2);
    assert!(members.contains(&a));

    c.collection_remove_asset(col, a).expect("remove a");
    let members = c.collection_members(col).expect("members");
    assert_eq!(members.len(), 1);
    assert_eq!(members[0], b);

    // Removing an absent member is a no-op too.
    c.collection_remove_asset(col, a).expect("remove absent");
    assert_eq!(c.collection_members(col).expect("members").len(), 1);

    // The Collections screen count reflects the live membership.
    assert_eq!(c.list_collections().expect("list")[0].asset_count, 1);
}

/// UI-SPEC §7: *"Trashed assets leave search, dup groups, and staging everywhere."*
///
/// The membership row deliberately survives — Trash is reversible, and a restore has to put the
/// asset back where it was — so "leaves" has to be enforced on the read. Both reads: the grid
/// filter and `import_staged` go through `collection_members`, the Collections screen through
/// `list_collections`. Until 2026-08-26 neither excluded them, so a staged asset in the Trash
/// still counted in the tray badge and would have been copied by an import.
#[test]
fn trashing_a_member_takes_it_out_of_the_collection_without_forgetting_it() {
    let (c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    let col = c.create_collection("Terrain").expect("create");
    c.collection_add_asset(col, a).expect("add a");
    c.collection_add_asset(col, b).expect("add b");

    c.set_trashed(&[a], true).expect("trash a");
    let members = c.collection_members(col).expect("members");
    assert_eq!(members, vec![b], "a trashed asset is still a member");
    assert_eq!(
        c.list_collections().expect("list")[0].asset_count,
        1,
        "the collection card counted a trashed asset"
    );

    // Restoring brings it back — the membership row was never deleted.
    c.set_trashed(&[a], false).expect("restore a");
    assert_eq!(
        c.collection_members(col).expect("members").len(),
        2,
        "restore lost the membership"
    );
}

/// The tray exists before any editor does, and connecting one **adopts** it (M20, ADR-0010).
///
/// M5 made staging per-project and deleted the project-less tray, on the reading that a shortlist
/// means nothing without somewhere to send it. The app disagreed in practice: picks are collected
/// long before a project is connected — and while the Unity plugin does not exist, *no* user has
/// a registered project — so the shell kept its own in-memory list instead and the two never
/// agreed. This checks the whole arc: stage with no project, connect one, keep the picks.
#[test]
fn the_staging_tray_survives_having_no_project_and_is_adopted_by_the_first_one() {
    let (c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);

    assert!(
        c.active_staging_tray_id().expect("read").is_none(),
        "a fresh catalog has a tray before anything staged into it"
    );
    let tray = c.ensure_active_staging_tray().expect("create");
    c.collection_add_asset(tray, a).expect("stage a");
    c.collection_add_asset(tray, b).expect("stage b");
    assert_eq!(c.active_staging_tray_id().expect("read"), Some(tray));
    assert_eq!(c.collection_members(tray).expect("members").len(), 2);

    // Creating it twice must not mint a second one — the M20 index says at most one, and a
    // duplicate would make "the tray" ambiguous rather than erroring somewhere useful.
    assert_eq!(c.ensure_active_staging_tray().expect("again"), tray);

    // Connect an editor. The picks come with it rather than being stranded in a tray nothing
    // resolves to any more.
    let project = c
        .register_project("C:/dev/MyRPG", Some("6000.0.23f1"), None)
        .expect("register");
    assert_eq!(
        c.staging_tray_id(project).expect("tray"),
        Some(tray),
        "connecting a project abandoned the picks instead of adopting them"
    );
    assert_eq!(
        c.active_staging_tray_id().expect("read"),
        Some(tray),
        "the active tray no longer resolves after registering"
    );
    assert_eq!(c.collection_members(tray).expect("members").len(), 2);

    // A *second* project gets its own empty tray — trays never mix (UI-SPEC §16.1 footnote).
    let other = c
        .register_project("C:/dev/Other", None, None)
        .expect("register 2");
    let other_tray = c.staging_tray_id(other).expect("tray 2").expect("exists");
    assert_ne!(
        other_tray, tray,
        "the second project reused the first's tray"
    );
    assert!(
        c.collection_members(other_tray)
            .expect("members")
            .is_empty(),
        "a new project started with someone else's picks"
    );
    // Registering made `other` active, so the tray on screen is now the empty one.
    assert_eq!(c.active_staging_tray_id().expect("read"), Some(other_tray));

    // Switching back brings the first project's picks back, which is the whole point of §16.2.
    c.set_active_project(project).expect("switch");
    assert_eq!(c.active_staging_tray_id().expect("read"), Some(tray));
    assert_eq!(c.collection_members(tray).expect("members").len(), 2);

    // And clearing empties the *current* tray only.
    assert_eq!(c.clear_staging_tray().expect("clear"), 2);
    assert!(c.collection_members(tray).expect("members").is_empty());
}
