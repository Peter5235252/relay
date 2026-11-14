//! Relay desktop application.

mod app;
mod bundled;
mod chat;
mod fonts;
mod keys;
mod models;
mod settings;
mod sidebar;
mod streaming;
mod terminal;
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

    let store = match Store::open_default() {
        Ok(store) => store,
        Err(error) => {
            eprintln!("Relay could not open its database: {error}");
            return ExitCode::FAILURE;
        }
    };

    let settings = store.load_settings().unwrap_or_default();
    let appearance = settings.appearance;

    let bundled = std::sync::Arc::new(std::sync::Mutex::new(bundled::BundledState::default()));
    {
        let b = bundled.clone();
        handle.spawn(async move {
            bundled::ensure_ready(b).await;
        });
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
