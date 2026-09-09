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
