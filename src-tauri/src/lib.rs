//! MaxBot Tauri application entry point.
//!
//! Wires the LLM client, SQLite-backed conversation store, and Tauri command
//! surface together. The webview (React/Vite) drives everything via the
//! `invoke` IPC; long-running streams emit `chat://chunk` events back to the
//! renderer so the UI can show tokens as they arrive.

mod commands;
mod llm;
mod storage;
mod tools;

use std::sync::Arc;

use commands::chat::StreamRegistry;
use storage::Database;
use tauri::Manager;
use tauri::async_runtime::Mutex as AsyncMutex;

/// Shared application state. The `Database` is `Send + Sync` and lives for
/// the whole process; the LLM client is constructed lazily on first use so
/// that we never block startup on an outbound HTTP call.
pub struct AppState {
    pub db: Arc<Database>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // Resolve the per-user data directory. On macOS this is
            // ~/Library/Application Support/com.maxbot.app/.
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("app data dir should be resolvable on a supported platform");
            std::fs::create_dir_all(&data_dir)
                .expect("create MaxBot data dir");
            let db_path = data_dir.join("maxbot.sqlite");
            let db = Database::open(&db_path).expect("open MaxBot sqlite database");
            app.manage(AppState { db: Arc::new(db) });
            app.manage(Arc::new(AsyncMutex::new(StreamRegistry::default())));
            log::info!("MaxBot ready; data dir = {}", data_dir.display());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::get_settings,
            commands::settings::save_settings,
            commands::settings::delete_api_key,
            commands::conversations::list_conversations,
            commands::conversations::create_conversation,
            commands::conversations::delete_conversation,
            commands::conversations::rename_conversation,
            commands::conversations::get_messages,
            commands::chat::send_message,
            commands::chat::stop_message,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MaxBot");
}
