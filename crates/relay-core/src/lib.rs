#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

//! Core domain types for Relay.
//!
//! This crate deliberately has no I/O. It owns the canonical conversation
//! format, the model catalog, prompt assembly, and usage accounting so every
//! provider and the UI speak the same language.
//!
//! # Canonical history
//!
//! Conversations are stored as [`ChatMessage`] values with typed
//! [`ContentBlock`]s. Provider-specific fields (for example Anthropic thinking
//! signatures) never become required to render or replay a conversation.
//! Reasoning blocks are display artifacts: Relay strips them when assembling a
//! request, so switching models mid-conversation is always safe.
//!
//! # Costs
//!
//! Prices are public list prices in USD per million tokens, captured on
//! 2026-11-14. [`Pricing::estimate`] is an estimate, not a bill; cached-token
//! accounting and long-context tiers are best effort.

#![forbid(unsafe_code)]

mod content;
mod conversation;
mod error;
mod model;
mod preset;
mod prompt;
mod settings;
mod tool;
mod usage;

pub use content::{ChatMessage, ContentBlock, Role};
pub use conversation::ConversationSummary;
pub use error::{CoreError, Result};
pub use model::{
    DEFAULT_MODEL_SLUG, ModelSelection, ModelSpec, Provider, ReasoningKind, find_model, models,
};
pub use preset::{
    BASE_SYSTEM_PROMPT, PersonalityPreset, builtin_presets, compose_system_prompt,
    find_builtin_preset,
};
pub use prompt::{
    RequestMessage, conversation_markdown, estimate_tokens, fit_history, request_history,
    title_from_prompt,
};
pub use settings::{AppSettings, Appearance, ColorPreset, ProviderKeys, ReasoningEffort};
pub use tool::{
    AgentMode, McpServerConfig, Tool, ToolApproval, ToolCall, ToolResult, ToolSpec,
    chat_tool_specs, is_command_nondestructive, should_auto_approve, tool_specs,
};
pub use usage::{CostEstimate, LongContextPricing, Pricing, TokenRates, Usage, format_tokens};

/// Crate version, useful for the About panel and diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The public project URL shown in the app.
pub const PROJECT_URL: &str = "https://github.com/Peter5235252/relay";
