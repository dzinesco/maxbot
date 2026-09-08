//! SQLite schema and CRUD operations for MaxBot's local state.
//!
//! Three tables: `conversations`, `messages`, and `settings`. The
//! `messages.tool_calls` column stores serialized tool-call fragments as
//! JSON so that we can re-hydrate a full assistant turn on reload.

use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    fn as_str(self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: MessageRole,
    pub content: String,
    /// Tool calls that accompanied this message (assistant role only).
    #[serde(default)]
    pub tool_calls: Vec<PersistedToolCall>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// If this conversation was started by (or for) a bot, the bot's id.
    /// Null for human-initiated chats. Used by the bot executor to keep
    /// a single conversation per recurring bot.
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// MiniMax API key. Stored unencrypted at rest in the local SQLite DB
    /// (which lives in the per-user Application Support directory with the
    /// default macOS file protection class). For higher security, swap this
    /// for the system Keychain — TODO once we have a settings UI that
    /// surfaces the tradeoff.
    pub minimax_api_key: Option<String>,
    /// Default model id (e.g. "MiniMax-M3"). Empty string falls back to the
    /// provider's `default_model()`.
    #[serde(default)]
    pub default_model: String,
    /// Base URL override for MiniMax. Empty string uses the international
    /// endpoint at api.minimax.io.
    #[serde(default)]
    pub base_url: String,
}

pub struct Database {
    conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;
        let db = Self { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                bot_id      TEXT
             );
             CREATE TABLE IF NOT EXISTS messages (
                id              TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                role            TEXT NOT NULL,
                content         TEXT NOT NULL,
                tool_calls_json TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_by_conversation
                 ON messages(conversation_id, created_at);
             CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS bots (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                description     TEXT NOT NULL DEFAULT '',
                system_prompt   TEXT NOT NULL DEFAULT '',
                default_model   TEXT NOT NULL DEFAULT 'MiniMax-M3',
                allowed_tools   TEXT NOT NULL DEFAULT '[]',
                icon            TEXT NOT NULL DEFAULT '',
                color           TEXT NOT NULL DEFAULT '',
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS bot_schedules (
                bot_id              TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
                interval_seconds    INTEGER NOT NULL DEFAULT 0,
                last_run_at         TEXT,
                last_conversation_id TEXT
             );
             CREATE TABLE IF NOT EXISTS bot_runs (
                id              TEXT PRIMARY KEY,
                bot_id          TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                conversation_id TEXT NOT NULL,
                status          TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                finished_at     TEXT,
                result_summary  TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS bot_runs_by_bot
                 ON bot_runs(bot_id, started_at DESC);
             CREATE TABLE IF NOT EXISTS bot_messages (
                id              TEXT PRIMARY KEY,
                from_bot_id     TEXT NOT NULL,
                to_bot_id       TEXT NOT NULL,
                body            TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                read            INTEGER NOT NULL DEFAULT 0,
                conversation_id TEXT
             );
             CREATE INDEX IF NOT EXISTS bot_messages_inbox
                 ON bot_messages(to_bot_id, read, created_at);",
        )?;
        Ok(())
    }

    // ----- conversations -----

    pub fn list_conversations(&self) -> rusqlite::Result<Vec<Conversation>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at, bot_id
             FROM conversations
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: parse_dt(row.get::<_, String>(2)?),
                updated_at: parse_dt(row.get::<_, String>(3)?),
                bot_id: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn create_conversation(
        &self,
        title: Option<String>,
        bot_id: Option<&str>,
    ) -> rusqlite::Result<Conversation> {
        let now = Utc::now();
        let convo = Conversation {
            id: Uuid::new_v4().to_string(),
            title: title.unwrap_or_else(|| "New chat".to_string()),
            created_at: now,
            updated_at: now,
            bot_id: bot_id.map(str::to_string),
        };
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at, bot_id) VALUES (?, ?, ?, ?, ?)",
            params![
                convo.id,
                convo.title,
                convo.created_at.to_rfc3339(),
                convo.updated_at.to_rfc3339(),
                convo.bot_id,
            ],
        )?;
        Ok(convo)
    }

    /// Create a conversation that belongs to a specific bot. The bot
    /// executor uses this for scheduled runs so the recurring bot has
    /// a single persistent log.
    pub fn create_bot_conversation(
        &self,
        bot_id: &str,
        title: Option<String>,
    ) -> rusqlite::Result<Conversation> {
        let now = Utc::now();
        let convo = Conversation {
            id: Uuid::new_v4().to_string(),
            title: title.unwrap_or_else(|| format!("Bot: {}", bot_id)),
            created_at: now,
            updated_at: now,
            bot_id: Some(bot_id.to_string()),
        };
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at, bot_id) VALUES (?, ?, ?, ?, ?)",
            params![
                convo.id,
                convo.title,
                convo.created_at.to_rfc3339(),
                convo.updated_at.to_rfc3339(),
                convo.bot_id,
            ],
        )?;
        Ok(convo)
    }

    pub fn delete_conversation(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute("DELETE FROM conversations WHERE id = ?", params![id])?;
        Ok(())
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> rusqlite::Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE conversations SET title = ?, updated_at = ? WHERE id = ?",
            params![title, now, id],
        )?;
        Ok(())
    }

    pub fn touch_conversation(&self, id: &str) -> rusqlite::Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![now, id],
        )?;
        Ok(())
    }

    // ----- messages -----

    pub fn list_messages(&self, conversation_id: &str) -> rusqlite::Result<Vec<Message>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, tool_calls_json, created_at
             FROM messages
             WHERE conversation_id = ?
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            let role_str: String = row.get(2)?;
            let tool_calls_json: String = row.get(4)?;
            let tool_calls: Vec<PersistedToolCall> =
                serde_json::from_str(&tool_calls_json).unwrap_or_default();
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: MessageRole::parse(&role_str).unwrap_or(MessageRole::User),
                content: row.get(3)?,
                tool_calls,
                created_at: parse_dt(row.get::<_, String>(5)?),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn insert_message(
        &self,
        conversation_id: &str,
        role: MessageRole,
        content: &str,
        tool_calls: &[PersistedToolCall],
    ) -> rusqlite::Result<Message> {
        let now = Utc::now();
        let message = Message {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role,
            content: content.to_string(),
            tool_calls: tool_calls.to_vec(),
            created_at: now,
        };
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_calls_json, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![
                message.id,
                message.conversation_id,
                message.role.as_str(),
                message.content,
                serde_json::to_string(&message.tool_calls).unwrap_or_else(|_| "[]".to_string()),
                message.created_at.to_rfc3339(),
            ],
        )?;
        // Bump the conversation's updated_at so the sidebar re-orders.
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![now.to_rfc3339(), conversation_id],
        )?;
        Ok(message)
    }

    /// Append to an existing assistant message in place. Used for streaming
    /// turns: the empty assistant message is inserted on send, and each
    /// token appends to its `content` until the stream finishes.
    pub fn append_message_content(
        &self,
        message_id: &str,
        delta: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE messages SET content = content || ? WHERE id = ?",
            params![delta, message_id],
        )?;
        Ok(())
    }

    // ----- bots -----

    pub fn list_bots(&self) -> rusqlite::Result<Vec<crate::bots::Bot>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, description, system_prompt, default_model, allowed_tools, icon, color, created_at, updated_at
             FROM bots ORDER BY name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let allowed_tools_json: String = row.get(5)?;
            let allowed_tools: Vec<String> =
                serde_json::from_str(&allowed_tools_json).unwrap_or_default();
            Ok(crate::bots::Bot {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                system_prompt: row.get(3)?,
                default_model: row.get(4)?,
                allowed_tools,
                icon: row.get(6)?,
                color: row.get(7)?,
                created_at: parse_dt(row.get::<_, String>(8)?),
                updated_at: parse_dt(row.get::<_, String>(9)?),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_bot(&self, id: &str) -> rusqlite::Result<Option<crate::bots::Bot>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, description, system_prompt, default_model, allowed_tools, icon, color, created_at, updated_at
             FROM bots WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        let allowed_tools_json: String = row.get(5)?;
        let allowed_tools: Vec<String> =
            serde_json::from_str(&allowed_tools_json).unwrap_or_default();
        Ok(Some(crate::bots::Bot {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            system_prompt: row.get(3)?,
            default_model: row.get(4)?,
            allowed_tools,
            icon: row.get(6)?,
            color: row.get(7)?,
            created_at: parse_dt(row.get::<_, String>(8)?),
            updated_at: parse_dt(row.get::<_, String>(9)?),
        }))
    }

    pub fn upsert_bot(&self, bot: &crate::bots::Bot) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let allowed_tools_json =
            serde_json::to_string(&bot.allowed_tools).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT INTO bots (id, name, description, system_prompt, default_model, allowed_tools, icon, color, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                system_prompt = excluded.system_prompt,
                default_model = excluded.default_model,
                allowed_tools = excluded.allowed_tools,
                icon = excluded.icon,
                color = excluded.color,
                updated_at = excluded.updated_at",
            params![
                bot.id,
                bot.name,
                bot.description,
                bot.system_prompt,
                bot.default_model,
                allowed_tools_json,
                bot.icon,
                bot.color,
                bot.created_at.to_rfc3339(),
                bot.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_bot(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute("DELETE FROM bots WHERE id = ?", params![id])?;
        Ok(())
    }

    pub fn get_schedule(&self, bot_id: &str) -> rusqlite::Result<Option<crate::bots::BotSchedule>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, interval_seconds, last_run_at, last_conversation_id
             FROM bot_schedules WHERE bot_id = ?",
        )?;
        let mut rows = stmt.query(params![bot_id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        let last_run_str: Option<String> = row.get(2)?;
        Ok(Some(crate::bots::BotSchedule {
            bot_id: row.get(0)?,
            interval_seconds: row.get::<_, i64>(1)? as u32,
            last_run_at: last_run_str.map(parse_dt),
            last_conversation_id: row.get(3)?,
        }))
    }

    pub fn upsert_schedule(&self, schedule: &crate::bots::BotSchedule) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let last_run_at = schedule
            .last_run_at
            .as_ref()
            .map(|d| d.to_rfc3339());
        conn.execute(
            "INSERT INTO bot_schedules (bot_id, interval_seconds, last_run_at, last_conversation_id)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(bot_id) DO UPDATE SET
                interval_seconds = excluded.interval_seconds,
                last_run_at = excluded.last_run_at,
                last_conversation_id = excluded.last_conversation_id",
            params![
                schedule.bot_id,
                schedule.interval_seconds as i64,
                last_run_at,
                schedule.last_conversation_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_due_schedules(&self, now: DateTime<Utc>) -> rusqlite::Result<Vec<crate::bots::BotSchedule>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, interval_seconds, last_run_at, last_conversation_id
             FROM bot_schedules
             WHERE interval_seconds > 0
               AND (last_run_at IS NULL OR
                    (julianday(?) - julianday(last_run_at)) * 86400.0 >= interval_seconds)",
        )?;
        let rows = stmt.query_map(params![now.to_rfc3339()], |row| {
            let last_run_str: Option<String> = row.get(2)?;
            Ok(crate::bots::BotSchedule {
                bot_id: row.get(0)?,
                interval_seconds: row.get::<_, i64>(1)? as u32,
                last_run_at: last_run_str.map(parse_dt),
                last_conversation_id: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_all_schedules(&self) -> rusqlite::Result<Vec<crate::bots::BotSchedule>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, interval_seconds, last_run_at, last_conversation_id FROM bot_schedules",
        )?;
        let rows = stmt.query_map([], |row| {
            let last_run_str: Option<String> = row.get(2)?;
            Ok(crate::bots::BotSchedule {
                bot_id: row.get(0)?,
                interval_seconds: row.get::<_, i64>(1)? as u32,
                last_run_at: last_run_str.map(parse_dt),
                last_conversation_id: row.get(3)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_bot_run(&self, run: &crate::bots::BotRun) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let finished_at = run.finished_at.as_ref().map(|d| d.to_rfc3339());
        let status_str = match run.status {
            crate::bots::BotRunStatus::Running => "running",
            crate::bots::BotRunStatus::Succeeded => "succeeded",
            crate::bots::BotRunStatus::Failed => "failed",
            crate::bots::BotRunStatus::Cancelled => "cancelled",
        };
        conn.execute(
            "INSERT INTO bot_runs (id, bot_id, conversation_id, status, started_at, finished_at, result_summary)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                finished_at = excluded.finished_at,
                result_summary = excluded.result_summary",
            params![
                run.id,
                run.bot_id,
                run.conversation_id,
                status_str,
                run.started_at.to_rfc3339(),
                finished_at,
                run.result_summary,
            ],
        )?;
        Ok(())
    }

    pub fn list_bot_runs(&self, bot_id: &str, limit: u32) -> rusqlite::Result<Vec<crate::bots::BotRun>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, bot_id, conversation_id, status, started_at, finished_at, result_summary
             FROM bot_runs WHERE bot_id = ?
             ORDER BY started_at DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![bot_id, limit as i64], |row| {
            let status_str: String = row.get(3)?;
            let finished_str: Option<String> = row.get(5)?;
            let status = match status_str.as_str() {
                "running" => crate::bots::BotRunStatus::Running,
                "failed" => crate::bots::BotRunStatus::Failed,
                "cancelled" => crate::bots::BotRunStatus::Cancelled,
                _ => crate::bots::BotRunStatus::Succeeded,
            };
            Ok(crate::bots::BotRun {
                id: row.get(0)?,
                bot_id: row.get(1)?,
                conversation_id: row.get(2)?,
                status,
                started_at: parse_dt(row.get::<_, String>(4)?),
                finished_at: finished_str.map(parse_dt),
                result_summary: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    // ----- inter-agent messages -----

    pub fn enqueue_bot_message(&self, msg: &crate::bots::BotMessage) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO bot_messages (id, from_bot_id, to_bot_id, body, created_at, read, conversation_id)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                msg.id,
                msg.from_bot_id,
                msg.to_bot_id,
                msg.body,
                msg.created_at.to_rfc3339(),
                msg.read as i64,
                msg.conversation_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_inbox(&self, bot_id: &str, include_read: bool) -> rusqlite::Result<Vec<crate::bots::BotMessage>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let query = if include_read {
            "SELECT id, from_bot_id, to_bot_id, body, created_at, read, conversation_id
             FROM bot_messages WHERE to_bot_id = ? ORDER BY created_at ASC"
        } else {
            "SELECT id, from_bot_id, to_bot_id, body, created_at, read, conversation_id
             FROM bot_messages WHERE to_bot_id = ? AND read = 0 ORDER BY created_at ASC"
        };
        let mut stmt = conn.prepare(query)?;
        let rows = stmt.query_map(params![bot_id], |row| {
            Ok(crate::bots::BotMessage {
                id: row.get(0)?,
                from_bot_id: row.get(1)?,
                to_bot_id: row.get(2)?,
                body: row.get(3)?,
                created_at: parse_dt(row.get::<_, String>(4)?),
                read: row.get::<_, i64>(5)? != 0,
                conversation_id: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn mark_bot_messages_read(&self, bot_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE bot_messages SET read = 1 WHERE to_bot_id = ? AND read = 0",
            params![bot_id],
        )?;
        Ok(())
    }

    // ----- conversations (continued) -----

    /// Update a conversation's bot ownership. Used by the bot executor
    /// when it creates a fresh conversation for a scheduled run.
    pub fn set_conversation_bot(&self, conversation_id: &str, bot_id: Option<&str>) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE conversations SET bot_id = ? WHERE id = ?",
            params![bot_id, conversation_id],
        )?;
        Ok(())
    }

    /// Replace a message's tool_calls JSON and bump the conversation's
    /// updated_at. Called once at the end of a streaming turn, after the
    /// concatenated tool-call fragments are known.
    pub fn append_tool_calls(
        &self,
        message_id: &str,
        tool_calls: &[PersistedToolCall],
        conversation_id: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE messages SET tool_calls_json = ? WHERE id = ?",
            params![
                serde_json::to_string(tool_calls).unwrap_or_else(|_| "[]".to_string()),
                message_id
            ],
        )?;
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![Utc::now().to_rfc3339(), conversation_id],
        )?;
        Ok(())
    }

    // ----- settings -----

    pub fn load_settings(&self) -> rusqlite::Result<Settings> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = 'singleton'")?;
        let raw: Option<String> = stmt.query_row([], |row| row.get(0)).optional()?;
        match raw {
            Some(s) => serde_json::from_str(&s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
                )
            }),
            None => Ok(Settings::default()),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> rusqlite::Result<()> {
        let raw = serde_json::to_string(settings).map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )))
        })?;
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('singleton', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![raw],
        )?;
        Ok(())
    }
}

fn parse_dt(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Cheap wrapper that lets the FromStr impl for chrono errors flow through
/// where needed. Unused at the moment, but kept here so future call sites
/// don't repeat the `Box::new(io::Error::...)` pattern.
#[allow(dead_code)]
fn io_err<E: ToString>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}
