//! Local JSON-RPC server that exposes the shared Relay engine.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use relay_core::{
    AppSettings, ChatMessage, ContentBlock, ConversationSummary, ModelSelection, Provider, Usage,
};
use relay_engine::{agent, bundled, keys, mcp, streaming};
use relay_providers::StreamEvent;
use relay_store::Store;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(not(unix))]
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

#[cfg(unix)]
use crate::protocol::socket_path;
use crate::protocol::{
    DaemonInfo, Endpoint, PROTOCOL_VERSION, RpcError, RpcNotification, RpcRequest, RpcResponse,
    StartedTurn, TurnResult, default_runtime_dir, info_path, token_path, write_private,
};

/// Largest accepted JSON-RPC frame.
const MAX_FRAME: usize = 16 * 1024 * 1024;
/// How long an approval request waits before the daemon denies it.
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
/// Default `chat.wait` deadline.
const DEFAULT_WAIT_MS: u64 = 600_000;
/// How many finished turns remain queryable by id.
const COMPLETED_TURNS: usize = 64;

use crate::BoxStream;

/// Everything the daemon needs from the host process.
pub struct DaemonOptions {
    /// Shared encrypted store opened by the host process.
    pub store: Store,
    /// Initial settings snapshot.
    pub settings: AppSettings,
    /// Tokio runtime used for provider streams and tool execution.
    pub runtime: tokio::runtime::Handle,
    /// Bundled llama-server state owned by the host process.
    pub bundled: Arc<Mutex<bundled::BundledState>>,
    /// Override the runtime directory (tests and portable installs).
    pub runtime_dir: Option<PathBuf>,
    /// Override the generated authentication token.
    pub token: Option<String>,
}

/// Run the daemon until `daemon.shutdown`, Ctrl-C, or SIGTERM.
pub async fn run(options: DaemonOptions) -> Result<(), String> {
    let daemon = Arc::new(Daemon::new(options)?);
    daemon.serve().await
}

struct ClientState {
    id: u64,
    authorized: AtomicBool,
    tx: mpsc::UnboundedSender<String>,
}

struct Turn {
    id: String,
    conversation_id: String,
    cancel: Mutex<Option<CancellationToken>>,
    cancel_requested: AtomicBool,
    result_tx: watch::Sender<Option<TurnResult>>,
    result_rx: watch::Receiver<Option<TurnResult>>,
}

impl Turn {
    fn new(id: String, conversation_id: String) -> Self {
        let (result_tx, result_rx) = watch::channel(None);
        Self {
            id,
            conversation_id,
            cancel: Mutex::new(None),
            cancel_requested: AtomicBool::new(false),
            result_tx,
            result_rx,
        }
    }

    fn attach_cancel(&self, token: CancellationToken) {
        if let Ok(mut slot) = self.cancel.lock() {
            *slot = Some(token.clone());
        }
        if self.cancel_requested.load(Ordering::Acquire) {
            token.cancel();
        }
    }

    fn request_cancel(&self) {
        self.cancel_requested.store(true, Ordering::Release);
        if let Ok(slot) = self.cancel.lock()
            && let Some(token) = slot.as_ref()
        {
            token.cancel();
        }
    }

    fn finish(&self, result: TurnResult) {
        let _ = self.result_tx.send(Some(result));
    }
}

struct Daemon {
    store: Store,
    runtime: tokio::runtime::Handle,
    bundled: Arc<Mutex<bundled::BundledState>>,
    settings: RwLock<AppSettings>,
    token: String,
    info: RwLock<DaemonInfo>,
    runtime_dir: PathBuf,
    clients: RwLock<HashMap<u64, Arc<ClientState>>>,
    next_client: AtomicU64,
    turns: Mutex<HashMap<String, Arc<Turn>>>,
    completed: Mutex<VecDeque<(String, Arc<Turn>)>>,
    conversation_turns: Mutex<HashMap<String, String>>,
    approvals: Mutex<HashMap<String, oneshot::Sender<bool>>>,
    next_approval: AtomicU64,
    shutdown: CancellationToken,
    started: Instant,
}

impl Daemon {
    fn new(options: DaemonOptions) -> Result<Self, String> {
        let runtime_dir = match options.runtime_dir {
            Some(dir) => dir,
            None => default_runtime_dir()?,
        };
        std::fs::create_dir_all(&runtime_dir)
            .map_err(|error| format!("could not create {}: {error}", runtime_dir.display()))?;
        restrict_dir(&runtime_dir)?;

        let token = options
            .token
            .filter(|token| !token.trim().is_empty())
            .unwrap_or_else(generate_token);

        let data_dir = relay_store::default_data_dir()
            .map(|dir| dir.display().to_string())
            .unwrap_or_default();
        let endpoint = if cfg!(unix) {
            Endpoint::Unix {
                path: String::new(),
            }
        } else {
            Endpoint::Tcp {
                address: String::new(),
            }
        };
        let info = DaemonInfo {
            protocol: PROTOCOL_VERSION,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            endpoint,
            data_dir,
        };

        Ok(Self {
            store: options.store,
            runtime: options.runtime,
            bundled: options.bundled,
            settings: RwLock::new(options.settings),
            token,
            info: RwLock::new(info),
            runtime_dir,
            clients: RwLock::new(HashMap::new()),
            next_client: AtomicU64::new(1),
            turns: Mutex::new(HashMap::new()),
            completed: Mutex::new(VecDeque::new()),
            conversation_turns: Mutex::new(HashMap::new()),
            approvals: Mutex::new(HashMap::new()),
            next_approval: AtomicU64::new(1),
            shutdown: CancellationToken::new(),
            started: Instant::now(),
        })
    }

    async fn serve(self: Arc<Self>) -> Result<(), String> {
        // Bind before publishing the token: a second daemon that finds an
        // active listener must not overwrite the token of the running one.
        let (listener, endpoint) = create_listener(&self.runtime_dir).await?;
        write_private(&token_path(&self.runtime_dir), self.token.as_bytes())?;
        {
            let mut info = self
                .info
                .write()
                .map_err(|_| "daemon state poisoned".to_owned())?;
            info.endpoint = endpoint.clone();
            let text = serde_json::to_string_pretty(&*info)
                .map_err(|error| format!("could not encode discovery file: {error}"))?;
            write_private(&info_path(&self.runtime_dir), text.as_bytes())?;
        }

        eprintln!("Relay daemon listening on {}", endpoint.display());
        eprintln!("  discovery: {}", info_path(&self.runtime_dir).display());
        info!(endpoint = %endpoint.display(), "Relay daemon listening");

        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                shutdown.cancel();
            }
        });
        #[cfg(unix)]
        {
            let shutdown = self.shutdown.clone();
            tokio::spawn(async move {
                if let Ok(mut signal) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    signal.recv().await;
                    shutdown.cancel();
                }
            });
        }

        loop {
            tokio::select! {
                _ = self.shutdown.cancelled() => break,
                accepted = listener.accept() => match accepted {
                    Ok(stream) => self.spawn_connection(stream),
                    Err(error) => warn!(%error, "daemon accept failed"),
                },
            }
        }

        self.shutdown.cancel();
        self.cancel_all_turns();
        // Give cancelled turns a moment to persist their final state.
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let drained = self
                .turns
                .lock()
                .map(|turns| turns.is_empty())
                .unwrap_or(true);
            if drained {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        bundled::stop(&self.bundled);
        cleanup(&self.runtime_dir);
        info!("Relay daemon stopped");
        Ok(())
    }

    fn spawn_connection(self: &Arc<Self>, stream: BoxStream) {
        let id = self.next_client.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        let client = Arc::new(ClientState {
            id,
            authorized: AtomicBool::new(false),
            tx,
        });
        if let Ok(mut clients) = self.clients.write() {
            clients.insert(id, Arc::clone(&client));
        }

        let daemon = Arc::clone(self);
        tokio::spawn(async move {
            Arc::clone(&daemon)
                .connection_loop(Arc::clone(&client), stream, rx)
                .await;
            if let Ok(mut clients) = daemon.clients.write() {
                clients.remove(&id);
            }
        });
    }

    async fn connection_loop(
        self: Arc<Self>,
        client: Arc<ClientState>,
        stream: BoxStream,
        mut outbound: mpsc::UnboundedReceiver<String>,
    ) {
        let (read_half, mut write_half) = tokio::io::split(stream);
        let writer = tokio::spawn(async move {
            while let Some(line) = outbound.recv().await {
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if write_half.write_all(b"\n").await.is_err() {
                    break;
                }
                if write_half.flush().await.is_err() {
                    break;
                }
            }
        });

        let mut reader = BufReader::new(read_half);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(read) => {
                    if read > MAX_FRAME {
                        self.send_response(
                            &client,
                            RpcResponse::error(
                                Value::Null,
                                RpcError::invalid_request("request frame is too large"),
                            ),
                        );
                        break;
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let request: RpcRequest = match serde_json::from_str(trimmed) {
                        Ok(request) => request,
                        Err(error) => {
                            self.send_response(
                                &client,
                                RpcResponse::error(Value::Null, RpcError::parse(error.to_string())),
                            );
                            continue;
                        }
                    };
                    let daemon = Arc::clone(&self);
                    let client = Arc::clone(&client);
                    tokio::spawn(async move {
                        daemon.handle_message(client, request).await;
                    });
                }
                Err(error) => {
                    debug!(%error, "daemon read failed");
                    break;
                }
            }
        }

        writer.abort();
        debug!(client = client.id, "daemon client disconnected");
    }

    fn send_response(&self, client: &ClientState, response: RpcResponse) {
        if let Ok(line) = serde_json::to_string(&response) {
            let _ = client.tx.send(line);
        }
    }

    async fn handle_message(self: Arc<Self>, client: Arc<ClientState>, request: RpcRequest) {
        let id = request.id.clone();
        let method = request.method.clone();
        let result = self.dispatch(&client, &request).await;
        if let Some(id) = id {
            let response = match result {
                Ok(value) => RpcResponse::result(id, value),
                Err(error) => RpcResponse::error(id, error),
            };
            self.send_response(&client, response);
        } else if let Err(error) = result {
            debug!(%method, code = error.code, message = %error.message, "notification failed");
        }
    }

    async fn dispatch(
        self: &Arc<Self>,
        client: &ClientState,
        request: &RpcRequest,
    ) -> Result<Value, RpcError> {
        match request.method.as_str() {
            "initialize" => return self.initialize(client, request.params.clone()),
            "ping" => {
                return Ok(json!({
                    "pong": true,
                    "version": env!("CARGO_PKG_VERSION"),
                    "protocol": PROTOCOL_VERSION,
                }));
            }
            _ => {}
        }
        if !client.authorized.load(Ordering::Relaxed) {
            return Err(RpcError::unauthorized());
        }

        match request.method.as_str() {
            "daemon.status" => self.status(),
            "daemon.shutdown" => self.daemon_shutdown(),
            "conversations.list" => self.conversations_list(request.params.clone()),
            "conversations.get" => self.conversations_get(request.params.clone()),
            "conversations.create" => self.conversations_create(request.params.clone()),
            "conversations.rename" => self.conversations_rename(request.params.clone()),
            "conversations.delete" => self.conversations_delete(request.params.clone()),
            "conversations.archive" => self.conversations_archive(request.params.clone()),
            "conversations.pin" => self.conversations_pin(request.params.clone()),
            "conversations.search" => self.conversations_search(request.params.clone()),
            "conversations.export" => self.conversations_export(request.params.clone()),
            "settings.get" => self.settings_get(),
            "settings.update" => self.settings_update(request.params.clone()),
            "settings.reload" => self.settings_get(),
            "models.list" => self.models_list(),
            "models.ensure" => self.models_ensure(request.params.clone()).await,
            "tools.list" => self.tools_list(),
            "presets.list" => self.presets_list(),
            "memory.list" => self.memory_list(),
            "memory.add" => self.memory_add(request.params.clone()),
            "memory.delete" => self.memory_delete(request.params.clone()),
            "chat.start" => self.chat_start(request.params.clone()).await,
            "chat.send" => self.chat_send(request.params.clone()).await,
            "chat.wait" => self.chat_wait(request.params.clone()).await,
            "chat.cancel" => self.chat_cancel(request.params.clone()),
            "approval.respond" => self.approval_respond(request.params.clone()),
            other => Err(RpcError::method_not_found(other)),
        }
    }

    fn initialize(&self, client: &ClientState, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            token: String,
            #[serde(default)]
            client: Option<String>,
        }
        let params: Params = parse_params(params)?;
        if params.token != self.token {
            return Err(RpcError::unauthorized());
        }
        client.authorized.store(true, Ordering::Relaxed);
        debug!(client = client.id, name = ?params.client, "daemon client initialized");
        Ok(json!({
            "server": "relay",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL_VERSION,
            "pid": std::process::id(),
            "capabilities": {
                "conversations": true,
                "settings": true,
                "models": true,
                "streaming": true,
                "approvals": true,
                "cancellation": true,
                "memory": true,
                "presets": true,
                "tools": true,
            },
        }))
    }

    fn status(&self) -> Result<Value, RpcError> {
        let info = self
            .info
            .read()
            .map_err(|_| RpcError::internal("daemon state poisoned"))?
            .clone();
        let clients = self
            .clients
            .read()
            .map(|clients| clients.len())
            .unwrap_or(0);
        let active_turns = self.turns.lock().map(|turns| turns.len()).unwrap_or(0);
        let bundled = self.bundled.lock().ok().map(|state| {
            let status = state.status();
            json!({
                "ready": status.ready,
                "endpoint": status.endpoint,
                "active_variant": status.active_variant,
                "message": status.message,
                "error": status.error,
                "progress": status.progress,
            })
        });
        Ok(json!({
            "daemon": info,
            "uptime_seconds": self.started.elapsed().as_secs(),
            "clients": clients,
            "active_turns": active_turns,
            "database": self.store.path().display().to_string(),
            "bundled": bundled,
        }))
    }

    fn daemon_shutdown(&self) -> Result<Value, RpcError> {
        let shutdown = self.shutdown.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            shutdown.cancel();
        });
        Ok(json!({"stopping": true}))
    }

    // MARK: Conversations

    fn conversations_list(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            #[serde(default)]
            archived: bool,
            #[serde(default)]
            limit: Option<usize>,
        }
        let params: Params = parse_params(params)?;
        let mut items = if params.archived {
            self.store.list_archived_conversations()
        } else {
            self.store.list_conversations()
        }
        .map_err(map_store)?;
        if let Some(limit) = params.limit {
            items.truncate(limit);
        }
        Ok(json!({"conversations": items}))
    }

    fn conversations_get(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            conversation_id: String,
        }
        let params: Params = parse_params(params)?;
        let conversation = self
            .store
            .conversation(&params.conversation_id)
            .map_err(map_store)?;
        let messages = self
            .store
            .messages(&params.conversation_id)
            .map_err(map_store)?;
        Ok(json!({"conversation": conversation, "messages": messages}))
    }

    fn conversations_create(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            #[serde(default)]
            model: Option<String>,
        }
        let params: Params = parse_params(params)?;
        let settings = self.load_settings();
        let model = params.model.unwrap_or(settings.default_model);
        if !agent::is_known_model(&model) {
            return Err(RpcError::invalid_params(format!("unknown model: {model}")));
        }
        let id = self.store.create_conversation(&model).map_err(map_store)?;
        let conversation = self.store.conversation(&id).map_err(map_store)?;
        self.broadcast(
            "conversations.changed",
            json!({"conversation_id": id, "action": "created"}),
        );
        Ok(json!({"conversation": conversation}))
    }

    fn conversations_rename(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            conversation_id: String,
            title: String,
        }
        let params: Params = parse_params(params)?;
        if params.title.trim().is_empty() {
            return Err(RpcError::invalid_params("title must not be empty"));
        }
        self.store
            .rename_conversation(&params.conversation_id, params.title.trim())
            .map_err(map_store)?;
        let conversation = self
            .store
            .conversation(&params.conversation_id)
            .map_err(map_store)?;
        self.broadcast(
            "conversations.changed",
            json!({"conversation_id": params.conversation_id, "action": "renamed"}),
        );
        Ok(json!({"conversation": conversation}))
    }

    fn conversations_delete(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            conversation_id: String,
        }
        let params: Params = parse_params(params)?;
        self.store
            .delete_conversation(&params.conversation_id)
            .map_err(map_store)?;
        self.broadcast(
            "conversations.changed",
            json!({"conversation_id": params.conversation_id, "action": "deleted"}),
        );
        Ok(json!({"conversation_id": params.conversation_id, "deleted": true}))
    }

    fn conversations_archive(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            conversation_id: String,
            #[serde(default = "default_true")]
            archived: bool,
        }
        let params: Params = parse_params(params)?;
        self.store
            .set_archived(&params.conversation_id, params.archived)
            .map_err(map_store)?;
        let conversation = self
            .store
            .conversation(&params.conversation_id)
            .map_err(map_store)?;
        self.broadcast(
            "conversations.changed",
            json!({"conversation_id": params.conversation_id, "action": "archived"}),
        );
        Ok(json!({"conversation": conversation}))
    }

    fn conversations_pin(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            conversation_id: String,
            #[serde(default = "default_true")]
            pinned: bool,
        }
        let params: Params = parse_params(params)?;
        self.store
            .set_pinned(&params.conversation_id, params.pinned)
            .map_err(map_store)?;
        let conversation = self
            .store
            .conversation(&params.conversation_id)
            .map_err(map_store)?;
        self.broadcast(
            "conversations.changed",
            json!({"conversation_id": params.conversation_id, "action": "pinned"}),
        );
        Ok(json!({"conversation": conversation}))
    }

    fn conversations_search(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            query: String,
        }
        let params: Params = parse_params(params)?;
        let items = self
            .store
            .search_conversations(&params.query)
            .map_err(map_store)?;
        Ok(json!({"conversations": items}))
    }

    fn conversations_export(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            conversation_id: String,
        }
        let params: Params = parse_params(params)?;
        let markdown = self
            .store
            .export_markdown(&params.conversation_id)
            .map_err(map_store)?;
        Ok(json!({
            "conversation_id": params.conversation_id,
            "markdown": markdown,
        }))
    }

    // MARK: Settings, models, and tools

    fn settings_get(&self) -> Result<Value, RpcError> {
        Ok(json!({"settings": self.load_settings()}))
    }

    fn settings_update(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            patch: Value,
        }
        let params: Params = parse_params(params)?;
        let patch = params
            .patch
            .as_object()
            .ok_or_else(|| RpcError::invalid_params("patch must be a JSON object"))?;
        let current = self.load_settings();
        let mut value = serde_json::to_value(&current)
            .map_err(|error| RpcError::internal(error.to_string()))?;
        if let Some(object) = value.as_object_mut() {
            for (key, value) in patch {
                object.insert(key.clone(), value.clone());
            }
        }
        let mut settings: AppSettings = serde_json::from_value(value)
            .map_err(|error| RpcError::invalid_params(error.to_string()))?;
        settings.normalize();
        self.store.save_settings(&settings).map_err(map_store)?;
        if let Ok(mut shared) = self.settings.write() {
            *shared = settings.clone();
        }
        mcp::init_global(&settings.mcp_servers);
        self.broadcast("settings.changed", json!({"settings": settings}));
        Ok(json!({"settings": settings}))
    }

    fn models_list(&self) -> Result<Value, RpcError> {
        let settings = self.load_settings();
        let mut models = Vec::new();
        for spec in relay_core::models() {
            let mut value = serde_json::to_value(spec)
                .map_err(|error| RpcError::internal(error.to_string()))?;
            if let Some(object) = value.as_object_mut() {
                object.insert("local".to_owned(), json!(false));
            }
            models.push(value);
        }
        for variant in bundled::BundledVariant::all() {
            models.push(json!({
                "slug": format!("local/{}", variant.id()),
                "api_id": variant.id(),
                "name": variant.display_name(),
                "provider": "openai-compatible",
                "tagline": variant.tagline(),
                "context_window": settings.bundled_context_size,
                "max_output": 4096,
                "local": true,
                "bundled": true,
                "downloaded": bundled::downloaded(variant),
                "server_installed": bundled::llama_server_path().is_some(),
            }));
        }
        if !settings.ollama_model.trim().is_empty() {
            models.push(json!({
                "slug": format!("ollama/{}", settings.ollama_model),
                "api_id": settings.ollama_model,
                "name": settings.ollama_model,
                "provider": "ollama",
                "local": true,
                "configured": true,
            }));
        }
        if !settings.local_model.trim().is_empty() {
            models.push(json!({
                "slug": format!("local/{}", settings.local_model),
                "api_id": settings.local_model,
                "name": settings.local_model,
                "provider": "openai-compatible",
                "local": true,
                "configured": true,
            }));
        }
        Ok(json!({"models": models, "default": settings.default_model}))
    }

    async fn models_ensure(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            model: String,
            #[serde(default)]
            download: bool,
        }
        let params: Params = parse_params(params)?;
        if !agent::is_known_model(&params.model) {
            return Err(RpcError::invalid_params(format!(
                "unknown model: {}",
                params.model
            )));
        }
        let model = agent::resolve_model(&params.model);
        let mut settings = self.load_settings();
        self.ensure_model_ready(&mut settings, &model, params.download)
            .await?;
        Ok(json!({"model": model.slug, "ready": true, "settings": settings}))
    }

    fn tools_list(&self) -> Result<Value, RpcError> {
        let settings = self.load_settings();
        Ok(json!({"tools": agent::tools_for(&settings)}))
    }

    fn presets_list(&self) -> Result<Value, RpcError> {
        let presets = self.store.presets().map_err(map_store)?;
        Ok(json!({"presets": presets}))
    }

    // MARK: Memory

    fn memory_list(&self) -> Result<Value, RpcError> {
        let memories = self.store.list_memories().map_err(map_store)?;
        let items: Vec<Value> = memories
            .into_iter()
            .map(|(id, content)| json!({"id": id, "content": content}))
            .collect();
        Ok(json!({"memories": items}))
    }

    fn memory_add(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            content: String,
        }
        let params: Params = parse_params(params)?;
        if params.content.trim().is_empty() {
            return Err(RpcError::invalid_params("content must not be empty"));
        }
        let id = self
            .store
            .add_memory(params.content.trim())
            .map_err(map_store)?;
        Ok(json!({"id": id}))
    }

    fn memory_delete(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            id: String,
        }
        let params: Params = parse_params(params)?;
        self.store.delete_memory(&params.id).map_err(map_store)?;
        Ok(json!({"id": params.id, "deleted": true}))
    }

    // MARK: Chat turns

    async fn chat_start(self: &Arc<Self>, params: Option<Value>) -> Result<Value, RpcError> {
        let (turn, model) = self.start_turn(params).await?;
        let started = StartedTurn {
            turn_id: turn.id.clone(),
            conversation_id: turn.conversation_id.clone(),
            model,
        };
        serde_json::to_value(started).map_err(|error| RpcError::internal(error.to_string()))
    }

    async fn chat_send(self: &Arc<Self>, params: Option<Value>) -> Result<Value, RpcError> {
        let (turn, _) = self.start_turn(params).await?;
        self.wait_for_turn(&turn, None).await
    }

    async fn chat_wait(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            turn_id: String,
            #[serde(default)]
            timeout_ms: Option<u64>,
        }
        let params: Params = parse_params(params)?;
        let turn = self.find_turn(&params.turn_id)?;
        self.wait_for_turn(&turn, params.timeout_ms).await
    }

    fn chat_cancel(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            turn_id: String,
        }
        let params: Params = parse_params(params)?;
        let turn = self.find_turn(&params.turn_id)?;
        turn.request_cancel();
        self.deny_turn_approvals(&turn.id);
        Ok(json!({"turn_id": params.turn_id, "cancelling": true}))
    }

    fn approval_respond(&self, params: Option<Value>) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct Params {
            approval_id: String,
            #[serde(default = "default_true")]
            approved: bool,
        }
        let params: Params = parse_params(params)?;
        let sender = self
            .approvals
            .lock()
            .map_err(|_| RpcError::internal("daemon state poisoned"))?
            .remove(&params.approval_id);
        match sender {
            Some(sender) => {
                let _ = sender.send(params.approved);
                Ok(json!({
                    "approval_id": params.approval_id,
                    "approved": params.approved,
                }))
            }
            None => Err(RpcError::not_found(format!(
                "approval {} is not pending",
                params.approval_id
            ))),
        }
    }

    async fn start_turn(
        self: &Arc<Self>,
        params: Option<Value>,
    ) -> Result<(Arc<Turn>, String), RpcError> {
        let params: StartTurnParams = parse_params(params)?;
        let mut settings = self.load_settings();

        if let Some(model) = &params.model
            && !agent::is_known_model(model)
        {
            return Err(RpcError::invalid_params(format!("unknown model: {model}")));
        }

        let conversation = match &params.conversation_id {
            Some(id) => self.store.conversation(id).map_err(map_store)?,
            None => {
                let model = params
                    .model
                    .clone()
                    .unwrap_or_else(|| settings.default_model.clone());
                if !agent::is_known_model(&model) {
                    return Err(RpcError::invalid_params(format!("unknown model: {model}")));
                }
                let id = self.store.create_conversation(&model).map_err(map_store)?;
                self.store.conversation(&id).map_err(map_store)?
            }
        };

        let model_slug = params
            .model
            .clone()
            .unwrap_or_else(|| conversation.model_slug.clone());
        if !agent::is_known_model(&model_slug) {
            return Err(RpcError::invalid_params(format!(
                "unknown model: {model_slug}"
            )));
        }
        if model_slug != conversation.model_slug {
            self.store
                .set_conversation_model(&conversation.id, &model_slug)
                .map_err(map_store)?;
        }
        let model = agent::resolve_model(&model_slug);
        self.ensure_model_ready(&mut settings, &model, false)
            .await?;

        let turn_id = params
            .turn_id
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        if turn_id.trim().is_empty() {
            return Err(RpcError::invalid_params("turn_id must not be empty"));
        }
        {
            let active = self
                .turns
                .lock()
                .map_err(|_| RpcError::internal("daemon state poisoned"))?;
            if active.contains_key(&turn_id) {
                return Err(RpcError::conflict(format!("turn {turn_id} already exists")));
            }
        }
        {
            let completed = self
                .completed
                .lock()
                .map_err(|_| RpcError::internal("daemon state poisoned"))?;
            if completed.iter().any(|(id, _)| id == &turn_id) {
                return Err(RpcError::conflict(format!("turn {turn_id} already exists")));
            }
        }
        {
            let mut active = self
                .conversation_turns
                .lock()
                .map_err(|_| RpcError::internal("daemon state poisoned"))?;
            if let Some(existing) = active.get(&conversation.id) {
                return Err(RpcError::conflict(format!(
                    "conversation already has active turn {existing}"
                )));
            }
            active.insert(conversation.id.clone(), turn_id.clone());
        }

        let result = self.start_turn_inner(
            &turn_id,
            &conversation,
            &model,
            &settings,
            &params.message,
            params.attachments,
        );
        if result.is_err()
            && let Ok(mut active) = self.conversation_turns.lock()
        {
            active.remove(&conversation.id);
        }
        result.map(|turn| (turn, model.slug))
    }

    fn start_turn_inner(
        self: &Arc<Self>,
        turn_id: &str,
        conversation: &ConversationSummary,
        model: &ModelSelection,
        settings: &AppSettings,
        message: &str,
        attachments: Vec<ContentBlock>,
    ) -> Result<Arc<Turn>, RpcError> {
        let mut blocks = Vec::new();
        if !message.trim().is_empty() {
            blocks.push(ContentBlock::text(message));
        }
        for block in attachments {
            if matches!(block, ContentBlock::Image(_)) {
                blocks.push(block);
            }
        }
        if blocks.is_empty() {
            return Err(RpcError::invalid_params(
                "message or image attachment required",
            ));
        }

        let user = ChatMessage::user_blocks(blocks);
        self.store
            .append_message(&conversation.id, &user)
            .map_err(map_store)?;
        let messages = self.store.messages(&conversation.id).map_err(map_store)?;
        let presets = self.store.presets().map_err(map_store)?;
        let memories = self.store.list_memories().map_err(map_store)?;
        let request = agent::prepare_request(settings, model, &messages, &presets, &memories);
        let started = streaming::start(
            &self.runtime,
            request,
            settings.agent_mode,
            settings.tool_approval,
            streaming::DelegateContext::from_settings(settings),
        )
        .map_err(|error| RpcError::internal(format!("could not start the request: {error}")))?;

        let turn = Arc::new(Turn::new(turn_id.to_owned(), conversation.id.clone()));
        turn.attach_cancel(started.cancel.clone());
        {
            let mut turns = self
                .turns
                .lock()
                .map_err(|_| RpcError::internal("daemon state poisoned"))?;
            turns.insert(turn.id.clone(), Arc::clone(&turn));
        }

        self.broadcast(
            "turn.started",
            json!({
                "turn_id": turn.id,
                "conversation_id": turn.conversation_id,
                "model": model.slug,
            }),
        );

        let daemon = Arc::clone(self);
        let task_turn = Arc::clone(&turn);
        let task_model = model.clone();
        tokio::spawn(async move {
            daemon.run_turn(task_turn, task_model, started).await;
        });
        Ok(turn)
    }

    async fn run_turn(
        self: Arc<Self>,
        turn: Arc<Turn>,
        model: ModelSelection,
        started: streaming::StartedStream,
    ) {
        let streaming::StartedStream {
            events,
            error,
            approvals,
            cancel,
        } = started;
        turn.attach_cancel(cancel);

        let mut message = ChatMessage::assistant(model.slug.clone(), Vec::new(), None);
        let mut text = String::new();
        let mut thinking = String::new();
        let mut usage: Option<Usage> = None;
        let mut approvals_open = true;

        loop {
            tokio::select! {
                event = events.recv() => {
                    match event {
                        Ok(StreamEvent::Text(chunk)) => {
                            text.push_str(&chunk);
                            message.append_text(&chunk);
                            self.broadcast("turn.text", json!({
                                "turn_id": turn.id,
                                "conversation_id": turn.conversation_id,
                                "delta": chunk,
                            }));
                        }
                        Ok(StreamEvent::Thinking(chunk)) => {
                            thinking.push_str(&chunk);
                            message.append_thinking(&chunk, model.provider.id());
                            self.broadcast("turn.thinking", json!({
                                "turn_id": turn.id,
                                "conversation_id": turn.conversation_id,
                                "delta": chunk,
                            }));
                        }
                        Ok(StreamEvent::Usage(next)) => {
                            if let Some(existing) = &mut usage {
                                existing.merge_max(next);
                            } else {
                                usage = Some(next);
                            }
                            message.usage = usage;
                            self.broadcast("turn.usage", json!({
                                "turn_id": turn.id,
                                "conversation_id": turn.conversation_id,
                                "usage": usage,
                            }));
                        }
                        Ok(StreamEvent::ToolCall(call)) => {
                            message.blocks.push(ContentBlock::ToolUse {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                arguments: call.arguments.clone(),
                            });
                            self.broadcast("turn.tool_call", json!({
                                "turn_id": turn.id,
                                "conversation_id": turn.conversation_id,
                                "call": call,
                            }));
                        }
                        Ok(StreamEvent::ToolResult { id, name, output }) => {
                            message.blocks.push(ContentBlock::ToolResult {
                                id: id.clone(),
                                output: output.clone(),
                                is_error: false,
                            });
                            self.broadcast("turn.tool_result", json!({
                                "turn_id": turn.id,
                                "conversation_id": turn.conversation_id,
                                "id": id,
                                "name": name,
                                "output": output,
                            }));
                        }
                        Ok(StreamEvent::Done { .. }) => {}
                        Err(_) => break,
                    }
                }
                approval = approvals.recv(), if approvals_open => {
                    match approval {
                        Ok(request) => self.request_approval(&turn, request),
                        Err(_) => approvals_open = false,
                    }
                }
            }
        }

        let error = error.recv().await.ok();
        let cancelled = turn.cancel_requested.load(Ordering::Relaxed);
        let reported_error = if cancelled { None } else { error.clone() };
        if let Some(error) = &reported_error {
            message.error = Some(error.clone());
        }
        let cost = usage.and_then(|usage| model.estimate_cost(&usage));

        let mut result = TurnResult {
            turn_id: turn.id.clone(),
            conversation_id: turn.conversation_id.clone(),
            model: model.slug.clone(),
            text,
            thinking,
            usage,
            cost_usd: cost,
            cancelled,
            error: reported_error,
            message_id: None,
            conversation: None,
        };

        let has_content = !message.text().trim().is_empty()
            || !message.thinking().trim().is_empty()
            || message.error.is_some();
        if has_content {
            match self.store.append_message(&turn.conversation_id, &message) {
                Ok(_) => result.message_id = Some(message.id.clone()),
                Err(error) => {
                    result.error = Some(format!("could not save the response: {error}"));
                }
            }
        }
        result.conversation = self.store.conversation(&turn.conversation_id).ok();

        // Retire the turn before announcing completion so a client that reacts
        // to `turn.completed` can immediately start the next turn.
        if let Ok(mut completed) = self.completed.lock() {
            completed.push_back((turn.id.clone(), Arc::clone(&turn)));
            while completed.len() > COMPLETED_TURNS {
                completed.pop_front();
            }
        }
        if let Ok(mut turns) = self.turns.lock() {
            turns.remove(&turn.id);
        }
        if let Ok(mut active) = self.conversation_turns.lock() {
            active.remove(&turn.conversation_id);
        }

        self.broadcast(
            "turn.completed",
            serde_json::to_value(&result).unwrap_or(Value::Null),
        );
        turn.finish(result.clone());
        self.broadcast(
            "conversations.changed",
            json!({"conversation_id": turn.conversation_id, "action": "message"}),
        );
    }

    fn request_approval(self: &Arc<Self>, turn: &Turn, request: streaming::ApprovalRequest) {
        let streaming::ApprovalRequest { call, respond } = request;
        let approval_id = format!(
            "{}-{}",
            turn.id,
            self.next_approval.fetch_add(1, Ordering::Relaxed)
        );
        if let Ok(mut approvals) = self.approvals.lock() {
            approvals.insert(approval_id.clone(), respond);
        }
        // A cancel may have landed between the provider asking for approval
        // and this turn observing the request.
        if turn.cancel_requested.load(Ordering::Acquire) {
            self.deny_turn_approvals(&turn.id);
            return;
        }
        self.broadcast(
            "approval.requested",
            json!({
                "turn_id": turn.id,
                "conversation_id": turn.conversation_id,
                "approval_id": approval_id,
                "call": call,
            }),
        );

        let daemon = Arc::clone(self);
        tokio::spawn(async move {
            tokio::time::sleep(APPROVAL_TIMEOUT).await;
            let sender = daemon
                .approvals
                .lock()
                .ok()
                .and_then(|mut approvals| approvals.remove(&approval_id));
            if let Some(sender) = sender {
                let _ = sender.send(false);
                daemon.broadcast("approval.expired", json!({"approval_id": approval_id}));
            }
        });
    }

    async fn wait_for_turn(&self, turn: &Turn, timeout_ms: Option<u64>) -> Result<Value, RpcError> {
        let timeout_ms = timeout_ms
            .unwrap_or(DEFAULT_WAIT_MS)
            .clamp(1, 24 * 60 * 60 * 1000);
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let mut receiver = turn.result_rx.clone();
        loop {
            if let Some(result) = receiver.borrow().as_ref().cloned() {
                return serde_json::to_value(result)
                    .map_err(|error| RpcError::internal(error.to_string()));
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(RpcError::timeout(format!(
                    "turn {} did not finish in time",
                    turn.id
                )));
            }
            if timeout(remaining, receiver.changed()).await.is_err() {
                return Err(RpcError::timeout(format!(
                    "turn {} did not finish in time",
                    turn.id
                )));
            }
        }
    }

    fn find_turn(&self, id: &str) -> Result<Arc<Turn>, RpcError> {
        if let Ok(turns) = self.turns.lock()
            && let Some(turn) = turns.get(id)
        {
            return Ok(Arc::clone(turn));
        }
        if let Ok(completed) = self.completed.lock()
            && let Some((_, turn)) = completed.iter().find(|(turn_id, _)| turn_id == id)
        {
            return Ok(Arc::clone(turn));
        }
        Err(RpcError::not_found(format!("turn {id} does not exist")))
    }

    async fn ensure_model_ready(
        &self,
        settings: &mut AppSettings,
        model: &ModelSelection,
        download: bool,
    ) -> Result<(), RpcError> {
        if let Some(variant) = bundled::BundledVariant::from_model_slug(&model.slug) {
            let running = bundled::probe_running().await;
            if !running {
                let installed =
                    bundled::downloaded(variant) && bundled::llama_server_path().is_some();
                if !installed && !download {
                    return Err(RpcError::model_not_ready(format!(
                        "bundled model {} is not installed; call models.ensure with download=true",
                        variant.display_name()
                    )));
                }
                bundled::ensure_ready(
                    Arc::clone(&self.bundled),
                    variant,
                    settings.bundled_context_size,
                )
                .await;
            }
            let status = self
                .bundled
                .lock()
                .map(|state| state.status())
                .unwrap_or_default();
            if !status.ready && !bundled::probe_running().await {
                return Err(RpcError::model_not_ready(status.error.unwrap_or_else(
                    || "the bundled model did not become ready".to_owned(),
                )));
            }
            settings.local_base_url = bundled::endpoint();
            settings.local_model = variant.id().to_owned();
            settings.bundled_model_variant = variant.id().to_owned();
            settings.normalize();
            self.store.save_settings(settings).map_err(map_store)?;
            if let Ok(mut shared) = self.settings.write() {
                *shared = settings.clone();
            }
            return Ok(());
        }

        if model.provider == Provider::Ollama && !relay_engine::local::installed(&model.api_id) {
            return Err(RpcError::model_not_ready(format!(
                "Ollama model {} is not installed; run `ollama pull {}`",
                model.api_id, model.api_id
            )));
        }
        if model.provider.requires_api_key() && keys::resolve(model.provider).is_none() {
            return Err(RpcError::model_not_ready(format!(
                "no API key is configured for {}; set {}",
                model.provider.label(),
                model.provider.env_keys().join(" or ")
            )));
        }
        Ok(())
    }

    fn load_settings(&self) -> AppSettings {
        let settings = self.store.load_settings().unwrap_or_else(|_| {
            self.settings
                .read()
                .map(|settings| settings.clone())
                .unwrap_or_default()
        });
        if let Ok(mut shared) = self.settings.write() {
            *shared = settings.clone();
        }
        settings
    }

    fn broadcast(&self, method: &str, params: Value) {
        let notification = RpcNotification::new(method, params);
        let Ok(line) = serde_json::to_string(&notification) else {
            return;
        };
        if let Ok(clients) = self.clients.read() {
            for client in clients.values() {
                if client.authorized.load(Ordering::Relaxed) {
                    let _ = client.tx.send(line.clone());
                }
            }
        }
    }

    fn cancel_all_turns(&self) {
        let turns: Vec<Arc<Turn>> = self
            .turns
            .lock()
            .map(|turns| turns.values().cloned().collect())
            .unwrap_or_default();
        for turn in turns {
            turn.request_cancel();
            self.deny_turn_approvals(&turn.id);
        }
    }

    /// Deny pending approvals for one turn so cancellation cannot leave the
    /// provider task blocked on a decision that will never arrive.
    fn deny_turn_approvals(&self, turn_id: &str) {
        let prefix = format!("{turn_id}-");
        let senders = {
            let Ok(mut approvals) = self.approvals.lock() else {
                return;
            };
            let ids: Vec<String> = approvals
                .keys()
                .filter(|id| id.starts_with(&prefix))
                .cloned()
                .collect();
            ids.into_iter()
                .filter_map(|id| approvals.remove(&id))
                .collect::<Vec<_>>()
        };
        for sender in senders {
            let _ = sender.send(false);
        }
    }
}

#[derive(Deserialize)]
struct StartTurnParams {
    #[serde(default)]
    conversation_id: Option<String>,
    #[serde(default)]
    message: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    attachments: Vec<ContentBlock>,
    #[serde(default)]
    turn_id: Option<String>,
}

fn parse_params<T: DeserializeOwned>(params: Option<Value>) -> Result<T, RpcError> {
    let value = params.unwrap_or_else(|| Value::Object(serde_json::Map::new()));
    serde_json::from_value(value).map_err(|error| RpcError::invalid_params(error.to_string()))
}

fn default_true() -> bool {
    true
}

fn map_store(error: relay_store::StoreError) -> RpcError {
    if matches!(&error, relay_store::StoreError::NotFound(_)) {
        RpcError::not_found(error.to_string())
    } else {
        RpcError::new(-32010, error.to_string())
    }
}

fn generate_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

fn restrict_dir(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not restrict {}: {error}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn cleanup(dir: &Path) {
    #[cfg(unix)]
    let _ = std::fs::remove_file(socket_path(dir));
    let _ = std::fs::remove_file(info_path(dir));
    let _ = std::fs::remove_file(token_path(dir));
}

enum Listener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg(not(unix))]
    Tcp(TcpListener),
}

impl Listener {
    async fn accept(&self) -> std::io::Result<BoxStream> {
        match self {
            #[cfg(unix)]
            Self::Unix(listener) => {
                let (stream, _) = listener.accept().await?;
                Ok(Box::new(stream))
            }
            #[cfg(not(unix))]
            Self::Tcp(listener) => {
                let (stream, _) = listener.accept().await?;
                let _ = stream.set_nodelay(true);
                Ok(Box::new(stream))
            }
        }
    }
}

async fn create_listener(dir: &Path) -> Result<(Listener, Endpoint), String> {
    #[cfg(unix)]
    {
        let path = socket_path(dir);
        if path.exists() {
            match std::os::unix::net::UnixStream::connect(&path) {
                Ok(_) => {
                    return Err(format!(
                        "another Relay daemon is already listening on {}",
                        path.display()
                    ));
                }
                Err(_) => {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let listener = tokio::net::UnixListener::bind(&path)
            .map_err(|error| format!("could not bind {}: {error}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|error| format!("could not restrict {}: {error}", path.display()))?;
        }
        Ok((
            Listener::Unix(listener),
            Endpoint::Unix {
                path: path.display().to_string(),
            },
        ))
    }
    #[cfg(not(unix))]
    {
        if let Ok(info) = crate::protocol::read_info_from(dir)
            && let Endpoint::Tcp { address } = &info.endpoint
            && matches!(
                timeout(
                    Duration::from_millis(250),
                    tokio::net::TcpStream::connect(address)
                )
                .await,
                Ok(Ok(_))
            )
        {
            return Err(format!(
                "another Relay daemon is already listening on {address}"
            ));
        }
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(|error| format!("could not bind the daemon TCP socket: {error}"))?;
        let address = listener
            .local_addr()
            .map_err(|error| format!("could not read the daemon address: {error}"))?;
        Ok((
            Listener::Tcp(listener),
            Endpoint::Tcp {
                address: address.to_string(),
            },
        ))
    }
}
