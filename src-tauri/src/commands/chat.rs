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
//!
//! When the model emits tool calls, the command runs them, appends the
//! results to the conversation, and re-issues the request. The agent
//! loop runs to completion within the same task (the UI doesn't need to
//! know about the loop, just sees the stream events fire one after
//! another per iteration). A hard 5-iteration cap prevents runaway.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use tauri::async_runtime::Mutex as AsyncMutex;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tokio_util::sync::CancellationToken;

use crate::llm::provider::{provider_for_settings, ChatMessage, ChatRequest, Provider};
use crate::llm::stream::{StreamChunk, StreamError};
use crate::storage::{Database, MessageRole, PersistedToolCall};
use crate::tools::tool::{ToolContext, ToolInvocation};
use crate::tools::ToolRegistry;
use crate::AppState;

/// Max agent iterations. Each iteration is a single streamed turn; the
/// loop runs until the model stops emitting tool calls. Capped to keep
/// runaway agents from hammering the API.
const MAX_AGENT_ITERATIONS: u32 = 5;

/// Per-chunk streaming timeout. If the provider goes silent for this
/// long between chunks (cold start, TCP half-open, hung model), we
/// surface a `stream stalled` error and let the UI recover. 90s
/// accommodates the slowest cold starts while still failing fast on
/// real stalls.
pub(crate) const STREAM_CHUNK_TIMEOUT: Duration = Duration::from_secs(90);

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

/// Tracks an in-flight stream so `stop_message` can cancel it. The map is
/// keyed by the *outermost* user message id, so cancelling the first
/// turn's user message kills every iteration of the agent loop for that
/// turn.
#[derive(Default)]
pub struct StreamRegistry {
    active: HashMap<String, CancellationToken>,
}

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

    // 3. Build the initial history and spawn the agent loop. The
    //    shared helper handles settings, history loading, tool
    //    registry assembly, and the cancellation token.
    let user_message_id = user_message.id.clone();
    let assistant_message_id = assistant_message.id.clone();
    let request_id_for_helper = request_id.clone();
    prepare_and_spawn_loop(
        app,
        state,
        streams,
        conversation_id,
        user_message_id,
        assistant_message_id,
        request_id_for_helper,
    )
    .await?;

    Ok(SendMessageResponse {
        user_message_id: user_message.id,
        assistant_message_id: assistant_message.id,
        request_id,
    })
}

/// "Regenerate" — wipe the last assistant response (and any tool
/// messages that came after the last user message) and re-run the
/// agent loop for the same user message. Used by the UI's Regenerate
/// button. Returns the new assistant_message_id; the user_message_id
/// is the existing one (no new user message is created).
#[tauri::command]
pub async fn regenerate_last(
    app: AppHandle,
    state: State<'_, AppState>,
    streams: State<'_, Arc<AsyncMutex<StreamRegistry>>>,
    conversation_id: String,
) -> Result<SendMessageResponse, String> {
    // 1. Find the most recent user message in the conversation.
    let db = state.db.clone();
    let convo_id_for_lookup = conversation_id.clone();
    let last_user = tokio::task::spawn_blocking(move || {
        db.last_user_message(&convo_id_for_lookup)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    let last_user = last_user
        .ok_or_else(|| "no user message in conversation to regenerate".to_string())?;

    // 2. Delete every message strictly after that user message
    //    (the previous assistant response + any tool messages).
    let db = state.db.clone();
    let convo_id_for_delete = conversation_id.clone();
    let user_id_for_delete = last_user.id.clone();
    let deleted = tokio::task::spawn_blocking(move || {
        db.delete_messages_after(&convo_id_for_delete, &user_id_for_delete)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    log::info!(
        "regenerate_last: deleted {} message(s) after user {}",
        deleted,
        last_user.id
    );

    // 3. Insert a fresh assistant placeholder.
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

    // 4. Use a fresh request_id so the chat event listener knows this
    //    is a new run.
    let request_id = format!("regen-{}", uuid::Uuid::new_v4());

    // 5. Spawn the agent loop using the same code path as send_message.
    let user_id_for_helper = last_user.id.clone();
    let assistant_id_for_helper = assistant_message.id.clone();
    let request_id_for_helper = request_id.clone();
    prepare_and_spawn_loop(
        app,
        state,
        streams,
        conversation_id,
        user_id_for_helper,
        assistant_id_for_helper,
        request_id_for_helper,
    )
    .await?;

    Ok(SendMessageResponse {
        user_message_id: last_user.id,
        assistant_message_id: assistant_message.id,
        request_id,
    })
}

/// Shared "load settings, build history, build provider/registry,
/// register cancel, spawn the agent loop" used by both `send_message`
/// and `regenerate_last`. The user message and assistant placeholder
/// must already be inserted in the DB before calling this — the
/// helper only handles the streaming setup.
async fn prepare_and_spawn_loop(
    app: AppHandle,
    state: State<'_, AppState>,
    streams: State<'_, Arc<AsyncMutex<StreamRegistry>>>,
    conversation_id: String,
    user_message_id: String,
    assistant_message_id: String,
    request_id: String,
) -> Result<(), String> {
    // Load settings and build the provider. `provider_for_settings`
    // honors `Settings::provider_kind` and routes to MiniMax, OpenAI,
    // Anthropic, or xAI with the matching API key + base URL.
    let db = state.db.clone();
    let settings = tokio::task::spawn_blocking(move || db.load_settings())
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let provider: Arc<dyn Provider> = match provider_for_settings(&settings) {
        Ok(p) => p,
        Err(e) => {
            log::warn!("chat: provider setup failed: {e}");
            return Err(e);
        }
    };
    let model = if settings.default_model.is_empty() {
        // Fall back to the active provider's built-in default if the
        // user hasn't picked a model explicitly. The provider's
        // `default_model()` reflects the right namespace (gpt-4o,
        // claude-3-5-sonnet-latest, …).
        provider.default_model().to_string()
    } else {
        settings.default_model.clone()
    };

    // Build the initial history (everything already in the conversation
    // up to and including the assistant placeholder).
    let db = state.db.clone();
    let convo_id_for_history = conversation_id.clone();
    let history = tokio::task::spawn_blocking(move || db.list_messages(&convo_id_for_history))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let initial_messages = history_to_provider(&history);

    // Tool registry includes MCP-backed tools loaded at startup.
    let registry = Arc::new(ToolRegistry::default_with_extras(
        state.mcp.tool_adapters(),
    ));

    // Register a cancellation token keyed by the user message id.
    let cancel = CancellationToken::new();
    {
        let mut registry_lock = streams.lock().await;
        registry_lock
            .active
            .insert(user_message_id.clone(), cancel.clone());
    }

    // Spawn the agent loop.
    let db_for_stream = state.db.clone();
    let streams_for_stream = streams.inner().clone();
    let app_for_stream = app.clone();
    let initial_messages_arc: Arc<Vec<ChatMessage>> = Arc::new(initial_messages);
    tokio::spawn(async move {
        run_agent_loop(
            app_for_stream,
            db_for_stream,
            streams_for_stream,
            provider,
            registry,
            initial_messages_arc,
            user_message_id,
            assistant_message_id,
            conversation_id,
            request_id,
            model,
            cancel,
        )
        .await;
    });

    Ok(())
}

#[tauri::command]
pub async fn stop_message(
    streams: State<'_, Arc<AsyncMutex<StreamRegistry>>>,
    user_message_id: String,
) -> Result<(), String> {
    let mut registry = streams.lock().await;
    if let Some(token) = registry.active.remove(&user_message_id) {
        token.cancel();
    }
    Ok(())
}

/// Map our persisted messages into the provider's `ChatMessage` enum.
fn history_to_provider(history: &[crate::storage::Message]) -> Vec<ChatMessage> {
    history
        .iter()
        .map(|m| match m.role {
            MessageRole::System => ChatMessage::System {
                content: m.content.clone(),
            },
            MessageRole::User => ChatMessage::User {
                content: m.content.clone(),
            },
            MessageRole::Assistant => ChatMessage::Assistant {
                content: m.content.clone(),
                tool_calls: m
                    .tool_calls
                    .iter()
                    .map(|tc| crate::llm::provider::ToolCall {
                        id: tc.id.clone(),
                        name: tc.name.clone(),
                        arguments: tc.arguments.clone(),
                    })
                    .collect(),
            },
            MessageRole::Tool => ChatMessage::Tool {
                tool_call_id: m
                    .tool_calls
                    .first()
                    .map(|tc| tc.id.clone())
                    .unwrap_or_default(),
                content: m.content.clone(),
            },
        })
        .collect()
}

/// Run the agent loop: stream a turn, run any tool calls, append the
/// results, and re-issue. Up to `MAX_AGENT_ITERATIONS` rounds.
#[allow(clippy::too_many_arguments)]
async fn run_agent_loop(
    app: AppHandle,
    db: Arc<Database>,
    streams: Arc<AsyncMutex<StreamRegistry>>,
    provider: Arc<dyn Provider>,
    registry: Arc<ToolRegistry>,
    initial_messages: Arc<Vec<ChatMessage>>,
    user_message_id: String,
    first_assistant_message_id: String,
    conversation_id: String,
    request_id: String,
    model: String,
    cancel: CancellationToken,
) {
    let tool_definitions = registry.definitions();
    let mut messages: Vec<ChatMessage> = initial_messages.as_ref().clone();
    let mut next_assistant_id = first_assistant_message_id;
    let mut iteration: u32 = 0;
    let mut stop_reason = "stop".to_string();

    while iteration < MAX_AGENT_ITERATIONS {
        iteration += 1;

        // Create a fresh assistant message placeholder for this iteration.
        // The previous iteration's placeholder will already have its
        // final content persisted; the new one starts empty and the
        // streaming code writes into it.
        if iteration > 1 {
            let db_clone = db.clone();
            let convo_clone = conversation_id.clone();
            let new_msg = tokio::task::spawn_blocking(move || {
                db_clone.insert_message(&convo_clone, MessageRole::Assistant, "", &[])
            })
            .await
            .map_err(|e| e.to_string());
            let new_msg = match new_msg {
                Ok(Ok(m)) => m,
                Ok(Err(e)) => {
                    handle_error(
                        &db,
                        &app,
                        &StreamError::Protocol(format!("DB insert failed: {e}")),
                        &next_assistant_id,
                        &request_id,
                    );
                    finish_turn(&streams, &user_message_id).await;
                    return;
                }
                Err(e) => {
                    handle_error(
                        &db,
                        &app,
                        &StreamError::Protocol(format!("DB insert join failed: {e}")),
                        &next_assistant_id,
                        &request_id,
                    );
                    finish_turn(&streams, &user_message_id).await;
                    return;
                }
            };
            next_assistant_id = new_msg.id;
        }

        let request = ChatRequest {
            model: model.clone(),
            messages: messages.clone(),
            tools: tool_definitions.clone(),
            temperature: 1.0,
        };

        let stream = match provider.stream(request).await {
            Ok(s) => s,
            Err(err) => {
                handle_error(&db, &app, &err, &next_assistant_id, &request_id);
                finish_turn(&streams, &user_message_id).await;
                return;
            }
        };
        futures_util::pin_mut!(stream);
        let mut full_text = String::new();
        let mut finish_reason = "stop".to_string();
        let mut hit_error: Option<StreamError> = None;
        let mut by_id: HashMap<String, PersistedToolCall> = HashMap::new();

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    finish_reason = "cancelled".to_string();
                    break;
                }
                // Per-chunk timeout: if the provider goes silent for
                // more than STREAM_CHUNK_TIMEOUT (no token at all,
                // TCP half-open, model hung), surface a clear error
                // instead of letting the UI spin forever. The first
                // chunk can take a while on cold start, so this
                // applies uniformly — `stream.next()` resolves as
                // soon as a single chunk arrives.
                next = tokio::time::timeout(STREAM_CHUNK_TIMEOUT, stream.next()) => {
                    let item = match next {
                        Ok(Some(item)) => item,
                        Ok(None) => break, // stream ended cleanly
                        Err(_elapsed) => {
                            hit_error = Some(StreamError::Network(
                                "stream stalled — no response for 90s".to_string(),
                            ));
                            break;
                        }
                    };
                    match item {
                        Ok(StreamChunk::Text { delta }) => {
                            full_text.push_str(&delta);
                            let db_clone = db.clone();
                            let id_clone = next_assistant_id.clone();
                            let delta_clone = delta.clone();
                            let _ = tokio::task::spawn_blocking(move || {
                                db_clone.append_message_content(&id_clone, &delta_clone)
                            }).await;
                            let _ = app.emit(
                                "chat://chunk",
                                ChunkEvent {
                                    request_id: request_id.clone(),
                                    assistant_message_id: next_assistant_id.clone(),
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
                                    assistant_message_id: next_assistant_id.clone(),
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

        // Persist the tool calls on this assistant turn (the streaming
        // append left the content in place; tool_calls_json is the
        // column that records the function-calls made during the turn).
        let tool_calls = if by_id.is_empty() {
            Vec::new()
        } else {
            by_id.into_values().collect::<Vec<_>>()
        };
        if !tool_calls.is_empty() {
            let db_clone = db.clone();
            let id_clone = next_assistant_id.clone();
            let convo_clone = conversation_id.clone();
            let calls_clone = tool_calls.clone();
            let _ = tokio::task::spawn_blocking(move || {
                db_clone.append_tool_calls(&id_clone, &calls_clone, &convo_clone)
            })
            .await;
        }

        if let Some(e) = hit_error {
            handle_error(&db, &app, &e, &next_assistant_id, &request_id);
            finish_turn(&streams, &user_message_id).await;
            return;
        }

        // No tool calls → this turn is final; emit done and exit.
        if tool_calls.is_empty() {
            let _ = app.emit(
                "chat://done",
                DoneEvent {
                    request_id: request_id.clone(),
                    assistant_message_id: next_assistant_id.clone(),
                    finish_reason: finish_reason.clone(),
                },
            );
            stop_reason = finish_reason;
            break;
        }

        // 8. Run each tool call (in the order the model emitted them).
        //    Persist a `tool` role message per result so the model sees
        //    them on the next iteration. If the user cancelled mid-loop,
        //    abort.
        for tc in &tool_calls {
            if cancel.is_cancelled() {
                stop_reason = "cancelled".to_string();
                break;
            }
            let consent_granted = if registry.requires_consent(&tc.name) {
                ask_consent(&app, &tc.name, &tc.arguments)
            } else {
                true
            };
            let invocation = ToolInvocation {
                name: tc.name.clone(),
                id: tc.id.clone(),
                arguments: parse_tool_arguments(&tc.arguments),
                bot_id: None,
            };
            let result = registry
                .execute(
                    invocation,
                    ToolContext {
                        consent_granted,
                        consent_prompt: None,
                        app: None,
                    },
                )
                .await;
            let (content, is_error) = match result {
                Ok(r) => (r.content, r.is_error),
                Err(e) => (format!("[error] {}", e), true),
            };
            // Persist the tool result as a `tool` message.
            let db_clone = db.clone();
            let convo_clone = conversation_id.clone();
            let content_clone = content.clone();
            let tc_id_clone = tc.id.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let persisted_tc = if is_error {
                    Vec::new()
                } else {
                    vec![PersistedToolCall {
                        id: tc_id_clone,
                        name: String::new(),
                        arguments: String::new(),
                    }]
                };
                db_clone.insert_message(
                    &convo_clone,
                    MessageRole::Tool,
                    &content_clone,
                    &persisted_tc,
                )
            })
            .await;
            // Append the tool result to the next request's history.
            messages.push(ChatMessage::Tool {
                tool_call_id: tc.id.clone(),
                content: if is_error {
                    content
                } else {
                    content
                },
            });
        }

        if cancel.is_cancelled() {
            stop_reason = "cancelled".to_string();
            break;
        }
    }

    // If we hit the iteration cap, surface a final done event with the
    // appropriate finish reason.
    if iteration >= MAX_AGENT_ITERATIONS && stop_reason == "stop" {
        let _ = app.emit(
            "chat://done",
            DoneEvent {
                request_id: request_id.clone(),
                assistant_message_id: next_assistant_id.clone(),
                finish_reason: "agent_iteration_limit".to_string(),
            },
        );
    }

    finish_turn(&streams, &user_message_id).await;
}

/// Ask the user for consent before running a side-effecting tool. Returns
/// `true` if the user approved, `false` otherwise. The dialog runs on the
/// main thread; if the user dismisses the dialog (escape / close), we
/// treat that as denial.
fn ask_consent(app: &AppHandle, tool_name: &str, arguments: &str) -> bool {
    let display = if arguments.len() > 240 {
        format!("{}…", &arguments[..240])
    } else {
        arguments.to_string()
    };
    let title = "MaxBot wants to run a side-effecting action".to_string();
    let body = format!("Tool: {}\n\n{}", tool_name, display);
    let confirmed = app
        .dialog()
        .message(body)
        .title(title)
        .kind(MessageDialogKind::Warning)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Allow".to_string(),
            "Deny".to_string(),
        ))
        .blocking_show();
    confirmed
}

fn parse_tool_arguments(arguments: &str) -> serde_json::Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| {
        // Some models stream malformed JSON; treat the raw string as the
        // whole payload so the tool sees what was intended.
        serde_json::Value::String(arguments.to_string())
    })
}

async fn finish_turn(streams: &Arc<AsyncMutex<StreamRegistry>>, user_message_id: &str) {
    let mut registry = streams.lock().await;
    registry.active.remove(user_message_id);
}

fn handle_error(
    db: &Arc<Database>,
    app: &AppHandle,
    err: &StreamError,
    assistant_message_id: &str,
    request_id: &str,
) {
    // The friendly mapping is the one the user actually sees —
    // short, actionable, no wire-protocol jargon. We persist it
    // on the assistant message row (so it survives a reload) and
    // ship it in the error event for the live UI.
    let friendly = err.friendly_message();
    let db_clone = db.clone();
    let id_clone = assistant_message_id.to_string();
    let friendly_owned = friendly.to_string();
    let _ = tokio::task::spawn_blocking(move || {
        db_clone.set_message_error_message(&id_clone, Some(&friendly_owned))
    });
    let _ = app.emit(
        "chat://error",
        ErrorEvent {
            request_id: request_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            message: friendly.to_string(),
        },
    );
    // The native dialog still gets the raw technical detail —
    // power users / logs benefit from it, and the chat UI hides
    // it from anyone who doesn't want to see it.
    let raw = err.to_string();
    let kind = match err {
        StreamError::MissingApiKey => Some(MessageDialogKind::Warning),
        StreamError::Http { status, .. } if *status == 401 || *status == 403 => {
            Some(MessageDialogKind::Warning)
        }
        _ => None,
    };
    if let Some(kind) = kind {
        app.dialog()
            .message(raw)
            .title("MaxBot")
            .kind(kind)
            .show(|_| {});
    }
}
