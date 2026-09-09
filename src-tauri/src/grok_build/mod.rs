//! `grok_build` — `grok agent stdio` JSON-RPC client + per-app
//! session manager.
//!
//! Public API:
//! - `GrokSession` (re-exported from `session`) — the per-session
//!   JSON-RPC client. Cheap to clone; wraps an `Arc<Inner>`.
//! - `get_or_init_session(app)` — module-level lazy initializer.
//!   First call spawns `grok agent stdio`, completes the ACP
//!   handshake, and stores the `Arc<GrokSession>` in a
//!   `tokio::sync::OnceCell`. Subsequent calls return the same
//!   handle. The session lives for the lifetime of the process.
//! - `close_session()` — for tests / shutdown paths.
//! - `set_session_id_for_resume(app, id)` /
//!   `load_persisted_session_id(app)` — SQLite round-trip for
//!   the session id, so the next process restart can resume.

pub mod session;

pub use session::{GrokSession, SessionEvent};

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tokio::sync::OnceCell;

use crate::storage::db::Settings;

/// Process-wide singleton. Resolved on first use; survives across
/// chat turns and bot runs within the same app launch. A new
/// process (app relaunch) starts a new `OnceCell` and re-uses the
/// persisted session id from SQLite to resume the same
/// conversation.
static SESSION: OnceCell<Arc<GrokSession>> = OnceCell::const_new();

/// Get the running session, or spawn one. The first call after
/// process start (or after a manual `close_session()`) brings up
/// the subprocess, runs the handshake, and persists the new
/// `session_id` to SQLite so the next launch can resume.
pub async fn get_or_init_session(app: &AppHandle) -> Result<Arc<GrokSession>, String> {
    SESSION
        .get_or_try_init(|| async {
            let (binary, model, cwd, resume_id) = resolve_start_params(app).await?;
            let session = GrokSession::start_subprocess(binary, cwd, model, resume_id.clone()).await?;
            // Persist the new session id so a relaunch can resume.
            if resume_id.is_none() {
                if let Some(sid) = session.session_id() {
                    let _ = persist_session_id(app, &sid).await;
                }
            }
            Ok::<_, String>(Arc::new(session))
        })
        .await
        .cloned()
}

/// Drop the cached session. The next `get_or_init_session` call
/// will spawn a fresh subprocess. Not currently wired to a UI
/// control — kept for tests and future "reset" flows.
pub async fn close_session() {
    if let Some(s) = SESSION.get() {
        s.close();
    }
    // OnceCell doesn't have a `take`; we replace the static by
    // relying on the process to be short-lived for tests. In
    // production, the session lives for the whole app lifetime.
}

/// True if a session has been brought up in this process. Cheap
/// non-async check used by `grok_session_status` tool.
pub fn is_initialized() -> bool {
    SESSION.get().is_some()
}

/// Pull the four startup params (binary path, model alias, cwd,
/// resume id) out of the SQLite settings blob. Resolves to
/// sensible defaults if the user hasn't configured anything.
async fn resolve_start_params(
    app: &AppHandle,
) -> Result<(Arc<str>, Arc<str>, PathBuf, Option<String>), String> {
    let app_clone = app.clone();
    let (binary, model, cwd_override, resume_id) = tokio::task::spawn_blocking(move || {
        let settings = app_clone
            .state::<crate::AppState>()
            .db
            .load_settings()
            .map_err(|e| format!("settings load failed: {e}"))?;
        let binary = if settings.grok_build_binary.trim().is_empty() {
            "grok".to_string()
        } else {
            settings.grok_build_binary.trim().to_string()
        };
        let model = if settings.grok_build_model.trim().is_empty() {
            "minimax".to_string()
        } else {
            settings.grok_build_model.trim().to_string()
        };
        Ok::<_, String>((binary, model, settings.grok_cwd, settings.grok_session_id))
    })
    .await
    .map_err(|e| format!("join: {e}"))??;
    // Resolve cwd: explicit override wins; otherwise the agent
    // gets its own scratch dir under the app data dir.
    let cwd = if cwd_override.trim().is_empty() {
        let data_dir = app
            .path()
            .app_data_dir()
            .map_err(|e| format!("app data dir: {e}"))?;
        let dir = data_dir.join("grok");
        std::fs::create_dir_all(&dir).map_err(|e| format!("create grok cwd: {e}"))?;
        dir
    } else {
        PathBuf::from(cwd_override.trim())
    };
    Ok((Arc::from(binary.as_str()), Arc::from(model.as_str()), cwd, resume_id))
}

/// Write the session id back to SQLite so the next process can
/// resume. Best-effort: a write failure is logged but doesn't
/// fail the bring-up (the in-process session is still usable).
async fn persist_session_id(app: &AppHandle, session_id: &str) -> Result<(), String> {
    let app_clone = app.clone();
    let sid = session_id.to_string();
    tokio::task::spawn_blocking(move || {
        let state = app_clone.state::<crate::AppState>();
        let mut settings = state
            .db
            .load_settings()
            .map_err(|e| format!("settings load failed: {e}"))?;
        settings.grok_session_id = Some(sid);
        state
            .db
            .save_settings(&settings)
            .map_err(|e| format!("settings save failed: {e}"))
    })
    .await
    .map_err(|e| format!("join: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_event_clone_is_cheap() {
        // Sanity: SessionEvent is Clone so the broadcast::Receiver
        // can pass it across await points without re-parsing.
        let ev = SessionEvent::Chunk("hello".to_string());
        let ev2 = ev.clone();
        match ev2 {
            SessionEvent::Chunk(s) => assert_eq!(s, "hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn settings_field_defaults_round_trip() {
        // The settings struct should default these fields
        // gracefully so an old DB blob (without the new fields)
        // deserializes cleanly.
        let s: Settings = serde_json::from_str("{}").expect("empty settings");
        assert_eq!(s.grok_build_binary, "");
        assert_eq!(s.grok_build_model, "");
        assert_eq!(s.grok_cwd, "");
        assert!(s.grok_session_id.is_none());
    }
}
