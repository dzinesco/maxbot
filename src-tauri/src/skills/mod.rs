//! v2.2.0 — Skills.
//!
//! A Skill is a saved, named list of `(tool, args)` steps that the
//! user can invoke on demand against a Bot. Skills are stored as
//! JSON blobs in SQLite so a single Skill row is `git diff`-able
//! and grep-friendly — no DSL, no compiled artifact. The user can:
//!
//! 1. **Create** a Skill manually (paste JSON, "Import" a
//!    `.skill.json` file, or record one with `skill_record_*`).
//! 2. **Run** a Skill against a Bot, optionally with `inputs` that
//!    are substituted into each step's `args` at run time.
//! 3. **Run** a Skill from inside a Bot's tool loop, via the
//!    `run_skill` tool, so the LLM can stitch a Skill into a longer
//!    task.
//!
//! ## Architecture choice: "Skill is content, not code"
//!
//! The Skill schema is plain JSON. We deliberately do NOT ship a
//! Skill DSL or a sandboxed mini-language — the
//! `(tool, args)` tuple is the existing Tool Registry contract,
//! so a Skill is just a `Vec<(tool_name, args_json)>` plus
//! optional `output_var` substitution. This keeps Skills:
//!
//! - **Diffable**: every change is a `git diff` of a JSON blob.
//! - **Inspectable**: a human can read a Skill file and see exactly
//!   what tools will fire and with what args.
//! - **Forward-compatible**: a Skill written today still works
//!   tomorrow because the tool registry is the only contract —
//!   adding a new tool in v2.3 doesn't break any v2.2 Skill.
//! - **Tool-reuse-only**: Skills can only call tools the Bot is
//!   already allowed to use. The tool allowlist stays the source
//!   of truth for what a Bot can do.
//!
//! ## Recording
//!
//! The user can record a Skill by clicking "Record" in the UI:
//!
//! 1. `skill_record_start(bot_id)` spawns a Bot run with a
//!    `recording_id` and returns `{ recording_id, bot_run_id }`.
//! 2. The Bot does its thing. The executor (modified to accept a
//!    `recording_id`) pushes every tool call it dispatches into
//!    the in-process `RecorderState`.
//! 3. `skill_record_stop(recording_id)` drains the recorder into
//!    a candidate `Skill` with `name = ""` and `description = ""`
//!    — the user fills those in via the UI before `skill_create`.

pub mod executor;
pub mod recorder;
pub mod scheduler_e2e;
pub mod store;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// A saved, named, runnable procedure. The full shape is what the
/// React UI renders, what `skill_create` accepts, and what the
/// recorder builds from a Bot run. Stored as JSON blobs in
/// SQLite (no per-field column) so the entire shape round-trips
/// cleanly through the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub inputs: Vec<Param>,
    pub steps: Vec<Step>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Skill {
    /// Build a fresh empty Skill. The user fills in name and
    /// description; steps and inputs start empty.
    pub fn new_empty() -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            description: String::new(),
            inputs: Vec::new(),
            steps: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// One tool call inside a Skill. The recorder always produces
/// `output_var = None`; the user can edit the JSON to add
/// `output_var` to a step so the next step can reference its
/// output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// The tool's name in the ToolRegistry. Validated at run
    /// time — unknown tools fail the Skill run with a friendly
    /// error.
    pub tool: String,
    /// The args to pass to the tool. Shape depends on the tool
    /// (validated at run time by the tool itself).
    #[serde(default = "default_empty_object")]
    pub args: Value,
    /// If `Some(name)`, the tool's output (a string) is bound to
    /// `name` in the run's substitution context, and any
    /// `"{{name}}"` placeholder in a later step's `args` is
    /// replaced with the output before the tool is called.
    /// Most recorders leave this `None`; it's a power-user
    /// affordance for chaining outputs into inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_var: Option<String>,
}

fn default_empty_object() -> Value {
    Value::Object(Default::default())
}

/// A user-supplied input. The Skill's UI renders a small form
/// from this list before the run starts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Param {
    pub name: String,
    /// One of `"string"`, `"number"`, `"path"`, `"choice"`.
    /// The renderer uses this to pick the right form widget
    /// (text input, number input, file picker, dropdown).
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    /// Only meaningful for `kind = "choice"`. The choices the
    /// user can pick from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
}

/// A recorded run of a Skill. The DB row stores the
/// (`status`, `started_at`, `finished_at`, `result_summary`,
/// `inputs`) summary. The in-memory copy kept in
/// `AppState::skill_runs` also carries the `steps: Vec<RunStep>`
/// so the UI can render per-step progress while the run is
/// alive.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRun {
    pub id: String,
    pub skill_id: String,
    pub bot_id: String,
    /// Inputs as the user supplied them at run start. The UI
    /// re-shows them in the run history.
    pub inputs: Value,
    pub status: SkillRunStatus,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub result_summary: String,
    /// In-memory only — the DB row does not persist per-step
    /// status. The run is the live progress mirror; once the
    /// run finishes, the `steps` vec is dropped.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<RunStep>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillRunStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl SkillRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// One step's progress inside a live `SkillRun`. The recorder
/// has a parallel concept (`RecordedStep`); the difference is
/// that `RunStep` carries run-time fields (`status`,
/// `started_at`, `finished_at`, `output`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunStep {
    pub tool: String,
    pub args: Value,
    /// "running" | "succeeded" | "failed" | "skipped"
    pub status: String,
    pub output: Option<String>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// If this step's run substituted an `output_var` into a
    /// later step, the substitution is visible here for the
    /// "show me the wiring" UX. None for runs that didn't use
    /// `output_var`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_var: Option<String>,
}

/// One tool call captured by the `RecorderState`. Lives in the
/// recorder's HashMap only — never persisted to the DB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordedStep {
    pub tool: String,
    pub args: Value,
    pub output: String,
    pub is_error: bool,
}

/// v3.3.0 — Per-step trace entry inside a `SkillRunTrace`.
///
/// One row per "thing that happened" during a Skill run: a
/// tool dispatch, the LLM-side preparation, an error, or a
/// substitution that bound an `output_var`. The renderer
/// renders these as the Last-run timeline. The shape is
/// intentionally lossy: not every field is meaningful for
/// every kind of entry. For a `web_fetch` call we'd see:
/// `{role: "tool", tool_name: "web_fetch", tool_args:
/// {"url": "..."}, tool_result: "..."}`. For a
/// substitution we'd see `{role: "substitute", content:
/// "bound var1"}`. Keep the JSON friendly to the
/// renderer's `stringify-and-display` path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepTrace {
    /// One of `"tool"`, `"substitute"`, `"error"`. Free-form
    /// for forward-compat: the renderer falls back to a
    /// generic display for unknown roles.
    #[serde(default = "default_step_role")]
    pub role: String,
    /// Free-form text. For `role = "tool"`, the tool's
    /// output. For `role = "error"`, the error message.
    /// Empty for `role = "substitute"` (the relevant data
    /// is in `tool_name`).
    #[serde(default)]
    pub content: String,
    /// Tool name for `role = "tool"`. Variable name for
    /// `role = "substitute"`. Empty for `role = "error"`.
    #[serde(default)]
    pub tool_name: String,
    /// Tool args for `role = "tool"`. Empty otherwise.
    #[serde(default)]
    pub tool_args: Value,
    /// Tool result for `role = "tool"`. Empty otherwise.
    #[serde(default)]
    pub tool_result: String,
    /// When this step fired, as an RFC3339 string. The
    /// renderer's timeline view sorts by this.
    pub ts: DateTime<Utc>,
}

fn default_step_role() -> String {
    "tool".to_string()
}

/// v3.3.0 — Per-row Skill run trace. One row per run, with
/// the per-step output as a JSON-encoded array. The
/// `skill_run_traces` table is the durable side of the
/// Last-run view; the in-memory `RunStep` array on
/// `SkillRun` is the live progress mirror. They share
/// their `run_id` so a single run has both a summary
/// row in `skill_runs` and a trace row here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRunTrace {
    pub id: String,
    /// Linkage back to `skill_runs.id`. The renderer uses
    /// this for cross-referencing in the timeline view
    /// ("this trace corresponds to the run that finished
    /// at HH:MM:SS").
    pub run_id: String,
    pub skill_id: String,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub per_step: Vec<StepTrace>,
    pub success: bool,
    /// The user message or scheduled payload that started
    /// the run. For empty / scheduled runs the executor
    /// passes an empty string.
    pub trigger_input: String,
}

impl SkillRunTrace {
    /// Serialize the `per_step` field to a JSON string for
    /// the `per_step_output` TEXT column. Returns `"[]"`
    /// on serialization failure (which shouldn't happen for
    /// a Vec of `Serialize` values) so the trace row still
    /// gets written.
    pub fn per_step_output_json(&self) -> String {
        serde_json::to_string(&self.per_step).unwrap_or_else(|_| "[]".to_string())
    }
}
