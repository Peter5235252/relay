#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

//! Local-first persistence for Relay.
//!
//! One SQLite database holds conversations, canonical message JSON, full-text
//! search, preferences, and personality presets. API keys never enter this
//! database; they live in the OS keyring.
//!
//! The database uses WAL mode and a single connection guarded by a mutex. Relay
//! is a single-process desktop app, so this keeps writes serialized without an
//! extra actor or background thread.

#![forbid(unsafe_code)]

mod demo;
mod error;
mod store;

pub use demo::seed_demo;
pub use error::{Result, StoreError};
pub use store::{AppendOutcome, Store, StoreStats, default_data_dir, default_database_path};
