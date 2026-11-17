#![allow(clippy::expect_used, clippy::unwrap_used)]

//! End-to-end streaming tests against local mock HTTP servers.

use std::time::Duration;

use relay_core::{ModelSelection, RequestMessage, Role, Usage, find_model};
use relay_providers::{ChatRequest, ProviderClient, ProviderError, StreamEvent};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn model(slug: &str) -> ModelSelection {
    ModelSelection::from_spec(find_model(slug).expect("model exists"))
}

fn request(model: ModelSelection, endpoint: &str, key: &str) -> ChatRequest {
    ChatRequest::new(
        model,
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
    .with_endpoint(endpoint)
    .with_api_key(key)
}

async fn collect(
    mut request: ChatRequest,
    cancel: CancellationToken,
) -> Result<Vec<StreamEvent>, ProviderError> {
    let client = ProviderClient::new().expect("client");
    let (tx, rx) = smol::channel::unbounded();
    let result = client.stream(&mut request, &tx, &cancel).await;
    drop(tx);
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    result?;
    Ok(events)
}

fn sse(body: &'static str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_raw(body, "text/event-stream")
}

#[tokio::test]
async fn gemini_streams_text_thinking_usage_and_done() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/interactions"))
        .respond_with(sse(
            "event: step.delta\n\
             data: {\"event_type\":\"step.delta\",\"delta\":{\"type\":\"thought_summary\",\"content\":{\"type\":\"text\",\"text\":\"Plan\"}}}\n\n\
             event: step.delta\n\
             data: {\"event_type\":\"step.delta\",\"delta\":{\"type\":\"text\",\"text\":\"Hello\"}}\n\n\
             event: interaction.completed\n\
             data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"completed\",\"usage\":{\"total_input_tokens\":10,\"total_output_tokens\":5,\"total_cached_tokens\":2,\"total_thought_tokens\":3}}}\n\n",
        ))
        .mount(&server)
        .await;

    let events = collect(
        request(model("gemini-4-argon"), &server.uri(), "test-key"),
        CancellationToken::new(),
    )
    .await
    .expect("stream");

    assert!(events.contains(&StreamEvent::Thinking("Plan".to_owned())));
    assert!(events.contains(&StreamEvent::Text("Hello".to_owned())));
    assert!(events.contains(&StreamEvent::Usage(Usage::new(10, 5, 2, 3))));
    assert!(events.contains(&StreamEvent::Done {
        stop_reason: Some("completed".to_owned())
    }));
}

#[tokio::test]
async fn anthropic_streams_thinking_and_maps_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse(
            "event: message_start\n\
             data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_read_input_tokens\":4,\"output_tokens\":0}}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"Plan\"}}\n\n\
             event: content_block_delta\n\
             data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"Answer\"}}\n\n\
             event: message_delta\n\
             data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\n\
             event: message_stop\n\
             data: {\"type\":\"message_stop\"}\n\n",
        ))
        .mount(&server)
        .await;

    let events = collect(
        request(model("claude-sonnet-5-5"), &server.uri(), "test-key"),
        CancellationToken::new(),
    )
    .await
    .expect("stream");

    assert!(events.contains(&StreamEvent::Thinking("Plan".to_owned())));
    assert!(events.contains(&StreamEvent::Text("Answer".to_owned())));
    assert!(events.contains(&StreamEvent::Usage(Usage::new(12, 7, 4, 0))));
    assert!(events.contains(&StreamEvent::Done {
        stop_reason: Some("end_turn".to_owned())
    }));
}

#[tokio::test]
async fn openai_streams_reasoning_summary_and_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse(
            "event: response.reasoning_summary_text.delta\n\
             data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"Reason\"}\n\n\
             event: response.output_text.delta\n\
             data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hi\"}\n\n\
             event: response.completed\n\
             data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":20,\"output_tokens\":8,\"input_tokens_details\":{\"cached_tokens\":5},\"output_tokens_details\":{\"reasoning_tokens\":2}}}}\n\n",
        ))
        .mount(&server)
        .await;

    let events = collect(
        request(model("gpt-6.2-sol"), &server.uri(), "test-key"),
        CancellationToken::new(),
    )
    .await
    .expect("stream");

    assert!(events.contains(&StreamEvent::Thinking("Reason".to_owned())));
    assert!(events.contains(&StreamEvent::Text("Hi".to_owned())));
    assert!(events.contains(&StreamEvent::Usage(Usage::new(20, 8, 5, 2))));
    assert!(events.contains(&StreamEvent::Done {
        stop_reason: Some("stop".to_owned())
    }));
}

#[tokio::test]
async fn ollama_streams_ndjson_with_usage() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/x-ndjson")
                .set_body_string(
                    "{\"message\":{\"role\":\"assistant\",\"thinking\":\"hmm\"},\"done\":false}\n\
                     {\"message\":{\"role\":\"assistant\",\"content\":\"Hello\"},\"done\":false}\n\
                     {\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":11,\"eval_count\":6}\n",
                ),
        )
        .mount(&server)
        .await;

    let events = collect(
        request(ModelSelection::ollama("qwen3:8b"), &server.uri(), ""),
        CancellationToken::new(),
    )
    .await
    .expect("stream");

    assert!(events.contains(&StreamEvent::Thinking("hmm".to_owned())));
    assert!(events.contains(&StreamEvent::Text("Hello".to_owned())));
    assert!(events.contains(&StreamEvent::Usage(Usage::new(11, 6, 0, 0))));
    assert!(events.contains(&StreamEvent::Done {
        stop_reason: Some("stop".to_owned())
    }));
}

#[tokio::test]
async fn local_endpoint_streams_chat_completions() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(sse(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"why\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{\"content\":\"Hey\"}}]}\n\n\
             data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":4}}\n\n\
             data: [DONE]\n\n",
        ))
        .mount(&server)
        .await;

    let events = collect(
        request(
            ModelSelection::openai_compatible("qwen3-coder-30b"),
            &format!("{}/v1", server.uri()),
            "",
        ),
        CancellationToken::new(),
    )
    .await
    .expect("stream");

    assert!(events.contains(&StreamEvent::Thinking("why".to_owned())));
    assert!(events.contains(&StreamEvent::Text("Hey".to_owned())));
    assert!(events.contains(&StreamEvent::Usage(Usage::new(9, 4, 0, 0))));
    assert!(events.contains(&StreamEvent::Done {
        stop_reason: Some("stop".to_owned())
    }));
}

#[tokio::test]
async fn http_errors_are_typed_and_redacted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_raw(
            "{\"error\":{\"message\":\"invalid key sk-ant-secret\"}}",
            "application/json",
        ))
        .mount(&server)
        .await;

    let error = collect(
        request(model("claude-opus-5-5"), &server.uri(), "sk-ant-secret"),
        CancellationToken::new(),
    )
    .await
    .expect_err("expected status error");

    match error {
        ProviderError::Status {
            status, message, ..
        } => {
            assert_eq!(status, 401);
            assert!(!message.contains("sk-ant-secret"));
            assert!(message.contains("[redacted]"));
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn missing_keys_fail_before_any_request() {
    let server = MockServer::start().await;
    let request = ChatRequest::new(
        model("gpt-6.2-astra"),
        vec![RequestMessage {
            role: Role::User,
            text: "Hello".to_owned(),
            images: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }],
    )
    .with_endpoint(server.uri());

    let error = collect(request, CancellationToken::new())
        .await
        .expect_err("missing key");
    assert!(matches!(error, ProviderError::MissingApiKey { .. }));
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty()
    );
}

#[tokio::test]
async fn cancellation_stops_a_pending_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            sse("event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"late\"}\n\n")
                .set_delay(Duration::from_secs(5)),
        )
        .mount(&server)
        .await;

    let cancel = CancellationToken::new();
    cancel.cancel();
    let error = collect(
        request(model("gpt-6-luna"), &server.uri(), "test-key"),
        cancel,
    )
    .await
    .expect_err("cancelled");
    assert!(matches!(error, ProviderError::Cancelled));
}
