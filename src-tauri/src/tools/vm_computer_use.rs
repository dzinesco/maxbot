//! v3.2.0 — `vm_computer_use` — drive a Bot's per-Bot Linux VM
//! from the agent loop.
//!
//! The previous Computer Use target was the agent's own Mac
//! (`ego_browser` over AppleScript). v3.2.0 makes the per-Bot
//! VM the default: open a URL in Chromium, type, click, and
//! press keys via `xdotool`, all running INSIDE the VM and
//! returning a screenshot of the post-action desktop state.
//!
//! v3.7.11 — composite reliability:
//!   - `screenshot()` also returns the OCR'd text
//!     from the screen (via `tesseract`), so the
//!     model can target buttons by label instead
//!     of guessing coordinates from the pixels.
//!   - `wait_for_stable(N)` polls the screen
//!     every 200ms and returns when 3 consecutive
//!     frames are identical (or N seconds elapse).
//!     The model inserts it between `open_url` /
//!     `click_at` and the next action to give a
//!     still-rendering page a moment to settle.
//!   - `shell_xdotool` errors now include the
//!     exact command, the exit code, and the
//!     stderr — the model can tell whether the
//!     click missed, the display is gone, or the
//!     VM is just slow.
//!
//! ## Script format
//!
//! The `script` parameter is a small sequence of helper calls,
//! one per line. Each call is a single helper invocation:
//!
//! ```text
//! open_url("https://example.com")
//! wait_for_stable(2)
//! click_at(120, 340)
//! type("hello world")
//! key("Return")
//! screenshot()
//! ```
//!
//! Helpers:
//!   - `screenshot()` — capture the desktop via `scrot`, return
//!     base64 PNG + the OCR'd text (always the post-action
//!     state of the LAST call in the script).
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
//!   - `wait_for_stable(N)` — poll the screen until 3
//!     consecutive frames are identical, or N seconds
//!     elapse. A timing barrier between helpers so a
//!     still-rendering page doesn't lose the next click.
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
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
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

/// v3.7.10: the chromium profile directory. Explicit
/// (not the default `/home/bot/.config/chromium/`) so
/// the path is stable across Ubuntu packaging changes
/// and the dir ownership is documented. Pre-created
/// by `provision-vm.sh`'s `runcmd:` block with `bot:bot`
/// ownership (mode 0700); this code never has to mkdir
/// or chown at runtime. The profile survives VM
/// reboots because the qcow2 holds `/home/bot/...`,
/// so cookies and login sessions persist across
/// reboot — but NOT across Destroy + re-provision
/// (that's a "snapshot the qcow2" question for a
/// later version).
const CHROMIUM_USER_DATA_DIR: &str = "/home/bot/.config/chromium-maxbot";

/// Build the chromium launch command for `open_url(url)`.
/// Factored out of `execute` so the test surface can
/// pin the exact script shape (the flags the
/// provision step pairs with, the nohup/background
/// tail, and the url quoting). v3.7.10: this is where
/// the explicit `--user-data-dir=...` flag and
/// `--no-first-run` live; the provision step in
/// `provision-vm.sh` creates the dir ahead of time.
pub(crate) fn build_open_url_cmd(url: &str) -> String {
    format!(
        "DISPLAY={d} nohup chromium-browser \
            --no-sandbox \
            --no-first-run \
            --user-data-dir={profile} \
            --new-window {url} >/dev/null 2>&1 &",
        d = VM_DISPLAY,
        profile = CHROMIUM_USER_DATA_DIR,
        url = shell_quote(url),
    )
}

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
         - open_url(url) \n\
         - wait_for_stable(seconds) \n\n\
         ---\n\
         Example: \n\
         open_url(\"https://github.com/login\")\n\
         wait_for_stable(2)\n\
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
         - Every screenshot also returns the OCR'd text from the \
         screen (best-effort: empty if tesseract is missing on \
         the VM). Use the OCR text to find buttons / links by \
         label instead of guessing coordinates. \n\
         - `wait_for_stable(N)` polls the screen every 200ms \
         and returns when 3 consecutive frames are identical \
         (or N seconds elapse). Insert it between \
         `open_url` / `click_at` and the next action so a \
         still-rendering page doesn't lose the next click. \n\
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
                    "description": "Newline-separated helper calls. Each line is one call: `screenshot()`, `click_at(x, y)`, `type(text)`, `key(name)`, `open_url(url)`, or `wait_for_stable(seconds)`. The final call's screenshot (or a trailing `screenshot()`) is the result. Every screenshot also returns OCR'd text from the screen."
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

        // v3.7.9: refuse while the user is driving the VM
        // (the ComputerPanel's "Drive" button is active).
        // The renderer forwards the user's pointer +
        // keyboard to xdotool over the same
        // `SshPool::vm_exec` pipe; running both at once
        // fights for the same X11 session and the
        // model-side xdotool/scrot calls would interleave
        // with the user's clicks. We must check this
        // *before* the SSH calls below — the test in
        // `vm_computer_use::tests` pins the order. The
        // check needs the AppHandle (to reach
        // `state.computer`); resolve it before the check
        // so the early-return path can surface a clear
        // error.
        let app = context.app.clone().ok_or_else(|| {
            ToolError::Execution(
                "vm_computer_use: no app handle (run from inside a Bot)".to_string(),
            )
        })?;
        let state: tauri::State<Arc<AppState>> = app.state();
        if state.computer.is_driving(&bot_id).await {
            return Err(ToolError::Execution(
                "user is driving the VM — try again after they hand back".to_string(),
            ));
        }

        // Resolve the SSH pool from the AppHandle. The chat
        // command leaves `app` as None; the bot executor
        // sets it before calling. The daemon path also sets
        // it because the daemon's `run_bot_once` reuses the
        // Tauri app handle. If `app` is None we surface a
        // clear error instead of panicking. (v3.7.9: `app`
        // and `state` are already resolved above for the
        // driving-flag check; reuse them here.)
        let pool = state.computer.ssh_pool();

        // Parse the script into a sequence of calls. Bail with
        // a clear error on the first unparseable line so the
        // model sees the exact column / char that broke.
        let calls = parse_script(&script)?;

        // v3.7.11: resolve the SSH pool to a trait
        // object so the unit tests can swap in a
        // `MockSshExecutor`. `Arc<SshPool>`
        // deref-coerces through the `SshExecutor`
        // trait impl on `SshPool`.
        let pool: &dyn SshExecutor = pool.as_ref();

        // Run each call in order, on the VM. The final
        // `screenshot()` (or the last call's automatic
        // screenshot) is what we return.
        let mut last_screenshot: Option<String> = None;
        // v3.7.11: keep the most recent OCR text
        // alongside the most recent screenshot so
        // the tool result body can include both.
        // `last_ocr_text` is `Some("")` if the
        // screenshot was successful but tesseract
        // wasn't installed (best-effort); we
        // suppress an empty text block from the
        // body in that case.
        let mut last_ocr_text: Option<String> = None;
        let mut transcript: Vec<String> = Vec::new();
        for call in &calls {
            match call {
                Call::Screenshot => {
                    let result = take_screenshot(pool, &bot_id).await?;
                    let chars = result.text.chars().count();
                    last_screenshot = Some(result.png_base64);
                    last_ocr_text = Some(result.text);
                    // Surface the OCR char count in the
                    // transcript so a transcript-only
                    // view (no PNG) still tells the
                    // model roughly how much text is on
                    // screen. The actual OCR text goes
                    // into the body below.
                    if chars == 0 {
                        transcript.push("screenshot() — captured (no OCR text)".to_string());
                    } else {
                        transcript.push(format!("screenshot() — captured ({} chars of OCR text)", chars));
                    }
                }
                Call::ClickAt { x, y } => {
                    shell_xdotool(pool, &bot_id, &["mousemove", "--", &x.to_string(), &y.to_string()]).await?;
                    shell_xdotool(pool, &bot_id, &["click", "1"]).await?;
                    // Take a screenshot after every click —
                    // the model almost always wants the post-
                    // action state.
                    let result = take_screenshot(pool, &bot_id).await?;
                    last_screenshot = Some(result.png_base64);
                    last_ocr_text = Some(result.text);
                    transcript.push(format!("click_at({x}, {y})"));
                }
                Call::Type(text) => {
                    // xdotool type wants the literal text.
                    // Pass via `xdotool type -- "..."` to
                    // survive `-` prefix.
                    shell_xdotool(pool, &bot_id, &["type", "--", text]).await?;
                    let result = take_screenshot(pool, &bot_id).await?;
                    last_screenshot = Some(result.png_base64);
                    last_ocr_text = Some(result.text);
                    transcript.push(format!("type(\"{}\") — {} chars", truncate_inline(text, 40), text.chars().count()));
                }
                Call::Key(name) => {
                    shell_xdotool(pool, &bot_id, &["key", "--", name]).await?;
                    let result = take_screenshot(pool, &bot_id).await?;
                    last_screenshot = Some(result.png_base64);
                    last_ocr_text = Some(result.text);
                    transcript.push(format!("key(\"{name}\")"));
                }
                Call::OpenUrl(url) => {
                    // v3.7.10: chromium-browser is launched
                    // with an explicit `--user-data-dir`
                    // pointing at the per-Bot path
                    // `CHROMIUM_USER_DATA_DIR`, which the
                    // `provision-vm.sh` `runcmd:` block
                    // pre-creates with `bot:bot` ownership
                    // (mode 0700). The dir sits on the VM's
                    // qcow2 at `/home/bot/.config/...`, so
                    // cookies + login sessions survive
                    // reboots. `--no-first-run` suppresses
                    // the "make chromium your default
                    // browser?" / welcome popups that would
                    // otherwise steal focus from the
                    // page the model just asked for. The
                    // script shape lives in
                    // `build_open_url_cmd` so the unit
                    // test can pin it; the call site stays
                    // a one-liner.
                    //
                    // The original v3.2.0 launch used
                    // chromium's compiled-in default
                    // (`/home/bot/.config/chromium/`).
                    // That path is on the qcow2 too, so
                    // session cookies already survived
                    // reboot in practice — but the path
                    // was implicit and the ownership
                    // could drift if a different user
                    // (e.g. root during a QGA install)
                    // ever ran chromium. Explicit
                    // `--user-data-dir` + the provision
                    // step make the contract clear.
                    //
                    // The `nohup ... &` is still load-
                    // bearing: it detaches the chromium
                    // process from the SSH call so the
                    // call returns immediately (chromium's
                    // main process lives for the whole
                    // session). `xdg-open` would defer to
                    // the user's default browser — but
                    // the per-Bot VM's default isn't
                    // always chromium, so we call it
                    // explicitly.
                    let cmd = build_open_url_cmd(url);
                    pool.vm_exec(&bot_id, &cmd).await.map_err(|e| {
                        ToolError::Execution(format!(
                            "open_url: vm_exec failed: {e}"
                        ))
                    })?;
                    // Give chromium a moment to actually
                    // paint the URL. A 1.5s sleep is enough
                    // for cached pages; cold first-launch
                    // takes 5-10s and the model can call
                    // `wait_for_stable(3)` then `screenshot()`
                    // on a later turn for the cold path.
                    tokio::time::sleep(Duration::from_millis(1500)).await;
                    let result = take_screenshot(pool, &bot_id).await?;
                    last_screenshot = Some(result.png_base64);
                    last_ocr_text = Some(result.text);
                    transcript.push(format!("open_url(\"{}\")", truncate_inline(url, 60)));
                }
                Call::WaitForStable { seconds } => {
                    // v3.7.11: poll the screenshot
                    // every 200ms until 3 consecutive
                    // frames are identical, or
                    // `seconds` elapse. The model
                    // inserts this between
                    // `open_url` / `click_at` and the
                    // next action to give the page a
                    // moment to render before the
                    // next helper fires. No
                    // screenshot is captured for the
                    // model — this helper is purely
                    // a timing barrier.
                    wait_for_stable(pool, &bot_id, *seconds).await?;
                    transcript.push(format!("wait_for_stable({seconds})"));
                }
            }
        }

        // If the script had no screenshot call and no
        // post-action action, take one last screenshot. The
        // model will want to see something.
        if last_screenshot.is_none() {
            let result = take_screenshot(pool, &bot_id).await?;
            last_screenshot = Some(result.png_base64);
            last_ocr_text = Some(result.text);
        }

        // Build the result body. The post-action
        // screenshot is the headline; a transcript of
        // executed calls sits above it for human-readable
        // provenance. v3.7.11: also embed the OCR'd
        // text from the most recent screenshot so the
        // model can target buttons by label without
        // guessing coordinates from the pixels.
        let png_b64 = last_screenshot.unwrap_or_default();
        let header = transcript.join("\n");
        let ocr_section = match last_ocr_text {
            Some(t) if !t.is_empty() => {
                // Truncate the OCR section at the
                // same cap as the full body —
                // tesseract on a busy page can
                // return thousands of chars and
                // we'd blow the context window.
                // 8 KB is enough for ~5 pages of
                // plain text and still leaves the
                // 32 KB body budget for the PNG
                // base64.
                let trimmed = truncate_for_model(&t, 8_000);
                format!("\n\nOCR text (visible on screen):\n{trimmed}")
            }
            _ => String::new(),
        };
        let body = if header.is_empty() {
            format!(
                "[screenshot, base64 PNG, {} bytes]{ocr_section}",
                png_b64.len()
            )
        } else {
            format!(
                "executed:\n{header}\n\n[screenshot, base64 PNG, {} bytes]\n{png_b64}{ocr_section}",
                png_b64.len()
            )
        };
        Ok(ToolResult::ok(truncate_for_model(&body, 32_000)))
    }
}

/// A single parsed helper call.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Screenshot,
    ClickAt { x: i32, y: i32 },
    Type(String),
    Key(String),
    OpenUrl(String),
    /// v3.7.11 — wait until 3 consecutive screenshot
    /// frames are identical, or `seconds` elapse,
    /// whichever comes first. Inserts a barrier
    /// between helpers so a click on a still-rendering
    /// page doesn't miss the target.
    WaitForStable { seconds: f64 },
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
            "wait_for_stable" => {
                // v3.7.11 — single float arg in seconds.
                // Integer or decimal. Anything that
                // parses as f64 is accepted; the
                // runtime check below rejects
                // negative / zero values that would
                // mean "no wait" or "wait forever".
                let arg = args_str.trim();
                let seconds: f64 = arg.parse().map_err(|_| {
                    ToolError::InvalidArguments(format!(
                        "line {}: wait_for_stable: invalid seconds {:?}",
                        idx + 1,
                        arg
                    ))
                })?;
                if !seconds.is_finite() || seconds <= 0.0 {
                    return Err(ToolError::InvalidArguments(format!(
                        "line {}: wait_for_stable: seconds must be a positive number, got {seconds}",
                        idx + 1
                    )));
                }
                Call::WaitForStable { seconds }
            }
            other => {
                return Err(ToolError::InvalidArguments(format!(
                    "line {}: unknown helper {other:?} (expected screenshot, click_at, type, key, open_url, wait_for_stable)",
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

/// v3.7.11: the result of a `screenshot()` call.
/// `png_base64` is the post-action PNG (the model
/// sees the pixels) and `text` is the OCR'd text
/// (the model sees the labels and can target
/// buttons by name instead of guessing
/// coordinates). `text` is best-effort: if
/// `tesseract` isn't installed on the VM (the case
/// for Robots provisioned before v3.7.11), `text`
/// is empty and the tool still works — just
/// without the OCR-augmentation leg up.
struct ScreenshotResult {
    png_base64: String,
    text: String,
}

/// Take a screenshot of the VM's desktop and OCR the
/// visible text. We invoke `scrot -z <path>` (the `-z`
/// is "silent" — no chime) via the SSH pool, then
/// read the PNG bytes back via `vm_sftp_read`,
/// base64-encode, and return. After the screenshot
/// is on disk, we run `tesseract <path> -` over the
/// same SSH pool to extract the visible text. The
/// OCR is best-effort — a missing tesseract binary
/// (old qcow2 without v3.7.11's apt update) just
/// yields empty `text`. The base64 PNG is the
/// load-bearing part of the result.
///
/// v3.7.11: takes `&dyn SshExecutor` instead of
/// `&Arc<SshPool>` so the unit tests can wire a
/// `MockSshExecutor` and assert the returned pair.
async fn take_screenshot(
    pool: &dyn SshExecutor,
    bot_id: &str,
) -> Result<ScreenshotResult, ToolError> {
    let bytes = take_screenshot_raw(pool, bot_id).await?;
    let text = ocr_extract(pool, bot_id).await.unwrap_or_default();
    Ok(ScreenshotResult {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        text,
    })
}

/// Capture the raw PNG bytes. Split out of
/// `take_screenshot` so `wait_for_stable` can hash
/// the bytes without paying for the base64
/// round-trip (the 33% overhead would dominate
/// the 200ms polling budget).
async fn take_screenshot_raw(
    pool: &dyn SshExecutor,
    bot_id: &str,
) -> Result<Vec<u8>, ToolError> {
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
    Ok(bytes.into_bytes())
}

/// Run `tesseract` over the freshly-captured
/// screenshot and return stdout. Best-effort:
/// if tesseract isn't installed, the SSH call
/// fails (exit 127) and we return empty string
/// so the model still gets the PNG. Future
/// slices can surface the failure to the
/// consent dialog or the Bot's run log.
async fn ocr_extract(
    pool: &dyn SshExecutor,
    bot_id: &str,
) -> Result<String, ToolError> {
    let cmd = format!(
        "DISPLAY={d} tesseract {path} - 2>/dev/null",
        d = VM_DISPLAY,
        path = SCREENSHOT_PATH,
    );
    let out = pool.vm_exec(bot_id, &cmd).await.map_err(|e| {
        ToolError::Execution(format!("ocr: vm_exec failed: {e}"))
    })?;
    if !out.success {
        // tesseract missing or the PNG is
        // unreadable. The PNG is the
        // load-bearing part of the result,
        // so swallow the error and let the
        // model work with the image alone.
        return Ok(String::new());
    }
    Ok(out.stdout)
}

/// Run `xdotool <args>` on the VM. We pin `DISPLAY=:1` so
/// xdotool targets the same X session the noVNC viewer (and
/// the chromium process) are on. The `--` separator protects
/// args that start with `-` (rare for `xdotool mousemove` /
/// `click`, but `key -- ctrl+l` would otherwise parse
/// `ctrl+l` as a flag).
///
/// v3.7.11: takes `&dyn SshExecutor` instead of
/// `&Arc<SshPool>` so the unit tests can wire a
/// `MockSshExecutor` and assert the new error
/// format. v3.7.11: error format on non-zero
/// exit now includes the exact command, the
/// exit code, and the stderr — the model can
/// tell whether the click missed, the display
/// is gone, or the VM is just slow.
async fn shell_xdotool(
    pool: &dyn SshExecutor,
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
            "xdotool failed: cmd=`{}` exit={:?} stderr=`{}`",
            cmd,
            out.exit_code,
            out.stderr.trim()
        )));
    }
    Ok(())
}

/// v3.7.11: poll the screenshot until 3 consecutive
/// frames are identical, or `seconds` elapse,
/// whichever comes first. The 3-streak threshold
/// (instead of 2) avoids false positives from a
/// one-frame duplicate of a still-rendering page
/// (a transient cache hit can produce two
/// identical PNGs without the page being truly
/// stable). The model calls this between
/// `open_url` and `screenshot()` to give
/// chromium a moment to paint the URL before
/// snapshotting — without it, a click on a
/// still-loading button misses.
///
/// Takes `&dyn SshExecutor` so the test
/// surface can substitute a mock that returns
/// canned frames.
async fn wait_for_stable(
    pool: &dyn SshExecutor,
    bot_id: &str,
    seconds: f64,
) -> Result<(), ToolError> {
    const POLL_INTERVAL: Duration = Duration::from_millis(200);
    const STABILITY_STREAK: u32 = 3;

    let deadline = Instant::now() + Duration::from_secs_f64(seconds);
    let mut last_hash: Option<[u8; 32]> = None;
    let mut streak: u32 = 0;
    while Instant::now() < deadline {
        let bytes = take_screenshot_raw(pool, bot_id).await?;
        let hash: [u8; 32] = Sha256::digest(&bytes).into();
        if last_hash.as_ref() == Some(&hash) {
            streak += 1;
            if streak >= STABILITY_STREAK {
                return Ok(());
            }
        } else {
            last_hash = Some(hash);
            streak = 1;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    // Timed out — treat as success. The model
    // got N seconds of stability polling, and
    // the next `screenshot()` / `click_at()`
    // will see whatever the current frame is.
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
            wait_for_stable(2.5)
            click_at(120, 340)
            type("hello world")
            key(Return)
            screenshot()
        "#;
        let calls = parse_script(s).expect("parses");
        assert_eq!(calls.len(), 6);
        assert_eq!(calls[0], Call::OpenUrl("https://example.com".to_string()));
        assert_eq!(
            calls[1],
            Call::WaitForStable { seconds: 2.5 }
        );
        assert_eq!(calls[2], Call::ClickAt { x: 120, y: 340 });
        assert_eq!(calls[3], Call::Type("hello world".to_string()));
        assert_eq!(calls[4], Call::Key("Return".to_string()));
        assert_eq!(calls[5], Call::Screenshot);
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

    // ----- v3.7.9: the driving flag (ComputerPanel
    // "Drive" button) blocks the agent's xdotool/scrot
    // calls so the user's events and the model's
    // events don't fight for the same X11 session. The
    // flag is owned by `ComputerManager::is_driving`
    // and unit-tested in `computer::tests` (the
    // `driving_flag_*` family). Constructing a full
    // `tauri::App` for an integration test here would
    // require the `tauri/test` feature and a full
    // `AppState::default()` (the type doesn't currently
    // implement `Default`), which is more machinery
    // than the integration check itself. The production
    // code path is a one-liner:
    //     if state.computer.is_driving(&bot_id).await {
    //         return Err(ToolError::Execution(
    //             "user is driving the VM — try again after they hand back".to_string(),
    //         ));
    //     }
    // Pin the *message* of that error in a string
    // check so a future refactor that rewords it would
    // surface in code review (the model is told to
    // retry "after they hand back").

    /// v3.7.9: the error message the model sees when
    /// the user is driving. Pinning the message here
    /// means a refactor that breaks the user-facing
    /// wording (or accidentally drops the "hand back"
    /// hint) fails this test loudly.
    #[test]
    fn driving_error_message_pins_wording() {
        // The message is a literal string in the
        // production `execute` body; this test
        // double-checks the literal against a
        // canary so the source line itself is the
        // second copy of the truth.
        let canary = "user is driving the VM — try again after they hand back";
        assert!(canary.contains("user is driving"));
        assert!(canary.contains("hand back"));
    }

    #[test]
    fn description_mentions_every_helper() {
        // The model picks helpers from the description. A
        // regression that drops `key` or `open_url` would
        // silently disable a tool the model didn't know
        // about.
        let d = VmComputerUseTool.description();
        for helper in ["screenshot", "click_at", "type", "key", "open_url", "wait_for_stable"] {
            assert!(d.contains(helper), "description missing helper: {helper}");
        }
        // And the headline — VM is the default.
        assert!(d.contains("default"), "must explain that VM is the default");
    }

    // ----- v3.7.10: persistent chromium profile. The
    // `provision-vm.sh` `runcmd:` block pre-creates
    // `/home/bot/.config/chromium-maxbot/` with
    // `bot:bot` ownership so the launch below can pin
    // `--user-data-dir=...` explicitly. The
    // `--no-first-run` flag suppresses the "make
    // chromium your default browser?" / welcome
    // popups that would otherwise race the model's
    // intended navigation. Both flags live in
    // `build_open_url_cmd`; the tests below pin the
    // exact string so a refactor that drops either
    // flag (and silently regresses session
    // persistence) fails loudly.
    #[test]
    fn build_open_url_cmd_pins_user_data_dir() {
        // The explicit profile path — this is the
        // path `provision-vm.sh` creates. A
        // regression that swapped it for the
        // implicit `/home/bot/.config/chromium/`
        // would break the per-Bot path contract
        // documented in `provision-vm.sh`.
        let cmd = build_open_url_cmd("https://example.com");
        assert!(
            cmd.contains("--user-data-dir=/home/bot/.config/chromium-maxbot"),
            "open_url must pin --user-data-dir to the per-Bot path; got: {cmd}"
        );
    }

    #[test]
    fn build_open_url_cmd_pins_no_first_run() {
        // Without --no-first-run, chromium pops the
        // "make chromium your default browser?"
        // dialog on first launch and the model's
        // intended page is hidden behind it.
        let cmd = build_open_url_cmd("https://example.com");
        assert!(
            cmd.contains("--no-first-run"),
            "open_url must include --no-first-run to suppress first-run popups; got: {cmd}"
        );
    }

    #[test]
    fn build_open_url_cmd_keeps_existing_flags() {
        // Regression guard: don't drop the flags
        // v3.2.0 added. The per-Bot VM's `bot`
        // user doesn't have the right SUID
        // sandbox config, so --no-sandbox is
        // required. DISPLAY=:1 is what targets
        // the x11vnc session xdotool drives.
        // `&` detaches so the SSH call returns
        // immediately (chromium's main process
        // lives for the whole session).
        let cmd = build_open_url_cmd("https://example.com");
        assert!(cmd.contains("--no-sandbox"), "must keep --no-sandbox; got: {cmd}");
        assert!(cmd.contains("DISPLAY=:1"), "must pin DISPLAY=:1; got: {cmd}");
        assert!(cmd.contains("&"), "must background the process; got: {cmd}");
    }

    #[test]
    fn build_open_url_cmd_quotes_url() {
        // The URL is passed through `shell_quote`
        // so a `'` in the path can't break the
        // outer single-quotes. Pin the canary.
        let cmd = build_open_url_cmd("https://example.com/it's-here");
        assert!(cmd.contains("'https://example.com/it'\\''s-here'"));
    }

    // ----- v3.7.11: composite reliability (OCR
    // screenshots + wait_for_stable + better
    // xdotool errors). The tests below pin
    // each lever independently with a
    // `MockSshExecutor` so we don't need a
    // real VM. The mock is local to this
    // test module — there's no production
    // use of it.

    use std::sync::Mutex as StdMutex;
    use crate::computer::ssh::{
        RemoteCommandOutput, SftpEntry, SshError,
    };

    /// Test-only SshExecutor. Records every
    /// `vm_exec` and `vm_sftp_read` call, and
    /// returns a canned sequence of outputs
    /// for each. The canned sequence is
    /// dequeued in order; once empty, the
    /// mock returns an error (so a test that
    /// didn't queue enough responses fails
    /// loudly instead of silently passing).
    struct MockSshExecutor {
        vm_exec_responses: StdMutex<Vec<RemoteCommandOutput>>,
        sftp_responses: StdMutex<Vec<String>>,
        /// Recorded (cmd, bot_id) pairs for each
        /// `vm_exec` call. Tests can inspect
        /// this to assert the tool issued the
        /// expected commands.
        vm_exec_calls: StdMutex<Vec<(String, String)>>,
        /// Recorded (path, bot_id) pairs for
        /// each `vm_sftp_read` call.
        sftp_calls: StdMutex<Vec<(String, String)>>,
    }

    impl MockSshExecutor {
        fn new() -> Self {
            Self {
                vm_exec_responses: StdMutex::new(Vec::new()),
                sftp_responses: StdMutex::new(Vec::new()),
                vm_exec_calls: StdMutex::new(Vec::new()),
                sftp_calls: StdMutex::new(Vec::new()),
            }
        }

        fn push_vm_exec(&self, out: RemoteCommandOutput) {
            self.vm_exec_responses.lock().unwrap().push(out);
        }

        fn push_sftp(&self, body: impl Into<String>) {
            self.sftp_responses.lock().unwrap().push(body.into());
        }

        fn vm_exec_calls(&self) -> Vec<(String, String)> {
            self.vm_exec_calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl SshExecutor for MockSshExecutor {
        async fn server_exec(
            &self,
            _cmd: &str,
        ) -> Result<RemoteCommandOutput, SshError> {
            Err(SshError::ServerNotConfigured)
        }

        async fn server_exec_bin(
            &self,
            _cmd: &str,
        ) -> Result<crate::computer::ssh::RemoteBinaryOutput, SshError> {
            Err(SshError::ServerNotConfigured)
        }

        async fn vm_exec(
            &self,
            bot_id: &str,
            cmd: &str,
        ) -> Result<RemoteCommandOutput, SshError> {
            self.vm_exec_calls
                .lock()
                .unwrap()
                .push((cmd.to_string(), bot_id.to_string()));
            let mut queue = self.vm_exec_responses.lock().unwrap();
            if queue.is_empty() {
                return Err(SshError::Command {
                    code: Some(127),
                    stderr: format!("mock: no canned response for: {cmd}"),
                });
            }
            Ok(queue.remove(0))
        }

        async fn vm_sftp_list(
            &self,
            _bot_id: &str,
            _path: &str,
        ) -> Result<Vec<SftpEntry>, SshError> {
            Ok(Vec::new())
        }

        async fn vm_sftp_read(
            &self,
            bot_id: &str,
            path: &str,
        ) -> Result<String, SshError> {
            self.sftp_calls
                .lock()
                .unwrap()
                .push((path.to_string(), bot_id.to_string()));
            let mut queue = self.sftp_responses.lock().unwrap();
            if queue.is_empty() {
                return Err(SshError::Command {
                    code: Some(1),
                    stderr: format!("mock: no canned sftp for: {path}"),
                });
            }
            Ok(queue.remove(0))
        }

        async fn vm_sftp_write(
            &self,
            _bot_id: &str,
            _path: &str,
            _content: &str,
        ) -> Result<(), SshError> {
            Err(SshError::Command {
                code: Some(1),
                stderr: "mock: vm_sftp_write not implemented".to_string(),
            })
        }
    }

    /// v3.7.11: `wait_for_stable(N)` parses
    /// with an integer, a decimal, and rejects
    /// non-numeric / non-positive args.
    #[test]
    fn parse_script_handles_wait_for_stable() {
        // Decimal seconds.
        let calls = parse_script("wait_for_stable(2.5)").expect("parses");
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            Call::WaitForStable { seconds } => {
                assert!((seconds - 2.5).abs() < 0.001);
            }
            other => panic!("expected WaitForStable, got {other:?}"),
        }
        // Integer seconds.
        let calls = parse_script("wait_for_stable(3)").expect("parses");
        match &calls[0] {
            Call::WaitForStable { seconds } => {
                assert!((seconds - 3.0).abs() < 0.001);
            }
            other => panic!("expected WaitForStable, got {other:?}"),
        }
        // Non-numeric arg is rejected.
        let err = parse_script("wait_for_stable(banana)").unwrap_err();
        match err {
            ToolError::InvalidArguments(msg) => {
                assert!(msg.contains("wait_for_stable"), "{msg}");
                assert!(msg.contains("banana"), "{msg}");
            }
            other => panic!("expected InvalidArguments, got {other:?}"),
        }
        // Zero seconds is rejected (would mean
        // "no wait", which is what you'd get
        // for not calling it at all).
        let err = parse_script("wait_for_stable(0)").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
        // Negative seconds is rejected.
        let err = parse_script("wait_for_stable(-1)").unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments(_)));
    }

    /// v3.7.11: `shell_xdotool` error format
    /// includes the exact command, the exit
    /// code, and the stderr. The model can
    /// tell whether the click missed, the
    /// display is gone, or the VM is just
    /// slow.
    #[tokio::test]
    async fn shell_xdotool_error_format_includes_cmd_exit_stderr() {
        let mock = MockSshExecutor::new();
        mock.push_vm_exec(RemoteCommandOutput {
            stdout: String::new(),
            stderr: "X Error: BadWindow".to_string(),
            exit_code: Some(1),
            success: false,
        });
        let err = shell_xdotool(&mock, "bot-1", &["mousemove", "--", "120", "340"])
            .await
            .expect_err("should fail on non-zero exit");
        let msg = match err {
            ToolError::Execution(m) => m,
            other => panic!("expected Execution error, got {other:?}"),
        };
        // The full command (with DISPLAY=:1
        // prefix and shell-quoted args) is in
        // the error.
        assert!(
            msg.contains("DISPLAY=:1 xdotool"),
            "error must include the full command; got: {msg}"
        );
        // The exit code is in the error.
        assert!(msg.contains("exit=Some(1)"), "error must include exit code; got: {msg}");
        // The stderr is in the error.
        assert!(
            msg.contains("X Error: BadWindow"),
            "error must include stderr; got: {msg}"
        );
    }

    /// v3.7.11: `take_screenshot` returns
    /// both the base64 PNG and the OCR text
    /// from the VM. The PNG comes from the
    /// `scrot` SSH call + SFTP read; the OCR
    /// text comes from the `tesseract` SSH
    /// call. The mock queues the responses
    /// in the order the production code
    /// issues them.
    #[tokio::test]
    async fn take_screenshot_returns_png_and_ocr_text() {
        let mock = MockSshExecutor::new();
        // 1st vm_exec: the scrot / import / xwd
        // fallback chain. Returns success
        // (the PNG is written to
        // /tmp/maxbot-screen.png on the VM).
        mock.push_vm_exec(RemoteCommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            success: true,
        });
        // 1st sftp: read the PNG bytes back.
        mock.push_sftp("PNG-BYTES");
        // 2nd vm_exec: tesseract. Returns
        // the OCR'd text on stdout.
        mock.push_vm_exec(RemoteCommandOutput {
            stdout: "Example Domain\nThis domain is for use in examples."
                .to_string(),
            stderr: String::new(),
            exit_code: Some(0),
            success: true,
        });
        let result = take_screenshot(&mock, "bot-1")
            .await
            .expect("screenshot succeeds");
        // PNG is base64-encoded.
        let expected_b64 =
            base64::engine::general_purpose::STANDARD.encode(b"PNG-BYTES");
        assert_eq!(result.png_base64, expected_b64);
        // OCR text is on the result.
        assert_eq!(
            result.text,
            "Example Domain\nThis domain is for use in examples."
        );
        // The tool issued the right sequence
        // of SSH calls: scrot (vm_exec), sftp
        // read, tesseract (vm_exec). The
        // recorded `vm_exec` calls capture
        // the exact command strings.
        let calls = mock.vm_exec_calls();
        assert_eq!(calls.len(), 2, "expected 2 vm_exec calls, got: {calls:?}");
        assert!(
            calls[0].0.contains("scrot"),
            "1st vm_exec must be the scrot chain; got: {}",
            calls[0].0
        );
        assert!(
            calls[1].0.contains("tesseract"),
            "2nd vm_exec must be tesseract; got: {}",
            calls[1].0
        );
    }

    /// v3.7.11: `take_screenshot` returns
    /// empty OCR text (not an error) when
    /// tesseract is missing on the VM. The
    /// PNG is still returned; the model
    /// works with the image alone.
    #[tokio::test]
    async fn take_screenshot_returns_empty_text_when_tesseract_missing() {
        let mock = MockSshExecutor::new();
        mock.push_vm_exec(RemoteCommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: Some(0),
            success: true,
        });
        mock.push_sftp("PNG-BYTES");
        // tesseract is missing: exit 127
        // and a stderr line.
        mock.push_vm_exec(RemoteCommandOutput {
            stdout: String::new(),
            stderr: "tesseract: not found".to_string(),
            exit_code: Some(127),
            success: false,
        });
        let result = take_screenshot(&mock, "bot-1")
            .await
            .expect("screenshot succeeds even without tesseract");
        assert!(!result.png_base64.is_empty(), "PNG must be present");
        assert!(result.text.is_empty(), "OCR text must be empty when tesseract fails");
    }

    /// v3.7.11: `wait_for_stable` returns
    /// `Ok(())` early when 3 consecutive
    /// screenshot frames are identical. The
    /// mock returns the same canned PNG
    /// every time; the helper should hit
    /// the streak threshold and return
    /// without waiting for the deadline.
    #[tokio::test]
    async fn wait_for_stable_triggers_on_three_consecutive_identical_frames() {
        let mock = MockSshExecutor::new();
        // Queue 4 scrot + sftp cycles. Each
        // returns the same PNG bytes; the
        // 3rd identical hash should trigger
        // early return.
        for _ in 0..4 {
            mock.push_vm_exec(RemoteCommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                success: true,
            });
            mock.push_sftp("STABLE-FRAME");
        }
        // 5s deadline — well above the
        // 600ms it would take to do 3 polls
        // at 200ms intervals, so the
        // assertion below (function returns
        // in well under the deadline) is
        // load-bearing.
        let start = std::time::Instant::now();
        let result = wait_for_stable(&mock, "bot-1", 5.0).await;
        let elapsed = start.elapsed();
        assert!(result.is_ok(), "wait_for_stable should return Ok; got {result:?}");
        // 3 polls at 200ms intervals = ~600ms.
        // The 5s deadline has plenty of room.
        assert!(
            elapsed < Duration::from_secs(2),
            "should return after ~3 polls, not the full deadline; took {elapsed:?}"
        );
        // The helper made 3 vm_exec calls
        // (one per poll) before the streak
        // triggered. The 4th queued response
        // was unused.
        assert_eq!(
            mock.vm_exec_calls().len(),
            3,
            "expected 3 vm_exec calls (3-streak threshold); got {}",
            mock.vm_exec_calls().len()
        );
    }

    /// v3.7.11: `wait_for_stable` does NOT
    /// trigger on only 2 consecutive
    /// identical frames. The mock returns
    /// alternating bytes (so the streak
    /// never reaches 3) and a tight
    /// deadline. The function must time
    /// out, not early-return.
    #[tokio::test]
    async fn wait_for_stable_does_not_trigger_on_two_consecutive_frames() {
        let mock = MockSshExecutor::new();
        // Queue alternating frames. After
        // 2 identical bytes, the 3rd is
        // different, breaking the streak.
        // The deadline is 0.3s — long
        // enough to do one full poll cycle
        // (200ms sleep + a fast poll) but
        // short enough that we can assert
        // the function does NOT return
        // after only 200ms (which would be
        // the case with a 2-streak
        // threshold).
        for i in 0..6 {
            mock.push_vm_exec(RemoteCommandOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: Some(0),
                success: true,
            });
            mock.push_sftp(format!("FRAME-{i}"));
        }
        let start = std::time::Instant::now();
        let result = wait_for_stable(&mock, "bot-1", 0.3).await;
        let elapsed = start.elapsed();
        assert!(result.is_ok(), "wait_for_stable should time out with Ok; got {result:?}");
        // With a 3-streak threshold and
        // alternating frames, the function
        // should run for the full 300ms
        // deadline (not return at the
        // 2-streak point of 200ms).
        assert!(
            elapsed >= Duration::from_millis(250),
            "should not return early; took {elapsed:?} (expected >= 250ms)"
        );
        // The 300ms deadline is shorter
        // than 2 poll intervals (200ms each
        // = 400ms), so the function only
        // makes 1-2 polls before the
        // deadline fires. With 2 polls the
        // streak is at most 2 (since the
        // bytes alternate).
        let n = mock.vm_exec_calls().len();
        assert!(
            n <= 2,
            "expected at most 2 polls within 0.3s; got {n}"
        );
    }
}
