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
    // No usable content in this event. OpenAI-compat providers send a
    // trailing chunk with usage stats and an empty `choices` array
    // (or empty `delta.content`); that's a benign end-of-stream
    // signal, not a protocol violation. Surface it as a normal
    // `Done` so the chat loop breaks cleanly instead of rendering
    // an "empty SSE chunk" error to the user after every reply.
    Ok(StreamChunk::Done {
        finish_reason: "stop".to_string(),
    })
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
    fn empty_event_is_done_not_protocol_error() {
        // OpenAI-compat providers send a trailing chunk with usage
        // stats and an empty `choices` array (or an empty
        // `delta.content`). That's a benign end-of-stream signal,
        // not a protocol violation — we should not surface it as
        // an error. It must translate to a normal `Done` so the
        // chat loop breaks cleanly without rendering an
        // "empty SSE chunk" error to the user.
        let empty_choices = text_event(r#"{"choices":[],"usage":{"prompt_tokens":5}}"#);
        match translate_event(empty_choices).unwrap() {
            StreamChunk::Done { finish_reason } => assert_eq!(finish_reason, "stop"),
            other => panic!("trailing usage chunk must be Done, got {other:?}"),
        }
        let empty_content = text_event(r#"{"choices":[{"delta":{"content":""}}]}"#);
        match translate_event(empty_content).unwrap() {
            StreamChunk::Done { finish_reason } => assert_eq!(finish_reason, "stop"),
            other => panic!("empty text delta must be Done, got {other:?}"),
        }
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

    // ---- End-to-end SSE pipeline tests (tokio::io::duplex) ----
    //
    // The bug we fixed: OpenAI-compat providers send a trailing
    // chunk with usage stats and no `delta` content. The old
    // `translate_event` returned `Err(Protocol("empty SSE chunk"))`
    // for that event, which surfaced in the live chat as
    // "[error] protocol: empty SSE chunk" after every successful
    // message. The fix is to return `Done { finish_reason: "stop" }`
    // instead. These tests drive the same parsing pipeline the
    // `reqwest::bytes_stream` path drives in production — they
    // feed raw SSE bytes through `eventsource_stream::EventStream`
    // and the same `translate_event` mapping the chat loop uses.
    //
    // The "server" side is the `tokio::io::duplex` half we write
    // raw bytes into; the "client" side becomes a `ReaderStream`
    // that the `EventStream` consumes.

    use eventsource_stream::EventStream as EsStream;
    use futures_util::stream::StreamExt;
    use tokio::io::{duplex, AsyncWriteExt};
    use tokio_util::io::ReaderStream;

    /// Drive a raw SSE byte stream through the same parser the
    /// HTTP path uses, and return every translated chunk the
    /// chat loop would see. Asserts that nothing in the byte
    /// stream is a `StreamError::Protocol` — that's the bug.
    async fn collect_sse_chunks(sse_bytes: &'static [u8]) -> Vec<StreamChunk> {
        let (client, mut server) = duplex(64 * 1024);
        let writer = tokio::spawn(async move {
            server.write_all(sse_bytes).await.expect("write");
            server.shutdown().await.expect("shutdown");
        });
        let byte_stream = ReaderStream::new(client);
        let parsed = EsStream::new(byte_stream).map(|item| match item {
            Ok(event) => translate_event(event),
            Err(e) => Err(StreamError::Network(e.to_string())),
        });
        let mut chunks = Vec::new();
        let mut stream = Box::pin(parsed);
        while let Some(result) = stream.next().await {
            match result {
                Ok(chunk) => chunks.push(chunk),
                Err(StreamError::Protocol(msg)) => {
                    panic!("protocol error during SSE stream: {msg}");
                }
                Err(e) => panic!("non-protocol stream error: {e:?}"),
            }
        }
        writer.await.expect("writer task");
        chunks
    }

    #[tokio::test]
    async fn e2e_sse_with_trailing_usage_chunk() {
        // Realistic OpenAI-compat wire bytes: a few text deltas,
        // a trailing usage-only chunk (empty `choices`), then
        // `[DONE]`. The trailing chunk is the case that triggered
        // the user-visible "empty SSE chunk" error before the fix.
        let sse = b"\
data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}],\"model\":\"x\"}

data: {\"choices\":[{\"delta\":{\"content\":\" world\"},\"finish_reason\":null}]}

data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2,\"total_tokens\":7}}

data: [DONE]

";
        let chunks = collect_sse_chunks(sse).await;
        // Expect four chunks: two text deltas, then a Done from
        // the trailing usage chunk (the case the fix turns into
        // a normal Done), then another Done from the `data:
        // [DONE]` sentinel that OpenAI-compat providers emit.
        assert_eq!(chunks.len(), 4, "got chunks: {chunks:?}");
        match &chunks[0] {
            StreamChunk::Text { delta } => assert_eq!(delta, "hello"),
            other => panic!("expected Text(hello), got {other:?}"),
        }
        match &chunks[1] {
            StreamChunk::Text { delta } => assert_eq!(delta, " world"),
            other => panic!("expected Text(\" world\"), got {other:?}"),
        }
        assert!(matches!(&chunks[2], StreamChunk::Done { finish_reason } if finish_reason == "stop"));
        assert!(matches!(&chunks[3], StreamChunk::Done { finish_reason } if finish_reason == "stop"));
    }

    #[tokio::test]
    async fn e2e_sse_with_text_then_stop_marker() {
        // A simpler shape: text deltas, a final `finish_reason:
        // stop` choice, then `[DONE]`. Verifies the regular
        // non-trailing-usage path still works after the fix.
        let sse = b"\
data: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}

data: {\"choices\":[{\"delta\":{\"content\":\"b\"}}]}

data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}

data: [DONE]

";
        let chunks = collect_sse_chunks(sse).await;
        // Text, Text, Done (from the finish_reason=stop choice),
        // Done (from the [DONE] sentinel).
        assert_eq!(chunks.len(), 4, "got chunks: {chunks:?}");
        assert!(matches!(&chunks[0], StreamChunk::Text { delta } if delta == "a"));
        assert!(matches!(&chunks[1], StreamChunk::Text { delta } if delta == "b"));
        assert!(matches!(&chunks[2], StreamChunk::Done { finish_reason } if finish_reason == "stop"));
        assert!(matches!(&chunks[3], StreamChunk::Done { finish_reason } if finish_reason == "stop"));
    }

    #[tokio::test]
    async fn e2e_sse_stream_ends_cleanly_on_trailing_done() {
        // The original bug also fired on the `data: [DONE]`
        // marker when no other end-of-stream signal preceded
        // it. Make sure that path still terminates cleanly with
        // a Done chunk (and no panic).
        let sse = b"\
data: {\"choices\":[{\"delta\":{\"content\":\"done\"}}]}

data: [DONE]

";
        let chunks = collect_sse_chunks(sse).await;
        assert_eq!(chunks.len(), 2, "got chunks: {chunks:?}");
        assert!(matches!(&chunks[0], StreamChunk::Text { delta } if delta == "done"));
        assert!(matches!(&chunks[1], StreamChunk::Done { finish_reason } if finish_reason == "stop"));
    }
}
