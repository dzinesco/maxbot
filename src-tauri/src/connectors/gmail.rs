//! v3.7.0 (Phase 8) — Gmail connector.
//!
//! Four tools: `gmail_list_messages`, `gmail_get_message`,
//! `gmail_send_message`, `gmail_draft_message`.
//!
//! ## Auth
//!
//! Per the brief: "OAuth or app-password auth: pick the
//! simpler path. App-passwords (Google App Passwords) avoid
//! the OAuth dance." Google App Passwords only work for
//! SMTP/IMAP, not the REST API, so the practical "simpler
//! path" for the REST API is a Google OAuth access token.
//! The user pastes the token into the existing Settings
//! panel (new `google_access_token` field); the connector
//! tools send it as `Authorization: Bearer <token>`.
//! Adding a real OAuth flow is deliberately out of scope
//! for v3.7.0 (per the brief's "Don't" list).
//!
//! Both Gmail and Calendar use the same Google account,
//! so the `google_access_token` field is shared. A
//! missing token returns a clear "set it in Settings"
//! error so the model can react.

use std::sync::Arc;

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use reqwest::Client;
use serde_json::{json, Value};
use tauri::Manager;

use crate::AppState;
use crate::connectors::oauth;
use crate::storage::Settings;
use crate::tools::registry::truncate_for_model;
use crate::tools::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

const GMAIL_BASE: &str = "https://gmail.googleapis.com/gmail/v1/users/me";
const DEFAULT_LIST_MAX: u32 = 10;
const MAX_LIST_MAX: u32 = 50;
const METADATA_HEADERS: &[&str] = &["From", "To", "Subject", "Date"];

/// Build a `reqwest::Client` with a sane user-agent
/// and a 20-second timeout. Same defaults as
/// `web_fetch`.
fn http_client() -> Result<Client, ToolError> {
    Client::builder()
        .user_agent("MaxBot/3.7 (+https://maxbot.app)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ToolError::Execution(format!("http client: {e}")))
}

/// Resolve the AppState from the ToolContext's
/// AppHandle. Returns a clear error if the AppHandle
/// is missing (e.g. test path). All four Gmail tools
/// need the DB to read the access token.
fn require_state(context: &ToolContext) -> Result<Arc<AppState>, ToolError> {
    let app = context
        .app
        .as_ref()
        .ok_or_else(|| ToolError::Execution("connector requires an AppHandle".to_string()))?;
    let state: tauri::State<Arc<AppState>> = app.state();
    Ok(state.inner().clone())
}

/// Resolve a usable Google access token. v3.7.12:
/// delegates to [`oauth::get_google_access_token`],
/// which checks the cached access token against
/// `google_access_token_expiry` and refreshes via the
/// stored refresh token when needed. The legacy
/// `Settings.google_access_token` field is the cache
/// for the OAuth flow; the durable secret is
/// `google_refresh_token`. If neither is set, the user
/// sees a "Google OAuth not connected" error pointing
/// them to Settings → Google Account.
async fn get_google_access_token(
    state: &AppState,
    settings: &Settings,
) -> Result<String, ToolError> {
    oauth::get_google_access_token(&state.db, settings)
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))
}

/// `gmail_list_messages` — list the most recent messages
/// matching an optional query. Read-only; no consent.
pub struct GmailListMessagesTool;

#[async_trait]
impl Tool for GmailListMessagesTool {
    fn name(&self) -> &str {
        "gmail_list_messages"
    }

    fn description(&self) -> &str {
        "List recent Gmail messages. Optional `query` runs against the Gmail search syntax (e.g. 'from:tyler newer_than:7d', 'subject:invoice', 'is:unread'). Returns sender, subject, date, and a 1-line snippet per message. Default 10, max 50. Read-only — no consent required."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Optional Gmail search query. Empty = list the most recent messages."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 10,
                    "description": "Max messages to return. Default 10, max 50."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let state = require_state(&context)?;
        let settings = state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let token = get_google_access_token(&state, &settings).await?;
        let query = invocation
            .arguments
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let max = invocation
            .arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(DEFAULT_LIST_MAX)
            .clamp(1, MAX_LIST_MAX);

        let client = http_client()?;
        let mut url = format!("{GMAIL_BASE}/messages?maxResults={max}");
        if !query.is_empty() {
            url.push_str(&format!("&q={}", urlencoding_encode(&query)));
        }
        let resp = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail list: {e}")))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "gmail list returned {status}: {}",
                truncate_for_model(&body, 400)
            )));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail list parse: {e}")))?;
        let ids: Vec<String> = body
            .get("messages")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if ids.is_empty() {
            return Ok(ToolResult::ok("(no messages matched)".to_string()));
        }
        // Fetch metadata for each id. The list endpoint
        // only returns ids + threadIds; the model needs
        // subject / from / date to be useful. Sequential
        // fetches (the request budget is small — at most
        // MAX_LIST_MAX per call) keep the code simple and
        // stay well under Gmail's per-user rate limit.
        let mut summaries: Vec<String> = Vec::with_capacity(ids.len());
        for id in &ids {
            let url = format!(
                "{GMAIL_BASE}/messages/{id}?format=metadata&metadataHeaders=From&metadataHeaders=To&metadataHeaders=Subject&metadataHeaders=Date"
            );
            let r = client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .map_err(|e| ToolError::Execution(format!("gmail get {id}: {e}")))?;
            if !r.status().is_success() {
                summaries.push(format!("[{id}] (fetch failed: {})", r.status()));
                continue;
            }
            let v: Value = r
                .json()
                .await
                .map_err(|e| ToolError::Execution(format!("gmail get {id} parse: {e}")))?;
            summaries.push(format_message_summary(&v));
        }
        let out = summaries.join("\n");
        Ok(ToolResult::ok(truncate_for_model(&out, 8_000)))
    }
}

/// Render a Gmail message metadata response (the
/// `format=metadata` shape) as a one-line summary:
/// `[id] From: …  Subject: …  Date: …  Snippet: …`.
fn format_message_summary(v: &Value) -> String {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
    let headers = v
        .get("payload")
        .and_then(|p| p.get("headers"))
        .and_then(|h| h.as_array());
    let mut from = String::new();
    let mut to = String::new();
    let mut subject = String::new();
    let mut date = String::new();
    if let Some(hs) = headers {
        for h in hs {
            let name = h.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let value = h.get("value").and_then(|x| x.as_str()).unwrap_or("");
            match name {
                "From" => from = value.to_string(),
                "To" => to = value.to_string(),
                "Subject" => subject = value.to_string(),
                "Date" => date = value.to_string(),
                _ => {}
            }
        }
    }
    let snippet = v.get("snippet").and_then(|x| x.as_str()).unwrap_or("");
    if from.is_empty() && subject.is_empty() {
        format!("[{id}] (no metadata) {snippet}")
    } else {
        format!("[{id}] From: {from}  To: {to}  Subject: {subject}  Date: {date}  Snippet: {snippet}")
    }
}

/// `gmail_get_message` — fetch a single message by id.
/// Returns subject, from, to, date, snippet, and a
/// plaintext body if the payload is `text/plain`.
/// Read-only; no consent.
pub struct GmailGetMessageTool;

#[async_trait]
impl Tool for GmailGetMessageTool {
    fn name(&self) -> &str {
        "gmail_get_message"
    }

    fn description(&self) -> &str {
        "Fetch a single Gmail message by id. Returns From, To, Subject, Date, and a plaintext body if the message is `text/plain`. Use after `gmail_list_messages` to read a specific id. Read-only — no consent required."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The Gmail message id (from `gmail_list_messages`)."
                }
            },
            "required": ["id"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let id = invocation
            .arguments
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `id`".to_string()))?;
        let state = require_state(&context)?;
        let settings = state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let token = get_google_access_token(&state, &settings).await?;
        let client = http_client()?;
        let url = format!("{GMAIL_BASE}/messages/{id}?format=full");
        let resp = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail get: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "gmail get returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail get parse: {e}")))?;
        let mut headers_text = String::new();
        if let Some(hs) = v
            .get("payload")
            .and_then(|p| p.get("headers"))
            .and_then(|h| h.as_array())
        {
            for h in hs {
                let name = h.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let value = h.get("value").and_then(|x| x.as_str()).unwrap_or("");
                if METADATA_HEADERS.contains(&name) {
                    headers_text.push_str(&format!("{name}: {value}\n"));
                }
            }
        }
        let body_text = extract_text_plain(v.get("payload"));
        let snippet = v.get("snippet").and_then(|x| x.as_str()).unwrap_or("");
        let out = if let Some(body) = body_text {
            format!("{headers_text}\n{body}")
        } else {
            format!("{headers_text}\n(no text/plain part)\nSnippet: {snippet}")
        };
        Ok(ToolResult::ok(truncate_for_model(&out, 8_000)))
    }
}

/// Walk a Gmail message payload tree looking for a
/// `text/plain` part and return its base64-decoded
/// body. Returns `None` if the message is HTML-only.
fn extract_text_plain(payload: Option<&Value>) -> Option<String> {
    let p = payload?;
    let mime = p.get("mimeType").and_then(|x| x.as_str()).unwrap_or("");
    if mime == "text/plain" {
        if let Some(data) = p.get("body").and_then(|b| b.get("data")).and_then(|d| d.as_str()) {
            return decode_base64url(data);
        }
    }
    if let Some(parts) = p.get("parts").and_then(|x| x.as_array()) {
        for part in parts {
            if let Some(s) = extract_text_plain(Some(part)) {
                return Some(s);
            }
        }
    }
    None
}

fn decode_base64url(s: &str) -> Option<String> {
    URL_SAFE_NO_PAD.decode(s).ok().and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Minimal URL-encoder for the `q` query parameter.
/// Only encodes spaces and a handful of punctuation —
/// enough for the simple queries the model will write.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push_str("%20"),
            ':' => out.push_str("%3A"),
            _ => out.push_str(&format!("%{:02X}", c as u32)),
        }
    }
    out
}

/// Build a base64url-encoded RFC822 message for the
/// Gmail `send` / `drafts` endpoints. Includes the
/// required `To` / `Subject` / MIME-Version / Content-Type
/// headers.
fn build_rfc822(to: &str, subject: &str, body: &str) -> String {
    let mut msg = String::new();
    msg.push_str("MIME-Version: 1.0\r\n");
    msg.push_str("Content-Type: text/plain; charset=UTF-8\r\n");
    msg.push_str(&format!("To: {to}\r\n"));
    msg.push_str(&format!("Subject: {subject}\r\n"));
    msg.push_str("\r\n");
    msg.push_str(body);
    URL_SAFE_NO_PAD.encode(msg.as_bytes())
}

/// `gmail_send_message` — send a new message. Per-call
/// consent; mutating.
pub struct GmailSendMessageTool;

#[async_trait]
impl Tool for GmailSendMessageTool {
    fn name(&self) -> &str {
        "gmail_send_message"
    }

    fn description(&self) -> &str {
        "Send a new Gmail message. Per-call consent (this hits the `send` endpoint and is irreversible). The message is RFC822-formatted with the supplied `to`/`subject`/`body`."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {"type": "string", "description": "Recipient email address."},
                "subject": {"type": "string", "description": "Email subject line."},
                "body": {"type": "string", "description": "Plaintext body of the message."}
            },
            "required": ["to", "subject", "body"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let to = invocation
            .arguments
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `to`".to_string()))?;
        let subject = invocation
            .arguments
            .get("subject")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `subject`".to_string()))?;
        let body = invocation
            .arguments
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `body`".to_string()))?;
        let state = require_state(&context)?;
        let settings = state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let token = get_google_access_token(&state, &settings).await?;
        let raw = build_rfc822(to, subject, body);
        let client = http_client()?;
        let resp = client
            .post(format!("{GMAIL_BASE}/messages/send"))
            .bearer_auth(&token)
            .json(&json!({ "raw": raw }))
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail send: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "gmail send returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail send parse: {e}")))?;
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
        Ok(ToolResult::ok(format!("sent: {id}")))
    }
}

/// `gmail_draft_message` — create a draft (reversible).
/// Per-call consent; the human reviews in Gmail before
/// sending.
pub struct GmailDraftMessageTool;

#[async_trait]
impl Tool for GmailDraftMessageTool {
    fn name(&self) -> &str {
        "gmail_draft_message"
    }

    fn description(&self) -> &str {
        "Create a Gmail draft. Per-call consent. Drafts live in the user's Drafts folder; the human reviews and sends from Gmail. Use when the user wants to write something they intend to edit before sending."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {"type": "string", "description": "Recipient email address."},
                "subject": {"type": "string", "description": "Email subject line."},
                "body": {"type": "string", "description": "Plaintext body of the message."}
            },
            "required": ["to", "subject", "body"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let to = invocation
            .arguments
            .get("to")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `to`".to_string()))?;
        let subject = invocation
            .arguments
            .get("subject")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `subject`".to_string()))?;
        let body = invocation
            .arguments
            .get("body")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `body`".to_string()))?;
        let state = require_state(&context)?;
        let settings = state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let token = get_google_access_token(&state, &settings).await?;
        let raw = build_rfc822(to, subject, body);
        let client = http_client()?;
        let resp = client
            .post(format!("{GMAIL_BASE}/drafts"))
            .bearer_auth(&token)
            .json(&json!({ "message": { "raw": raw } }))
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail draft: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "gmail draft returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("gmail draft parse: {e}")))?;
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
        Ok(ToolResult::ok(format!("drafted: {id}")))
    }
}

/// Test the Gmail connection by calling
/// `users.getProfile`. Used by the BotEditor's
/// "Test connection" button.
pub async fn test_connection(
    db: &crate::storage::Database,
    settings: &Settings,
) -> Result<String, String> {
    let token = oauth::get_google_access_token(db, settings)
        .await
        .map_err(|e| e.to_string())?;
    let client = http_client().map_err(|e| e.to_string())?;
    let resp = client
        .get(format!("{GMAIL_BASE}/profile"))
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("gmail profile: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "gmail profile returned {}: {}",
            status.as_u16(),
            truncate_for_model(&body, 400)
        ));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("gmail profile parse: {e}"))?;
    let email = v
        .get("emailAddress")
        .and_then(|x| x.as_str())
        .unwrap_or("(unknown)");
    Ok(format!("Gmail OK: signed in as {email}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Database;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_db() -> Database {
        let path = std::env::temp_dir().join(format!(
            "maxbot-gmail-test-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        Database::open(&path).expect("open temp db")
    }

    fn now_epoch() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
    }

    #[tokio::test]
    async fn get_google_access_token_returns_cached_token_when_not_expired() {
        let db = unique_temp_db();
        let mut s = Settings::default();
        s.google_access_token = Some("cached-tok".to_string());
        s.google_access_token_expiry = Some(now_epoch() + 3600);
        let token = get_google_access_token_via_helper(&db, &s).await.unwrap();
        assert_eq!(token, "cached-tok");
    }

    #[tokio::test]
    async fn get_google_access_token_errors_when_no_refresh_token() {
        let db = unique_temp_db();
        let mut s = Settings::default();
        // Cached access token but no refresh token —
        // we cannot refresh, so the helper should
        // surface a "reconnect" error.
        s.google_access_token = Some("stale".to_string());
        s.google_access_token_expiry = Some(now_epoch() - 60);
        let err = get_google_access_token_via_helper(&db, &s)
            .await
            .unwrap_err();
        // The error message should mention the user
        // needs to reconnect.
        assert!(err.contains("reconnect") || err.contains("refresh"));
    }

    #[tokio::test]
    async fn get_google_access_token_treats_missing_expiry_as_expired() {
        let db = unique_temp_db();
        let mut s = Settings::default();
        s.google_access_token = Some("stale-tok".to_string());
        s.google_access_token_expiry = None;
        // Same expectation as the no-refresh-token
        // case: with no refresh token, the helper
        // returns the reconnect error rather than
        // silently using the (unknown-expiry) cached
        // token.
        let err = get_google_access_token_via_helper(&db, &s)
            .await
            .unwrap_err();
        assert!(err.contains("reconnect") || err.contains("refresh"));
    }

    /// Call the OAuth helper with a Database + Settings
    /// (the production call site has an AppState; the
    /// tests construct the Database directly). This
    /// keeps the test surface tight — we don't need to
    /// build a full AppState for a unit test.
    async fn get_google_access_token_via_helper(
        db: &Database,
        settings: &Settings,
    ) -> Result<String, String> {
        oauth::get_google_access_token(db, settings)
            .await
            .map_err(|e| e.to_string())
    }

    #[test]
    fn build_rfc822_includes_required_headers() {
        let s = build_rfc822("a@b.com", "hi", "hello");
        let bytes = URL_SAFE_NO_PAD.decode(&s).expect("base64");
        let text = String::from_utf8(bytes).expect("utf8");
        assert!(text.contains("To: a@b.com"));
        assert!(text.contains("Subject: hi"));
        assert!(text.contains("hello"));
        assert!(text.contains("MIME-Version: 1.0"));
    }

    #[test]
    fn extract_text_plain_returns_none_for_html_only() {
        // URL_SAFE_NO_PAD decoding (Gmail's `body.data`
        // shape); the test string is
        // base64url("<h1> hi </h1>") with no padding.
        let html_only = json!({
            "mimeType": "text/html",
            "body": { "data": "PGgxPiBoaSA8L2gxPg" }
        });
        assert!(extract_text_plain(Some(&html_only)).is_none());
    }

    #[test]
    fn extract_text_plain_decodes_nested_plain_part() {
        // Same shape — URL_SAFE_NO_PAD, no padding.
        // Decodes "PGgxPiBoaSA8L2gxPg" → "<h1> hi </h1>"
        // and "aGVsbG8gd29ybGQ" → "hello world".
        let msg = json!({
            "mimeType": "multipart/alternative",
            "parts": [
                { "mimeType": "text/html", "body": { "data": "PGgxPiBoaSA8L2gxPg" } },
                { "mimeType": "text/plain", "body": { "data": "aGVsbG8gd29ybGQ" } }
            ]
        });
        let out = extract_text_plain(Some(&msg)).expect("text/plain");
        assert_eq!(out, "hello world");
    }

    #[test]
    fn format_message_summary_includes_subject_from_and_snippet() {
        let v = json!({
            "id": "msg1",
            "snippet": "first 100 chars",
            "payload": {
                "headers": [
                    {"name": "From", "value": "Alice <a@b.com>"},
                    {"name": "Subject", "value": "hi"},
                    {"name": "Date", "value": "Wed, 10 Sep 2026 00:00:00 +0000"}
                ]
            }
        });
        let s = format_message_summary(&v);
        assert!(s.contains("msg1"));
        assert!(s.contains("Alice <a@b.com>"));
        assert!(s.contains("hi"));
        assert!(s.contains("first 100 chars"));
    }

    #[test]
    fn urlencoding_encode_handles_spaces_and_colons() {
        assert_eq!(urlencoding_encode("hello world"), "hello%20world");
        assert_eq!(urlencoding_encode("from:alice"), "from%3Aalice");
        assert_eq!(urlencoding_encode("a-b_c.d~e"), "a-b_c.d~e");
    }
}
