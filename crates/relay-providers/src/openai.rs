//! OpenAI Responses API.
//!
//! Relay omits sampling controls because the GPT-6 family rejects temperature
//! on reasoning requests, and stores nothing server-side (`store: false`).

use reqwest::Client;
use serde_json::{Map, Value, json};
use smol::channel::Sender;
use tokio_util::sync::CancellationToken;

use relay_core::{Provider, Usage};

use crate::http::{
    base_url, join_url, json_headers, next_chunk, pointer_u32, secret_header, send_json,
    value_message,
};
use crate::request::ChatRequest;
use crate::sse::SseDecoder;
use crate::{OPENAI_BASE_URL, ProviderError, StreamEvent, emit};

/// Build a Responses request body.
pub(crate) fn body(request: &ChatRequest) -> Value {
    let input: Vec<Value> = request
        .messages
        .iter()
        .filter(|message| !message.text.trim().is_empty())
        .map(|message| {
            let role = match message.role {
                relay_core::Role::Assistant => "assistant",
                _ => "user",
            };
            json!({ "role": role, "content": message.text })
        })
        .collect();

    let mut map = Map::new();
    map.insert("model".to_owned(), json!(request.model.api_id));
    map.insert("input".to_owned(), Value::Array(input));
    map.insert("stream".to_owned(), json!(true));
    map.insert("store".to_owned(), json!(false));
    map.insert(
        "max_output_tokens".to_owned(),
        json!(request.output_limit()),
    );
    if let Some(system) = request.system_prompt() {
        map.insert("instructions".to_owned(), json!(system));
    }
    if let Some(effort) = request.reasoning.openai_effort() {
        map.insert(
            "reasoning".to_owned(),
            json!({ "effort": effort, "summary": "auto" }),
        );
    }
    Value::Object(map)
}

/// Stream an OpenAI response.
pub(crate) async fn stream(
    http: &Client,
    request: &ChatRequest,
    events: &Sender<StreamEvent>,
    cancel: &CancellationToken,
) -> Result<(), ProviderError> {
    let key = request
        .api_key
        .as_ref()
        .filter(|key| !key.is_empty())
        .ok_or(ProviderError::MissingApiKey {
            provider: Provider::OpenAi,
        })?;

    let base = base_url(request.endpoint.as_deref(), OPENAI_BASE_URL);
    let url = join_url(base, "/v1/responses")?;
    let mut headers = json_headers();
    secret_header(
        &mut headers,
        "authorization",
        &format!("Bearer {}", key.expose()),
        Provider::OpenAi,
    )?;

    let response = send_json(
        http,
        Provider::OpenAi,
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

    while let Some(chunk) = next_chunk(&mut stream, Provider::OpenAi, cancel).await? {
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
                .get("type")
                .and_then(Value::as_str)
                .or(event.event.as_deref())
                .unwrap_or_default();

            match kind {
                "response.output_text.delta" => {
                    if let Some(text) = value.get("delta").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        emit(events, StreamEvent::Text(text.to_owned())).await?;
                    }
                }
                "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                    if let Some(text) = value.get("delta").and_then(Value::as_str)
                        && !text.is_empty()
                    {
                        emit(events, StreamEvent::Thinking(text.to_owned())).await?;
                    }
                }
                "response.completed" => {
                    if let Some(usage_value) = value.pointer("/response/usage") {
                        usage.merge_max(read_usage(usage_value));
                    }
                    stop_reason = Some("stop".to_owned());
                    finished = true;
                    break;
                }
                "response.incomplete" => {
                    if let Some(usage_value) = value.pointer("/response/usage") {
                        usage.merge_max(read_usage(usage_value));
                    }
                    stop_reason = value
                        .pointer("/response/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                        .or_else(|| Some("incomplete".to_owned()));
                    finished = true;
                    break;
                }
                "response.failed" | "error" => {
                    return Err(ProviderError::Remote {
                        provider: Provider::OpenAi,
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
        for _event in decoder.finish() {}
    }

    if !usage.is_empty() {
        emit(events, StreamEvent::Usage(usage)).await?;
    }
    emit(events, StreamEvent::Done { stop_reason }).await?;
    Ok(())
}

fn read_usage(value: &Value) -> Usage {
    Usage::new(
        pointer_u32(value, "/input_tokens"),
        pointer_u32(value, "/output_tokens"),
        pointer_u32(value, "/input_tokens_details/cached_tokens"),
        pointer_u32(value, "/output_tokens_details/reasoning_tokens"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use relay_core::{ModelSelection, ReasoningEffort, RequestMessage, Role, find_model};

    fn request() -> ChatRequest {
        let model = ModelSelection::from_spec(find_model("gpt-6.1-sol").expect("model"));
        ChatRequest::new(
            model,
            vec![
                RequestMessage {
                    role: Role::User,
                    text: "Hello".to_owned(),
                },
                RequestMessage {
                    role: Role::Assistant,
                    text: "Hi".to_owned(),
                },
                RequestMessage {
                    role: Role::User,
                    text: "Continue".to_owned(),
                },
            ],
        )
        .with_system_prompt(Some("Be terse.".to_owned()))
        .with_reasoning(ReasoningEffort::Medium)
    }

    #[test]
    fn body_omits_sampling_controls_and_sets_reasoning() {
        let value = body(&request());
        assert_eq!(value["model"], "gpt-6.1-sol");
        assert_eq!(value["store"], false);
        assert_eq!(value["stream"], true);
        assert_eq!(value["instructions"], "Be terse.");
        assert_eq!(value["reasoning"]["effort"], "medium");
        assert_eq!(value["reasoning"]["summary"], "auto");
        assert_eq!(value["input"][0]["role"], "user");
        assert_eq!(value["input"][1]["role"], "assistant");
        assert!(value.get("temperature").is_none());
    }

    #[test]
    fn off_omits_reasoning_object() {
        let value = body(&request().with_reasoning(ReasoningEffort::Off));
        assert!(value.get("reasoning").is_none());
    }

    #[test]
    fn usage_maps_all_counters() {
        let value = json!({
            "input_tokens": 100,
            "output_tokens": 40,
            "input_tokens_details": {"cached_tokens": 30},
            "output_tokens_details": {"reasoning_tokens": 12}
        });
        assert_eq!(read_usage(&value), Usage::new(100, 40, 30, 12));
    }
}
