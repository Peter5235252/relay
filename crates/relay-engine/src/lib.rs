//! Headless Relay engine.
//!
//! This crate owns everything the desktop app and the CLI must do without a
//! window: provider request assembly, agent tool execution, web access, MCP
//! servers, encrypted credentials, bundled local models, and cross-session
//! memory. Keeping these pieces here is what makes the CLI a true peer of the
//! desktop application instead of a reimplementation.

#![forbid(unsafe_code)]

pub mod agent;
pub mod bundled;
pub mod keys;
pub mod local;
pub mod mcp;
pub mod memory;
pub mod search;
pub mod streaming;
pub mod vault;
