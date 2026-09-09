//! v2.4.0 — Tauri command surface for multi-Bot groups.
//!
//! Thin IPC shims over `crate::groups` + the storage
//! methods on `Database`. The size validation (2-6
//! members) lives here, not in the DB layer, so the
//! storage code can stay generic.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::groups;
use crate::storage::db::{GroupChatWithMembers, GroupMessage};
use crate::AppState;

#[tauri::command]
pub async fn group_list(
    state: State<'_, AppState>,
) -> Result<Vec<GroupChatWithMembers>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.list_groups().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn group_get(
    state: State<'_, AppState>,
    group_id: String,
) -> Result<Option<GroupChatWithMembers>, String> {
    let db = state.db.clone();
    let id = group_id.clone();
    tokio::task::spawn_blocking(move || db.get_group(&id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Create a new group. Rejects sizes outside the
/// inclusive 2-6 range (counting the owner as member 1).
#[tauri::command]
pub async fn group_create(
    state: State<'_, AppState>,
    name: String,
    owner_bot_id: String,
    member_bot_ids: Vec<String>,
) -> Result<GroupChatWithMembers, String> {
    // Total members = owner (1) + additional. We dedupe
    // so a caller that accidentally re-passes the owner
    // id doesn't get rejected for being too big.
    let mut all: Vec<String> = Vec::with_capacity(member_bot_ids.len() + 1);
    all.push(owner_bot_id.clone());
    for m in &member_bot_ids {
        if !all.iter().any(|x| x == m) {
            all.push(m.clone());
        }
    }
    groups::validate_group_size(all.len()).map_err(|e| e.to_string())?;
    let db = state.db.clone();
    let name_clone = name.clone();
    let owner_clone = owner_bot_id.clone();
    tokio::task::spawn_blocking(move || {
        db.create_group(&name_clone, &owner_clone, &member_bot_ids)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn group_add_member(
    state: State<'_, AppState>,
    group_id: String,
    bot_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let gid = group_id.clone();
    let bid = bot_id.clone();
    tokio::task::spawn_blocking(move || {
        db.add_group_member(&gid, &bid)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn group_remove_member(
    state: State<'_, AppState>,
    group_id: String,
    bot_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let gid = group_id.clone();
    let bid = bot_id.clone();
    tokio::task::spawn_blocking(move || {
        db.remove_group_member(&gid, &bid)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Append a user-sent message to a group transcript.
/// Returns the persisted `GroupMessage` so the renderer
/// can immediately append it to the local transcript
/// without an extra fetch.
#[tauri::command]
pub async fn group_send(
    state: State<'_, AppState>,
    group_id: String,
    body: String,
    mentions: Option<Vec<String>>,
) -> Result<GroupMessage, String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err("message body is empty".to_string());
    }
    let mentions = mentions.unwrap_or_default();
    let db = state.db.clone();
    let gid = group_id.clone();
    let body = trimmed.to_string();
    let mentions_clone = mentions.clone();
    tokio::task::spawn_blocking(move || {
        db.append_group_message(&gid, None, "user", &body, &mentions_clone, None)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Last `limit` messages for a group, oldest-first. A
/// limit of 0 returns every message.
#[tauri::command]
pub async fn group_history(
    state: State<'_, AppState>,
    group_id: String,
    limit: Option<u32>,
) -> Result<Vec<GroupMessage>, String> {
    let db = state.db.clone();
    let gid = group_id.clone();
    let limit = limit.unwrap_or(50);
    tokio::task::spawn_blocking(move || {
        db.list_group_messages(&gid, limit)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Run one Bot in the group for one turn. Returns the
/// new assistant message id. The `bot://chunk` /
/// `bot://done` / `bot://error` events fire as usual
/// during the run, so the renderer can stream the
/// response.
#[tauri::command]
pub async fn group_run_turn(
    app: AppHandle,
    state: State<'_, AppState>,
    group_id: String,
    bot_id: String,
    handoff_from_message_id: Option<String>,
) -> Result<String, String> {
    let state_arc: Arc<AppState> = Arc::new(AppState {
        db: state.db.clone(),
        mcp: state.mcp.clone(),
        bot_runs: state.bot_runs.clone(),
        computer: state.computer.clone(),
        recorder: state.recorder.clone(),
    });
    groups::run_group_turn(app, state_arc, group_id, bot_id, handoff_from_message_id).await
}
