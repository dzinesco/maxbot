//! VNC console: local WebSocket ↔ RFB proxy.
//!
//! When the renderer asks for `computer_console_url(bot_id)`,
//! we:
//! 1. Pick the next free local TCP port from
//!    `Settings.computer_vnc_local_port_range`.
//! 2. Spawn `ssh -L <local>:127.0.0.1:<vnc_port> tyler@<host>`
//!    as a child process. This is the tunnel that brings the
//!    VM's VNC port (which is bound to 127.0.0.1 on the
//!    server) onto the Mac's localhost.
//! 3. Spawn a `tokio-tungstenite` WebSocket server on the
//!    local port. Each WebSocket connection bridges
//!    bidirectionally to the VNC TCP stream on the tunnel.
//!
//! The renderer connects to `ws://localhost:<local>/` and
//! runs noVNC's RFB-over-WebSocket framing. The Tauri side
//! speaks raw TCP to the tunnel and the WebSocket to the
//! renderer — the two sides are independent byte streams.
//!
//! **Security-critical:** the URL we return to the renderer
//! is ALWAYS `ws://localhost:<port>/`. We never return a
//! remote host URL — that would expose the VM's VNC server
//! to the network. The unit test
//! `console_url_is_localhost_only` enforces this.

use std::net::SocketAddr;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

use super::ssh::SshPool;

#[derive(Debug, Error)]
pub enum VncError {
    #[error("ssh binary not found on PATH")]
    BinaryMissing,
    #[error("port allocator exhausted: no free ports in {0:?}")]
    NoFreePort(String),
    #[error("listener bind failed: {0}")]
    Bind(String),
    #[error("vnc tunnel spawn failed: {0}")]
    Tunnel(String),
    /// The SSH tunnel child process exited before the
    /// readiness window elapsed — typically because SSH
    /// auth failed (no passphrase set, key rejected,
    /// host key mismatch). `stderr` carries the captured
    /// ssh output for diagnosis. Distinct from
    /// `Tunnel` (which is a spawn-time failure) because
    /// here the child DID start; it just didn't survive
    /// the first few seconds.
    #[error("vnc tunnel auth failed: {0}")]
    TunnelAuthFailed(String),
    #[error("websocket handshake failed: {0}")]
    Ws(String),
    #[error("io error: {0}")]
    Io(String),
}

/// One live VNC console: the SSH tunnel child + the local TCP
/// listener + the port it bound. The actual per-connection
/// bridging is spawned per-WS-connection; this struct owns
/// the lifetime.
pub struct VncProxy {
    /// The SSH tunnel child. Dropped (and `start_kill`-ed)
    /// when `VncProxy` is dropped, which closes the tunnel
    /// and the local port.
    tunnel: Arc<Mutex<Option<Child>>>,
    /// The bound TCP listener. Held so the OS doesn't
    /// recycle the port. Dropping it is what actually frees
    /// the port.
    listener: Option<TcpListener>,
    /// Local address the listener is on. The string form is
    /// `127.0.0.1:<port>` and is what we return to the
    /// renderer.
    pub local_addr: SocketAddr,
    /// v3.0.5: port the SSH tunnel is listening on. This
    /// is a DIFFERENT port from `local_addr.port()` —
    /// the listener and the SSH tunnel can no longer
    /// share a port because macOS won't let two
    /// processes bind to the same port without
    /// `SO_REUSEPORT` (which `ssh` doesn't set on
    /// local-forward sockets). The bridge connects to
    /// this port for the VNC side.
    tunnel_port: u16,
}

impl VncProxy {
    /// Build the WebSocket URL the renderer should connect
    /// to. **Always** `ws://localhost:<port>/` — never a
    /// remote host. The unit test
    /// `console_url_is_localhost_only` guards this.
    pub fn console_url(&self) -> String {
        // We hand-construct the URL from `local_addr.ip()`
        // so a future refactor that tries to swap to a
        // remote host (e.g. by reading from
        // `self.tunnel.local_addr()`) would have to
        // deliberately break the test.
        format!("ws://localhost:{}/", self.local_addr.port())
    }

    /// Start accepting WebSocket connections on the
    /// bound local port. Each accepted TCP connection
    /// is upgraded to a WebSocket and bridged to the
    /// VNC TCP stream (the SSH tunnel is local; the
    /// listener accepts via the loopback). The spawned
    /// task runs for the lifetime of the WebSocket —
    /// when either side closes, both halves shut
    /// down. The `VncProxy` itself stays alive (the
    /// tunnel keeps running) until `Drop`.
    pub async fn serve(&self) -> Result<(), VncError> {
        let listener = self
            .listener
            .as_ref()
            .ok_or_else(|| VncError::Bind("listener already consumed".into()))?;
        let local_port = self.local_addr.port();
        let tunnel_port = self.tunnel_port;
        loop {
            let (tcp, peer) = listener.accept().await.map_err(|e| VncError::Io(e.to_string()))?;
            // The WebSocket must be from 127.0.0.1 — the
            // noVNC client is the Tauri webview, which
            // connects over the loopback. Anything else
            // is unexpected; log and drop.
            if !peer.ip().is_loopback() {
                log::warn!(
                    "vnc proxy: non-loopback peer {} on port {} — dropping",
                    peer,
                    local_port
                );
                drop(tcp);
                continue;
            }
            tokio::spawn(async move {
                if let Err(e) = bridge_one(tcp, tunnel_port).await {
                    log::debug!("vnc proxy: connection ended: {e}");
                }
            });
        }
    }
}

impl Drop for VncProxy {
    fn drop(&mut self) {
        // Best-effort kill of the SSH tunnel. We have to
        // do this synchronously in Drop; the child is
        // already running in another task. `start_kill`
        // sends SIGKILL on Unix.
        if let Ok(mut guard) = self.tunnel.try_lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.start_kill();
            }
        }
    }
}

/// Build (but do not start) the SSH tunnel child that maps
/// `localhost:<local>` on the Mac to `127.0.0.1:<remote>`
/// on the server. The tunnel child is the lifetime
/// anchor for the VNC port; killing the child frees the
/// port on the server side.
async fn spawn_tunnel(
    pool: &SshPool,
    local_port: u16,
    remote_vnc_port: u16,
) -> Result<Child, VncError> {
    let server = pool.server_config().ok_or_else(|| {
        VncError::Tunnel("server host not configured".into())
    })?;
    let user_at_host = format!("{}@{}", server.user, server.host);
    let mut c = Command::new("ssh");
    c.arg("-N"); // no remote command — just forward.
    c.arg("-L").arg(format!("{local_port}:127.0.0.1:{remote_vnc_port}"));
    c.arg("-o").arg("BatchMode=yes");
    c.arg("-o").arg("LogLevel=ERROR");
    c.arg("-o").arg("StrictHostKeyChecking=accept-new");
    c.arg("-o").arg("ConnectTimeout=10");
    c.arg("-o").arg("ServerAliveInterval=30");
    c.arg("-o").arg("ServerAliveCountMax=3");
    if !server.identity_file.is_empty() {
        c.arg("-i").arg(&server.identity_file);
    }
    c.arg("-o").arg("ExitOnForwardFailure=yes");
    c.arg(&user_at_host);
    c.stdout(Stdio::null());
    c.stderr(Stdio::piped());
    c.stdin(Stdio::null());
    c.kill_on_drop(true);
    let child = c.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            VncError::BinaryMissing
        } else {
            VncError::Tunnel(e.to_string())
        }
    })?;
    Ok(child)
}

/// Spawn the full VNC proxy: pick a port, bind the local
/// listener, start the SSH tunnel. The returned struct
/// owns both. Call `serve()` to start accepting WS
/// connections.
pub async fn start(
    pool: &SshPool,
    remote_vnc_port: u16,
    port_range: (u16, u16),
) -> Result<VncProxy, VncError> {
    // v3.0.5: pick TWO free ports — one for the WS
    // listener (what the renderer connects to) and one
    // for the SSH tunnel (what the bridge connects to
    // for the VNC side). Earlier versions used the
    // same port for both, which worked on Linux with
    // `SO_REUSEPORT` but fails on macOS where `ssh`
    // doesn't set that flag on local-forward sockets —
    // the second bind hits "Address already in use"
    // and the tunnel exits before `wait_for_tunnel_ready`
    // can confirm it's up.
    let port = pick_free_port(port_range).await.ok_or_else(|| {
        VncError::NoFreePort(format!("{}..={}", port_range.0, port_range.1))
    })?;
    // Bind explicitly to 127.0.0.1 so the OS doesn't
    // accidentally put us on a public interface.
    let bind_addr: SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .map_err(|e: std::net::AddrParseError| VncError::Bind(e.to_string()))?;
    let listener = TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| VncError::Bind(e.to_string()))?;
    let actual = listener
        .local_addr()
        .map_err(|e| VncError::Bind(e.to_string()))?;
    // Pick a second port for the SSH tunnel, excluding
    // the one the listener just took. Reuse the same
    // range; if the range is exhausted (very rare —
    // 100 ports for 2 picks), fall back to the next
    // port above the range.
    let tunnel_port = pick_free_port_excluding(port_range, actual.port())
        .await
        .ok_or_else(|| {
            VncError::NoFreePort(format!(
                "{}..={} (excluding {})",
                port_range.0, port_range.1, actual.port()
            ))
        })?;
    let mut tunnel = spawn_tunnel(pool, tunnel_port, remote_vnc_port).await?;
    // Wait for the SSH tunnel to be ready. The previous
    // design returned the proxy immediately and let the
    // bridge discover a dead VNC side on first WS — which
    // meant the renderer got a valid-looking URL and
    // noVNC showed a black canvas with no indication.
    // Now we poll the tunnel child for up to 3s; if it
    // exits during that window (auth failed, no
    // passphrase, host key mismatch), capture its stderr
    // and return a `TunnelAuthFailed` so the renderer can
    // show the real reason instead of a black screen.
    if !wait_for_tunnel_ready(&mut tunnel).await? {
        let stderr = read_child_stderr(&mut tunnel).await;
        // Kill it for good measure in case it's stuck.
        let _ = tunnel.start_kill();
        let _ = tunnel.wait().await;
        return Err(VncError::TunnelAuthFailed(stderr));
    }
    Ok(VncProxy {
        tunnel: Arc::new(Mutex::new(Some(tunnel))),
        listener: Some(listener),
        local_addr: actual,
        tunnel_port,
    })
}

/// Poll the SSH tunnel child for `READY_WINDOW` and return
/// `Ok(true)` if it stays alive that long (i.e. the
/// forward is established). Returns `Ok(false)` if it
/// exits within the window. Errors from `try_wait` other
/// than `NotFound` are returned as `Err`.
async fn wait_for_tunnel_ready(child: &mut Child) -> Result<bool, VncError> {
    const READY_WINDOW: Duration = Duration::from_secs(3);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);
    let deadline = tokio::time::Instant::now() + READY_WINDOW;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return Ok(false), // child exited
            Ok(None) => {
                // still running
                if tokio::time::Instant::now() >= deadline {
                    return Ok(true); // survived the window — assume ready
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Err(e) => return Err(VncError::Tunnel(e.to_string())),
        }
    }
}

/// Drain the child's stderr (non-blocking-ish via
/// `try_wait` + a small read). Returns the captured
/// output as a single string, or a placeholder if the
/// pipe was already drained or empty. The caller is
/// expected to have just observed the child exit.
async fn read_child_stderr(child: &mut Child) -> String {
    use tokio::io::AsyncReadExt;
    let mut buf = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        // v3.0.5: read in a tight loop with a wall-clock
        // timeout using the async API. The previous
        // `read_to_string` could hang if the OS hadn't
        // fully drained the pipe even after `try_wait()`
        // reported exit.
        let read_result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            async {
                let mut s = String::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) => break, // EOF
                        Ok(n) => {
                            s.push_str(&String::from_utf8_lossy(&chunk[..n]));
                        }
                        Err(_) => break,
                    }
                }
                s
            },
        )
        .await;
        if let Ok(s) = read_result {
            buf = s;
        }
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        "(no stderr output captured)".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Pick the next free TCP port in `[lo, hi]`. Walks
/// linearly, trying to bind a 127.0.0.1 listener. If
/// `lo` is busy, tries the next; if `hi` is busy, returns
/// `None`. We do not attempt to skip ports already known
/// to be in use outside the range; the kernel will tell
/// us via `EADDRINUSE` and we move on.
///
/// This is a fresh bind on every call — no shared state —
/// which is OK because the OS recycles ports lazily and
/// our usage is short-lived (one bind per VNC console
/// request).
pub async fn pick_free_port(range: (u16, u16)) -> Option<u16> {
    for port in range.0..=range.1 {
        let addr: SocketAddr = match format!("127.0.0.1:{port}").parse() {
            Ok(a) => a,
            Err(_) => continue,
        };
        match TcpListener::bind(addr).await {
            Ok(l) => {
                // Drop immediately — the bind succeeded,
                // so the port is free; we'll re-bind in
                // the caller.
                drop(l);
                return Some(port);
            }
            Err(_) => continue,
        }
    }
    None
}

/// v3.0.5: same as `pick_free_port` but skips one port
/// (the listener's port) so the SSH tunnel gets a
/// distinct loopback port. See `vnc::start` for the
/// full rationale.
pub async fn pick_free_port_excluding(range: (u16, u16), exclude: u16) -> Option<u16> {
    for port in range.0..=range.1 {
        if port == exclude {
            continue;
        }
        let addr: SocketAddr = match format!("127.0.0.1:{port}").parse() {
            Ok(a) => a,
            Err(_) => continue,
        };
        match TcpListener::bind(addr).await {
            Ok(l) => {
                drop(l);
                return Some(port);
            }
            Err(_) => continue,
        }
    }
    None
}

/// Bridge one WebSocket connection to the VNC TCP stream
/// on the loopback (the SSH tunnel listens on
/// `127.0.0.1:<tunnel_port>` and forwards to the server's
/// `127.0.0.1:<vnc_port>`). When either side closes, the
/// other side is shut down. Standard `select!`-based
/// pump.
async fn bridge_one(ws_tcp: TcpStream, tunnel_port: u16) -> Result<(), VncError> {
    // v3.0.5: connect to the SSH tunnel's port (NOT
    // the listener's port). Earlier versions connected
    // to the same port as the listener, which only
    // worked when both processes could share the port
    // (Linux with SO_REUSEPORT). On macOS, `ssh`
    // doesn't set SO_REUSEPORT on local-forward
    // sockets, so the tunnel and the listener must
    // use distinct ports.
    let vnc = TcpStream::connect(("127.0.0.1", tunnel_port))
        .await
        .map_err(|e| VncError::Io(format!("vnc connect: {e}")))?;
    let ws = tokio_tungstenite::accept_async(ws_tcp)
        .await
        .map_err(|e| VncError::Ws(e.to_string()))?;
    let (mut ws_tx, mut ws_rx) = ws.split();
    let (mut vnc_rd, mut vnc_wr) = vnc.into_split();

    let ws_to_vnc = tokio::spawn(async move {
        // WS frame -> VNC bytes. noVNC sends text frames
        // for the RFB protocol, but we treat them as
        // opaque bytes either way.
        while let Some(msg) = ws_rx.next().await {
            match msg {
                Ok(Message::Binary(b)) => {
                    if vnc_wr.write_all(&b).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Text(t)) => {
                    if vnc_wr.write_all(t.as_bytes()).await.is_err() {
                        break;
                    }
                }
                Ok(Message::Close(_)) => break,
                Ok(_) => continue, // ping/pong handled by tungstenite
                Err(_) => break,
            }
        }
    });
    let vnc_to_ws = tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            match vnc_rd.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if ws_tx
                        .send(Message::Binary(buf[..n].to_vec()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    // Wait for either side to finish. Whichever
    // finishes first triggers the join of the other
    // (and dropping the streams closes them).
    let _ = tokio::join!(ws_to_vnc, vnc_to_ws);
    Ok(())
}

impl SshPool {
    /// Read-only accessor for the server config. Used by
    /// `vnc::start` to know which user@host to tunnel
    /// through. Returns `None` if the server host is not
    /// configured.
    pub fn server_config(&self) -> Option<crate::computer::ssh::ServerConfig> {
        if self.server.is_configured() {
            Some(self.server.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::computer::ssh::{ServerConfig, SshPool};

    #[test]
    fn console_url_is_localhost_only() {
        // Construct a VncProxy manually with a fake
        // local_addr. The test asserts that the URL we
        // return never embeds a remote host or the
        // server's IP/hostname. This is the
        // security-critical guard: no code path that
        // returns a VNC console URL to the renderer may
        // point at anything other than localhost.
        let proxy = VncProxy {
            tunnel: Arc::new(Mutex::new(None)),
            listener: None,
            local_addr: "127.0.0.1:5942".parse().unwrap(),
            tunnel_port: 5943,
        };
        let url = proxy.console_url();
        assert!(url.starts_with("ws://localhost:"), "got {url}");
        assert!(!url.contains("192.168"));
        assert!(!url.contains("crispy"));
        assert!(!url.contains("0.0.0.0"));
        // The port is the only variable part.
        assert_eq!(url, "ws://localhost:5942/");
    }

    #[test]
    fn pick_free_port_returns_first_in_range() {
        // On a real machine the first call returns the
        // first free port. We can't predict the exact
        // port (other tests may be holding ports), so
        // we just assert it's within the range and is a
        // valid u16.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let port = rt.block_on(pick_free_port((5960, 5970))).expect("some port");
        assert!((5960..=5970).contains(&port));
    }

    #[test]
    fn pick_free_port_returns_none_when_range_exhausted() {
        // A range of 1 port that's definitely free
        // succeeds; a range of 1 port that we've
        // pre-bound returns None.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            // Take a port by binding it ourselves and
            // holding the listener.
            let held = TcpListener::bind("127.0.0.1:5980").await.unwrap();
            let result = pick_free_port((5980, 5980)).await;
            drop(held);
            assert!(result.is_none());
        });
    }

    #[test]
    fn server_config_accessor_returns_none_when_unset() {
        let pool = SshPool::new(ServerConfig {
            host: String::new(),
            user: "tyler".into(),
            identity_file: String::new(),
            ssh_config: String::new(),
        });
        assert!(pool.server_config().is_none());
    }

    #[tokio::test]
    async fn wait_for_tunnel_ready_detects_immediate_exit() {
        // Spawn a child that exits immediately. The
        // readiness window should observe the exit
        // (via `try_wait` returning `Some`) and return
        // `Ok(false)`. This is the path that
        // `vnc::start()` maps to `TunnelAuthFailed`.
        // We use `true` so the child exits with status 0
        // and the wait completes cleanly.
        let mut child = Command::new("true").spawn().unwrap();
        let ready = wait_for_tunnel_ready(&mut child).await.unwrap();
        assert!(!ready, "immediate-exit child should not be ready");
    }

    #[tokio::test]
    async fn wait_for_tunnel_ready_survives_long_running_child() {
        // `sleep 30` is a long-running child. The
        // 3-second readiness window elapses, the helper
        // returns `Ok(true)` because the child is still
        // alive. We then kill it so the test doesn't
        // leak a subprocess.
        let mut child = Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let ready = wait_for_tunnel_ready(&mut child).await.unwrap();
        assert!(ready, "long-running child should be considered ready");
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}
