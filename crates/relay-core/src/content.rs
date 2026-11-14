//! Canonical conversation messages and typed content blocks.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::Usage;

/// Who authored a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Instructions supplied outside the conversation.
    System,
    /// A human turn.
    User,
    /// A model turn.
    Assistant,
}

impl Role {
    /// Stable database representation.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }

    /// Parse a database representation.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            _ => None,
        }
    }

    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::User => "You",
            Self::Assistant => "Assistant",
        }
    }
}

/// A provider-neutral content block.
///
/// Unknown block types are preserved as [`ContentBlock::Unknown`] so a future
/// Relay release can add block kinds without making old databases unreadable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Visible answer text.
    Text {
        /// Markdown or plain text.
        text: String,
    },
    /// A reasoning summary. Never sent back to a provider.
    Thinking {
        /// Human-readable reasoning summary, when the provider exposes one.
        text: String,
        /// Opaque provider signature, retained only for diagnostics.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Provider that produced the block.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
    },
    /// A block from a newer Relay version.
    #[serde(other)]
    Unknown,
}

impl ContentBlock {
    /// Create a text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Create a reasoning block.
    pub fn thinking(text: impl Into<String>, provider: impl Into<String>) -> Self {
        Self::Thinking {
            text: text.into(),
            signature: None,
            provider: Some(provider.into()),
        }
    }

    /// Visible text, if this is a text block.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Reasoning text, if this is a thinking block.
    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            Self::Thinking { text, .. } => Some(text),
            _ => None,
        }
    }
}

/// One canonical message in a conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Stable UUID used by the store and UI.
    pub id: String,
    /// Author role.
    pub role: Role,
    /// Ordered content blocks.
    pub blocks: Vec<ContentBlock>,
    /// Relay slug of the model that produced an assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reported usage for an assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Error text when a response failed or was interrupted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Creation time in UTC.
    pub created_at: DateTime<Utc>,
}

impl ChatMessage {
    fn new(role: Role, blocks: Vec<ContentBlock>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            role,
            blocks,
            model: None,
            usage: None,
            error: None,
            created_at: Utc::now(),
        }
    }

    /// A human text message.
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![ContentBlock::text(text)])
    }

    /// An assistant message.
    pub fn assistant(
        model: impl Into<String>,
        blocks: Vec<ContentBlock>,
        usage: Option<Usage>,
    ) -> Self {
        let mut message = Self::new(Role::Assistant, blocks);
        message.model = Some(model.into());
        message.usage = usage;
        message
    }

    /// Attach an error to a message.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    /// Concatenated visible text.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for block in &self.blocks {
            if let Some(text) = block.as_text() {
                if !out.is_empty() {
                    out.push_str("\n\n");
                }
                out.push_str(text);
            }
        }
        out
    }

    /// Concatenated reasoning text.
    pub fn thinking(&self) -> String {
        self.blocks
            .iter()
            .filter_map(ContentBlock::as_thinking)
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// Whether the message has no visible content. Thinking-only turns are not
    /// useful to replay, so request assembly skips them.
    pub fn is_empty(&self) -> bool {
        self.text().trim().is_empty()
    }

    /// Append streamed text to the first text block, creating one if needed.
    pub fn append_text(&mut self, chunk: &str) {
        match self
            .blocks
            .iter_mut()
            .find(|b| matches!(b, ContentBlock::Text { .. }))
        {
            Some(ContentBlock::Text { text }) => text.push_str(chunk),
            _ => self.blocks.push(ContentBlock::text(chunk)),
        }
    }

    /// Append streamed reasoning to the first thinking block, creating one if needed.
    pub fn append_thinking(&mut self, chunk: &str, provider: &str) {
        match self
            .blocks
            .iter_mut()
            .find(|b| matches!(b, ContentBlock::Thinking { .. }))
        {
            Some(ContentBlock::Thinking { text, .. }) => text.push_str(chunk),
            _ => {
                self.blocks.push(ContentBlock::thinking(chunk, provider));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_thinking_helpers_extract_only_their_blocks() {
        let mut message = ChatMessage::user("Hello");
        message.append_thinking("first ", "claude");
        message.append_thinking("second", "claude");
        message.append_text(" world");
        assert_eq!(message.text(), "Hello world");
        assert_eq!(message.thinking(), "first second");
        assert!(!message.is_empty());
    }

    #[test]
    fn empty_assistant_message_is_detected() {
        let message = ChatMessage::assistant("claude-sonnet-5-5", Vec::new(), None);
        assert!(message.is_empty());
    }

    #[test]
    fn unknown_blocks_deserialize_without_losing_the_message() {
        let json = r#"{
            "id":"m1","role":"assistant","blocks":[{"type":"future_block","data":1}],
            "created_at":"2026-11-14T00:00:00Z"
        }"#;
        let message: ChatMessage = serde_json::from_str(json).expect("valid message");
        assert_eq!(message.blocks, vec![ContentBlock::Unknown]);
        assert!(message.text().is_empty());
    }

    #[test]
    fn content_round_trips_through_json() {
        let mut message = ChatMessage::user("hi");
        message.append_thinking("thinking", "gemini");
        let json = serde_json::to_string(&message).expect("serialize");
        let parsed: ChatMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed, message);
    }
}
