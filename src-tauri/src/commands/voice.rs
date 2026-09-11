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

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
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

/// v3.7.14 — Public wire shape returned by `audio_to_text`.
/// The renderer drops the inner `text` straight into the chat
/// as a user message. The struct exists (vs. a plain `String`)
/// so future STT metadata (language, duration, confidence)
/// can be added without breaking the IPC contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAITranscribeResponse {
    pub text: String,
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

/// v3.7.14 — Public command: take a base64-encoded audio blob
/// (the wire shape the new `VoiceToolbar.tsx` produces from
/// `MediaRecorder` in the chat header) and POST it to OpenAI
/// Whisper, returning the transcript wrapped in
/// `OpenAITranscribeResponse`.
///
/// The base64 path is preferred over the raw-bytes path for
/// the renderer because JSON is the only type Tauri can carry
/// across the IPC boundary — `Vec<u8>` requires the
/// `Array.from(uint8Array)` round-trip the existing
/// `transcribeAudio` wrapper does, which is a few extra
/// characters the new `audioToText` wrapper doesn't have to
/// write. The decode here is symmetric and cheap.
///
/// Errors come back as a plain `String` so the renderer can
/// show them in a toast without unwrapping a structured error
/// type. A missing API key is a separate, clear error: the
/// brief specifies the exact text the user sees.
#[tauri::command]
pub async fn audio_to_text(
    state: State<'_, AppState>,
    audio_base64: String,
    mime_type: String,
) -> Result<OpenAITranscribeResponse, String> {
    audio_to_text_impl(state.inner(), audio_base64, mime_type).await
}

/// Inner async body for `audio_to_text`. Factored out of the
/// `#[tauri::command]` wrapper so unit tests can hit it
/// without spinning up a Tauri `App`.
pub(crate) async fn audio_to_text_impl(
    state: &AppState,
    audio_base64: String,
    mime_type: String,
) -> Result<OpenAITranscribeResponse, String> {
    if audio_base64.is_empty() {
        return Err("STT failed: empty audio buffer".to_string());
    }
    let audio_bytes = B64
        .decode(audio_base64.as_bytes())
        .map_err(|e| format!("STT failed: base64 decode: {e}"))?;
    if audio_bytes.is_empty() {
        return Err("STT failed: empty audio buffer".to_string());
    }
    let db = state.db.clone();
    let settings = tokio::task::spawn_blocking(move || db.load_settings())
        .await
        .map_err(|e| format!("STT failed: settings load join: {e}"))?
        .map_err(|e| format!("STT failed: settings load: {e}"))?;
    let api_key = settings
        .openai_api_key
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            "STT failed: OpenAI API key not configured. \
             Open Settings → Voice → OpenAI API key."
                .to_string()
        })?
        .to_string();
    let transcript = stt::transcribe(&api_key, &audio_bytes, &mime_type)
        .await
        .map_err(|e| format!("STT failed: {e}"))?;
    Ok(OpenAITranscribeResponse { text: transcript })
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

    /// v3.7.14 — Test-only serializer. The OVERRIDE global
    /// is a single static; with multiple tests touching it
    /// (the existing `transcribe_audio_returns_text_for_valid_input`
    /// plus the new `audio_to_text_returns_wrapped_response`),
    /// `cargo test`'s default parallelism races them. Each
    /// test that touches OVERRIDE acquires this lock for its
    /// entire body so the set → call → reset sequence is
    /// atomic from the test runner's perspective.
    ///
    /// We use a `std::sync::Mutex<()>` rather than a
    /// `tokio::sync::Mutex` because `#[tokio::test]` runs on
    /// the current_thread runtime by default, so holding a
    /// sync lock across an `.await` doesn't deadlock.
    #[cfg(test)]
    pub(crate) static TEST_LOCK: std::sync::Mutex<()> =
        std::sync::Mutex::new(());

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
        // v3.7.14 — Serialize with the new TEST_LOCK so
        // this test's set → call → reset sequence is
        // atomic. The companion `audio_to_text_*` tests
        // also acquire this lock, so they can't race
        // against us.
        let _serial = stt::TEST_LOCK.lock().unwrap();
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

    // ---- v3.7.14 — audio_to_text (base64 in, struct out) ----
    //
    // The new `VoiceToolbar.tsx` is wired through a different
    // wire shape (base64 in, `OpenAITranscribeResponse` out).
    // The tests below lock down: (a) the round-trip decode +
    // wrap, (b) the user-facing empty-buffer error, and
    // (c) the user-facing "missing API key" error that names
    // the right Settings page so the user can find the field.

    /// v3.7.14 — Happy path: base64 in, struct out. The
    /// canned transcript from the STT stub is wrapped in
    /// `OpenAITranscribeResponse { text }` so the renderer
    /// can spread the field straight onto a user message.
    #[tokio::test]
    async fn audio_to_text_returns_wrapped_response() {
        // Serialize with the existing test (and any
        // other future OVERRIDE user) via TEST_LOCK.
        let _serial = stt::TEST_LOCK.lock().unwrap();
        let _ = env_logger::builder().is_test(true).try_init();
        let (state_arc, _dir) = fresh_state();
        *OVERRIDE.lock().unwrap() = Some(Ok("hello world".to_string()));
        let b64 = B64.encode(b"fake-audio-bytes");
        let result =
            audio_to_text_impl(&state_arc, b64, "audio/webm".to_string()).await;
        *OVERRIDE.lock().unwrap() = None;
        let resp = result.expect("audio_to_text should return the canned response");
        assert_eq!(resp.text, "hello world");
    }

    /// v3.7.14 — Empty base64 strings are rejected up
    /// front (before the STT round-trip) so the user gets a
    /// clear error instead of a 400 from Whisper.
    #[tokio::test]
    async fn audio_to_text_rejects_empty_input() {
        let _ = env_logger::builder().is_test(true).try_init();
        let (state_arc, _dir) = fresh_state();
        let result = audio_to_text_impl(&state_arc, String::new(), "audio/webm".to_string()).await;
        assert!(result.is_err(), "empty input must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("empty audio buffer"),
            "expected 'empty audio buffer' in error, got: {err}"
        );
    }

    /// v3.7.14 — A missing OpenAI key surfaces the
    /// user-facing error: the brief specifies the exact text
    /// (mentioning "OpenAI API key" + the Settings path) so
    /// the user can find the field. This test guards against
    /// the error text regressing to a generic "key missing".
    #[tokio::test]
    async fn audio_to_text_missing_api_key_surfaces_hint() {
        let _ = env_logger::builder().is_test(true).try_init();
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("voice.sqlite");
        let db = Database::open(&path).expect("open test db");
        // Deliberately leave openai_api_key unset on this
        // fresh DB so the production code path returns the
        // missing-key error.
        let computer = Arc::new(ComputerManager::new(&db.load_settings().expect("load default settings")));
        let state = Arc::new(AppState {
            db: Arc::new(db),
            mcp: McpRegistry::default(),
            bot_runs: Arc::new(BotRunRegistry::new()),
            computer,
            recorder: Arc::new(RecorderState::new()),
            llm_key_override: Arc::new(std::sync::RwLock::new(None)),
        });
        let b64 = B64.encode(b"fake-audio-bytes");
        let result = audio_to_text_impl(&state, b64, "audio/webm".to_string()).await;
        assert!(result.is_err(), "missing key must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("OpenAI API key"),
            "expected 'OpenAI API key' in error, got: {err}"
        );
        assert!(
            err.contains("Settings"),
            "expected 'Settings' in error, got: {err}"
        );
    }

    /// v3.7.14 — Garbage base64 (e.g. user-supplied data
    /// that isn't valid base64) is rejected with a clear
    /// decode error before any network call.
    #[tokio::test]
    async fn audio_to_text_rejects_invalid_base64() {
        let _ = env_logger::builder().is_test(true).try_init();
        let (state_arc, _dir) = fresh_state();
        let result = audio_to_text_impl(
            &state_arc,
            "not-valid-base64-!@#$".to_string(),
            "audio/webm".to_string(),
        )
        .await;
        assert!(result.is_err(), "invalid base64 must error");
        let err = result.unwrap_err();
        assert!(
            err.contains("base64"),
            "expected 'base64' in error, got: {err}"
        );
    }
}
