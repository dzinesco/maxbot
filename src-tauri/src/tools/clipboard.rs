//! `clipboard_read` and `clipboard_write` — read or set the macOS
//! clipboard via AppleScript. Read is no-consent (the user can already
//! see the clipboard from the menu bar); write requires consent because
//! it overwrites whatever the user has on their clipboard.
//!
//! For `clipboard_write`, we write the text to a temp file from Rust
//! (so we can embed any bytes including quotes, newlines, NULs), then
//! have AppleScript read it back and assign to the clipboard. This is
//! more robust than trying to escape the text into an AppleScript
//! string literal.

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::time::Duration;

use super::apple_script_exec::run_osa_script;
use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct ClipboardReadTool;

#[async_trait]
impl Tool for ClipboardReadTool {
    fn name(&self) -> &str {
        "clipboard_read"
    }

    fn description(&self) -> &str {
        "Read the current contents of the macOS clipboard as plain text. Returns up to ~16K characters; longer content is truncated. No consent required (read-only)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // `the clipboard` is a built-in AppleScript noun. Reading
        // doesn't require TCC.
        let script = "the clipboard as text";
        let out = run_osa_script(script, 5)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.is_empty() {
                "(clipboard is empty or contains non-text content)".to_string()
            } else {
                out.stdout.clone()
            };
            return Ok(ToolResult::ok(truncate_for_model(&body, 16_000)));
        }
        let mut message = out.stderr.clone();
        if let Some(hint) = out.tcc_hint() {
            message.push_str(&format!("\n\nHint: {hint}"));
        }
        Ok(ToolResult::err(truncate_for_model(&message, 4_000)))
    }
}

pub struct ClipboardWriteTool;

#[async_trait]
impl Tool for ClipboardWriteTool {
    fn name(&self) -> &str {
        "clipboard_write"
    }

    fn description(&self) -> &str {
        "Overwrite the macOS clipboard with the given text. Requires per-call consent because it replaces whatever the user currently has on their clipboard."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to put on the clipboard."
                }
            },
            "required": ["text"],
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
                "user denied the clipboard_write action".to_string(),
            ));
        }
        let text = require_str(&invocation.arguments, "text")?.to_string();
        write_clipboard_via_tempfile(&text).await
    }
}

/// Write the text to a temp file, then have AppleScript read it and
/// assign to the clipboard. Robust to any byte content (quotes,
/// newlines, NULs, unicode).
async fn write_clipboard_via_tempfile(text: &str) -> Result<ToolResult, ToolError> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("maxbot-clip-{}.txt", uuid::Uuid::new_v4()));
    if let Err(e) = tokio::fs::write(&path, text).await {
        return Err(ToolError::Execution(format!(
            "could not write temp file: {e}"
        )));
    }
    let path_str = path.to_string_lossy().to_string();
    let script = format!(
        r#"set the clipboard to (read file POSIX file "{}" as «class utf8»)"#,
        path_str.replace('"', "\\\"")
    );
    let result = tokio::time::timeout(Duration::from_secs(5), run_osa_script(&script, 5)).await;
    let _ = tokio::fs::remove_file(&path).await;
    let out = match result {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(ToolError::Execution(e)),
        Err(_) => {
            return Err(ToolError::Execution(
                "clipboard write exceeded 5s timeout".to_string(),
            ));
        }
    };
    if out.succeeded() {
        Ok(ToolResult::ok(format!(
            "wrote {} characters to the clipboard",
            text.chars().count()
        )))
    } else {
        let mut message = out.stderr.clone();
        if let Some(hint) = out.tcc_hint() {
            message.push_str(&format!("\n\nHint: {hint}"));
        }
        Ok(ToolResult::err(truncate_for_model(&message, 4_000)))
    }
}
