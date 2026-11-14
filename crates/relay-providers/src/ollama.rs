//! Local Ollama models via `/api/chat` NDJSON streaming.

use reqwest::Client;
use serde_json::{Map, Value, json};
use smol::channel::Sender;
use tokio_util::sync::CancellationToken;

use relay_core::{Provider, Usage};

use crate::http::{join_url, next_chunk, number_u32, remote_message};
use crate::ndjson::NdjsonDecoder;
use crate::request::ChatRequest;
use crate::{OLLAMA_BASE_URL, ProviderError, StreamEvent, emit};

/// Build an `/api/chat` request body.
pub(crate) fn body(request: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = request.system_prompt() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in &request.messages {
        if message.text.trim().is_empty()
            && message.tool_calls.is_empty()
            && message.tool_call_id.is_none()
        {
            continue;
        }
        let role = match message.role {
            relay_core::Role::Assistant => "assistant",
            relay_core::Role::Tool => "tool",
            relay_core::Role::System => "system",
            relay_core::Role::User => "user",
        };
        let mut msg = json!({ "role": role, "content": message.text });
        if !message.tool_calls.is_empty() {
            let tcs: Vec<Value> = message
                .tool_calls
                .iter()
                .map(|c| {
                    json!({
                        "function": { "name": c.name, "arguments": c.arguments }
                    })
                })
                .collect();
            msg["tool_calls"] = Value::Array(tcs);
        }
        if let Some(id) = &message.tool_call_id {
            msg["tool_call_id"] = json!(id);
        }
        messages.push(msg);
    }

    let mut options = Map::new();
    options.insert(
        "temperature".to_owned(),
        json!(request.temperature.clamp(0.0, 2.0)),
    );
    options.insert("num_predict".to_owned(), json!(request.output_limit()));
    options.insert(
        "num_ctx".to_owned(),
        json!(request.model.context_window.max(2_048)),
    );

    let mut map = Map::new();
    map.insert("model".to_owned(), json!(request.model.api_id));
    map.insert("messages".to_owned(), Value::Array(messages));
    map.insert("stream".to_owned(), json!(true));
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request.tools.iter().map(|t| json!({
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
        })).collect();
        map.insert("tools".to_owned(), Value::Array(tools));
    }
    map.insert("options".to_owned(), Value::Object(options));
    map.insert("keep_alive".to_owned(), json!("10m"));
    if let Some(level) = request.reasoning.ollama_level() {
        map.insert("think".to_owned(), json!(level));
    }
    Value::Object(map)
}

/// Stream an Ollama response.
pub(crate) async fn stream(
    http: &Client,
    request: &ChatRequest,
    events: &Sender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let base = request.endpoint.as_deref().unwrap_or(OLLAMA_BASE_URL);
    let url = join_url(base, "/api/chat")?;
    let body = body(request);

    let send = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        http.post(&url).json(&body).send(),
    );
    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
        result = send => match result {
            Err(_) => return Err(ProviderError::Timeout { provider: Provider::Ollama }),
            Ok(Err(error)) => return Err(ProviderError::Transport {
                provider: Provider::Ollama,
                message: error.to_string(),
            }),
            Ok(Ok(response)) => response,
        }
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Status {
            provider: Provider::Ollama,
            status,
            message: remote_message(&body),
        });
    }

    let mut stream = Box::pin(response.bytes_stream());
    let mut decoder = NdjsonDecoder::new();
    let mut usage = Usage::default();
    let mut stop_reason = None;
    let mut finished = false;

    while let Some(chunk) = next_chunk(&mut stream, Provider::Ollama, cancel).await? {
        for line in decoder.push(&chunk) {
            let value = parse_line(&line)?;
            if apply_line(&value, events, &mut usage, &mut stop_reason).await? {
                finished = true;
                break;
            }
        }
        if finished {
            break;
        }
    }

    if !finished && let Some(line) = decoder.finish() {
        let value = parse_line(&line)?;
        apply_line(&value, events, &mut usage, &mut stop_reason).await?;
    }

    if !usage.is_empty() {
        emit(events, StreamEvent::Usage(usage)).await?;
    }
    emit(events, StreamEvent::Done { stop_reason }).await?;
    Ok(())
}

fn parse_line(line: &str) -> Result<Value, ProviderError> {
    serde_json::from_str(line).map_err(|error| ProviderError::Decode {
        provider: Provider::Ollama,
        message: error.to_string(),
    })
}

async fn apply_line(
    value: &Value,
    events: &Sender<StreamEvent>,
    usage: &mut Usage,
    stop_reason: &mut Option<String>,
) -> Result<bool, ProviderError> {
    if let Some(message) = value.get("error") {
        return Err(ProviderError::Remote {
            provider: Provider::Ollama,
            message: message
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| remote_message(&value.to_string())),
        });
    }

    if let Some(message) = value.get("message") {
        if let Some(thinking) = message.get("thinking").and_then(Value::as_str)
            && !thinking.is_empty()
        {
            emit(events, StreamEvent::Thinking(thinking.to_owned())).await?;
        }
        if let Some(content) = message.get("content").and_then(Value::as_str)
            && !content.is_empty()
        {
            emit(events, StreamEvent::Text(content.to_owned())).await?;
        }
        if let Some(tcs) = message.get("tool_calls").and_then(Value::as_array) {
            for (i, tc) in tcs.iter().enumerate() {
                let name = tc
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                if name.trim().is_empty() {
                    continue;
                }
                let args = tc
                    .pointer("/function/arguments")
                    .cloned()
                    .unwrap_or(Value::Object(Map::new()));
                let id = tc
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("call_{}", i));
                emit(
                    events,
                    StreamEvent::ToolCall(relay_core::ToolCall {
                        id,
                        name,
                        arguments: args,
                    }),
                )
                .await?;
            }
        }
    }

    if value.get("done").and_then(Value::as_bool).unwrap_or(false) {
        *usage = Usage::new(
            field_u32(value, "prompt_eval_count"),
            field_u32(value, "eval_count"),
            0,
            0,
        );
        *stop_reason = value
            .get("done_reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        return Ok(true);
    }
    Ok(false)
}

fn field_u32(value: &Value, key: &str) -> u32 {
    value.get(key).and_then(number_u32).unwrap_or(0)
}

/// List installed Ollama models.
pub(crate) async fn list_models(
    http: &Client,
    endpoint: &str,
) -> Result<Vec<String>, ProviderError> {
    let url = join_url(endpoint, "/api/tags")?;
    let response = http
        .get(&url)
        .timeout(std::time::Duration::from_secs(20))
        .send()
        .await
        .map_err(|error| ProviderError::Transport {
            provider: Provider::Ollama,
            message: error.to_string(),
        })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Status {
            provider: Provider::Ollama,
            status,
            message: remote_message(&body),
        });
    }
    let value: Value = response
        .json()
        .await
        .map_err(|error| ProviderError::Decode {
            provider: Provider::Ollama,
            message: error.to_string(),
        })?;
    let mut names: Vec<String> = value
        .get("models")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| {
                    model
                        .get("name")
                        .or_else(|| model.get("model"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names.dedup();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::{ModelSelection, ReasoningEffort, RequestMessage, Role};

    fn request() -> ChatRequest {
        ChatRequest::new(
            ModelSelection::ollama("qwen3:8b"),
            vec![
                RequestMessage {
                    role: Role::User,
                    text: "Hello".to_owned(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                RequestMessage {
                    role: Role::Assistant,
                    text: "Hi".to_owned(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                RequestMessage {
                    role: Role::User,
                    text: "Continue".to_owned(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
            ],
        )
        .with_system_prompt(Some("Be terse.".to_owned()))
        .with_reasoning(ReasoningEffort::Medium)
        .with_endpoint("http://localhost:11434/")
    }

    #[test]
    fn body_includes_system_options_and_thinking() {
        let value = body(&request());
        assert_eq!(value["model"], "qwen3:8b");
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][1]["content"], "Hello");
        assert_eq!(value["stream"], true);
        assert_eq!(value["think"], "medium");
        assert_eq!(value["options"]["num_ctx"], 8_192);
        assert_eq!(value["options"]["num_predict"], 4_096);
    }

    #[test]
    fn off_omits_think() {
        let value = body(&request().with_reasoning(ReasoningEffort::Off));
        assert!(value.get("think").is_none());
    }

    #[test]
    fn line_parser_rejects_non_json() {
        assert!(parse_line("not json").is_err());
        assert!(parse_line("{\"done\":true}").is_ok());
    }
}
