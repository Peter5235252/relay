//! End-to-end tests for the local IPC daemon.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use relay_core::AppSettings;
use relay_daemon::client::Client;
use relay_daemon::{DaemonOptions, read_info_from, run};
use relay_engine::bundled;
use relay_store::{DatabaseKey, Store};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

const TOKEN: &str = "test-token";

fn runtime_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("relay-daemon-{label}-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("create runtime dir");
    dir
}

fn open_store(settings: &AppSettings) -> Store {
    let store = Store::open_in_memory(&DatabaseKey::new([7u8; 32])).expect("open in-memory store");
    store.save_settings(settings).expect("save settings");
    store
}

async fn start_daemon(dir: &Path, settings: AppSettings) -> JoinHandle<Result<(), String>> {
    let store = open_store(&settings);
    tokio::spawn(run(DaemonOptions {
        store,
        settings,
        runtime: tokio::runtime::Handle::current(),
        bundled: Arc::new(Mutex::new(bundled::BundledState::default())),
        runtime_dir: Some(dir.to_path_buf()),
        token: Some(TOKEN.to_owned()),
    }))
}

async fn connect(dir: &Path) -> Client {
    for _ in 0..400 {
        if let Ok(client) = Client::connect_dir(dir).await {
            return client;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("daemon did not accept connections in {}", dir.display());
}

async fn stop(client: &mut Client, handle: JoinHandle<Result<(), String>>) {
    let _ = client.call("daemon.shutdown", json!({})).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serves_conversations_settings_and_memory() {
    let dir = runtime_dir("crud");
    let handle = start_daemon(&dir, AppSettings::default()).await;
    let mut client = connect(&dir).await;

    let pong = client.call("ping", json!({})).await.expect("ping");
    assert_eq!(pong["pong"], json!(true));

    let status = client
        .call("daemon.status", json!({}))
        .await
        .expect("status");
    assert_eq!(status["daemon"]["protocol"], json!(1));
    assert_eq!(status["clients"], json!(1));

    let created = client
        .call("conversations.create", json!({}))
        .await
        .expect("create conversation");
    let conversation_id = created["conversation"]["id"]
        .as_str()
        .expect("conversation id")
        .to_owned();

    let listed = client
        .call("conversations.list", json!({}))
        .await
        .expect("list conversations");
    assert_eq!(listed["conversations"].as_array().map(Vec::len), Some(1));

    let renamed = client
        .call(
            "conversations.rename",
            json!({"conversation_id": conversation_id, "title": "IPC test"}),
        )
        .await
        .expect("rename conversation");
    assert_eq!(renamed["conversation"]["title"], json!("IPC test"));

    client
        .call(
            "conversations.pin",
            json!({"conversation_id": conversation_id, "pinned": true}),
        )
        .await
        .expect("pin conversation");
    let pinned = client
        .call(
            "conversations.get",
            json!({"conversation_id": conversation_id}),
        )
        .await
        .expect("get conversation");
    assert_eq!(pinned["conversation"]["pinned"], json!(true));

    let updated = client
        .call(
            "settings.update",
            json!({"patch": {"temperature": 0.4, "show_thinking": false}}),
        )
        .await
        .expect("update settings");
    assert_eq!(updated["settings"]["temperature"], json!(0.4));
    assert_eq!(updated["settings"]["show_thinking"], json!(false));

    let settings = client
        .call("settings.get", json!({}))
        .await
        .expect("get settings");
    assert_eq!(settings["settings"]["temperature"], json!(0.4));

    let memory = client
        .call(
            "memory.add",
            json!({"content": "The user prefers dark mode."}),
        )
        .await
        .expect("add memory");
    let memory_id = memory["id"].as_str().expect("memory id").to_owned();
    let memories = client
        .call("memory.list", json!({}))
        .await
        .expect("list memories");
    assert_eq!(memories["memories"].as_array().map(Vec::len), Some(1));

    client
        .call("memory.delete", json!({"id": memory_id}))
        .await
        .expect("delete memory");
    let memories = client
        .call("memory.list", json!({}))
        .await
        .expect("list memories after delete");
    assert_eq!(memories["memories"].as_array().map(Vec::len), Some(0));

    let tools = client
        .call("tools.list", json!({}))
        .await
        .expect("list tools");
    assert!(
        tools["tools"]
            .as_array()
            .is_some_and(|tools| !tools.is_empty())
    );

    let presets = client
        .call("presets.list", json!({}))
        .await
        .expect("list presets");
    assert!(
        presets["presets"]
            .as_array()
            .is_some_and(|items| !items.is_empty())
    );

    let models = client
        .call("models.list", json!({}))
        .await
        .expect("list models");
    assert!(
        models["models"]
            .as_array()
            .is_some_and(|models| !models.is_empty())
    );

    stop(&mut client, handle).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_a_wrong_token() {
    let dir = runtime_dir("auth");
    let handle = start_daemon(&dir, AppSettings::default()).await;

    // Wait until the discovery document is available.
    for _ in 0..400 {
        if read_info_from(&dir).is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let info = read_info_from(&dir).expect("discovery document");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(relay_daemon::token_path(&dir))
            .expect("token metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "token file must not be group/world readable");
    }
    let error = Client::connect_info(&info, "not-the-token")
        .await
        .expect_err("wrong token must fail");
    assert!(error.contains("authentication required"), "{error}");

    let mut client = connect(&dir).await;
    stop(&mut client, handle).await;
    let _ = std::fs::remove_dir_all(dir);
}

/// A minimal OpenAI-compatible SSE endpoint used to exercise the turn pipeline
/// without a network provider or an API key.
async fn fake_openai(delay: Duration) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind fake endpoint");
    let address = listener.local_addr().expect("fake endpoint address");
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut buffer = [0u8; 8192];
        let _ = socket.read(&mut buffer).await;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        let body = concat!(
            "HTTP/1.1 200 OK\r\n",
            "Content-Type: text/event-stream\r\n",
            "Cache-Control: no-cache\r\n",
            "Connection: close\r\n",
            "\r\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\" from the daemon.\"}}],",
            "\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5,\"total_tokens\":15}}\n\n",
            "data: [DONE]\n\n"
        );
        let _ = socket.write_all(body.as_bytes()).await;
        let _ = socket.shutdown().await;
    });
    format!("http://{address}/v1")
}

fn local_settings(endpoint: &str) -> AppSettings {
    AppSettings {
        interaction_mode: "chat".to_owned(),
        local_base_url: endpoint.to_owned(),
        local_model: "test-model".to_owned(),
        ..AppSettings::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streams_a_chat_turn_and_persists_it() {
    let endpoint = fake_openai(Duration::ZERO).await;
    let dir = runtime_dir("turn");
    let handle = start_daemon(&dir, local_settings(&endpoint)).await;
    let mut client = connect(&dir).await;

    let mut events = Vec::new();
    let result = client
        .call_with_events(
            "chat.send",
            json!({"message": "Say hello", "model": "local/test-model"}),
            |event| events.push(event.method.clone()),
        )
        .await
        .expect("chat.send");

    assert_eq!(result["text"], json!("Hello from the daemon."));
    assert_eq!(result["usage"]["input_tokens"], json!(10));
    assert_eq!(result["usage"]["output_tokens"], json!(5));
    assert!(result["message_id"].is_string());
    assert_eq!(result["cancelled"], json!(false));
    assert!(
        events.iter().any(|event| event == "turn.started"),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| event == "turn.text"),
        "{events:?}"
    );
    assert!(
        events.iter().any(|event| event == "turn.completed"),
        "{events:?}"
    );

    let conversation_id = result["conversation_id"].as_str().expect("conversation id");
    let stored = client
        .call(
            "conversations.get",
            json!({"conversation_id": conversation_id}),
        )
        .await
        .expect("stored conversation");
    assert_eq!(stored["conversation"]["message_count"], json!(2));
    assert_eq!(stored["messages"][0]["role"], json!("user"));
    assert_eq!(stored["messages"][1]["role"], json!("assistant"));

    stop(&mut client, handle).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_a_second_turn_and_cancels_the_first() {
    let endpoint = fake_openai(Duration::from_secs(5)).await;
    let dir = runtime_dir("conflict");
    let handle = start_daemon(&dir, local_settings(&endpoint)).await;
    let mut client = connect(&dir).await;

    let started = client
        .call(
            "chat.start",
            json!({"message": "hold on", "model": "local/test-model"}),
        )
        .await
        .expect("chat.start");
    let turn_id = started["turn_id"].as_str().expect("turn id").to_owned();
    let conversation_id = started["conversation_id"]
        .as_str()
        .expect("conversation id")
        .to_owned();

    let error = client
        .call(
            "chat.start",
            json!({
                "message": "second",
                "conversation_id": conversation_id,
                "model": "local/test-model",
            }),
        )
        .await
        .expect_err("a concurrent turn must be rejected");
    assert!(error.contains("already has active turn"), "{error}");

    client
        .call("chat.cancel", json!({"turn_id": turn_id}))
        .await
        .expect("cancel");
    let result = client
        .call(
            "chat.wait",
            json!({"turn_id": turn_id, "timeout_ms": 10_000}),
        )
        .await
        .expect("wait for cancelled turn");
    assert_eq!(result["cancelled"], json!(true));
    assert!(result["error"].is_null());

    stop(&mut client, handle).await;
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_unknown_methods_and_models() {
    let dir = runtime_dir("errors");
    let handle = start_daemon(&dir, AppSettings::default()).await;
    let mut client = connect(&dir).await;

    let error = client
        .call("does.not.exist", json!({}))
        .await
        .expect_err("unknown method");
    assert!(error.contains("unknown method"), "{error}");

    let error = client
        .call("conversations.create", json!({"model": "nope/nope"}))
        .await
        .expect_err("unknown model");
    assert!(error.contains("unknown model"), "{error}");

    let error = client
        .call("chat.wait", json!({"turn_id": "missing"}))
        .await
        .expect_err("unknown turn");
    assert!(error.contains("does not exist"), "{error}");

    let unknown: Value = client
        .call("daemon.status", json!({}))
        .await
        .expect("status");
    assert_eq!(unknown["active_turns"], json!(0));

    stop(&mut client, handle).await;
    let _ = std::fs::remove_dir_all(dir);
}
