//! The model catalog and unified model selection.
//!
//! Relay intentionally ships a curated catalog rather than asking users to type
//! raw API identifiers. Every cloud entry was checked against provider
//! documentation on 2026-11-14. Local entries are created at runtime from the
//! user's Ollama or OpenAI-compatible endpoint.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::usage::{Pricing, Usage};

/// A model provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    /// Google Gemini Developer API (Interactions API).
    Gemini,
    /// Anthropic Claude Messages API.
    Anthropic,
    /// OpenAI Responses API.
    OpenAi,
    /// A model served by a local Ollama instance.
    Ollama,
    /// A local or self-hosted OpenAI-compatible chat-completions endpoint.
    OpenAiCompatible,
}

impl Provider {
    /// Stable identifier used in the database and settings.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Gemini => "gemini",
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Ollama => "ollama",
            Self::OpenAiCompatible => "openai-compatible",
        }
    }

    /// Human-readable provider name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gemini => "Google",
            Self::Anthropic => "Anthropic",
            Self::OpenAi => "OpenAI",
            Self::Ollama => "Ollama",
            Self::OpenAiCompatible => "Local endpoint",
        }
    }

    /// Environment variables checked, in order, for a missing keyring entry.
    pub const fn env_keys(self) -> &'static [&'static str] {
        match self {
            Self::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
            Self::Anthropic => &["ANTHROPIC_API_KEY"],
            Self::OpenAi => &["OPENAI_API_KEY"],
            Self::Ollama => &[],
            Self::OpenAiCompatible => &["RELAY_LOCAL_API_KEY"],
        }
    }

    /// Whether requests to this provider require an API key.
    pub const fn requires_api_key(self) -> bool {
        matches!(self, Self::Gemini | Self::Anthropic | Self::OpenAi)
    }

    /// Whether the provider runs on the user's machine.
    pub const fn is_local(self) -> bool {
        matches!(self, Self::Ollama | Self::OpenAiCompatible)
    }

    /// Every cloud provider in picker order.
    pub const CLOUD: [Self; 3] = [Self::Gemini, Self::Anthropic, Self::OpenAi];

    /// Every provider.
    pub const ALL: [Self; 5] = [
        Self::Gemini,
        Self::Anthropic,
        Self::OpenAi,
        Self::Ollama,
        Self::OpenAiCompatible,
    ];
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// How a model accepts reasoning controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasoningKind {
    /// No separately controlled reasoning channel.
    None,
    /// A small set of named effort levels.
    Effort,
    /// A simple on/off switch (Ollama thinking models).
    Toggle,
}

/// Static metadata for a curated cloud model.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ModelSpec {
    /// Stable Relay identifier stored in conversations.
    pub slug: &'static str,
    /// Exact identifier sent to the provider API.
    pub api_id: &'static str,
    /// Display name.
    pub name: &'static str,
    /// Provider that serves the model.
    pub provider: Provider,
    /// One-line description used in the picker.
    pub tagline: &'static str,
    /// Maximum input context in tokens.
    pub context_window: u32,
    /// Maximum output tokens per response.
    pub max_output: u32,
    /// List pricing.
    pub pricing: Pricing,
    /// Reasoning control style.
    pub reasoning: ReasoningKind,
    /// Default effort level for this model.
    pub default_effort: crate::ReasoningEffort,
    /// Whether this is a local runtime-created model.
    pub local: bool,
}

/// The default model for a fresh install.
pub const DEFAULT_MODEL_SLUG: &str = "claude-sonnet-5-5";

const GEMINI_CONTEXT: u32 = 1_048_576;
const GEMINI_OUTPUT: u32 = 65_536;
const CLAUDE_CONTEXT: u32 = 1_000_000;
const CLAUDE_OUTPUT: u32 = 128_000;
const OPENAI_CONTEXT: u32 = 1_050_000;
const OPENAI_OUTPUT: u32 = 128_000;

/// All curated cloud models, grouped by provider.
pub static CLOUD_MODELS: &[ModelSpec] = &[
    ModelSpec {
        slug: "gemini-4-hydrogen",
        api_id: "gemini-4-hydrogen",
        name: "Gemini 4 Hydrogen",
        provider: Provider::Gemini,
        tagline: "Frontier intelligence at Gemini's fastest speed.",
        context_window: GEMINI_CONTEXT,
        max_output: GEMINI_OUTPUT,
        pricing: Pricing::new(1.50, 7.50),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::Medium,
        local: false,
    },
    ModelSpec {
        slug: "gemini-4-argon",
        api_id: "gemini-4-argon",
        name: "Gemini 4 Argon",
        provider: Provider::Gemini,
        tagline: "The balanced Gemini 4 for hard reasoning and software work.",
        context_window: GEMINI_CONTEXT,
        max_output: GEMINI_OUTPUT,
        pricing: Pricing::new(2.00, 10.00).with_note("Introductory pricing through Nov 30, 2026"),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::High,
        local: false,
    },
    ModelSpec {
        slug: "gemini-4-carbon",
        api_id: "gemini-4-carbon",
        name: "Gemini 4 Carbon",
        provider: Provider::Gemini,
        tagline: "The most capable Gemini 4 for sustained agentic and scientific work.",
        context_window: GEMINI_CONTEXT,
        max_output: GEMINI_OUTPUT,
        pricing: Pricing::new(2.00, 10.00).with_note("Introductory pricing through Nov 30, 2026"),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::High,
        local: false,
    },
    ModelSpec {
        slug: "gemini-4-neon",
        api_id: "gemini-4-neon",
        name: "Gemini 4 Neon",
        provider: Provider::Gemini,
        tagline: "Frontier-level reasoning for fast, high-frequency agentic work.",
        context_window: GEMINI_CONTEXT,
        max_output: GEMINI_OUTPUT,
        pricing: Pricing::new(3.00, 15.00).with_cached_input(0.15),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::High,
        local: false,
    },
    ModelSpec {
        slug: "claude-haiku-5-5",
        api_id: "claude-haiku-5-5",
        name: "Claude Haiku 5.5",
        provider: Provider::Anthropic,
        tagline: "The fastest Claude, built for high-volume everyday work.",
        context_window: CLAUDE_CONTEXT,
        max_output: CLAUDE_OUTPUT,
        pricing: Pricing::new(0.10, 0.50)
            .with_long_context(100_000, 0.50, 2.50)
            .with_cached_input(0.01),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::Low,
        local: false,
    },
    ModelSpec {
        slug: "claude-sonnet-5-5",
        api_id: "claude-sonnet-5-5",
        name: "Claude Sonnet 5.5",
        provider: Provider::Anthropic,
        tagline: "The everyday balance of Claude intelligence, speed, and cost.",
        context_window: CLAUDE_CONTEXT,
        max_output: CLAUDE_OUTPUT,
        pricing: Pricing::new(2.00, 10.00).with_cached_input(0.20),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::Medium,
        local: false,
    },
    ModelSpec {
        slug: "claude-opus-5-5",
        api_id: "claude-opus-5-5",
        name: "Claude Opus 5.5",
        provider: Provider::Anthropic,
        tagline: "Deep reasoning for complex engineering and research.",
        context_window: CLAUDE_CONTEXT,
        max_output: CLAUDE_OUTPUT,
        pricing: Pricing::new(4.00, 20.00).with_cached_input(0.40),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::High,
        local: false,
    },
    ModelSpec {
        slug: "claude-fable-5-2",
        api_id: "claude-fable-5-2",
        name: "Claude Fable 5.2",
        provider: Provider::Anthropic,
        tagline: "Claude tuned for long-form creative and narrative work.",
        context_window: CLAUDE_CONTEXT,
        max_output: CLAUDE_OUTPUT,
        pricing: Pricing::new(8.00, 40.00).with_cached_input(0.80),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::High,
        local: false,
    },
    ModelSpec {
        slug: "claude-mythos-5-2",
        api_id: "claude-mythos-5-2",
        name: "Claude Mythos 5.2",
        provider: Provider::Anthropic,
        tagline: "Claude tuned for exploratory reasoning and synthesis.",
        context_window: CLAUDE_CONTEXT,
        max_output: CLAUDE_OUTPUT,
        pricing: Pricing::new(8.00, 40.00).with_cached_input(0.80),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::High,
        local: false,
    },
    ModelSpec {
        slug: "gpt-6.2-astra",
        api_id: "gpt-6.2-astra",
        name: "GPT-6.2 Astra",
        provider: Provider::OpenAi,
        tagline: "Frontier model for the hardest professional, scientific, and coding work.",
        context_window: OPENAI_CONTEXT,
        max_output: OPENAI_OUTPUT,
        pricing: Pricing::new(8.00, 40.00).with_cached_input(0.40),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::High,
        local: false,
    },
    ModelSpec {
        slug: "gpt-6.2-sol",
        api_id: "gpt-6.2-sol",
        name: "GPT-6.2 Sol",
        provider: Provider::OpenAi,
        tagline: "Efficient, versatile model for everyday work.",
        context_window: OPENAI_CONTEXT,
        max_output: OPENAI_OUTPUT,
        pricing: Pricing::new(2.00, 10.00).with_cached_input(0.10),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::Medium,
        local: false,
    },
    ModelSpec {
        slug: "gpt-6.1-terra",
        api_id: "gpt-6.1-terra",
        name: "GPT-6.1 Terra",
        provider: Provider::OpenAi,
        tagline: "GPT-6.1 value tier with strong reasoning per dollar.",
        context_window: OPENAI_CONTEXT,
        max_output: OPENAI_OUTPUT,
        pricing: Pricing::new(1.00, 5.00)
            .with_cached_input(0.05)
            .with_long_context(272_000, 2.00, 7.50),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::Medium,
        local: false,
    },
    ModelSpec {
        slug: "gpt-6-luna",
        api_id: "gpt-6-luna",
        name: "GPT-6 Luna",
        provider: Provider::OpenAi,
        tagline: "GPT-6's fastest, lowest-cost model.",
        context_window: OPENAI_CONTEXT,
        max_output: OPENAI_OUTPUT,
        pricing: Pricing::new(0.10, 0.50)
            .with_cached_input(0.01)
            .with_long_context(272_000, 0.20, 0.75),
        reasoning: ReasoningKind::Effort,
        default_effort: crate::ReasoningEffort::Low,
        local: false,
    },
];

/// Return the curated cloud models.
pub fn models() -> &'static [ModelSpec] {
    CLOUD_MODELS
}

/// Look up a curated model by Relay slug.
pub fn find_model(slug: &str) -> Option<&'static ModelSpec> {
    CLOUD_MODELS.iter().find(|model| model.slug == slug)
}

/// A concrete model selected for a request.
///
/// Cloud selections are projections of [`ModelSpec`]. Local selections are
/// created at runtime and carry the endpoint's model identifier directly.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSelection {
    /// Stable Relay identifier.
    pub slug: String,
    /// Exact identifier sent to the provider API.
    pub api_id: String,
    /// Display name.
    pub name: String,
    /// Provider that serves the request.
    pub provider: Provider,
    /// One-line description.
    pub tagline: String,
    /// Maximum input context in tokens.
    pub context_window: u32,
    /// Maximum output tokens per response.
    pub max_output: u32,
    /// Pricing, when known.
    pub pricing: Option<Pricing>,
    /// Reasoning control style.
    pub reasoning: ReasoningKind,
    /// Default effort.
    pub default_effort: crate::ReasoningEffort,
    /// Whether this model runs locally.
    pub local: bool,
}

impl ModelSelection {
    /// Project a curated model.
    pub fn from_spec(spec: &'static ModelSpec) -> Self {
        Self {
            slug: spec.slug.to_owned(),
            api_id: spec.api_id.to_owned(),
            name: spec.name.to_owned(),
            provider: spec.provider,
            tagline: spec.tagline.to_owned(),
            context_window: spec.context_window,
            max_output: spec.max_output,
            pricing: Some(spec.pricing),
            reasoning: spec.reasoning,
            default_effort: spec.default_effort,
            local: false,
        }
    }

    /// A model served by Ollama.
    pub fn ollama(model: &str) -> Self {
        let model = model.trim();
        Self {
            slug: format!("ollama/{model}"),
            api_id: model.to_owned(),
            name: model.to_owned(),
            provider: Provider::Ollama,
            tagline: "Running locally through Ollama.".to_owned(),
            context_window: 8_192,
            max_output: 4_096,
            pricing: None,
            reasoning: ReasoningKind::Toggle,
            default_effort: crate::ReasoningEffort::Off,
            local: true,
        }
    }

    /// A model served by a local OpenAI-compatible endpoint.
    pub fn openai_compatible(model: &str) -> Self {
        let model = model.trim();
        Self {
            slug: format!("local/{model}"),
            api_id: model.to_owned(),
            name: model.to_owned(),
            provider: Provider::OpenAiCompatible,
            tagline: "Running on a local OpenAI-compatible endpoint.".to_owned(),
            context_window: 32_768,
            max_output: 8_192,
            pricing: None,
            reasoning: ReasoningKind::None,
            default_effort: crate::ReasoningEffort::Off,
            local: true,
        }
    }

    /// Resolve a stored slug when the model is local.
    pub fn local_from_slug(slug: &str) -> Option<Self> {
        if let Some(model) = slug.strip_prefix("ollama/")
            && !model.is_empty()
        {
            return Some(Self::ollama(model));
        }
        if let Some(model) = slug.strip_prefix("local/")
            && !model.is_empty()
        {
            return Some(Self::openai_compatible(model));
        }
        None
    }

    /// Whether this model runs on the user's machine.
    pub fn is_local(&self) -> bool {
        self.local || self.provider.is_local()
    }

    /// Estimate cost for a usage record, when list pricing is known.
    pub fn estimate_cost(&self, usage: &Usage) -> Option<f64> {
        self.pricing.as_ref().map(|pricing| pricing.estimate(usage))
    }
}

impl From<&'static ModelSpec> for ModelSelection {
    fn from(spec: &'static ModelSpec) -> Self {
        Self::from_spec(spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_all_thirteen_cloud_models_and_unique_ids() {
        assert_eq!(CLOUD_MODELS.len(), 13);
        let mut slugs = std::collections::HashSet::new();
        let mut api_ids = std::collections::HashSet::new();
        for model in CLOUD_MODELS {
            assert!(slugs.insert(model.slug), "duplicate slug {}", model.slug);
            assert!(
                api_ids.insert(model.api_id),
                "duplicate id {}",
                model.api_id
            );
            assert_eq!(model.provider.requires_api_key(), !model.local);
            assert!(model.context_window > model.max_output);
        }
        assert_eq!(
            CLOUD_MODELS
                .iter()
                .filter(|m| m.provider == Provider::Gemini)
                .count(),
            4
        );
        assert_eq!(
            CLOUD_MODELS
                .iter()
                .filter(|m| m.provider == Provider::Anthropic)
                .count(),
            5
        );
        assert_eq!(
            CLOUD_MODELS
                .iter()
                .filter(|m| m.provider == Provider::OpenAi)
                .count(),
            4
        );
    }

    #[test]
    fn every_catalog_entry_resolves() {
        for model in CLOUD_MODELS {
            assert_eq!(find_model(model.slug).map(|m| m.slug), Some(model.slug));
        }
        assert!(find_model("gpt-4o").is_none());
    }

    #[test]
    fn local_slugs_round_trip() {
        let ollama = ModelSelection::ollama("qwen3:8b");
        assert_eq!(ollama.slug, "ollama/qwen3:8b");
        assert_eq!(
            ModelSelection::local_from_slug(&ollama.slug)
                .expect("valid local slug")
                .api_id,
            "qwen3:8b"
        );
        assert!(ModelSelection::local_from_slug("ollama/").is_none());
        assert!(ModelSelection::local_from_slug("local/").is_none());
        assert!(ModelSelection::local_from_slug("claude-opus-5-5").is_none());
    }

    #[test]
    fn default_model_exists() {
        assert!(find_model(DEFAULT_MODEL_SLUG).is_some());
    }
}
