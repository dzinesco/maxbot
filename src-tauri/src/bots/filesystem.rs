//! Per-bot filesystem: gives each bot its own directory at
//! `<app_data_dir>/bots/<id>/` containing `agents.md` (long-term
//! memory), `scratchpad.md` (working notes), and `outputs/` (artifacts
//! the bot produces).
//!
//! The directory is created on first access; existing bots get a
//! directory on next run. `agents.md` is auto-seeded from the bot's
//! `system_prompt` SQLite column on first read if absent.
//!
//! The id is validated as a UUID v4 string before any path joining —
//! `Bot::new_id()` is already UUID-shaped, so this is defense in depth
//! against a stray foreign key or hand-edited DB row.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tauri::Manager;

/// Returns the bot's directory, creating `bots/<id>/` and
/// `bots/<id>/outputs/` if missing. The data dir is resolved via
/// Tauri's `app.path().app_data_dir()` so it matches the SQLite path
/// (which is set the same way in `lib.rs`).
pub fn bot_dir<R: tauri::Runtime>(app: &tauri::AppHandle<R>, bot_id: &str) -> Result<PathBuf, String> {
    validate_id(bot_id)?;
    let root = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("could not resolve app data dir: {e}"))?;
    let dir = root.join("bots").join(bot_id);
    fs::create_dir_all(dir.join("outputs"))
        .map_err(|e| format!("could not create bot dir {}: {e}", dir.display()))?;
    Ok(dir)
}

/// Resolved `agents.md` path for the given bot. Read via
/// `read_agents_md` rather than calling this directly when you need
/// the file contents; the read helper auto-seeds.
pub fn agents_md_path(dir: &Path) -> PathBuf {
    dir.join("agents.md")
}

pub fn scratchpad_path(dir: &Path) -> PathBuf {
    dir.join("scratchpad.md")
}

pub fn outputs_dir(dir: &Path) -> PathBuf {
    dir.join("outputs")
}

/// Read the bot's `agents.md`. If the file doesn't exist, create it
/// from the supplied `fallback` (typically the SQLite `system_prompt`)
/// and return that. The auto-seed is a one-time event per bot — once
/// the file exists, the fallback is ignored.
pub fn read_agents_md(dir: &Path, fallback: &str) -> Result<String, String> {
    let path = agents_md_path(dir);
    if path.exists() {
        return fs::read_to_string(&path)
            .map_err(|e| format!("could not read agents.md: {e}"));
    }
    // First-time seed. Write the fallback so the next call (and the
    // next bot run) sees consistent content. Use atomic write so a
    // mid-write crash doesn't leave a half-formed file.
    atomic_write(&path, fallback.as_bytes())?;
    Ok(fallback.to_string())
}

/// Overwrite `agents.md` atomically (write to .tmp, rename). Used by
/// the `memory_write` tool.
pub fn write_agents_md(dir: &Path, contents: &str) -> Result<(), String> {
    atomic_write(&agents_md_path(dir), contents.as_bytes())
}

/// Append `append` to `agents.md` with a blank-line separator. Used
/// by the `memory_append` tool.
pub fn append_agents_md(dir: &Path, append: &str) -> Result<(), String> {
    let path = agents_md_path(dir);
    let mut current = if path.exists() {
        fs::read_to_string(&path).map_err(|e| format!("read agents.md: {e}"))?
    } else {
        String::new()
    };
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    if !current.is_empty() {
        current.push('\n');
    }
    current.push_str(append);
    atomic_write(&path, current.as_bytes())
}

pub fn read_scratchpad(dir: &Path) -> Result<String, String> {
    let path = scratchpad_path(dir);
    if !path.exists() {
        return Ok(String::new());
    }
    fs::read_to_string(&path).map_err(|e| format!("read scratchpad: {e}"))
}

pub fn write_scratchpad(dir: &Path, contents: &str) -> Result<(), String> {
    atomic_write(&scratchpad_path(dir), contents.as_bytes())
}

pub fn append_scratchpad(dir: &Path, append: &str) -> Result<(), String> {
    let path = scratchpad_path(dir);
    let mut current = if path.exists() {
        fs::read_to_string(&path).map_err(|e| format!("read scratchpad: {e}"))?
    } else {
        String::new()
    };
    if !current.is_empty() && !current.ends_with('\n') {
        current.push('\n');
    }
    if !current.is_empty() {
        current.push('\n');
    }
    current.push_str(append);
    atomic_write(&path, current.as_bytes())
}

pub fn list_outputs(dir: &Path) -> Result<Vec<(String, u64)>, String> {
    let out_dir = outputs_dir(dir);
    if !out_dir.exists() {
        return Ok(Vec::new());
    }
    let mut out: Vec<(String, u64)> = Vec::new();
    for entry in fs::read_dir(&out_dir).map_err(|e| format!("read outputs dir: {e}"))? {
        let entry = entry.map_err(|e| format!("dir entry: {e}"))?;
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        out.push((name, size));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// Write a file to `outputs/<filename>`. The filename is sanitized
/// to its basename so `..` and `/` can't escape the outputs dir.
pub fn write_output(dir: &Path, filename: &str, contents: &[u8]) -> Result<PathBuf, String> {
    let clean = sanitize_filename(filename)?;
    let path = outputs_dir(dir).join(&clean);
    atomic_write(&path, contents)?;
    Ok(path)
}

pub fn read_output(dir: &Path, filename: &str) -> Result<Vec<u8>, String> {
    let clean = sanitize_filename(filename)?;
    let path = outputs_dir(dir).join(&clean);
    fs::read(&path).map_err(|e| format!("read output: {e}"))
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("bot id is empty".to_string());
    }
    // Use a tiny in-line regex via the `regex` crate? Avoid the
    // dependency by checking length + hex/dash characters.
    if id.len() != 36 {
        return Err(format!("bot id '{id}' is not a UUID (length {})", id.len()));
    }
    let bytes = id.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        let ok = match i {
            8 | 13 | 18 | 23 => *b == b'-',
            _ => b.is_ascii_hexdigit(),
        };
        if !ok {
            return Err(format!(
                "bot id '{id}' is not a UUID (invalid character at position {i})"
            ));
        }
    }
    Ok(())
}

/// Strip the filename to its last path component, refuse empty
/// results, and reject names that start with `.` (hidden files). This
/// is intentionally stricter than a full POSIX basename because we
/// don't want to depend on the `std::path::Path::file_name` quirks
/// across platforms and we want a single source of truth.
fn sanitize_filename(raw: &str) -> Result<String, String> {
    if raw.is_empty() {
        return Err("filename is empty".to_string());
    }
    // Reject separators outright. This is stronger than a basename
    // pass and blocks every path-traversal vector (including
    // backslashes, NUL, etc. — we accept a tiny whitelist of chars).
    for b in raw.bytes() {
        let ok = b.is_ascii_alphanumeric()
            || matches!(b, b'.' | b'-' | b'_' | b' ' | b'(' | b')' | b'!' | b',' | b'[' | b']');
        if !ok {
            return Err(format!(
                "filename '{raw}' contains illegal byte {b:02x}; \
                 allowed: alphanumerics, '.', '-', '_', ' ', '()!,[]'"
            ));
        }
    }
    // Use Path::new to get the last component anyway, in case a
    // weird client passes something we missed.
    let leaf = Path::new(raw)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(raw);
    let leaf = leaf.to_string();
    if leaf.starts_with('.') {
        return Err(format!("filename '{raw}' is hidden (starts with '.')"));
    }
    if leaf.len() > 200 {
        return Err(format!("filename '{raw}' is too long ({} chars)", leaf.len()));
    }
    Ok(leaf)
}

/// Write atomically: `path.tmp` first, then rename onto `path`.
/// `fs::rename` is atomic on the same filesystem, which is always
/// the case for our use (bot dir on the user's data volume).
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)
            .map_err(|e| format!("create {}: {e}", tmp.display()))?;
        f.write_all(bytes).map_err(|e| format!("write: {e}"))?;
        f.sync_all().map_err(|e| format!("sync: {e}"))?;
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch dir for tests. Each test gets a unique subdir so
    /// tests can run in parallel without stomping on each other. The
    /// counter is process-local (cargo test runs multiple test
    /// threads within the same process) and atomically incremented.
    fn scratch_dir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join("maxbot-bot-fs-tests");
        let unique = format!("{}-{}-{}", std::process::id(), n, std::env::temp_dir().display());
        let p = base.join(unique);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn validate_id_accepts_uuid_v4_shape() {
        let id = "550e8400-e29b-41d4-a716-446655440000";
        assert!(validate_id(id).is_ok(), "{id} should be valid");
    }

    #[test]
    fn validate_id_rejects_empty() {
        assert!(validate_id("").is_err());
    }

    #[test]
    fn validate_id_rejects_non_uuid() {
        assert!(validate_id("not-a-uuid").is_err());
        assert!(validate_id("../etc").is_err());
        assert!(validate_id("a/../../b").is_err());
    }

    #[test]
    fn validate_id_rejects_short_uuids() {
        // Right format but too short.
        assert!(validate_id("550e8400-e29b-41d4-a716").is_err());
    }

    #[test]
    fn read_agents_md_seeds_from_fallback_when_missing() {
        let dir = scratch_dir();
        let s = read_agents_md(&dir, "hello world").expect("read");
        assert_eq!(s, "hello world");
        // Now the file exists; next call returns the same content
        // (we don't re-seed).
        let s2 = read_agents_md(&dir, "different fallback").expect("read");
        assert_eq!(s2, "hello world");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_agents_md_returns_file_when_present() {
        let dir = scratch_dir();
        write_agents_md(&dir, "manual content").expect("write");
        let s = read_agents_md(&dir, "fallback ignored").expect("read");
        assert_eq!(s, "manual content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_agents_md_round_trips() {
        let dir = scratch_dir();
        write_agents_md(&dir, "first").expect("write");
        write_agents_md(&dir, "second").expect("write");
        let s = read_agents_md(&dir, "fallback").expect("read");
        assert_eq!(s, "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_agents_md_joins_with_separator() {
        let dir = scratch_dir();
        append_agents_md(&dir, "a").expect("append");
        append_agents_md(&dir, "b").expect("append");
        let s = read_agents_md(&dir, "fallback").expect("read");
        // The pattern: previous content, optional newline if missing,
        // blank-line separator, then the append.
        assert!(s.contains("a\n\nb"), "got: {s:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn scratchpad_round_trip() {
        let dir = scratch_dir();
        assert_eq!(read_scratchpad(&dir).unwrap(), "");
        write_scratchpad(&dir, "x").unwrap();
        assert_eq!(read_scratchpad(&dir).unwrap(), "x");
        append_scratchpad(&dir, "y").unwrap();
        assert!(read_scratchpad(&dir).unwrap().contains("x\n\ny"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outputs_list_empty_when_dir_missing() {
        let dir = scratch_dir();
        // No outputs/ yet.
        assert!(!dir.join("outputs").exists());
        let entries = list_outputs(&dir).expect("list");
        assert!(entries.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn outputs_write_sanitizes_filename() {
        let dir = scratch_dir();
        // ../../../etc/passwd is rejected.
        let err = write_output(&dir, "../../../etc/passwd", b"x");
        assert!(err.is_err(), "expected traversal rejection, got: {err:?}");
        // report.md is fine.
        let p = write_output(&dir, "report.md", b"hi").expect("write");
        assert!(p.exists());
        let bytes = read_output(&dir, "report.md").expect("read");
        assert_eq!(bytes, b"hi");
        // Hidden files rejected.
        let err = write_output(&dir, ".env", b"x");
        assert!(err.is_err());
        // Empty rejected.
        let err = write_output(&dir, "", b"x");
        assert!(err.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_write_replaces_existing() {
        let dir = scratch_dir();
        write_agents_md(&dir, "first").expect("write");
        write_agents_md(&dir, "second").expect("write");
        // No .tmp file should be left around.
        let tmp = dir.join("agents.md.tmp");
        assert!(!tmp.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sanitize_filename_accepts_typical_names() {
        for name in [
            "report.md",
            "data_2025.csv",
            "Brief (1).docx",
            "fig-1.png",
            "name with spaces.txt",
        ] {
            let clean = sanitize_filename(name).expect(name);
            assert_eq!(clean, name);
        }
    }

    #[test]
    fn sanitize_filename_rejects_separators() {
        for bad in ["a/b", "a\\b", "../x", "..\\x", "/etc/passwd"] {
            assert!(sanitize_filename(bad).is_err(), "{bad} should fail");
        }
    }
}
