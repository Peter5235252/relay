//! Google Gemini Interactions API.
//!
//! Relay uses the stateless Interactions stream (`store: false`) and replays
//! canonical history on every request. Thought summaries are display-only and
//! never sent back.

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
use crate::{GEMINI_BASE_URL, ProviderError, StreamEvent, emit};

const API_VERSION: &str = "v1beta";

/// Build the Interactions request body. Kept pure for unit tests.
pub(crate) fn body(request: &ChatRequest) -> Value {
    // Follow-up turn after a function call: send function_result(s) with previous_interaction_id.
    if let Some(prev) = &request.previous_interaction_id {
        // collect trailing Tool messages
        let mut tool_msgs: Vec<&relay_core::RequestMessage> = Vec::new();
        for m in request.messages.iter().rev() {
            if m.role == relay_core::Role::Tool {
                tool_msgs.push(m);
            } else {
                break;
            }
        }
        tool_msgs.reverse();
        if !tool_msgs.is_empty() {
            // map call_id -> name from last assistant tool_calls
            let mut name_map: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            for m in request.messages.iter().rev() {
                if m.role == relay_core::Role::Assistant {
                    for c in &m.tool_calls {
                        name_map.insert(c.id.clone(), c.name.clone());
                    }
                    break;
                }
            }
            let input: Vec<Value> = tool_msgs
                .iter()
                .map(|m| {
                    let call_id = m.tool_call_id.clone().unwrap_or_default();
                    let name = name_map.get(&call_id).cloned().unwrap_or_default();
                    json!({
                        "type": "function_result",
                        "name": name,
                        "call_id": call_id,
                        "result": { "content": [{ "type": "text", "text": m.text }] },
                    })
                })
                .collect();
            let mut body = Map::new();
            body.insert("model".to_owned(), json!(request.model.api_id));
            body.insert("previous_interaction_id".to_owned(), json!(prev));
            body.insert("input".to_owned(), Value::Array(input));
            body.insert("stream".to_owned(), json!(true));
            if !request.tools.is_empty() {
                let mut tools: Vec<Value> = request.tools.iter().map(|t| json!({"type":"function","name":t.name,"description":t.description,"parameters":t.parameters})).collect();
                tools.push(json!({"type":"google_search"}));
                body.insert("tools".to_owned(), Value::Array(tools));
            }
            return Value::Object(body);
        }
    }

    let input: Vec<Value> = request
        .messages
        .iter()
        .filter(|message| !message.text.trim().is_empty() || !message.tool_calls.is_empty())
        .map(|message| {
            // For assistant messages with tool calls, include function_call step
            if message.role == relay_core::Role::Assistant && !message.tool_calls.is_empty() {
                // Represent as model_output with tool calls handled separately; Gemini needs function_call steps.
                // We'll emit a single model_output step for text; tool calls are sent by the model, not us.
                let step_type = "model_output";
                json!({
                    "type": step_type,
                    "content": [{ "type": "text", "text": message.text }],
                })
            } else {
                let step_type = match message.role {
                    relay_core::Role::Assistant => "model_output",
                    relay_core::Role::Tool => "tool",
                    _ => "user_input",
                };
                json!({
                    "type": step_type,
                    "content": [{ "type": "text", "text": message.text }],
                })
            }
        })
        .collect();

    let mut generation = Map::new();
    generation.insert(
        "max_output_tokens".to_owned(),
        json!(request.output_limit()),
    );
    match request.reasoning {
        relay_core::ReasoningEffort::Off => {
            generation.insert("thinking_level".to_owned(), json!("minimal"));
            generation.insert("thinking_summaries".to_owned(), json!("none"));
        }
        effort => {
            if let Some(level) = effort.gemini_level() {
                generation.insert("thinking_level".to_owned(), json!(level));
            }
            generation.insert("thinking_summaries".to_owned(), json!("auto"));
        }
    }

    let mut body = Map::new();
    body.insert("model".to_owned(), json!(request.model.api_id));
    body.insert("input".to_owned(), Value::Array(input));
    body.insert("generation_config".to_owned(), Value::Object(generation));
    body.insert("stream".to_owned(), json!(true));
    // Stateless mode keeps Relay's canonical history authoritative and avoids
    // server-side retention.
    body.insert("store".to_owned(), json!(false));
    if let Some(system) = request.system_prompt() {
        body.insert("system_instruction".to_owned(), json!(system));
    }
    if !request.tools.is_empty() {
        let mut tools: Vec<Value> = request
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        tools.push(json!({"type": "google_search"}));
        body.insert("tools".to_owned(), Value::Array(tools));
    }
    Value::Object(body)
}

/// Stream a Gemini response.
pub(crate) async fn stream(
    http: &Client,
    request: &mut ChatRequest,
    events: &Sender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<Option<String>, ProviderError> {
    let fact = request
        .api_key
        .as_ref()
        .filter(|key| !key.is_empty())
        .ok_or(ProviderError::MissingApiKey {
            provider: Provider::Gemini,
        })?;

    let base = base_url(request.endpoint.as_deref(), GEMINI_BASE_URL);
    let url = join_url(base, &format!("/{API_VERSION}/interactions?alt=sse"))?;
    let mut headers = json_headers();
    secret_header(
        &mut headers,
        "x-goog-api-key",
        fact.expose(),
        Provider::Gemini,
    )?;

    let response = send_json(
        http,
        Provider::Gemini,
        &url,
        headers,
        &body(request),
        Some(fact.expose()),
        cancel,
    )
    .await?;

    let mut stream = Box::pin(response.bytes_stream());
    let mut decoder = SseDecoder::new();
    let mut usage = Usage::default();
    let mut stop_reason = None;
    let mut finished = false;
    let mut interaction_id: Option<String> = None;
    let mut func_call_id: Option<String> = None;
    let mut func_call_name: Option<String> = None;
    let mut func_args = String::new();

    while let Some(chunk) = next_chunk(&mut stream, Provider::Gemini, cancel).await? {
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
            let kind = value
                .get("event_type")
                .and_then(Value::as_str)
                .or_else(|| value.get("type").and_then(Value::as_str))
                .unwrap_or_default();

            match kind {
                "step.delta" => {
                    if let Some(total) = value.pointer("/metadata/total_usage") {
                        usage.merge_max(read_usage(total));
                    }
                    let Some(delta) = value.get("delta") else {
                        continue;
                    };
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            if let Some(text) = delta.get("text").and_then(Value::as_str)
                                && !text.is_empty()
                            {
                                emit(events, StreamEvent::Text(text.to_owned())).await?;
                            }
                        }
                        Some("thought_summary") => {
                            if let Some(text) = delta.get("content").and_then(text_from_content)
                                && !text.is_empty()
                            {
                                emit(events, StreamEvent::Thinking(text)).await?;
                            }
                        }
                        Some("arguments_delta") => {
                            if let Some(a) = delta.get("arguments").and_then(Value::as_str) {
                                func_args.push_str(a);
                            }
                        }
                        _ => {}
                    }
                }
                "interaction.created" => {
                    if let Some(id) = value.pointer("/interaction/id").and_then(Value::as_str) {
                        interaction_id = Some(id.to_owned());
                        request.previous_interaction_id = Some(id.to_owned());
                    }
                }
                "step.start" => {
                    if let Some(step) = value.get("step") {
                        if step.get("type").and_then(Value::as_str) == Some("function_call") {
                            func_call_id =
                                step.get("id").and_then(Value::as_str).map(str::to_owned);
                            func_call_name =
                                step.get("name").and_then(Value::as_str).map(str::to_owned);
                            func_args.clear();
                            if let Some(args) = step.get("arguments") {
                                if let Some(s) = args.as_str() {
                                    func_args.push_str(s);
                                } else if !args.is_null() {
                                    func_args = args.to_string();
                                }
                            }
                        }
                    }
                }
                "step.stop" => {
                    if let (Some(id), Some(name)) = (func_call_id.take(), func_call_name.take()) {
                        let args: Value = serde_json::from_str(&func_args).unwrap_or(json!({}));
                        func_args.clear();
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
                "interaction.completed" => {
                    if let Some(usage_value) = value.pointer("/interaction/usage") {
                        usage.merge_max(read_usage(usage_value));
                    }
                    stop_reason = value
                        .pointer("/interaction/status")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| Some("stop".to_owned()));
                    finished = true;
                    break;
                }
                "interaction.failed" | "error" => {
                    return Err(ProviderError::Remote {
                        provider: Provider::Gemini,
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
            let data = event.data.trim();
            if data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            match value.get("event_type").and_then(Value::as_str) {
                Some("interaction.completed") => {
                    if let Some(usage_value) = value.pointer("/interaction/usage") {
                        usage.merge_max(read_usage(usage_value));
                    }
                }
                Some("interaction.failed" | "error") => {
                    return Err(ProviderError::Remote {
                        provider: Provider::Gemini,
                        message: value_message(&value),
                    });
                }
                _ => {}
            }
        }
    }

    if !usage.is_empty() {
        emit(events, StreamEvent::Usage(usage)).await?;
    }
    emit(
        events,
        StreamEvent::Done {
            stop_reason: stop_reason.or_else(|| Some("end_turn".to_owned())),
        },
    )
    .await?;
    Ok(interaction_id)
}

fn read_usage(value: &Value) -> Usage {
    Usage::new(
        field_u32(value, "total_input_tokens"),
        field_u32(value, "total_output_tokens"),
        field_u32(value, "total_cached_tokens"),
        field_u32(value, "total_thought_tokens"),
    )
}

fn field_u32(value: &Value, key: &str) -> u32 {
    value.get(key).and_then(number_u32).unwrap_or(0)
}

fn text_from_content(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Array(items) => {
            let text = items
                .iter()
                .filter_map(text_from_content)
                .collect::<Vec<_>>()
                .join("");
            (!text.is_empty()).then_some(text)
        }
        Value::Object(map) => map
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| map.get("content").and_then(text_from_content)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::{ModelSelection, ReasoningEffort, RequestMessage, Role, find_model};

    fn request() -> ChatRequest {
        let model = ModelSelection::from_spec(find_model("gemini-4-argon").expect("model"));
        ChatRequest::new(
            model,
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
        .with_reasoning(ReasoningEffort::High)
        .with_max_output_tokens(4_096)
    }

    #[test]
    fn body_is_stateless_and_uses_typed_steps() {
        let value = body(&request());
        assert_eq!(value["model"], "gemini-4-argon");
        assert_eq!(value["store"], false);
        assert_eq!(value["stream"], true);
        assert_eq!(value["input"][0]["type"], "user_input");
        assert_eq!(value["input"][1]["type"], "model_output");
        assert_eq!(value["input"][0]["content"][0]["text"], "Hello");
        assert_eq!(value["system_instruction"], "Be terse.");
        assert_eq!(value["generation_config"]["thinking_level"], "high");
        assert_eq!(value["generation_config"]["thinking_summaries"], "auto");
        assert_eq!(value["generation_config"]["max_output_tokens"], 4_096);
        assert!(value.get("temperature").is_none());
    }

    #[test]
    fn off_disables_summaries() {
        let value = body(&request().with_reasoning(ReasoningEffort::Off));
        assert_eq!(value["generation_config"]["thinking_level"], "minimal");
        assert_eq!(value["generation_config"]["thinking_summaries"], "none");
    }

    #[test]
    fn content_extraction_handles_objects_and_arrays() {
        assert_eq!(
            text_from_content(&json!({"text":"a","type":"text"})).as_deref(),
            Some("a")
        );
        assert_eq!(
            text_from_content(&json!([{"text":"a"},{"text":"b"}])).as_deref(),
            Some("ab")
        );
        assert_eq!(text_from_content(&json!({"signature":"x"})), None);
    }
}
