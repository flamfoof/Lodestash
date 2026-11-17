//! The catalog's own invariants — the properties every derived surface (routes, MCP tools, the
//! generic forwarder) relies on. Parity with today's router and today's MCP tools is proved in
//! `lodestash-api/tests` and `lodestash-mcp/tests`, against the real servers.

use std::collections::{BTreeSet, HashSet};

use lodestash_protocol::{find_op, Audience, Effect, HttpMethod, OPERATIONS};
use serde_json::Value;

fn schema(f: fn() -> schemars::Schema) -> Value {
    serde_json::to_value(f()).unwrap()
}

fn names(filter: impl Fn(&lodestash_protocol::OpSpec) -> bool) -> BTreeSet<&'static str> {
    OPERATIONS
        .iter()
        .filter(|o| filter(o))
        .map(|o| o.name)
        .collect()
}

#[test]
fn names_are_unique_and_every_address_is_distinct() {
    let mut seen_names = HashSet::new();
    let mut seen_addresses = HashSet::new();
    for op in OPERATIONS {
        assert!(seen_names.insert(op.name), "{} is declared twice", op.name);
        assert!(
            seen_addresses.insert((op.method, op.path)),
            "{} {} is declared twice",
            op.method.as_str(),
            op.path
        );
        assert!(op.path.starts_with("/v1/"), "{}: {}", op.name, op.path);
        assert!(!op.summary.trim().is_empty(), "{} has no summary", op.name);
    }
}

#[test]
fn find_op_finds_exactly_the_declared_operations() {
    for op in OPERATIONS {
        assert!(std::ptr::eq(find_op(op.name).unwrap(), op));
    }
    assert!(find_op("delete_everything").is_none());
}

/// Today's surface: the 27 agent tools and the three Unity-plugin routes, no `Ui` operation yet.
#[test]
fn the_catalog_is_todays_agent_surface_plus_the_plugin_routes() {
    assert_eq!(names(|o| o.audience == Audience::Agent).len(), 27);
    assert_eq!(
        names(|o| o.audience == Audience::Engine),
        BTreeSet::from(["asset_file", "import_queue", "import_queue_ack"])
    );
    assert!(names(|o| o.audience == Audience::Ui).is_empty());
}

/// Exactly the operations today's server parks on the person's approval gate are `Gated`, and the
/// two that start a job without one are `Job`.
#[test]
fn effects_name_the_approval_gate_and_the_jobs() {
    assert_eq!(
        names(|o| o.effect == Effect::Gated),
        BTreeSet::from([
            "export_asset",
            "import_staged",
            "import_to_project",
            "write_tags"
        ])
    );
    assert_eq!(
        names(|o| o.effect == Effect::Job),
        BTreeSet::from(["identify_asset", "unpack_package"])
    );
    for op in OPERATIONS {
        if op.method == HttpMethod::Get {
            assert_eq!(
                op.effect,
                Effect::Read,
                "{} is a GET with side effects",
                op.name
            );
        }
    }
}

/// Every `{name}` in a path is a required field of the operation's input, so binding the path can
/// always succeed for an input the schema accepts.
#[test]
fn every_path_parameter_is_a_required_input_field() {
    for op in OPERATIONS {
        let s = schema(op.input_schema);
        let required: BTreeSet<&str> = s["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        for p in op.path_params() {
            assert!(
                s["properties"].get(p).is_some(),
                "{}: {{{p}}} is not an input field",
                op.name
            );
            assert!(required.contains(p), "{}: {{{p}}} is optional", op.name);
        }
    }
}

/// MCP requires an object input schema; the HTTP adapter assembles an object from path, query and
/// body. Every input is therefore an object.
#[test]
fn every_input_schema_is_an_object() {
    for op in OPERATIONS {
        assert_eq!(schema(op.input_schema)["type"], "object", "{}", op.name);
    }
}

/// A query string carries scalars and lists of scalars, nothing nested — so a GET's non-path
/// fields must be exactly that.
#[test]
fn query_operations_take_only_flat_inputs() {
    fn scalar(s: &Value) -> bool {
        let scalar_name = |t: &Value| {
            matches!(
                t.as_str(),
                Some("string" | "integer" | "number" | "boolean" | "null")
            )
        };
        match &s["type"] {
            Value::String(_) => scalar_name(&s["type"]),
            Value::Array(ts) => ts.iter().all(scalar_name),
            _ => false,
        }
    }
    fn flat(s: &Value) -> bool {
        let is_array = s["type"] == "array"
            || s["type"]
                .as_array()
                .is_some_and(|ts| ts.iter().any(|t| t == "array"));
        if is_array {
            scalar(&s["items"])
        } else {
            scalar(s)
        }
    }
    for op in OPERATIONS
        .iter()
        .filter(|o| matches!(o.method, HttpMethod::Get | HttpMethod::Delete))
    {
        let s = schema(op.input_schema);
        let params: BTreeSet<&str> = op.path_params().collect();
        for (field, prop) in s["properties"].as_object().into_iter().flatten() {
            if !params.contains(field.as_str()) {
                assert!(
                    flat(prop),
                    "{}.{field} cannot travel in a query: {prop}",
                    op.name
                );
            }
        }
    }
}
