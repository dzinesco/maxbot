//! `things_*` tools — drive Things 3 (https://culturedcode.com/things/) via
//! its AppleScript dictionary.
//!
//! - `things_today` (no consent) — list to-dos in the Today list
//! - `things_inbox` (no consent) — list to-dos in the Inbox
//! - `things_upcoming` (no consent) — list to-dos scheduled in the
//!   next `days_ahead` days (default 7)
//! - `things_projects` (no consent) — list project names (sidebar
//!   projects only, not areas)
//! - `things_add` (per-call consent) — create a new to-do in the
//!   named list (default: Inbox)
//! - `things_complete` (per-call consent) — mark a to-do complete
//!   by name or id
//!
//! All scripts use `escape_osa()` for any user-supplied string before
//! embedding it in an AppleScript double-quoted literal. If Things 3
//! is not installed the `osascript` call returns a clear error like
//! `Can't get application "Things3"`; we surface that verbatim so the
//! user (or model) can install Things 3 or fall back to Reminders.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::system::{escape_osa, format_stderr, optional_string, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

/// AppleScript boilerplate we wrap around every script so the
/// `tell application "Things3"` block is consistent and we can
/// spot malformed output early.
fn things_tell(inner: &str) -> String {
    format!(
        r#"tell application "Things3"
{inner}
end tell"#,
    )
}

/// Common run-and-shape helper. Returns the trimmed stdout on success
/// or a `ToolResult::err` containing the formatted stderr + TCC hint
/// (if any) on failure.
async fn run_things_script(script: &str, timeout_secs: u64) -> Result<String, ToolResult> {
    let out = run_osa_script(script, timeout_secs)
        .await
        .map_err(ToolError::Execution)
        .map_err(|e| ToolResult::err(e.to_string()))?;
    if out.succeeded() {
        Ok(out.stdout.trim().to_string())
    } else {
        Err(ToolResult::err(format_stderr(&out)))
    }
}

pub struct ThingsTodayTool;

#[async_trait]
impl Tool for ThingsTodayTool {
    fn name(&self) -> &str {
        "things_today"
    }

    fn description(&self) -> &str {
        "List the to-dos in the Things 3 Today list. Returns name and \
         due date for each (one per line). Empty list returns \
         '(no to-dos today)'. Requires Things 3 to be installed."
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
        let script = things_tell(
            r#"
            set out to ""
            set tds to to dos of list "Today"
            if (count of tds) is 0 then
                return "(no to-dos today)"
            end if
            repeat with td in tds
                set nm to name of td
                set dd to due date of td
                if dd is missing value then
                    set out to out & "- " & nm & linefeed
                else
                    set out to out & "- " & nm & " (due " & (dd as string) & ")" & linefeed
                end if
            end repeat
            return out
            "#,
        );
        match run_things_script(&script, 30).await {
            Ok(s) => Ok(ToolResult::ok(truncate_for_model(&s, 8_000))),
            Err(e) => Ok(e),
        }
    }
}

pub struct ThingsInboxTool;

#[async_trait]
impl Tool for ThingsInboxTool {
    fn name(&self) -> &str {
        "things_inbox"
    }

    fn description(&self) -> &str {
        "List the to-dos in the Things 3 Inbox. Returns name and \
         notes (if any) for each. Requires Things 3 to be installed."
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
        let script = things_tell(
            r#"
            set out to ""
            set tds to to dos of list "Inbox"
            if (count of tds) is 0 then
                return "(inbox is empty)"
            end if
            repeat with td in tds
                set nm to name of td
                set nt to notes of td
                if nt is missing value then
                    set out to out & "- " & nm & linefeed
                else
                    set out to out & "- " & nm & " — " & nt & linefeed
                end if
            end repeat
            return out
            "#,
        );
        match run_things_script(&script, 30).await {
            Ok(s) => Ok(ToolResult::ok(truncate_for_model(&s, 8_000))),
            Err(e) => Ok(e),
        }
    }
}

pub struct ThingsUpcomingTool;

#[async_trait]
impl Tool for ThingsUpcomingTool {
    fn name(&self) -> &str {
        "things_upcoming"
    }

    fn description(&self) -> &str {
        "List Things 3 to-dos scheduled in the next `days_ahead` days \
         (default 7). Returns name, due date, and the project name \
         (if any). Requires Things 3 to be installed."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "days_ahead": {
                    "type": "integer",
                    "description": "How many days ahead to look (1..=60, default 7)."
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
        let days = invocation
            .arguments
            .get("days_ahead")
            .and_then(|v| v.as_i64())
            .map(|n| n.clamp(1, 60) as i32)
            .unwrap_or(7);
        let inner = format!(
            r#"
            set out to ""
            set tds to to dos of list "Upcoming"
            set cutoff to (current date) + ({days} * days)
            repeat with td in tds
                set dd to due date of td
                if dd is not missing value and dd ≤ cutoff then
                    set nm to name of td
                    set out to out & "- " & nm & " (due " & (dd as string) & ")" & linefeed
                end if
            end repeat
            if out is "" then
                return "(nothing scheduled in the next {days} days)"
            end if
            return out
            "#
        );
        let script = things_tell(&inner);
        match run_things_script(&script, 30).await {
            Ok(s) => Ok(ToolResult::ok(truncate_for_model(&s, 8_000))),
            Err(e) => Ok(e),
        }
    }
}

pub struct ThingsProjectsTool;

#[async_trait]
impl Tool for ThingsProjectsTool {
    fn name(&self) -> &str {
        "things_projects"
    }

    fn description(&self) -> &str {
        "List Things 3 projects (not areas) by name. Returns one \
         project per line. Useful as a discovery step before \
         `things_add` with a project assignment."
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
        let script = things_tell(
            r#"
            set out to ""
            set ps to projects
            if (count of ps) is 0 then
                return "(no projects)"
            end if
            repeat with p in ps
                set out to out & "- " & (name of p) & linefeed
            end repeat
            return out
            "#,
        );
        match run_things_script(&script, 30).await {
            Ok(s) => Ok(ToolResult::ok(truncate_for_model(&s, 8_000))),
            Err(e) => Ok(e),
        }
    }
}

pub struct ThingsAddTool;

#[async_trait]
impl Tool for ThingsAddTool {
    fn name(&self) -> &str {
        "things_add"
    }

    fn description(&self) -> &str {
        "Create a new Things 3 to-do. `name` is required; `notes` and \
         `list` (Inbox | Today | Anytime | Someday) are optional. \
         Default list is Inbox. Returns the new to-do's Things 3 id. \
         Per-call consent because it mutates your task list."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "The to-do name." },
                "notes": { "type": "string", "description": "Optional notes body." },
                "list": {
                    "type": "string",
                    "description": "Where to create the to-do. One of: Inbox (default), Today, Anytime, Someday."
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
                "user denied the things_add action".to_string(),
            ));
        }
        let name = require_string(&invocation.arguments, "name")?;
        let notes = optional_string(&invocation.arguments, "notes");
        let list = optional_string(&invocation.arguments, "list")
            .unwrap_or_else(|| "Inbox".to_string());
        // Sanitize list name — only allow a known set; ignore anything
        // else and fall back to Inbox.
        let allowed_lists = ["Inbox", "Today", "Anytime", "Someday"];
        let list = if allowed_lists.contains(&list.as_str()) {
            list
        } else {
            "Inbox".to_string()
        };
        let name_osa = escape_osa(&name);
        let notes_clause = if let Some(ref n) = notes {
            let n_osa = escape_osa(n);
            format!(", notes:\"{n_osa}\"")
        } else {
            String::new()
        };
        let list_osa = escape_osa(&list);
        let inner = format!(
            r#"
            set t to make new to do in list "{list_osa}" with properties {{name:"{name_osa}"{notes_clause}}}
            return id of t
            "#
        );
        let script = things_tell(&inner);
        match run_things_script(&script, 30).await {
            Ok(id) => Ok(ToolResult::ok(format!(
                "added to-do '{name}' to {list} (id {id})"
            ))),
            Err(e) => Ok(e),
        }
    }
}

pub struct ThingsCompleteTool;

#[async_trait]
impl Tool for ThingsCompleteTool {
    fn name(&self) -> &str {
        "things_complete"
    }

    fn description(&self) -> &str {
        "Mark a Things 3 to-do complete. Provide either `id` \
         (Things 3 UUID, fastest) or `name` (case-sensitive search \
         across Today + Inbox). Returns the matched to-do's name. \
         Per-call consent because it mutates your task list."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "Things 3 UUID of the to-do." },
                "name": { "type": "string", "description": "Case-sensitive name to search Today + Inbox for." }
            },
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
                "user denied the things_complete action".to_string(),
            ));
        }
        let id = optional_string(&invocation.arguments, "id");
        let name = optional_string(&invocation.arguments, "name");
        if id.is_none() && name.is_none() {
            return Err(ToolError::InvalidArguments(
                "must provide either `id` or `name`".to_string(),
            ));
        }
        let inner = if let Some(id) = id {
            let id_osa = escape_osa(&id);
            format!(
                r#"
                set t to first to do whose id is "{id_osa}"
                set nm to name of t
                set status of t to completed
                return nm
                "#
            )
        } else {
            // Search Today first, then Inbox.
            let name_osa = escape_osa(name.as_deref().unwrap_or(""));
            format!(
                r#"
                set targetName to "{name_osa}"
                repeat with L in {{"Today", "Inbox"}}
                    set lst to to dos of list (L as string)
                    repeat with td in lst
                        if name of td is targetName then
                            set status of td to completed
                            return (name of td)
                        end if
                    end repeat
                end repeat
                return "NOT_FOUND"
                "#
            )
        };
        let script = things_tell(&inner);
        match run_things_script(&script, 30).await {
            Ok(s) if s == "NOT_FOUND" => Ok(ToolResult::err(
                "no to-do with that name in Today or Inbox".to_string(),
            )),
            Ok(name) => Ok(ToolResult::ok(format!("completed: {name}"))),
            Err(e) => Ok(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_escape_is_safe() {
        // Embedded quotes / backslashes must round-trip through the
        // AppleScript literal.
        let s = r#"He said "hi" and used \backslash"#;
        let escaped = escape_osa(s);
        assert!(escaped.contains(r#"\""#));
        assert!(escaped.contains("\\\\"));
    }

    #[test]
    fn things_tell_wraps_correctly() {
        let s = things_tell("return 1");
        assert!(s.starts_with(r#"tell application "Things3""#));
        assert!(s.ends_with("end tell"));
        assert!(s.contains("return 1"));
    }
}
