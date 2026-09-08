//! Settings commands: get/save the local settings blob (API key, default
//! model, base URL).

use tauri::State;

use crate::storage::Settings;
use crate::AppState;

#[tauri::command]
pub async fn get_settings(state: State<'_, AppState>) -> Result<Settings, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.load_settings().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn save_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.save_settings(&settings).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Wipe the MiniMax API key from disk.
#[tauri::command]
pub async fn delete_api_key(state: State<'_, AppState>) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        let mut current = db.load_settings().map_err(|e| e.to_string())?;
        current.minimax_api_key = None;
        db.save_settings(&current).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
