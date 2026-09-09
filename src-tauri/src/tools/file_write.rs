//! `file_write` tool: write a string to disk, creating parent dirs.
//!
//! Two routing paths:
//!
//! 1. **Bot has a computer** (the `bot_id` in the
//!    `ToolInvocation` resolves to a `computers` row in
//!    the DB): write to the Bot's VM via SFTP. The
//!    writes go through a temp file on the VM followed
//!    by an atomic rename (see `ssh::run_sftp_write`).
//!
//! 2. **Otherwise**: write locally. Always requires
//!    user consent (filesystem side-effect). Returns
//!    the absolute path on success so the model can
//!    quote it in its reply.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};
use tauri::Manager;
use tokio::fs;

use super::registry::require_str;
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};
use crate::AppState;

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &str {
        "file_write"
    }

    fn description(&self) -> &str {
        "Write a UTF-8 string to a file at the given path, creating parent directories if they don't exist. Use this to save notes, code snippets, or any other text the user asks you to write. Requires user consent. If the calling Bot has a provisioned computer, the file is written to the Bot's VM via SFTP instead of locally."
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
                    "description": "Absolute path, or '~/...' for the home directory. On a Bot's VM, the path is interpreted relative to the VM's filesystem."
                },
                "content": {
                    "type": "string",
                    "description": "The full text content to write to the file."
                }
            },
            "required": ["path", "content"],
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
                "user denied the file_write action".to_string(),
            ));
        }
        let path = require_str(&invocation.arguments, "path")?;
        let content = require_str(&invocation.arguments, "content")?;

        // v2.0 Slice B: route to VM via SFTP if the
        // bot has a computer.
        if let Some(bot_id) = invocation.bot_id.as_deref() {
            if let Some(app) = context.app.as_ref() {
                if let Some(routed) = try_write_on_computer(app, bot_id, path, content).await {
                    return routed;
                }
            }
        }

        let resolved = expand_tilde(path);
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| ToolError::Execution(format!("mkdir failed: {e}")))?;
        }
        fs::write(&resolved, content.as_bytes())
            .await
            .map_err(|e| ToolError::Execution(format!("write failed: {e}")))?;
        Ok(ToolResult::ok(format!(
            "wrote {} bytes to {}",
            content.len(),
            resolved.display()
        )))
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

/// If the given bot has a computer, write `content` to
/// `path` on the VM via SFTP. Returns `None` if the bot
/// has no computer (fall through to local). Returns
/// `Some(Err)` for actual SFTP failures.
async fn try_write_on_computer(
    app: &tauri::AppHandle,
    bot_id: &str,
    path: &str,
    content: &str,
) -> Option<Result<ToolResult, ToolError>> {
    let state = app.state::<AppState>();
    let db = state.db.clone();
    let mgr = state.computer.clone();
    let probe = tokio::task::spawn_blocking({
        let bot_id = bot_id.to_string();
        let db = db.clone();
        move || db.get_computer(&bot_id)
    })
    .await
    .ok()?;
    match probe {
        Ok(Some(r)) if !r.vm_name.is_empty() && r.state == "running" => {}
        Ok(_) => return None,
        Err(e) => {
            return Some(Err(ToolError::Execution(format!(
                "computer lookup failed: {e}"
            ))));
        }
    }
    match mgr.file_write(&db, bot_id, path, content).await {
        Ok(()) => Some(Ok(ToolResult::ok(format!(
            "wrote {} bytes to {} on the Bot's VM",
            content.len(),
            path
        )))),
        Err(crate::computer::ComputerError::PassphraseMissing) => Some(Err(
            ToolError::Execution(
                "computer passphrase not set in Settings; cannot write to the Bot's VM"
                    .into(),
            ),
        )),
        Err(e) => Some(Err(ToolError::Execution(format!("sftp: {e}")))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn expand_tilde_preserves_absolute_paths() {
        // Non-tilde paths pass through. (HOME is set on
        // macOS dev machines, so we use a string that
        // is clearly not starting with ~/ to avoid the
        // branch.)
        let p = expand_tilde("/etc/hosts");
        assert_eq!(p, PathBuf::from("/etc/hosts"));
    }
}
