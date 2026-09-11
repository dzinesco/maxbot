//! Sub-slice B — File I/O for the keep-alive loop.
//!
//! This module defines the thin trait the supervisor (Sub-slice C)
//! consumes plus a concrete `FileLoopIO` implementation that reads
//! and writes the four contract files plus the lock:
//!
//! ```text
//! <root>/
//! ├── TASK.md            — markdown + YAML frontmatter
//! ├── MEMORY.md          — markdown, durable + recent + summary
//! ├── STATE.json         — machine state (JSON)
//! ├── LOCK               — single-line "pid=<n>" + acquired_at
//! └── journal/
//!     └── YYYY-MM-DD.md  — per-day journal (UTC rollover)
//! ```
//!
//! ## File formats
//!
//! ### TASK.md
//!
//! ```markdown
//! ---
//! status: idle|running|blocked|done
//! updated_at: 2026-09-11T10:00:00Z
//! ---
//!
//! <free-form task description, markdown>
//! ```
//!
//! The frontmatter `status` is the only field the I/O layer cares
//! about; `updated_at` is informational. The body is whatever the
//! supervisor writes.
//!
//! ### MEMORY.md
//!
//! ```markdown
//! # MEMORY
//!
//! ## Durable
//!
//! - 2026-09-11T10:00:00Z [fact] user_name = Tyler #durable
//! - 2026-09-11T10:01:00Z [preference] timezone = America/Denver #durable
//!
//! ## Recent
//!
//! - 2026-09-11T10:02:00Z [fact] current_task = check email
//!
//! ## Summary
//!
//! 12 older non-durable entries (compacted at 2026-09-11T11:00:00Z)
//! ```
//!
//! `Durable` facts survive compaction; `Recent` facts are condensed
//! into a single-line `Summary` once the threshold is exceeded.
//!
//! ### STATE.json
//!
//! Plain JSON object matching the [`State`] struct. Updated on every
//! turn (heartbeat); supervisor reads it on startup to detect an
//! unclean exit.
//!
//! ### journal/YYYY-MM-DD.md
//!
//! One file per UTC day. Each call to `append_journal` opens today's
//! file in append mode and writes a single `## Turn N @ <RFC3339>`
//! block followed by `### Actions` / `### Errors` / `### Note`
//! subsections. At UTC midnight the next append creates the next
//! day's file automatically (no rollover logic — we just compute the
//! date on every call).
//!
//! ### LOCK
//!
//! ```text
//! pid=12345
//! acquired_at=2026-09-11T10:00:00Z
//! ```
//!
//! Acquire semantics: write the file atomically. If a file exists
//! with a different pid, check `kill -0 <pid>` — if the pid is
//! alive, fail with `LockError::Held(pid)`; if dead, overwrite.
//!
//! ## Compaction rule (MEMORY.md)
//!
//! `append_memory` triggers a compaction when **either**:
//!
//! - The combined entry count (durable + recent) exceeds
//!   `compaction_entries` (production: **100**, tests: **5**).
//! - The serialized byte size exceeds `compaction_bytes`
//!   (production: **50 KiB**).
//!
//! Compaction action: keep every durable entry; keep the **most
//! recent half** of non-durable entries; condense the rest into a
//! single-line summary section (`## Summary`). The summary is
//! lossy by design — the goal is to bound the file size, not
//! preserve history.
//!
//! ## Defense in depth
//!
//! Every public I/O method calls `assert_safe(path)` first. If the
//! path's lowercased string contains `passphrase` or
//! `computer_passphrase`, the call fails with
//! `LoopError::ForbiddenPath`. This is the secondary guard; the
//! primary guard is the sandbox bind (Sub-slice D) which keeps the
//! passphrase DB on a path the I/O trait never names.

pub mod file_impl;
pub mod types;

pub use file_impl::{atomic_write, parse_lock, pid_alive, FileLoopIO};
pub use types::{
    Fact, FactKind, JournalEntry, LockError, LoopError, LoopIO, Memory, State, Task,
    TaskStatus,
};

/// Default compaction thresholds (production values).
/// Tests can override via `FileLoopIO::with_threshold`.
pub const DEFAULT_COMPACTION_BYTES: u64 = 50 * 1024;
pub const DEFAULT_COMPACTION_ENTRIES: usize = 100;

/// Defense-in-depth guard. Returns `Err(LoopError::ForbiddenPath)` if
/// the path's lowercased string contains `passphrase` or
/// `computer_passphrase`. Exposed at the module root so Sub-slice D
/// (sandbox bind) can use the same check on its own paths.
pub fn assert_safe(path: &std::path::Path) -> Result<(), LoopError> {
    let s = path.to_string_lossy().to_lowercase();
    if s.contains("passphrase") || s.contains("computer_passphrase") {
        return Err(LoopError::ForbiddenPath(path.display().to_string()));
    }
    Ok(())
}
