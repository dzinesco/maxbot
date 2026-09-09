//! Bot commands: list/upsert/delete bots, manage schedules, trigger runs,
//! and read the inter-agent inbox. Mirrors the `conversations` module's
//! shape: cheap blocking work goes through `spawn_blocking`; the actual
//! LLM-backed execution lives in `bots::executor` and is invoked by
//! either the scheduler or the `run_bot_now` command.

use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, State};
use tauri_plugin_opener::OpenerExt;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::bots::executor::{run_bot_once, BotRunOutput};
use crate::bots::{Bot, BotMessage, BotRun, BotRunStatus, BotSchedule};
use crate::llm::provider::ToolDefinition;
use crate::tools::ToolRegistry;
use crate::AppState;

/// Tool descriptor the frontend uses to render the allowlist picker.
/// Mirrors the shape of `ToolDefinition` minus the JSON-Schema body, which
/// the UI doesn't need.
#[derive(serde::Serialize, Clone)]
pub struct ToolSummary {
    pub name: String,
    pub description: String,
    /// True if running this tool requires the user to OK it via the
    /// consent dialog. The frontend uses this to render a small "(consent
    /// required)" hint next to the tool name in the picker.
    pub requires_consent: bool,
}

#[tauri::command]
pub async fn list_bots(state: State<'_, AppState>) -> Result<Vec<Bot>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.list_bots().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_bot(state: State<'_, AppState>, id: String) -> Result<Option<Bot>, String> {
    let db = state.db.clone();
    let id_clone = id.clone();
    tokio::task::spawn_blocking(move || db.get_bot(&id_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn upsert_bot(state: State<'_, AppState>, bot: Bot) -> Result<Bot, String> {
    let mut bot = bot;
    if bot.id.is_empty() {
        bot.id = Bot::new_id();
    }
    if bot.created_at.timestamp() == 0 {
        bot.created_at = Utc::now();
    }
    bot.updated_at = Utc::now();
    let db = state.db.clone();
    let bot_clone = bot.clone();
    tokio::task::spawn_blocking(move || db.upsert_bot(&bot_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())??;
    Ok(bot)
}

#[tauri::command]
pub async fn delete_bot(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.delete_bot(&id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// v2.0 Slice E: set a Bot's persisted presence state. Called
/// from the bot executor (run start, run end, blocked) and from
/// the renderer's defensive path when it sees a `bot_run.status`
/// of `failed` (the executor will catch up shortly, but the
/// renderer can preempt the UI with this). The state string is
/// one of `idle`, `thinking`, `working`, `waiting`, `blocked`,
/// `done` — anything else is rejected with a clear error so a
/// future enum addition on the renderer side doesn't silently
/// write a garbage value.
#[tauri::command]
pub async fn bot_set_state(
    state: State<'_, AppState>,
    bot_id: String,
    bot_state: String,
) -> Result<(), String> {
    // Validate up front so the DB layer never sees a bad value
    // (the `as_str()` mapper would round-trip anything via the
    // default arm — but the call should be loud if a typo sneaks
    // in).
    let parsed = crate::bots::BotState::parse(&bot_state);
    if parsed.as_str() != bot_state.to_ascii_lowercase() {
        return Err(format!(
            "invalid bot state '{bot_state}'; expected one of idle, thinking, working, waiting, blocked, done"
        ));
    }
    let db = state.db.clone();
    let id = bot_id.clone();
    tokio::task::spawn_blocking(move || {
        db.set_bot_state(&id, parsed).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(())
}

/// v2.0 Slice E: fetch a Bot with the new presence fields
/// (`avatar_color`, `last_active_at`, `state`) populated.
/// Functionally equivalent to `get_bot` — kept as a separate
/// command so the renderer's roster code can ask for the
/// "full" view explicitly, and so future presence-only
/// columns can be added here without touching the existing
/// `get_bot` surface.
#[tauri::command]
pub async fn bot_get_with_state(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<Option<crate::bots::Bot>, String> {
    let db = state.db.clone();
    let id = bot_id.clone();
    tokio::task::spawn_blocking(move || db.get_bot(&id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn get_schedule(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<Option<BotSchedule>, String> {
    let db = state.db.clone();
    let id_clone = bot_id.clone();
    tokio::task::spawn_blocking(move || db.get_schedule(&id_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn upsert_schedule(
    state: State<'_, AppState>,
    schedule: BotSchedule,
) -> Result<BotSchedule, String> {
    let db = state.db.clone();
    let s_clone = schedule.clone();
    tokio::task::spawn_blocking(move || db.upsert_schedule(&s_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())??;
    Ok(schedule)
}

#[tauri::command]
pub async fn list_all_schedules(
    state: State<'_, AppState>,
) -> Result<Vec<BotSchedule>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.list_all_schedules().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_bot_runs(
    state: State<'_, AppState>,
    bot_id: String,
    limit: Option<u32>,
) -> Result<Vec<BotRun>, String> {
    let db = state.db.clone();
    let id_clone = bot_id.clone();
    let limit = limit.unwrap_or(20);
    tokio::task::spawn_blocking(move || {
        db.list_bot_runs(&id_clone, limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_inbox(
    state: State<'_, AppState>,
    bot_id: String,
    include_read: Option<bool>,
) -> Result<Vec<BotMessage>, String> {
    let db = state.db.clone();
    let id_clone = bot_id.clone();
    let include = include_read.unwrap_or(false);
    tokio::task::spawn_blocking(move || {
        db.list_inbox(&id_clone, include).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn mark_inbox_read(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let id_clone = bot_id.clone();
    tokio::task::spawn_blocking(move || {
        db.mark_bot_messages_read(&id_clone).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Returns the names of the tools available in the registry, plus a
/// human-readable description and whether they require consent. The
/// frontend uses this to render the allowlist picker in the bot editor.
#[tauri::command]
pub async fn list_available_tools(
    state: State<'_, AppState>,
) -> Result<Vec<ToolSummary>, String> {
    let registry = ToolRegistry::default_with_extras(state.mcp.tool_adapters());
    let defs: Vec<ToolDefinition> = registry.definitions();
    let summaries: Vec<ToolSummary> = defs
        .into_iter()
        .map(|d| {
            let name = d.function.name.clone();
            ToolSummary {
                name,
                description: d.function.description,
                requires_consent: registry.requires_consent(&d.function.name),
            }
        })
        .collect();
    Ok(summaries)
}

/// "Run now" — fires the bot once and returns when finished. The Rust
/// side emits `bot://chunk` / `bot://done` / `bot://error` events along
/// the way; the UI listens to them and shows the streaming text in the
/// chat view. The command itself just waits for the result so the
/// caller can surface success/failure as a toast.
#[tauri::command]
pub async fn run_bot_now(
    app: AppHandle,
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<BotRunOutput, String> {
    let db = state.db.clone();
    let id_for_lookup = bot_id.clone();
    let bot = tokio::task::spawn_blocking(move || db.get_bot(&id_for_lookup))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let Some(bot) = bot else {
        return Ok(BotRunOutput {
            run_id: uuid::Uuid::new_v4().to_string(),
            conversation_id: String::new(),
            status: BotRunStatus::Failed,
            result_summary: format!("no bot found with id {bot_id}"),
        });
    };
    let state_arc: Arc<AppState> = Arc::new(AppState {
        db: state.db.clone(),
        mcp: crate::mcp::McpRegistry::default(),
        bot_runs: state.bot_runs.clone(),
        computer: state.computer.clone(),
    });
    // The bot runs to completion here; cancel is a no-op for now (UI
    // doesn't yet expose a per-run Stop button).
    run_bot_once(app.clone(), state_arc, bot, CancellationToken::new()).await;
    // `run_bot_once` always returns the real run id and conversation
    // id, even on success. Re-fetch the latest run for this bot so the
    // UI gets a consistent snapshot.
    let db = state.db.clone();
    let id_for_run = bot_id.clone();
    let latest = tokio::task::spawn_blocking(move || db.list_bot_runs(&id_for_run, 1))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let last = latest.into_iter().next().ok_or_else(|| {
        "bot finished but no run row was persisted (this should not happen)"
            .to_string()
    })?;
    Ok(BotRunOutput {
        run_id: last.id,
        conversation_id: last.conversation_id,
        status: last.status,
        result_summary: last.result_summary,
    })
}

/// Cancel a running bot by id. Returns true if a run was found and
/// cancelled, false if no run is active with that id (e.g. it
/// finished between the UI rendering "running" and the user
/// clicking Stop). The cancellation is best-effort: the executor
/// checks between iterations and during tool calls, so a bot in the
/// middle of a tool call will be cancelled when the tool returns.
#[tauri::command]
pub async fn stop_bot_run(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<bool, String> {
    Ok(state.bot_runs.cancel(&run_id).await)
}

/// List run ids that are currently active (i.e. cancellable). The UI
/// uses this to know which bot rows in the panel have a live run
/// that a Stop button can fire against.
#[tauri::command]
pub async fn list_active_bot_runs(
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let _ = &state.bot_runs; // future: cross-reference
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.list_active_run_ids())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}

/// Sentinel from_bot_id used by user-sent messages in the bot inbox.
/// The executor's inbox-formatting recognizes this and shows "user"
/// instead of the raw sentinel.
pub const USER_SENDER: &str = "__user__";

/// Enqueue a message from the human user to a bot. If `trigger_run` is
/// true (default), the bot is also kicked off immediately so the user
/// sees the response. The bot reads the inbox on its next run start,
/// so a "send only" message is also fine — the bot will pick it up on
/// its next scheduled or manual run.
#[tauri::command]
pub async fn send_to_bot(
    app: AppHandle,
    state: State<'_, AppState>,
    to_bot_id: String,
    body: String,
    trigger_run: Option<bool>,
) -> Result<BotMessage, String> {
    let body = body.trim();
    if body.is_empty() {
        return Err("message body is empty".to_string());
    }
    let msg = BotMessage {
        id: Uuid::new_v4().to_string(),
        from_bot_id: USER_SENDER.to_string(),
        to_bot_id: to_bot_id.clone(),
        body: body.to_string(),
        created_at: Utc::now(),
        read: false,
        conversation_id: None,
    };
    state
        .db
        .enqueue_bot_message(&msg)
        .map_err(|e| e.to_string())?;
    if trigger_run.unwrap_or(true) {
        // Fire the bot. We do this in a background task so the
        // command returns quickly with the enqueued message; the UI
        // listens to bot://chunk/done/error and shows the response
        // stream as it arrives.
        let state_arc: Arc<AppState> = Arc::new(AppState {
            db: state.db.clone(),
            mcp: crate::mcp::McpRegistry::default(),
            bot_runs: state.bot_runs.clone(),
            computer: state.computer.clone(),
        });
        let db_for_lookup = state.db.clone();
        let id_for_lookup = to_bot_id.clone();
        let bot = tokio::task::spawn_blocking(move || {
            db_for_lookup.get_bot(&id_for_lookup)
        })
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
        if let Some(bot) = bot {
            tauri::async_runtime::spawn(async move {
                let _ = run_bot_once(
                    app,
                    state_arc,
                    bot,
                    CancellationToken::new(),
                )
                .await;
            });
        }
    }
    Ok(msg)
}

// ---- MCP ----

#[derive(serde::Serialize, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub tool_count: usize,
    pub tool_names: Vec<String>,
}

/// Reveal the bot's folder in Finder (or the host file manager).
/// The folder is created on first access via the filesystem helper,
/// so the user always sees the directory — even for a brand-new bot
/// that's never run.
#[tauri::command]
pub async fn reveal_bot_folder(
    app: AppHandle,
    bot_id: String,
) -> Result<String, String> {
    let dir = crate::bots::filesystem::bot_dir(&app, &bot_id)?;
    let path_str = dir.to_string_lossy().to_string();
    app.opener()
        .open_path(path_str.clone(), None::<&str>)
        .map_err(|e| format!("could not open {}: {e}", path_str))?;
    Ok(path_str)
}

/// List the MCP servers currently loaded and the tools they
/// contributed. Surfaced in the Settings → Computer Use / MCP panel.
#[tauri::command]
pub async fn list_mcp_servers(
    state: State<'_, AppState>,
) -> Result<Vec<McpServerInfo>, String> {
    let mut grouped: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for adapter in state.mcp.tool_adapters() {
        let full = adapter.name();
        if let Some(rest) = full.strip_prefix("mcp__") {
            if let Some((server, tool)) = rest.split_once("__") {
                grouped
                    .entry(server.to_string())
                    .or_default()
                    .push(tool.to_string());
            }
        }
    }
    Ok(grouped
        .into_iter()
        .map(|(name, mut tool_names)| {
            tool_names.sort();
            let tool_count = tool_names.len();
            McpServerInfo {
                name,
                tool_count,
                tool_names,
            }
        })
        .collect())
}
