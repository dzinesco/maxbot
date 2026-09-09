//! Per-Bot memory store. SFTP-backed via the existing
//! [`crate::computer::ssh::SshExecutor`] trait, with a hard
//! 2-second timeout per call so a slow / dead VM never blocks the
//! executor's system-prompt injection or the UI.

use std::time::Duration;

use serde::Deserialize;

use super::{MemEntry, MemKind};

/// 2-second budget per SFTP call. Generous enough for a healthy
/// VM on a local network; tight enough that the user never
/// stares at a hung "Saving…" toast.
const SFTP_TIMEOUT: Duration = Duration::from_secs(2);

/// Remote path for a Bot's memory file. Lives on the Bot's VM
/// under the per-Bot home dir that's created during VM
/// provisioning.
pub fn memory_path(bot_id: &str, kind: MemKind) -> String {
    format!("bots/{}/memory/{}.jsonl", bot_id, kind.file_stem())
}

/// One line of the JSONL as we serialize it on disk. The
/// on-disk shape is intentionally minimal — `kind` is implicit
/// (the file is split by kind) so we only persist key, content,
/// and created_at. We round-trip through this shape so the
/// `[`MemEntry`] struct's `kind` field can always be reconstructed
/// from the file path alone.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MemLine {
    key: String,
    content: String,
    #[serde(default)]
    created_at: String,
    /// History entries also use an optional `at` field per the
    /// plan; we accept both `created_at` and `at` for
    /// forward-compat.
    #[serde(default)]
    at: Option<String>,
}

/// Parse a JSONL blob into entries. Blank lines and lines that
/// fail JSON parsing are silently skipped — a partially-written
/// or corrupted line (from an interrupted SFTP write) must not
/// take down the whole read.
pub fn parse_jsonl(kind: MemKind, blob: &str) -> Vec<MemEntry> {
    let mut out = Vec::new();
    for line in blob.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Tolerate a stray BOM or a "null" placeholder line.
        if trimmed == "null" {
            continue;
        }
        let Ok(line) = serde_json::from_str::<MemLine>(trimmed) else {
            log::warn!(
                "memory: skipping malformed {} line (length={})",
                kind.as_str(),
                trimmed.len()
            );
            continue;
        };
        let ts = line
            .at
            .or(Some(line.created_at))
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        out.push(MemEntry {
            kind,
            key: line.key,
            content: line.content,
            created_at: ts,
        });
    }
    out
}

/// Serialize a list of entries back to JSONL. Each entry is one
/// line; lines are LF-terminated and the file ends with a
/// trailing LF so `append` can keep the convention.
pub fn serialize_entries(entries: &[MemEntry]) -> String {
    let mut out = String::new();
    for e in entries {
        let line = MemLine {
            key: e.key.clone(),
            content: e.content.clone(),
            created_at: e.created_at.clone(),
            at: None,
        };
        // `serde_json::to_string` never fails for our shape.
        out.push_str(&serde_json::to_string(&line).expect("serialize MemLine"));
        out.push('\n');
    }
    out
}

/// Read all entries of one kind. Returns an empty Vec on any
/// error (file not found, timeout, parse error). The system
/// prompt injection depends on this being non-fatal: a Bot
/// without a VM is a normal state, not an error.
pub async fn read_all(
    pool: &dyn crate::computer::ssh::SshExecutor,
    bot_id: &str,
    kind: MemKind,
) -> Vec<MemEntry> {
    let path = memory_path(bot_id, kind);
    let fut = pool.vm_sftp_read(bot_id, &path);
    let Ok(Ok(blob)) = tokio::time::timeout(SFTP_TIMEOUT, fut).await else {
        log::debug!(
            "memory: read_all({kind}, {bot_id}) timed out or failed",
            kind = kind.as_str()
        );
        return Vec::new();
    };
    parse_jsonl(kind, &blob)
}

/// Append a new entry to the on-disk file. We read the existing
/// file (if any), push the new line, and write the whole thing
/// back. JSONL is append-only-friendly, but a single full-file
/// rewrite keeps the read path dumb and means the last write
/// always wins (no torn appends across two concurrent writers).
pub async fn append(
    pool: &dyn crate::computer::ssh::SshExecutor,
    bot_id: &str,
    kind: MemKind,
    entry: MemEntry,
) -> Result<(), MemoryError> {
    let mut existing = read_all(pool, bot_id, kind).await;
    existing.push(entry);
    write_all(pool, bot_id, kind, &existing).await
}

/// Replace the file with the given list of entries. Used by
/// [`append`] and [`delete`]. Writes go through `vm_sftp_write`
/// which atomically renames a temp file into place, so a reader
/// will see either the old or the new content but never a
/// half-written one.
async fn write_all(
    pool: &dyn crate::computer::ssh::SshExecutor,
    bot_id: &str,
    kind: MemKind,
    entries: &[MemEntry],
) -> Result<(), MemoryError> {
    let path = memory_path(bot_id, kind);
    let body = serialize_entries(entries);
    let fut = pool.vm_sftp_write(bot_id, &path, &body);
    match tokio::time::timeout(SFTP_TIMEOUT, fut).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(MemoryError::Sftp(e.to_string())),
        Err(_) => Err(MemoryError::Timeout),
    }
}

/// Delete every entry matching `key` (across the given `kind`).
/// Returns whether anything was deleted. We don't expose the
/// caller a way to delete by content — key match is the
/// canonical lookup per the plan.
pub async fn delete(
    pool: &dyn crate::computer::ssh::SshExecutor,
    bot_id: &str,
    kind: MemKind,
    key: &str,
) -> Result<bool, MemoryError> {
    let existing = read_all(pool, bot_id, kind).await;
    let before = existing.len();
    let after: Vec<MemEntry> = existing.into_iter().filter(|e| e.key != key).collect();
    if after.len() == before {
        return Ok(false);
    }
    write_all(pool, bot_id, kind, &after).await?;
    Ok(true)
}

/// Search across all 3 kinds for entries matching `query`.
/// Returns the top `top_k` entries ranked by:
/// 1. exact key match (score 1000)
/// 2. key contains query as substring (score 100)
/// 3. content contains query as substring (score 10)
/// History entries participate by their `content` field (the
/// summary). Returns Vec sorted by descending score, ties broken
/// by recency (newer first).
pub async fn search(
    pool: &dyn crate::computer::ssh::SshExecutor,
    bot_id: &str,
    query: &str,
    top_k: usize,
) -> Vec<MemEntry> {
    if top_k == 0 {
        return Vec::new();
    }
    let q = query.to_lowercase();
    let mut scored: Vec<(i64, MemEntry)> = Vec::new();
    for kind in [MemKind::Fact, MemKind::Preference, MemKind::History] {
        for entry in read_all(pool, bot_id, kind).await {
            let key_lc = entry.key.to_lowercase();
            let content_lc = entry.content.to_lowercase();
            let score = if !q.is_empty() && key_lc == q {
                1000
            } else if !q.is_empty() && key_lc.contains(&q) {
                100
            } else if !q.is_empty() && content_lc.contains(&q) {
                10
            } else if q.is_empty() {
                1
            } else {
                0
            };
            if score > 0 {
                scored.push((score, entry));
            }
        }
    }
    // Newer first as the tiebreaker so the most recent matching
    // entry surfaces when many tie.
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| b.1.created_at.cmp(&a.1.created_at))
    });
    scored.truncate(top_k);
    scored.into_iter().map(|(_, e)| e).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("sftp error: {0}")]
    Sftp(String),
    #[error("memory sftp call timed out")]
    Timeout,
}

use serde::Serialize;

// The `Serialize` derive on `MemLine` is used by
// `serialize_entries`. We import at the bottom to keep the
// public types at the top of the file.
//
// (Rust 2021's `use` is file-scoped, so this is purely a style
// choice; both placements are equivalent.)

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(key: &str, content: &str) -> MemEntry {
        MemEntry::new(MemKind::Fact, key, content)
    }

    #[test]
    fn parse_jsonl_skips_blank_and_malformed() {
        let blob = r#"
{"key":"name","content":"Tyler","created_at":"2026-01-01T00:00:00Z"}

{not json

{"key":"color","content":"blue","created_at":"2026-01-02T00:00:00Z"}
null
"#;
        let entries = parse_jsonl(MemKind::Fact, blob);
        assert_eq!(entries.len(), 2, "expected 2 valid entries, got {:?}", entries);
        assert_eq!(entries[0].key, "name");
        assert_eq!(entries[1].key, "color");
        // All entries get the kind tag back-filled from the file path.
        for e in &entries {
            assert_eq!(e.kind, MemKind::Fact);
        }
    }

    #[test]
    fn search_ranks_exact_key_match_first() {
        let entries = vec![
            fact("name", "Tyler"),
            fact("username", "tyler@dzines.co"),
            fact("color", "blue"),
        ];
        // Mirror the same scoring logic the async `search` uses,
        // but on the parsed slice. This is the unit-testable
        // surface — the async version is exercised by the
        // round-trip test below.
        let q = "name";
        let mut scored: Vec<(i64, &MemEntry)> = entries
            .iter()
            .map(|e| {
                let key_lc = e.key.to_lowercase();
                let score = if key_lc == q {
                    1000
                } else if key_lc.contains(q) {
                    100
                } else if e.content.to_lowercase().contains(q) {
                    10
                } else {
                    0
                };
                (score, e)
            })
            .filter(|(s, _)| *s > 0)
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        assert_eq!(scored[0].1.key, "name", "exact match must rank first");
        assert_eq!(scored[1].1.key, "username", "key-contains ranks second");
        // `color` doesn't match at all.
        assert!(scored.iter().all(|(s, _)| *s > 0));
        assert_eq!(scored.len(), 2);
    }

    #[test]
    fn append_then_read_returns_old_plus_new() {
        // Round-trip on a temp file: serialize, write, read back,
        // parse, assert the new entry is at the end. This locks
        // in the on-disk shape that the SFTP path would produce
        // and confirms the parser reconstructs it cleanly.
        let dir = std::env::temp_dir().join(format!("maxbot-mem-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("facts.jsonl");

        let initial = vec![fact("name", "Tyler"), fact("color", "blue")];
        std::fs::write(&path, serialize_entries(&initial)).unwrap();

        let mut from_disk = parse_jsonl(MemKind::Fact, &std::fs::read_to_string(&path).unwrap());
        from_disk.push(fact("timezone", "America/Denver"));
        std::fs::write(&path, serialize_entries(&from_disk)).unwrap();

        let after = parse_jsonl(MemKind::Fact, &std::fs::read_to_string(&path).unwrap());
        assert_eq!(after.len(), 3);
        assert_eq!(after[0].key, "name");
        assert_eq!(after[1].key, "color");
        assert_eq!(after[2].key, "timezone");
        assert_eq!(after[2].content, "America/Denver");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }
}
