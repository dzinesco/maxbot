//! `apple_script_run` — the AppleScript escape hatch.
//!
//! Takes a raw AppleScript snippet, runs it via `osascript -e`, returns
//! stdout (the natural result of the script) plus any stderr. Requires
//! per-call consent because the script can do anything an AppleScript
//! can do — including `do shell script` for full shell access and any
//! app's AppleScript dictionary for app control.
//!
//! The dedicated tools (`mail_inbox`, `calendar_today`, etc.) are
//! preferred for the common cases; this one is for when there's no
//! dedicated tool yet, or for a quick one-off script.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct AppleScriptRunTool;

#[async_trait]
impl Tool for AppleScriptRunTool {
    fn name(&self) -> &'static str {
        "apple_script_run"
    }

    fn description(&self) -> &'static str {
        "Run an arbitrary AppleScript snippet via `osascript -e` and return the result. Use this as an escape hatch when a dedicated tool (mail_*, calendar_*, etc.) doesn't cover your need. The script can include `do shell script \"...\"` for terminal access, and any app's AppleScript dictionary for app control — but the dedicated tools are preferred because they surface TCC errors and structured output. Hard 30-second timeout. Requires per-call consent."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "string",
                    "description": "The AppleScript to run. Multiple lines are fine; pass as one string."
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Override the default 30-second timeout. Useful for slow operations like a large Mail inbox read.",
                    "minimum": 1,
                    "maximum": 600,
                    "default": 30
                }
            },
            "required": ["script"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the apple_script_run action".to_string(),
            ));
        }
        let script = require_str(&invocation.arguments, "script")?.to_string();
        let timeout = invocation
            .arguments
            .get("timeout_seconds")
            .and_then(|v| v.as_u64())
            .unwrap_or(30);

        let out = run_osa_script(&script, timeout)
            .await
            .map_err(ToolError::Execution)?;

        if out.succeeded() {
            let body = if out.stdout.is_empty() {
                "(script returned no output)".to_string()
            } else {
                out.stdout.clone()
            };
            return Ok(ToolResult::ok(truncate_for_model(&body, 8_000)));
        }

        // Build a useful error: include the script's stderr, and if it's
        // a TCC denial, append the friendly hint.
        let mut message = if out.stderr.is_empty() {
            format!("osascript exited with code {:?}", out.exit_code)
        } else {
            out.stderr.clone()
        };
        if let Some(hint) = out.tcc_hint() {
            message.push_str(&format!("\n\nHint: {hint}"));
        }
        Ok(ToolResult::err(truncate_for_model(&message, 8_000)))
    }
}
