//! Application settings and user-facing reasoning controls.

use serde::{Deserialize, Serialize};

use crate::DEFAULT_MODEL_SLUG;

/// How much reasoning effort to request from models that expose it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningEffort {
    /// Ask the model not to reason when it supports that; otherwise use its
    /// lowest setting.
    Off,
    /// Fast, shallow reasoning.
    Low,
    /// The default balance.
    #[default]
    Medium,
    /// Deeper reasoning for hard problems.
    High,
    /// The provider's maximum setting.
    Max,
}

impl ReasoningEffort {
    /// All levels, from least to most effort.
    pub const ALL: [Self; 5] = [Self::Off, Self::Low, Self::Medium, Self::High, Self::Max];

    /// Short label for settings and pickers.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
            Self::Max => "Max",
        }
    }

    /// Longer explanation for settings.
    pub const fn description(self) -> &'static str {
        match self {
            Self::Off => "Skip deliberate reasoning when the model allows it.",
            Self::Low => "Fast responses with light reasoning.",
            Self::Medium => "A balanced default for most work.",
            Self::High => "More thought for hard or ambiguous problems.",
            Self::Max => "The deepest setting the provider offers.",
        }
    }

    /// Gemini `thinking_level` value.
    pub const fn gemini_level(self) -> Option<&'static str> {
        match self {
            Self::Off => Some("minimal"),
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Max => Some("xhigh"),
        }
    }

    /// Anthropic `output_config.effort` value, or `None` to disable thinking.
    pub const fn anthropic_effort(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Max => Some("max"),
        }
    }

    /// OpenAI Responses `reasoning.effort` value. `Off` omits the reasoning
    /// object rather than sending a level the model may not accept.
    pub const fn openai_effort(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Max => Some("high"),
        }
    }

    /// Ollama `think` value.
    pub const fn ollama_level(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Low => Some("low"),
            Self::Medium => Some("medium"),
            Self::High => Some("high"),
            Self::Max => Some("max"),
        }
    }
}

/// Light, dark, or follow the desktop.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Appearance {
    /// Follow the desktop's color scheme.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

impl Appearance {
    /// Stable settings representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }
}

use crate::tool::{AgentMode, ToolApproval};

/// Curated interface color preset. Presets are tuned to remain readable in both
/// light and dark appearance modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorPreset {
    /// Use the default Relay/kit theme accents.
    #[default]
    Default,
    /// Blue accents.
    Ocean,
    /// Teal/green accents.
    Forest,
    /// Warm rose accents.
    Rose,
    /// Amber accents.
    Amber,
    /// Violet accents.
    Violet,
    /// Neutral graphite accents.
    Graphite,
}

impl ColorPreset {
    /// All presets in display order.
    pub const ALL: [Self; 7] = [
        Self::Default,
        Self::Ocean,
        Self::Forest,
        Self::Rose,
        Self::Amber,
        Self::Violet,
        Self::Graphite,
    ];

    /// Stable id for persistence and lookups.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Ocean => "ocean",
            Self::Forest => "forest",
            Self::Rose => "rose",
            Self::Amber => "amber",
            Self::Violet => "violet",
            Self::Graphite => "graphite",
        }
    }

    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Ocean => "Ocean",
            Self::Forest => "Forest",
            Self::Rose => "Rose",
            Self::Amber => "Amber",
            Self::Violet => "Violet",
            Self::Graphite => "Graphite",
        }
    }

    /// Resolve from a stored id/label.
    pub fn from_str(value: &str) -> Self {
        let v = value.trim().to_ascii_lowercase();
        match v.as_str() {
            "ocean" | "blue" => Self::Ocean,
            "forest" | "green" | "teal" => Self::Forest,
            "rose" | "pink" => Self::Rose,
            "amber" | "orange" => Self::Amber,
            "violet" | "purple" => Self::Violet,
            "graphite" | "slate" | "neutral" => Self::Graphite,
            _ => Self::Default,
        }
    }
}

/// Non-secret application preferences, persisted in SQLite.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub default_model: String,
    /// Color scheme.
    pub appearance: Appearance,
    /// Agent operating mode for terminal tools.
    pub agent_mode: AgentMode,
    /// Auto-approval policy for tool execution.
    pub tool_approval: ToolApproval,
    /// UI font family.
    pub font_family: String,
    /// Monospace font family for code.
    pub mono_font_family: String,
    /// Base UI font size in pixels.
    pub font_size: f32,
    /// Curated interface color preset.
    pub color_preset: ColorPreset,
    /// User-authored instructions appended to the active personality preset.
    pub custom_instructions: String,
    /// Active built-in or user preset id.
    pub active_preset_id: String,
    /// Show reasoning summaries in the transcript.
    pub show_thinking: bool,
    /// Submit the composer with Enter (Shift+Enter inserts a newline).
    pub send_on_enter: bool,
    /// Sampling temperature for providers that accept it.
    pub temperature: f64,
    /// Optional output cap. Zero uses the model default.
    pub max_output_tokens: u32,
    /// Requested reasoning effort.
    pub reasoning: ReasoningEffort,
    /// Ollama endpoint.
    pub ollama_base_url: String,
    /// Preferred Ollama model.
    pub ollama_model: String,
    /// OpenAI-compatible endpoint.
    pub local_base_url: String,
    /// Preferred local endpoint model.
    pub local_model: String,
    /// Selected bundled Nemotron variant id (e.g. standard/uncensored).
    pub bundled_model_variant: String,
    /// Whether the welcome panel has been dismissed.
    pub onboarding_complete: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            default_model: DEFAULT_MODEL_SLUG.to_owned(),
            appearance: Appearance::System,
            agent_mode: AgentMode::Plan,
            tool_approval: ToolApproval::None,
            font_family: "Fira Sans".to_owned(),
            mono_font_family: "DejaVu Sans Mono".to_owned(),
            font_size: 15.0,
            color_preset: ColorPreset::Default,
            custom_instructions: String::new(),
            active_preset_id: "balanced".to_owned(),
            show_thinking: true,
            send_on_enter: true,
            temperature: 0.7,
            max_output_tokens: 0,
            reasoning: ReasoningEffort::Medium,
            ollama_base_url: "http://localhost:11434".to_owned(),
            ollama_model: String::new(),
            local_base_url: "http://localhost:8000/v1".to_owned(),
            local_model: String::new(),
            bundled_model_variant: "Nemotron-3.5-Bolt-E4B-Q4_K_M".to_owned(),
            onboarding_complete: false,
        }
    }
}

impl AppSettings {
    /// Clamp values and normalize endpoints before saving.
    pub fn normalize(&mut self) {
        self.default_model = self.default_model.trim().to_owned();
        if self.default_model.is_empty()
            || (crate::find_model(&self.default_model).is_none()
                && crate::ModelSelection::local_from_slug(&self.default_model).is_none())
        {
            self.default_model = DEFAULT_MODEL_SLUG.to_owned();
        }
        self.temperature = self.temperature.clamp(0.0, 2.0);
        self.max_output_tokens = self.max_output_tokens.min(1_000_000);
        self.font_size = self.font_size.clamp(11.0, 24.0);
        if self.font_family.trim().is_empty() {
            self.font_family = "Fira Sans".to_owned();
        }
        if self.mono_font_family.trim().is_empty() {
            self.mono_font_family = "DejaVu Sans Mono".to_owned();
        }
        self.ollama_base_url = normalize_url(&self.ollama_base_url);
        self.local_base_url = normalize_url(&self.local_base_url);
        self.ollama_model = self.ollama_model.trim().to_owned();
        self.local_model = self.local_model.trim().to_owned();
        self.bundled_model_variant = self.bundled_model_variant.trim().to_owned();
        if self.bundled_model_variant.is_empty() {
            self.bundled_model_variant = "Nemotron-3.5-Bolt-E4B-Q4_K_M".to_owned();
        }
        if self.active_preset_id.trim().is_empty() {
            self.active_preset_id = "balanced".to_owned();
        }
    }
}

fn normalize_url(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        trimmed.to_owned()
    }
}

/// API keys loaded from the OS keyring or environment.
///
/// The values are not serializable and never written to SQLite.
#[derive(Clone, Default)]
pub struct ProviderKeys {
    /// Gemini Developer API key.
    pub gemini: Option<String>,
    /// Anthropic API key.
    pub anthropic: Option<String>,
    /// OpenAI API key.
    pub openai: Option<String>,
    /// Optional bearer token for a local OpenAI-compatible endpoint.
    pub local: Option<String>,
}

impl std::fmt::Debug for ProviderKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderKeys")
            .field("gemini", &self.gemini.as_ref().map(|_| "<redacted>"))
            .field("anthropic", &self.anthropic.as_ref().map(|_| "<redacted>"))
            .field("openai", &self.openai.as_ref().map(|_| "<redacted>"))
            .field("local", &self.local.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl ProviderKeys {
    /// Key for a cloud provider.
    pub fn for_provider(&self, provider: crate::Provider) -> Option<&str> {
        match provider {
            crate::Provider::Gemini => self.gemini.as_deref(),
            crate::Provider::Anthropic => self.anthropic.as_deref(),
            crate::Provider::OpenAi => self.openai.as_deref(),
            crate::Provider::OpenAiCompatible => self.local.as_deref(),
            crate::Provider::Ollama => None,
        }
    }

    /// Whether at least one provider is configured.
    pub fn any_configured(&self) -> bool {
        self.gemini.is_some() || self.anthropic.is_some() || self.openai.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_defaults_are_valid() {
        let mut settings = AppSettings::default();
        settings.normalize();
        assert_eq!(settings.default_model, DEFAULT_MODEL_SLUG);
        assert_eq!(settings.active_preset_id, "balanced");
        assert!(settings.send_on_enter);
        assert_eq!(settings.ollama_base_url, "http://localhost:11434");
    }

    #[test]
    fn normalize_trims_urls_and_clamps_temperature() {
        let mut settings = AppSettings {
            temperature: 5.0,
            ollama_base_url: " http://localhost:11434/ ".to_owned(),
            local_base_url: "http://localhost:8080/v1/".to_owned(),
            default_model: "not-real".to_owned(),
            ..Default::default()
        };
        settings.normalize();
        assert_eq!(settings.temperature, 2.0);
        assert_eq!(settings.ollama_base_url, "http://localhost:11434");
        assert_eq!(settings.local_base_url, "http://localhost:8080/v1");
        assert_eq!(settings.default_model, DEFAULT_MODEL_SLUG);
    }

    #[test]
    fn effort_maps_to_provider_vocabularies() {
        assert_eq!(ReasoningEffort::Max.gemini_level(), Some("xhigh"));
        assert_eq!(ReasoningEffort::Max.anthropic_effort(), Some("max"));
        assert_eq!(ReasoningEffort::Max.openai_effort(), Some("high"));
        assert_eq!(ReasoningEffort::Off.anthropic_effort(), None);
        assert_eq!(ReasoningEffort::Off.ollama_level(), None);
    }
}
