//! `reminders_*` tools — drive Reminders.app via AppleScript.
//!
//! - `reminders_list` (no consent) — list reminders, optionally filtered
//!   by list name and completion state.
//! - `reminders_add` (per-call consent) — create a new reminder.
//! - `reminders_complete` (per-call consent) — mark a reminder as
//!   completed by name match.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::system::{escape_osa, format_stderr, optional_string, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct RemindersListTool;

#[async_trait]
impl Tool for RemindersListTool {
    fn name(&self) -> &str {
        "reminders_list"
    }

    fn description(&self) -> &str {
        "List reminders, optionally filtered by list name. By default, incomplete reminders across all lists. Returns title, parent list, due date, and body (if any). No consent required (read-only)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "list_name": {
                    "type": "string",
                    "description": "Optional list name to filter by (e.g. 'Personal', 'Work'). If empty, returns reminders from all lists."
                },
                "include_completed": {
                    "type": "boolean",
                    "default": false,
                    "description": "If true, also returns completed reminders."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 500,
                    "default": 100,
                    "description": "Max reminders to return."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let list_name = optional_string(&invocation.arguments, "list_name");
        let include_completed = invocation
            .arguments
            .get("include_completed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = invocation
            .arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, 500))
            .unwrap_or(100);

        let script = build_list_script(list_name.as_deref(), include_completed, limit);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                "(no reminders)".to_string()
            } else {
                out.stdout
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 10_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct RemindersAddTool;

#[async_trait]
impl Tool for RemindersAddTool {
    fn name(&self) -> &str {
        "reminders_add"
    }

    fn description(&self) -> &str {
        "Create a new reminder. Optionally assign to a list and set a due date. Requires per-call consent because it mutates the user's reminders."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Reminder title."
                },
                "body": {
                    "type": "string",
                    "description": "Optional body / notes."
                },
                "list_name": {
                    "type": "string",
                    "description": "Optional list name (e.g. 'Personal'). If not provided, Reminders.app picks the default."
                },
                "due": {
                    "type": "string",
                    "description": "Optional due date in 'yyyy-mm-dd HH:MM' local time (or 'yyyy-mm-dd HH:MM:SS'). If omitted, the reminder has no due date."
                }
            },
            "required": ["name"],
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
                "user denied the reminders_add action".to_string(),
            ));
        }
        let name = require_string(&invocation.arguments, "name")?;
        let body = optional_string(&invocation.arguments, "body");
        let list_name = optional_string(&invocation.arguments, "list_name");
        let due = optional_string(&invocation.arguments, "due");

        let script = build_add_script(
            &name,
            body.as_deref(),
            list_name.as_deref(),
            due.as_deref(),
        );
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!("added reminder: \"{}\"", name)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct RemindersCompleteTool;

#[async_trait]
impl Tool for RemindersCompleteTool {
    fn name(&self) -> &str {
        "reminders_complete"
    }

    fn description(&self) -> &str {
        "Mark a reminder as completed by matching its name (case-insensitive, exact match first, falls back to first containing reminder). Requires per-call consent because it mutates state."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The reminder name to mark complete."
                }
            },
            "required": ["name"],
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
                "user denied the reminders_complete action".to_string(),
            ));
        }
        let name = require_string(&invocation.arguments, "name")?;
        let script = build_complete_script(&name);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!("completed: \"{}\"", name)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

// ---- AppleScript builders ----

fn build_list_script(list_name: Option<&str>, include_completed: bool, limit: u32) -> String {
    let completed_filter = if include_completed {
        "every reminder"
    } else {
        "every reminder whose completed is false"
    };
    let list_filter = if let Some(name) = list_name {
        let e = escape_osa(name);
        format!(
            r#"set targetList to first list whose name is "{e}"
if targetList is missing value then
  return "list not found: {e}"
end if
set rs to (every reminder of targetList whose completed is {cmp})
"#,
            cmp = if include_completed { "true" } else { "false" },
            e = e
        )
    } else {
        format!("set rs to ({})\n", completed_filter)
    };
    format!(
        r#"
{list_filter}set out to ""
set n to (count rs)
if n > {limit} then set n to {limit}
repeat with i from 1 to n
  set r to item i of rs
  set rName to (name of r) as string
  if rName is missing value then set rName to "(no name)"
  set listName to ""
  try
    set listName to (name of (get container of r)) as string
  end try
  set dueStr to ""
  try
    set dueStr to (due date of r) as string
  end try
  set bodyStr to ""
  try
    set bodyStr to (body of r) as string
  end try
  set out to out & "<<<REMINDER>>>" & return & "Title: " & rName & return & "List: " & listName & return
  if dueStr is not "" then set out to out & "Due: " & dueStr & return
  if bodyStr is not "" then set out to out & "Body: " & bodyStr & return
  set out to out & "Completed: " & (completed of r) & return & return
end repeat
return out
"#
    )
}

fn build_add_script(
    name: &str,
    body: Option<&str>,
    list_name: Option<&str>,
    due: Option<&str>,
) -> String {
    let name_e = escape_osa(name);
    let body_set = body
        .map(|b| format!(r#"set body of newR to "{}""#, escape_osa(b)))
        .unwrap_or_default();
    let due_set = due
        .map(|d| {
            format!(
                r#"set dueDate to date "{}"
set due date of newR to dueDate"#,
                escape_osa(d)
            )
        })
        .unwrap_or_default();
    let list_pick = if let Some(list) = list_name {
        let e = escape_osa(list);
        format!(
            r#"set targetList to first list whose name is "{e}"
if targetList is missing value then
  return "list not found: {e}"
end if
"#,
            e = e
        )
    } else {
        String::new()
    };
    let list_assign = if list_name.is_some() {
        "move newR to targetList\n"
    } else {
        ""
    };
    format!(
        r#"{list_pick}tell application "Reminders"
  set newR to make new reminder with properties {{name:"{name_e}"}}
  {body_set}
  {due_set}
  {list_assign}end tell
return "added"
"#
    )
}

fn build_complete_script(name: &str) -> String {
    let name_e = escape_osa(name);
    format!(
        r#"
tell application "Reminders"
  set exactMatches to (every reminder whose name is "{name_e}" and completed is false)
  set exactCount to (count exactMatches)
  set target to missing value
  if exactCount > 0 then
    set target to item 1 of exactMatches
  else
    set fuzzyMatches to (every reminder whose name contains "{name_e}" and completed is false)
    set fuzzyCount to (count fuzzyMatches)
    if fuzzyCount is 0 then
      return "no incomplete reminder found matching: {name_e}"
    else if fuzzyCount is 1 then
      set target to item 1 of fuzzyMatches
    else
      set names to ""
      repeat with r in fuzzyMatches
        set names to names & (name of r as string) & "|"
      end repeat
      return "multiple matches; please disambiguate: " & names
    end if
  end if
  set completed of target to true
  return "completed: " & (name of target as string)
end tell
"#
    )
}
