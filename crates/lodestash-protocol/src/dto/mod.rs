//! Request types — one per operation input, wire-compatible with v1's JSON.
//!
//! Field doc comments are the descriptions MCP hosts show for each tool argument; keep them
//! written for the agent reading them. Rules every type here follows (crate docs, "Wire rules"):
//!
//! - A field's name is its name on both planes. Where v1 REST spelled a field differently from the
//!   MCP tool, the type accepts both and sends the REST spelling (only `SearchArgs::query`).
//! - `None` serializes as `null`, and a `null` field reads as an absent one.
//! - A default lives in the type only when it is the field's meaning (`namespace` is `free`,
//!   `with_dependencies`); a limit's default and cap belong to the operation, because they come as a
//!   pair and differ per operation.

mod common;
mod discovery;
mod packages;
mod projects;
mod tags;

pub use common::{AssetArgs, JobArgs, NoInput};
pub use discovery::{GetAssetsArgs, ListAssetsArgs, ListLibrariesArgs, SearchArgs, SimilarArgs};
pub use packages::{ExportArgs, UnpackArgs};
pub use projects::{
    AckAsset, AckRequest, ImportQueueArgs, ImportRequest, RegisterProjectArgs, StageArgs,
};
pub use tags::{
    AddTagArgs, IdentifyArgs, MappingArgs, TagBatchAsset, TagBatchRequest, TagIn, TagsArgs,
};
