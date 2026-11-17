//! `lodestash-protocol`'s request types read today's bodies exactly as this crate's own types do,
//! so P3 can swap them in without a client noticing (v2 plan Task 2.1).

use lodestash_protocol::dto;
use serde_json::{json, Value};

fn tag_batch_bodies() -> Vec<Value> {
    vec![
        // routes.rs tests, `batch_body`.
        json!({
            "request_id": "8f14e45f-ceea-4e6b-9c39-6a4e2f6b8d11",
            "assets": [{ "asset_id": 3, "expected_revision": "r1-abc", "producer_id": "codex@test",
                         "model_id": "gpt-5-codex", "mode": "append",
                         "tags": [{ "namespace": "subject", "value": "stone", "confidence": 0.8 }] }],
        }),
        // ENGINE-API §8.3.
        json!({
            "request_id": "730a3a8b-61d4-4c29-9253-f96c50a12848",
            "vocabulary_revision": "opaque-vocabulary-revision",
            "assets": [{ "asset_id": 4821, "expected_revision": "opaque-v1-revision",
                         "producer_id": "configured-analysis-host", "model_id": null,
                         "mode": "replace_producer",
                         "tags": [{ "namespace": "subject", "value": "stone arch", "confidence": null }],
                         "description": "A weathered stone arch with a broken upper edge." }],
        }),
        // Fields in an unusual order, and no tags at all.
        json!({
            "assets": [{ "mode": "append", "producer_id": "p", "expected_revision": "r",
                         "asset_id": 1 }],
            "request_id": "6f1c1e0e-3333-4333-8333-333333333333",
        }),
    ]
}

/// A committed `write_tags` request is remembered by a digest of its serialization (`lib.rs`,
/// `request_tag_batch`), and a retry is matched against that receipt. The protocol type must
/// serialize byte-for-byte as this crate's type does, or every retry after P3 would be refused as
/// `request_conflict`.
#[test]
fn a_tag_batch_serializes_byte_for_byte_like_todays_type() {
    for body in tag_batch_bodies() {
        let today: lodestash_api::TagBatchRequest = serde_json::from_value(body.clone()).unwrap();
        let protocol: dto::TagBatchRequest = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(
            serde_json::to_string(&protocol).unwrap(),
            serde_json::to_string(&today).unwrap(),
            "{body}"
        );
    }
    for bad in [
        json!({ "request_id": "r", "assets": [], "extra": 1 }),
        json!({ "assets": [] }),
    ] {
        assert!(serde_json::from_value::<lodestash_api::TagBatchRequest>(bad.clone()).is_err());
        assert!(
            serde_json::from_value::<dto::TagBatchRequest>(bad.clone()).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn an_import_body_reads_as_todays_type_reads_it() {
    for body in [
        // lodestash-mcp `import_to_project`, forwarded to the app.
        json!({ "asset_ids": [4821, 4822], "project_id": null, "dest_subfolder": null,
                "with_dependencies": true }),
        // ENGINE-API §5.2.
        json!({ "asset_ids": [4821], "project_id": 2, "dest_subfolder": "Wildlife",
                "with_dependencies": false }),
    ] {
        let today: lodestash_api::ImportRequest = serde_json::from_value(body.clone()).unwrap();
        let protocol: dto::ImportRequest = serde_json::from_value(body.clone()).unwrap();
        assert_eq!(protocol.asset_ids, today.asset_ids);
        assert_eq!(protocol.project_id, today.project_id);
        assert_eq!(protocol.dest_subfolder, today.dest_subfolder);
        assert_eq!(
            protocol.with_dependencies, today.with_dependencies,
            "{body}"
        );
    }
}

#[test]
fn an_ack_body_reads_as_todays_type_reads_it() {
    let body = json!({ "asset_id": 4821, "dest_path": "Assets/Wildlife/beaver.fbx",
                       "unity_guid": "8e1c" });
    let today: lodestash_api::AckAsset = serde_json::from_value(body.clone()).unwrap();
    let protocol: dto::AckAsset = serde_json::from_value(body).unwrap();
    assert_eq!(
        (protocol.asset_id, protocol.dest_path, protocol.unity_guid),
        (today.asset_id, today.dest_path, today.unity_guid)
    );
}

#[test]
fn the_handshake_file_is_the_same_file() {
    let today = lodestash_api::Handshake::new(51731, "b3f1");
    for h in [
        today.clone(),
        today.with_catalog("e5406dff133f7c9196a15354e6dbbb71"),
    ] {
        let written = serde_json::to_string(&h).unwrap();
        let read: lodestash_protocol::Handshake = serde_json::from_str(&written).unwrap();
        assert_eq!(serde_json::to_string(&read).unwrap(), written);
        let back: lodestash_api::Handshake =
            serde_json::from_str(&serde_json::to_string(&read).unwrap()).unwrap();
        assert_eq!(back, h);
    }
}
