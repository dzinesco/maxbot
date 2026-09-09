//! SQLite-backed storage for conversations, messages, and app settings.
//!
//! Schema is created on first open. All writes go through a single
//! connection guarded by a Mutex; reads are short enough that contention
//! isn't a real concern for a single-user chat client.

pub mod db;

pub use db::{Conversation, Database, Message, MessageRole, PersistedToolCall, Settings, SshKeyRow};
