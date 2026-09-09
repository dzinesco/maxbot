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

impl StreamError {
    /// Map a wire-level `StreamError` to a short, user-facing
    /// string suitable for display in the chat UI's error block.
    /// The intent is to give the user a clear next step ("open
    /// Settings", "check your network") without exposing the raw
    /// status codes or internal protocol names. Anything we can't
    /// categorize lands on the generic catch-all.
    pub fn friendly_message(&self) -> &'static str {
        match self {
            StreamError::MissingApiKey => "API key not set — open Settings",
            StreamError::Protocol(_) => "The model returned an unexpected response",
            // 401 / 403 = auth/rejected; everything else in the 4xx
            // family falls under the same "your key is the problem"
            // hint, since the user can only act on it by rotating
            // the key.
            StreamError::Http { status, .. } if *status == 401 || *status == 403 => {
                "Authentication failed — check your API key"
            }
            // 4xx is the user's problem (bad request, model not
            // found, etc.). Keep the message generic — surfacing
            // every 4xx verbatim is what got us here in the first
            // place.
            StreamError::Http { status, .. } if (400..500).contains(status) => {
                "Authentication failed — check your API key"
            }
            StreamError::Http { status, .. } if (500..600).contains(status) => {
                "The provider is having trouble — try again"
            }
            StreamError::Http { .. } => "Something went wrong. Try again.",
            StreamError::Network(_) => "Connection lost — check your network",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::StreamError;

    #[test]
    fn missing_api_key_friendly() {
        assert_eq!(
            StreamError::MissingApiKey.friendly_message(),
            "API key not set — open Settings"
        );
    }

    #[test]
    fn protocol_error_friendly() {
        assert_eq!(
            StreamError::Protocol("empty SSE chunk".to_string()).friendly_message(),
            "The model returned an unexpected response"
        );
    }

    #[test]
    fn network_error_friendly() {
        assert_eq!(
            StreamError::Network("connection reset".to_string()).friendly_message(),
            "Connection lost — check your network"
        );
    }

    #[test]
    fn http_401_friendly() {
        assert_eq!(
            StreamError::Http {
                status: 401,
                body: "unauthorized".to_string(),
            }
            .friendly_message(),
            "Authentication failed — check your API key"
        );
    }

    #[test]
    fn http_500_friendly() {
        assert_eq!(
            StreamError::Http {
                status: 503,
                body: "unavailable".to_string(),
            }
            .friendly_message(),
            "The provider is having trouble — try again"
        );
    }
}
