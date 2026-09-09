//! `tts_speak` and `tts_stop` — text-to-speech via the built-in macOS
//! `say` command. `say` is at `/usr/bin/say` and ships with every macOS
//! install, so no setup is required. Both tools are no-consent because
//! they don't touch the filesystem, network, or any state outside the
//! user's own audio output — TTS is a benign "make noise" operation.
//!
//! The same `speak()` / `stop()` functions are also exposed as Tauri
//! commands so the chat UI can show a speaker button on each
//! assistant message without going through the model's tool call.

use std::process::Stdio;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::registry::require_str;
use super::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};

/// Default voice when the user hasn't picked one. Samantha is a
/// high-quality en-US voice that ships with every macOS install.
pub const DEFAULT_TTS_VOICE: &str = "Samantha";

/// Speak `text` aloud using the macOS `say` command. Returns the
/// resolved voice (after substituting the default when `voice` is
/// empty) and the truncated text. The returned `truncated_chars`
/// count lets the caller log how much was actually spoken.
///
/// Errors are returned as plain `String` so the function is usable
/// from both the tool and the direct Tauri-command path without
/// pulling the `ToolError` enum in.
pub async fn speak(
    text: &str,
    voice: Option<&str>,
    rate: Option<i32>,
) -> Result<TtsResult, String> {
    if text.is_empty() {
        return Err("text must not be empty".to_string());
    }
    // Truncate to keep the TTS call bounded — the model shouldn't
    // accidentally send a 100K-character essay to `say`.
    let (text, truncated) = if text.chars().count() > 32_000 {
        let mut t: String = text.chars().take(32_000).collect();
        t.push_str("… [truncated for TTS]");
        (t, text.chars().count() - 32_000)
    } else {
        (text.to_string(), 0)
    };
    let voice = voice
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_TTS_VOICE)
        .to_string();
    let resolved_voice = voice.clone();
    let rate = rate.map(|r| r.clamp(50, 400));
    build_say_command(&text, Some(&voice), rate)
        .spawn()
        .map_err(|e| format!("could not spawn `say`: {e}"))?;
    Ok(TtsResult {
        chars: text.chars().count(),
        voice: resolved_voice,
        truncated_chars: truncated,
    })
}

/// Kill any in-flight `say -v` process. Safe to call when nothing is
/// speaking — the function returns `false` in that case so the caller
/// can phrase the response accordingly.
pub async fn stop() -> Result<bool, String> {
    let mut cmd = Command::new("pkill");
    cmd.arg("-f").arg("say -v");
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let output = cmd
        .output()
        .await
        .map_err(|e| format!("could not spawn `pkill`: {e}"))?;
    Ok(output.status.success())
}

/// Result of a successful `speak()` call. Surfaced to the model in the
/// tool-result message and to the frontend via the Tauri command.
#[derive(Debug, Clone)]
pub struct TtsResult {
    pub chars: usize,
    pub voice: String,
    pub truncated_chars: usize,
}

pub struct TtsSpeakTool;

#[async_trait]
impl Tool for TtsSpeakTool {
    fn name(&self) -> &str {
        "tts_speak"
    }

    fn description(&self) -> &str {
        "Speak the given text aloud using the built-in macOS `say` command. \
         The audio plays through the user's current output device. `voice` is \
         optional — defaults to a sensible system voice when omitted. Use \
         `tts_stop` to interrupt any in-flight speech. List available voices \
         on the host with `say -v ?` in Terminal."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "text": {
                    "type": "string",
                    "description": "The text to speak. Up to ~32K characters; longer inputs are truncated."
                },
                "voice": {
                    "type": "string",
                    "description": "Optional voice name (e.g. 'Samantha', 'Daniel', 'Karen', 'Alex'). Falls back to the system default when omitted or unknown."
                },
                "rate": {
                    "type": "integer",
                    "description": "Optional speech rate in words per minute (default 175). Range 50..=400."
                }
            },
            "required": ["text"],
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let text = require_str(&invocation.arguments, "text")?.to_string();
        let voice = invocation
            .arguments
            .get("voice")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let rate = invocation
            .arguments
            .get("rate")
            .and_then(|v| v.as_i64())
            .map(|n| n as i32);
        let result =
            speak(&text, voice.as_deref(), rate).await.map_err(ToolError::Execution)?;
        let mut summary = format!("spoke {} characters using voice '{}'", result.chars, result.voice);
        if result.truncated_chars > 0 {
            summary.push_str(&format!(
                " (truncated {} chars before speaking)",
                result.truncated_chars
            ));
        }
        Ok(ToolResult::ok(summary))
    }
}

pub struct TtsStopTool;

#[async_trait]
impl Tool for TtsStopTool {
    fn name(&self) -> &str {
        "tts_stop"
    }

    fn description(&self) -> &str {
        "Stop any in-flight `say` process. Safe to call when nothing is \
         speaking (it just no-ops). Use this after `tts_speak` to interrupt \
         a long utterance."
    }

    fn requires_consent(&self) -> bool {
        false
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        _invocation: ToolInvocation,
        _context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let stopped = stop().await.map_err(ToolError::Execution)?;
        if stopped {
            Ok(ToolResult::ok("stopped in-flight speech".to_string()))
        } else {
            Ok(ToolResult::ok("no speech was in flight".to_string()))
        }
    }
}

/// Build the `say` command for a given text + voice + rate. Exposed
/// publicly so tests can assert the right argument layout without
/// having to spawn the process.
pub fn build_say_command(text: &str, voice: Option<&str>, rate: Option<i32>) -> Command {
    let mut cmd = Command::new("say");
    // We always pass `-v` so the resulting command line includes the
    // substring "say -v", which is what `stop()`'s `pkill -f` looks
    // for. When the user didn't pick a voice we substitute the
    // built-in default (`Samantha`), keeping `-v` consistent.
    let voice = voice.unwrap_or(DEFAULT_TTS_VOICE);
    cmd.arg("-v").arg(voice);
    if let Some(r) = rate {
        let clamped = r.clamp(50, 400);
        cmd.arg("-r").arg(clamped.to_string());
    }
    cmd.arg(text);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    // Detach the child from the Tauri app so closing the app (or the
    // tool's task) doesn't kill the speech. We accomplish this by
    // explicitly setting `kill_on_drop(false)`.
    cmd.kill_on_drop(false);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_voice_is_substituted_when_omitted() {
        let cmd = build_say_command("hello", None, None);
        let debug = format!("{cmd:?}");
        assert!(debug.contains("say"), "missing 'say' in {debug}");
        assert!(debug.contains("-v"), "missing -v flag in {debug}");
        assert!(
            debug.contains(DEFAULT_TTS_VOICE),
            "missing default voice in {debug}"
        );
        assert!(debug.contains("hello"));
    }

    #[test]
    fn explicit_voice_is_honored() {
        let cmd = build_say_command("hi", Some("Daniel"), None);
        let debug = format!("{cmd:?}");
        assert!(debug.contains("Daniel"), "voice not in {debug}");
        assert!(!debug.contains(DEFAULT_TTS_VOICE));
    }

    #[test]
    fn rate_is_clamped_to_400() {
        let cmd = build_say_command("fast", Some("Samantha"), Some(9999));
        let debug = format!("{cmd:?}");
        assert!(debug.contains("-r"));
        assert!(debug.contains("400"), "rate not clamped in {debug}");
    }

    #[test]
    fn rate_below_50_is_clamped_up() {
        let cmd = build_say_command("slow", Some("Samantha"), Some(1));
        let debug = format!("{cmd:?}");
        assert!(debug.contains("50"), "low rate not clamped in {debug}");
    }

    #[test]
    fn empty_text_rejected() {
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(speak("", None, None));
        assert!(result.is_err(), "empty text should be rejected");
        assert!(result.unwrap_err().contains("empty"));
    }

    #[test]
    fn speak_uses_default_voice_when_omitted() {
        // We can't actually let `say` run in tests, but we can confirm
        // that `speak()` validates input correctly and returns the
        // resolved voice in the result struct (without spawning).
        // Skipping the spawn test path: we just check that empty text
        // is rejected and a sane text is accepted. The spawn itself
        // is exercised manually via the smoke test.
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(speak("hello", Some(""), None));
        // Should attempt to spawn. We don't actually want it to play
        // in the test, so just confirm it returns Ok.
        // (If `say` is missing on the test host, this errors. That's
        //  fine — it still proves the validation passed.)
        match result {
            Ok(r) => {
                assert_eq!(r.voice, DEFAULT_TTS_VOICE);
                assert_eq!(r.chars, "hello".chars().count());
            }
            Err(e) => {
                // Allowed: `say` not present on test host.
                assert!(e.contains("say") || e.contains("not found"));
            }
        }
    }
}
