//! Browser-automation tools via AppleScript. Covers both Safari and
//! Google Chrome. The two browsers share a tool set (open, current_url,
//! exec_js, tabs) and are differentiated by name only.
//!
//! - `*_open` (per-call consent) — open a URL in a new tab or focus
//!   the existing tab on that URL.
//! - `*_current_url` (no consent) — get the URL of the frontmost tab.
//! - `*_exec_js` (per-call consent) — execute JavaScript in the
//!   frontmost tab. JS can do anything in the page's origin, so this
//!   is gated.
//! - `*_tabs` (no consent) — list all open tabs with title + URL.
//!
//! Chrome's AppleScript dictionary uses `active tab` (singular) and
//! `title` (rather than `name`). Safari uses `current tab` and `name`.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::system::{escape_osa, format_stderr, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

// ---- Safari ----

pub struct SafariOpenTool;

#[async_trait]
impl Tool for SafariOpenTool {
    fn name(&self) -> &str {
        "safari_open"
    }

    fn description(&self) -> &str {
        "Open the given URL in Safari. If Safari is not running, launches it. If a tab is already on this URL, focuses it; otherwise opens a new tab. Requires per-call consent because it navigates the browser."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to open. Include the scheme (https://...)."
                }
            },
            "required": ["url"],
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
                "user denied the safari_open action".to_string(),
            ));
        }
        let url = require_string(&invocation.arguments, "url")?;
        let script = safari_open_script(&url);
        let out = run_osa_script(&script, 15)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!("opened: {}", url)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SafariCurrentUrlTool;

#[async_trait]
impl Tool for SafariCurrentUrlTool {
    fn name(&self) -> &str {
        "safari_current_url"
    }

    fn description(&self) -> &str {
        "Get the URL of the frontmost Safari tab. No consent required (read-only)."
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
        let out = run_osa_script(SAFARI_CURRENT_URL_SCRIPT, 10)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(out.stdout.trim().to_string()))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SafariExecJsTool;

#[async_trait]
impl Tool for SafariExecJsTool {
    fn name(&self) -> &str {
        "safari_exec_js"
    }

    fn description(&self) -> &str {
        "Execute JavaScript in the frontmost Safari tab. The script runs in the page's origin and can read/modify the DOM, set cookies, call fetch(), etc. Requires per-call consent because JS in a page context is essentially unbounded."
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
                    "description": "The JavaScript source to execute. The script's final expression is the result, returned as a string."
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
                "user denied the safari_exec_js action".to_string(),
            ));
        }
        let script = require_string(&invocation.arguments, "script")?;
        // AppleScript's `do JavaScript` accepts a string; we pass the
        // script verbatim. The result is coerced to a string.
        let body = format!(
            r#"tell application "Safari"
  set jsResult to do JavaScript "{}" in current tab of front window
  return jsResult as string
end tell"#,
            // AppleScript escapes inside a string literal: backslash
            // and double-quote. We do NOT shell-escape (no $(), no
            // backticks), so the only chars that matter are these.
            escape_osa(&script)
        );
        let out = run_osa_script(&body, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(truncate_for_model(&out.stdout, 16_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SafariTabsTool;

#[async_trait]
impl Tool for SafariTabsTool {
    fn name(&self) -> &str {
        "safari_tabs"
    }

    fn description(&self) -> &str {
        "List all open Safari tabs across all windows. Returns title and URL for each. No consent required (read-only)."
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
        let out = run_osa_script(SAFARI_TABS_SCRIPT, 15)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                "(no Safari tabs)".to_string()
            } else {
                out.stdout
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 12_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

// ---- Chrome ----

pub struct ChromeOpenTool;

#[async_trait]
impl Tool for ChromeOpenTool {
    fn name(&self) -> &str {
        "chrome_open"
    }

    fn description(&self) -> &str {
        "Open the given URL in Google Chrome. Launches Chrome if not running. Requires per-call consent because it navigates the browser. Will fail if Chrome is not installed."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to open. Include the scheme (https://...)."
                }
            },
            "required": ["url"],
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
                "user denied the chrome_open action".to_string(),
            ));
        }
        let url = require_string(&invocation.arguments, "url")?;
        let script = chrome_open_script(&url);
        let out = run_osa_script(&script, 15)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!("opened: {}", url)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct ChromeCurrentUrlTool;

#[async_trait]
impl Tool for ChromeCurrentUrlTool {
    fn name(&self) -> &str {
        "chrome_current_url"
    }

    fn description(&self) -> &str {
        "Get the URL of the frontmost Google Chrome tab. No consent required (read-only). Will return an error if Chrome is not running or not installed."
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
        let out = run_osa_script(CHROME_CURRENT_URL_SCRIPT, 10)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(out.stdout.trim().to_string()))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct ChromeExecJsTool;

#[async_trait]
impl Tool for ChromeExecJsTool {
    fn name(&self) -> &str {
        "chrome_exec_js"
    }

    fn description(&self) -> &str {
        "Execute JavaScript in the frontmost Google Chrome tab. Same model as safari_exec_js: runs in the page's origin, can do anything. Requires per-call consent."
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
                    "description": "The JavaScript source to execute."
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
                "user denied the chrome_exec_js action".to_string(),
            ));
        }
        let script = require_string(&invocation.arguments, "script")?;
        let body = format!(
            r#"tell application "Google Chrome"
  set jsResult to execute javascript "{}" in active tab of front window
  return jsResult as string
end tell"#,
            escape_osa(&script)
        );
        let out = run_osa_script(&body, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(truncate_for_model(&out.stdout, 16_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct ChromeTabsTool;

#[async_trait]
impl Tool for ChromeTabsTool {
    fn name(&self) -> &str {
        "chrome_tabs"
    }

    fn description(&self) -> &str {
        "List all open Google Chrome tabs across all windows. Returns title and URL. No consent required (read-only). Errors if Chrome is not running."
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
        let out = run_osa_script(CHROME_TABS_SCRIPT, 15)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                "(no Chrome tabs)".to_string()
            } else {
                out.stdout
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 12_000)))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

// ---- AppleScript constants ----

const SAFARI_CURRENT_URL_SCRIPT: &str = r#"
tell application "Safari"
  if (count of windows) is 0 then return "(no Safari windows open)"
  return URL of current tab of front window
end tell
"#;

const SAFARI_TABS_SCRIPT: &str = r#"
set out to ""
tell application "Safari"
  set winCount to (count of windows)
  if winCount is 0 then return ""
  repeat with w in windows
    set winIndex to (index of w)
    repeat with t in tabs of w
      set tName to (name of t) as string
      if tName is missing value then set tName to "(no title)"
      set tURL to (URL of t) as string
      set out to out & "Window " & winIndex & " — " & tName & return & "  " & tURL & return
    end repeat
  end repeat
end tell
return out
"#;

const CHROME_CURRENT_URL_SCRIPT: &str = r#"
tell application "Google Chrome"
  if (count of windows) is 0 then return "(no Chrome windows open)"
  return URL of active tab of front window
end tell
"#;

const CHROME_TABS_SCRIPT: &str = r#"
set out to ""
tell application "Google Chrome"
  set winCount to (count of windows)
  if winCount is 0 then return ""
  repeat with w in windows
    set winIndex to (index of w)
    repeat with t in tabs of w
      set tTitle to (title of t) as string
      if tTitle is missing value then set tTitle to "(no title)"
      set tURL to (URL of t) as string
      set out to out & "Window " & winIndex & " — " & tTitle & return & "  " & tURL & return
    end repeat
  end repeat
end tell
return out
"#;

fn safari_open_script(url: &str) -> String {
    format!(
        r#"
tell application "Safari"
  activate
  if (count of windows) is 0 then make new document
  set URL of front document to "{}"
end tell
return "opened"
"#,
        escape_osa(url)
    )
}

fn chrome_open_script(url: &str) -> String {
    format!(
        r#"
tell application "Google Chrome"
  activate
  if (count of windows) is 0 then make new window
  set URL of active tab of front window to "{}"
end tell
return "opened"
"#,
        escape_osa(url)
    )
}
