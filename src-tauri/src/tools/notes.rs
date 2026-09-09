//! `notes_*` tools — drive Notes.app via AppleScript.
//!
//! - `notes_search` (no consent) — find notes by title substring.
//! - `notes_read` (no consent) — fetch the full body of a note
//!   identified by name (exact match first, then containing match).
//! - `notes_create` (per-call consent) — create a new note.
//!
//! Notes have a `name` (title) and a `body` (rich text in modern
//! Notes.app, plain text in older versions). We use `body` and accept
//! that some rich-text formatting may be lost.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::system::{escape_osa, format_stderr, optional_string, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct NotesSearchTool;

#[async_trait]
impl Tool for NotesSearchTool {
    fn name(&self) -> &'static str {
        "notes_search"
    }

    fn description(&self) -> &'static str {
        "Find notes by title substring. Returns title, modification date, and a short body snippet for each match. No consent required (read-only)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Case-insensitive substring to match against note titles."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 200,
                    "default": 25,
                    "description": "Max notes to return."
                },
                "snippet_chars": {
                    "type": "integer",
                    "minimum": 50,
                    "maximum": 2000,
                    "default": 200,
                    "description": "Max characters of body to include in each snippet."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let query = require_string(&invocation.arguments, "query")?;
        if query.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "query must not be empty".to_string(),
            ));
        }
        let limit = invocation
            .arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, 200))
            .unwrap_or(25);
        let snippet = invocation
            .arguments
            .get("snippet_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(50, 2_000))
            .unwrap_or(200);

        let script = build_search_script(&query, limit, snippet);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                format!("(no notes matching '{}')", query)
            } else {
                out.stdout
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 10_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct NotesReadTool;

#[async_trait]
impl Tool for NotesReadTool {
    fn name(&self) -> &'static str {
        "notes_read"
    }

    fn description(&self) -> &'static str {
        "Read the full body of a note by name. Tries exact match first, then falls back to a single containing match. Returns 'multiple matches' with a list if more than one note contains the query — use notes_search to disambiguate. No consent required (read-only)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Note title (exact or substring match)."
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 100,
                    "maximum": 100_000,
                    "default": 20_000,
                    "description": "Cap on body length returned. Notes longer than this are truncated."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let name = require_string(&invocation.arguments, "name")?;
        let max_chars = invocation
            .arguments
            .get("max_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(100, 100_000))
            .unwrap_or(20_000);
        let script = build_read_script(&name, max_chars);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(truncate_for_model(&out.stdout, max_chars)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct NotesCreateTool;

#[async_trait]
impl Tool for NotesCreateTool {
    fn name(&self) -> &'static str {
        "notes_create"
    }

    fn description(&self) -> &'static str {
        "Create a new note with the given name and (optional) body. Requires per-call consent because it mutates the user's Notes."
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
                    "description": "Note title."
                },
                "body": {
                    "type": "string",
                    "description": "Optional plain-text body."
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
                "user denied the notes_create action".to_string(),
            ));
        }
        let name = require_string(&invocation.arguments, "name")?;
        let body = optional_string(&invocation.arguments, "body");
        let script = build_create_script(&name, body.as_deref());
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!("created note: \"{}\"", name)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

// ---- AppleScript builders ----

fn build_search_script(query: &str, limit: u32, snippet: usize) -> String {
    let q = escape_osa(query);
    format!(
        r#"
set theQuery to "{q}"
set snippetLen to {snippet}
set out to ""
tell application "Notes"
  set allNotes to notes
  set matched to {{}}
  repeat with n in allNotes
    set nName to (name of n) as string
    if nName contains theQuery then
      set end of matched to n
    end if
  end repeat
  set total to (count matched)
  set lim to {limit}
  if total < lim then set lim to total
  repeat with i from 1 to lim
    set n to item i of matched
    set nName to (name of n) as string
    set nBody to (body of n) as string
    if (count of nBody) > snippetLen then
      set nBody to text 1 thru snippetLen of nBody
    end if
    set modDate to ""
    try
      set modDate to (modification date of n) as string
    end try
    set out to out & "<<<NOTE " & i & ">>>" & return & "Title: " & nName & return & "Modified: " & modDate & return & "Snippet: " & nBody & return & return
  end repeat
end tell
return out
"#
    )
}

fn build_read_script(name: &str, max_chars: usize) -> String {
    let n = escape_osa(name);
    format!(
        r#"
set theName to "{n}"
set maxLen to {max_chars}
tell application "Notes"
  set exacts to (every note whose name is theName)
  set exactCount to (count exacts)
  set target to missing value
  if exactCount > 0 then
    set target to item 1 of exacts
  else
    set fuzzies to (every note whose name contains theName)
    set fuzzyCount to (count fuzzies)
    if fuzzyCount is 0 then
      return "no note found matching: {n}"
    else if fuzzyCount is 1 then
      set target to item 1 of fuzzies
    else
      set names to ""
      repeat with x in fuzzies
        set names to names & (name of x as string) & "|"
      end repeat
      return "multiple matches; please disambiguate: " & names
    end if
  end if
  set outName to (name of target) as string
  set outBody to (body of target) as string
  if (count of outBody) > maxLen then
    set outBody to text 1 thru maxLen of outBody
  end if
  return "Title: " & outName & return & return & outBody
end tell
"#
    )
}

fn build_create_script(name: &str, body: Option<&str>) -> String {
    let n = escape_osa(name);
    let body_set = body
        .map(|b| format!(r#"set body of newN to "{}""#, escape_osa(b)))
        .unwrap_or_default();
    format!(
        r#"
tell application "Notes"
  set newN to make new note with properties {{name:"{n}"}}
  {body_set}
end tell
return "created"
"#
    )
}
