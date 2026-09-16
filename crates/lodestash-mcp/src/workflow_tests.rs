//! Disposable ten-model workflow: the identification bridge is a deterministic stub, never a model call.
use super::*;
use lodestash_core::catalog::{
    Catalog, CrawlRecord, HistoryOrigin, TagAssignment, TagBatch, TagBatchItem, TagMode,
};
use serde_json::{json, Value};

fn body(result: CallToolResult) -> Value {
    assert_ne!(result.is_error, Some(true), "{result:?}");
    let result = serde_json::to_value(result).unwrap();
    serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
}

async fn settle(state: &Arc<ServerState>, id: &str) -> Value {
    for _ in 0..250 {
        let job = state.jobs.get(id).unwrap().to_json();
        if !matches!(
            job["state"].as_str(),
            Some("queued" | "running" | "awaiting_approval")
        ) {
            assert_eq!(job["state"], "done", "{job}");
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("job did not settle");
}

fn assignments(ids: &[i64], source: &str, values: &[&str]) -> TagBatch {
    TagBatch {
        items: ids
            .iter()
            .map(|id| TagBatchItem {
                asset_id: *id,
                source: source.into(),
                model_id: None,
                mode: TagMode::ReplaceSource,
                tags: values
                    .iter()
                    .map(|v| TagAssignment::exact("subject", v))
                    .collect(),
                description: None,
                expected_revision: None,
            })
            .collect(),
        completes_queue: false,
        history: Some(HistoryOrigin::Agent),
        receipt: None,
    }
}

#[tokio::test]
async fn ten_models_identify_propose_review_write_and_read_back_with_a_stub() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("catalog.db");
    let mut cat = Catalog::open(&db).unwrap();
    cat.seed_media_types().unwrap();
    let source = cat
        .upsert_import_source("Models", dir.path().to_str().unwrap(), "folder")
        .unwrap();
    cat.record_crawl(
        source,
        None,
        &(0..15)
            .map(|i| CrawlRecord {
                workspace_rel: None,
                content_hash: None,
                rel_path: format!("model{i}.obj"),
                name: format!("model{i}"),
                media_type: "model_3d".into(),
                size_bytes: 1,
                mtime: 1,
            })
            .collect::<Vec<_>>(),
    )
    .unwrap();
    drop(cat);
    let state =
        Arc::new(ServerState::new(&db, dir.path().to_path_buf(), Box::new(|_| None)).unwrap());
    state.set_agent_producer(Some("fixture-host".into()));
    let writer_db = db.clone();
    state.set_write(Box::new(move |operation| {
        let mut cat = Catalog::open(&writer_db).map_err(|e| e.to_string())?;
        match operation {
            lodestash_api::AgentWrite::Identify { asset_ids } => {
                // Deliberately seed a matching manual association; rejecting AI must preserve it.
                cat.apply_tag_batch(&assignments(&asset_ids, "manual", &["wrong"]))
                    .map_err(|e| e.to_string())?;
                cat.apply_tag_batch(&assignments(&asset_ids, "ai:stub", &["tower", "wrong"]))
                    .map(|out| out.changed_assets.len())
                    .map_err(|e| e.to_string())
            }
            lodestash_api::AgentWrite::TagBatch { request, digest } => cat
                .apply_tag_batch_guarded(
                    &request.to_core_batch(&digest),
                    Some((
                        request.vocabulary_revision.as_deref().unwrap(),
                        "extend_with_approval",
                    )),
                )
                .map(|out| out.changed_assets.len())
                .map_err(|e| e.to_string()),
            lodestash_api::AgentWrite::TagReview {
                batch,
                vocabulary_revision,
            } => cat
                .apply_tag_review_guarded(
                    &batch,
                    Some((&vocabulary_revision, "extend_with_approval")),
                )
                .map(|out| out.changed_assets.len())
                .map_err(|e| e.to_string()),
            lodestash_api::AgentWrite::VocabularyProposal { proposal, digest } => cat
                .apply_vocabulary_proposal(&proposal, "extend_with_approval", &digest)
                .map(|_| 1)
                .map_err(|e| e.to_string()),
            other => Err(format!("unexpected fixture operation {other:?}")),
        }
    }));
    let mcp = Lodestash::new(state.clone());
    let selected = body(
        mcp.list_assets(Parameters(
            serde_json::from_value(json!({
                "source_id":source,"media_type":"model_3d","selection":"random","limit":10
            }))
            .unwrap(),
        ))
        .await
        .unwrap(),
    );
    let ids: Vec<i64> = selected["assets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_i64().unwrap())
        .collect();
    assert_eq!(
        ids.iter().collect::<std::collections::HashSet<_>>().len(),
        10
    );
    assert!(selected["assets"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["media_type"] == "model_3d" && a["source_id"] == source));
    let started = body(
        mcp.identify_asset(Parameters(IdentifyArgs {
            asset_ids: ids.clone(),
        }))
        .await
        .unwrap(),
    );
    settle(&state, started["job_id"].as_str().unwrap()).await;
    let vocabulary = body(
        mcp.get_vocabulary(Parameters(
            serde_json::from_value(json!({"media_type":"model_3d"})).unwrap(),
        ))
        .await
        .unwrap(),
    );
    let proposal = json!({"request_id":"60000000-0000-4000-8000-000000000001","expected_revision":vocabulary["revision"],"changes":[{"kind":"trait","namespace":"subject","value":"tower","scope":"model_3d","teach":false}]});
    let parked = body(
        mcp.propose_vocabulary_changes(Parameters(
            serde_json::from_value(proposal.clone()).unwrap(),
        ))
        .await
        .unwrap(),
    );
    let job = parked["job_id"].as_str().unwrap();
    state.approve_job(job);
    let proposal_result = settle(&state, job).await;
    let replay = body(
        mcp.propose_vocabulary_changes(Parameters(serde_json::from_value(proposal).unwrap()))
            .await
            .unwrap(),
    );
    assert_eq!(replay["result"], proposal_result["result"]);
    let vocabulary = body(
        mcp.get_vocabulary(Parameters(
            serde_json::from_value(json!({"media_type":"model_3d"})).unwrap(),
        ))
        .await
        .unwrap(),
    );
    let before = state
        .read_assets(&lodestash_api::ReadAssetsRequest {
            asset_ids: ids.clone(),
            source_id: Some(source),
        })
        .unwrap();
    let review = json!({"request_id":"60000000-0000-4000-8000-000000000002","vocabulary_revision":vocabulary["revision"],"assets":before["assets"].as_array().unwrap().iter().map(|a| json!({
        "asset_id":a["id"],"expected_revision":a["annotation_revision"],"decisions":[
            {"namespace":"subject","value":"tower","source":"ai:stub","action":"accept","reason":"Stub evidence matches the approved trait"},
            {"namespace":"subject","value":"wrong","source":"ai:stub","action":"reject","reason":"Stub evidence contradicts this assignment"}
        ]})).collect::<Vec<_>>()});
    let parked = body(
        mcp.review_tags(Parameters(serde_json::from_value(review).unwrap()))
            .await
            .unwrap(),
    );
    let job = parked["job_id"].as_str().unwrap();
    state.approve_job(job);
    settle(&state, job).await;
    let fresh = state
        .read_assets(&lodestash_api::ReadAssetsRequest {
            asset_ids: ids.clone(),
            source_id: Some(source),
        })
        .unwrap();
    let write = json!({"request_id":"60000000-0000-4000-8000-000000000003","vocabulary_revision":vocabulary["revision"],"assets":fresh["assets"].as_array().unwrap().iter().map(|a| json!({
        "asset_id":a["id"],"expected_revision":a["annotation_revision"],"producer_id":"fixture-host","mode":"replace_producer",
        "tags":[{"namespace":"subject","value":"tower"}],"description":"A synthetic tower used by the workflow fixture."
    })).collect::<Vec<_>>()});
    let parked = body(
        mcp.write_tags(Parameters(serde_json::from_value(write.clone()).unwrap()))
            .await
            .unwrap(),
    );
    let job = parked["job_id"].as_str().unwrap();
    state.approve_job(job);
    let committed = settle(&state, job).await;
    let replay = body(
        mcp.write_tags(Parameters(serde_json::from_value(write).unwrap()))
            .await
            .unwrap(),
    );
    assert_eq!(
        replay["result"]["operation_id"],
        committed["result"]["operation_id"]
    );
    let final_read = body(
        mcp.get_assets(Parameters(
            serde_json::from_value(json!({"asset_ids":ids,"source_id":source})).unwrap(),
        ))
        .await
        .unwrap(),
    );
    assert_eq!(
        final_read["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["id"].as_i64().unwrap())
            .collect::<Vec<_>>(),
        ids
    );
    let independent = Catalog::open_reader(&db).unwrap();
    for id in ids {
        let rows = independent.asset_tag_rows(id).unwrap();
        assert!(rows
            .iter()
            .any(|r| r.source == "manual" && r.value == "wrong"));
        assert!(rows
            .iter()
            .any(|r| r.source == "ai:stub" && r.value == "tower"));
        assert!(rows
            .iter()
            .any(|r| r.source == "ai:fixture-host" && r.value == "tower"));
        assert!(!rows
            .iter()
            .any(|r| r.source == "ai:stub" && r.value == "wrong"));
    }
}

#[test]
fn authoring_planes_reject_unknown_review_fields_and_null_descriptions() {
    let review = json!({"request_id":"60000000-0000-4000-8000-000000000004","vocabulary_revision":"v","assets":[{"asset_id":1,"expected_revision":"r","decisions":[{"namespace":"subject","value":"tower","source":"ai:stub","action":"accept","reason":"test","confirmed":true}]}]});
    assert!(serde_json::from_value::<ReviewTagsArgs>(review.clone()).is_err());
    assert!(serde_json::from_value::<lodestash_api::TagReviewRequest>(review).is_err());
    let write = json!({"request_id":"60000000-0000-4000-8000-000000000004","vocabulary_revision":"v","assets":[{"asset_id":1,"expected_revision":"r","producer_id":"test","mode":"append","tags":[],"description":null}]});
    assert!(serde_json::from_value::<WriteTagsArgs>(write.clone()).is_err());
    assert!(serde_json::from_value::<lodestash_api::TagBatchRequest>(write).is_err());
}
