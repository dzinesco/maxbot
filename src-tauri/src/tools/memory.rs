//! v2.5.0 — `MemorySearchTool` / `MemoryRememberTool` /
//! `MemoryForgetTool`.
//!
//! These three tools let a Bot's agent loop search, write, and
//! delete entries in the per-Bot memory store. Memory is JSONL
//! on the Bot's VM (see `crate::memory::store`) — SFTP writes go
//! through the existing `SshPool`, so we don't add a new SSH
//! subsystem.
//!
//! `requires_consent = false` for all three: the Bot is
//! autonomous inside its agent loop, and the bot's
//! `allowed_tools` allowlist already gates which tools it can
//! reach. A user who didn't want a bot mutating its own memory
//! simply leaves these tools out of the allowlist.
//!
//! `history` kind is intentionally NOT exposed: history is
//! auto-written by the executor at the end of a successful
//! turn (see `bots::executor::run_bot_once`). Letting the LLM
//! fabricate history rows would defeat the audit value of
//! the auto-summary.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tauri::Manager;

use crate::memory::store;
use crate::memory::{MemEntry, MemKind};
use crate::tools::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};
use crate::AppState;

// ---- MemorySearchTool -------------------------------------------------

pub struct MemorySearchTool;

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search the current Bot's persistent memory (facts, \
         preferences, and conversation-history summaries) for \
         entries matching the query. Returns up to `top_k` \
         matches ranked by relevance (exact-key > key-contains > \
         content-contains). Use this to recall user preferences, \
         prior decisions, or earlier conversation topics before \
         responding. Memory lives on the Bot's VM and survives \
         app restarts."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Free-text query. Case-insensitive \
                        substring match against key, content, and \
                        history summary. Empty string returns the most \
                        recent entries."
                },
                "top_k": {
                    "type": "integer",
                    "description": "Maximum number of entries to return. \
                        Defaults to 5. Cap at 20 to keep the response \
                        compact.",
                    "minimum": 1,
                    "maximum": 20,
                    "default": 5
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let bot_id = invocation.bot_id.clone().ok_or_else(|| {
            ToolError::InvalidArguments(
                "memory_search requires a bot context (no bot_id)".to_string(),
            )
        })?;
        let query = invocation
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidArguments(
                    "memory_search: missing required field 'query'".to_string(),
                )
            })?
            .to_string();
        let top_k = invocation
            .arguments
            .get("top_k")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).min(20))
            .unwrap_or(5);

        let app = _context
            .app
            .clone()
            .ok_or_else(|| ToolError::Execution("memory_search: no app handle".to_string()))?;
        let state: tauri::State<Arc<AppState>> = app.state();
        let pool = state.computer.ssh_pool();
        let entries = store::search(&*pool, &bot_id, &query, top_k).await;
        // Return a compact, model-friendly summary. The full
        // MemEntry JSON is fine — Claude/Opus/Grok all eat JSON
        // results happily.
        let payload: Vec<serde_json::Value> = entries
            .into_iter()
            .map(|e| {
                serde_json::json!({
                    "kind": e.kind.as_str(),
                    "key": e.key,
                    "content": e.content,
                    "created_at": e.created_at,
                })
            })
            .collect();
        let body = serde_json::to_string(&payload).unwrap_or_else(|_| "[]".to_string());
        Ok(ToolResult::ok(body))
    }
}

// ---- MemoryRememberTool -----------------------------------------------

pub struct MemoryRememberTool;

#[async_trait]
impl Tool for MemoryRememberTool {
    fn name(&self) -> &'static str {
        "memory_remember"
    }

    fn description(&self) -> &str {
        "Save a fact or preference to the current Bot's persistent \
         memory. Use this when the user states a durable preference \
         (e.g. 'I prefer dark mode', 'My timezone is PT') or a \
         stable fact about themselves or their world. \
         `kind` must be 'fact' or 'preference' — `history` is \
         auto-written by the system and not exposed here. The \
         entry survives app restarts and is recalled in future \
         turns via memory_search."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["fact", "preference"],
                    "description": "What kind of memory entry. `fact` is \
                        a stable piece of information about the user or \
                        their world (name, project name, etc). \
                        `preference` is a stated user preference (formatting, \
                        defaults, style)."
                },
                "key": {
                    "type": "string",
                    "description": "Short snake_case identifier (e.g. \
                        'user_name', 'preferred_unit_system'). This is \
                        the lookup handle; an existing entry with the \
                        same key+kind is replaced."
                },
                "content": {
                    "type": "string",
                    "description": "The value to remember. Free text. \
                        Keep it short — this is surfaced verbatim in \
                        future prompts."
                }
            },
            "required": ["kind", "key", "content"]
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let bot_id = invocation.bot_id.clone().ok_or_else(|| {
            ToolError::InvalidArguments(
                "memory_remember requires a bot context (no bot_id)".to_string(),
            )
        })?;
        let kind_str = invocation
            .arguments
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidArguments(
                    "memory_remember: missing required field 'kind'".to_string(),
                )
            })?;
        let kind = match kind_str {
            "fact" => MemKind::Fact,
            "preference" => MemKind::Preference,
            other => {
                return Ok(ToolResult::err(format!(
                    "memory_remember: invalid kind '{other}'; \
                     expected 'fact' or 'preference'. \
                     'history' is auto-written and not exposed to the model."
                )));
            }
        };
        let key = invocation
            .arguments
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidArguments(
                    "memory_remember: missing required field 'key'".to_string(),
                )
            })?
            .to_string();
        let content = invocation
            .arguments
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidArguments(
                    "memory_remember: missing required field 'content'".to_string(),
                )
            })?
            .to_string();
        if key.trim().is_empty() {
            return Ok(ToolResult::err(
                "memory_remember: 'key' must be non-empty".to_string(),
            ));
        }
        if content.trim().is_empty() {
            return Ok(ToolResult::err(
                "memory_remember: 'content' must be non-empty".to_string(),
            ));
        }

        let app = _context
            .app
            .clone()
            .ok_or_else(|| ToolError::Execution("memory_remember: no app handle".to_string()))?;
        let state: tauri::State<Arc<AppState>> = app.state();
        let pool = state.computer.ssh_pool();
        let entry = MemEntry::new(kind, key, content);
        match store::append(&*pool, &bot_id, kind, entry).await {
            Ok(()) => Ok(ToolResult::ok(format!(
                "remembered {} '{}'",
                kind.as_str(),
                invocation
                    .arguments
                    .get("key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ))),
            Err(e) => Ok(ToolResult::err(format!("memory_remember: {e}"))),
        }
    }
}

// ---- MemoryForgetTool -------------------------------------------------

pub struct MemoryForgetTool;

#[async_trait]
impl Tool for MemoryForgetTool {
    fn name(&self) -> &'static str {
        "memory_forget"
    }

    fn description(&self) -> &str {
        "Delete a fact or preference from the current Bot's memory \
         by its key. Searches both `fact` and `preference` kinds; \
         the first match is removed. Returns whether anything was \
         deleted. Use this when the user asks to forget something \
         or when a prior fact has been corrected and the new value \
         would conflict."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The key of the memory entry to delete. \
                        Searches facts and preferences. Empty key is a no-op."
                }
            },
            "required": ["key"]
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let bot_id = invocation.bot_id.clone().ok_or_else(|| {
            ToolError::InvalidArguments(
                "memory_forget requires a bot context (no bot_id)".to_string(),
            )
        })?;
        let key = invocation
            .arguments
            .get("key")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidArguments(
                    "memory_forget: missing required field 'key'".to_string(),
                )
            })?
            .to_string();
        if key.trim().is_empty() {
            return Ok(ToolResult::ok("memory_forget: empty key, nothing to do".to_string()));
        }
        let app = _context
            .app
            .clone()
            .ok_or_else(|| ToolError::Execution("memory_forget: no app handle".to_string()))?;
        let state: tauri::State<Arc<AppState>> = app.state();
        let pool = state.computer.ssh_pool();
        // Try fact first, then preference. Whichever matches is
        // the one we delete. If both have the same key (rare —
        // it'd mean the user has the same key in two kinds),
        // delete both.
        let mut deleted = 0usize;
        for kind in [MemKind::Fact, MemKind::Preference] {
            match store::delete(&*pool, &bot_id, kind, &key).await {
                Ok(true) => deleted += 1,
                Ok(false) => {}
                Err(e) => {
                    return Ok(ToolResult::err(format!(
                        "memory_forget: {e}"
                    )));
                }
            }
        }
        Ok(ToolResult::ok(format!(
            "memory_forget: deleted {deleted} entry/entries for key '{key}'"
        )))
    }
}
