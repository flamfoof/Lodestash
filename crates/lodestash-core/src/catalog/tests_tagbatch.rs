//! The tag batch contract (v1 plan S04, ADR-0017): every check the plan names for it.

use super::*;

fn catalog_with(names: &[(&str, &str)]) -> (Catalog, Vec<i64>) {
    let mut c = Catalog::open_in_memory().expect("open");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    let records: Vec<CrawlRecord> = names
        .iter()
        .map(|(rel, media)| CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: rel.to_string(),
            name: rel.to_string(),
            media_type: media.to_string(),
            size_bytes: 1,
            mtime: 1,
        })
        .collect();
    c.record_crawl(src, None, &records).expect("crawl");
    let ids = names
        .iter()
        .map(|(rel, _)| {
            c.conn
                .query_row("SELECT id FROM asset WHERE name = ?1", [rel], |r| r.get(0))
                .expect("id")
        })
        .collect();
    (c, ids)
}

fn ai(value: &str, confidence: f64) -> TagAssignment {
    TagAssignment {
        namespace: "subject".into(),
        value: value.into(),
        confidence: Some(confidence),
    }
}

fn item(
    asset_id: i64,
    source: &str,
    model: Option<&str>,
    mode: TagMode,
    tags: Vec<TagAssignment>,
) -> TagBatchItem {
    TagBatchItem {
        asset_id,
        source: source.into(),
        model_id: model.map(str::to_string),
        mode,
        tags,
        description: None,
        expected_revision: None,
    }
}

fn batch(
    items: Vec<TagBatchItem>,
    completes_queue: bool,
    history: Option<HistoryOrigin>,
) -> TagBatch {
    TagBatch {
        items,
        completes_queue,
        history,
        receipt: None,
    }
}

type RowView = (String, String, Option<f64>, Option<String>);

/// `(source, label, confidence, model)` for one asset, sorted — the whole provenance picture.
fn rows(c: &Catalog, asset: i64) -> Vec<RowView> {
    let mut stmt = c
        .conn
        .prepare(
            "SELECT at.source, t.name, at.confidence, at.model_id
               FROM asset_tag at JOIN tag t ON t.id = at.tag_id
              WHERE at.asset_id = ?1 ORDER BY 1, 2",
        )
        .expect("prep");
    stmt.query_map([asset], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
    })
    .expect("query")
    .collect::<std::result::Result<Vec<_>, _>>()
    .expect("rows")
}

fn row(source: &str, label: &str, confidence: Option<f64>, model: Option<&str>) -> RowView {
    (
        source.to_string(),
        label.to_string(),
        confidence,
        model.map(str::to_string),
    )
}

fn op_count(c: &Catalog) -> i64 {
    c.conn
        .query_row("SELECT COUNT(*) FROM op_log", [], |r| r.get(0))
        .expect("count")
}

fn tags_dirty(c: &Catalog, asset: i64) -> i64 {
    c.conn
        .query_row("SELECT tags_dirty FROM asset WHERE id = ?1", [asset], |r| {
            r.get(0)
        })
        .expect("dirty")
}

use HistoryOrigin::{Agent, Background, User};
use TagMode::{Append, RejectAllSources, RemoveScoped, ReplaceSource};

/// A person's tag and two models' results live side by side, and re-running one model replaces
/// that model's set and nothing else — not the person's, not the other model's.
#[test]
fn manual_and_two_producers_coexist_and_a_rerun_replaces_only_its_own() {
    let (mut c, ids) = catalog_with(&[("rock.png", "image")]);
    let a = ids[0];
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "manual",
            None,
            Append,
            vec![TagAssignment::exact("free", "mine")],
        )],
        false,
        Some(User),
    ))
    .expect("manual");
    c.apply_tag_batch(&batch(
        vec![
            item(
                a,
                "ai:vlm@local",
                Some("qwen3:8b"),
                ReplaceSource,
                vec![ai("stone", 0.9), ai("moss", 0.6)],
            ),
            item(
                a,
                "ai:siglip-zeroshot",
                Some("siglip2"),
                ReplaceSource,
                vec![ai("stone", 0.7), ai("cliff", 0.5)],
            ),
        ],
        true,
        Some(Background),
    ))
    .expect("two producers");
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "ai:vlm@local",
            Some("qwen3:8b"),
            ReplaceSource,
            vec![ai("boulder", 0.8)],
        )],
        true,
        Some(Background),
    ))
    .expect("rerun");
    assert_eq!(
        rows(&c, a),
        vec![
            row("ai:siglip-zeroshot", "cliff", Some(0.5), Some("siglip2")),
            row("ai:siglip-zeroshot", "stone", Some(0.7), Some("siglip2")),
            row("ai:vlm@local", "boulder", Some(0.8), Some("qwen3:8b")),
            row("manual", "mine", None, None),
        ]
    );

    // A model-authored row with no score is still AI: provenance says so, not the confidence.
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "ai:agent-host",
            None,
            Append,
            vec![TagAssignment::exact("subject", "granite")],
        )],
        false,
        None,
    ))
    .expect("unscored");
    let granite = rows(&c, a)
        .into_iter()
        .find(|r| r.1 == "granite")
        .expect("granite");
    assert!(is_ai_source(&granite.0) && granite.2.is_none());
}

/// Nine spellings of one label are one assignment — deduplication runs before the per-namespace
/// cap of eight, and keeps the most confident guess. Nine *different* labels are refused.
#[test]
fn labels_are_normalised_and_deduplicated_before_the_caps() {
    let (mut c, ids) = catalog_with(&[("wall.png", "image")]);
    let a = ids[0];
    let spellings = [
        "stone wall",
        "Stone Wall",
        "STONE WALL",
        "stone_wall",
        "stone.wall",
        " stone  wall ",
        "Stone_Wall",
        "stone   wall",
        "STONE_WALL",
    ];
    let tags: Vec<_> = spellings
        .iter()
        .enumerate()
        .map(|(i, s)| ai(s, 0.1 + i as f64 * 0.05))
        .collect();
    c.apply_tag_batch(&batch(
        vec![item(a, "ai:m", Some("m"), ReplaceSource, tags)],
        true,
        None,
    ))
    .expect("nine spellings of one label");
    let got = rows(&c, a);
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(
        got[0].2,
        Some(0.1 + 8.0 * 0.05),
        "the most confident spelling's score is kept"
    );

    let nine: Vec<_> = (0..9).map(|i| ai(&format!("thing{i}x"), 0.5)).collect();
    let err = c
        .apply_tag_batch(&batch(
            vec![item(a, "ai:m", Some("m"), ReplaceSource, nine)],
            true,
            None,
        ))
        .expect_err("nine distinct values in one namespace");
    assert!(err.to_string().contains("per namespace"), "{err}");
}

/// A model's output is untrusted even from a local model. A score in the label slot, a
/// non-finite or out-of-range confidence, a namespace it may not author, an overlong label — each
/// refuses the **whole** batch before anything is written, the queue flag included.
#[test]
fn a_score_in_the_label_slot_and_bad_confidences_are_refused_whole() {
    let (mut c, ids) = catalog_with(&[("rock.png", "image")]);
    let a = ids[0];
    let bad = [
        ai("0.87", 0.5),
        ai("{\"confidence\": 0.9}", 0.5),
        ai("stone", f64::NAN),
        ai("stone", f64::INFINITY),
        ai("stone", 1.5),
        ai("stone", -0.1),
        TagAssignment {
            namespace: "technical".into(),
            value: "2k".into(),
            confidence: None,
        },
        ai(&"x".repeat(MAX_LABEL_CHARS + 1), 0.5),
    ];
    for b in bad {
        let shown = format!("{b:?}");
        let err = c.apply_tag_batch(&batch(
            vec![item(a, "ai:m", Some("m"), Append, vec![ai("fine", 0.5), b])],
            true,
            Some(Background),
        ));
        assert!(err.is_err(), "accepted {shown}");
        assert!(
            rows(&c, a).is_empty(),
            "{shown}: the valid half was written"
        );
        assert_eq!(
            tags_dirty(&c, a),
            1,
            "{shown}: a refused batch cleared the queue"
        );
        assert_eq!(op_count(&c), 0, "{shown}: a refused batch left history");
    }
    // A person's tag is exact — it carries no score — and nobody types a technical fact.
    for (tag, why) in [
        (ai("stone", 0.5), "manual with a confidence"),
        (TagAssignment::exact("technical", "2k"), "manual technical"),
    ] {
        assert!(
            c.apply_tag_batch(&batch(
                vec![item(a, "manual", None, Append, vec![tag])],
                false,
                None
            ))
            .is_err(),
            "accepted {why}"
        );
    }
    for source in ["ai", LEGACY_AI_SOURCE, "robot", "ai:"] {
        assert!(
            c.apply_tag_batch(&batch(
                vec![item(a, source, None, Append, vec![ai("stone", 0.5)])],
                false,
                None
            ))
            .is_err(),
            "accepted source {source:?}"
        );
    }
    assert!(
        c.apply_tag_batch(&batch(
            vec![item(
                a,
                "ai:m",
                None,
                RejectAllSources,
                vec![ai("stone", 0.5)]
            )],
            false,
            None
        ))
        .is_err(),
        "a model rejected a term across every source"
    );
}

/// One id that does not exist refuses the batch — the assets before and after it in the list are
/// untouched, and no history is left behind.
#[test]
fn a_missing_asset_refuses_the_whole_batch() {
    let (mut c, ids) = catalog_with(&[("a.png", "image"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    let tag = || vec![TagAssignment::exact("free", "crate")];
    let err = c.apply_tag_batch(&batch(
        vec![
            item(a, "manual", None, Append, tag()),
            item(999_999, "manual", None, Append, tag()),
            item(b, "manual", None, Append, tag()),
        ],
        false,
        Some(User),
    ));
    assert!(err.is_err());
    assert!(rows(&c, a).is_empty() && rows(&c, b).is_empty());
    assert_eq!(op_count(&c), 0);
}

/// A hundred-asset edit is one history row: one undo takes all hundred back, one redo re-applies
/// them. A hundred and one assets is over the batch limit.
#[test]
fn a_hundred_asset_edit_undoes_and_redoes_as_one() {
    let names: Vec<(String, &str)> = (0..101).map(|i| (format!("a{i}.png"), "image")).collect();
    let refs: Vec<(&str, &str)> = names.iter().map(|(n, m)| (n.as_str(), *m)).collect();
    let (mut c, ids) = catalog_with(&refs);
    let tag = || vec![TagAssignment::exact("free", "crate")];
    let hundred: Vec<_> = ids[..100]
        .iter()
        .map(|&id| item(id, "manual", None, Append, tag()))
        .collect();
    let out = c
        .apply_tag_batch(&batch(hundred, false, Some(User)))
        .expect("hundred");
    assert_eq!(out.changed_assets.len(), 100);
    let op = out.op_id.expect("recorded");
    assert_eq!(
        c.recent_ops(10).expect("recent").len(),
        1,
        "one row for the whole edit"
    );

    c.revert_op(op).expect("undo");
    assert!(
        ids[..100].iter().all(|&id| rows(&c, id).is_empty()),
        "undo took all hundred back"
    );
    c.redo_op(op).expect("redo");
    assert!(
        ids[..100].iter().all(|&id| rows(&c, id).len() == 1),
        "redo re-applied all hundred"
    );
    assert_eq!(
        c.recent_ops(10).expect("recent").len(),
        1,
        "redo puts the edit back on the undo stack"
    );
    c.revert_op(op).expect("undo the redone edit");
    assert!(
        ids[..100].iter().all(|&id| rows(&c, id).is_empty()),
        "a redone edit undoes again"
    );
    c.redo_op(op).expect("redo again");

    let all: Vec<_> = ids
        .iter()
        .map(|&id| item(id, "manual", None, Append, tag()))
        .collect();
    assert!(
        c.apply_tag_batch(&batch(all, false, Some(User))).is_err(),
        "101 assets accepted"
    );
}

/// Retention is partitioned by origin. The interactive tail keeps its 200 newest rows, and a
/// thousand background results after a person's edit neither push that edit out of reach nor
/// clear it: the background lane is capped at its own 200.
#[test]
fn history_keeps_two_hundred_interactive_rows_whatever_the_background_writes() {
    let (mut c, ids) = catalog_with(&[("a.png", "image"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    for i in 0..201 {
        c.apply_tag_batch(&batch(
            vec![item(
                a,
                "manual",
                None,
                Append,
                vec![TagAssignment::exact("free", &format!("t{i}"))],
            )],
            false,
            Some(User),
        ))
        .expect("user op");
    }
    assert_eq!(c.recent_ops(1000).expect("recent").len(), 200);

    let keep = c
        .apply_tag_batch(&batch(
            vec![item(
                a,
                "manual",
                None,
                Append,
                vec![TagAssignment::exact("free", "keep")],
            )],
            false,
            Some(User),
        ))
        .expect("the edit that must survive")
        .op_id
        .expect("recorded");
    for i in 0..1000 {
        c.apply_tag_batch(&batch(
            vec![item(
                b,
                "ai:m",
                Some("m"),
                ReplaceSource,
                vec![ai(&format!("guess{}", i % 7), 0.5)],
            )],
            true,
            Some(Background),
        ))
        .expect("background result");
    }
    let background: i64 = c
        .conn
        .query_row(
            "SELECT COUNT(*) FROM op_log WHERE origin = 'background'",
            [],
            |r| r.get(0),
        )
        .expect("count");
    assert_eq!(background, 200, "the background lane keeps its own 200");
    let recent = c.recent_ops(1).expect("recent");
    assert_eq!(
        recent[0].id, keep,
        "the person's last edit is still the undo target"
    );
    c.revert_op(keep).expect("and it still undoes");
    assert!(!rows(&c, a).iter().any(|r| r.1 == "keep"));
}

/// A write that changes nothing consumes no history and moves no rows — so it publishes no change.
#[test]
fn a_no_op_writes_and_records_nothing() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let mine = || {
        vec![item(
            a,
            "manual",
            None,
            Append,
            vec![TagAssignment::exact("free", "mine")],
        )]
    };
    c.apply_tag_batch(&batch(mine(), false, Some(User)))
        .expect("first");
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "ai:m",
            Some("m"),
            ReplaceSource,
            vec![ai("stone", 0.5)],
        )],
        false,
        Some(Background),
    ))
    .expect("model");

    let before = c.total_changes();
    let again = c
        .apply_tag_batch(&batch(mine(), false, Some(User)))
        .expect("again");
    let same = c
        .apply_tag_batch(&batch(
            vec![item(
                a,
                "ai:m",
                Some("m"),
                ReplaceSource,
                vec![ai("stone", 0.5)],
            )],
            false,
            Some(Background),
        ))
        .expect("same model result");
    assert_eq!((again.op_id, same.op_id), (None, None));
    assert_eq!(again.unchanged_assets, vec![a]);
    assert_eq!(c.total_changes(), before, "a no-op wrote something");
    assert_eq!(op_count(&c), 2);
}

/// Completion is explicit. Only a batch that says the configured tagger finished clears the queue;
/// a person's edit is not model work, and neither is an agent's annotation.
#[test]
fn only_finished_model_work_leaves_the_queue() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    assert_eq!(tags_dirty(&c, a), 1);
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "manual",
            None,
            Append,
            vec![TagAssignment::exact("free", "mine")],
        )],
        false,
        Some(User),
    ))
    .expect("manual");
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "ai:agent-host",
            None,
            Append,
            vec![ai("stone", 0.5)],
        )],
        false,
        Some(Agent),
    ))
    .expect("agent");
    assert_eq!(
        tags_dirty(&c, a),
        1,
        "neither a person nor an agent finished the tagger's work"
    );
    c.apply_tag_batch(&batch(
        vec![item(a, "ai:m", Some("m"), ReplaceSource, vec![])],
        true,
        None,
    ))
    .expect("nothing to say");
    assert_eq!(
        tags_dirty(&c, a),
        0,
        "an empty result is still a finished one"
    );
}

/// No batch confirms an asset. Confirmation is a person's act on the asset, not a tag write.
#[test]
fn no_batch_confirms_an_asset() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let status = |c: &Catalog| -> String {
        c.conn
            .query_row("SELECT status FROM asset WHERE id = ?1", [a], |r| r.get(0))
            .expect("status")
    };
    let before = status(&c);
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "ai:agent-host",
            None,
            Append,
            vec![ai("stone", 0.99)],
        )],
        false,
        Some(Agent),
    ))
    .expect("agent");
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "manual",
            None,
            Append,
            vec![TagAssignment::exact("free", "mine")],
        )],
        false,
        Some(User),
    ))
    .expect("manual");
    assert_eq!(status(&c), before);
    assert_ne!(status(&c), "confirmed");
}

/// Undo replays a snapshot only where the associations still look the way the operation left
/// them. The same producer writing again since is a conflict, reported and left alone; a newer
/// write by an **independent** producer is not in the way, and survives the undo.
#[test]
fn undo_refuses_to_overwrite_a_newer_write_by_the_same_producer() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let first = c
        .apply_tag_batch(&batch(
            vec![item(a, "ai:host", None, Append, vec![ai("stone", 0.5)])],
            false,
            Some(Agent),
        ))
        .expect("agent")
        .op_id
        .expect("op");
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "ai:host",
            None,
            ReplaceSource,
            vec![ai("granite", 0.6)],
        )],
        false,
        Some(Agent),
    ))
    .expect("the same producer again");
    let err = c.revert_op(first).expect_err("undo over a newer write");
    assert!(matches!(err, CoreError::Conflict(_)), "{err:?}");
    assert_eq!(
        rows(&c, a),
        vec![row("ai:host", "granite", Some(0.6), None)],
        "left alone"
    );

    let (mut c, ids) = catalog_with(&[("b.png", "image")]);
    let b = ids[0];
    let mine = c
        .apply_tag_batch(&batch(
            vec![item(b, "ai:x", None, Append, vec![ai("stone", 0.5)])],
            false,
            Some(Agent),
        ))
        .expect("x")
        .op_id
        .expect("op");
    c.apply_tag_batch(&batch(
        vec![item(b, "ai:y", Some("y1"), Append, vec![ai("moss", 0.4)])],
        true,
        Some(Background),
    ))
    .expect("an independent producer");
    c.revert_op(mine)
        .expect("independent writes are not a conflict");
    assert_eq!(
        rows(&c, b),
        vec![row("ai:y", "moss", Some(0.4), Some("y1"))]
    );
}

/// Undoing a person's rejection puts every removed association back exactly — each source's own
/// row, with its score and model — not re-added as the person's tag.
#[test]
fn undoing_a_rejection_restores_every_source_exactly() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    c.apply_tag_batch(&batch(
        vec![
            item(
                a,
                "manual",
                None,
                Append,
                vec![TagAssignment::exact("subject", "stone")],
            ),
            item(
                a,
                "ai:vlm",
                Some("qwen3:8b"),
                Append,
                vec![ai("stone", 0.7), ai("moss", 0.3)],
            ),
        ],
        false,
        None,
    ))
    .expect("seed");
    let before = rows(&c, a);
    let op = c
        .apply_tag_batch(&batch(
            vec![item(
                a,
                "manual",
                None,
                RejectAllSources,
                vec![TagAssignment::exact("subject", "stone")],
            )],
            false,
            Some(User),
        ))
        .expect("reject")
        .op_id
        .expect("op");
    assert_eq!(
        rows(&c, a),
        vec![row("ai:vlm", "moss", Some(0.3), Some("qwen3:8b"))]
    );
    c.revert_op(op).expect("undo the rejection");
    assert_eq!(rows(&c, a), before);

    // And removing a manual tag removes only the manual row.
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "manual",
            None,
            RemoveScoped,
            vec![TagAssignment::exact("subject", "stone")],
        )],
        false,
        None,
    ))
    .expect("remove mine");
    assert!(rows(&c, a)
        .iter()
        .any(|r| r.0 == "ai:vlm" && r.1 == "stone"));
}

/// The configured caption producer's caption lands in the same transaction as its tags, is
/// searchable at once, and undoes with them.
#[test]
fn a_caption_lands_with_its_tags_and_undoes_with_them() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let mut it = item(
        a,
        "ai:vlm",
        Some("qwen3:8b"),
        ReplaceSource,
        vec![ai("stone", 0.8)],
    );
    it.description = Some("A mossy stone wall.".into());
    let op = c
        .apply_tag_batch(&batch(vec![it], true, Some(Background)))
        .expect("caption")
        .op_id
        .expect("op");
    assert_eq!(c.search_assets("mossy", 10).unwrap(), Some(vec![a]));
    assert_eq!(c.search_assets("stone", 10).unwrap(), Some(vec![a]));
    c.revert_op(op).expect("undo");
    let caption: Option<String> = c
        .conn
        .query_row("SELECT description FROM asset WHERE id = ?1", [a], |r| {
            r.get(0)
        })
        .expect("caption");
    assert_eq!(caption, None, "the caption went back with the tags");
    assert_eq!(c.search_assets("mossy", 10).unwrap(), Some(vec![]));

    let mut one = item(a, "ai:vlm", None, Append, vec![]);
    one.description = Some("one".into());
    let mut two = item(a, "ai:other", None, Append, vec![]);
    two.description = Some("two".into());
    assert!(
        c.apply_tag_batch(&batch(vec![one, two], false, None))
            .is_err(),
        "two captions"
    );
    let mut empty = item(a, "ai:vlm", None, Append, vec![]);
    empty.description = Some("   ".into());
    assert!(
        c.apply_tag_batch(&batch(vec![empty], false, None)).is_err(),
        "empty caption"
    );
}

/// Tag ops recorded before M26 carry only an inverse — "remove this tag", "re-add it as manual" —
/// which could delete another source's copy or re-attribute a model's row to the user. They refuse
/// to replay, and say why.
#[test]
fn legacy_tag_ops_refuse_to_replay_and_say_why() {
    let mut c = Catalog::open_in_memory().expect("open");
    let op = c
        .record_op(
            "bulk_tag",
            r#"{"asset_ids":[1],"namespace":"free","value":"x","on":true}"#,
            None,
        )
        .expect("legacy row");
    let err = c.revert_op(op).expect_err("replayed a legacy op");
    assert!(err.to_string().contains("M26"), "{err}");
    assert_eq!(c.recent_ops(10).expect("recent").len(), 1, "the row stays");
}

/// M26: every pre-provenance AI row becomes `ai:legacy` with its recorded model kept, a person's
/// rows are untouched, and old history belongs to the interactive tail.
#[test]
fn m26_marks_old_ai_rows_legacy_and_keeps_their_model() {
    let conn = Connection::open_in_memory().expect("open");
    configure_connection(&conn).expect("pragmas");
    let mut c = Catalog {
        conn,
        db_path: None,
        appraise_focus: AppraiseFocus::default(),
    };
    c.apply_migrations_up_to(25).expect("m25");
    c.seed_media_types().expect("seed");
    let src = c
        .upsert_import_source("Lib", "C:/lib", "folder")
        .expect("source");
    c.record_crawl(
        src,
        None,
        &[CrawlRecord {
            workspace_rel: None,
            content_hash: None,
            rel_path: "a.png".into(),
            name: "a.png".into(),
            media_type: "image".into(),
            size_bytes: 1,
            mtime: 1,
        }],
    )
    .expect("crawl");
    c.conn
        .execute_batch(
            "INSERT INTO tag (name, norm_name, namespace) VALUES ('stone', 'subject:stone', 'subject');
             INSERT INTO asset_tag (asset_id, tag_id, source, confidence, model_id, created_at)
                  SELECT a.id, t.id, 'ai', 0.7, 'qwen3:8b', 0 FROM asset a, tag t;
             INSERT INTO asset_tag (asset_id, tag_id, source, confidence, model_id, created_at)
                  SELECT a.id, t.id, 'manual', NULL, NULL, 0 FROM asset a, tag t;
             INSERT INTO op_log (op_kind, payload_json, created_at) VALUES ('set_trashed', '{}', 0);",
        )
        .expect("pre-M26 rows");

    c.apply_migrations_up_to(26).expect("m26");

    let a: i64 = c
        .conn
        .query_row("SELECT id FROM asset", [], |r| r.get(0))
        .expect("id");
    assert_eq!(
        rows(&c, a),
        vec![
            row(LEGACY_AI_SOURCE, "stone", Some(0.7), Some("qwen3:8b")),
            row("manual", "stone", None, None),
        ]
    );
    let origin: String = c
        .conn
        .query_row("SELECT origin FROM op_log", [], |r| r.get(0))
        .expect("origin");
    assert_eq!(origin, "user");
    assert_eq!(
        c.recent_ops(10).expect("recent").len(),
        1,
        "old history is still undoable"
    );
}

/// A writer that names the revision it read is refused if anything moved since — tags or caption —
/// and the refusal is the whole batch. The revision itself only moves when an annotation does.
#[test]
fn a_stale_revision_refuses_the_batch_and_a_fresh_one_passes() {
    let (mut c, ids) = catalog_with(&[("a.png", "image"), ("b.png", "image")]);
    let (a, b) = (ids[0], ids[1]);
    let seen = c.annotation_revision(a).expect("revision");
    assert_eq!(
        seen,
        c.annotation_revision(a).expect("again"),
        "a read does not move it"
    );

    c.apply_tag_batch(&batch(
        vec![item(a, "ai:m", Some("m"), Append, vec![ai("stone", 0.5)])],
        true,
        None,
    ))
    .expect("someone else writes");
    let moved = c.annotation_revision(a).expect("moved");
    assert_ne!(seen, moved, "a tag write moves the revision");

    let mut stale = item(a, "ai:host", None, Append, vec![ai("granite", 0.4)]);
    stale.expected_revision = Some(seen);
    let err = c
        .apply_tag_batch(&batch(
            vec![
                item(b, "ai:host", None, Append, vec![ai("moss", 0.4)]),
                stale,
            ],
            false,
            Some(Agent),
        ))
        .expect_err("a stale revision");
    assert!(
        matches!(err, CoreError::RevisionConflict { asset_id, .. } if asset_id == a),
        "{err:?}"
    );
    assert!(
        rows(&c, b).is_empty(),
        "the other asset in the batch was written anyway"
    );

    let mut fresh = item(a, "ai:host", None, Append, vec![ai("granite", 0.4)]);
    fresh.expected_revision = Some(moved.clone());
    c.apply_tag_batch(&batch(vec![fresh], false, Some(Agent)))
        .expect("a fresh revision passes");

    let before_caption = c.annotation_revision(a).expect("rev");
    let mut captioned = item(a, "ai:vlm", None, Append, vec![]);
    captioned.description = Some("A stone.".into());
    c.apply_tag_batch(&batch(vec![captioned], false, None))
        .expect("caption");
    assert_ne!(
        before_caption,
        c.annotation_revision(a).expect("rev"),
        "a caption moves it too"
    );
}

/// A receipt is written in the batch's own transaction: it exists exactly when the write does. A
/// refused batch leaves none, and a no-op still leaves one — a retry must find its answer.
#[test]
fn a_request_receipt_commits_with_its_batch_and_only_then() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let key = |id: &str| TagReceiptKey {
        request_id: id.into(),
        digest: format!("digest-{id}"),
    };
    let mut refused = batch(
        vec![item(a, "ai:host", None, Append, vec![ai("0.9", 0.5)])],
        false,
        Some(Agent),
    );
    refused.receipt = Some(key("r1"));
    assert!(c.apply_tag_batch(&refused).is_err());
    assert_eq!(
        c.tag_request_receipt("r1").expect("read"),
        None,
        "a refused batch left a receipt"
    );

    let mut ok = batch(
        vec![item(a, "ai:host", None, Append, vec![ai("stone", 0.5)])],
        false,
        Some(Agent),
    );
    ok.receipt = Some(key("r2"));
    let out = c.apply_tag_batch(&ok).expect("write");
    let receipt = c.tag_request_receipt("r2").expect("read").expect("receipt");
    assert_eq!(receipt.digest, "digest-r2");
    assert_eq!(receipt.result["operation_id"], serde_json::json!(out.op_id));
    assert_eq!(receipt.result["changed_assets"], serde_json::json!([a]));

    let mut again = ok.clone();
    again.receipt = Some(key("r3"));
    c.apply_tag_batch(&again).expect("no-op");
    let noop = c
        .tag_request_receipt("r3")
        .expect("read")
        .expect("a no-op still answers");
    assert_eq!(noop.result["unchanged_assets"], serde_json::json!([a]));

    let mut reused = ok.clone();
    reused.receipt = Some(key("r2"));
    assert!(
        c.apply_tag_batch(&reused).is_err(),
        "one request id, one write"
    );
}

// ---- typed error kinds (v2 plan Task 1.1) ----------------------------------------------------
//
// Each refusal is its own variant, produced by the operation that means it, so an adapter maps
// 404/409/422 from `CoreError::kind` instead of reading the message.

/// A write that names a revision the asset has moved past is a `RevisionConflict` carrying both
/// revisions — what the caller read and what is there now — and nothing in the batch is written.
#[test]
fn a_stale_revision_is_a_revision_conflict_naming_both_revisions() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let seen = c.annotation_revision(a).expect("revision");
    c.apply_tag_batch(&batch(
        vec![item(a, "ai:m", Some("m"), Append, vec![ai("stone", 0.5)])],
        true,
        None,
    ))
    .expect("someone else writes");
    let now = c.annotation_revision(a).expect("moved");

    let mut stale = item(a, "ai:host", None, Append, vec![ai("granite", 0.4)]);
    stale.expected_revision = Some(seen.clone());
    let err = c
        .apply_tag_batch(&batch(vec![stale], false, Some(Agent)))
        .expect_err("a stale revision");
    match &err {
        CoreError::RevisionConflict {
            asset_id,
            expected,
            actual,
        } => assert_eq!((*asset_id, expected, actual), (a, &seen, &now)),
        other => panic!("not a revision conflict: {other:?}"),
    }
    assert_eq!(err.kind(), crate::error::ErrorKind::Conflict);
    assert_eq!(
        rows(&c, a),
        vec![row("ai:m", "stone", Some(0.5), Some("m"))],
        "the refused batch wrote nothing"
    );
}

/// Undoing an edit whose associations another write has since replaced is a `Conflict`: the newer
/// write is left alone, and the caller can tell that apart from a failure.
#[test]
fn an_undo_over_a_newer_write_is_a_conflict() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let first = c
        .apply_tag_batch(&batch(
            vec![item(a, "ai:host", None, Append, vec![ai("stone", 0.5)])],
            false,
            Some(Agent),
        ))
        .expect("agent")
        .op_id
        .expect("op");
    c.apply_tag_batch(&batch(
        vec![item(
            a,
            "ai:host",
            None,
            ReplaceSource,
            vec![ai("granite", 0.6)],
        )],
        false,
        Some(Agent),
    ))
    .expect("the same producer again");

    let err = c.revert_op(first).expect_err("undo over a newer write");
    assert!(matches!(err, CoreError::Conflict(_)), "{err:?}");
    assert_eq!(err.kind(), crate::error::ErrorKind::Conflict);
    assert_eq!(
        rows(&c, a),
        vec![row("ai:host", "granite", Some(0.6), None)],
        "the newer write is left as it is"
    );
}

/// A batch that breaks a rule — the same asset twice under one source, or an asset that does not
/// exist — is `Invalid`, and nothing in it is written.
#[test]
fn a_malformed_tag_batch_is_invalid() {
    let (mut c, ids) = catalog_with(&[("a.png", "image")]);
    let a = ids[0];
    let twice = batch(
        vec![
            item(a, "ai:host", None, Append, vec![ai("stone", 0.5)]),
            item(a, "ai:host", None, Append, vec![ai("moss", 0.5)]),
        ],
        false,
        Some(Agent),
    );
    let missing = batch(
        vec![
            item(a, "ai:host", None, Append, vec![ai("stone", 0.5)]),
            item(a + 1000, "ai:host", None, Append, vec![ai("moss", 0.5)]),
        ],
        false,
        Some(Agent),
    );
    for (why, b) in [("twice", twice), ("missing asset", missing)] {
        let err = c.apply_tag_batch(&b).expect_err(why);
        assert!(matches!(err, CoreError::Invalid(_)), "{why}: {err:?}");
        assert_eq!(err.kind(), crate::error::ErrorKind::Invalid, "{why}");
    }
    assert!(rows(&c, a).is_empty(), "a refused batch wrote nothing");
}
