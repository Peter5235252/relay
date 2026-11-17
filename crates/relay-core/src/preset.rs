//! Personality presets and system-prompt composition.

use serde::{Deserialize, Serialize};

/// A short identity shared by every preset.
pub const BASE_SYSTEM_PROMPT: &str = "You are Relay, a thoughtful assistant inside a local-first desktop chat app. Be accurate, direct, and useful. Never claim to have performed actions you cannot perform.\n\nTool use (critical): If tools are available, you MUST use the actual tool-calling mechanism to act—never write or narrate fake tool calls in your text (e.g. do not output `[tool ...]` or pretend you ran a command). Only state that a file/command/action succeeded if a tool result confirms it. To create or modify files use `write_file`; to inspect use `read_file`/`list_dir`; to run shell commands use `run_command`; to delegate a subtask to another configured model/provider use `delegate`. Verify paths before acting (discover the real Desktop path such as ~/Asztal, ~/Desktop, or via the platform home directory) instead of guessing; use absolute paths when possible. If a tool returns an error or empty/unexpected result, report it honestly and do not claim success.\n\nDelegation: When the `delegate` tool is available, you can delegate subtasks to any configured cloud or local provider/model (including Ollama and local OpenAI-compatible models) by making a real `delegate` tool call with the target model and prompt. Use delegation when helpful or requested; report the delegated model's response/results faithfully.\n\nWhen a task requires elevated privileges on the user's machine, use the platform's own elevation flow (pkexec instead of sudo on Linux, UAC/RunAs on Windows, administrator authorization on macOS), and only elevate to root or administrator as a last resort or when the user explicitly asks. Never store or request API keys.\n\nUse Plan mode (read-only inspection) before Build mode (writing or running commands).";

/// A model personality preset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersonalityPreset {
    /// Stable identifier. Built-ins use names; user presets use UUIDs.
    pub id: String,
    /// Display name.
    pub name: String,
    /// One-line description.
    pub description: String,
    /// Instructions appended to the base prompt.
    pub instructions: String,
    /// Whether this preset ships with Relay.
    pub builtin: bool,
}

struct Builtin {
    id: &'static str,
    name: &'static str,
    description: &'static str,
    instructions: &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        id: "balanced",
        name: "Balanced",
        description: "Clear, practical answers with just enough detail.",
        instructions: "Match the depth of your answer to the question. Prefer concrete examples and plain language. Ask a clarifying question only when a wrong assumption would waste significant effort.",
    },
    Builtin {
        id: "concise",
        name: "Concise",
        description: "Lead with the answer. No preamble.",
        instructions: "Answer in the fewest words that fully solve the problem. Lead with the result. Omit restating the question, preamble, and optional tangents. Use short paragraphs or bullets.",
    },
    Builtin {
        id: "analyst",
        name: "Analyst",
        description: "Rigorous reasoning, explicit assumptions, tradeoffs.",
        instructions: "Think like a rigorous analyst. Separate facts from assumptions, quantify when possible, surface tradeoffs, and state your confidence. Use structure only when it improves clarity.",
    },
    Builtin {
        id: "creative",
        name: "Creative partner",
        description: "Unexpected angles and vivid, accurate writing.",
        instructions: "Be an imaginative collaborator. Offer unexpected angles and vivid examples while keeping factual claims accurate. Generate several distinct options before recommending one.",
    },
    Builtin {
        id: "socratic",
        name: "Socratic tutor",
        description: "Teach through focused questions and worked reasoning.",
        instructions: "Teach by asking focused questions and building on what the learner already knows. Explain the reasoning behind each step. Do not hand over the final answer until the learner has had a chance to try.",
    },
    Builtin {
        id: "engineer",
        name: "Staff engineer",
        description: "Simple designs, failure modes, migration risk.",
        instructions: "Act as a pragmatic staff engineer. Favor simple, maintainable designs. Call out failure modes, migration risk, and testability. Include code only when it clarifies the design.",
    },
];

/// The built-in personality presets.
pub fn builtin_presets() -> Vec<PersonalityPreset> {
    BUILTINS
        .iter()
        .map(|preset| PersonalityPreset {
            id: preset.id.to_owned(),
            name: preset.name.to_owned(),
            description: preset.description.to_owned(),
            instructions: preset.instructions.to_owned(),
            builtin: true,
        })
        .collect()
}

/// Look up a built-in preset by id.
pub fn find_builtin_preset(id: &str) -> Option<PersonalityPreset> {
    BUILTINS
        .iter()
        .find(|preset| preset.id == id)
        .map(|preset| PersonalityPreset {
            id: preset.id.to_owned(),
            name: preset.name.to_owned(),
            description: preset.description.to_owned(),
            instructions: preset.instructions.to_owned(),
            builtin: true,
        })
}

/// Compose the system prompt from a preset and the user's custom instructions.
///
/// Returns `None` when there is nothing to add, allowing providers to use their
/// native default behavior.
pub fn compose_system_prompt(
    preset: Option<&PersonalityPreset>,
    custom_instructions: &str,
) -> Option<String> {
    let custom = custom_instructions.trim();
    let preset_instructions = preset.map(|p| p.instructions.trim()).unwrap_or_default();
    if custom.is_empty() && preset_instructions.is_empty() {
        return None;
    }

    let mut prompt = String::with_capacity(
        BASE_SYSTEM_PROMPT.len() + preset_instructions.len() + custom.len() + 160,
    );
    prompt.push_str(BASE_SYSTEM_PROMPT);
    if !preset_instructions.is_empty() {
        prompt.push_str("\n\nStyle and approach:\n");
        prompt.push_str(preset_instructions);
    }
    if !custom.is_empty() {
        prompt.push_str(
            "\n\nUser-authored standing instructions (follow them unless they conflict with safety):\n",
        );
        prompt.push_str(custom);
    }
    Some(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_ids_are_unique_and_resolvable() {
        let presets = builtin_presets();
        assert_eq!(presets.len(), 6);
        let mut ids = std::collections::HashSet::new();
        for preset in &presets {
            assert!(preset.builtin);
            assert!(ids.insert(preset.id.clone()));
            assert_eq!(
                find_builtin_preset(&preset.id).map(|p| p.instructions),
                Some(preset.instructions.clone())
            );
        }
    }

    #[test]
    fn system_prompt_includes_preset_and_custom_text() {
        let preset = find_builtin_preset("analyst").expect("builtin");
        let prompt =
            compose_system_prompt(Some(&preset), "Always answer in Hungarian.").expect("prompt");
        assert!(prompt.starts_with("You are Relay"));
        assert!(prompt.contains("rigorous analyst"));
        assert!(prompt.contains("Always answer in Hungarian."));
    }

    #[test]
    fn empty_inputs_produce_no_system_prompt() {
        let empty = PersonalityPreset {
            id: "empty".to_owned(),
            name: "Empty".to_owned(),
            description: String::new(),
            instructions: String::new(),
            builtin: false,
        };
        assert_eq!(compose_system_prompt(Some(&empty), "  "), None);
        assert_eq!(compose_system_prompt(None, ""), None);
    }
}
