//! Bot commands: list/upsert/delete bots, manage schedules, trigger runs,
//! and read the inter-agent inbox. Mirrors the `conversations` module's
//! shape: cheap blocking work goes through `spawn_blocking`; the actual
//! LLM-backed execution lives in `bots::executor` and is invoked by
//! either the scheduler or the `run_bot_now` command.

use std::sync::Arc;

use chrono::Utc;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

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
pub async fn list_available_tools() -> Result<Vec<ToolSummary>, String> {
    let registry = ToolRegistry::default_set();
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
