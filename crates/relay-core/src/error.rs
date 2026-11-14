use thiserror::Error;

/// Errors produced by the provider-neutral core.
#[derive(Debug, Error)]
pub enum CoreError {
    /// A model slug in stored data no longer exists in the catalog.
    #[error("unknown model: {0}")]
    UnknownModel(String),
    /// A local model slug was malformed.
    #[error("invalid local model reference: {0}")]
    InvalidLocalModel(String),
    /// A request could not fit in the model's context window.
    #[error("conversation does not fit in the model context window")]
    ContextOverflow,
    /// Serialized content was not valid canonical JSON.
    #[error("invalid message content: {0}")]
    InvalidContent(String),
}

/// Convenient result alias for the core crate.
pub type Result<T> = std::result::Result<T, CoreError>;
