# Relay

Relay is a memory-safe Rust agentic work and chat client with desktop and terminal
interfaces, unifying Gemini, Claude, GPT, Ollama, and OpenAI-compatible models
behind a single interface. The desktop app is built with
[GPUI](https://github.com/zed-industries/zed).

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
The CLI binaries are ad-hoc signed. If macOS quarantines a downloaded copy,
clear the flag once with `xattr -dr com.apple.quarantine Relay.app relay-cli relay-clear`.

### Windows 11 (x64)

Download `relay-setup-windows-x64.exe` and run the installer, or use
`relay-windows-x64.zip` for a portable copy. Windows 10 is not supported.

### Build and install from source (Linux, macOS, Windows 11)

The repository includes one cross-platform installer, `install.py`. It installs
native build dependencies where supported, bootstraps the pinned Rust toolchain,
builds the desktop app and CLI, and installs them for the current user. Python
3.10 or newer is required.

```sh
git clone https://github.com/Peter5235252/relay.git
cd relay

# Linux and macOS
python3 install.py
```

On Windows 11, run this from PowerShell:

```powershell
py -3 install.py
```

The Linux installer supports apt-based distributions (Debian, Ubuntu 24.04/26.04
LTS and derivatives), Void, Arch-based distributions, Fedora, RHEL-compatible
systems, openSUSE, Alpine, and Gentoo. It uses `sudo` or `doas` to install
system packages. On RHEL, enable the required AppStream and CodeReady Builder
repositories; some packages may also require EPEL.

On macOS, install Xcode Command Line Tools and Homebrew first. The script uses
Homebrew to install CMake and pkg-config. On Windows 11, it can install Visual
Studio 2022 Build Tools with the C++ workload and Windows 11 SDK, plus CMake,
through winget.

Use `--skip-system-deps` when the required native packages are already installed,
`--no-install` to build without copying the executables into user install
locations, or `--debug` for a faster debug build. Unrecognized Linux
distributions can use `--skip-system-deps` after installing equivalent GPUI
development packages manually.

## Attachments

Relay accepts image attachments (PNG, JPEG, GIF, WebP) in both the desktop
app and the CLI:

- Desktop: click `+`, drag-and-drop files onto the composer, or paste a path.
- CLI: `relay-cli --image ./photo.png "describe this"` or `/image ./photo.png`
  inside the REPL.

Images are stored inside the encrypted SQLCipher database alongside the
conversation.

## Building from source

For a one-command build and per-user installation, run `python3 install.py`
on Linux/macOS or `py -3 install.py` on Windows 11. The installer handles
supported native dependencies and the Rust toolchain before building both apps.

With prerequisites already installed, build both applications from the
repository root on any supported platform:

```sh
cargo relay-build
```

This creates `relay` (desktop), `relay-cli` (terminal client), and
`relay-clear` (data cleanup utility) under `target/release` or
`target\\release` on Windows. A plain `cargo build` also builds both apps
in the debug profile.

The repository sets `RUST_FONTCONFIG_DLOPEN` for
`yeslogic-fontconfig-sys`. Fontconfig is loaded at runtime, so builds no longer
need `fontconfig.pc` or the Fontconfig development package at compile time.
The Fontconfig shared library is still required at runtime.

To build only one application, select its package:

```sh
cargo build --release --locked -p relay-ui
cargo build --release --locked -p relay-cli
```

Run an application from the checkout with `cargo run -p relay-ui` or
`cargo run -p relay-cli --bin relay-cli`.

## Command line

The terminal client shares the same engine, settings store, history, keys, and
personality presets as the desktop app. Generation controls match the desktop
Settings panel and persist for later runs.

```sh
cargo relay-build
cargo run --release -p relay-cli --bin relay-cli

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

| Platform             | Desktop app | CLI | Integrated terminal | Bundled local models |
| -------------------- | ----------- | --- | ------------------- | -------------------- |
| Linux x86_64         | ✓           | ✓   | bash/zsh            | ✓ (Vulkan)           |
| Linux arm64          | ✓           | ✓   | bash/zsh            | ✓ (CPU)              |
| macOS arm64 + x86_64 | ✓           | ✓   | zsh                 | ✓ (Metal)            |
| Windows 11 x64       | ✓           | ✓   | PowerShell / pwsh   | ✓ (Vulkan)           |
| Windows arm64        | ✓           | ✓   | PowerShell / pwsh   | ✓ (CPU)              |

Prebuilt release archives are produced for Linux x86_64, macOS (universal), and
Windows x64. The other rows build from source with the bundled llama.cpp
archive for that target.

On Windows the desktop terminal, `relay-cli shell`, and the model's
`run_command` tool all use PowerShell (`pwsh` when installed, otherwise
Windows PowerShell). On macOS they use zsh. Linux uses bash or the login shell.
Set `RELAY_SHELL` to override the interactive shell.

Linux, macOS, and Windows binaries are built and tested by GitHub Actions.

## License

MIT. See [LICENSE](LICENSE).
