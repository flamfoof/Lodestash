//! Wire compatibility: bodies that today's clients and tests actually send deserialise into the
//! protocol types and serialise back to the same value. "The same" treats a null field as an
//! absent one (crate docs, "Null is absent"); nothing else may differ.
//!
//! Each fixture names where it was captured. Byte-for-byte agreement with `lodestash-api`'s own
//! types (the `write_tags` receipt digest depends on it) is proved in `lodestash-api/tests`.

use lodestash_protocol::dto::{AckRequest, ImportRequest, TagBatchRequest};
use lodestash_protocol::Handshake;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

/// `v` with every null-valued object field removed, at every depth.
fn without_nulls(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k.clone(), without_nulls(v)))
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(without_nulls).collect()),
        other => other.clone(),
    }
}

fn round_trips<T: DeserializeOwned + Serialize>(captured: Value) {
    let parsed: T = serde_json::from_value(captured.clone())
        .unwrap_or_else(|e| panic!("{captured} does not parse: {e}"));
    let again = serde_json::to_value(&parsed).unwrap();
    assert_eq!(without_nulls(&again), without_nulls(&captured));
}

#[test]
fn captured_tag_batch_bodies_round_trip() {
    // lodestash-api routes.rs tests, `batch_body`.
    round_trips::<TagBatchRequest>(json!({
        "request_id": "8f14e45f-ceea-4e6b-9c39-6a4e2f6b8d11",
        "assets": [{
            "asset_id": 3,
            "expected_revision": "r1-abc",
            "producer_id": "codex@test",
            "model_id": "gpt-5-codex",
            "mode": "append",
            "tags": [{ "namespace": "subject", "value": "stone", "confidence": 0.8 }],
        }],
    }));
    // lodestash-mcp lib.rs tests, `write_tags_is_one_contract_on_both_planes`.
    round_trips::<TagBatchRequest>(json!({
        "request_id": "6f1c1e0e-3333-4333-8333-333333333333",
        "assets": [{ "asset_id": 1, "expected_revision": "r1-x", "producer_id": "codex@test",
                     "mode": "append",
                     "tags": [{ "namespace": "subject", "value": "stone", "confidence": 0.5 }] }],
    }));
    // ENGINE-API §8.3, the documented envelope with every optional field present.
    round_trips::<TagBatchRequest>(json!({
        "request_id": "730a3a8b-61d4-4c29-9253-f96c50a12848",
        "vocabulary_revision": "opaque-vocabulary-revision",
        "assets": [{
            "asset_id": 4821,
            "expected_revision": "opaque-v1-revision",
            "producer_id": "configured-analysis-host",
            "model_id": null,
            "mode": "replace_producer",
            "tags": [{ "namespace": "subject", "value": "stone arch", "confidence": null }],
            "description": "A weathered stone arch with a broken upper edge.",
        }],
    }));
}

/// `write_tags` refuses unknown fields at every level (ENGINE-API §8.3).
#[test]
fn a_tag_batch_refuses_unknown_fields() {
    for body in [
        json!({ "request_id": "r", "assets": [], "extra": 1 }),
        json!({ "request_id": "r", "assets": [{ "asset_id": 1, "expected_revision": "x",
                "producer_id": "p", "mode": "append", "colour": "red" }] }),
        json!({ "request_id": "r", "assets": [{ "asset_id": 1, "expected_revision": "x",
                "producer_id": "p", "mode": "append",
                "tags": [{ "namespace": "subject", "value": "v", "weight": 2 }] }] }),
    ] {
        assert!(
            serde_json::from_value::<TagBatchRequest>(body.clone()).is_err(),
            "{body}"
        );
    }
}

#[test]
fn captured_import_bodies_round_trip() {
    // lodestash-mcp lib.rs `import_to_project`, the body it forwards to a running app.
    round_trips::<ImportRequest>(json!({
        "asset_ids": [4821, 4822],
        "project_id": null,
        "dest_subfolder": null,
        "with_dependencies": true,
    }));
    // ENGINE-API §5.2.
    round_trips::<ImportRequest>(json!({
        "asset_ids": [4821], "project_id": 2,
        "dest_subfolder": "Wildlife", "with_dependencies": true,
    }));
}

#[test]
fn captured_ack_body_round_trips() {
    // lodestash-api routes.rs tests, the simulated plugin's ack; the request id is a path field.
    round_trips::<AckRequest>(json!({
        "request_id": "req_3a",
        "imported": [{ "asset_id": 4821, "dest_path": "Assets/Wildlife/beaver.fbx",
                       "unity_guid": "8e1c" }],
    }));
    round_trips::<AckRequest>(json!({ "request_id": "req_3a", "imported": [] }));
}

#[test]
fn the_handshake_file_round_trips() {
    // ENGINE-API §1, with and without the catalog id (a file from before v1 plan S11).
    let full = json!({ "version": 1, "port": 51731, "token": "b3f1", "pid": 8123,
                       "api_version": "v1", "catalog_id": "e5406dff133f7c9196a15354e6dbbb71" });
    let old = json!({ "version": 1, "port": 51731, "token": "b3f1", "pid": 8123,
                      "api_version": "v1" });
    // v2 spec §6: the app also names its own binary, for the MCP server's launcher.
    let with_exe = json!({ "version": 1, "port": 51731, "token": "b3f1", "pid": 8123,
                           "api_version": "v1", "exe": "C:\\Lodestash\\lodestash-desktop.exe" });
    for file in [full, old, with_exe] {
        let h: Handshake = serde_json::from_value(file.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(&h).unwrap(),
            file,
            "absent stays absent"
        );
    }
}

/// The old shell's whole-object Settings write (`crates/lodestash-desktop/ui/src/lib/api.ts`
/// `Settings`, as `SettingsScreen.svelte` sends it with a key typed in) parses into the sectioned
/// DTO and is the same flat object again — the sections are a Rust shape, not a wire one.
#[test]
fn the_old_shells_settings_object_round_trips_flat() {
    let written = json!({
        "m_kb": 1024, "l_kb": 4096, "xl_kb": 8192, "cap_kb": 10240,
        "preview_quality": "medium",
        "skip_onboarding": false, "watch_sources": true, "auto_identify": true,
        "cache_extracted": false, "preview_cache_cap_gb": 8, "auto_purge_preview_cache": true,
        "confirm_before_trash": true, "mcp_disabled_tools": ["export_asset"],
        "tagger_model": null, "tagger_vision_model": null,
        "api_tagging_enabled": true,
        "api_base_url": "https://generativelanguage.googleapis.com/v1beta/openai",
        "api_model": "gemini-flash", "api_key": "sk-typed", "api_key_configured": false,
        "api_max_per_run": 500, "remote_image_bytes": true, "mcp_bytes_enabled": true,
    });
    round_trips::<lodestash_protocol::dto::Settings>(written.clone());

    // A write from a client that never saw the key (a read carries none) leaves it out.
    let mut read = written;
    read.as_object_mut().unwrap().remove("api_key");
    let s: lodestash_protocol::dto::Settings = serde_json::from_value(read).unwrap();
    assert_eq!(s.cloud.api_key, None, "absent is 'keep the stored key'");
    assert!(s.cloud.remote_image_bytes && s.agents.mcp_bytes_enabled);
}
