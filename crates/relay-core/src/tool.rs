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
            Self::Plan => "Plan (read-only)",
            Self::Build => "Build (can write)",
        }
    }
    pub fn allows_writes(self) -> bool {
        matches!(self, Self::Build)
    }
}

/// A tool the model may call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name", rename_all = "snake_case")]
pub enum Tool {
    ReadFile { path: String },
    ListDir { path: String },
    RunCommand { command: String },
    WriteFile { path: String, content: String },
}

impl Tool {
    pub fn name(&self) -> &'static str {
        match self {
            Tool::ReadFile { .. } => "read_file",
            Tool::ListDir { .. } => "list_dir",
            Tool::RunCommand { .. } => "run_command",
            Tool::WriteFile { .. } => "write_file",
        }
    }
    /// Tools available in the given mode.
    pub fn available(mode: AgentMode) -> Vec<&'static str> {
        let mut v = vec!["read_file", "list_dir", "run_command"];
        if mode.allows_writes() {
            v.push("write_file");
        }
        v
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub call_id: String,
    pub output: String,
    pub is_error: bool,
}
