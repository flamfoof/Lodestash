//! Lodestash's wire protocol: every `/v1` operation, declared once.
//!
//! Spec: `docs/superpowers/specs/2026-11-16-v2-architecture-design.md` §5 (the catalog) and §7
//! (the MCP server that forwards through it); paths and shapes: `docs/ENGINE-API.md` §2–§8. Wire
//! types only — no I/O, no catalog, no runtime.
//!
//! # The rules
//!
//! - **One declaration per operation.** [`OPERATIONS`], the [`Operations`] trait and [`dispatch`]
//!   are generated from the single list in `ops.rs`. A surface that exposes operations — the HTTP
//!   router, the MCP server, the UI's client — derives them from the catalog; it does not keep a
//!   list of its own.
//! - **`Agent` ⇒ an MCP tool and an HTTP route.** The MCP↔REST 1:1 invariant is a property of the
//!   declaration, not of a parity test. `Engine` operations (the Unity plugin's plumbing) and `Ui`
//!   operations are HTTP only.
//! - **Paths follow ENGINE-API §2** — no `/v1` break. A `{field}` segment is filled from the input
//!   field of that name, so a placeholder is named after the field: `/v1/jobs/{job_id}` is the
//!   address ENGINE-API writes `/v1/jobs/{id}`. A trailing `?k=v` is a fixed query selecting one
//!   variant of a route two operations share (`semantic_search` is `GET /v1/search?mode=ai`); a
//!   router registers [`OpSpec::route_path`] once and tells the variants apart by that query.
//! - **One input type per operation, for both planes.** It is the MCP tool's input schema and the
//!   REST request's shape (path + query, or path + body). See [`dto`] for the wire rules its types
//!   follow.
//!
//! # From input to request
//!
//! [`OpSpec::request`] turns an operation's JSON input into the HTTP request that performs it, the
//! same way for every operation, so a forwarder needs no per-operation code:
//!
//! 1. **Check and canonicalise.** The input is decoded into the operation's input type and
//!    encoded again: a refusal is `invalid` before anything is sent, defaults are filled in, and
//!    fields take their wire spelling.
//! 2. **Bind the path** ([`bind_path`]). Each `{field}` is taken out of the input and written into
//!    its segment, percent-encoded. A missing one is `invalid` with `detail.field` naming it.
//! 3. **Send the rest.** GET and DELETE: as the query ([`encode_query`]), after the fixed query if
//!    the path has one — an input field cannot override the fixed query. POST and PUT: as the JSON
//!    body, always an object (`{}` when nothing is left).
//!
//! **The query-string rule.** Keys in sorted order. A string, number or boolean is one
//! `key=value` pair. An array is one pair per element under the same key (`tags=a&tags=b`), the
//! convention v1 REST reads. Null and the empty array are absent. Anything nested is `invalid` —
//! the catalog keeps GET inputs flat, and a test holds it to that. Keys and values are UTF-8,
//! percent-encoded except for RFC 3986's unreserved characters. Decoding needs the input type to
//! tell a one-element list from a scalar: a field is a list when the input schema says `array`.
//!
//! **Null is absent.** A null field reads as an absent one — on decoding, and in the remainder
//! [`bind_path`] returns — so `{"namespace": null}` means the namespace's default.

#[macro_use]
mod macros;

pub mod dto;
mod error;
mod handshake;
mod ops;
mod wire;

/// The schema library the catalog's schemas are built with, for callers that read them.
pub use schemars;

pub use error::{codes, ApiError, ErrorEnvelope};
pub use handshake::Handshake;
pub use ops::{
    dispatch, find_op, Audience, Caller, Effect, HttpMethod, OpSpec, Operations, OPERATIONS,
};
pub use wire::{bind_path, encode_query, WireRequest};
