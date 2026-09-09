//! `grok_prompt` — send a prompt to the running `grok agent stdio`
//! session and return the assistant's reply.
//!
//! `grok_prompt` is the long-lived-session sibling of the old
//! one-shot `grok --single` flow that we considered earlier. It
//! shares a single `grok agent stdio` subprocess for the lifetime
//! of the app, with a persisted session id so a relaunch resumes
//! the same conversation. Multi-turn is "just call the tool
//! again" — the agent keeps the prior context.
//!
//! Per-call consent because the agent can read/write files in
//! its working directory and run shell commands via its own
//! tools.
//!
//! `grok_session_status` (no consent) reports the live state of
//! the session so the UI can show "connected" / "not started" /
//! "error: …" without surfacing the subprocess machinery to the
//! model.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::registry::truncate_for_model;
use super::system::require_string;
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

/// Per-prompt hard cap (in addition to the 45s response timeout
/// inside the session). Generous because the agent may take
/// real time on a coding task; bounded so a hung model surfaces
/// as a tool error instead of blocking the chat turn.
const PROMPT_HARD_TIMEOUT: Duration = Duration::from_secs(300);

pub struct GrokPromptTool;

#[async_trait]
impl Tool for GrokPromptTool {
    fn name(&self) -> &str {
        "grok_prompt"
    }

    fn description(&self) -> &str {
        "Send a prompt to the running `grok agent stdio` session and \
         return the assistant's reply. The session is multi-turn: \
         calling this tool again in the same conversation continues \
         the same agent context. The agent has its own tools (read \
         / write files, run shell commands) and operates in its \
         configured working directory. Per-call consent because \
         the agent can read / write files in its cwd and run \
         commands. First call in a session pays the ~3-5s bring-up \
         cost (subprocess spawn + ACP handshake). Returns the \
         assistant's final message; streamed chunks are collected \
         but not surfaced individually."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The prompt to send. Free-form; passed to the grok agent verbatim. Long prompts are fine; the session is multi-turn so prefer follow-up calls over cramming everything into one prompt."
                }
            },
            "required": ["prompt"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Validate first so a malformed call doesn't get a
        // consent dialog.
        let prompt = require_string(&invocation.arguments, "prompt")?;
        if prompt.trim().is_empty() {
            return Err(ToolError::InvalidArguments("prompt is empty".to_string()));
        }
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the grok_prompt action".to_string(),
            ));
        }
        // Bring up (or reuse) the session.
        let app = context
            .app
            .clone()
            .ok_or_else(|| ToolError::Execution("missing app handle".to_string()))?;
        let session = crate::grok_build::get_or_init_session(&app)
            .await
            .map_err(ToolError::Execution)?;
        // Send the prompt under the hard cap.
        let reply = match tokio::time::timeout(PROMPT_HARD_TIMEOUT, session.prompt(&prompt)).await {
            Ok(Ok(text)) => text,
            Ok(Err(e)) => {
                return Ok(ToolResult::err(format!("grok_prompt failed: {e}")));
            }
            Err(_) => {
                return Ok(ToolResult::err(format!(
                    "grok_prompt timed out after {PROMPT_HARD_TIMEOUT:?}"
                )));
            }
        };
        if reply.is_empty() {
            // The agent produced no chunks. Surface a clear
            // empty result so the model doesn't loop trying to
            // extract text from "".
            return Ok(ToolResult::ok(
                "(grok returned an empty reply — the agent may have \
                 errored or used only tool calls without a final \
                 text message.)",
            ));
        }
        // The session id is useful in the result so the model
        // can see "still the same conversation".
        let sid = session.session_id().unwrap_or_else(|| "(none)".to_string());
        let body = format!("[session {sid}]\n\n{reply}");
        Ok(ToolResult::ok(truncate_for_model(&body, 24_000)))
    }
}

pub struct GrokSessionStatusTool;

#[async_trait]
impl Tool for GrokSessionStatusTool {
    fn name(&self) -> &str {
        "grok_session_status"
    }

    fn description(&self) -> &str {
        "Report the current state of the `grok agent stdio` session \
         backing `grok_prompt`. Returns 'not_started' before the \
         first prompt, or 'live' with the session id and binary \
         path once it's been brought up. No consent required \
         (read-only)."
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
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if !crate::grok_build::is_initialized() {
            return Ok(ToolResult::ok(
                "status: not_started\nfirst grok_prompt call will bring up the session.",
            ));
        }
        let app = context
            .app
            .clone()
            .ok_or_else(|| ToolError::Execution("missing app handle".to_string()))?;
        // Already initialized — fetch the cached handle and read
        // its identity.
        let session = crate::grok_build::get_or_init_session(&app)
            .await
            .map_err(ToolError::Execution)?;
        let body = format!(
            "status: live\nbinary: {}\nmodel: {}\nsession_id: {}",
            session.binary_path(),
            session.model_alias(),
            session.session_id().unwrap_or_else(|| "(none)".to_string()),
        );
        Ok(ToolResult::ok(body))
    }
}
