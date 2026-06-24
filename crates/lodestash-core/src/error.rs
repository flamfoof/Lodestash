use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("catalog error: {0}")]
    Catalog(String),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T, E = CoreError> = std::result::Result<T, E>;
