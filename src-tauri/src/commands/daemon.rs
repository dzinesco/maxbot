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

/// v3.7.3 — Push the current LLM settings to the
/// daemon's in-memory store. The Mac app calls this
/// on launch and on every Settings save so the
/// daemon-driven runs (webhooks + scheduler) can
/// reach the LLM while the Mac is closed.
///
/// The push is best-effort: if the daemon is
/// unreachable (laptop closed, daemon not yet
/// booted, network glitch) the function logs a
/// warning and returns `Ok(())` without surfacing
/// the error to the renderer. The Mac app's local
/// Bot runs still work — they use the locally-
/// stored `Settings.minimax_api_key` directly. The
/// daemon-driven runs will fail at the LLM call
/// with the v3.7.3 "no LLM key configured; Mac app
/// must POST /settings" error, which is surfaced
/// in the activity feed.
///
/// Auth: the daemon's `/settings` route uses a
/// per-Bot bearer token (same as `/shared`,
/// `/hooks/<id>`, `/bots/<id>/recent_runs`). The
/// Mac app picks any Bot's token from the local
/// `daemon_tokens` table — first by sorted
/// `created_at`, falling back to the first Bot
/// with a token. The `?bot_id=<id>` query param
/// pins the daemon's token-lookup row.
#[tauri::command]
pub async fn push_settings_to_daemon(
    state: State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let db = state.db.clone();
    let (settings, bot_id, token) = tokio::task::spawn_blocking(move || {
        let settings = db
            .load_settings()
            .map_err(|e| format!("db error reading settings: {e}"))?;
        // Find a Bot with a configured daemon token.
        // The daemon's `/settings` route uses the
        // per-Bot token (same as the other auth'd
        // routes), so the Mac app needs to send a
        // valid `bot_id`. Pick the most-recently
        // created Bot that has a token; if none
        // exists, the push is a no-op (the daemon
        // has no way to authenticate the request
        // anyway).
        let bots = db.list_bots().map_err(|e| format!("db error: {e}"))?;
        let mut chosen: Option<(String, String)> = None;
        for bot in &bots {
            if let Ok(Some(tok)) = db.get_daemon_token(&bot.id) {
                if !tok.is_empty() {
                    chosen = Some((bot.id.clone(), tok));
                    break;
                }
            }
        }
        let (bot_id, token) = match chosen {
            Some(pair) => pair,
            None => return Ok::<_, String>((settings, String::new(), String::new())),
        };
        Ok::<_, String>((settings, bot_id, token))
    })
    .await
    .map_err(|e| format!("settings push task panicked: {e}"))??;

    // No Bot with a token → can't push. This is
    // normal for a fresh install before the user
    // has opened a Bot editor and clicked
    // "Generate token". Skip silently.
    if token.is_empty() {
        log::debug!("push_settings_to_daemon: no bot with a daemon token, skipping");
        return Ok(());
    }

    // Build the request body. Sending `null` for
    // `minimax_api_key` would be a no-op on the
    // daemon side (the daemon only updates the
    // in-memory key when the field is `Some`),
    // but a missing field reads as "the Mac app
    // hasn't set one yet" — which is also a no-op.
    // The Mac app's launch + save flow always
    // has a value to push, so we send `Some(_)`.
    let body = serde_json::json!({
        "minimax_api_key": settings.minimax_api_key,
    });

    let url = if settings.maxbotd_url.trim().is_empty() {
        "http://127.0.0.1:8443/settings".to_string()
    } else {
        format!(
            "{}/settings",
            settings.maxbotd_url.trim().trim_end_matches('/')
        )
    };

    // Fire and forget. The Tauri command returns
    // `Ok(())` regardless of outcome so the
    // Mac app's launch + save flow isn't blocked
    // on a dead daemon. We log the result so an
    // operator can grep `maxbotd:` in journald or
    // the Tauri app's stdout and see what
    // happened.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
        .map_err(|e| format!("reqwest builder: {e}"))?;

    let resp = match client
        .post(&url)
        .query(&[("bot_id", bot_id.as_str())])
        .bearer_auth(&token)
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            log::warn!(
                "push_settings_to_daemon: POST {url} failed: {e} (daemon unreachable; daemon-driven runs will fail at the LLM call until the Mac app can reach the daemon)"
            );
            return Ok(());
        }
    };
    if !resp.status().is_success() {
        log::warn!(
            "push_settings_to_daemon: POST {url} returned {} (daemon rejected the push)",
            resp.status()
        );
    } else {
        log::info!("push_settings_to_daemon: POST {url} ok (key stored in memory)");
    }
    Ok(())
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
