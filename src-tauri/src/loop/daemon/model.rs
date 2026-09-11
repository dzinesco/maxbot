//! The model-calling trait the supervisor uses to run one turn.
//!
//! In production this will be wired to the real LLM client (the
//! integration turn wires the full pipeline). For Slice 1 — keep-alive
//! loop supervisor, we use [`NoopModelCaller`] as a placeholder that
//! simply marks the task `Done` after one "action". Tests use the noop
//! to verify the supervisor's state-machine + lock + I/O contract
//! without depending on the LLM stack.
//!
//! ## Why a trait?
//!
//! The supervisor's contract is "given a task and a memory, produce a
//! turn outcome". The LLM side can swap implementations — a noop for
//! tests, a real model call for production, a replay log for
//! debugging — without touching the supervisor's call sites.
//!
//! Slice 1 — keep-alive loop supervisor.

use std::fmt;

use super::state::{Fact, Memory, Task, TaskStatus};

/// What the model produces for one turn. The supervisor translates
/// this into I/O calls (journal append, memory append, task status
/// update).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelOutput {
    /// Human-readable actions taken this turn. Each becomes a bullet
    /// under `### Actions` in today's journal file.
    pub actions: Vec<String>,
    /// New facts to remember. The supervisor appends them via
    /// `LoopIO::append_memory`, which triggers compaction if needed.
    pub facts: Vec<Fact>,
    /// What status to set TASK.md to after this turn. The supervisor
    /// moves `Running` → `Done` on success, `Running` → `Blocked`
    /// when the model needs more input, and leaves `Idle`/`Done`
    /// unchanged.
    pub new_status: TaskStatus,
}

#[derive(Debug)]
pub enum ModelError {
    /// The model client failed. The supervisor records the error in
    /// the journal and keeps the task `Running` so the next
    /// supervisor invocation can retry.
    Backend(String),
}

impl fmt::Display for ModelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(s) => write!(f, "model backend error: {s}"),
        }
    }
}

impl std::error::Error for ModelError {}

/// The trait the supervisor calls once per turn.
pub trait ModelCaller: Send + Sync {
    fn call(&self, task: &Task, memory: &Memory) -> Result<ModelOutput, ModelError>;
}

/// A placeholder that marks the task done and emits a single
/// `noop` action. Tests use this; the integration turn replaces it
/// with a real model client.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopModelCaller;

impl ModelCaller for NoopModelCaller {
    fn call(&self, task: &Task, _memory: &Memory) -> Result<ModelOutput, ModelError> {
        let new_status = match task.status {
            TaskStatus::Running => TaskStatus::Done,
            // Blocked stays Blocked — the model didn't make progress.
            // Idle and Done stay as they are; the supervisor's call
            // site only invokes us when status is Running or Blocked.
            other => other,
        };
        Ok(ModelOutput {
            actions: vec!["noop".to_string()],
            facts: Vec::new(),
            new_status,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_moves_running_to_done() {
        let task = Task {
            status: TaskStatus::Running,
            body: "do it".into(),
            updated_at: String::new(),
        };
        let out = NoopModelCaller.call(&task, &Memory::default()).unwrap();
        assert_eq!(out.new_status, TaskStatus::Done);
        assert_eq!(out.actions, vec!["noop".to_string()]);
        assert!(out.facts.is_empty());
    }

    #[test]
    fn noop_keeps_blocked_blocked() {
        let task = Task {
            status: TaskStatus::Blocked,
            body: "needs input".into(),
            updated_at: String::new(),
        };
        let out = NoopModelCaller.call(&task, &Memory::default()).unwrap();
        assert_eq!(out.new_status, TaskStatus::Blocked);
    }

    #[test]
    fn noop_keeps_done_done() {
        let task = Task {
            status: TaskStatus::Done,
            body: "finished".into(),
            updated_at: String::new(),
        };
        let out = NoopModelCaller.call(&task, &Memory::default()).unwrap();
        assert_eq!(out.new_status, TaskStatus::Done);
    }
}
