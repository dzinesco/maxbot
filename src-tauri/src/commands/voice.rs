//! v2.7.0 — Voice (bidirectional) command surface.
//!
//! Today this module is exactly one command: `transcribe_audio`,
//! the Speech-to-Text half of the voice loop. The Text-to-Speech
//! half (`tts_speak` / `tts_stop`) lives in `commands::tts` — the
//! existing TTS module already had the right shape, so v2.7 just
//! wires the UI to it instead of duplicating it.
//!
//! ## STT provider
//!
//! OpenAI Whisper via `https://api.openai.com/v1/audio/transcriptions`.
//! The API key comes from `Settings.openai_api_key`; we don't add
//! a new `voice_stt_api_key` field because every Tyler-style
//! install that uses the OpenAI provider for chat has the same
//! key. If a future v2.x release wants a separate key, the right
//! path is `add_column_if_missing("settings", "voice_stt_api_key",
//! "TEXT")` plus a new `Settings` field — both additive.
//!
//! ## Request shape
//!
//! The MediaRecorder API in the webview produces `audio/webm`
//! (Opus) blobs by default. Whisper accepts `webm` directly, so we
//! pass the raw bytes through as a multipart `file` field. The
//! `model` field is set to `whisper-1` (the only Whisper model
//! OpenAI exposes for the transcriptions endpoint today).
//!
//! ## Testing
//!
//! `transcribe_audio` calls a thin `stt::transcribe` helper that
//! is `#[cfg(test)]`-swappable. The test in this module stubs the
//! helper to return a canned transcript, then asserts the public
//! command's wire shape (the returned string is the transcript,
//! not the raw Whisper JSON).

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::AppState;

/// Wrapper struct returned by the STT provider. Whisper's actual
/// response is `{"text": "..."}`. We deserialize into this and
/// return the inner string so the React side can drop the result
/// straight into the Composer without a JSON parse.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WhisperResponse {
    text: String,
}

/// Public command: take raw audio bytes + their MIME type
/// (typically `audio/webm` from the browser's MediaRecorder),
/// POST them to OpenAI Whisper, return the transcript.
///
/// Errors come back as a plain `String` so the renderer can show
/// them in a toast without unwrapping a structured error type.
#[tauri::command]
pub async fn transcribe_audio(
    state: State<'_, AppState>,
    audio_bytes: Vec<u8>,
    mime_type: String,
) -> Result<String, String> {
    transcribe_audio_impl(state.inner(), audio_bytes, mime_type).await
}

/// Inner async body, factored out of the `#[tauri::command]`
/// wrapper so unit tests can hit it without spinning up a Tauri
/// `App` (which would require the `tauri/test` feature and a
/// mock app handle). The wrapper is a single line; the body is
/// here so tests can call it directly with a real `&AppState`.
pub(crate) async fn transcribe_audio_impl(
    state: &AppState,
    audio_bytes: Vec<u8>,
    mime_type: String,
) -> Result<String, String> {
    if audio_bytes.is_empty() {
        return Err("STT failed: empty audio buffer".to_string());
    }
    let db = state.db.clone();
    // The settings blob lives in SQLite; load_settings takes the
    // DB lock, so we push it onto a blocking thread to keep the
    // tokio runtime happy.
    let settings = tokio::task::spawn_blocking(move || db.load_settings())
        .await
        .map_err(|e| format!("STT failed: settings load join: {e}"))?
        .map_err(|e| format!("STT failed: settings load: {e}"))?;
    let api_key = settings
        .openai_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "STT failed: OpenAI API key not configured".to_string())?
        .to_string();
    let transcript = stt::transcribe(&api_key, &audio_bytes, &mime_type)
        .await
        .map_err(|e| format!("STT failed: {e}"))?;
    Ok(transcript)
}

/// Thin HTTP wrapper so the test can swap it out. The production
/// path POSTs a multipart form to OpenAI's transcriptions endpoint
/// and pulls the `text` field out of the JSON response.
mod stt {
    use super::WhisperResponse;

    /// Test-only override. The unit test in this module sets
    /// this to a `Some` value to skip the real HTTP call.
    /// Always restored to `None` at the end of the test. In
    /// release builds the static is dead code; the compiler
    /// elides it.
    #[cfg(test)]
    pub(crate) static OVERRIDE: std::sync::Mutex<
        Option<Result<String, String>>,
    > = std::sync::Mutex::new(None);

    /// Production HTTP path. Uses `reqwest` (already a dependency)
    /// to send a multipart form to OpenAI. Returns a plain
    /// `String` error so the caller can prefix it with "STT failed:"
    /// without worrying about non-`std::error::Error` sources.
    ///
    /// In test builds, the override short-circuits the network
    /// call so unit tests run offline.
    pub(crate) async fn transcribe(
        api_key: &str,
        audio_bytes: &[u8],
        mime_type: &str,
    ) -> Result<String, String> {
        #[cfg(test)]
        {
            if let Some(forced) = OVERRIDE.lock().unwrap().clone() {
                // Sanity-check: production arguments should
                // still be passed through. A regression where
                // the command stops forwarding the audio buffer
                // would surface here.
                assert!(!audio_bytes.is_empty());
                assert!(!mime_type.is_empty());
                assert!(!api_key.is_empty());
                return forced;
            }
        }
        let part = reqwest::multipart::Part::bytes(audio_bytes.to_vec())
            .file_name("audio.webm")
            .mime_str(mime_type)
            .map_err(|e| format!("mime: {e}"))?;
        let form = reqwest::multipart::Form::new()
            .text("model", "whisper-1")
            .text("response_format", "json")
            .part("file", part);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .map_err(|e| format!("client build: {e}"))?;
        let resp = client
            .post("https://api.openai.com/v1/audio/transcriptions")
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| format!("network: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("provider returned {status}: {body}"));
        }
        let parsed: WhisperResponse = resp
            .json()
            .await
            .map_err(|e| format!("response parse: {e}"))?;
        Ok(parsed.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::registry::BotRunRegistry;
    use crate::computer::ComputerManager;
    use crate::mcp::McpRegistry;
    use crate::skills::recorder::RecorderState;
    use crate::storage::Database;
    use std::sync::Arc;
    use stt::OVERRIDE;
    use tempfile::TempDir;

    /// Build a minimal `AppState` for the voice tests. Mirrors
    /// `approvals::queue::tests::fresh_state` so the test fixtures
    /// stay consistent across modules.
    fn fresh_state() -> (Arc<AppState>, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("voice.sqlite");
        let db = Database::open(&path).expect("open test db");
        // Seed an OpenAI key so the production code path doesn't
        // bail on "API key not configured" before the stub fires.
        let mut settings = db.load_settings().expect("load default settings");
        settings.openai_api_key = Some("sk-test".to_string());
        db.save_settings(&settings).expect("save seeded settings");
        let computer = Arc::new(ComputerManager::new(&settings));
        let state = Arc::new(AppState {
            db: Arc::new(db),
            mcp: McpRegistry::default(),
            bot_runs: Arc::new(BotRunRegistry::new()),
            computer,
            recorder: Arc::new(RecorderState::new()),
            llm_key_override: Arc::new(std::sync::RwLock::new(None)),
        });
        (state, dir)
    }

    /// The transcribe_audio command must return the transcript
    /// string verbatim when the upstream STT call succeeds. We
    /// stub the network call via the `OVERRIDE` mutex so the test
    /// runs offline; the assertion is on the command's wire shape
    /// (a plain `String` transcript, not a JSON object), which is
    /// what the React side expects to drop into the Composer.
    #[tokio::test]
    async fn transcribe_audio_returns_text_for_valid_input() {
        let _ = env_logger::builder().is_test(true).try_init();
        let (state_arc, _dir) = fresh_state();
        // Force the stub to return a canned transcript.
        *OVERRIDE.lock().unwrap() = Some(Ok("hello world".to_string()));
        let result =
            transcribe_audio_impl(&state_arc, b"fake-audio-bytes".to_vec(), "audio/webm".to_string())
                .await;
        // Reset the override so we don't leak state into other
        // tests in the same process.
        *OVERRIDE.lock().unwrap() = None;
        let transcript =
            result.expect("transcribe_audio should return the canned transcript");
        assert_eq!(transcript, "hello world");
    }

    /// Empty audio buffers must be rejected before the network
    /// call so the user gets an immediate error instead of a
    /// 400 from Whisper.
    #[tokio::test]
    async fn transcribe_audio_rejects_empty_buffer() {
        let _ = env_logger::builder().is_test(true).try_init();
        let (state_arc, _dir) = fresh_state();
        let result = transcribe_audio_impl(&state_arc, Vec::new(), "audio/webm".to_string()).await;
        assert!(result.is_err(), "empty buffer must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("empty audio buffer"),
            "expected 'empty audio buffer' in error, got: {err}"
        );
    }
}
