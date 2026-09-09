//! Per-bot filesystem tools: `memory_*`, `scratchpad_*`, `outputs_*`.
//!
//! All seven tools are scoped to the calling bot's directory at
//! `<app_data_dir>/bots/<bot_id>/`. They require:
//!
//! - `ToolInvocation::bot_id` set (the executor does this for bot
//!   runs; chat runs leave it `None`).
//! - `ToolContext::app` set (the executor sets this so the tools
//!   can resolve the app data dir; chat runs leave it `None`).
//!
//! When either is missing, the tool returns a clear error rather
//! than silently operating on the wrong path. `outputs_write` is
//! per-call consent because the resulting file lives in the user's
//! filesystem and may be opened by another app.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::system::require_string;
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

const MISSING_BOT_CONTEXT: &str =
    "this tool only runs in a bot context (no bot_id or app handle attached)";

/// Resolve the calling bot's directory. Errors out cleanly if
/// `bot_id` or the `AppHandle` is missing.
fn resolve_bot_dir(
    invocation: &ToolInvocation,
    context: &ToolContext,
) -> Result<std::path::PathBuf, ToolError> {
    let bot_id = invocation
        .bot_id
        .as_deref()
        .ok_or_else(|| ToolError::Execution(MISSING_BOT_CONTEXT.to_string()))?;
    let app = context
        .app
        .as_ref()
        .ok_or_else(|| ToolError::Execution(MISSING_BOT_CONTEXT.to_string()))?;
    crate::bots::filesystem::bot_dir(app, bot_id).map_err(ToolError::Execution)
}

// ---- memory_* ----

pub struct MemoryReadTool;

#[async_trait]
impl Tool for MemoryReadTool {
    fn name(&self) -> &str {
        "memory_read"
    }

    fn description(&self) -> &str {
        "Read this bot's long-term memory (agents.md). Returns the \
         full contents of the file, or an empty string if it doesn't \
         exist yet. The bot's per-run system prompt already includes \
         the contents; call this when you want to see the raw file \
         (e.g. to trim or rewrite it)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let app = context.app.as_ref().unwrap();
        let bot_id = invocation.bot_id.as_deref().unwrap();
        let s = crate::bots::filesystem::read_agents_md(&dir, "")
            .map_err(ToolError::Execution)?;
        if s.is_empty() {
            // Empty means the file was missing AND the fallback
            // (which we passed as "") was empty. The bot has no
            // memory yet. Surface a helpful hint.
            let _ = (app, bot_id);
            return Ok(ToolResult::ok(String::new()));
        }
        Ok(ToolResult::ok(s))
    }
}

pub struct MemoryWriteTool;

#[async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        "memory_write"
    }

    fn description(&self) -> &str {
        "Overwrite this bot's long-term memory (agents.md). Use \
         sparingly — prefer memory_append to add to what you already \
         know. The next bot run will see the new content in its \
         system prompt."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "New file contents." }
            },
            "required": ["content"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let content = require_string(&invocation.arguments, "content")?.to_string();
        crate::bots::filesystem::write_agents_md(&dir, &content)
            .map_err(ToolError::Execution)?;
        Ok(ToolResult::ok(format!(
            "wrote {} chars to agents.md",
            content.chars().count()
        )))
    }
}

pub struct MemoryAppendTool;

#[async_trait]
impl Tool for MemoryAppendTool {
    fn name(&self) -> &str {
        "memory_append"
    }

    fn description(&self) -> &str {
        "Append to this bot's long-term memory (agents.md), separated \
         by a blank line. Use this to record facts you want to remember \
         across runs — user preferences, project context, decisions, etc."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "Content to append." }
            },
            "required": ["content"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let content = require_string(&invocation.arguments, "content")?.to_string();
        crate::bots::filesystem::append_agents_md(&dir, &content)
            .map_err(ToolError::Execution)?;
        Ok(ToolResult::ok(format!(
            "appended {} chars to agents.md",
            content.chars().count()
        )))
    }
}

// ---- scratchpad_* ----

pub struct ScratchpadReadTool;

#[async_trait]
impl Tool for ScratchpadReadTool {
    fn name(&self) -> &str {
        "scratchpad_read"
    }

    fn description(&self) -> &str {
        "Read this bot's working notes (scratchpad.md). The scratchpad \
         is NOT included in the per-run system prompt — use this when \
         you want to recall earlier working notes from prior runs. \
         Returns an empty string if the scratchpad is empty or absent."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let s = crate::bots::filesystem::read_scratchpad(&dir).map_err(ToolError::Execution)?;
        Ok(ToolResult::ok(s))
    }
}

pub struct ScratchpadWriteTool;

#[async_trait]
impl Tool for ScratchpadWriteTool {
    fn name(&self) -> &str {
        "scratchpad_write"
    }

    fn description(&self) -> &str {
        "Overwrite this bot's working notes (scratchpad.md). Prefer \
         scratchpad_append when adding to existing notes."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "New file contents." }
            },
            "required": ["content"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let content = require_string(&invocation.arguments, "content")?.to_string();
        crate::bots::filesystem::write_scratchpad(&dir, &content)
            .map_err(ToolError::Execution)?;
        Ok(ToolResult::ok(format!(
            "wrote {} chars to scratchpad.md",
            content.chars().count()
        )))
    }
}

pub struct ScratchpadAppendTool;

#[async_trait]
impl Tool for ScratchpadAppendTool {
    fn name(&self) -> &str {
        "scratchpad_append"
    }

    fn description(&self) -> &str {
        "Append to this bot's working notes (scratchpad.md), separated \
         by a blank line."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "Content to append." }
            },
            "required": ["content"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let content = require_string(&invocation.arguments, "content")?.to_string();
        crate::bots::filesystem::append_scratchpad(&dir, &content)
            .map_err(ToolError::Execution)?;
        Ok(ToolResult::ok(format!(
            "appended {} chars to scratchpad.md",
            content.chars().count()
        )))
    }
}

// ---- outputs_* ----

pub struct OutputsListTool;

#[async_trait]
impl Tool for OutputsListTool {
    fn name(&self) -> &str {
        "outputs_list"
    }

    fn description(&self) -> &str {
        "List the files in this bot's outputs/ directory, with sizes. \
         Returns '(no outputs)' if the directory is empty or absent."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let entries =
            crate::bots::filesystem::list_outputs(&dir).map_err(ToolError::Execution)?;
        if entries.is_empty() {
            return Ok(ToolResult::ok("(no outputs)".to_string()));
        }
        let body = entries
            .iter()
            .map(|(name, size)| format!("- {} ({} bytes)", name, size))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ToolResult::ok(body))
    }
}

pub struct OutputsWriteTool;

#[async_trait]
impl Tool for OutputsWriteTool {
    fn name(&self) -> &str {
        "outputs_write"
    }

    fn description(&self) -> &str {
        "Write a file to this bot's outputs/ directory. The filename is \
         sanitized (alphanumerics, '.', '-', '_', ' ', '()!,[]' only, no \
         path separators) so it cannot escape the outputs directory. \
         Per-call consent because the file ends up in the user's \
         filesystem and may be opened by other apps."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "description": "File name within outputs/ (e.g. 'report.md'). No path separators."
                },
                "content": {
                    "type": "string",
                    "description": "File contents (UTF-8 text)."
                }
            },
            "required": ["filename", "content"],
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
                "user denied the outputs_write action".to_string(),
            ));
        }
        let dir = resolve_bot_dir(&invocation, &context)?;
        let filename = require_string(&invocation.arguments, "filename")?.to_string();
        let content = require_string(&invocation.arguments, "content")?.to_string();
        let path = crate::bots::filesystem::write_output(&dir, &filename, content.as_bytes())
            .map_err(ToolError::Execution)?;
        Ok(ToolResult::ok(format!(
            "wrote {} chars to {}",
            content.chars().count(),
            path.file_name().and_then(|s| s.to_str()).unwrap_or("?")
        )))
    }
}

pub struct OutputsReadTool;

#[async_trait]
impl Tool for OutputsReadTool {
    fn name(&self) -> &str {
        "outputs_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file in this bot's outputs/ directory. \
         Use outputs_list to discover filenames. Returns the file as a \
         UTF-8 string (best-effort: non-UTF-8 bytes are surfaced as an \
         error)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filename": {
                    "type": "string",
                    "description": "File name within outputs/."
                }
            },
            "required": ["filename"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let dir = resolve_bot_dir(&invocation, &context)?;
        let filename = require_string(&invocation.arguments, "filename")?.to_string();
        let bytes =
            crate::bots::filesystem::read_output(&dir, &filename).map_err(ToolError::Execution)?;
        let s = String::from_utf8(bytes)
            .map_err(|e| ToolError::Execution(format!("file is not UTF-8: {e}")))?;
        // Truncate to a sane size so we don't blow context.
        if s.len() > 64_000 {
            return Ok(ToolResult::ok(format!(
                "{}\n\n[truncated to 64K; file is larger]",
                &s[..64_000]
            )));
        }
        Ok(ToolResult::ok(s))
    }
}
