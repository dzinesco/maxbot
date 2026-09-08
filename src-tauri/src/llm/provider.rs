//! Provider-agnostic chat request/response types and the Provider trait.
//!
//! The shape is intentionally close to the OpenAI chat completions surface so
//! any OpenAI-compatible backend (MiniMax, OpenRouter, vLLM, etc.) can be
//! adapted by translating the request/response at the provider boundary.

use std::pin::Pin;

use async_trait::async_trait;
use futures_util::Stream;

use super::stream::{StreamChunk, StreamError};

/// Single role-tagged message in a conversation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Tool/function declaration sent in the chat request. Matches OpenAI's
/// `tools` array. The provider is expected to pass this through verbatim.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String, // "function"
    pub function: ToolFunctionSpec,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolFunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatRequest {
    /// Model id (e.g. "MiniMax-M3"). Provider-specific.
    pub model: String,
    /// Conversation, oldest first. The provider should not mutate this.
    pub messages: Vec<ChatMessage>,
    /// Tools the model is allowed to call. Empty for plain chat.
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
    /// Sampling temperature. 1.0 for MiniMax defaults.
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

fn default_temperature() -> f32 {
    1.0
}

/// Final, non-streamed response. Most paths return a stream instead.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

/// A streaming provider. `stream` returns one `StreamChunk` per token. The
/// provider is responsible for opening the connection, parsing the SSE wire
/// format, and translating provider-specific deltas into `StreamChunk`.
#[async_trait]
pub trait Provider: Send + Sync {
    fn name(&self) -> &'static str;
    fn default_model(&self) -> &'static str;
    async fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, StreamError>> + Send>>, StreamError>;
}
