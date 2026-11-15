//! Provider-neutral request values.

use relay_core::{ModelSelection, ReasoningEffort, RequestMessage, ToolSpec};
use zeroize::Zeroize;

/// An API key that zeroizes its buffer when dropped.
///
/// `Debug` output is always redacted so keys cannot leak into logs or panic
/// messages.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wrap a key. Leading and trailing whitespace is removed; an empty key is
    /// still accepted so callers can report a clear authentication error.
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into().trim().to_owned())
    }

    /// Expose the secret to a header builder.
    pub fn expose(&self) -> &str {
        &self.0
    }

    /// Whether the key has no content.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

impl Drop for ApiKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A fully specified streaming request.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// Selected model and its metadata.
    pub model: ModelSelection,
    /// Canonical, alternating conversation turns. System turns are ignored.
    pub messages: Vec<RequestMessage>,
    /// Composed system prompt.
    pub system_prompt: Option<String>,
    /// Sampling temperature for providers that accept it.
    pub temperature: f64,
    /// Output cap. Zero uses the model default.
    pub max_output_tokens: u32,
    /// Requested reasoning effort.
    pub reasoning: ReasoningEffort,
    /// Credential for the selected provider, when required.
    pub api_key: Option<ApiKey>,
    /// Optional base URL override. Used by local endpoints, enterprise
    /// proxies, and tests. Cloud providers default to their public API.
    pub endpoint: Option<String>,
    /// Tools available to the model.
    pub tools: Vec<ToolSpec>,
    /// Gemini Interactions API: previous interaction id for multi-turn function calling.
    pub previous_interaction_id: Option<String>,
}

impl ChatRequest {
    /// Start a request for a model and canonical history.
    pub fn new(model: ModelSelection, messages: Vec<RequestMessage>) -> Self {
        let reasoning = model.default_effort;
        Self {
            model,
            messages,
            system_prompt: None,
            temperature: 0.7,
            max_output_tokens: 0,
            reasoning,
            api_key: None,
            endpoint: None,
            tools: Vec::new(),
            previous_interaction_id: None,
        }
    }

    /// Set the composed system prompt.
    pub fn with_system_prompt(mut self, prompt: Option<String>) -> Self {
        self.system_prompt = prompt.filter(|prompt| !prompt.trim().is_empty());
        self
    }

    /// Set the reasoning effort.
    pub fn with_reasoning(mut self, reasoning: ReasoningEffort) -> Self {
        self.reasoning = reasoning;
        self
    }

    /// Set the output cap. Zero means the model default.
    pub fn with_max_output_tokens(mut self, max_output_tokens: u32) -> Self {
        self.max_output_tokens = max_output_tokens;
        self
    }

    /// Set the sampling temperature.
    pub fn with_temperature(mut self, temperature: f64) -> Self {
        self.temperature = temperature;
        self
    }

    /// Attach an API key.
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(ApiKey::new(api_key));
        self
    }

    /// Attach available tools.
    pub fn with_tools(mut self, tools: Vec<ToolSpec>) -> Self {
        self.tools = tools;
        self
    }

    /// Override the provider base URL.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        let endpoint = endpoint.into();
        let trimmed = endpoint.trim().trim_end_matches('/');
        self.endpoint = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_owned())
        };
        self
    }

    /// Resolve the output limit for this request.
    pub fn output_limit(&self) -> u32 {
        let chosen = if self.max_output_tokens == 0 {
            self.model.max_output
        } else {
            self.max_output_tokens
        };
        let chosen = if self.model.local {
            chosen
        } else {
            chosen.min(self.model.max_output)
        };
        chosen.max(1)
    }

    /// The system prompt, trimmed, when non-empty.
    pub fn system_prompt(&self) -> Option<&str> {
        self.system_prompt
            .as_deref()
            .map(str::trim)
            .filter(|prompt| !prompt.is_empty())
    }
}
