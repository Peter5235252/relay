//! Bridges Tokio-based provider streams into the GPUI event loop.
//!
//! The provider client is async and requires Tokio. GPUI runs its own executor
//! on the UI thread, so Relay keeps a Tokio runtime for network work and hands
//! events back over a `smol` channel. That channel is executor-agnostic and is
//! what the GPUI task awaits.

use relay_core::{AgentMode, RequestMessage, Role, ToolApproval, ToolCall, should_auto_approve};
use relay_providers::{ChatRequest, ProviderClient, StreamEvent};
use smol::channel::{Receiver, unbounded};
use std::process::Command;
use tokio::runtime::Handle;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Approval request sent to the UI for a tool call.
pub struct ApprovalRequest {
    pub call: ToolCall,
    pub respond: oneshot::Sender<bool>,
}

/// A provider request that has been handed to the network runtime.
pub struct StartedStream {
    pub events: Receiver<StreamEvent>,
    pub error: Receiver<String>,
    pub approvals: Receiver<ApprovalRequest>,
    pub cancel: CancellationToken,
}

fn run_tool(call: &ToolCall, mode: AgentMode) -> String {
    // Strict Plan mode: read-only only.
    if mode == AgentMode::Plan && !matches!(call.name.as_str(), "read_file" | "list_dir") {
        return "error: tool not allowed in Plan mode (read-only)".to_owned();
    }
    match call.name.as_str() {
        "read_file" => {
            let path = call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                return "error: missing path".to_owned();
            }
            std::fs::read_to_string(path).unwrap_or_else(|e| format!("error: {e}"))
        }
        "list_dir" => {
            let path = call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or(".")
                .trim();
            let path = if path.is_empty() { "." } else { path };
            match std::fs::read_dir(path) {
                Ok(rd) => rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
                Err(e) => format!("error: {e}"),
            }
        }
        "run_command" => {
            if mode == AgentMode::Plan {
                return "error: run_command not allowed in Plan mode".to_owned();
            }
            let cmd = call
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if cmd.is_empty() {
                return "error: missing command".to_owned();
            }
            let out = Command::new("sh").arg("-c").arg(cmd).output();
            match out {
                Ok(o) => format!(
                    "exit={}\nstdout:\n{}\nstderr:\n{}",
                    o.status,
                    String::from_utf8_lossy(&o.stdout),
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => format!("error: {e}"),
            }
        }
        "write_file" => {
            if mode == AgentMode::Plan {
                return "error: write_file not allowed in Plan mode".to_owned();
            }
            let path = call
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim();
            if path.is_empty() {
                return "error: missing path".to_owned();
            }
            let content = call
                .arguments
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            match std::fs::write(path, content) {
                Ok(_) => "written".to_owned(),
                Err(e) => format!("error: {e}"),
            }
        }
        _ => format!("unknown tool: {}", call.name),
    }
}

pub fn start(
    handle: &Handle,
    request: ChatRequest,
    mode: AgentMode,
    approval: ToolApproval,
) -> Result<StartedStream, String> {
    let client = ProviderClient::new().map_err(|e| e.to_string())?;
    let (events_tx, events_rx) = unbounded::<StreamEvent>();
    let (error_tx, error_rx) = unbounded::<String>();
    let (approval_tx, approval_rx) = unbounded::<ApprovalRequest>();
    let cancel = CancellationToken::new();
    let token = cancel.clone();

    handle.spawn(async move {
        let mut request = request;
        let max_iters = 8;
        for _ in 0..max_iters {
            let (prov_tx, prov_rx) = unbounded::<StreamEvent>();
            let client2 = client.clone();
            let req = request.clone();
            let tok = token.clone();
            let jh = tokio::spawn(async move { client2.stream(&req, &prov_tx, &tok).await });

            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut pending_tool_events: Vec<ToolCall> = Vec::new();
            let mut assistant_text = String::new();
            while let Ok(ev) = prov_rx.recv().await {
                match &ev {
                    StreamEvent::ToolCall(c) => {
                        pending_tool_events.push(c.clone());
                    }
                    StreamEvent::Text(t) => {
                        assistant_text.push_str(t);
                        let _ = events_tx.send(ev).await;
                    }
                    _ => {
                        let _ = events_tx.send(ev).await;
                    }
                }
            }
            // Filter and dedupe tool calls.
            let mut seen = std::collections::HashSet::new();
            for c in pending_tool_events {
                if c.name.trim().is_empty() {
                    continue;
                }
                let key = if c.id.is_empty() {
                    format!("{}:{:?}", c.name, c.arguments)
                } else {
                    c.id.clone()
                };
                if seen.insert(key) {
                    tool_calls.push(c);
                }
            }
            // Enforce mode: drop disallowed tools in Plan.
            if mode == AgentMode::Plan {
                tool_calls.retain(|c| matches!(c.name.as_str(), "read_file" | "list_dir"));
            }
            for c in &tool_calls {
                let _ = events_tx.send(StreamEvent::ToolCall(c.clone())).await;
            }
            let stream_result = jh
                .await
                .unwrap_or(Err(relay_providers::ProviderError::Cancelled));
            if let Err(e) = stream_result {
                let _ = events_tx.close();
                let _ = error_tx.send(e.to_string()).await;
                return;
            }
            if tool_calls.is_empty() {
                break;
            }
            // Append assistant turn with tool calls.
            request.messages.push(RequestMessage {
                role: Role::Assistant,
                text: assistant_text.clone(),
                tool_calls: tool_calls.clone(),
                tool_call_id: None,
            });
            // Execute tools (with approval in Build mode).
            for call in &tool_calls {
                let mut allowed = should_auto_approve(mode, approval, &call.name, &call.arguments);
                if !allowed {
                    let (tx, rx) = oneshot::channel();
                    let _ = approval_tx
                        .send(ApprovalRequest {
                            call: call.clone(),
                            respond: tx,
                        })
                        .await;
                    // If UI doesn't respond, default deny.
                    allowed = rx.await.unwrap_or(false);
                }
                let output = if allowed {
                    run_tool(call, mode)
                } else {
                    "error: denied by user".to_owned()
                };
                let _ = events_tx
                    .send(StreamEvent::ToolResult {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        output: output.clone(),
                    })
                    .await;
                request.messages.push(RequestMessage {
                    role: Role::Tool,
                    text: output,
                    tool_calls: Vec::new(),
                    tool_call_id: Some(call.id.clone()),
                });
            }
        }
        drop(events_tx);
    });

    Ok(StartedStream {
        events: events_rx,
        error: error_rx,
        approvals: approval_rx,
        cancel,
    })
}
