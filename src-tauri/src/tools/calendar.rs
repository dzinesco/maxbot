//! `calendar_*` tools — drive Calendar.app via AppleScript.
//!
//! - `calendar_today` (no consent) — events from the start of today
//!   through end of today, all calendars.
//! - `calendar_week` (no consent) — events from the start of today
//!   through `days_ahead` days (default 7).
//! - `calendar_create_event` (per-call consent) — create a new event
//!   in a named calendar (default = first calendar).
//!
//! Date input format for `calendar_create_event.start`: "yyyy-mm-dd
//! HH:MM" in local time, or "yyyy-mm-dd HH:MM:SS". AppleScript's
//! `date` class parses these.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::system::{escape_osa, format_stderr, optional_string, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct CalendarTodayTool;

#[async_trait]
impl Tool for CalendarTodayTool {
    fn name(&self) -> &str {
        "calendar_today"
    }

    fn description(&self) -> &str {
        "List all events scheduled for today across every calendar. Returns event title, start time, end time, location, and notes (if any). No consent required (read-only)."
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
        let script = build_range_script(1);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                "(no events today)".to_string()
            } else {
                out.stdout
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 8_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct CalendarWeekTool;

#[async_trait]
impl Tool for CalendarWeekTool {
    fn name(&self) -> &str {
        "calendar_week"
    }

    fn description(&self) -> &str {
        "List events from today through `days_ahead` days (default 7), across every calendar. Returns title, start, end, location, notes. No consent required (read-only)."
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
                    "minimum": 1,
                    "maximum": 30,
                    "default": 7,
                    "description": "How many days ahead to look, starting from today."
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
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, 30))
            .unwrap_or(7);
        let script = build_range_script(days);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                format!("(no events in the next {} days)", days)
            } else {
                out.stdout
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 12_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct CalendarCreateEventTool;

#[async_trait]
impl Tool for CalendarCreateEventTool {
    fn name(&self) -> &str {
        "calendar_create_event"
    }

    fn description(&self) -> &str {
        "Create a new event. start is 'yyyy-mm-dd HH:MM' (or 'yyyy-mm-dd HH:MM:SS') in local time. duration_minutes defaults to 60. calendar_name defaults to the first calendar if not provided. location and notes are optional. Requires per-call consent because it mutates the user's calendar."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Event title (summary)."
                },
                "start": {
                    "type": "string",
                    "description": "Start time in 'yyyy-mm-dd HH:MM' (or 'yyyy-mm-dd HH:MM:SS') local time. Example: '2026-09-09 14:00'."
                },
                "duration_minutes": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 60 * 24 * 7,
                    "default": 60,
                    "description": "How long the event lasts. Defaults to 60."
                },
                "calendar_name": {
                    "type": "string",
                    "description": "Name of the calendar to add to. Defaults to the first calendar."
                },
                "location": {
                    "type": "string",
                    "description": "Optional location string."
                },
                "notes": {
                    "type": "string",
                    "description": "Optional notes/description."
                }
            },
            "required": ["title", "start"],
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
                "user denied the calendar_create_event action".to_string(),
            ));
        }
        let title = require_string(&invocation.arguments, "title")?;
        let start_str = require_string(&invocation.arguments, "start")?;
        let duration = invocation
            .arguments
            .get("duration_minutes")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, 60 * 24 * 7))
            .unwrap_or(60);
        let calendar = optional_string(&invocation.arguments, "calendar_name");
        let location = optional_string(&invocation.arguments, "location");
        let notes = optional_string(&invocation.arguments, "notes");

        let script = build_create_script(
            &title,
            &start_str,
            duration,
            calendar.as_deref(),
            location.as_deref(),
            notes.as_deref(),
        );
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!(
                "created: \"{}\" at {} ({} min)",
                title, start_str, duration
            )))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

// ---- AppleScript builders ----

fn build_range_script(days_ahead: u32) -> String {
    format!(
        r#"
set numDays to {days_ahead}
set out to ""
set now to current date
set startDate to now - (time of now)
set endDate to startDate + (numDays * days)
tell application "Calendar"
  repeat with cal in calendars
    set calName to (name of cal) as string
    set evts to (every event of cal whose start date >= startDate and start date < endDate)
    repeat with e in evts
      set s to (start date of e) as string
      set en to (end date of e) as string
      set summ to (summary of e) as string
      if summ is missing value then set summ to "(no title)"
      set loc to ""
      try
        set loc to (location of e) as string
        if loc is missing value then set loc to ""
      end try
      set desc to ""
      try
        set desc to (description of e) as string
        if desc is missing value then set desc to ""
      end try
      set out to out & "<<<EVENT>>>" & return & "Calendar: " & calName & return & "Title: " & summ & return & "Start: " & s & return & "End: " & en & return
      if loc is not "" then set out to out & "Location: " & loc & return
      if desc is not "" then set out to out & "Notes: " & desc & return
      set out to out & return
    end repeat
  end repeat
end tell
return out
"#
    )
}

fn build_create_script(
    title: &str,
    start_str: &str,
    duration_minutes: u32,
    calendar_name: Option<&str>,
    location: Option<&str>,
    notes: Option<&str>,
) -> String {
    let title_e = escape_osa(title);
    let start_e = escape_osa(start_str);
    let loc_e = location.map(escape_osa);
    let notes_e = notes.map(escape_osa);
    let cal_pick = if let Some(cn) = calendar_name {
        let cn_e = escape_osa(cn);
        format!(
            r#"  set targetCal to first calendar whose name is "{cn_e}"
  if targetCal is missing value then
    return "calendar not found: {cn_e}"
  end if
  set cal to targetCal
"#,
            cn_e = cn_e
        )
    } else {
        r#"  set cal to first calendar
"#
        .to_string()
    };
    let loc_prop = loc_e
        .as_ref()
        .map(|l| format!(r#"  set locProp to "{l}"
"#))
        .unwrap_or_default();
    let notes_prop = notes_e
        .as_ref()
        .map(|n| format!(r#"  set notesProp to "{n}"
"#))
        .unwrap_or_default();
    let loc_set = if loc_e.is_some() {
        r#"  set location of newEvent to locProp
"#
    } else {
        ""
    };
    let notes_set = if notes_e.is_some() {
        r#"  set description of newEvent to notesProp
"#
    } else {
        ""
    };
    format!(
        r#"set startStr to "{start_e}"
set startDate to date startStr
set endDate to startDate + ({duration_minutes} * minutes)
{cal_pick}{loc_prop}{notes_prop}tell application "Calendar"
  set newEvent to make new event at end of events of cal with properties {{summary:"{title_e}", start date:startDate, end date:endDate}}
{loc_set}{notes_set}end tell
return "created"
"#
    )
}
