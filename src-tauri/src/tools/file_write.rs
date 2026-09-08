//! `file_write` tool: write a string to disk, creating parent dirs.
//!
//! Always requires user consent (filesystem side-effect). Returns the
//! absolute path on success so the model can quote it in its reply.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

use super::registry::require_str;
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct FileWriteTool;

#[async_trait]
impl Tool for FileWriteTool {
    fn name(&self) -> &'static str {
        "file_write"
    }

    fn description(&self) -> &'static str {
        "Write a UTF-8 string to a file at the given path, creating parent directories if they don't exist. Use this to save notes, code snippets, or any other text the user asks you to write. Requires user consent."
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
                    "description": "Absolute path, or '~/...' for the home directory."
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
