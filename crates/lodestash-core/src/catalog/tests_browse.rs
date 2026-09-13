//! Scoped discovery (v1 plan S12): the walk, the sample and the scoped searches.

use super::*;

/// A library with `images` images and `models` models under one source, plus a second source with
/// one model. Returns the catalog, the first source's id and the second's.
fn library(images: usize, models: usize) -> (Catalog, i64, i64) {
    let mut cat = Catalog::open_in_memory().expect("open");
    cat.seed_media_types().expect("seed");
    let src = cat
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    let rec = |name: String, media: &str| CrawlRecord {
        workspace_rel: None,
        content_hash: None,
        rel_path: name.clone(),
        name,
        media_type: media.into(),
        size_bytes: 1,
        mtime: 1,
    };
    let mut records: Vec<CrawlRecord> = (0..images)
        .map(|i| rec(format!("stone wall {i:04}.png"), "image"))
        .collect();
    records.extend((0..models).map(|i| rec(format!("stone pillar {i:02}.fbx"), "model_3d")));
    cat.record_crawl(src, None, &records).expect("crawl");
    let other = cat
        .upsert_import_source("Other", "C:/other", "folder")
        .expect("other");
    cat.record_crawl(other, None, &[rec("stone arch.fbx".into(), "model_3d")])
        .expect("crawl other");
    (cat, src, other)
}

fn ids_named(cat: &Catalog, like: &str) -> Vec<i64> {
    let mut stmt = cat
        .conn
        .prepare("SELECT id FROM asset WHERE name LIKE ?1 ORDER BY id")
        .unwrap();
    stmt.query_map(params![like], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
}

/// A walk visits every in-scope asset exactly once across pages, and stays below the upper bound
/// it started with: an asset created mid-walk waits for the next walk, and an asset trashed
/// mid-walk is not returned by a later page.
#[test]
fn an_ordered_walk_visits_each_asset_once_and_holds_its_bound() {
    let (mut cat, src, _) = library(55, 0);
    let scope = AssetScope {
        source_id: Some(src),
        ..Default::default()
    };
    let upper = cat.max_asset_id().unwrap();

    let first = cat.browse_asset_ids(&scope, 0, upper, 20).unwrap();
    assert_eq!(first.len(), 20);

    // Mid-walk: one asset later in the order is trashed, one new asset arrives.
    let doomed = cat.browse_asset_ids(&scope, first[19], upper, 1).unwrap()[0];
    cat.set_trashed(&[doomed], true).unwrap();
    cat.record_crawl(
        src,
        None,
        &[CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: "late.png".into(),
            name: "late.png".into(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 1,
        }],
    )
    .unwrap();

    let mut seen = first.clone();
    let mut after = *first.last().unwrap();
    loop {
        let page = cat.browse_asset_ids(&scope, after, upper, 20).unwrap();
        if page.is_empty() {
            break;
        }
        after = *page.last().unwrap();
        seen.extend(page);
    }
    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "an asset was visited twice");
    assert_eq!(seen.len(), 54, "55 assets, one trashed mid-walk");
    assert!(!seen.contains(&doomed), "a trashed asset was returned");
    assert!(
        seen.iter().all(|&id| id <= upper),
        "the walk went past its upper bound to an asset created after it started"
    );
}

/// Ten models behind hundreds of images: the scope is applied before any LIMIT, on the walk, the
/// sample and both kinds of search — so the ten are found whatever the images' ranks are.
#[test]
fn a_small_scope_behind_a_large_library_is_still_found_whole() {
    let (cat, src, other) = library(300, 10);
    let models: Vec<i64> = ids_named(&cat, "stone pillar%");
    let scope = AssetScope {
        source_id: Some(src),
        media_type: Some("model_3d".into()),
        ..Default::default()
    };

    let upper = cat.max_asset_id().unwrap();
    assert_eq!(cat.browse_asset_ids(&scope, 0, upper, 25).unwrap(), models);

    let mut sampled = cat.sample_asset_ids(&scope, 25).unwrap();
    sampled.sort();
    assert_eq!(
        sampled, models,
        "a sample of a smaller population returns all of it once"
    );

    // "stone" matches all 311 names; BM25's top 25 would be almost all images without the scope.
    let mut searched = cat
        .search_assets_scoped("stone", &scope, 25)
        .unwrap()
        .expect("a query");
    searched.sort();
    assert_eq!(searched, models);
    let mut hybrid = cat.hybrid_search_scoped("stone", &[], &scope, 25).unwrap();
    hybrid.sort();
    assert_eq!(hybrid, models);

    // And a scope is a scope: the other library's model is not in this one.
    let other_model = ids_named(&cat, "stone arch%")[0];
    assert!(!cat.scoped_asset_ids(&scope).unwrap().contains(&other_model));
    let theirs = AssetScope {
        source_id: Some(other),
        ..Default::default()
    };
    assert_eq!(
        cat.scoped_asset_ids(&theirs).unwrap(),
        [other_model].into_iter().collect()
    );
}

/// Tag filters are AND over canonical pairs, and `tag_state` splits tagged from untagged.
#[test]
fn tags_narrow_by_every_pair_and_tag_state_splits_the_library() {
    let (mut cat, src, _) = library(4, 0);
    let ids = ids_named(&cat, "stone wall%");
    cat.add_asset_tag(ids[0], "manual", "subject", "wall")
        .unwrap();
    cat.add_asset_tag(ids[0], "manual", "material", "stone")
        .unwrap();
    cat.add_asset_tag(ids[1], "manual", "subject", "wall")
        .unwrap();

    let scope = |tags: Vec<(&str, &str)>, state: TagState| AssetScope {
        source_id: Some(src),
        tag_state: state,
        tags: tags
            .into_iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect(),
        ..Default::default()
    };
    let upper = cat.max_asset_id().unwrap();
    let walk = |s: &AssetScope| cat.browse_asset_ids(s, 0, upper, 100).unwrap();

    assert_eq!(
        walk(&scope(vec![("subject", "wall")], TagState::Any)),
        vec![ids[0], ids[1]]
    );
    assert_eq!(
        walk(&scope(
            vec![("subject", "Wall"), ("material", "stone")],
            TagState::Any
        )),
        vec![ids[0]],
        "pairs are AND-ed, and matched canonically"
    );
    assert_eq!(walk(&scope(vec![], TagState::Tagged)), vec![ids[0], ids[1]]);
    assert_eq!(
        walk(&scope(vec![], TagState::Untagged)),
        vec![ids[2], ids[3]]
    );
    assert!(
        walk(&scope(vec![("mood", "wall")], TagState::Any)).is_empty(),
        "namespace is part of the match"
    );

    // The signature ignores tag order and case, so a cursor survives a reordered request.
    assert_eq!(
        scope(
            vec![("material", "stone"), ("subject", "wall")],
            TagState::Any
        )
        .signature(),
        scope(
            vec![("subject", "WALL"), ("material", "stone")],
            TagState::Any
        )
        .signature()
    );
}

/// The analysis revision moves when the annotations or the file's identity move, and not otherwise.
#[test]
fn the_analysis_revision_follows_evidence_and_annotations() {
    let (mut cat, _, _) = library(1, 0);
    let id = ids_named(&cat, "stone wall%")[0];
    let r0 = cat.analysis_revision(id).unwrap().expect("a live asset");
    assert_eq!(
        cat.analysis_revision(id).unwrap().unwrap(),
        r0,
        "a read changed it"
    );

    cat.add_asset_tag(id, "manual", "subject", "wall").unwrap();
    let r1 = cat.analysis_revision(id).unwrap().unwrap();
    assert_ne!(r1, r0, "a new tag did not change it");

    cat.conn
        .execute(
            "UPDATE asset_file SET size_bytes = 99 WHERE asset_id = ?1",
            params![id],
        )
        .unwrap();
    assert_ne!(
        cat.analysis_revision(id).unwrap().unwrap(),
        r1,
        "a changed file did not change it"
    );
    assert_eq!(cat.analysis_revision(999_999).unwrap(), None);
}
