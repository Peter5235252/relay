# Relay daemon and IPC protocol

`relay --daemon` (or `relay --background`) runs Relay headlessly and serves the
same engine, encrypted store, settings, model catalog, tools, and streaming
provider loop that the GPUI desktop app and `relay-cli` use. It is not a second
implementation: native clients connect to the running process instead of
opening the database or reimplementing provider behavior.

The GPUI application remains the default client. The daemon is an additive
transport for native tools, editor integrations, and automation.

## Starting and discovering the daemon

```sh
RELAY_MASTER_KEY=<64 hex characters> relay --daemon
```

Without an OS keyring, set `RELAY_MASTER_KEY` to the 64-hex-character master
key that unlocks the existing database. On a desktop with a keyring, the daemon
uses the same key as the GUI and CLI.

On Unix the daemon creates a private runtime directory:

```
$XDG_RUNTIME_DIR/relay/
├── daemon.sock   # Unix-domain socket (mode 0600)
├── daemon.json   # discovery document (mode 0600)
└── daemon.token  # authentication token (mode 0600)
```

`RELAY_DAEMON_DIR` overrides the directory. On Windows the daemon binds a
loopback TCP port instead of a Unix socket and writes the same discovery and
token files under the Relay data directory (`%APPDATA%\relay\daemon` by
default, or `RELAY_DAEMON_DIR`).

`daemon.json` is written after the listener is ready:

```json
{
  "protocol": 1,
  "version": "0.1.10",
  "pid": 12345,
  "started_at": "2026-11-28T12:00:00+00:00",
  "endpoint": { "transport": "unix", "path": "/run/user/1000/relay/daemon.sock" },
  "data_dir": "/home/user/.local/share/relay"
}
```

On Windows the endpoint is `{ "transport": "tcp", "address": "127.0.0.1:54321" }`.

The token file contains a fresh random token for each daemon run. Clients must
read it (not guess it) and send it in `initialize`. Files are removed on a clean
shutdown; a stale discovery file means the daemon is not running.

## Transport

- JSON-RPC 2.0, one UTF-8 JSON object per line, terminated by `\n`.
- Requests may carry an `id`; notifications omit it. Responses echo the id.
- Server notifications use `"jsonrpc": "2.0"`, a `method`, and `params`, and
  have no `id`.
- Frames larger than 16 MiB are rejected.
- Requests on one connection may be answered out of order. In particular,
  `chat.cancel` and `approval.respond` can be sent while `chat.send` or
  `chat.wait` is still pending.

Every method except `ping` and `initialize` requires a successful
`initialize` on that connection.

### `initialize`

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"token":"...","client":"my-editor/1.0"}}
```

The response describes the server and its capabilities. A wrong token returns
JSON-RPC error `-32001` and leaves the connection unauthenticated.

### `ping`

Always available, even before authentication. Returns the protocol version and
server version.

## Methods

| Method | Params | Result |
| --- | --- | --- |
| `initialize` | `token`, optional `client` | server version, protocol, capabilities |
| `ping` | none | `pong`, version, protocol |
| `daemon.status` | none | endpoint, uptime, client/turn counts, database path, bundled-model status |
| `daemon.shutdown` | none | `stopping: true`; the process exits shortly after |
| `conversations.list` | optional `archived`, `limit` | `conversations` |
| `conversations.get` | `conversation_id` | `conversation`, `messages` |
| `conversations.create` | optional `model` | `conversation` |
| `conversations.rename` | `conversation_id`, `title` | `conversation` |
| `conversations.delete` | `conversation_id` | `deleted: true` |
| `conversations.archive` | `conversation_id`, optional `archived` (default `true`) | `conversation` |
| `conversations.pin` | `conversation_id`, optional `pinned` (default `true`) | `conversation` |
| `conversations.search` | `query` | `conversations` |
| `conversations.export` | `conversation_id` | `markdown` |
| `settings.get` | none | `settings` |
| `settings.update` | `patch` (JSON object) | merged, normalized `settings` |
| `settings.reload` | none | same as `settings.get` |
| `models.list` | none | curated, bundled, Ollama, and local entries; `default` |
| `models.ensure` | `model`, optional `download` | model readiness and updated settings |
| `tools.list` | none | tools available under current settings |
| `presets.list` | none | personality presets |
| `memory.list` | none | `memories` (`id`, `content`) |
| `memory.add` | `content` | `id` |
| `memory.delete` | `id` | `deleted: true` |
| `chat.start` | turn params | `turn_id`, `conversation_id`, `model` |
| `chat.send` | turn params | `TurnResult` after the turn finishes |
| `chat.wait` | `turn_id`, optional `timeout_ms` | `TurnResult` |
| `chat.cancel` | `turn_id` | `cancelling: true` |
| `approval.respond` | `approval_id`, optional `approved` (default `true`) | decision |

Turn params for `chat.start` and `chat.send`:

```json
{
  "conversation_id": "optional existing id",
  "message": "user text",
  "model": "optional model slug",
  "attachments": [
    {
      "type": "image",
      "media_type": "image/png",
      "data": "<base64 without a data: prefix>",
      "name": "diagram.png",
      "width": 1280,
      "height": 720
    }
  ],
  "turn_id": "optional client-supplied id"
}
```

Omit `conversation_id` to create a conversation. `model` defaults to the
conversation's model, then to the app default. A conversation can have only one
active turn; a second `chat.start` for it returns error `-32003`.

`settings.update` merges top-level keys into the current settings and writes the
normalized result to SQLCipher. Read the object from `settings.get` before
patching so a structured value such as `mcp_servers` stays complete.

`models.ensure` with `download: true` can download and start the bundled
Nemotron llama-server. Without it, a missing bundled model returns error
`-32004` and the caller decides whether to trigger the download.

## Events

Authenticated connections receive server notifications for all conversations,
not only their own turns. Clients should filter by `conversation_id` or
`turn_id` when needed.

| Event | Payload |
| --- | --- |
| `turn.started` | `turn_id`, `conversation_id`, `model` |
| `turn.text` | `turn_id`, `conversation_id`, `delta` |
| `turn.thinking` | `turn_id`, `conversation_id`, `delta` |
| `turn.usage` | `turn_id`, `conversation_id`, `usage` |
| `turn.tool_call` | `turn_id`, `conversation_id`, `call` |
| `turn.tool_result` | `turn_id`, `conversation_id`, `id`, `name`, `output` |
| `turn.completed` | the full `TurnResult` |
| `approval.requested` | `turn_id`, `conversation_id`, `approval_id`, `call` |
| `approval.expired` | `approval_id` |
| `conversations.changed` | `conversation_id`, `action` (`created`, `renamed`, `deleted`, `archived`, `pinned`, `message`) |
| `settings.changed` | `settings` |

`TurnResult` contains `turn_id`, `conversation_id`, `model`, visible `text`,
`thinking`, optional `usage` and `cost_usd`, `cancelled`, optional `error`,
optional stored `message_id`, and the refreshed conversation summary.

## Turn flow

Streaming with `chat.start`:

```json
{"jsonrpc":"2.0","id":10,"method":"chat.start","params":{"message":"Explain this file"}}
{"jsonrpc":"2.0","method":"turn.started","params":{"turn_id":"...","conversation_id":"...","model":"claude-sonnet-5-5"}}
{"jsonrpc":"2.0","method":"turn.text","params":{"turn_id":"...","conversation_id":"...","delta":"This file "}}
{"jsonrpc":"2.0","method":"turn.completed","params":{"turn_id":"...","text":"This file ..."}}
{"jsonrpc":"2.0","id":10,"result":{"turn_id":"...","conversation_id":"...","model":"claude-sonnet-5-5"}}
```

Call `chat.wait` to block for the final `TurnResult`, or consume
`turn.completed` notifications. `chat.wait` defaults to a 10-minute deadline and
returns error `-32005` if it expires.

Tool approval is opt-in through settings. When the engine needs approval it
emits `approval.requested`; answer with `approval.respond`. If no client answers
within 120 seconds, the daemon denies the call and emits `approval.expired`.

Cancellation is cooperative: `chat.cancel` cancels the provider stream and tool
loop, and the final `TurnResult` has `cancelled: true`.

## Reference client

Release archives and `install.py` include `relay-daemon-client`. From a source
checkout, `cargo relay-build` builds it alongside the desktop and CLI apps, or
build just this binary:

```sh
cargo build --release --locked -p relay-daemon --bin relay-daemon-client

relay-daemon-client ping
relay-daemon-client status
relay-daemon-client models
relay-daemon-client conversations list
relay-daemon-client --model claude-sonnet-5-5 send "write a haiku"
relay-daemon-client --conversation <id> send "continue"
relay-daemon-client --thinking send "why is the sky blue?"
relay-daemon-client approve <approval_id>
relay-daemon-client shutdown
```

Use `--dir` to point at a non-default runtime directory and `--json` for
machine-readable output.

Rust clients can use the library API:

```rust
use relay_daemon::client::Client;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), String> {
    let mut client = Client::connect().await?;
    let status = client.call("daemon.status", json!({})).await?;
    println!("{status}");

    let result = client
        .call_with_events(
            "chat.send",
            json!({"message": "Hello", "model": "claude-sonnet-5-5"}),
            |event| println!("{}", event.method),
        )
        .await?;
    println!("{}", result["text"]);
    Ok(())
}
```

`Client::connect_dir(path)` connects to a specific runtime directory.
`Client::connect_info(info, token)` accepts an already-read discovery document,
which is useful for launchers and tests.

## Errors

| Code | Meaning |
| --- | --- |
| `-32700` | invalid JSON |
| `-32600` | invalid request or oversized frame |
| `-32601` | unknown method |
| `-32602` | invalid params or unknown model |
| `-32603` | internal failure |
| `-32001` | authentication required |
| `-32002` | conversation, turn, or approval not found |
| `-32003` | conflicting turn in the same conversation |
| `-32004` | selected model is not ready (missing key, Ollama model, or bundled download) |
| `-32005` | `chat.wait` deadline exceeded |
| `-32010` | encrypted store failure |

## Security notes

- The runtime directory is mode `0700`; the socket, discovery file, and token
  are mode `0600` on Unix.
- The daemon never listens on a public interface. Windows uses `127.0.0.1` with
  an ephemeral port.
- The token is per run and is compared before any state-changing method runs.
- Provider API keys are never returned over IPC. Configure or rotate them with
  the desktop app or `relay-cli keys`.
- `daemon.shutdown` is authenticated but intentionally available to any
  authenticated client. Run the daemon under the user's own session.
