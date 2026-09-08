//! Background scheduler. A tokio task that wakes up every 30 seconds,
//! asks the database for any bot whose schedule is due, and fires a
//! `run_bot_once` for each.
//!
//! The scheduler is a single instance; it lives on the Tauri AppHandle
//! and runs in the background until the process exits. Per-bot runs are
//! independent: a slow bot doesn't block the others.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tauri::{AppHandle, Manager};

use crate::bots::executor::run_bot_once;
use crate::bots::Bot;
use crate::AppState;

const TICK_SECONDS: u64 = 30;

/// Spawn the scheduler. Returns immediately; the actual loop runs in a
/// tokio task that lives for the lifetime of the process. The Tauri
/// app handle is captured so the task can run bots that emit events.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        scheduler_loop(app).await;
    });
}

async fn scheduler_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(Duration::from_secs(TICK_SECONDS)).await;
        let state = match app.try_state::<Arc<AppState>>() {
            Some(s) => s.inner().clone(),
            None => continue,
        };
        let due = match state.db.list_due_schedules(Utc::now()) {
            Ok(d) => d,
            Err(e) => {
                log::warn!("scheduler: list_due_schedules failed: {e}");
                continue;
            }
        };
        for schedule in due {
            // Look up the bot. If it was deleted between the list_due
            // and now, skip.
            let bot = match state.db.get_bot(&schedule.bot_id) {
                Ok(Some(b)) => b,
                _ => continue,
            };
            if schedule.interval_seconds == 0 {
                continue;
            }
            // Spawn the run as a separate task so a slow bot doesn't
            // block the next tick. The run persists its own
            // last_run_at / last_conversation_id when it finishes.
            let app_for_run = app.clone();
            let state_for_run = state.clone();
            let bot_clone = bot.clone();
            tauri::async_runtime::spawn(async move {
                log::info!(
                    "scheduler: firing bot {} ({}s interval)",
                    bot_clone.id,
                    schedule.interval_seconds
                );
                let cancel = tokio_util::sync::CancellationToken::new();
                let _ = run_bot_once(app_for_run, state_for_run, bot_clone, cancel).await;
            });
        }
    }
}

/// Convenience for tests and the future `/api/v1/...` path: list all
/// bots, with their schedule and the last run summary.
pub fn list_bots_with_schedule(
    state: &AppState,
) -> Vec<(Bot, Option<crate::bots::BotSchedule>, Option<crate::bots::BotRun>)> {
    let mut out = Vec::new();
    if let Ok(bots) = state.db.list_bots() {
        for bot in bots {
            let schedule = state.db.get_schedule(&bot.id).ok().flatten();
            let last_run = state
                .db
                .list_bot_runs(&bot.id, 1)
                .ok()
                .and_then(|mut r| r.pop());
            out.push((bot, schedule, last_run));
        }
    }
    out
}
