//! MaxBot Tauri application entry point.
//!
//! Wires the LLM client, SQLite-backed conversation store, and Tauri command
//! surface together. The webview (React/Vite) drives everything via the
//! `invoke` IPC; long-running streams emit `chat://chunk` events back to the
//! renderer so the UI can show tokens as they arrive.

// v2.3.0 — Routines: the `bots`, `skills`, `tools`,
// `mcp`, `computer`, and `storage` modules are `pub` so
// external integration tests (in
// `skills::scheduler_e2e`) can name them as
// `maxbot_lib::bots::...`. The `bots::scheduler::tick`
// and `skills::executor::run_skill_inner` are the
// scheduler-test surface.
pub mod approvals;
pub mod bots;
pub mod computer;
pub mod connectors;
pub mod groups;
pub mod memory;
pub mod mcp;
pub mod skills;
pub mod storage;
pub mod tools;
mod commands;
mod env_loader;
mod grok_build;
mod llm;

// Re-export so `use maxbot_lib::Settings;` works from
// tests and from the (currently non-existent) external
// API consumer.
pub use storage::Settings;

use std::sync::Arc;

use commands::chat::StreamRegistry;
use computer::ComputerManager;
use skills::recorder::RecorderState;
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
    /// v2.0 Slice B: the per-Bot ComputerManager. Built
    /// once from `Settings` on first use so a missing
    /// passphrase / host doesn't block startup.
    pub computer: Arc<ComputerManager>,
    /// v2.2.0 — Skill recorder. The Bot executor pushes
    /// every tool call into this state when a recording
    /// session is active; `skill_record_stop` drains the
    /// captured calls into a candidate Skill.
    pub recorder: Arc<RecorderState>,
    /// v3.7.3 — LLM key override. The Mac app pushes
    /// its `Settings.minimax_api_key` to the daemon on
    /// launch and on every Settings save; the daemon
    /// stores the key in memory only (never persisted)
    /// and shares this `Arc<RwLock>` into the executor's
    /// `AppState`. The executor checks this lock first
    /// when reading the API key; if `None`, it falls
    /// back to the DB-loaded `Settings.minimax_api_key`.
    /// The Mac app's own `AppState` initializes the
    /// lock to `None` — the in-memory override is a
    /// daemon-only path.
    pub llm_key_override: Arc<std::sync::RwLock<Option<String>>>,
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
            // v3.3.0 — Seed a placeholder "demo" Skill on
            // first launch so the new "Re-record" button
            // and "Last run" view have something to point
            // at out of the box. Idempotent: if any Skill
            // already exists we leave the install alone.
            // The seeded skill is a no-op-friendly
            // `web_fetch` step against a known small URL —
            // it demonstrates the recording/playback loop
            // without needing real network access to
            // succeed in any meaningful way.
            seed_demo_skill(&db);
            // v3.7.0 (Phase 8) — Seed the three
            // connector demo Skills
            // (`gmail-daily-summary` / `calendar-today`
            // / `github-my-issues`). Same idempotency
            // rule as the v3.3.0 skill: if any Skill
            // already exists (the v3.3.0 default), the
            // seed is a no-op. The new skills are
            // canaries for the Phase 8 connector
            // wiring; the user can run them from the
            // Skills panel to confirm the connector
            // tool is reachable.
            crate::skills::seeded::seed_connector_demo_skills(&db);
            // Read Settings once before `db` is moved into
            // the Arc so we can pass them to the
            // ComputerManager constructor.
            let initial_settings = db.load_settings().unwrap_or_default();
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
                // v3.7.3 — LLM key override lock. The Mac
                // app never sets this; the daemon sets it
                // to the key it received via `POST /settings`.
                // Initialized to `None` so the executor's
                // fallback (DB-loaded Settings) applies.
                llm_key_override: std::sync::Arc::new(std::sync::RwLock::new(None)),
                // v2.0 Slice B: build the ComputerManager
                // from the current Settings. The
                // manager is cheap to construct (it
                // holds Arc<SshPool> + a LibvirtClient
                // + a HashMap for active VNC proxies)
                // and lazy on first use, so this doesn't
                // block startup on the server being
                // reachable.
                computer: std::sync::Arc::new(ComputerManager::new(
                    &initial_settings,
                )),
                // v2.2.0 — Skill recorder. Empty by default;
                // `skill_record_start` allocates a session id
                // on demand.
                recorder: std::sync::Arc::new(RecorderState::new()),
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
            commands::conversations::migrate_message_error_shape,
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
            commands::bots::reveal_bot_folder,
            // v2.0 Slice E — Bot presence. The executor calls
            // `bot_set_state` at run start / run end / blocked;
            // the roster's avatar derives its visual from
            // `bot.state` (column) + the last `bot_run.status` +
            // `computers.state` (renderer-side). `bot_get_with_state`
            // returns the same shape as `get_bot` — kept as a
            // separate command so the renderer's "full presence
            // view" path can be wired independently of the
            // editor's "full Bot config" path.
            commands::bots::bot_set_state,
            commands::bots::bot_get_with_state,
            commands::tcc::list_controllable_apps,
            commands::tcc::request_tcc_for,
            commands::tcc::open_automation_settings,
            commands::tts::tts_speak,
            commands::tts::tts_stop,
            // v2.7.0 — Voice (bidirectional). Hold-to-talk
            // in the Composer; the STT call lands here. The
            // TTS commands above are the outbound half.
            commands::voice::transcribe_audio,
            commands::meta::meta_get,
            commands::meta::meta_set,
            commands::meta::meta_list,
            // v2.0 Slice B — per-Bot Computer Tauri
            // surface. The frontend (Slices C/D) calls
            // these; the actual VM work is in the
            // ComputerManager.
            commands::computer::computer_get,
            commands::computer::computer_provision,
            commands::computer::computer_start,
            commands::computer::computer_stop,
            commands::computer::computer_destroy,
            // v3.7.2: in-app preview is now a
            // `virsh screenshot` poll on the host. The
            // noVNC `computer_console_url` path is
            // gone. Takeover uses macOS `Screen Sharing`
            // via the existing `ssh -L` tunnel.
            commands::computer::computer_screenshot,
            commands::computer::computer_input_open,
            commands::computer::computer_input_event,
            commands::computer::computer_input_close,
            commands::computer::computer_takeover_open,
            commands::computer::computer_takeover_close,
            commands::computer::computer_test_connection,
            commands::computer::computer_file_list,
            commands::computer::computer_file_read,
            commands::computer::computer_file_write,
            // v2.3.5 — bootstrap-install the user's default
            // SSH public key into the VM via the QEMU guest
            // agent. Lets existing VMs switch to the
            // default-key path without Destroy.
            commands::computer::computer_install_default_key,
            // v2.2.0 — Skills. List/get/create/delete a
            // Skill, run one against a Bot, and
            // start/stop a recording session.
            commands::skills::skill_list,
            commands::skills::skill_get,
            commands::skills::skill_create,
            commands::skills::skill_update,
            commands::skills::skill_delete,
            commands::skills::skill_run,
            commands::skills::skill_run_cancel,
            commands::skills::skill_run_status,
            commands::skills::skill_run_history,
            commands::skills::skill_run_last_trace,
            commands::skills::skill_record_start,
            commands::skills::skill_record_stop,
            // v3.7.0 (Phase 8) — Connectors. Test
            // connection surfaces in the
            // BotEditor's Connectors section.
            // The actual connector tools
            // (`gmail_*` / `calendar_*` /
            // `github_*`) are dispatched via the
            // normal tool-call path; this command
            // is just the human "Test connection"
            // button.
            crate::connectors::connector_test,
            // v2.4.0 — Multi-Bot groups: list / get / create
            // groups, manage members, send messages, fetch
            // history, and run a single Bot in the group for
            // one turn (which streams via bot://chunk as
            // usual).
            commands::groups::group_list,
            commands::groups::group_get,
            commands::groups::group_create,
            commands::groups::group_add_member,
            commands::groups::group_remove_member,
            commands::groups::group_send,
            commands::groups::group_history,
            commands::groups::group_run_turn,
            // v2.5.0 — Persistent per-Bot memory.
            // JSONL on the Bot's VM; SFTP via the existing
            // SshPool. Read failures collapse to empty Vec.
            commands::memory::memory_search,
            commands::memory::memory_remember,
            commands::memory::memory_forget,
            commands::memory::memory_list,
            // v3.7.8 — back up a Bot's memory to the
            // host's shared/ tree so other Bots (or a
            // future restore) can read it.
            commands::memory::backup_memory,
            // v3.6.0 (Phase 7) — Reflect-step
            // companion. Thin alias for `memory_forget`
            // exposed under the brief's literal name.
            // Same SFTP path, same return shape. The
            // ActivityFeed's memory-write pill
            // dismiss button calls this to roll back
            // an auto-written fact/preference.
            commands::memory::delete_memory_entry,
            // v2.6.0 — Approval flows: per-Bot per-tool
            // auto/ask/deny rules, the pending queue, and
            // the Approve / Reject / Edit & send decision
            // surface. See `crate::approvals` and
            // `commands::approvals`.
            commands::approvals::approval_list,
            commands::approvals::approval_get,
            commands::approvals::approval_pending_count,
            commands::approvals::approval_rule_list,
            commands::approvals::approval_rule_set,
            commands::approvals::approval_decide,
            // v3.4.0 (Phase 5) — Takeover state
            // IPC. Lets the renderer poll per-Bot
            // takeover state on mount and on every
            // approval decide, so a daemon-driven
            // run that paused a Bot surfaces the
            // pending approval in the queue on next
            // app open.
            commands::approvals::bot_takeover_state,
            commands::approvals::list_paused_bots,
            // v3.4.0 (Phase 5) — Grok Bot defaults
            // reset. Re-applies the preset to an
            // existing Bot, overwriting any user
            // customizations.
            commands::bots::apply_grok_bot_defaults,
            // v2.8.0 — Always-on Daemon (24/7). ActivityFeed
            // data for the Sidebar + the per-Bot daemon
            // token (read / rotate) for the BotEditor.
            commands::daemon::list_recent_activity,
            commands::daemon::get_daemon_token,
            commands::daemon::rotate_daemon_token,
            // v3.7.3 — Push the Mac app's LLM settings
            // to the daemon on launch + on Settings
            // save. Best-effort: the function logs a
            // warning and returns `Ok(())` if the
            // daemon is unreachable, so the Mac app
            // continues to work even when the daemon
            // is down.
            commands::daemon::push_settings_to_daemon,
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
    let seed_optional_string = |field: &mut Option<String>,
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
    let seed_string = |field: &mut String, key: &str, log_label: &str| -> bool {
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
    changed |= seed_string(&mut settings.grok_build_binary, "MAXBOT_GROK_BUILD_BINARY", "MAXBOT_GROK_BUILD_BINARY");
    changed |= seed_string(&mut settings.grok_build_model, "MAXBOT_GROK_BUILD_MODEL", "MAXBOT_GROK_BUILD_MODEL");
    changed |= seed_string(&mut settings.grok_cwd, "MAXBOT_GROK_CWD", "MAXBOT_GROK_CWD");

    if changed {
        if let Err(e) = db.save_settings(&settings) {
            log::warn!("env_loader: could not save seeded settings: {e}");
        }
    }
}

/// v3.3.0 — Seed a placeholder "demo" Skill on first
/// launch. Idempotent: if any Skill already exists we
/// leave the install alone (we don't want to clobber a
/// user's real Skills with a re-seed). The seeded Skill
/// is a single-step `web_fetch` against
/// `https://example.com` — it demonstrates the
/// recording/playback loop and the new Re-record
/// button. Users can re-record it (click Re-record,
/// edit the JSON, Update) to make it their own; the
/// seeded `name`/`description`/step are all editable
/// in place.
fn seed_demo_skill(db: &Database) {
    // Cheap existence check: if any Skill row is in the
    // table, the user has already used the Skills panel
    // and we should not seed over their work. This is
    // deliberately `count > 0`, not "id == demo-skill",
    // so a user who deleted the demo skill and re-launched
    // the app doesn't get a surprise re-seed.
    let already_seeded = db
        .list_skills()
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if already_seeded {
        return;
    }
    let now = chrono::Utc::now();
    let demo = crate::skills::Skill {
        id: "demo-maxbot-daily-checkin".to_string(),
        name: "maxbot-daily-checkin".to_string(),
        description:
            "Demo Skill shipped with v3.3.0. Re-record it \
             (click Re-record on the skill row) to make it \
             your own. Edit the JSON to point at a URL you \
             actually care about, then click Update."
                .to_string(),
        inputs: Vec::new(),
        steps: vec![crate::skills::Step {
            tool: "web_fetch".to_string(),
            args: serde_json::json!({
                "url": "https://example.com",
            }),
            output_var: None,
        }],
        created_at: now,
        updated_at: now,
    };
    if let Err(e) = db.upsert_skill(&demo) {
        log::warn!("seed_demo_skill: upsert_skill failed: {e}");
        return;
    }
    log::info!("seed_demo_skill: seeded `maxbot-daily-checkin`");
}
