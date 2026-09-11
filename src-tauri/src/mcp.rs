//! Minimal Model Context Protocol (MCP) client. Speaks JSON-RPC 2.0
//! over stdio to a child process. Loads server configurations from
//! `mcp_servers.json` in the app data directory and wraps each
//! server's tools as MaxBot `Tool` implementations.
//!
//! Scope (v0.4.6): stdio transport only, no auth, no resources, no
//! prompts. Tools/list + tools/call. Servers are started on app
//! startup; failures are logged and the server is skipped (the
//! rest of the app keeps working). Config file is re-read on
//! demand via the `reload_mcp_servers` Tauri command.
//!
//! The protocol spec lives at
//! <https://modelcontextprotocol.io/specification/2024-11-05>. We
//! implement just enough for tool use — initialize,
//! notifications/initialized, tools/list, tools/call.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};

use crate::tools::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

const PROTOCOL_VERSION: &str = "2024-11-05";
const INIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// One entry in `mcp_servers.json`. A server is a child process we
/// speak JSON-RPC 2.0 to over its stdin/stdout.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    /// Human-readable name. Surfaced in the Settings UI.
    pub name: String,
    /// Executable to run (e.g. "npx", "uvx", "/usr/local/bin/mcp-fs").
    pub command: String,
    /// Args passed to the command. For "npx" this is typically
    /// ["-y", "@modelcontextprotocol/server-filesystem", "/path"].
    #[serde(default)]
    pub args: Vec<String>,
    /// Optional env vars merged into the child process's environment.
    #[serde(default)]
    pub env: HashMap<String, String>,
}

/// The shape of a tool as the server reports it via tools/list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolDef {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Value,
}

/// A running MCP server. Owns the child process, the stdin writer,
/// and a background task that reads responses off stdout and
/// dispatches them to the pending request. The `pending` map is
/// shared between McpServer (writer) and the reader task (dispatcher)
/// via `Arc<Mutex<...>>` so that new requests from `request()` are
/// visible to the reader immediately.
pub struct McpServer {
    name: String,
    stdin: tokio::process::ChildStdin,
    next_id: i64,
    pending: Arc<std::sync::Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
    _child: Arc<Mutex<Child>>,
    _reader: tokio::task::JoinHandle<()>,
    stderr_task: Option<tokio::task::JoinHandle<()>>,
}

struct PendingRequest {
    id: i64,
    pending: Arc<std::sync::Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
}

impl Drop for PendingRequest {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}

impl Drop for McpServer {
    fn drop(&mut self) {
        self._reader.abort();
        if let Some(task) = &self.stderr_task {
            task.abort();
        }
    }
}

impl McpServer {
    /// Spawn the configured child process, perform the initialize
    /// handshake, and return a shared handle. The child stays running
    /// for the lifetime of the process; tool calls multiplex over the
    /// single stdin/stdout pair via the JSON-RPC id correlator.
    pub async fn start(
        config: McpServerConfig,
    ) -> Result<Arc<Mutex<Self>>, String> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("spawn {}: {}", config.command, e))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "no stdin pipe".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "no stdout pipe".to_string())?;
        let stderr = child.stderr.take();
        let child_arc = Arc::new(Mutex::new(child));

        // Background task: read stdout line by line, parse each
        // line as a JSON-RPC response, dispatch by id to the pending
        // oneshot. The pending map is shared with McpServer so
        // requests issued after start can register entries the
        // reader will see.
        let pending: Arc<std::sync::Mutex<HashMap<i64, oneshot::Sender<Value>>>> =
            Arc::new(std::sync::Mutex::new(HashMap::new()));
        let (init_tx, init_rx) = oneshot::channel::<Value>();
        pending.lock().unwrap().insert(1, init_tx);
        let pending_for_reader = pending.clone();
        let reader = tokio::spawn(read_responses(stdout, pending_for_reader));

        // Background task: forward child stderr to log::warn! at info
        // level so a misbehaving server is visible without crashing
        // the app.
        let stderr_task = stderr.map(|stderr| {
            let server_name = config.name.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => return,
                        Ok(_) => {
                            let trimmed = line.trim_end();
                            if !trimmed.is_empty() {
                                log::info!("[mcp:{} stderr] {}", server_name, trimmed);
                            }
                        }
                    }
                }
            })
        });

        let mut server = Self {
            name: config.name.clone(),
            stdin,
            next_id: 2,
            pending: pending.clone(),
            _child: child_arc,
            _reader: reader,
            stderr_task,
        };

        // Send initialize request.
        let init_request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "MaxBot",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });
        server.write_request(&init_request).await?;

        // Wait for the initialize response.
        let resp = tokio::time::timeout(INIT_TIMEOUT, init_rx)
            .await
            .map_err(|_| format!("initialize timed out after {}s", INIT_TIMEOUT.as_secs()))?
            .map_err(|_| "initialize channel closed".to_string())?;
        if let Some(err) = resp.get("error") {
            return Err(format!("initialize error: {}", err));
        }
        log::info!(
            "mcp: server '{}' initialized, serverInfo={}",
            config.name,
            resp.get("result")
                .and_then(|r| r.get("serverInfo"))
                .cloned()
                .unwrap_or(Value::Null)
        );

        // Send the initialized notification (no response expected).
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        server.write_request(&notif).await?;

        Ok(Arc::new(Mutex::new(server)))
    }

    async fn write_request(&mut self, request: &Value) -> Result<(), String> {
        let mut line =
            serde_json::to_string(request).map_err(|e| e.to_string())?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| format!("write to {}: {}", self.name, e))?;
        self.stdin
            .flush()
            .await
            .map_err(|e| format!("flush to {}: {}", self.name, e))?;
        Ok(())
    }

    /// Send a JSON-RPC request, await the response, return the
    /// `result` field (or an error string).
    pub async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let (tx, rx) = oneshot::channel();
        // Register before writing: a local server can reply immediately.
        self.pending.lock().unwrap().retain(|_, sender| !sender.is_closed());
        self.pending.lock().unwrap().insert(id, tx);
        let _registration = PendingRequest { id, pending: self.pending.clone() };
        if let Err(error) = self.write_request(&request).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(error);
        }
        let response = tokio::time::timeout(REQUEST_TIMEOUT, rx).await;
        self.pending.lock().unwrap().remove(&id);
        let resp = response
            .map_err(|_| {
                format!(
                    "{}: request {} timed out after {}s",
                    self.name,
                    method,
                    REQUEST_TIMEOUT.as_secs()
                )
            })?
            .map_err(|_| format!("{}: response channel closed", self.name))?;
        if let Some(err) = resp.get("error") {
            return Err(format!("{}: {}", self.name, err));
        }
        Ok(resp.get("result").cloned().unwrap_or(Value::Null))
    }

    pub async fn list_tools(&mut self) -> Result<Vec<McpToolDef>, String> {
        let result = self.request("tools/list", json!({})).await?;
        let tools = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let defs: Vec<McpToolDef> = serde_json::from_value(Value::Array(tools))
            .map_err(|e| format!("parse tools/list: {}", e))?;
        Ok(defs)
    }

    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolResult, String> {
        let result = self
            .request("tools/call", json!({ "name": name, "arguments": arguments }))
            .await?;
        let is_error = result
            .get("isError")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let content = result
            .get("content")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut out = String::new();
        for item in content {
            // MCP content items: { type: "text", text: "..." } is the
            // only kind we currently handle.
            if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(text);
            } else if let Some(kind) = item.get("type").and_then(|v| v.as_str()) {
                // Unknown content type — surface the JSON so the model
                // at least sees what came back.
                out.push_str(&format!("[{}] ", kind));
                if let Ok(s) = serde_json::to_string(&item) {
                    out.push_str(&s);
                }
            }
        }
        if out.is_empty() {
            // Server returned no text content. Surface the raw
            // result so the model can still react.
            out = serde_json::to_string(&result).unwrap_or_default();
        }
        if is_error {
            Ok(ToolResult::err(out))
        } else {
            Ok(ToolResult::ok(out))
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Spawns the Python smoke server (or any external process) and
    /// runs the full JSON-RPC handshake + tools/list + tools/call
    /// against it. The smoke server is committed at
    /// `/tmp/maxbot-mcp-smoke.py` for local testing; this test is
    /// gated on its presence so it doesn't fail in environments where
    /// the file isn't there.
    #[tokio::test]
    async fn end_to_end_against_smoke_server() {
        let path = "/tmp/maxbot-mcp-smoke.py";
        if !std::path::Path::new(path).exists() {
            eprintln!("skipping: {} not present", path);
            return;
        }
        let config = McpServerConfig {
            name: "smoke".to_string(),
            command: "python3".to_string(),
            args: vec![path.to_string()],
            env: HashMap::new(),
        };
        let server = McpServer::start(config)
            .await
            .expect("server start");
        let mut locked = server.lock().await;

        let tools = locked.list_tools().await.expect("tools/list");
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"maxbot_smoke_echo"));
        assert!(names.contains(&"maxbot_smoke_weather"));

        let result = locked
            .call_tool(
                "maxbot_smoke_echo",
                json!({ "message": "hello from MaxBot" }),
            )
            .await
            .expect("call echo");
        assert!(!result.is_error);
        assert_eq!(result.content, "echo: hello from MaxBot");

        let result = locked
            .call_tool("maxbot_smoke_weather", json!({ "city": "Boulder" }))
            .await
            .expect("call weather");
        assert!(!result.is_error);
        assert!(result.content.contains("Boulder"));
    }

    #[tokio::test]
    async fn immediate_replies_and_cancelled_requests_release_resources() {
        let server = McpServer::start(McpServerConfig {
            name: "regression".into(),
            command: "python3".into(),
            args: vec!["-u".into(), "-c".into(), r#"
import sys, json
for line in sys.stdin:
    request = json.loads(line)
    if "id" in request and request["method"] != "hang":
        print(json.dumps({"jsonrpc":"2.0", "id":request["id"], "result":{}}), flush=True)
"#.into()],
            env: HashMap::new(),
        }).await.unwrap();
        let mut locked = server.lock().await;
        for _ in 0..100 {
            locked.request("echo", json!({})).await.unwrap();
        }
        for _ in 0..10 {
            assert!(tokio::time::timeout(Duration::from_millis(10), locked.request("hang", json!({}))).await.is_err());
            assert!(locked.pending.lock().unwrap().is_empty());
        }
        let reader = locked._reader.abort_handle();
        let stderr = locked.stderr_task.as_ref().unwrap().abort_handle();
        drop(locked);
        drop(server);
        tokio::task::yield_now().await;
        assert!(reader.is_finished());
        assert!(stderr.is_finished());
    }

    #[tokio::test]
    async fn timeout_when_server_doesnt_respond() {
        // Sleep is the simplest "doesn't respond" server. We expect
        // the start handshake to time out, not hang forever.
        let config = McpServerConfig {
            name: "sleeper".to_string(),
            command: "sleep".to_string(),
            args: vec!["30".to_string()],
            env: HashMap::new(),
        };
        let result =
            tokio::time::timeout(Duration::from_secs(20), McpServer::start(config))
                .await;
        assert!(result.is_err() || result.unwrap().is_err());
    }
}

async fn read_responses<R>(
    stdout: R,
    pending: Arc<std::sync::Mutex<HashMap<i64, oneshot::Sender<Value>>>>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let v: Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if let Some(id) = v.get("id").and_then(|x| x.as_i64()) {
                    if let Some(tx) = pending.lock().unwrap().remove(&id) {
                        let _ = tx.send(v);
                    }
                }
                // Lines without an id are notifications or
                // requests-from-server, which we currently ignore.
            }
            Err(_) => break,
        }
    }
    // Drain any pending requests with an EOF error so callers don't
    // hang forever.
    let mut p = pending.lock().unwrap();
    for (_, tx) in p.drain() {
        let _ = tx.send(json!({ "error": { "code": -1, "message": "EOF" } }));
    }
}

/// Load and start the MCP servers listed in `mcp_servers.json` at
/// `path`. A missing file is treated as "no servers configured" and
/// is not an error. Per-server startup failures are logged and the
/// server is skipped; the rest of the app keeps working.
pub async fn load_from_config_path(
    path: &Path,
) -> Result<McpRegistry, String> {
    let mut registry = McpRegistry::default();
    if !path.exists() {
        return Ok(registry);
    }
    let text = tokio::fs::read_to_string(path)
        .await
        .map_err(|e| format!("read {}: {}", path.display(), e))?;
    let configs: Vec<McpServerConfig> = if text.trim().is_empty() {
        Vec::new()
    } else {
        serde_json::from_str(&text)
            .map_err(|e| format!("parse {}: {}", path.display(), e))?
    };
    for config in configs {
        let name = config.name.clone();
        match McpServer::start(config).await {
            Ok(server) => {
                // List tools inside a scoped lock, then drop the
                // lock before calling add_server (which moves the
                // Arc).
                let tools_result = {
                    let mut locked = server.lock().await;
                    locked.list_tools().await
                };
                match tools_result {
                    Ok(tools) => {
                        log::info!(
                            "mcp: server '{}' loaded with {} tool(s)",
                            name,
                            tools.len()
                        );
                        registry.add_server(name, server, tools);
                    }
                    Err(e) => {
                        log::warn!("mcp: server '{}' tools/list failed: {}", name, e);
                    }
                }
            }
            Err(e) => {
                log::warn!("mcp: server '{}' failed to start: {}", name, e);
            }
        }
    }
    Ok(registry)
}

/// Holds running MCP servers and the MaxBot `Tool` adapters that
/// wrap each server's tool definitions. The registry is the way the
/// rest of the app reaches MCP-backed tools.
#[derive(Default, Clone)]
pub struct McpRegistry {
    servers: Vec<Arc<Mutex<McpServer>>>,
    adapters: Vec<Arc<dyn Tool>>,
}

impl std::fmt::Debug for McpRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpRegistry")
            .field("servers", &self.servers.len())
            .field("adapters", &self.adapters.len())
            .finish()
    }
}

impl McpRegistry {
    /// Add a running server and the list of tools it reported. Each
    /// tool gets its own `McpToolAdapter` and the adapters are what
    /// the ToolRegistry consumes. The name is passed separately so
    /// we don't need to lock the server just to read its name.
    pub fn add_server(
        &mut self,
        name: String,
        server: Arc<Mutex<McpServer>>,
        tools: Vec<McpToolDef>,
    ) {
        self.servers.push(server.clone());
        for def in tools {
            self.adapters.push(Arc::new(McpToolAdapter {
                full_name: format!("mcp__{}__{}", name, def.name),
                server_name: name.clone(),
                tool_name: def.name,
                description: def.description,
                parameters_schema: def.input_schema,
                server: server.clone(),
            }));
        }
    }

    /// The dynamic tools contributed by all loaded MCP servers. Pass
    /// this to `ToolRegistry::default_with_extras()`.
    pub fn tool_adapters(&self) -> Vec<Arc<dyn Tool>> {
        self.adapters.clone()
    }

    pub fn server_count(&self) -> usize {
        self.servers.len()
    }
}

/// A single MCP tool exposed as a MaxBot tool. The name is mangled
/// (`mcp__<server>__<tool>`) so the model can see distinct tool names
/// per server without name collisions. All MCP tools require
/// per-call consent (the server could do anything).
pub struct McpToolAdapter {
    full_name: String,
    server_name: String,
    tool_name: String,
    description: String,
    parameters_schema: Value,
    server: Arc<Mutex<McpServer>>,
}

#[async_trait]
impl Tool for McpToolAdapter {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        // MCP servers return JSON Schema with `properties` and
        // `required` at the top level. The model's tool spec wants
        // the same shape, so we forward as-is.
        let mut s = self.parameters_schema.clone();
        if !s.is_object() {
            s = json!({
                "type": "object",
                "properties": {},
                "additionalProperties": true
            });
        }
        if s.get("type").is_none() {
            if let Some(obj) = s.as_object_mut() {
                obj.insert("type".to_string(), json!("object"));
            }
        }
        s
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if !context.consent_granted {
            return Err(ToolError::Execution(format!(
                "user denied the {} action",
                self.full_name
            )));
        }
        let mut server = self.server.lock().await;
        server
            .call_tool(&self.tool_name, invocation.arguments)
            .await
            .map_err(|e| {
                ToolError::Execution(format!(
                    "mcp[{}] {}: {}",
                    self.server_name, self.tool_name, e
                ))
            })
    }
}
