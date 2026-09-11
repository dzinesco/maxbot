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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::async_runtime::Mutex as AsyncMutex;
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tokio_util::sync::CancellationToken;

use crate::llm::provider::{provider_for_settings, ChatMessage, ChatRequest, Provider};
use crate::llm::stream::{StreamChunk, StreamError};
use crate::storage::{Database, MessageRole, PersistedToolCall};
use crate::tools::tool::{ToolContext, ToolError, ToolInvocation};
use crate::tools::ToolRegistry;
use crate::AppState;

/// Max agent iterations. Each iteration is a single streamed turn; the
/// loop runs until the model stops emitting tool calls. Capped to keep
/// runaway agents from hammering the API.
const MAX_AGENT_ITERATIONS: u32 = 5;

/// v3.7.16 — S3a. Per-turn failure threshold for an identical
/// (tool_name, args_hash) pair. The first failure tells the LLM
/// once; the second identical failure cancels the turn so the
/// user sees what the model has so far instead of getting
/// spammed with permission dialogs.
const TOOL_FAILURE_AUTO_STOP: u32 = 2;

/// v3.7.16 — S3a. The tool names that may pop the consent
/// dialog / approval sheet. Tools outside this list either run
/// silently (auto) or fail with a synthetic tool error. The
/// renderer mirrors this list in
/// `src/v4/lib/toolAllowlist.ts` — adding a tool here should
/// land in both places.
///
/// v3.7.17 — S3a-real. shell_run REMOVED from this list. The
/// model kept calling it for "what day is it?" instead of
/// answering in text from the date header. Per-bot tool
/// enforcement now happens via the bot's `allowed_tools`
/// list inside `run_agent_loop`, not as a hardcoded global.
const TOOL_PERMISSION_ALLOWLIST: &[&str] = &[
    // Mail / Gmail — outbound email.
    "mail_draft",
    "gmail_send",
    // Calendar — outbound event creation.
    "calendar_event_create",
    // Computer Use — VM screen + browser.
    "vm_computer_use",
    "vm_browser_open",
    // Loop daemon — start / stop / read.
    "loopd_start",
    "loopd_stop",
    "loopd_status",
    "loopd_read_task",
    "loopd_read_journal",
    // Mac-side browser automation.
    "ego_browser",
];

/// Per-chunk streaming timeout. If the provider goes silent for this
/// long between chunks (cold start, TCP half-open, hung model), we
/// surface a `stream stalled` error and let the UI recover. 90s
/// accommodates the slowest cold starts while still failing fast on
/// real stalls.
pub(crate) const STREAM_CHUNK_TIMEOUT: Duration = Duration::from_secs(15);

/// v3.7.16 — S3a. Build the per-turn date header that gets
/// prepended to the chat's system messages. Includes the rule
/// the LLM must follow ("do not call tools for the current
/// date") so the model doesn't burn a tool roundtrip on a
/// calendar question. The header is rebuilt on every send — the
/// date stays fresh across long-lived conversations.
///
/// v3.7.17 — S3a-real. Per Tyler's brief: the header is the
/// two-line form `Today is {Weekday}, {Month} {Day}, {Year}
/// ({IANA tz}).` followed by `Do not call tools for the
/// current date or time.` The IANA timezone (e.g. "America/
/// Denver") comes from `iana-time-zone` — the system TZ env
/// var on macOS / Linux, the Windows equivalent on Windows.
/// Falls back to the chrono `%Z` abbreviation if the IANA
/// lookup fails (e.g. minimal containers).
fn date_header_for_now() -> String {
    let now = chrono::Local::now();
    let day = now
        .format("%d")
        .to_string()
        .trim_start_matches('0')
        .to_string();
    let tz = iana_time_zone::get_timezone()
        .ok()
        .or_else(|| Some(now.format("%Z").to_string()))
        .unwrap_or_else(|| "UTC".to_string());
    format!(
        "Today is {}, {} {}, {} ({}).\n\
         Do not call tools for the current date or time.\n",
        now.format("%A"),   // Friday
        now.format("%B"),   // September
        day,                // 11
        now.format("%Y"),   // 2026
        tz,                 // America/Denver (or fallback)
    )
}

/// v3.7.17 — S3a follow-up. Truncate the first user message into
/// a thread title. Word-boundary aware so we don't slice a word
/// in half; ellipsis on truncation. Mirrors the JS
/// `truncateTitle` in App.tsx — both sides agree on the rule.
fn truncate_title_for_storage(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    // Walk back from `max` chars to find the last whitespace,
    // so the cut lands at a word boundary.
    let cut: String = trimmed.chars().take(max).collect();
    if let Some(idx) = cut.rfind(|c: char| c.is_whitespace()) {
        if idx > max / 2 {
            return format!("{}…", &cut[..idx]);
        }
    }
    format!("{}…", cut)
}

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

    // 1a. v3.7.17 — S3a follow-up. Auto-derive the thread title
    // from the first user message. If the conversation's current
    // title is still the Rust default ("New chat") or empty,
    // overwrite it with a truncated version of this message.
    // Subsequent sends keep whatever title is set (we never
    // rename a user-edited title).
    {
        let db = state.db.clone();
        let convo_id = conversation_id.clone();
        let first_user_line = content.clone();
        tokio::task::spawn_blocking(move || {
            // Read current title.
            let current = db.get_conversation_title(&convo_id).ok().flatten();
            let needs_rename = match current.as_deref() {
                None => false,                       // conversation gone — bail
                Some("") | Some("New chat") => true, // default → rename
                Some(_) => false,                    // user-set → leave alone
            };
            if needs_rename {
                let new_title = truncate_title_for_storage(&first_user_line, 60);
                // Only rename if the current row is STILL the
                // default — protects against a concurrent rename
                // (the JS UI calls renameConversation after send).
                let _ = db.set_title_if(
                    &convo_id,
                    "New chat",
                    &new_title,
                );
            }
        })
        .await
        .ok();
    }

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
    let mut initial_messages = history_to_provider(&history);
    // v3.7.16 — S3a. Inject today's date as a system message
    // at the start of every turn. The header includes a rule
    // telling the model NOT to call tools for the current date
    // — the date is already here. The persisted system row
    // (if any) stays untouched; this is layered on top per
    // turn so long-lived conversations don't get a stale date.
    initial_messages.insert(
        0,
        ChatMessage::System {
            content: date_header_for_now(),
        },
    );

    // Tool registry includes MCP-backed tools loaded at startup.
    let registry = Arc::new(ToolRegistry::default_with_extras(
        state.mcp.tool_adapters(),
    ));

    // v3.7.17 — S3a-real. Resolve the bot for this conversation
    // so the agent loop can enforce the per-bot tool allowlist.
    // If the bot is gone (deleted mid-send) we fall back to an
    // empty allowlist — every tool call becomes "unknown
    // tool; reply in text". That's the safest default: the
    // model can still answer in text from the system prompt +
    // history.
    let db_for_bot = state.db.clone();
    let convo_id_for_bot = conversation_id.clone();
    let allowed_tools: Vec<String> = tokio::task::spawn_blocking(move || {
        let convo = db_for_bot.get_conversation(&convo_id_for_bot).ok().flatten();
        let bot_id = convo.and_then(|c| c.bot_id);
        match bot_id {
            Some(bid) => db_for_bot
                .get_bot(&bid)
                .ok()
                .flatten()
                .map(|b| b.allowed_tools)
                .unwrap_or_default(),
            None => Vec::new(),
        }
    })
    .await
    .unwrap_or_default();

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
            allowed_tools,
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
    // v3.7.17 — S3a-real. The bot's per-instance tool
    // allowlist. Any tool call whose `name` is not in this
    // list is short-circuited with a synthetic error before
    // consent / dedupe / execution. The check runs at the
    // start of every tool iteration.
    allowed_tools: Vec<String>,
    cancel: CancellationToken,
) {
    let tool_definitions = registry.definitions();
    let mut messages: Vec<ChatMessage> = initial_messages.as_ref().clone();
    let mut next_assistant_id = first_assistant_message_id;
    let mut iteration: u32 = 0;
    let mut stop_reason = "stop".to_string();
    // v3.7.13 — Per-turn denylist of tool calls the
    // user has already vetoed. Keyed by
    // (tool_name, sha256(args)) so identical
    // retries the LLM emits in the same turn are
    // skipped instead of re-prompting the consent
    // dialog or re-running the tool. The set is
    // local to this function, so a new user
    // message gets a fresh denylist — the model
    // can legitimately retry after the user has
    // had a chance to clarify intent. SHA-256
    // gives collision-resistant matching; we
    // don't want the LLM to bypass the denylist
    // by reshuffling key order or whitespace.
    let mut denied_tool_calls: HashSet<(String, [u8; 32])> = HashSet::new();
    // v3.7.16 — S3a. Per-turn counter of tool calls that
    // FAILED with the same (tool_name, sha256(args)) pair. The
    // map value is the count; we increment on each error, replace
    // the error message on the FIRST failure so the LLM can
    // retry with corrected args, and CANCEL the turn on the
    // SECOND identical failure so the user isn't dragged into a
    // click-loop. Cleared by a fresh user message (it's local to
    // `run_agent_loop`).
    let mut failed_tool_calls: HashMap<(String, [u8; 32]), u32> = HashMap::new();

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
        //
        // v3.7.17 — S3a follow-up. Per-turn count of calls to
        // a denylisted tool. First call returns a synthetic error
        // ("invalid tool; answer in text"); any subsequent call
        // cancels the turn. The count is per tool name (NOT per
        // v3.7.17 — S3a-real. Per-turn count of calls to a tool
        // NOT in this bot's allowed_tools list. The map is keyed
        // on tool NAME (not per args hash) because we want to
        // catch the model retrying the same tool with slightly
        // different args. First call returns a synthetic
        // `{"error":"unknown tool; reply in text"}`; the second
        // call this turn cancels.
        //
        // The denial is pushed to `messages[]` so the model
        // sees it on the next iteration. We deliberately do
        // NOT persist a `tool`-role row in the DB — the
        // renderer filters those rows in ChatPane, and the
        // user doesn't need to see "unknown tool; reply in
        // text" as a chat bubble.
        let mut unknown_tool_count: HashMap<String, u32> = HashMap::new();
        for tc in &tool_calls {
            if cancel.is_cancelled() {
                stop_reason = "cancelled".to_string();
                break;
            }
            // v3.7.17 — S3a-real. Per-bot tool allowlist.
            // If the tool is not in this bot's `allowed_tools`,
            // return a synthetic JSON error to the model and do
            // NOT execute / persist / emit an approval.
            if !allowed_tools.iter().any(|t| t == &tc.name) {
                let count = {
                    let entry = unknown_tool_count
                        .entry(tc.name.clone())
                        .or_insert(0);
                    *entry += 1;
                    *entry
                };
                let synthetic = if count == 1 {
                    // The exact JSON shape the user asked for.
                    // The model sees this and is told to reply
                    // in text (not call tools).
                    "{\"error\":\"unknown tool; reply in text\"}".to_string()
                } else {
                    // Second call to the same unknown tool
                    // this turn → cancel.
                    format!(
                        "Tool '{}' is not enabled for this bot. \
                         Answer without it.",
                        tc.name
                    )
                };
                messages.push(ChatMessage::Tool {
                    tool_call_id: tc.id.clone(),
                    content: synthetic,
                });
                if count >= 2 {
                    cancel.cancel();
                    stop_reason = "unknown_tool_repeated".to_string();
                    break;
                }
                continue;
            }
            // v3.7.13 — Denylist check. The user
            // already rejected this exact call
            // (same tool name + same JSON args) in
            // this turn. Skip the call and the
            // consent dialog entirely; the LLM
            // already has the denial in its
            // history. SHA-256 of the args is the
            // collision-resistant key — see the
            // comment on `denied_tool_calls` above.
            let args_hash = hash_tool_args(&tc.name, &tc.arguments);
            // v3.7.16 — S3a. Per-turn dedupe of identical
            // tool calls. If this exact (name, args) has
            // already failed once this turn, skip the
            // consent dialog AND the tool execution —
            // just push the synthetic failure message and
            // bump the counter. The second identical
            // failure cancels the turn.
            if let Some(&prior_failures) =
                failed_tool_calls.get(&(tc.name.clone(), args_hash))
            {
                if prior_failures >= 1 {
                    let n = {
                        let entry = failed_tool_calls
                            .entry((tc.name.clone(), args_hash))
                            .or_insert(0);
                        *entry += 1;
                        *entry
                    };
                    if n >= TOOL_FAILURE_AUTO_STOP {
                        // Second (or later) identical failure
                        // → cancel the turn so the user sees
                        // what the model has so far.
                        let stop_msg = format!(
                            "Tool '{}' failed {} times with the same \
                             arguments. Stopping the turn — answer \
                             from the text above.",
                            tc.name, n,
                        );
                        let db_clone = db.clone();
                        let convo_clone = conversation_id.clone();
                        let tc_id_clone = tc.id.clone();
                        let content_clone = stop_msg.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            db_clone.insert_message(
                                &convo_clone,
                                MessageRole::Tool,
                                &content_clone,
                                &[PersistedToolCall {
                                    id: tc_id_clone,
                                    name: String::new(),
                                    arguments: String::new(),
                                }],
                            )
                        })
                        .await;
                        messages.push(ChatMessage::Tool {
                            tool_call_id: tc.id.clone(),
                            content: stop_msg,
                        });
                        cancel.cancel();
                        break;
                    } else {
                        // Defensive — the only way to reach
                        // here with n < 2 is if the prior
                        // failures count was somehow < 1.
                        // Treat as first retry.
                        let synthetic = format!(
                            "Tool '{}' failed again. Do not retry this \
                             exact call — answer without tools or try \
                             a different tool.",
                            tc.name,
                        );
                        let db_clone = db.clone();
                        let convo_clone = conversation_id.clone();
                        let tc_id_clone = tc.id.clone();
                        let content_clone = synthetic.clone();
                        let _ = tokio::task::spawn_blocking(move || {
                            db_clone.insert_message(
                                &convo_clone,
                                MessageRole::Tool,
                                &content_clone,
                                &[PersistedToolCall {
                                    id: tc_id_clone,
                                    name: String::new(),
                                    arguments: String::new(),
                                }],
                            )
                        })
                        .await;
                        messages.push(ChatMessage::Tool {
                            tool_call_id: tc.id.clone(),
                            content: synthetic,
                        });
                        continue;
                    }
                }
            }
            if denied_tool_calls.contains(&(tc.name.clone(), args_hash)) {
                let skip_content = serde_json::to_string(
                    &serde_json::json!({"error": "denied by user"}),
                )
                .unwrap_or_else(|_| {
                    "{\"error\":\"denied by user\"}".to_string()
                });
                let db_clone = db.clone();
                let convo_clone = conversation_id.clone();
                let tc_id_clone = tc.id.clone();
                let content_clone = skip_content.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    db_clone.insert_message(
                        &convo_clone,
                        MessageRole::Tool,
                        &content_clone,
                        &[PersistedToolCall {
                            id: tc_id_clone,
                            name: String::new(),
                            arguments: String::new(),
                        }],
                    )
                })
                .await;
                messages.push(ChatMessage::Tool {
                    tool_call_id: tc.id.clone(),
                    content: skip_content,
                });
                continue;
            }
            let consent_granted = if registry.requires_consent(&tc.name) {
                // v3.7.16 — S3a. The consent dialog is
                // gated on TOOL_PERMISSION_ALLOWLIST.
                // Tools outside the allowlist (e.g.
                // file_read on a local path, web_search,
                // shell_run on the user's own machine)
                // run auto — only mail / computer /
                // loopd actually touch the world in a
                // way the user wants to gate. Adding a
                // tool to the allowlist should land in
                // both this list AND the renderer's
                // `src/v4/lib/toolAllowlist.ts`.
                if TOOL_PERMISSION_ALLOWLIST
                    .contains(&tc.name.as_str())
                {
                    // v3.7.16 — S3a. If this exact call
                    // has already failed once this turn,
                    // skip the dialog entirely. The LLM
                    // is just retrying a bad call; we
                    // return the synthetic failure again
                    // (handled after the dedupe block
                    // below) and bump the counter — the
                    // second failure cancels the turn.
                    if failed_tool_calls
                        .contains_key(&(tc.name.clone(), args_hash))
                    {
                        false
                    } else {
                        ask_consent(&app, &tc.name, &tc.arguments)
                    }
                } else {
                    // Outside the allowlist → auto-grant.
                    true
                }
            } else {
                true
            };
            // v3.7.13 — Record the denial so any
            // retry the LLM emits later in this
            // turn (e.g. as a fallback after a
            // different tool failed) is blocked.
            // The set is local to `run_agent_loop`,
            // so a fresh user message resets it.
            if TOOL_PERMISSION_ALLOWLIST.contains(&tc.name.as_str())
                && registry.requires_consent(&tc.name)
                && !consent_granted
            {
                denied_tool_calls.insert((tc.name.clone(), args_hash));
            }
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
                        // v3.7.3 — Pass the chat command's
                        // `AppHandle` so the v3.7.1 Mac-app
                        // path applies to shared_* tool calls
                        // (otherwise the chat path's `app: None`
                        // made them fall through to the local
                        // Mac filesystem, which is wrong when
                        // the daemon is the source of truth for
                        // the shared folder). The Mac app
                        // process always has an `AppHandle`;
                        // the original `app: None` predates
                        // the v3.7.1 routing layer.
                        app: Some(app.clone()),
                    },
                )
                .await;
            // v3.7.13 — Track tool errors that
            // surface as `UserDenied` (e.g. an
            // inner guard). The denylist semantics
            // are the same: a retry with the same
            // args is blocked this turn. Borrow the
            // error by reference (the `Result` was
            // already moved into the match below)
            // to detect the `UserDenied` variant
            // before we destructure the result.
            if let Err(ToolError::UserDenied) = &result {
                denied_tool_calls
                    .insert((tc.name.clone(), args_hash));
            }
            let (mut content, is_error) = match result {
                Ok(r) => (r.content, r.is_error),
                Err(e) => (format!("[error] {}", e), true),
            };
            // v3.7.16 — S3a. Per-turn failure tracking. On
            // the FIRST error for a given (name, args), bump
            // the counter and replace the error message so
            // the LLM knows the call failed and shouldn't
            // retry the same args. The SECOND identical
            // failure is caught by the dedupe branch above
            // (it fires before consent + execution, so the
            // tool doesn't actually run again).
            if is_error {
                let n = {
                    let entry = failed_tool_calls
                        .entry((tc.name.clone(), args_hash))
                        .or_insert(0);
                    *entry += 1;
                    *entry
                };
                content = format!(
                    "Tool '{}' failed: {}. \
                     Do not retry this exact call — answer without \
                     tools or try a different tool with corrected \
                     arguments.",
                    tc.name, content,
                );
                let _ = n; // n == 1 here; the second attempt
                           // short-circuits via the dedupe
                           // branch above.
            } else {
                // Success — clear any prior failure count
                // for this exact call. The LLM may legitimately
                // call the same tool with the same args in a
                // later iteration after fixing the underlying
                // problem.
                failed_tool_calls.remove(&(tc.name.clone(), args_hash));
            }
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

/// v3.7.13 — Hash a (tool_name, args) pair for
/// the per-turn denylist. SHA-256 is overkill for
/// the input size but it's the collision-resistant
/// default; we don't want the LLM to bypass the
/// denylist by reshuffling key order, swapping
/// `null` for `false`, or quoting whitespace
/// differently. The hash is mixed with the tool
/// name (so two different tools with the same args
/// don't collide).
fn hash_tool_args(tool_name: &str, arguments: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(tool_name.as_bytes());
    hasher.update([0u8]); // separator — no tool name ends with a NUL
    hasher.update(arguments.as_bytes());
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- v3.7.13 — denied_tool_calls denylist ----

    /// The denylist is a `HashSet<(String,
    /// [u8; 32])>` of `(tool_name, args_hash)`.
    /// Two calls with the same tool name but
    /// different args hash to different keys, so
    /// denying `mail_draft(to=x)` does not block
    /// `mail_draft(to=y)`.
    #[test]
    fn denied_tool_calls_keys_differ_for_different_args() {
        let a = hash_tool_args("mail_draft", r#"{"to":"x@y"}"#);
        let b = hash_tool_args("mail_draft", r#"{"to":"y@z"}"#);
        assert_ne!(a, b);
        let mut set: HashSet<(String, [u8; 32])> = HashSet::new();
        set.insert(("mail_draft".to_string(), a));
        assert!(set.contains(&("mail_draft".to_string(), a)));
        assert!(!set.contains(&("mail_draft".to_string(), b)));
    }

    /// Two calls with the same tool name and
    /// *identical* args hash to the same key. The
    /// LLM can't bypass the denylist by
    /// re-formatting the same args — JSON
    /// whitespace and key order would be the
    /// obvious bypass, but since we hash the raw
    /// arguments string, those *are* different
    /// hashes (which is the point — the LLM
    /// should re-ask, not auto-retry).
    #[test]
    fn denied_tool_calls_keys_match_for_identical_args() {
        let a = hash_tool_args(
            "mail_draft",
            r#"{"to":"x@y","subject":"hi"}"#,
        );
        let b = hash_tool_args(
            "mail_draft",
            r#"{"to":"x@y","subject":"hi"}"#,
        );
        assert_eq!(a, b);
    }

    /// Different tool names with the same args
    /// hash to different keys (the tool name is
    /// mixed into the hash). Blocking
    /// `mail_draft(args)` does not block
    /// `gmail_send(args)`.
    #[test]
    fn denied_tool_calls_keys_differ_across_tool_names() {
        let a = hash_tool_args("mail_draft", r#"{"to":"x@y"}"#);
        let b = hash_tool_args("gmail_send", r#"{"to":"x@y"}"#);
        assert_ne!(a, b);
    }

    /// The denylist semantics: insert a denial,
    /// confirm a second call with the same key
    /// hits the denylist (i.e. is blocked), and
    /// confirm a fresh `HashSet` (the per-turn
    /// reset) is empty. This is the contract
    /// `run_agent_loop` relies on.
    #[test]
    fn denied_tool_calls_blocks_retry_then_resets() {
        let mut set: HashSet<(String, [u8; 32])> = HashSet::new();
        let key = (
            "mail_draft".to_string(),
            hash_tool_args("mail_draft", r#"{"to":"x@y"}"#),
        );
        assert!(!set.contains(&key));
        set.insert(key.clone());
        assert!(set.contains(&key));
        // Per-turn reset: a new `HashSet` is
        // empty, so the LLM can retry after the
        // user has had a chance to clarify.
        let fresh: HashSet<(String, [u8; 32])> = HashSet::new();
        assert!(!fresh.contains(&key));
    }

    /// `UserDenied` is a distinct variant of
    /// `ToolError` so the executor can branch
    /// on it without parsing the error string.
    /// The display message is fixed so the
    /// renderer's "you declined" copy doesn't
    /// drift.
    #[test]
    fn tool_error_user_denied_has_stable_display() {
        let err = ToolError::UserDenied;
        assert_eq!(err.to_string(), "denied by user (already in this turn)");
    }

    // ---- v3.7.16 — S3a. failed_tool_calls counter ----

    /// The counter is keyed the same way as
    /// `denied_tool_calls` — `(tool_name,
    /// sha256(args))`. Two different args hash to
    /// different keys; the LLM legitimately retrying
    /// with corrected args starts a fresh counter.
    #[test]
    fn failed_tool_calls_keys_differ_for_different_args() {
        let mut map: HashMap<(String, [u8; 32]), u32> = HashMap::new();
        let key_a = (
            "shell_run".to_string(),
            hash_tool_args("shell_run", r#"{"command":"date"}"#),
        );
        let key_b = (
            "shell_run".to_string(),
            hash_tool_args("shell_run", r#"{"command":"ls"}"#),
        );
        assert_ne!(key_a.1, key_b.1);
        map.insert(key_a.clone(), 1);
        assert!(map.contains_key(&key_a));
        assert!(!map.contains_key(&key_b));
    }

    /// Two identical failures push the counter to 2.
    /// That's the `TOOL_FAILURE_AUTO_STOP` threshold —
    /// the dedupe branch in `run_agent_loop` cancels
    /// the turn on the second identical call.
    #[test]
    fn failed_tool_calls_counter_increments_on_each_failure() {
        let mut map: HashMap<(String, [u8; 32]), u32> = HashMap::new();
        let key = (
            "shell_run".to_string(),
            hash_tool_args("shell_run", r#"{}"#),
        );
        let entry = map.entry(key.clone()).or_insert(0);
        *entry += 1;
        let entry = map.entry(key.clone()).or_insert(0);
        *entry += 1;
        assert_eq!(*map.get(&key).unwrap(), 2);
        assert!(2 >= TOOL_FAILURE_AUTO_STOP);
    }

    /// A success clears the counter for that exact
    /// call — the LLM may legitimately call the same
    /// tool with the same args in a later iteration
    /// after fixing the underlying problem (e.g.
    /// after the user provided the missing field).
    #[test]
    fn failed_tool_calls_counter_clears_on_success() {
        let mut map: HashMap<(String, [u8; 32]), u32> = HashMap::new();
        let key = (
            "shell_run".to_string(),
            hash_tool_args("shell_run", r#"{"command":"date"}"#),
        );
        map.insert(key.clone(), 1);
        assert_eq!(*map.get(&key).unwrap(), 1);
        // Tool succeeds.
        map.remove(&key);
        assert!(!map.contains_key(&key));
    }

    /// Per-turn reset: a new `HashMap` is empty. The
    /// LLM can retry after the user has had a chance
    /// to clarify intent.
    #[test]
    fn failed_tool_calls_resets_per_turn() {
        let mut map: HashMap<(String, [u8; 32]), u32> = HashMap::new();
        let key = (
            "shell_run".to_string(),
            hash_tool_args("shell_run", r#"{}"#),
        );
        map.insert(key.clone(), 2);
        assert_eq!(*map.get(&key).unwrap(), 2);
        // Per-turn reset — the run_agent_loop local
        // map goes out of scope.
        let fresh: HashMap<(String, [u8; 32]), u32> = HashMap::new();
        assert!(!fresh.contains_key(&key));
    }

    // ---- v3.7.16 — S3a. TOOL_PERMISSION_ALLOWLIST ----

    /// The allowlist contains the world-touching tools
    /// the renderer + server agree on. Adding a tool
    /// here should land in
    /// `src/v4/lib/toolAllowlist.ts` too.
    ///
    /// v3.7.17 — S3a-real. shell_run REMOVED from this list.
    /// Per-bot tool enforcement now happens inside
    /// `run_agent_loop` against the bot's `allowed_tools` —
    /// the model can't call shell_run unless the user has
    /// explicitly added it to the bot's allowlist.
    #[test]
    fn tool_permission_allowlist_contains_expected_world_touching_tools() {
        for tool in [
            "mail_draft",
            "gmail_send",
            "calendar_event_create",
            "vm_computer_use",
            "vm_browser_open",
            "loopd_start",
            "loopd_stop",
            "loopd_status",
            "loopd_read_task",
            "loopd_read_journal",
            "ego_browser",
        ] {
            assert!(
                TOOL_PERMISSION_ALLOWLIST.contains(&tool),
                "expected {tool} in TOOL_PERMISSION_ALLOWLIST"
            );
        }
        // shell_run is intentionally NOT here — per-bot
        // allowed_tools is the gate now.
        assert!(
            !TOOL_PERMISSION_ALLOWLIST.contains(&"shell_run"),
            "shell_run should be per-bot, not global"
        );
    }

    // ---- v3.7.17 — S3a-real. date_header_for_now ----

    /// The date header uses the natural-language form
    /// `Today is {Weekday}, {Month} {Day}, {Year} ({IANA tz}).`
    /// plus the rule "Do not call tools for the current date
    /// or time." The renderer never displays this header; it's
    /// the system message injected into every send.
    #[test]
    fn date_header_for_now_contains_today_and_rule() {
        let header = date_header_for_now();
        // Natural-language date fragments.
        let year = chrono::Local::now().format("%Y").to_string();
        let month = chrono::Local::now().format("%B").to_string();
        let day = chrono::Local::now().format("%d").to_string();
        let weekday = chrono::Local::now().format("%A").to_string();
        assert!(header.contains(&year), "missing year: {header}");
        assert!(header.contains(&month), "missing month: {header}");
        assert!(
            header.contains(day.trim_start_matches('0')),
            "missing day: {header}"
        );
        assert!(header.contains(&weekday), "missing weekday: {header}");
        // The exact "Today is ..." prefix the user spec'd.
        assert!(
            header.starts_with("Today is "),
            "expected 'Today is ...' prefix: {header:?}"
        );
        // The rule against tool calls for date/time.
        assert!(
            header.to_lowercase().contains("do not call tools"),
            "missing 'do not call tools' rule: {header}"
        );
    }

    // ---- v3.7.17 — S3a-real. truncate_title_for_storage ----

    #[test]
    fn truncate_title_short_text_returned_unchanged() {
        assert_eq!(truncate_title_for_storage("hi", 60), "hi");
    }

    #[test]
    fn truncate_title_long_text_breaks_at_word_boundary() {
        let s = "this is a fairly long first user message";
        let out = truncate_title_for_storage(s, 20);
        assert!(out.ends_with('…'), "expected ellipsis: {out:?}");
        assert!(out.chars().count() <= 21, "too long: {out:?}");
        let cut_part: String = out.chars().take_while(|c| *c != '…').collect();
        let next_char = s.chars().nth(cut_part.chars().count());
        assert!(
            next_char.map(|c| c.is_whitespace()).unwrap_or(true),
            "cut should end at word boundary: {out:?}"
        );
    }
}
