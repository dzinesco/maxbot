//! Shared wire-format translator for OpenAI-compatible chat-completions APIs.
//!
//! MiniMax, OpenAI, and xAI all use the same `POST /chat/completions`
//! JSON body and the same `data: {json}` SSE response. The only
//! differences between them are:
//!
//! - the base URL,
//! - the `Authorization: Bearer <key>` scheme (identical across all three),
//! - the model id namespace,
//! - the `X-Title` header (cosmetic).
//!
//! This module is the single place that knows how to talk to that wire
//! shape. Each provider struct in `minimax.rs`, `openai.rs`, and `xai.rs`
//! just supplies its base URL, model default, and `name()`.

use std::pin::Pin;

use eventsource_stream::Event;
use futures_util::{stream::Stream, StreamExt};
use reqwest::Client;
use serde::Deserialize;

use super::provider::{ChatMessage, ChatRequest};
use super::stream::{StreamChunk, StreamError};

/// Build a `reqwest::RequestBuilder` for an OpenAI-compatible chat
/// completions call. The caller is responsible for the bearer token and
/// any extra headers (e.g. `X-Title`).
pub fn build_request(
    http: &Client,
    endpoint: &str,
    api_key: &str,
    title_header: &str,
    body: &serde_json::Value,
) -> reqwest::RequestBuilder {
    http.post(endpoint)
        .bearer_auth(api_key)
        .header("X-Title", title_header)
        .json(body)
}

/// Translate our internal `ChatMessage` into the OpenAI-compatible wire
/// shape. Shared by all three OpenAI-compat providers.
pub fn message_to_wire(message: &ChatMessage) -> serde_json::Value {
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

/// Translate one Server-Sent Event into a `StreamChunk`. MiniMax, OpenAI,
/// and xAI all use the same `data: {json}` shape; the function returns
/// `StreamError::Protocol` on any non-message named event or empty chunk
/// so the caller can keep streaming without treating it as fatal.
pub fn translate_event(event: Event) -> Result<StreamChunk, StreamError> {
    if !event.event.is_empty() && event.event.as_str() != "message" {
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
    Err(StreamError::Protocol("empty SSE chunk".to_string()))
}

/// Streaming body for a chat-completions request. Shared by all three
/// OpenAI-compat providers — each one builds the body, hits its own
/// endpoint, then hands the byte stream to this function.
pub async fn stream_request(
    http: Client,
    endpoint: String,
    api_key: String,
    title_header: &'static str,
    request: ChatRequest,
) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk, StreamError>> + Send>>, StreamError> {
    if api_key.is_empty() {
        return Err(StreamError::MissingApiKey);
    }
    let body = serde_json::json!({
        "model": request.model,
        "messages": request.messages.iter().map(message_to_wire).collect::<Vec<_>>(),
        "stream": true,
        "temperature": request.temperature,
        "tools": request.tools,
    });
    let response = build_request(&http, &endpoint, &api_key, title_header, &body)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text_event(data: &str) -> Event {
        Event {
            event: "message".to_string(),
            data: data.to_string(),
            id: String::new(),
            retry: None,
        }
    }

    #[test]
    fn translates_text_delta() {
        let ev = text_event(
            r#"{"choices":[{"delta":{"content":"hello"},"finish_reason":null}]}"#,
        );
        match translate_event(ev).unwrap() {
            StreamChunk::Text { delta } => assert_eq!(delta, "hello"),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn translates_tool_call_delta() {
        let ev = text_event(
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"call_1","function":{"name":"shell_run","arguments":""}}]}}]}"#,
        );
        match translate_event(ev).unwrap() {
            StreamChunk::ToolCallDelta { id, name, arguments_delta } => {
                assert_eq!(id, "call_1");
                assert_eq!(name.as_deref(), Some("shell_run"));
                assert!(arguments_delta.unwrap_or_default().is_empty());
            }
            other => panic!("expected tool delta, got {other:?}"),
        }
    }

    #[test]
    fn translates_done_marker() {
        let ev = text_event("[DONE]");
        match translate_event(ev).unwrap() {
            StreamChunk::Done { finish_reason } => assert_eq!(finish_reason, "stop"),
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn empty_text_delta_is_protocol_error() {
        // The renderer should ignore this; we surface it as a protocol
        // error so the streaming loop can drop it without committing
        // empty text to the message.
        let ev = text_event(r#"{"choices":[{"delta":{"content":""}}]}"#);
        assert!(matches!(
            translate_event(ev),
            Err(StreamError::Protocol(_))
        ));
    }

    #[test]
    fn message_to_wire_for_system_user_assistant_tool() {
        let sys = message_to_wire(&ChatMessage::System {
            content: "be terse".into(),
        });
        assert_eq!(sys["role"], "system");
        assert_eq!(sys["content"], "be terse");
        let user = message_to_wire(&ChatMessage::User {
            content: "hi".into(),
        });
        assert_eq!(user["role"], "user");
        let asst = message_to_wire(&ChatMessage::Assistant {
            content: "yo".into(),
            tool_calls: vec![],
        });
        assert_eq!(asst["role"], "assistant");
        assert!(asst.get("tool_calls").is_none());
        let tool = message_to_wire(&ChatMessage::Tool {
            tool_call_id: "call_x".into(),
            content: "ok".into(),
        });
        assert_eq!(tool["role"], "tool");
        assert_eq!(tool["tool_call_id"], "call_x");
    }
}
