//! v3.7.2: VNC tunnel for the "Take over with Screen Sharing"
//! button. The in-app preview is now a host-side QEMU
//! framebuffer poll (see `screenshot.rs`); this module is
//! the path the user takes when they want full keyboard
//! + mouse control via the macOS `Screen Sharing` app.
//!
//! Flow:
//! 1. Pick a free local TCP port from
//!    `Settings.computer_vnc_local_port_range`.
//! 2. Spawn `ssh -N -L <local>:127.0.0.1:<qemu_vnc> <user>@<host>`
//!    as a child process. The tunnel is the only thing
//!    that needs to outlive a single Tauri command call.
//! 3. The Tauri side returns the local port to the
//!    renderer, which runs `open vnc://127.0.0.1:<port>`.
//!    macOS opens `Screen Sharing` (or `Preview` on older
//!    releases) against the tunnel.
//! 4. "Stop takeover" kills the child; the port is freed.
//!
//! v3.7.2 deliberately drops the WS↔RFB bridge that powered
//! the v3.0.x noVNC console. The Tauri webview (WKWebView)
//! doesn't render noVNC's canvas path reliably — the brief
//! names this as the root cause of the v3.0.x "console
//! shows blank canvas" failure mode. The host-side poll
//! sidesteps the webview's canvas path entirely.

use std::process::Stdio;
use std::sync::Arc;

use thiserror::Error;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::ssh::SshPool;

#[derive(Debug, Error)]
pub enum VncError {
    #[error("ssh binary not found on PATH")]
    BinaryMissing,
    #[error("port allocator exhausted: no free ports in {0:?}")]
    NoFreePort(String),
    /// The SSH tunnel child exited before the readiness
    /// window elapsed — auth failed, key rejected, host
    /// key mismatch. `stderr` carries the captured ssh
    /// output.
    #[error("vnc tunnel auth failed: {0}")]
    TunnelAuthFailed(String),
    #[error("vnc tunnel spawn failed: {0}")]
    Tunnel(String),
}

/// v3.7.2: a "Take over with Screen Sharing" tunnel. The
/// SSH child is the only state — the renderer connects
/// to it via the loopback port and macOS handles the
/// rest. Dropping the handle `start_kill`s the child.
pub struct TakeoverHandle {
    /// The SSH tunnel child. Dropped (and `start_kill`-ed)
    /// when `TakeoverHandle` is dropped.
    pub child: Arc<Mutex<Option<Child>>>,
    /// The local port the tunnel is listening on. The
    /// renderer gets this back so it can hand
    /// `vnc://127.0.0.1:<port>` to `open`.
    pub local_port: u16,
}

impl Drop for TakeoverHandle {
    fn drop(&mut self) {
        // Best-effort kill of the SSH tunnel. We do this
        // synchronously in Drop; the child is already
        // running in another task. `start_kill` sends
        // SIGKILL on Unix.
        if let Ok(mut guard) = self.child.try_lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.start_kill();
            }
        }
    }
}

/// Build (but do not start) the SSH tunnel child that
/// maps `localhost:<local>` on the Mac to
/// `127.0.0.1:<remote>` on the server. The child is the
/// lifetime anchor for the tunnel; killing the child
/// frees the port on the server side.
async fn spawn_tunnel(
    pool: &SshPool,
    local_port: u16,
    remote_vnc_port: u16,
) -> Result<Child, VncError> {
    let server = pool
        .server_config()
        .ok_or_else(|| VncError::Tunnel("server host not configured".into()))?;
    let user_at_host = format!("{}@{}", server.user, server.host);
    let mut c = Command::new("ssh");
    c.arg("-N"); // no remote command — just forward.
    c.arg("-L")
        .arg(format!("{local_port}:127.0.0.1:{remote_vnc_port}"));
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

/// v3.7.2: open a takeover tunnel. Returns the handle
/// the caller stores in their `takeover_tunnels` map
/// (the `Drop` on the handle kills the child). `port`
/// is the local loopback port the renderer should hand
/// to `open vnc://127.0.0.1:<port>`.
pub async fn open_takeover(
    pool: &SshPool,
    remote_vnc_port: u16,
    port_range: (u16, u16),
) -> Result<TakeoverHandle, VncError> {
    let local_port = pick_free_port(port_range)
        .await
        .ok_or_else(|| VncError::NoFreePort(format!("{}..={}", port_range.0, port_range.1)))?;
    let mut child = spawn_tunnel(pool, local_port, remote_vnc_port).await?;
    // Wait for the SSH tunnel to be ready (auth window).
    // If the child exits during the window, capture its
    // stderr so the user sees the real reason instead of
    // a `Connection refused` from `Screen Sharing`.
    if !wait_for_tunnel_ready(&mut child).await? {
        let stderr = read_child_stderr(&mut child).await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(VncError::TunnelAuthFailed(stderr));
    }
    Ok(TakeoverHandle {
        child: Arc::new(Mutex::new(Some(child))),
        local_port,
    })
}

/// Poll the SSH tunnel child for `READY_WINDOW` and
/// return `Ok(true)` if it stays alive that long. Returns
/// `Ok(false)` if it exits within the window. Errors from
/// `try_wait` other than `NotFound` are returned as `Err`.
async fn wait_for_tunnel_ready(child: &mut Child) -> Result<bool, VncError> {
    use std::time::Duration;
    const READY_WINDOW: Duration = Duration::from_secs(3);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);
    let deadline = tokio::time::Instant::now() + READY_WINDOW;
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => return Ok(false), // child exited
            Ok(None) => {
                if tokio::time::Instant::now() >= deadline {
                    return Ok(true);
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Err(e) => return Err(VncError::Tunnel(e.to_string())),
        }
    }
}

/// Drain the child's stderr (non-blocking via a wall-clock
/// timeout). The caller is expected to have just observed
/// the child exit.
async fn read_child_stderr(child: &mut Child) -> String {
    use tokio::io::AsyncReadExt;
    let mut buf = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let read_result = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            async {
                let mut s = String::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match stderr.read(&mut chunk).await {
                        Ok(0) => break,
                        Ok(n) => s.push_str(&String::from_utf8_lossy(&chunk[..n])),
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
/// linearly, trying to bind a 127.0.0.1 listener. Used by
/// the takeover path (`open_takeover`) — and exposed so
/// the ComputerManager can use the same allocator for
/// other tunneled resources.
pub async fn pick_free_port(range: (u16, u16)) -> Option<u16> {
    for port in range.0..=range.1 {
        let addr: std::net::SocketAddr = match format!("127.0.0.1:{port}").parse() {
            Ok(a) => a,
            Err(_) => continue,
        };
        match tokio::net::TcpListener::bind(addr).await {
            Ok(l) => {
                drop(l);
                return Some(port);
            }
            Err(_) => continue,
        }
    }
    None
}

impl SshPool {
    /// Read-only accessor for the server config. Used by
    /// `vnc::open_takeover` to know which user@host to
    /// tunnel through. Returns `None` if the server host
    /// is not configured.
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

    /// v3.7.4: a process-wide lock that serializes the
    /// `pick_free_port_*` tests so they can't fight over
    /// 127.0.0.1 ports when cargo runs them in parallel.
    /// The companion test `pick_free_port_returns_port_in_range`
    /// holds port 5980; the exhausted test holds whatever
    /// port the OS gives it via the probe. If they
    /// overlap, the exhausted test's `pick_free_port`
    /// may briefly observe its probe port as free (the
    /// companion test released 5980 before the exhausted
    /// test's bind) and the test would race.
    /// `PORT_LOCK` makes the two tests mutually exclusive.
    /// (The pattern is the same `static Mutex<()>` that
    /// `computer/mod.rs` uses for `HOME_LOCK` in
    /// v3.0.3 — both serialize access to a shared OS
    /// resource across parallel tests.)
    static PORT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// v3.7.2: `pick_free_port` returns a port in the
    /// requested range. The exact port isn't predictable
    /// (other tests may be holding ports), so we assert
    /// the invariant rather than a specific value.
    ///
    /// v3.7.4: acquire `PORT_LOCK` so the parallel
    /// `pick_free_port_exhausted_returns_none` can't
    /// race with this one over 127.0.0.1:5980.
    #[tokio::test]
    async fn pick_free_port_returns_port_in_range() {
        let _lock = PORT_LOCK.lock().expect("port lock");
        // Use a range unlikely to be in use by other
        // tests — high ports, single-port range to keep
        // the test deterministic.
        let range = (5980, 5980);
        let port = pick_free_port(range).await;
        assert_eq!(port, Some(5980));
    }

    /// An empty / exhausted range yields `None`.
    ///
    /// v3.7.4: acquire `PORT_LOCK` (same rationale as
    /// the companion test) so the probe-vs-allocator
    /// race on a parallel run is impossible.
    #[tokio::test]
    async fn pick_free_port_exhausted_returns_none() {
        let _lock = PORT_LOCK.lock().expect("port lock");
        // Pick a port, hold it for the test duration, then
        // assert the allocator can't return it.
        let probe = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("probe");
        let port = probe.local_addr().expect("addr").port();
        // Probe is still alive — the allocator must skip
        // this port and find no fallback.
        let _guard = probe;
        // Use a one-port range matching the held port.
        // The probe will fail; no fallback exists.
        // The previous (pre-v3.7.4) version of this
        // comment said "we can't be sure OTHER ports
        // in the same range are free" — the one-port
        // range makes that concern moot, and the
        // `PORT_LOCK` above prevents any parallel
        // test from binding the probe port before
        // our allocator checks.
        let result = pick_free_port((port, port)).await;
        assert!(result.is_none(), "expected None for held port");
    }
}
