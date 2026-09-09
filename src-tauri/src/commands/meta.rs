//! Meta key/value commands: read/write the small flat table that holds
//! first-run flags and other non-settings state. Separate from the
//! `settings` blob because meta is a key/value shape with no schema
//! migration story — just write whatever string the caller wants.

use tauri::State;

use crate::AppState;

#[tauri::command]
pub async fn meta_get(
    state: State<'_, AppState>,
    key: String,
) -> Result<Option<String>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.meta_get(&key).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn meta_set(
    state: State<'_, AppState>,
    key: String,
    value: String,
) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.meta_set(&key, &value).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn meta_list(state: State<'_, AppState>) -> Result<Vec<(String, String)>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.meta_list().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}
