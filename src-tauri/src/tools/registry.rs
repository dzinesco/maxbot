//! Central tool registry. Owns the concrete tool implementations and
//! exposes:
//! - `definitions()` — array of `ToolDefinition`s for the chat request
//! - `execute()` — invoke a tool by name, returning a result or error

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::llm::provider::ToolDefinition;

use super::apple_script::AppleScriptRunTool;
use super::app_launcher::{AppListTool, AppOpenTool};
use super::bot_fs::{
    MemoryAppendTool, MemoryReadTool, MemoryWriteTool, OutputsListTool, OutputsReadTool,
    OutputsWriteTool, ScratchpadAppendTool, ScratchpadReadTool, ScratchpadWriteTool,
};
use super::browsers::{
    ChromeCurrentUrlTool, ChromeExecJsTool, ChromeOpenTool, ChromeTabsTool, SafariCurrentUrlTool,
    SafariExecJsTool, SafariOpenTool, SafariTabsTool,
};
use super::calendar::{CalendarCreateEventTool, CalendarTodayTool, CalendarWeekTool};
use super::clipboard::{ClipboardReadTool, ClipboardWriteTool};
use super::coding::{GrokPromptTool, GrokSessionStatusTool};
use super::ego_browser::EgoBrowserTool;
use super::file_read::FileReadTool;
use super::file_write::FileWriteTool;
use super::mail::{MailDraftTool, MailInboxTool, MailSearchTool, MailSendTool};
use super::notes::{NotesCreateTool, NotesReadTool, NotesSearchTool};
use super::reminders::{RemindersAddTool, RemindersCompleteTool, RemindersListTool};
use super::run_skill::RunSkillTool;
use super::shell_run::ShellRunTool;
use super::system::{
    SystemDarkModeGetTool, SystemDarkModeSetTool, SystemFrontAppTool, SystemNotifyTool,
    SystemVolumeGetTool, SystemVolumeSetTool,
};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};
use super::tts::{TtsSpeakTool, TtsStopTool};
use super::web_fetch::WebFetchTool;
use super::web_search::WebSearchTool;
use super::window::{WindowFocusTool, WindowListTool};

pub struct ToolRegistry {
    by_name: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Build a registry with the default set of tools. v0.4 adds the
    /// AppleScript Computer Use foundation: the escape hatch
    /// (`apple_script_run`), clipboard, and the system tools (notify,
    /// volume, dark mode, frontmost app). Per-app Computer Use tools
    /// (mail, calendar, safari, etc.) are added in their own slices.
    ///
    /// `extra` lets callers add dynamic tools (e.g. MCP-backed) on top
    /// of the built-in defaults.
    pub fn default_with_extras(extra: Vec<Arc<dyn Tool>>) -> Self {
        let mut tools: Vec<Arc<dyn Tool>> = vec![
            // v0.2 — web + filesystem + shell
            Arc::new(WebFetchTool),
            Arc::new(WebSearchTool),
            Arc::new(FileReadTool),
            Arc::new(FileWriteTool),
            Arc::new(ShellRunTool),
            // v0.4.0 — AppleScript Computer Use foundation
            Arc::new(AppleScriptRunTool),
            Arc::new(ClipboardReadTool),
            Arc::new(ClipboardWriteTool),
            Arc::new(SystemNotifyTool),
            Arc::new(SystemVolumeGetTool),
            Arc::new(SystemVolumeSetTool),
            Arc::new(SystemDarkModeGetTool),
            Arc::new(SystemDarkModeSetTool),
            Arc::new(SystemFrontAppTool),
            // v0.4.1 — Mail
            Arc::new(MailInboxTool),
            Arc::new(MailSearchTool),
            Arc::new(MailSendTool),
            Arc::new(MailDraftTool),
            // v0.4.2 — Calendar + Reminders + Notes
            Arc::new(CalendarTodayTool),
            Arc::new(CalendarWeekTool),
            Arc::new(CalendarCreateEventTool),
            Arc::new(RemindersListTool),
            Arc::new(RemindersAddTool),
            Arc::new(RemindersCompleteTool),
            Arc::new(NotesSearchTool),
            Arc::new(NotesReadTool),
            Arc::new(NotesCreateTool),
            // v0.4.3 — Browsers + Windows
            Arc::new(SafariOpenTool),
            Arc::new(SafariCurrentUrlTool),
            Arc::new(SafariExecJsTool),
            Arc::new(SafariTabsTool),
            Arc::new(ChromeOpenTool),
            Arc::new(ChromeCurrentUrlTool),
            Arc::new(ChromeExecJsTool),
            Arc::new(ChromeTabsTool),
            Arc::new(WindowListTool),
            Arc::new(WindowFocusTool),
            // v0.6.1 — Text-to-Speech via macOS `say`
            Arc::new(TtsSpeakTool),
            Arc::new(TtsStopTool),
            // v0.6.7 — App launcher (open any macOS app by name)
            Arc::new(AppOpenTool),
            Arc::new(AppListTool),
            // v0.6.9 — ego-browser integration: run a JS script in
            // the ego (lite) embedded Node.js runtime, which drives a
            // real Chromium browser the agent controls. Replaces the
            // AppleScript safari_*/chrome_* path for the common
            // "drive a real browser" workflow. Per-call consent.
            Arc::new(EgoBrowserTool),
            // v0.7.0 — Grok Build session: spawn a long-lived
            // `grok agent stdio` subprocess and speak ACP /
            // JSON-RPC 2.0 over its stdin/stdout. Multi-turn:
            // sessionId is persisted to SQLite so a relaunch
            // resumes the same conversation. Per-call consent
            // because the agent can read/write files and run
            // shell commands in its cwd.
            Arc::new(GrokPromptTool),
            Arc::new(GrokSessionStatusTool),
            // v0.6.8 — Per-bot filesystem (memory / scratchpad / outputs)
            Arc::new(MemoryReadTool),
            Arc::new(MemoryWriteTool),
            Arc::new(MemoryAppendTool),
            Arc::new(ScratchpadReadTool),
            Arc::new(ScratchpadWriteTool),
            Arc::new(ScratchpadAppendTool),
            Arc::new(OutputsListTool),
            Arc::new(OutputsReadTool),
            Arc::new(OutputsWriteTool),
            // v2.2.0 — run a saved Skill from the agent loop.
            // The model calls this with a `skill_id` and the
            // Skill's `inputs`; the registry handles
            // `output_var` chaining and per-step error
            // handling. Consent is implicit because the bot's
            // allowlist already gates tool use.
            Arc::new(RunSkillTool),
        ];
        tools.extend(extra);
        let mut by_name: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        for t in tools {
            by_name.insert(t.name().to_string(), t);
        }
        Self { by_name }
    }

    /// All tool declarations to send to the model.
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut out: Vec<ToolDefinition> = self
            .by_name
            .values()
            .map(|t| t.definition())
            .collect();
        // Sort for stable ordering across builds; keeps model
        // tool-choice patterns reproducible.
        out.sort_by(|a, b| a.function.name.cmp(&b.function.name));
        out
    }

    pub fn requires_consent(&self, name: &str) -> bool {
        self.by_name.get(name).map(|t| t.requires_consent()).unwrap_or(false)
    }

    /// Run a tool by name. Validates the name; the tool itself is
    /// responsible for parsing its `arguments` JSON.
    pub async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let tool = self
            .by_name
            .get(invocation.name.as_str())
            .ok_or_else(|| ToolError::UnknownTool(invocation.name.clone()))?;
        tool.execute(invocation, context).await
    }

    /// Return a new registry with only the named tools. Unknown names
    /// are silently dropped — this is how a bot's `allowed_tools` list
    /// gets enforced. Consent requirements are taken from the
    /// underlying tool unchanged.
    pub fn filtered(&self, allowed: &[String]) -> ToolRegistry {
        let mut by_name: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        for name in allowed {
            if let Some(tool) = self.by_name.get(name.as_str()) {
                by_name.insert(tool.name().to_string(), tool.clone());
            }
        }
        ToolRegistry { by_name }
    }
}

/// Convenience for tests: a totally empty registry.
impl Default for ToolRegistry {
    fn default() -> Self {
        Self {
            by_name: HashMap::new(),
        }
    }
}

/// Pull a `String` field out of a tool invocation's arguments, returning
/// a clear error if it's missing or the wrong type.
pub fn require_str<'a>(
    args: &'a Value,
    field: &str,
) -> Result<&'a str, ToolError> {
    let value = args
        .get(field)
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing field: {field}")))?;
    value
        .as_str()
        .ok_or_else(|| ToolError::InvalidArguments(format!("field {field} is not a string")))
}

/// Truncate a string to `max_chars`, appending a `... [truncated]` marker
/// if anything was cut. Keeps tool outputs from blowing the model's
/// context window.
pub fn truncate_for_model(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }
    let mut out: String = input.chars().take(max_chars).collect();
    out.push_str("\n\n… [truncated]");
    out
}
