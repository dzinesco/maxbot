//! MaxBot Tauri application entry point.
//!
//! Wires the LLM client, SQLite-backed conversation store, and Tauri command
//! surface together. The webview (React/Vite) drives everything via the
//! `invoke` IPC; long-running streams emit `chat://chunk` events back to the
//! renderer so the UI can show tokens as they arrive.

mod bots;
mod commands;
mod env_loader;
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
        .plugin(tauri_plugin_opener::init())
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
            // Seed the SQLite settings from the .env file on first launch.
            // Once the user has used the Settings UI, those values win
            // (we only seed when the SQLite field is empty). Recognized
            // keys: MINIMAX_API_KEY, SAND_MINIMAX_BASE_URL,
            // SAND_MINIMAX_MODEL.
            seed_settings_from_env(&db);
            app.manage(AppState { db: Arc::new(db) });
            app.manage(Arc::new(AsyncMutex::new(StreamRegistry::default())));
            // Start the bot scheduler. It runs in a background tokio task
            // for the lifetime of the process, waking every 30s to fire
            // any bot whose schedule is due.
            bots::scheduler::spawn(app.handle().clone());
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
            commands::bots::list_bots,
            commands::bots::get_bot,
            commands::bots::upsert_bot,
            commands::bots::delete_bot,
            commands::bots::get_schedule,
            commands::bots::upsert_schedule,
            commands::bots::list_all_schedules,
            commands::bots::list_bot_runs,
            commands::bots::list_inbox,
            commands::bots::mark_inbox_read,
            commands::bots::list_available_tools,
            commands::bots::run_bot_now,
            commands::bots::send_to_bot,
            commands::tcc::list_controllable_apps,
            commands::tcc::request_tcc_for,
            commands::tcc::open_automation_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MaxBot");
}

/// Populate the SQLite settings row with values from the .env file, but
/// only for fields the user hasn't already set. Recognized keys:
///
/// - `MINIMAX_API_KEY` → `Settings::minimax_api_key`
/// - `SAND_MINIMAX_BASE_URL` → `Settings::base_url`
/// - `SAND_MINIMAX_MODEL` → `Settings::default_model`
///
/// We deliberately do *not* overwrite an existing SQLite value with the
/// env value: once the user has set a key via the Settings UI, that
/// key is theirs. The .env is a developer-friendly seed, not a runtime
/// override.
fn seed_settings_from_env(db: &Database) {
    let env = env_loader::load_dotenv();
    if env.is_empty() {
        return;
    }
    let mut settings = match db.load_settings() {
        Ok(s) => s,
        Err(e) => {
            log::warn!("env_loader: could not load settings: {e}");
            return;
        }
    };
    let mut changed = false;
    if settings
        .minimax_api_key
        .as_deref()
        .map_or(true, str::is_empty)
    {
        if let Some(v) = env.get("MINIMAX_API_KEY") {
            if !v.is_empty() {
                log::info!("env_loader: seeded MINIMAX_API_KEY from .env");
                settings.minimax_api_key = Some(v.clone());
                changed = true;
            }
        }
    }
    if settings.base_url.is_empty() {
        if let Some(v) = env.get("SAND_MINIMAX_BASE_URL") {
            if !v.is_empty() {
                log::info!("env_loader: seeded SAND_MINIMAX_BASE_URL from .env");
                settings.base_url = v.clone();
                changed = true;
            }
        }
    }
    if settings.default_model.is_empty() {
        if let Some(v) = env.get("SAND_MINIMAX_MODEL") {
            if !v.is_empty() {
                log::info!("env_loader: seeded SAND_MINIMAX_MODEL from .env");
                settings.default_model = v.clone();
                changed = true;
            }
        }
    }
    if changed {
        if let Err(e) = db.save_settings(&settings) {
            log::warn!("env_loader: could not save seeded settings: {e}");
        }
    }
}
