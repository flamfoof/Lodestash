//! How an operation's JSON input becomes an HTTP request: path binding, the query-string rule and
//! the body rule (crate docs, "From input to request"). A generic client — the P4 MCP forwarder —
//! does exactly this for every operation, so these are the rules it lives by.

use lodestash_protocol::{bind_path, encode_query, find_op, HttpMethod, OpSpec};
use serde_json::{json, Map, Value};

fn op(name: &str) -> &'static OpSpec {
    find_op(name).unwrap_or_else(|| panic!("{name} is not in the catalog"))
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        other => panic!("not an object: {other}"),
    }
}

#[test]
fn bind_path_fills_path_fields_and_leaves_the_rest() {
    let (path, rest) = bind_path(
        op("add_tag"),
        &json!({ "asset_id": 7, "namespace": "material", "value": "rusty" }),
    )
    .unwrap();
    assert_eq!(path, "/v1/assets/7/tags");
    assert_eq!(
        rest,
        obj(json!({ "namespace": "material", "value": "rusty" }))
    );

    let (path, rest) =
        bind_path(op("get_job_status"), &json!({ "job_id": "export_9f2a" })).unwrap();
    assert_eq!(path, "/v1/jobs/export_9f2a");
    assert!(rest.is_empty());
}

#[test]
fn bind_path_reports_a_missing_path_field_as_invalid() {
    for input in [json!({}), json!({ "job_id": null }), Value::Null] {
        let err = bind_path(op("cancel_job"), &input).unwrap_err();
        assert_eq!(err.code, "invalid", "{input}");
        assert!(err.message.contains("job_id"), "{}", err.message);
        assert_eq!(err.detail, Some(json!({ "field": "job_id" })));
    }
}

/// A path value is one segment, whatever it contains: a separator is encoded, and a value that a
/// URL parser would collapse (`.`/`..`) or that leaves the segment empty is refused rather than
/// sent somewhere else.
#[test]
fn bind_path_keeps_a_value_inside_its_segment() {
    let (path, _) = bind_path(op("get_job_status"), &json!({ "job_id": "a/b c?d" })).unwrap();
    assert_eq!(path, "/v1/jobs/a%2Fb%20c%3Fd");

    for bad in [
        json!(""),
        json!("."),
        json!(".."),
        json!({ "x": 1 }),
        json!([1]),
        json!(true),
    ] {
        let err = bind_path(op("get_job_status"), &json!({ "job_id": bad })).unwrap_err();
        assert_eq!(err.code, "invalid", "{bad}");
    }
}

#[test]
fn bind_path_keeps_an_operations_fixed_query() {
    let (path, rest) = bind_path(op("semantic_search"), &json!({ "query": "mossy wall" })).unwrap();
    assert_eq!(path, "/v1/search?mode=ai");
    assert_eq!(rest, obj(json!({ "query": "mossy wall" })));
    assert_eq!(op("semantic_search").route_path(), "/v1/search");
    assert_eq!(op("semantic_search").fixed_query(), Some("mode=ai"));
    assert_eq!(op("search_assets").fixed_query(), None);
}

/// The query-string rule: keys in sorted order; a scalar is one pair; an array is one pair per
/// element under the same key; null and an empty array are absent; anything nested is refused.
#[test]
fn query_encoding_rule() {
    let q = encode_query(&obj(json!({
        "tags": ["subject:rock", "style:low poly"],
        "limit": 5,
        "q": "stone & moss = 100%",
        "flag": false,
        "ratio": 0.5,
        "cursor": null,
        "none": [],
        "name": "Ünïcødé",
    })))
    .unwrap();
    assert_eq!(
        q,
        "flag=false&limit=5&name=%C3%9Cn%C3%AFc%C3%B8d%C3%A9&q=stone%20%26%20moss%20%3D%20100%25\
         &ratio=0.5&tags=subject%3Arock&tags=style%3Alow%20poly"
    );
    assert_eq!(encode_query(&Map::new()).unwrap(), "");

    for nested in [
        json!({ "a": { "b": 1 } }),
        json!({ "a": [[1]] }),
        json!({ "a": [{ "b": 1 }] }),
    ] {
        assert_eq!(
            encode_query(&obj(nested.clone())).unwrap_err().code,
            "invalid",
            "{nested}"
        );
    }
}

#[test]
fn a_get_request_carries_its_fields_in_the_query() {
    let r = op("search_assets")
        .request(
            &json!({ "query": "stone wall", "tags": ["subject:rock", "style:gothic"], "limit": 5 }),
        )
        .unwrap();
    assert_eq!(r.method, HttpMethod::Get);
    // `q` is the v1 REST name of the search text (ENGINE-API §2); agents call it `query`.
    assert_eq!(
        r.target,
        "/v1/search?limit=5&q=stone%20wall&tags=subject%3Arock&tags=style%3Agothic"
    );
    assert_eq!(r.body, None);

    let r = op("find_similar")
        .request(&json!({ "id": 12, "limit": 3 }))
        .unwrap();
    assert_eq!(r.target, "/v1/assets/12/similar?limit=3");

    let r = op("list_packages").request(&Value::Null).unwrap();
    assert_eq!(r.target, "/v1/packages");
}

/// A fixed query selects a variant of a shared route; the input's fields follow it and cannot
/// override it.
#[test]
fn a_fixed_query_comes_first_and_wins() {
    let r = op("semantic_search")
        .request(&json!({ "query": "metallic clang", "limit": 2 }))
        .unwrap();
    assert_eq!(r.target, "/v1/search?mode=ai&limit=2&q=metallic%20clang");
}

#[test]
fn a_post_request_carries_its_fields_in_the_body() {
    let r = op("add_tag")
        .request(&json!({ "asset_id": 7, "value": "rusty" }))
        .unwrap();
    assert_eq!(r.method, HttpMethod::Post);
    assert_eq!(r.target, "/v1/assets/7/tags");
    // The namespace default is the operation's meaning, so it is on the wire.
    assert_eq!(
        r.body,
        Some(json!({ "namespace": "free", "value": "rusty" }))
    );

    let r = op("cancel_job")
        .request(&json!({ "job_id": "export_1" }))
        .unwrap();
    assert_eq!(r.target, "/v1/jobs/export_1/cancel");
    assert_eq!(r.body, Some(json!({})));

    // Absent `with_dependencies` means "with the closure" — what the tool has always meant.
    let r = op("import_to_project")
        .request(&json!({ "asset_ids": [1, 2] }))
        .unwrap();
    assert_eq!(
        r.body,
        Some(json!({ "asset_ids": [1, 2], "with_dependencies": true }))
    );
}

/// A null field is an absent one, on the way in and on the wire.
#[test]
fn null_is_absent() {
    let r = op("add_tag")
        .request(&json!({ "asset_id": 7, "namespace": null, "value": "rusty" }))
        .unwrap();
    assert_eq!(
        r.body,
        Some(json!({ "namespace": "free", "value": "rusty" }))
    );

    let r = op("export_asset")
        .request(&json!({ "asset_ids": [3], "dest_path": "D:/out", "with_dependencies": null }))
        .unwrap();
    assert_eq!(
        r.body,
        Some(json!({ "asset_ids": [3], "dest_path": "D:/out", "with_dependencies": false }))
    );

    let r = op("list_assets")
        .request(&json!({ "source_id": 1, "cursor": null, "status": null }))
        .unwrap();
    assert_eq!(r.target, "/v1/assets?source_id=1");
}

/// The input is checked against the operation's type before anything is sent: a wrong type, a
/// missing required field or an unknown field where the type refuses them is `invalid`.
#[test]
fn a_request_refuses_input_the_operation_cannot_take() {
    for (name, input) in [
        ("get_asset", json!({ "id": "seven" })),
        ("add_tag", json!({ "asset_id": 7 })),
        ("list_assets", json!({})),
        (
            "write_tags",
            json!({ "request_id": "r", "assets": [], "surprise": 1 }),
        ),
        ("get_assets", json!({ "asset_ids": [1], "extra": true })),
        ("search_assets", json!("not an object")),
    ] {
        let err = op(name).request(&input).unwrap_err();
        assert_eq!(err.code, "invalid", "{name} {input}");
    }
}
