//! Conversation summaries shared by storage, search, and the sidebar.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lightweight conversation row used by the sidebar and command palette.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversationSummary {
    /// Stable UUID.
    pub id: String,
    /// Human-readable title.
    pub title: String,
    /// Relay slug of the conversation's current model.
    pub model_slug: String,
    /// Creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last activity timestamp.
    pub updated_at: DateTime<Utc>,
    /// Number of stored messages.
    pub message_count: u32,
    /// A short preview of the latest visible text, when available.
    pub preview: Option<String>,
    /// Whether the conversation is pinned.
    #[serde(default)]
    pub pinned: bool,
    /// Whether the conversation is archived.
    #[serde(default)]
    pub archived: bool,
}

impl ConversationSummary {
    /// Pick a compact model label for the sidebar.
    pub fn model_label(&self) -> String {
        if let Some(spec) = crate::find_model(&self.model_slug) {
            return spec.name.to_owned();
        }
        if let Some(model) = crate::ModelSelection::local_from_slug(&self.model_slug) {
            return model.api_id;
        }
        self.model_slug.clone()
    }
}
