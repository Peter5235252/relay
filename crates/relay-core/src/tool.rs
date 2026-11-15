//! Tool definitions available to the agent.

use serde::{Deserialize, Serialize};

/// Agent operating mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentMode {
    #[default]
    Plan,
    Build,
}

impl AgentMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Build => "build",
        }
    }
    pub const fn label(self) -> &'static str {
        match self {
            Self::Plan => "Plan",
            Self::Build => "Build",
        }
    }
    pub fn allows_writes(self) -> bool {
        matches!(self, Self::Build)
    }
}

/// Auto-approval policy for tool execution in Build mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolApproval {
    /// Ask before every tool/command.
    #[default]
    None,
    /// Auto-approve read-only and non-destructive actions; ask for destructive ones.
    NonDestructive,
    /// Auto-approve everything (dangerous).
    All,
}

impl ToolApproval {
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Auto-approve none",
            Self::NonDestructive => "Auto-approve non-destructive",
            Self::All => "Auto-approve all",
        }
    }
}

/// A tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case")]
pub enum Tool {
    ReadFile {
        path: String,
    },
    ListDir {
        path: String,
    },
    RunCommand {
        command: String,
    },
    WriteFile {
        path: String,
        content: String,
    },
    /// Delegate a subtask to another configured model/provider (including local).
    Delegate {
        model: String,
        prompt: String,
    },
}

impl Tool {
    pub fn name(&self) -> &'static str {
        match self {
            Tool::ReadFile { .. } => "read_file",
            Tool::ListDir { .. } => "list_dir",
            Tool::RunCommand { .. } => "run_command",
            Tool::WriteFile { .. } => "write_file",
            Tool::Delegate { .. } => "delegate",
        }
    }
    /// Tools available in the given mode.
    pub fn available(mode: AgentMode) -> Vec<&'static str> {
        match mode {
            // Plan is read-only for local filesystem/commands, but delegation is allowed.
            AgentMode::Plan => vec!["read_file", "list_dir", "delegate"],
            AgentMode::Build => vec![
                "read_file",
                "list_dir",
                "run_command",
                "write_file",
                "delegate",
            ],
        }
    }
}

/// Heuristic: is a shell command likely non-destructive/read-only?
pub fn is_command_nondestructive(cmd: &str) -> bool {
    let c = cmd.trim().to_lowercase();
    let safe_prefixes = [
        "ls",
        "cat ",
        "head ",
        "tail ",
        "grep ",
        "rg ",
        "find ",
        "pwd",
        "echo ",
        "whoami",
        "id",
        "uname",
        "which ",
        "type ",
        "env",
        "printenv",
        "stat ",
        "file ",
        "wc ",
        "du ",
        "df ",
        "tree",
        "git status",
        "git log",
        "git diff",
        "git show",
        "git branch",
        "cargo --version",
        "rustc --version",
        "python3 --version",
        "node --version",
    ];
    if c.is_empty() {
        return false;
    }
    // obvious destructive patterns
    let bad = [
        " rm ",
        "rm ",
        "sudo",
        "pkexec",
        "mv ",
        "cp ",
        "chmod",
        "chown",
        "dd ",
        "mkfs",
        "shutdown",
        "reboot",
        "kill",
        "pkill",
        ">",
        ">>",
        "tee ",
        "curl ",
        "wget ",
        "pip install",
        "npm install",
        "apt",
        "pacman",
        "dnf",
        "systemctl",
    ];
    for b in bad {
        if c.contains(b) || c.starts_with(b.trim()) {
            return false;
        }
    }
    safe_prefixes.iter().any(|p| c == *p || c.starts_with(p))
}

/// Should this tool call be auto-approved given mode/policy?
pub fn should_auto_approve(
    mode: AgentMode,
    policy: ToolApproval,
    name: &str,
    args: &serde_json::Value,
) -> bool {
    match mode {
        AgentMode::Plan => matches!(name, "read_file" | "list_dir" | "delegate"),
        AgentMode::Build => match policy {
            ToolApproval::All => true,
            ToolApproval::None => false,
            ToolApproval::NonDestructive => match name {
                "read_file" | "list_dir" => true,
                "run_command" => {
                    let cmd = args.get("command").and_then(|v| v.as_str()).unwrap_or("");
                    is_command_nondestructive(cmd)
                }
                _ => false,
            },
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,
}

/// Configuration for an external MCP (Model Context Protocol) server launched over stdio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct McpServerConfig {
    /// Friendly name used as a prefix for its tools.
    pub name: String,
    /// Executable to spawn.
    pub command: String,
    /// Arguments passed to the executable.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables (KEY=VALUE).
    #[serde(default)]
    pub env: Vec<String>,
}

/// JSON-schema description of a callable tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Build the list of tool specs for the given agent mode.
pub fn tool_specs(mode: AgentMode) -> Vec<ToolSpec> {
    let mut specs = vec![
        ToolSpec {
            name: "read_file".into(),
            description: "Read the contents of a file on the user's machine (read-only).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": {"type":"string","description":"Absolute or relative file path"} },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "list_dir".into(),
            description: "List entries in a directory (read-only).".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "path": {"type":"string","description":"Directory path"} },
                "required": ["path"]
            }),
        },
    ];
    if mode.allows_writes() {
        specs.push(ToolSpec {
            name: "run_command".into(),
            description: "Run a shell command and return its output. Requires user approval unless auto-approved.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": { "command": {"type":"string","description":"Shell command to execute"} },
                "required": ["command"]
            }),
        });
        specs.push(ToolSpec {
            name: "write_file".into(),
            description: "Write text content to a file (creates or overwrites). Requires user approval unless auto-approved.".into(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type":"string"},
                    "content": {"type":"string"}
                },
                "required": ["path","content"]
            }),
        });
    }
    specs.push(ToolSpec {
        name: "delegate".into(),
        description: "Delegate a subtask to another configured model/provider and return its response. Use a model slug (e.g. 'gemini-2.5-pro', 'claude-sonnet-4', 'gpt-4o', 'ollama/<model>', 'local/<model>') or 'ollama'/'local' to use the configured default local model. Works with cloud and local models that are configured/available.".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "model": {"type":"string","description":"Target model slug/name/provider alias"},
                "prompt": {"type":"string","description":"Subtask instructions/prompt for the delegated model"}
            },
            "required": ["model","prompt"]
        }),
    });
    specs
}
