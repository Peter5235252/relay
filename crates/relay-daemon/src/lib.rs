#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

//! Local JSON-RPC daemon for Relay.
//!
//! `relay --daemon` serves the same encrypted store, settings, model catalog,
//! and streaming engine as the desktop app and CLI. Native clients connect to
//! a private Unix socket (or a loopback TCP socket on Windows), authenticate
//! with a per-run token, and exchange newline-delimited JSON-RPC 2.0 messages.
//!
//! The GPUI application remains the default front end. This crate only adds a
//! headless transport so other native clients can share the engine instead of
//! reimplementing provider, tool, and storage behavior.

#![forbid(unsafe_code)]

pub mod client;
mod protocol;
mod server;

/// Any full-duplex byte stream the daemon can serve.
pub(crate) trait Transport:
    tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send
{
}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Transport for T {}

/// Boxed transport used for accepted connections and reference clients.
pub(crate) type BoxStream = Box<dyn Transport>;

pub use protocol::{
    DaemonInfo, Endpoint, INFO_FILE, PROTOCOL_VERSION, RpcError, RpcNotification, RpcRequest,
    RpcResponse, SOCKET_FILE, TOKEN_FILE, TurnResult, default_runtime_dir, info_path, read_info,
    read_info_from, read_token, read_token_from, token_path,
};
pub use server::{DaemonOptions, run};
