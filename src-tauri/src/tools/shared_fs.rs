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

use std::path::{Component, Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
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
/// during testing); falls back to `~/bots/_shared/`.
fn shared_root() -> PathBuf {
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
                        suffix = PathBuf::from(name).join(&suffix);
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
        let _ = context;
        let path = require_str(&invocation.arguments, "path")?;
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
        let _ = context;
        let prefix = invocation
            .arguments
            .get("prefix")
            .and_then(|v| v.as_str())
            .unwrap_or("");
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
}
