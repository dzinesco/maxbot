//! Sub-slice A — the keep-alive loop supervisor.

pub mod io;
pub mod model;
pub mod state;
pub mod supervisor;

pub use io::{assert_safe, assert_safe_str, FileLoopIO, LoopIO};
pub use model::{ModelCaller, ModelError, ModelOutput, NoopModelCaller};
pub use state::{
    Fact, FactKind, JournalEntry, LockError, LoopError, Memory, State, Task, TaskStatus,
};
pub use supervisor::{run_one_turn, run_supervisor, SupervisorConfig, TurnOutcome};
