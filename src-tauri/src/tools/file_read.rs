//! `file_read` tool: read a UTF-8 file with a character cap.
//!
//! Two routing paths:
//!
//! 1. **Bot has a computer** (the `bot_id` in the
//!    `ToolInvocation` resolves to a `computers` row in
//!    the DB): read the file from the Bot's VM via
//!    SFTP. The 1 MB / 12 000-character cap is preserved
//!    but the SFTP path can't pre-stat the file, so
//!    it's enforced after read.
//!
//! 2. **Otherwise**: read locally. Paths are resolved
//!    relative to the user's home directory if they
//!    start with `~/`; everything else is treated as
//!    absolute. Hard 1 MB byte cap and 12 000-character
//!    cap protect both the model and the user from
//!    accidental dumps of huge logs or binaries.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;
use tokio::fs;

use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};
use crate::AppState;

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a UTF-8 text file at the given path. Use this to inspect source files, configs, or small logs. Paths starting with '~' are expanded to the user's home directory. Files larger than 1 MB or 12,000 characters are truncated with a marker. If the calling Bot has a provisioned computer, the file is read from the Bot's VM via SFTP instead of locally."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path, or a '~/...' path that will be expanded to the home directory. On a Bot's VM, the path is interpreted relative to the VM's filesystem."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path = require_str(&invocation.arguments, "path")?;

        // v2.0 Slice B: route to VM via SFTP if the
        // bot has a computer provisioned.
        if let Some(bot_id) = invocation.bot_id.as_deref() {
            if let Some(app) = context.app.as_ref() {
                if let Some(routed) =
                    try_read_on_computer(app, bot_id, path).await
                {
                    return routed;
                }
            }
        }

        let resolved = expand_tilde(path);
        let meta = fs::metadata(&resolved)
            .await
            .map_err(|e| ToolError::Execution(format!("stat failed: {e}")))?;
        if !meta.is_file() {
            return Err(ToolError::Execution(format!(
                "{} is not a regular file",
                resolved.display()
            )));
        }
        if meta.len() > 1_048_576 {
            return Ok(ToolResult::ok(format!(
                "{} is {} bytes (over 1 MB); refusing to read fully. Use shell_run with a streaming command instead.",
                resolved.display(),
                meta.len()
            )));
        }
        let body = fs::read_to_string(&resolved)
            .await
            .map_err(|e| ToolError::Execution(format!("read failed: {e}")))?;
        Ok(ToolResult::ok(truncate_for_model(&body, 12_000)))
    }
}

fn expand_tilde(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(input)
}

/// If the given bot has a computer, read `path` from the
/// VM via SFTP. Returns `None` if the bot has no
/// computer (fall through to local). Returns
/// `Some(Err)` for actual SFTP failures.
async fn try_read_on_computer(
    app: &tauri::AppHandle,
    bot_id: &str,
    path: &str,
) -> Option<Result<ToolResult, ToolError>> {
    let state = app.state::<AppState>();
    let db = state.db.clone();
    let mgr = state.computer.clone();

    // Probe on a blocking thread.
    let probe = tokio::task::spawn_blocking({
        let bot_id = bot_id.to_string();
        let db = db.clone();
        move || db.get_computer(&bot_id)
    })
    .await
    .ok()?;
    match probe {
        Ok(Some(r)) if !r.vm_name.is_empty() && r.state == "running" => {}
        Ok(_) => return None, // no computer → local
        Err(e) => {
            return Some(Err(ToolError::Execution(format!(
                "computer lookup failed: {e}"
            ))));
        }
    }
    match mgr.file_read(&db, bot_id, path).await {
        Ok(s) => Some(Ok(ToolResult::ok(truncate_for_model(&s, 12_000)))),
        Err(crate::computer::ComputerError::PassphraseMissing) => Some(Err(
            ToolError::Execution(
                "computer passphrase not set in Settings; cannot read from the Bot's VM"
                    .into(),
            ),
        )),
        Err(e) => Some(Err(ToolError::Execution(format!("sftp: {e}")))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract: the local path still works for the
    /// pre-v2.0 case (no Tauri context, no bot). The
    /// VM-routed path is verified manually
    /// end-to-end against a real VM.
    #[tokio::test]
    async fn expand_tilde_uses_home_when_present() {
        // When HOME is set, ~/foo resolves to
        // $HOME/foo. When HOME is unset, the path is
        // returned unchanged.
        let p = expand_tilde("~/foo");
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(p, PathBuf::from(home).join("foo"));
        } else {
            assert_eq!(p, PathBuf::from("~/foo"));
        }
    }
}
