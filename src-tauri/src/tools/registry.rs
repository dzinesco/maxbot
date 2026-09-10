//! Central tool registry. Owns the concrete tool implementations and
//! exposes:
//! - `definitions()` — array of `ToolDefinition`s for the chat request
//! - `execute()` — invoke a tool by name, returning a result or error

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::connectors::{
    self, CalendarCreateEventTool, CalendarGetEventTool, CalendarListEventsTool,
    CalendarUpdateEventTool, GithubAddCommentTool, GithubCreateIssueTool, GithubGetIssueTool,
    GithubListIssuesTool, GmailDraftMessageTool, GmailGetMessageTool, GmailListMessagesTool,
    GmailSendMessageTool,
};
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
use super::calendar::{CalendarTodayTool, CalendarWeekTool};
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
use super::shared_fs::{SharedListTool, SharedReadTool, SharedWriteTool};
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
            // v3.7.0 (Phase 8) — Calendar (Google).
            // Note: `CalendarCreateEventTool` here is the
            // **Google Calendar** connector (imported from
            // `crate::connectors` above), not the Apple
            // Calendar one. Per the brief, the Google
            // connector owns the `calendar_create_event`
            // name from this release forward; the Apple
            // Calendar `calendar_today` / `calendar_week`
            // tools (unique names) remain.
            Arc::new(CalendarListEventsTool),
            Arc::new(CalendarGetEventTool),
            Arc::new(CalendarCreateEventTool),
            Arc::new(CalendarUpdateEventTool),
            // v3.7.0 (Phase 8) — Gmail. The four
            // tools: list / get / send / draft.
            // `gmail_send_message` and
            // `gmail_draft_message` are
            // per-call-consent; the two reads
            // are auto. The brief's Grok Bot
            // preset maps the existing
            // `mail_send` / `mail_draft`
            // (Apple Mail) names to these
            // Gmail tools, so a user who
            // enables Gmail for a Bot gets
            // the same Auto/Ask pattern
            // without picking rules again.
            Arc::new(GmailListMessagesTool),
            Arc::new(GmailGetMessageTool),
            Arc::new(GmailSendMessageTool),
            Arc::new(GmailDraftMessageTool),
            // v3.7.0 (Phase 8) — GitHub. PAT-
            // based, four tools, two reads
            // (auto) and two writes
            // (per-call consent).
            Arc::new(GithubListIssuesTool),
            Arc::new(GithubGetIssueTool),
            Arc::new(GithubCreateIssueTool),
            Arc::new(GithubAddCommentTool),
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
            // v3.5.0 (Phase 6) — Server-side shared
            // folder. The `maxbotd` daemon owns the path
            // (`~/bots/_shared/`) on the host. These three
            // tools are the only sanctioned way for a Bot
            // to leave artifacts for another Bot in the
            // same group without going through the per-Bot
            // VM. The path-safety guard inside
            // `tools::shared_fs` is the load-bearing
            // security piece: it refuses absolute paths,
            // `..` segments, and symlinks that resolve
            // outside the shared root.
            // `shared_write` requires consent (mutating);
            // `shared_read` / `shared_list` do not
            // (read-only). The Grok Bot defaults preset
            // matches the same read-only/mutating split.
            Arc::new(SharedWriteTool),
            Arc::new(SharedReadTool),
            Arc::new(SharedListTool),
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

    /// v3.7.0 (Phase 8) — return a new registry with
    /// the **connector** tools filtered by the bot's
    /// `connectors_enabled` setting. The
    /// `connectors_enabled` string is the
    /// comma-separated list of connector ids stored on
    /// the Bot row (e.g. `"gmail,calendar"`). The
    /// method drops every tool whose name is in
    /// `connectors::is_connector_tool(name)` AND whose
    /// owning connector is not in the enabled list.
    /// Non-connector tools are untouched.
    ///
    /// The allowlist still applies — the caller is
    /// expected to pass the bot's `allowed_tools`
    /// list, and `connectors_filtered` is applied
    /// AFTER the allowlist (i.e., a bot that doesn't
    /// have `gmail_send_message` in its allowlist
    /// still doesn't see it, even with `"gmail"` in
    /// `connectors_enabled`).
    ///
    /// Compose with `computer_use_filtered` to get the
    /// full per-Bot view; the executor's
    /// `registry_for` does this in v3.7.0.
    pub fn connectors_filtered(
        &self,
        allowed: &[String],
        connectors_enabled: &str,
    ) -> ToolRegistry {
        let base = self.filtered(allowed);
        let enabled: std::collections::HashSet<String> =
            crate::connectors::parse_enabled(connectors_enabled)
                .into_iter()
                .collect();
        let mut by_name = base.by_name;
        if enabled.is_empty() {
            // No connectors enabled — drop every
            // connector tool. The safe default.
            by_name.retain(|name, _| !connectors::is_connector_tool(name));
        } else {
            by_name.retain(|name, _| {
                if !connectors::is_connector_tool(name) {
                    return true;
                }
                // For each registered connector id,
                // check whether this tool belongs to it.
                // The first match wins; if no match, the
                // tool is dropped (defensive — every
                // connector-owned tool should map to
                // exactly one connector id).
                for cid in &enabled {
                    if connectors::tools_for_connector(cid).contains(&name.as_str()) {
                        return true;
                    }
                }
                false
            });
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

    // v3.7.0 (Phase 8) — `connectors_filtered` is the
    // per-Bot connector-enable gate. The
    // `connectors_enabled` string is the comma-separated
    // list stored on the Bot row; the tool list drops
    // every `connectors::*`-owned tool whose connector
    // id is not in the list. The allowlist still
    // applies — `connectors_filtered` is applied AFTER
    // the allowlist, so a Bot that doesn't have
    // `gmail_send_message` in its `allowed_tools`
    // doesn't see it even when `"gmail"` is in
    // `connectors_enabled`. (Compose with
    // `computer_use_filtered` to get the full per-Bot
    // view; the executor's `registry_for` does this.)
    fn fixture_registry_with_connectors() -> ToolRegistry {
        let mut by_name: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        by_name.insert(
            "gmail_list_messages".to_string(),
            Arc::new(GmailListMessagesTool),
        );
        by_name.insert(
            "gmail_send_message".to_string(),
            Arc::new(GmailSendMessageTool),
        );
        by_name.insert(
            "calendar_list_events".to_string(),
            Arc::new(CalendarListEventsTool),
        );
        by_name.insert(
            "github_list_issues".to_string(),
            Arc::new(GithubListIssuesTool),
        );
        by_name.insert(
            "file_write".to_string(),
            Arc::new(crate::tools::file_write::FileWriteTool),
        );
        ToolRegistry { by_name }
    }

    /// No `connectors_enabled` = no connector tools.
    /// This is the safe default for Bots created on
    /// the v3.7.0 code path (the column is NULL until
    /// the user opts in) and the pre-v3.7.0 behavior
    /// (a Bot from v3.6.0 has no connector tools to
    /// see at all).
    #[test]
    fn connectors_empty_hides_all_connector_tools() {
        let r = fixture_registry_with_connectors();
        let allowed = vec![
            "gmail_list_messages".to_string(),
            "gmail_send_message".to_string(),
            "calendar_list_events".to_string(),
            "github_list_issues".to_string(),
            "file_write".to_string(),
        ];
        let out = r.connectors_filtered(&allowed, "");
        assert!(!out.by_name.contains_key("gmail_list_messages"));
        assert!(!out.by_name.contains_key("gmail_send_message"));
        assert!(!out.by_name.contains_key("calendar_list_events"));
        assert!(!out.by_name.contains_key("github_list_issues"));
        // Non-connector tool survives.
        assert!(out.by_name.contains_key("file_write"));
    }

    /// `connectors_enabled = "gmail"` enables the four
    /// Gmail tools and nothing else.
    #[test]
    fn connectors_gmail_only_enables_gmail_tools() {
        let r = fixture_registry_with_connectors();
        let allowed = vec![
            "gmail_list_messages".to_string(),
            "gmail_send_message".to_string(),
            "calendar_list_events".to_string(),
            "github_list_issues".to_string(),
            "file_write".to_string(),
        ];
        let out = r.connectors_filtered(&allowed, "gmail");
        assert!(out.by_name.contains_key("gmail_list_messages"));
        assert!(out.by_name.contains_key("gmail_send_message"));
        assert!(!out.by_name.contains_key("calendar_list_events"));
        assert!(!out.by_name.contains_key("github_list_issues"));
        assert!(out.by_name.contains_key("file_write"));
    }

    /// All three connectors enabled, allowlist
    /// permits all four tools each = full per-Bot
    /// connector surface.
    #[test]
    fn connectors_all_three_enables_all_twelve_tools() {
        let r = fixture_registry_with_connectors();
        let allowed = vec![
            "gmail_list_messages".to_string(),
            "gmail_send_message".to_string(),
            "calendar_list_events".to_string(),
            "github_list_issues".to_string(),
            "file_write".to_string(),
        ];
        let out = r.connectors_filtered(&allowed, "gmail,calendar,github");
        assert!(out.by_name.contains_key("gmail_list_messages"));
        assert!(out.by_name.contains_key("gmail_send_message"));
        assert!(out.by_name.contains_key("calendar_list_events"));
        assert!(out.by_name.contains_key("github_list_issues"));
        assert!(out.by_name.contains_key("file_write"));
    }

    /// Allowlist is binding: a Bot without
    /// `gmail_send_message` in `allowed_tools` doesn't
    /// see it, even with `"gmail"` in
    /// `connectors_enabled`.
    #[test]
    fn connectors_respects_allowlist() {
        let r = fixture_registry_with_connectors();
        // User only allowed the read-only tool.
        let allowed = vec!["gmail_list_messages".to_string()];
        let out = r.connectors_filtered(&allowed, "gmail");
        assert!(out.by_name.contains_key("gmail_list_messages"));
        assert!(!out.by_name.contains_key("gmail_send_message"));
    }

    /// Unknown connector id is a no-op (filtered out
    /// by `parse_enabled`). A typo in the editor can't
    /// accidentally enable something.
    #[test]
    fn connectors_unknown_id_is_ignored() {
        let r = fixture_registry_with_connectors();
        let allowed = vec![
            "gmail_list_messages".to_string(),
            "file_write".to_string(),
        ];
        let out = r.connectors_filtered(&allowed, "slack, gmail ,  ");
        assert!(out.by_name.contains_key("gmail_list_messages"));
        assert!(!out.by_name.contains_key("slack_list_channels")); // not registered, but would be dropped if it were
    }
}
