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
mod mcp;
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
    pub mcp: mcp::McpRegistry,
    pub bot_runs: bots::registry::SharedBotRunRegistry,
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
            // Load MCP servers from mcp_servers.json. We block on the
            // async load here so the registry is populated by the time
            // the first chat message arrives. Servers are local
            // processes, so the load completes in well under a second
            // for typical configs.
            let mcp_config_path = data_dir.join("mcp_servers.json");
            let mcp_registry = tauri::async_runtime::block_on(async {
                mcp::load_from_config_path(&mcp_config_path).await
            })
            .unwrap_or_else(|e| {
                log::warn!("mcp: registry load failed: {e}");
                mcp::McpRegistry::default()
            });
            log::info!(
                "mcp: {} server(s) loaded",
                mcp_registry.server_count()
            );
            app.manage(AppState {
                db: Arc::new(db),
                mcp: mcp_registry,
                bot_runs: std::sync::Arc::new(bots::registry::BotRunRegistry::new()),
            });
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
            commands::conversations::search_messages,
            commands::chat::send_message,
            commands::chat::stop_message,
            commands::chat::regenerate_last,
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
            commands::bots::list_mcp_servers,
            commands::bots::stop_bot_run,
            commands::bots::list_active_bot_runs,
            commands::tcc::list_controllable_apps,
            commands::tcc::request_tcc_for,
            commands::tcc::open_automation_settings,
            commands::tts::tts_speak,
            commands::tts::tts_stop,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MaxBot");
}

/// Populate the SQLite settings row with values from the .env file, but
/// only for fields the user hasn't already set. Recognized keys:
///
/// - `MINIMAX_API_KEY` → `Settings::minimax_api_key`
/// - `OPENAI_API_KEY` → `Settings::openai_api_key`
/// - `ANTHROPIC_API_KEY` → `Settings::anthropic_api_key`
/// - `XAI_API_KEY` → `Settings::xai_api_key`
/// - `SAND_MINIMAX_BASE_URL` → `Settings::minimax_base_url`
/// - `SAND_MINIMAX_MODEL` → `Settings::default_model`
/// - `MAXBOT_PROVIDER` → `Settings::provider_kind`
/// - `MAXBOT_TTS_VOICE` → `Settings::tts_voice`
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

    // Helper: if the SQLite field is empty, take from .env.
    let mut seed_optional_string = |field: &mut Option<String>,
                                    key: &str,
                                    log_label: &str|
     -> bool {
        if field.as_deref().map_or(true, str::is_empty) {
            if let Some(v) = env.get(key) {
                if !v.is_empty() {
                    log::info!("env_loader: seeded {log_label} from .env");
                    *field = Some(v.clone());
                    return true;
                }
            }
        }
        false
    };
    let mut seed_string = |field: &mut String, key: &str, log_label: &str| -> bool {
        if field.is_empty() {
            if let Some(v) = env.get(key) {
                if !v.is_empty() {
                    log::info!("env_loader: seeded {log_label} from .env");
                    *field = v.clone();
                    return true;
                }
            }
        }
        false
    };

    changed |= seed_optional_string(&mut settings.minimax_api_key, "MINIMAX_API_KEY", "MINIMAX_API_KEY");
    changed |= seed_optional_string(&mut settings.openai_api_key, "OPENAI_API_KEY", "OPENAI_API_KEY");
    changed |= seed_optional_string(&mut settings.anthropic_api_key, "ANTHROPIC_API_KEY", "ANTHROPIC_API_KEY");
    changed |= seed_optional_string(&mut settings.xai_api_key, "XAI_API_KEY", "XAI_API_KEY");
    changed |= seed_string(&mut settings.minimax_base_url, "SAND_MINIMAX_BASE_URL", "SAND_MINIMAX_BASE_URL");
    changed |= seed_string(&mut settings.default_model, "SAND_MINIMAX_MODEL", "SAND_MINIMAX_MODEL");
    changed |= seed_string(&mut settings.provider_kind, "MAXBOT_PROVIDER", "MAXBOT_PROVIDER");
    changed |= seed_string(&mut settings.tts_voice, "MAXBOT_TTS_VOICE", "MAXBOT_TTS_VOICE");

    if changed {
        if let Err(e) = db.save_settings(&settings) {
            log::warn!("env_loader: could not save seeded settings: {e}");
        }
    }
}
