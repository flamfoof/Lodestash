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

#[test]
fn revert_set_tags_restores_the_snapshotted_prior_set() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d")]);
    let asset = ids[0];
    let a1 = TagAssignment::exact("subject", "rock");
    let a2 = TagAssignment::exact("style", "rpg");
    c.set_tags(asset, "manual", None, &[a1.clone(), a2.clone()])
        .expect("initial");

    let op = c
        .record_set_tags_op(asset, "manual", &[])
        .expect("snapshot");
    c.set_tags(
        asset,
        "manual",
        None,
        &[TagAssignment::exact("subject", "changed")],
    )
    .expect("mutate");
    assert_eq!(
        c.tags_for_asset(asset).expect("tags").len(),
        1,
        "now the new tag only"
    );

    c.revert_op(op).expect("revert");
    let tags = c.tags_for_asset(asset).expect("tags");
    assert_eq!(tags.len(), 2);
    assert!(tags.contains(&a1));
    assert!(tags.contains(&a2));
    assert!(
        c.recent_ops(10).expect("recent").is_empty(),
        "reverted op is consumed"
    );
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

/// Bulk-tag undo is the exact inverse per asset: an add is undone by removing that tag
/// only; a removal by re-adding it. One op row covers the whole batch.
#[test]
fn bulk_tag_revert_is_the_exact_inverse_per_asset() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);

    // Add one tag to both, record the affected pair, then revert.
    assert_eq!(
        c.add_asset_tag(a, "manual", "subject", "shiny").expect("a"),
        1
    );
    assert_eq!(
        c.add_asset_tag(b, "manual", "subject", "shiny").expect("b"),
        1
    );
    let op = c
        .record_bulk_tag_op(&[a, b], "subject", "shiny", true)
        .expect("record");
    c.revert_op(op).expect("revert");
    assert!(
        c.tags_for_asset(a).expect("tags").is_empty()
            && c.tags_for_asset(b).expect("tags").is_empty(),
        "revert removed the added tag from both assets"
    );

    // Remove-then-revert re-adds it.
    assert_eq!(
        c.add_asset_tag(a, "manual", "subject", "dull").expect("a2"),
        1
    );
    let op2 = c
        .record_bulk_tag_op(&[a], "subject", "dull", false)
        .expect("record");
    c.remove_asset_tag(a, "subject", "dull").expect("remove");
    assert!(c.tags_for_asset(a).expect("tags").is_empty());
    c.revert_op(op2).expect("revert2");
    let tags = c.tags_for_asset(a).expect("tags");
    assert_eq!(tags.len(), 1, "revert re-added the removed tag");
    assert!(tags.contains(&TagAssignment::exact("subject", "dull")));
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

#[test]
fn redo_applies_the_forward_payload_then_consumes_the_op() {
    let (mut c, ids) = tagged_catalog(&[("a.fbx", "model_3d")]);
    let a = ids[0];

    // Trash → undo → redo: the trash lands twice, the row is consumed, and the undo
    // button's view (recent_ops) shows nothing again.
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
    assert!(
        c.recent_ops(10).expect("recent").is_empty(),
        "redo consumes the row"
    );
    assert!(
        c.redo_candidates(10).expect("redo stack").is_empty(),
        "nothing left to redo"
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
    assert!(
        c.revert_set_tags(op).is_err(),
        "wrong kind for the focused revert"
    );
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
