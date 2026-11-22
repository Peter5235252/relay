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

### Windows (x64, Windows 10 1809+)

Download `relay-setup-windows-x64.exe` and run the installer, or use
`relay-windows-x64.zip` for a portable copy. The installer adds Start Menu
shortcuts and can put `relay-cli` on your `PATH`. SmartScreen may warn about an
unknown publisher; choose **More info → Run anyway**.

### Local install from a checkout (Linux)

Build both applications and install them into `~/.local`:

```sh
cargo relay-build
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

## Building from source

### Prerequisites

Install Git and [rustup](https://rustup.rs/). The repository pins Rust
1.100.0 in `rust-toolchain.toml`; rustup automatically downloads the required
toolchain the first time you build.

The desktop app uses GPUI and needs a native compiler toolchain and platform
libraries in addition to Rust:

- **Linux (Debian/Ubuntu):** install the packages used by CI:

  ```sh
  sudo apt-get update
  sudo apt-get install -y build-essential pkg-config cmake clang \
    libssl-dev libfontconfig1-dev libfreetype6-dev \
    libx11-dev libx11-xcb-dev libxcb1-dev \
    libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev \
    libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
    libegl1-mesa-dev libvulkan-dev libasound2-dev \
    libglib2.0-dev libgtk-3-dev
  ```

  On other Linux distributions, install the equivalent development packages
  from your distribution's repositories.

- **macOS:** install Xcode and its command-line tools, launch Xcode once to
  finish installing its components, then run `xcode-select --install` if the
  command-line tools are not installed. Install CMake with
  `brew install cmake` or your preferred package manager.

- **Windows:** install Visual Studio or Build Tools with the **Desktop
  development with C++** workload, including the MSVC x64/x86 build tools and
  a Windows 10 or 11 SDK. Install CMake as well. Run the build from a Visual
  Studio Developer PowerShell or Developer Command Prompt so the MSVC tools
  are available in the environment.

See the [GPUI build prerequisites](https://github.com/zed-industries/zed/tree/main/docs/src/development)
for additional platform-specific details.

### Build both applications

Clone the repository and run this same command from its root on Linux, macOS,
or Windows:

```sh
git clone https://github.com/Peter5235252/relay.git
cd relay
cargo relay-build
```

`cargo relay-build` is a repository-local Cargo alias for a locked,
optimized release build of both application packages. It builds:

- `relay` — desktop application.
- `relay-cli` — command-line client.
- `relay-clear` — local data cleanup utility.

The executables are written to `target/release` (`target\\release` on
Windows, with the `.exe` extension).

A regular `cargo build` also builds both applications, using the faster debug
profile. To build just one application, select its package explicitly:

```sh
cargo build --release --locked -p relay-ui
cargo build --release --locked -p relay-cli
```

To run the applications directly from the checkout:

```sh
cargo run --release -p relay-ui
cargo run --release -p relay-cli --bin relay-cli
```

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
| Windows x64          | ✓           | ✓   | PowerShell / pwsh   | ✓ (Vulkan)           |
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
