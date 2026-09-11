//! Conversation + message CRUD commands. Cheap, blocking operations
//! dispatched onto the blocking pool so the Tauri command runtime stays free.

use tauri::State;

use crate::storage::{Conversation, Message, MessageRole, PersistedToolCall};
use crate::AppState;

#[tauri::command]
pub async fn list_conversations(
    state: State<'_, AppState>,
    bot_id: Option<String>,
) -> Result<Vec<Conversation>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        db.list_conversations(bot_id.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn create_conversation(
    state: State<'_, AppState>,
    title: Option<String>,
    bot_id: Option<String>,
) -> Result<Conversation, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        db.create_conversation(title, bot_id.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn delete_conversation(
    state: State<'_, AppState>,
    id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.delete_conversation(&id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn rename_conversation(
    state: State<'_, AppState>,
    id: String,
    title: String,
) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.rename_conversation(&id, &title).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_messages(
    state: State<'_, AppState>,
    conversation_id: String,
) -> Result<Vec<Message>, String> {
    let db = state.db.clone();
    let id = conversation_id;
    tokio::task::spawn_blocking(move || db.list_messages(&id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn search_messages(
    state: State<'_, AppState>,
    query: String,
    limit: Option<u32>,
) -> Result<Vec<Message>, String> {
    let db = state.db.clone();
    let q = query;
    let lim = limit.unwrap_or(100);
    tokio::task::spawn_blocking(move || {
        db.search_messages(&q, lim).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// v0.7.6 one-time migration: write the post-split shape of a
/// legacy "[error] …" assistant message. Sets `content` to the
/// streamed prefix and `error_message` to the friendly description
/// in one statement, so the next reload sees the clean shape
/// without re-running the suffix split. Called by the renderer's
/// `splitLegacyErrorSuffix` migration when it sees a legacy error
/// suffix on a message.
#[tauri::command]
pub async fn migrate_message_error_shape(
    state: State<'_, AppState>,
    message_id: String,
    content: String,
    error_message: String,
) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        db.migrate_message_to_error_shape(&message_id, &content, &error_message)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// `MessageRole` and `PersistedToolCall` are re-exported in case future
// commands need them. They are kept in this file to colocate all
// conversation-related commands.
#[allow(dead_code)]
fn _ensure_imports_used(_: MessageRole, _: PersistedToolCall) {}
