//! Relay desktop application.

// Release builds are windowed apps on Windows: no console window appears when
// the user launches Relay from Explorer or the Start Menu.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bundled;
mod chat;
mod fonts;
mod keys;
mod mcp;
mod models;
mod search;
mod settings;
mod sidebar;
mod streaming;
mod terminal;
mod theme;
mod vault;

use std::process::ExitCode;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, WindowBounds, WindowDecorations, WindowOptions, px, size};
use relay_store::Store;

use crate::app::RelayApp;

const HELP: &str = "\
Relay — one conversation, every frontier model.

Usage: relay [OPTIONS]

Options:
  --version      Print the version and exit
  --daemon       Serve the shared engine over local IPC (no window)
  --background   Alias for --daemon
  -h, --help     Print this help and exit

Environment:
  RELAY_DATA_DIR       Override the data directory
  RELAY_DAEMON_DIR     Override the daemon runtime directory
                       (Unix default: $XDG_RUNTIME_DIR/relay)
  RELAY_MASTER_KEY     Master key for encrypted storage (64 hex characters;
                       headless fallback when no OS keyring is available)
  GEMINI_API_KEY       Fallback for the Gemini key
  ANTHROPIC_API_KEY    Fallback for the Anthropic key
  OPENAI_API_KEY       Fallback for the OpenAI key
  RELAY_LOCAL_API_KEY  Optional bearer token for a local endpoint
  RUST_LOG             Log filter (default: relay_ui=info,warn)
";

/// Keep newly created databases, journals, and model downloads private.
#[cfg(unix)]
fn restrict_umask() {
    rustix::process::umask(rustix::fs::Mode::from_bits_truncate(0o077));
}

#[cfg(not(unix))]
fn restrict_umask() {}

fn main() -> ExitCode {
    if let Some(argument) = std::env::args().nth(1) {
        match argument.as_str() {
            "--version" | "-V" => {
                println!("relay {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            "--daemon" | "--background" => {
                // handled later; fall through
            }
            other => {
                eprintln!("Unknown option: {other}\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
    }

    restrict_umask();

    // Explorer and Finder launch apps with an unrelated working directory
    // (`C:\Windows\System32`, `/`). Run from the user's home so model tools and
    // relative paths behave like the integrated terminal.
    if let Some(home) = relay_core::platform::home_dir() {
        let _ = std::env::set_current_dir(home);
    }

    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "relay_ui=info,warn".into());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("relay-net")
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Relay could not start its network runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let handle = runtime.handle().clone();

    let vault = match vault::Vault::init() {
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
    let _appearance = settings.appearance;

    // Initialize MCP servers (stdio) from settings.
    mcp::init_global(&settings.mcp_servers);

    let bundled = std::sync::Arc::new(std::sync::Mutex::new(bundled::BundledState::default()));

    // Daemon / background mode: serve the shared engine over local IPC and
    // run without a window. Native clients discover the endpoint through the
    // runtime directory written by `relay-daemon`.
    let daemon = std::env::args().any(|a| a == "--daemon" || a == "--background");
    if daemon {
        let result = runtime.block_on(relay_daemon::run(relay_daemon::DaemonOptions {
            store,
            settings,
            runtime: handle,
            bundled,
            runtime_dir: None,
            token: None,
        }));
        drop(runtime);
        return match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("Relay daemon stopped with an error: {error}");
                ExitCode::FAILURE
            }
        };
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            fonts::register(cx);
            theme::install_typography(cx, &settings);
            theme::apply_colors(&settings, cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1280.), px(840.)), cx)),
                window_min_size: Some(size(px(980.), px(620.))),
                app_id: Some("io.github.csemanpeter.Relay".into()),
                window_decorations: Some(WindowDecorations::Client),
                ..TitleBar::window_options()
            };

            let store = store.clone();
            let handle = handle.clone();
            let bundled = bundled.clone();
            cx.activate(true);
            let settings_for_window = settings.clone();
            match gpui_kit::open_window(options, cx, move |window, cx| {
                theme::apply_theme(&settings_for_window, window, cx);
                let store = store.clone();
                let handle = handle.clone();
                let bundled = bundled.clone();
                cx.new(|cx| RelayApp::new(store, handle, bundled, window, cx))
            }) {
                Ok((window, _)) => {
                    let _ = window.update(cx, |_, window, _| window.activate_window());
                }
                Err(error) => {
                    eprintln!("Relay could not open its window: {error}");
                }
            }
        });

    drop(runtime);
    ExitCode::SUCCESS
}
