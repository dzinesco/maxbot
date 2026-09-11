//! Sub-slice A — the keep-alive loop supervisor.
//!
//! The supervisor consumes [`crate::loop::io::LoopIO`] for all filesystem
//! access. State types (Task, Memory, State, Fact, etc.) live in
//! [`crate::loop::io`] — this sub-slice does not own a second copy of the
//! contract types or the lock. One I/O implementation, one lock, one set
//! of state types.

pub mod model;
pub mod supervisor;

pub use model::{ModelCaller, ModelError, ModelOutput, NoopModelCaller};
pub use supervisor::{run_one_turn, run_supervisor, SupervisorConfig, TurnOutcome};

// Re-export the I/O contract types from `crate::loop::io` so existing
// callers of `crate::loop::daemon::Task`, `crate::loop::daemon::State`,
// etc. keep working without churn.
pub use crate::r#loop::io::{
    Fact, FactKind, FileLoopIO, JournalEntry, LockError, LoopError, LoopIO, Memory, State,
    Task, TaskStatus,
};
