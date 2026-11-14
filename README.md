# Relay

Relay is a memory-safe Rust desktop chat client that unifies Gemini, Claude,
GPT, Ollama, and OpenAI-compatible models behind a single interface.
Built with [GPUI](https://github.com/zed-industries/zed).

## Features

- Unified model switcher across Gemini, Claude, GPT, Ollama, and local
  OpenAI-compatible endpoints.
- Conversation history with full-text search, grouped sidebar.
- Per-conversation model selection and app-global reasoning effort.
- Settings, custom instructions, and personality presets.
- Markdown transcript, usage and cost tracking, Markdown export.
- OS keyring for API keys (never stored in the database).

## Building

```sh
cargo build -p relay-ui --release
```

The binary is `relay`.

## License

Apache-2.0. See [LICENSE](LICENSE).
