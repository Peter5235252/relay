//! Relay command-line client.

mod args;
mod chat;
mod commands;
mod output;

use std::process::ExitCode;

use crate::args::Command;
use commands::CliContext;
use relay_engine::bundled;
use relay_store::Store;

#[cfg(unix)]
fn restrict_umask() {
    let _ = rustix::process::umask(rustix::fs::Mode::from_bits_truncate(0o077));
}

#[cfg(not(unix))]
fn restrict_umask() {}

fn main() -> ExitCode {
    let command = match args::parse() {
        Ok(command) => command,
        Err(error) => {
            if let Some(help) = args::help_text(&error) {
                print!("{help}");
                return ExitCode::SUCCESS;
            }
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };

    restrict_umask();

    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("relay-cli")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Relay could not start its runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let handle = runtime.handle().clone();

    let vault = match relay_engine::vault::Vault::init() {
        Ok(vault) => vault,
        Err(error) => {
            eprintln!("Relay could not unlock its encryption key: {error}");
            return ExitCode::FAILURE;
        }
    };
    let database_key = match vault.database_key() {
        Ok(key) => key,
        Err(error) => {
            eprintln!("Relay could not derive its database key: {error}");
            return ExitCode::FAILURE;
        }
    };
    let store = match Store::open_default(&database_key) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("Relay could not open its database: {error}");
            return ExitCode::FAILURE;
        }
    };

    let settings = store.load_settings().unwrap_or_default();
    relay_engine::mcp::init_global(&settings.mcp_servers);

    let bundled_state = std::sync::Arc::new(std::sync::Mutex::new(
        relay_engine::bundled::BundledState::default(),
    ));

    let mut ctx = CliContext {
        store,
        settings,
        runtime: handle,
        bundled: bundled_state,
        ollama_child: None,
    };

    if let Command::Help(topic) = command {
        print!("{}", args::help_for(topic.as_deref().unwrap_or("")));
        ctx.shutdown();
        return ExitCode::SUCCESS;
    }
    if let Command::Version = command {
        println!("relay-cli {}", env!("CARGO_PKG_VERSION"));
        ctx.shutdown();
        return ExitCode::SUCCESS;
    }

    let result = runtime.block_on(commands::dispatch(command, &mut ctx));
    ctx.shutdown();

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
