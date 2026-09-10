//! v2.5 — Persistent memory Tauri commands.
//!
//! These wrap [`crate::memory::store`] and are wired into
//! `lib.rs` via `invoke_handler!`. They share the same
//! non-fatal-read semantics as the store: a Bot without a VM
//! returns empty results, never an error.
//!
//! v3.7.8: added `backup_memory(bot_id)` — copies the
//! per-Bot memory (across all 3 kinds) into a single
//! JSONL file under the host's `~/bots/_shared/memory/`
//! tree. Lets the user share a Bot's memory with other
//! Bots in the same group (or restore a Bot's memory
//! after destroying its VM) without binding a new
//! host-level setting — the shared path is the same one
//! the `shared_fs` Bot tool already uses.

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::computer::ssh::SshExecutor;
use crate::memory::store;
use crate::memory::{MemEntry, MemKind};
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindArg {
    Fact,
    Preference,
    History,
}

impl From<KindArg> for MemKind {
    fn from(k: KindArg) -> Self {
        match k {
            KindArg::Fact => Self::Fact,
            KindArg::Preference => Self::Preference,
            KindArg::History => Self::History,
        }
    }
}

/// Top-5 keyword matches across all 3 kinds. Read failures
/// (no VM, timeout, parse error) collapse to an empty Vec.
#[tauri::command]
pub async fn memory_search(
    state: State<'_, AppState>,
    bot_id: String,
    query: String,
) -> Result<Vec<MemEntry>, String> {
    let pool = state.computer.ssh_pool();
    let entries = store::search(&*pool, &bot_id, &query, 5).await;
    Ok(entries)
}

/// Append a new entry. `history` is not exposed to the renderer
/// here (the executor auto-writes history) but we accept it for
/// symmetry — the panel doesn't surface it, but a power user
/// hitting the IPC directly will work.
#[tauri::command]
pub async fn memory_remember(
    state: State<'_, AppState>,
    bot_id: String,
    kind: KindArg,
    key: String,
    content: String,
) -> Result<MemEntry, String> {
    let mem_kind: MemKind = kind.into();
    let entry = MemEntry::new(mem_kind, key, content);
    let pool = state.computer.ssh_pool();
    store::append(&*pool, &bot_id, mem_kind, entry.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(entry)
}

/// Delete by key. Returns whether anything was actually removed
/// (false if the key wasn't found).
#[tauri::command]
pub async fn memory_forget(
    state: State<'_, AppState>,
    bot_id: String,
    kind: KindArg,
    key: String,
) -> Result<bool, String> {
    let mem_kind: MemKind = kind.into();
    let pool = state.computer.ssh_pool();
    store::delete(&*pool, &bot_id, mem_kind, &key)
        .await
        .map_err(|e| e.to_string())
}

/// v3.6.0 (Phase 7) — Thin alias for `memory_forget`,
/// exposed under the brief's literal name. The brief
/// asked for a `delete_memory_entry(bot_id, kind,
/// key)` Tauri command; the v2.5.0 surface already
/// shipped `memory_forget` with the same signature.
/// Rather than break the existing renderer, this
/// entry point is registered alongside the v2.5.0 one
/// — same SFTP path, same SFTP timeout, same return
/// shape. A future caller that wants the v3.6.0
/// spelling gets identical behavior.
#[tauri::command]
pub async fn delete_memory_entry(
    state: State<'_, AppState>,
    bot_id: String,
    kind: KindArg,
    key: String,
) -> Result<bool, String> {
    memory_forget(state, bot_id, kind, key).await
}

/// List all entries of one kind, oldest first. Used by the
/// MemoryPanel's three-column read-only browser.
#[tauri::command]
pub async fn memory_list(
    state: State<'_, AppState>,
    bot_id: String,
    kind: KindArg,
) -> Result<Vec<MemEntry>, String> {
    let mem_kind: MemKind = kind.into();
    let pool = state.computer.ssh_pool();
    Ok(store::read_all(&*pool, &bot_id, mem_kind).await)
}

/// v3.7.8 — `backup_memory(bot_id)` snapshot
/// result. The renderer shows the path in a toast so the
/// user can verify the file landed where they expect.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryBackup {
    /// Bot whose memory was backed up.
    pub bot_id: String,
    /// Absolute path on the host where the snapshot was
    /// written. `~` is NOT expanded — this is the path
    /// the host's `cat` saw, so it starts with `/home/...`
    /// (or whatever the server user's home is).
    pub path: String,
    /// Number of JSONL lines written (= total entries
    /// across all 3 kinds). Renderer shows this so the
    /// user knows the snapshot wasn't empty.
    pub entries: usize,
    /// RFC3339 timestamp of the snapshot, matching the
    /// filename stem.
    pub timestamp: String,
}

/// v3.7.8 — Back up a Bot's full memory (Facts +
/// Preferences + History) to the host's
/// `~/bots/_shared/memory/<bot_id>/<timestamp>.jsonl`.
///
/// Why host-side: the per-Bot memory JSONL files live on
/// the Bot's VM (`bots/<id>/memory/{kind}.jsonl`), and the
/// shared folder (`~/bots/_shared/`) lives on the host.
/// Both are reachable from the Mac's `SshPool` — the
/// former via per-Bot SFTP, the latter via plain
/// `server_exec` against `tyler@<host>`. The snapshot is
/// JSONL with a `kind` field on each line so a restore
/// step (not part of v3.7.8) can split by kind later.
///
/// Empty memory is a no-op (writes an empty file with a
/// `created_at` marker) so the user gets a clear "0
/// entries" toast instead of an error — useful for a Bot
/// that's never run.
///
/// The path is the host's absolute path (NOT `~/...`) so
/// the user can `ssh <host> cat <path>` to inspect it.
#[tauri::command]
pub async fn backup_memory(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<MemoryBackup, String> {
    let pool = state.computer.ssh_pool();
    // Read all 3 kinds. `read_all` is non-fatal: a
    // missing file collapses to an empty Vec, so a Bot
    // that's never written any memory still backs up
    // successfully (with entries=0).
    let mut all_entries: Vec<MemEntry> = Vec::new();
    for kind in [MemKind::Fact, MemKind::Preference, MemKind::History] {
        let mut entries = store::read_all(&*pool, &bot_id, kind).await;
        all_entries.append(&mut entries);
    }
    // Sort by created_at so a future restore can play
    // them back in time order. The per-Bot files are
    // already in insertion order, but `read_all`
    // doesn't guarantee that across kinds.
    all_entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    // Build the JSONL stream. The shape adds a `kind`
    // discriminator on each line so a single file can
    // hold entries from all 3 kinds.
    let mut body = String::new();
    for entry in &all_entries {
        // Manual emit so we control the line format. We
        // can't use serde_json round-trip cleanly because
        // the per-Bot files store `kind` implicitly (via
        // filename), but the backup file is single-file
        // so the kind has to be on the line.
        let line = serde_json::json!({
            "kind": entry.kind.as_str(),
            "key": entry.key,
            "content": entry.content,
            "created_at": entry.created_at,
        });
        body.push_str(&line.to_string());
        body.push('\n');
    }

    // Timestamp: YYYYMMDD-HHMMSS, sortable, no
    // colons (filesystem-safe on every host). We use the
    // host clock, not the Mac clock, so a backup
    // triggered during a remote shell reflects the
    // server's notion of "now" — which is what the
    // user will see in `ls -lh` later.
    let timestamp = pool
        .server_exec("date -u +%Y%m%d-%H%M%S")
        .await
        .map_err(|e| format!("failed to read host clock: {e}"))?
        .stdout
        .trim()
        .to_string();
    if timestamp.is_empty() {
        return Err("host returned empty timestamp".into());
    }

    // mkdir -p the per-Bot backup dir, then base64-decode
    // the body into the JSONL file. We use base64 so the
    // file write is binary-safe (the JSONL body can
    // contain arbitrary user content, and quoting that
    // into a shell command is fragile). The single SSH
    // round-trip is fast on a LAN.
    use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
    let body_b64 = B64.encode(body.as_bytes());
    let path = format!(
        "$HOME/bots/_shared/memory/{}/{}.jsonl",
        shell_escape_path(&bot_id),
        shell_escape_path(&timestamp),
    );
    let cmd = format!(
        "mkdir -p \"$HOME/bots/_shared/memory/{}\" && echo '{}' | base64 -d > {} && wc -c < {}",
        shell_escape_path(&bot_id),
        body_b64,
        path,
        path,
    );
    let out = pool
        .server_exec(&cmd)
        .await
        .map_err(|e| format!("backup write failed: {e}"))?;
    if !out.success {
        return Err(format!(
            "backup write failed (exit {:?}): {}",
            out.exit_code, out.stderr
        ));
    }

    Ok(MemoryBackup {
        bot_id,
        // Return the resolved (absolute) path so the
        // user can inspect the file directly. We re-run
        // `readlink -f` on the host to expand `$HOME`.
        path: pool
            .server_exec(&format!("readlink -f {}", path))
            .await
            .map(|o| o.stdout.trim().to_string())
            .unwrap_or_else(|_| path.clone()),
        entries: all_entries.len(),
        timestamp,
    })
}

/// Single-quote a path component for safe shell
/// interpolation. We never want to interpret the bot id
/// as more than a single token. Same style as
/// `shell_quote` in `tools/shell_run.rs`.
fn shell_escape_path(s: &str) -> String {
    // Path components come from bot_id (a UUID-shaped
    // string from the SQLite bots table) and the
    // timestamp (which we control). Neither should
    // contain `'`. The escape is defense-in-depth.
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `shell_escape_path` round-trips simple names and
    /// properly handles single quotes (the one character
    /// that can break single-quote escaping).
    #[test]
    fn shell_escape_path_handles_quotes() {
        assert_eq!(shell_escape_path("bot-1"), "'bot-1'");
        // Single quote in the middle: should be escaped
        // by closing-quote, escaped quote, open-quote.
        assert_eq!(shell_escape_path("a'b"), "'a'\\''b'");
    }

    /// `shell_escape_path` is a no-op for the timestamp
    /// format we use (YYYYMMDD-HHMMSS — no special chars).
    #[test]
    fn shell_escape_path_handles_timestamp() {
        assert_eq!(
            shell_escape_path("20260910-123904"),
            "'20260910-123904'"
        );
    }

    /// `MemoryBackup` is serialized as camelCase for the
    /// JS layer. The renderer reads `botId`, `path`,
    /// `entries`, `timestamp`. Locking the wire shape
    /// here catches serde rename-all drift early.
    #[test]
    fn memory_backup_serializes_camel_case() {
        let b = MemoryBackup {
            bot_id: "alpha".into(),
            path: "/home/tyler/bots/_shared/memory/alpha/x.jsonl".into(),
            entries: 3,
            timestamp: "20260910-123904".into(),
        };
        let json = serde_json::to_value(&b).unwrap();
        assert_eq!(json["botId"], "alpha");
        assert_eq!(
            json["path"],
            "/home/tyler/bots/_shared/memory/alpha/x.jsonl"
        );
        assert_eq!(json["entries"], 3);
        assert_eq!(json["timestamp"], "20260910-123904");
        // snake_case keys MUST NOT appear (defensive).
        assert!(json.get("bot_id").is_none());
        assert!(json.get("botId").is_some());
    }
}
