//! v2.8.0 — Always-on Daemon (24/7). Tauri IPC surface
//! for the daemon's per-Bot bearer tokens and the
//! ActivityFeed data the Sidebar polls.
//!
//! The Tauri app and `maxbotd` share the same SQLite
//! file (and the same `Database` wrapper), so reads
//! from the Mac app see whatever the daemon wrote
//! moments ago. The 10-second Sidebar poll is plenty
//! for a "what fired while I was away" feed.
//!
//! TLS, the outbound IPC back to the Mac for native
//! notifications, and the VM-side `bots/<id>/daemon.json`
//! token storage are all deferred to v2.8.1 / v3.0.

use std::sync::Arc;

use serde::Serialize;
use tauri::State;

use crate::approvals::Approval;
use crate::bots::BotRun;
use crate::skills::SkillRun;
use crate::AppState;

/// Bundle of "what's been happening" rows the
/// ActivityFeed renders. Three sections, each capped at
/// 5 rows. All times are RFC-3339; the renderer formats
/// them with `Intl.RelativeTimeFormat`.
#[derive(Debug, Clone, Serialize)]
pub struct ActivityFeed {
    pub bot_runs: Vec<BotRun>,
    pub skill_runs: Vec<SkillRun>,
    pub approvals: Vec<Approval>,
}

const ACTIVITY_LIMIT: u32 = 5;

/// Return the three-section activity bundle for the
/// Sidebar's ActivityFeed. Cheap: three small `LIMIT 5`
/// queries against the same SQLite file. Called by the
/// ActivityFeed's mount + 10s `setInterval`.
#[tauri::command]
pub async fn list_recent_activity(
    state: State<'_, Arc<AppState>>,
) -> Result<ActivityFeed, String> {
    let db = state.db.clone();
    let bot_runs = tokio::task::spawn_blocking(move || db.list_recent_bot_runs(ACTIVITY_LIMIT))
        .await
        .map_err(|e| format!("list_recent_bot_runs task: {e}"))?
        .map_err(|e| format!("list_recent_bot_runs: {e}"))?;
    let db = state.db.clone();
    let skill_runs = tokio::task::spawn_blocking(move || db.list_recent_skill_runs(ACTIVITY_LIMIT))
        .await
        .map_err(|e| format!("list_recent_skill_runs task: {e}"))?
        .map_err(|e| format!("list_recent_skill_runs: {e}"))?;
    let db = state.db.clone();
    let approvals = tokio::task::spawn_blocking(move || db.list_recent_approvals(ACTIVITY_LIMIT))
        .await
        .map_err(|e| format!("list_recent_approvals task: {e}"))?
        .map_err(|e| format!("list_recent_approvals: {e}"))?;
    Ok(ActivityFeed {
        bot_runs,
        skill_runs,
        approvals,
    })
}

/// Fetch the per-Bot bearer token for the
/// `maxbotd` HTTP server. `None` means the user hasn't
/// generated one yet — the BotEditor surfaces this as
/// "(not set)" and a Generate button.
#[tauri::command]
pub async fn get_daemon_token(
    bot_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<Option<String>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.get_daemon_token(&bot_id))
        .await
        .map_err(|e| format!("get_daemon_token task: {e}"))?
        .map_err(|e| format!("get_daemon_token: {e}"))
}

/// Generate (or rotate) a fresh per-Bot bearer token,
/// write it to `daemon_tokens`, and return the new value
/// for the UI to display + put on the clipboard. The
/// old token is invalidated immediately; if the user
/// had any inbound webhooks using it, they'll start
/// returning 401.
#[tauri::command]
pub async fn rotate_daemon_token(
    bot_id: String,
    state: State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.rotate_daemon_token(&bot_id))
        .await
        .map_err(|e| format!("rotate_daemon_token task: {e}"))?
        .map_err(|e| format!("rotate_daemon_token: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ActivityFeed's `ACTIVITY_LIMIT` is a
    /// small, bounded number. Pin it: a future bump
    /// to 50 would balloon the renderer's payload.
    /// 5 is the right "what fired while I was away"
    /// granularity.
    #[test]
    fn activity_limit_is_bounded() {
        assert!(ACTIVITY_LIMIT <= 10);
        assert!(ACTIVITY_LIMIT >= 1);
    }
}
