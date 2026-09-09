//! Central tool registry. Owns the concrete tool implementations and
//! exposes:
//! - `definitions()` — array of `ToolDefinition`s for the chat request
//! - `execute()` — invoke a tool by name, returning a result or error

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::llm::provider::ToolDefinition;

use super::apple_script::AppleScriptRunTool;
use super::calendar::{CalendarCreateEventTool, CalendarTodayTool, CalendarWeekTool};
use super::clipboard::{ClipboardReadTool, ClipboardWriteTool};
use super::file_read::FileReadTool;
use super::file_write::FileWriteTool;
use super::mail::{MailDraftTool, MailInboxTool, MailSearchTool, MailSendTool};
use super::notes::{NotesCreateTool, NotesReadTool, NotesSearchTool};
use super::reminders::{RemindersAddTool, RemindersCompleteTool, RemindersListTool};
use super::shell_run::ShellRunTool;
use super::system::{
    SystemDarkModeGetTool, SystemDarkModeSetTool, SystemFrontAppTool, SystemNotifyTool,
    SystemVolumeGetTool, SystemVolumeSetTool,
};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};
use super::web_fetch::WebFetchTool;
use super::web_search::WebSearchTool;

pub struct ToolRegistry {
    by_name: HashMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    /// Build a registry with the default set of tools. v0.4 adds the
    /// AppleScript Computer Use foundation: the escape hatch
    /// (`apple_script_run`), clipboard, and the system tools (notify,
    /// volume, dark mode, frontmost app). Per-app Computer Use tools
    /// (mail, calendar, safari, etc.) are added in their own slices.
    pub fn default_set() -> Self {
        let tools: Vec<Arc<dyn Tool>> = vec![
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
        ];
        let mut by_name: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();
        for t in tools {
            by_name.insert(t.name(), t);
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
        let mut by_name: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();
        for name in allowed {
            if let Some(tool) = self.by_name.get(name.as_str()) {
                by_name.insert(tool.name(), tool.clone());
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
