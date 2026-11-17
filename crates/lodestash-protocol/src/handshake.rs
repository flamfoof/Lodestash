//! The `engine.json` handshake (ENGINE-API §1): how a client finds the app's port and token.
//!
//! The same fields and serde shape as `lodestash-api`'s `Handshake`, which writes the file today;
//! the file's location and writing stay with the server (this crate does no I/O).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Handshake {
    pub version: u32,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub api_version: String,
    /// The catalog the app serving this port has open (M31). Absent from a file written before
    /// v1 plan S11, and then treated as a different catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_id: Option<String>,
    /// The binary serving this port (v2 spec §6). The MCP server launches it again when the app is
    /// gone. Absent from a file written by an app that does not record it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exe: Option<String>,
}
