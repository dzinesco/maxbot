//! `system_*` tools — small macOS system queries and toggles.
//!
//! - `system_notify` — show a macOS notification banner. No consent.
//! - `system_volume_get` / `system_volume_set` — read or change the
//!   output volume. Set requires consent.
//! - `system_dark_mode_get` / `system_dark_mode_set` — read or toggle
//!   the system appearance. Set requires consent.
//! - `system_front_app` — name of the frontmost app. No consent.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::apple_script_exec::run_osa_script;
use super::registry::truncate_for_model;
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

pub struct SystemNotifyTool;

#[async_trait]
impl Tool for SystemNotifyTool {
    fn name(&self) -> &'static str {
        "system_notify"
    }

    fn description(&self) -> &'static str {
        "Show a macOS notification banner. Title is required; subtitle and message are optional. Uses the system Notification Center. No consent required — but be sparing: the user can mute MaxBot notifications globally if you spam them."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "The notification title (bold line)."
                },
                "subtitle": {
                    "type": "string",
                    "description": "Optional second line, slightly de-emphasized."
                },
                "message": {
                    "type": "string",
                    "description": "Optional body text."
                }
            },
            "required": ["title"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let title = require_string(&invocation.arguments, "title")?;
        let subtitle = optional_string(&invocation.arguments, "subtitle");
        let message = optional_string(&invocation.arguments, "message");
        let mut parts = vec![format!("display notification \"{}\"", escape_osa(&title))];
        if let Some(sub) = subtitle {
            parts.push(format!("subtitle \"{}\"", escape_osa(&sub)));
        }
        if let Some(msg) = message {
            parts.push(format!("with title \"{}\"", escape_osa(&msg)));
        }
        let script = parts.join(" ");
        let out = run_osa_script(&script, 5)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok("notification shown".to_string()))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SystemVolumeGetTool;

#[async_trait]
impl Tool for SystemVolumeGetTool {
    fn name(&self) -> &'static str {
        "system_volume_get"
    }

    fn description(&self) -> &'static str {
        "Get the current system output volume as a 0-100 integer."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(
        &self,
        _invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let script = "output volume of (get volume settings)";
        let out = run_osa_script(script, 5)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(out.stdout.trim().to_string()))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SystemVolumeSetTool;

#[async_trait]
impl Tool for SystemVolumeSetTool {
    fn name(&self) -> &'static str {
        "system_volume_set"
    }

    fn description(&self) -> &'static str {
        "Set the system output volume. Pass an integer 0-100. Requires per-call consent because it changes the user's environment."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "level": {
                    "type": "integer",
                    "minimum": 0,
                    "maximum": 100,
                    "description": "The desired output volume (0 = mute, 100 = max)."
                }
            },
            "required": ["level"],
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
                "user denied the system_volume_set action".to_string(),
            ));
        }
        let level = invocation
            .arguments
            .get("level")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| ToolError::InvalidArguments("missing field: level".to_string()))?;
        if !(0..=100).contains(&level) {
            return Err(ToolError::InvalidArguments(
                "level must be 0-100".to_string(),
            ));
        }
        let script = format!("set volume output volume {}", level);
        let out = run_osa_script(&script, 5)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!("set volume to {level}")))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SystemDarkModeGetTool;

#[async_trait]
impl Tool for SystemDarkModeGetTool {
    fn name(&self) -> &'static str {
        "system_dark_mode_get"
    }

    fn description(&self) -> &'static str {
        "Get the current system appearance: 'dark' or 'light'."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(
        &self,
        _invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let script = r#"tell application "System Events" to tell appearance preferences to get dark mode"#;
        let out = run_osa_script(script, 5)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            let v = out.stdout.trim();
            let human = if v == "true" {
                "dark"
            } else if v == "false" {
                "light"
            } else {
                v
            };
            Ok(ToolResult::ok(human.to_string()))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SystemDarkModeSetTool;

#[async_trait]
impl Tool for SystemDarkModeSetTool {
    fn name(&self) -> &'static str {
        "system_dark_mode_set"
    }

    fn description(&self) -> &'static str {
        "Set the system appearance. Pass 'dark' or 'light'. Requires per-call consent because it changes the user's environment."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["dark", "light"],
                    "description": "The desired appearance."
                }
            },
            "required": ["mode"],
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
                "user denied the system_dark_mode_set action".to_string(),
            ));
        }
        let mode = require_string(&invocation.arguments, "mode")?;
        if mode != "dark" && mode != "light" {
            return Err(ToolError::InvalidArguments(
                "mode must be 'dark' or 'light'".to_string(),
            ));
        }
        let script = format!(
            r#"tell application "System Events" to tell appearance preferences to set dark mode to {}"#,
            mode == "dark"
        );
        let out = run_osa_script(&script, 5)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(format!("set appearance to {mode}")))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

pub struct SystemFrontAppTool;

#[async_trait]
impl Tool for SystemFrontAppTool {
    fn name(&self) -> &'static str {
        "system_front_app"
    }

    fn description(&self) -> &'static str {
        "Get the name of the frontmost (focused) application."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({ "type": "object", "properties": {}, "additionalProperties": false })
    }

    async fn execute(
        &self,
        _invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let script = r#"tell application "System Events" to get name of (first application process whose frontmost is true)"#;
        let out = run_osa_script(script, 5)
            .await
            .map_err(ToolError::Execution)?;
        if out.succeeded() {
            Ok(ToolResult::ok(out.stdout.trim().to_string()))
        } else {
            Ok(ToolResult::err(format_stderr(&out)))
        }
    }
}

// ---- helpers shared with other tool files in this module ----

pub(crate) fn require_string(args: &Value, field: &str) -> Result<String, ToolError> {
    args.get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| ToolError::InvalidArguments(format!("missing field: {field}")))
}

pub(crate) fn optional_string(args: &Value, field: &str) -> Option<String> {
    args.get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Escape a string for safe embedding inside an AppleScript double-
/// quoted literal. We escape backslashes and double-quotes.
pub(crate) fn escape_osa(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

pub(crate) fn format_stderr(out: &super::apple_script_exec::AppleScriptOutput) -> String {
    let mut s = if out.stderr.is_empty() {
        format!("osascript exited with code {:?}", out.exit_code)
    } else {
        out.stderr.clone()
    };
    if let Some(hint) = out.tcc_hint() {
        s.push_str(&format!("\n\nHint: {hint}"));
    }
    truncate_for_model(&s, 4_000)
}
