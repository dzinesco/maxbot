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
                updated_at  TEXT NOT NULL
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
             );",
        )?;
        Ok(())
    }

    // ----- conversations -----

    pub fn list_conversations(&self) -> rusqlite::Result<Vec<Conversation>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at
             FROM conversations
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: parse_dt(row.get::<_, String>(2)?),
                updated_at: parse_dt(row.get::<_, String>(3)?),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn create_conversation(&self, title: Option<String>) -> rusqlite::Result<Conversation> {
        let now = Utc::now();
        let convo = Conversation {
            id: Uuid::new_v4().to_string(),
            title: title.unwrap_or_else(|| "New chat".to_string()),
            created_at: now,
            updated_at: now,
        };
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?, ?, ?, ?)",
            params![
                convo.id,
                convo.title,
                convo.created_at.to_rfc3339(),
                convo.updated_at.to_rfc3339(),
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
