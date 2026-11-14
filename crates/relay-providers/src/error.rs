//! Typed provider failures.

use relay_core::Provider;
use thiserror::Error;

/// A provider request or stream failure.
#[derive(Debug, Error)]
pub enum ProviderError {
    /// A cloud provider was selected without a key.
    #[error("{provider} API key is missing or empty")]
    MissingApiKey {
        /// Provider that rejected the request.
        provider: Provider,
    },

    /// A credential could not be used as an HTTP header.
    #[error("{provider} API key contains invalid characters")]
    InvalidApiKey {
        /// Provider that rejected the key.
        provider: Provider,
    },

    /// The network request failed.
    #[error("{provider} request failed: {message}")]
    Transport {
        /// Provider that was called.
        provider: Provider,
        /// Redacted transport error.
        message: String,
    },

    /// The provider returned a non-success status.
    #[error("{provider} returned HTTP {status}: {message}")]
    Status {
        /// Provider that was called.
        provider: Provider,
        /// HTTP status code.
        status: u16,
        /// Redacted and truncated error body.
        message: String,
    },

    /// The stream ended with a transport error.
    #[error("{provider} stream failed: {message}")]
    Stream {
        /// Provider that was called.
        provider: Provider,
        /// Redacted transport error.
        message: String,
    },

    /// The provider did not produce data before the idle timeout.
    #[error("{provider} stopped responding")]
    Timeout {
        /// Provider that was called.
        provider: Provider,
    },

    /// The provider reported an error inside the event stream.
    #[error("{provider} reported an error: {message}")]
    Remote {
        /// Provider that reported the error.
        provider: Provider,
        /// Provider-supplied message.
        message: String,
    },

    /// A response line was not valid JSON or had an unexpected shape.
    #[error("could not decode the {provider} response: {message}")]
    Decode {
        /// Provider that was called.
        provider: Provider,
        /// Decoder message.
        message: String,
    },

    /// The endpoint was empty or not HTTP(S).
    #[error("invalid endpoint: {0}")]
    InvalidEndpoint(String),

    /// The HTTP client could not be initialized.
    #[error("could not initialize the HTTP client: {0}")]
    Client(String),

    /// The request was cancelled by the user or window shutdown.
    #[error("request cancelled")]
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_key_error_names_the_provider() {
        let error = ProviderError::MissingApiKey {
            provider: Provider::Anthropic,
        };
        assert_eq!(error.to_string(), "Anthropic API key is missing or empty");
    }
}
