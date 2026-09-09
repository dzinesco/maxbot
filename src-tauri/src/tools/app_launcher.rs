//! `app_open` and `app_list` — launch any macOS application by name
//! and discover what's installed. Built on the built-in `open` shell
//! command (no AppleScript, no TCC), which makes the path universally
//! applicable: Calendar, Safari, Mail, Music, Notes, Finder, System
//! Settings, third-party apps — anything in `/Applications`.
//!
//! `app_open` requires per-call consent because it launches software.
//! `app_list` is no-consent (read-only directory listing).

use std::process::Stdio;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::registry::truncate_for_model;
use super::system::{require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

/// Standard places macOS apps live. `open -a` searches the Launch
/// Services database which includes all of these plus anything
/// Launch Services has indexed.
const SEARCH_DIRECTORIES: &[&str] = &[
    "/Applications",
    "/System/Applications",
    "/System/Library/CoreServices",
];

pub struct AppOpenTool;

#[async_trait]
impl Tool for AppOpenTool {
    fn name(&self) -> &str {
        "app_open"
    }

    fn description(&self) -> &str {
        "Launch a macOS application by name and bring it to the front. \
         `name` is the app's display name (e.g. 'Calendar', 'Safari', \
         'Mail', 'Music', 'Notes', 'Finder', 'System Settings') or its \
         bundle id (e.g. 'com.apple.calendar'). Equivalent to running \
         `open -a <name>` in Terminal. Returns the resolved app path \
         when the launch succeeds. Per-call consent because it launches \
         software."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "App display name (e.g. 'Calendar') or bundle id (e.g. 'com.apple.calendar')."
                }
            },
            "required": ["name"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Validate first so a malformed call doesn't get a consent
        // dialog. `require_string` is the standard "missing or wrong
        // type" check; the second check rejects whitespace-only.
        let name = require_string(&invocation.arguments, "name")?;
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ToolError::InvalidArguments("name is empty".to_string()));
        }
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the app_open action".to_string(),
            ));
        }
        // Build the open command. `-a` matches by app name OR bundle
        // id. We use the `-W` flag to block until the app has launched
        // (or failed) so the tool result reflects actual launch status
        // instead of fire-and-forget.
        let mut cmd = Command::new("open");
        cmd.arg("-W").arg("-a").arg(trimmed);
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());
        let output = cmd
            .output()
            .await
            .map_err(|e| ToolError::Execution(format!("spawn `open`: {e}")))?;
        if output.status.success() {
            Ok(ToolResult::ok(format!("launched '{trimmed}'")))
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            // `open` returns 1 with a clear stderr when it can't find
            // the app. Surface that as a tool error so the model can
            // fall back to `app_list` to discover the right name.
            let message = if stderr.is_empty() {
                format!("could not launch '{trimmed}' (exit {:?})", output.status.code())
            } else {
                format!("could not launch '{trimmed}': {stderr}")
            };
            Ok(ToolResult::err(message))
        }
    }
}

pub struct AppListTool;

#[async_trait]
impl Tool for AppListTool {
    fn name(&self) -> &str {
        "app_list"
    }

    fn description(&self) -> &str {
        "List macOS applications installed in /Applications, \
         /System/Applications, and /System/Library/CoreServices. \
         `filter` is an optional case-insensitive substring (e.g. \
         'chat', 'studio', 'term'); only apps whose display name \
         contains the substring are returned. Results are sorted \
         alphabetically. No consent required (read-only)."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "filter": {
                    "type": "string",
                    "description": "Optional case-insensitive substring; only apps whose .app name contains it are returned."
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
        let filter = invocation
            .arguments
            .get("filter")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty());

        // Walk each search directory on a blocking thread so we don't
        // stall the async runtime on slow filesystems.
        let dirs: Vec<String> = SEARCH_DIRECTORIES.iter().map(|s| s.to_string()).collect();
        let filter_clone = filter.clone();
        let entries = tokio::task::spawn_blocking(move || -> Vec<String> {
            let mut out: Vec<String> = Vec::new();
            for dir in dirs {
                let read = match std::fs::read_dir(&dir) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                for entry in read.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if !name.ends_with(".app") {
                        continue;
                    }
                    let display = name.trim_end_matches(".app").to_string();
                    if let Some(ref f) = filter_clone {
                        if !display.to_lowercase().contains(f) {
                            continue;
                        }
                    }
                    out.push(display);
                }
            }
            out.sort();
            out.dedup();
            out
        })
        .await
        .map_err(|e| ToolError::Execution(format!("list apps join: {e}")))?;

        if entries.is_empty() {
            return Ok(ToolResult::ok(match filter {
                Some(f) => format!("(no apps matching '{f}')"),
                None => "(no apps found)".to_string(),
            }));
        }
        let body = entries
            .iter()
            .map(|n| format!("- {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(ToolResult::ok(truncate_for_model(&body, 16_000)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn app_list_returns_at_least_a_few_system_apps() {
        // We can't be too strict (CI / different machines) but
        // /System/Applications always has Finder, Terminal, etc.
        let result = AppListTool
            .execute(
                ToolInvocation {
                    name: "app_list".to_string(),
                    arguments: json!({ "filter": "finder" }),
                    id: "x".to_string(),
                },
                ToolContext::default(),
            )
            .await
            .expect("execute");
        assert!(!result.is_error);
        assert!(
            result.content.to_lowercase().contains("finder"),
            "expected 'finder' in {}",
            result.content
        );
    }

    #[tokio::test]
    async fn app_open_rejects_empty_name() {
        let result = AppOpenTool
            .execute(
                ToolInvocation {
                    name: "app_open".to_string(),
                    arguments: json!({ "name": "   " }),
                    id: "x".to_string(),
                },
                ToolContext::default(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn app_open_requires_consent() {
        let result = AppOpenTool
            .execute(
                ToolInvocation {
                    name: "app_open".to_string(),
                    arguments: json!({ "name": "Finder" }),
                    id: "x".to_string(),
                },
                ToolContext::default(),
            )
            .await;
        // Now that validation runs first, a non-empty name with
        // ungranted consent should hit the consent branch.
        assert!(matches!(result, Err(ToolError::Execution(_))));
    }
}
