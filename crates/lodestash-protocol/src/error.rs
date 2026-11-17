//! The error envelope every operation answers with (ENGINE-API §4, v2 spec §4.4).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Stable error kinds of the v2 engine (spec §4.4). The HTTP adapter maps each to its status;
/// operations may also answer the finer v1 codes listed in ENGINE-API §4 and §8.4.
pub mod codes {
    pub const NOT_FOUND: &str = "not_found";
    pub const INVALID: &str = "invalid";
    pub const CONFLICT: &str = "conflict";
    pub const DENIED: &str = "denied";
    pub const UNAVAILABLE: &str = "unavailable";
    pub const BUSY: &str = "busy";
    pub const TOO_LARGE: &str = "too_large";
    pub const INTERNAL: &str = "internal";
}

/// One failure: a stable machine `code`, a sentence for a person, and optional structured detail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    /// What a caller needs beyond the message — the offending field, a hint, a conflict's sides.
    /// v1 handlers send a string here (often empty); v2 operations send structured values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

impl ApiError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            detail: None,
        }
    }

    /// The caller sent something the operation cannot take (`invalid`).
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(codes::INVALID, message)
    }

    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }

    /// This error as the body an HTTP response carries.
    pub fn into_envelope(self) -> ErrorEnvelope {
        ErrorEnvelope { error: self }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

/// `{"error": {code, message, detail?}}` — the body of every non-2xx response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}
