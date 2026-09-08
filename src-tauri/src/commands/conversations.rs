//! Conversation + message CRUD commands. Cheap, blocking operations
//! dispatched onto the blocking pool so the Tauri command runtime stays free.

use tauri::State;

use crate::storage::{Conversation, Message, MessageRole, PersistedToolCall};
use crate::AppState;

#[tauri::command]
pub async fn list_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<Conversation>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.list_conversations().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn create_conversation(
    state: State<'_, AppState>,
    title: Option<String>,
) -> Result<Conversation, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.create_conversation(title).map_err(|e| e.to_string()))
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

// `MessageRole` and `PersistedToolCall` are re-exported in case future
// commands need them. They are kept in this file to colocate all
// conversation-related commands.
#[allow(dead_code)]
fn _ensure_imports_used(_: MessageRole, _: PersistedToolCall) {}
