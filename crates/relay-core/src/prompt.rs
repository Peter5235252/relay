//! Prompt assembly, history fitting, and Markdown export.

use crate::{ChatMessage, ImageAttachment, Role};

/// A plain text turn ready for a provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestMessage {
    /// Author role. System instructions are carried separately.
    pub role: Role,
    /// Visible text. Reasoning blocks are never included.
    pub text: String,
    /// Attached images. Only user turns carry these.
    pub images: Vec<ImageAttachment>,
    /// Tool calls emitted by an assistant turn.
    pub tool_calls: Vec<crate::ToolCall>,
    /// Tool-call id this message is responding to.
    pub tool_call_id: Option<String>,
}

/// Convert canonical messages to request turns.
///
/// Thinking blocks are display-only. Stripping them unconditionally is what
/// makes mid-conversation model switches portable: no provider receives another
/// provider's opaque signatures.
pub fn request_history(messages: &[ChatMessage]) -> Vec<RequestMessage> {
    let mut turns = Vec::with_capacity(messages.len());
    for message in messages {
        if message.role == Role::System {
            continue;
        }
        let text = message.text();
        let images = message.images();
        if text.trim().is_empty() && images.is_empty() {
            continue;
        }
        turns.push(RequestMessage {
            role: message.role,
            text,
            images,
            tool_calls: Vec::new(),
            tool_call_id: None,
        });
    }
    turns
}

/// Rough token estimate used for context fitting.
///
/// Four characters per token is intentionally conservative for English and
/// adequate for CJK. The exact count is reported back by the provider.
pub fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let chars = text.chars().count();
    chars.div_ceil(4) as u32 + 1
}

/// Trim oldest turns until the request fits the model's context window.
///
/// Keeps the newest turn even when it is too large by truncating it at a
/// character boundary. Returns an empty vector if there is no usable budget.
pub fn fit_history(
    mut history: Vec<RequestMessage>,
    context_window: u32,
    max_output_tokens: u32,
    system_prompt: Option<&str>,
) -> Vec<RequestMessage> {
    let reserved_output = max_output_tokens.min(context_window / 2);
    let system_tokens = system_prompt.map(estimate_tokens).unwrap_or(0);
    let mut budget = context_window
        .saturating_sub(reserved_output)
        .saturating_sub(system_tokens)
        .saturating_sub(512);
    if budget < 64 {
        budget = 64;
    }

    let total = |turns: &[RequestMessage]| -> u32 {
        turns
            .iter()
            .map(|turn| estimate_tokens(&turn.text) + turn.images.len() as u32 * 1024 + 4)
            .sum()
    };

    // Drop oldest turns in one pass instead of repeatedly removing the
    // front of the vector (which is O(n) per removal).
    let mut total_tokens = total(&history);
    let mut drop = 0usize;
    while drop + 1 < history.len() && total_tokens > budget {
        let turn = &history[drop];
        total_tokens = total_tokens
            .saturating_sub(estimate_tokens(&turn.text) + turn.images.len() as u32 * 1024 + 4);
        drop += 1;
    }
    if drop > 0 {
        history.drain(0..drop);
    }

    // Some providers require the first turn to be from the user.
    while history.first().is_some_and(|turn| turn.role != Role::User) {
        history.remove(0);
    }

    if let Some(last) = history.last_mut() {
        let tokens = estimate_tokens(&last.text);
        if tokens > budget {
            let max_chars = (budget as usize).saturating_mul(4).saturating_sub(4);
            if last.text.chars().count() > max_chars {
                let truncated: String = last.text.chars().take(max_chars).collect();
                last.text = format!(
                    "{truncated}\n\n[Earlier text truncated to fit the model context window.]"
                );
            }
        }
    }

    history
}

/// Turn the first user prompt into a concise conversation title.
pub fn title_from_prompt(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return "New chat".to_owned();
    }
    if normalized.chars().count() <= 56 {
        return normalized;
    }
    let mut title = String::new();
    for word in normalized.split(' ') {
        if title.chars().count() + word.chars().count() + 1 > 52 {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    if title.is_empty() {
        title = normalized.chars().take(52).collect();
    }
    format!("{}…", title.trim_end_matches(['.', ',', ';', ':']))
}

/// Export a conversation as Markdown for the clipboard or a file.
pub fn conversation_markdown(title: &str, messages: &[ChatMessage]) -> String {
    let mut out = format!("# {title}\n\n");
    for message in messages {
        match message.role {
            Role::User => out.push_str("## You\n\n"),
            Role::Assistant => {
                let model = message.model.as_deref().unwrap_or("Assistant");
                out.push_str(&format!("## Relay · {model}\n\n"));
            }
            Role::System => out.push_str("## System\n\n"),
            Role::Tool => out.push_str("## Tool\n\n"),
        }
        let thinking = message.thinking();
        if !thinking.trim().is_empty() {
            out.push_str("<details><summary>Reasoning</summary>\n\n");
            out.push_str(thinking.trim());
            out.push_str("\n\n</details>\n\n");
        }
        out.push_str(message.text().trim());
        if let Some(error) = &message.error {
            out.push_str(&format!("\n\n> **Error:** {error}"));
        }
        out.push_str("\n\n");
    }
    out.trim_end().to_owned() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_is_stripped_from_requests() {
        let mut message = ChatMessage::user("Explain the borrow checker.");
        message.append_thinking("private chain of thought", "claude");
        let history = request_history(&[message]);
        assert_eq!(history.len(), 1);
        assert!(!history[0].text.contains("private"));
        assert!(history[0].text.contains("borrow checker"));
    }

    #[test]
    fn empty_assistant_turns_are_skipped() {
        let user = ChatMessage::user("hi");
        let empty = ChatMessage::assistant("gpt-6-astra", Vec::new(), None);
        let history = request_history(&[user, empty]);
        assert_eq!(history.len(), 1);
    }

    #[test]
    fn fit_history_drops_oldest_pairs_and_starts_with_user() {
        let turns = vec![
            RequestMessage {
                role: Role::Assistant,
                text: "old assistant".to_owned(),
                images: Vec::new(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            },
            RequestMessage {
                role: Role::User,
                text: "old user".repeat(50),
                images: Vec::new(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            },
            RequestMessage {
                role: Role::Assistant,
                text: "new assistant".repeat(50),
                images: Vec::new(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            },
            RequestMessage {
                role: Role::User,
                text: "newest question".to_owned(),
                images: Vec::new(),
                tool_calls: Vec::new(),
                tool_call_id: None,
            },
        ];
        let fitted = fit_history(turns, 400, 100, None);
        assert_eq!(
            fitted.last().map(|m| m.text.as_str()),
            Some("newest question")
        );
        assert!(fitted.first().is_some_and(|m| m.role == Role::User));
    }

    #[test]
    fn oversized_single_turn_is_truncated() {
        let turns = vec![RequestMessage {
            role: Role::User,
            text: "x".repeat(100_000),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }];
        let fitted = fit_history(turns, 1_000, 100, None);
        assert_eq!(fitted.len(), 1);
        assert!(fitted[0].text.chars().count() < 10_000);
        assert!(fitted[0].text.ends_with("window.]"));
    }

    #[test]
    fn titles_are_shortened_at_word_boundaries() {
        assert_eq!(title_from_prompt("  Hello   world  "), "Hello world");
        assert_eq!(title_from_prompt(""), "New chat");
        let long = "A ".repeat(100);
        let title = title_from_prompt(&long);
        assert!(title.chars().count() <= 57);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn markdown_export_has_roles_and_reasoning() {
        let mut assistant = ChatMessage::assistant(
            "claude-sonnet-5-5",
            vec![crate::ContentBlock::text("Answer")],
            None,
        );
        assistant.append_thinking("because", "claude");
        let export = conversation_markdown("Test", &[ChatMessage::user("Question"), assistant]);
        assert!(export.contains("# Test"));
        assert!(export.contains("## You"));
        assert!(export.contains("## Relay · claude-sonnet-5-5"));
        assert!(export.contains("<summary>Reasoning</summary>"));
    }
}
