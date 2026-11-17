//! Anthropic Claude Messages API with adaptive thinking.

use reqwest::Client;
use serde_json::{Map, Value, json};
use smol::channel::Sender;
use tokio_util::sync::CancellationToken;

use relay_core::{Provider, Usage};

use crate::http::{
    base_url, join_url, json_headers, next_chunk, number_u32, secret_header, send_json,
    value_message,
};
use crate::request::ChatRequest;
use crate::sse::SseDecoder;
use crate::{ANTHROPIC_BASE_URL, ProviderError, StreamEvent, emit};

/// Anthropic requires clients to pin the API version.
const API_VERSION: &str = "2026-10-21";

/// Build a Messages request body.
pub(crate) fn body(request: &ChatRequest) -> Value {
    let mut map = Map::new();
    map.insert("model".to_owned(), json!(request.model.api_id));
    map.insert("max_tokens".to_owned(), json!(request.output_limit()));
    map.insert("messages".to_owned(), Value::Array(messages(request)));
    map.insert("stream".to_owned(), json!(true));
    if let Some(system) = request.system_prompt() {
        map.insert("system".to_owned(), json!(system));
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();
        map.insert("tools".to_owned(), Value::Array(tools));
    }

    match request.reasoning {
        relay_core::ReasoningEffort::Off => {
            // Sampling controls are only safe without adaptive thinking.
            map.insert(
                "temperature".to_owned(),
                json!(request.temperature.clamp(0.0, 1.0)),
            );
        }
        effort => {
            map.insert(
                "thinking".to_owned(),
                json!({ "type": "adaptive", "display": "summarized" }),
            );
            if let Some(level) = effort.anthropic_effort() {
                map.insert("output_config".to_owned(), json!({ "effort": level }));
            }
        }
    }

    Value::Object(map)
}

fn messages(request: &ChatRequest) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for turn in &request.messages {
        if turn.role == relay_core::Role::System {
            continue;
        }
        match turn.role {
            relay_core::Role::Assistant => {
                let mut content: Vec<Value> = Vec::new();
                if !turn.text.trim().is_empty() {
                    content.push(json!({ "type": "text", "text": turn.text }));
                }
                for call in &turn.tool_calls {
                    content.push(json!({
                        "type": "tool_use",
                        "id": call.id,
                        "name": call.name,
                        "input": call.arguments,
                    }));
                }
                if content.is_empty() {
                    continue;
                }
                out.push(json!({ "role": "assistant", "content": content }));
            }
            relay_core::Role::Tool => {
                let id = turn.tool_call_id.clone().unwrap_or_default();
                out.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": id,
                        "content": turn.text,
                    }]
                }));
            }
            _ => {
                if turn.text.trim().is_empty() && turn.images.is_empty() {
                    continue;
                }
                // Merge consecutive user text into one turn content array.
                if let Some(last) = out.last_mut()
                    && last.get("role").and_then(Value::as_str) == Some("user")
                    && last.get("content").map(Value::is_array).unwrap_or(false)
                    && let Some(arr) = last.get_mut("content").and_then(Value::as_array_mut)
                {
                    for image in &turn.images {
                        arr.push(json!({
                            "type": "image",
                            "source": {
                                "type": "base64",
                                "media_type": image.media_type,
                                "data": image.data,
                            }
                        }));
                    }
                    if !turn.text.trim().is_empty() {
                        arr.push(json!({ "type": "text", "text": turn.text }));
                    }
                    continue;
                }
                let mut content: Vec<Value> = Vec::new();
                for image in &turn.images {
                    content.push(json!({
                        "type": "image",
                        "source": {
                            "type": "base64",
                            "media_type": image.media_type,
                            "data": image.data,
                        }
                    }));
                }
                if !turn.text.trim().is_empty() {
                    content.push(json!({ "type": "text", "text": turn.text }));
                }
                if content.is_empty() {
                    continue;
                }
                out.push(json!({
                    "role": "user",
                    "content": content
                }));
            }
        }
    }
    // The Messages API requires the first turn to be from the user.
    while out
        .first()
        .is_some_and(|m| m.get("role").and_then(Value::as_str) == Some("assistant"))
    {
        out.remove(0);
    }
    out
}

/// Stream a Claude response.
pub(crate) async fn stream(
    http: &Client,
    request: &mut ChatRequest,
    events: &Sender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let key = request
        .api_key
        .as_ref()
        .filter(|key| !key.is_empty())
        .ok_or(ProviderError::MissingApiKey {
            provider: Provider::Anthropic,
        })?;

    let base = base_url(request.endpoint.as_deref(), ANTHROPIC_BASE_URL);
    let url = join_url(base, "/v1/messages")?;
    let mut headers = json_headers();
    secret_header(&mut headers, "x-api-key", key.expose(), Provider::Anthropic)?;
    headers.insert(
        "anthropic-version",
        reqwest::header::HeaderValue::from_static(API_VERSION),
    );

    let response = send_json(
        http,
        Provider::Anthropic,
        &url,
        headers,
        &body(request),
        Some(key.expose()),
        cancel,
    )
    .await?;

    let mut stream = Box::pin(response.bytes_stream());
    let mut decoder = SseDecoder::new();
    let mut usage = Usage::default();
    let mut stop_reason = None;
    let mut finished = false;
    let mut tool_id: Option<String> = None;
    let mut tool_name: Option<String> = None;
    let mut tool_args = String::new();

    while let Some(chunk) = next_chunk(&mut stream, Provider::Anthropic, cancel).await? {
        for event in decoder.push(&chunk) {
            let data = event.data.trim();
            if data.is_empty() {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .or(event.event.as_deref())
                .unwrap_or_default();

            match kind {
                "message_start" => {
                    if let Some(usage_value) = value.pointer("/message/usage") {
                        usage.merge_max(Usage::new(
                            field_u32(usage_value, "input_tokens"),
                            0,
                            field_u32(usage_value, "cache_read_input_tokens"),
                            0,
                        ));
                    }
                }
                "content_block_start" => {
                    if let Some(block) = value.get("content_block") {
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                if let Some(text) = block.get("text").and_then(Value::as_str)
                                    && !text.is_empty()
                                {
                                    emit(events, StreamEvent::Text(text.to_owned())).await?;
                                }
                            }
                            Some("thinking") => {
                                if let Some(text) = block.get("thinking").and_then(Value::as_str)
                                    && !text.is_empty()
                                {
                                    emit(events, StreamEvent::Thinking(text.to_owned())).await?;
                                }
                            }
                            Some("tool_use") => {
                                tool_id =
                                    block.get("id").and_then(Value::as_str).map(str::to_owned);
                                tool_name =
                                    block.get("name").and_then(Value::as_str).map(str::to_owned);
                                tool_args.clear();
                                if let Some(input) = block.get("input")
                                    && !input.is_null()
                                {
                                    tool_args = input.to_string();
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "content_block_delta" => {
                    let Some(delta) = value.get("delta") else {
                        continue;
                    };
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            if let Some(text) = delta.get("text").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                emit(events, StreamEvent::Text(text.to_owned())).await?;
                            }
                        }
                        Some("thinking_delta") => {
                            if let Some(text) = delta.get("thinking").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                emit(events, StreamEvent::Thinking(text.to_owned())).await?;
                            }
                        }
                        Some("input_json_delta") => {
                            if let Some(partial) = delta.get("partial_json").and_then(Value::as_str)
                            {
                                tool_args.push_str(partial);
                            }
                        }
                        // Signatures and future delta kinds are ignored.
                        _ => {}
                    }
                }
                "message_delta" => {
                    if let Some(usage_value) = value.get("usage") {
                        usage.merge_max(Usage::new(
                            0,
                            field_u32(usage_value, "output_tokens"),
                            0,
                            0,
                        ));
                    }
                    if let Some(reason) =
                        value.pointer("/delta/stop_reason").and_then(Value::as_str)
                    {
                        stop_reason = Some(reason.to_owned());
                    }
                }
                "content_block_stop" => {
                    if let (Some(id), Some(name)) = (tool_id.take(), tool_name.take()) {
                        let args =
                            serde_json::from_str(&tool_args).unwrap_or(serde_json::json!({}));
                        if !name.trim().is_empty() {
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
                        tool_args.clear();
                    }
                }
                "message_stop" => {
                    finished = true;
                    break;
                }
                "error" => {
                    return Err(ProviderError::Remote {
                        provider: Provider::Anthropic,
                        message: value_message(&value),
                    });
                }
                _ => {}
            }
        }
        if finished {
            break;
        }
    }

    if !finished {
        for event in decoder.finish() {
            if event.data.trim().is_empty() {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&event.data)
                && value.get("type").and_then(Value::as_str) == Some("error")
            {
                return Err(ProviderError::Remote {
                    provider: Provider::Anthropic,
                    message: value_message(&value),
                });
            }
        }
    }

    if !usage.is_empty() {
        emit(events, StreamEvent::Usage(usage)).await?;
    }
    emit(events, StreamEvent::Done { stop_reason }).await?;
    Ok(())
}

fn field_u32(value: &Value, key: &str) -> u32 {
    value.get(key).and_then(number_u32).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::{ModelSelection, ReasoningEffort, RequestMessage, Role, find_model};

    fn request() -> ChatRequest {
        let model = ModelSelection::from_spec(find_model("claude-sonnet-5-5").expect("model"));
        ChatRequest::new(
            model,
            vec![
                RequestMessage {
                    role: Role::Assistant,
                    text: "orphan".to_owned(),
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                RequestMessage {
                    role: Role::User,
                    text: "First".to_owned(),
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                RequestMessage {
                    role: Role::User,
                    text: "Second".to_owned(),
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
                RequestMessage {
                    role: Role::Assistant,
                    text: "Answer".to_owned(),
                    images: Vec::new(),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                },
            ],
        )
        .with_system_prompt(Some("Be precise.".to_owned()))
        .with_reasoning(ReasoningEffort::High)
    }

    #[test]
    fn body_uses_adaptive_thinking_and_merges_turns() {
        let value = body(&request());
        assert_eq!(value["model"], "claude-sonnet-5-5");
        assert_eq!(value["stream"], true);
        assert_eq!(value["system"], "Be precise.");
        assert_eq!(value["thinking"]["type"], "adaptive");
        assert_eq!(value["thinking"]["display"], "summarized");
        assert_eq!(value["output_config"]["effort"], "high");
        assert!(value.get("temperature").is_none());
        assert_eq!(value["messages"].as_array().expect("messages").len(), 2);
        assert_eq!(value["messages"][0]["role"], "user");
        assert_eq!(value["messages"][0]["content"][0]["text"], "First");
        assert_eq!(value["messages"][0]["content"][1]["text"], "Second");
        assert_eq!(value["messages"][1]["role"], "assistant");
    }

    #[test]
    fn off_uses_temperature_without_thinking() {
        let value = body(
            &request()
                .with_reasoning(ReasoningEffort::Off)
                .with_temperature(1.9),
        );
        assert_eq!(value["temperature"], 1.0);
        assert!(value.get("thinking").is_none());
        assert!(value.get("output_config").is_none());
    }
}
