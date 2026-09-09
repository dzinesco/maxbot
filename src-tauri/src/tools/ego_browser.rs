//! `ego_browser` — run a JavaScript script in the ego (lite) embedded
//! Node.js runtime, which drives a real Chromium browser the agent
//! controls.
//!
//! Why this tool exists: the older `safari_*` / `chrome_*` tools use
//! AppleScript to drive Safari or Chrome. They need TCC permissions
//! (Automation > Safari/Chrome), and they're brittle — every page is
//! a different DOM. ego (lite) is purpose-built for agents: it gives
//! the agent a task space, reuses the user's login state, and exposes
//! a Node.js runtime preloaded with helpers (`snapshotText`, `click`,
//! `openOrReuseTab`, `js`, `cdp`, etc.) so the agent can drive a real
//! page with one script. The full skill is at
//! `~/.agents/skills/ego-browser/SKILL.md` if more detail is needed.
//!
//! Wire format (one subprocess per call):
//!   `ego-browser nodejs -e <script>`
//! stdout is returned to the model (truncated to 16K chars). Stderr
//! is included on non-zero exit so the model can see what failed.
//!
//! Per-call consent because the runtime can navigate, click, type,
//! fill forms, and run arbitrary JS in the page's origin. The
//! subprocess is hard-capped at 2 minutes; ego's helpers handle
//! their own timeouts inside the script (wait / timeout / settle
//! values are in seconds; only `*Ms` params are milliseconds).

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::registry::truncate_for_model;
use super::system::{optional_string, require_string};
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

/// Hard cap on a single `ego-browser nodejs` invocation. 2 minutes
/// matches ego's own recommended script budget — long enough for a
/// full multi-step flow (login, navigate, snapshot, extract), short
/// enough that a hung runtime surfaces as a tool error instead of
/// blocking the chat turn.
const HARD_TIMEOUT: Duration = Duration::from_secs(120);

/// Default install location for the `ego-browser` CLI on macOS.
/// Onboarding drops it here per the skill's install doc.
const DEFAULT_BINARY: &str = ".local/bin/ego-browser";

/// Cached resolved path of the `ego-browser` binary. We resolve
/// once per process because the path doesn't change at runtime and
/// the lookup touches the filesystem.
fn resolved_binary() -> &'static str {
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE.get_or_init(|| resolve_binary_inner()).as_str()
}

fn resolve_binary_inner() -> String {
    // 1. $HOME/.local/bin/ego-browser — the standard install path
    //    documented in the skill. Check it first because PATH
    //    inside a hardened-runtime Tauri app may not include
    //    ~/.local/bin even when the user's normal shell does.
    if let Some(home) = std::env::var_os("HOME") {
        let p = PathBuf::from(home).join(DEFAULT_BINARY);
        if p.is_file() {
            return p.to_string_lossy().to_string();
        }
    }
    // 2. Bare name on PATH — covers anything else (`brew install`,
    //    custom locations the user symlinked into PATH). The OS
    //    resolves at exec time so we don't have to walk PATH
    //    ourselves.
    "ego-browser".to_string()
}

pub struct EgoBrowserTool;

#[async_trait]
impl Tool for EgoBrowserTool {
    fn name(&self) -> &str {
        "ego_browser"
    }

    fn description(&self) -> &str {
        "Run a JavaScript script in the ego (lite) embedded Node.js \
         runtime, which drives a real Chromium browser the agent \
         controls. ego (lite) is an agent-friendly browser: each \
         script runs in an isolated 'task space' that inherits the \
         user's login state, so you can drive authenticated sites \
         without competing for the user's own tabs. \n\n\
         ---\n\
         The runtime preloads helpers. Use them, don't reinvent:\n\
         - Task spaces: useOrCreateTaskSpace, listTaskSpaces, \
         claimTaskSpace, handOffTaskSpace, takeOverTaskSpace, \
         waitForAgentControl, completeTaskSpace\n\
         - Navigation: openOrReuseTab, gotoAndWait, gotoUrl, \
         listTabs, switchTab, currentTab, pageInfo, closeTab, \
         ensureRealTab\n\
         - Observation: snapshotText (full-page semantic tree with \
         @N refs and loc=... values), captureScreenshot, drainEvents\n\
         - Interaction: click, doubleClick, hover, dragMouse, \
         scroll, scrollBy, scrollToBottomUntil, typeText, fillInput, \
         pressKey, dispatchKey, uploadFile\n\
         - Wait: wait, waitForLoad, waitForElement, waitForNetworkIdle\n\
         - Fetch: serverFetch (from Node), browserFetch (page context)\n\
         - CDP / DOM: js (≈ Runtime.evaluate — string, not function), cdp\n\
         - Output: cliLog (the ONLY way to print — terminal stdout)\n\
         ---\n\
         Three workflows. Pick the right one:\n\
         1. SEMANTIC (default): snapshotText → click @N / loc=... → \
         snapshotText again. Best for normal pages with text, \
         buttons, forms.\n\
         2. VISUAL: captureScreenshot → click [x,y] / typeText → \
         screenshot again. Best for canvas / rich editors / maps / \
         virtualized surfaces (Google Docs, Sheets, Notion, Figma).\n\
         3. DIRECT DOM/CDP: js / cdp for compact data extraction or \
         custom browser state. Wrap multi-step logic in a single IIFE \
         and return once.\n\
         ---\n\
         Multi-call: Node exits after each call and keeps no state, \
         so to operate on the same browser context across multiple \
         tool calls pass a stable `task_space` name. The tool \
         auto-prepends `useOrCreateTaskSpace(name)` to your script, \
         selecting (or creating) that space. Pick a name that \
         reflects the user goal (e.g. 'github pr review'). Reuse it \
         for follow-ups, corrections, validations.\n\
         ---\n\
         RULES (read these — they bite):\n\
         - cliLog(value) is the only way to print. All final results \
         must go through it. The tool returns whatever you cliLog.\n\
         - wait(...) and timeout values are in SECONDS. Only *Ms \
         params are milliseconds.\n\
         - js() takes a STRING, not a function. Closures are not \
         captured. Wrap multi-step logic in a self-invoking IIFE \
         and return once.\n\
         - Inside js() template strings, double backslashes \
         (\\\\d, \\\\s) or use String.raw.\n\
         - @N refs are only valid for the most recent snapshotText \
         call. After DOM changes or new snapshots, take a fresh \
         snapshot.\n\
         - js() returns the evaluated RESULT (not a JSON string). Do \
         not wrap in JSON.parse.\n\
         - pageInfo() returns {dialog:...} when a native dialog is \
         open — handle with cdp('Page.handleJavaScriptDialog', ...) \
         before running page JS.\n\
         - When the task is done, call \
         completeTaskSpace(name, { keep: false }) in a final \
         dedicated call. { keep: true } is only for cases where the \
         user must see the page live.\n\
         - If the runtime reports 'user is controlling', STOP. Do \
         not retry. Ask the user and wait for explicit confirmation.\n\
         ---\n\
         Login / captcha: when the site needs human action, call \
         handOffTaskSpace(name) to give control back to the user, \
         tell them what to do, and wait. Never put passwords or \
         verification codes in the script. After they confirm \
         'continue', start the next call with \
         takeOverTaskSpace(name) and resume.\n\
         ---\n\
         The full reference (every helper, every option) lives at \
         ~/.agents/skills/ego-browser/SKILL.md — read it if you're \
         about to do something not covered above.\n\
         ---\n\
         Per-call consent because the runtime can navigate, click, \
         fill forms, and run arbitrary JS in the page's origin."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "script": {
                    "type": "string",
                    "description": "JavaScript source to run in the ego-browser Node.js runtime. Helpers (snapshotText, click, openOrReuseTab, cliLog, js, cdp, etc.) are preloaded. All final results must go through cliLog(...). Multi-step work should be one coherent script: observe → act → verify → cliLog the answer."
                },
                "task_space": {
                    "type": "string",
                    "description": "Optional task space name or id to reuse across calls. If provided, the tool prepends `useOrCreateTaskSpace(name)` to your script so the same browser context (tabs, login state) is selected. Use a short name that reflects the user goal (e.g. 'github pr review') and reuse it for follow-ups, corrections, and validations. If omitted, the script is responsible for calling useOrCreateTaskSpace(...) itself."
                }
            },
            "required": ["script"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Validate first so a malformed call doesn't get a consent
        // dialog. require_string catches missing / wrong-type;
        // the trim+empty check rejects whitespace-only.
        let script = require_string(&invocation.arguments, "script")?;
        if script.trim().is_empty() {
            return Err(ToolError::InvalidArguments("script is empty".to_string()));
        }
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the ego_browser action".to_string(),
            ));
        }
        let task_space = optional_string(&invocation.arguments, "task_space")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());

        // If a task_space was provided, prepend useOrCreateTaskSpace
        // so the runtime has a selected context. The return value
        // is the task object; the user's script can ignore it.
        let final_script = match task_space.as_deref() {
            Some(name) => {
                // JSON-encode the name so backslashes, quotes, and
                // unicode are safely embedded in the JS source.
                let encoded = serde_json::to_string(name)
                    .map_err(|e| ToolError::Execution(format!("encode task_space: {e}")))?;
                format!(
                    "const __maxbotTask = await useOrCreateTaskSpace({encoded})\n{script}"
                )
            }
            None => script.clone(),
        };

        // Resolve the binary. If ~/.local/bin/ego-browser doesn't
        // exist and the bare name isn't on PATH, the spawn below
        // will fail with a clear error; we don't try to preflight
        // with `which` (matches the skill's "don't pre-check" rule
        // for an already-installed environment).
        let binary = resolved_binary().to_string();

        let mut cmd = Command::new(&binary);
        cmd.arg("nodejs").arg("-e").arg(&final_script);
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let output = match tokio::time::timeout(HARD_TIMEOUT, cmd.output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => {
                return Err(ToolError::Execution(format!(
                    "spawn '{binary}': {e}. Is ego (lite) installed? \
                     See ~/.agents/skills/ego-browser/references/install.md."
                )));
            }
            Err(_) => {
                return Err(ToolError::Execution(format!(
                    "ego-browser timed out after {HARD_TIMEOUT:?}"
                )));
            }
        };

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            // Even on success, surface a short stderr tail so the
            // model can see ego's own log lines (e.g. "[agent]
            // claimTaskSpace(...)") if anything noteworthy happened.
            let body = if stderr.trim().is_empty() {
                stdout.to_string()
            } else {
                format!(
                    "{stdout}\n--- ego stderr ---\n{}",
                    stderr.trim()
                )
            };
            Ok(ToolResult::ok(truncate_for_model(&body, 16_000)))
        } else {
            // Failure: combine stdout + stderr so the model sees
            // both. cliLog output may still contain the partial
            // result before the script errored.
            let mut msg = format!(
                "ego-browser exited with code {:?}\n",
                output.status.code()
            );
            if !stderr.trim().is_empty() {
                msg.push_str(&format!("stderr:\n{}\n", stderr.trim()));
            }
            if !stdout.trim().is_empty() {
                msg.push_str(&format!("stdout:\n{}\n", stdout.trim()));
            }
            Ok(ToolResult::err(truncate_for_model(&msg, 8_000)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_binary_prefers_local_install_path() {
        // When HOME is set (always, in tests) we look for
        // $HOME/.local/bin/ego-browser first. If that file
        // exists, it wins. Otherwise we fall back to the bare
        // name. We don't assert which branch is taken — the
        // important thing is the result is non-empty and is
        // either the absolute path or 'ego-browser'.
        let resolved = resolved_binary();
        assert!(!resolved.is_empty());
        assert!(
            resolved == "ego-browser" || resolved.ends_with("/ego-browser"),
            "unexpected resolved binary: {resolved}"
        );
    }

    #[test]
    fn description_is_substantial_enough_to_drive_a_script() {
        // The model writes a script based on this description. It
        // has to mention the key helpers and the cliLog rule, or
        // the model will reimplement them badly. Cheap regression
        // check that protects against accidental description
        // trims.
        let d = EgoBrowserTool.description();
        assert!(d.contains("cliLog"), "must mention cliLog");
        assert!(d.contains("snapshotText"), "must mention snapshotText");
        assert!(d.contains("useOrCreateTaskSpace"), "must mention useOrCreateTaskSpace");
        assert!(d.contains("task space"), "must explain task spaces");
        assert!(d.len() > 800, "description got trimmed below 800 chars");
    }

    #[tokio::test]
    async fn rejects_empty_script() {
        let result = EgoBrowserTool
            .execute(
                ToolInvocation {
                    name: "ego_browser".to_string(),
                    arguments: json!({ "script": "   \n  " }),
                    id: "x".to_string(),
                    bot_id: None,
                },
                ToolContext::default(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn requires_consent() {
        // A non-empty script with default (no consent) context
        // should hit the consent branch, not validation.
        let result = EgoBrowserTool
            .execute(
                ToolInvocation {
                    name: "ego_browser".to_string(),
                    arguments: json!({ "script": "cliLog('hi')" }),
                    id: "x".to_string(),
                    bot_id: None,
                },
                ToolContext::default(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::Execution(_))));
    }

    #[test]
    fn task_space_is_json_encoded_into_script() {
        // Verify the prepended line is syntactically valid JS and
        // that special characters in the task_space name don't
        // break the literal. We test by constructing the same
        // string the tool would and checking the substring is
        // present.
        let name = "weird name with \"quotes\" and \\backslashes";
        let encoded = serde_json::to_string(name).unwrap();
        let user_script = "cliLog('hello')";
        let prepended = format!(
            "const __maxbotTask = await useOrCreateTaskSpace({encoded})\n{user_script}"
        );
        // The encoded form is the canonical JSON string with
        // escaped quotes and backslashes — so the JS literal is
        // safe.
        assert!(prepended.contains("useOrCreateTaskSpace(\"weird name with"));
        assert!(prepended.contains("\\\"quotes\\\""));
        assert!(prepended.contains("\\\\backslashes"));
        // And the user's script follows on the next line.
        assert!(prepended.ends_with("cliLog('hello')"));
    }
}
