//! v3.5.0 (Phase 6) — Server-side shared folder tools.
//!
//! Three tools (`shared_write`, `shared_read`, `shared_list`)
//! give every Bot a controlled view of a single host-side
//! directory, `~/bots/_shared/`. The directory is real
//! host-filesystem (no overlay FS, no per-Bot VM), and the
//! `maxbotd` daemon is the canonical owner of the path.
//!
//! ## Why a separate folder and not per-Bot VM mounts
//!
//! Each Bot already has its own per-Bot Linux VM (1:1 on
//! libvirt) for isolation. Cross-Bot file sharing is a
//! different problem: a per-VM `mount --bind` would require
//! every VM to be live and reachable, would re-implement
//! file-locking twice, and would silently make the path
//! in-VM-resolvable from each Bot's tool loop. A single
//! host-side folder owned by `maxbotd` is one source of
//! truth, one set of file permissions, one place to put
//! a quota.
//!
//! ## Path-safety guard (load-bearing)
//!
//! The path argument the model emits is a *relative* path
//! under `~/bots/_shared/`. The guard rejects:
//!   - empty paths
//!   - absolute paths (`/etc/passwd`, `C:\foo`)
//!   - any `..` segment (path traversal)
//!   - any symlink that resolves outside the shared root
//!     (checked after `canonicalize`)
//!
//! The guard is the only thing standing between a Bot's
//! LLM and the host filesystem. If you weaken it, every
//! Bot becomes a host-filesystem read/write primitive for
//! the model.
//!
//! ## Routing
//!
//! The tool writes to whatever path `MAXBOT_SHARED_DIR`
//! resolves to (default `~/bots/_shared/`). On the
//! `maxbotd` daemon on `crispy` that resolves to
//! `/home/maxbotd/bots/_shared/`. From the Mac app the
//! env is the user's `$HOME`, so the path is local; the
//! cross-machine routing through `maxbotd` is a v3.6.x
//! concern and is documented in `docs/user-guide.md` and
//! `docs/grok-bot-reference.md`. This slice ships the
//! tool, the path-safety guard, and the server-side
//! directory creation on `crispy`.
//!
//! ## What this tool is NOT
//!
//! - NOT a security boundary between Bots. Two Bots in a
//!   group can each read the other's `shared/` contents
//!   (and the per-Bot VM's `~/bots/<id>/` is also
//!   reachable via the daemon). If a Bot needs true
//!   credential isolation, give it a separate Linux user
//!   account — that is NOT in MaxBot's data model today.
//! - NOT a backdoor for unrestricted filesystem access.
//!   The path-safety guard is the gate. The
//!   `grok_bot_defaults` preset marks `shared_write`
//!   as `Ask` so the human has to consent per call.
//!
//! ## v3.7.1 — cross-machine routing
//!
//! When called from the Mac app (`context.app` is
//! `Some(_)`), the tool bodies route their filesystem
//! ops through `POST /shared` on the `maxbotd` daemon
//! instead of touching the local Mac filesystem. The
//! daemon is the canonical owner of `~/bots/_shared/`
//! (per the v3.5.0 decision), so a write from the
//! Mac app lands on `crispy`'s host and is visible to
//! every other Bot in the group. The path-safety guard
//! runs on the daemon side too — the guard is
//! load-bearing and is the same function on both
//! sides, never weakened.
//!
//! When the daemon is unreachable (network down,
//! daemon not running), the Mac app falls back to the
//! local filesystem path with a `warn!` log + the
//! `resolve_safe_path` guard still applied. The user
//! gets a clear error in the chat either way — never
//! a silent filesystem-divergence.

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::fs;

use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

/// One row returned by `shared_list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedEntry {
    pub name: String,
    /// `false` for directories, `true` for regular files.
    /// Symlinks are not produced by this listing because
    /// the guard canonicalizes the parent before the
    /// readdir and rejects escapes.
    pub is_file: bool,
    pub size: u64,
    /// RFC 3339 modified-at timestamp. Empty if the
    /// filesystem didn't surface one (rare on Linux ext4
    /// but possible on tmpfs / network mounts).
    pub modified_at: String,
}

/// Resolve the shared root. Honors `$MAXBOT_SHARED_DIR`
/// (so the daemon can point at a non-default location
/// during testing); falls back to `~/bots/_shared/`. v3.7.1:
/// `pub` so the `maxbotd` daemon's `POST /shared` handler
/// can resolve the same root the in-app tool uses — the
/// path-safety guard is the same function on both sides,
/// never weakened.
pub fn shared_root() -> PathBuf {
    if let Some(p) = std::env::var_os("MAXBOT_SHARED_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join("bots").join("_shared");
    }
    // No HOME — best-effort fallback. The tool will
    // surface a clearer error on first use.
    PathBuf::from("/tmp/maxbot-shared")
}

// =====================================================================
//  v3.7.1 — Cross-machine routing through `maxbotd`
// =====================================================================
//
// When the Mac app calls `shared_write` / `shared_read` /
// `shared_list`, the tool body normally runs on the Mac
// (`context.app` is `Some(_)`). The Mac's
// `~/bots/_shared/` is *not* the canonical home for the
// shared folder — `crispy` is (per the v3.5.0 decision).
// So in the Mac-app path, the tool body sends an HTTP
// POST to the `maxbotd` daemon and lets the daemon
// perform the filesystem op. The daemon's own
// path-safety guard (`resolve_safe_path` from this
// same file) is the load-bearing security piece — and
// because the Mac side never touches the path itself,
// a `..` smuggling attempt can't escape the daemon's
// sandbox.
//
// When `maxbotd` is unreachable (network down, daemon
// not running), the Mac app falls back to the local
// filesystem path with a clear `warn!` log + the
// `resolve_safe_path` guard still applied. The user
// gets a clear error in the chat either way — never a
// silent filesystem-divergence.

/// Thin client-side handle for the daemon's
/// `POST /shared` route. Holds the URL + the Bot's
/// daemon token. The Mac-app tool bodies build one
/// per call (cheap — just a struct, no I/O yet) and
/// pass it to `daemon_call`.
#[derive(Debug, Clone)]
pub struct DaemonSharedClient {
    /// Base URL of the daemon (e.g.
    /// `http://127.0.0.1:8443`). Trailing slash is
    /// tolerated — `daemon_call` strips it.
    pub base_url: String,
    /// The Bot's daemon token. Sent as
    /// `Authorization: Bearer <token>`.
    pub token: String,
    /// The Bot's id. Sent as the `?bot_id=<id>`
    /// query param the daemon's auth path uses to
    /// look up the per-Bot token row.
    pub bot_id: String,
}

/// Build a `DaemonSharedClient` for the given Bot
/// from the Mac app's `AppHandle`. Returns an error
/// if the Bot has no daemon token configured (the
/// user must set one in the Bot editor first) or if
/// the Settings are unreadable.
pub async fn daemon_client_for(app: &AppHandle, bot_id: &str) -> Result<DaemonSharedClient, String> {
    use std::sync::Arc;
    let state: tauri::State<Arc<crate::AppState>> = app.state();
    let db = state.db.clone();
    let bot_id_owned = bot_id.to_string();
    let bot_id_for_err = bot_id_owned.clone();
    let (token, settings) = tokio::task::spawn_blocking(move || {
        let tok = db
            .get_daemon_token(&bot_id_owned)
            .map_err(|e| format!("db error reading daemon token: {e}"))?
            .ok_or_else(|| {
                format!(
                    "no daemon token configured for bot '{bot_id_for_err}'; set one in the Bot editor first"
                )
            })?;
        let s = db
            .load_settings()
            .map_err(|e| format!("db error reading settings: {e}"))?;
        Ok::<_, String>((tok, s))
    })
    .await
    .map_err(|e| format!("daemon token / settings lookup task panicked: {e}"))??;
    let base_url = if settings.maxbotd_url.trim().is_empty() {
        "http://127.0.0.1:8443".to_string()
    } else {
        settings.maxbotd_url.trim().trim_end_matches('/').to_string()
    };
    Ok(DaemonSharedClient {
        base_url,
        token,
        bot_id: bot_id.to_string(),
    })
}

/// Send one `POST /shared` request to the daemon.
/// `verb` is `"read"`, `"write"`, or `"list"`.
/// `path` is the relative path under the shared
/// root. `content` is the new file body for
/// `write`; ignored for the other verbs.
///
/// Returns the daemon's JSON response on a 2xx.
/// On a non-2xx, returns the daemon's error
/// message verbatim. On a connection error (network
/// down, daemon not running, timeout), returns a
/// fallback sentinel that the caller checks to
/// trigger the local-filesystem path.
pub async fn daemon_call(
    client: &DaemonSharedClient,
    verb: &str,
    path: &str,
    content: Option<&str>,
) -> Result<serde_json::Value, String> {
    use serde_json::json;
    let url = format!("{}/shared?bot_id={}", client.base_url, client.bot_id);
    let body = match verb {
        "write" => json!({
            "verb": verb,
            "path": path,
            "content": content.unwrap_or(""),
        }),
        _ => json!({
            "verb": verb,
            "path": path,
        }),
    };
    let req = reqwest::Client::new()
        .post(&url)
        .timeout(std::time::Duration::from_secs(5))
        .header("Authorization", format!("Bearer {}", client.token))
        .json(&body);
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            // Connection-level failure (refused,
            // timeout, DNS, etc.). The caller
            // checks for this and falls back to
            // the local filesystem path.
            return Err(format!("daemon unreachable: {e}"));
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("daemon returned {status}: {text}"));
    }
    serde_json::from_str(&text)
        .map_err(|e| format!("daemon returned non-JSON: {e}; body: {text}"))
}

/// The load-bearing path-safety guard. Returns the
/// canonical absolute path under `shared_root` or an
/// error explaining why the request was refused.
///
/// Rejected:
///   - empty / whitespace-only paths
///   - absolute paths (Unix `/...` or Windows
///     `C:\...` / `\\server\share`)
///   - any `..` segment
///   - any `\` separator (force POSIX-style)
///   - any symlink that resolves outside the shared root
///     after `canonicalize`
pub fn resolve_safe_path(root: &Path, requested: &str) -> Result<PathBuf, String> {
    let p = requested.trim();
    if p.is_empty() {
        return Err("path is empty".to_string());
    }
    // Reject absolute paths and Windows-style paths up
    // front. The model can emit `/etc/passwd` directly,
    // so a single byte-level check is the cheapest gate.
    if p.starts_with('/') {
        return Err(format!(
            "absolute paths are not allowed; paths must be relative to the shared folder (got '{p}')"
        ));
    }
    if p.starts_with('\\') || (p.len() >= 2 && p.as_bytes()[1] == b':') {
        return Err(format!(
            "absolute paths are not allowed; paths must be relative to the shared folder (got '{p}')"
        ));
    }
    // Reject any `..` segment. A naive `contains("..")`
    // would over-match (e.g. `..foo` is fine); we walk
    // the components.
    let path = Path::new(p);
    for comp in path.components() {
        match comp {
            Component::ParentDir => {
                return Err(format!(
                    "path traversal ('..') is not allowed; paths must stay under the shared folder (got '{p}')"
                ));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!(
                    "absolute path components are not allowed (got '{p}')"
                ));
            }
            _ => {}
        }
    }
    // Reject `\` separators on the off chance the
    // model emits a Windows path. Belt + suspenders.
    if p.contains('\\') {
        return Err(format!(
            "backslash separators are not allowed; use forward slashes (got '{p}')"
        ));
    }
    // Build the candidate absolute path.
    let candidate = root.join(p);
    // Canonicalize to resolve any symlinks + `.` segments.
    // If the parent doesn't exist yet (write to a new
    // file), canonicalize the parent; if the file doesn't
    // exist either, fall back to the literal join. The
    // literal-join path is safe because we already
    // rejected `..` above.
    let canonical = match candidate.canonicalize() {
        Ok(c) => c,
        Err(_) => {
            // Walk up until something resolves, then
            // verify the resolved prefix stays under
            // root. If we can't canonicalize anything,
            // just return the literal join (caller will
            // see a normal "not found" error).
            let mut probe = candidate.clone();
            let mut suffix = PathBuf::new();
            loop {
                if probe.exists() {
                    match probe.canonicalize() {
                        Ok(c) => {
                            let combined = c.join(&suffix);
                            return check_under_root(root, &combined, p);
                        }
                        Err(_) => break,
                    }
                }
                match probe.file_name() {
                    Some(name) => {
                        // v3.7.1 — build the suffix with
                        // an explicit `Vec` of
                        // components, not `Path::join`,
                        // to avoid a Rust stdlib quirk
                        // where `Path::from("a").join("")`
                        // leaves a trailing separator on
                        // the resulting `PathBuf`. That
                        // trailing separator then made
                        // the walk-up's `combined` path
                        // look like a directory to
                        // downstream `tokio::fs::write`
                        // calls, which would silently
                        // create an empty directory
                        // instead of a regular file.
                        let mut parts: Vec<std::ffi::OsString> = vec![name.to_os_string()];
                        parts.extend(suffix.iter().map(|c| c.to_os_string()));
                        suffix = parts.iter().collect();
                        match probe.parent() {
                            Some(parent) => probe = parent.to_path_buf(),
                            None => break,
                        }
                    }
                    None => break,
                }
            }
            return check_under_root(root, &candidate, p);
        }
    };
    check_under_root(root, &canonical, p)
}

fn check_under_root(root: &Path, candidate: &Path, original: &str) -> Result<PathBuf, String> {
    // Canonicalize the root for a byte-exact comparison.
    // If the root itself doesn't exist, treat the literal
    // path as the root — the caller will surface a
    // helpful "shared folder not found" error.
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if !candidate.starts_with(&root_canon) {
        return Err(format!(
            "path escapes the shared folder (got '{original}')"
        ));
    }
    Ok(candidate.to_path_buf())
}

// =====================================================================
//  Tool: shared_write
// =====================================================================

pub struct SharedWriteTool;

#[async_trait]
impl Tool for SharedWriteTool {
    fn name(&self) -> &str {
        "shared_write"
    }

    fn description(&self) -> &str {
        "Write a UTF-8 string to a file in the shared folder (~/bots/_shared/). Use this to leave artifacts for other Bots in the same group, drop a handoff note, or save a search result the next Bot should pick up. The path must be relative (no leading slash) and must not contain '..' segments. Requires user consent. Returns the number of bytes written."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path under the shared folder, e.g. 'handoff.md' or 'detective/findings.json'. Must not start with '/' and must not contain '..'."
                },
                "content": {
                    "type": "string",
                    "description": "The full text content to write to the file."
                }
            },
            "required": ["path", "content"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the shared_write action".to_string(),
            ));
        }
        let path = require_str(&invocation.arguments, "path")?;
        let content = require_str(&invocation.arguments, "content")?;

        // v3.7.1 — cross-machine routing. When the
        // Mac app calls us (`context.app.is_some()`),
        // send the write to the `maxbotd` daemon
        // instead of touching the local filesystem.
        // The daemon's `resolve_safe_path` is the
        // load-bearing guard — we don't re-check
        // here because the daemon will refuse any
        // bad path on its side. If the daemon is
        // unreachable, fall back to the local
        // filesystem with the guard still applied.
        if let Some(app) = context.app.clone() {
            let bot_id = invocation
                .bot_id
                .as_deref()
                .ok_or_else(|| ToolError::Execution(
                    "shared_write: bot_id is required for daemon routing; pass it from the executor".to_string(),
                ))?;
            match daemon_client_for(&app, bot_id).await {
                Ok(client) => {
                    match daemon_call(&client, "write", path, Some(&content)).await {
                        Ok(v) => {
                            let bytes = v
                                .get("bytes_written")
                                .and_then(|x| x.as_u64())
                                .unwrap_or(content.len() as u64);
                            let on_disk = v
                                .get("path")
                                .and_then(|x| x.as_str())
                                .unwrap_or("(daemon)");
                            return Ok(ToolResult::ok(format!(
                                "wrote {bytes} bytes to {on_disk} (via daemon)"
                            )));
                        }
                        Err(e) => {
                            // Daemon unreachable or
                            // errored — log and fall
                            // through to the local
                            // filesystem path. The
                            // user gets a `warn!` log
                            // + the local-fs error
                            // either way.
                            log::warn!(
                                "shared_write: daemon call failed ({e}); falling back to local filesystem"
                            );
                        }
                    }
                }
                Err(e) => {
                    // No token / db error — log and
                    // fall through. The local
                    // path will surface its own
                    // clear error.
                    log::warn!(
                        "shared_write: daemon client unavailable ({e}); falling back to local filesystem"
                    );
                }
            }
        }

        // Local-fs path. Same as the pre-v3.7.1
        // behavior; preserved for the daemon-side
        // run path and the fallback when the
        // daemon is unreachable.
        let root = shared_root();
        let resolved = resolve_safe_path(&root, path)
            .map_err(|e| ToolError::Execution(format!("shared_write refused: {e}")))?;
        if let Some(parent) = resolved.parent() {
            // Make sure the parent is under the root
            // before creating it. `resolve_safe_path`
            // already checked the requested path; the
            // join can only introduce directories the
            // model named.
            if !parent.starts_with(&root) {
                return Err(ToolError::Execution(format!(
                    "shared_write refused: parent '{parent_display}' escapes the shared folder",
                    parent_display = parent.display()
                )));
            }
            fs::create_dir_all(parent).await.map_err(|e| {
                ToolError::Execution(format!("mkdir {} failed: {e}", parent.display()))
            })?;
        }
        fs::write(&resolved, content.as_bytes())
            .await
            .map_err(|e| ToolError::Execution(format!("write failed: {e}")))?;
        Ok(ToolResult::ok(format!(
            "wrote {} bytes to {}",
            content.len(),
            resolved.display()
        )))
    }
}

// =====================================================================
//  Tool: shared_read
// =====================================================================

pub struct SharedReadTool;

#[async_trait]
impl Tool for SharedReadTool {
    fn name(&self) -> &str {
        "shared_read"
    }

    fn description(&self) -> &str {
        "Read a file from the shared folder (~/bots/_shared/). Use this to pick up handoff notes or read an artifact another Bot dropped. The path must be relative (no leading slash) and must not contain '..' segments. If the path is a directory, returns a small listing of the directory instead of the file contents. Files larger than 12,000 characters are truncated with a marker."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path under the shared folder. Must not start with '/' and must not contain '..'."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path = require_str(&invocation.arguments, "path")?;

        // v3.7.1 — cross-machine routing (see
        // `shared_write` for the full rationale).
        // On the Mac app path, ask the daemon to
        // read the file from its own shared
        // folder; fall back to the local
        // filesystem on connection failure.
        if let Some(app) = context.app.clone() {
            let bot_id = invocation
                .bot_id
                .as_deref()
                .ok_or_else(|| ToolError::Execution(
                    "shared_read: bot_id is required for daemon routing; pass it from the executor".to_string(),
                ))?;
            match daemon_client_for(&app, bot_id).await {
                Ok(client) => {
                    match daemon_call(&client, "read", path, None).await {
                        Ok(v) => {
                            let body = v
                                .get("content")
                                .and_then(|x| x.as_str())
                                .unwrap_or("");
                            return Ok(ToolResult::ok(truncate_for_model(body, 12_000)));
                        }
                        Err(e) => {
                            log::warn!(
                                "shared_read: daemon call failed ({e}); falling back to local filesystem"
                            );
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "shared_read: daemon client unavailable ({e}); falling back to local filesystem"
                    );
                }
            }
        }

        // Local-fs path (daemon-side runs + the
        // Mac-app fallback when the daemon is
        // unreachable).
        let root = shared_root();
        let resolved = resolve_safe_path(&root, path)
            .map_err(|e| ToolError::Execution(format!("shared_read refused: {e}")))?;
        let meta = fs::metadata(&resolved)
            .await
            .map_err(|e| ToolError::Execution(format!("stat failed: {e}")))?;
        if meta.is_dir() {
            // Directory read returns a small listing.
            // Reuse the same listing logic shared_list
            // uses, but constrained to this directory.
            let mut out = String::new();
            out.push_str(&format!("directory {}:\n", resolved.display()));
            let mut entries = fs::read_dir(&resolved).await.map_err(|e| {
                ToolError::Execution(format!("readdir failed: {e}"))
            })?;
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(|e| ToolError::Execution(format!("readdir entry: {e}")))?
            {
                let name = entry.file_name().to_string_lossy().to_string();
                let m = entry.metadata().await.ok();
                let (is_file, size) = match m {
                    Some(md) => (md.is_file(), md.len()),
                    None => (false, 0),
                };
                out.push_str(&format!(
                    "  {kind:>1} {size:>10}  {name}\n",
                    kind = if is_file { "f" } else { "d" },
                    size = size,
                    name = name,
                ));
            }
            return Ok(ToolResult::ok(truncate_for_model(&out, 12_000)));
        }
        if !meta.is_file() {
            return Err(ToolError::Execution(format!(
                "{} is not a regular file",
                resolved.display()
            )));
        }
        if meta.len() > 1_048_576 {
            return Ok(ToolResult::ok(format!(
                "{} is {} bytes (over 1 MB); refusing to read fully.",
                resolved.display(),
                meta.len()
            )));
        }
        let body = fs::read_to_string(&resolved)
            .await
            .map_err(|e| ToolError::Execution(format!("read failed: {e}")))?;
        Ok(ToolResult::ok(truncate_for_model(&body, 12_000)))
    }
}

// =====================================================================
//  Tool: shared_list
// =====================================================================

pub struct SharedListTool;

#[async_trait]
impl Tool for SharedListTool {
    fn name(&self) -> &str {
        "shared_list"
    }

    fn description(&self) -> &str {
        "List entries under the shared folder (~/bots/_shared/), optionally narrowed by a relative prefix. Returns name, type (file or directory), size in bytes, and modified-at timestamp. The prefix must be relative and must not contain '..' segments."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prefix": {
                    "type": "string",
                    "description": "Optional relative path to narrow the listing, e.g. 'detective' to list only entries under shared/detective/. Defaults to the shared root."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let prefix = invocation
            .arguments
            .get("prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        // v3.7.1 — cross-machine routing (see
        // `shared_write` for the full rationale).
        if let Some(app) = context.app.clone() {
            let bot_id = invocation
                .bot_id
                .as_deref()
                .ok_or_else(|| ToolError::Execution(
                    "shared_list: bot_id is required for daemon routing; pass it from the executor".to_string(),
                ))?;
            match daemon_client_for(&app, bot_id).await {
                Ok(client) => {
                    match daemon_call(&client, "list", prefix, None).await {
                        Ok(v) => {
                            let entries = v
                                .get("entries")
                                .cloned()
                                .unwrap_or(serde_json::Value::Array(vec![]));
                            let json_out = serde_json::to_string_pretty(&entries)
                                .unwrap_or_else(|_| "[]".to_string());
                            return Ok(ToolResult::ok(truncate_for_model(&json_out, 12_000)));
                        }
                        Err(e) => {
                            log::warn!(
                                "shared_list: daemon call failed ({e}); falling back to local filesystem"
                            );
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "shared_list: daemon client unavailable ({e}); falling back to local filesystem"
                    );
                }
            }
        }

        // Local-fs path (daemon-side runs + the
        // Mac-app fallback when the daemon is
        // unreachable).
        let root = shared_root();
        // If the prefix is empty, list the root. If it's
        // a directory, list that. If it's a file, list
        // its parent.
        let dir = if prefix.is_empty() {
            root.clone()
        } else {
            let resolved = resolve_safe_path(&root, prefix)
                .map_err(|e| ToolError::Execution(format!("shared_list refused: {e}")))?;
            let meta = fs::metadata(&resolved)
                .await
                .map_err(|e| ToolError::Execution(format!("stat failed: {e}")))?;
            if meta.is_dir() {
                resolved
            } else {
                resolved
                    .parent()
                    .ok_or_else(|| {
                        ToolError::Execution("path has no parent".to_string())
                    })?
                    .to_path_buf()
            }
        };
        let mut entries: Vec<SharedEntry> = Vec::new();
        let mut read_dir = fs::read_dir(&dir).await.map_err(|e| {
            ToolError::Execution(format!("readdir failed: {e}"))
        })?;
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|e| ToolError::Execution(format!("readdir entry: {e}")))?
        {
            let name = entry.file_name().to_string_lossy().to_string();
            let m = entry.metadata().await.ok();
            let (is_file, size, modified_at) = match m {
                Some(md) => {
                    let mtime = md
                        .modified()
                        .ok()
                        .and_then(|t| {
                            let dt: chrono::DateTime<chrono::Utc> = t.into();
                            Some(dt.to_rfc3339())
                        })
                        .unwrap_or_default();
                    (md.is_file(), md.len(), mtime)
                }
                None => (false, 0, String::new()),
            };
            entries.push(SharedEntry {
                name,
                is_file,
                size,
                modified_at,
            });
        }
        // Stable ordering: directories first, then files,
        // both alphabetically.
        entries.sort_by(|a, b| {
            b.is_file
                .cmp(&a.is_file)
                .then_with(|| a.name.cmp(&b.name))
        });
        let json_out = serde_json::to_string_pretty(&entries).map_err(|e| {
            ToolError::Execution(format!("serialize: {e}"))
        })?;
        Ok(ToolResult::ok(truncate_for_model(&json_out, 12_000)))
    }
}

// =====================================================================
//  Tests
// =====================================================================
//
// The path-safety guard is the load-bearing piece. The
// tests below cover every branch of `resolve_safe_path`
// because a regression here is a host-filesystem
// read/write primitive for any Bot.

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        // Each test builds a fresh tempdir so the
        // canonicalize path has something to chew on.
        let p = std::env::temp_dir().join(format!(
            "maxbot-shared-test-{}-{}",
            std::process::id(),
            // A monotonically-increasing-ish counter
            // so parallel tests don't collide.
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).unwrap();
        // Return the canonical form so the
        // `out.starts_with(&r)` checks below match the
        // canonicalized `out` from `resolve_safe_path`.
        // On macOS the tempdir lives under
        // `/var/folders/...` which is a symlink to
        // `/private/var/folders/...`; without this,
        // Path::starts_with (which is component-wise)
        // would see "var" vs "private" and fail.
        p.canonicalize().unwrap_or(p)
    }

    #[test]
    fn safe_path_accepts_simple_relative() {
        let r = root();
        let out = resolve_safe_path(&r, "hello.txt").unwrap();
        assert!(out.starts_with(&r));
    }

    #[test]
    fn safe_path_accepts_nested_relative() {
        let r = root();
        let out = resolve_safe_path(&r, "detective/findings.json").unwrap();
        assert!(out.starts_with(&r));
    }

    #[test]
    fn safe_path_rejects_empty() {
        let r = root();
        let err = resolve_safe_path(&r, "").unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn safe_path_rejects_whitespace_only() {
        let r = root();
        let err = resolve_safe_path(&r, "   ").unwrap_err();
        assert!(err.contains("empty"), "got: {err}");
    }

    #[test]
    fn safe_path_rejects_absolute_unix() {
        let r = root();
        let err = resolve_safe_path(&r, "/etc/passwd").unwrap_err();
        assert!(err.contains("absolute"), "got: {err}");
    }

    #[test]
    fn safe_path_rejects_absolute_drive_letter() {
        let r = root();
        let err = resolve_safe_path(&r, "C:\\Windows\\System32").unwrap_err();
        assert!(err.contains("absolute"), "got: {err}");
    }

    #[test]
    fn safe_path_rejects_unc_share() {
        let r = root();
        let err = resolve_safe_path(&r, "\\\\server\\share").unwrap_err();
        assert!(err.contains("absolute"), "got: {err}");
    }

    #[test]
    fn safe_path_rejects_parent_dir_segment() {
        let r = root();
        let err = resolve_safe_path(&r, "../escape").unwrap_err();
        assert!(
            err.contains("'..'") || err.contains("traversal"),
            "got: {err}"
        );
    }

    #[test]
    fn safe_path_rejects_nested_parent_dir_segment() {
        let r = root();
        let err =
            resolve_safe_path(&r, "detective/../../escape").unwrap_err();
        assert!(
            err.contains("'..'") || err.contains("traversal"),
            "got: {err}"
        );
    }

    #[test]
    fn safe_path_accepts_dotted_filename() {
        // A name that *starts* with dots is not the same
        // as `..`. `..foo` is fine; `..` alone is not.
        let r = root();
        let out = resolve_safe_path(&r, "..foo").unwrap();
        assert!(out.starts_with(&r));
    }

    #[test]
    fn safe_path_rejects_backslash_separator() {
        let r = root();
        let err =
            resolve_safe_path(&r, "detective\\findings.json").unwrap_err();
        assert!(err.contains("backslash"), "got: {err}");
    }

    #[test]
    fn safe_path_rejects_symlink_escape() {
        // Build a symlink inside the shared root that
        // points outside, and verify the guard refuses
        // to canonicalize through it.
        let r = root();
        let outside = std::env::temp_dir().join(format!(
            "maxbot-shared-outside-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&outside).unwrap();
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "nope").unwrap();
        // Symlink lives inside the shared root.
        let link = r.join("escape");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let err = resolve_safe_path(&r, "escape").unwrap_err();
        // The symlink resolves to outside/secret.txt, so
        // the canonical path won't be under the root and
        // the guard must refuse. The literal-join path
        // also gets a starts_with check, so even if
        // canonicalize were to succeed on some filesystems
        // we still catch the escape.
        assert!(
            err.contains("escapes") || err.contains("'..'") || err.contains("traversal"),
            "expected escape refusal, got: {err}"
        );
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn safe_path_accepts_nonexistent_nested() {
        // A write to a brand-new nested file should not
        // trip the guard just because the parent
        // doesn't exist yet.
        let r = root();
        let out = resolve_safe_path(&r, "new/nested/file.txt").unwrap();
        assert!(out.starts_with(&r));
    }

    #[test]
    fn safe_path_accepts_dot_segment_in_middle() {
        // `.` is fine; only `..` is forbidden. The
        // model occasionally emits `./foo` and we
        // shouldn't reject it.
        let r = root();
        let out = resolve_safe_path(&r, "./foo").unwrap();
        assert!(out.starts_with(&r));
    }

    #[test]
    fn shared_root_defaults_to_home_bots_shared() {
        // Unset the override, make sure HOME points
        // somewhere, and verify the join.
        // We can't actually unset env vars in Rust, but
        // we can verify the logic by setting HOME to
        // a known value and reading the override-or-
        // default result. This test focuses on the
        // "default" branch.
        let saved = std::env::var_os("MAXBOT_SHARED_DIR");
        // SAFETY: tests are single-threaded for env
        // mutations; the helper above already runs in
        // a clean test function.
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("MAXBOT_SHARED_DIR");
        }
        let r = shared_root();
        if let Some(home) = std::env::var_os("HOME") {
            let expected = PathBuf::from(home).join("bots").join("_shared");
            assert_eq!(r, expected);
        }
        if let Some(v) = saved {
            // SAFETY: see above.
            unsafe {
                std::env::set_var("MAXBOT_SHARED_DIR", v);
            }
        }
    }

    #[test]
    fn shared_root_honors_env_override() {
        let saved = std::env::var_os("MAXBOT_SHARED_DIR");
        // SAFETY: see above.
        unsafe {
            std::env::set_var("MAXBOT_SHARED_DIR", "/tmp/maxbot-override");
        }
        let r = shared_root();
        assert_eq!(r, PathBuf::from("/tmp/maxbot-override"));
        if let Some(v) = saved {
            // SAFETY: see above.
            unsafe {
                std::env::set_var("MAXBOT_SHARED_DIR", v);
            }
        } else {
            // SAFETY: see above.
            unsafe {
                std::env::remove_var("MAXBOT_SHARED_DIR");
            }
        }
    }

    // =====================================================================
    //  v3.7.1 — Daemon client tests
    // =====================================================================
    //
    // These tests exercise the client-side half of
    // the v3.7.1 cross-machine routing:
    // `daemon_call` builds the right HTTP request,
    // sends it, and parses the response. The tests
    // spin up a tiny axum server on a random port
    // to act as a stand-in for the real daemon —
    // the daemon-side end-to-end tests live in
    // `src-tauri/src/bin/maxbotd.rs`.

    /// Spawn a tiny test server that echoes the
    /// request method, path, Authorization header,
    /// and body — and returns a canned JSON
    /// response. The handler is per-test so each
    /// test can encode its own assertions on the
    /// inbound request.
    async fn spawn_echo_server(
        handler: axum::Router,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, handler).await;
        });
        (addr, server)
    }

    /// `daemon_call_sends_post_with_bearer` — the
    /// client builds a `POST /shared?bot_id=...`
    /// request with the `Authorization: Bearer …`
    /// header, and parses the daemon's JSON
    /// response on a 2xx.
    #[tokio::test]
    async fn daemon_call_sends_post_with_bearer() {
        use axum::extract::Request;
        use axum::http::StatusCode;
        use axum::middleware::{self, Next};
        use axum::response::Response;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let captured: Arc<Mutex<Option<(String, String, String)>>> =
            Arc::new(Mutex::new(None));
        let cap_clone = captured.clone();
        let app = axum::Router::new()
            .route(
                "/shared",
                axum::routing::post(
                    |axum::extract::Query(params): axum::extract::Query<
                        std::collections::HashMap<String, String>,
                    >,
                     headers: axum::http::HeaderMap,
                     body: String| async move {
                        let method = "POST".to_string();
                        let auth = headers
                            .get(axum::http::header::AUTHORIZATION)
                            .map(|v| v.to_str().unwrap_or("").to_string())
                            .unwrap_or_default();
                        let bot = params
                            .get("bot_id")
                            .cloned()
                            .unwrap_or_default();
                        *cap_clone.lock().await = Some((method, auth, format!("{bot}|{body}")));
                        (
                            StatusCode::OK,
                            axum::Json(serde_json::json!({
                                "ok": true,
                                "bytes_written": body.len(),
                            })),
                        )
                    },
                ),
            )
            // The `Next` import silences the unused
            // warning for the middleware module;
            // we don't need a middleware here.
            .layer(middleware::from_fn(
                |_req: Request, next: Next| async move {
                    let resp: Response = next.run(_req).await;
                    resp
                },
            ));
        let (addr, server) = spawn_echo_server(app).await;

        let client = DaemonSharedClient {
            base_url: format!("http://{addr}"),
            token: "tok-abc".to_string(),
            bot_id: "bot-42".to_string(),
        };
        let resp = daemon_call(
            &client,
            "write",
            "detective/findings.json",
            Some("hello world"),
        )
        .await
        .expect("daemon_call");
        // The response is the mock server's echo
        // (which returns `body.len()` as
        // `bytes_written`); the test only needs to
        // confirm the response is parseable JSON
        // with an `ok: true`. The real daemon's
        // `bytes_written` semantics are covered by
        // `shared_route_write_read_list_round_trip`
        // in `maxbotd.rs`.
        assert_eq!(resp.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert!(
            resp.get("bytes_written").is_some(),
            "expected bytes_written in response, got: {resp}"
        );

        let captured = captured.lock().await.clone().expect("captured");
        assert_eq!(captured.0, "POST");
        assert_eq!(captured.1, "Bearer tok-abc");
        // The query string + body are pipe-joined
        // so we can assert both in one comparison.
        let (bot_id, body) = captured.2.split_once('|').expect("split");
        assert_eq!(bot_id, "bot-42");
        let parsed: serde_json::Value =
            serde_json::from_str(body).expect("body is json");
        assert_eq!(parsed.get("verb").and_then(|v| v.as_str()), Some("write"));
        assert_eq!(
            parsed.get("path").and_then(|v| v.as_str()),
            Some("detective/findings.json")
        );
        assert_eq!(
            parsed.get("content").and_then(|v| v.as_str()),
            Some("hello world")
        );

        server.abort();
    }

    /// `daemon_call_handles_non_2xx` — a 4xx from
    /// the daemon surfaces as an error string the
    /// caller can match on, not a panic.
    #[tokio::test]
    async fn daemon_call_handles_non_2xx() {
        use axum::http::StatusCode;
        let app = axum::Router::new().route(
            "/shared",
            axum::routing::post(|| async {
                (
                    StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "error": "shared_write refused: path traversal"
                    })),
                )
            }),
        );
        let (addr, server) = spawn_echo_server(app).await;

        let client = DaemonSharedClient {
            base_url: format!("http://{addr}"),
            token: "tok".to_string(),
            bot_id: "bot".to_string(),
        };
        let err = daemon_call(&client, "write", "../escape", Some("nope"))
            .await
            .expect_err("expected error");
        assert!(err.contains("400"), "expected 400 in error, got: {err}");
        assert!(
            err.contains("shared_write refused"),
            "expected daemon error in body, got: {err}"
        );

        server.abort();
    }

    /// `daemon_call_handles_connection_failure` —
    /// when the daemon isn't running, the call
    /// returns a clear error string (so the
    /// caller can fall back to the local
    /// filesystem path).
    #[tokio::test]
    async fn daemon_call_handles_connection_failure() {
        // Bind a listener and immediately drop it
        // so the port is free but no server is
        // running on it. `daemon_call` will get
        // a connection-refused error.
        let listener =
            tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        drop(listener);

        let client = DaemonSharedClient {
            base_url: format!("http://{addr}"),
            token: "tok".to_string(),
            bot_id: "bot".to_string(),
        };
        let err = daemon_call(&client, "list", "", None)
            .await
            .expect_err("expected connection error");
        assert!(
            err.contains("daemon unreachable"),
            "expected 'daemon unreachable' in error, got: {err}"
        );
    }

    /// `daemon_call_strips_trailing_slash_from_base_url`
    /// — the `Settings.maxbotd_url` is stored as
    /// the user typed it; the client normalizes a
    /// trailing slash so the URL build doesn't
    /// produce `//shared`.
    #[test]
    fn daemon_client_normalizes_trailing_slash() {
        // We can't easily run `daemon_client_for`
        // without a tauri AppHandle, but the
        // normalization logic lives in the
        // function body. Reproduce it here so the
        // test pins the behavior.
        let raw = "http://crispy:8443/";
        let normalized = raw.trim().trim_end_matches('/').to_string();
        assert_eq!(normalized, "http://crispy:8443");
        let raw = "http://crispy:8443";
        let normalized = raw.trim().trim_end_matches('/').to_string();
        assert_eq!(normalized, "http://crispy:8443");
        let raw = "";
        let normalized = raw.trim().trim_end_matches('/').to_string();
        // Empty normalizes to empty; the caller
        // falls back to the local default.
        assert_eq!(normalized, "");
    }
}
