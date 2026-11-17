#!/usr/bin/env bash
set -euo pipefail

# Install Relay from a local release build into ~/.local.
# Build first: cargo build -p relay-ui -p relay-cli --release

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN_DIR="${HOME}/.local/bin"
APP_DIR="${HOME}/.local/share/applications"
ICON_ROOT="${HOME}/.local/share/icons/hicolor"

mkdir -p "$BIN_DIR" "$APP_DIR"
install -m755 "$ROOT/target/release/relay" "$BIN_DIR/relay"
install -m755 "$ROOT/target/release/relay-cli" "$BIN_DIR/relay-cli"
install -m755 "$ROOT/target/release/relay-clear" "$BIN_DIR/relay-clear" 2>/dev/null || true
install -m644 "$ROOT/packaging/io.github.csemanpeter.Relay.desktop" "$APP_DIR/io.github.csemanpeter.Relay.desktop"

for dir in "$ROOT"/packaging/icons/hicolor/*; do
    size="$(basename "$dir")"
    if [ -d "$dir/apps" ]; then
        mkdir -p "$ICON_ROOT/$size/apps"
        cp "$dir"/apps/* "$ICON_ROOT/$size/apps/"
    fi
done

update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
gtk-update-icon-cache "$ICON_ROOT" 2>/dev/null || true
echo "Installed Relay to $BIN_DIR"