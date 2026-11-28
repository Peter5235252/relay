# Relay Studio plan (superseded)

Status: superseded, not implemented.

This draft proposed an optional React + Tauri + TypeScript desktop client. The
project direction changed: Relay stays native, with Rust + GPUI as the default
client, and external integrations use the headless daemon instead of a second
UI stack.

See [daemon.md](daemon.md) for the implemented IPC server, protocol, and
reference client.
