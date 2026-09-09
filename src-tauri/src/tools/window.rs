//! `window_*` tools — list and focus windows via System Events.
//!
//! - `window_list` (no consent) — every non-background process with
//!   its open window titles.
//! - `window_focus` (per-call consent) — bring a named application
//!   to the foreground.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::system::{escape_osa, format_stderr, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct WindowListTool;

#[async_trait]
impl Tool for WindowListTool {
    fn name(&self) -> &str {
        "window_list"
    }

    fn description(&self) -> &str {
        "List every visible (non-background-only) application process and the titles of its open windows. No consent required (read-only)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(
        &self,
        _invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let out = run_osa_script(WINDOW_LIST_SCRIPT, 10)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                "(no visible windows)".to_string()
            } else {
                out.stdout
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 16_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct WindowFocusTool;

#[async_trait]
impl Tool for WindowFocusTool {
    fn name(&self) -> &str {
        "window_focus"
    }

    fn description(&self) -> &str {
        "Bring a named application to the foreground. Requires per-call consent because it changes the user's focused app. Errors if the app isn't running."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "app_name": {
                    "type": "string",
                    "description": "The application name as it appears in System Events (e.g. 'Safari', 'Finder', 'Terminal')."
                }
            },
            "required": ["app_name"],
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
                "user denied the window_focus action".to_string(),
            ));
        }
        let app = require_string(&invocation.arguments, "app_name")?;
        let script = format!(
            r#"
tell application "System Events"
  set target to first application process whose name is "{}"
  if target is missing value then
    return "no running process: {}"
  end if
  set frontmost of target to true
  return "focused: {}"
end tell
"#,
            escape_osa(&app),
            escape_osa(&app),
            escape_osa(&app)
        );
        let out = run_osa_script(&script, 10)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(out.stdout.trim().to_string()))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

const WINDOW_LIST_SCRIPT: &str = r#"
set out to ""
tell application "System Events"
  set procs to (every process whose background only is false)
  repeat with p in procs
    set pName to (name of p) as string
    set winCount to (count of windows of p)
    if winCount is 0 then
      set out to out & pName & " (no windows)" & return
    else
      set winTitles to ""
      repeat with w in windows of p
        set t to (name of w) as string
        if t is missing value then set t to "(untitled)"
        set winTitles to winTitles & t & "|"
      end repeat
      set out to out & pName & " (" & winCount & "): " & winTitles & return
    end if
  end repeat
end tell
return out
"#;
