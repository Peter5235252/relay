//! Reference client for the Relay daemon.

use std::path::Path;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
#[cfg(unix)]
use tokio::net::UnixStream;

use crate::protocol::{
    DaemonInfo, Endpoint, PROTOCOL_VERSION, RpcError, RpcNotification, RpcResponse,
    default_runtime_dir, read_info_from, read_token_from,
};

use crate::BoxStream;

/// A synchronous request/notification client over one daemon connection.
pub struct Client {
    reader: BufReader<ReadHalf<BoxStream>>,
    writer: WriteHalf<BoxStream>,
    next_id: u64,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("next_id", &self.next_id)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Connect to the daemon in the default runtime directory and initialize.
    pub async fn connect() -> Result<Self, String> {
        Self::connect_dir(&default_runtime_dir()?).await
    }

    /// Connect to the daemon in a specific runtime directory and initialize.
    pub async fn connect_dir(dir: &Path) -> Result<Self, String> {
        let info = read_info_from(dir)?;
        let token = read_token_from(dir)?;
        Self::connect_info(&info, &token).await
    }

    /// Connect with an already-read discovery document and token.
    pub async fn connect_info(info: &DaemonInfo, token: &str) -> Result<Self, String> {
        let stream: BoxStream = match &info.endpoint {
            #[cfg(unix)]
            Endpoint::Unix { path } => Box::new(
                UnixStream::connect(path)
                    .await
                    .map_err(|error| format!("could not connect to {path}: {error}"))?,
            ),
            Endpoint::Tcp { address } => Box::new(
                TcpStream::connect(address)
                    .await
                    .map_err(|error| format!("could not connect to {address}: {error}"))?,
            ),
            #[cfg(not(unix))]
            Endpoint::Unix { .. } => {
                return Err(
                    "this client cannot connect to a Unix socket on this platform".to_owned(),
                );
            }
        };
        let (read_half, write_half) = tokio::io::split(stream);
        let mut client = Self {
            reader: BufReader::new(read_half),
            writer: write_half,
            next_id: 1,
        };
        let hello = client
            .call(
                "initialize",
                json!({
                    "token": token,
                    "client": format!("relay-daemon-client/{}", env!("CARGO_PKG_VERSION")),
                }),
            )
            .await?;
        let protocol = hello.get("protocol").and_then(Value::as_u64).unwrap_or(0);
        if protocol != u64::from(PROTOCOL_VERSION) {
            return Err(format!(
                "the daemon speaks protocol {protocol}, but this client needs {PROTOCOL_VERSION}"
            ));
        }
        Ok(client)
    }

    /// Send one request and wait for its response. Notifications are ignored.
    pub async fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.call_with_events(method, params, |_| {}).await
    }

    /// Send one request, invoking `on_event` for every notification that
    /// arrives before the matching response.
    pub async fn call_with_events<F>(
        &mut self,
        method: &str,
        params: Value,
        mut on_event: F,
    ) -> Result<Value, String>
    where
        F: FnMut(&RpcNotification),
    {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }))
        .await?;

        loop {
            match self.read_incoming().await?.classify()? {
                IncomingMessage::Notification(notification) => {
                    on_event(&notification);
                    continue;
                }
                IncomingMessage::Response(response) => {
                    if response.id == json!(id) {
                        return match response.error {
                            Some(error) => Err(format!("{} (code {})", error.message, error.code)),
                            None => Ok(response.result.unwrap_or(Value::Null)),
                        };
                    }
                    continue;
                }
            }
        }
    }

    /// Return the next server notification, waiting if necessary.
    pub async fn next_notification(&mut self) -> Result<Option<RpcNotification>, String> {
        match self.read_incoming().await?.classify()? {
            IncomingMessage::Notification(notification) => Ok(Some(notification)),
            IncomingMessage::Response(response) => Err(format!(
                "unexpected response while waiting for an event (id {})",
                response.id
            )),
        }
    }

    async fn send(&mut self, value: &Value) -> Result<(), String> {
        let line = serde_json::to_string(value).map_err(|error| error.to_string())?;
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(|error| format!("daemon write failed: {error}"))?;
        self.writer
            .write_all(b"\n")
            .await
            .map_err(|error| format!("daemon write failed: {error}"))?;
        self.writer
            .flush()
            .await
            .map_err(|error| format!("daemon write failed: {error}"))
    }

    async fn read_incoming(&mut self) -> Result<Incoming, String> {
        let mut line = String::new();
        let read = self
            .reader
            .read_line(&mut line)
            .await
            .map_err(|error| format!("daemon read failed: {error}"))?;
        if read == 0 {
            return Err("the daemon closed the connection".to_owned());
        }
        serde_json::from_str(line.trim())
            .map_err(|error| format!("invalid daemon message: {error}"))
    }
}

#[derive(Debug, Deserialize)]
struct Incoming {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

enum IncomingMessage {
    Notification(RpcNotification),
    Response(RpcResponse),
}

impl Incoming {
    fn classify(self) -> Result<IncomingMessage, String> {
        let jsonrpc = self.jsonrpc.unwrap_or_else(|| "2.0".to_owned());
        if let Some(method) = self.method {
            if self.id.is_some() {
                return Err("the daemon sent an unsupported server request".to_owned());
            }
            return Ok(IncomingMessage::Notification(RpcNotification {
                jsonrpc,
                method,
                params: self.params.unwrap_or(Value::Null),
            }));
        }
        if let Some(id) = self.id {
            return Ok(IncomingMessage::Response(RpcResponse {
                jsonrpc,
                id,
                result: self.result,
                error: self.error,
            }));
        }
        Err("daemon sent a message with neither a method nor an id".to_owned())
    }
}
