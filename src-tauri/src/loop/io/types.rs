//! Data types for the LoopIO contract.
//!
//! These types are the on-disk + IPC shapes. They live here so the
//! `FileLoopIO` impl can `use` them without naming sub-paths, and so
//! tests can construct fixtures without depending on the impl.

use std::io;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Lifecycle states for the current task. The supervisor advances
/// the state machine as it runs each turn.
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

/// The current task. `status` lives in the YAML frontmatter; `body`
/// is the free-form description (markdown). `updated_at` is the
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
    /// Build a fresh State. `started_at` becomes the initial
    /// `last_heartbeat` (no heartbeat has been emitted yet).
    pub fn new(
        pid: u32,
        started_at: impl Into<String> + Clone,
        run_id: impl Into<String>,
    ) -> Self {
        let started_at = started_at.into();
        Self {
            run_id: run_id.into(),
            turn: 0,
            pid,
            started_at: started_at.clone(),
            last_heartbeat: started_at,
            last_action_id: None,
        }
    }
}

/// One turn's worth of actions + errors + optional note. Appended
/// to today's journal file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalEntry {
    pub turn: u64,
    pub at: String,
    pub actions: Vec<String>,
    pub errors: Vec<String>,
    pub note: Option<String>,
}

/// Lock acquisition errors. The `release_lock` method returns the
/// more general `LoopError` because it can fail in non-lock ways
/// (e.g. permission denied when trying to remove a file owned by
/// another process).
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
}

/// The trait the supervisor (Sub-slice C) consumes. Thin on
/// purpose — every method maps 1:1 to a contract file. The
/// concrete impl (`FileLoopIO`) does the file work; a future
/// `MemoryLoopIO` or `NetworkLoopIO` could swap in transparently.
pub trait LoopIO: Send + Sync {
    /// Acquire the LOCK. Idempotent if the same process calls it
    /// twice. Fails if another live pid owns it.
    fn acquire_lock(&self) -> Result<(), LockError>;

    /// Release the LOCK. No-op if not held. Fails if held by a
    /// different pid (refuses to delete a foreign lock).
    fn release_lock(&self) -> Result<(), LoopError>;

    fn read_task(&self) -> Result<Task, LoopError>;
    fn write_task(&self, task: &Task) -> Result<(), LoopError>;

    /// Convenience: read-modify-write the status field. Preserves
    /// the body. Updates `updated_at` to now.
    fn update_task_status(&self, status: TaskStatus) -> Result<(), LoopError>;

    fn read_memory(&self) -> Result<Memory, LoopError>;
    fn append_memory(&self, facts: &[Fact]) -> Result<(), LoopError>;
    fn compact_memory(&self) -> Result<(), LoopError>;

    fn read_state(&self) -> Result<State, LoopError>;
    fn write_state(&self, state: &State) -> Result<(), LoopError>;

    fn append_journal(&self, entry: &JournalEntry) -> Result<(), LoopError>;
}
