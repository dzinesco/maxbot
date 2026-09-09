//! `file_read` tool: read a UTF-8 file with a character cap.
//!
//! Paths are resolved relative to the user's home directory if they start
//! with `~/`; everything else is treated as absolute. Hard 1 MB byte cap
//! and 12,000-character cap protect both the model and the user from
//! accidental dumps of huge logs or binaries.

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::fs;

use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct FileReadTool;

#[async_trait]
impl Tool for FileReadTool {
    fn name(&self) -> &str {
        "file_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a UTF-8 text file at the given path. Use this to inspect source files, configs, or small logs. Paths starting with '~' are expanded to the user's home directory. Files larger than 1 MB or 12,000 characters are truncated with a marker."
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
                    "description": "Absolute path, or a '~/...' path that will be expanded to the home directory."
                }
            },
            "required": ["path"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let path = require_str(&invocation.arguments, "path")?;
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
