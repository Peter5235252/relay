//! Small reference client for `relay --daemon`.
//!
//! It is intentionally dependency-free beyond the daemon crate so it can be
//! used to smoke-test the IPC surface from a terminal.

use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use relay_daemon::client::Client;
use serde_json::{Value, json};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut dir: Option<PathBuf> = None;
    let mut json_output = false;
    let mut show_thinking = false;
    let mut model: Option<String> = None;
    let mut conversation: Option<String> = None;
    let mut command: Option<String> = None;
    let mut rest = Vec::new();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            "--json" => json_output = true,
            "--thinking" => show_thinking = true,
            "--model" => model = args.next(),
            "--conversation" => conversation = args.next(),
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            _ if command.is_none() => command = Some(arg),
            _ => rest.push(arg),
        }
    }

    let Some(command) = command else {
        print_help();
        return Ok(());
    };
    let mut client = match &dir {
        Some(dir) => Client::connect_dir(dir).await?,
        None => Client::connect().await?,
    };

    match command.as_str() {
        "ping" => {
            let value = client.call("ping", json!({})).await?;
            print_value(&value, json_output);
        }
        "status" => {
            let value = client.call("daemon.status", json!({})).await?;
            print_value(&value, json_output);
        }
        "models" => {
            let value = client.call("models.list", json!({})).await?;
            if json_output {
                print_json(&value);
            } else if let Some(models) = value.get("models").and_then(Value::as_array) {
                for model in models {
                    println!(
                        "{:<36} {:<18} {}",
                        model.get("slug").and_then(Value::as_str).unwrap_or(""),
                        model.get("provider").and_then(Value::as_str).unwrap_or(""),
                        model.get("name").and_then(Value::as_str).unwrap_or(""),
                    );
                }
            }
        }
        "tools" => {
            let value = client.call("tools.list", json!({})).await?;
            if json_output {
                print_json(&value);
            } else if let Some(tools) = value.get("tools").and_then(Value::as_array) {
                for tool in tools {
                    println!(
                        "{:<20} {}",
                        tool.get("name").and_then(Value::as_str).unwrap_or(""),
                        tool.get("description")
                            .and_then(Value::as_str)
                            .unwrap_or(""),
                    );
                }
            }
        }
        "presets" => {
            let value = client.call("presets.list", json!({})).await?;
            print_value(&value, json_output);
        }
        "settings" => {
            let value = if rest.first().map(String::as_str) == Some("update") {
                let patch: Value = serde_json::from_str(&rest[1..].join(" "))
                    .map_err(|error| format!("invalid JSON patch: {error}"))?;
                client
                    .call("settings.update", json!({"patch": patch}))
                    .await?
            } else {
                client.call("settings.get", json!({})).await?
            };
            print_value(&value, json_output);
        }
        "conversations" => {
            let sub = rest.first().map(String::as_str).unwrap_or("list");
            match sub {
                "list" => {
                    let value = client.call("conversations.list", json!({})).await?;
                    if json_output {
                        print_json(&value);
                    } else {
                        print_conversations(&value);
                    }
                }
                "search" => {
                    let query = rest[1..].join(" ");
                    let value = client
                        .call("conversations.search", json!({"query": query}))
                        .await?;
                    if json_output {
                        print_json(&value);
                    } else {
                        print_conversations(&value);
                    }
                }
                "show" => {
                    let id = rest
                        .get(1)
                        .ok_or_else(|| "usage: conversations show <id>".to_owned())?;
                    let value = client
                        .call("conversations.get", json!({"conversation_id": id}))
                        .await?;
                    if json_output {
                        print_json(&value);
                    } else {
                        print_conversation(&value);
                    }
                }
                other => return Err(format!("unknown conversations command: {other}")),
            }
        }
        "memory" => match rest.first().map(String::as_str).unwrap_or("list") {
            "list" => {
                let value = client.call("memory.list", json!({})).await?;
                print_value(&value, json_output);
            }
            "add" => {
                let content = rest[1..].join(" ");
                let value = client
                    .call("memory.add", json!({"content": content}))
                    .await?;
                print_value(&value, json_output);
            }
            "delete" => {
                let id = rest
                    .get(1)
                    .ok_or_else(|| "usage: memory delete <id>".to_owned())?;
                let value = client.call("memory.delete", json!({"id": id})).await?;
                print_value(&value, json_output);
            }
            other => return Err(format!("unknown memory command: {other}")),
        },
        "approve" => {
            let approved = !rest.iter().any(|arg| arg == "--deny");
            let id = rest
                .iter()
                .find(|arg| !arg.starts_with("--"))
                .ok_or_else(|| "usage: approve <approval_id> [--deny]".to_owned())?;
            let value = client
                .call(
                    "approval.respond",
                    json!({"approval_id": id, "approved": approved}),
                )
                .await?;
            print_value(&value, json_output);
        }
        "send" | "ask" => {
            let prompt = if rest.is_empty() {
                let mut input = String::new();
                std::io::stdin()
                    .read_to_string(&mut input)
                    .map_err(|error| error.to_string())?;
                input
            } else {
                rest.join(" ")
            };
            if prompt.trim().is_empty() {
                return Err("a prompt is required".to_owned());
            }
            let mut params = json!({"message": prompt});
            if let Some(model) = &model {
                params["model"] = json!(model);
            }
            if let Some(conversation) = &conversation {
                params["conversation_id"] = json!(conversation);
            }

            let mut printed = false;
            let result = client
                .call_with_events("chat.send", params, |event| {
                    if json_output {
                        return;
                    }
                    match event.method.as_str() {
                        "turn.text" => {
                            if let Some(delta) =
                                event.params.get("delta").and_then(Value::as_str)
                            {
                                print!("{delta}");
                                let _ = std::io::stdout().flush();
                                printed = true;
                            }
                        }
                        "turn.thinking" => {
                            if show_thinking
                                && let Some(delta) =
                                    event.params.get("delta").and_then(Value::as_str)
                            {
                                eprint!("{delta}");
                                let _ = std::io::stderr().flush();
                            }
                        }
                        "turn.tool_call" => {
                            let call = event.params.get("call");
                            eprintln!(
                                "\n[tool] {} {}",
                                call.and_then(|call| call.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("?"),
                                call.and_then(|call| call.get("arguments"))
                                    .map(Value::to_string)
                                    .unwrap_or_default(),
                            );
                        }
                        "turn.tool_result" => {
                            eprintln!(
                                "[tool result] {}: {}",
                                event
                                    .params
                                    .get("name")
                                    .and_then(Value::as_str)
                                    .unwrap_or("?"),
                                short(
                                    event
                                        .params
                                        .get("output")
                                        .and_then(Value::as_str)
                                        .unwrap_or(""),
                                    400,
                                ),
                            );
                        }
                        "approval.requested" => {
                            let call = event.params.get("call");
                            eprintln!(
                                "\n[approval] {} requires approval:\n{}\n\
                                 Run `relay-daemon-client approve {}` (or `--deny`) in another terminal.",
                                call.and_then(|call| call.get("name"))
                                    .and_then(Value::as_str)
                                    .unwrap_or("?"),
                                call.and_then(|call| call.get("arguments"))
                                    .map(|arguments| serde_json::to_string_pretty(arguments)
                                        .unwrap_or_else(|_| arguments.to_string()))
                                    .unwrap_or_default(),
                                event
                                    .params
                                    .get("approval_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("?"),
                            );
                        }
                        "approval.expired" => {
                            eprintln!(
                                "[approval] request {} expired",
                                event
                                    .params
                                    .get("approval_id")
                                    .and_then(Value::as_str)
                                    .unwrap_or("?"),
                            );
                        }
                        _ => {}
                    }
                })
                .await;

            let value = result?;
            if json_output {
                print_json(&value);
            } else {
                if printed {
                    println!();
                }
                let text = value.get("text").and_then(Value::as_str).unwrap_or("");
                if !printed && !text.is_empty() {
                    println!("{text}");
                }
                if let Some(error) = value.get("error").and_then(Value::as_str) {
                    eprintln!("error: {error}");
                }
                if value
                    .get("cancelled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    eprintln!("cancelled");
                }
                eprintln!(
                    "conversation {} · turn {}",
                    value
                        .get("conversation_id")
                        .and_then(Value::as_str)
                        .unwrap_or("?"),
                    value.get("turn_id").and_then(Value::as_str).unwrap_or("?"),
                );
            }
        }
        "shutdown" => {
            let value = client.call("daemon.shutdown", json!({})).await?;
            print_value(&value, json_output);
        }
        other => return Err(format!("unknown command: {other}")),
    }

    Ok(())
}

fn print_help() {
    println!(
        "\
relay-daemon-client — talk to `relay --daemon`

Usage:
  relay-daemon-client [OPTIONS] <COMMAND> [ARGS]

Options:
  --dir <DIR>            Runtime directory (default: $XDG_RUNTIME_DIR/relay)
  --json                 Print machine-readable JSON
  --thinking             Show reasoning deltas while sending
  --model <SLUG>         Model override for `send`
  --conversation <ID>    Continue an existing conversation
  -h, --help             Print this help

Commands:
  ping                   Check that the daemon is alive
  status                 Show daemon, store, and bundled-model status
  models                 List curated and local models
  tools                  List tools available with the current settings
  presets                List personality presets
  settings [update JSON] Read settings, or merge a JSON object into them
  conversations [list|search <query>|show <id>]
  memory [list|add <text>|delete <id>]
  send [PROMPT]          Send a prompt and stream the answer (stdin if omitted)
  approve <ID> [--deny]  Answer a pending tool approval
  shutdown               Stop the daemon"
    );
}

fn print_value(value: &Value, json_output: bool) {
    if json_output {
        print_json(value);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
        );
    }
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
    );
}

fn print_conversations(value: &Value) {
    let Some(items) = value.get("conversations").and_then(Value::as_array) else {
        print_json(value);
        return;
    };
    if items.is_empty() {
        println!("No conversations.");
        return;
    }
    for item in items {
        println!(
            "{}  {:<48}  {:<18}  {} msgs",
            item.get("id").and_then(Value::as_str).unwrap_or(""),
            short(item.get("title").and_then(Value::as_str).unwrap_or(""), 48),
            item.get("model_slug").and_then(Value::as_str).unwrap_or(""),
            item.get("message_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    }
}

fn print_conversation(value: &Value) {
    if let Some(conversation) = value.get("conversation") {
        println!(
            "{}  [{}]",
            conversation
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("?"),
            conversation
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("?"),
        );
        println!();
    }
    if let Some(messages) = value.get("messages").and_then(Value::as_array) {
        for message in messages {
            let role = message.get("role").and_then(Value::as_str).unwrap_or("?");
            println!("[{role}] {}", message_text(message));
            println!();
        }
    }
}

fn message_text(message: &Value) -> String {
    let mut out = String::new();
    if let Some(blocks) = message.get("blocks").and_then(Value::as_array) {
        for block in blocks {
            if let Some(text) = block.get("text").and_then(Value::as_str) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            }
        }
    }
    out
}

fn short(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.to_owned();
    }
    let mut out: String = value.chars().take(max).collect();
    out.push('…');
    out
}
