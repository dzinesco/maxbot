//! v2.2.0 — Skill recorder.
//!
//! The recorder is a process-local store of "what tools did
//! the Bot call" for a single recording session. The Bot
//! executor (in `bots/executor.rs`) is modified to accept an
//! `Option<String>` `recording_id`; when set, after every
//! `bot_registry.execute(...)` it pushes the result into
//! the recorder's HashMap.
//!
//! The lifecycle:
//!
//! 1. UI calls `skill_record_start(bot_id)` →
//!    `recorder.start(bot_id, bot_run_id)` allocates a new
//!    `recording_id`, registers an empty `Vec<RecordedStep>`,
//!    and kicks off `run_bot_once` with that recording_id.
//! 2. The executor's tool loop captures every dispatch
//!    via `recorder.record(recording_id, step)`.
//! 3. UI calls `skill_record_stop(recording_id)` →
//!    `recorder.stop(recording_id)` drains the
//!    `Vec<RecordedStep>` into a candidate `Skill` and
//!    returns it. The user fills in name/description in the
//!    React UI before `skill_create` persists the final
//!    version.
//!
//! The recorder is intentionally `Arc<Mutex<...>>` — it's
//! accessed from both the command thread (start/stop) and
//! the executor's task thread (record). `parking_lot` would
//! be marginally faster but std `Mutex` is fine for a
//! single-user app where one recording is active at a time.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use crate::skills::{RecordedStep, Skill};

/// Process-local recorder state. Owns the HashMap of active
/// recording sessions, keyed by `recording_id` (a UUID v4
/// generated at `start` time).
#[derive(Default)]
pub struct RecorderState {
    inner: Mutex<HashMap<String, Vec<RecordedStep>>>,
}

impl RecorderState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh recording session. Returns the new
    /// `recording_id`; the caller is expected to pass this to
    /// `run_bot_once` so the executor knows to push into it.
    pub fn start(&self) -> String {
        let id = Uuid::new_v4().to_string();
        let mut inner = self.inner.lock().expect("recorder lock poisoned");
        inner.insert(id.clone(), Vec::new());
        id
    }

    /// Append one captured tool call. Called by the executor
    /// after every `bot_registry.execute(...)` when
    /// `recording_id` is `Some`. A `None` id or a missing
    /// session are both no-ops — defensive against the
    /// executor running across a `skill_record_stop` that
    /// just drained the session.
    pub fn record(&self, recording_id: Option<&str>, step: RecordedStep) {
        let Some(id) = recording_id else {
            return;
        };
        let mut inner = self.inner.lock().expect("recorder lock poisoned");
        if let Some(vec) = inner.get_mut(id) {
            vec.push(step);
        }
    }

    /// Drain the session and build a candidate `Skill` from
    /// the captured tool calls. The candidate has
    /// `name = ""`, `description = ""`, and `inputs = []` —
    /// the user fills those in via the UI before saving.
    ///
    /// Returns `None` if the `recording_id` is unknown (the
    /// caller should treat that as a "recording session not
    /// found" error).
    pub fn stop(&self, recording_id: &str) -> Option<Skill> {
        let mut inner = self.inner.lock().expect("recorder lock poisoned");
        let steps = inner.remove(recording_id)?;
        let now = Utc::now();
        let skill_steps: Vec<crate::skills::Step> = steps
            .into_iter()
            .map(|r| crate::skills::Step {
                tool: r.tool,
                args: r.args,
                output_var: None,
            })
            .collect();
        Some(Skill {
            id: Uuid::new_v4().to_string(),
            name: String::new(),
            description: String::new(),
            inputs: Vec::new(),
            steps: skill_steps,
            created_at: now,
            updated_at: now,
        })
    }

    /// Discard a recording session without building a Skill.
    /// Used when the user cancels a recording in the UI.
    pub fn abort(&self, recording_id: &str) {
        let mut inner = self.inner.lock().expect("recorder lock poisoned");
        inner.remove(recording_id);
    }
}

/// Convenience alias matching the `AppState` field shape.
pub type SharedRecorderState = Arc<RecorderState>;

/// Helper used by tests + the executor hook: build a
/// `RecordedStep` from the raw pieces.
pub fn make_recorded_step(
    tool: impl Into<String>,
    args: Value,
    output: impl Into<String>,
    is_error: bool,
) -> RecordedStep {
    RecordedStep {
        tool: tool.into(),
        args,
        output: output.into(),
        is_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_then_record_appends_in_order() {
        let r = RecorderState::new();
        let id = r.start();
        r.record(Some(&id), make_recorded_step("a", serde_json::json!({}), "out-a", false));
        r.record(Some(&id), make_recorded_step("b", serde_json::json!({}), "out-b", false));
        let skill = r.stop(&id).expect("session exists");
        assert_eq!(skill.steps.len(), 2);
        assert_eq!(skill.steps[0].tool, "a");
        assert_eq!(skill.steps[1].tool, "b");
        // Candidate has empty name/description; the UI fills them in.
        assert!(skill.name.is_empty());
        assert!(skill.description.is_empty());
    }

    #[test]
    fn record_with_none_id_is_a_noop() {
        let r = RecorderState::new();
        // No session started; should silently drop.
        r.record(None, make_recorded_step("a", serde_json::json!({}), "out", false));
        // A nonexistent id is also a noop.
        r.record(Some("nope"), make_recorded_step("a", serde_json::json!({}), "out", false));
        // The empty map is still empty.
        assert!(r.stop("nope").is_none());
    }

    #[test]
    fn abort_discards_captured_steps_without_building_skill() {
        let r = RecorderState::new();
        let id = r.start();
        r.record(Some(&id), make_recorded_step("a", serde_json::json!({}), "out", false));
        r.abort(&id);
        // A second stop on the same id returns None — the
        // session is gone.
        assert!(r.stop(&id).is_none());
    }
}
