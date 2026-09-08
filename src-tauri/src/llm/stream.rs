//! Stream chunk types emitted to the renderer over Tauri events.

use serde::{Deserialize, Serialize};

/// A single delta from a streaming chat completion. Mirrors what the UI needs:
/// either a content token, a tool-call fragment, or a "done" marker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StreamChunk {
    /// Plain-text token to append to the assistant message.
    Text { delta: String },
    /// Tool/function call fragment. The provider may stream tool calls across
    /// many deltas; the renderer concatenates by `id`.
    ToolCallDelta {
        id: String,
        name: Option<String>,
        arguments_delta: Option<String>,
    },
    /// Final chunk. Carries the full stop reason.
    Done { finish_reason: String },
}

#[derive(Debug, thiserror::Error, Clone, Serialize, Deserialize)]
pub enum StreamError {
    #[error("missing API key — set it in Settings → Router")]
    MissingApiKey,
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("network: {0}")]
    Network(String),
    #[error("protocol: {0}")]
    Protocol(String),
}
