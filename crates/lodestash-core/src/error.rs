//! Core's error type.
//!
//! Most failures are internal and carry a message (`Catalog`). The ones a caller has to *act on*
//! differently — answer 404, 409 or 422, or tell the user to upgrade — are their own variants, so
//! an adapter maps them with [`CoreError::kind`] and never by parsing text (v2 spec §4.4).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    /// Internal: something that should not happen did. The message is for a log.
    #[error("catalog error: {0}")]
    Catalog(String),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// The thing named does not exist. The message names it.
    #[error("{0}")]
    NotFound(String),

    /// The request is malformed or breaks a rule; nothing was written. The message says which.
    #[error("{0}")]
    Invalid(String),

    /// The catalog moved on since the caller's view of it, so the change was refused rather than
    /// laid over someone else's. The message says what moved.
    #[error("{0}")]
    Conflict(String),

    /// A write named the annotation revision it read, and the asset is no longer at it.
    /// `revision_conflict` leads the text because it is the stable code agents are told to expect
    /// (ENGINE-API §8.4) and, until the engine maps kinds itself, the text is what reaches them.
    #[error(
        "revision_conflict: asset {asset_id} changed since revision {expected} (now {actual})"
    )]
    RevisionConflict {
        asset_id: i64,
        expected: String,
        actual: String,
    },

    /// The catalog was written by a newer Lodestash than this one.
    #[error(
        "the catalog is at schema v{found}, newer than this build (v{supported}) — update Lodestash"
    )]
    SchemaTooNew { found: u32, supported: u32 },

    /// The catalog predates this build and this connection may not upgrade it (a reader).
    #[error(
        "the catalog is at schema v{found} and this build reads v{supported} — open the Lodestash \
         app once to upgrade it"
    )]
    SchemaTooOld { found: u32, supported: u32 },
}

/// What a caller does about an error — the part of it an adapter maps to a status code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    NotFound,
    Invalid,
    Conflict,
    /// The catalog cannot serve this right now: locked by another writer, on a disk that is not
    /// there, or at a schema this build cannot read. Retrying later (or upgrading) can help.
    Unavailable,
    Internal,
}

impl CoreError {
    pub fn kind(&self) -> ErrorKind {
        match self {
            Self::NotFound(_) => ErrorKind::NotFound,
            Self::Invalid(_) => ErrorKind::Invalid,
            Self::Conflict(_) | Self::RevisionConflict { .. } => ErrorKind::Conflict,
            Self::SchemaTooNew { .. } | Self::SchemaTooOld { .. } | Self::Io(_) => {
                ErrorKind::Unavailable
            }
            Self::Db(e) if is_busy(e) => ErrorKind::Unavailable,
            Self::Catalog(_) | Self::Db(_) | Self::Json(_) => ErrorKind::Internal,
        }
    }
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
