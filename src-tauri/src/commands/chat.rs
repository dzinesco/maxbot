//! Chat commands: send a user message, stream the assistant response back
//! via Tauri events, and let the UI cancel an in-flight stream.
//!
//! Wire protocol with the renderer:
//!   - `send_message(conversation_id, content, request_id)` returns
//!     `(user_message_id, assistant_message_id)`. Tokens arrive on
//!     `chat://chunk` events keyed by `assistant_message_id`; a final
//!     `chat://done` or `chat://error` event closes the turn.
//!   - `stop_message(assistant_message_id)` cancels the in-flight stream,
//!     persists whatever was received so far, and emits `chat://done`.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tauri::async_runtime::Mutex as AsyncMutex;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tokio_util::sync::CancellationToken;

use crate::llm::minimax::{MiniMaxProvider, DEFAULT_MODEL as DEFAULT_MINIMAX_MODEL};
use crate::llm::provider::{ChatMessage, ChatRequest, Provider, ToolDefinition};
use crate::llm::stream::{StreamChunk, StreamError};
use crate::storage::{Database, MessageRole, PersistedToolCall};
use crate::AppState;

#[derive(Serialize, Clone)]
struct ChunkEvent {
    request_id: String,
    assistant_message_id: String,
    chunk: StreamChunk,
}

#[derive(Serialize, Clone)]
struct DoneEvent {
    request_id: String,
    assistant_message_id: String,
    finish_reason: String,
}

#[derive(Serialize, Clone)]
struct ErrorEvent {
    request_id: String,
    assistant_message_id: String,
    message: String,
}

/// Tracks an in-flight stream so `stop_message` can cancel it.
#[derive(Default)]
pub struct StreamRegistry {
    active: HashMap<String, CancellationToken>,
}

/// Payload returned to the renderer after `send_message` completes the
/// setup phase (history persisted, LLM call dispatched). Tokens arrive
/// asynchronously via events.
#[derive(Serialize, Clone)]
pub struct SendMessageResponse {
    pub user_message_id: String,
    pub assistant_message_id: String,
    pub request_id: String,
}

#[tauri::command]
pub async fn send_message(
    app: AppHandle,
    state: State<'_, AppState>,
    streams: State<'_, Arc<AsyncMutex<StreamRegistry>>>,
    conversation_id: String,
    content: String,
    request_id: String,
) -> Result<SendMessageResponse, String> {
    if content.trim().is_empty() {
        return Err("message is empty".to_string());
    }

    // 1. Persist the user message.
    let db = state.db.clone();
    let convo_id_for_user = conversation_id.clone();
    let content_for_user = content.clone();
    let user_message = tokio::task::spawn_blocking(move || {
        db.insert_message(
            &convo_id_for_user,
            MessageRole::User,
            &content_for_user,
            &[],
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    // 2. Pre-create the assistant message so we can stream into it.
    let db = state.db.clone();
    let convo_id_for_assistant = conversation_id.clone();
    let assistant_message = tokio::task::spawn_blocking(move || {
        db.insert_message(
            &convo_id_for_assistant,
            MessageRole::Assistant,
            "",
            &[],
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;

    // 3. Resolve the settings, build the provider, and dispatch the stream.
    let db = state.db.clone();
    let settings = tokio::task::spawn_blocking(move || db.load_settings())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let api_key = settings
        .minimax_api_key
        .clone()
        .ok_or_else(|| "set your MiniMax API key in Settings first".to_string())?;
    let base_url = if settings.base_url.is_empty() {
        None
    } else {
        Some(settings.base_url.clone())
    };
    let model = if settings.default_model.is_empty() {
        DEFAULT_MINIMAX_MODEL.to_string()
    } else {
        settings.default_model.clone()
    };
    let provider: Arc<dyn Provider> = match base_url {
        Some(url) => Arc::new(MiniMaxProvider::with_base_url(api_key, url)),
        None => Arc::new(MiniMaxProvider::new(api_key)),
    };

    // 4. Build the message history by loading from the DB. We use the
    //    assistant_message_id as the canonical handle for events.
    let db = state.db.clone();
    let convo_id_for_history = conversation_id.clone();
    let history = tokio::task::spawn_blocking(move || db.list_messages(&convo_id_for_history))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let messages: Vec<ChatMessage> = history
        .into_iter()
        .map(|m| match m.role {
            MessageRole::System => ChatMessage::System { content: m.content },
            MessageRole::User => ChatMessage::User { content: m.content },
            MessageRole::Assistant => ChatMessage::Assistant {
                content: m.content,
                tool_calls: m
                    .tool_calls
                    .into_iter()
                    .map(|tc| crate::llm::provider::ToolCall {
                        id: tc.id,
                        name: tc.name,
                        arguments: tc.arguments,
                    })
                    .collect(),
            },
            MessageRole::Tool => ChatMessage::Tool {
                tool_call_id: String::new(),
                content: m.content,
            },
        })
        .collect();

    let request = ChatRequest {
        model,
        messages,
        tools: Vec::<ToolDefinition>::new(),
        temperature: 1.0,
    };

    // 5. Register a cancellation token for this turn, then spawn the stream
    //    consumer that persists chunks and emits events.
    let cancel = CancellationToken::new();
    {
        let mut registry = streams.lock().await;
        registry
            .active
            .insert(assistant_message.id.clone(), cancel.clone());
    }

    let db_for_stream = state.db.clone();
    let streams_clone = streams.inner().clone();
    let app_for_stream = app.clone();
    let assistant_message_id = assistant_message.id.clone();
    let request_id_for_stream = request_id.clone();
    let conversation_id_for_stream = conversation_id.clone();
    tokio::spawn(async move {
        run_stream(
            app_for_stream,
            db_for_stream,
            streams_clone,
            provider,
            request,
            assistant_message_id,
            conversation_id_for_stream,
            request_id_for_stream,
            cancel,
        )
        .await;
    });

    Ok(SendMessageResponse {
        user_message_id: user_message.id,
        assistant_message_id: assistant_message.id,
        request_id,
    })
}

#[tauri::command]
pub async fn stop_message(
    streams: State<'_, Arc<AsyncMutex<StreamRegistry>>>,
    assistant_message_id: String,
) -> Result<(), String> {
    let mut registry = streams.lock().await;
    if let Some(token) = registry.active.remove(&assistant_message_id) {
        token.cancel();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_stream(
    app: AppHandle,
    db: Arc<crate::storage::Database>,
    streams: Arc<AsyncMutex<StreamRegistry>>,
    provider: Arc<dyn Provider>,
    request: ChatRequest,
    assistant_message_id: String,
    conversation_id: String,
    request_id: String,
    cancel: CancellationToken,
) {
    let stream = match provider.stream(request).await {
        Ok(s) => s,
        Err(err) => {
            handle_error(&app, &err, &assistant_message_id, &request_id);
            finish_turn(&streams, &assistant_message_id).await;
            return;
        }
    };
    futures_util::pin_mut!(stream);
    let mut full_text = String::new();
    let mut finish_reason = "stop".to_string();
    let mut hit_error: Option<StreamError> = None;
    // Track tool-call fragments keyed by id so the streaming concatenation
    // ends up with a coherent call per id.
    let mut by_id: HashMap<String, PersistedToolCall> = HashMap::new();

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                finish_reason = "cancelled".to_string();
                break;
            }
            next = stream.next() => {
                let Some(item) = next else { break };
                match item {
                    Ok(StreamChunk::Text { delta }) => {
                        full_text.push_str(&delta);
                        let db_clone = db.clone();
                        let id_clone = assistant_message_id.clone();
                        let delta_clone = delta.clone();
                        // Best-effort append; SQLite writes are fast and we
                        // want a reloadable transcript.
                        let _ = tokio::task::spawn_blocking(move || {
                            db_clone.append_message_content(&id_clone, &delta_clone)
                        }).await;
                        let _ = app.emit(
                            "chat://chunk",
                            ChunkEvent {
                                request_id: request_id.clone(),
                                assistant_message_id: assistant_message_id.clone(),
                                chunk: StreamChunk::Text { delta },
                            },
                        );
                    }
                    Ok(StreamChunk::ToolCallDelta { id, name, arguments_delta }) => {
                        let entry = by_id.entry(id.clone()).or_insert_with(|| PersistedToolCall {
                            id: id.clone(),
                            name: String::new(),
                            arguments: String::new(),
                        });
                        if let Some(n) = name.clone() { entry.name = n; }
                        if let Some(args) = arguments_delta.clone() { entry.arguments.push_str(&args); }
                        let _ = app.emit(
                            "chat://chunk",
                            ChunkEvent {
                                request_id: request_id.clone(),
                                assistant_message_id: assistant_message_id.clone(),
                                chunk: StreamChunk::ToolCallDelta { id, name, arguments_delta },
                            },
                        );
                    }
                    Ok(StreamChunk::Done { finish_reason: reason }) => {
                        finish_reason = reason;
                        break;
                    }
                    Err(e) => {
                        hit_error = Some(e);
                        break;
                    }
                }
            }
        }
    }

    // Persist any accumulated tool calls on the assistant message.
    if !by_id.is_empty() {
        let tool_calls = by_id.into_values().collect::<Vec<_>>();
        let db_clone = db.clone();
        let id_clone = assistant_message_id.clone();
        let convo_clone = conversation_id.clone();
        let calls_clone = tool_calls.clone();
        let _ = tokio::task::spawn_blocking(move || {
            Database::append_tool_calls(&db_clone, &id_clone, &calls_clone, &convo_clone)
        }).await;
    }

    match hit_error {
        Some(e) => handle_error(&app, &e, &assistant_message_id, &request_id),
        None => {
            let _ = app.emit(
                "chat://done",
                DoneEvent {
                    request_id: request_id.clone(),
                    assistant_message_id: assistant_message_id.clone(),
                    finish_reason,
                },
            );
        }
    }
    finish_turn(&streams, &assistant_message_id).await;
}

async fn finish_turn(streams: &Arc<AsyncMutex<StreamRegistry>>, assistant_message_id: &str) {
    let mut registry = streams.lock().await;
    registry.active.remove(assistant_message_id);
}

fn handle_error(
    app: &AppHandle,
    err: &StreamError,
    assistant_message_id: &str,
    request_id: &str,
) {
    let message = err.to_string();
    let _ = app.emit(
        "chat://error",
        ErrorEvent {
            request_id: request_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            message: message.clone(),
        },
    );
    // Surface hard failures (missing API key, auth errors) as a native
    // dialog so the user sees them even if the chat window doesn't have
    // focus. We deliberately skip this for transient network/protocol
    // errors to avoid spamming dialogs on retry.
    let kind = match err {
        StreamError::MissingApiKey => Some(MessageDialogKind::Warning),
        StreamError::Http { status, .. } if *status == 401 || *status == 403 => {
            Some(MessageDialogKind::Warning)
        }
        _ => None,
    };
    if let Some(kind) = kind {
        app.dialog()
            .message(message)
            .title("MaxBot")
            .kind(kind)
            .show(|_| {});
    }
}
