//! Background scheduler. A tokio task that wakes up every 30 seconds,
//! asks the database for any bot whose schedule is due, and fires a
//! `run_bot_once` for each.
//!
//! A bot is "due" if EITHER:
//! - `cron_expression` is set and the cron's next firing time falls
//!   at or before `now` (with `last_run_at` as the reference); or
//! - `interval_seconds` is > 0 and `now - last_run_at >= interval_seconds`.
//!
//! When both are set, cron wins. When both are empty/0, the bot never
//! fires. Cron expressions are evaluated in the bot's local timezone —
//! we pass a `Local`-flavored `DateTime<Utc>` cast to the parser, so
//! "0 9 * * 1-5" means "9 AM local on weekdays" regardless of the
//! user's UTC offset.
//!
//! The scheduler is a single instance; it lives on the Tauri AppHandle
//! and runs in the background until the process exits. Per-bot runs are
//! independent: a slow bot doesn't block the others.

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, TimeZone, Utc};
use cron::Schedule;
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
        let now_utc = Utc::now();
        let due = match state.db.list_due_schedules(now_utc) {
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
            if !is_due(&schedule, now_utc) {
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
                    "scheduler: firing bot {} (cron='{}', interval={}s)",
                    bot_clone.id,
                    schedule.cron_expression,
                    schedule.interval_seconds
                );
                let cancel = tokio_util::sync::CancellationToken::new();
                let _ = run_bot_once(app_for_run, state_for_run, bot_clone, cancel).await;
            });
        }
    }
}

/// Decide whether a schedule is due right now. Cron takes precedence
/// when set; otherwise falls back to the simple interval-seconds
/// comparison.
fn is_due(schedule: &crate::bots::BotSchedule, now_utc: DateTime<Utc>) -> bool {
    if !schedule.cron_expression.is_empty() {
        return is_due_cron(&schedule.cron_expression, schedule.last_run_at, now_utc);
    }
    if schedule.interval_seconds > 0 {
        let last = schedule.last_run_at.unwrap_or_else(|| {
            // Never run before → due immediately. We use a long-ago
            // timestamp so the simple interval check passes.
            DateTime::<Utc>::from_timestamp(0, 0).unwrap()
        });
        let elapsed = (now_utc - last).num_seconds();
        return elapsed >= schedule.interval_seconds as i64;
    }
    false
}

fn is_due_cron(expr: &str, last_run_at: Option<DateTime<Utc>>, now_utc: DateTime<Utc>) -> bool {
    // The `cron` crate's `Schedule::from_str` expects a 6-field
    // expression (with seconds prepended). Standard 5-field cron
    // (the format users see in tutorials and our UI) needs a "0 "
    // prepended to put it in 6-field shape.
    let expr6 = if expr.split_whitespace().count() == 5 {
        format!("0 {}", expr)
    } else {
        expr.to_string()
    };
    let schedule = match Schedule::from_str(&expr6) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("scheduler: invalid cron expression '{}': {}", expr, e);
            return false;
        }
    };
    // Anchor in local time so "0 9 * * 1-5" is "9 AM local on weekdays"
    // for the user, not 9 AM UTC.
    let now_local: DateTime<Local> = now_utc.with_timezone(&Local);
    // If the bot has never run, treat it as if it last ran 7 days
    // ago. That way a "weekdays at 9 AM" bot is considered "due" the
    // first time we see it on a Tuesday at 9:30 AM, but not at 8:30
    // AM (next fire is at 9 AM). For weekly schedules this gives the
    // bot one chance to fire on the same calendar day; for faster
    // schedules it's a no-op since the next firing is minutes away.
    let prev_local: DateTime<Local> = match last_run_at {
        Some(utc) => utc.with_timezone(&Local),
        None => now_local - chrono::Duration::days(7),
    };
    // Count the number of cron firings strictly between prev and now.
    // If > 0, the bot is due. This handles missed firings correctly: if
    // the bot was off for 3 days and the cron is "every minute", we
    // fire it once now and skip the backlog (the next tick will fire
    // it again 1 minute later).
    let count = schedule
        .after(&prev_local)
        .take_while(|t| *t <= now_local)
        .count();
    count > 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn sched(cron: &str, interval: u32) -> crate::bots::BotSchedule {
        crate::bots::BotSchedule {
            bot_id: "test".to_string(),
            interval_seconds: interval,
            cron_expression: cron.to_string(),
            last_run_at: None,
            last_conversation_id: None,
        }
    }

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn interval_seconds_due_when_elapsed() {
        let now = at(2026, 9, 8, 12, 0);
        let mut s = sched("", 60);
        s.last_run_at = Some(at(2026, 9, 8, 11, 59));
        assert!(is_due(&s, now));
        s.last_run_at = Some(at(2026, 9, 8, 12, 0));
        assert!(!is_due(&s, now));
    }

    #[test]
    fn interval_seconds_never_run_is_due() {
        let now = at(2026, 9, 8, 12, 0);
        let s = sched("", 60);
        assert!(is_due(&s, now));
    }

    #[test]
    fn cron_weekdays_at_9am() {
        // 2026-09-08 is a Tuesday. Schedule: "0 9 * * 1-5" (weekdays at
        // 9 AM local). The scheduler accepts 5-field standard cron and
        // internally prepends a "0 " second field.
        let now = at(2026, 9, 8, 9, 30);
        let s = sched("0 9 * * 1-5", 0);
        // Never run → due (because the cron would have fired today
        // in the past 7 days).
        assert!(is_due(&s, now));
        // Last ran today at 9:00 AM → not yet due (next fire is
        // tomorrow).
        let mut s2 = sched("0 9 * * 1-5", 0);
        s2.last_run_at = Some(at(2026, 9, 8, 9, 0));
        assert!(!is_due(&s2, now));
    }

    #[test]
    fn cron_with_invalid_expression_is_not_due() {
        let now = at(2026, 9, 8, 12, 0);
        let s = sched("not a cron", 0);
        assert!(!is_due(&s, now));
    }

    #[test]
    fn cron_wins_over_interval() {
        // Both cron and interval set — cron takes precedence. The
        // cron's next fire is tomorrow at 9 AM, so the bot is not due
        // at noon today.
        let now = at(2026, 9, 8, 12, 0);
        let mut s = sched("0 9 * * 1-5", 3600);
        s.last_run_at = Some(at(2026, 9, 8, 11, 0));
        assert!(!is_due(&s, now));
    }

    #[test]
    fn no_schedule_is_not_due() {
        let now = at(2026, 9, 8, 12, 0);
        let s = sched("", 0);
        assert!(!is_due(&s, now));
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
