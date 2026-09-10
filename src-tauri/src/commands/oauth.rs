//! v3.7.12 — Tauri command surface for real Google OAuth.
//!
//! The heavy lifting (URL building, listener, token
//! exchange) lives in [`crate::connectors::oauth`]. The
//! commands in this module are thin wrappers that stash
//! the in-flight [`GoogleOauthFlow`] in a process-wide
//! slot (`oauth::take_pending_flow` /
//! `oauth::set_pending_flow`) so the renderer can drive
//! the flow over IPC.
//!
//! The Tauri command shape is:
//!
//! 1. `start_google_oauth_cmd` — Renderer invokes with
//!    the client_id + client_secret the user pasted into
//!    Settings → Google Account. Returns the Google
//!    authorization URL + the port the Rust side is
//!    listening on. Renderer opens the URL in the default
//!    browser via `tauri-plugin-opener`'s `open_url`.
//! 2. `complete_google_oauth_cmd` — Renderer invokes
//!    after the Tauri event `google_oauth://complete`
//!    fires (emitted by the listener task once the user
//!    lands on the callback). Returns the
//!    [`GoogleOauthStatus`] so the renderer can flip its
//!    UI to "Connected".
//! 3. `cancel_google_oauth_cmd` — Renderer invokes when
//!    the user clicks "Cancel" or closes the Settings
//!    modal mid-flow. Drops the in-flight flow, which
//!    stops the listener via the [`Drop`] impl.
//! 4. `disconnect_google_oauth_cmd` — Renderer invokes
//!    when the user clicks "Disconnect". Clears all
//!    stored OAuth fields; idempotent.
//! 5. `google_oauth_status_cmd` — Renderer invokes on
//!    mount to render the "Connected" / "Not connected"
//!    row.

use std::sync::Arc;

use serde_json::{json, Value};
use tauri::Emitter;

use crate::AppState;
use crate::connectors::oauth::{
    self, complete_google_oauth, fetch_google_user_email, set_pending_flow,
    start_google_oauth, status_from_settings, take_pending_flow, GoogleOauthStatus,
    DEFAULT_SCOPES,
};

/// Tauri command: start the OAuth flow.
#[tauri::command]
pub async fn start_google_oauth_cmd(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    client_id: String,
    client_secret: String,
    scopes: Option<Vec<String>>,
) -> Result<Value, String> {
    let scopes = scopes.unwrap_or_else(|| {
        DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect()
    });
    let flow = start_google_oauth(Some(&app), client_id, client_secret, scopes)
        .await
        .map_err(|e| e.to_string())?;
    let port = flow.redirect_port;
    let auth_url = flow.auth_url.clone();
    // Drop any previous flow (which cancels its
    // listener) and stash the new one.
    set_pending_flow(flow);
    Ok(json!({
        "auth_url": auth_url,
        "port": port,
    }))
}

/// Tauri command: complete the OAuth flow. Called by the
/// renderer after the `google_oauth://complete` event.
#[tauri::command]
pub async fn complete_google_oauth_cmd(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<GoogleOauthStatus, String> {
    let state: Arc<AppState> = state.inner().clone();
    let flow = take_pending_flow().ok_or_else(|| {
        "no active Google OAuth flow — click 'Connect Google' to start one".to_string()
    })?;
    let database = state.db.clone();
    complete_google_oauth(&database, flow)
        .await
        .map_err(|e| e.to_string())?;
    let settings = database.load_settings().unwrap_or_default();
    // Best-effort email lookup via userinfo. We do this
    // before returning so the renderer can show
    // "Connected as user@example.com" without a second
    // round-trip.
    let email = fetch_google_user_email(&settings)
        .await
        .ok()
        .flatten();
    let mut status = status_from_settings(&settings);
    if status.email.is_none() {
        status.email = email;
    }
    let _ = app.emit("google_oauth://complete", &status);
    Ok(status)
}

/// Tauri command: cancel an in-flight OAuth flow.
#[tauri::command]
pub fn cancel_google_oauth_cmd(
    _state: tauri::State<'_, Arc<AppState>>,
) -> Result<(), String> {
    let _ = _state;
    oauth::clear_pending_flow();
    Ok(())
}

/// Tauri command: disconnect the user's Google account.
#[tauri::command]
pub fn disconnect_google_oauth_cmd(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<GoogleOauthStatus, String> {
    let state: Arc<AppState> = state.inner().clone();
    oauth::clear_google_tokens(&state.db).map_err(|e| e.to_string())?;
    let settings = state.db.load_settings().unwrap_or_default();
    Ok(status_from_settings(&settings))
}

/// Tauri command: return the current Google OAuth
/// status.
#[tauri::command]
pub fn google_oauth_status_cmd(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<GoogleOauthStatus, String> {
    let state: Arc<AppState> = state.inner().clone();
    let settings = state.db.load_settings().unwrap_or_default();
    Ok(status_from_settings(&settings))
}
