//! Provider-agnostic chat request/response types and the Provider trait.
//!
//! The internal `ChatMessage` shape mirrors OpenAI chat completions so any
//! OpenAI-compatible backend (MiniMax, OpenAI, xAI, vLLM, OpenRouter, …) can
//! be adapted by translating the request/response at the provider boundary.
//! Anthropic uses a different on-the-wire shape (system-as-top-level,
//! `messages` API, content blocks); the Anthropic provider handles that
//! translation internally so callers always see the same `StreamChunk`s.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::Stream;
use serde::{Deserialize, Serialize};

use super::stream::{StreamChunk, StreamError};
use crate::storage::Settings;

/// Which LLM backend to talk to. Stored in `Settings::provider_kind` as a
/// lowercase string so adding a new provider doesn't require a DB migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// `https://api.minimax.io/v1` (international) — chat-completions wire.
    MiniMax,
    /// `https://api.openai.com/v1` — chat-completions wire.
    Openai,
    /// `https://api.anthropic.com` — Anthropic `messages` API.
    Anthropic,
    /// `https://api.x.ai/v1` — chat-completions wire, Grok models.
    Xai,
}

impl ProviderKind {
    // `ALL`, `as_str`, and `is_openai_compat` are part of the public
    // ProviderKind surface for future use (a Settings UI loop, a
    // provider-picker, a switch-on-kind in the chat command) but
    // none of the in-tree callers reach for them yet. The
    // `#[allow(dead_code)]` keeps the warning quiet without
    // shrinking the public surface — a future caller can pick
    // them up without re-adding them.
    #[allow(dead_code)]
    pub const ALL: &'static [ProviderKind] =
        &[ProviderKind::MiniMax, ProviderKind::Openai, ProviderKind::Anthropic, ProviderKind::Xai];

    #[allow(dead_code)]
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::MiniMax => "minimax",
            ProviderKind::Openai => "openai",
            ProviderKind::Anthropic => "anthropic",
            ProviderKind::Xai => "xai",
        }
    }

    /// Human-friendly display name for the Settings UI dropdown.
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderKind::MiniMax => "MiniMax (MiniMax-M3)",
            ProviderKind::Openai => "OpenAI (gpt-4o, o1, …)",
            ProviderKind::Anthropic => "Anthropic (Claude 3.5/3.7)",
            ProviderKind::Xai => "xAI (Grok)",
        }
    }

    /// Built-in default base URL. Empty for Anthropic since the path differs
    /// (no `/v1` segment — the messages endpoint is at the root).
    pub fn default_base_url(&self) -> &'static str {
        match self {
            ProviderKind::MiniMax => "https://api.minimax.io/v1",
            ProviderKind::Openai => "https://api.openai.com/v1",
            ProviderKind::Anthropic => "https://api.anthropic.com",
            ProviderKind::Xai => "https://api.x.ai/v1",
        }
    }

    /// Built-in default model id when the user hasn't picked one.
    pub fn default_model(&self) -> &'static str {
        match self {
            ProviderKind::MiniMax => "MiniMax-M3",
            ProviderKind::Openai => "gpt-4o",
            ProviderKind::Anthropic => "claude-3-5-sonnet-latest",
            ProviderKind::Xai => "grok-2-latest",
        }
    }

    /// "OpenAI-compat" providers share the chat-completions wire format and
    /// accept the same JSON request body. Anthropic is the odd one out and
    /// has its own translator.
    #[allow(dead_code)]
    pub fn is_openai_compat(&self) -> bool {
        matches!(self, ProviderKind::MiniMax | ProviderKind::Openai | ProviderKind::Xai)
    }

    pub fn from_settings(s: &Settings) -> Self {
        match s.provider_kind.as_str() {
            "openai" => ProviderKind::Openai,
            "anthropic" => ProviderKind::Anthropic,
            "xai" => ProviderKind::Xai,
            _ => ProviderKind::MiniMax,
        }
    }
}

/// Single role-tagged message in a conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum ChatMessage {
    /// System prompt. Always the first message when present.
    System { content: String },
    /// User message.
    User { content: String },
    /// Assistant message (model output).
    Assistant { content: String, #[serde(default)] tool_calls: Vec<ToolCall> },
    /// Tool result, returned to the model after it called a tool.
    Tool { tool_call_id: String, content: String },
}

impl ChatMessage {
    pub fn text(&self) -> Option<&str> {
        match self {
            ChatMessage::System { content }
            | ChatMessage::User { content }
            | ChatMessage::Assistant { content, .. } => Some(content.as_str()),
            ChatMessage::Tool { .. } => None,
        }
    }
}

/// Tool/function call. The shape mirrors OpenAI's tool_calls array so a tool
/// call arriving in a streamed assistant delta is straightforward to surface
/// to the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Tool/function declaration sent in the chat request. Matches OpenAI's
/// `tools` array. The Anthropic provider maps this onto Anthropic's
/// `{name, description, input_schema}` shape; OpenAI-compat providers
/// pass it through verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String, // "function"
    pub function: ToolFunctionSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    /// Model id (e.g. "MiniMax-M3", "gpt-4o", "claude-3-5-sonnet-latest").
    /// Provider-specific.
    pub model: String,
    /// Conversation, oldest first. The provider should not mutate this.
    pub messages: Vec<ChatMessage>,
    /// Tools the model is allowed to call. Empty for plain chat.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Sampling temperature. 1.0 for the built-in defaults.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

fn default_temperature() -> f32 {
    1.0
}

/// Final, non-streamed response. Most paths return a stream instead.
/// `#[allow(dead_code)]` keeps this in the public surface — the
/// streaming path always wins today, but a future "test echo" or
/// "non-streaming tool eval" path may want it.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

/// A streaming provider. `stream` returns one `StreamChunk` per token. The
/// provider is responsible for opening the connection, parsing the SSE wire
/// format, and translating provider-specific deltas into `StreamChunk`.
#[async_trait]
pub trait Provider: Send + Sync {
    // `name` and `kind` are part of the trait surface; every impl
    // fills them in. They aren't called from the in-tree call sites
    // (we use `ProviderKind::from_settings(&settings)` instead) but
    // keeping them on the trait means a future caller can switch
    // on a `dyn Provider` without downcasting. `#[allow(dead_code)]`
    // silences the warning without removing the trait methods.
    #[allow(dead_code)]
    fn name(&self) -> &'static str;
    #[allow(dead_code)]
    fn kind(&self) -> ProviderKind;
    fn default_model(&self) -> &'static str;
    async fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, StreamError>> + Send>>, StreamError>;
}

/// Build the right provider for the user's settings. Looks up the active
/// `provider_kind` and dispatches to the matching implementation, picking
/// the right API key and base URL.
///
/// Returns an error string suitable for surfacing to the UI.
pub fn provider_for_settings(
    settings: &Settings,
) -> Result<Arc<dyn Provider>, String> {
    let kind = ProviderKind::from_settings(settings);
    let base_url = effective_base_url(kind, settings);
    let model = effective_model(kind, settings);
    match kind {
        ProviderKind::MiniMax => {
            let key = require_key(settings, kind)?;
            Ok(Arc::new(
                super::minimax::MiniMaxProvider::with_base_url(
                    key,
                    base_url,
                ),
            ))
        }
        ProviderKind::Openai => {
            let key = require_key(settings, kind)?;
            Ok(Arc::new(super::openai::OpenAiProvider::with_base_url(
                key,
                base_url,
                model,
            )))
        }
        ProviderKind::Xai => {
            let key = require_key(settings, kind)?;
            Ok(Arc::new(super::xai::XaiProvider::with_base_url(
                key,
                base_url,
                model,
            )))
        }
        ProviderKind::Anthropic => {
            let key = require_key(settings, kind)?;
            Ok(Arc::new(
                super::anthropic::AnthropicProvider::with_base_url(
                    key,
                    base_url,
                ),
            ))
        }
    }
}

/// Pick the base URL for the active provider, honoring the per-provider
/// override field and falling back to the built-in default. Trims
/// whitespace and a trailing slash so the result can be safely
/// concatenated with endpoint paths.
pub fn effective_base_url(kind: ProviderKind, settings: &Settings) -> String {
    let override_field = match kind {
        ProviderKind::MiniMax => &settings.minimax_base_url,
        ProviderKind::Openai => &settings.openai_base_url,
        ProviderKind::Anthropic => &settings.anthropic_base_url,
        ProviderKind::Xai => &settings.xai_base_url,
    };
    let raw = if override_field.trim().is_empty() {
        kind.default_base_url().to_string()
    } else {
        override_field.trim().to_string()
    };
    raw.trim_end_matches('/').to_string()
}

/// Backwards-compat helper for callers that still hold a `base_url`
/// field. New code should read `settings.minimax_base_url` directly.
#[allow(dead_code)]
pub fn legacy_minimax_base_url(settings: &Settings) -> &str {
    &settings.minimax_base_url
}

/// Pick the model for the active provider: the user's explicit choice if
/// set, otherwise the provider's default.
pub fn effective_model(kind: ProviderKind, settings: &Settings) -> String {
    if !settings.default_model.trim().is_empty() {
        return settings.default_model.trim().to_string();
    }
    kind.default_model().to_string()
}

fn require_key(settings: &Settings, kind: ProviderKind) -> Result<String, String> {
    let raw = match kind {
        ProviderKind::MiniMax => &settings.minimax_api_key,
        ProviderKind::Openai => &settings.openai_api_key,
        ProviderKind::Anthropic => &settings.anthropic_api_key,
        ProviderKind::Xai => &settings.xai_api_key,
    };
    match raw {
        Some(k) if !k.is_empty() => Ok(k.clone()),
        _ => Err(format!(
            "set your {} API key in Settings first",
            kind.display_name()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_settings() -> Settings {
        Settings::default()
    }

    #[test]
    fn parses_provider_kind_strings() {
        assert_eq!(ProviderKind::from_settings(&Settings {
            provider_kind: "openai".into(),
            ..empty_settings()
        }), ProviderKind::Openai);
        assert_eq!(ProviderKind::from_settings(&Settings {
            provider_kind: "anthropic".into(),
            ..empty_settings()
        }), ProviderKind::Anthropic);
        assert_eq!(ProviderKind::from_settings(&Settings {
            provider_kind: "xai".into(),
            ..empty_settings()
        }), ProviderKind::Xai);
        // Unknown / empty / "minimax" → MiniMax
        assert_eq!(ProviderKind::from_settings(&Settings {
            provider_kind: "garbage".into(),
            ..empty_settings()
        }), ProviderKind::MiniMax);
        assert_eq!(ProviderKind::from_settings(&empty_settings()), ProviderKind::MiniMax);
    }

    #[test]
    fn effective_base_url_falls_back_to_default() {
        let s = empty_settings();
        assert_eq!(
            effective_base_url(ProviderKind::MiniMax, &s),
            "https://api.minimax.io/v1"
        );
        assert_eq!(
            effective_base_url(ProviderKind::Openai, &s),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            effective_base_url(ProviderKind::Anthropic, &s),
            "https://api.anthropic.com"
        );
        assert_eq!(
            effective_base_url(ProviderKind::Xai, &s),
            "https://api.x.ai/v1"
        );
    }

    #[test]
    fn effective_base_url_uses_override() {
        let s = Settings {
            openai_base_url: "https://api.openai.com/v1/".into(),
            ..empty_settings()
        };
        assert_eq!(
            effective_base_url(ProviderKind::Openai, &s),
            "https://api.openai.com/v1"
        );
    }

    #[test]
    fn effective_model_uses_default_when_empty() {
        let s = empty_settings();
        assert_eq!(effective_model(ProviderKind::Openai, &s), "gpt-4o");
        assert_eq!(
            effective_model(ProviderKind::Anthropic, &s),
            "claude-3-5-sonnet-latest"
        );
        assert_eq!(effective_model(ProviderKind::Xai, &s), "grok-2-latest");
    }

    #[test]
    fn effective_model_honors_user_choice() {
        let s = Settings {
            default_model: "o1-preview".into(),
            ..empty_settings()
        };
        assert_eq!(effective_model(ProviderKind::Openai, &s), "o1-preview");
    }

    #[test]
    fn provider_for_settings_requires_key() {
        let s = empty_settings();
        let err = match provider_for_settings(&s) {
            Ok(_) => panic!("MiniMax without key should fail"),
            Err(e) => e,
        };
        assert!(err.contains("MiniMax"), "got error: {err}");
        // Switching to openai without a key should also error and mention OpenAI
        let s = Settings {
            provider_kind: "openai".into(),
            ..empty_settings()
        };
        let err = match provider_for_settings(&s) {
            Ok(_) => panic!("OpenAI without key should fail"),
            Err(e) => e,
        };
        assert!(err.contains("OpenAI"), "got error: {err}");
    }

    #[test]
    fn provider_for_settings_routes_correctly() {
        let s = Settings {
            provider_kind: "openai".into(),
            openai_api_key: Some("sk-test".into()),
            default_model: "gpt-4o-mini".into(),
            ..empty_settings()
        };
        let p = provider_for_settings(&s).unwrap();
        assert_eq!(p.kind(), ProviderKind::Openai);
        assert_eq!(p.default_model(), "gpt-4o");
    }
}
