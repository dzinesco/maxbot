//! v2.2.0 — Skill execution.
//!
//! `run_skill` walks a Skill's `steps` sequentially, calling
//! each step's tool via the shared `ToolRegistry` and threading
//! the previous step's output into the next step's `args` via
//! `"{{var}}"` substitution (where `var` is the previous
//! step's `output_var`).
//!
//! Halts on the first error. The run row in
//! `AppState::skill_runs` is updated to `failed` with the
//! failing step highlighted; subsequent steps get a
//! `"skipped"` `RunStep` for the UI.
//!
//! `output_var` substitution: each step's `args` (a JSON
//! `Value`) is walked recursively. Any string value that
//! equals `"{{var_name}}"` is replaced with the captured
//! output. We deliberately do not do partial-string
//! substitution — the placeholder is the entire string — so
//! the user can't accidentally substitute into a URL or a
//! SQL fragment they didn't intend.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde_json::{json, Value};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

use crate::skills::{RunStep, Skill, SkillRun, SkillRunStatus};
use crate::tools::registry::ToolRegistry;
use crate::tools::tool::{ToolContext, ToolInvocation};
use crate::AppState;

/// Run a Skill against a Bot. Returns a fully-populated
/// `SkillRun` (the in-memory copy, with per-step status).
/// The caller is expected to have inserted the row in
/// `skill_runs` *before* calling this (so a crash mid-run
/// still leaves a "running" row to clean up later) and to
/// `update_skill_run` once this returns.
///
/// We build a fresh `ToolRegistry` per call (same pattern
/// `run_bot_once` uses) — there's no per-Bot filter applied,
/// because the user is invoking the Skill directly. The
/// Skill runner is autonomous: it does not pop a consent
/// dialog (the user already opted in by running the Skill).
/// The Bot's `allowed_tools` allowlist is the chat-time
/// guardrail; the Skill runner is a user-initiated batch.
pub async fn run_skill(
    app: AppHandle,
    state: Arc<AppState>,
    skill_run_id: String,
    skill: Skill,
    bot_id: String,
    user_inputs: Value,
    cancel: CancellationToken,
) -> SkillRun {
    let now = Utc::now();
    let mut run = SkillRun {
        id: skill_run_id,
        skill_id: skill.id.clone(),
        bot_id: bot_id.clone(),
        inputs: user_inputs,
        status: SkillRunStatus::Running,
        started_at: now,
        finished_at: None,
        result_summary: String::new(),
        steps: Vec::new(),
    };

    // Build the registry once per call; same shape the
    // chat loop and the Bot executor use.
    let registry = ToolRegistry::default_with_extras(state.mcp.tool_adapters());

    // Substitution context: a flat map of `output_var` ->
    // captured tool output. Filled as we go; read by
    // `substitute_args` before each tool call.
    let mut bindings: HashMap<String, String> = HashMap::new();

    for step in &skill.steps {
        if cancel.is_cancelled() {
            // Mark the current step as skipped; previous ones
            // already have their final status.
            let skipped = RunStep {
                tool: step.tool.clone(),
                args: step.args.clone(),
                status: "skipped".to_string(),
                output: None,
                started_at: Utc::now(),
                finished_at: Some(Utc::now()),
                output_var: step.output_var.clone(),
            };
            run.steps.push(skipped);
            run.status = SkillRunStatus::Cancelled;
            run.finished_at = Some(Utc::now());
            run.result_summary = "cancelled".to_string();
            return run;
        }
        let started_at = Utc::now();
        let mut run_step = RunStep {
            tool: step.tool.clone(),
            args: step.args.clone(),
            status: "running".to_string(),
            output: None,
            started_at,
            finished_at: None,
            output_var: step.output_var.clone(),
        };

        // Resolve the args. If substitution references a
        // missing binding, fail the run with a friendly
        // message — much easier to debug than a tool
        // crashing on a literal `"{{name}}"` string.
        let resolved_args = match substitute_args(&step.args, &bindings) {
            Ok(v) => v,
            Err(e) => {
                run_step.status = "failed".to_string();
                run_step.output = Some(e.clone());
                run_step.finished_at = Some(Utc::now());
                run.steps.push(run_step);
                run.status = SkillRunStatus::Failed;
                run.finished_at = Some(Utc::now());
                run.result_summary = e;
                return run;
            }
        };
        run_step.args = resolved_args.clone();

        let tool_name = step.tool.clone();
        let tool_invocation = ToolInvocation {
            name: tool_name.clone(),
            id: format!("skill-run-{}-{}", run.id, run.steps.len()),
            arguments: resolved_args,
            bot_id: Some(bot_id.clone()),
        };
        let tool_context = ToolContext {
            consent_granted: true,
            consent_prompt: None,
            app: Some(app.clone()),
        };

        let (output, is_error) = match registry.execute(tool_invocation, tool_context).await {
            Ok(r) => (r.content, r.is_error),
            Err(e) => (format!("[error] {e}"), true),
        };

        run_step.finished_at = Some(Utc::now());
        run_step.output = Some(output.clone());
        if is_error {
            run_step.status = "failed".to_string();
            run.steps.push(run_step);
            run.status = SkillRunStatus::Failed;
            run.finished_at = Some(Utc::now());
            run.result_summary = output;
            return run;
        } else {
            run_step.status = "succeeded".to_string();
        }

        // Bind the output for the next step's `output_var`
        // reference.
        if let Some(var) = &step.output_var {
            bindings.insert(var.clone(), output);
        }

        run.steps.push(run_step);
    }

    run.status = SkillRunStatus::Succeeded;
    run.finished_at = Some(Utc::now());
    run.result_summary = format!("{} step(s) succeeded", run.steps.len());
    run
}

/// Walk a JSON `Value` and replace every string
/// `"{{var_name}}"` with the corresponding binding. Non-string
/// values are passed through unchanged. We don't substitute
/// into a partially-matching string — the entire string must
/// be the placeholder.
pub fn substitute_args(
    args: &Value,
    bindings: &HashMap<String, String>,
) -> Result<Value, String> {
    match args {
        Value::String(s) => {
            if let Some(var) = extract_placeholder(s) {
                match bindings.get(&var) {
                    Some(v) => Ok(Value::String(v.clone())),
                    None => Err(format!(
                        "step references {{{{ {} }}}} but no earlier step bound that variable",
                        var
                    )),
                }
            } else {
                Ok(args.clone())
            }
        }
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                out.insert(k.clone(), substitute_args(v, bindings)?);
            }
            Ok(Value::Object(out))
        }
        Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for v in items {
                out.push(substitute_args(v, bindings)?);
            }
            Ok(Value::Array(out))
        }
        // Number, bool, null — pass through.
        _ => Ok(args.clone()),
    }
}

/// If `s` is exactly `"{{ name }}"` (any whitespace around
/// `name`), return the trimmed name. Otherwise return None.
pub fn extract_placeholder(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if !trimmed.starts_with("{{") || !trimmed.ends_with("}}") {
        return None;
    }
    let inner = &trimmed[2..trimmed.len() - 2];
    let name = inner.trim();
    if name.is_empty() {
        return None;
    }
    Some(name.to_string())
}

// Suppress an unused-import warning for `json!` — kept
// around as a tiny helper for future tests / debug logging.
#[allow(dead_code)]
fn _unused_placeholder_for_visibility() -> Value {
    json!({})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_placeholder_returns_trimmed_name() {
        assert_eq!(
            extract_placeholder("{{  query  }}").as_deref(),
            Some("query")
        );
        assert_eq!(extract_placeholder("{{query}}").as_deref(), Some("query"));
        assert_eq!(extract_placeholder("query").as_deref(), None);
        assert_eq!(extract_placeholder("{{  }}").as_deref(), None);
        assert_eq!(extract_placeholder("").as_deref(), None);
    }

    #[test]
    fn substitute_replaces_whole_string_placeholders() {
        let mut bindings = HashMap::new();
        bindings.insert("url".to_string(), "https://example.com".to_string());

        let args = json!({
            "url": "{{url}}",
            "method": "GET",
            "options": { "follow": "{{url}}" }
        });
        let out = substitute_args(&args, &bindings).expect("substitute");
        assert_eq!(out["url"], "https://example.com");
        assert_eq!(out["method"], "GET");
        // Whole-string placeholder inside a nested object.
        assert_eq!(out["options"]["follow"], "https://example.com");
    }

    #[test]
    fn substitute_does_not_touch_partial_strings() {
        let mut bindings = HashMap::new();
        bindings.insert("url".to_string(), "https://example.com".to_string());
        let args = json!("prefix-{{url}}-suffix");
        let out = substitute_args(&args, &bindings).expect("substitute");
        // Partial match — leave untouched.
        assert_eq!(out, "prefix-{{url}}-suffix");
    }

    #[test]
    fn substitute_reports_missing_binding_with_friendly_message() {
        let bindings: HashMap<String, String> = HashMap::new();
        let args = json!("{{unknown}}");
        let err = substitute_args(&args, &bindings).expect_err("should fail");
        assert!(err.contains("unknown"), "got: {err}");
        assert!(err.contains("no earlier step bound"), "got: {err}");
    }

    #[test]
    fn substitute_passes_through_non_string_values() {
        let bindings = HashMap::new();
        let args = json!({ "n": 42, "b": true, "x": null });
        let out = substitute_args(&args, &bindings).expect("substitute");
        assert_eq!(out, args);
    }
}
