//! Management commands: sessions, models, settings, keys, presets, memory,
//! MCP, local runtimes, web access, and diagnostics.
//!
//! Every command reads and writes the same encrypted database and the same
//! engine helpers as the desktop app. Nothing here has a desktop-only shortcut.

use std::path::PathBuf;
use std::process::Child;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex};

use relay_core::{
    AgentMode, AppSettings, ColorPreset, ConversationSummary, McpServerConfig, PersonalityPreset,
    Provider, ReasoningEffort, ReasoningKind, ToolApproval, format_tokens_exact, models,
};
use relay_engine::{agent, bundled, keys, local, mcp, search};
use relay_store::Store;
use serde_json::{Value, json};

use crate::args::{
    BundledCommand, Command, KeysCommand, McpCommand, MemoryCommand, OllamaCommand, PresetsCommand,
    SessionsCommand, SettingsCommand,
};
use crate::chat;
use crate::output;

/// Shared state for one CLI invocation.
pub struct CliContext {
    /// Encrypted store opened with the same master key as the desktop app.
    pub store: Store,
    /// Current preferences. Commands persist through [`Self::save`].
    pub settings: AppSettings,
    /// Tokio handle used by the provider/agent bridge.
    pub runtime: tokio::runtime::Handle,
    /// Bundled llama-server process, kept alive for the whole invocation.
    pub bundled: Arc<Mutex<bundled::BundledState>>,
    /// Ollama daemon started by this CLI, stopped on exit.
    pub ollama_child: Option<Child>,
}

impl CliContext {
    /// Persist settings and reload the normalized copy.
    pub fn save(&mut self) -> Result<(), String> {
        self.store
            .save_settings(&self.settings)
            .map_err(|error| error.to_string())?;
        self.settings = self
            .store
            .load_settings()
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Reload settings changed by another Relay process.
    #[allow(dead_code)]
    pub fn reload(&mut self) -> Result<(), String> {
        self.settings = self
            .store
            .load_settings()
            .map_err(|error| error.to_string())?;
        Ok(())
    }

    /// Restart MCP servers from the current settings.
    pub fn reload_mcp(&self) {
        mcp::init_global(&self.settings.mcp_servers);
    }

    /// Stop processes this CLI started. A server that was already running is
    /// left alone because it belongs to the desktop app or the user.
    pub fn shutdown(&mut self) {
        bundled::stop(&self.bundled);
        if let Some(mut child) = self.ollama_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Dispatch one parsed command.
pub async fn dispatch(command: Command, ctx: &mut CliContext) -> Result<(), String> {
    match command {
        Command::Chat(options) => chat::run_chat(ctx, options).await,
        Command::Prompt(options, prompt) => chat::run_prompt(ctx, options, prompt).await,
        Command::Sessions(command) => sessions(ctx, command),
        Command::Models { json } => list_models(&ctx.settings, json),
        Command::Model { slug } => model(ctx, slug),
        Command::Settings(command) => settings(ctx, command),
        Command::Keys(command) => provider_keys(command),
        Command::Presets(command) => presets(ctx, command),
        Command::Memory(command) => memory_command(ctx, command).await,
        Command::Mcp(command) => mcp_command(ctx, command),
        Command::Bundled(command) => bundled_command(ctx, command).await,
        Command::Ollama(command) => ollama_command(ctx, command).await,
        Command::Web { query, json } => web(&query, json).await,
        Command::Fetch { url } => {
            println!("{}", search::fetch_url(&url).await);
            Ok(())
        }
        Command::Shell => shell(),
        Command::Completions { shell } => {
            print!("{}", completions(&shell));
            Ok(())
        }
        Command::Clear => {
            output::clear_screen();
            Ok(())
        }
        Command::Status { json } => status(ctx, json).await,
        Command::About { json } => about(json),
        Command::Help(topic) => {
            print!("{}", crate::args::help_for(topic.as_deref().unwrap_or("")));
            Ok(())
        }
        Command::Version => {
            println!("relay-cli {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

// MARK: Sessions

fn sessions(ctx: &mut CliContext, command: SessionsCommand) -> Result<(), String> {
    match command {
        SessionsCommand::List {
            archived,
            json,
            limit,
        } => {
            let mut items = if archived {
                ctx.store
                    .list_archived_conversations()
                    .map_err(|error| error.to_string())?
            } else {
                ctx.store
                    .list_conversations()
                    .map_err(|error| error.to_string())?
            };
            if let Some(limit) = limit {
                items.truncate(limit);
            }
            if json {
                return output::json(&items);
            }
            if items.is_empty() {
                println!(
                    "No {}conversations.",
                    if archived { "archived " } else { "" }
                );
                return Ok(());
            }
            let rows = items
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    vec![
                        (index + 1).to_string(),
                        output::short_id(&item.id),
                        item.title.clone(),
                        item.model_label(),
                        item.message_count.to_string(),
                        output::local_time(item.updated_at),
                        flags(item),
                    ]
                })
                .collect::<Vec<_>>();
            output::table(
                &["#", "ID", "Title", "Model", "Msgs", "Updated", "Flags"],
                &rows,
            );
            Ok(())
        }
        SessionsCommand::Search { query, json } => {
            let items = ctx
                .store
                .search_conversations(&query)
                .map_err(|error| error.to_string())?;
            if json {
                return output::json(&items);
            }
            if items.is_empty() {
                println!("No conversations matched “{query}”.");
                return Ok(());
            }
            let rows = items
                .iter()
                .map(|item| {
                    vec![
                        output::short_id(&item.id),
                        item.title.clone(),
                        item.model_label(),
                        item.message_count.to_string(),
                        output::local_time(item.updated_at),
                    ]
                })
                .collect::<Vec<_>>();
            output::table(&["ID", "Title", "Model", "Msgs", "Updated"], &rows);
            Ok(())
        }
        SessionsCommand::Show { id, json, thinking } => {
            let summary = resolve_session(&ctx.store, &id)?;
            let messages = ctx
                .store
                .messages(&summary.id)
                .map_err(|error| error.to_string())?;
            if json {
                return output::json(&json!({
                    "conversation": summary,
                    "messages": messages,
                }));
            }
            println!("{}  [{}]", summary.title, output::short_id(&summary.id));
            println!(
                "{} · {} messages · updated {}",
                summary.model_label(),
                summary.message_count,
                output::local_time(summary.updated_at)
            );
            println!();
            print!(
                "{}",
                chat::render_messages(&messages, thinking, &ctx.settings)
            );
            Ok(())
        }
        SessionsCommand::New { model } => {
            let slug = model.unwrap_or_else(|| ctx.settings.default_model.clone());
            if !agent::is_known_model(&slug) {
                return Err(format!("Unknown model: {slug}"));
            }
            let id = ctx
                .store
                .create_conversation(&slug)
                .map_err(|error| error.to_string())?;
            ctx.settings.default_model = slug.clone();
            ctx.save()?;
            println!("{id}");
            eprintln!("New conversation · {slug}");
            Ok(())
        }
        SessionsCommand::Rename { id, title } => {
            let summary = resolve_session(&ctx.store, &id)?;
            ctx.store
                .rename_conversation(&summary.id, &title)
                .map_err(|error| error.to_string())?;
            println!(
                "Renamed {} to “{}”.",
                output::short_id(&summary.id),
                title.trim()
            );
            Ok(())
        }
        SessionsCommand::Pin { id, pinned } => {
            let summary = resolve_session(&ctx.store, &id)?;
            ctx.store
                .set_pinned(&summary.id, pinned)
                .map_err(|error| error.to_string())?;
            println!(
                "{} {}.",
                if pinned { "Pinned" } else { "Unpinned" },
                output::short_id(&summary.id)
            );
            Ok(())
        }
        SessionsCommand::Archive { id, archived } => {
            let summary = resolve_session(&ctx.store, &id)?;
            ctx.store
                .set_archived(&summary.id, archived)
                .map_err(|error| error.to_string())?;
            println!(
                "{} {}.",
                if archived { "Archived" } else { "Unarchived" },
                output::short_id(&summary.id)
            );
            Ok(())
        }
        SessionsCommand::Model { id, slug } => {
            if !agent::is_known_model(&slug) {
                return Err(format!("Unknown model: {slug}"));
            }
            let summary = resolve_session(&ctx.store, &id)?;
            ctx.store
                .set_conversation_model(&summary.id, &slug)
                .map_err(|error| error.to_string())?;
            ctx.settings.default_model = slug.clone();
            ctx.save()?;
            println!("{} now uses {slug}.", output::short_id(&summary.id));
            Ok(())
        }
        SessionsCommand::Delete { ids, all, yes } => {
            if all {
                let count = ctx
                    .store
                    .list_conversations()
                    .map_err(|error| error.to_string())?
                    .len();
                let archived = ctx
                    .store
                    .list_archived_conversations()
                    .map_err(|error| error.to_string())?
                    .len();
                let total = count + archived;
                if !output::confirm(
                    &format!("Delete all {total} conversations? This cannot be undone."),
                    yes,
                ) {
                    return Err(output::cancelled());
                }
                ctx.store
                    .delete_all_conversations()
                    .map_err(|error| error.to_string())?;
                println!("Deleted {total} conversations.");
                return Ok(());
            }
            let mut resolved = Vec::new();
            for id in &ids {
                resolved.push(resolve_session(&ctx.store, id)?);
            }
            if !output::confirm(
                &format!(
                    "Delete {} conversation{}? This cannot be undone.",
                    resolved.len(),
                    if resolved.len() == 1 { "" } else { "s" }
                ),
                yes,
            ) {
                return Err(output::cancelled());
            }
            for summary in &resolved {
                ctx.store
                    .delete_conversation(&summary.id)
                    .map_err(|error| error.to_string())?;
                println!(
                    "Deleted “{}” ({}).",
                    summary.title,
                    output::short_id(&summary.id)
                );
            }
            Ok(())
        }
        SessionsCommand::Export {
            id,
            all,
            output: path,
            copy,
        } => {
            let (label, text) = if all {
                (
                    "all conversations".to_owned(),
                    ctx.store
                        .export_all_markdown()
                        .map_err(|error| error.to_string())?,
                )
            } else {
                let summary = match id {
                    Some(id) => resolve_session(&ctx.store, &id)?,
                    None => ctx
                        .store
                        .latest_conversation()
                        .map_err(|error| error.to_string())?
                        .ok_or_else(|| "No conversations to export.".to_owned())?,
                };
                (
                    summary.title.clone(),
                    ctx.store
                        .export_markdown(&summary.id)
                        .map_err(|error| error.to_string())?,
                )
            };

            let mut acted = false;
            if let Some(path) = path {
                write_export(&path, &text)?;
                eprintln!("Exported {label} to {}", path.display());
                acted = true;
            }
            if copy {
                output::copy_to_clipboard(&text)?;
                eprintln!("Copied {label} to the clipboard.");
                acted = true;
            }
            if !acted {
                print!("{text}");
            }
            Ok(())
        }
    }
}

fn flags(summary: &ConversationSummary) -> String {
    let mut parts = Vec::new();
    if summary.pinned {
        parts.push("pinned");
    }
    if summary.archived {
        parts.push("archived");
    }
    parts.join(",")
}

fn write_export(path: &PathBuf, text: &str) -> Result<(), String> {
    std::fs::write(path, text)
        .map_err(|error| format!("could not write {}: {error}", path.display()))
}

/// Resolve a full id, unique prefix, `last`, or a 1-based list index.
pub fn resolve_session(store: &Store, input: &str) -> Result<ConversationSummary, String> {
    let input = input.trim();
    if input.is_empty()
        || input.eq_ignore_ascii_case("last")
        || input.eq_ignore_ascii_case("latest")
    {
        return store
            .latest_conversation()
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "No conversations yet.".to_owned());
    }

    if let Ok(summary) = store.conversation(input) {
        return Ok(summary);
    }

    let mut all = store
        .list_conversations()
        .map_err(|error| error.to_string())?;
    all.extend(
        store
            .list_archived_conversations()
            .map_err(|error| error.to_string())?,
    );

    let mut matches: Vec<ConversationSummary> = all
        .iter()
        .filter(|summary| summary.id.starts_with(input))
        .cloned()
        .collect();
    match matches.len() {
        1 => return Ok(matches.remove(0)),
        0 => {}
        _ => {
            let choices = matches
                .iter()
                .take(5)
                .map(|summary| format!("{} ({})", output::short_id(&summary.id), summary.title))
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "“{input}” matches {} conversations: {choices}",
                matches.len()
            ));
        }
    }

    if let Ok(index) = input.parse::<usize>()
        && index > 0
        && let Some(summary) = store
            .list_conversations()
            .map_err(|error| error.to_string())?
            .get(index - 1)
    {
        return Ok(summary.clone());
    }

    Err(format!("No conversation matches “{input}”."))
}

// MARK: Models

pub fn list_models(settings: &AppSettings, json: bool) -> Result<(), String> {
    let installed = local::installed_ollama_models();
    if json {
        let cloud = models()
            .iter()
            .map(|spec| {
                json!({
                    "slug": spec.slug,
                    "name": spec.name,
                    "provider": spec.provider.id(),
                    "provider_label": spec.provider.label(),
                    "tagline": spec.tagline,
                    "context_window": spec.context_window,
                    "max_output": spec.max_output,
                    "reasoning": spec.reasoning,
                })
            })
            .collect::<Vec<_>>();
        let bundled_models = bundled::BundledVariant::all()
            .iter()
            .map(|variant| {
                json!({
                    "slug": format!("local/{}", variant.id()),
                    "name": variant.display_name(),
                    "downloaded": bundled::downloaded(*variant),
                    "tagline": variant.tagline(),
                    "context_window": settings.bundled_context_size.max(512),
                    "reasoning": ReasoningKind::Toggle,
                })
            })
            .collect::<Vec<_>>();
        let curated = local::CURATED
            .iter()
            .map(|model| {
                json!({
                    "slug": format!("ollama/{}", model.tag),
                    "tag": model.tag,
                    "name": model.name,
                    "license": model.license,
                    "installed": local::installed(model.tag),
                    "reasoning": ReasoningKind::Toggle,
                })
            })
            .collect::<Vec<_>>();
        return output::json(&json!({
            "cloud": cloud,
            "bundled": bundled_models,
            "local": {
                "base_url": settings.local_base_url,
                "model": settings.local_model,
            },
            "ollama": {
                "base_url": settings.ollama_base_url,
                "model": settings.ollama_model,
                "installed": installed,
                "curated": curated,
            },
        }));
    }

    output::heading("Cloud models");
    for provider in Provider::CLOUD {
        println!("{}", provider.label());
        for spec in models().iter().filter(|spec| spec.provider == provider) {
            println!(
                "  {:<26} {:<26} {:>9} ctx · {}",
                spec.slug,
                spec.name,
                relay_core::format_tokens_exact(u64::from(spec.context_window)),
                reasoning_kind_label(spec.reasoning)
            );
            println!("  {:<26} {}", "", output::truncate(spec.tagline, 90));
        }
    }

    output::heading("Bundled local models (strictly opt-in downloads)");
    for variant in bundled::BundledVariant::all() {
        println!(
            "  {:<46} {} · {} · {} ctx",
            format!("local/{}", variant.id()),
            if bundled::downloaded(variant) {
                "downloaded"
            } else {
                "not downloaded"
            },
            variant.display_name(),
            relay_core::format_tokens_exact(u64::from(settings.bundled_context_size.max(512)))
        );
    }

    output::heading("OpenAI-compatible endpoint");
    let local_model = settings.local_model.trim();
    let local_is_bundled = !local_model.is_empty()
        && (bundled::BundledVariant::from_id(local_model).is_some()
            || bundled::BundledVariant::from_model_slug(&format!("local/{local_model}")).is_some());
    if local_model.is_empty() || local_is_bundled {
        println!("  No local model configured (settings set local_model <name>).");
    } else {
        println!(
            "  local/{} · {} · {} context · reasoning not supported",
            local_model,
            settings.local_base_url,
            format_tokens_exact(u64::from(
                relay_core::ModelSelection::openai_compatible(local_model).context_window
            ))
        );
    }

    output::heading("Ollama models");
    let configured_ollama = settings.ollama_model.trim();
    if configured_ollama.is_empty() && installed.is_empty() {
        println!("  No installed models detected (Ollama may be stopped).");
    } else {
        if !configured_ollama.is_empty() && !installed.iter().any(|name| name == configured_ollama)
        {
            println!("  ollama/{configured_ollama} · configured · on/off reasoning");
        }
        for name in &installed {
            println!("  ollama/{name} · on/off reasoning");
        }
    }
    let mut installable = Vec::new();
    for model in local::CURATED {
        if !local::installed(model.tag) {
            installable.push(format!(
                "  ollama/{:<24} {} · {}",
                model.tag, model.name, model.license
            ));
        }
    }
    if !installable.is_empty() {
        println!("  Installable (needs download):");
        for line in installable {
            println!("{line}");
        }
    }
    Ok(())
}

fn model(ctx: &mut CliContext, slug: Option<String>) -> Result<(), String> {
    if let Some(slug) = slug {
        if !agent::is_known_model(&slug) {
            return Err(format!("Unknown model: {slug}"));
        }
        ctx.settings.default_model = slug.clone();
        ctx.save()?;
        println!("Default model: {slug}");
        return Ok(());
    }
    let current = agent::resolve_model(&ctx.settings.default_model);
    println!("{}", current.slug);
    eprintln!(
        "{} · {} · {} context · model max output {}",
        current.name,
        current.provider.label(),
        relay_core::format_tokens_exact(u64::from(agent::context_window(&ctx.settings, &current))),
        format_tokens_exact(u64::from(current.max_output))
    );
    eprintln!(
        "reasoning {} ({}) · temperature {:.2} · output cap {} · preset {}",
        agent::effective_reasoning(&ctx.settings, &current).label(),
        reasoning_kind_label(current.reasoning),
        ctx.settings.temperature,
        output::token_limit(ctx.settings.max_output_tokens),
        ctx.settings.active_preset_id
    );
    Ok(())
}

// MARK: Settings

fn settings(ctx: &mut CliContext, command: SettingsCommand) -> Result<(), String> {
    match command {
        SettingsCommand::Show { json } => {
            let value = serde_json::to_value(&ctx.settings).map_err(|error| error.to_string())?;
            if json {
                return output::json(&value);
            }
            output::heading("Relay settings");
            if let Some(object) = value.as_object() {
                for (key, value) in object {
                    println!("{:<28} {}", key, format_setting(value));
                }
            }
            println!();
            println!("Database: {}", ctx.store.path().display());
            Ok(())
        }
        SettingsCommand::Get { key } => {
            let key = canonical_setting_key(&key);
            let value = serde_json::to_value(&ctx.settings).map_err(|error| error.to_string())?;
            let value = value
                .get(key)
                .ok_or_else(|| format!("Unknown setting: {key}"))?;
            match value {
                Value::String(text) => println!("{text}"),
                other => println!("{}", output::json_compact(other)),
            }
            Ok(())
        }
        SettingsCommand::Set { key, value } => set_setting(ctx, &key, &value),
        SettingsCommand::Reset { yes } => {
            if !output::confirm(
                "Reset every preference to its default? Conversations, keys, and presets are kept.",
                yes,
            ) {
                return Err(output::cancelled());
            }
            ctx.settings = AppSettings::default();
            ctx.save()?;
            ctx.reload_mcp();
            println!("Settings reset to defaults.");
            Ok(())
        }
        SettingsCommand::Path => {
            println!("data:     {}", data_dir()?.display());
            println!("database: {}", ctx.store.path().display());
            Ok(())
        }
    }
}

fn data_dir() -> Result<PathBuf, String> {
    relay_store::default_data_dir().map_err(|error| error.to_string())
}

fn format_setting(value: &Value) -> String {
    match value {
        Value::String(text) => {
            if !text.is_empty() && !text.chars().any(char::is_whitespace) && text.len() <= 80 {
                text.clone()
            } else {
                output::json_compact(text)
            }
        }
        other => output::json_compact(other),
    }
}

/// Canonical setting name, accepting the friendly aliases used in chat flags.
fn canonical_setting_key(key: &str) -> &str {
    let key = key.trim();
    match key {
        "max_output" | "max-output" | "max_tokens" | "max-tokens" => "max_output_tokens",
        "reasoning_effort" | "reasoning-effort" => "reasoning",
        "instructions" | "custom-instructions" => "custom_instructions",
        "preset" | "personality" => "active_preset_id",
        "ollama_url" => "ollama_base_url",
        "local_url" => "local_base_url",
        "thinking" => "show_thinking",
        _ => key,
    }
}

fn set_setting(ctx: &mut CliContext, key: &str, value: &str) -> Result<(), String> {
    let key = canonical_setting_key(key);
    let value = value.trim();
    match key {
        "default_model" => {
            if !agent::is_known_model(value) {
                return Err(format!("Unknown model: {value}"));
            }
            ctx.settings.default_model = value.to_owned();
        }
        "appearance" => ctx.settings.appearance = parse_appearance(value)?,
        "agent_mode" => ctx.settings.agent_mode = parse_agent_mode(value)?,
        "tool_approval" => ctx.settings.tool_approval = parse_approval(value)?,
        "interaction_mode" => ctx.settings.interaction_mode = parse_interaction(value)?,
        "reasoning" => ctx.settings.reasoning = parse_reasoning(value)?,
        "temperature" => ctx.settings.temperature = parse_temperature(value)?,
        "max_output_tokens" => {
            ctx.settings.max_output_tokens = parse_max_output_tokens(value)?;
        }
        "custom_instructions" => ctx.settings.custom_instructions = value.to_owned(),
        "active_preset_id" => {
            if !preset_exists(ctx, value)? {
                return Err(format!("Unknown preset: {value}"));
            }
            ctx.settings.active_preset_id = value.to_owned();
        }
        "show_thinking" => ctx.settings.show_thinking = parse_bool(value)?,
        "send_on_enter" => ctx.settings.send_on_enter = parse_bool(value)?,
        "ollama_base_url" => ctx.settings.ollama_base_url = value.to_owned(),
        "ollama_model" => ctx.settings.ollama_model = value.to_owned(),
        "local_base_url" => ctx.settings.local_base_url = value.to_owned(),
        "local_model" => ctx.settings.local_model = value.to_owned(),
        "bundled_model_variant" => {
            let variant = bundled::BundledVariant::from_id(value)
                .ok_or_else(|| "bundled_model_variant must be standard or uncensored".to_owned())?;
            ctx.settings.bundled_model_variant = variant.id().to_owned();
        }
        "bundled_context_size" => {
            ctx.settings.bundled_context_size = value
                .parse()
                .map_err(|_| "bundled_context_size must be an integer".to_owned())?;
        }
        "mcp_servers" => {
            let configs: Vec<McpServerConfig> = serde_json::from_str(value)
                .map_err(|error| format!("mcp_servers must be a JSON array: {error}"))?;
            validate_mcp_configs(&configs)?;
            ctx.settings.mcp_servers = configs;
        }
        "font_family" => ctx.settings.font_family = value.to_owned(),
        "mono_font_family" => ctx.settings.mono_font_family = value.to_owned(),
        "font_size" => {
            ctx.settings.font_size = value
                .parse()
                .map_err(|_| "font_size must be a number".to_owned())?;
        }
        "color_preset" => {
            let parsed = ColorPreset::from_str(value);
            if parsed == ColorPreset::Default
                && !value.eq_ignore_ascii_case("default")
                && !value.eq_ignore_ascii_case("system")
            {
                return Err(format!(
                    "Unknown color preset: {value} (default, ocean, forest, rose, amber, violet, graphite)"
                ));
            }
            ctx.settings.color_preset = parsed;
        }
        "onboarding_complete" => ctx.settings.onboarding_complete = parse_bool(value)?,
        "last_memory_summary_at" => {
            ctx.settings.last_memory_summary_at = value
                .parse()
                .map_err(|_| "last_memory_summary_at must be a timestamp".to_owned())?;
        }
        other => {
            return Err(format!(
                "Unknown setting: {other}\n\n{}",
                crate::args::settings_help()
            ));
        }
    }
    ctx.save()?;
    if key == "mcp_servers" {
        ctx.reload_mcp();
    }
    if key == "temperature" {
        println!("temperature = {:.2}", ctx.settings.temperature);
    } else {
        let snapshot = serde_json::to_value(&ctx.settings).map_err(|error| error.to_string())?;
        let shown = snapshot
            .get(key)
            .map(format_setting)
            .unwrap_or_else(|| format_setting(&Value::String(value.to_owned())));
        println!("{key} = {shown}");
    }
    Ok(())
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err("value must be true or false".to_owned()),
    }
}

/// Parse and validate a sampling temperature.
pub fn parse_temperature(value: &str) -> Result<f64, String> {
    let parsed: f64 = value
        .trim()
        .parse()
        .map_err(|_| "temperature must be a number between 0 and 2".to_owned())?;
    if !parsed.is_finite() || !(0.0..=2.0).contains(&parsed) {
        return Err("temperature must be between 0 and 2".to_owned());
    }
    Ok(parsed)
}

/// Parse and validate an output cap. Zero means the model default.
pub fn parse_max_output_tokens(value: &str) -> Result<u32, String> {
    let parsed: u32 = value
        .trim()
        .parse()
        .map_err(|_| "max_output_tokens must be an integer".to_owned())?;
    if parsed > 1_000_000 {
        return Err("max_output_tokens must be 1,000,000 or less".to_owned());
    }
    Ok(parsed)
}

/// Human-readable reasoning capability for a model.
pub fn reasoning_kind_label(kind: ReasoningKind) -> &'static str {
    match kind {
        ReasoningKind::None => "not supported",
        ReasoningKind::Effort => "effort levels",
        ReasoningKind::Toggle => "on/off switch",
    }
}

/// Whether a preset id names a stored or built-in personality.
pub fn preset_exists(ctx: &CliContext, id: &str) -> Result<bool, String> {
    if relay_core::find_builtin_preset(id).is_some() {
        return Ok(true);
    }
    Ok(ctx
        .store
        .presets()
        .map_err(|error| error.to_string())?
        .iter()
        .any(|preset| preset.id == id))
}

fn parse_appearance(value: &str) -> Result<relay_core::Appearance, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "system" | "auto" => Ok(relay_core::Appearance::System),
        "light" => Ok(relay_core::Appearance::Light),
        "dark" => Ok(relay_core::Appearance::Dark),
        _ => Err("appearance must be system, light, or dark".to_owned()),
    }
}

fn parse_agent_mode(value: &str) -> Result<AgentMode, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "plan" => Ok(AgentMode::Plan),
        "build" => Ok(AgentMode::Build),
        _ => Err("agent_mode must be plan or build".to_owned()),
    }
}

pub fn parse_approval(value: &str) -> Result<ToolApproval, String> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "none" | "ask" => Ok(ToolApproval::None),
        "non-destructive" | "nondestructive" | "safe" => Ok(ToolApproval::NonDestructive),
        "all" | "auto" => Ok(ToolApproval::All),
        _ => Err("tool_approval must be none, non-destructive, or all".to_owned()),
    }
}

pub fn parse_interaction(value: &str) -> Result<String, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "chat" => Ok("chat".to_owned()),
        "work" => Ok("work".to_owned()),
        _ => Err("interaction_mode must be chat or work".to_owned()),
    }
}

pub fn parse_reasoning(value: &str) -> Result<ReasoningEffort, String> {
    ReasoningEffort::ALL
        .iter()
        .copied()
        .find(|effort| effort.label().eq_ignore_ascii_case(value.trim()))
        .ok_or_else(|| "reasoning must be off, low, medium, high, or max".to_owned())
}

// MARK: Keys

fn provider_keys(command: KeysCommand) -> Result<(), String> {
    match command {
        KeysCommand::List => {
            let rows = Provider::ALL
                .iter()
                .filter(|provider| **provider != Provider::Ollama)
                .map(|provider| {
                    let source = keys::source(*provider);
                    vec![
                        provider.id().to_owned(),
                        provider.label().to_owned(),
                        source.label().to_owned(),
                        keys::environment_hint(*provider),
                    ]
                })
                .collect::<Vec<_>>();
            output::table(&["Provider", "Service", "Source", "Fallback"], &rows);
            Ok(())
        }
        KeysCommand::Set { provider, value } => {
            let provider = parse_provider(&provider)?;
            let value = match value {
                Some(value) => value,
                None => {
                    output::read_secret(&format!("{} API key (blank clears): ", provider.label()))?
                }
            };
            keys::save(provider, &value)?;
            if value.trim().is_empty() {
                println!("Cleared the {} key.", provider.label());
            } else {
                println!(
                    "Saved the {} key ({}) .",
                    provider.label(),
                    keys::source(provider).label()
                );
            }
            Ok(())
        }
        KeysCommand::Clear { provider, yes } => {
            let provider = parse_provider(&provider)?;
            if !output::confirm(
                &format!("Remove the stored {} API key?", provider.label()),
                yes,
            ) {
                return Err(output::cancelled());
            }
            keys::clear(provider)?;
            println!("Cleared the {} key.", provider.label());
            Ok(())
        }
    }
}

fn parse_provider(value: &str) -> Result<Provider, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "gemini" | "google" => Ok(Provider::Gemini),
        "anthropic" | "claude" => Ok(Provider::Anthropic),
        "openai" | "gpt" => Ok(Provider::OpenAi),
        "local" | "local-endpoint" | "openai-compatible" | "openai_compatible" => {
            Ok(Provider::OpenAiCompatible)
        }
        "ollama" => Err("Ollama models do not need an API key".to_owned()),
        other => Err(format!(
            "Unknown provider: {other} (gemini, anthropic, openai, local)"
        )),
    }
}

// MARK: Presets

fn presets(ctx: &mut CliContext, command: PresetsCommand) -> Result<(), String> {
    match command {
        PresetsCommand::List { json } => {
            let presets = ctx.store.presets().map_err(|error| error.to_string())?;
            if json {
                return output::json(&presets);
            }
            let rows = presets
                .iter()
                .map(|preset| {
                    vec![
                        preset.id.clone(),
                        preset.name.clone(),
                        if preset.builtin {
                            "built-in".to_owned()
                        } else {
                            "custom".to_owned()
                        },
                        preset.description.clone(),
                    ]
                })
                .collect::<Vec<_>>();
            output::table(&["ID", "Name", "Kind", "Description"], &rows);
            Ok(())
        }
        PresetsCommand::Show { id, json } => {
            let preset = find_preset(ctx, &id)?;
            if json {
                return output::json(&preset);
            }
            println!("{}  [{}]", preset.name, preset.id);
            println!(
                "{}{}",
                preset.description,
                if preset.builtin { " · built-in" } else { "" }
            );
            println!();
            println!("{}", preset.instructions);
            Ok(())
        }
        PresetsCommand::Use { id } => {
            let preset = find_preset(ctx, &id)?;
            ctx.settings.active_preset_id = preset.id.clone();
            ctx.save()?;
            println!("Active personality: {} ({})", preset.name, preset.id);
            Ok(())
        }
        PresetsCommand::Create {
            name,
            description,
            instructions,
        } => {
            let preset = PersonalityPreset {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.trim().to_owned(),
                description,
                instructions,
                builtin: false,
            };
            if preset.name.is_empty() {
                return Err("A preset needs a name.".to_owned());
            }
            ctx.store
                .save_preset(&preset)
                .map_err(|error| error.to_string())?;
            println!("{}", preset.id);
            eprintln!("Created preset “{}”.", preset.name);
            Ok(())
        }
        PresetsCommand::Edit {
            id,
            name,
            description,
            instructions,
        } => {
            let mut preset = find_preset(ctx, &id)?;
            if preset.builtin {
                return Err(
                    "Built-in presets are read-only; duplicate it first with `presets duplicate`."
                        .to_owned(),
                );
            }
            if let Some(name) = name {
                if name.trim().is_empty() {
                    return Err("A preset needs a name.".to_owned());
                }
                preset.name = name;
            }
            if let Some(description) = description {
                preset.description = description;
            }
            if let Some(instructions) = instructions {
                preset.instructions = instructions;
            }
            ctx.store
                .save_preset(&preset)
                .map_err(|error| error.to_string())?;
            println!("Updated preset “{}”.", preset.name);
            Ok(())
        }
        PresetsCommand::Duplicate { id, name } => {
            let source = find_preset(ctx, &id)?;
            let preset = PersonalityPreset {
                id: uuid::Uuid::new_v4().to_string(),
                name: name.unwrap_or_else(|| format!("{} copy", source.name)),
                description: source.description,
                instructions: source.instructions,
                builtin: false,
            };
            ctx.store
                .save_preset(&preset)
                .map_err(|error| error.to_string())?;
            println!("{}", preset.id);
            eprintln!("Duplicated to “{}”.", preset.name);
            Ok(())
        }
        PresetsCommand::Delete { id, yes } => {
            let preset = find_preset(ctx, &id)?;
            if preset.builtin {
                return Err(
                    "Built-in presets cannot be deleted; duplicate it to customize.".to_owned(),
                );
            }
            if !output::confirm(&format!("Delete preset “{}”?", preset.name), yes) {
                return Err(output::cancelled());
            }
            ctx.store
                .delete_preset(&preset.id)
                .map_err(|error| error.to_string())?;
            if ctx.settings.active_preset_id == preset.id {
                ctx.settings.active_preset_id = "balanced".to_owned();
                ctx.save()?;
            }
            println!("Deleted preset “{}”.", preset.name);
            Ok(())
        }
    }
}

fn find_preset(ctx: &CliContext, id: &str) -> Result<PersonalityPreset, String> {
    let presets = ctx.store.presets().map_err(|error| error.to_string())?;
    presets
        .iter()
        .find(|preset| preset.id == id)
        .cloned()
        .or_else(|| relay_core::find_builtin_preset(id))
        .ok_or_else(|| format!("Unknown preset: {id}"))
}

// MARK: Memory

async fn memory_command(ctx: &mut CliContext, command: MemoryCommand) -> Result<(), String> {
    match command {
        MemoryCommand::List { json } => {
            let memories = ctx
                .store
                .list_memories()
                .map_err(|error| error.to_string())?;
            if json {
                let items = memories
                    .iter()
                    .map(|(id, content)| json!({"id": id, "content": content}))
                    .collect::<Vec<_>>();
                return output::json(&items);
            }
            if memories.is_empty() {
                println!("No cross-session memories yet.");
                return Ok(());
            }
            for (id, content) in memories {
                println!("[{}] {}", output::short_id(&id), content);
            }
            Ok(())
        }
        MemoryCommand::Add { text } => {
            let id = ctx
                .store
                .add_memory(&text)
                .map_err(|error| error.to_string())?;
            println!("{}", output::short_id(&id));
            eprintln!("Remembered.");
            Ok(())
        }
        MemoryCommand::Delete { id, yes } => {
            let memories = ctx
                .store
                .list_memories()
                .map_err(|error| error.to_string())?;
            let matches: Vec<(String, String)> = memories
                .into_iter()
                .filter(|(memory_id, _)| memory_id.starts_with(&id))
                .collect();
            let (memory_id, content) = match matches.len() {
                1 => matches
                    .into_iter()
                    .next()
                    .ok_or_else(|| "No match.".to_owned())?,
                0 => return Err(format!("No memory matches “{id}”.")),
                _ => {
                    return Err(format!(
                        "“{id}” matches more than one memory; use more characters."
                    ));
                }
            };
            if !output::confirm(
                &format!("Forget this memory?\n  {}", output::truncate(&content, 160)),
                yes,
            ) {
                return Err(output::cancelled());
            }
            ctx.store
                .delete_memory(&memory_id)
                .map_err(|error| error.to_string())?;
            println!("Memory forgotten.");
            Ok(())
        }
        MemoryCommand::Summarize { force } => chat::summarize_memory(ctx, force).await,
    }
}

// MARK: MCP

fn mcp_command(ctx: &mut CliContext, command: McpCommand) -> Result<(), String> {
    match command {
        McpCommand::List { json } => {
            let servers = &ctx.settings.mcp_servers;
            if json {
                return output::json(servers);
            }
            if servers.is_empty() {
                println!("No MCP servers configured. Add one with `relay-cli mcp add`.");
                return Ok(());
            }
            let rows = servers
                .iter()
                .map(|server| {
                    vec![
                        server.name.clone(),
                        server.command.clone(),
                        server.args.join(" "),
                        server.env.join(" "),
                    ]
                })
                .collect::<Vec<_>>();
            output::table(&["Name", "Command", "Arguments", "Environment"], &rows);
            Ok(())
        }
        McpCommand::Tools { json } => {
            let tools = mcp::global_tools();
            if json {
                return output::json(&tools);
            }
            if tools.is_empty() {
                println!("No MCP tools are available.");
                return Ok(());
            }
            let rows = tools
                .iter()
                .map(|tool| vec![tool.name.clone(), output::truncate(&tool.description, 80)])
                .collect::<Vec<_>>();
            output::table(&["Tool", "Description"], &rows);
            Ok(())
        }
        McpCommand::Call { tool, arguments } => {
            let arguments: Value = serde_json::from_str(&arguments)
                .map_err(|error| format!("arguments must be JSON: {error}"))?;
            let output = match mcp::global_call(&tool, arguments.clone()) {
                Some(output) => output,
                None => {
                    ctx.reload_mcp();
                    mcp::global_call(&tool, arguments)
                        .ok_or_else(|| format!("MCP tool not found: {tool}"))?
                }
            };
            println!("{output}");
            Ok(())
        }
        McpCommand::Add {
            name,
            command,
            args,
            env,
        } => {
            let config = McpServerConfig {
                name,
                command,
                args,
                env,
            };
            validate_mcp_configs(std::slice::from_ref(&config))?;
            ctx.settings
                .mcp_servers
                .retain(|server| server.name != config.name);
            ctx.settings.mcp_servers.push(config);
            ctx.save()?;
            ctx.reload_mcp();
            println!(
                "Added MCP server “{}”; {} tools available.",
                ctx.settings
                    .mcp_servers
                    .last()
                    .map(|server| server.name.as_str())
                    .unwrap_or(""),
                mcp::global_tools().len()
            );
            Ok(())
        }
        McpCommand::Remove { name, yes } => {
            if !ctx
                .settings
                .mcp_servers
                .iter()
                .any(|server| server.name == name)
            {
                return Err(format!("No MCP server named “{name}”."));
            }
            if !output::confirm(&format!("Remove MCP server “{name}”?"), yes) {
                return Err(output::cancelled());
            }
            ctx.settings
                .mcp_servers
                .retain(|server| server.name != name);
            ctx.save()?;
            ctx.reload_mcp();
            println!("Removed MCP server “{name}”.");
            Ok(())
        }
        McpCommand::Reload => {
            ctx.reload_mcp();
            println!(
                "Reloaded {} MCP server{}; {} tools available.",
                ctx.settings.mcp_servers.len(),
                if ctx.settings.mcp_servers.len() == 1 {
                    ""
                } else {
                    "s"
                },
                mcp::global_tools().len()
            );
            Ok(())
        }
    }
}

fn validate_mcp_configs(configs: &[McpServerConfig]) -> Result<(), String> {
    let mut names = std::collections::HashSet::new();
    for config in configs {
        if config.name.trim().is_empty() {
            return Err("Every MCP server needs a name.".to_owned());
        }
        if config.name.contains("__") {
            return Err("MCP server names cannot contain “__”.".to_owned());
        }
        if config.command.trim().is_empty() {
            return Err(format!("MCP server “{}” needs a command.", config.name));
        }
        if !names.insert(config.name.clone()) {
            return Err(format!("Duplicate MCP server name: {}", config.name));
        }
    }
    Ok(())
}

// MARK: Bundled models

async fn bundled_command(ctx: &mut CliContext, command: BundledCommand) -> Result<(), String> {
    match command {
        BundledCommand::Status { json } => {
            let status = bundled_status(ctx).await;
            if json {
                return output::json(&status);
            }
            output::heading("Bundled Nemotron");
            println!("Selected variant: {}", ctx.settings.bundled_model_variant);
            println!(
                "Model downloaded: {}",
                yes_no(status["downloaded"].as_bool())
            );
            if let Some(path) = status["model_path"].as_str() {
                println!("Model path:       {path}");
            }
            println!(
                "llama-server:     {}",
                yes_no(status["llama_server"].as_bool())
            );
            println!("Running:          {}", yes_no(status["running"].as_bool()));
            if status["running"].as_bool() == Some(true) {
                println!("Endpoint:         {}", bundled::endpoint());
            }
            if let Some(message) = status["message"].as_str()
                && !message.is_empty()
            {
                println!("State:            {message}");
            }
            if let Some(error) = status["error"].as_str() {
                println!("Error:            {error}");
            }
            Ok(())
        }
        BundledCommand::Prepare { variant, yes } => {
            let variant = bundled::BundledVariant::from_id(&variant)
                .ok_or_else(|| "variant must be standard or uncensored".to_owned())?;
            if bundled::probe_running().await {
                println!(
                    "Bundled server is already running at {}.",
                    bundled::endpoint()
                );
                return Ok(());
            }
            let need_download = !bundled::downloaded(variant);
            let need_server = bundled::llama_server_path().is_none();
            if (need_download || need_server) && !yes {
                if need_download {
                    eprintln!(
                        "This downloads {} from:\n  {}\nto {}",
                        variant.download_label(),
                        variant.gguf_url(),
                        bundled::model_path_for(variant)
                            .map(|path| path.display().to_string())
                            .unwrap_or_else(|| "the Relay data directory".to_owned())
                    );
                }
                if need_server {
                    eprintln!(
                        "It also downloads a llama.cpp llama-server build (Vulkan, ~{} MB).",
                        50
                    );
                }
                if !output::confirm(
                    "Download and run the bundled model? This is a large network transfer.",
                    false,
                ) {
                    return Err(output::cancelled());
                }
            }
            chat::prepare_bundled(ctx, variant, ctx.settings.bundled_context_size).await?;
            println!("Bundled model ready at {}.", bundled::endpoint());
            println!("Press Ctrl+C to stop the server.");
            let _ = tokio::signal::ctrl_c().await;
            bundled::stop(&ctx.bundled);
            Ok(())
        }
        BundledCommand::Path => {
            let variant = selected_bundled_variant(&ctx.settings);
            println!(
                "model: {}",
                bundled::model_path_for(variant)
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "unavailable".to_owned())
            );
            println!(
                "server: {}",
                bundled::llama_server_path()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "not installed".to_owned())
            );
            println!("endpoint: {}", bundled::endpoint());
            Ok(())
        }
    }
}

fn yes_no(value: Option<bool>) -> &'static str {
    if value == Some(true) { "yes" } else { "no" }
}

fn selected_bundled_variant(settings: &AppSettings) -> bundled::BundledVariant {
    bundled::BundledVariant::from_id(&settings.bundled_model_variant)
        .or_else(|| bundled::BundledVariant::from_model_slug(&settings.local_model))
        .unwrap_or(bundled::DEFAULT_VARIANT)
}

async fn bundled_status(ctx: &CliContext) -> Value {
    let variant = selected_bundled_variant(&ctx.settings);
    let status = ctx
        .bundled
        .lock()
        .map(|state| state.status())
        .unwrap_or_default();
    let running = bundled::probe_running().await;
    json!({
        "variant": variant.id(),
        "name": variant.display_name(),
        "downloaded": bundled::downloaded(variant),
        "model_path": bundled::model_path_for(variant).map(|path| path.display().to_string()),
        "llama_server": bundled::llama_server_path().map(|path| path.display().to_string()),
        "running": running,
        "endpoint": bundled::endpoint(),
        "message": status.message,
        "error": status.error,
        "progress": status.progress,
        "in_process_ready": status.ready,
    })
}

// MARK: Ollama

async fn ollama_command(ctx: &mut CliContext, command: OllamaCommand) -> Result<(), String> {
    match command {
        OllamaCommand::Status { json } => {
            let reachable = local::ollama_reachable(&ctx.settings.ollama_base_url).await;
            let installed = if reachable {
                local::list_models_api(&ctx.settings.ollama_base_url)
                    .await
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            if json {
                return output::json(&json!({
                    "base_url": ctx.settings.ollama_base_url,
                    "reachable": reachable,
                    "installed": installed,
                    "default_model": ctx.settings.ollama_model,
                }));
            }
            println!("Endpoint: {}", ctx.settings.ollama_base_url);
            println!("Reachable: {}", yes_no(Some(reachable)));
            println!(
                "Default model: {}",
                if ctx.settings.ollama_model.is_empty() {
                    "(none)"
                } else {
                    &ctx.settings.ollama_model
                }
            );
            println!("Installed models: {}", installed.len());
            for name in installed {
                println!("  {name}");
            }
            Ok(())
        }
        OllamaCommand::List { json } => {
            ensure_ollama(ctx).await?;
            let installed = local::list_models_api(&ctx.settings.ollama_base_url)
                .await
                .unwrap_or_else(|_| local::installed_ollama_models());
            if json {
                let curated = local::CURATED
                    .iter()
                    .map(|model| {
                        json!({
                            "tag": model.tag,
                            "name": model.name,
                            "license": model.license,
                            "installed": local::installed(model.tag),
                        })
                    })
                    .collect::<Vec<_>>();
                return output::json(&json!({"installed": installed, "curated": curated}));
            }
            if installed.is_empty() {
                println!("No Ollama models installed.");
            } else {
                output::heading("Installed Ollama models");
                for name in &installed {
                    println!("  {name}");
                }
            }
            let missing = local::CURATED
                .iter()
                .filter(|model| !local::installed(model.tag))
                .collect::<Vec<_>>();
            if !missing.is_empty() {
                println!();
                output::heading("Installable curated models");
                for model in missing {
                    println!("  {:<24} {} · {}", model.tag, model.name, model.license);
                }
                println!("\nInstall with `relay-cli ollama pull <tag>` (asks before downloading).");
            }
            Ok(())
        }
        OllamaCommand::Pull { tag, yes } => {
            ensure_ollama(ctx).await?;
            let label = local::curated(&tag)
                .map(|model| format!("{} ({})", model.name, model.license))
                .unwrap_or_else(|| tag.clone());
            if !output::confirm(
                &format!("Download Ollama model {label}? This can be several gigabytes."),
                yes,
            ) {
                return Err(output::cancelled());
            }
            eprintln!("Pulling {tag}…");
            local::pull(&tag)?;
            ctx.settings.ollama_model = tag.clone();
            ctx.save()?;
            println!("Installed {tag}.");
            Ok(())
        }
        OllamaCommand::Serve => {
            if local::ollama_reachable(&ctx.settings.ollama_base_url).await {
                println!(
                    "Ollama is already running at {}.",
                    ctx.settings.ollama_base_url
                );
                return Ok(());
            }
            ensure_ollama(ctx).await?;
            println!(
                "Ollama running at {}. Press Ctrl+C to stop it.",
                ctx.settings.ollama_base_url
            );
            let _ = tokio::signal::ctrl_c().await;
            if let Some(mut child) = ctx.ollama_child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            Ok(())
        }
    }
}

/// Make sure an Ollama daemon is answering, starting one if necessary.
pub async fn ensure_ollama(ctx: &mut CliContext) -> Result<(), String> {
    if local::ollama_reachable(&ctx.settings.ollama_base_url).await {
        return Ok(());
    }
    if let Some(child) = local::ensure_ollama_running(&ctx.settings.ollama_base_url).await? {
        ctx.ollama_child = Some(child);
    }
    Ok(())
}

// MARK: Web, shell, diagnostics

async fn web(query: &str, json: bool) -> Result<(), String> {
    let results = search::web_search(query, 8).await;
    if json {
        return output::json(&json!({"query": query, "results": results}));
    }
    println!("{results}");
    Ok(())
}

fn shell() -> Result<(), String> {
    let shell = relay_core::default_shell();
    let flavor = relay_core::shell_flavor(&shell);
    eprintln!("Opening {shell}. Type `exit` to return to Relay CLI.");
    let status = ProcessCommand::new(&shell)
        .args(flavor.interactive_args())
        .env("RELAY_CLI", "1")
        .status()
        .map_err(|error| format!("could not start {shell}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{shell} exited with {status}"))
    }
}

pub async fn status(ctx: &mut CliContext, json: bool) -> Result<(), String> {
    let stats = ctx.store.stats().map_err(|error| error.to_string())?;
    let count = ctx
        .store
        .list_conversations()
        .map_err(|error| error.to_string())?
        .len();
    let archived = ctx
        .store
        .list_archived_conversations()
        .map_err(|error| error.to_string())?
        .len();
    let model = agent::resolve_model(&ctx.settings.default_model);
    let key_sources = Provider::CLOUD
        .iter()
        .map(|provider| {
            (
                provider.id().to_owned(),
                keys::source(*provider).label().to_owned(),
            )
        })
        .collect::<Vec<_>>();
    let ollama_reachable = local::ollama_reachable(&ctx.settings.ollama_base_url).await;
    let bundled_running = bundled::probe_running().await;
    let variant = selected_bundled_variant(&ctx.settings);
    let mcp_tools = mcp::global_tools().len();

    if json {
        return output::json(&json!({
            "version": env!("CARGO_PKG_VERSION"),
            "data_dir": data_dir().ok().map(|path| path.display().to_string()),
            "database": ctx.store.path().display().to_string(),
            "conversations": count,
            "archived": archived,
            "messages": stats.messages,
            "tokens": stats.tokens,
            "model": {
                "slug": model.slug,
                "name": model.name,
                "provider": model.provider.id(),
                "context_window": agent::context_window(&ctx.settings, &model),
                "max_output": model.max_output,
                "reasoning": model.reasoning,
                "effective_reasoning": agent::effective_reasoning(&ctx.settings, &model),
            },
            "keys": key_sources,
            "ollama": {
                "base_url": ctx.settings.ollama_base_url,
                "reachable": ollama_reachable,
                "model": ctx.settings.ollama_model,
            },
            "bundled": {
                "variant": variant.id(),
                "downloaded": bundled::downloaded(variant),
                "running": bundled_running,
                "endpoint": bundled::endpoint(),
            },
            "mcp": {
                "servers": ctx.settings.mcp_servers.len(),
                "tools": mcp_tools,
            },
            "settings": {
                "interaction_mode": ctx.settings.interaction_mode,
                "agent_mode": ctx.settings.agent_mode,
                "tool_approval": ctx.settings.tool_approval,
                "reasoning": ctx.settings.reasoning,
                "temperature": ctx.settings.temperature,
                "max_output_tokens": ctx.settings.max_output_tokens,
                "active_preset_id": ctx.settings.active_preset_id,
                "custom_instructions": !ctx.settings.custom_instructions.trim().is_empty(),
                "show_thinking": ctx.settings.show_thinking,
            },
        }));
    }

    output::heading("Relay CLI status");
    println!("Version:      {}", env!("CARGO_PKG_VERSION"));
    println!("Database:     {}", ctx.store.path().display());
    println!(
        "History:      {} active · {} archived · {} messages · {} tokens",
        count,
        archived,
        stats.messages,
        format_tokens_exact(stats.tokens)
    );
    println!();
    println!(
        "Model:        {} ({}) · {} context",
        model.name,
        model.provider.label(),
        relay_core::format_tokens_exact(u64::from(agent::context_window(&ctx.settings, &model)))
    );
    println!(
        "Generation:   temp {:.2} · output {} · preset {}",
        ctx.settings.temperature,
        output::token_limit(ctx.settings.max_output_tokens),
        ctx.settings.active_preset_id
    );
    println!(
        "Mode:         {} · {} · approval {}",
        if ctx.settings.interaction_mode == "chat" {
            "Chat"
        } else {
            "Work"
        },
        ctx.settings.agent_mode.label(),
        ctx.settings.tool_approval.label()
    );
    println!(
        "Reasoning:    {} · {}",
        agent::effective_reasoning(&ctx.settings, &model).label(),
        reasoning_kind_label(model.reasoning)
    );
    println!();
    println!("Cloud keys:");
    for (provider, source) in &key_sources {
        println!("  {provider:<12} {source}");
    }
    println!();
    println!(
        "Ollama:       {} · {}",
        ctx.settings.ollama_base_url,
        if ollama_reachable {
            "reachable"
        } else {
            "offline"
        }
    );
    println!(
        "Bundled:      {} · {} · {}",
        variant.display_name(),
        if bundled::downloaded(variant) {
            "downloaded"
        } else {
            "not downloaded"
        },
        if bundled_running {
            "running"
        } else {
            "stopped"
        }
    );
    println!(
        "MCP:          {} servers · {} tools",
        ctx.settings.mcp_servers.len(),
        mcp_tools
    );
    Ok(())
}

fn about(json: bool) -> Result<(), String> {
    if json {
        return output::json(&json!({
            "name": "Relay CLI",
            "version": env!("CARGO_PKG_VERSION"),
            "license": "Apache-2.0",
            "repository": relay_core::PROJECT_URL,
            "description": "The Relay desktop app's capability set without the window.",
        }));
    }
    println!("Relay CLI {}", env!("CARGO_PKG_VERSION"));
    println!("One conversation, every frontier model.");
    println!();
    println!("A headless client for the same encrypted history, model catalog, agent");
    println!("tools, MCP servers, presets, and cross-session memory as the Relay");
    println!("desktop app. It links the shared relay-engine and never embeds GPUI.");
    println!();
    println!("License:    Apache-2.0");
    println!("Repository: {}", relay_core::PROJECT_URL);
    println!();
    println!("Privacy:");
    println!("  · History is stored in a local SQLCipher database.");
    println!("  · API keys are sealed with ChaCha20-Poly1305 in the OS keyring.");
    println!("  · Cloud requests go only to the provider you select.");
    println!("  · Local models and web tools run from your machine.");
    Ok(())
}

/// Print a shell completion script for the requested shell.
pub fn completions(shell: &str) -> String {
    match shell.to_ascii_lowercase().as_str() {
        "zsh" => ZSH_COMPLETIONS.to_owned(),
        "fish" => FISH_COMPLETIONS.to_owned(),
        "powershell" | "pwsh" => POWERSHELL_COMPLETIONS.to_owned(),
        _ => BASH_COMPLETIONS.to_owned(),
    }
}

const BASH_COMPLETIONS: &str = r#"
_relay_cli() {
    local cur prev words cword
    _init_completion || COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    local commands="chat ask prompt sessions conversations history models model settings config keys key presets preset memory memories mcp bundled nemotron ollama web search fetch url shell terminal clear cls status about help version completions completion"
    if [[ $COMP_CWORD -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "$commands" -- "$cur") )
        return 0
    fi
    case "${COMP_WORDS[1]}" in
        completions|completion)
            COMPREPLY=( $(compgen -W "bash zsh fish powershell" -- "$cur") )
            ;;
    esac
    return 0
}
complete -F _relay_cli relay-cli
"#;

const ZSH_COMPLETIONS: &str = r#"
#compdef relay-cli
_relay_cli() {
    local -a commands
    commands=(
        'chat:Open an interactive chat'
        'models:List models'
        'settings:Show or set preferences'
        'status:Show status'
        'version:Print version'
        'completions:Print shell completions'
    )
    _describe 'command' commands
}
compdef _relay_cli relay-cli
"#;

const FISH_COMPLETIONS: &str = r#"
complete -c relay-cli -f
complete -c relay-cli -n '__fish_use_subcommand' -a 'chat models settings status version completions'
complete -c relay-cli -n '__fish_seen_subcommand_from completions' -a 'bash zsh fish powershell'
"#;

const POWERSHELL_COMPLETIONS: &str = r#"
Register-ArgumentCompleter -Native -CommandName relay-cli -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @(
        'chat', 'ask', 'prompt', 'sessions', 'conversations', 'history', 'models',
        'model', 'settings', 'config', 'keys', 'key', 'presets', 'preset',
        'memory', 'memories', 'mcp', 'bundled', 'nemotron', 'ollama', 'web',
        'search', 'fetch', 'url', 'shell', 'terminal', 'clear', 'cls', 'status',
        'about', 'help', 'version', 'completions', 'completion'
    )
    $tokens = $commandAst.CommandElements
    if ($tokens.Count -le 2) {
        $commands | Where-Object { $_ -like "$wordToComplete*" } | ForEach-Object {
            [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
        }
    } elseif ($tokens[1].Value -in @('completions', 'completion')) {
        @('bash', 'zsh', 'fish', 'powershell') |
            Where-Object { $_ -like "$wordToComplete*" } |
            ForEach-Object {
                [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
            }
    }
}
"#;
