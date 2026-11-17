//! From an operation's JSON input to the HTTP request that performs it (crate docs, "From input
//! to request"). Pure string work: no client, no I/O.

use std::fmt::Write as _;

use serde_json::{json, Map, Value};

use crate::error::ApiError;
use crate::ops::{of_op, HttpMethod, OpSpec};

/// An operation as an HTTP request, ready for any client to send.
#[derive(Debug, Clone, PartialEq)]
pub struct WireRequest {
    pub method: HttpMethod,
    /// Path and query, percent-encoded: `/v1/assets/7/similar?limit=3`.
    pub target: String,
    /// The JSON body of a POST or PUT (an object, possibly empty); `None` for GET and DELETE.
    pub body: Option<Value>,
}

/// Fill the `{field}` segments of `spec.path` from `input`, and return the path (with the
/// operation's fixed query, if any) and the input's remaining fields — the ones the caller sends
/// as the query (GET, DELETE) or the JSON body (POST, PUT).
///
/// A path field must be present and non-null (else `invalid`, with `detail.field` naming it), an
/// integer or a string; a string is percent-encoded into its one segment, and one that would leave
/// it (`""`, `.`, `..`) is `invalid`. Null fields are dropped from the remainder: absent and null
/// mean the same here.
pub fn bind_path(spec: &OpSpec, input: &Value) -> Result<(String, Map<String, Value>), ApiError> {
    let mut rest = match input {
        Value::Object(fields) => fields.clone(),
        Value::Null => Map::new(),
        other => {
            return Err(ApiError::invalid(format!(
                "{} takes a JSON object, not {}",
                spec.name,
                kind(other)
            )))
        }
    };
    rest.retain(|_, v| !v.is_null());

    let mut path = String::with_capacity(spec.path.len());
    for (i, segment) in spec.route_path().split('/').enumerate() {
        if i > 0 {
            path.push('/');
        }
        match param_name(segment) {
            None => path.push_str(segment),
            Some(field) => {
                let value = rest.remove(field).ok_or_else(|| {
                    ApiError::invalid(format!("{}: {field} is required", spec.name))
                        .with_detail(json!({ "field": field }))
                })?;
                push_segment(&mut path, spec, field, &value)?;
            }
        }
    }
    if let Some(query) = spec.fixed_query() {
        path.push('?');
        path.push_str(query);
    }
    Ok((path, rest))
}

/// The query string for `fields` (no leading `?`). Keys in sorted order; a string, number or
/// boolean is one `key=value` pair; an array is one pair per element under the same key
/// (`tags=a&tags=b`); null and the empty array are absent; anything nested is `invalid`. Keys and
/// values are UTF-8, percent-encoded except for RFC 3986 unreserved characters.
pub fn encode_query(fields: &Map<String, Value>) -> Result<String, ApiError> {
    let mut keys: Vec<&String> = fields.keys().collect();
    keys.sort();
    let mut out = String::new();
    let mut pair = |key: &str, value: &Value| -> Result<(), ApiError> {
        let text = scalar_text(value).ok_or_else(|| {
            ApiError::invalid(format!(
                "{key}: a query carries strings, numbers, booleans and lists of them, not {}",
                kind(value)
            ))
            .with_detail(json!({ "field": key }))
        })?;
        if !out.is_empty() {
            out.push('&');
        }
        percent_encode(key, &mut out);
        out.push('=');
        percent_encode(&text, &mut out);
        Ok(())
    };
    for key in keys {
        match &fields[key] {
            Value::Null => {}
            Value::Array(items) => {
                for item in items {
                    pair(key, item)?;
                }
            }
            value => pair(key, value)?,
        }
    }
    Ok(out)
}

/// See [`OpSpec::request`].
pub(crate) fn request(spec: &OpSpec, input: &Value) -> Result<WireRequest, ApiError> {
    let canonical = (spec.canonical_input)(input).map_err(|e| of_op(spec.name, e))?;
    let (path, mut rest) = bind_path(spec, &canonical)?;
    let (target, body) = match spec.method {
        HttpMethod::Get | HttpMethod::Delete => {
            // The fixed query selects the operation; the input cannot override it.
            for fixed in spec.fixed_query().into_iter().flat_map(|q| q.split('&')) {
                rest.remove(fixed.split_once('=').map_or(fixed, |(key, _)| key));
            }
            let query = encode_query(&rest).map_err(|e| of_op(spec.name, e))?;
            let target = match (query.is_empty(), spec.fixed_query().is_some()) {
                (true, _) => path,
                (false, true) => format!("{path}&{query}"),
                (false, false) => format!("{path}?{query}"),
            };
            (target, None)
        }
        HttpMethod::Post | HttpMethod::Put => (path, Some(Value::Object(rest))),
    };
    Ok(WireRequest {
        method: spec.method,
        target,
        body,
    })
}

/// `{field}` → `field`.
pub(crate) fn param_name(segment: &str) -> Option<&str> {
    segment.strip_prefix('{')?.strip_suffix('}')
}

/// The JSON type of `v`, for messages.
pub(crate) fn kind(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "a list",
        Value::Object(_) => "an object",
    }
}

fn push_segment(
    path: &mut String,
    spec: &OpSpec,
    field: &str,
    value: &Value,
) -> Result<(), ApiError> {
    let refuse = |why: &str| {
        ApiError::invalid(format!("{}: {field} {why}", spec.name))
            .with_detail(json!({ "field": field }))
    };
    match value {
        Value::Number(n) if n.is_i64() || n.is_u64() => {
            let _ = write!(path, "{n}");
            Ok(())
        }
        Value::String(s) if s.is_empty() || s == "." || s == ".." => {
            Err(refuse("must name something: it is empty, `.` or `..`"))
        }
        Value::String(s) => {
            percent_encode(s, path);
            Ok(())
        }
        other => Err(refuse(&format!(
            "must be an integer or a string, not {}",
            kind(other)
        ))),
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// Percent-encode everything but RFC 3986's unreserved characters.
fn percent_encode(s: &str, out: &mut String) {
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
}
