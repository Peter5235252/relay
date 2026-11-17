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

## Installation

Relay ships prebuilt binaries for Linux, macOS, and Windows from
[GitHub Releases](https://github.com/Peter5235252/relay/releases).

### Linux (x86_64)

Download `relay-linux-x86_64.tar.gz`, extract it, and run `./relay-linux-x86_64`.
The archive also contains `relay-cli`, `relay-clear`, the `.desktop` entry, and
icons for a system-wide install.

### macOS (Apple Silicon and Intel)

Download `Relay-<version>-macos-universal.dmg`, open it, and drag `Relay` into
`Applications`. The DMG also includes `relay-cli` and `relay-clear` in its
`CLI` folder. The app is a universal binary, ad-hoc signed for both
architectures; on first launch, right-click `Relay` and choose **Open** to get
past Gatekeeper's unidentified-developer warning.

The `.tar.gz` archive is available for scripted installs: it contains
`Relay.app` plus standalone `relay`, `relay-cli`, and `relay-clear` binaries.

### Windows (x64, Windows 10 1809+)

Download `relay-setup-windows-x64.exe` and run the installer, or use
`relay-windows-x64.zip` for a portable copy. The installer adds Start Menu
shortcuts and can put `relay-cli` on your `PATH`. SmartScreen may warn about an
unknown publisher; choose **More info → Run anyway**.

### Arch (AUR-style, from source)

```sh
cd packaging/aur
makepkg -si
```

### Local install from a checkout (Linux)

```sh
cargo build -p relay-ui -p relay-cli --release
./packaging/install-local.sh
```

## Attachments

Relay accepts image attachments (PNG, JPEG, GIF, WebP) in both the desktop
app and the CLI:

- Desktop: click `+`, drag-and-drop files onto the composer, or paste a path.
- CLI: `relay-cli --image ./photo.png "describe this"` or `/image ./photo.png`
  inside the REPL.

Images are stored inside the encrypted SQLCipher database alongside the
conversation.

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

## Platform support

| Platform | Desktop app | CLI | Integrated terminal | Bundled local models |
| --- | --- | --- | --- | --- |
| Linux x86_64 | ✅ | ✅ | bash/zsh | ✅ (Vulkan) |
| Linux arm64 | ✅ | ✅ | bash/zsh | ✅ (CPU) |
| macOS arm64 + x86_64 | ✅ | ✅ | zsh | ✅ (Metal) |
| Windows x64 | ✅ | ✅ | PowerShell / pwsh | ✅ (Vulkan) |
| Windows arm64 | ✅ | ✅ | PowerShell / pwsh | ✅ (CPU) |

On Windows the desktop terminal, `relay-cli shell`, and the model's
`run_command` tool all use PowerShell (`pwsh` when installed, otherwise
Windows PowerShell). On macOS they use zsh. Linux uses bash or the login shell.
Set `RELAY_SHELL` to override the interactive shell.

Linux, macOS, and Windows binaries are built and tested by GitHub Actions.

## License

Apache-2.0. See [LICENSE](LICENSE).
