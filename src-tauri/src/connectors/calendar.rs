//! v3.7.0 (Phase 8) — Google Calendar connector.
//!
//! Four tools: `calendar_list_events`, `calendar_get_event`,
//! `calendar_create_event`, `calendar_update_event`.
//!
//! ## Auth
//!
//! Same Google OAuth access token as Gmail (the
//! `google_access_token` Settings field). One Google
//! account, one token. The user already enters it for
//! Gmail; Calendar picks it up for free.
//!
//! ## Note on the existing `tools::calendar` module
//!
//! The pre-existing `calendar_today`, `calendar_week`,
//! `calendar_create_event` tools in
//! `src-tauri/src/tools/calendar.rs` drive the local
//! macOS Calendar.app via AppleScript. The brief asks
//! for the same `calendar_create_event` name on the
//! Google Calendar side; per the HashMap registration
//! rule, the new tool wins in the tool registry. The
//! Apple Calendar `calendar_today` / `calendar_week`
//! tools are unaffected (no name collision).
//!
//! ## Endpoint shape
//!
//! - `calendar_list_events(time_min, time_max)` —
//!   `GET /calendar/v3/calendars/primary/events?timeMin=…&timeMax=…&maxResults=N`
//! - `calendar_get_event(id)` —
//!   `GET /calendar/v3/calendars/primary/events/{id}`
//! - `calendar_create_event(summary, start, end, attendees)` —
//!   `POST /calendar/v3/calendars/primary/events`.
//!   Per-call consent.
//! - `calendar_update_event(id, ...)` —
//!   `PATCH /calendar/v3/calendars/primary/events/{id}`.
//!   Per-call consent.

use std::sync::Arc;

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};
use tauri::Manager;

use crate::AppState;
use crate::storage::Settings;
use crate::tools::registry::truncate_for_model;
use crate::tools::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

const CAL_BASE: &str = "https://www.googleapis.com/calendar/v3/calendars/primary";
const DEFAULT_LIST_MAX: u32 = 10;
const MAX_LIST_MAX: u32 = 50;

fn http_client() -> Result<Client, ToolError> {
    Client::builder()
        .user_agent("MaxBot/3.7 (+https://maxbot.app)")
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| ToolError::Execution(format!("http client: {e}")))
}

fn require_state(context: &ToolContext) -> Result<Arc<AppState>, ToolError> {
    let app = context
        .app
        .as_ref()
        .ok_or_else(|| ToolError::Execution("connector requires an AppHandle".to_string()))?;
    let state: tauri::State<Arc<AppState>> = app.state();
    Ok(state.inner().clone())
}

fn require_google_token(settings: &Settings) -> Result<String, ToolError> {
    settings
        .google_access_token
        .as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ToolError::Execution(
                "Gmail/Calendar connector not configured: set google_access_token in Settings"
                    .to_string(),
            )
        })
}

/// `calendar_list_events` — events on the primary
/// calendar in a `[time_min, time_max)` window. Read-only;
/// no consent.
pub struct CalendarListEventsTool;

#[async_trait]
impl Tool for CalendarListEventsTool {
    fn name(&self) -> &str {
        "calendar_list_events"
    }

    fn description(&self) -> &str {
        "List Google Calendar events on the primary calendar within an optional `[time_min, time_max)` window. ISO-8601 timestamps (e.g. '2026-09-10T00:00:00Z'). Empty = default to the next 7 days. Returns summary, start, end, location, attendees. Read-only — no consent required."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "time_min": {
                    "type": "string",
                    "description": "Lower bound (inclusive) for the event end time. ISO-8601. Empty = now."
                },
                "time_max": {
                    "type": "string",
                    "description": "Upper bound (exclusive) for the event start time. ISO-8601. Empty = now + 7 days."
                },
                "max_results": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 50,
                    "default": 10,
                    "description": "Max events to return. Default 10, max 50."
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
        let token = require_google_token(&settings)?;
        let time_min = invocation
            .arguments
            .get("time_min")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
        let time_max = invocation
            .arguments
            .get("time_max")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| (chrono::Utc::now() + chrono::Duration::days(7)).to_rfc3339());
        let max = invocation
            .arguments
            .get("max_results")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32)
            .unwrap_or(DEFAULT_LIST_MAX)
            .clamp(1, MAX_LIST_MAX);

        let client = http_client()?;
        let url = format!(
            "{CAL_BASE}/events?timeMin={}&timeMax={}&maxResults={}&singleEvents=true&orderBy=startTime",
            urlencoding_encode(&time_min),
            urlencoding_encode(&time_max),
            max
        );
        let resp = client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar list: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "calendar list returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar list parse: {e}")))?;
        let items = v
            .get("items")
            .and_then(|x| x.as_array())
            .cloned()
            .unwrap_or_default();
        if items.is_empty() {
            return Ok(ToolResult::ok("(no events in window)".to_string()));
        }
        let mut out = String::new();
        for item in &items {
            out.push_str(&format_event_summary(item));
            out.push('\n');
        }
        Ok(ToolResult::ok(truncate_for_model(out.trim_end(), 8_000)))
    }
}

fn format_event_summary(v: &Value) -> String {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
    let summary = v.get("summary").and_then(|x| x.as_str()).unwrap_or("(no title)");
    let start = v
        .get("start")
        .and_then(|x| x.get("dateTime").or_else(|| x.get("date")))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let end = v
        .get("end")
        .and_then(|x| x.get("dateTime").or_else(|| x.get("date")))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let location = v.get("location").and_then(|x| x.as_str()).unwrap_or("");
    let location_part = if location.is_empty() {
        String::new()
    } else {
        format!("  ({location})")
    };
    format!("[{id}] {summary}  {start} → {end}{location_part}")
}

/// `calendar_get_event` — fetch a single event by id.
/// Read-only; no consent.
pub struct CalendarGetEventTool;

#[async_trait]
impl Tool for CalendarGetEventTool {
    fn name(&self) -> &str {
        "calendar_get_event"
    }

    fn description(&self) -> &str {
        "Fetch a single Google Calendar event by id. Returns summary, start, end, location, attendees, description. Use after `calendar_list_events` to drill into a specific event. Read-only — no consent required."
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
                    "description": "The Google Calendar event id (from `calendar_list_events`)."
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
        let token = require_google_token(&settings)?;
        let client = http_client()?;
        let resp = client
            .get(format!("{CAL_BASE}/events/{id}"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar get: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "calendar get returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar get parse: {e}")))?;
        let out = format_event_full(&v);
        Ok(ToolResult::ok(truncate_for_model(&out, 8_000)))
    }
}

fn format_event_full(v: &Value) -> String {
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
    let summary = v.get("summary").and_then(|x| x.as_str()).unwrap_or("(no title)");
    let start = v
        .get("start")
        .and_then(|x| x.get("dateTime").or_else(|| x.get("date")))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let end = v
        .get("end")
        .and_then(|x| x.get("dateTime").or_else(|| x.get("date")))
        .and_then(|x| x.as_str())
        .unwrap_or("?");
    let location = v.get("location").and_then(|x| x.as_str()).unwrap_or("");
    let description = v.get("description").and_then(|x| x.as_str()).unwrap_or("");
    let attendees: Vec<String> = v
        .get("attendees")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| a.get("email").and_then(|e| e.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let attendees_part = if attendees.is_empty() {
        String::new()
    } else {
        format!("\nAttendees: {}", attendees.join(", "))
    };
    let location_part = if location.is_empty() {
        String::new()
    } else {
        format!("\nLocation: {location}")
    };
    let description_part = if description.is_empty() {
        String::new()
    } else {
        format!("\n\n{description}")
    };
    format!("[{id}] {summary}\nStart: {start}  End: {end}{location_part}{attendees_part}{description_part}")
}

/// `calendar_create_event` — create a new event on the
/// primary calendar. Per-call consent; mutating.
pub struct CalendarCreateEventTool;

#[async_trait]
impl Tool for CalendarCreateEventTool {
    fn name(&self) -> &str {
        "calendar_create_event"
    }

    fn description(&self) -> &str {
        "Create a new event on the primary Google Calendar. Per-call consent (this hits the `insert` endpoint). `summary` is the title. `start` and `end` are ISO-8601 strings (e.g. '2026-09-10T15:00:00-06:00'). `attendees` is an optional list of email addresses."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "Event title."},
                "start": {"type": "string", "description": "Event start (ISO-8601)."},
                "end": {"type": "string", "description": "Event end (ISO-8601)."},
                "attendees": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional list of attendee email addresses."
                }
            },
            "required": ["summary", "start", "end"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let summary = invocation
            .arguments
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `summary`".to_string()))?;
        let start = invocation
            .arguments
            .get("start")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `start`".to_string()))?;
        let end = invocation
            .arguments
            .get("end")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArguments("missing `end`".to_string()))?;
        let attendees: Vec<String> = invocation
            .arguments
            .get("attendees")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|a| a.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let state = require_state(&context)?;
        let settings = state
            .db
            .load_settings()
            .map_err(|e| ToolError::Execution(format!("load settings: {e}")))?;
        let token = require_google_token(&settings)?;

        let mut body = json!({
            "summary": summary,
            "start": { "dateTime": start },
            "end": { "dateTime": end },
        });
        if !attendees.is_empty() {
            body["attendees"] = json!(
                attendees
                    .iter()
                    .map(|e| json!({ "email": e }))
                    .collect::<Vec<_>>()
            );
        }

        let client = http_client()?;
        let resp = client
            .post(format!("{CAL_BASE}/events"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar create: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "calendar create returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar create parse: {e}")))?;
        let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
        Ok(ToolResult::ok(format!("created: {id}")))
    }
}

/// `calendar_update_event` — patch an existing event.
/// Per-call consent; mutating.
pub struct CalendarUpdateEventTool;

#[async_trait]
impl Tool for CalendarUpdateEventTool {
    fn name(&self) -> &str {
        "calendar_update_event"
    }

    fn description(&self) -> &str {
        "Update an existing Google Calendar event. Per-call consent (this hits the `patch` endpoint). Supply any subset of `summary`, `start`, `end`, `attendees`; missing fields are left untouched. PATCH semantics — partial updates are fine."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Event id to update."},
                "summary": {"type": "string", "description": "New title (optional)."},
                "start": {"type": "string", "description": "New start (ISO-8601, optional)."},
                "end": {"type": "string", "description": "New end (ISO-8601, optional)."},
                "attendees": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "New attendee list (optional — replaces the existing one)."
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
        let token = require_google_token(&settings)?;

        let mut body = json!({});
        if let Some(s) = invocation.arguments.get("summary").and_then(|v| v.as_str()) {
            body["summary"] = json!(s);
        }
        if let Some(s) = invocation.arguments.get("start").and_then(|v| v.as_str()) {
            body["start"] = json!({ "dateTime": s });
        }
        if let Some(s) = invocation.arguments.get("end").and_then(|v| v.as_str()) {
            body["end"] = json!({ "dateTime": s });
        }
        if let Some(arr) = invocation.arguments.get("attendees").and_then(|v| v.as_array()) {
            let emails: Vec<String> = arr
                .iter()
                .filter_map(|a| a.as_str().map(String::from))
                .collect();
            body["attendees"] = json!(
                emails
                    .iter()
                    .map(|e| json!({ "email": e }))
                    .collect::<Vec<_>>()
            );
        }
        if body.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            return Err(ToolError::InvalidArguments(
                "calendar_update_event needs at least one of summary/start/end/attendees".to_string(),
            ));
        }

        let client = http_client()?;
        let resp = client
            .patch(format!("{CAL_BASE}/events/{id}"))
            .bearer_auth(&token)
            .json(&body)
            .send()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar update: {e}")))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(ToolError::Execution(format!(
                "calendar update returned {}: {}",
                status.as_u16(),
                truncate_for_model(&body, 400)
            )));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ToolError::Execution(format!("calendar update parse: {e}")))?;
        let new_id = v.get("id").and_then(|x| x.as_str()).unwrap_or("?");
        Ok(ToolResult::ok(format!("updated: {new_id}")))
    }
}

/// Test the Calendar connection by listing a single
/// event from the next 24 hours. Used by the
/// BotEditor's "Test connection" button.
pub async fn test_connection(settings: &Settings) -> Result<String, String> {
    let token = require_google_token(settings).map_err(|e| e.to_string())?;
    let client = http_client().map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();
    let tomorrow = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let url = format!(
        "{CAL_BASE}/events?timeMin={}&timeMax={}&maxResults=1",
        urlencoding_encode(&now),
        urlencoding_encode(&tomorrow)
    );
    let resp = client
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| format!("calendar list: {e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "calendar list returned {}: {}",
            status.as_u16(),
            truncate_for_model(&body, 400)
        ));
    }
    Ok("Calendar OK".to_string())
}

/// Local copy of the urlencoder used by the Gmail
/// connector — both tools need the same minimal
/// percent-encoding. (We could lift it to a shared
/// `connectors::http` module, but for three uses a
/// private fn is shorter.)
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push_str("%20"),
            ':' => out.push_str("%3A"),
            '+' => out.push_str("%2B"),
            _ => out.push_str(&format!("%{:02X}", c as u32)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_token(tok: &str) -> Settings {
        let mut s = Settings::default();
        s.google_access_token = Some(tok.to_string());
        s
    }

    #[test]
    fn require_google_token_returns_trimmed_value() {
        let s = settings_with_token("  abc  ");
        let t = require_google_token(&s).unwrap();
        assert_eq!(t, "abc");
    }

    #[test]
    fn require_google_token_errors_when_unset() {
        let s = Settings::default();
        assert!(require_google_token(&s).is_err());
    }

    #[test]
    fn format_event_summary_includes_summary_and_window() {
        let v = json!({
            "id": "evt1",
            "summary": "Standup",
            "start": { "dateTime": "2026-09-10T15:00:00-06:00" },
            "end": { "dateTime": "2026-09-10T15:30:00-06:00" }
        });
        let s = format_event_summary(&v);
        assert!(s.contains("evt1"));
        assert!(s.contains("Standup"));
        assert!(s.contains("2026-09-10T15:00:00-06:00"));
    }

    #[test]
    fn format_event_full_includes_attendees_and_location() {
        let v = json!({
            "id": "evt1",
            "summary": "Lunch",
            "start": { "dateTime": "2026-09-10T12:00:00-06:00" },
            "end": { "dateTime": "2026-09-10T13:00:00-06:00" },
            "location": "Cafe",
            "attendees": [{"email": "a@b.com"}, {"email": "c@d.com"}],
            "description": "Casual"
        });
        let s = format_event_full(&v);
        assert!(s.contains("Lunch"));
        assert!(s.contains("Cafe"));
        assert!(s.contains("a@b.com"));
        assert!(s.contains("c@d.com"));
        assert!(s.contains("Casual"));
    }

    #[test]
    fn urlencoding_encode_handles_plus_and_colons() {
        assert_eq!(urlencoding_encode("a+b"), "a%2Bb");
        assert_eq!(urlencoding_encode("2026-09-10T15:00:00+00:00"), "2026-09-10T15%3A00%3A00%2B00%3A00");
    }
}
