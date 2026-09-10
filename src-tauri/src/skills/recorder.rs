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
//!
//! ## v3.2.0 — `vm_browser_open` skill primitive
//!
//! When the bot calls `vm_computer_use` with a script that
//! is exactly one `open_url(url)` call, the recorder
//! recognizes the pattern and stores the step as
//! `vm_browser_open(url)` instead. The skill's stored form
//! is cleaner (a single field vs a script string), the
//! replay path still works (the `vm_browser_open` tool is a
//! thin wrapper around `vm_computer_use`), and the LLM has
//! one less decision to make at replay time.
//!
//! The rewrite is opportunistic: a `vm_computer_use` step
//! with a script that contains anything more than a single
//! `open_url(...)` call is left untouched. We don't try to
//! be clever about it — "open URL X" is a useful skill
//! shape, "open URL X, then click, then type" is a richer
//! shape that should stay as a `vm_computer_use` step.

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
    ///
    /// v3.2.0 — the recorded step is opportunistically
    /// rewritten via `rewrite_step_for_skill` before
    /// appending. Today the only rewrite is
    /// `vm_computer_use` (single `open_url` script) →
    /// `vm_browser_open(url)`. A step the rewrite doesn't
    /// recognize is left as-is.
    pub fn record(&self, recording_id: Option<&str>, step: RecordedStep) {
        let Some(id) = recording_id else {
            return;
        };
        let rewritten = rewrite_step_for_skill(step);
        let mut inner = self.inner.lock().expect("recorder lock poisoned");
        if let Some(vec) = inner.get_mut(id) {
            vec.push(rewritten);
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

/// v3.2.0 — rewrite a captured step into a cleaner skill
/// primitive when one is recognized. Today the only
/// recognized pattern is:
///
///     vm_computer_use(script="open_url(\"<url>\")")
///         ↦
///     vm_browser_open(url="<url>")
///
/// The script must be EXACTLY one `open_url(...)` call
/// (trimmed, no other helper calls, no comments). A
/// multi-step script is left as `vm_computer_use` so the
/// replay path runs the full sequence.
///
/// The function is pure — same input, same output, no side
/// effects — so it's safe to call from the executor's hot
/// path (one match-and-clone per recorded step).
pub fn rewrite_step_for_skill(step: RecordedStep) -> RecordedStep {
    if step.tool != "vm_computer_use" {
        return step;
    }
    let Some(url) = extract_single_open_url(&step.args) else {
        return step;
    };
    RecordedStep {
        tool: "vm_browser_open".to_string(),
        args: serde_json::json!({ "url": url }),
        // The output of the original `vm_computer_use` call
        // (a base64 PNG) is dropped on the rewrite — the
        // skill spec shouldn't carry a stale screenshot
        // from the recording session. The replay path will
        // produce a fresh screenshot.
        output: String::new(),
        is_error: step.is_error,
    }
}

/// Pull a URL out of a `vm_computer_use` step's `script`
/// argument when the script is exactly one `open_url(...)`
/// call. Returns `None` for any other shape (multi-call
/// scripts, missing `script`, non-string `script`,
/// non-URL arg, etc).
fn extract_single_open_url(args: &Value) -> Option<String> {
    let script = args.get("script")?.as_str()?.trim();
    let line = match script.lines().filter(|l| !l.trim().is_empty() && !l.trim().starts_with('#')).count() {
        1 => script
            .lines()
            .map(|l| l.trim())
            .find(|l| !l.is_empty() && !l.starts_with('#'))?,
        _ => return None,
    };
    // Pull the helper name and the arg list, mirroring
    // `parse_script` in `vm_computer_use.rs`. The first
    // '(' marks the start of the args; the last ')' marks
    // the end. Anything between is the arg list.
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close != line.len() - 1 {
        return None;
    }
    let name = line[..open].trim();
    if name != "open_url" {
        return None;
    }
    let args_str = line[open + 1..close].trim();
    // Accept either a JSON-style quoted string or a bare
    // token. The bare form is what the LLM tends to write.
    if args_str.starts_with('"') && args_str.ends_with('"') && args_str.len() >= 2 {
        let inner = &args_str[1..args_str.len() - 1];
        Some(inner.to_string())
    } else {
        Some(args_str.to_string())
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

    // -- v3.2.0 — vm_browser_open rewrite -----------------

    #[test]
    fn rewrite_recognizes_single_open_url_script() {
        let step = RecordedStep {
            tool: "vm_computer_use".to_string(),
            args: serde_json::json!({
                "script": "open_url(\"https://example.com\")"
            }),
            output: "out".to_string(),
            is_error: false,
        };
        let out = rewrite_step_for_skill(step);
        assert_eq!(out.tool, "vm_browser_open");
        assert_eq!(out.args, serde_json::json!({ "url": "https://example.com" }));
        // The base64 PNG from the original call is dropped —
        // the skill spec shouldn't carry a stale screenshot.
        assert!(out.output.is_empty());
    }

    #[test]
    fn rewrite_accepts_bare_token_url() {
        // The LLM often writes `open_url(https://...)`
        // without quotes. The rewrite should still pick it
        // up.
        let step = RecordedStep {
            tool: "vm_computer_use".to_string(),
            args: serde_json::json!({
                "script": "open_url(https://example.com/path?q=1)"
            }),
            output: String::new(),
            is_error: false,
        };
        let out = rewrite_step_for_skill(step);
        assert_eq!(out.tool, "vm_browser_open");
        assert_eq!(out.args, serde_json::json!({ "url": "https://example.com/path?q=1" }));
    }

    #[test]
    fn rewrite_ignores_multi_call_scripts() {
        // A multi-step script (open + click + type) is a
        // richer primitive than `vm_browser_open`. Leave it
        // as `vm_computer_use` so the replay runs the full
        // sequence.
        let step = RecordedStep {
            tool: "vm_computer_use".to_string(),
            args: serde_json::json!({
                "script": "open_url(\"https://example.com\")\nclick_at(100, 200)"
            }),
            output: String::new(),
            is_error: false,
        };
        let out = rewrite_step_for_skill(step);
        assert_eq!(out.tool, "vm_computer_use");
    }

    #[test]
    fn rewrite_ignores_non_open_url_scripts() {
        // A `vm_computer_use` step that does NOT open a
        // URL is left as `vm_computer_use` — the
        // `vm_browser_open` primitive is URL-specific.
        let step = RecordedStep {
            tool: "vm_computer_use".to_string(),
            args: serde_json::json!({
                "script": "screenshot()"
            }),
            output: String::new(),
            is_error: false,
        };
        let out = rewrite_step_for_skill(step);
        assert_eq!(out.tool, "vm_computer_use");
    }

    #[test]
    fn rewrite_passes_through_non_vm_computer_use_steps() {
        // The bot calls many tools; the rewrite is
        // specific to `vm_computer_use` → `vm_browser_open`.
        // A `file_write` step is left untouched.
        let step = RecordedStep {
            tool: "file_write".to_string(),
            args: serde_json::json!({ "path": "/tmp/x" }),
            output: String::new(),
            is_error: false,
        };
        let out = rewrite_step_for_skill(step);
        assert_eq!(out.tool, "file_write");
    }

    #[test]
    fn record_rewrites_open_url_to_vm_browser_open() {
        // End-to-end: `recorder.record()` rewrites the
        // step before appending. The skill captured by
        // `stop()` reflects the rewrite.
        let r = RecorderState::new();
        let id = r.start();
        r.record(
            Some(&id),
            make_recorded_step(
                "vm_computer_use",
                serde_json::json!({ "script": "open_url(\"https://example.com\")" }),
                "stale-screenshot",
                false,
            ),
        );
        let skill = r.stop(&id).expect("session exists");
        assert_eq!(skill.steps.len(), 1);
        assert_eq!(skill.steps[0].tool, "vm_browser_open");
        assert_eq!(
            skill.steps[0].args,
            serde_json::json!({ "url": "https://example.com" })
        );
    }
}
