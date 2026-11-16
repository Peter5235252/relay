//! Interactive and one-shot chat.
//!
//! The transport, tool loop, approvals, and request assembly come from
//! `relay-engine`. This module only owns terminal presentation, slash
//! commands, and the conversation lifecycle. That keeps the CLI's behavior
//! identical to the desktop app instead of a parallel implementation.

use std::io::{IsTerminal as _, Read as _, Write};

use relay_core::{
    AgentMode, ChatMessage, ContentBlock, ConversationSummary, ModelSelection, Provider,
    ReasoningEffort, Role, ToolApproval, Usage, format_tokens,
};
use relay_engine::{agent, bundled, keys, local, mcp, memory, search, streaming};
use relay_providers::StreamEvent;

use crate::commands::{self, CliContext};
use crate::output;

/// Options accepted by `chat`, `ask`, and bare prompts.
#[derive(Debug, Clone)]
pub struct ChatOptions {
    /// Model slug override.
    pub model: Option<String>,
    /// Conversation id/prefix, or `last`.
    pub session: Option<String>,
    /// Start a blank conversation even when history exists.
    pub new: bool,
    /// Reasoning effort override.
    pub reasoning: Option<ReasoningEffort>,
    /// Interaction mode override (`chat` or `work`).
    pub interaction_mode: Option<String>,
    /// Plan/Build override.
    pub agent_mode: Option<AgentMode>,
    /// Tool approval policy override.
    pub approval: Option<ToolApproval>,
    /// Confirm downloads non-interactively.
    pub yes: bool,
    /// Emit a JSON result instead of the rendered answer.
    pub json: bool,
    /// Reasoning display override.
    pub show_thinking: Option<bool>,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            model: None,
            session: None,
            new: false,
            reasoning: None,
            interaction_mode: None,
            agent_mode: None,
            approval: None,
            yes: false,
            json: false,
            show_thinking: None,
        }
    }
}

/// The result of one assistant turn.
pub struct TurnReport {
    /// Visible answer text.
    pub text: String,
    /// Reasoning summary text, even when not displayed.
    pub thinking: String,
    /// Reported usage.
    pub usage: Option<Usage>,
    /// Estimated list-price cost.
    pub cost: Option<f64>,
    /// Provider or transport error.
    pub error: Option<String>,
    /// Whether the user stopped the response.
    pub cancelled: bool,
}

struct ChatSession {
    conversation_id: Option<String>,
    title: String,
    model_slug: String,
    messages: Vec<ChatMessage>,
    yes: bool,
    show_thinking: bool,
}

enum Flow {
    Continue,
    Quit,
}

/// Run the interactive REPL.
pub async fn run_chat(ctx: &mut CliContext, options: ChatOptions) -> Result<(), String> {
    let show_thinking = apply_options(ctx, &options)?;
    let mut session = open_session(ctx, &options, false)?;
    session.show_thinking = show_thinking;
    session.yes = options.yes;
    print_banner(ctx, &session);
    if !session.messages.is_empty() {
        let start = session.messages.len().saturating_sub(10);
        print!(
            "{}",
            render_messages_from(&session.messages, start, show_thinking)
        );
        if start > 0 {
            println!("({start} earlier messages hidden; use /show all to see them)");
        }
        println!();
    }
    spawn_auto_summary(ctx);

    loop {
        let prompt = match read_input("› ")? {
            Some(prompt) => prompt,
            None => break,
        };
        let prompt = prompt.trim();
        if prompt.is_empty() {
            continue;
        }
        if prompt.starts_with('/') {
            match run_slash(ctx, &mut session, prompt).await {
                Ok(Flow::Continue) => {}
                Ok(Flow::Quit) => break,
                Err(error) => eprintln!("error: {error}"),
            }
            continue;
        }

        match send_prompt(ctx, &mut session, prompt, false).await {
            Ok(report) => {
                if let Some(error) = report.error {
                    eprintln!("{}", output::red(&format!("error: {error}")));
                }
            }
            Err(error) => eprintln!("error: {error}"),
        }
    }
    Ok(())
}

/// Run one prompt and exit. With `--json`, stdout contains only one JSON object.
pub async fn run_prompt(
    ctx: &mut CliContext,
    options: ChatOptions,
    prompt: String,
) -> Result<(), String> {
    let show_thinking = apply_options(ctx, &options)?;
    let prompt = if prompt.trim().is_empty() {
        if std::io::stdin().is_terminal() {
            return Err("No prompt given. Try `relay-cli \"your question\"`.".to_owned());
        }
        let mut input = String::new();
        std::io::stdin()
            .read_to_string(&mut input)
            .map_err(|error| error.to_string())?;
        input
    } else {
        prompt
    };
    if prompt.trim().is_empty() {
        return Err("The prompt is empty.".to_owned());
    }

    let mut session = open_session(ctx, &options, true)?;
    session.show_thinking = show_thinking;
    session.yes = options.yes;
    let report = send_prompt(ctx, &mut session, prompt.trim(), options.json).await?;
    if options.json {
        let usage = report.usage.map(|usage| {
            serde_json::json!({
                "input_tokens": usage.input_tokens,
                "output_tokens": usage.output_tokens,
                "cached_input_tokens": usage.cached_input_tokens,
                "reasoning_tokens": usage.reasoning_tokens,
            })
        });
        output::json(&serde_json::json!({
            "conversation_id": session.conversation_id,
            "title": session.title,
            "model": session.model_slug,
            "text": report.text,
            "thinking": if show_thinking { Some(report.thinking.clone()) } else { None },
            "usage": usage,
            "cost_usd": report.cost,
            "error": report.error,
            "cancelled": report.cancelled,
        }))?;
    }
    match report.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn apply_options(ctx: &mut CliContext, options: &ChatOptions) -> Result<bool, String> {
    if let Some(model) = &options.model {
        if !agent::is_known_model(model) {
            return Err(format!("Unknown model: {model}"));
        }
        ctx.settings.default_model = model.clone();
    }
    if let Some(reasoning) = options.reasoning {
        ctx.settings.reasoning = reasoning;
    }
    if let Some(mode) = &options.interaction_mode {
        ctx.settings.interaction_mode = commands::parse_interaction(mode)?;
    }
    if let Some(mode) = options.agent_mode {
        ctx.settings.agent_mode = mode;
    }
    if let Some(approval) = options.approval {
        ctx.settings.tool_approval = approval;
    }
    if let Some(show) = options.show_thinking {
        ctx.settings.show_thinking = show;
    }
    ctx.save()?;
    Ok(ctx.settings.show_thinking)
}

fn open_session(
    ctx: &CliContext,
    options: &ChatOptions,
    one_shot: bool,
) -> Result<ChatSession, String> {
    if !options.new
        && let Some(selector) = &options.session
    {
        let summary = commands::resolve_session(&ctx.store, selector)?;
        return session_from_summary(ctx, summary, options.model.as_deref());
    }
    if !options.new
        && !one_shot
        && let Some(summary) = ctx
            .store
            .latest_conversation()
            .map_err(|error| error.to_string())?
    {
        return session_from_summary(ctx, summary, options.model.as_deref());
    }

    let slug = options
        .model
        .clone()
        .unwrap_or_else(|| ctx.settings.default_model.clone());
    Ok(ChatSession {
        conversation_id: None,
        title: "New chat".to_owned(),
        model_slug: slug,
        messages: Vec::new(),
        yes: options.yes,
        show_thinking: ctx.settings.show_thinking,
    })
}

fn session_from_summary(
    ctx: &CliContext,
    summary: ConversationSummary,
    model_override: Option<&str>,
) -> Result<ChatSession, String> {
    let messages = ctx
        .store
        .messages(&summary.id)
        .map_err(|error| error.to_string())?;
    let model_slug = match model_override {
        Some(slug) => {
            ctx.store
                .set_conversation_model(&summary.id, slug)
                .map_err(|error| error.to_string())?;
            slug.to_owned()
        }
        None => summary.model_slug,
    };
    Ok(ChatSession {
        conversation_id: Some(summary.id),
        title: summary.title,
        model_slug,
        messages,
        yes: false,
        show_thinking: ctx.settings.show_thinking,
    })
}

fn print_banner(ctx: &CliContext, session: &ChatSession) {
    let model = agent::resolve_model(&session.model_slug);
    println!("Relay CLI {} · {}", env!("CARGO_PKG_VERSION"), model.name);
    println!(
        "{} · {} · reasoning {} · approval {}",
        if ctx.settings.interaction_mode == "chat" {
            "Chat"
        } else {
            "Work"
        },
        ctx.settings.agent_mode.label(),
        ctx.settings.reasoning.label(),
        ctx.settings.tool_approval.label()
    );
    match &session.conversation_id {
        Some(id) => println!(
            "Session {} · “{}” · {} messages",
            output::short_id(id),
            session.title,
            session.messages.len()
        ),
        None => println!("New conversation · the first message creates it"),
    }
    println!("Type /help for commands. Ctrl+C stops a reply.");
    println!();
}

fn read_input(prompt: &str) -> Result<Option<String>, String> {
    let mut collected = String::new();
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    loop {
        let mut line = String::new();
        let read = std::io::stdin()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return if collected.is_empty() {
                Ok(None)
            } else {
                Ok(Some(collected))
            };
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if let Some(continuation) = line.strip_suffix('\\') {
            collected.push_str(continuation);
            collected.push('\n');
            print!("… ");
            let _ = std::io::stdout().flush();
            continue;
        }
        collected.push_str(line);
        return Ok(Some(collected));
    }
}

async fn send_prompt(
    ctx: &mut CliContext,
    session: &mut ChatSession,
    prompt: &str,
    silent: bool,
) -> Result<TurnReport, String> {
    let model = agent::resolve_model(&session.model_slug);
    ensure_model_ready(ctx, &model, session.yes).await?;
    ensure_conversation(ctx, session)?;
    let Some(id) = session.conversation_id.clone() else {
        return Err("Could not create a conversation.".to_owned());
    };
    let user = ChatMessage::user(prompt);
    let outcome = ctx
        .store
        .append_message(&id, &user)
        .map_err(|error| format!("could not save your message: {error}"))?;
    if let Some(title) = outcome.title {
        session.title = title;
    }
    session.messages.push(user);
    stream_assistant(ctx, session, &model, silent).await
}

fn ensure_conversation(ctx: &mut CliContext, session: &mut ChatSession) -> Result<(), String> {
    if session.conversation_id.is_some() {
        return Ok(());
    }
    let id = ctx
        .store
        .create_conversation(&session.model_slug)
        .map_err(|error| format!("could not create a conversation: {error}"))?;
    session.conversation_id = Some(id);
    Ok(())
}

async fn stream_assistant(
    ctx: &mut CliContext,
    session: &mut ChatSession,
    model: &ModelSelection,
    silent: bool,
) -> Result<TurnReport, String> {
    let Some(conversation_id) = session.conversation_id.clone() else {
        return Err("No active conversation.".to_owned());
    };
    let presets = ctx.store.presets().map_err(|error| error.to_string())?;
    let memories = ctx
        .store
        .list_memories()
        .map_err(|error| error.to_string())?;
    let request =
        agent::prepare_request(&ctx.settings, model, &session.messages, &presets, &memories);
    let started = streaming::start(
        &ctx.runtime,
        request,
        ctx.settings.agent_mode,
        ctx.settings.tool_approval,
        streaming::DelegateContext::from_settings(&ctx.settings),
    )
    .map_err(|error| format!("could not start the request: {error}"))?;

    let streaming::StartedStream {
        mut events,
        error: mut error_rx,
        mut approvals,
        cancel,
    } = started;
    let mut approvals_open = true;

    let mut message = ChatMessage::assistant(model.slug.clone(), Vec::new(), None);
    let mut report = TurnReport {
        text: String::new(),
        thinking: String::new(),
        usage: None,
        cost: None,
        error: None,
        cancelled: false,
    };

    loop {
        tokio::select! {
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(_) => break,
                };
                match event {
                    StreamEvent::Text(chunk) => {
                        message.append_text(&chunk);
                        report.text.push_str(&chunk);
                    }
                    StreamEvent::Thinking(chunk) => {
                        message.append_thinking(&chunk, model.provider.id());
                        report.thinking.push_str(&chunk);
                    }
                    StreamEvent::Usage(usage) => {
                        if let Some(existing) = &mut report.usage {
                            existing.merge_max(usage);
                        } else {
                            report.usage = Some(usage);
                        }
                        message.usage = report.usage;
                    }
                    StreamEvent::ToolCall(call) => {
                        if !silent {
                            eprintln!(
                                "\n  {} {}",
                                output::cyan("↳"),
                                format!(
                                    "{} {}",
                                    output::bold(&call.name),
                                    output::dim(&output::truncate(
                                        &serde_json::to_string(&call.arguments).unwrap_or_default(),
                                        400
                                    ))
                                )
                            );
                        }
                        message.blocks.push(ContentBlock::ToolUse {
                            id: call.id,
                            name: call.name,
                            arguments: call.arguments,
                        });
                    }
                    StreamEvent::ToolResult { id, name, output } => {
                        if !silent {
                            eprintln!(
                                "  {} {}: {}",
                                output::dim("←"),
                                output::cyan(&name),
                                output::dim(&output::truncate(&output, 400))
                            );
                        }
                        message.blocks.push(ContentBlock::ToolResult {
                            id,
                            output,
                            is_error: false,
                        });
                    }
                    StreamEvent::Done { .. } => {}
                }
            }
            approval = approvals.recv(), if approvals_open => {
                match approval {
                    Ok(request) => {
                        let approved = ask_approval(&request.call);
                        let _ = request.respond.send(approved);
                    }
                    Err(_) => approvals_open = false,
                }
            }
            _ = tokio::signal::ctrl_c() => {
                cancel.cancel();
                report.cancelled = true;
                eprintln!("\nStopping…");
            }
        }
    }

    if !silent {
        if session.show_thinking && !report.thinking.trim().is_empty() {
            eprintln!("{}", output::dim("  reasoning"));
            eprintln!("{}", output::dim(&report.thinking.trim()));
            eprintln!();
        }
        if !report.text.trim().is_empty() {
            print!("{}", output::render_markdown(&report.text));
            if !report.text.ends_with('\n') {
                println!();
            }
        }
    }

    let error = error_rx.recv().await.ok();
    if let Some(error) = error
        && !report.cancelled
    {
        message.error = Some(error.clone());
        report.error = Some(error);
    }
    if let Some(usage) = report.usage {
        report.cost = model.estimate_cost(&usage);
        if !silent {
            let cost = report
                .cost
                .map(|cost| format!(" · ${cost:.4}"))
                .unwrap_or_default();
            eprintln!(
                "{}",
                output::dim(&format!(
                    "[{} · {} in · {} out{}]",
                    model.name,
                    format_tokens(u64::from(usage.input_tokens)),
                    format_tokens(u64::from(usage.output_tokens)),
                    cost
                ))
            );
        }
    }
    if let Some(error) = &report.error {
        eprintln!("{}", output::red(&format!("error: {error}")));
    }

    let has_content = !message.text().trim().is_empty()
        || !message.thinking().trim().is_empty()
        || message.error.is_some();
    if has_content {
        ctx.store
            .append_message(&conversation_id, &message)
            .map_err(|error| format!("could not save the response: {error}"))?;
        session.messages.push(message);
        if let Ok(summary) = ctx.store.conversation(&conversation_id) {
            session.title = summary.title;
        }
    }
    Ok(report)
}

fn ask_approval(call: &relay_core::ToolCall) -> bool {
    eprintln!();
    eprintln!("Tool approval required: {}", call.name);
    eprintln!(
        "{}",
        serde_json::to_string_pretty(&call.arguments)
            .unwrap_or_else(|_| call.arguments.to_string())
    );
    if !std::io::stdin().is_terminal() {
        eprintln!("Denied: no interactive terminal. Use --approval non-destructive or all.");
        return false;
    }
    output::confirm_stderr("Run this tool?", false)
}

// MARK: Slash commands

async fn run_slash(
    ctx: &mut CliContext,
    session: &mut ChatSession,
    input: &str,
) -> Result<Flow, String> {
    let without_slash = input.trim_start_matches('/');
    let (name, rest) = match without_slash.split_once(char::is_whitespace) {
        Some((name, rest)) => (name.to_ascii_lowercase(), rest.trim()),
        None => (without_slash.to_ascii_lowercase(), ""),
    };
    match name.as_str() {
        "help" | "?" => print!("{}", crate::args::chat_help()),
        "quit" | "exit" | "q" => return Ok(Flow::Quit),
        "new" => {
            *session = ChatSession {
                conversation_id: None,
                title: "New chat".to_owned(),
                model_slug: ctx.settings.default_model.clone(),
                messages: Vec::new(),
                yes: session.yes,
                show_thinking: session.show_thinking,
            };
            println!("New conversation · {}", session.model_slug);
        }
        "sessions" => {
            let items = if rest.is_empty() {
                ctx.store
                    .list_conversations()
                    .map_err(|error| error.to_string())?
            } else {
                ctx.store
                    .search_conversations(rest)
                    .map_err(|error| error.to_string())?
            };
            if items.is_empty() {
                println!("No conversations found.");
            } else {
                let rows = items
                    .iter()
                    .enumerate()
                    .map(|(index, summary)| {
                        vec![
                            (index + 1).to_string(),
                            output::short_id(&summary.id),
                            summary.title.clone(),
                            summary.model_label(),
                            summary.message_count.to_string(),
                            output::local_time(summary.updated_at),
                        ]
                    })
                    .collect::<Vec<_>>();
                output::table(&["#", "ID", "Title", "Model", "Msgs", "Updated"], &rows);
                println!("Open with /open <id|#>.");
            }
        }
        "open" => {
            if rest.is_empty() {
                return Err("Usage: /open <id|#>".to_owned());
            }
            let summary = commands::resolve_session(&ctx.store, rest)?;
            let yes = session.yes;
            *session = session_from_summary(ctx, summary, None)?;
            session.yes = yes;
            println!();
            println!(
                "{}  [{}]",
                session.title,
                output::short_id(session.conversation_id.as_deref().unwrap_or(""))
            );
            println!();
            print!(
                "{}",
                render_messages(&session.messages, session.show_thinking)
            );
        }
        "show" => {
            if session.messages.is_empty() {
                println!("No messages in this conversation.");
            } else {
                if rest.eq_ignore_ascii_case("all") {
                    print!(
                        "{}",
                        render_messages(&session.messages, session.show_thinking)
                    );
                } else {
                    let count = rest.parse::<usize>().unwrap_or(10);
                    let start = session.messages.len().saturating_sub(count);
                    print!(
                        "{}",
                        render_messages_from(&session.messages, start, session.show_thinking)
                    );
                }
            }
        }
        "edit" => {
            let (number, text) = rest
                .split_once(char::is_whitespace)
                .ok_or_else(|| "Usage: /edit <message#> <new text>".to_owned())?;
            let index: usize = number
                .parse()
                .map_err(|_| "The message number must be an integer.".to_owned())?;
            if index == 0 || index > session.messages.len() {
                return Err(format!(
                    "Message {index} does not exist; this conversation has {} messages.",
                    session.messages.len()
                ));
            }
            if session.messages[index - 1].role != Role::User {
                return Err(format!("Message {index} is not a user message."));
            }
            if !session.messages[index - 1]
                .blocks
                .iter()
                .any(|block| block.as_text().is_some())
            {
                return Err("That user message has no editable text.".to_owned());
            }
            let Some(id) = session.conversation_id.clone() else {
                return Err("No active conversation.".to_owned());
            };
            let model = agent::resolve_model(&session.model_slug);
            ensure_model_ready(ctx, &model, session.yes).await?;
            let message_id = session.messages[index - 1].id.clone();
            ctx.store
                .update_message_text(&id, &message_id, text)
                .map_err(|error| error.to_string())?;
            ctx.store
                .delete_messages_after(&id, &message_id)
                .map_err(|error| error.to_string())?;
            session.messages.truncate(index);
            session.messages[index - 1].blocks = vec![ContentBlock::text(text)];
            eprintln!("Edited message {index}; later turns removed. Regenerating…");
            let report = stream_assistant(ctx, session, &model, false).await?;
            if let Some(error) = report.error {
                return Err(error);
            }
        }
        "retry" => {
            if session.messages.is_empty() {
                return Err("There is nothing to retry.".to_owned());
            }
            let Some(id) = session.conversation_id.clone() else {
                return Err("No active conversation.".to_owned());
            };
            let Some(position) = session
                .messages
                .iter()
                .rposition(|message| message.role == Role::User)
            else {
                return Err("There is no user message to regenerate from.".to_owned());
            };
            let model = agent::resolve_model(&session.model_slug);
            ensure_model_ready(ctx, &model, session.yes).await?;
            let message_id = session.messages[position].id.clone();
            if position + 1 < session.messages.len() {
                ctx.store
                    .delete_messages_after(&id, &message_id)
                    .map_err(|error| error.to_string())?;
                session.messages.truncate(position + 1);
            }
            eprintln!("Regenerating…");
            let report = stream_assistant(ctx, session, &model, false).await?;
            if let Some(error) = report.error {
                return Err(error);
            }
        }
        "model" => {
            if rest.is_empty() {
                let model = agent::resolve_model(&session.model_slug);
                println!(
                    "{} · {} · {} context · reasoning {}",
                    model.slug,
                    model.provider.label(),
                    output::tokens(model.context_window as u64),
                    ctx.settings.reasoning.label()
                );
                println!("Use /model <slug> or /models.");
            } else {
                if !agent::is_known_model(rest) {
                    return Err(format!("Unknown model: {rest}"));
                }
                session.model_slug = rest.to_owned();
                if let Some(id) = &session.conversation_id {
                    ctx.store
                        .set_conversation_model(id, rest)
                        .map_err(|error| error.to_string())?;
                }
                ctx.settings.default_model = rest.to_owned();
                ctx.save()?;
                println!("Model: {rest}");
            }
        }
        "models" => commands::list_models(false)?,
        "mode" => {
            if rest.is_empty() {
                println!("{}", ctx.settings.interaction_mode);
            } else {
                ctx.settings.interaction_mode = commands::parse_interaction(rest)?;
                ctx.save()?;
                println!("Mode: {}", ctx.settings.interaction_mode);
            }
        }
        "plan" => {
            ctx.settings.agent_mode = AgentMode::Plan;
            ctx.save()?;
            println!("Agent mode: Plan (read-only)");
        }
        "build" => {
            ctx.settings.agent_mode = AgentMode::Build;
            ctx.save()?;
            println!("Agent mode: Build (tools may write, subject to approval)");
        }
        "reasoning" => {
            if rest.is_empty() {
                println!("{}", ctx.settings.reasoning.label());
            } else {
                ctx.settings.reasoning = commands::parse_reasoning(rest)?;
                ctx.save()?;
                println!("Reasoning: {}", ctx.settings.reasoning.label());
            }
        }
        "approval" => {
            if rest.is_empty() {
                println!("{}", ctx.settings.tool_approval.label());
            } else {
                ctx.settings.tool_approval = commands::parse_approval(rest)?;
                ctx.save()?;
                println!("Tool approval: {}", ctx.settings.tool_approval.label());
            }
        }
        "thinking" => {
            if rest.is_empty() {
                println!(
                    "Reasoning display: {}",
                    if session.show_thinking { "on" } else { "off" }
                );
            } else {
                let show = match rest.to_ascii_lowercase().as_str() {
                    "on" | "yes" | "true" | "1" => true,
                    "off" | "no" | "false" | "0" => false,
                    _ => return Err("Usage: /thinking [on|off]".to_owned()),
                };
                session.show_thinking = show;
                ctx.settings.show_thinking = show;
                ctx.save()?;
                println!("Reasoning display: {}", if show { "on" } else { "off" });
            }
        }
        "rename" => {
            if rest.is_empty() {
                return Err("Usage: /rename <new title>".to_owned());
            }
            let Some(id) = session.conversation_id.clone() else {
                return Err("No conversation yet.".to_owned());
            };
            ctx.store
                .rename_conversation(&id, rest)
                .map_err(|error| error.to_string())?;
            session.title = rest.to_owned();
            println!("Renamed to “{rest}”.");
        }
        "pin" | "unpin" => {
            let Some(id) = session.conversation_id.clone() else {
                return Err("No conversation yet.".to_owned());
            };
            let pinned = name == "pin";
            ctx.store
                .set_pinned(&id, pinned)
                .map_err(|error| error.to_string())?;
            println!(
                "{} conversation.",
                if pinned { "Pinned" } else { "Unpinned" }
            );
        }
        "archive" => {
            let Some(id) = session.conversation_id.clone() else {
                return Err("No conversation yet.".to_owned());
            };
            ctx.store
                .set_archived(&id, true)
                .map_err(|error| error.to_string())?;
            println!("Archived. Starting a new conversation.");
            *session = ChatSession {
                conversation_id: None,
                title: "New chat".to_owned(),
                model_slug: ctx.settings.default_model.clone(),
                messages: Vec::new(),
                yes: session.yes,
                show_thinking: session.show_thinking,
            };
        }
        "delete" => {
            let Some(id) = session.conversation_id.clone() else {
                return Err("No conversation yet.".to_owned());
            };
            if !output::confirm("Delete this conversation? This cannot be undone.", false) {
                return Err(output::cancelled());
            }
            ctx.store
                .delete_conversation(&id)
                .map_err(|error| error.to_string())?;
            println!("Deleted. Starting a new conversation.");
            *session = ChatSession {
                conversation_id: None,
                title: "New chat".to_owned(),
                model_slug: ctx.settings.default_model.clone(),
                messages: Vec::new(),
                yes: session.yes,
                show_thinking: session.show_thinking,
            };
        }
        "export" => {
            let Some(id) = session.conversation_id.clone() else {
                return Err("No conversation to export.".to_owned());
            };
            let markdown = ctx
                .store
                .export_markdown(&id)
                .map_err(|error| error.to_string())?;
            let path = if rest.is_empty() {
                format!("relay-{}.md", output::short_id(&id))
            } else {
                rest.to_owned()
            };
            std::fs::write(&path, markdown)
                .map_err(|error| format!("could not write {path}: {error}"))?;
            println!("Exported to {path}.");
        }
        "copy" => {
            if session.messages.is_empty() {
                return Err("No messages to copy.".to_owned());
            }
            let selected = if rest.is_empty() || rest.eq_ignore_ascii_case("last") {
                session
                    .messages
                    .iter()
                    .rev()
                    .find(|message| !message.text().trim().is_empty())
            } else {
                let index: usize = rest
                    .parse()
                    .map_err(|_| "Usage: /copy [message#|last]".to_owned())?;
                if index == 0 || index > session.messages.len() {
                    return Err(format!("Message {index} does not exist."));
                }
                session.messages.get(index - 1)
            };
            let Some(message) = selected else {
                return Err("That message has no text.".to_owned());
            };
            let text = message.text();
            if text.trim().is_empty() {
                return Err("That message has no text.".to_owned());
            }
            output::copy_to_clipboard(&text)?;
            println!("Copied to the clipboard.");
        }
        "settings" => print_settings(ctx),
        "presets" => {
            let presets = ctx.store.presets().map_err(|error| error.to_string())?;
            let rows = presets
                .iter()
                .map(|preset| {
                    vec![
                        if preset.id == ctx.settings.active_preset_id {
                            "*".to_owned()
                        } else {
                            String::new()
                        },
                        preset.id.clone(),
                        preset.name.clone(),
                        preset.description.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            output::table(&["", "ID", "Name", "Description"], &rows);
        }
        "preset" => {
            if rest.is_empty() {
                println!("Active preset: {}", ctx.settings.active_preset_id);
                return Ok(Flow::Continue);
            }
            let presets = ctx.store.presets().map_err(|error| error.to_string())?;
            let exists = presets.iter().any(|preset| preset.id == rest)
                || relay_core::find_builtin_preset(rest).is_some();
            if !exists {
                return Err(format!("Unknown preset: {rest}"));
            }
            ctx.settings.active_preset_id = rest.to_owned();
            ctx.save()?;
            println!("Active preset: {rest}");
        }
        "memory" => {
            let (sub, argument) = match rest.split_once(char::is_whitespace) {
                Some((sub, argument)) => (sub.to_ascii_lowercase(), argument.trim()),
                None => (rest.to_ascii_lowercase(), ""),
            };
            match sub.as_str() {
                "" | "list" => {
                    let memories = ctx
                        .store
                        .list_memories()
                        .map_err(|error| error.to_string())?;
                    if memories.is_empty() {
                        println!("No cross-session memories yet.");
                    } else {
                        for (id, content) in memories {
                            println!("[{}] {}", output::short_id(&id), content);
                        }
                    }
                }
                "add" | "remember" => {
                    if argument.is_empty() {
                        return Err("Usage: /memory add <text>".to_owned());
                    }
                    let id = ctx
                        .store
                        .add_memory(argument)
                        .map_err(|error| error.to_string())?;
                    println!("Remembered [{}].", output::short_id(&id));
                }
                "delete" | "forget" => {
                    if argument.is_empty() {
                        return Err("Usage: /memory delete <id>".to_owned());
                    }
                    let memories = ctx
                        .store
                        .list_memories()
                        .map_err(|error| error.to_string())?;
                    let matches: Vec<_> = memories
                        .into_iter()
                        .filter(|(id, _)| id.starts_with(argument))
                        .collect();
                    if matches.len() != 1 {
                        return Err("Use a unique memory id prefix.".to_owned());
                    }
                    let (id, _) = matches
                        .into_iter()
                        .next()
                        .ok_or_else(|| "No match.".to_owned())?;
                    if !output::confirm("Forget this memory?", false) {
                        return Err(output::cancelled());
                    }
                    ctx.store
                        .delete_memory(&id)
                        .map_err(|error| error.to_string())?;
                    println!("Memory forgotten.");
                }
                "summarize" | "summary" => {
                    summarize_memory(ctx, argument.contains("force")).await?;
                }
                other => return Err(format!("Unknown /memory command: {other}")),
            }
        }
        "tools" => {
            let tools = agent::tools_for(&ctx.settings);
            if tools.is_empty() {
                println!("No tools are available in this mode.");
            } else {
                let rows = tools
                    .iter()
                    .map(|tool| vec![tool.name.clone(), output::truncate(&tool.description, 80)])
                    .collect::<Vec<_>>();
                output::table(&["Tool", "Description"], &rows);
            }
        }
        "web" => {
            if rest.is_empty() {
                return Err("Usage: /web <query>".to_owned());
            }
            print!("{}", search::web_search(rest, 8).await);
        }
        "fetch" => {
            if rest.is_empty() {
                return Err("Usage: /fetch <url>".to_owned());
            }
            println!("{}", search::fetch_url(rest).await);
        }
        "mcp" => {
            let tools = mcp::global_tools();
            if tools.is_empty() {
                println!("No MCP tools are available.");
            } else {
                let rows = tools
                    .iter()
                    .map(|tool| vec![tool.name.clone(), output::truncate(&tool.description, 80)])
                    .collect::<Vec<_>>();
                output::table(&["MCP tool", "Description"], &rows);
            }
        }
        "status" => commands::status(ctx, false).await?,
        "about" => {
            println!(
                "Relay CLI {} · {} · Apache-2.0",
                env!("CARGO_PKG_VERSION"),
                relay_core::PROJECT_URL
            );
        }
        other => return Err(format!("Unknown command: /{other}. Type /help.")),
    }
    Ok(Flow::Continue)
}

fn print_settings(ctx: &CliContext) {
    let model = agent::resolve_model(&ctx.settings.default_model);
    println!(
        "Model:            {} ({})",
        model.name,
        model.provider.label()
    );
    println!("Interaction:      {}", ctx.settings.interaction_mode);
    println!("Agent mode:       {}", ctx.settings.agent_mode.label());
    println!("Tool approval:    {}", ctx.settings.tool_approval.label());
    println!("Reasoning:        {}", ctx.settings.reasoning.label());
    println!("Show thinking:    {}", ctx.settings.show_thinking);
    println!("Preset:           {}", ctx.settings.active_preset_id);
    println!("Temperature:      {:.2}", ctx.settings.temperature);
    println!(
        "Max output:       {}",
        if ctx.settings.max_output_tokens == 0 {
            "model default".to_owned()
        } else {
            ctx.settings.max_output_tokens.to_string()
        }
    );
    println!("Ollama:           {}", ctx.settings.ollama_base_url);
    println!("Ollama model:     {}", ctx.settings.ollama_model);
    println!("Local endpoint:   {}", ctx.settings.local_base_url);
    println!("Local model:      {}", ctx.settings.local_model);
    println!("Bundled variant:  {}", ctx.settings.bundled_model_variant);
    println!("MCP servers:      {}", ctx.settings.mcp_servers.len());
    if !ctx.settings.custom_instructions.trim().is_empty() {
        println!(
            "Instructions:     {}",
            output::truncate(&ctx.settings.custom_instructions, 120)
        );
    }
}

// MARK: Rendering

/// Render every message with 1-based indices.
pub fn render_messages(messages: &[ChatMessage], show_thinking: bool) -> String {
    render_messages_from(messages, 0, show_thinking)
}

/// Render a suffix of messages while keeping absolute 1-based indices.
fn render_messages_from(messages: &[ChatMessage], start: usize, show_thinking: bool) -> String {
    let mut out = String::new();
    for (offset, message) in messages.iter().enumerate().skip(start) {
        let index = offset + 1;
        let header = match message.role {
            Role::User => "You".to_owned(),
            Role::Assistant => format!(
                "Relay · {}",
                message.model.as_deref().unwrap_or("assistant")
            ),
            Role::System => "System".to_owned(),
            Role::Tool => "Tool".to_owned(),
        };
        out.push_str(&format!(
            "[{index}] {header} · {}\n",
            output::local_time(message.created_at)
        ));
        if show_thinking {
            let thinking = message.thinking();
            if !thinking.trim().is_empty() {
                out.push_str(&format!("{}\n", output::dim("reasoning:")));
                for line in thinking.trim().lines() {
                    out.push_str("  ");
                    out.push_str(&output::dim(line));
                    out.push('\n');
                }
            }
        }
        let text = message.text();
        if !text.trim().is_empty() {
            if message.role == Role::Assistant {
                out.push_str(&output::render_markdown(text.trim()));
            } else {
                out.push_str(text.trim());
                out.push('\n');
            }
        }
        for block in &message.blocks {
            match block {
                ContentBlock::ToolUse {
                    name, arguments, ..
                } => {
                    out.push_str(&format!(
                        "  ↳ {name}({})\n",
                        output::truncate(
                            &serde_json::to_string(arguments).unwrap_or_default(),
                            400
                        )
                    ));
                }
                ContentBlock::ToolResult { output, .. } => {
                    out.push_str(&format!("  ← {}\n", output::truncate(output, 400)));
                }
                _ => {}
            }
        }
        if let Some(error) = &message.error {
            out.push_str(&format!("  ! {error}\n"));
        }
        out.push('\n');
    }
    out
}

// MARK: Local model readiness

/// Ensure a selected model can actually serve a request.
///
/// Bundled downloads and `ollama pull` are strictly opt-in. A missing cloud key
/// is reported before a request is attempted, matching the desktop's preflight.
pub async fn ensure_model_ready(
    ctx: &mut CliContext,
    model: &ModelSelection,
    assume_yes: bool,
) -> Result<(), String> {
    if let Some(variant) = bundled::BundledVariant::from_model_slug(&model.slug) {
        let running = bundled::probe_running().await;
        let need_download = !bundled::downloaded(variant);
        let need_server = bundled::llama_server_path().is_none();
        if !running && (need_download || need_server) {
            if need_download {
                eprintln!("Bundled model not downloaded: {}", variant.display_name());
                eprintln!("  {}", variant.gguf_url());
            }
            if need_server {
                eprintln!("A llama.cpp llama-server build will also be downloaded.");
            }
            if !output::confirm_stderr(
                "Download the bundled model now? This is a large network transfer.",
                assume_yes,
            ) {
                return Err(output::cancelled());
            }
        }
        prepare_bundled(ctx, variant, ctx.settings.bundled_context_size).await?;
        return Ok(());
    }

    if model.provider == Provider::Ollama {
        commands::ensure_ollama(ctx).await?;
        let tag = model.api_id.as_str();
        let installed = local::list_models_api(&ctx.settings.ollama_base_url)
            .await
            .unwrap_or_else(|_| local::installed_ollama_models());
        if !installed.iter().any(|name| same_ollama_model(name, tag)) {
            let label = local::curated(tag)
                .map(|model| format!("{} ({})", model.name, model.license))
                .unwrap_or_else(|| tag.to_owned());
            if !output::confirm_stderr(
                &format!("Ollama model {label} is not installed. Download it now?"),
                assume_yes,
            ) {
                return Err(output::cancelled());
            }
            local::pull(tag)?;
        }
        if ctx.settings.ollama_model != tag {
            ctx.settings.ollama_model = tag.to_owned();
            ctx.save()?;
        }
        return Ok(());
    }

    if model.provider == Provider::OpenAiCompatible {
        if ctx.settings.local_model != model.api_id {
            ctx.settings.local_model = model.api_id.clone();
            ctx.save()?;
        }
        return Ok(());
    }

    if model.provider.requires_api_key() && keys::resolve(model.provider).is_none() {
        return Err(format!(
            "No {} API key is configured. Run `relay-cli keys set {}` or set {}.",
            model.provider.label(),
            model.provider.id(),
            model.provider.env_keys().join(" or ")
        ));
    }
    Ok(())
}

fn same_ollama_model(name: &str, tag: &str) -> bool {
    name == tag
        || name
            .strip_prefix(tag)
            .is_some_and(|rest| rest.starts_with(':'))
        || (!tag.contains(':') && name == format!("{tag}:latest").as_str())
}

/// Start (or reuse) the bundled llama-server and point settings at it.
pub async fn prepare_bundled(
    ctx: &mut CliContext,
    variant: bundled::BundledVariant,
    context_size: u32,
) -> Result<(), String> {
    if bundled::probe_running().await {
        apply_bundled_settings(ctx, variant)?;
        return Ok(());
    }

    let watch = ctx.bundled.clone();
    let progress = tokio::spawn(async move {
        let mut last = String::new();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let status = match watch.lock().map(|state| state.status()) {
                Ok(status) => status,
                Err(_) => break,
            };
            if status.ready || status.error.is_some() {
                break;
            }
            let message = match status.progress {
                Some(progress) => format!("{} ({}%)", status.message, progress),
                None => status.message.clone(),
            };
            if !message.is_empty() && message != last {
                eprintln!("{message}");
                last = message;
            }
        }
    });

    bundled::ensure_ready(ctx.bundled.clone(), variant, context_size).await;
    progress.abort();

    let status = ctx
        .bundled
        .lock()
        .map(|state| state.status())
        .unwrap_or_default();
    if !status.ready {
        return Err(status
            .error
            .or_else(|| (!status.message.is_empty()).then_some(status.message))
            .unwrap_or_else(|| "the bundled model did not become ready".to_owned()));
    }
    apply_bundled_settings(ctx, variant)?;
    Ok(())
}

fn apply_bundled_settings(
    ctx: &mut CliContext,
    variant: bundled::BundledVariant,
) -> Result<(), String> {
    ctx.settings.local_base_url = bundled::endpoint();
    ctx.settings.local_model = variant.id().to_owned();
    ctx.settings.bundled_model_variant = variant.id().to_owned();
    ctx.save()
}

// MARK: Memory summaries

/// Run a cross-session summary now or only when one is due.
pub async fn summarize_memory(ctx: &mut CliContext, force: bool) -> Result<(), String> {
    let mut bundled_endpoint = if bundled::probe_running().await {
        Some(bundled::endpoint())
    } else {
        None
    };

    let variant = bundled::BundledVariant::from_id(&ctx.settings.bundled_model_variant)
        .or_else(|| bundled::BundledVariant::from_model_slug(&ctx.settings.local_model))
        .unwrap_or(bundled::DEFAULT_VARIANT);
    let wants_bundled = bundled::BundledVariant::from_model_slug(&ctx.settings.default_model)
        .is_some()
        || bundled::BundledVariant::from_id(&ctx.settings.local_model).is_some();

    if bundled_endpoint.is_none() && wants_bundled && bundled::downloaded(variant) {
        prepare_bundled(ctx, variant, ctx.settings.bundled_context_size).await?;
        bundled_endpoint = Some(bundled::endpoint());
    }

    if bundled_endpoint.is_none()
        && ctx.settings.local_model.trim().is_empty()
        && !ctx.settings.ollama_model.trim().is_empty()
    {
        commands::ensure_ollama(ctx).await?;
    }

    let Some((endpoint, model)) =
        memory::configured_endpoint(&ctx.settings, bundled_endpoint.as_deref())
    else {
        return Err(
            "No local model is configured for summaries. Select an Ollama model or run `relay-cli bundled prepare`."
                .to_owned(),
        );
    };
    let summary = memory::maybe_summarize(&ctx.store, Some(&endpoint), &model, force).await?;
    match summary {
        Some(summary) => println!("{summary}"),
        None if force => println!("No conversation history to summarize."),
        None => println!("No summary is due yet (the interval is 10 hours)."),
    }
    Ok(())
}

fn spawn_auto_summary(ctx: &CliContext) {
    if !memory::due(&ctx.settings) {
        return;
    }
    let bundled_endpoint = ctx.bundled.lock().ok().and_then(|state| {
        let status = state.status();
        status.ready.then_some(status.endpoint).flatten()
    });
    let Some((endpoint, model)) =
        memory::configured_endpoint(&ctx.settings, bundled_endpoint.as_deref())
    else {
        return;
    };
    let store = ctx.store.clone();
    ctx.runtime.spawn(async move {
        let _ = memory::maybe_summarize(&store, Some(&endpoint), &model, false).await;
    });
}
