//! v3.2.0 — `vm_computer_use` — drive a Bot's per-Bot Linux VM
//! from the agent loop.
//!
//! The previous Computer Use target was the agent's own Mac
//! (`ego_browser` over AppleScript). v3.2.0 makes the per-Bot
//! VM the default: open a URL in Chromium, type, click, and
//! press keys via `xdotool`, all running INSIDE the VM and
//! returning a screenshot of the post-action desktop state.
//!
//! ## Script format
//!
//! The `script` parameter is a small sequence of helper calls,
//! one per line. Each call is a single helper invocation:
//!
//! ```text
//! open_url("https://example.com")
//! click_at(120, 340)
//! type("hello world")
//! key("Return")
//! screenshot()
//! ```
//!
//! Helpers:
//!   - `screenshot()` — capture the desktop via `scrot`, return
//!     base64 PNG (always the post-action state of the LAST
//!     call in the script).
//!   - `click_at(x, y)` — move the mouse to `(x, y)` and click
//!     button 1. Implemented via `xdotool mousemove x y` +
//!     `xdotool click 1`.
//!   - `type(text)` — type the literal text. Implemented via
//!     `xdotool type -- "..."`. The double-dash is critical
//!     because the LLM's text may start with a `-`.
//!   - `key(name)` — press a key by name. Implemented via
//!     `xdotool key -- name` (e.g. "Return", "Escape",
//!     "ctrl+l").
//!   - `open_url(url)` — open the URL in Chromium with
//!     `--no-sandbox` (the per-Bot VM runs the `bot` user
//!     without the right SUID sandbox config; chromium fails
//!     to launch without `--no-sandbox`).
//!
//! ## Per-call consent
//!
//! `requires_consent = true` because the script can navigate,
//! click, type, and fill arbitrary forms. The chat command
//! surfaces a native consent dialog before invoking `execute`,
//! matching the `ego_browser` policy.
//!
//! ## Why a bespoke parser and not JavaScript
//!
//! `ego_browser` runs in a Node.js runtime with a rich helper
//! library. The VM has no Node.js (and no ego installed), and
//! shipping a JS runtime to the VM for one tool would be a
//! multi-MB bloat. A line-oriented helper-call grammar is
//! sufficient — the model just writes a sequence of
//! `helper(args)` calls and the tool dispatches each via SSH.
//! The grammar is parsed in pure Rust; no second runtime on
//! the VM, no extra dependencies, no escape-hatch risk.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};
use tauri::Manager;

use super::registry::truncate_for_model;
use super::system::require_string;
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};
use crate::AppState;
use crate::computer::ssh::SshExecutor;

/// Path on the VM where `scrot` writes the screenshot before
/// the tool SCPs it back. `/tmp` is a tmpfs on Ubuntu 24.04 by
/// default, so writes are fast; the file is overwritten on each
/// `screenshot()` call and not cleaned up — the VM's overlay
/// reset on destroy makes that a non-issue.
const SCREENSHOT_PATH: &str = "/tmp/maxbot-screen.png";

/// `xdotool` requires a `:N` display to dispatch input; the
/// per-Bot VM's x11vnc session exposes `:1` (see
/// `provision-vm.sh`'s `runcmd` block). We pass this through to
/// xdotool so it doesn't have to guess from `$DISPLAY`.
const VM_DISPLAY: &str = ":1";

pub struct VmComputerUseTool;

#[async_trait]
impl Tool for VmComputerUseTool {
    fn name(&self) -> &str {
        "vm_computer_use"
    }

    fn description(&self) -> &str {
        "Drive the Bot's per-Bot Linux VM (its own Chromium browser \
         and the XFCE desktop) via SSH + xdotool. The VM is the \
         default Computer Use target in v3.2.0 — ego_browser on the \
         Mac is an opt-in fallback. \n\n\
         ---\n\
         The `script` parameter is a sequence of helper calls, one per line. \
         Each call is `helper(arg, arg, ...)`. Available helpers: \n\
         - screenshot() \n\
         - click_at(x, y) \n\
         - type(text) \n\
         - key(name) \n\
         - open_url(url) \n\n\
         ---\n\
         Example: \n\
         open_url(\"https://github.com/login\")\n\
         click_at(420, 180)\n\
         type(\"my-username\")\n\
         key(\"Tab\")\n\
         type(\"my-password\")\n\
         key(\"Return\")\n\
         screenshot()\n\n\
         ---\n\
         RULES: \n\
         - The post-action screenshot (PNG, base64) of the LAST \
         call is the result the model sees. Add an explicit \
         `screenshot()` call if you need to see intermediate state. \n\
         - `key(name)` accepts xdotool key names: \"Return\", \
         \"Tab\", \"Escape\", \"ctrl+l\", \"ctrl+shift+n\", etc. \n\
         - `click_at(x, y)` clicks button 1. There's no right-click; \
         if you need one, type a JS snippet via the browser's \
         address bar. \n\
         - `type(text)` types literally. There is no key repeat; \
         long text may take a few seconds. \n\
         - `open_url(url)` launches Chromium with `--no-sandbox`. \
         The first launch takes 5-10s; subsequent calls reuse the \
         running process. \n\
         - If the VM is unreachable (no computer row, no IP, \
         SSH auth fails), the tool returns a clear error and \
         the model can fall back to `ego_browser` if \
         `bot.computer_use == \"mac\"`. \n\n\
         ---\n\
         Per-call consent because the script can navigate, click, \
         and fill forms in the Bot's own browser."
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
                    "description": "Newline-separated helper calls. Each line is one call: `screenshot()`, `click_at(x, y)`, `type(text)`, `key(name)`, or `open_url(url)`. The final call's screenshot (or a trailing `screenshot()`) is the result."
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
        let script = require_string(&invocation.arguments, "script")?;
        if script.trim().is_empty() {
            return Err(ToolError::InvalidArguments("script is empty".to_string()));
        }
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the vm_computer_use action".to_string(),
            ));
        }
        let bot_id = invocation.bot_id.clone().ok_or_else(|| {
            ToolError::InvalidArguments(
                "vm_computer_use requires a bot context (no bot_id)".to_string(),
            )
        })?;

        // Resolve the SSH pool from the AppHandle. The chat
        // command leaves `app` as None; the bot executor
        // sets it before calling. The daemon path also sets
        // it because the daemon's `run_bot_once` reuses the
        // Tauri app handle. If `app` is None we surface a
        // clear error instead of panicking.
        let app = context.app.clone().ok_or_else(|| {
            ToolError::Execution(
                "vm_computer_use: no app handle (run from inside a Bot)".to_string(),
            )
        })?;
        let state: tauri::State<Arc<AppState>> = app.state();
        let pool = state.computer.ssh_pool();

        // Parse the script into a sequence of calls. Bail with
        // a clear error on the first unparseable line so the
        // model sees the exact column / char that broke.
        let calls = parse_script(&script)?;

        // Run each call in order, on the VM. The final
        // `screenshot()` (or the last call's automatic
        // screenshot) is what we return.
        let mut last_screenshot: Option<String> = None;
        let mut transcript: Vec<String> = Vec::new();
        for call in &calls {
            match call {
                Call::Screenshot => {
                    let png_b64 = take_screenshot(&pool, &bot_id).await?;
                    last_screenshot = Some(png_b64);
                    transcript.push("screenshot() — captured".to_string());
                }
                Call::ClickAt { x, y } => {
                    shell_xdotool(&pool, &bot_id, &["mousemove", "--", &x.to_string(), &y.to_string()]).await?;
                    shell_xdotool(&pool, &bot_id, &["click", "1"]).await?;
                    // Take a screenshot after every click —
                    // the model almost always wants the post-
                    // action state.
                    let png_b64 = take_screenshot(&pool, &bot_id).await?;
                    last_screenshot = Some(png_b64);
                    transcript.push(format!("click_at({x}, {y})"));
                }
                Call::Type(text) => {
                    // xdotool type wants the literal text.
                    // Pass via `xdotool type -- "..."` to
                    // survive `-` prefix.
                    shell_xdotool(&pool, &bot_id, &["type", "--", text]).await?;
                    let png_b64 = take_screenshot(&pool, &bot_id).await?;
                    last_screenshot = Some(png_b64);
                    transcript.push(format!("type(\"{}\") — {} chars", truncate_inline(text, 40), text.chars().count()));
                }
                Call::Key(name) => {
                    shell_xdotool(&pool, &bot_id, &["key", "--", name]).await?;
                    let png_b64 = take_screenshot(&pool, &bot_id).await?;
                    last_screenshot = Some(png_b64);
                    transcript.push(format!("key(\"{name}\")"));
                }
                Call::OpenUrl(url) => {
                    // `chromium-browser --no-sandbox <url>`. Use
                    // `&` to detach so the SSH call returns
                    // immediately (chromium's main process
                    // lives for the whole session).
                    // `xdg-open` would defer to the user's
                    // default browser — but the per-Bot VM's
                    // default isn't always chromium, so we
                    // call it explicitly.
                    let cmd = format!(
                        "DISPLAY={d} nohup chromium-browser --no-sandbox --new-window {url} >/dev/null 2>&1 &",
                        d = VM_DISPLAY,
                        url = shell_quote(url),
                    );
                    pool.vm_exec(&bot_id, &cmd).await.map_err(|e| {
                        ToolError::Execution(format!(
                            "open_url: vm_exec failed: {e}"
                        ))
                    })?;
                    // Give chromium a moment to actually
                    // paint the URL. A 1.5s sleep is enough
                    // for cached pages; cold first-launch
                    // takes 5-10s and the model can call
                    // `screenshot()` again on a later turn.
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let png_b64 = take_screenshot(&pool, &bot_id).await?;
                    last_screenshot = Some(png_b64);
                    transcript.push(format!("open_url(\"{}\")", truncate_inline(url, 60)));
                }
            }
        }

        // If the script had no screenshot call and no
        // post-action action, take one last screenshot. The
        // model will want to see something.
        if last_screenshot.is_none() {
            let png_b64 = take_screenshot(&pool, &bot_id).await?;
            last_screenshot = Some(png_b64);
        }

        // Build the result body. The post-action screenshot
        // is the headline; a transcript of executed calls
        // sits above it for human-readable provenance.
        let png_b64 = last_screenshot.unwrap_or_default();
        let header = transcript.join("\n");
        let body = if header.is_empty() {
            format!("[screenshot, base64 PNG, {} bytes]", png_b64.len())
        } else {
            format!(
                "executed:\n{header}\n\n[screenshot, base64 PNG, {} bytes]\n{png_b64}",
                png_b64.len()
            )
        };
        Ok(ToolResult::ok(truncate_for_model(&body, 32_000)))
    }
}

/// A single parsed helper call.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Screenshot,
    ClickAt { x: i32, y: i32 },
    Type(String),
    Key(String),
    OpenUrl(String),
}

/// Parse the script into a sequence of `Call`s. The grammar:
/// one helper invocation per line, lines separated by `\n` or
/// `\r\n`. Empty lines and lines starting with `#` (after
/// trimming) are comments and are skipped.
///
/// The parser is intentionally tiny: no escaping inside string
/// args (the LLM writes one-line helper calls), no nested
/// calls, no arithmetic. If a line doesn't match, the parser
/// returns a clear error with the line number and the bad
/// fragment.
fn parse_script(script: &str) -> Result<Vec<Call>, ToolError> {
    let mut out: Vec<Call> = Vec::new();
    for (idx, raw) in script.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Pull the helper name and the arg list. We look for
        // the FIRST '(' so multi-paren strings like
        // `type("(foo)")` parse correctly: the helper name
        // is everything before the first `(`.
        let Some(open) = line.find('(') else {
            return Err(ToolError::InvalidArguments(format!(
                "line {}: missing '(' in call: {line:?}",
                idx + 1
            )));
        };
        let name = line[..open].trim();
        // The closing `)` is the last char; if it isn't, the
        // arg list spans multiple lines (we don't support
        // that) or the call is malformed.
        let Some(close) = line.rfind(')') else {
            return Err(ToolError::InvalidArguments(format!(
                "line {}: missing ')' in call: {line:?}",
                idx + 1
            )));
        };
        if close != line.len() - 1 {
            return Err(ToolError::InvalidArguments(format!(
                "line {}: trailing chars after ')': {line:?}",
                idx + 1
            )));
        }
        let args_str = &line[open + 1..close];
        let call = match name {
            "screenshot" => {
                if !args_str.trim().is_empty() {
                    return Err(ToolError::InvalidArguments(format!(
                        "line {}: screenshot() takes no args, got {args_str:?}",
                        idx + 1
                    )));
                }
                Call::Screenshot
            }
            "click_at" => {
                // Two integer args, comma-separated.
                let parts: Vec<&str> = args_str.split(',').map(|s| s.trim()).collect();
                if parts.len() != 2 {
                    return Err(ToolError::InvalidArguments(format!(
                        "line {}: click_at(x, y) wants 2 args, got {}",
                        idx + 1,
                        parts.len()
                    )));
                }
                let x: i32 = parts[0].parse().map_err(|_| {
                    ToolError::InvalidArguments(format!(
                        "line {}: click_at x is not an integer: {:?}",
                        idx + 1,
                        parts[0]
                    ))
                })?;
                let y: i32 = parts[1].parse().map_err(|_| {
                    ToolError::InvalidArguments(format!(
                        "line {}: click_at y is not an integer: {:?}",
                        idx + 1,
                        parts[1]
                    ))
                })?;
                Call::ClickAt { x, y }
            }
            "type" => {
                let text = parse_string_arg(args_str.trim(), idx + 1, "type")?;
                Call::Type(text)
            }
            "key" => {
                let name = parse_string_arg(args_str.trim(), idx + 1, "key")?;
                if name.is_empty() {
                    return Err(ToolError::InvalidArguments(format!(
                        "line {}: key() requires a non-empty key name",
                        idx + 1
                    )));
                }
                Call::Key(name)
            }
            "open_url" => {
                let url = parse_string_arg(args_str.trim(), idx + 1, "open_url")?;
                if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")) {
                    return Err(ToolError::InvalidArguments(format!(
                        "line {}: open_url requires an http(s):// or file:// URL, got {url:?}",
                        idx + 1
                    )));
                }
                Call::OpenUrl(url)
            }
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "line {}: unknown helper {other:?} (expected screenshot, click_at, type, key, open_url)",
                    idx + 1
                )));
            }
        };
        out.push(call);
    }
    if out.is_empty() {
        return Err(ToolError::InvalidArguments(
            "script is empty (no helper calls)".to_string(),
        ));
    }
    Ok(out)
}

/// Parse a single string argument. The grammar accepts either
/// a JSON-style double-quoted string (`"hello \"world\""`) or
/// a bare token (`Return`, `https://...`). The bare form is
/// needed because the LLM writes `key(Return)` and
/// `open_url(https://example.com)` without quoting.
fn parse_string_arg(s: &str, line_no: usize, helper: &str) -> Result<String, ToolError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ToolError::InvalidArguments(format!(
            "line {line_no}: {helper}() requires a non-empty argument"
        )));
    }
    if s.starts_with('"') {
        if !s.ends_with('"') || s.len() < 2 {
            return Err(ToolError::InvalidArguments(format!(
                "line {line_no}: {helper}() quoted string is malformed: {s:?}"
            )));
        }
        // Strip the quotes and unescape \" and \\. No other
        // escapes — the LLM writes single-line strings so
        // the rare \n / \t use case doesn't matter.
        let inner = &s[1..s.len() - 1];
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some(other) => {
                        return Err(ToolError::InvalidArguments(format!(
                            "line {line_no}: {helper}() unknown escape: \\{other}"
                        )));
                    }
                    None => {
                        return Err(ToolError::InvalidArguments(format!(
                            "line {line_no}: {helper}() string ends with backslash"
                        )));
                    }
                }
            } else {
                out.push(c);
            }
        }
        Ok(out)
    } else {
        Ok(s.to_string())
    }
}

/// Shell-quote a string for inclusion in a single-quoted
/// argument. We pass URLs and small strings through here so
/// that special characters in the LLM's input can't break
/// out of the SSH command's quoting.
fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

/// Truncate a string for inline display in the transcript.
/// Keeps the tool result body compact when the LLM types a
/// long URL or a long string.
fn truncate_inline(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Take a screenshot of the VM's desktop. We invoke
/// `scrot -z <path>` (the `-z` is "silent" — no chime) via
/// the SSH pool, then read the PNG bytes back via
/// `vm_sftp_read`, base64-encode, and return. Errors from
/// either step bubble up as `ToolError::Execution` with the
/// underlying SSH error message.
async fn take_screenshot(
    pool: &Arc<crate::computer::ssh::SshPool>,
    bot_id: &str,
) -> Result<String, ToolError> {
    let cmd = format!(
        "DISPLAY={d} scrot -z {path} 2>/dev/null || (DISPLAY={d} import -window root {path} 2>/dev/null) || (DISPLAY={d} xwd -root -silent > {path} 2>/dev/null)",
        d = VM_DISPLAY,
        path = SCREENSHOT_PATH,
    );
    pool.vm_exec(bot_id, &cmd)
        .await
        .map_err(|e| ToolError::Execution(format!("screenshot: vm_exec failed: {e}")))?;
    let bytes = pool
        .vm_sftp_read(bot_id, SCREENSHOT_PATH)
        .await
        .map_err(|e| ToolError::Execution(format!("screenshot: sftp read failed: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes.as_bytes()))
}

/// Run `xdotool <args>` on the VM. We pin `DISPLAY=:1` so
/// xdotool targets the same X session the noVNC viewer (and
/// the chromium process) are on. The `--` separator protects
/// args that start with `-` (rare for `xdotool mousemove` /
/// `click`, but `key -- ctrl+l` would otherwise parse
/// `ctrl+l` as a flag).
async fn shell_xdotool(
    pool: &Arc<crate::computer::ssh::SshPool>,
    bot_id: &str,
    args: &[&str],
) -> Result<(), ToolError> {
    let arg_list = args
        .iter()
        .map(|a| shell_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let cmd = format!("DISPLAY={d} xdotool {arg_list}", d = VM_DISPLAY);
    let out = pool
        .vm_exec(bot_id, &cmd)
        .await
        .map_err(|e| ToolError::Execution(format!("xdotool: vm_exec failed: {e}")))?;
    if !out.success {
        return Err(ToolError::Execution(format!(
            "xdotool exited {:?}: {}",
            out.exit_code,
            out.stderr.trim()
        )));
    }
    Ok(())
}

/// v3.2.0 — `vm_browser_open` is the skill-replay shape for
/// "open a URL in the Bot's VM." The recorder
/// (`skills/recorder.rs::rewrite_step_for_skill`) recognizes
/// a `vm_computer_use` step whose script is a single
/// `open_url(...)` call and rewrites it to
/// `vm_browser_open(url)` — a thinner, more readable skill
/// primitive. This tool is the replay side of that
/// primitive: it dispatches back to `vm_computer_use` with
/// a one-line `open_url(url)` script, so the LLM and the
/// skill replay path see the same end-state (chromium
/// launched in the VM, screenshot returned).
///
/// The LLM can also call this tool directly when it just
/// wants to open a URL — it's a small convenience over
/// `vm_computer_use` for the one-shot "navigate and
/// snapshot" case. Per-call consent because the URL can
/// land on any page (form fills, downloads, etc).
pub struct VmBrowserOpenTool;

#[async_trait]
impl Tool for VmBrowserOpenTool {
    fn name(&self) -> &str {
        "vm_browser_open"
    }

    fn description(&self) -> &str {
        "Open a URL in the Bot's per-Bot Linux VM (its own \
         Chromium browser, xdotool, and scrot). The post-action \
         screenshot of the page is returned. \n\n\
         ---\n\
         Thin wrapper over `vm_computer_use` for the common \
         \"navigate to a URL and capture a screenshot\" case. \
         For click / type / key sequences, use `vm_computer_use` \
         directly. \n\n\
         ---\n\
         `url` must be an `http://` or `https://` URL \
         (`file://` is also accepted). The first chromium \
         launch in a session takes 5-10s; subsequent calls \
         reuse the running process. \n\n\
         ---\n\
         Per-call consent because the URL can land on any \
         page (form fills, downloads, account pages)."
    }

    fn requires_consent(&self) -> bool {
        true
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "An http:// or https:// URL to open in the VM's Chromium browser."
                }
            },
            "required": ["url"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // The `url` argument is a string. We forward to
        // `vm_computer_use` with a one-line `open_url`
        // script — the same dispatch the recorder's
        // rewrite produces. Validation lives in
        // `vm_computer_use::parse_script` (the URL must
        // start with http(s):// or file://); we let that
        // validator catch malformed input so the two tools
        // agree on what's a valid URL.
        let url = require_string(&invocation.arguments, "url")?;
        if !context.consent_granted {
            return Err(ToolError::Execution(
                "user denied the vm_browser_open action".to_string(),
            ));
        }
        // The executor sets `bot_id` on every invocation.
        // `vm_computer_use::execute` needs it; the tool
        // also reads it from `invocation.bot_id` to look
        // up the SSH pool.
        let bot_id = invocation.bot_id.clone().ok_or_else(|| {
            ToolError::InvalidArguments(
                "vm_browser_open requires a bot context (no bot_id)".to_string(),
            )
        })?;

        // Build a one-line script. We use the JSON-style
        // quoted form so a URL with a `"` in the path
        // (rare but possible) doesn't break the
        // `parse_script` tokenizer.
        let escaped_url = url.replace('\\', "\\\\").replace('"', "\\\"");
        let script = format!("open_url(\"{escaped_url}\")");

        // Dispatch back to `vm_computer_use` with the
        // same invocation shape. The user already
        // consented to this call; `vm_computer_use` will
        // check `consent_granted` again and short-circuit
        // to a clear error if the upstream consent flow
        // didn't grant this exact call. That's the
        // expected behavior: a chained dispatch should
        // never bypass consent.
        let inner = VmComputerUseTool;
        inner
            .execute(
                ToolInvocation {
                    name: "vm_computer_use".to_string(),
                    arguments: json!({ "script": script }),
                    id: invocation.id.clone(),
                    bot_id: Some(bot_id),
                },
                context,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_script_recognizes_all_helpers() {
        let s = r#"
            # comment line
            open_url("https://example.com")
            click_at(120, 340)
            type("hello world")
            key(Return)
            screenshot()
        "#;
        let calls = parse_script(s).expect("parses");
        assert_eq!(calls.len(), 5);
        assert_eq!(calls[0], Call::OpenUrl("https://example.com".to_string()));
        assert_eq!(calls[1], Call::ClickAt { x: 120, y: 340 });
        assert_eq!(calls[2], Call::Type("hello world".to_string()));
        assert_eq!(calls[3], Call::Key("Return".to_string()));
        assert_eq!(calls[4], Call::Screenshot);
    }

    #[test]
    fn parse_script_accepts_bare_token_for_key_and_open_url() {
        // LLM commonly writes key(Return) and
        // open_url(https://example.com) without quotes.
        let calls = parse_script("key(Tab)\nopen_url(https://example.com/path)").expect("parses");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0], Call::Key("Tab".to_string()));
        assert_eq!(calls[1], Call::OpenUrl("https://example.com/path".to_string()));
    }

    #[test]
    fn parse_script_rejects_unknown_helper() {
        let err = parse_script("foo()").unwrap_err();
        match err {
            ToolError::InvalidArguments(msg) => assert!(msg.contains("unknown helper")),
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[test]
    fn parse_script_rejects_click_at_wrong_arg_count() {
        let err = parse_script("click_at(1, 2, 3)").unwrap_err();
        match err {
            ToolError::InvalidArguments(msg) => {
                assert!(msg.contains("wants 2 args"), "{msg}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[test]
    fn parse_script_rejects_empty() {
        let err = parse_script("").unwrap_err();
        match err {
            ToolError::InvalidArguments(msg) => {
                assert!(msg.contains("empty"), "{msg}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[test]
    fn parse_script_rejects_non_url_for_open_url() {
        let err = parse_script(r#"open_url("not-a-url")"#).unwrap_err();
        match err {
            ToolError::InvalidArguments(msg) => {
                assert!(msg.contains("http"), "{msg}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
    }

    #[test]
    fn parse_string_arg_handles_escapes() {
        let s = parse_string_arg(r#""hello \"world\"""#, 1, "type").expect("parses");
        assert_eq!(s, "hello \"world\"");
        let s = parse_string_arg(r#""back\\slash""#, 1, "type").expect("parses");
        assert_eq!(s, "back\\slash");
    }

    #[test]
    fn shell_quote_wraps_in_single_quotes() {
        assert_eq!(shell_quote("hello"), "'hello'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn truncate_inline_keeps_short_strings_unchanged() {
        assert_eq!(truncate_inline("hello", 40), "hello");
        let s = "a".repeat(50);
        let out = truncate_inline(&s, 10);
        assert_eq!(out.chars().count(), 11); // 10 + ellipsis
    }

    #[tokio::test]
    async fn execute_rejects_empty_script() {
        let result = VmComputerUseTool
            .execute(
                ToolInvocation {
                    name: "vm_computer_use".to_string(),
                    arguments: json!({ "script": "  \n  " }),
                    id: "x".to_string(),
                    bot_id: Some("bot-1".to_string()),
                },
                ToolContext::default(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[tokio::test]
    async fn execute_requires_consent() {
        let result = VmComputerUseTool
            .execute(
                ToolInvocation {
                    name: "vm_computer_use".to_string(),
                    arguments: json!({ "script": "screenshot()" }),
                    id: "x".to_string(),
                    bot_id: Some("bot-1".to_string()),
                },
                ToolContext::default(),
            )
            .await;
        assert!(matches!(result, Err(ToolError::Execution(_))));
    }

    #[tokio::test]
    async fn execute_requires_bot_id() {
        let mut ctx = ToolContext::default();
        ctx.consent_granted = true;
        let result = VmComputerUseTool
            .execute(
                ToolInvocation {
                    name: "vm_computer_use".to_string(),
                    arguments: json!({ "script": "screenshot()" }),
                    id: "x".to_string(),
                    bot_id: None,
                },
                ctx,
            )
            .await;
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn description_mentions_every_helper() {
        // The model picks helpers from the description. A
        // regression that drops `key` or `open_url` would
        // silently disable a tool the model didn't know
        // about.
        let d = VmComputerUseTool.description();
        for helper in ["screenshot", "click_at", "type", "key", "open_url"] {
            assert!(d.contains(helper), "description missing helper: {helper}");
        }
        // And the headline — VM is the default.
        assert!(d.contains("default"), "must explain that VM is the default");
    }
}
