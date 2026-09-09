//! `mail_*` tools — drive Apple Mail via AppleScript.
//!
//! Tools:
//! - `mail_inbox` (no consent) — list the most recent inbox messages
//!   with subject, sender, date received, and a short snippet.
//! - `mail_search` (no consent) — search inbox messages by query
//!   string against subject + sender + content.
//! - `mail_send` (per-call consent) — send a new outgoing message.
//! - `mail_draft` (per-call consent) — create a draft (reversible).
//!
//! All tools operate on the default Mail account; multi-account
//! support is a v0.4.4+ concern.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::system::{escape_osa, format_stderr, optional_string, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

const DEFAULT_LIMIT: u32 = 20;
const MAX_LIMIT: u32 = 100;
const SNIPPET_CHARS: usize = 200;

pub struct MailInboxTool;

#[async_trait]
impl Tool for MailInboxTool {
    fn name(&self) -> &str {
        "mail_inbox"
    }

    fn description(&self) -> &str {
        "List the most recent messages in the default Mail account's inbox. Each entry includes sender, subject, date received, and a short snippet. Default 20 messages, max 100. No consent required (read-only)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20,
                    "description": "How many of the most recent messages to return."
                },
                "snippet_chars": {
                    "type": "integer",
                    "minimum": 50,
                    "maximum": 2000,
                    "default": 200,
                    "description": "Max characters of each message body to include as a snippet."
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let limit = invocation
            .arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);
        let snippet = invocation
            .arguments
            .get("snippet_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(50, 2_000))
            .unwrap_or(SNIPPET_CHARS);

        let script = build_inbox_script(limit, snippet);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;

        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                "(inbox is empty)".to_string()
            } else {
                out.stdout
            };
            return Ok(ToolResult::ok(truncate_for_model(&body, 12_000)));
        }
        Ok(ToolResult::err(format_stderr(&out)))
    }
}

pub struct MailSearchTool;

#[async_trait]
impl Tool for MailSearchTool {
    fn name(&self) -> &str {
        "mail_search"
    }

    fn description(&self) -> &str {
        "Search the default Mail account's inbox for messages matching a query string. The search is case-insensitive and matches against subject, sender, and content. Returns the same format as mail_inbox. No consent required (read-only)."
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
                    "description": "Search string. Matched case-insensitively against subject, sender, and content."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "default": 20,
                    "description": "How many matching messages to return."
                },
                "snippet_chars": {
                    "type": "integer",
                    "minimum": 50,
                    "maximum": 2000,
                    "default": 200,
                    "description": "Max characters of each message body to include as a snippet."
                }
            },
            "required": ["query"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let query = require_string(&invocation.arguments, "query")?;
        if query.trim().is_empty() {
            return Err(ToolError::InvalidArguments(
                "query must not be empty".to_string(),
            ));
        }
        let limit = invocation
            .arguments
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| (n as u32).clamp(1, MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT);
        let snippet = invocation
            .arguments
            .get("snippet_chars")
            .and_then(|v| v.as_u64())
            .map(|n| (n as usize).clamp(50, 2_000))
            .unwrap_or(SNIPPET_CHARS);

        let script = build_search_script(&query, limit, snippet);
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let body = if out.stdout.trim().is_empty() {
                format!("(no messages matched '{}')", query)
            } else {
                out.stdout
            };
            return Ok(ToolResult::ok(truncate_for_model(&body, 12_000)));
        }
        Ok(ToolResult::err(format_stderr(&out)))
    }
}

pub struct MailSendTool;

#[async_trait]
impl Tool for MailSendTool {
    fn name(&self) -> &str {
        "mail_send"
    }

    fn description(&self) -> &str {
        "Send a new email from the default Mail account. Requires per-call consent because mail_send is a destructive external action. Use mail_draft instead if you want to compose a draft for the user to review before sending."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Comma-separated list of recipient addresses, e.g. 'alice@example.com, bob@example.com'."
                },
                "subject": {
                    "type": "string",
                    "description": "The email subject line."
                },
                "body": {
                    "type": "string",
                    "description": "The plain-text body of the email."
                },
                "cc": {
                    "type": "string",
                    "description": "Optional comma-separated CC list."
                },
                "bcc": {
                    "type": "string",
                    "description": "Optional comma-separated BCC list."
                }
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
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the mail_send action".to_string(),
            ));
        }
        let to = require_string(&invocation.arguments, "to")?;
        let subject = require_string(&invocation.arguments, "subject")?;
        let body = require_string(&invocation.arguments, "body")?;
        let cc = optional_string(&invocation.arguments, "cc");
        let bcc = optional_string(&invocation.arguments, "bcc");

        let script = build_send_script(&to, &subject, &body, cc.as_deref(), bcc.as_deref());
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!(
                "sent: to={} subject=\"{}\"",
                to,
                subject
            )))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct MailDraftTool;

#[async_trait]
impl Tool for MailDraftTool {
    fn name(&self) -> &str {
        "mail_draft"
    }

    fn description(&self) -> &str {
        "Create a draft email in the default Mail account (no send). The draft is reversible — the user can edit, delete, or send it from Mail.app. Requires per-call consent because drafts still appear in the user's Drafts folder and may auto-send if they have a rule."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Comma-separated list of recipient addresses."
                },
                "subject": {
                    "type": "string",
                    "description": "The email subject line."
                },
                "body": {
                    "type": "string",
                    "description": "The plain-text body of the email."
                },
                "cc": {
                    "type": "string",
                    "description": "Optional comma-separated CC list."
                },
                "bcc": {
                    "type": "string",
                    "description": "Optional comma-separated BCC list."
                }
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
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the mail_draft action".to_string(),
            ));
        }
        let to = require_string(&invocation.arguments, "to")?;
        let subject = require_string(&invocation.arguments, "subject")?;
        let body = require_string(&invocation.arguments, "body")?;
        let cc = optional_string(&invocation.arguments, "cc");
        let bcc = optional_string(&invocation.arguments, "bcc");

        let script = build_draft_script(&to, &subject, &body, cc.as_deref(), bcc.as_deref());
        let out = run_osa_script(&script, 30)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!(
                "drafted: to={} subject=\"{}\"",
                to,
                subject
            )))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

// ---- AppleScript builders ----

fn build_inbox_script(limit: u32, snippet: usize) -> String {
    format!(
        r#"
set lim to {limit}
set snippetLen to {snippet}
set out to ""
tell application "Mail"
  set msgs to messages of inbox
  set msgCount to (count msgs)
  if msgCount < lim then set lim to msgCount
  repeat with i from 1 to lim
    set m to item i of msgs
    set subj to (subject of m) as string
    if subj is missing value then set subj to "(no subject)"
    set sndr to (sender of m) as string
    if sndr is missing value then set sndr to "(unknown sender)"
    set dt to (date received of m) as string
    set body to (content of m) as string
    if (count of body) > snippetLen then
      set body to text 1 thru snippetLen of body
    end if
    set out to out & "<<<MESSAGE " & i & ">>>" & return & "From: " & sndr & return & "Subject: " & subj & return & "Received: " & dt & return & "Snippet: " & body & return & return
  end repeat
end tell
return out
"#
    )
}

fn build_search_script(query: &str, limit: u32, snippet: usize) -> String {
    let q_escaped = escape_osa(query);
    format!(
        r#"
set theQuery to "{q_escaped}"
set lim to {limit}
set snippetLen to {snippet}
set out to ""
tell application "Mail"
  set msgs to (every message of inbox whose (subject contains theQuery) or (sender contains theQuery) or (content contains theQuery))
  set msgCount to (count msgs)
  if msgCount < lim then set lim to msgCount
  if lim is 0 then return ""
  repeat with i from 1 to lim
    set m to item i of msgs
    set subj to (subject of m) as string
    if subj is missing value then set subj to "(no subject)"
    set sndr to (sender of m) as string
    if sndr is missing value then set sndr to "(unknown sender)"
    set dt to (date received of m) as string
    set body to (content of m) as string
    if (count of body) > snippetLen then
      set body to text 1 thru snippetLen of body
    end if
    set out to out & "<<<MESSAGE " & i & ">>>" & return & "From: " & sndr & return & "Subject: " & subj & return & "Received: " & dt & return & "Snippet: " & body & return & return
  end repeat
end tell
return out
"#
    )
}

fn build_send_script(
    to: &str,
    subject: &str,
    body: &str,
    cc: Option<&str>,
    bcc: Option<&str>,
) -> String {
    let subj = escape_osa(subject);
    let body = escape_osa(body);
    let to = escape_osa(to);
    let cc_block = cc
        .map(|c| {
            format!(
                r#"    repeat with addr in (my splitCSV("{e}"))
      make new cc recipient at end of cc recipients with properties {{address:addr}}
    end repeat
"#,
                e = escape_osa(c)
            )
        })
        .unwrap_or_default();
    let bcc_block = bcc
        .map(|b| {
            format!(
                r#"    repeat with addr in (my splitCSV("{e}"))
      make new bcc recipient at end of bcc recipients with properties {{address:addr}}
    end repeat
"#,
                e = escape_osa(b)
            )
        })
        .unwrap_or_default();
    format!(
        r#"on splitCSV(s)
  set tid to AppleScript's text item delimiters
  set AppleScript's text item delimiters to ","
  set rawParts to text items of s
  set AppleScript's text item delimiters to tid
  set cleaned to {{}}
  repeat with p in rawParts
    set end of cleaned to (my trim(p))
  end repeat
  return cleaned
end splitCSV

on trim(s)
  set t to s
  repeat while (count of t) > 0 and (first character of t) is " "
    set t to text 2 thru -1 of t
  end repeat
  repeat while (count of t) > 0 and (last character of t) is " "
    set t to text 1 thru -2 of t
  end repeat
  return t
end trim

tell application "Mail"
  set newMsg to make new outgoing message with properties {{subject:"{subj}", content:"{body}"}}
  tell newMsg
    repeat with addr in (my splitCSV("{to}"))
      make new to recipient at end of to recipients with properties {{address:addr}}
    end repeat
{cc_block}{bcc_block}    send newMsg
  end tell
end tell
return "sent"
"#
    )
}

fn build_draft_script(
    to: &str,
    subject: &str,
    body: &str,
    cc: Option<&str>,
    bcc: Option<&str>,
) -> String {
    let subj = escape_osa(subject);
    let body = escape_osa(body);
    let to = escape_osa(to);
    let cc_block = cc
        .map(|c| {
            format!(
                r#"    repeat with addr in (my splitCSV("{e}"))
      make new cc recipient at end of cc recipients with properties {{address:addr}}
    end repeat
"#,
                e = escape_osa(c)
            )
        })
        .unwrap_or_default();
    let bcc_block = bcc
        .map(|b| {
            format!(
                r#"    repeat with addr in (my splitCSV("{e}"))
      make new bcc recipient at end of bcc recipients with properties {{address:addr}}
    end repeat
"#,
                e = escape_osa(b)
            )
        })
        .unwrap_or_default();
    format!(
        r#"on splitCSV(s)
  set tid to AppleScript's text item delimiters
  set AppleScript's text item delimiters to ","
  set rawParts to text items of s
  set AppleScript's text item delimiters to tid
  set cleaned to {{}}
  repeat with p in rawParts
    set end of cleaned to (my trim(p))
  end repeat
  return cleaned
end splitCSV

on trim(s)
  set t to s
  repeat while (count of t) > 0 and (first character of t) is " "
    set t to text 2 thru -1 of t
  end repeat
  repeat while (count of t) > 0 and (last character of t) is " "
    set t to text 1 thru -2 of t
  end repeat
  return t
end trim

tell application "Mail"
  set newMsg to make new outgoing message with properties {{subject:"{subj}", content:"{body}"}}
  tell newMsg
    repeat with addr in (my splitCSV("{to}"))
      make new to recipient at end of to recipients with properties {{address:addr}}
    end repeat
{cc_block}{bcc_block}  end tell
end tell
return "drafted"
"#
    )
}
