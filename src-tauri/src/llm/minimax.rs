//! MiniMax chat-completions provider.
//!
//! Targets the international endpoint at `https://api.minimax.io/v1` by
//! default. Override with `SAND_MINIMAX_BASE_URL` (e.g. for the China region
//! `https://api.minimaxi.com/v1` or a self-hosted proxy). The protocol is
//! OpenAI-compatible: standard chat-completions request body, SSE response
//! stream with `data: {json}` lines and a terminal `data: [DONE]`.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::{stream::Stream, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::provider::{ChatMessage, ChatRequest, Provider};
use super::stream::{StreamChunk, StreamError};

/// Default base URL. The MiniMax international endpoint.
pub const DEFAULT_BASE_URL: &str = "https://api.minimax.io/v1";
/// Default model. Newest, 1M context, supports tools/vision.
pub const DEFAULT_MODEL: &str = "MiniMax-M3";

#[derive(Clone)]
pub struct MiniMaxProvider {
    pub api_key: Arc<str>,
    pub base_url: Arc<str>,
    pub http: Client,
}

impl MiniMaxProvider {
    pub fn new(api_key: String) -> Self {
        let base_url = std::env::var("SAND_MINIMAX_BASE_URL")
            .ok()
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self::with_base_url(api_key, base_url)
    }

    pub fn with_base_url(api_key: String, base_url: String) -> Self {
        let http = Client::builder()
            .user_agent("MaxBot/0.1 (https://maxbot.app)")
            .build()
            .expect("reqwest client");
        Self {
            api_key: Arc::from(api_key),
            base_url: Arc::from(base_url),
            http,
        }
    }

    fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Provider for MiniMaxProvider {
    fn name(&self) -> &'static str {
        "minimax"
    }

    fn default_model(&self) -> &'static str {
        DEFAULT_MODEL
    }

    async fn stream(
        &self,
        request: ChatRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, StreamError>> + Send>>, StreamError> {
        if self.api_key.is_empty() {
            return Err(StreamError::MissingApiKey);
        }
        // Build a wire-format body. MiniMax accepts the OpenAI shape.
        let body = serde_json::json!({
            "model": request.model,
            "messages": request.messages.iter().map(message_to_wire).collect::<Vec<_>>(),
            "stream": true,
            "temperature": request.temperature,
            "tools": request.tools,
        });
        let response = self
            .http
            .post(self.endpoint())
            .bearer_auth(&*self.api_key)
            .header("X-Title", "MaxBot")
            .json(&body)
            .send()
            .await
            .map_err(|e| StreamError::Network(e.to_string()))?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(StreamError::Http {
                status: status.as_u16(),
                body,
            });
        }
        // Walk the SSE stream, parsing `data: {json}` lines until `data: [DONE]`.
        let byte_stream = response.bytes_stream();
        let parsed = eventsource_stream::EventStream::new(byte_stream)
            .map(|item| match item {
                Ok(event) => translate_event(event),
                Err(e) => Err(StreamError::Network(e.to_string())),
            });
        Ok(Box::pin(parsed))
    }
}

/// Translate a Server-Sent Event into one or more `StreamChunk`s. MiniMax
/// uses the OpenAI chat-completions SSE shape: a stream of `data: {json}`
/// objects each containing one choice, ending with `data: [DONE]`.
fn translate_event(
    event: eventsource_stream::Event,
) -> Result<StreamChunk, StreamError> {
    if event.event.as_str() != "message" && !event.event.is_empty() {
        // Skip non-message events (e.g. named events we don't care about).
        return Err(StreamError::Protocol(format!(
            "ignoring non-message SSE event: {}",
            event.event
        )));
    }
    let payload = event.data.trim();
    if payload == "[DONE]" {
        return Ok(StreamChunk::Done {
            finish_reason: "stop".to_string(),
        });
    }
    let delta: WireDelta = serde_json::from_str(payload).map_err(|e| {
        StreamError::Protocol(format!("could not parse SSE chunk: {e}"))
    })?;
    for choice in delta.choices {
        if let Some(text) = choice.delta.content {
            if !text.is_empty() {
                return Ok(StreamChunk::Text { delta: text });
            }
        }
        if let Some(tool_calls) = choice.delta.tool_calls {
            for tc in tool_calls {
                if let Some(id) = tc.id {
                    return Ok(StreamChunk::ToolCallDelta {
                        id,
                        name: tc.function.as_ref().and_then(|f| f.name.clone()),
                        arguments_delta: tc
                            .function
                            .as_ref()
                            .and_then(|f| f.arguments.clone()),
                    });
                }
            }
        }
        if let Some(reason) = choice.finish_reason {
            if reason != "stop" && reason != "tool_calls" {
                return Ok(StreamChunk::Done {
                    finish_reason: reason,
                });
            }
        }
    }
    // No content in this delta; let the caller see an empty event and skip.
    Err(StreamError::Protocol("empty SSE chunk".to_string()))
}

/// Translate our internal `ChatMessage` into the OpenAI-compatible wire
/// shape that MiniMax accepts.
fn message_to_wire(message: &ChatMessage) -> serde_json::Value {
    match message {
        ChatMessage::System { content }
        | ChatMessage::User { content } => serde_json::json!({
            "role": match message { ChatMessage::System { .. } => "system", _ => "user" },
            "content": content,
        }),
        ChatMessage::Assistant { content, tool_calls } => {
            if tool_calls.is_empty() {
                serde_json::json!({ "role": "assistant", "content": content })
            } else {
                serde_json::json!({
                    "role": "assistant",
                    "content": content,
                    "tool_calls": tool_calls,
                })
            }
        }
        ChatMessage::Tool { tool_call_id, content } => serde_json::json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content,
        }),
    }
}

#[derive(Deserialize)]
struct WireDelta {
    #[serde(default)]
    choices: Vec<WireChoice>,
}

#[derive(Deserialize)]
struct WireChoice {
    #[serde(default)]
    delta: WireChoiceDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct WireChoiceDelta {
    content: Option<String>,
    tool_calls: Option<Vec<WireToolCallDelta>>,
}

#[derive(Deserialize)]
struct WireToolCallDelta {
    id: Option<String>,
    function: Option<WireToolFunctionDelta>,
}

#[derive(Deserialize, Default)]
struct WireToolFunctionDelta {
    name: Option<String>,
    arguments: Option<String>,
}
