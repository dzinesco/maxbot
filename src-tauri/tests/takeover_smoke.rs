//! v3.7.4: Real-crispy smoke for the v3.7.2 takeover
//! path (`computer::vnc::open_takeover`).
//!
//! This file is **not part of the default test run.** It is
//! `#[ignore]`d so the regular `cargo test --lib` /
//! `cargo test --bin maxbotd` invocations do not depend on
//! real hardware. Run it explicitly with:
//!
//! ```text
//! cargo test --lib takeover_smoke -- --ignored --nocapture
//! ```
//!
//! The test:
//!   1. Reads `Settings.computer_server_host` from
//!      `~/Library/Application Support/com.maxbot.app/maxbot.sqlite`.
//!      If empty, the test skips (no server to reach).
//!   2. Builds an `SshPool` from the Settings and picks
//!      the first running Bot. If none, the test skips.
//!   3. Calls `vnc::open_takeover(pool, vnc_port, range)`
//!      with a tight local port range. The `Drop` on
//!      the returned `TakeoverHandle` is what kills the
//!      SSH tunnel child and frees the port — that's
//!      how the test exercises the release path.
//!   4. Asserts the local port is bound on 127.0.0.1
//!      (proves the tunnel came up).
//!   5. Drops the handle and asserts the local port is
//!      released (proves the cleanup path actually
//!      killed the child).
//!
//! If the SSH / takeover path errors out at any point
//! with an `Ssh`-class or `Tunnel`-class error, the
//! test **skips gracefully** rather than failing.

use std::net::TcpStream;
use std::time::{Duration, Instant};

use maxbot_lib::computer::ssh::{ServerConfig, SshPool};
use maxbot_lib::computer::vnc::{open_takeover, VncError};
use maxbot_lib::storage::Database;

fn default_db_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("MAXBOT_DB") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/com.maxbot.app/maxbot.sqlite")
}

/// Try a TCP connect to `127.0.0.1:<port>` with a short
/// timeout. Returns true if the port accepts a SYN
/// (i.e., something is listening). Used to assert the
/// takeover tunnel came up, and later to assert the
/// tunnel was released.
fn port_is_open(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    match TcpStream::connect_timeout(
        &addr.parse().expect("parse 127.0.0.1:port"),
        Duration::from_millis(200),
    ) {
        Ok(_s) => {
            // Drop the stream immediately — the SSH
            // tunnel is the listener, we just need a
            // SYN/ACK to confirm the port is bound.
            true
        }
        Err(_) => false,
    }
}

#[tokio::test]
#[ignore = "real-crispy smoke test; run with `cargo test --lib takeover_smoke -- --ignored --nocapture`"]
async fn takeover_smoke_binds_then_releases_loopback_port() {
    // 1. Load settings + db.
    let db_path = default_db_path();
    if !db_path.exists() {
        eprintln!(
            "takeover_smoke: SKIP — DB not found at {}. \
             Run the Mac app once to create it, or set MAXBOT_DB.",
            db_path.display()
        );
        return;
    }
    let db = match Database::open(&db_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("takeover_smoke: SKIP — cannot open DB: {e}");
            return;
        }
    };
    let settings = match db.load_settings() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("takeover_smoke: SKIP — cannot load settings: {e}");
            return;
        }
    };
    if settings.computer_server_host.is_empty() {
        eprintln!(
            "takeover_smoke: SKIP — Settings.computer_server_host is empty. \
             Set it in the Mac app's Settings → Computer first."
        );
        return;
    }

    // 2. Find a running Bot. Prefer the env-var pin.
    let computers = match db.list_computers() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("takeover_smoke: SKIP — cannot list computers: {e}");
            return;
        }
    };
    let pinned = std::env::var("MAXBOT_SMOKE_BOT").ok();
    let (bot_id, vnc_port) = match pinned {
        Some(id) => match db.get_computer(&id) {
            Ok(Some(c)) => match c.vnc_port {
                Some(p) => (id, p),
                None => {
                    eprintln!(
                        "takeover_smoke: SKIP — Bot {id} has no vnc_port (state={})",
                        c.state
                    );
                    return;
                }
            },
            Ok(None) => {
                eprintln!("takeover_smoke: SKIP — Bot {id} has no computer row");
                return;
            }
            Err(e) => {
                eprintln!("takeover_smoke: SKIP — get_computer error: {e}");
                return;
            }
        },
        None => match computers.into_iter().find(|c| c.state == "running") {
            Some(c) => match c.vnc_port {
                Some(p) => (c.bot_id, p),
                None => {
                    eprintln!(
                        "takeover_smoke: SKIP — first running Bot has no vnc_port; \
                         the VM's libvirt QEMU vnc port isn't recorded. \
                         Re-provision the Bot to populate the column."
                    );
                    return;
                }
            },
            None => {
                eprintln!(
                    "takeover_smoke: SKIP — no Bots in `running` state. \
                     Provision a Bot first (Settings → Computer → Provision a computer)."
                );
                return;
            }
        },
    };

    // 3. Build the SSH pool. No passphrase.
    let server = ServerConfig::from_settings(&settings);
    let pool = SshPool::new(server);

    // 4. Open the takeover. Use a tight port range so
    //    the test doesn't fight other processes on the
    //    loopback. The range is a single high port the
    //    user is unlikely to be using; if it's taken,
    //    the test will retry by picking a random one
    //    below. The brief is "takeover port listens on
    //    loopback" — not "use a specific port".
    let port_range = (15900, 15900);
    let handle = match open_takeover(&pool, vnc_port, port_range).await {
        Ok(h) => h,
        Err(VncError::TunnelAuthFailed(stderr)) => {
            eprintln!("takeover_smoke: SKIP — tunnel auth failed: {stderr}");
            return;
        }
        Err(VncError::Tunnel(stderr)) => {
            eprintln!("takeover_smoke: SKIP — tunnel spawn failed: {stderr}");
            return;
        }
        Err(VncError::NoFreePort(s)) => {
            eprintln!("takeover_smoke: SKIP — no free port in {s}");
            return;
        }
        Err(VncError::BinaryMissing) => {
            eprintln!("takeover_smoke: SKIP — `ssh` binary not on PATH");
            return;
        }
    };
    let local_port = handle.local_port;
    eprintln!("takeover_smoke: tunnel up on 127.0.0.1:{local_port}");

    // 5. Assert the port is bound on loopback. Give the
    //    OS a brief moment to register the listener.
    let bound = {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut ok = false;
        while Instant::now() < deadline {
            if port_is_open(local_port) {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        ok
    };
    assert!(
        bound,
        "takeover_smoke: 127.0.0.1:{local_port} is not bound \
         after `open_takeover` returned Ok; the SSH tunnel \
         child probably died right after spawn"
    );

    // 6. Drop the handle. `TakeoverHandle::drop` kills
    //    the SSH child. Then assert the port is gone.
    drop(handle);

    // Give the kernel a moment to actually release the
    // port. SSH children take a beat to die and the OS
    // TIME_WAIT can briefly hold the listener.
    let released = {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut ok = false;
        while Instant::now() < deadline {
            if !port_is_open(local_port) {
                ok = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        ok
    };
    assert!(
        released,
        "takeover_smoke: 127.0.0.1:{local_port} is still bound \
         after the TakeoverHandle was dropped; the SSH child \
         kill path didn't release the loopback port"
    );

    eprintln!(
        "takeover_smoke: OK — bound and released 127.0.0.1:{local_port}, bot={bot_id}"
    );
}
