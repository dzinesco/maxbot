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
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BotRunStatus {
    Running,
    Succeeded,
    Failed,
    Cancelled,
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
pub fn registry_for(bot: &Bot, full: &ToolRegistry) -> ToolRegistry {
    full.filtered(&bot.allowed_tools)
}

#[cfg(test)]
mod tests {
    use super::BotState;

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
}
