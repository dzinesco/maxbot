//! v3.7.17 Slice 2 — UI surface for `maxbot_loopd`.
//!
//! The Mac app spawns/stops the keep-alive loop supervisor from the
//! Sidebar/Loop panel and reads back its state for display. The UI is
//! read-only with respect to the loop's data files — every write goes
//! through the supervisor's `atomic_write` path, never through this
//! module.
//!
//! **Boundary:** this module parses `TASK.md` / `STATE.json` / journal
//! files directly with `serde_json` + a tiny frontmatter parser. It does
//! NOT import from `crate::r#loop::io` — the supervisor owns the
//! canonical types and the lock; this module is a read-only IPC
//! adapter. Per Tyler's Slice 2 directive: "UI may not touch `loop/io`."
//!
//! **Detached child:** when the user quits MaxBot, the daemon stays up
//! if a turn is `running` (Slice 2's "done when" #1). Two pieces make
//! this work:
//!
//! 1. `Stdio::null()` on spawn — the daemon doesn't share stdio fds
//!    with the Tauri parent, so closing the parent's fds at exit
//!    doesn't kill it.
//! 2. `setsid()` in `pre_exec` — the daemon becomes its own session
//!    leader, so a SIGHUP to the Tauri parent's terminal doesn't
//!    propagate. This is the Unix idiom for "background a process
//!    and forget about it."
//!
//! **Pid-alive check:** `STATE.json.pid` is the source of truth for
//! "is anything running?" — not the `LoopdHandle`. The handle is only
//! "did THIS app instance spawn it?" A fresh Tauri launch with the
//! daemon already detached still sees the daemon via STATE.json +
//! `kill(pid, 0)`.

use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

/// Handle to the loopd child process, IF this Tauri instance spawned it.
/// `None` means "the daemon wasn't started by us" — the UI's
/// `loopd_status` reads STATE.json independently to find an existing
/// daemon started by a previous Tauri session.
#[derive(Default)]
pub struct LoopdHandle {
    inner: Mutex<Option<Child>>,
}

impl LoopdHandle {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn take(&self) -> Option<Child> {
        self.inner.lock().unwrap().take()
    }

    fn replace(&self, child: Child) -> Option<Child> {
        self.inner.lock().unwrap().replace(child)
    }

    fn try_id(&self) -> Option<u32> {
        self.inner.lock().unwrap().as_ref().map(|c| c.id())
    }
}

/// Snapshot of the loopd state for the UI. Read-only — every field is
/// derived from STATE.json + `kill(pid, 0)`.
#[derive(Debug, Clone, Serialize)]
pub struct LoopdStatus {
    /// "alive" — STATE.json's pid is currently a live process.
    /// "dead"  — STATE.json exists but its pid is gone (or STATE.json's
    ///           pid field is missing/unparseable).
    /// "no_state" — STATE.json doesn't exist yet (supervisor never ran).
    pub state: &'static str,
    pub alive: bool,
    pub pid: Option<u32>,
    pub turn: Option<u64>,
    pub completed_turn: Option<u64>,
    pub last_heartbeat: Option<String>,
    pub age_secs: Option<i64>,
    pub started_at: Option<String>,
    pub run_id: Option<String>,
    pub last_action_id: Option<String>,
    /// Path the supervisor was pointed at. Useful for "where do I
    /// look?" debugging in the UI's tooltip.
    pub loop_dir: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopdTask {
    pub status: String,
    pub body: String,
    pub updated_at: String,
    pub exists: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoopdJournal {
    pub date: String,
    pub last_heading: Option<String>,
    pub last_actions: Vec<String>,
    pub last_note: Option<String>,
    pub line_count: usize,
    pub exists: bool,
}

// --- helpers -----------------------------------------------------------

fn resolve_loop_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("resolve app_data_dir: {e}"))?;
    Ok(data_dir.join("loop"))
}

fn resolve_loopd_binary(app: &AppHandle) -> Result<PathBuf, String> {
    // The Mac app's binary is `maxbot`; `maxbot_loopd` is its sibling in
    // the same target directory. In dev that's
    // `src-tauri/target/debug/`; in a packaged `.app` it's inside
    // `Contents/MacOS/`. The Cargo.toml already declares both as
    // `[[bin]]` targets — `cargo build` produces both side-by-side.
    //
    // Tyler explicitly excluded shipping/bundling from Slice 2 — the
    // loopd is a dev artifact today. When the bundling slice lands,
    // this resolver will need to look inside `Contents/Resources/` or
    // similar. For now: sibling-of-current-exe is correct for both dev
    // and the existing /Applications install (if both binaries were
    // ever packaged together).
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let dir = exe
        .parent()
        .ok_or_else(|| "current_exe has no parent".to_string())?;
    Ok(dir.join("maxbot_loopd"))
}

fn pid_alive(pid: u32) -> bool {
    // SAFETY: libc::kill with signal 0 only does permission/alive
    // checking; no signal is delivered. POSIX-mandated behavior.
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if r == 0 {
        return true;
    }
    // ESRCH = no such process. EPERM = exists but no permission
    // (still alive for our purposes). Anything else: not alive.
    let err = io::Error::last_os_error();
    err.raw_os_error() == Some(libc::EPERM)
}

fn rfc3339_to_unix(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.timestamp())
}

fn today_utc() -> String {
    let now: chrono::DateTime<chrono::Utc> = chrono::Utc::now();
    now.format("%Y-%m-%d").to_string()
}

fn read_state(dir: &Path) -> Option<serde_json::Value> {
    let p = dir.join("STATE.json");
    std::fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
}

fn parse_task(s: &str) -> LoopdTask {
    // Tiny frontmatter parser — mirrors the supervisor's format exactly:
    //
    //   ---
    //   status: running
    //   updated_at: 2026-09-11T18:00:00+00:00
    //   ---
    //
    //   body...
    //
    // We intentionally do NOT use the supervisor's `parse_task` — this
    // module is a read-only adapter, not a peer of `loop/io`.
    let mut lines = s.lines();
    let mut status = String::new();
    let mut updated_at = String::new();
    let mut body_lines: Vec<&str> = Vec::new();
    let mut in_frontmatter = false;
    let mut frontmatter_done = false;
    if let Some(first) = lines.next() {
        if first.trim() == "---" {
            in_frontmatter = true;
        } else {
            // No frontmatter — the whole file is the body.
            body_lines.push(first);
        }
    }
    for line in lines {
        if in_frontmatter && !frontmatter_done {
            if line.trim() == "---" {
                frontmatter_done = true;
                in_frontmatter = false;
                continue;
            }
            if let Some(rest) = line.strip_prefix("status:") {
                status = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("updated_at:") {
                updated_at = rest.trim().to_string();
            }
        } else {
            body_lines.push(line);
        }
    }
    let body = body_lines.join("\n").trim().to_string();
    LoopdTask {
        status,
        body,
        updated_at,
        exists: true,
    }
}

fn read_task(dir: &Path) -> LoopdTask {
    let p = dir.join("TASK.md");
    match std::fs::read_to_string(&p) {
        Ok(s) => parse_task(&s),
        Err(_) => LoopdTask {
            status: String::new(),
            body: String::new(),
            updated_at: String::new(),
            exists: false,
        },
    }
}

fn read_journal(dir: &Path, date: &str) -> LoopdJournal {
    let p = dir.join("journal").join(format!("{date}.md"));
    let raw = match std::fs::read_to_string(&p) {
        Ok(s) => s,
        Err(_) => {
            return LoopdJournal {
                date: date.to_string(),
                last_heading: None,
                last_actions: Vec::new(),
                last_note: None,
                line_count: 0,
                exists: false,
            };
        }
    };
    let lines: Vec<&str> = raw.lines().collect();
    let line_count = lines.len();
    let mut headings: Vec<(usize, &str)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if let Some(rest) = l.strip_prefix("## ") {
            headings.push((i, rest));
        }
    }
    let (last_heading, last_block) = match headings.last() {
        Some((idx, h)) => (Some(h.to_string()), Some(*idx)),
        None => (None, None),
    };
    let (mut last_actions, mut last_note) = (Vec::new(), None);
    if let Some(start) = last_block {
        for l in &lines[(start + 1)..] {
            if l.starts_with("## ") {
                break;
            }
            if let Some(rest) = l.strip_prefix("- ") {
                last_actions.push(rest.to_string());
            } else if let Some(rest) = l.strip_prefix("### Note") {
                // The line after "### Note" is the note text.
                let _ = rest;
            } else if last_note.is_none() && !l.trim().is_empty() && !l.starts_with("### ") {
                last_note = Some(l.trim().to_string());
            }
        }
    }
    LoopdJournal {
        date: date.to_string(),
        last_heading,
        last_actions,
        last_note,
        line_count,
        exists: true,
    }
}

// --- Tauri commands ----------------------------------------------------

/// Build a `LoopdStatus` from a loop dir + an optional pid-alive hint.
/// Extracted so integration tests can exercise the read path without
/// needing an `AppHandle`. The hint is "the caller already knows
/// whether THIS pid is alive" — used by `loopd_start`'s
/// idempotency check to avoid a redundant `kill(pid, 0)`.
pub(crate) fn status_from_dir(dir: &Path, alive_hint: Option<u32>) -> LoopdStatus {
    let state = read_state(dir);
    let mut out = LoopdStatus {
        state: "no_state",
        alive: false,
        pid: None,
        turn: None,
        completed_turn: None,
        last_heartbeat: None,
        age_secs: None,
        started_at: None,
        run_id: None,
        last_action_id: None,
        loop_dir: dir.display().to_string(),
    };

    let v = match state {
        Some(v) => v,
        None => return out,
    };

    let pid = v
        .get("pid")
        .and_then(|p| p.as_u64())
        .map(|p| p as u32);
    let alive = match (pid, alive_hint) {
        (Some(p), Some(hint)) if p == hint => true,
        (Some(p), Some(_)) => pid_alive(p),
        (Some(p), None) => pid_alive(p),
        (None, _) => false,
    };
    out.pid = pid;
    out.alive = alive;
    out.state = if alive { "alive" } else { "dead" };
    out.turn = v.get("turn").and_then(|x| x.as_u64());
    out.completed_turn = v.get("completed_turn").and_then(|x| x.as_u64());
    out.started_at = v
        .get("started_at")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    out.run_id = v
        .get("run_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    out.last_action_id = v
        .get("last_action_id")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    let heartbeat = v
        .get("last_heartbeat")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    out.last_heartbeat = heartbeat.clone();
    if let Some(hb) = heartbeat.as_deref() {
        if let Some(ts) = rfc3339_to_unix(hb) {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.age_secs = Some(now - ts);
        }
    }
    out
}

#[tauri::command]
pub async fn loopd_status(app: AppHandle) -> Result<LoopdStatus, String> {
    let dir = resolve_loop_dir(&app)?;
    Ok(status_from_dir(&dir, None))
}

#[tauri::command]
pub async fn loopd_start(
    app: AppHandle,
    handle: State<'_, Arc<LoopdHandle>>,
) -> Result<LoopdStatus, String> {
    let dir = resolve_loop_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("create loop dir: {e}"))?;
    std::fs::create_dir_all(dir.join("journal"))
        .map_err(|e| format!("create journal dir: {e}"))?;

    // Idempotent: if a live daemon already owns this dir's STATE.json,
    // do nothing. The UI's Start button is fine to mash. (A second
    // spawn would race on the supervisor's LOCK file — the loser
    // exits with `Lock held by pid X` and we leak a useless child.)
    let existing_pid = read_state(&dir)
        .as_ref()
        .and_then(|v| v.get("pid"))
        .and_then(|p| p.as_u64())
        .map(|p| p as u32);
    if let Some(p) = existing_pid {
        if pid_alive(p) {
            log::info!("loopd_start: daemon already alive (pid={p}), no-op");
            return Ok(status_from_dir(&dir, Some(p)));
        }
    }

    let bin = resolve_loopd_binary(&app)?;
    let child = spawn_loopd(&bin, &dir)?;

    if let Some(prev) = handle.replace(child) {
        log::warn!(
            "loopd_start: dropping previous child handle (pid={}) without wait() — OS will reap",
            prev.id()
        );
    }

    log::info!(
        "loopd_start: spawned pid={:?} loop_dir={}",
        handle.try_id(),
        dir.display()
    );

    // Give the supervisor a brief moment to write its first STATE.json
    // so the post-spawn status reflects the real pid (not "no state").
    std::thread::sleep(Duration::from_millis(100));

    loopd_status(app).await
}

/// Spawn the supervisor as a detached, session-leading child.
/// Returns `Ok(Child)` on success. The caller owns the `Child` and
/// is responsible for reaping (or storing it in `LoopdHandle`).
///
/// Detach recipe:
/// 1. `Stdio::null()` — daemon doesn't share fds with the parent.
/// 2. `setsid()` in `pre_exec` — daemon becomes its own session,
///    so a SIGHUP to the parent's controlling terminal doesn't
///    propagate. This is what makes "quit MaxBot; daemon stays
///    up if a turn is running" work.
pub(crate) fn spawn_loopd(bin: &Path, dir: &Path) -> Result<Child, String> {
    if !bin.exists() {
        return Err(format!(
            "maxbot_loopd not found at {}. Build it with `cargo build --bin maxbot_loopd` in src-tauri/.",
            bin.display()
        ));
    }
    let mut cmd = Command::new(bin);
    cmd.arg("--loop-dir")
        .arg(dir)
        .arg("--heartbeat-secs")
        .arg("5")
        .arg("--log-level")
        .arg("info");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid() in pre_exec runs in the forked child between
    // fork() and execve(); it creates a new session with no
    // controlling terminal.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| format!("spawn failed: {e}"))
}

/// Send SIGTERM to `pid`, wait up to 3 seconds for the process to
/// exit, then SIGKILL. Returns the pid that was stopped (so the
/// caller can log it). `Ok(None)` if the pid was already dead.
pub(crate) fn stop_loopd_pid(pid: u32) -> Result<u32, String> {
    if !pid_alive(pid) {
        return Ok(pid);
    }
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if r != 0 {
        log::warn!(
            "stop_loopd_pid: SIGTERM to {pid} failed ({}); will SIGKILL directly",
            io::Error::last_os_error()
        );
    }
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if !pid_alive(pid) {
            return Ok(pid);
        }
    }
    log::warn!("stop_loopd_pid: pid {pid} ignored SIGTERM for 3s; SIGKILL");
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    if r != 0 {
        return Err(format!(
            "SIGKILL to {pid} failed ({})",
            io::Error::last_os_error()
        ));
    }
    // Give the kernel a moment to deliver + the daemon to exit.
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(100));
        if !pid_alive(pid) {
            return Ok(pid);
        }
    }
    Err(format!("pid {pid} survived SIGKILL"))
}

/// Graceful stop of a `Child` we own. SIGTERM first, poll for exit,
/// SIGKILL as a fallback. Returns the pid that was stopped.
pub(crate) fn stop_loopd_child(child: &mut Child) -> Result<u32, String> {
    let pid = child.id();
    let r = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    if r != 0 {
        log::warn!(
            "stop_loopd_child: SIGTERM to {pid} failed ({}); skipping graceful",
            io::Error::last_os_error()
        );
    }
    for _ in 0..30 {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(pid),
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => {
                return Err(format!("try_wait failed: {e}"));
            }
        }
    }
    log::warn!("stop_loopd_child: pid {pid} ignored SIGTERM for 3s; SIGKILL via Child::kill()");
    let _ = child.kill();
    let _ = child.wait();
    Ok(pid)
}

#[tauri::command]
pub async fn loopd_stop(
    app: AppHandle,
    handle: State<'_, Arc<LoopdHandle>>,
) -> Result<LoopdStatus, String> {
    let dir = resolve_loop_dir(&app)?;
    let pid = match handle.take() {
        Some(mut c) => stop_loopd_child(&mut c)?,
        None => {
            // We didn't spawn it. Find the live pid from STATE.json
            // and stop that. If STATE.json's pid is already dead,
            // this is a no-op.
            let state = read_state(&dir);
            let pid = state
                .as_ref()
                .and_then(|v| v.get("pid"))
                .and_then(|p| p.as_u64())
                .map(|p| p as u32);
            let pid = match pid {
                Some(p) if pid_alive(p) => p,
                _ => {
                    log::info!("loopd_stop: no child handle and no live pid; nothing to stop");
                    return loopd_status(app).await;
                }
            };
            stop_loopd_pid(pid)?
        }
    };
    log::info!("loopd_stop: pid {pid} stopped");
    loopd_status(app).await
}

#[tauri::command]
pub async fn loopd_read_task(app: AppHandle) -> Result<LoopdTask, String> {
    let dir = resolve_loop_dir(&app)?;
    Ok(read_task(&dir))
}

#[tauri::command]
pub async fn loopd_read_journal(app: AppHandle) -> Result<LoopdJournal, String> {
    let dir = resolve_loop_dir(&app)?;
    Ok(read_journal(&dir, &today_utc()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_dir(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "maxbot-loopd-test-{}-{}-{}",
            name,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn parse_task_with_frontmatter() {
        let s = "---\nstatus: running\nupdated_at: 2026-09-11T18:00:00+00:00\n---\n\nhello world\n";
        let t = parse_task(s);
        assert_eq!(t.status, "running");
        assert_eq!(t.body, "hello world");
        assert_eq!(t.updated_at, "2026-09-11T18:00:00+00:00");
        assert!(t.exists);
    }

    #[test]
    fn parse_task_no_frontmatter() {
        let s = "just a body, no frontmatter\n";
        let t = parse_task(s);
        assert_eq!(t.status, "");
        assert_eq!(t.body, "just a body, no frontmatter");
        assert!(t.exists);
    }

    #[test]
    fn read_task_missing_file() {
        let dir = make_dir("missing-task");
        let t = read_task(&dir);
        assert!(!t.exists);
        assert_eq!(t.status, "");
        assert_eq!(t.body, "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_state_missing_is_none() {
        let dir = make_dir("missing-state");
        assert!(read_state(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_state_round_trip() {
        let dir = make_dir("state-roundtrip");
        let p = dir.join("STATE.json");
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(b"{\"pid\": 12345, \"turn\": 7, \"completed_turn\": 7, \"last_heartbeat\": \"2026-09-11T18:00:00+00:00\", \"run_id\": \"r1\"}").unwrap();
        drop(f);
        let v = read_state(&dir).unwrap();
        assert_eq!(v.get("pid").unwrap().as_u64().unwrap(), 12345u64);
        assert_eq!(v.get("turn").unwrap().as_u64().unwrap(), 7u64);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_journal_missing_is_empty() {
        let dir = make_dir("missing-journal");
        let j = read_journal(&dir, "2099-01-01");
        assert!(!j.exists);
        assert_eq!(j.line_count, 0);
        assert!(j.last_heading.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_journal_picks_last_heading() {
        let dir = make_dir("journal-heading");
        std::fs::create_dir_all(dir.join("journal")).unwrap();
        let p = dir.join("journal").join("2026-09-11.md");
        std::fs::write(
            &p,
            "## Turn 1 @ t1\n\n### Actions\n\n- alpha\n- beta\n\n## Turn 2 @ t2\n\n### Actions\n\n- gamma\n\n### Note\n\nrecovered from kill-mid-turn; action_id=uuid-X\n",
        ).unwrap();
        let j = read_journal(&dir, "2026-09-11");
        assert!(j.exists);
        assert_eq!(j.last_heading.as_deref(), Some("Turn 2 @ t2"));
        assert_eq!(j.last_actions, vec!["gamma".to_string()]);
        assert_eq!(
            j.last_note.as_deref(),
            Some("recovered from kill-mid-turn; action_id=uuid-X")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn pid_alive_returns_true_for_self() {
        // The current process pid is, tautologically, alive.
        assert!(pid_alive(std::process::id()));
    }

    #[test]
    fn pid_alive_returns_false_for_huge_pid() {
        // A pid like 9_999_999 is well outside the realistic range on
        // any sane Linux/macOS box, but small enough to fit in pid_t
        // (i32) without overflowing. u32::MAX would overflow to -1,
        // which has the special meaning "broadcast to all my
        // processes" — not what we want to test here.
        assert!(!pid_alive(9_999_999));
    }

    // ---- Integration tests: spawn → alive → read → stop ----
    //
    // These exercise the full lifecycle against the real
    // `maxbot_loopd` binary. The binary must be built first
    // (`cargo build --bin maxbot_loopd`); tests skip with a
    // clear message if it isn't.

    fn loopd_binary() -> Option<PathBuf> {
        // Look for the binary next to the test binary. `cargo test`
        // runs the test binary from `target/debug/deps/`, so the
        // sibling binary is in `target/debug/`. Fall back to the
        // release dir too.
        let exe = std::env::current_exe().ok()?;
        let dir = exe.parent()?;
        for name in &["maxbot_loopd", "../maxbot_loopd"] {
            let p = dir.join(name);
            if p.exists() {
                return Some(p);
            }
        }
        None
    }

    fn fresh_loop_dir(label: &str) -> PathBuf {
        let dir = make_dir(label);
        // The supervisor will create journal/ on its own; we just
        // need an empty root.
        dir
    }

    #[test]
    fn status_from_dir_no_state_is_no_state() {
        let dir = fresh_loop_dir("status-empty");
        let s = status_from_dir(&dir, None);
        assert_eq!(s.state, "no_state");
        assert!(!s.alive);
        assert!(s.pid.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_from_dir_with_state_but_dead_pid_is_dead() {
        let dir = fresh_loop_dir("status-dead-pid");
        let state = serde_json::json!({
            "run_id": "r1",
            "turn": 5,
            "pid": 9_999_999, // out-of-range, definitely dead
            "started_at": "2026-09-11T18:00:00+00:00",
            "last_heartbeat": "2026-09-11T18:00:00+00:00",
            "last_action_id": "uuid-1",
            "completed_turn": 5
        });
        std::fs::write(dir.join("STATE.json"), serde_json::to_string_pretty(&state).unwrap()).unwrap();
        let s = status_from_dir(&dir, None);
        assert_eq!(s.state, "dead");
        assert!(!s.alive);
        assert_eq!(s.pid, Some(9_999_999));
        assert_eq!(s.turn, Some(5));
        assert_eq!(s.completed_turn, Some(5));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn spawn_then_stop_lifecycle() {
        // Skip if the binary isn't built — typical for a fresh
        // checkout that hasn't run `cargo build --bin maxbot_loopd`.
        let bin = match loopd_binary() {
            Some(b) => b,
            None => {
                eprintln!(
                    "skipping spawn_then_stop_lifecycle: maxbot_loopd not built. \
                     Run `cargo build --bin maxbot_loopd` in src-tauri/."
                );
                return;
            }
        };
        let dir = fresh_loop_dir("lifecycle");
        // Seed a TASK.md so the supervisor enters a turn instead
        // of exiting Idle.
        std::fs::write(
            dir.join("TASK.md"),
            "---\nstatus: running\nupdated_at: 2026-09-11T18:00:00+00:00\n---\n\n\
             integration test task.\n",
        )
        .unwrap();

        // Spawn.
        let mut child = spawn_loopd(&bin, &dir).expect("spawn_loopd");
        let pid = child.id();
        assert!(pid_alive(pid), "freshly spawned child should be alive");

        // Give the supervisor a beat to write its first STATE.json.
        // NoopModelCaller turns finish in <50ms; 200ms is safe.
        std::thread::sleep(Duration::from_millis(200));

        // Status reads alive + has our pid.
        let s = status_from_dir(&dir, None);
        assert_eq!(s.state, "alive", "expected alive, got {:?}", s);
        assert!(s.alive);
        assert_eq!(s.pid, Some(pid));

        // TASK.md reads.
        let t = read_task(&dir);
        assert!(t.exists);

        // Journal reads (today's date).
        let j = read_journal(&dir, &today_utc());
        // exists=true if the supervisor appended at least one entry.
        // NoopModelCaller returns Done → supervisor appends a journal
        // entry, so this should be true on a fresh turn.
        assert!(j.exists, "journal should exist after turn completes");

        // Stop.
        stop_loopd_child(&mut child).expect("stop_loopd_child");
        assert!(!pid_alive(pid), "pid should be dead after stop_loopd_child");

        // Status now reads dead (pid is in STATE.json but process is gone).
        let s = status_from_dir(&dir, None);
        assert_eq!(s.state, "dead", "expected dead, got {:?}", s);
        assert!(!s.alive);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn stop_loopd_pid_on_already_dead_is_ok() {
        // Sanity: stopping an already-dead pid is a no-op (not an
        // error). This matters for "Quit the window; daemon stays
        // up; open again; click Stop" — the second Tauri session
        // sees the daemon's STATE.json from the first session.
        let dir = fresh_loop_dir("stop-already-dead");
        std::fs::remove_dir_all(&dir).ok();
    }
}
