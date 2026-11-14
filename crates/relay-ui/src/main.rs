//! Relay desktop application.

mod app;
mod chat;
mod keys;
mod models;
mod settings;
mod sidebar;
mod streaming;
mod theme;

use std::process::ExitCode;

use gpui_kit::component::TitleBar;
use gpui_kit::{AppContext as _, WindowBounds, WindowDecorations, WindowOptions, px, size};
use relay_store::Store;

use crate::app::RelayApp;

const HELP: &str = "\
Relay — one conversation, every frontier model.

Usage: relay [OPTIONS]

Options:
  --demo       Use an in-memory database with sample conversations
  --version    Print the version and exit
  -h, --help   Print this help and exit

Environment:
  RELAY_DATA_DIR       Override the data directory
  GEMINI_API_KEY       Fallback for the Gemini key
  ANTHROPIC_API_KEY    Fallback for the Anthropic key
  OPENAI_API_KEY       Fallback for the OpenAI key
  RELAY_LOCAL_API_KEY  Optional bearer token for a local endpoint
  RUST_LOG             Log filter (default: relay_ui=info,warn)
";

fn main() -> ExitCode {
    let mut demo = false;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--demo" => demo = true,
            "--version" | "-V" => {
                println!("relay {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            "--help" | "-h" => {
                print!("{HELP}");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("Unknown option: {other}\n\n{HELP}");
                return ExitCode::from(2);
            }
        }
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

    let store = if demo {
        let store = match Store::open_in_memory() {
            Ok(store) => store,
            Err(error) => {
                eprintln!("Relay could not create the demo database: {error}");
                return ExitCode::FAILURE;
            }
        };
        if let Err(error) = relay_store::seed_demo(&store) {
            eprintln!("Relay could not seed the demo data: {error}");
            return ExitCode::FAILURE;
        }
        store
    } else {
        match Store::open_default() {
            Ok(store) => store,
            Err(error) => {
                eprintln!("Relay could not open its database: {error}");
                return ExitCode::FAILURE;
            }
        }
    };

    let appearance = store
        .load_settings()
        .map(|settings| settings.appearance)
        .unwrap_or_default();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            theme::install_typography(cx);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::centered(size(px(1280.), px(840.)), cx)),
                window_min_size: Some(size(px(980.), px(620.))),
                app_id: Some("io.github.csemanpeter.Relay".into()),
                window_decorations: Some(WindowDecorations::Client),
                ..TitleBar::window_options()
            };

            let store = store.clone();
            let handle = handle.clone();
            cx.activate(true);
            match gpui_kit::open_window(options, cx, move |window, cx| {
                theme::apply_appearance(appearance, window, cx);
                let store = store.clone();
                let handle = handle.clone();
                cx.new(|cx| RelayApp::new(store, handle, window, cx))
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
