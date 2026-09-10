//! Bot executor: runs a single bot's agent loop.
//!
//! On a "Run now" or scheduled fire, the executor:
//! 1. Looks up the bot, its schedule, and (if it's a recurring bot) the
//!    last conversation it wrote into.
//! 2. Creates a new conversation (or reuses the existing one for
//!    recurring bots).
//! 3. Builds a system prompt = bot.system_prompt + (optional) inbox
//!    messages from other bots.
//! 4. Runs a small agent loop with the bot's filtered tool registry.
//! 5. Persists a `bot_runs` row with the outcome.
//!
//! The executor shares the chat command's per-user-message cancellation
//! tokens via a `BotRunHandle` so the UI can stop a running bot.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use futures_util::StreamExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::approvals::queue::{enqueue_if_ask_rule, pending_message};
use crate::bots::{Bot, BotRun, BotRunStatus, registry_for};
use crate::llm::provider::{provider_for_settings, ChatMessage, ChatRequest, Provider};
use crate::llm::stream::{StreamChunk, StreamError};
use crate::storage::{MessageRole, PersistedToolCall};
use crate::tools::registry::ToolRegistry;
use crate::tools::tool::{ToolContext, ToolInvocation};
use crate::AppState;

const MAX_BOT_ITERATIONS: u32 = 5;

#[derive(Serialize, Clone)]
struct BotChunkEvent {
    bot_id: String,
    bot_run_id: String,
    conversation_id: String,
    chunk: StreamChunk,
}

#[derive(Serialize, Clone)]
struct BotDoneEvent {
    bot_id: String,
    bot_run_id: String,
    conversation_id: String,
    result_summary: String,
}

#[derive(Serialize, Clone)]
struct BotErrorEvent {
    bot_id: String,
    bot_run_id: String,
    conversation_id: String,
    message: String,
}

/// v3.6.0 (Phase 7) — Reflect-step event. The executor
/// emits this on the Tauri bus every time the reflect
/// step writes a new fact or preference to the Bot's
/// memory file. The ActivityFeed listens for it and
/// surfaces a small inline pill; the user can dismiss
/// the pill, which calls `memory_forget` to roll back
/// the write. `kind` is `"fact"` or `"preference"` —
/// History is intentionally not auto-written, so it
/// never appears here.
#[derive(Serialize, Clone)]
struct BotMemoryWriteEvent {
    bot_id: String,
    bot_run_id: String,
    kind: String,
    key: String,
    content: String,
}

/// Result of a single bot run.
#[derive(Debug, Clone, Serialize)]
pub struct BotRunOutput {
    pub run_id: String,
    pub conversation_id: String,
    pub status: BotRunStatus,
    pub result_summary: String,
}

/// Run a bot once. Used by both the "Run now" UI command and the
/// scheduler. Returns once the bot has finished or failed; the caller
/// can wait on the returned `BotRunOutput` to know the outcome.
///
/// Cancellation: the executor registers the `cancel` parameter with
/// the `state.bot_runs` registry, keyed by the freshly-generated
/// `run_id`. External callers (the UI Stop button via `stop_bot_run`)
/// fire that same token by run_id. The token is unregistered on
/// completion so the registry doesn't grow unbounded.
///
/// `recording_id` is an optional v2.2 Skill-recording hook. When
/// `Some`, every tool call dispatched inside the agent loop is
/// pushed into the shared `RecorderState` (keyed by `recording_id`)
/// so the caller can later `recorder.stop(recording_id)` to drain
/// the captured tool calls into a candidate Skill. Passing `None`
/// preserves the pre-v2.2 behavior exactly.
///
/// `existing_conversation_id` (v2.6.2) lets the caller pin the run
/// to a specific conversation instead of letting the executor
/// create a new one (or reuse a scheduled bot's last conversation).
/// `None` preserves the pre-v2.6.2 behavior. `Some(id)` is the
/// v2.6.2 auto-resume path: after the user Approves a tool call,
/// the Bot picks up where it left off in the same conversation —
/// no new chat tab, no lost history.
///
/// `triggered_by` (v3.1.0) stamps the entry point on the
/// `bot_runs` row so the ActivityFeed can show "via daemon" /
/// "via webhook" / "via app" badges. Defaults to `"app"` for any
/// caller that still passes `None` (preserves pre-v3.1.0
/// behavior). The daemon path passes `"daemon"` (scheduler) or
/// `"webhook"` (POST handler).
pub async fn run_bot_once(
    app: Option<AppHandle>,
    state: Arc<AppState>,
    bot: Bot,
    cancel: CancellationToken,
    recording_id: Option<String>,
    existing_conversation_id: Option<String>,
    triggered_by: Option<&'static str>,
) -> BotRunOutput {
    let triggered_by = triggered_by.unwrap_or("app");
    let run_id = Uuid::new_v4().to_string();
    state.bot_runs.register(run_id.clone(), cancel.clone()).await;
    let started_at = Utc::now();
    // Build the full registry (built-in tools + MCP tools) and then
    // apply the bot's allowed_tools filter so the bot only sees the
    // tools its owner has granted.
    let full_tool_registry = Arc::new(ToolRegistry::default_with_extras(
        state.mcp.tool_adapters(),
    ));

    // 1. Reuse the previous conversation for recurring bots, or create
    //    a fresh one for one-off runs. v2.6.2: if the caller pinned us
    //    to a specific conversation (the auto-resume path), honor
    //    that — verify it still exists, otherwise fall through to
    //    the default behavior.
    let conversation = if let Some(pinned) = existing_conversation_id.clone() {
        let exists = state
            .db
            .list_conversations()
            .ok()
            .map(|list| list.iter().any(|c| c.id == pinned))
            .unwrap_or(false);
        if exists {
            let _ = state.db.touch_conversation(&pinned);
            ConversationRef::Existing(pinned)
        } else {
            // Pinned id vanished between the approval-decide
            // call and now. Fall through to "create fresh" so
            // the user still gets a result rather than a silent
            // failure. The synthetic tool message they just
            // appended is in the (now orphan) conversation —
            // it'll be visible in the history but unreachable
            // from the new conversation. Best-effort.
            let convo = state
                .db
                .create_bot_conversation(&bot.id, Some(format!("{} — run", bot.name)))
                .unwrap_or_else(|_| {
                    state
                        .db
                        .create_conversation(Some(format!("{} — run", bot.name)), Some(&bot.id))
                        .unwrap()
                });
            ConversationRef::Fresh(convo)
        }
    } else if let Some(schedule) = state
        .db
        .get_schedule(&bot.id)
        .ok()
        .flatten()
    {
        if let Some(existing_id) = schedule.last_conversation_id.clone() {
            // Verify the conversation still exists.
            let exists = state
                .db
                .list_conversations()
                .ok()
                .map(|list| list.iter().any(|c| c.id == existing_id))
                .unwrap_or(false);
            if exists {
                state
                    .db
                    .touch_conversation(&existing_id)
                    .ok();
                ConversationRef::Existing(existing_id)
            } else {
                let convo = state
                    .db
                    .create_bot_conversation(&bot.id, Some(format!("{} — run", bot.name)))
                    .unwrap_or_else(|_| {
                        state
                            .db
                            .create_conversation(Some(format!("{} — run", bot.name)), Some(&bot.id))
                            .unwrap()
                    });
                ConversationRef::Fresh(convo)
            }
        } else {
            let convo = state
                .db
                .create_bot_conversation(&bot.id, Some(format!("{} — run", bot.name)))
                .unwrap_or_else(|_| {
                    state
                        .db
                        .create_conversation(Some(format!("{} — run", bot.name)), Some(&bot.id))
                        .unwrap()
                });
            ConversationRef::Fresh(convo)
        }
    } else {
        let convo = state
            .db
            .create_bot_conversation(&bot.id, Some(format!("{} — run", bot.name)))
            .unwrap_or_else(|_| {
                state
                    .db
                    .create_conversation(Some(format!("{} — run", bot.name)), Some(&bot.id))
                    .unwrap()
            });
        ConversationRef::Fresh(convo)
    };

    let conversation_id = match &conversation {
        ConversationRef::Existing(id) => id.clone(),
        ConversationRef::Fresh(c) => c.id.clone(),
    };

    // Insert the initial run record (Running).
    let mut run = BotRun {
        id: run_id.clone(),
        bot_id: bot.id.clone(),
        conversation_id: conversation_id.clone(),
        status: BotRunStatus::Running,
        started_at,
        finished_at: None,
        result_summary: String::new(),
        triggered_by: triggered_by.to_string(),
    };
    let _ = state.db.upsert_bot_run(&run);

    // 2. Build the LLM provider from the global settings.
    let mut settings = match state.db.load_settings() {
        Ok(s) => s,
        Err(e) => {
            return fail_run(
                app.as_ref(),
                &state,
                &mut run,
                format!("settings load failed: {e}"),
            );
        }
    };
    // v3.7.3 — LLM key override. The Mac app pushes its
    // `Settings.minimax_api_key` to the daemon on launch
    // and on every Settings save (via `POST /settings`).
    // The daemon stores the pushed key in memory only
    // (never persisted to disk) and shares it into the
    // executor's `AppState.llm_key_override`. If the
    // override is `Some`, it takes precedence over the
    // DB-loaded `settings.minimax_api_key` so a key
    // change on the Mac app reaches the daemon-driven
    // run without a DB copy. The Mac app's own runs
    // (Tauri process) initialize the lock to `None`, so
    // this branch is daemon-only.
    if let Ok(guard) = state.llm_key_override.read() {
        if let Some(k) = guard.as_ref() {
            if !k.is_empty() {
                settings.minimax_api_key = Some(k.clone());
            }
        }
    }
    let api_key = match settings.minimax_api_key.clone() {
        Some(k) if !k.is_empty() => k,
        _ => {
            // v3.7.3 — Distinct error when the daemon
            // has no key. The Mac app sets this on
            // launch + on every Settings save; if the
            // daemon was started before the Mac app
            // booted, the Mac app's launch-time push
            // fills it in. If neither side has a key,
            // the Mac app is the only one that can fix
            // it — surface that.
            let msg = if app.is_none() {
                "maxbotd: no LLM key configured; Mac app must POST /settings".to_string()
            } else {
                "set your MiniMax API key in Settings first".to_string()
            };
            return fail_run(app.as_ref(), &state, &mut run, msg);
        }
    };
    // Build the provider from the active settings (`provider_kind` +
    // per-provider key + base URL). Bots inherit the global provider
    // config; only the model id is per-bot.
    let provider: Arc<dyn Provider> = match provider_for_settings(&settings) {
        Ok(p) => p,
        Err(e) => {
            return fail_run(app.as_ref(), &state, &mut run, e);
        }
    };
    // Model precedence: bot.default_model > settings.default_model >
    // provider's built-in default.
    let model = if !bot.default_model.trim().is_empty() {
        bot.default_model.clone()
    } else if !settings.default_model.trim().is_empty() {
        settings.default_model.clone()
    } else {
        provider.default_model().to_string()
    };
    // Keep the api_key binding alive so the compiler doesn't warn about
    // an unused variable — the per-provider factory now picks the right
    // key from settings, so we don't reference `api_key` directly.
    let _ = api_key;

    // 3. Build the bot's tool registry (filtered to its allowlist).
    let bot_registry = registry_for(&bot, &full_tool_registry);
    let tool_definitions = bot_registry.definitions();

    // 4. Compose the system prompt: bot.system_prompt + (optionally) any
    //    unread inter-agent messages in its inbox.
    let inbox = state
        .db
        .list_inbox(&bot.id, false)
        .ok()
        .unwrap_or_default();
    let inbox_section = if inbox.is_empty() {
        String::new()
    } else {
        let bodies: Vec<String> = inbox
            .iter()
            .map(|m| {
                // `__user__` is the sentinel for messages that the
                // human user sent from the chat composer. Show them as
                // "user" in the prompt so the model knows to address
                // the human by name.
                let sender = if m.from_bot_id == "__user__" {
                    "user"
                } else {
                    m.from_bot_id.as_str()
                };
                format!(
                    "[{}] {}: {}",
                    m.created_at.format("%Y-%m-%dT%H:%M:%SZ"),
                    sender,
                    m.body
                )
            })
            .collect();
        format!(
            "\n\nYou have {} unread message(s) from other bots:\n{}",
            inbox.len(),
            bodies.join("\n")
        )
    };
    // 4a. Append the bot's persistent memory (agents.md). The file
    //     lives at <app_data_dir>/bots/<id>/agents.md and is
    //     auto-seeded from the SQLite system_prompt on first run.
    //     Soft-warn on read failure so a transient FS error doesn't
    //     break a bot run.
    //
    //     v2.8.0 — daemon (no Tauri AppHandle) skips this
    //     step. The agents.md file lives on the Mac's app
    //     data dir, not the server. The daemon's bot runs
    //     fall back to `bot.system_prompt` only. If the
    //     user later wants the daemon to honor agents.md,
    //     they'll need a synced copy (out of scope for
    //     v2.8).
    let agents_md_section = match app
        .as_ref()
        .and_then(|a| crate::bots::filesystem::bot_dir(a, &bot.id).ok())
        .and_then(|dir| crate::bots::filesystem::read_agents_md(&dir, &bot.system_prompt).ok())
    {
        Some(s) if s.trim().is_empty() => String::new(),
        Some(s) => format!("\n\n# Long-term memory (agents.md)\n\n{}", s),
        None => {
            log::warn!(
                "executor: could not read agents.md for bot {} (app={} or read failed)",
                bot.id,
                app.is_some()
            );
            String::new()
        }
    };

    let system_prompt = format!(
        "{}{}{}{}",
        bot.system_prompt,
        inbox_section,
        agents_md_section,
        // v2.5.0 — Persistent memory: inject a small slice of
        // relevant memories at turn start. Top-5 keyword
        // matches against the kickoff prompt (or, if a real
        // user message is present in the history, the most
        // recent user message). Read failures (no VM, slow
        // SFTP, parse error) collapse to an empty Vec — the
        // section becomes "" and the prompt stays clean.
        build_memory_section(
            &state.computer.ssh_pool(),
            &bot.id,
            &conversation_id,
            &state,
        )
        .await,
    );

    // 5. Persist the system message as the first turn in the conversation
    //    (if not already there). Reusing a recurring conversation: the
    //    system message from the previous run is still present, so we
    //    only insert a fresh one for new conversations.
    let is_new_conversation = matches!(conversation, ConversationRef::Fresh(_));
    if is_new_conversation {
        let system_msg = ChatMessage::System {
            content: system_prompt.clone(),
        };
        persist_message(&state, &conversation_id, MessageRole::System, &system_msg);
    }

    // Mark inbox messages as read since the bot is now processing them.
    let _ = state.db.mark_bot_messages_read(&bot.id);

    // 6. Insert the user-style kickoff message (the "what to do" prompt).
    //    v2.6.2 — skip the kickoff on the auto-resume path: the
    //    caller just appended a synthetic `role=tool` message and
    //    wants the LLM to continue from there, not to see another
    //    "Run tick — proceed with your task." line interrupting
    //    the flow.
    let is_auto_resume = existing_conversation_id.is_some();
    if !is_auto_resume {
        let kickoff = "Run tick — proceed with your task.";
        let kickoff_msg = ChatMessage::User {
            content: kickoff.to_string(),
        };
        persist_message(&state, &conversation_id, MessageRole::User, &kickoff_msg);
    }

    // 7. Build the conversation history for the first turn.
    let history = state
        .db
        .list_messages(&conversation_id)
        .ok()
        .unwrap_or_default();
    let mut messages: Vec<ChatMessage> = history
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
        .collect();

    // 8. The agent loop. Same shape as the user chat loop, but auto-
    //    approves consent (bots are autonomous) and uses the bot's
    //    tool registry.
    let mut iteration: u32 = 0;
    let mut final_text = String::new();
    let mut hit_error: Option<String> = None;
    while iteration < MAX_BOT_ITERATIONS {
        if cancel.is_cancelled() {
            hit_error = Some("cancelled".to_string());
            break;
        }
        iteration += 1;

        // Insert a fresh assistant placeholder for this iteration.
        let assistant_msg = match state
            .db
            .insert_message(&conversation_id, MessageRole::Assistant, "", &[])
        {
            Ok(m) => m,
            Err(e) => {
                hit_error = Some(format!("DB insert failed: {e}"));
                break;
            }
        };
        let assistant_id = assistant_msg.id;

        let request = ChatRequest {
            model: model.clone(),
            messages: messages.clone(),
            tools: tool_definitions.clone(),
            temperature: 1.0,
        };
        let stream = match provider.stream(request).await {
            Ok(s) => s,
            Err(e) => {
                hit_error = Some(e.to_string());
                break;
            }
        };
        futures_util::pin_mut!(stream);
        let mut full_text = String::new();
        let mut stream_error: Option<StreamError> = None;
        let mut by_id: std::collections::HashMap<String, PersistedToolCall> =
            std::collections::HashMap::new();

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    break;
                }
                // Per-chunk timeout — mirrors the chat command so a
                // hung bot run surfaces an error instead of spinning
                // forever. The user can stop a stalled run via the
                // BotsPanel Run→Stop toggle.
                next = tokio::time::timeout(crate::commands::chat::STREAM_CHUNK_TIMEOUT, stream.next()) => {
                    let item = match next {
                        Ok(Some(item)) => item,
                        Ok(None) => break, // stream ended cleanly
                        Err(_elapsed) => {
                            stream_error = Some(StreamError::Network(
                                "stream stalled — no response for 90s".to_string(),
                            ));
                            break;
                        }
                    };
                    match item {
                        Ok(StreamChunk::Text { delta }) => {
                            full_text.push_str(&delta);
                            let _ = state.db.append_message_content(&assistant_id, &delta);
                            // v2.8.0 — daemon (no AppHandle) skips
                            // the live chunk event. The bot_runs
                            // row + the persisted assistant
                            // message are the durable record;
                            // the Mac app sees the result via
                            // ActivityFeed on next open / poll.
                            if let Some(a) = app.as_ref() {
                                let _ = a.emit(
                                    "bot://chunk",
                                    BotChunkEvent {
                                        bot_id: bot.id.clone(),
                                        bot_run_id: run_id.clone(),
                                        conversation_id: conversation_id.clone(),
                                        chunk: StreamChunk::Text { delta },
                                    },
                                );
                            }
                        }
                        Ok(StreamChunk::ToolCallDelta { id, name, arguments_delta }) => {
                            let entry = by_id.entry(id.clone()).or_insert_with(|| PersistedToolCall {
                                id: id.clone(),
                                name: String::new(),
                                arguments: String::new(),
                            });
                            if let Some(n) = name.clone() { entry.name = n; }
                            if let Some(args) = arguments_delta.clone() { entry.arguments.push_str(&args); }
                            if let Some(a) = app.as_ref() {
                                let _ = a.emit(
                                    "bot://chunk",
                                    BotChunkEvent {
                                        bot_id: bot.id.clone(),
                                        bot_run_id: run_id.clone(),
                                        conversation_id: conversation_id.clone(),
                                        chunk: StreamChunk::ToolCallDelta { id, name, arguments_delta },
                                    },
                                );
                            }
                        }
                        Ok(StreamChunk::Done { .. }) => {
                            break;
                        }
                        Err(e) => {
                            stream_error = Some(e);
                            break;
                        }
                    }
                }
            }
        }

        // Persist accumulated tool_calls on the assistant turn.
        let tool_calls_collected: Vec<PersistedToolCall> =
            by_id.into_values().collect::<Vec<_>>();
        if !tool_calls_collected.is_empty() {
            let _ = state
                .db
                .append_tool_calls(&assistant_id, &tool_calls_collected, &conversation_id);
        }

        if let Some(e) = stream_error {
            hit_error = Some(e.to_string());
            break;
        }

        // No tool calls → done with this iteration.
        if tool_calls_collected.is_empty() {
            final_text = full_text;
            break;
        }

        // 9. Run the tool calls. Bots are autonomous: consent is auto-
        //    granted (the user already consented by enabling the tool in
        //    the bot's allowlist). The `message_bot` tool is special: it
        //    routes the message through the DB instead of running shell.
        //
        //    v2.6.0 — Approval gate. Before invoking a tool
        //    (or `message_bot`), we ask the approvals queue
        //    whether the Bot's rule for that tool is
        //    `auto`/`ask`/`deny`. `auto` runs the tool
        //    immediately. `ask` enqueues an approval and
        //    returns a "pending (id=...)" string to the
        //    LLM so the reasoning loop can continue. `deny`
        //    returns an error to the LLM. The user clicks
        //    Approve / Reject / Edit in the queue; the
        //    actual tool runs then (v2.6.1 will auto-resume
        //    the Bot; for v2.6 the user follows up
        //    manually).
        for tc in &tool_calls_collected {
            if cancel.is_cancelled() {
                break;
            }
            let resolved_args = parse_tool_args(&tc.arguments);
            // v3.4.0 (Phase 5) — derive a short
            // human-readable reason for the approval
            // row. Rule-based: "sending email" /
            // "writing file" / etc. Falls through to a
            // generic "tool {name} requires approval"
            // for tools we don't have a friendlier
            // phrase for. The reason is a best-effort
            // UX aid — the audit log surfaces it as
            // "Why this asked".
            let reason = reason_for_tool(&tc.name, &resolved_args);
            // Step 1 — approval gate. The gate runs for
            // every tool, including `message_bot`, so the
            // user can flip the per-Bot rule on
            // cross-Bot pings too.
            match enqueue_if_ask_rule(
                &state,
                &bot.id,
                &tc.name,
                &resolved_args,
                Some(&run_id),
                Some(&tc.id),
                Some(&reason),
            )
            .await
            {
                Ok(None) => {
                    // Auto — proceed.
                }
                Ok(Some(approval_id)) => {
                    // Ask — enqueue and tell the LLM the
                    // call is gated. The Bot's loop
                    // continues; the user decides
                    // separately. We persist the
                    // placeholder as a `tool` message so
                    // the conversation log shows the
                    // pending state and the LLM has
                    // something to reason about on the
                    // next turn.
                    let content = pending_message(&approval_id);
                    state.recorder.record(
                        recording_id.as_deref(),
                        crate::skills::recorder::make_recorded_step(
                            tc.name.clone(),
                            resolved_args,
                            content.clone(),
                            false,
                        ),
                    );
                    let _ = state.db.insert_message(
                        &conversation_id,
                        MessageRole::Tool,
                        &content,
                        &[],
                    );
                    messages.push(ChatMessage::Tool {
                        tool_call_id: tc.id.clone(),
                        content,
                    });
                    continue;
                }
                Err(deny_msg) => {
                    // Deny — surface to the LLM as a
                    // tool error.
                    let content = format!("[error] {deny_msg}");
                    state.recorder.record(
                        recording_id.as_deref(),
                        crate::skills::recorder::make_recorded_step(
                            tc.name.clone(),
                            resolved_args,
                            content.clone(),
                            true,
                        ),
                    );
                    let _ = state.db.insert_message(
                        &conversation_id,
                        MessageRole::Tool,
                        &content,
                        &[],
                    );
                    messages.push(ChatMessage::Tool {
                        tool_call_id: tc.id.clone(),
                        content,
                    });
                    continue;
                }
            }
            let (content, is_error) = if tc.name == "message_bot" {
                handle_message_bot(&state, &bot, tc)
            } else {
                match bot_registry
                    .execute(
                        ToolInvocation {
                            name: tc.name.clone(),
                            id: tc.id.clone(),
                            arguments: resolved_args.clone(),
                            bot_id: Some(bot.id.clone()),
                        },
                        ToolContext {
                            consent_granted: true,
                            consent_prompt: None,
                            app: app.clone(),
                        },
                    )
                    .await
                {
                    Ok(r) => (r.content, r.is_error),
                    Err(e) => (format!("[error] {}", e), true),
                }
            };
            // v2.2.0 — Skill recording. When a recording is
            // active, push this tool dispatch into the
            // recorder. We capture the resolved args (after
            // any model-side synthesis, before the tool
            // mutated them) so the recorded Skill is
            // replayable. A `None` `recording_id` is a no-op.
            state.recorder.record(
                recording_id.as_deref(),
                crate::skills::recorder::make_recorded_step(
                    tc.name.clone(),
                    resolved_args,
                    content.clone(),
                    is_error,
                ),
            );
            // v3.4.0 (Phase 5) — Takeover for 2FA /
            // CAPTCHA. The plan's decision is option (b):
            // the LLM self-reports `needs_human` in the
            // tool's JSON return. We try to parse the
            // content as JSON; if it has a
            // `needs_human` string field, we treat the
            // tool return as a "pause and ask" signal.
            // The Bot is parked (not failed), a
            // Takeover approval is enqueued, the per-Bot
            // takeover state row is upserted, and we
            // break the inner tool loop. The outer
            // `while iteration < MAX_BOT_ITERATIONS`
            // loop will exit on the next cancel check
            // because we cancel the token below — same
            // path a user-initiated stop takes.
            if let Some(needs_human) = extract_needs_human(&content) {
                let payload = serde_json::json!({
                    "needs_human": needs_human,
                    "tool": tc.name,
                });
                let takeover_reason =
                    format!("{} needs human help: {}", tc.name, needs_human);
                let takeover_id = state
                    .db
                    .enqueue_approval(
                        &bot.id,
                        crate::approvals::APPROVAL_TOOL_TAKEOVER,
                        &payload,
                        Some(&run_id),
                        Some(&tc.id),
                        Some(&takeover_reason),
                    )
                    .ok();
                if let Some(aid) = takeover_id {
                    let _ = state.db.set_bot_takeover_state(
                        &bot.id,
                        "needs_human",
                        &aid,
                        &needs_human,
                        &tc.name,
                    );
                    // Persist the tool result as a
                    // `tool` message so the conversation
                    // log shows the pause.
                    let _ = state.db.insert_message(
                        &conversation_id,
                        MessageRole::Tool,
                        &content,
                        &[],
                    );
                    messages.push(ChatMessage::Tool {
                        tool_call_id: tc.id.clone(),
                        content,
                    });
                    hit_error = Some(format!(
                        "bot paused — takeover needed: {needs_human}"
                    ));
                    // Cancel the run so the outer loop
                    // exits cleanly. The run row is
                    // marked Cancelled by the existing
                    // `if cancel.is_cancelled()` branch
                    // below.
                    cancel.cancel();
                    break;
                }
            }
            // Persist the tool result as a `tool` message.
            let _ = state.db.insert_message(
                &conversation_id,
                MessageRole::Tool,
                &content,
                &[],
            );
            // Append to the next turn's history.
            messages.push(ChatMessage::Tool {
                tool_call_id: tc.id.clone(),
                content,
            });
            let _ = is_error;
        }
    }

    let result_summary = if let Some(err) = hit_error {
        // If we were cancelled, mark the run as Cancelled rather than
        // Failed — different status surfaces differently in the UI.
        if cancel.is_cancelled() {
            run.status = BotRunStatus::Cancelled;
            run.finished_at = Some(Utc::now());
            run.result_summary = first_line(&err, 200);
            let _ = state.db.upsert_bot_run(&run);
            err
        } else {
            fail_run(app.as_ref(), &state, &mut run, err.clone()).result_summary
        }
    } else {
        // Successful run: write a one-line summary.
        let summary = first_line(&final_text, 200);
        run.status = BotRunStatus::Succeeded;
        run.finished_at = Some(Utc::now());
        run.result_summary = summary.clone();
        let _ = state.db.upsert_bot_run(&run);
        // v2.5.0 — auto-write a history entry summarizing
        // the turn. We only do this on success; failed /
        // cancelled runs are intentionally not recorded.
        // The kickoff prompt is the user side; the final
        // assistant text is the bot side. Both are best-
        // effort — `auto_write_history` is non-fatal.
        let user_text = most_recent_user_message(&state, &conversation_id);
        auto_write_history(
            &state.computer.ssh_pool(),
            &bot.id,
            &user_text,
            &final_text,
        )
        .await;
        // v3.6.0 (Phase 7) — Reflect step. After the
        // LLM stream finishes, spawn a short background
        // task that re-prompts the model with the same
        // conversation and asks for 0–2 new facts or
        // preferences. New entries are deduped by `key`
        // against existing memory, then appended to the
        // Bot's on-disk JSONL via the existing
        // `memory::store::append` SFTP path (2-second
        // per-call timeout). Each new entry also emits a
        // `memory:written` Tauri event so the
        // ActivityFeed can surface an inline pill.
        //
        // Best-effort: the task is spawned on the runtime
        // and its result is dropped. If the LLM call
        // fails, the response isn't parseable as JSON,
        // the SFTP write times out, or the Bot has no
        // VM, the run still completes — the reflect
        // failure is logged at `warn` and otherwise
        // invisible.
        let reflect_pool = state.computer.ssh_pool();
        let reflect_provider = provider.clone();
        let reflect_model = model.clone();
        let reflect_bot_id = bot.id.clone();
        let reflect_run_id = run_id.clone();
        let reflect_conv_id = conversation_id.clone();
        let reflect_app = app.clone();
        let reflect_state = state.clone();
        tokio::spawn(async move {
            reflect_and_write_memory(
                reflect_app,
                reflect_state,
                reflect_provider,
                reflect_model,
                reflect_pool,
                reflect_bot_id,
                reflect_run_id,
                reflect_conv_id,
            )
            .await;
        });
        summary
    };

    // Drop the cancel token from the registry so the next call to
    // stop_bot_run for this id is a no-op. The token itself is
    // dropped at the end of the function.
    state.bot_runs.unregister(&run_id).await;

    // Update the schedule's last_run_at + last_conversation_id so
    // recurring runs continue in the same thread.
    if let Ok(Some(mut schedule)) = state.db.get_schedule(&bot.id) {
        schedule.last_run_at = Some(Utc::now());
        schedule.last_conversation_id = Some(conversation_id.clone());
        let _ = state.db.upsert_schedule(&schedule);
    }

    // Emit the final bot://done event for the UI. v2.8.0:
    // the daemon (no AppHandle) skips this and the bot_runs
    // row is the source of truth.
    if let Some(a) = app.as_ref() {
        let _ = a.emit(
            "bot://done",
            BotDoneEvent {
                bot_id: bot.id.clone(),
                bot_run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
                result_summary: result_summary.clone(),
            },
        );
    }

    BotRunOutput {
        run_id,
        conversation_id,
        status: run.status,
        result_summary,
    }
}

#[derive(Debug, Clone)]
enum ConversationRef {
    Fresh(crate::storage::Conversation),
    Existing(String),
}

fn fail_run(
    app: Option<&AppHandle>,
    state: &AppState,
    run: &mut BotRun,
    message: String,
) -> BotRunOutput {
    run.status = BotRunStatus::Failed;
    run.finished_at = Some(Utc::now());
    run.result_summary = first_line(&message, 200);
    let _ = state.db.upsert_bot_run(run);
    // v2.8.0 — daemon runs (no AppHandle) skip the
    // Tauri event emit. The bot_runs row is the
    // source of truth; the Mac app sees it via the
    // ActivityFeed on next open / poll.
    if let Some(a) = app {
        let _ = a.emit(
            "bot://error",
            BotErrorEvent {
                bot_id: run.bot_id.clone(),
                bot_run_id: run.id.clone(),
                conversation_id: run.conversation_id.clone(),
                message: message.clone(),
            },
        );
    }
    BotRunOutput {
        run_id: run.id.clone(),
        conversation_id: run.conversation_id.clone(),
        status: BotRunStatus::Failed,
        result_summary: run.result_summary.clone(),
    }
}

fn persist_message(
    state: &AppState,
    conversation_id: &str,
    role: MessageRole,
    msg: &ChatMessage,
) {
    let text = msg.text().unwrap_or("").to_string();
    let _ = state
        .db
        .insert_message(conversation_id, role, &text, &[]);
}

fn parse_tool_args(arguments: &str) -> serde_json::Value {
    serde_json::from_str(arguments).unwrap_or_else(|_| {
        serde_json::Value::String(arguments.to_string())
    })
}

/// Route a `message_bot` tool call into the inter-agent mailbox.
fn handle_message_bot(
    state: &AppState,
    from_bot: &Bot,
    tc: &PersistedToolCall,
) -> (String, bool) {
    let args = parse_tool_args(&tc.arguments);
    let target = args
        .get("to")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let body = args
        .get("body")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let (Some(target), Some(body)) = (target, body) else {
        return (
            r#"missing required arguments: "to" (string) and "body" (string)"#.to_string(),
            true,
        );
    };
    // Look up the target bot to make sure it exists. Otherwise drop with an error.
    match state.db.get_bot(&target) {
        Ok(Some(_)) => {
            let msg = crate::bots::BotMessage {
                id: Uuid::new_v4().to_string(),
                from_bot_id: from_bot.id.clone(),
                to_bot_id: target.clone(),
                body: body.clone(),
                created_at: Utc::now(),
                read: false,
                conversation_id: None,
            };
            match state.db.enqueue_bot_message(&msg) {
                Ok(_) => (
                    format!("message queued for delivery to bot {}: {}", target, body),
                    false,
                ),
                Err(e) => (format!("failed to queue message: {e}"), true),
            }
        }
        Ok(None) => (
            format!("no bot found with id {}", target),
            true,
        ),
        Err(e) => (format!("db error: {e}"), true),
    }
}

fn first_line(text: &str, max: usize) -> String {
    let one_line = text.lines().next().unwrap_or("").trim();
    if one_line.chars().count() <= max {
        one_line.to_string()
    } else {
        let mut out: String = one_line.chars().take(max).collect();
        out.push('…');
        out
    }
}

// ---- v2.5.0 — Persistent memory ------------------------------------
//
// `build_memory_section` is the system-prompt injection point. It
// runs at every Bot turn start, takes a short amount of time
// (~one SFTP round-trip in the worst case), and is intentionally
// non-fatal: a Bot without a VM (or with a slow one) gets an
// empty section, not an error. The hard 2-second per-call
// timeout lives inside `memory::store`.
//
// `auto_write_history` runs at the end of a successful run and
// appends a `history` entry summarizing the turn. No LLM call
// — we just truncate the user message and assistant reply to
// 200 chars each. The executor passes a `&str` for the user
// kickoff and the final assistant text; the helper concatenates
// them and writes one entry.

/// Truncate on a char boundary, not a byte boundary. Mirrors
/// `first_line` but keeps a trailing ellipsis.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max).collect();
        out.push('…');
        out
    }
}

/// Pull the most recent user message from the conversation
/// history. The DB is the source of truth (we may have just
/// persisted the kickoff user message). Returns an empty string
/// when the DB lookup fails — the caller falls back to the
/// kickoff prompt in that case.
fn most_recent_user_message(state: &AppState, conversation_id: &str) -> String {
    let Ok(msgs) = state.db.list_messages(conversation_id) else {
        return String::new();
    };
    msgs.into_iter()
        .rev()
        .find(|m| matches!(m.role, MessageRole::User))
        .map(|m| m.content)
        .unwrap_or_default()
}

/// Build the "## Relevant memories" block for the system prompt.
/// Capped at ~200 tokens (we count chars / 4 as a rough proxy) so
/// the system prompt stays tight. Returns "" on any failure —
/// memory is best-effort.
async fn build_memory_section(
    pool: &std::sync::Arc<crate::computer::ssh::SshPool>,
    bot_id: &str,
    conversation_id: &str,
    state: &AppState,
) -> String {
    // Pick the best query we have. Prefer the most recent
    // real user message; fall back to the kickoff prompt.
    let query = most_recent_user_message(state, conversation_id);
    let query = if query.trim().is_empty() {
        "Run tick — proceed with your task.".to_string()
    } else {
        query
    };
    let entries = crate::memory::store::search(&**pool, bot_id, &query, 5).await;
    if entries.is_empty() {
        return String::new();
    }
    // Rough token cap: 4 chars per token → 800 chars max.
    const CHAR_CAP: usize = 800;
    let mut section = String::from(
        "\n\n## Relevant memories\n\n\
         These are facts, preferences, and recent conversation \
         summaries you've remembered. They survive app restarts.\n",
    );
    let mut used = 0usize;
    for e in &entries {
        // One line per entry, in a stable shape. For history
        // entries the `key` is empty — just render the summary.
        let line = if e.kind == crate::memory::MemKind::History {
            format!("- (history) {}", e.content)
        } else {
            format!("- [{}] {} = {}", e.kind.as_str(), e.key, e.content)
        };
        // +1 for the newline.
        if used + line.len() + 1 > CHAR_CAP {
            break;
        }
        used += line.len() + 1;
        section.push_str(&line);
        section.push('\n');
    }
    section
}

/// Append a `history` entry summarizing a successful turn.
/// First 200 chars of the user message + first 200 chars of the
/// assistant reply. Failure is logged + swallowed — the run
/// already succeeded, so a memory write failure must not flip
/// the run to "failed".
async fn auto_write_history(
    pool: &std::sync::Arc<crate::computer::ssh::SshPool>,
    bot_id: &str,
    user_text: &str,
    assistant_text: &str,
) {
    let user_piece = truncate_chars(user_text.trim(), 200);
    let assistant_piece = truncate_chars(assistant_text.trim(), 200);
    let summary = if user_piece.is_empty() && assistant_piece.is_empty() {
        "(empty turn)".to_string()
    } else if user_piece.is_empty() {
        format!("bot: {assistant_piece}")
    } else if assistant_piece.is_empty() {
        format!("user: {user_piece}")
    } else {
        format!("user: {user_piece}  |  bot: {assistant_piece}")
    };
    let entry = crate::memory::MemEntry {
        kind: crate::memory::MemKind::History,
        key: String::new(),
        content: summary,
        created_at: Utc::now().to_rfc3339(),
    };
    if let Err(e) = crate::memory::store::append(&**pool, bot_id, crate::memory::MemKind::History, entry).await {
        log::warn!(
            "executor: auto-write history failed for bot {bot_id}: {e}"
        );
    }
}

// ---- v3.6.0 (Phase 7) — Reflect step ----
//
// After a successful Bot run, the executor spawns a
// short background task that re-prompts the same LLM
// with the recent conversation and asks for 0–2 new
// facts or preferences. New entries are deduped by
// `key` against existing memory, then appended to the
// on-disk JSONL via `memory::store::append` (2-second
// per-call SFTP timeout, same as the v2.5.0 helpers).
// Each new entry also emits a `memory:written` Tauri
// event so the ActivityFeed can surface an inline
// pill; the user can dismiss the pill, which calls
// `memory_forget` to roll back the write.
//
// Best-effort: the entire path is wrapped in
// `tokio::spawn` from the caller, and every failure
// inside the helper is logged at `warn` and
// otherwise invisible. The Bot's next run is
// unchanged whether the reflect step succeeded,
// partially succeeded (some entries deduped), or
// failed entirely.

/// One entry the reflect LLM can propose. `kind`
/// is normalized to `"fact"` or `"preference"` (we
/// silently drop a `history` proposal — the plan is
/// explicit that History stays explicit). `key` is
/// snake_case; we trim whitespace before dedupe.
#[derive(Debug, Clone, serde::Deserialize)]
struct ReflectProposal {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    content: String,
}

/// Pull the JSON array out of the LLM's reflect
/// response. The model is asked to return pure JSON
/// but in practice wraps it in prose like
/// `Here you go: [{...}]\nLet me know...`. We find
/// the first `[` and the last balanced `]` and
/// parse the slice. Returns an empty Vec on any
/// failure — the reflect step is best-effort.
fn parse_reflect_response(response: &str) -> Vec<ReflectProposal> {
    let trimmed = response.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    // Cheap bracket scan: find the first `[`, then
    // track depth across the rest of the string so
    // strings and escapes inside the JSON don't fool
    // us. This is not a full JSON parser but it's
    // enough to bracket-extract a top-level array.
    let bytes = trimmed.as_bytes();
    let mut start: Option<usize> = None;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'[' {
            start = Some(i);
            break;
        }
    }
    let Some(start) = start else {
        return Vec::new();
    };
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;
    let mut end: Option<usize> = None;
    for (i, b) in bytes.iter().enumerate().skip(start) {
        if escape {
            escape = false;
            continue;
        }
        if in_string {
            match *b {
                b'\\' => escape = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match *b {
            b'"' => in_string = true,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let Some(end) = end else {
        return Vec::new();
    };
    let slice = &trimmed[start..=end];
    let parsed: Result<Vec<ReflectProposal>, _> = serde_json::from_str(slice);
    parsed.unwrap_or_default()
}

/// Map the LLM's `kind` string to a `MemKind`.
/// Returns `None` for anything other than `"fact"`
/// or `"preference"` — History is intentionally not
/// auto-written.
fn reflect_kind_to_mem_kind(kind: &str) -> Option<crate::memory::MemKind> {
    match kind.trim().to_lowercase().as_str() {
        "fact" => Some(crate::memory::MemKind::Fact),
        "preference" => Some(crate::memory::MemKind::Preference),
        _ => None,
    }
}

/// Build the reflect-step system prompt. Short and
/// directive: ask for 0–2 facts or preferences in
/// a specific JSON shape, and explain the rules
/// (snake_case key, dedupe by key, no History).
const REFLECT_SYSTEM_PROMPT: &str = r#"You are the memory-reflect step for an autonomous agent.

Look at the recent conversation and decide if there are 0, 1, or 2 new facts or preferences worth remembering about the user.

Rules:
- "fact" = a stable thing about the user (name, location, role, project name, etc.).
- "preference" = how the user wants things done (formatting, tone, defaults, "always X, never Y").
- Skip if there's nothing new or the user only said ephemeral things ("thanks", "ok", "lol").
- Skip if the conversation is a tool-running flow with no user-stated preferences.
- key MUST be snake_case, ≤ 40 chars, stable across runs ("summary_format", "name", "primary_email").
- content should be a single short sentence.
- History entries are NOT auto-written. Never return kind "history".
- If there are no new facts/preferences, return [].

Respond with ONLY a JSON array. No prose, no code fences.

Examples:
[{"kind":"preference","key":"summary_format","content":"Tyler prefers bullet-point summaries, never paragraphs."}]
[]
"#;

/// Build the reflect-step user message from the last
/// ~6 user/assistant turns. We strip the system
/// message and tool calls to keep the prompt tight —
/// the LLM only needs the conversation's shape, not
/// its full tool trace.
fn build_reflect_user_message(messages: &[crate::llm::provider::ChatMessage]) -> String {
    const MAX_TURNS: usize = 6;
    let mut turns: Vec<(String, String)> = Vec::new();
    for m in messages.iter().rev() {
        match m {
            crate::llm::provider::ChatMessage::User { content } => {
                turns.push(("user".to_string(), truncate_chars(content, 600)));
            }
            crate::llm::provider::ChatMessage::Assistant { content, .. } => {
                if !content.trim().is_empty() {
                    turns.push(("assistant".to_string(), truncate_chars(content, 600)));
                }
            }
            _ => {}
        }
        if turns.len() >= MAX_TURNS * 2 {
            break;
        }
    }
    turns.reverse();
    let mut out = String::from("Recent conversation (most recent last):\n");
    for (role, content) in &turns {
        out.push_str(&format!("\n[{}] {}", role, content));
    }
    out
}

/// Run the reflect step: small LLM call, parse the
/// response, dedupe, write new entries, emit
/// `memory:written` events. Best-effort: every
/// failure is logged and swallowed.
#[allow(clippy::too_many_arguments)]
async fn reflect_and_write_memory(
    app: Option<AppHandle>,
    state: Arc<AppState>,
    provider: Arc<dyn crate::llm::provider::Provider>,
    model: String,
    pool: std::sync::Arc<crate::computer::ssh::SshPool>,
    bot_id: String,
    bot_run_id: String,
    conversation_id: String,
) {
    // Re-load the conversation from the DB (the
    // executor's local `messages` Vec was moved into
    // the request; we need a fresh list here).
    let history = match state.db.list_messages(&conversation_id) {
        Ok(h) => h,
        Err(e) => {
            log::warn!(
                "reflect: could not load history for bot {bot_id}: {e}"
            );
            return;
        }
    };
    let reflect_messages: Vec<crate::llm::provider::ChatMessage> = history
        .iter()
        .map(|m| match m.role {
            crate::storage::MessageRole::System => {
                crate::llm::provider::ChatMessage::System {
                    content: m.content.clone(),
                }
            }
            crate::storage::MessageRole::User => {
                crate::llm::provider::ChatMessage::User {
                    content: m.content.clone(),
                }
            }
            crate::storage::MessageRole::Assistant => {
                crate::llm::provider::ChatMessage::Assistant {
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
                }
            }
            crate::storage::MessageRole::Tool => {
                crate::llm::provider::ChatMessage::Tool {
                    tool_call_id: m
                        .tool_calls
                        .first()
                        .map(|tc| tc.id.clone())
                        .unwrap_or_default(),
                    content: m.content.clone(),
                }
            }
        })
        .collect();

    let user_message = build_reflect_user_message(&reflect_messages);
    let request = crate::llm::provider::ChatRequest {
        model,
        messages: vec![
            crate::llm::provider::ChatMessage::System {
                content: REFLECT_SYSTEM_PROMPT.to_string(),
            },
            crate::llm::provider::ChatMessage::User {
                content: user_message,
            },
        ],
        tools: Vec::new(),
        // Low temperature so the LLM returns the same
        // JSON shape across runs; the reflect step is
        // a deterministic extract, not a generation.
        temperature: 0.0,
    };

    // Bound the reflect LLM call to 30s. The
    // reflect prompt is small and most providers
    // respond in < 5s; 30s is generous enough for a
    // slow first-byte without letting a stuck call
    // pin the spawned task forever.
    let stream = match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        provider.stream(request),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            log::warn!(
                "reflect: LLM stream call failed for bot {bot_id}: {e}"
            );
            return;
        }
        Err(_) => {
            log::warn!(
                "reflect: LLM stream call timed out (30s) for bot {bot_id}"
            );
            return;
        }
    };

    futures_util::pin_mut!(stream);
    let mut text = String::new();
    let mut stream_error: Option<crate::llm::stream::StreamError> = None;
    while let Some(item) = stream.next().await {
        match item {
            Ok(crate::llm::stream::StreamChunk::Text { delta }) => {
                text.push_str(&delta);
            }
            Ok(crate::llm::stream::StreamChunk::Done { .. }) => break,
            Ok(crate::llm::stream::StreamChunk::ToolCallDelta { .. }) => {
                // The reflect prompt asks for no tool
                // calls; ignore any spurious delta.
            }
            Err(e) => {
                stream_error = Some(e);
                break;
            }
        }
    }
    if let Some(e) = stream_error {
        log::warn!(
            "reflect: LLM stream errored mid-flight for bot {bot_id}: {e}"
        );
        return;
    }
    if text.trim().is_empty() {
        return;
    }

    // Parse the response. An empty list is a
    // legitimate "I have nothing to write" answer
    // and the helper returns cleanly.
    let proposals = parse_reflect_response(&text);
    if proposals.is_empty() {
        return;
    }

    // Dedupe by key against existing memory. We
    // re-read both kinds in parallel so a Bot with
    // hundreds of entries doesn't pay a serial
    // round-trip cost. The reads are SFTP — bounded
    // by `SFTP_TIMEOUT` inside the store.
    let pool_for_read = pool.clone();
    let bot_id_for_read = bot_id.clone();
    let existing = tokio::join!(
        crate::memory::store::read_all(
            &*pool_for_read,
            &bot_id_for_read,
            crate::memory::MemKind::Fact,
        ),
        crate::memory::store::read_all(
            &*pool_for_read,
            &bot_id_for_read,
            crate::memory::MemKind::Preference,
        ),
    );
    let mut existing_keys: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for list in [existing.0, existing.1] {
        for e in list {
            if !e.key.is_empty() {
                existing_keys.insert(e.key);
            }
        }
    }

    // Write each new entry. We do them serially so
    // a single SFTP timeout doesn't race with
    // itself on the same file. Each entry is a
    // full-file rewrite inside `append`, so order
    // matters (later writes see the previous
    // entry's content).
    let mut written: Vec<(crate::memory::MemKind, String, String)> = Vec::new();
    for proposal in proposals {
        // Normalize the kind. History is dropped
        // silently — the plan is explicit that
        // History stays explicit.
        let Some(kind) = reflect_kind_to_mem_kind(&proposal.kind) else {
            log::debug!(
                "reflect: dropping proposal with kind `{}` (history not auto-written)",
                proposal.kind
            );
            continue;
        };
        let key = proposal.key.trim().to_string();
        let content = proposal.content.trim().to_string();
        if key.is_empty() || content.is_empty() {
            log::debug!(
                "reflect: dropping proposal with empty key or content"
            );
            continue;
        }
        if key.len() > 64 {
            log::debug!(
                "reflect: dropping proposal with oversized key ({} chars)",
                key.len()
            );
            continue;
        }
        if existing_keys.contains(&key) {
            log::debug!(
                "reflect: skipping duplicate key `{key}` for bot {bot_id}"
            );
            continue;
        }
        let entry = crate::memory::MemEntry {
            kind,
            key: key.clone(),
            content: content.clone(),
            created_at: Utc::now().to_rfc3339(),
        };
        match crate::memory::store::append(&*pool, &bot_id, kind, entry).await {
            Ok(()) => {
                log::info!(
                    "reflect: wrote {} `{key}` for bot {bot_id}",
                    kind.as_str()
                );
                existing_keys.insert(key.clone());
                written.push((kind, key, content));
            }
            Err(e) => {
                log::warn!(
                    "reflect: write failed for {} `{key}` on bot {bot_id}: {e}",
                    kind.as_str()
                );
            }
        }
    }

    // Emit one `memory:written` event per new
    // entry. The renderer listens for this and
    // surfaces an inline pill; the dismiss button
    // on the pill calls `memory_forget` to roll
    // the write back. The event is best-effort —
    // a UI that isn't listening just loses the
    // pill, the on-disk write is still durable.
    if let Some(a) = app.as_ref() {
        for (kind, key, content) in written {
            let _ = a.emit(
                "memory:written",
                BotMemoryWriteEvent {
                    bot_id: bot_id.clone(),
                    bot_run_id: bot_run_id.clone(),
                    kind: kind.as_str().to_string(),
                    key,
                    content,
                },
            );
        }
    }
}

/// `timeout_secs` keeps the run from hanging forever on a misbehaving
/// network call. The default 5 minutes is generous for a single
/// iteration; increase for longer models or heavier tool use.
#[allow(dead_code)]
pub async fn run_with_timeout(
    app: Option<AppHandle>,
    state: Arc<AppState>,
    bot_id: String,
    timeout_secs: u64,
) -> BotRunOutput {
    let bot = match state.db.get_bot(&bot_id) {
        Ok(Some(b)) => b,
        Ok(None) => {
            return BotRunOutput {
                run_id: Uuid::new_v4().to_string(),
                conversation_id: String::new(),
                status: BotRunStatus::Failed,
                result_summary: format!("no bot found with id {bot_id}"),
            };
        }
        Err(e) => {
            return BotRunOutput {
                run_id: Uuid::new_v4().to_string(),
                conversation_id: String::new(),
                status: BotRunStatus::Failed,
                result_summary: format!("db error: {e}"),
            };
        }
    };
    let cancel = CancellationToken::new();
    let cancel_for_timeout = cancel.clone();
    let bot_for_task = bot.clone();
    let app_for_task = app.clone();
    let state_for_task = state.clone();
    let task = tokio::spawn(async move {
        run_bot_once(
            app_for_task,
            state_for_task,
            bot_for_task,
            cancel_for_timeout,
            None,
            None,
            None,
        )
        .await
    });
    match tokio::time::timeout(Duration::from_secs(timeout_secs), task).await {
        Ok(Ok(out)) => out,
        Ok(Err(join)) => BotRunOutput {
            run_id: Uuid::new_v4().to_string(),
            conversation_id: String::new(),
            status: BotRunStatus::Failed,
            result_summary: format!("internal task error: {join}"),
        },
        Err(_) => {
            cancel.cancel();
            BotRunOutput {
                run_id: Uuid::new_v4().to_string(),
                conversation_id: String::new(),
                status: BotRunStatus::Cancelled,
                result_summary: format!("run exceeded {timeout_secs}s timeout"),
            }
        }
    }
}

// ---- v3.4.0 (Phase 5) — Reason + needs_human helpers ----
//
// `reason_for_tool` is the "Why this asked" string
// surfaced in the audit log. Rule-derived by default —
// we look at the tool name + a couple of common args
// and produce a one-liner the user can scan. The cost
// is a single static match; the alternative (an LLM
// call) would defeat the point of the audit log, which
// is fast + always-on.
//
// `extract_needs_human` is the Phase 5 Takeover signal
// parser. Per the brief, the plan's decision was
// option (b): the LLM self-reports
// `needs_human: "<reason>"` in a tool's JSON return.
// The tool's `ToolResult.content` is a `String`; we
// try to parse it as JSON and pull the field. Plain
// (non-JSON) returns are ignored — the spec is
// explicit: only when the LLM self-reports the signal
// does the Bot pause.

/// Build a short human-readable reason for an approval
/// row. The renderer surfaces this in the audit log
/// ("Why this asked") and in the ApprovalQueue's row
/// copy. We keep the cost to a single match and avoid
/// any LLM call.
fn reason_for_tool(tool_name: &str, args: &serde_json::Value) -> String {
    match tool_name {
        "mail_send" => {
            let to = args
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("recipient");
            let subject = args.get("subject").and_then(|v| v.as_str());
            match subject {
                Some(s) if !s.is_empty() => format!("sending email to {to} — {s}"),
                _ => format!("sending email to {to}"),
            }
        }
        "mail_draft" => {
            let to = args
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("recipient");
            format!("drafting email to {to}")
        }
        "file_write" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .unwrap_or("file");
            format!("writing file: {path}")
        }
        "file_read" => "reading file".to_string(),
        "shell_run" => {
            let cmd = args
                .get("cmd")
                .and_then(|v| v.as_str())
                .unwrap_or("command");
            format!("running shell command: {}", first_line(cmd, 80))
        }
        "message_bot" => {
            let to = args
                .get("to")
                .and_then(|v| v.as_str())
                .unwrap_or("another bot");
            format!("messaging bot: {to}")
        }
        "vm_browser_open" => {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("URL");
            format!("opening browser: {url}")
        }
        "vm_computer_use" => {
            "driving the Bot's VM (computer use action)".to_string()
        }
        "coding" | "grok_prompt" => "running coding task".to_string(),
        "run_skill" => {
            let id = args
                .get("skill_id")
                .and_then(|v| v.as_str())
                .unwrap_or("skill");
            format!("running skill: {id}")
        }
        "ego_browser" => "using browser (ego_browser)".to_string(),
        "apple_script" => "running AppleScript".to_string(),
        _ => format!("tool `{tool_name}` requires approval"),
    }
}

/// Parse a tool result's content as JSON and pull the
/// `needs_human` string field. Returns `None` if the
/// content is not JSON, doesn't have a `needs_human`
/// field, or has a non-string value for that field.
/// Quietly swallows all parse errors — the spec is
/// explicit: only when the LLM self-reports the signal
/// does the Bot pause, and an over-eager parser would
/// spuriously pause runs on incidental JSON content.
fn extract_needs_human(content: &str) -> Option<String> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with('{') {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(trimmed).ok()?;
    let s = v.get("needs_human")?.as_str()?;
    if s.trim().is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

// =====================================================================
//  Tests
// =====================================================================
//
// We don't have an LLM in the unit-test path, so the
// interesting Phase 5 paths are tested through the
// data-layer helpers (`reason_for_tool` and
// `extract_needs_human`) directly. The Takeover flow
// is exercised end-to-end in `commands/approvals.rs`
// (the approval-decide path resolves a Takeover
// approval back to "running" + clears the state row).

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reason_for_mail_send_includes_recipient() {
        let r = reason_for_tool(
            "mail_send",
            &json!({ "to": "x@y", "subject": "hi" }),
        );
        assert!(r.contains("x@y"));
        assert!(r.contains("hi"), "subject: {r}");
    }

    #[test]
    fn reason_for_file_write_includes_path() {
        let r = reason_for_tool("file_write", &json!({ "path": "/etc/hosts" }));
        assert!(r.contains("/etc/hosts"), "got {r}");
    }

    #[test]
    fn reason_falls_back_to_generic_for_unknown_tool() {
        let r = reason_for_tool("some_future_tool", &json!({}));
        assert!(r.contains("some_future_tool"), "got {r}");
        assert!(r.contains("approval"), "got {r}");
    }

    #[test]
    fn extract_needs_human_parses_json_signal() {
        let content =
            r#"{"screenshot": "…", "needs_human": "2FA prompt visible"}"#;
        assert_eq!(
            extract_needs_human(content).as_deref(),
            Some("2FA prompt visible"),
        );
    }

    #[test]
    fn extract_needs_human_ignores_plain_text() {
        // Per the spec, a plain-text return must NOT
        // pause the bot. Only an explicit JSON
        // `needs_human` field triggers Takeover.
        assert!(extract_needs_human("screenshot taken successfully").is_none());
    }

    #[test]
    fn extract_needs_human_ignores_json_without_field() {
        let content = r#"{"screenshot": "…", "url": "https://x"}"#;
        assert!(extract_needs_human(content).is_none());
    }

    #[test]
    fn extract_needs_human_ignores_empty_string() {
        let content = r#"{"needs_human": "   "}"#;
        assert!(extract_needs_human(content).is_none());
    }

    #[test]
    fn extract_needs_human_ignores_non_string_value() {
        let content = r#"{"needs_human": 42}"#;
        assert!(extract_needs_human(content).is_none());
    }

    // ---- v3.6.0 (Phase 7) — Reflect-step helpers ----
    //
    // These tests cover the small, pure helpers that
    // shape the reflect-step LLM response. The
    // `reflect_and_write_memory` async path is
    // exercised end-to-end by the executor's existing
    // tests; the helpers below are the unit-testable
    // surface that the LLM call hands off to.

    #[test]
    fn parse_reflect_response_accepts_pure_json_array() {
        let resp = r#"[{"kind":"preference","key":"summary_format","content":"bullet points, never paragraphs"}]"#;
        let v = parse_reflect_response(resp);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, "preference");
        assert_eq!(v[0].key, "summary_format");
        assert!(v[0].content.contains("bullet"));
    }

    #[test]
    fn parse_reflect_response_strips_surrounding_prose() {
        // The LLM is asked for JSON but in practice
        // wraps it. Make sure the bracket-scan finds
        // the array even when there's prose around it.
        let resp = "Sure! Here you go:\n\n[{\"kind\":\"fact\",\"key\":\"name\",\"content\":\"Tyler\"}]\n\nLet me know.";
        let v = parse_reflect_response(resp);
        assert_eq!(v.len(), 1, "got: {v:?}");
        assert_eq!(v[0].key, "name");
    }

    #[test]
    fn parse_reflect_response_handles_multiple_entries() {
        let resp = r#"[
            {"kind":"fact","key":"name","content":"Tyler"},
            {"kind":"preference","key":"tone","content":"friendly"}
        ]"#;
        let v = parse_reflect_response(resp);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].key, "name");
        assert_eq!(v[1].key, "tone");
    }

    #[test]
    fn parse_reflect_response_handles_escaped_quotes() {
        // The bracket scan must not be fooled by a
        // `"` inside a JSON string value.
        let resp = r#"[{"kind":"fact","key":"name","content":"Tyler \"T\""}]"#;
        let v = parse_reflect_response(resp);
        assert_eq!(v.len(), 1);
        assert!(v[0].content.contains("Tyler"));
    }

    #[test]
    fn parse_reflect_response_returns_empty_on_garbage() {
        assert!(parse_reflect_response("").is_empty());
        assert!(parse_reflect_response("not json at all").is_empty());
        assert!(parse_reflect_response("[]").is_empty());
        // Unbalanced bracket — extract fails, the
        // whole response is treated as a no-op.
        assert!(parse_reflect_response("[{\"key\":\"a\"}").is_empty());
    }

    #[test]
    fn reflect_kind_maps_known_strings() {
        assert_eq!(
            reflect_kind_to_mem_kind("fact"),
            Some(crate::memory::MemKind::Fact)
        );
        assert_eq!(
            reflect_kind_to_mem_kind("preference"),
            Some(crate::memory::MemKind::Preference)
        );
        // Case-insensitive + tolerant of whitespace.
        assert_eq!(
            reflect_kind_to_mem_kind("  FACT  "),
            Some(crate::memory::MemKind::Fact)
        );
    }

    #[test]
    fn reflect_kind_rejects_history_and_garbage() {
        // History is intentionally not auto-written.
        assert_eq!(reflect_kind_to_mem_kind("history"), None);
        assert_eq!(reflect_kind_to_mem_kind("HISTORY"), None);
        assert_eq!(reflect_kind_to_mem_kind(""), None);
        assert_eq!(reflect_kind_to_mem_kind("note"), None);
    }

    #[test]
    fn build_reflect_user_message_truncates_long_turns() {
        use crate::llm::provider::ChatMessage;
        let long_user = "a".repeat(2000);
        let long_assistant = "b".repeat(2000);
        let msgs = vec![
            ChatMessage::System {
                content: "system".into(),
            },
            ChatMessage::User {
                content: long_user,
            },
            ChatMessage::Assistant {
                content: long_assistant,
                tool_calls: vec![],
            },
        ];
        let out = build_reflect_user_message(&msgs);
        // 600-char cap per turn + the role prefix
        // means each turn body is well under 700 chars.
        // We assert that the full 2000-char body did
        // NOT make it through.
        assert!(!out.contains(&"a".repeat(1500)));
        assert!(!out.contains(&"b".repeat(1500)));
        assert!(out.contains("[user]"));
        assert!(out.contains("[assistant]"));
    }

    #[test]
    fn build_reflect_user_message_caps_turns() {
        use crate::llm::provider::ChatMessage;
        // 8 user + 8 assistant turns → only the most
        // recent 6 turns (12 messages) should appear.
        let mut msgs = vec![ChatMessage::System {
            content: "sys".into(),
        }];
        for i in 0..8 {
            msgs.push(ChatMessage::User {
                content: format!("u{i}"),
            });
            msgs.push(ChatMessage::Assistant {
                content: format!("a{i}"),
                tool_calls: vec![],
            });
        }
        let out = build_reflect_user_message(&msgs);
        // The earliest 2 user turns (u0, u1) should
        // be truncated away.
        assert!(!out.contains("[user] u0"));
        assert!(!out.contains("[user] u1"));
        assert!(out.contains("[user] u6"));
        assert!(out.contains("[user] u7"));
    }
}
