//! OpenAI-compatible local endpoints (llama.cpp, LM Studio, vLLM, Ollama's
//! compatibility layer, and similar servers).

use reqwest::Client;
use serde_json::{Map, Value, json};
use smol::channel::Sender;
use tokio_util::sync::CancellationToken;

use relay_core::{Provider, Usage};

use crate::http::{
    join_url, json_headers, next_chunk, pointer_u32, remote_message, secret_header, send_json,
    value_message,
};
use crate::request::{ApiKey, ChatRequest};
use crate::sse::SseDecoder;
use crate::{ProviderError, StreamEvent, emit};

/// Build a `/chat/completions` request body.
pub(crate) fn body(request: &ChatRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(system) = request.system_prompt() {
        messages.push(json!({ "role": "system", "content": system }));
    }
    for message in &request.messages {
        if message.text.trim().is_empty()
            && message.tool_calls.is_empty()
            && message.tool_call_id.is_none()
            && message.images.is_empty()
        {
            continue;
        }
        match message.role {
            relay_core::Role::Assistant => {
                if !message.text.trim().is_empty() {
                    messages.push(json!({ "role": "assistant", "content": message.text }));
                }
                for call in &message.tool_calls {
                    messages.push(json!({
                        "role": "assistant",
                        "tool_calls": [{
                            "id": call.id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments.to_string() }
                        }]
                    }));
                }
            }
            relay_core::Role::Tool => {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": message.tool_call_id,
                    "content": message.text,
                }));
            }
            _ => {
                let role = match message.role {
                    relay_core::Role::Assistant => "assistant",
                    _ => "user",
                };
                if !message.images.is_empty() {
                    let mut content: Vec<Value> = Vec::new();
                    if !message.text.trim().is_empty() {
                        content.push(json!({ "type": "text", "text": message.text }));
                    }
                    for image in &message.images {
                        content.push(json!({
                            "type": "image_url",
                            "image_url": {
                                "url": format!("data:{};base64,{}", image.media_type, image.data)
                            }
                        }));
                    }
                    if !content.is_empty() {
                        messages.push(json!({ "role": role, "content": content }));
                    }
                } else if !message.text.trim().is_empty() {
                    messages.push(json!({ "role": role, "content": message.text }));
                }
            }
        }
    }

    let mut map = Map::new();
    map.insert("model".to_owned(), json!(request.model.api_id));
    map.insert("messages".to_owned(), Value::Array(messages));
    map.insert("stream".to_owned(), json!(true));
    map.insert(
        "temperature".to_owned(),
        json!(request.temperature.clamp(0.0, 2.0)),
    );
    map.insert("max_tokens".to_owned(), json!(request.output_limit()));
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request.tools.iter().map(|t| json!({
            "type": "function",
            "function": { "name": t.name, "description": t.description, "parameters": t.parameters }
        })).collect();
        map.insert("tools".to_owned(), Value::Array(tools));
    }
    Value::Object(map)
}

/// Stream from an OpenAI-compatible endpoint.
pub(crate) async fn stream(
    http: &Client,
    request: &mut ChatRequest,
    events: &Sender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let endpoint = request.endpoint.as_deref().ok_or_else(|| {
        ProviderError::InvalidEndpoint("the local endpoint is not configured".to_owned())
    })?;
    let url = join_url(endpoint, "/chat/completions")?;
    let mut headers = json_headers();
    if let Some(key) = request.api_key.as_ref().filter(|key| !key.is_empty()) {
        secret_header(
            &mut headers,
            "authorization",
            &format!("Bearer {}", key.expose()),
            Provider::OpenAiCompatible,
        )?;
    }

    let response = send_json(
        http,
        Provider::OpenAiCompatible,
        &url,
        headers,
        &body(request),
        request.api_key.as_ref().map(ApiKey::expose),
        cancel,
    )
    .await?;

    let mut stream = Box::pin(response.bytes_stream());
    let mut decoder = SseDecoder::new();
    let mut usage = Usage::default();
    let mut stop_reason = None;
    let mut finished = false;
    // index -> (id, name, args_buffer)
    let mut tool_states: std::collections::BTreeMap<usize, (String, String, String)> =
        std::collections::BTreeMap::new();

    while let Some(chunk) = next_chunk(&mut stream, Provider::OpenAiCompatible, cancel).await? {
        for event in decoder.push(&chunk) {
            let data = event.data.trim();
            if data.is_empty() {
                continue;
            }
            if data == "[DONE]" {
                finished = true;
                break;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if value.get("error").is_some() {
                return Err(ProviderError::Remote {
                    provider: Provider::OpenAiCompatible,
                    message: value_message(&value),
                });
            }
            if let Some(usage_value) = value.get("usage").filter(|usage| !usage.is_null()) {
                usage.merge_max(read_usage(usage_value));
            }
            if let Some(choice) = value.pointer("/choices/0") {
                if let Some(delta) = choice.get("delta") {
                    if let Some(thinking) = delta
                        .get("reasoning_content")
                        .or_else(|| delta.get("reasoning"))
                        .and_then(Value::as_str)
                        && !thinking.is_empty()
                    {
                        emit(events, StreamEvent::Thinking(thinking.to_owned())).await?;
                    }
                    if let Some(content) = delta.get("content").and_then(Value::as_str)
                        && !content.is_empty()
                    {
                        emit(events, StreamEvent::Text(content.to_owned())).await?;
                    }
                    if let Some(tcs) = delta.get("tool_calls").and_then(Value::as_array) {
                        for tc in tcs {
                            let idx = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                            let entry = tool_states
                                .entry(idx)
                                .or_insert_with(|| (String::new(), String::new(), String::new()));
                            if let Some(id) = tc.get("id").and_then(Value::as_str)
                                && !id.is_empty()
                            {
                                entry.0 = id.to_owned();
                            }
                            if let Some(name) = tc.pointer("/function/name").and_then(Value::as_str)
                                && !name.is_empty()
                            {
                                entry.1.push_str(name);
                            }
                            if let Some(args) =
                                tc.pointer("/function/arguments").and_then(Value::as_str)
                            {
                                entry.2.push_str(args);
                            }
                        }
                    }
                }
                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    stop_reason = Some(reason.to_owned());
                }
            }
        }
        if finished {
            break;
        }
    }

    for (idx, (id, name, args_str)) in tool_states {
        if name.trim().is_empty() {
            continue;
        }
        let id = if id.is_empty() {
            format!("call_{idx}")
        } else {
            id
        };
        let args = serde_json::from_str(&args_str).unwrap_or(Value::Object(Map::new()));
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

    if !usage.is_empty() {
        emit(events, StreamEvent::Usage(usage)).await?;
    }
    emit(events, StreamEvent::Done { stop_reason }).await?;
    Ok(())
}

fn read_usage(value: &Value) -> Usage {
    Usage::new(
        pointer_u32(value, "/prompt_tokens"),
        pointer_u32(value, "/completion_tokens"),
        pointer_u32(value, "/prompt_tokens_details/cached_tokens"),
        pointer_u32(value, "/completion_tokens_details/reasoning_tokens"),
    )
}

/// List models exposed by the endpoint.
pub(crate) async fn list_models(
    http: &Client,
    endpoint: &str,
    api_key: Option<&ApiKey>,
) -> Result<Vec<String>, ProviderError> {
    let url = join_url(endpoint, "/models")?;
    let mut request = http
        .get(&url)
        .timeout(std::time::Duration::from_secs(20))
        .header("accept", "application/json");
    if let Some(key) = api_key.filter(|key| !key.is_empty()) {
        request = request.bearer_auth(key.expose());
    }
    let response = request
        .send()
        .await
        .map_err(|error| ProviderError::Transport {
            provider: Provider::OpenAiCompatible,
            message: error.to_string(),
        })?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = response.text().await.unwrap_or_default();
        return Err(ProviderError::Status {
            provider: Provider::OpenAiCompatible,
            status,
            message: remote_message(&body),
        });
    }
    let value: Value = response
        .json()
        .await
        .map_err(|error| ProviderError::Decode {
            provider: Provider::OpenAiCompatible,
            message: error.to_string(),
        })?;
    let mut names: Vec<String> = value
        .get("data")
        .and_then(Value::as_array)
        .map(|models| {
            models
                .iter()
                .filter_map(|model| model.get("id").and_then(Value::as_str))
                .map(str::to_owned)
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
    use relay_core::{ModelSelection, RequestMessage, Role};

    fn request() -> ChatRequest {
        ChatRequest::new(
            ModelSelection::openai_compatible("qwen3-coder-30b"),
            vec![
                RequestMessage {
                    role: Role::User,
                    text: "Hello".to_owned(),
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                RequestMessage {
                    role: Role::Assistant,
                    text: "Hi".to_owned(),
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                RequestMessage {
                    role: Role::User,
                    text: "Continue".to_owned(),
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
            ],
        )
        .with_system_prompt(Some("Be terse.".to_owned()))
        .with_temperature(2.5)
        .with_endpoint("http://localhost:8000/v1/")
    }

    #[test]
    fn body_uses_chat_completions_shape() {
        let value = body(&request());
        assert_eq!(value["model"], "qwen3-coder-30b");
        assert_eq!(value["messages"][0]["role"], "system");
        assert_eq!(value["messages"][1]["content"], "Hello");
        assert_eq!(value["stream"], true);
        assert_eq!(value["temperature"], 2.0);
        assert_eq!(value["max_tokens"], 8_192);
        assert!(value.get("reasoning").is_none());
    }

    #[test]
    fn usage_maps_openai_chat_fields() {
        let value = json!({
            "prompt_tokens": 30,
            "completion_tokens": 9,
            "prompt_tokens_details": {"cached_tokens": 12},
            "completion_tokens_details": {"reasoning_tokens": 3}
        });
        assert_eq!(read_usage(&value), Usage::new(30, 9, 12, 3));
    }
}
