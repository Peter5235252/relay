//! Wire types and discovery helpers for the Relay daemon.
//!
//! The transport is newline-delimited JSON-RPC 2.0. On Unix the endpoint is a
//! private Unix-domain socket; on Windows it is a loopback TCP socket. In both
//! cases the client must read the discovery file and authenticate with the
//! token file before calling anything except `ping` and `initialize`.

use std::path::{Path, PathBuf};

use relay_core::{ConversationSummary, Usage};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Version of the daemon protocol. Bump this for incompatible wire changes.
pub const PROTOCOL_VERSION: u32 = 1;

/// Unix socket file inside the runtime directory.
pub const SOCKET_FILE: &str = "daemon.sock";
/// Discovery file inside the runtime directory.
pub const INFO_FILE: &str = "daemon.json";
/// Authentication token file inside the runtime directory.
pub const TOKEN_FILE: &str = "daemon.token";

/// One JSON-RPC request or notification.
#[derive(Debug, Clone, Deserialize)]
pub struct RpcRequest {
    /// Always `"2.0"` for conforming clients.
    #[serde(default)]
    pub jsonrpc: Option<String>,
    /// Omitted for notifications.
    #[serde(default)]
    pub id: Option<Value>,
    /// Method name.
    pub method: String,
    /// Method parameters.
    #[serde(default)]
    pub params: Option<Value>,
}

/// A JSON-RPC error object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcError {
    /// Numeric error code.
    pub code: i64,
    /// Human-readable message.
    pub message: String,
    /// Optional structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    /// Build an error with an explicit code.
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Attach structured detail.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Invalid JSON.
    pub fn parse(message: impl Into<String>) -> Self {
        Self::new(-32700, message)
    }

    /// Structurally invalid request.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(-32600, message)
    }

    /// Unknown method.
    pub fn method_not_found(method: &str) -> Self {
        Self::new(-32601, format!("unknown method: {method}"))
    }

    /// Wrong or missing parameters.
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(-32602, message)
    }

    /// Unexpected server failure.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(-32603, message)
    }

    /// The connection has not completed `initialize`.
    pub fn unauthorized() -> Self {
        Self::new(
            -32001,
            "authentication required: call initialize with the daemon token",
        )
    }

    /// A requested conversation, turn, or approval does not exist.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(-32002, message)
    }

    /// A conflicting operation, such as two turns in one conversation.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(-32003, message)
    }

    /// The selected model cannot run yet.
    pub fn model_not_ready(message: impl Into<String>) -> Self {
        Self::new(-32004, message)
    }

    /// The operation exceeded its deadline.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(-32005, message)
    }
}

/// A JSON-RPC success or error response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Echo of the request id.
    pub id: Value,
    /// Result on success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error on failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

impl RpcResponse {
    /// Build a success response.
    pub fn result(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Build an error response.
    pub fn error(id: Value, error: RpcError) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// A server-initiated JSON-RPC notification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcNotification {
    /// Always `"2.0"`.
    pub jsonrpc: String,
    /// Event name.
    pub method: String,
    /// Event payload.
    pub params: Value,
}

impl RpcNotification {
    /// Build a notification.
    pub fn new(method: impl Into<String>, params: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_owned(),
            method: method.into(),
            params,
        }
    }
}

/// How clients reach the daemon.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "transport", rename_all = "snake_case")]
pub enum Endpoint {
    /// Private Unix-domain socket.
    Unix {
        /// Absolute socket path.
        path: String,
    },
    /// Loopback TCP socket, used on Windows.
    Tcp {
        /// `host:port` address.
        address: String,
    },
}

impl Endpoint {
    /// Human-readable endpoint for logs.
    pub fn display(&self) -> String {
        match self {
            Self::Unix { path } => format!("unix:{path}"),
            Self::Tcp { address } => format!("tcp:{address}"),
        }
    }
}

/// Discovery document written next to the socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonInfo {
    /// Wire protocol version.
    pub protocol: u32,
    /// Relay version that started the daemon.
    pub version: String,
    /// Process id.
    pub pid: u32,
    /// RFC 3339 start time.
    pub started_at: String,
    /// Connection endpoint.
    pub endpoint: Endpoint,
    /// Directory that holds the encrypted database.
    pub data_dir: String,
}

/// Result of one completed assistant turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnResult {
    /// Server-assigned turn id.
    pub turn_id: String,
    /// Conversation the turn belongs to.
    pub conversation_id: String,
    /// Relay model slug used for the turn.
    pub model: String,
    /// Visible answer text.
    pub text: String,
    /// Reasoning text, even when the client does not display it.
    pub thinking: String,
    /// Provider-reported token usage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Estimated list-price cost in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    /// Whether the turn was cancelled.
    pub cancelled: bool,
    /// Provider or persistence error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Stored assistant message id, when the turn produced content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Refreshed conversation summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<ConversationSummary>,
}

/// Immediate response to `chat.start`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartedTurn {
    /// Server-assigned turn id.
    pub turn_id: String,
    /// Conversation the turn belongs to.
    pub conversation_id: String,
    /// Relay model slug used for the turn.
    pub model: String,
}

/// Resolve the directory that holds the socket, discovery file, and token.
///
/// `RELAY_DAEMON_DIR` wins. On Unix the daemon prefers
/// `$XDG_RUNTIME_DIR/relay`; elsewhere it falls back to the Relay data
/// directory.
pub fn default_runtime_dir() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("RELAY_DAEMON_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() {
            return Ok(dir);
        }
    }

    #[cfg(unix)]
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let dir = PathBuf::from(dir);
        if !dir.as_os_str().is_empty() {
            return Ok(dir.join("relay"));
        }
    }

    let data = relay_store::default_data_dir().map_err(|error| error.to_string())?;
    Ok(data.join("daemon"))
}

/// Path of the discovery document.
pub fn info_path(dir: &Path) -> PathBuf {
    dir.join(INFO_FILE)
}

/// Path of the authentication token.
pub fn token_path(dir: &Path) -> PathBuf {
    dir.join(TOKEN_FILE)
}

/// Path of the Unix socket.
#[cfg(unix)]
pub fn socket_path(dir: &Path) -> PathBuf {
    dir.join(SOCKET_FILE)
}

/// Read the discovery document from a runtime directory.
pub fn read_info_from(dir: &Path) -> Result<DaemonInfo, String> {
    let path = info_path(dir);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    serde_json::from_str(&text)
        .map_err(|error| format!("invalid daemon discovery file {}: {error}", path.display()))
}

/// Read the discovery document from the default runtime directory.
pub fn read_info() -> Result<DaemonInfo, String> {
    read_info_from(&default_runtime_dir()?)
}

/// Read the daemon token from a runtime directory.
pub fn read_token_from(dir: &Path) -> Result<String, String> {
    let path = token_path(dir);
    let token = std::fs::read_to_string(&path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let token = token.trim().to_owned();
    if token.is_empty() {
        return Err(format!("daemon token file {} is empty", path.display()));
    }
    Ok(token)
}

/// Read the daemon token from the default runtime directory.
pub fn read_token() -> Result<String, String> {
    read_token_from(&default_runtime_dir()?)
}

/// Write a file readable only by the current user where the platform supports
/// Unix permissions.
pub(crate) fn write_private(path: &Path, contents: &[u8]) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};

        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
        // `mode` only applies when the file is created, so tighten a stale one.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("could not restrict {}: {error}", path.display()))?;
        file.write_all(contents)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, contents)
            .map_err(|error| format!("could not write {}: {error}", path.display()))?;
    }
    Ok(())
}
