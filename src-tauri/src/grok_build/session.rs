//! `grok agent stdio` JSON-RPC client (ACP — Agent Client Protocol).
//!
//! Spawn `grok agent stdio` as a single long-lived subprocess and
//! speak JSON-RPC 2.0 over its stdin/stdout. Multi-turn: send
//! `session/prompt` repeatedly in the same `sessionId`; resume
//! after app relaunch by persisting the id to SQLite.
//!
//! Wire format (verified against `grok 1.0.24` on this host):
//!
//! Initialization (one round-trip each, request → response):
//!   → initialize{ protocolVersion: 1, clientCapabilities: {fs, terminal} }
//!   ← result{ authMethods: [{id: "xai.api_key"}, {id: "cached_token"}, ...] }
//!   → authenticate{ methodId: "cached_token", _meta: { headless: true } }
//!   ← result{}
//!   → session/new{ cwd, mcpServers: [] }
//!   ← result{ sessionId: "01a08..." }
//!
//! Sending a prompt (one round-trip + streaming):
//!   → session/prompt{ sessionId, prompt: [{type:"text", text:"..."}] }
//!   ← session/update{ update: { sessionUpdate: "agent_message_chunk", content: {text} } }
//!     … (many chunks, in order)
//!   ← result{ stopReason: "end_turn" }
//!
//! The streaming `session/update` events arrive on the same wire as
//! the final result. We split them with two channels: response
//! messages (with an `id` field) go to a `pending` map keyed by
//! request id, and `session/update` notifications go to a broadcast
//! channel so multiple prompt subscribers can see the stream.
//!
//! Subprocess layout:
//!   - One writer task owns the `ChildStdin` and pumps bytes from an
//!     mpsc channel. Public API just enqueues `WriteBytes`.
//!   - One read task owns the `ChildStdout` and parses NDJSON,
//!     dispatching responses vs. notifications.
//!   - The child handle lives in `Inner.child` so `Drop` can kill
//!     the subprocess cleanly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock as StdRwLock};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, mpsc, oneshot};

/// Per-handshake-step timeout. Initialize / authenticate / session/new
/// are quick round-trips against `grok agent stdio`.
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Per-prompt response timeout — the request itself, not the
/// stream. The agent's chunks arrive on the broadcast channel
/// concurrently.
const PROMPT_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
/// Channel capacity for the broadcast stream. Big enough for a
/// long streamed response without lagging.
const EVENT_CHANNEL_CAPACITY: usize = 256;
/// Drain window after the prompt's final result comes back. Any
/// chunks buffered in the broadcast are collected here.
const POST_RESULT_DRAIN: std::time::Duration = std::time::Duration::from_millis(200);

/// One chunk of a session's streamed response. Surfaced to the
/// tool result and (later) the chat UI.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    /// A text chunk from the streamed assistant message.
    Chunk(String),
    /// The agent invoked a tool. Surfaced for the UI to show a
    /// progress line; the text result still comes through chunks.
    /// The name/args are part of the event but no current consumer
    /// reads them (the tool dispatcher is a separate `ToolUse`
    /// channel on the `GrokSession` itself).
    #[allow(dead_code)]
    ToolUse { name: String, args: Value },
    /// Something went wrong inside the agent (network blip, model
    /// refusal, EOF). `prompt` prefers a non-empty chunk buffer
    /// over this when both are present. The string payload is the
    /// human-readable message and is logged at warn level by the
    /// receiving side.
    Error(String),
}

#[derive(Debug)]
enum WriteCommand {
    WriteBytes(Vec<u8>),
    Shutdown,
}

/// The shared, cloneable handle to a running `grok agent stdio`
/// subprocess + JSON-RPC client. Cheap to clone (Arc inside).
#[derive(Clone)]
pub struct GrokSession {
    inner: Arc<Inner>,
    binary_path: Arc<str>,
    model_alias: Arc<str>,
    cwd: PathBuf,
}

struct Inner {
    child: StdMutex<Option<Child>>,
    writer_tx: mpsc::UnboundedSender<WriteCommand>,
    pending: StdMutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>,
    next_id: AtomicU64,
    session_id: StdRwLock<Option<String>>,
    event_tx: broadcast::Sender<SessionEvent>,
}

impl GrokSession {
    /// Spawn `grok --cwd <dir> agent --model <alias>
    /// --always-approve stdio` and complete the ACP handshake.
    /// The session is ready to receive `prompt` calls when
    /// this returns.
    ///
    /// Flag layout verified against `grok 1.0.24` on this
    /// host: `--cwd` lives on the parent `grok` command,
    /// `--model` / `--always-approve` live on `grok agent`,
    /// and `stdio` is the subcommand that takes over stdin /
    /// stdout. The `stdio` subcommand itself only accepts
    /// `--debug` / `--debug-file` / `--leader-socket`.
    pub async fn start_subprocess(
        binary_path: Arc<str>,
        cwd: std::path::PathBuf,
        model_alias: Arc<str>,
        resume_session_id: Option<String>,
    ) -> Result<Self, String> {
        let mut cmd = Command::new(binary_path.as_ref());
        cmd.arg("--cwd")
            .arg(&cwd)
            .arg("agent")
            .arg("--model")
            .arg(model_alias.as_ref())
            .arg("--always-approve")
            .arg("stdio");
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(false);
        let mut child: Child = cmd
            .spawn()
            .map_err(|e| format!("spawn '{}': {e}", binary_path))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "could not capture child stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "could not capture child stdout".to_string())?;
        // Drain stderr so the child doesn't block on a full pipe.
        if let Some(se) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(se).lines();
                while let Ok(Some(_line)) = lines.next_line().await {}
            });
        }
        Self::start_with_io(
            stdout,
            stdin,
            binary_path,
            cwd,
            model_alias,
            resume_session_id,
        )
        .await
    }

    /// Lower-level constructor that takes the subprocess's stdout
    /// and stdin as generic async I/O. Used by the real
    /// `start_subprocess` and by tests, which pass a
    /// `tokio::io::duplex` pair to simulate the subprocess without
    /// spawning a real binary.
    pub async fn start_with_io<R, W>(
        reader: R,
        writer: W,
        binary_path: Arc<str>,
        cwd: PathBuf,
        model_alias: Arc<str>,
        resume_session_id: Option<String>,
    ) -> Result<Self, String>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (writer_tx, writer_rx) = mpsc::unbounded_channel::<WriteCommand>();
        let (event_tx, _) = broadcast::channel::<SessionEvent>(EVENT_CHANNEL_CAPACITY);
        let inner = Arc::new(Inner {
            child: StdMutex::new(None), // no Child in the IO-only path
            writer_tx,
            pending: StdMutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            session_id: StdRwLock::new(resume_session_id.clone()),
            event_tx: event_tx.clone(),
        });
        // Writer task: dumb byte pump.
        tokio::spawn(writer_task(writer, writer_rx));
        // Read task: NDJSON parser, dispatches responses to
        // `pending` and notifications to `event_tx`.
        tokio::spawn(read_task(inner.clone(), reader));
        let session = Self {
            inner: inner.clone(),
            binary_path,
            model_alias,
            cwd,
        };
        // Do the handshake. We need a fresh session id unless we're
        // resuming (in which case the agent already knows it).
        session.handshake(resume_session_id.is_none()).await?;
        Ok(session)
    }

    async fn handshake(&self, create_new_session: bool) -> Result<(), String> {
        let init = self
            .send_request(
                "initialize",
                serde_json::json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": { "readTextFile": true, "writeTextFile": true },
                        "terminal": true,
                    },
                }),
                HANDSHAKE_TIMEOUT,
            )
            .await?;
        // Pick an auth method. Prefer `cached_token` (already
        // authenticated locally via `~/.grok/auth.json`); fall
        // back to `xai.api_key`.
        let methods: Vec<String> = init
            .get("authMethods")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let method_id = if methods.iter().any(|m| m == "cached_token") {
            "cached_token"
        } else if methods.iter().any(|m| m == "xai.api_key") {
            "xai.api_key"
        } else {
            return Err(format!(
                "no usable auth method; authMethods = {methods:?}. \
                 Run `grok login` or set XAI_API_KEY."
            ));
        };
        self.send_request(
            "authenticate",
            serde_json::json!({
                "methodId": method_id,
                "_meta": { "headless": true },
            }),
            HANDSHAKE_TIMEOUT,
        )
        .await?;
        if create_new_session {
            let res = self
                .send_request(
                    "session/new",
                    serde_json::json!({
                        "cwd": self.cwd.to_string_lossy(),
                        "mcpServers": [],
                    }),
                    HANDSHAKE_TIMEOUT,
                )
                .await?;
            let id = res
                .get("sessionId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| "session/new response missing sessionId".to_string())?
                .to_string();
            *self
                .inner
                .session_id
                .write()
                .map_err(|e| format!("session_id lock poisoned: {e}"))? = Some(id);
        }
        Ok(())
    }

    /// The current session id (set after handshake). Persist this
    /// to SQLite and pass back as `resume_session_id` on the next
    /// `start` call to resume the same conversation.
    pub fn session_id(&self) -> Option<String> {
        self.inner.session_id.read().ok().and_then(|g| g.clone())
    }

    pub fn binary_path(&self) -> &str {
        &self.binary_path
    }

    pub fn model_alias(&self) -> &str {
        &self.model_alias
    }

    /// Send a raw JSON-RPC request and await the response. Used
    /// by `prompt` and by handshake steps.
    pub async fn send_request(
        &self,
        method: &str,
        params: Value,
        timeout: std::time::Duration,
    ) -> Result<Value, String> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (response_tx, response_rx) = oneshot::channel();
        // Insert into pending BEFORE writing so the read task can
        // dispatch a fast response without dropping it on the floor.
        self.inner
            .pending
            .lock()
            .map_err(|e| format!("pending lock poisoned: {e}"))?
            .insert(id, response_tx);
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("serialize {method}: {e}"))?;
        bytes.push(b'\n');
        self.inner
            .writer_tx
            .send(WriteCommand::WriteBytes(bytes))
            .map_err(|_| "writer task closed".to_string())?;
        match tokio::time::timeout(timeout, response_rx).await {
            Ok(Ok(r)) => r,
            Ok(Err(_)) => {
                // Read task dropped the oneshot (e.g. EOF). Clean up.
                if let Ok(mut p) = self.inner.pending.lock() {
                    p.remove(&id);
                }
                Err(format!("{method}: response channel dropped"))
            }
            Err(_) => {
                if let Ok(mut p) = self.inner.pending.lock() {
                    p.remove(&id);
                }
                Err(format!("{method} timed out after {timeout:?}"))
            }
        }
    }

    /// Send a prompt to the running session and return the
    /// assistant's reply. Streams chunks via the broadcast
    /// channel concurrently; the returned string is the
    /// concatenation of all chunks.
    pub async fn prompt(&self, prompt_text: &str) -> Result<String, String> {
        let session_id = self
            .session_id()
            .ok_or_else(|| "session not initialized".to_string())?;
        // Subscribe BEFORE sending so we don't miss any chunks.
        // The read task is already running; new chunks from the
        // agent will be broadcast to all subscribers.
        let mut events = self.inner.event_tx.subscribe();
        // Send the prompt. The result is the final response with
        // stopReason; chunks come on `events` before that.
        let prompt_fut = self.send_request(
            "session/prompt",
            serde_json::json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": prompt_text}],
            }),
            PROMPT_RESPONSE_TIMEOUT,
        );
        tokio::pin!(prompt_fut);
        let mut text = String::new();
        loop {
            tokio::select! {
                biased;
                res = &mut prompt_fut => {
                    // Drain any remaining chunks for
                    // POST_RESULT_DRAIN before returning.
                    let _result = res?;
                    let drain_until = tokio::time::Instant::now() + POST_RESULT_DRAIN;
                    loop {
                        tokio::select! {
                            ev = events.recv() => match ev {
                                Ok(SessionEvent::Chunk(s)) => text.push_str(&s),
                                Ok(_) => {}
                                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                                Err(broadcast::error::RecvError::Closed) => break,
                            },
                            _ = tokio::time::sleep_until(drain_until) => break,
                        }
                    }
                    return Ok(text);
                }
                ev = events.recv() => match ev {
                    Ok(SessionEvent::Chunk(s)) => text.push_str(&s),
                    Ok(SessionEvent::ToolUse { .. }) => {
                        // Tool uses are silent. The final
                        // prompt result decides the text.
                    }
                    Ok(SessionEvent::Error(msg)) => {
                        // Pre-result errors get logged so
                        // debugging is possible without
                        // surfacing them to the model;
                        // the final prompt result decides
                        // the text.
                        if !msg.is_empty() {
                            log::warn!("grok session pre-result error: {msg}");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        return Err("session stream closed before result".to_string());
                    }
                }
            }
        }
    }

    /// Send a shutdown signal to the writer and kill the
    /// subprocess. Idempotent.
    pub fn close(&self) {
        let _ = self.inner.writer_tx.send(WriteCommand::Shutdown);
        if let Ok(mut g) = self.inner.child.lock() {
            if let Some(mut c) = g.take() {
                let _ = c.start_kill();
            }
        }
    }
}

impl Drop for GrokSession {
    fn drop(&mut self) {
        self.close();
    }
}

async fn writer_task<W>(mut writer: W, mut rx: mpsc::UnboundedReceiver<WriteCommand>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    while let Some(cmd) = rx.recv().await {
        match cmd {
            WriteCommand::WriteBytes(bytes) => {
                if let Err(e) = writer.write_all(&bytes).await {
                    log::warn!("grok writer: write_all failed: {e}");
                    break;
                }
                if let Err(e) = writer.flush().await {
                    log::warn!("grok writer: flush failed: {e}");
                    break;
                }
            }
            WriteCommand::Shutdown => break,
        }
    }
    let _ = writer.shutdown().await;
}

async fn read_task<R>(inner: Arc<Inner>, reader: R)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let snippet: String = line.chars().take(200).collect();
                let _ = inner.event_tx.send(SessionEvent::Error(format!(
                    "malformed JSON-RPC line: {e}; line: {snippet}"
                )));
                continue;
            }
        };
        // Notification?
        if msg.get("method").and_then(|m| m.as_str()) == Some("session/update") {
            let ev = parse_session_update(&msg);
            let _ = inner.event_tx.send(ev);
            continue;
        }
        // Response to a pending request?
        if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
            if let Ok(mut pending) = inner.pending.lock() {
                if let Some(tx) = pending.remove(&id) {
                    let result = if let Some(err) = msg.get("error") {
                        Err(err
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("unknown")
                            .to_string())
                    } else {
                        Ok(msg.get("result").cloned().unwrap_or(Value::Null))
                    };
                    let _ = tx.send(result);
                }
            }
            continue;
        }
        // Unknown shape — log at debug, don't surface to the model.
        let snippet: String = line.chars().take(200).collect();
        log::debug!("grok read: unknown message shape: {snippet}");
    }
    // EOF: surface as an Error so any in-flight prompt() can decide
    // whether to surface it.
    let _ = inner
        .event_tx
        .send(SessionEvent::Error("session closed (EOF)".to_string()));
}

fn parse_session_update(msg: &Value) -> SessionEvent {
    let update = msg.get("params").and_then(|p| p.get("update"));
    let kind = update
        .and_then(|u| u.get("sessionUpdate"))
        .and_then(|s| s.as_str())
        .unwrap_or("");
    match kind {
        "agent_message_chunk" => {
            let text = update
                .and_then(|u| u.get("content"))
                .and_then(|c| c.get("text"))
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string();
            SessionEvent::Chunk(text)
        }
        "tool_use" => {
            let name = update
                .and_then(|u| u.get("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string();
            let args = update
                .and_then(|u| u.get("args"))
                .cloned()
                .unwrap_or(Value::Null);
            SessionEvent::ToolUse { name, args }
        }
        other => SessionEvent::Error(format!("unknown sessionUpdate: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    /// Fake JSON-RPC server end. Owns the read + write halves
    /// from the `tokio::io::duplex` pair and exposes typed
    /// helpers for the common shapes (response to a request id,
    /// `session/update` notification).
    struct FakeServer {
        reader: BufReader<tokio::io::ReadHalf<DuplexStream>>,
        writer: tokio::io::WriteHalf<DuplexStream>,
    }

    impl FakeServer {
        fn from_split(
            read: tokio::io::ReadHalf<DuplexStream>,
            write: tokio::io::WriteHalf<DuplexStream>,
        ) -> Self {
            Self {
                reader: BufReader::new(read),
                writer: write,
            }
        }

        /// Read one NDJSON line and parse it as a JSON-RPC request.
        async fn recv_request(&mut self) -> Value {
            let mut buf = String::new();
            self.reader
                .read_line(&mut buf)
                .await
                .expect("read_line");
            serde_json::from_str(&buf).expect("parse request")
        }

        /// Write a `result` response keyed to a request's id.
        async fn send_result(&mut self, id: u64, result: Value) {
            self.write_ndjson(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": result,
            }))
            .await;
        }

        /// Write a `session/update` agent_message_chunk.
        async fn send_chunk(&mut self, text: &str) {
            self.write_ndjson(&json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"text": text}
                    }
                }
            }))
            .await;
        }

        /// Write a JSON-RPC error result keyed to a request id.
        async fn send_error(&mut self, id: u64, message: &str) {
            self.write_ndjson(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32600, "message": message}
            }))
            .await;
        }

        async fn write_ndjson(&mut self, v: &Value) {
            let mut bytes = serde_json::to_vec(v).expect("serialize");
            bytes.push(b'\n');
            self.writer.write_all(&bytes).await.expect("write");
            self.writer.flush().await.expect("flush");
        }
    }

    /// Drive the three-step handshake (initialize →
    /// authenticate → session/new) and return a ready session +
    /// the FakeServer for further scripted responses.
    ///
    /// The session's `start_with_io` blocks on the handshake,
    /// which needs the server to respond. So we run the server
    /// driver in a background task concurrently with the client,
    /// and use a oneshot to return the server once the handshake
    /// is done. (Awaiting the client future on its own would
    /// deadlock: the test wouldn't get a chance to drive the
    /// server until the handshake completed, which it can't
    /// without the test driving the server.)
    async fn boot_with_handshake() -> (GrokSession, FakeServer) {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (server_read, server_write) = tokio::io::split(server_side);
        let mut server = FakeServer::from_split(server_read, server_write);
        let (server_tx, server_rx) = tokio::sync::oneshot::channel();

        // Server task: drive the 3-step handshake, then hand
        // `server` back via the oneshot.
        let server_task = tokio::spawn(async move {
            // 1. initialize
            let req = server.recv_request().await;
            assert_eq!(req["method"], "initialize");
            server
                .send_result(
                    req["id"].as_u64().unwrap(),
                    json!({
                        "authMethods": [
                            {"id": "cached_token"},
                            {"id": "xai.api_key"}
                        ]
                    }),
                )
                .await;
            // 2. authenticate
            let req = server.recv_request().await;
            assert_eq!(req["method"], "authenticate");
            server
                .send_result(req["id"].as_u64().unwrap(), json!({}))
                .await;
            // 3. session/new
            let req = server.recv_request().await;
            assert_eq!(req["method"], "session/new");
            server
                .send_result(
                    req["id"].as_u64().unwrap(),
                    json!({"sessionId": "01test_session_abc"}),
                )
                .await;
            // Hand the server back.
            let _ = server_tx.send(server);
        });

        let session = GrokSession::start_with_io(
            client_read,
            client_write,
            Arc::from("fake-grok"),
            PathBuf::from("/tmp/fake-grok-cwd"),
            Arc::from("test-model"),
            None,
        )
        .await
        .expect("session start");

        let server = server_rx.await.expect("server back");
        let _ = server_task.await;
        (session, server)
    }

    #[tokio::test]
    async fn handshake_sets_session_id() {
        let (session, _server) = boot_with_handshake().await;
        assert_eq!(
            session.session_id().as_deref(),
            Some("01test_session_abc"),
            "session_id should be set after handshake"
        );
        assert_eq!(session.binary_path(), "fake-grok");
        assert_eq!(session.model_alias(), "test-model");
    }

    #[tokio::test]
    async fn prompt_collects_chunks_into_text() {
        let (session, mut server) = boot_with_handshake().await;
        // Subscribe BEFORE the prompt so we can verify the
        // broadcast path emits chunks.
        let mut events = session.inner.event_tx.subscribe();

        // Drive the prompt in a background task so the test can
        // assert on the broadcast channel independently.
        let session_clone = session.clone();
        let prompt_task =
            tokio::spawn(async move { session_clone.prompt("say hello").await });

        // Server: receive the prompt, send 2 chunks + result.
        let req = server.recv_request().await;
        assert_eq!(req["method"], "session/prompt");
        let id = req["id"].as_u64().unwrap();
        server.send_chunk("Hello, ").await;
        server.send_chunk("world!").await;
        server.send_result(id, json!({"stopReason": "end_turn"})).await;

        let text = prompt_task.await.expect("prompt task").expect("prompt ok");
        assert_eq!(text, "Hello, world!", "chunks must concatenate in order");

        // Broadcast channel should have delivered the chunks too.
        let mut drained = Vec::new();
        while let Ok(ev) = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            events.recv(),
        )
        .await
        {
            match ev {
                Ok(SessionEvent::Chunk(s)) => drained.push(s),
                _ => break,
            }
        }
        assert_eq!(
            drained.concat(),
            "Hello, world!",
            "broadcast channel also surfaces chunks"
        );
    }

    #[tokio::test]
    async fn prompt_surfaces_error_from_result() {
        let (session, mut server) = boot_with_handshake().await;
        let session_clone = session.clone();
        let prompt_task =
            tokio::spawn(async move { session_clone.prompt("anything").await });

        let req = server.recv_request().await;
        let id = req["id"].as_u64().unwrap();
        server.send_error(id, "model refused").await;

        let result = prompt_task.await.expect("prompt task");
        assert!(result.is_err(), "jsonrpc error result must surface as Err");
        let err = result.unwrap_err();
        assert!(
            err.contains("model refused"),
            "expected 'model refused' in {err}"
        );
    }

    #[tokio::test]
    async fn unknown_auth_method_rejects_at_start() {
        // Build a server that responds to initialize with an
        // auth list lacking cached_token and xai.api_key. The
        // client should reject at handshake time.
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (server_read, server_write) = tokio::io::split(server_side);

        let server_task = tokio::spawn(async move {
            let mut server = FakeServer::from_split(server_read, server_write);
            let req = server.recv_request().await;
            server
                .send_result(
                    req["id"].as_u64().unwrap(),
                    json!({"authMethods": [{"id": "grok.com"}]}),
                )
                .await;
        });

        let result = GrokSession::start_with_io(
            client_read,
            client_write,
            Arc::from("fake-grok"),
            PathBuf::from("/tmp/fake-grok-cwd"),
            Arc::from("test-model"),
            None,
        )
        .await;
        assert!(result.is_err(), "must fail without a usable auth method");
        let err = match result {
            Ok(_) => panic!("expected Err, got Ok"),
            Err(e) => e,
        };
        assert!(
            err.contains("no usable auth method"),
            "unexpected error: {err}"
        );

        let _ = server_task.await;
    }

    #[tokio::test]
    async fn resume_session_id_is_preserved() {
        // If the caller passes a `resume_session_id`, the
        // handshake must NOT issue a session/new; the existing
        // id is used as-is.
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (server_read, server_write) = tokio::io::split(server_side);

        let server_task = tokio::spawn(async move {
            let mut server = FakeServer::from_split(server_read, server_write);
            // initialize
            let req = server.recv_request().await;
            server
                .send_result(
                    req["id"].as_u64().unwrap(),
                    json!({"authMethods": [{"id": "cached_token"}]}),
                )
                .await;
            // authenticate
            let req = server.recv_request().await;
            server
                .send_result(req["id"].as_u64().unwrap(), json!({}))
                .await;
            // No session/new. The client must NOT send one. We
            // close the server side after a short grace period;
            // the test passes if `start_with_io` returned Ok.
            // The 100ms timeout below catches a misbehaving
            // client that does try to send session/new.
            let mut line = String::new();
            let _ = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                server.reader.read_line(&mut line),
            )
            .await;
        });

        let session = GrokSession::start_with_io(
            client_read,
            client_write,
            Arc::from("fake-grok"),
            PathBuf::from("/tmp/fake-grok-cwd"),
            Arc::from("test-model"),
            Some("01existing_resume".to_string()),
        )
        .await
        .expect("session start (resume path)");

        assert_eq!(
            session.session_id().as_deref(),
            Some("01existing_resume"),
            "resume id must be used as-is, no session/new"
        );

        let _ = server_task.await;
    }

    #[tokio::test]
    async fn prompt_after_eof_errors_cleanly() {
        // If the subprocess dies after handshake, the next
        // prompt should return an error rather than hang.
        let (session, server) = boot_with_handshake().await;
        // Drop the server → client read sees EOF.
        drop(server);
        // Tiny grace period for the read task to surface the
        // EOF on the broadcast channel.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let result = session.prompt("anything").await;
        assert!(result.is_err(), "prompt after EOF must error");
    }
}
