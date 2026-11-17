//! Command-line parsing and help text.
//!
//! Relay deliberately avoids a heavyweight argument framework so the CLI
//! binary stays small and starts instantly.

use std::path::PathBuf;

use relay_core::{AgentMode, ReasoningEffort, ToolApproval};

use crate::chat::ChatOptions;

#[derive(Debug, Clone)]
pub enum Command {
    Chat(ChatOptions),
    Prompt(ChatOptions, String),
    Sessions(SessionsCommand),
    Models { json: bool },
    Model { slug: Option<String> },
    Settings(SettingsCommand),
    Keys(KeysCommand),
    Presets(PresetsCommand),
    Memory(MemoryCommand),
    Mcp(McpCommand),
    Bundled(BundledCommand),
    Ollama(OllamaCommand),
    Web { query: String, json: bool },
    Fetch { url: String },
    Shell,
    Clear,
    Status { json: bool },
    About { json: bool },
    Help(Option<String>),
    Version,
}

#[derive(Debug, Clone)]
pub enum SessionsCommand {
    List {
        archived: bool,
        json: bool,
        limit: Option<usize>,
    },
    Search {
        query: String,
        json: bool,
    },
    Show {
        id: String,
        json: bool,
        thinking: bool,
    },
    New {
        model: Option<String>,
    },
    Rename {
        id: String,
        title: String,
    },
    Pin {
        id: String,
        pinned: bool,
    },
    Archive {
        id: String,
        archived: bool,
    },
    Model {
        id: String,
        slug: String,
    },
    Delete {
        ids: Vec<String>,
        all: bool,
        yes: bool,
    },
    Export {
        id: Option<String>,
        all: bool,
        output: Option<PathBuf>,
        copy: bool,
    },
}

#[derive(Debug, Clone)]
pub enum SettingsCommand {
    Show { json: bool },
    Get { key: String },
    Set { key: String, value: String },
    Reset { yes: bool },
    Path,
}

#[derive(Debug, Clone)]
pub enum KeysCommand {
    List,
    Set {
        provider: String,
        value: Option<String>,
    },
    Clear {
        provider: String,
        yes: bool,
    },
}

#[derive(Debug, Clone)]
pub enum PresetsCommand {
    List {
        json: bool,
    },
    Show {
        id: String,
        json: bool,
    },
    Use {
        id: String,
    },
    Create {
        name: String,
        description: String,
        instructions: String,
    },
    Edit {
        id: String,
        name: Option<String>,
        description: Option<String>,
        instructions: Option<String>,
    },
    Duplicate {
        id: String,
        name: Option<String>,
    },
    Delete {
        id: String,
        yes: bool,
    },
}

#[derive(Debug, Clone)]
pub enum MemoryCommand {
    List { json: bool },
    Add { text: String },
    Delete { id: String, yes: bool },
    Summarize { force: bool },
}

#[derive(Debug, Clone)]
pub enum McpCommand {
    List {
        json: bool,
    },
    Tools {
        json: bool,
    },
    Call {
        tool: String,
        arguments: String,
    },
    Add {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<String>,
    },
    Remove {
        name: String,
        yes: bool,
    },
    Reload,
}

#[derive(Debug, Clone)]
pub enum BundledCommand {
    Status { json: bool },
    Prepare { variant: String, yes: bool },
    Path,
}

#[derive(Debug, Clone)]
pub enum OllamaCommand {
    Status { json: bool },
    List { json: bool },
    Pull { tag: String, yes: bool },
    Serve,
}

pub const HELP_MARKER: &str = "\u{1f4d8}HELP\u{1f4d8}";
pub fn help_text(error: &str) -> Option<&str> {
    error.strip_prefix(HELP_MARKER)
}

pub fn parse() -> Result<Command, String> {
    parse_from(std::env::args().skip(1).collect())
}

pub fn parse_from(args: Vec<String>) -> Result<Command, String> {
    if args.is_empty() {
        return Ok(Command::Chat(ChatOptions::default()));
    }
    let first = args[0].as_str();
    match first {
        "-h" | "--help" => return Ok(Command::Help(None)),
        "-V" | "--version" => return Ok(Command::Version),
        _ => {}
    }

    if first.starts_with('-') {
        return parse_chat_options(&args);
    }

    let rest = &args[1..];
    match first {
        "chat" | "repl" => parse_chat_options(rest),
        "ask" | "prompt" | "say" => parse_prompt(rest),
        "sessions" | "conversations" | "history" => parse_sessions(rest),
        "models" | "model-list" => Ok(Command::Models {
            json: has_flag(rest, "--json"),
        }),
        "model" => Ok(Command::Model {
            slug: rest.first().filter(|arg| !arg.starts_with('-')).cloned(),
        }),
        "settings" | "config" => parse_settings(rest),
        "keys" | "key" => parse_keys(rest),
        "presets" | "preset" => parse_presets(rest),
        "memory" | "memories" => parse_memory(rest),
        "mcp" => parse_mcp(rest),
        "bundled" | "nemotron" => parse_bundled(rest),
        "ollama" => parse_ollama(rest),
        "web" | "search" => {
            let json = has_flag(rest, "--json");
            let query = positional(rest).join(" ");
            if query.trim().is_empty() {
                Err("Usage: relay-cli web <query> [--json]".to_owned())
            } else {
                Ok(Command::Web { query, json })
            }
        }
        "fetch" | "url" => match rest.first() {
            Some(url) => Ok(Command::Fetch { url: url.clone() }),
            None => Err("Usage: relay-cli fetch <url>".to_owned()),
        },
        "shell" | "terminal" => Ok(Command::Shell),
        "clear" | "cls" | "relay-clear" => Ok(Command::Clear),
        "status" => Ok(Command::Status {
            json: has_flag(rest, "--json"),
        }),
        "about" => Ok(Command::About {
            json: has_flag(rest, "--json"),
        }),
        "help" => Ok(Command::Help(rest.first().cloned())),
        "version" => Ok(Command::Version),
        other if other.starts_with('-') => {
            Err(format!("Unknown option: {other}\n\n{}", main_help()))
        }
        _ => {
            // A bare prompt is the friendliest one-shot form:
            // `relay-cli explain this codebase`.
            let prompt = args.join(" ");
            Ok(Command::Prompt(ChatOptions::default(), prompt))
        }
    }
}

fn parse_chat_options(args: &[String]) -> Result<Command, String> {
    let (options, prompt) = parse_chat(args)?;
    match prompt {
        Some(prompt) if !prompt.trim().is_empty() => Ok(Command::Prompt(options, prompt)),
        _ => Ok(Command::Chat(options)),
    }
}

fn parse_prompt(args: &[String]) -> Result<Command, String> {
    let (options, prompt) = parse_chat(args)?;
    match prompt {
        Some(prompt) if !prompt.trim().is_empty() => Ok(Command::Prompt(options, prompt)),
        _ => Ok(Command::Prompt(options, String::new())),
    }
}

fn parse_chat(args: &[String]) -> Result<(ChatOptions, Option<String>), String> {
    let mut options = ChatOptions::default();
    let mut prompt: Vec<String> = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        if arg == "--" {
            prompt.extend(args[index + 1..].iter().cloned());
            break;
        }
        if !arg.starts_with('-') {
            prompt.extend(args[index..].iter().cloned());
            break;
        }
        let (name, inline) = split_option(arg);
        let take_value = |index: &mut usize| -> Result<String, String> {
            if let Some(value) = inline {
                return Ok(value.to_owned());
            }
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match name {
            "-p" | "--prompt" => {
                let value = take_value(&mut index)?;
                prompt.push(value);
                index += 1;
            }
            "--model" | "-m" => {
                options.model = Some(take_value(&mut index)?);
                index += 1;
            }
            "--session" | "-s" => {
                let value = take_value(&mut index)?;
                if value == "new" {
                    options.new = true;
                } else {
                    options.session = Some(value);
                }
                index += 1;
            }
            "--new" | "-n" => {
                options.new = true;
                index += 1;
            }
            "--reasoning" | "-r" | "--reasoning-effort" => {
                let value = take_value(&mut index)?;
                options.reasoning = Some(parse_reasoning(&value)?);
                index += 1;
            }
            "--temperature" | "--temp" => {
                let value = take_value(&mut index)?;
                options.temperature = Some(parse_temperature(&value)?);
                index += 1;
            }
            "--max-output" | "--max-output-tokens" | "--max-tokens" => {
                let value = take_value(&mut index)?;
                options.max_output_tokens = Some(parse_max_output_tokens(&value)?);
                index += 1;
            }
            "--preset" | "--personality" => {
                options.preset = Some(take_value(&mut index)?);
                index += 1;
            }
            "--instructions" | "--custom-instructions" => {
                options.instructions = Some(take_value(&mut index)?);
                index += 1;
            }
            "--instructions-file" => {
                let path = take_value(&mut index)?;
                options.instructions = Some(
                    std::fs::read_to_string(&path)
                        .map_err(|error| format!("could not read {path}: {error}"))?,
                );
                index += 1;
            }
            "--image" | "--attach" => {
                options
                    .images
                    .push(std::path::PathBuf::from(take_value(&mut index)?));
                index += 1;
            }
            "--file" => {
                options
                    .files
                    .push(std::path::PathBuf::from(take_value(&mut index)?));
                index += 1;
            }
            "--mode" => {
                let value = take_value(&mut index)?;
                options.interaction_mode = Some(parse_interaction(&value)?);
                index += 1;
            }
            "--plan" => {
                options.agent_mode = Some(AgentMode::Plan);
                index += 1;
            }
            "--build" => {
                options.agent_mode = Some(AgentMode::Build);
                index += 1;
            }
            "--approval" | "-a" => {
                let value = take_value(&mut index)?;
                options.approval = Some(parse_approval(&value)?);
                index += 1;
            }
            "--yes" | "-y" => {
                options.yes = true;
                index += 1;
            }
            "--json" => {
                options.json = true;
                index += 1;
            }
            "--thinking" => {
                options.show_thinking = Some(true);
                index += 1;
            }
            "--show-thinking" => {
                options.show_thinking = Some(true);
                index += 1;
            }
            "--no-thinking" | "--hide-thinking" => {
                options.show_thinking = Some(false);
                index += 1;
            }
            "-h" | "--help" => {
                return Err(format!("{}{}", HELP_MARKER, chat_help()));
            }
            other => return Err(format!("Unknown chat option: {other}\n\n{}", chat_help())),
        }
    }
    let prompt = if prompt.is_empty() {
        None
    } else {
        Some(prompt.join(" "))
    };
    Ok((options, prompt))
}

fn parse_sessions(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Sessions(SessionsCommand::List {
            archived: false,
            json: false,
            limit: None,
        }));
    };
    let rest = &args[1..];
    let json = has_flag(rest, "--json");
    match sub {
        "list" | "ls" | "recent" => Ok(Command::Sessions(SessionsCommand::List {
            archived: has_flag(rest, "--archived"),
            json,
            limit: option_value(rest, "--limit").and_then(|value| value.parse().ok()),
        })),
        "search" | "find" => {
            let query = positional(rest).join(" ");
            if query.trim().is_empty() {
                Err("Usage: relay-cli sessions search <query>".to_owned())
            } else {
                Ok(Command::Sessions(SessionsCommand::Search { query, json }))
            }
        }
        "show" | "view" | "cat" => {
            let id = positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli sessions show <id>".to_owned())?;
            Ok(Command::Sessions(SessionsCommand::Show {
                id,
                json,
                thinking: has_flag(rest, "--thinking"),
            }))
        }
        "new" => Ok(Command::Sessions(SessionsCommand::New {
            model: option_value(rest, "--model"),
        })),
        "rename" | "title" => {
            let parts = positional(rest);
            if parts.len() < 2 {
                return Err("Usage: relay-cli sessions rename <id> <new title>".to_owned());
            }
            Ok(Command::Sessions(SessionsCommand::Rename {
                id: parts[0].clone(),
                title: parts[1..].join(" "),
            }))
        }
        "pin" | "unpin" => Ok(Command::Sessions(SessionsCommand::Pin {
            id: positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli sessions pin <id>".to_owned())?,
            pinned: sub == "pin",
        })),
        "archive" | "unarchive" => Ok(Command::Sessions(SessionsCommand::Archive {
            id: positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli sessions archive <id>".to_owned())?,
            archived: sub == "archive",
        })),
        "model" => {
            let parts = positional(rest);
            if parts.len() != 2 {
                return Err("Usage: relay-cli sessions model <id> <model-slug>".to_owned());
            }
            Ok(Command::Sessions(SessionsCommand::Model {
                id: parts[0].clone(),
                slug: parts[1].clone(),
            }))
        }
        "delete" | "rm" | "remove" => {
            let all = has_flag(rest, "--all");
            let yes = has_flag(rest, "--yes") || has_flag(rest, "-y");
            let ids = positional(rest);
            if !all && ids.is_empty() {
                return Err(
                    "Usage: relay-cli sessions delete <id...> [--yes] | --all --yes".to_owned(),
                );
            }
            Ok(Command::Sessions(SessionsCommand::Delete { ids, all, yes }))
        }
        "delete-all" | "clear" => Ok(Command::Sessions(SessionsCommand::Delete {
            ids: Vec::new(),
            all: true,
            yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
        })),
        "export" | "save" => {
            let all = has_flag(rest, "--all");
            let id = positional(rest).first().cloned();
            if !all && id.is_none() {
                return Err(
                    "Usage: relay-cli sessions export <id> [--output file] [--copy] | --all [--output file]"
                        .to_owned(),
                );
            }
            Ok(Command::Sessions(SessionsCommand::Export {
                id,
                all,
                output: option_value(rest, "--output").map(PathBuf::from),
                copy: has_flag(rest, "--copy"),
            }))
        }
        "help" => Err(format!("{}{}", HELP_MARKER, sessions_help())),
        other => Err(format!(
            "Unknown sessions command: {other}\n\n{}",
            sessions_help()
        )),
    }
}

fn parse_settings(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Settings(SettingsCommand::Show { json: false }));
    };
    let rest = &args[1..];
    if !matches!(
        sub,
        "show" | "list" | "get" | "set" | "reset" | "path" | "help"
    ) {
        // `relay-cli settings temperature 0.4` is shorthand for set.
        let value = rest.join(" ");
        if value.trim().is_empty() {
            return Ok(Command::Settings(SettingsCommand::Get {
                key: sub.to_owned(),
            }));
        }
        return Ok(Command::Settings(SettingsCommand::Set {
            key: sub.to_owned(),
            value,
        }));
    }
    match sub {
        "show" | "list" => Ok(Command::Settings(SettingsCommand::Show {
            json: has_flag(rest, "--json"),
        })),
        "get" => Ok(Command::Settings(SettingsCommand::Get {
            key: rest
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli settings get <key>".to_owned())?,
        })),
        "set" => {
            if rest.len() < 2 {
                return Err("Usage: relay-cli settings set <key> <value>".to_owned());
            }
            Ok(Command::Settings(SettingsCommand::Set {
                key: rest[0].clone(),
                value: rest[1..].join(" "),
            }))
        }
        "reset" => Ok(Command::Settings(SettingsCommand::Reset {
            yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
        })),
        "path" => Ok(Command::Settings(SettingsCommand::Path)),
        "help" => Err(format!("{}{}", HELP_MARKER, settings_help())),
        _ => unreachable!(),
    }
}

fn parse_keys(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Keys(KeysCommand::List));
    };
    let rest = &args[1..];
    match sub {
        "list" | "status" | "ls" => Ok(Command::Keys(KeysCommand::List)),
        "set" => {
            let provider = rest.first().cloned().ok_or_else(|| {
                "Usage: relay-cli keys set <gemini|anthropic|openai|local> [key]".to_owned()
            })?;
            Ok(Command::Keys(KeysCommand::Set {
                provider,
                value: rest.get(1).cloned(),
            }))
        }
        "clear" | "delete" | "remove" => Ok(Command::Keys(KeysCommand::Clear {
            provider: rest
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli keys clear <provider> [--yes]".to_owned())?,
            yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
        })),
        "help" => Err(format!("{}{}", HELP_MARKER, keys_help())),
        other => Err(format!("Unknown keys command: {other}\n\n{}", keys_help())),
    }
}

fn parse_presets(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Presets(PresetsCommand::List { json: false }));
    };
    let rest = &args[1..];
    match sub {
        "list" | "ls" => Ok(Command::Presets(PresetsCommand::List {
            json: has_flag(rest, "--json"),
        })),
        "show" | "cat" => Ok(Command::Presets(PresetsCommand::Show {
            id: rest
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli presets show <id>".to_owned())?,
            json: has_flag(rest, "--json"),
        })),
        "use" => Ok(Command::Presets(PresetsCommand::Use {
            id: rest
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli presets use <id>".to_owned())?,
        })),
        "create" | "new" => {
            let parts = positional(rest);
            if parts.is_empty() {
                return Err(
                    "Usage: relay-cli presets create <name> [description] [instructions]"
                        .to_owned(),
                );
            }
            let instructions = option_value(rest, "--instructions")
                .or_else(|| {
                    option_value(rest, "--instructions-file")
                        .and_then(|path| std::fs::read_to_string(path).ok())
                })
                .or_else(|| parts.get(2).cloned())
                .unwrap_or_default();
            Ok(Command::Presets(PresetsCommand::Create {
                name: parts[0].clone(),
                description: parts.get(1).cloned().unwrap_or_default(),
                instructions,
            }))
        }
        "edit" | "update" => {
            let id = positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli presets edit <id> [--name ...] [--description ...] [--instructions ...]".to_owned())?;
            Ok(Command::Presets(PresetsCommand::Edit {
                id,
                name: option_value(rest, "--name"),
                description: option_value(rest, "--description"),
                instructions: option_value(rest, "--instructions").or_else(|| {
                    option_value(rest, "--instructions-file")
                        .and_then(|path| std::fs::read_to_string(path).ok())
                }),
            }))
        }
        "duplicate" | "copy" => Ok(Command::Presets(PresetsCommand::Duplicate {
            id: positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli presets duplicate <id> [name]".to_owned())?,
            name: positional(rest).get(1).cloned(),
        })),
        "delete" | "rm" => Ok(Command::Presets(PresetsCommand::Delete {
            id: positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli presets delete <id> [--yes]".to_owned())?,
            yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
        })),
        "help" => Err(format!("{}{}", HELP_MARKER, presets_help())),
        other => Err(format!(
            "Unknown presets command: {other}\n\n{}",
            presets_help()
        )),
    }
}

fn parse_memory(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Memory(MemoryCommand::List { json: false }));
    };
    let rest = &args[1..];
    match sub {
        "list" | "ls" => Ok(Command::Memory(MemoryCommand::List {
            json: has_flag(rest, "--json"),
        })),
        "add" | "remember" => {
            let text = positional(rest).join(" ");
            if text.trim().is_empty() {
                Err("Usage: relay-cli memory add <text>".to_owned())
            } else {
                Ok(Command::Memory(MemoryCommand::Add { text }))
            }
        }
        "delete" | "rm" | "forget" => Ok(Command::Memory(MemoryCommand::Delete {
            id: positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli memory delete <id> [--yes]".to_owned())?,
            yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
        })),
        "summarize" | "summary" => Ok(Command::Memory(MemoryCommand::Summarize {
            force: has_flag(rest, "--force") || has_flag(rest, "-f"),
        })),
        "help" => Err(format!("{}{}", HELP_MARKER, memory_help())),
        other => Err(format!(
            "Unknown memory command: {other}\n\n{}",
            memory_help()
        )),
    }
}

fn parse_mcp(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Mcp(McpCommand::List { json: false }));
    };
    let rest = &args[1..];
    match sub {
        "list" | "ls" => Ok(Command::Mcp(McpCommand::List {
            json: has_flag(rest, "--json"),
        })),
        "tools" => Ok(Command::Mcp(McpCommand::Tools {
            json: has_flag(rest, "--json"),
        })),
        "call" => {
            let parts = positional(rest);
            if parts.is_empty() {
                return Err("Usage: relay-cli mcp call <tool> [json-arguments]".to_owned());
            }
            Ok(Command::Mcp(McpCommand::Call {
                tool: parts[0].clone(),
                arguments: parts.get(1).cloned().unwrap_or_else(|| "{}".to_owned()),
            }))
        }
        "add" => {
            let parts = positional(rest);
            if parts.len() < 2 {
                return Err("Usage: relay-cli mcp add <name> <command> [args...]".to_owned());
            }
            let args = rest
                .iter()
                .skip_while(|arg| arg.as_str() != "--")
                .skip(1)
                .cloned()
                .collect();
            Ok(Command::Mcp(McpCommand::Add {
                name: parts[0].clone(),
                command: parts[1].clone(),
                args,
                env: option_value(rest, "--env")
                    .map(|value| vec![value])
                    .unwrap_or_default(),
            }))
        }
        "remove" | "rm" => Ok(Command::Mcp(McpCommand::Remove {
            name: positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli mcp remove <name> [--yes]".to_owned())?,
            yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
        })),
        "reload" => Ok(Command::Mcp(McpCommand::Reload)),
        "help" => Err(format!("{}{}", HELP_MARKER, mcp_help())),
        other => Err(format!("Unknown mcp command: {other}\n\n{}", mcp_help())),
    }
}

fn parse_bundled(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Bundled(BundledCommand::Status { json: false }));
    };
    let rest = &args[1..];
    match sub {
        "status" => Ok(Command::Bundled(BundledCommand::Status {
            json: has_flag(rest, "--json"),
        })),
        "prepare" | "download" | "start" => {
            let variant = positional(rest)
                .first()
                .cloned()
                .unwrap_or_else(|| "standard".to_owned());
            Ok(Command::Bundled(BundledCommand::Prepare {
                variant,
                yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
            }))
        }
        "path" | "where" => Ok(Command::Bundled(BundledCommand::Path)),
        "help" => Err(format!("{}{}", HELP_MARKER, bundled_help())),
        other => Err(format!(
            "Unknown bundled command: {other}\n\n{}",
            bundled_help()
        )),
    }
}

fn parse_ollama(args: &[String]) -> Result<Command, String> {
    let Some(sub) = args.first().map(String::as_str) else {
        return Ok(Command::Ollama(OllamaCommand::Status { json: false }));
    };
    let rest = &args[1..];
    match sub {
        "status" => Ok(Command::Ollama(OllamaCommand::Status {
            json: has_flag(rest, "--json"),
        })),
        "list" | "ls" => Ok(Command::Ollama(OllamaCommand::List {
            json: has_flag(rest, "--json"),
        })),
        "pull" | "install" => Ok(Command::Ollama(OllamaCommand::Pull {
            tag: positional(rest)
                .first()
                .cloned()
                .ok_or_else(|| "Usage: relay-cli ollama pull <model> [--yes]".to_owned())?,
            yes: has_flag(rest, "--yes") || has_flag(rest, "-y"),
        })),
        "serve" | "start" => Ok(Command::Ollama(OllamaCommand::Serve)),
        "help" => Err(format!("{}{}", HELP_MARKER, ollama_help())),
        other => Err(format!(
            "Unknown ollama command: {other}\n\n{}",
            ollama_help()
        )),
    }
}

fn parse_reasoning(value: &str) -> Result<ReasoningEffort, String> {
    ReasoningEffort::ALL
        .iter()
        .copied()
        .find(|effort| effort.label().eq_ignore_ascii_case(value.trim()))
        .ok_or_else(|| "Reasoning must be one of: off, low, medium, high, max".to_owned())
}

fn parse_temperature(value: &str) -> Result<f64, String> {
    let parsed: f64 = value
        .trim()
        .parse()
        .map_err(|_| "Temperature must be a number between 0 and 2".to_owned())?;
    if !parsed.is_finite() || !(0.0..=2.0).contains(&parsed) {
        return Err("Temperature must be between 0 and 2".to_owned());
    }
    Ok(parsed)
}

fn parse_max_output_tokens(value: &str) -> Result<u32, String> {
    let parsed: u32 = value
        .trim()
        .parse()
        .map_err(|_| "Max output must be an integer".to_owned())?;
    if parsed > 1_000_000 {
        return Err("Max output must be 1,000,000 or less".to_owned());
    }
    Ok(parsed)
}

fn parse_interaction(value: &str) -> Result<String, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "chat" => Ok("chat".to_owned()),
        "work" => Ok("work".to_owned()),
        _ => Err("Mode must be `chat` or `work`".to_owned()),
    }
}

fn parse_approval(value: &str) -> Result<ToolApproval, String> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "none" | "ask" => Ok(ToolApproval::None),
        "non-destructive" | "safe" | "nondestructive" => Ok(ToolApproval::NonDestructive),
        "all" | "auto" => Ok(ToolApproval::All),
        _ => Err("Approval must be none, non-destructive, or all".to_owned()),
    }
}

fn split_option(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((name, value)) => (name, Some(value)),
        None => (arg, None),
    }
}

fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == flag)
}

fn option_value(args: &[String], name: &str) -> Option<String> {
    for (index, arg) in args.iter().enumerate() {
        if arg == name {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
            return Some(value.to_owned());
        }
    }
    None
}

fn positional(args: &[String]) -> Vec<String> {
    let mut values = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--" {
            values.extend(args[index + 1..].iter().cloned());
            break;
        }
        if arg.starts_with('-') {
            // Skip the value of flags that take one.
            if matches!(
                arg.as_str(),
                "--limit"
                    | "--model"
                    | "--output"
                    | "--instructions"
                    | "--instructions-file"
                    | "--name"
                    | "--description"
                    | "--env"
            ) {
                index += 1;
            }
            index += 1;
            continue;
        }
        values.push(arg.clone());
        index += 1;
    }
    values
}

pub fn main_help() -> &'static str {
    "\
Relay CLI — chat with every model, manage your history, and run tools from the terminal.

Usage:
  relay-cli                               Open an interactive chat
  relay-cli \"your question\"               Ask once and print the answer
  relay-cli -p \"question\" --json          Machine-readable one-shot answer
  relay-cli chat --new                    Start a fresh conversation

Examples:
  relay-cli \"summarize this repo\"
  relay-cli chat --reasoning high --temperature 0.2
  relay-cli -p \"write a haiku\" --max-output 256 --preset creative
  relay-cli models
  relay-cli sessions list
  relay-cli settings set temperature 0.4
  relay-cli keys set gemini
  relay-cli bundled status
  relay-cli clear

Type `relay-cli help <command>` for details, for example `relay-cli help sessions`.

Commands:
  sessions   list, search, show, new, rename, pin, archive, model, delete, export
  models     List cloud and local models (ready + installable)
  model      Show or set the default model
  settings   Show, get, set, reset preferences
  keys       List, set, or clear API keys (sealed in the OS keyring)
  presets    List, show, use, create, edit, duplicate, delete personalities
  memory     List, add, delete, or summarize cross-session memories
  mcp        List servers/tools, add/remove servers, call a tool
  bundled    Manage bundled Nemotron (status, prepare, path)
  ollama     Check Ollama, list or pull models, run the daemon
  web        Search the web (DuckDuckGo, keyless)
  fetch      Fetch a URL as readable text
  shell      Open your shell (same terminal capability as the app)
  clear      Clear the terminal view (alias: relay-clear)
  status     Connectivity, model, keys, storage, and local runtime status
  about      Version, license, privacy guarantees, project URL
  help       Show this help; `help <command>` for details

Chat options:
  -m, --model <slug>        Model for this chat
  -s, --session <id|last>   Resume a conversation
  -n, --new                 Start a new conversation
  -r, --reasoning <level>   off | low | medium | high | max
      --temperature <0-2>   Sampling temperature (like the desktop Settings panel)
      --max-output <tokens> Output cap; 0 uses the model default
      --preset <id>         Personality preset
      --instructions <text> Custom instructions for every conversation
      --instructions-file <file>  Read custom instructions from a file
      --image <path>             Attach an image (repeatable)
      --mode <chat|work>    Chat is web-only; Work enables file/command/MCP tools
      --plan / --build      Read-only Plan or writable Build mode (Work)
  -a, --approval <policy>   none | non-destructive | all
  -y, --yes                 Confirm installs/downloads non-interactively
      --json                Machine-readable output
      --thinking            Show reasoning summaries (--show-thinking)
      --no-thinking         Hide reasoning summaries (--hide-thinking)

Options are saved to the same settings the desktop app uses, so the next run
keeps them. Use `relay-cli settings set` for a persistent value without a chat.
Some providers ignore temperature while reasoning is active, matching the
desktop app.

Environment:
  RELAY_DATA_DIR       Override the data directory
  RELAY_MASTER_KEY     Master key for encrypted storage (64 hex chars)
  RUST_LOG             Optional tracing filter
"
}

pub fn chat_help() -> &'static str {
    "\
Usage: relay-cli [chat] [options] [prompt...]

Interactive commands:
  /help                 Show chat commands
  /new                  Start a new conversation
  /sessions [query]     List or search conversations
  /open <id|index>      Open a conversation (index from /sessions)
  /show [n]             Show transcript (all, or the last n messages)
  /edit <n> <text>      Edit a user message, delete later turns, regenerate
  /retry                Regenerate the last turn
  /model [slug]         Show or switch the conversation's model
  /models               List all models
  /mode [chat|work]     Switch interaction mode
  /plan | /build        Switch Plan/Build agent mode
  /reasoning [level]    Show or set reasoning effort
  /temperature [value]  Show or set sampling temperature (0.0-2.0)
  /max-output [tokens|default]  Show or set the output cap
  /instructions [text|clear|file <path>]  Show or set custom instructions
  /approval [policy]    Show or set tool approval policy
  /thinking [on|off]    Show or hide reasoning
  /rename <title>       Rename the current conversation
  /pin | /unpin         Pin or unpin the current conversation
  /archive              Archive the current conversation
  /delete               Delete the current conversation (asks first)
  /export [file]        Export the conversation as Markdown
  /copy [n|last]        Copy a message to the clipboard
  /clear                Clear the screen and show the header again
  /settings             Show settings
  /presets              List personality presets
  /preset <id>          Use a personality preset
  /memory [list|add|delete]  Manage cross-session memories
  /tools                List tools active in this mode
  /web <query>          Search the web directly
  /fetch <url>          Fetch a page directly
  /mcp                  List configured MCP tools
  /status               Runtime/connectivity status
  /quit                 Exit

While Relay is responding, Ctrl+C stops the response and keeps partial text.
End a line with \\ for a multi-line prompt.
"
}

pub fn sessions_help() -> &'static str {
    "\
Usage: relay-cli sessions <command>

  list [--archived] [--limit N] [--json]
  search <query> [--json]
  show <id> [--thinking] [--json]
  new [--model <slug>]
  rename <id> <new title>
  pin|unpin <id>
  archive|unarchive <id>
  model <id> <slug>
  delete <id...> [--yes] | delete --all [--yes]
  export <id> [--output file] [--copy] | export --all [--output file]

Ids may be full UUIDs or unique prefixes.
"
}

pub fn settings_help() -> &'static str {
    "\
Usage: relay-cli settings [show|get|set|reset|path]

  show [--json]
  get <key>
  set <key> <value>
  reset [--yes]
  path

Keys: default_model, appearance, agent_mode, tool_approval, interaction_mode,
reasoning, temperature, max_output_tokens, custom_instructions, active_preset_id,
show_thinking, send_on_enter, ollama_base_url, ollama_model, local_base_url,
local_model, bundled_model_variant, bundled_context_size, mcp_servers (JSON),
font_family, mono_font_family, font_size, color_preset.

Aliases: max_output, reasoning_effort, instructions, preset, thinking.
Temperature accepts 0-2; max_output_tokens accepts 0 (model default) through
1,000,000. Values live in the same settings store as the desktop app.
"
}

pub fn keys_help() -> &'static str {
    "\
Usage: relay-cli keys <list|set|clear> [provider] [key]

Providers: gemini, anthropic, openai, local.
Keys are sealed with ChaCha20-Poly1305 and stored in the OS keyring. If no key
is passed to `set`, Relay prompts without echoing it. Environment variables are
used as a fallback.
"
}

pub fn presets_help() -> &'static str {
    "\
Usage: relay-cli presets <list|show|use|create|edit|duplicate|delete>

  list [--json]
  show <id> [--json]
  use <id>
  create <name> [description] [instructions]
  edit <id> [--name ...] [--description ...] [--instructions ...|--instructions-file file]
  duplicate <id> [new name]
  delete <id> [--yes]

Built-in presets are read-only; duplicate one to customize it.
"
}

pub fn memory_help() -> &'static str {
    "\
Usage: relay-cli memory <list|add|delete|summarize>

  list [--json]
  add <text>
  delete <id> [--yes]
  summarize [--force]

Automatic summaries use a local model only, at most once every 10 hours.
"
}

pub fn mcp_help() -> &'static str {
    "\
Usage: relay-cli mcp <list|tools|call|add|remove|reload>

  list [--json]
  tools [--json]
  call <mcp__server__tool> [json-arguments]
  add <name> <command> [-- <args...>] [--env KEY=VALUE]
  remove <name> [--yes]
  reload

MCP servers use the same stdio configuration stored in Relay settings.
"
}

pub fn bundled_help() -> &'static str {
    "\
Usage: relay-cli bundled <status|prepare|path>

  status [--json]
  prepare [standard|uncensored] [--yes]
  path

Downloads are strictly opt-in. `prepare` starts llama-server and keeps it
running until Ctrl+C; `prepare --yes` confirms the download non-interactively.
"
}

pub fn ollama_help() -> &'static str {
    "\
Usage: relay-cli ollama <status|list|pull|serve>

  status [--json]
  list [--json]
  pull <model> [--yes]
  serve

Relay starts `ollama serve` automatically when a local model is selected and
the daemon is not already running.
"
}

pub fn help_for(topic: &str) -> &'static str {
    match topic {
        "chat" | "prompt" | "ask" => chat_help(),
        "sessions" | "history" => sessions_help(),
        "settings" | "config" => settings_help(),
        "keys" => keys_help(),
        "presets" => presets_help(),
        "memory" => memory_help(),
        "mcp" => mcp_help(),
        "bundled" => bundled_help(),
        "ollama" => ollama_help(),
        "clear" | "cls" | "relay-clear" => clear_help(),
        _ => main_help(),
    }
}

pub fn clear_help() -> &'static str {
    "\
Usage: relay-cli clear    (also: relay-clear)

Clears the visible screen and scrollback without touching your conversation.
Inside the interactive chat, /clear does the same and reprints the header.
"
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;

    #[test]
    fn parses_bare_prompt() {
        let command = parse_from(vec!["hello".to_owned(), "world".to_owned()]).expect("parse");
        match command {
            Command::Prompt(_, prompt) => assert_eq!(prompt, "hello world"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_chat_flags() {
        let command = parse_from(vec![
            "chat".to_owned(),
            "--mode".to_owned(),
            "chat".to_owned(),
            "--reasoning=high".to_owned(),
            "--new".to_owned(),
        ])
        .expect("parse");
        match command {
            Command::Chat(options) => {
                assert_eq!(options.interaction_mode.as_deref(), Some("chat"));
                assert_eq!(options.reasoning, Some(ReasoningEffort::High));
                assert!(options.new);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_generation_flags() {
        let command = parse_from(vec![
            "chat".to_owned(),
            "--temperature".to_owned(),
            "0.25".to_owned(),
            "--max-output".to_owned(),
            "512".to_owned(),
            "--preset".to_owned(),
            "creative".to_owned(),
            "--instructions".to_owned(),
            "Be concise.".to_owned(),
            "--show-thinking".to_owned(),
        ])
        .expect("parse");
        match command {
            Command::Chat(options) => {
                assert_eq!(options.temperature, Some(0.25));
                assert_eq!(options.max_output_tokens, Some(512));
                assert_eq!(options.preset.as_deref(), Some("creative"));
                assert_eq!(options.instructions.as_deref(), Some("Be concise."));
                assert_eq!(options.show_thinking, Some(true));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn parses_prompt_flags_after_prompt() {
        let command = parse_from(vec![
            "chat".to_owned(),
            "-p".to_owned(),
            "hello world".to_owned(),
            "--json".to_owned(),
            "--temperature".to_owned(),
            "0.2".to_owned(),
        ])
        .expect("parse");
        match command {
            Command::Prompt(options, prompt) => {
                assert_eq!(prompt, "hello world");
                assert!(options.json);
                assert_eq!(options.temperature, Some(0.2));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn rejects_invalid_generation_flags() {
        let temperature = parse_from(vec![
            "chat".to_owned(),
            "--temperature".to_owned(),
            "3".to_owned(),
        ]);
        assert!(temperature.is_err());

        let output = parse_from(vec![
            "chat".to_owned(),
            "--max-output".to_owned(),
            "1000001".to_owned(),
        ]);
        assert!(output.is_err());
    }

    #[test]
    fn parses_clear_command() {
        assert!(matches!(
            parse_from(vec!["clear".to_owned()]).expect("parse"),
            Command::Clear
        ));
        assert!(matches!(
            parse_from(vec!["relay-clear".to_owned()]).expect("parse"),
            Command::Clear
        ));
    }

    #[test]
    fn parses_session_delete_all() {
        let command = parse_from(vec![
            "sessions".to_owned(),
            "delete".to_owned(),
            "--all".to_owned(),
            "--yes".to_owned(),
        ])
        .expect("parse");
        match command {
            Command::Sessions(SessionsCommand::Delete { all, yes, .. }) => {
                assert!(all);
                assert!(yes);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
