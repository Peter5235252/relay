//! Minimal MCP (Model Context Protocol) client over stdio.
//!
//! Spawns configured servers, performs the JSON-RPC `initialize` handshake,
//! discovers their tools, and routes `tools/call` requests.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::sync::oneshot;

use relay_core::McpServerConfig;

#[derive(Clone, Debug)]
pub struct McpTool {
    pub server: String,
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

impl Drop for McpServer {
    fn drop(&mut self) {
        if let Ok(mut g) = self.child.lock()
            && let Some(mut c) = g.take()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub struct McpServer {
    name: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<std::process::ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    next_id: AtomicU64,
}

impl McpServer {
    pub fn start(config: &McpServerConfig) -> Result<Arc<Self>, String> {
        // Honor PATHEXT on Windows so commands like `npx` and `uvx` resolve to
        // their .cmd/.bat shims instead of failing with "not found".
        let program = relay_core::platform::resolve_program(&config.command)
            .unwrap_or_else(|| std::path::PathBuf::from(&config.command));
        let mut cmd = Command::new(&program);
        cmd.args(&config.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt as _;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        for kv in &config.env {
            if let Some((k, v)) = kv.split_once('=') {
                cmd.env(k, v);
            }
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {e}", config.command))?;
        let stdin = child.stdin.take().ok_or("no stdin")?;
        let stdout = child.stdout.take().ok_or("no stdout")?;

        let server = Arc::new(Self {
            name: config.name.clone(),
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
        });

        // Reader thread dispatches responses.
        let pending = server.pending.clone();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines().map_while(Result::ok) {
                let line = line.trim().to_owned();
                if line.is_empty() {
                    continue;
                }
                if let Ok(v) = serde_json::from_str::<Value>(&line)
                    && let Some(id) = v.get("id").and_then(|x| x.as_u64())
                    && let Some(tx) = pending.lock().ok().and_then(|mut m| m.remove(&id))
                {
                    let _ = tx.send(v);
                }
            }
        });

        Ok(server)
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, mut rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|e| e.to_string())?
            .insert(id, tx);
        let msg = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params});
        {
            let mut g = self.stdin.lock().map_err(|e| e.to_string())?;
            let stdin = g.as_mut().ok_or("stdin closed")?;
            writeln!(stdin, "{msg}").map_err(|e| e.to_string())?;
            stdin.flush().map_err(|e| e.to_string())?;
        }
        // Block on response with timeout.
        let resp = tokio::task::block_in_place(|| {
            // Use a simple polling wait since we're sync here.
            let start = std::time::Instant::now();
            loop {
                if start.elapsed() > std::time::Duration::from_secs(30) {
                    return Err("timeout".to_owned());
                }
                // try receive without blocking forever
                match rx.try_recv() {
                    Ok(v) => return Ok(v),
                    Err(oneshot::error::TryRecvError::Closed) => return Err("closed".to_owned()),
                    Err(oneshot::error::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
            }
        })?;
        if let Some(err) = resp.get("error") {
            return Err(err.to_string());
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub fn initialize(&self) -> Result<(), String> {
        let _ = self.request(
            "initialize",
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name":"relay","version":"0.1.8"}
            }),
        )?;
        // Send initialized notification (no response expected).
        let msg = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        if let Ok(mut g) = self.stdin.lock()
            && let Some(stdin) = g.as_mut()
        {
            let _ = writeln!(stdin, "{msg}");
            let _ = stdin.flush();
        }
        Ok(())
    }

    pub fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let res = self.request("tools/list", json!({}))?;
        let mut out = Vec::new();
        if let Some(tools) = res.get("tools").and_then(|t| t.as_array()) {
            for t in tools {
                let name = t
                    .get("name")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_owned();
                let desc = t
                    .get("description")
                    .and_then(|x| x.as_str())
                    .unwrap_or("")
                    .to_owned();
                let params = t
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or(json!({"type":"object"}));
                out.push(McpTool {
                    server: self.name.clone(),
                    name,
                    description: desc,
                    parameters: params,
                });
            }
        }
        Ok(out)
    }

    pub fn call_tool(&self, tool: &str, args: Value) -> Result<String, String> {
        let res = self.request("tools/call", json!({"name":tool,"arguments":args}))?;
        // Extract text content.
        if let Some(content) = res.get("content").and_then(|c| c.as_array()) {
            let mut s = String::new();
            for c in content {
                if let Some(t) = c.get("text").and_then(|x| x.as_str()) {
                    s.push_str(t);
                    s.push('\n');
                }
            }
            if !s.is_empty() {
                return Ok(s);
            }
        }
        Ok(res.to_string())
    }
}

pub struct McpRegistry {
    pub servers: Vec<(McpServerConfig, Arc<McpServer>)>,
    pub tools: Vec<McpTool>,
}

impl McpRegistry {
    pub fn load(configs: &[McpServerConfig]) -> Self {
        let mut servers = Vec::new();
        let mut tools = Vec::new();
        for c in configs {
            if c.command.trim().is_empty() {
                continue;
            }
            match McpServer::start(c) {
                Ok(srv) => {
                    if let Err(e) = srv.initialize() {
                        eprintln!("MCP {} init failed: {e}", c.name);
                        continue;
                    }
                    match srv.list_tools() {
                        Ok(ts) => {
                            for t in ts {
                                tools.push(t);
                            }
                        }
                        Err(e) => eprintln!("MCP {} list_tools failed: {e}", c.name),
                    }
                    servers.push((c.clone(), srv));
                }
                Err(e) => eprintln!("MCP {} start failed: {e}", c.name),
            }
        }
        Self { servers, tools }
    }

    pub fn tool_specs(&self) -> Vec<relay_core::ToolSpec> {
        self.tools
            .iter()
            .map(|t| relay_core::ToolSpec {
                name: format!("mcp__{}__{}", t.server, t.name),
                description: format!("[MCP:{}] {}", t.server, t.description),
                parameters: t.parameters.clone(),
            })
            .collect()
    }

    pub fn call(&self, prefixed: &str, args: Value) -> String {
        // prefixed: mcp__<server>__<tool>
        let rest = prefixed.trim_start_matches("mcp__");
        let mut parts = rest.splitn(2, "__");
        let server_name = parts.next().unwrap_or("");
        let tool_name = parts.next().unwrap_or("");
        for (cfg, srv) in &self.servers {
            if cfg.name == server_name {
                return srv
                    .call_tool(tool_name, args)
                    .unwrap_or_else(|e| format!("error: {e}"));
            }
        }
        "error: MCP server not found".to_owned()
    }
}

// Global registry.
static REGISTRY: std::sync::OnceLock<Arc<Mutex<Option<McpRegistry>>>> = std::sync::OnceLock::new();

pub fn init_global(configs: &[McpServerConfig]) {
    let reg = McpRegistry::load(configs);
    let cell = REGISTRY.get_or_init(|| Arc::new(Mutex::new(None)));
    if let Ok(mut g) = cell.lock() {
        *g = Some(reg);
    }
}

pub fn global_tools() -> Vec<relay_core::ToolSpec> {
    REGISTRY
        .get()
        .and_then(|c| c.lock().ok())
        .and_then(|g| g.as_ref().map(|r| r.tool_specs()))
        .unwrap_or_default()
}

pub fn global_call(name: &str, args: Value) -> Option<String> {
    REGISTRY.get().and_then(|c| {
        c.lock()
            .ok()
            .and_then(|g| g.as_ref().map(|r| r.call(name, args)))
    })
}
