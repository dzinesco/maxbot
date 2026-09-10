//! Sub-agents ("bots") in MaxBot. A bot is a named persona with its own
//! system prompt, default model, allowed tool set, and optional
//! schedule. Bots run on demand (the "Run now" button) or on a schedule;
//! they can also message other bots via the `message_bot` tool.

pub mod executor;
pub mod filesystem;
pub mod registry;
pub mod scheduler;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::tools::registry::ToolRegistry;

/// v2.0 Slice E: the six states the renderer derives a presence
/// indicator from. Persisted on the `bots` row by `bot_set_state`
/// (called from the bot executor at run start / end / blocked). The
/// `working` / `thinking` / `done` / `waiting` / `blocked` variants
/// are short-lived; `idle` is the resting state.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BotState {
    #[default]
    Idle,
    Thinking,
    Working,
    Waiting,
    Blocked,
    Done,
}

impl BotState {
    pub fn as_str(self) -> &'static str {
        match self {
            BotState::Idle => "idle",
            BotState::Thinking => "thinking",
            BotState::Working => "working",
            BotState::Waiting => "waiting",
            BotState::Blocked => "blocked",
            BotState::Done => "done",
        }
    }

    /// Parse a free-form string. Unknown values fall back to `Idle`
    /// so a future enum addition on the renderer side (or a typo in
    /// an old DB row) doesn't crash the read path.
    pub fn parse(value: &str) -> Self {
        match value {
            "thinking" => Self::Thinking,
            "working" => Self::Working,
            "waiting" => Self::Waiting,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => Self::Idle,
        }
    }
}

/// One row in the `bots` table. Persisted across launches. Tools are
/// referenced by name; the registry resolves them at run time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bot {
    pub id: String,
    pub name: String,
    pub description: String,
    pub system_prompt: String,
    pub default_model: String,
    /// Names of tools the bot is allowed to call. Empty = no tools.
    /// Anything not in the registry is silently dropped.
    pub allowed_tools: Vec<String>,
    /// Optional emoji or short label used as the bot's avatar in the UI.
    pub icon: String,
    /// Color hint for the avatar (hex). Empty string falls back to the
    /// theme's accent.
    pub color: String,
    /// v2.0 Slice E: a second color slot for the avatar gradient. The
    /// `BotAvatar` mixes `color` + `avatar_color` to render a tinted
    /// presence badge. Empty string → fall back to `color`.
    #[serde(default)]
    pub avatar_color: String,
    /// v2.0 Slice E: last time the user (or the bot executor) touched
    /// the bot — used by the sidebar to show "2m ago" timestamps.
    /// `None` on a brand-new bot.
    #[serde(default)]
    pub last_active_at: Option<chrono::DateTime<chrono::Utc>>,
    /// v2.0 Slice E: persisted presence state. Default `Idle`. Written
    /// by the `bot_set_state` Tauri command from the bot executor.
    /// The renderer treats this as a hint; it also factors in the
    /// most-recent `bot_run.status` and the `computers.state` row.
    #[serde(default)]
    pub state: BotState,
    /// v3.2.0 — Computer Use target. The Bot editor dropdown
    /// lets the user pick between three values:
    ///   - `"vm"` (default) — the Bot's own per-Bot Linux VM
    ///     is the Computer Use target. The
    ///     `vm_computer_use` tool (Chromium + xdotool + scrot
    ///     via SSH) is in the per-Bot tool list, and the
    ///     Mac-based `ego_browser` is NOT.
    ///   - `"mac"` — the legacy Mac / AppleScript path. The
    ///     `ego_browser` tool is in the per-Bot tool list,
    ///     and `vm_computer_use` is NOT.
    ///   - `"mac-with-approval"` — the Mac / AppleScript
    ///     path, but every `ego_browser` call is wrapped
    ///     in an approval gate (Phase 5's territory; the
    ///     approval flow itself lands in v3.4.0, but the
    ///     `bot.computer_use` enum value is reserved here).
    ///
    /// Stored as a free-form String (not a typed enum) so a
    /// future addition — `"hybrid"`, `"none"`, etc — doesn't
    /// require a schema migration. `default_computer_use()`
    /// returns `"vm"` for any new bot, and
    /// `parse_computer_use()` normalizes unknown / empty
    /// values to `"vm"` on the read path so a typo can't
    /// silently disable Computer Use.
    #[serde(default = "default_computer_use")]
    pub computer_use: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Default value for the new `computer_use` field. Used by
/// `#[serde(default = ...)]` for old rows that pre-date v3.2.0
/// and by the `Bot::new_*` constructors. "vm" is the
/// v3.2.0 default — VM is now the primary Computer Use
/// target, the Mac path is an opt-in.
fn default_computer_use() -> String {
    "vm".to_string()
}

/// Parse the stored `bot.computer_use` value, falling back
/// to `"vm"` for any unknown / null / empty string. Mirrors
/// `BotState::parse` — keeps the read path tolerant of
/// future enum additions or typos in old DB rows.
pub fn parse_computer_use(s: &str) -> &'static str {
    match s {
        "vm" => "vm",
        "mac" => "mac",
        "mac-with-approval" => "mac-with-approval",
        _ => "vm",
    }
}

impl Bot {
    pub fn new_id() -> String {
        Uuid::new_v4().to_string()
    }
}

/// A bot's optional run schedule. Two ways to express a schedule:
/// (1) `interval_seconds` — "every N seconds" (simple, low-precision).
/// (2) `cron_expression` — a 5-field crontab string in the bot's local
///     timezone, e.g. "0 9 * * 1-5" for "Weekdays at 9:00 AM".
///     Evaluated by the v0.4.4 scheduler tick.
///
/// Both can be set; the scheduler prefers cron when non-empty. Set both
/// to 0 / "" to disable. (interval_seconds is kept for backwards
/// compat with v0.3 schedules.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotSchedule {
    pub bot_id: String,
    /// Run interval in seconds. 0 = disabled when cron is empty.
    #[serde(default)]
    pub interval_seconds: u32,
    /// Optional 5-field crontab expression. Empty = no cron schedule.
    /// When set, takes precedence over `interval_seconds`.
    #[serde(default)]
    pub cron_expression: String,
    /// Last time the scheduler fired this bot. Null = never.
    #[serde(default)]
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Last conversation the scheduled run wrote into. Lets the next
    /// run continue in the same thread, so a recurring bot has one
    /// persistent log.
    #[serde(default)]
    pub last_conversation_id: Option<String>,
    /// v2.3.0 — Routines: optional Skill to run on this schedule.
    /// When `Some`, the scheduler dispatches to `run_skill` instead
    /// of `run_bot_once`. The bot is still the "owner" (its
    /// `system_prompt`, `default_model`, and Computer VM are
    /// available to tools that look at `bot_id`), but the user
    /// gets a fixed step-by-step procedure rather than the
    /// LLM-driven chat loop.
    #[serde(default)]
    pub skill_id: Option<String>,
}

/// A single bot invocation, whether triggered by hand or by the
/// scheduler. Kept so the UI can show "last run at HH:MM, finished
/// successfully".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotRun {
    pub id: String,
    pub bot_id: String,
    pub conversation_id: String,
    pub status: BotRunStatus,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
    pub result_summary: String,
    /// v3.1.0 — which entry point fired this run.
    /// `"app"` for in-app "Run now" / `send_to_bot`, `"daemon"`
    /// for the always-on scheduler, `"webhook"` for a webhook
    /// POST. Defaults to `"app"` for legacy rows.
    #[serde(default = "default_triggered_by")]
    pub triggered_by: String,
}

fn default_triggered_by() -> String {
    "app".to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BotRunStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// v3.1.0 — typed enum for the `bot_runs.triggered_by` column.
/// The three values cover the only entry points that can fire a
/// Bot run today. Anything not in this set is treated as `"app"`
/// by the executor and the renderer.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BotRunTriggeredBy {
    App,
    Daemon,
    Webhook,
}

impl BotRunTriggeredBy {
    pub fn as_str(self) -> &'static str {
        match self {
            BotRunTriggeredBy::App => "app",
            BotRunTriggeredBy::Daemon => "daemon",
            BotRunTriggeredBy::Webhook => "webhook",
        }
    }

    /// Parse the stored value, falling back to `App` for any
    /// unknown / null / empty string. Keeps reads tolerant of
    /// legacy rows that predate the column.
    pub fn parse(s: &str) -> Self {
        match s {
            "daemon" => BotRunTriggeredBy::Daemon,
            "webhook" => BotRunTriggeredBy::Webhook,
            _ => BotRunTriggeredBy::App,
        }
    }
}

/// Inter-agent message queued by a bot via the `message_bot` tool.
/// Delivered when the recipient bot next runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotMessage {
    pub id: String,
    pub from_bot_id: String,
    pub to_bot_id: String,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Whether the recipient bot has read it (the run loop processes
    /// unread messages first, then marks them read).
    pub read: bool,
    /// The conversation the message was delivered into (the recipient's
    /// "inbox" or, if the bot has a current conversation, that one).
    pub conversation_id: Option<String>,
}

/// Build a tool registry whose `by_name` is filtered down to the bot's
/// allowed tools. The returned registry still owns the same backing
/// implementations, but `definitions()` only emits the ones the bot
/// is permitted to call. Unknown tool names are silently dropped.
///
/// v3.2.0 — the Computer Use tools (`vm_computer_use` vs
/// `ego_browser`) are swapped in or out per the bot's
/// `computer_use` setting. The two filters are composed:
/// the allowlist is applied first (so a bot that doesn't
/// have `vm_computer_use` in its allowlist doesn't see it
/// even when `computer_use == "vm"`), then the
/// computer-use-specific swap is applied (so a bot with
/// `ego_browser` in its allowlist but `computer_use ==
/// "vm"` doesn't see `ego_browser` either).
pub fn registry_for(bot: &Bot, full: &ToolRegistry) -> ToolRegistry {
    full.computer_use_filtered(&bot.allowed_tools, &bot.computer_use)
}

#[cfg(test)]
mod tests {
    use super::{default_computer_use, parse_computer_use, BotState};

    /// `BotState::parse` is the bridge between the DB column
    /// (free-form TEXT) and the typed enum. Future enum
    /// additions on the renderer side should not crash the
    /// read path — unknown values fall back to Idle. This
    /// test pins the fallback behavior so a future "we
    /// should default to Error" change has to touch the
    /// test.
    #[test]
    fn bot_state_parse_round_trips_known_values() {
        assert_eq!(BotState::parse("idle"), BotState::Idle);
        assert_eq!(BotState::parse("thinking"), BotState::Thinking);
        assert_eq!(BotState::parse("working"), BotState::Working);
        assert_eq!(BotState::parse("waiting"), BotState::Waiting);
        assert_eq!(BotState::parse("blocked"), BotState::Blocked);
        assert_eq!(BotState::parse("done"), BotState::Done);
    }

    #[test]
    fn bot_state_parse_falls_back_to_idle_for_unknown_values() {
        // A future enum addition on the renderer side
        // should not crash a read of the old row.
        assert_eq!(BotState::parse(""), BotState::Idle);
        assert_eq!(BotState::parse("running"), BotState::Idle);
        assert_eq!(
            BotState::parse("panic-now-if-this-matches"),
            BotState::Idle
        );
    }

    #[test]
    fn bot_state_as_str_round_trips_through_parse() {
        for state in [
            BotState::Idle,
            BotState::Thinking,
            BotState::Working,
            BotState::Waiting,
            BotState::Blocked,
            BotState::Done,
        ] {
            assert_eq!(BotState::parse(state.as_str()), state);
        }
    }

    /// v2.0 Slice E: the DB layer must tolerate the
    /// absence of the new `state` column on a row that
    /// pre-dates the migration. The migration uses
    /// `add_column_if_missing` to add it with a default of
    /// `'idle'`, so the read path is exercised against a
    /// row that has the default value.
    #[test]
    fn bot_state_default_is_idle() {
        assert_eq!(BotState::default(), BotState::Idle);
    }

    /// v3.2.0 — `parse_computer_use` is the read-path
    /// bridge between the DB column and the registry
    /// filter. A typo in the editor must not silently
    /// disable Computer Use. The fallback is "vm" (the
    /// v3.2.0 default) — same as `parse_computer_use`
    /// elsewhere in the codebase.
    #[test]
    fn parse_computer_use_recognizes_three_known_values() {
        assert_eq!(parse_computer_use("vm"), "vm");
        assert_eq!(parse_computer_use("mac"), "mac");
        assert_eq!(parse_computer_use("mac-with-approval"), "mac-with-approval");
    }

    #[test]
    fn parse_computer_use_falls_back_to_vm_for_unknown_values() {
        // Empty (pre-v3.2.0 row never set the column,
        // though the migration DEFAULT makes that rare),
        // typo, future value — all fall back to "vm".
        assert_eq!(parse_computer_use(""), "vm");
        assert_eq!(parse_computer_use("VMM"), "vm");
        assert_eq!(parse_computer_use("mac-approval"), "vm");
        assert_eq!(parse_computer_use("hybrid"), "vm");
    }

    #[test]
    fn default_computer_use_returns_vm() {
        // Pinned: v3.2.0 made "vm" the default for new
        // Bots. A future change ("mac"? "vm-with-approval"?)
        // would be a deliberate policy shift, not a typo
        // to slip through.
        assert_eq!(default_computer_use(), "vm");
    }
}
