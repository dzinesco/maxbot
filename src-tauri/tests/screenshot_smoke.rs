//! v3.7.4: Real-crispy smoke for the v3.7.2 host-side
//! screenshot path (`computer::screenshot::capture_jpeg`).
//!
//! This file is **not part of the default test run.** It is
//! `#[ignore]`d so the regular `cargo test --lib` /
//! `cargo test --bin maxbotd` invocations do not depend on
//! real hardware. Run it explicitly with:
//!
//! ```text
//! cargo test --lib screenshot_smoke -- --ignored --nocapture
//! ```
//!
//! The test:
//!   1. Reads `Settings.computer_server_host` from
//!      `~/Library/Application Support/com.maxbot.app/maxbot.sqlite`
//!      (the same DB the Mac app uses). If that host is
//!      empty, the test skips — there's no server to
//!      reach.
//!   2. Builds an `SshPool` from the Settings and tries
//!      to list Bots with a `computers` row in `running`.
//!      If none are running, the test skips (this is
//!      the common case on a fresh install).
//!   3. Picks the first running Bot, calls
//!      `capture_jpeg(...)`, and asserts:
//!        - the bytes are non-empty
//!        - the first two bytes are `0xFF 0xD8` (the JPEG
//!          magic / SOI marker). Anything else means
//!          `virsh screenshot` failed and the Rust side
//!          handed back a PPM, PNG, or empty buffer
//!          without decoding — a regression we'd want
//!          to catch on real hardware.
//!
//! If the SSH pool can't be reached (e.g., the user
//! has not configured `computer_server_host`, the DB is
//! missing, or the host is offline) the test **skips
//! gracefully** rather than failing. That's deliberate:
//! the goal is "a maintainer can verify the display path
//! against real hardware in one command", not "every CI
//! run must reach a crispy the CI doesn't have".

use maxbot_lib::computer::screenshot::{capture_jpeg, ScreenshotError};
use maxbot_lib::computer::ssh::{ServerConfig, SshPool};
use maxbot_lib::storage::{Database, Settings};

/// Default path to the Mac app's SQLite. Matches
/// `~/Library/Application Support/com.maxbot.app/maxbot.sqlite`
/// on macOS. The test falls back to `MAXBOT_DB` if set,
/// so a CI runner can override.
fn default_db_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("MAXBOT_DB") {
        return std::path::PathBuf::from(p);
    }
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    home.join("Library/Application Support/com.maxbot.app/maxbot.sqlite")
}

#[tokio::test]
#[ignore = "real-crispy smoke test; run with `cargo test --lib screenshot_smoke -- --ignored --nocapture`"]
async fn screenshot_smoke_returns_jpeg_magic() {
    // 1. Load settings + db.
    let db_path = default_db_path();
    if !db_path.exists() {
        eprintln!(
            "screenshot_smoke: SKIP — DB not found at {}. \
             Run the Mac app once to create it, or set MAXBOT_DB.",
            db_path.display()
        );
        return;
    }
    let db = match Database::open(&db_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("screenshot_smoke: SKIP — cannot open DB: {e}");
            return;
        }
    };
    let settings: Settings = match db.load_settings() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("screenshot_smoke: SKIP — cannot load settings: {e}");
            return;
        }
    };
    if settings.computer_server_host.is_empty() {
        eprintln!(
            "screenshot_smoke: SKIP — Settings.computer_server_host is empty. \
             Set it in the Mac app's Settings → Computer first."
        );
        return;
    }

    // 2. Find a running Bot. We prefer the env-var pin
    //    `MAXBOT_SMOKE_BOT` so a maintainer can target a
    //    specific Bot; otherwise the first running Bot
    //    wins.
    let computers = match db.list_computers() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("screenshot_smoke: SKIP — cannot list computers: {e}");
            return;
        }
    };
    let pinned = std::env::var("MAXBOT_SMOKE_BOT").ok();
    let bot_id = match pinned {
        Some(id) => id,
        None => match computers.into_iter().find(|c| c.state == "running") {
            Some(c) => c.bot_id,
            None => {
                eprintln!(
                    "screenshot_smoke: SKIP — no Bots in `running` state. \
                     Provision a Bot first (Settings → Computer → Provision a computer)."
                );
                return;
            }
        },
    };

    // 3. Look up the Bot's VM name.
    let computer = match db.get_computer(&bot_id) {
        Ok(Some(c)) => c,
        Ok(None) => {
            eprintln!("screenshot_smoke: SKIP — Bot {bot_id} has no computer row");
            return;
        }
        Err(e) => {
            eprintln!("screenshot_smoke: SKIP — get_computer error: {e}");
            return;
        }
    };
    let vm_name = computer.vm_name.clone();

    // 4. Build the SSH pool. No passphrase — the smoke
    //    path goes through the user's default SSH key /
    //    ssh-agent, the same as the in-app path.
    let server = ServerConfig::from_settings(&settings);
    let pool = SshPool::new(server);

    // 5. Capture. Failure here is a hard test failure
    //    (the brief: "screenshot bytes start with
    //    `FF D8`" — if the SSH / libvirt path is broken
    //    on real hardware, this is the test that catches
    //    it). SSH-unreachable is the one exception —
    //    a flaky network is not a code regression.
    let bytes = match capture_jpeg(&pool, &vm_name, "running").await {
        Ok(b) => b,
        Err(ScreenshotError::Ssh(msg)) => {
            eprintln!("screenshot_smoke: SKIP — SSH unreachable: {msg}");
            return;
        }
        Err(ScreenshotError::DomainNotFound { vm_name: v }) => {
            eprintln!("screenshot_smoke: SKIP — VM {v} not found on host");
            return;
        }
        Err(e) => panic!("screenshot_smoke: capture_jpeg failed: {e}"),
    };

    // 6. Assert JPEG magic + non-empty.
    assert!(
        !bytes.is_empty(),
        "screenshot_smoke: capture_jpeg returned empty bytes"
    );
    assert!(
        bytes.len() >= 2,
        "screenshot_smoke: bytes too short to contain a magic number"
    );
    assert_eq!(
        bytes[0], 0xFF,
        "screenshot_smoke: first byte is not 0xFF (got 0x{:02X}); \
         the path returned something other than JPEG",
        bytes[0]
    );
    assert_eq!(
        bytes[1], 0xD8,
        "screenshot_smoke: second byte is not 0xD8 (got 0x{:02X}); \
         the path returned something other than JPEG",
        bytes[1]
    );

    eprintln!(
        "screenshot_smoke: OK — {} bytes, JPEG magic 0xFF 0xD8, vm={vm_name}, bot={bot_id}",
        bytes.len()
    );
}
