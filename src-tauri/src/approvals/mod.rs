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

pub mod defaults;
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

/// v3.4.0 (Phase 5) — Sentinel `tool_name` for a
/// Takeover approval. Takeover is not a real tool —
/// it's a request for the user to drive the Bot's VM
/// interactively (e.g. solve a 2FA prompt). The
/// underlying reason lives in `payload.needs_human`;
/// the triggering tool name (the tool whose return
/// carried `needs_human`) is in `payload.tool`.
///
/// We use a sentinel rather than e.g. `__takeover__`
/// vs. a NULL `tool_name` so the existing `NOT NULL`
/// constraint on the `approvals.tool_name` column is
/// satisfied and existing index code keeps working.
pub const APPROVAL_TOOL_TAKEOVER: &str = "__takeover__";

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
    /// The tool name. For a Takeover approval (see
    /// `APPROVAL_TOOL_TAKEOVER`) this is the
    /// sentinel value `"__takeover__"` and the
    /// `payload` carries the takeover request shape
    /// (`{"needs_human": "...", "tool": "..."}`).
    /// The ActivityFeed / ApprovalQueue render the
    /// payload's `needs_human` as the human-readable
    /// reason; the `tool` is the tool that triggered
    /// the takeover (e.g. `vm_browser_open`).
    pub tool_name: String,
    /// `"pending" | "approved" | "rejected" | "edited"`.
    /// Takeover approvals also flow through this state
    /// machine: `pending` (waiting on the human) →
    /// `approved` (user took over + handed back) or
    /// `rejected` (user skipped the takeover — the Bot
    /// resumes without intervention).
    pub status: String,
    pub payload: Value,
    /// The tool's result string. `None` until decided.
    pub result: Option<Value>,
    pub bot_run_id: Option<String>,
    /// v2.6.2 — The LLM-issued tool_call id (so the
    /// auto-resume can append a synthetic `role=tool`
    /// message that the LLM will match against its
    /// outstanding `tool_calls` block). `None` for
    /// pre-v2.6.2 rows or for tools that pre-date the
    /// `tc.id` plumbing.
    #[serde(default)]
    pub tool_call_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
    /// v3.4.0 (Phase 5) — "Why this asked" reason.
    /// A short, human-readable explanation of why
    /// this approval was queued. Populated when the
    /// approval is enqueued (not when it is decided),
    /// so a pending row in the queue already carries
    /// the reason. Rule-derived by default
    /// (e.g. "sending email — gated by `ask` rule for
    /// `mail_send`"); for Takeover approvals the
    /// reason is the LLM's self-reported `needs_human`
    /// string. `None` for rows enqueued before v3.4.0
    /// — the ActivityFeed / ApprovalQueue render the
    /// absence as a generic "approval required" copy
    /// rather than failing.
    #[serde(default)]
    pub reason: Option<String>,
}

/// v3.4.0 (Phase 5) — Per-Bot Takeover state. Tracks
/// whether a Bot is currently paused waiting for a
/// human to drive the VM interactively. The state
/// table is the source of truth across app restarts
/// (per the brief: "takeover state persists across app
/// close/reopen — daemon-driven runs surface the
/// takeover entry on next app open").
///
/// `state` is one of:
/// - `"running"` — normal. The Bot is iterating.
/// - `"needs_human"` — the LLM self-reported a
///   `needs_human` signal; an approval has been
///   enqueued and the Bot's run is paused.
/// - `"takeover"` — the user is actively driving
///   the VM (Computer panel open in `takeover` mode).
///
/// `approval_id` is the row that gates this state.
/// When the approval is decided (`approved` /
/// `rejected`), the state flips back to `running`
/// and the Bot's run is resumed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotTakeoverState {
    pub bot_id: String,
    pub state: String,
    pub approval_id: String,
    pub reason: String,
    pub triggering_tool: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
