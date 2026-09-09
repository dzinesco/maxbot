//! `shell_run` tool: run a shell command and return stdout + stderr.
//!
//! Spawns the command via `sh -c` on Unix and `cmd /C` on Windows, with a
//! hard 30-second timeout. The user is asked to confirm before the
//! action runs (consent dialog); refusing returns an error so the
//! model can react.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::registry::{require_str, truncate_for_model};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct ShellRunTool;

#[async_trait]
impl Tool for ShellRunTool {
    fn name(&self) -> &str {
        "shell_run"
    }

    fn description(&self) -> &str {
        "Run a shell command and return its combined stdout + stderr. Hard 30-second timeout. The user is asked to confirm before the action runs. Use for one-off inspections, builds, or quick filesystem operations that don't have a dedicated tool."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to run. Interpreted by `sh -c` on Unix and `cmd /C` on Windows."
                }
            },
            "required": ["command"],
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
                "user denied the shell_run action".to_string(),
            ));
        }
        let command_str = require_str(&invocation.arguments, "command")?;
        let mut cmd = build_shell_command(command_str);
        cmd.kill_on_drop(true);
        let output = match tokio::time::timeout(
            Duration::from_secs(30),
            cmd.output(),
        )
        .await
        {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Err(ToolError::Execution(format!("spawn failed: {e}")));
            }
            Err(_) => {
                return Err(ToolError::Execution(
                    "command exceeded the 30-second timeout".to_string(),
                ));
            }
        };
        let mut body = String::new();
        if !output.stdout.is_empty() {
            body.push_str("--- stdout ---\n");
            body.push_str(&String::from_utf8_lossy(&output.stdout));
        }
        if !output.stderr.is_empty() {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str("--- stderr ---\n");
            body.push_str(&String::from_utf8_lossy(&output.stderr));
        }
        if body.is_empty() {
            body = format!("(no output; exit status {:?})", output.status.code());
        } else {
            body.push_str(&format!("\n--- exit status {:?} ---", output.status.code()));
        }
        if !output.status.success() {
            return Ok(ToolResult::err(truncate_for_model(&body, 6_000)));
        }
        Ok(ToolResult::ok(truncate_for_model(&body, 6_000)))
    }
}

#[cfg(target_os = "windows")]
fn build_shell_command(command: &str) -> Command {
    let mut cmd = Command::new("cmd");
    cmd.args(["/C", command]);
    cmd
}

#[cfg(not(target_os = "windows"))]
fn build_shell_command(command: &str) -> Command {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command);
    cmd
}
