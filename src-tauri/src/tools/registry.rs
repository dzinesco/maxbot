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
use super::memory::{MemoryForgetTool, MemoryRememberTool, MemorySearchTool};
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
use super::vm_computer_use::{VmBrowserOpenTool, VmComputerUseTool};
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
            // v2.5.0 — Persistent per-Bot memory.
            // JSONL on the Bot's VM, SFTP via the existing
            // SshPool. `history` is auto-written by the
            // executor and intentionally not exposed here.
            Arc::new(MemorySearchTool),
            Arc::new(MemoryRememberTool),
            Arc::new(MemoryForgetTool),
            // v3.2.0 — in-VM Computer Use. The Bot's
            // per-Bot Linux VM is the new default Computer
            // Use target. `vm_computer_use` shells out to
            // `chromium-browser`, `xdotool`, and `scrot` over
            // the existing SshPool. The `bot.computer_use`
            // field on the `Bot` row controls whether this
            // tool (vm) or `ego_browser` (mac) is in the
            // per-Bot tool list — see the executor's
            // `tool_list_for_bot`. Per-call consent because
            // the script can navigate, click, and type.
            Arc::new(VmComputerUseTool),
            // v3.2.0 — `vm_browser_open` is the skill-
            // replay shape of "open a URL in the VM." The
            // recorder rewrites a single-`open_url`
            // `vm_computer_use` step to this thinner
            // primitive (`{ url }` instead of a script
            // string); this tool is the replay side. The
            // LLM can also call it directly when it just
            // wants "navigate and snapshot."
            Arc::new(VmBrowserOpenTool),
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

    /// v3.2.0 — return a new registry with the Computer Use
    /// tools (currently `vm_computer_use`, `vm_browser_open`,
    /// and `ego_browser`) filtered by the bot's
    /// `computer_use` setting. The `allowed` list is the
    /// bot's `allowed_tools` allowlist — this function
    /// re-applies it AFTER swapping the Computer Use tool in
    /// or out, so a bot that doesn't have `ego_browser` in
    /// its allowlist still doesn't see `ego_browser` even
    /// when `bot.computer_use == "mac"`.
    ///
    /// The three branches:
    ///   - `"vm"` (default) — `vm_computer_use` and
    ///     `vm_browser_open` ARE in the list (if the
    ///     allowlist permits); `ego_browser` is removed.
    ///     The new in-VM path.
    ///   - `"mac"` — `ego_browser` IS in the list (if the
    ///     allowlist permits); `vm_computer_use` and
    ///     `vm_browser_open` are removed. The legacy
    ///     v3.1.0 path.
    ///   - `"mac-with-approval"` — same as `"mac"`, but
    ///     the caller (`registry_for`) is expected to wrap
    ///     each `ego_browser` call in an approval gate.
    ///     Today that gate is a no-op placeholder — the
    ///     real approval flow lands in v3.4.0.
    pub fn computer_use_filtered(
        &self,
        allowed: &[String],
        computer_use: &str,
    ) -> ToolRegistry {
        // First apply the allowlist as normal.
        let base = self.filtered(allowed);
        let mut by_name = base.by_name;
        match computer_use {
            "vm" => {
                // Drop ego_browser if the user happened to
                // have left it in the allowlist from a
                // pre-v3.2.0 setup. The VM path is the
                // exclusive Computer Use path.
                by_name.remove("ego_browser");
            }
            "mac" | "mac-with-approval" => {
                // Drop both VM tools. The Mac / AppleScript
                // path is the exclusive Computer Use path.
                by_name.remove("vm_computer_use");
                by_name.remove("vm_browser_open");
            }
            _ => {
                // Unknown / empty: behave like "vm" so a
                // typo can't accidentally land the model
                // on the Mac path.
                by_name.remove("ego_browser");
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ego_browser::EgoBrowserTool;
    use crate::tools::vm_computer_use::{VmBrowserOpenTool, VmComputerUseTool};

    /// Build a registry with all three Computer Use tools
    /// plus a couple of non-CU tools, mirroring the
    /// production `ToolRegistry::default_with_extras` set
    /// (without the long tail of unrelated tools).
    fn fixture_registry() -> ToolRegistry {
        let mut by_name: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        by_name.insert(VmComputerUseTool.name().to_string(), Arc::new(VmComputerUseTool));
        by_name.insert(VmBrowserOpenTool.name().to_string(), Arc::new(VmBrowserOpenTool));
        by_name.insert(EgoBrowserTool.name().to_string(), Arc::new(EgoBrowserTool));
        // A non-CU tool, to confirm the filter leaves it
        // alone.
        by_name.insert(
            "file_write".to_string(),
            Arc::new(crate::tools::file_write::FileWriteTool),
        );
        ToolRegistry { by_name }
    }

    /// v3.2.0 — `computer_use = "vm"` keeps the VM tools
    /// in the registry and drops `ego_browser`, even when
    /// the allowlist has it (a v3.1.0 carry-over).
    #[test]
    fn computer_use_vm_drops_ego_browser() {
        let r = fixture_registry();
        let allowed = vec![
            "vm_computer_use".to_string(),
            "vm_browser_open".to_string(),
            "ego_browser".to_string(),
            "file_write".to_string(),
        ];
        let out = r.computer_use_filtered(&allowed, "vm");
        assert!(out.by_name.contains_key("vm_computer_use"));
        assert!(out.by_name.contains_key("vm_browser_open"));
        assert!(!out.by_name.contains_key("ego_browser"));
        assert!(out.by_name.contains_key("file_write"));
    }

    /// `computer_use = "mac"` keeps `ego_browser` and
    /// drops the VM tools.
    #[test]
    fn computer_use_mac_drops_vm_tools() {
        let r = fixture_registry();
        let allowed = vec![
            "vm_computer_use".to_string(),
            "vm_browser_open".to_string(),
            "ego_browser".to_string(),
        ];
        let out = r.computer_use_filtered(&allowed, "mac");
        assert!(!out.by_name.contains_key("vm_computer_use"));
        assert!(!out.by_name.contains_key("vm_browser_open"));
        assert!(out.by_name.contains_key("ego_browser"));
    }

    /// `computer_use = "mac-with-approval"` behaves the
    /// same as `"mac"` today (the approval gate is a
    /// no-op until v3.4.0).
    #[test]
    fn computer_use_mac_with_approval_also_drops_vm_tools() {
        let r = fixture_registry();
        let allowed = vec![
            "vm_computer_use".to_string(),
            "vm_browser_open".to_string(),
            "ego_browser".to_string(),
        ];
        let out = r.computer_use_filtered(&allowed, "mac-with-approval");
        assert!(!out.by_name.contains_key("vm_computer_use"));
        assert!(!out.by_name.contains_key("vm_browser_open"));
        assert!(out.by_name.contains_key("ego_browser"));
    }

    /// Unknown / empty `computer_use` falls back to
    /// `"vm"` (matches `parse_computer_use`). A typo
    /// can't silently disable the in-VM path.
    #[test]
    fn computer_use_unknown_falls_back_to_vm() {
        let r = fixture_registry();
        let allowed = vec!["ego_browser".to_string(), "vm_computer_use".to_string()];
        let out = r.computer_use_filtered(&allowed, "VMM");
        assert!(!out.by_name.contains_key("ego_browser"));
        assert!(out.by_name.contains_key("vm_computer_use"));
    }

    /// The allowlist is the binding constraint: a bot
    /// without `vm_computer_use` in its allowlist doesn't
    /// see it, even when `computer_use == "vm"`.
    #[test]
    fn computer_use_vm_respects_allowlist() {
        let r = fixture_registry();
        let allowed = vec!["file_write".to_string()];
        let out = r.computer_use_filtered(&allowed, "vm");
        assert!(!out.by_name.contains_key("vm_computer_use"));
        assert!(!out.by_name.contains_key("vm_browser_open"));
        assert!(!out.by_name.contains_key("ego_browser"));
        assert!(out.by_name.contains_key("file_write"));
    }
}
