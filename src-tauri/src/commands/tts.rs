//! Direct TTS Tauri commands. The chat UI uses these for the speaker
//! button on each assistant message; the model uses the `tts_speak` /
//! `tts_stop` tools for in-conversation TTS. Both paths share the same
//! `tools::tts::speak` / `stop` helpers.

use serde::Serialize;
use tauri::State;

use crate::storage::Database;
use crate::tools::tts::{self, DEFAULT_TTS_VOICE};
use crate::AppState;

#[derive(Serialize, Clone)]
pub struct TtsSpeakResponse {
    pub chars: usize,
    pub voice: String,
    pub truncated_chars: usize,
}

#[tauri::command]
pub async fn tts_speak(
    state: State<'_, AppState>,
    text: String,
    voice: Option<String>,
    rate: Option<i32>,
) -> Result<TtsSpeakResponse, String> {
    // If the caller didn't pass an explicit voice, fall back to the
    // user's stored preference. The model's `tts_speak` tool is the
    // same way (it reads settings); the Tauri command can do it
    // directly because it has `AppState`.
    let voice = match voice {
        Some(v) if !v.trim().is_empty() => v,
        _ => {
            let db = state.db.clone();
            let settings = tokio::task::spawn_blocking(move || db.load_settings())
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            if settings.tts_voice.trim().is_empty() {
                DEFAULT_TTS_VOICE.to_string()
            } else {
                settings.tts_voice
            }
        }
    };
    let result = tts::speak(&text, Some(&voice), rate)
        .await
        .map_err(|e| e.to_string())?;
    Ok(TtsSpeakResponse {
        chars: result.chars,
        voice: result.voice,
        truncated_chars: result.truncated_chars,
    })
}

#[tauri::command]
pub async fn tts_stop() -> Result<bool, String> {
    tts::stop().await.map_err(|e| e.to_string())
}
