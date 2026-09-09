//! Anthropic Claude provider.
//!
//! Targets `https://api.anthropic.com` by default. Hits the
//! `POST /v1/messages` endpoint with the Anthropic-native request body
//! (system-as-top-level, content blocks, `x-api-key` auth) and parses
//! Anthropic's SSE event stream (`message_start`, `content_block_*`,
//! `message_delta`, `message_stop`).
//!
//! Internal callers always see the same `StreamChunk`s they see from
//! the OpenAI-compat providers; the translation is contained here.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use eventsource_stream::Event;
use futures_util::{stream::Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;

use super::provider::{ChatMessage, ChatRequest, Provider, ProviderKind};
use super::stream::{StreamChunk, StreamError};

pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const DEFAULT_MODEL: &str = "claude-3-5-sonnet-latest";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Anthropic requires `max_tokens` on every request. Pick something
/// generous — Claude 3.5/3.7 supports up to 8K output by default and
/// 64K+ on specific models. 8K is enough for most chat + tool loops.
pub const DEFAULT_MAX_TOKENS: u32 = 8192;

#[derive(Clone)]
pub struct AnthropicProvider {
    pub api_key: Arc<str>,
    pub base_url: Arc<str>,
    pub http: Client,
}

impl AnthropicProvider {
    pub fn new(api_key: String) -> Self {
        Self::with_base_url(api_key, DEFAULT_BASE_URL.to_string())
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

    pub fn endpoint(&self) -> String {
        format!("{}/v1/messages", self.base_url.trim_end_matches('/'))
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    fn name(&self) -> &'static str {
        "anthropic"
    }

    fn kind(&self) -> ProviderKind {
        ProviderKind::Anthropic
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
        let body = build_request_body(&request);
        let response = self
            .http
            .post(self.endpoint())
            .header("x-api-key", self.api_key.as_ref())
            .header("anthropic-version", ANTHROPIC_VERSION)
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
        let byte_stream = response.bytes_stream();
        let parsed = eventsource_stream::EventStream::new(byte_stream)
            .map(|item| match item {
                Ok(event) => translate_event(event),
                Err(e) => Err(StreamError::Network(e.to_string())),
            });
        Ok(Box::pin(parsed))
    }
}

// ---- Request translation ---------------------------------------------------

/// Build the Anthropic-native request body from our internal `ChatRequest`.
/// - System messages are pulled out of the conversation and concatenated
///   into the top-level `system` field (Anthropic requires this).
/// - Assistant `tool_calls` become `tool_use` content blocks.
/// - Internal `Tool` messages become a `user` message containing
///   `tool_result` content blocks (Anthropic's wire shape).
pub fn build_request_body(request: &ChatRequest) -> serde_json::Value {
    let mut system_parts: Vec<String> = Vec::new();
    let mut messages: Vec<serde_json::Value> = Vec::new();

    for msg in &request.messages {
        match msg {
            ChatMessage::System { content } => {
                system_parts.push(content.clone());
            }
            ChatMessage::User { content } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "text", "text": content}],
                }));
            }
            ChatMessage::Assistant { content, tool_calls } => {
                let mut blocks: Vec<serde_json::Value> = Vec::new();
                if !content.is_empty() {
                    blocks.push(serde_json::json!({"type": "text", "text": content}));
                }
                for tc in tool_calls {
                    let input: serde_json::Value = serde_json::from_str(&tc.arguments)
                        .unwrap_or_else(|_| serde_json::Value::String(tc.arguments.clone()));
                    blocks.push(serde_json::json!({
                        "type": "tool_use",
                        "id": tc.id,
                        "name": tc.name,
                        "input": input,
                    }));
                }
                if blocks.is_empty() {
                    // Anthropic rejects empty assistant content — emit an
                    // empty text block so the conversation history stays
                    // well-formed.
                    blocks.push(serde_json::json!({"type": "text", "text": ""}));
                }
                messages.push(serde_json::json!({"role": "assistant", "content": blocks}));
            }
            ChatMessage::Tool { tool_call_id, content } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": content,
                    }],
                }));
            }
        }
    }

    let system = if system_parts.is_empty() {
        None
    } else {
        Some(system_parts.join("\n\n"))
    };

    let tools: Vec<serde_json::Value> = request
        .tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "name": t.function.name,
                "description": t.function.description,
                "input_schema": t.function.parameters,
            })
        })
        .collect();

    let mut body = serde_json::json!({
        "model": request.model,
        "messages": messages,
        "max_tokens": DEFAULT_MAX_TOKENS,
        "temperature": request.temperature,
        "stream": true,
    });
    if let Some(sys) = system {
        body["system"] = serde_json::Value::String(sys);
    }
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
    }
    body
}

// ---- SSE translation -------------------------------------------------------

/// Translate one Anthropic SSE event into one of our `StreamChunk`s.
/// Anthropic uses named events (`message_start`, `content_block_start`,
/// `content_block_delta`, `content_block_stop`, `message_delta`,
/// `message_stop`) rather than OpenAI's `data: {json}` with
/// `data: [DONE]`.
pub fn translate_event(event: Event) -> Result<StreamChunk, StreamError> {
    // eventsource_stream parses both `event:` and `data:` lines. The
    // `data` field carries the JSON payload for every Anthropic event
    // type. Skip events that aren't meaningful to us (ping, etc.).
    let payload = event.data.trim();
    if payload.is_empty() {
        return Err(StreamError::Protocol("empty SSE data".to_string()));
    }
    let parsed: AnthropicEvent = serde_json::from_str(payload).map_err(|e| {
        StreamError::Protocol(format!("anthropic: could not parse SSE: {e}"))
    })?;
    match parsed {
        AnthropicEvent::Ping => Err(StreamError::Protocol("ping".to_string())),
        AnthropicEvent::MessageStart { .. } => {
            // No content yet; just a header.
            Err(StreamError::Protocol("message_start".to_string()))
        }
        AnthropicEvent::ContentBlockStart {
            content_block: ContentBlock::ToolUse { id, name, .. },
            ..
        } => Ok(StreamChunk::ToolCallDelta {
            id,
            name: Some(name),
            arguments_delta: None,
        }),
        AnthropicEvent::ContentBlockStart { .. } => {
            // text block start — no payload to emit.
            Err(StreamError::Protocol("content_block_start".to_string()))
        }
        AnthropicEvent::ContentBlockDelta {
            delta: ContentDelta::TextDelta { text },
            ..
        } => {
            if text.is_empty() {
                Err(StreamError::Protocol("empty text_delta".to_string()))
            } else {
                Ok(StreamChunk::Text { delta: text })
            }
        }
        AnthropicEvent::ContentBlockDelta {
            delta: ContentDelta::InputJsonDelta { partial_json },
            ..
        } => Ok(StreamChunk::ToolCallDelta {
            // The renderer concatenates by `id`; we don't know the id
            // from input_json_delta alone, so the caller is expected to
            // associate the partial JSON with the id from the prior
            // content_block_start. We send a synthetic key here; the
            // actual accumulation happens in the chat/bot loop using
            // the by-id map keyed by the start event's id.
            //
            // We carry the partial JSON as `arguments_delta` with `id`
            // empty — the chat loop's `by_id` map will already have
            // an entry keyed by the tool_use id from the start event,
            // and appending to that entry by matching the most-recent
            // start is straightforward. To keep things simple we
            // special-case: emit a ToolCallDelta with id="" and
            // arguments_delta set; the chat loop treats id="" as
            // "append to the most recent open tool call".
            id: String::new(),
            name: None,
            arguments_delta: Some(partial_json),
        }),
        AnthropicEvent::ContentBlockDelta {
            delta: ContentDelta::Other,
            ..
        } => Err(StreamError::Protocol(
            "unknown content_block_delta".to_string(),
        )),
        AnthropicEvent::ContentBlockStop { .. } => {
            Err(StreamError::Protocol("content_block_stop".to_string()))
        }
        AnthropicEvent::MessageDelta { delta } => {
            // `message_delta` carries the stop_reason. We surface it as
            // a `Done` chunk; the chat loop breaks on Done. Anthropic
            // always emits `message_stop` after this; we let that close
            // the stream naturally without emitting a second Done.
            let reason = delta.stop_reason.unwrap_or_else(|| "end_turn".to_string());
            Ok(StreamChunk::Done { finish_reason: reason })
        }
        AnthropicEvent::MessageStop => {
            // Stream is over; we don't need to emit a chunk. The byte
            // stream will EOF after this, which the eventsource parser
            // surfaces as `None` and the chat loop handles.
            Err(StreamError::Protocol("message_stop".to_string()))
        }
        AnthropicEvent::Error { error } => {
            let body = format!("{}: {}", error.kind, error.message);
            Err(StreamError::Http { status: 400, body })
        }
        AnthropicEvent::Unknown => Err(StreamError::Protocol(
            "unknown anthropic event".to_string(),
        )),
    }
}

// ---- Wire types (private) --------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicEvent {
    MessageStart {
        #[serde(default)]
        message: serde_json::Value,
    },
    ContentBlockStart {
        #[serde(default)]
        index: u32,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        #[serde(default)]
        index: u32,
        delta: ContentDelta,
    },
    ContentBlockStop {
        #[serde(default)]
        index: u32,
    },
    MessageDelta {
        delta: MessageDeltaPayload,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicErrorBody,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
struct AnthropicErrorBody {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentBlock {
    Text { #[serde(default)] text: String },
    ToolUse {
        id: String,
        name: String,
        #[serde(default)]
        input: serde_json::Value,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentDelta {
    TextDelta { text: String },
    InputJsonDelta { partial_json: String },
    #[serde(other)]
    Other,
}

#[derive(Debug, Default, Deserialize)]
struct MessageDeltaPayload {
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    stop_sequence: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{ToolCall, ToolDefinition, ToolFunctionSpec};

    fn text_event(data: &str) -> Event {
        Event {
            event: String::new(),
            data: data.to_string(),
            id: String::new(),
            retry: None,
        }
    }

    #[test]
    fn build_request_pulls_out_system_and_tool_results() {
        let request = ChatRequest {
            model: "claude-3-5-sonnet-latest".into(),
            messages: vec![
                ChatMessage::System {
                    content: "be terse".into(),
                },
                ChatMessage::User {
                    content: "what's the weather?".into(),
                },
                ChatMessage::Assistant {
                    content: String::new(),
                    tool_calls: vec![ToolCall {
                        id: "toolu_1".into(),
                        name: "get_weather".into(),
                        arguments: r#"{"city":"SF"}"#.into(),
                    }],
                },
                ChatMessage::Tool {
                    tool_call_id: "toolu_1".into(),
                    content: "72F sunny".into(),
                },
            ],
            tools: vec![ToolDefinition {
                kind: "function".into(),
                function: ToolFunctionSpec {
                    name: "get_weather".into(),
                    description: "Look up the weather".into(),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {"city": {"type": "string"}},
                    }),
                },
            }],
            temperature: 1.0,
        };
        let body = build_request_body(&request);
        assert_eq!(body["system"], "be terse");
        assert_eq!(body["model"], "claude-3-5-sonnet-latest");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(
            body["messages"][0]["content"][0]["text"],
            "what's the weather?"
        );
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["messages"][1]["content"][0]["type"], "tool_use");
        assert_eq!(body["messages"][1]["content"][0]["id"], "toolu_1");
        assert_eq!(body["messages"][1]["content"][0]["name"], "get_weather");
        assert_eq!(body["messages"][1]["content"][0]["input"]["city"], "SF");
        // Tool result is wrapped in a user message with tool_result block.
        assert_eq!(body["messages"][2]["role"], "user");
        assert_eq!(body["messages"][2]["content"][0]["type"], "tool_result");
        assert_eq!(
            body["messages"][2]["content"][0]["tool_use_id"],
            "toolu_1"
        );
        assert_eq!(body["messages"][2]["content"][0]["content"], "72F sunny");
        // Tools translated to Anthropic's input_schema shape.
        assert_eq!(body["tools"][0]["name"], "get_weather");
        assert!(body["tools"][0].get("input_schema").is_some());
    }

    #[test]
    fn translate_text_delta_event() {
        let ev = text_event(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
        );
        match translate_event(ev).unwrap() {
            StreamChunk::Text { delta } => assert_eq!(delta, "Hello"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn translate_tool_use_start_event() {
        let ev = text_event(
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_x","name":"shell_run","input":{}}}"#,
        );
        match translate_event(ev).unwrap() {
            StreamChunk::ToolCallDelta { id, name, arguments_delta } => {
                assert_eq!(id, "toolu_x");
                assert_eq!(name.as_deref(), Some("shell_run"));
                assert!(arguments_delta.is_none());
            }
            other => panic!("expected tool delta, got {other:?}"),
        }
    }

    #[test]
    fn translate_input_json_delta_event() {
        let ev = text_event(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"ci"}}"#,
        );
        match translate_event(ev).unwrap() {
            StreamChunk::ToolCallDelta { id, name, arguments_delta } => {
                // id is empty — the chat loop appends to the open tool_use.
                assert!(id.is_empty());
                assert!(name.is_none());
                assert_eq!(arguments_delta.as_deref(), Some(r#"{"ci"#));
            }
            other => panic!("expected tool delta, got {other:?}"),
        }
    }

    #[test]
    fn translate_message_delta_done() {
        let ev = text_event(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use"}}"#,
        );
        match translate_event(ev).unwrap() {
            StreamChunk::Done { finish_reason } => assert_eq!(finish_reason, "tool_use"),
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn translate_message_delta_end_turn() {
        let ev = text_event(
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"}}"#,
        );
        match translate_event(ev).unwrap() {
            StreamChunk::Done { finish_reason } => assert_eq!(finish_reason, "end_turn"),
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn translate_message_stop_is_protocol() {
        let ev = text_event(r#"{"type":"message_stop"}"#);
        assert!(matches!(
            translate_event(ev),
            Err(StreamError::Protocol(_))
        ));
    }

    #[test]
    fn translate_ping_is_protocol() {
        let ev = text_event(r#"{"type":"ping"}"#);
        assert!(matches!(
            translate_event(ev),
            Err(StreamError::Protocol(_))
        ));
    }

    #[test]
    fn assistant_text_only_no_tool_use_block() {
        let request = ChatRequest {
            model: "claude-3-5-sonnet-latest".into(),
            messages: vec![ChatMessage::Assistant {
                content: "hi".into(),
                tool_calls: vec![],
            }],
            tools: vec![],
            temperature: 1.0,
        };
        let body = build_request_body(&request);
        assert_eq!(body["messages"][0]["content"][0]["text"], "hi");
        assert!(body["messages"][0]["content"]
            .as_array()
            .unwrap()
            .iter()
            .all(|b| b["type"] == "text"));
    }
}
