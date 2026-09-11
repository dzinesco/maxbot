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

/// v3.7.17 Slice 3 — minimal poll shape. The UI's 2-second tick
/// pulls ONLY these fields. Full TASK.md + journal + turn counters
/// are loaded on explicit Read (`loopd_read_task` / `loopd_read_journal`),
/// not on every tick. The full STATE.json is read at most once per
/// tick; the body bytes and journal markdown can be KB and don't
/// change every 2s — the cost of pulling them on every tick is
/// real CPU + serialization time the UI doesn't need.
///
/// Per Tyler's Slice 3 brief: "LoopPanel 2s poll returns ONLY
/// { pid, state, task_status, task_excerpt_len }. Full TASK.md /
/// journal only on explicit Read, not every tick."
#[derive(Debug, Clone, Serialize)]
pub struct LoopdStatus {
    /// "alive" — STATE.json's pid is currently a live process.
    /// "dead"  — STATE.json exists but its pid is gone.
    /// "no_state" — STATE.json doesn't exist yet.
    pub state: &'static str,
    pub alive: bool,
    pub pid: Option<u32>,
    /// Current TASK.md `status:` line, or "" if TASK.md doesn't exist
    /// yet. The renderer derives the pill color from this string.
    pub task_status: String,
    /// Length of the TASK.md body in bytes (UTF-8 char count). Cheap
    /// to compute and gives the renderer enough to decide "the task
    /// body changed since last full Read" without re-parsing markdown.
    pub task_excerpt_len: u64,
    pub last_heartbeat: Option<String>,
    pub age_secs: Option<i64>,
    /// Path the supervisor was pointed at. Useful for the UI's
    /// tooltip and for "where do I look?" debugging.
    pub loop_dir: String,
}

/// Full TASK.md frontmatter + body. Returned by `loopd_read_task` on
/// explicit Read (initial mount + manual Refresh).
#[derive(Debug, Clone, Serialize)]
pub struct LoopdTask {
    pub status: String,
    pub body: String,
    pub updated_at: String,
    pub exists: bool,
}

/// Last journal heading + actions + note. Returned by
/// `loopd_read_journal` on explicit Read.
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

/// v3.7.17 Slice 3 — bundled binary resolver. In a packaged
/// `MaxBot.app`, `maxbot_loopd` sits next to `maxbot` in
/// `Contents/MacOS/`. In dev (`cargo run`), both binaries sit in
/// `target/debug/` (or `release/`). This helper covers both with
/// one rule: "is `maxbot_loopd` next to the running `maxbot`?".
///
/// Tauri 2's `bundle.externalBin` declaration places the sidecar
/// next to the host binary (no target-triple suffix in the bundle),
/// so `parent().join("maxbot_loopd")` is correct for both layouts.
///
/// Per Tyler's Slice 3 brief: "loopd_start resolves that bundled
/// path first, then a documented dev fallback (target/debug).
/// Never PATH-only in production." We never search `$PATH` —
/// the binary is always located via filesystem convention.
pub(crate) fn resolve_bundled_loopd_path(exe_dir: &Path) -> Option<PathBuf> {
    let candidate = exe_dir.join("maxbot_loopd");
    if candidate.is_file() {
        Some(candidate)
    } else {
        None
    }
}

/// Resolve the `maxbot_loopd` binary path.
///
/// 1. **Bundled / side-by-side:** next to the running `maxbot`
///    binary (`Contents/MacOS/maxbot_loopd` in a packaged `.app`;
///    `target/debug/maxbot_loopd` next to a `cargo run`-launched
///    `maxbot`). Same lookup rule covers both.
/// 2. **Dev fallback:** `src-tauri/target/{debug,release}/maxbot_loopd`.
///    Used when the user runs the Tauri app from a build dir that
///    isn't the same dir `cargo build` put the binary in (e.g.
///    `cargo tauri dev` from a worktree, or running the binary
///    straight from `target/release` without `cargo run`).
///
/// Never consults `$PATH`. Never reads symlinks to a different
/// version. The user is responsible for putting the binary
/// somewhere this resolver looks.
fn resolve_loopd_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    if let Some(dir) = exe.parent() {
        if let Some(p) = resolve_bundled_loopd_path(dir) {
            return Ok(p);
        }
    }
    // Dev fallback: search both profiles so `cargo build --release`
    // works without re-running `--bin maxbot_loopd` from a debug dir.
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    for profile in &["debug", "release"] {
        let candidate = manifest_dir.join("target").join(profile).join("maxbot_loopd");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "maxbot_loopd not found. Looked next to current_exe ({}) and in src-tauri/target/{{debug,release}}/. \
         Build it with `cargo build --bin maxbot_loopd` in src-tauri/, or place the bundled sidecar at \
         MaxBot.app/Contents/MacOS/maxbot_loopd.",
        exe.display()
    ))
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
/// Build a `LoopdStatus` from a loop dir + an optional pid-alive hint.
/// Extracted so integration tests can exercise the read path without
/// needing an `AppHandle`. The hint is "the caller already knows
/// whether THIS pid is alive" — used by `loopd_start`'s
/// idempotency check to avoid a redundant `kill(pid, 0)`.
///
/// v3.7.17 Slice 3 — minimal poll shape. Reads STATE.json for the
/// daemon's pid + heartbeat, and reads the byte length of TASK.md's
/// body for "did the task change?" Without parsing the markdown
/// frontmatter on every 2s tick. `task_status` and `task_excerpt_len`
/// are derived from a tiny peek at TASK.md, not the full parse — the
/// renderer calls `loopd_read_task` on explicit Read to get the body.
pub(crate) fn status_from_dir(dir: &Path, alive_hint: Option<u32>) -> LoopdStatus {
    let state = read_state(dir);
    let mut out = LoopdStatus {
        state: "no_state",
        alive: false,
        pid: None,
        task_status: String::new(),
        task_excerpt_len: 0,
        last_heartbeat: None,
        age_secs: None,
        loop_dir: dir.display().to_string(),
    };

    let v = match state {
        Some(v) => v,
        None => {
            // No STATE.json — but TASK.md may still exist from a
            // previous run. Peek at it for the renderer.
            out.task_status = peek_task_status(dir);
            out.task_excerpt_len = peek_task_body_len(dir);
            return out;
        }
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
    out.task_status = peek_task_status(dir);
    out.task_excerpt_len = peek_task_body_len(dir);
    out
}

/// Cheap peek at TASK.md's `status:` line. Returns "" if the file is
/// missing or the frontmatter is malformed. This intentionally does
/// NOT parse the whole frontmatter — the renderer's `loopd_read_task`
/// does that on explicit Read. We just want the status pill color.
fn peek_task_status(dir: &Path) -> String {
    let p = dir.join("TASK.md");
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return String::new();
    };
    let mut lines = raw.lines();
    let Some(first) = lines.next() else {
        return String::new();
    };
    if first.trim() != "---" {
        return String::new();
    }
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        if let Some(rest) = line.strip_prefix("status:") {
            return rest.trim().to_string();
        }
    }
    String::new()
}

/// Cheap byte-length peek at TASK.md's body (everything after the
/// closing `---` of the frontmatter, trimmed). Used to drive the
/// "task changed since last full Read" hint in the UI without
/// pulling the body on every 2s tick.
fn peek_task_body_len(dir: &Path) -> u64 {
    let p = dir.join("TASK.md");
    let Ok(raw) = std::fs::read_to_string(&p) else {
        return 0;
    };
    let mut lines = raw.lines();
    let Some(first) = lines.next() else {
        return 0;
    };
    if first.trim() != "---" {
        // No frontmatter — the whole file is body.
        return raw.trim().len() as u64;
    }
    let mut in_body = false;
    let mut body_len = 0u64;
    for line in lines {
        if !in_body && line.trim() == "---" {
            in_body = true;
            continue;
        }
        if in_body {
            body_len = body_len.saturating_add(line.len() as u64 + 1); // +1 for \n
        }
    }
    body_len
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

    let bin = resolve_loopd_binary()?;
    log::info!("loopd_start: resolved binary = {}", bin.display());
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
        // Drop a TASK.md so the peek paths have something to read.
        std::fs::write(
            dir.join("TASK.md"),
            "---\nstatus: done\nupdated_at: 2026-09-11T18:00:00+00:00\n---\n\ntask body here\n",
        )
        .unwrap();
        let s = status_from_dir(&dir, None);
        assert_eq!(s.state, "dead");
        assert!(!s.alive);
        assert_eq!(s.pid, Some(9_999_999));
        // v3.7.17 Slice 3 minimal poll shape — turn / completed_turn
        // are NOT in the polling response anymore. The renderer
        // asks for full info on explicit Read.
        assert_eq!(s.task_status, "done");
        assert!(s.task_excerpt_len > 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn peek_task_status_reads_frontmatter_status() {
        let dir = fresh_loop_dir("peek-status");
        std::fs::write(
            dir.join("TASK.md"),
            "---\nstatus: blocked\nupdated_at: 2026-09-11T18:00:00+00:00\n---\n\nbody\n",
        )
        .unwrap();
        assert_eq!(peek_task_status(&dir), "blocked");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn peek_task_status_returns_empty_when_no_file() {
        let dir = fresh_loop_dir("peek-status-empty");
        assert_eq!(peek_task_status(&dir), "");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn peek_task_body_len_includes_only_body() {
        let dir = fresh_loop_dir("peek-bodylen");
        std::fs::write(
            dir.join("TASK.md"),
            "---\nstatus: running\nupdated_at: x\n---\n\nthis is the body of the task\n",
        )
        .unwrap();
        let len = peek_task_body_len(&dir);
        assert!(len > 0, "len should be > 0 for a real body, got {len}");
        // Should not include the frontmatter bytes (the `status: running` line etc).
        let full = std::fs::read_to_string(dir.join("TASK.md")).unwrap();
        assert!(len < full.len() as u64, "peek should exclude frontmatter, got {len} >= {}", full.len());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_bundled_loopd_path_finds_sibling() {
        // Simulate the packaged layout: MaxBot.app/Contents/MacOS/
        //   maxbot
        //   maxbot_loopd
        let root = make_dir("resolve-bundled");
        let contents = root.join("Contents").join("MacOS");
        std::fs::create_dir_all(&contents).unwrap();
        // Touch a fake "main" binary and a fake "loopd" binary.
        std::fs::write(contents.join("maxbot"), b"#!/bin/sh\n").unwrap();
        std::fs::write(contents.join("maxbot_loopd"), b"#!/bin/sh\n").unwrap();
        let resolved = resolve_bundled_loopd_path(&contents);
        assert_eq!(resolved, Some(contents.join("maxbot_loopd")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_bundled_loopd_path_returns_none_when_absent() {
        // Empty dir, no maxbot_loopd.
        let root = make_dir("resolve-bundled-empty");
        let contents = root.join("Contents").join("MacOS");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(contents.join("maxbot"), b"x").unwrap();
        let resolved = resolve_bundled_loopd_path(&contents);
        assert_eq!(resolved, None);
        std::fs::remove_dir_all(&root).ok();
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
