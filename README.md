# Relay

Relay is a memory-safe Rust desktop agentic work and chat client that unifies Gemini, Claude,
GPT, Ollama, and OpenAI-compatible models behind a single interface.
Built with [GPUI](https://github.com/zed-industries/zed).

## Features

- Unified model switcher across Gemini, Claude, GPT, Ollama, and local
  OpenAI-compatible endpoints.
- Conversation history with full-text search, grouped sidebar.
- Per-conversation model selection and app-global reasoning effort.
- Chat and Work mode toggles.
- Settings, cross-session memory, custom instructions, and personality presets.
- Markdown transcript, usage and cost tracking, Markdown export.
- SQLCipher-encrypted conversation history, local full-text search.
- API keys sealed with ChaCha20-Poly1305 before storage in the OS keyring
  (never stored in the database).

## Security

Relay keeps all conversation data in a single SQLCipher database under the
local data directory. On first launch Relay generates a random 256-bit master
key, stores it in the OS keyring, and derives separate keys with HKDF-SHA256 for
the database and for sealing API keys. Existing plaintext databases and
plaintext keyring entries are migrated automatically.

For headless environments without a keyring, set `RELAY_MASTER_KEY` to 64 hex
characters. Keep that value secret: it unlocks the local database.

## Building

```sh
cargo build -p relay-ui --release
```

The binary is `relay`.

## Command line

The terminal client shares the same engine, settings store, history, keys, and
personality presets as the desktop app. Generation controls match the desktop
Settings panel and persist for later runs.

```sh
cargo build -p relay-cli --release
./target/release/relay-cli

relay-cli "explain this repository"
relay-cli chat --reasoning high --temperature 0.3 --max-output 1024
relay-cli -p "write a haiku" --preset concise --json
relay-cli settings set temperature 0.7
relay-cli models
```

`--max-output 0` uses the model's default output limit. `relay-cli help chat`
lists every flag, and `/help` inside the interactive client lists slash
commands such as `/temperature`, `/max-output`, and `/instructions`.

Linux builds only; macOS and Windows builds are not available.

## License

Apache-2.0. See [LICENSE](LICENSE).
