//! On-disk + IPC data types for the keep-alive loop.
//!
//! These types are the contract between the supervisor (this module) and
//! the I/O layer (a separate sub-slice that owns `loop::io`). The
//! supervisor uses these types via the [`crate::loop::daemon::io::LoopIO`]
//! trait; the I/O sub-slice implements the trait with its own copy of
//! these types (or, in integration, replaces this file with their
//! canonical version).
//!
//! The shapes are deliberately close to what the file I/O sub-slice is
//! expected to define so the integration turn can swap one impl for the
//! other without changes to the supervisor's call sites.
//!
//! Slice 1 — keep-alive loop supervisor.

use std::io;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lifecycle states for the current task. The supervisor advances
/// this state machine on every turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Idle,
    Running,
    Blocked,
    Done,
}

impl TaskStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Blocked => "blocked",
            Self::Done => "done",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "idle" => Some(Self::Idle),
            "running" => Some(Self::Running),
            "blocked" => Some(Self::Blocked),
            "done" => Some(Self::Done),
            _ => None,
        }
    }
}

impl Default for TaskStatus {
    fn default() -> Self {
        Self::Idle
    }
}

/// The current task. `status` lives in the YAML frontmatter; `body` is
/// the free-form description (markdown). `updated_at` is the
/// last-write timestamp in RFC3339.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Task {
    pub status: TaskStatus,
    pub body: String,
    pub updated_at: String,
}

impl Default for Task {
    fn default() -> Self {
        Self {
            status: TaskStatus::Idle,
            body: String::new(),
            updated_at: String::new(),
        }
    }
}

/// Two flavors of memory entry — `Fact` is a piece of information
/// about the world; `Preference` is a directive about how to act.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FactKind {
    Fact,
    Preference,
}

impl FactKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Preference => "preference",
        }
    }
}

/// One memory entry. `durable: true` means "survive compaction
/// forever"; `durable: false` means "may be condensed into a summary
/// during compaction".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fact {
    pub kind: FactKind,
    pub key: String,
    pub value: String,
    pub durable: bool,
    pub timestamp: String,
}

/// The in-memory representation of MEMORY.md. Parsed on read,
/// serialized on write. `summary` is a single-line lossy digest of
/// older non-durable entries that were compacted away.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Memory {
    pub durable: Vec<Fact>,
    pub recent: Vec<Fact>,
    pub summary: Option<String>,
}

/// Machine state for the supervisor. Written on every turn so a
/// crashed/restarted supervisor can detect the previous run's last
/// `turn` + `last_action_id` and resume cleanly.
///
/// `started_at` is the RFC3339 timestamp when this run began;
/// `last_heartbeat` is updated every 5s during a turn. Both come from
/// the supervisor's wall clock and are informational — the
/// authoritative liveness signal is the LOCK file's pid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct State {
    pub run_id: String,
    pub turn: u64,
    pub pid: u32,
    pub started_at: String,
    pub last_heartbeat: String,
    pub last_action_id: Option<String>,
}

impl State {
    /// Construct a fresh `State` for a new supervisor run. `turn` is
    /// 0; `last_action_id` is `None` until the first turn writes one.
    pub fn new(pid: u32, started_at: impl Into<String>, run_id: impl Into<String>) -> Self {
        let started_at = started_at.into();
        Self {
            run_id: run_id.into(),
            turn: 0,
            pid,
            last_heartbeat: started_at.clone(),
            started_at,
            last_action_id: None,
        }
    }
}

/// One turn's worth of actions + errors + optional note. Appended to
/// today's journal file (`journal/YYYY-MM-DD.md`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalEntry {
    pub turn: u64,
    pub at: String,
    pub actions: Vec<String>,
    pub errors: Vec<String>,
    pub note: Option<String>,
}

/// Lock acquisition errors.
#[derive(Debug, Error)]
pub enum LockError {
    #[error("loop is locked by pid {0} (still alive)")]
    Held(u32),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("malformed LOCK file: {0}")]
    Malformed(String),
}

/// The general I/O error returned by every method except
/// `acquire_lock`.
#[derive(Debug, Error)]
pub enum LoopError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("serialize error: {0}")]
    Serialize(String),
    #[error("path is forbidden (passphrase/computer_passphrase): {0}")]
    ForbiddenPath(String),
    #[error("lock error: {0}")]
    Lock(#[from] LockError),
    #[error("model error: {0}")]
    Model(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_round_trip_strings() {
        for s in [
            TaskStatus::Idle,
            TaskStatus::Running,
            TaskStatus::Blocked,
            TaskStatus::Done,
        ] {
            assert_eq!(TaskStatus::from_str(s.as_str()), Some(s));
        }
        assert_eq!(TaskStatus::from_str("nonsense"), None);
        assert_eq!(TaskStatus::from_str("  running  "), Some(TaskStatus::Running));
    }

    #[test]
    fn state_default_helper_sets_pid_and_run_id() {
        let s = State::new(4242u32, "2026-09-11T10:00:00Z", "abc-123");
        assert_eq!(s.pid, 4242);
        assert_eq!(s.run_id, "abc-123");
        assert_eq!(s.turn, 0);
        assert_eq!(s.last_action_id, None);
        assert_eq!(s.started_at, "2026-09-11T10:00:00Z");
        assert_eq!(s.last_heartbeat, "2026-09-11T10:00:00Z");
    }

    #[test]
    fn task_default_is_idle_empty() {
        let t = Task::default();
        assert_eq!(t.status, TaskStatus::Idle);
        assert!(t.body.is_empty());
        assert!(t.updated_at.is_empty());
    }

    #[test]
    fn memory_serializes_with_all_sections() {
        let m = Memory {
            durable: vec![Fact {
                kind: FactKind::Fact,
                key: "k".into(),
                value: "v".into(),
                durable: true,
                timestamp: "now".into(),
            }],
            recent: vec![],
            summary: Some("compacted".into()),
        };
        let s = serde_json::to_string(&m).unwrap();
        let back: Memory = serde_json::from_str(&s).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn loop_error_wraps_lock_error() {
        let lock_err = LockError::Held(1234);
        let loop_err: LoopError = lock_err.into();
        assert!(matches!(loop_err, LoopError::Lock(LockError::Held(1234))));
    }
}
