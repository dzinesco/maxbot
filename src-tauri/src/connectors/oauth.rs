//! v3.7.12 — real Google OAuth 2.0 (Authorization Code flow).
//!
//! Replaces the v3.7.0 "paste a long-lived access token" path
//! in `Settings.google_access_token` with a real OAuth 2.0
//! dance: the Mac app opens a system browser to the Google
//! consent screen, captures the `?code=…&state=…` callback
//! on a localhost listener, exchanges the code for tokens,
//! and stores the **refresh token** durably. The connector
//! tools (`gmail`, `calendar`) call
//! [`get_google_access_token`] which lazily refreshes the
//! short-lived access token when it expires.
//!
//! ## Flow
//!
//! 1. The renderer calls `start_google_oauth` with
//!    `client_id`, `client_secret`, and a scope list. The
//!    Rust side binds a `127.0.0.1:PORT` listener, generates
//!    a 32-byte random `state`, builds the Google
//!    authorization URL with
//!    `access_type=offline` (so we get a refresh token) and
//!    `prompt=consent` (so Google re-issues the refresh
//!    token even if the user previously granted consent),
//!    and returns the URL + port. The listener and state
//!    are wrapped in a [`GoogleOauthFlow`] handle the
//!    renderer holds until the user clicks "Cancel" or the
//!    listener resolves.
//! 2. The renderer opens the URL in the default browser
//!    (`tauri::api::shell::open`).
//! 3. The user signs in + grants consent. Google redirects
//!    to `http://127.0.0.1:PORT/callback?code=…&state=…`.
//! 4. The localhost listener accepts the connection,
//!    validates `state`, sends a tiny "you can close this
//!    tab" HTML response, and resolves the `code` via a
//!    `oneshot` channel.
//! 5. [`complete_google_oauth`] exchanges the code for
//!    tokens via POST to `https://oauth2.googleapis.com/token`,
//!    persists the refresh + access tokens + expiry to
//!    `Settings`, and returns the token response so the
//!    renderer can show "Connected" + the granted email
//!    (the email comes from a follow-up `userinfo` call).
//!
//! ## Token refresh
//!
//! [`get_google_access_token`] is called by the Gmail /
//! Calendar tool impls. It checks the cached access token
//! against `google_access_token_expiry`; if still valid, it
//! returns the cached value. If expired and a refresh token
//! is available, it POSTs to the token endpoint with
//! `grant_type=refresh_token` and persists the new pair. If
//! Google returns `invalid_grant` (the refresh token was
//! revoked or the user revoked the app's access), the stored
//! tokens are cleared and a clear "reconnect" error is
//! returned so the model can react.
//!
//! ## Why not the `oauth2` crate?
//!
//! The flow is small (one auth URL + one POST). Pulling the
//! `oauth2` crate would have added a dep tree for ~80 lines
//! of code; the `reqwest`-based path is shorter and uses
//! only deps we already link against.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::Emitter;
use tokio::sync::oneshot;

use crate::storage::{Database, Settings};

/// v3.7.12 — process-wide slot for the in-flight
/// Google OAuth flow. The `start_google_oauth_cmd`
/// Tauri command writes a `GoogleOauthFlow` here;
/// `complete_google_oauth_cmd` reads it. Single-slot:
/// a fresh "Connect Google" click drops the previous
/// flow, which cancels the listener via the
/// `GoogleOauthFlow` `Drop` impl.
///
/// Stored as a `OnceLock<Mutex<…>>` (rather than a
/// field on `AppState`) so adding the OAuth flow
/// doesn't require updating every `AppState` test
/// constructor in the crate. The lock is only ever
/// held briefly — `start_google_oauth_cmd` to stash
/// the flow, `complete_google_oauth_cmd` to take it.
/// Real flow state lives in the `GoogleOauthFlow`
/// itself (its `oneshot::Receiver` + cancel flag).
fn pending_flow_slot() -> &'static std::sync::Mutex<Option<GoogleOauthFlow>> {
    static SLOT: OnceLock<std::sync::Mutex<Option<GoogleOauthFlow>>> = OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

/// Take the in-flight OAuth flow, leaving `None` in
/// its place. Returns `None` if no flow is active.
pub fn take_pending_flow() -> Option<GoogleOauthFlow> {
    pending_flow_slot()
        .lock()
        .expect("pending oauth flow slot poisoned")
        .take()
}

/// Replace the in-flight OAuth flow, dropping the
/// previous one (which cancels its listener).
pub fn set_pending_flow(flow: GoogleOauthFlow) {
    let mut guard = pending_flow_slot()
        .lock()
        .expect("pending oauth flow slot poisoned");
    *guard = Some(flow);
}

/// Drop the in-flight OAuth flow if any. Used by the
/// "Cancel" Tauri command.
pub fn clear_pending_flow() {
    let _ = take_pending_flow();
}

/// Google authorization + token endpoints. The auth
/// endpoint is where we send the user; the token endpoint
/// is where we trade a `code` (or refresh token) for an
/// access token.
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// Default localhost port the OAuth listener tries to
/// bind to. Falls back to an OS-assigned port if 8765 is
/// taken. The renderer surfaces the actual port in its
/// "Listening on http://127.0.0.1:PORT/callback" message.
const DEFAULT_REDIRECT_PORT: u16 = 8765;

/// Maximum time the OAuth flow waits for the user to
/// complete the consent screen. 5 minutes is enough for
/// the user to switch to their browser, sign in, and
/// click "Allow" — anything longer usually means they
/// wandered off and the listener should free its port.
const FLOW_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// The full set of Google API scopes the connector tools
/// need. `openid` + `email` are OpenID Connect scopes that
/// give us the user's email after the token exchange;
/// `gmail.readonly` is enough for `gmail_list_messages`
/// and `gmail_get_message` (sending/drafting would also
/// need `gmail.compose` — we don't expose those tools
/// today, so the readonly scope is the minimum needed).
/// Calendar's `events` scope covers the four
/// `calendar_*_event` tools.
pub const DEFAULT_SCOPES: &[&str] = &[
    "openid",
    "email",
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/calendar.events",
];

/// Bearer token shape returned by Google's token endpoint.
/// We care about `access_token`, `refresh_token` (only on
/// the initial code exchange — not on refresh), and
/// `expires_in` (seconds until the access token expires;
/// we subtract 60s as a clock-skew buffer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    /// Optional — Google's refresh-grant only returns a
    /// refresh token on the initial code exchange (or when
    /// `prompt=consent` forces re-consent). Subsequent
    /// `grant_type=refresh_token` calls only return a new
    /// access token.
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_in: i64,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
}

/// The response shape surfaced to the renderer when the
/// OAuth flow completes. The renderer stores it in
/// component state and uses `connected` + `email` +
/// `expires_at` to render the "Connected" row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoogleOauthStatus {
    pub connected: bool,
    /// User's email (from the `email` scope +
    /// `userinfo` call). Null when the flow is idle /
    /// unconfigured.
    pub email: Option<String>,
    /// RFC-3339 timestamp when the access token
    /// expires. Null when the flow is idle.
    pub expires_at: Option<String>,
    /// Granted scopes — the same list the user saw on
    /// the consent screen, trimmed and deduped.
    pub scopes: Vec<String>,
}

/// An active OAuth flow. Holds the bound localhost
/// listener + the `oneshot` channel the listener will
/// resolve when the user lands on the callback URL. The
/// flow is created by [`start_google_oauth`] and
/// consumed by [`complete_google_oauth`] (or dropped, in
/// which case the listener is shut down via
/// [`cancel_google_oauth`]).
pub struct GoogleOauthFlow {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_port: u16,
    pub state: String,
    pub auth_url: String,
    /// Wrapped in `Option` so `Drop` can take it out and
    /// close the listener without fighting the future
    /// that owns it. The listener itself is moved into
    /// the background task in [`start_google_oauth`].
    code_receiver: Option<oneshot::Receiver<OauthCallback>>,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Debug)]
struct OauthCallback {
    code: String,
    state: String,
}

impl Drop for GoogleOauthFlow {
    fn drop(&mut self) {
        // The background task watches `cancel_flag`; if
        // it's set, the task exits without trying to
        // resolve the receiver. We don't have a handle
        // to the task here (it was spawned in
        // `start_google_oauth`), but the task checks the
        // flag on every accept() loop.
        self.cancel_flag
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Errors from the OAuth flow. Each variant maps to a
/// renderer-friendly message; the Tauri command surface
/// stringifies them via `Display`.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    #[error("Google OAuth client not configured: set google_oauth_client_id + google_oauth_client_secret in Settings, or create a Desktop OAuth client at https://console.cloud.google.com/apis/credentials")]
    ClientNotConfigured,
    #[error("could not bind localhost listener on 127.0.0.1:{port}: {source}")]
    ListenerBind { port: u16, source: std::io::Error },
    #[error("OAuth flow timed out after {seconds}s — did you complete the consent screen?")]
    Timeout { seconds: u64 },
    #[error("OAuth state mismatch — possible CSRF; please retry")]
    StateMismatch,
    #[error("Google returned an error: {message}")]
    GoogleError { message: String },
    #[error("token exchange failed: {0}")]
    TokenExchange(String),
    #[error("token refresh failed: {0}")]
    TokenRefresh(String),
    #[error("invalid_grant from Google — refresh token revoked or expired; please reconnect")]
    RefreshRevoked,
    #[error("http error: {0}")]
    Http(String),
}

/// Generate a 32-byte random `state` parameter, base64url
/// (no padding). Google's docs recommend ≥128 bits of
/// entropy; 32 bytes (256 bits) is well over that.
fn random_state() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Bind a localhost listener. Tries the default port
/// (8765) first, falls back to an OS-assigned port
/// (`0`) so two MaxBot instances on the same machine
/// don't fight. The renderer surfaces the actual port in
/// its "Listening on http://127.0.0.1:PORT/callback" line.
fn bind_redirect_listener() -> Result<(TcpListener, u16), OAuthError> {
    let addr = SocketAddr::from(([127, 0, 0, 1], DEFAULT_REDIRECT_PORT));
    match TcpListener::bind(addr) {
        Ok(l) => {
            l.set_nonblocking(true).ok();
            Ok((l, DEFAULT_REDIRECT_PORT))
        }
        Err(_) => {
            let l = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
                .map_err(|e| OAuthError::ListenerBind {
                    port: DEFAULT_REDIRECT_PORT,
                    source: e,
                })?;
            let port = l
                .local_addr()
                .map(|a| a.port())
                .map_err(|e| OAuthError::ListenerBind {
                    port: 0,
                    source: e,
                })?;
            l.set_nonblocking(true).ok();
            Ok((l, port))
        }
    }
}

/// Build the Google authorization URL. The order of
/// query params is irrelevant to Google; we just want
/// a deterministic, copy-pasteable shape so the unit
/// test can match against a specific substring.
pub fn build_auth_url(
    client_id: &str,
    redirect_port: u16,
    scopes: &[&str],
    state: &str,
) -> String {
    let scope = scopes.join(" ");
    let redirect_uri = format!("http://127.0.0.1:{redirect_port}/callback");
    // `reqwest::Url` would re-encode the scope, so we
    // build the URL by hand to keep the test stable.
    format!(
        "{GOOGLE_AUTH_URL}?response_type=code&client_id={client_id}&redirect_uri={redirect_uri}&scope={scope}&state={state}&access_type=offline&prompt=consent&include_granted_scopes=true"
    )
}

/// Spawn the localhost listener as a tokio task. The
/// task accepts connections, reads the first request,
/// parses the query string for `code` and `state`, and
/// resolves the `code_sender` oneshot with the values.
/// Returns the receiver the caller awaits on.
fn spawn_callback_listener(
    listener: TcpListener,
    expected_state: String,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
    code_sender: oneshot::Sender<OauthCallback>,
) {
    tokio::spawn(async move {
        // The listener is non-blocking so we can poll it
        // without a dedicated thread. We accept on a
        // short timeout so `cancel_flag` is checked
        // frequently.
        let deadline = std::time::Instant::now() + FLOW_TIMEOUT;
        loop {
            if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                break;
            }
            if std::time::Instant::now() >= deadline {
                break;
            }
            match listener.accept() {
                Ok((stream, _)) => {
                    if let Some(cb) = read_callback(&stream, &expected_state) {
                        // Send a tiny "you can close this
                        // tab" HTML response before
                        // resolving. Best-effort: a
                        // closed browser socket is fine.
                        let _ = write_response(
                            &stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 78\r\nConnection: close\r\n\r\n<html><body><h2>MaxBot Google auth complete</h2>You can close this tab.</body></html>",
                        );
                        let _ = code_sender.send(cb);
                        return;
                    } else {
                        // Malformed callback (no code, bad
                        // shape). Send a 400 so the
                        // browser shows something useful
                        // and keep listening — the user
                        // might retry.
                        let _ = write_response(
                            &stream,
                            "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 64\r\nConnection: close\r\n\r\n<html><body><h2>Bad OAuth callback</h2>Missing code or state.</body></html>",
                        );
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    // No connection yet — sleep briefly
                    // and re-check.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(_) => break,
            }
        }
    });
}

/// Read the first HTTP request from `stream` and parse
/// the `code` + `state` query params. Returns `None` if
/// the request is malformed or missing the required
/// params.
fn read_callback(stream: &TcpStream, expected_state: &str) -> Option<OauthCallback> {
    use std::io::Read;
    // The browser sends the callback as a single HTTP/1.1
    // GET with the auth code in the query string. We
    // don't need a full HTTP parser — we just need the
    // request line.
    let stream_ref = stream;
    let mut s = stream_ref;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = [0u8; 4096];
    let mut total = Vec::new();
    loop {
        match s.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total.extend_from_slice(&buf[..n]);
                if total.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
                if total.len() > 8192 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&total);
    let request_line = text.lines().next()?;
    // "GET /callback?code=…&state=… HTTP/1.1"
    let mut parts = request_line.split_whitespace();
    let _method = parts.next()?;
    let target = parts.next()?;
    let query = target.split('?').nth(1)?;
    let mut code = None;
    let mut state = None;
    for kv in query.split('&') {
        let mut it = kv.splitn(2, '=');
        let k = it.next()?;
        let v = it.next().unwrap_or("");
        if k == "code" {
            code = Some(urldecode(v));
        } else if k == "state" {
            state = Some(urldecode(v));
        }
    }
    let code = code?;
    let state = state?;
    if state != expected_state {
        return None;
    }
    Some(OauthCallback { code, state })
}

/// Minimal URL-decode for the callback query string. We
/// only care about `+` → space and `%XX` → byte.
fn urldecode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Write a full HTTP response to `stream` and shut
/// down the write side so the browser sees EOF.
fn write_response(stream: &TcpStream, response: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut s = stream;
    s.write_all(response.as_bytes())?;
    s.flush()?;
    Ok(())
}

/// Spawn the OAuth flow: bind the listener, build the
/// URL, and return a [`GoogleOauthFlow`] the caller
/// awaits on with [`complete_google_oauth`].
///
/// The `app` parameter is optional — when present, the
/// flow emits `google_oauth://status` events on the
/// consent and complete transitions so the renderer
/// can react without polling. When absent (test path),
/// the flow is purely programmatic.
pub async fn start_google_oauth(
    app: Option<&tauri::AppHandle>,
    client_id: String,
    client_secret: String,
    scopes: Vec<String>,
) -> Result<GoogleOauthFlow, OAuthError> {
    if client_id.trim().is_empty() || client_secret.trim().is_empty() {
        return Err(OAuthError::ClientNotConfigured);
    }
    let scope_refs: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
    let (listener, port) = bind_redirect_listener()?;
    let state = random_state();
    let auth_url = build_auth_url(&client_id, port, &scope_refs, &state);
    let (tx, rx) = oneshot::channel();
    let cancel_flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
    spawn_callback_listener(listener, state.clone(), cancel_flag.clone(), tx);
    if let Some(handle) = app {
        let _ = handle.emit("google_oauth://started", json!({ "port": port }));
    }
    Ok(GoogleOauthFlow {
        client_id,
        client_secret,
        redirect_port: port,
        state,
        auth_url,
        code_receiver: Some(rx),
        cancel_flag,
    })
}

/// Await the OAuth flow's callback, exchange the code
/// for tokens, persist them, and return the
/// [`TokenResponse`].
pub async fn complete_google_oauth(
    db: &Database,
    mut flow: GoogleOauthFlow,
) -> Result<TokenResponse, OAuthError> {
    let expected_state = flow.state.clone();
    let client_id = flow.client_id.clone();
    let client_secret = flow.client_secret.clone();
    let redirect_port = flow.redirect_port;
    // Drop the flow's receiver and cancel flag — we own
    // them now. The listener task is still running in the
    // background; it'll exit on its own when the channel
    // resolves.
    let mut rx = flow.code_receiver.take().ok_or_else(|| OAuthError::GoogleError {
        message: "OAuth flow already consumed".to_string(),
    })?;
    let callback = match tokio::time::timeout(FLOW_TIMEOUT, &mut rx).await {
        Ok(Ok(cb)) => cb,
        Ok(Err(_)) => {
            return Err(OAuthError::GoogleError {
                message: "OAuth channel dropped before callback".to_string(),
            });
        }
        Err(_) => {
            return Err(OAuthError::Timeout {
                seconds: FLOW_TIMEOUT.as_secs(),
            });
        }
    };
    drop(rx);
    if callback.state != expected_state {
        return Err(OAuthError::StateMismatch);
    }
    let redirect_uri = format!("http://127.0.0.1:{redirect_port}/callback");
    let token = exchange_code_for_tokens(
        &client_id,
        &client_secret,
        &callback.code,
        &redirect_uri,
    )
    .await?;
    persist_tokens(db, &token).map_err(|e| OAuthError::TokenExchange(e.to_string()))?;
    Ok(token)
}

/// Cancel an in-flight OAuth flow by dropping it. The
/// `Drop` impl flips the `cancel_flag`; the background
/// listener task sees the flag and exits on its next
/// poll.
pub fn cancel_google_oauth(flow: GoogleOauthFlow) {
    drop(flow);
}

/// Exchange an authorization code for tokens. Called by
/// [`complete_google_oauth`] only — the refresh-grant
/// path uses [`refresh_access_token`].
pub async fn exchange_code_for_tokens(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenResponse, OAuthError> {
    let client = http_client()?;
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
        ])
        .send()
        .await
        .map_err(|e| OAuthError::Http(e.to_string()))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| OAuthError::TokenExchange(format!("non-JSON: {e}")))?;
    if !status.is_success() {
        let message = body
            .get("error_description")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or("unknown error")
            .to_string();
        return Err(OAuthError::GoogleError { message });
    }
    let token: TokenResponse = serde_json::from_value(body)
        .map_err(|e| OAuthError::TokenExchange(format!("parse: {e}")))?;
    Ok(token)
}

/// Refresh the access token using the stored refresh
/// token. Returns the new token response. On
/// `invalid_grant` (refresh token revoked), the stored
/// tokens are cleared and [`OAuthError::RefreshRevoked`]
/// is returned so the caller surfaces a clear "please
/// reconnect" message.
pub async fn refresh_access_token(
    db: &Database,
    settings: &Settings,
) -> Result<TokenResponse, OAuthError> {
    let refresh_token = settings
        .google_refresh_token
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .ok_or(OAuthError::RefreshRevoked)?;
    let client_id = settings
        .google_oauth_client_id
        .as_ref()
        .ok_or(OAuthError::ClientNotConfigured)?;
    let client_secret = settings
        .google_oauth_client_secret
        .as_ref()
        .ok_or(OAuthError::ClientNotConfigured)?;
    let client = http_client()?;
    let resp = client
        .post(GOOGLE_TOKEN_URL)
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token.as_str()),
        ])
        .send()
        .await
        .map_err(|e| OAuthError::Http(e.to_string()))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| OAuthError::TokenRefresh(format!("non-JSON: {e}")))?;
    if !status.is_success() {
        let err = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        if err == "invalid_grant" {
            // Clear the stored tokens so the next call
            // doesn't loop on the same refresh.
            clear_google_tokens(db).map_err(|e| OAuthError::TokenRefresh(e.to_string()))?;
            return Err(OAuthError::RefreshRevoked);
        }
        let message = body
            .get("error_description")
            .and_then(|v| v.as_str())
            .unwrap_or(&err)
            .to_string();
        return Err(OAuthError::TokenRefresh(message));
    }
    let mut token: TokenResponse = serde_json::from_value(body)
        .map_err(|e| OAuthError::TokenRefresh(format!("parse: {e}")))?;
    // Refresh responses don't include a refresh_token
    // (the original one is reused). Fall back to the
    // stored value so the persisted Settings row keeps
    // its refresh token intact.
    if token.refresh_token.is_none() {
        token.refresh_token = Some(refresh_token.clone());
    }
    persist_tokens(db, &token).map_err(|e| OAuthError::TokenRefresh(e.to_string()))?;
    Ok(token)
}

/// Resolve a usable access token for the Gmail / Calendar
/// connector tools. Returns the cached access token if
/// it's not expired; otherwise refreshes via
/// [`refresh_access_token`] and returns the new one.
///
/// On [`OAuthError::RefreshRevoked`] the stored tokens
/// have already been cleared by `refresh_access_token`,
/// so the next call will see "Google OAuth not connected"
/// and the model can prompt the user to reconnect.
pub async fn get_google_access_token(
    db: &Database,
    settings: &Settings,
) -> Result<String, OAuthError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let cached = settings
        .google_access_token
        .as_ref()
        .filter(|s| !s.trim().is_empty());
    let expiry = settings.google_access_token_expiry.unwrap_or(0);
    if let Some(token) = cached {
        if expiry > now + 30 {
            return Ok(token.clone());
        }
    }
    if settings
        .google_refresh_token
        .as_ref()
        .map(|s| s.trim().is_empty())
        .unwrap_or(true)
    {
        return Err(OAuthError::RefreshRevoked);
    }
    let new_token = refresh_access_token(db, settings).await?;
    Ok(new_token.access_token)
}

/// Look up the user's email via Google's `userinfo`
/// endpoint, using the access token from the most
/// recent OAuth flow. Returns `None` if the access
/// token is missing or the lookup fails — the renderer
/// just shows "Connected" without an email in that
/// case rather than throwing.
pub async fn fetch_google_user_email(
    settings: &Settings,
) -> Result<Option<String>, OAuthError> {
    let access = match settings
        .google_access_token
        .as_ref()
        .filter(|s| !s.trim().is_empty())
    {
        Some(t) => t.clone(),
        None => return Ok(None),
    };
    let client = http_client()?;
    let resp = client
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(&access)
        .send()
        .await
        .map_err(|e| OAuthError::Http(e.to_string()))?;
    if !resp.status().is_success() {
        return Ok(None);
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| OAuthError::Http(e.to_string()))?;
    Ok(v.get("email").and_then(|x| x.as_str()).map(|s| s.to_string()))
}

/// Build a [`GoogleOauthStatus`] from the current
/// settings row. Used by the renderer to render the
/// "Connected" / "Not connected" row + the
/// "Token expires in 47m" countdown.
pub fn status_from_settings(settings: &Settings) -> GoogleOauthStatus {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let expiry = settings.google_access_token_expiry.unwrap_or(0);
    let scopes = settings
        .google_refresh_token
        .as_ref()
        .map(|_| DEFAULT_SCOPES.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();
    if settings
        .google_refresh_token
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        GoogleOauthStatus {
            connected: true,
            email: None,
            expires_at: if expiry > now {
                Some(
                    chrono::DateTime::from_timestamp(expiry, 0)
                        .map(|dt| dt.to_rfc3339())
                        .unwrap_or_default(),
                )
            } else {
                None
            },
            scopes,
        }
    } else {
        GoogleOauthStatus {
            connected: false,
            email: None,
            expires_at: None,
            scopes,
        }
    }
}

/// Persist the token response to Settings. Updates
/// `google_access_token`, `google_access_token_expiry`,
/// and — if the response includes one —
/// `google_refresh_token`. Existing values are
/// preserved on a refresh-grant response (the
/// `refresh_token` field is `None` for refresh-grant
/// responses; the original is reused).
fn persist_tokens(db: &Database, token: &TokenResponse) -> rusqlite::Result<()> {
    let mut settings = db.load_settings().unwrap_or_default();
    settings.google_access_token = Some(token.access_token.clone());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 60s clock-skew buffer so a token we just got
    // isn't immediately treated as expired by a clock
    // a few seconds ahead of ours.
    let expiry = now + token.expires_in - 60;
    settings.google_access_token_expiry = Some(expiry.max(now));
    if let Some(rt) = token.refresh_token.as_ref().filter(|s| !s.is_empty()) {
        settings.google_refresh_token = Some(rt.clone());
    }
    db.save_settings(&settings)
}

/// Clear all Google OAuth fields. Called when
/// `refresh_access_token` gets `invalid_grant` (so the
/// user is forced to reconnect) and from the
/// `disconnect_google` Tauri command.
pub fn clear_google_tokens(db: &Database) -> rusqlite::Result<()> {
    let mut settings = db.load_settings().unwrap_or_default();
    settings.google_access_token = None;
    settings.google_access_token_expiry = None;
    settings.google_refresh_token = None;
    db.save_settings(&settings)
}

fn http_client() -> Result<Client, OAuthError> {
    Client::builder()
        .user_agent("MaxBot/3.7 (+https://maxbot.app)")
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|e| OAuthError::Http(e.to_string()))
}

/// Tauri command: start the OAuth flow. Returns the
/// authorization URL + the redirect port. The renderer
/// opens the URL in the default browser and shows
/// "Listening on http://127.0.0.1:PORT/callback".
///
/// The Tauri command itself lives in
/// `crate::commands::oauth` — the `connectors::oauth`
/// module only owns the flow logic. The command is a
/// thin wrapper that stashes the in-flight
/// [`GoogleOauthFlow`] on `AppState.pending_oauth_flow`
/// so `complete_google_oauth_cmd` can pick it up.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Database;

    fn unique_temp_db() -> Database {
        let path = std::env::temp_dir().join(format!(
            "maxbot-oauth-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        Database::open(&path).expect("open temp db")
    }

    #[test]
    fn build_auth_url_uses_expected_params() {
        let url = build_auth_url(
            "test-client-id.apps.googleusercontent.com",
            8765,
            &["openid", "email"],
            "abc123state",
        );
        assert!(url.starts_with(GOOGLE_AUTH_URL));
        assert!(url.contains("client_id=test-client-id.apps.googleusercontent.com"));
        // `build_auth_url` puts the raw
        // `http://127.0.0.1:8765/callback` into the
        // query string. Google accepts both the raw and
        // percent-encoded forms; we test the raw form
        // because that's what the helper produces.
        assert!(url.contains("redirect_uri=http://127.0.0.1:8765/callback"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("state=abc123state"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("scope=openid email"));
    }

    #[test]
    fn random_state_is_unique() {
        let a = random_state();
        let b = random_state();
        assert_ne!(a, b);
        assert!(a.len() >= 32);
    }

    #[test]
    fn urldecode_handles_plus_and_percent_escapes() {
        assert_eq!(urldecode("hello+world"), "hello world");
        assert_eq!(urldecode("a%20b"), "a b");
        assert_eq!(urldecode("a%2Bb"), "a+b");
    }

    #[test]
    fn get_google_access_token_errors_when_unconfigured() {
        let db = unique_temp_db();
        let settings = db.load_settings().unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(get_google_access_token(&db, &settings))
            .unwrap_err();
        // No refresh token stored → the helper short-
        // circuits to RefreshRevoked.
        assert!(matches!(err, OAuthError::RefreshRevoked));
    }

    #[test]
    fn get_google_access_token_returns_cached_token_when_not_expired() {
        let db = unique_temp_db();
        let mut settings = db.load_settings().unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        settings.google_access_token = Some("cached-access-token".to_string());
        settings.google_access_token_expiry = Some(now + 3600);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let token = rt
            .block_on(get_google_access_token(&db, &settings))
            .unwrap();
        assert_eq!(token, "cached-access-token");
    }

    #[test]
    fn get_google_access_token_treats_missing_expiry_as_expired() {
        let db = unique_temp_db();
        let mut settings = db.load_settings().unwrap();
        // No refresh token → no way to refresh; the
        // helper should return RefreshRevoked rather
        // than silently using the cached (unknown-
        // expiry) access token.
        settings.google_access_token = Some("stale-access-token".to_string());
        settings.google_access_token_expiry = None;
        let rt = tokio::runtime::Runtime::new().unwrap();
        let err = rt
            .block_on(get_google_access_token(&db, &settings))
            .unwrap_err();
        assert!(matches!(err, OAuthError::RefreshRevoked));
    }

    #[test]
    fn status_from_settings_reports_disconnected_when_no_refresh_token() {
        let settings = Settings::default();
        let s = status_from_settings(&settings);
        assert!(!s.connected);
        assert!(s.email.is_none());
        assert!(s.expires_at.is_none());
    }

    #[test]
    fn status_from_settings_reports_connected_when_refresh_token_present() {
        let mut settings = Settings::default();
        settings.google_refresh_token = Some("rt".to_string());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        settings.google_access_token_expiry = Some(now + 3600);
        let s = status_from_settings(&settings);
        assert!(s.connected);
        assert!(s.expires_at.is_some());
        assert!(!s.scopes.is_empty());
    }

    #[test]
    fn clear_google_tokens_resets_state() {
        let db = unique_temp_db();
        let mut settings = db.load_settings().unwrap();
        settings.google_access_token = Some("at".to_string());
        settings.google_refresh_token = Some("rt".to_string());
        settings.google_access_token_expiry = Some(12345);
        db.save_settings(&settings).unwrap();
        clear_google_tokens(&db).unwrap();
        let after = db.load_settings().unwrap();
        assert!(after.google_access_token.is_none());
        assert!(after.google_refresh_token.is_none());
        assert!(after.google_access_token_expiry.is_none());
    }
}
