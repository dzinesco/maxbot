//! v2.6.0 — Approval flows.
//!
//! Per-Bot per-tool rule (`auto` / `ask` / `deny`) and a
//! queue of pending approvals. The executor asks the
//! queue before calling `tool_registry.execute(...)`:
//!
//! 1. Rule is `auto` → run the tool, return the result.
//! 2. Rule is `ask` → enqueue, return a
//!    `"approval pending (id=...)"` string to the LLM so
//!    the model knows the call is gated. The user
//!    decides later; the Bot's reasoning loop continues.
//! 3. Rule is `deny` → return an error to the LLM
//!    (the tool is forbidden for this Bot).
//!
//! ## Auto-resume (out of scope for v2.6)
//!
//! When the user Approves / Rejects / Edits, the queue
//! runs the underlying tool and writes the result to
//! `approvals.result_json`. The Bot does NOT auto-resume
//! — the user manually sends a follow-up message if
//! they want the loop to continue. Wiring the
//! "approval-decided → bot resumes" feedback path is
//! v2.6.1.

pub mod queue;
pub mod store;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The three states a per-Bot per-tool rule can be in.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Rule {
    /// Run the tool without asking.
    Auto,
    /// Enqueue an approval; the user decides.
    Ask,
    /// Refuse the tool call (returns an error to the LLM).
    Deny,
}

impl Rule {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ask => "ask",
            Self::Deny => "deny",
        }
    }

    /// Parse a string into a Rule. Unknown values fall
    /// back to `Auto` (same default as `get_rule` for
    /// missing rows). Centralized so the storage layer
    /// and the migration-seeded defaults agree.
    pub fn parse(s: &str) -> Self {
        match s {
            "ask" => Self::Ask,
            "deny" => Self::Deny,
            _ => Self::Auto,
        }
    }
}

/// One row of `approval_rules`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalRule {
    pub bot_id: String,
    pub tool_name: String,
    pub rule: Rule,
}

/// One row of `approvals`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Approval {
    pub id: String,
    pub bot_id: String,
    pub tool_name: String,
    /// `"pending" | "approved" | "rejected" | "edited"`.
    pub status: String,
    pub payload: Value,
    /// The tool's result string. `None` until decided.
    pub result: Option<Value>,
    pub bot_run_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}
