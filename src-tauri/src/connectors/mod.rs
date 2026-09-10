//! v3.7.0 (Phase 8) — Connectors.
//!
//! Three first-party connectors that turn MaxBot into a
//! semi-useful "Grok Bot equivalent" against the user's
//! real Gmail / Google Calendar / GitHub accounts:
//!
//! - **Gmail** — `connectors::gmail` exposes
//!   `gmail_list_messages`, `gmail_get_message`,
//!   `gmail_send_message`, `gmail_draft_message`. Talks
//!   to the Gmail API with a Google App Password (Basic
//!   Auth + `https://mail.google.com/.../smtp/` is the
//!   OAuth path; we deliberately pick the simpler App
//!   Password for v3.7.0 — the OAuth dance can land in a
//!   later slice if Tyler ever wants it).
//! - **Google Calendar** — `connectors::calendar`
//!   exposes `calendar_list_events`, `calendar_get_event`,
//!   `calendar_create_event`, `calendar_update_event`.
//!   Same auth story as Gmail (one Google account, one
//!   App Password).
//! - **GitHub** — `connectors::github` exposes
//!   `github_list_issues`, `github_get_issue`,
//!   `github_create_issue`, `github_add_comment`. Uses
//!   a GitHub Personal Access Token (PAT) per the brief
//!   — the simpler auth path; OAuth Apps are deferred.
//!
//! ## Why not MCP?
//!
//! The brief allows either the existing `mcp.rs` JSON-RPC
//! pattern or a direct `Tool` implementation. The MCP
//! path requires spawning a separate child process for
//! each connector (a small Python or Node script that
//! speaks JSON-RPC). For three connectors with simple
//! HTTP APIs and a single shared auth model, the direct
//! `Tool` path is shorter, has fewer moving parts, and
//! keeps credentials in the same process the user can
//! audit. The MCP infrastructure stays in place for
//! future work (filesystem / web / DB connectors Tyler
//! might want later).
//!
//! ## Credentials
//!
//! All three connectors read their credentials from the
//! existing `Settings` struct (`gmail_app_password`,
//! `google_app_password` — shared with Calendar, and
//! `github_pat`). A connector tool that finds the
//! relevant Settings field empty returns a clear error
//! asking the user to set the credential. The Settings
//! UI already exposes similar text fields for the LLM
//! API keys; the new credential fields slot in next to
//! them.
//!
//! ## Per-Bot toggle
//!
//! Each Bot row carries a `connectors_enabled: String`
//! field (comma-separated list of connector ids:
//! `gmail,calendar,github`). The tool registry filters
//! its tool list by this field on top of the existing
//! `allowed_tools` allowlist, so a Bot that doesn't
//! have `gmail` enabled doesn't see `gmail_*` in its
//! tools — even if the user accidentally allowed the
//! tool in the BotEditor's Capabilities list.

pub mod calendar;
pub mod github;
pub mod gmail;
pub mod oauth;

// Re-export the connector tool structs so the tool
// registry can `use crate::connectors::GmailListMessagesTool`
// (etc.) without going through the submodule. The
// same names already exist as `pub` in
// `connectors::gmail` / `connectors::calendar` /
// `connectors::github`; the re-exports keep the
// registry's import list flat.
pub use calendar::{
    CalendarCreateEventTool, CalendarGetEventTool, CalendarListEventsTool,
    CalendarUpdateEventTool,
};
pub use github::{
    GithubAddCommentTool, GithubCreateIssueTool, GithubGetIssueTool,
    GithubListIssuesTool,
};
pub use gmail::{
    GmailDraftMessageTool, GmailGetMessageTool, GmailListMessagesTool,
    GmailSendMessageTool,
};

use std::sync::Arc;

use tauri::State;

use crate::AppState;

/// Canonical list of connector ids. Stored on
/// `Bot.connectors_enabled` as a comma-separated string
/// so adding a new connector doesn't require a DB
/// migration. The renderer uses this list to render
/// the per-Bot Connectors tab.
pub const CONNECTOR_IDS: &[&str] = &["gmail", "calendar", "github"];

/// Parse a `Bot.connectors_enabled` string into a
/// `Vec<String>` of trimmed, non-empty entries. The
/// stored value is the comma-separated list produced by
/// `BotEditor.tsx` when the user toggles checkboxes;
/// empty / unset means "no connectors enabled." Duplicates
/// are removed (preserving first-seen order) so a
/// `"gmail, gmail"` stored value doesn't get counted
/// twice in the registry's enable set.
pub fn parse_enabled(value: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    value
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .filter(|s| CONNECTOR_IDS.contains(&s.as_str()))
        .filter(|s| seen.insert(s.clone()))
        .collect()
}

/// True if `name` is one of the connector tools. The
/// tool registry uses this to decide which tools to
/// hide from a Bot whose `connectors_enabled` doesn't
/// list a given connector.
pub fn is_connector_tool(name: &str) -> bool {
    matches!(
        name,
        "gmail_list_messages"
            | "gmail_get_message"
            | "gmail_send_message"
            | "gmail_draft_message"
            | "calendar_list_events"
            | "calendar_get_event"
            | "calendar_create_event"
            | "calendar_update_event"
            | "github_list_issues"
            | "github_get_issue"
            | "github_create_issue"
            | "github_add_comment"
    )
}

/// Map a connector id (`"gmail"`, `"calendar"`,
/// `"github"`) to the list of tool names that
/// connector contributes. Used by the registry's
/// `connectors_filtered` method to drop tools when the
/// Bot's `connectors_enabled` list doesn't include the
/// matching id.
pub fn tools_for_connector(connector_id: &str) -> &'static [&'static str] {
    match connector_id {
        "gmail" => &[
            "gmail_list_messages",
            "gmail_get_message",
            "gmail_send_message",
            "gmail_draft_message",
        ],
        "calendar" => &[
            "calendar_list_events",
            "calendar_get_event",
            "calendar_create_event",
            "calendar_update_event",
        ],
        "github" => &[
            "github_list_issues",
            "github_get_issue",
            "github_create_issue",
            "github_add_comment",
        ],
        _ => &[],
    }
}

/// v3.7.0 (Phase 8) — Tauri command surface for the
/// "Test connection" button in the BotEditor's
/// Connectors section. The renderer calls
/// `invoke("connector_test", { connector: "gmail" })`
/// (or `"calendar"` / `"github"`) and surfaces the
/// returned message (or error) in a toast.
///
/// Each branch delegates to the connector's own
/// `test_connection` function, which loads the
/// relevant credential from `Settings` and pings the
/// upstream API. A missing credential returns a
/// clear "set X in Settings" error so the user can
/// fix it without debugging HTTP-level failures.
#[tauri::command]
pub async fn connector_test(
    state: State<'_, Arc<AppState>>,
    connector: String,
) -> Result<String, String> {
    let settings = state
        .db
        .load_settings()
        .map_err(|e| format!("load settings: {e}"))?;
    match connector.as_str() {
        // v3.7.12: Gmail + Calendar need the `db` so
        // their `test_connection` can resolve an access
        // token via the OAuth refresh path. GitHub still
        // takes only `&Settings` (it reads the PAT).
        "gmail" => gmail::test_connection(&state.db, &settings).await,
        "calendar" => calendar::test_connection(&state.db, &settings).await,
        "github" => github::test_connection(&settings).await,
        other => Err(format!(
            "unknown connector `{other}`; expected one of gmail, calendar, github"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_enabled_handles_empty_string() {
        assert!(parse_enabled("").is_empty());
    }

    #[test]
    fn parse_enabled_handles_whitespace_only() {
        assert!(parse_enabled("   ").is_empty());
    }

    #[test]
    fn parse_enabled_dedupes_and_drops_unknowns() {
        let v = parse_enabled("gmail, unknown, calendar , bogus ,gmail");
        assert_eq!(v, vec!["gmail", "calendar"]);
    }

    #[test]
    fn is_connector_tool_recognizes_known_names() {
        assert!(is_connector_tool("gmail_list_messages"));
        assert!(is_connector_tool("calendar_create_event"));
        assert!(is_connector_tool("github_add_comment"));
        assert!(!is_connector_tool("file_write"));
        assert!(!is_connector_tool("mail_send"));
    }

    #[test]
    fn tools_for_connector_returns_expected_set() {
        assert_eq!(
            tools_for_connector("gmail"),
            &["gmail_list_messages", "gmail_get_message", "gmail_send_message", "gmail_draft_message"]
        );
        assert_eq!(
            tools_for_connector("calendar"),
            &["calendar_list_events", "calendar_get_event", "calendar_create_event", "calendar_update_event"]
        );
        assert_eq!(
            tools_for_connector("github"),
            &["github_list_issues", "github_get_issue", "github_create_issue", "github_add_comment"]
        );
        assert_eq!(tools_for_connector("unknown"), &[] as &[&str]);
    }
}
