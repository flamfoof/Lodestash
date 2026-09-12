//! Resident embedding snapshots (v1 plan S09): one load per generation, shared in the process,
//! invalidated by a fact in the catalog rather than by an event.

use super::*;

/// A file-backed catalog — the snapshot cache is keyed by the database's identity, and an
/// in-memory catalog deliberately shares nothing.
fn catalog_on_disk(dir: &std::path::Path, name: &str) -> Catalog {
    let c = Catalog::open(&dir.join(name)).expect("open");
    c.seed_media_types().expect("seed");
    c
}

fn asset(c: &mut Catalog, src: i64, rel: &str, media: &str) -> i64 {
    c.record_crawl(
        src,
        None,
        &[CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: rel.into(),
            name: rel.into(),
            media_type: media.into(),
            size_bytes: 1,
            mtime: 1,
        }],
    )
    .expect("crawl");
    c.conn
        .query_row(
            "SELECT a.id FROM asset a JOIN asset_file af ON af.id = a.primary_file_id
              WHERE af.rel_path = ?1",
            params![rel],
            |r| r.get(0),
        )
        .expect("id")
}

/// The snapshot answers exactly what loading the table per query answered, and it is the *same*
/// snapshot until something writes a vector — a tag edit is not something that writes a vector.
#[test]
fn a_snapshot_is_reused_until_an_embedding_write_moves_the_generation() {
    let dir = tempfile::tempdir().expect("tmp");
    let mut c = catalog_on_disk(dir.path(), "catalog.db");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    let a = asset(&mut c, src, "a.png", "image");
    let b = asset(&mut c, src, "b.png", "image");
    c.upsert_embedding(a, "m", "image", None, None, &[1.0, 0.0])
        .expect("a");
    c.upsert_embedding(b, "m", "image", None, None, &[0.0, 1.0])
        .expect("b");

    let first = c.embedding_snapshot("m").expect("snapshot");
    assert_eq!(first.len(), 2);
    let again = c.embedding_snapshot("m").expect("snapshot");
    assert!(
        std::sync::Arc::ptr_eq(&first, &again),
        "a second query rebuilt the index instead of reusing it"
    );

    // A tag write touches no vector, so the resident copy stands — the point of a durable
    // per-model generation rather than "something in the catalog changed".
    c.add_asset_tag(a, "manual", "subject", "rock")
        .expect("tag");
    let after_tag = c.embedding_snapshot("m").expect("snapshot");
    assert!(
        std::sync::Arc::ptr_eq(&first, &after_tag),
        "a tag edit reloaded the vectors"
    );

    // An embedding write does move it, and the new snapshot carries the new vector.
    let generation = first.generation;
    let third = asset(&mut c, src, "c.png", "image");
    c.upsert_embedding(third, "m", "image", None, None, &[1.0, 1.0])
        .expect("c");
    let fresh = c.embedding_snapshot("m").expect("snapshot");
    assert!(fresh.generation > generation, "the generation stood still");
    assert_eq!(fresh.len(), 3);

    // Ranked results match what the old per-query index produced, scores included.
    let eligible: std::collections::HashSet<i64> = [a, b, third].into_iter().collect();
    let via_snapshot = fresh.index.knn_eligible(&[1.0, 0.0], 3, &eligible);
    let via_load =
        crate::vector::BruteForceIndex::new(c.embeddings_for_knn("m", None).expect("load"));
    let expected = <crate::vector::BruteForceIndex as crate::vector::VectorIndex>::knn(
        &via_load,
        &[1.0, 0.0],
        3,
    );
    assert_eq!(via_snapshot, expected);
}

/// The write a GUI process makes has to be visible to a headless one, which cannot be notified.
/// Two handles on one database stand in for two processes: the generation is the whole mechanism.
#[test]
fn another_handle_sees_the_write_and_reloads() {
    let dir = tempfile::tempdir().expect("tmp");
    let mut writer = catalog_on_disk(dir.path(), "shared.db");
    let src = writer
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    let a = asset(&mut writer, src, "a.png", "image");
    writer
        .upsert_embedding(a, "m", "image", None, None, &[1.0, 0.0])
        .expect("a");

    let reader = Catalog::open(&dir.path().join("shared.db")).expect("second handle");
    let before = reader.embedding_snapshot("m").expect("snapshot");
    assert_eq!(before.len(), 1);

    let b = asset(&mut writer, src, "b.png", "image");
    writer
        .upsert_embedding(b, "m", "image", None, None, &[0.0, 1.0])
        .expect("b");

    let after = reader.embedding_snapshot("m").expect("snapshot");
    assert_eq!(after.len(), 2, "the other handle kept a stale snapshot");
    assert!(after.generation > before.generation);
}

/// Scoring happens over the eligible set, so a trashed or filtered asset cannot spend a slot in the
/// top K — and a vector stored at the wrong dimension scores rather than panics.
#[test]
fn only_eligible_assets_are_scored_and_a_bad_dimension_cannot_panic() {
    let dir = tempfile::tempdir().expect("tmp");
    let mut c = catalog_on_disk(dir.path(), "filter.db");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    let near = asset(&mut c, src, "near.png", "image");
    let far = asset(&mut c, src, "far.png", "image");
    let odd = asset(&mut c, src, "odd.png", "image");
    c.upsert_embedding(near, "m", "image", None, None, &[1.0, 0.0])
        .expect("near");
    c.upsert_embedding(far, "m", "image", None, None, &[0.0, 1.0])
        .expect("far");
    // A vector of another dimension entirely — a model swapped under a stale row.
    c.upsert_embedding(odd, "m", "image", None, None, &[1.0, 0.0, 0.0, 0.0])
        .expect("odd");

    let snapshot = c.embedding_snapshot("m").expect("snapshot");
    let all: std::collections::HashSet<i64> = [near, far, odd].into_iter().collect();
    let ranked = snapshot.index.knn_eligible(&[1.0, 0.0], 3, &all);
    assert_eq!(ranked[0].0, near, "the nearest vector did not rank first");
    assert_eq!(ranked.len(), 3, "a mismatched dimension dropped a row");

    // Exclude the nearest: the answer is the next eligible asset, not an empty slot.
    let without: std::collections::HashSet<i64> = [far, odd].into_iter().collect();
    let ranked = snapshot.index.knn_eligible(&[1.0, 0.0], 2, &without);
    assert!(
        ranked.iter().all(|(id, _)| *id != near),
        "an ineligible asset reached the results"
    );
    assert_eq!(ranked.len(), 2);

    // Soft-deleting is the library's trash: the eligible set follows it without a rebuild.
    c.set_trashed(&[far], true).expect("trash");
    let eligible = c.eligible_for_search(None).expect("eligible");
    assert!(!eligible.contains(&far), "a trashed asset stayed eligible");
    let ranked = snapshot.index.knn_eligible(&[1.0, 0.0], 3, &eligible);
    assert!(ranked.iter().all(|(id, _)| *id != far));
}

/// Several queries arriving at once on a cold cache share one load: every thread comes back
/// holding the very same snapshot, not four copies of the same table.
#[test]
fn concurrent_misses_load_one_snapshot() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("busy.db");
    let mut c = catalog_on_disk(dir.path(), "busy.db");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    for name in ["a.png", "b.png", "c.png"] {
        let id = asset(&mut c, src, name, "image");
        c.upsert_embedding(id, "m", "image", None, None, &[1.0, 0.0])
            .expect("embed");
    }
    drop(c);

    let snapshots: Vec<std::sync::Arc<EmbeddingSnapshot>> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let path = path.clone();
                s.spawn(move || {
                    let cat = Catalog::open(&path).expect("open");
                    cat.embedding_snapshot("m").expect("snapshot")
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("join"))
            .collect()
    });
    assert!(
        snapshots
            .windows(2)
            .all(|w| std::sync::Arc::ptr_eq(&w[0], &w[1])),
        "four concurrent misses built more than one snapshot"
    );
    assert_eq!(snapshots[0].len(), 3);
    // The cache is process-wide and this database is one key in it.
    assert!(resident_snapshot_count() >= 1);
}

/// Both retrievers answer out of the same hard-filter set: a trashed asset whose *name* matches the
/// query must not come back through BM25 while the vector side is excluding it.
#[test]
fn the_keyword_side_of_a_hybrid_search_honours_the_same_filter() {
    let dir = tempfile::tempdir().expect("tmp");
    let mut c = catalog_on_disk(dir.path(), "hybrid.db");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("src");
    let keep = asset(&mut c, src, "rock_keep.png", "image");
    let trashed = asset(&mut c, src, "rock_trashed.png", "image");
    c.upsert_embedding(keep, "m", "image", None, None, &[1.0, 0.0])
        .expect("keep");
    c.upsert_embedding(trashed, "m", "image", None, None, &[1.0, 0.0])
        .expect("trashed");

    let query = [1.0f32, 0.0];
    let spaces: Vec<(&str, &[f32])> = vec![("m", &query)];
    let hits = c.hybrid_search("rock", &spaces, None, 10).expect("search");
    assert!(hits.contains(&keep) && hits.contains(&trashed), "{hits:?}");

    c.set_trashed(&[trashed], true).expect("trash");
    let hits = c.hybrid_search("rock", &spaces, None, 10).expect("search");
    assert!(
        hits.contains(&keep),
        "the live asset stopped matching: {hits:?}"
    );
    assert!(
        !hits.contains(&trashed),
        "a trashed asset came back through the keyword side: {hits:?}"
    );

    // Same query, narrowed to images: a model named the same way matches the keywords and must
    // still be excluded, because the media filter applies to both retrievers.
    let model = asset(&mut c, src, "rock_model.fbx", "model_3d");
    c.upsert_embedding(model, "m", "image", None, None, &[1.0, 0.0])
        .expect("model");
    let images = c
        .media_type_id("image")
        .expect("lookup")
        .expect("image media type");
    let hits = c
        .hybrid_search("rock", &spaces, Some(images), 10)
        .expect("search");
    assert!(hits.contains(&keep), "{hits:?}");
    assert!(
        !hits.contains(&model),
        "a model reached an image-only search through the keyword side: {hits:?}"
    );
}
