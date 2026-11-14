//! Shared HTTP, URL, error-body, and stream helpers.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use relay_core::Provider;
use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Response};
use tokio_util::sync::CancellationToken;

use crate::ProviderError;

/// Maximum time between streamed chunks before a request is abandoned.
const IDLE_TIMEOUT: Duration = Duration::from_secs(180);
/// Maximum time to wait for response headers.
const HEADERS_TIMEOUT: Duration = Duration::from_secs(120);
/// Error bodies are truncated to this many bytes.
const ERROR_BODY_LIMIT: usize = 16 * 1024;
/// The message shown to users is truncated to this many characters.
const ERROR_MESSAGE_LIMIT: usize = 700;

/// Build the headers every JSON streaming request needs.
pub(crate) fn json_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers
}

/// Insert a credential header without ever including it in an error message.
pub(crate) fn secret_header(
    headers: &mut HeaderMap,
    name: &'static str,
    value: &str,
    provider: Provider,
) -> Result<(), ProviderError> {
    let value =
        HeaderValue::from_str(value).map_err(|_| ProviderError::InvalidApiKey { provider })?;
    headers.insert(HeaderName::from_static(name), value);
    Ok(())
}

/// POST a JSON body and return a successful streaming response.
///
/// `secret` is used only to scrub the key from a provider error body.
pub(crate) async fn send_json(
    http: &Client,
    provider: Provider,
    url: &str,
    headers: HeaderMap,
    body: &serde_json::Value,
    secret: Option<&str>,
    cancel: &CancellationToken,
) -> Result<Response, ProviderError> {
    let request = http.post(url).headers(headers).json(body);
    let send = tokio::time::timeout(HEADERS_TIMEOUT, request.send());

    let response = tokio::select! {
        _ = cancel.cancelled() => return Err(ProviderError::Cancelled),
        result = send => {
            match result {
                Err(_) => return Err(ProviderError::Timeout { provider }),
                Ok(Err(error)) => {
                    return Err(ProviderError::Transport {
                        provider,
                        message: redact(&error.to_string(), secret),
                    });
                }
                Ok(Ok(response)) => response,
            }
        }
    };

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let body = read_error_body(response).await;
        let body = redact(&body, secret);
        return Err(ProviderError::Status {
            provider,
            status,
            message: remote_message(&body),
        });
    }

    Ok(response)
}

/// Read at most one chunk from a streaming response, honoring cancellation and
/// the idle timeout.
pub(crate) async fn next_chunk<S>(
    stream: &mut S,
    provider: Provider,
    cancel: &CancellationToken,
) -> Result<Option<Bytes>, ProviderError>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    tokio::select! {
        _ = cancel.cancelled() => Err(ProviderError::Cancelled),
        result = tokio::time::timeout(IDLE_TIMEOUT, stream.next()) => {
            match result {
                Err(_) => Err(ProviderError::Timeout { provider }),
                Ok(None) => Ok(None),
                Ok(Some(Ok(chunk))) => Ok(Some(chunk)),
                Ok(Some(Err(error))) => Err(ProviderError::Stream {
                    provider,
                    message: error.to_string(),
                }),
            }
        }
    }
}

async fn read_error_body(response: Response) -> String {
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(Ok(chunk)) = stream.next().await {
        let remaining = ERROR_BODY_LIMIT.saturating_sub(body.len());
        if remaining == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// Extract a concise, human-readable message from an error body.
pub(crate) fn remote_message(body: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        for pointer in [
            "/error/message",
            "/error/error/message",
            "/message",
            "/detail",
            "/error",
        ] {
            if let Some(text) = value.pointer(pointer).and_then(serde_json::Value::as_str) {
                return truncate(&compact(text), ERROR_MESSAGE_LIMIT);
            }
        }
    }
    truncate(&compact(body), ERROR_MESSAGE_LIMIT)
}

/// Remove a known credential from text before it is shown, logged, or stored.
pub(crate) fn redact(text: &str, secret: Option<&str>) -> String {
    let Some(secret) = secret.filter(|secret| secret.len() >= 4) else {
        return text.to_owned();
    };
    text.replace(secret, "[redacted]")
}

/// Resolve a provider base URL, applying an explicit override.
pub(crate) fn base_url<'a>(endpoint: Option<&'a str>, default: &'a str) -> &'a str {
    endpoint
        .map(str::trim)
        .filter(|endpoint| !endpoint.is_empty())
        .unwrap_or(default)
}

/// Join a base URL and an absolute path.
pub(crate) fn join_url(base: &str, path: &str) -> Result<String, ProviderError> {
    let base = base.trim().trim_end_matches('/');
    if base.is_empty() {
        return Err(ProviderError::InvalidEndpoint(
            "endpoint is empty".to_owned(),
        ));
    }
    if !(base.starts_with("http://") || base.starts_with("https://")) {
        return Err(ProviderError::InvalidEndpoint(base.to_owned()));
    }
    Ok(format!("{base}{path}"))
}

/// Read a JSON number that may be encoded as a string.
pub(crate) fn number_u32(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .map(|number| number.min(u64::from(u32::MAX)) as u32)
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<u32>().ok())
        })
}

/// Read a JSON pointer as a `u32`, defaulting to zero.
pub(crate) fn pointer_u32(value: &serde_json::Value, pointer: &str) -> u32 {
    value
        .pointer(pointer)
        .and_then(number_u32)
        .unwrap_or_default()
}

/// Extract a provider message from an in-stream error object.
pub(crate) fn value_message(value: &serde_json::Value) -> String {
    for pointer in [
        "/error/message",
        "/error/error/message",
        "/error",
        "/message",
        "/detail",
    ] {
        if let Some(text) = value.pointer(pointer).and_then(serde_json::Value::as_str) {
            return truncate(&compact(text), ERROR_MESSAGE_LIMIT);
        }
    }
    truncate(&compact(&value.to_string()), ERROR_MESSAGE_LIMIT)
}

fn compact(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_message_prefers_structured_fields() {
        let body = r#"{"error":{"message":"invalid api key"},"detail":"ignored"}"#;
        assert_eq!(remote_message(body), "invalid api key");
        assert_eq!(remote_message("  plain   failure \n"), "plain failure");
    }

    #[test]
    fn redaction_removes_the_exact_secret() {
        assert_eq!(
            redact("bad key sk-ant-secret here", Some("sk-ant-secret")),
            "bad key [redacted] here"
        );
        assert_eq!(redact("no secret", None), "no secret");
        assert_eq!(redact("too short", Some("abc")), "too short");
    }

    #[test]
    fn urls_are_validated() {
        assert_eq!(
            join_url("http://localhost:11434/", "/api/chat").expect("valid"),
            "http://localhost:11434/api/chat"
        );
        assert!(join_url("", "/x").is_err());
        assert!(join_url("localhost:1234", "/x").is_err());
    }

    #[test]
    fn numbers_accept_strings() {
        let value = serde_json::json!({"a": 12, "b": "34"});
        assert_eq!(pointer_u32(&value, "/a"), 12);
        assert_eq!(pointer_u32(&value, "/b"), 34);
        assert_eq!(pointer_u32(&value, "/missing"), 0);
    }
}
