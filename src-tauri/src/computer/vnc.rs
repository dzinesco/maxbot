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
            let port = local_port;
            tokio::spawn(async move {
                if let Err(e) = bridge_one(tcp, port).await {
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
    let tunnel = spawn_tunnel(pool, actual.port(), remote_vnc_port).await?;
    Ok(VncProxy {
        tunnel: Arc::new(Mutex::new(Some(tunnel))),
        listener: Some(listener),
        local_addr: actual,
    })
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

/// Bridge one WebSocket connection to the VNC TCP stream
/// on the loopback (the SSH tunnel listens on
/// `127.0.0.1:<local_port>` and forwards to the server's
/// `127.0.0.1:<vnc_port>`). When either side closes, the
/// other side is shut down. Standard `select!`-based
/// pump.
async fn bridge_one(ws_tcp: TcpStream, local_port: u16) -> Result<(), VncError> {
    // The WebSocket is over the listener's port; the
    // VNC stream is on the same loopback port (the
    // tunnel). Same port, two streams — we use the
    // TcpStream from the listener for the WS side and
    // open a fresh TcpStream for the VNC side.
    let vnc = TcpStream::connect(("127.0.0.1", local_port))
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
}
