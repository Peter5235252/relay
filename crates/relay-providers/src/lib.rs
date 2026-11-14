#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

//! Streaming model providers for Relay.
//!
//! Every provider is adapted to the same event stream:
//! [`StreamEvent::Text`], [`StreamEvent::Thinking`], [`StreamEvent::Usage`],
//! and [`StreamEvent::Done`]. Requests are always built from canonical history,
//! so no provider-specific state is required to switch models mid-conversation.
//!
//! All requests are cancellable through a [`CancellationToken`]. API keys are
//! held in [`ApiKey`], which zeroizes its buffer on drop and redacts itself from
//! `Debug` output. Keys are only ever sent in headers, never query strings.

#![forbid(unsafe_code)]

mod anthropic;
mod error;
mod gemini;
mod http;
mod local;
mod ndjson;
mod ollama;
mod openai;
mod request;
mod sse;

use std::time::Duration;

use reqwest::Client;
use smol::channel::Sender;
use tokio_util::sync::CancellationToken;

pub use crate::error::ProviderError;
pub use crate::request::{ApiKey, ChatRequest};
pub use relay_core::{ModelSelection, Provider, ReasoningEffort, RequestMessage, Role, Usage};

/// The base URL of the Gemini Developer API.
pub const GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com";
/// The base URL of the Anthropic Messages API.
pub const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
/// The base URL of the OpenAI API.
pub const OPENAI_BASE_URL: &str = "https://api.openai.com";
/// The default local Ollama endpoint.
pub const OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// One event in a streamed model response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// Visible answer text.
    Text(String),
    /// A human-readable reasoning summary.
    Thinking(String),
    /// Token usage, normally sent once at the end of the response.
    Usage(Usage),
    /// The response finished normally.
    Done {
        /// Provider-specific finish reason, when reported.
        stop_reason: Option<String>,
    },
}

/// A reusable HTTP client for every provider.
#[derive(Clone)]
pub struct ProviderClient {
    http: Client,
}

impl std::fmt::Debug for ProviderClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderClient").finish_non_exhaustive()
    }
}

impl ProviderClient {
    /// Create a client with conservative connection and idle-stream timeouts.
    pub fn new() -> Result<Self, ProviderError> {
        let http = Client::builder()
            .user_agent(concat!("Relay/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| ProviderError::Client(error.to_string()))?;
        Ok(Self { http })
    }

    /// Stream one model response, sending events to `events`.
    ///
    /// Errors are returned once; events already sent remain valid partial
    /// output. A closed receiver is treated as cancellation.
    pub async fn stream(
        &self,
        request: &ChatRequest,
        events: &Sender<StreamEvent>,
        cancel: &CancellationToken,
    ) -> Result<(), ProviderError> {
        match request.model.provider {
            Provider::Gemini => gemini::stream(&self.http, request, events, cancel).await,
            Provider::Anthropic => anthropic::stream(&self.http, request, events, cancel).await,
            Provider::OpenAi => openai::stream(&self.http, request, events, cancel).await,
            Provider::Ollama => ollama::stream(&self.http, request, events, cancel).await,
            Provider::OpenAiCompatible => local::stream(&self.http, request, events, cancel).await,
        }
    }

    /// List model names installed in a local Ollama instance.
    pub async fn list_ollama_models(&self, endpoint: &str) -> Result<Vec<String>, ProviderError> {
        ollama::list_models(&self.http, endpoint).await
    }

    /// List model identifiers exposed by an OpenAI-compatible endpoint.
    pub async fn list_local_models(
        &self,
        endpoint: &str,
        api_key: Option<&ApiKey>,
    ) -> Result<Vec<String>, ProviderError> {
        local::list_models(&self.http, endpoint, api_key).await
    }
}

/// Convenience helper used by providers and tests.
pub(crate) async fn emit(
    events: &Sender<StreamEvent>,
    event: StreamEvent,
) -> Result<(), ProviderError> {
    events
        .send(event)
        .await
        .map_err(|_| ProviderError::Cancelled)
}
