//! Shared request assembly for the desktop app and CLI.
//!
//! Both front ends call [`prepare_request`] so system prompts, tool sets,
//! history fitting, endpoint selection, and credentials cannot drift apart.

use relay_core::{
    AppSettings, ChatMessage, ModelSelection, PersonalityPreset, Provider, ReasoningEffort,
    ReasoningKind, ToolSpec, chat_tool_specs, compose_system_prompt, find_builtin_preset,
    find_model, fit_history, request_history, tool_specs,
};
use relay_providers::ChatRequest;

use crate::{keys, mcp};

/// Resolve a stored Relay slug to full model metadata.
///
/// Unknown slugs fall back exactly like the desktop picker: the default cloud
/// model, then a safe local fallback.
pub fn resolve_model(slug: &str) -> ModelSelection {
    if let Some(spec) = find_model(slug) {
        return ModelSelection::from_spec(spec);
    }
    if let Some(local) = ModelSelection::local_from_slug(slug) {
        return local;
    }
    find_model(relay_core::DEFAULT_MODEL_SLUG)
        .map(ModelSelection::from_spec)
        .unwrap_or_else(|| ModelSelection::ollama("llama3.2"))
}

/// Whether a slug names a curated cloud model. Local slugs are accepted by
/// their `ollama/` or `local/` prefix.
pub fn is_known_model(slug: &str) -> bool {
    find_model(slug).is_some() || ModelSelection::local_from_slug(slug).is_some()
}

/// Reasoning effort actually sent for this model.
pub fn effective_reasoning(settings: &AppSettings, model: &ModelSelection) -> ReasoningEffort {
    if model.reasoning == ReasoningKind::None {
        ReasoningEffort::Off
    } else {
        settings.reasoning
    }
}

/// Tools exposed for the currently selected interaction mode.
///
/// Chat mode is deliberately limited to web access. Work mode adds files,
/// commands, delegation, and any configured MCP tools.
pub fn tools_for(settings: &AppSettings) -> Vec<ToolSpec> {
    if settings.interaction_mode == "chat" {
        chat_tool_specs()
    } else {
        let mut specs = tool_specs(settings.agent_mode);
        specs.extend(mcp::global_tools());
        specs
    }
}

/// Local endpoint used by a selected Ollama or OpenAI-compatible model.
pub fn provider_endpoint(settings: &AppSettings, model: &ModelSelection) -> Option<String> {
    if !model.provider.is_local() {
        return None;
    }
    match model.provider {
        Provider::Ollama => Some(settings.ollama_base_url.clone()),
        _ => Some(settings.local_base_url.clone()),
    }
}

/// Build the complete provider request for one turn.
///
/// `messages` is canonical history (including the just-saved user message).
/// `presets` is the store's preset list and `memories` are the cross-session
/// memories. The request is fitted to the selected model's context window.
pub fn prepare_request(
    settings: &AppSettings,
    model: &ModelSelection,
    messages: &[ChatMessage],
    presets: &[PersonalityPreset],
    memories: &[(String, String)],
) -> ChatRequest {
    let reasoning = effective_reasoning(settings, model);
    let tools = tools_for(settings);

    let mut request = ChatRequest::new(model.clone(), Vec::new())
        .with_reasoning(reasoning)
        .with_temperature(settings.temperature)
        .with_max_output_tokens(settings.max_output_tokens)
        .with_tools(tools);

    let preset = presets
        .iter()
        .find(|preset| preset.id == settings.active_preset_id)
        .cloned()
        .or_else(|| find_builtin_preset(&settings.active_preset_id));

    let mut custom = settings.custom_instructions.clone();
    if !memories.is_empty() {
        custom.push_str("\n\nCross-session memory:\n");
        for (_, content) in memories {
            custom.push_str("- ");
            custom.push_str(content);
            custom.push('\n');
        }
    }

    let mut system = compose_system_prompt(preset.as_ref(), &custom)
        .unwrap_or_else(|| relay_core::BASE_SYSTEM_PROMPT.to_owned());
    if model.is_local() {
        system.push_str("\n\nLocal model tool-use rules: Use the provided function/tool calls exactly as specified—do not invent tool call syntax in plain text. Before claiming a file was created/changed, ensure a write tool call succeeded; before claiming contents, ensure read/list tool results show them. For Desktop on localized systems (e.g. Hungarian 'Asztal'), locate it via $HOME (e.g. run_command: echo $HOME, then list_dir) rather than guessing paths. If unsure, inspect first, then act, then verify with a tool.");
    }
    if settings.interaction_mode == "chat" {
        system.push_str("\n\nYou have access to web_search and fetch_url tools. Use web_search to find current information on the internet; use fetch_url to read a specific page. Never say you cannot access the internet or live data—use these tools instead.");
    } else {
        system.push_str("\n\nYou have access to tools including web_search, fetch_url, file, and terminal operations (when enabled). Use them when needed to obtain current information or perform actions. Never say you cannot access the internet or live data—use web_search instead.");
    }

    let system_opt = if system.trim().is_empty() {
        None
    } else {
        Some(system)
    };
    request = request.with_system_prompt(system_opt.clone());
    request.messages = fit_history(
        request_history(messages),
        model.context_window,
        request.output_limit(),
        system_opt.as_deref(),
    );

    if let Some(endpoint) = provider_endpoint(settings, model) {
        request = request.with_endpoint(endpoint);
    }
    if let Some(key) = keys::resolve(model.provider) {
        request = request.with_api_key(key);
    }
    request
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::{ChatMessage, ReasoningEffort};

    fn model() -> ModelSelection {
        resolve_model(relay_core::DEFAULT_MODEL_SLUG)
    }

    #[test]
    fn chat_mode_only_exposes_web_tools() {
        let settings = AppSettings {
            interaction_mode: "chat".to_owned(),
            ..Default::default()
        };
        let names: Vec<_> = tools_for(&settings)
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, vec!["web_search", "fetch_url"]);
    }

    #[test]
    fn work_plan_mode_omits_write_tools() {
        let settings = AppSettings::default();
        let names: Vec<_> = tools_for(&settings)
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert!(names.contains(&"read_file".to_owned()));
        assert!(!names.contains(&"write_file".to_owned()));
    }

    #[test]
    fn prompt_mentions_web_and_memory() {
        let settings = AppSettings {
            interaction_mode: "chat".to_owned(),
            ..Default::default()
        };
        let request = prepare_request(
            &settings,
            &model(),
            &[ChatMessage::user("hello")],
            &[],
            &[("id".to_owned(), "likes tea".to_owned())],
        );
        let prompt = request.system_prompt().unwrap_or_default();
        assert!(prompt.contains("web_search"));
        assert!(prompt.contains("likes tea"));
        assert_eq!(request.reasoning, settings.reasoning);
        assert!(request.reasoning != ReasoningEffort::Max);
    }
}
