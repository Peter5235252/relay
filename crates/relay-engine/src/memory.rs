//! Cross-session memory summarization shared by the desktop app and CLI.
//!
//! Relay stores explicit memories and one rolling automatic summary. The
//! summary is produced by a local model only, at most once every ten hours.

use std::time::Duration;

use relay_core::AppSettings;
use relay_store::Store;

/// Minimum time between automatic summaries.
pub const SUMMARY_INTERVAL_SECS: i64 = 10 * 60 * 60;

/// Whether an automatic summary is due.
pub fn due(settings: &AppSettings) -> bool {
    let now = chrono::Utc::now().timestamp();
    now - settings.last_memory_summary_at >= SUMMARY_INTERVAL_SECS
}

/// Build the text handed to the local summarizer.
pub fn conversation_digest(store: &Store) -> Result<String, String> {
    let conversations = store
        .list_conversations()
        .map_err(|error| error.to_string())?;
    let mut text = String::new();
    for conversation in conversations.iter().take(8) {
        let messages = match store.messages(&conversation.id) {
            Ok(messages) => messages,
            Err(_) => continue,
        };
        text.push_str(&format!("Conversation: {}\n", conversation.title));
        for message in messages.iter().take(30) {
            let content = message.text();
            if content.trim().is_empty() {
                continue;
            }
            text.push_str(&format!(
                "{}: {}\n",
                message.role.as_str(),
                content.chars().take(800).collect::<String>()
            ));
        }
    }
    Ok(text)
}

/// Choose a local endpoint for summarization.
///
/// Preference order: an already-running bundled Nemotron, then a configured
/// OpenAI-compatible endpoint, then Ollama. Cloud providers are never used.
pub fn configured_endpoint(
    settings: &AppSettings,
    bundled_endpoint: Option<&str>,
) -> Option<(String, String)> {
    if let Some(endpoint) = bundled_endpoint.filter(|endpoint| !endpoint.trim().is_empty()) {
        return Some((endpoint.to_owned(), "local-model".to_owned()));
    }
    let local_model = settings.local_model.trim();
    if !local_model.is_empty()
        && crate::bundled::BundledVariant::from_id(local_model).is_none()
        && crate::bundled::BundledVariant::from_model_slug(&format!("local/{local_model}"))
            .is_none()
    {
        return Some((settings.local_base_url.clone(), local_model.to_owned()));
    }
    if !settings.ollama_model.trim().is_empty() {
        return Some((
            settings.ollama_base_url.clone(),
            settings.ollama_model.trim().to_owned(),
        ));
    }
    None
}

/// OpenAI-compatible chat-completions URL for a base endpoint.
pub fn chat_completions_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/v1") || base.contains("/v1/") {
        format!("{base}/chat/completions")
    } else if base.ends_with(":11434") {
        format!("{base}/v1/chat/completions")
    } else {
        format!("{base}/chat/completions")
    }
}

/// Generate a rolling summary if one is due, or when `force` is set.
///
/// Returns the new summary when one was written. Empty history still advances
/// the timestamp so Relay does not retry on every launch.
pub async fn maybe_summarize(
    store: &Store,
    endpoint: Option<&str>,
    model: &str,
    force: bool,
) -> Result<Option<String>, String> {
    let mut settings = store.load_settings().map_err(|error| error.to_string())?;
    if !force && !due(&settings) {
        return Ok(None);
    }
    let Some(endpoint) = endpoint.filter(|endpoint| !endpoint.trim().is_empty()) else {
        return Err("no local model endpoint is configured for memory summaries".to_owned());
    };

    let digest = conversation_digest(store)?;
    if digest.trim().is_empty() {
        settings.last_memory_summary_at = chrono::Utc::now().timestamp();
        store
            .save_settings(&settings)
            .map_err(|error| error.to_string())?;
        return Ok(None);
    }

    let prompt = format!(
        "Summarize the following conversation history into a concise cross-session memory \
         of user preferences, facts, and ongoing tasks. Output only the summary, no preamble.\n\n{digest}"
    );
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())?;
    let body = serde_json::json!({
        "model": if model.trim().is_empty() { "local-model" } else { model.trim() },
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 512,
    });
    let response = client
        .post(chat_completions_url(endpoint))
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("memory summary request failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "memory summary endpoint returned {}",
            response.status()
        ));
    }
    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("memory summary response was not JSON: {error}"))?;
    let summary = json
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_str())
        .map(str::trim)
        .filter(|summary| !summary.is_empty())
        .ok_or_else(|| "memory summary endpoint returned no text".to_owned())?;

    if let Ok(memories) = store.list_memories() {
        for (id, content) in memories {
            if content.starts_with("[Auto-summary]") {
                let _ = store.delete_memory(&id);
            }
        }
    }
    store
        .add_memory(&format!("[Auto-summary] {summary}"))
        .map_err(|error| error.to_string())?;
    settings.last_memory_summary_at = chrono::Utc::now().timestamp();
    store
        .save_settings(&settings)
        .map_err(|error| error.to_string())?;
    Ok(Some(summary.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::chat_completions_url;

    #[test]
    fn builds_openai_compatible_urls() {
        assert_eq!(
            chat_completions_url("http://127.0.0.1:11435/v1"),
            "http://127.0.0.1:11435/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://localhost:11434"),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            chat_completions_url("http://localhost:8000/v1/"),
            "http://localhost:8000/v1/chat/completions"
        );
    }
}
