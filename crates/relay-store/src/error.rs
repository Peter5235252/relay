use thiserror::Error;

/// Storage errors surfaced to the UI.
#[derive(Debug, Error)]
pub enum StoreError {
    /// SQLite returned an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// JSON data in the database was malformed.
    #[error("invalid stored data: {0}")]
    Json(#[from] serde_json::Error),
    /// The data directory could not be created or accessed.
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    /// A thread panicked while holding the database lock.
    #[error("database lock was poisoned")]
    Lock,
    /// A requested conversation does not exist.
    #[error("conversation not found: {0}")]
    NotFound(String),
    /// Built-in presets cannot be edited or deleted.
    #[error("built-in presets cannot be modified: {0}")]
    BuiltinPreset(String),
    /// A stored timestamp was not valid RFC 3339.
    #[error("invalid timestamp in database: {0}")]
    Timestamp(String),
    /// A message row had an unrecognized role.
    #[error("invalid message role in database: {0}")]
    InvalidRole(String),
    /// The OS data directory could not be determined.
    #[error("could not determine a data directory; set RELAY_DATA_DIR")]
    NoDataDirectory,
}

/// Convenient result alias for storage operations.
pub type Result<T> = std::result::Result<T, StoreError>;
