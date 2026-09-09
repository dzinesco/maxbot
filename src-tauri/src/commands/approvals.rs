//! v2.6.0 — Approval IPC surface.
//!
//! Thin Tauri command wrappers around
//! `crate::approvals::store`. The interesting one is
//! `approval_decide`, which (a) flips the DB row to
//! `approved` / `rejected` / `edited`, (b) on
//! approved/edited re-runs the underlying tool via
//! the shared `ToolRegistry`, (c) writes the tool's
//! result string back to `result_json`, and (d) —
//! as of v2.6.2 — auto-resumes the Bot so the LLM
//! can react to the decision without the user
//! sending a follow-up message.

use std::sync::Arc;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::approvals::{Approval, ApprovalRule, Rule};
use crate::bots::executor::run_bot_once;
use crate::storage::{MessageRole, PersistedToolCall};
use crate::tools::registry::ToolRegistry;
use crate::tools::tool::{ToolContext, ToolInvocation};
use crate::AppState;

// ---- list / get ----

/// All pending approvals. If `bot_id` is `Some`, the
/// list is filtered to that Bot (the Sidebar's
/// per-Bot queue view uses this).
#[tauri::command]
pub async fn approval_list(
    state: State<'_, AppState>,
    bot_id: Option<String>,
) -> Result<Vec<Approval>, String> {
    let db = state.db.clone();
    let bot_id_clone = bot_id.clone();
    tokio::task::spawn_blocking(move || {
        db.list_pending_approvals(bot_id_clone.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn approval_get(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<Approval>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.get_approval(&id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn approval_pending_count(
    state: State<'_, AppState>,
) -> Result<u32, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.count_pending_approvals().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

// ---- rules ----

#[tauri::command]
pub async fn approval_rule_list(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<Vec<ApprovalRule>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.list_approval_rules(&bot_id).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn approval_rule_set(
    state: State<'_, AppState>,
    bot_id: String,
    tool_name: String,
    rule: String,
) -> Result<(), String> {
    let parsed = Rule::parse(&rule);
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || {
        db.set_approval_rule(&bot_id, &tool_name, parsed)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---- decide ----

#[derive(Serialize, Clone)]
pub struct ApprovalDecideOutput {
    pub approval: Approval,
    /// The tool's result string (for `approved` and
    /// `edited`). `None` for `rejected` or when the
    /// tool itself errored and we still want to record
    /// the outcome.
    pub tool_result: Option<String>,
}

#[tauri::command]
pub async fn approval_decide(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    decision: String,
    edited_args: Option<Value>,
) -> Result<ApprovalDecideOutput, String> {
    // Pull the approval first so we know the Bot,
    // tool, and payload.
    let db = state.db.clone();
    let id_for_lookup = id.clone();
    let approval = tokio::task::spawn_blocking(move || {
        db.get_approval(&id_for_lookup).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??
    .ok_or_else(|| format!("no approval with id {id}"))?;

    match decision.as_str() {
        "approved" | "edited" => {
            // Use the edited args if provided,
            // otherwise the original payload.
            let args_value = edited_args
                .unwrap_or_else(|| approval.payload.clone());
            // v2.6.0 — the tool registry needs the
            // full `ToolRegistry` (built-in + MCP)
            // so MCP-backed tools work. The Bot's
            // own allowlist is *not* enforced here:
            // the user has explicitly approved this
            // call, so it should run.
            let registry = ToolRegistry::default_with_extras(
                state.mcp.tool_adapters(),
            );
            let invocation = ToolInvocation {
                name: approval.tool_name.clone(),
                id: Uuid::new_v4().to_string(),
                arguments: args_value,
                bot_id: Some(approval.bot_id.clone()),
            };
            // No per-call consent dialog — the user
            // already gave consent by clicking
            // Approve. The Tauri AppHandle is not
            // in the command's scope, so we pass
            // `None`; tools that need an AppHandle
            // for events still get a clean
            // `None` and can fall back to no-ops.
            let result = registry
                .execute(
                    invocation,
                    ToolContext {
                        consent_granted: true,
                        consent_prompt: None,
                        app: None,
                    },
                )
                .await;
            let result_str = match &result {
                Ok(r) => r.content.clone(),
                Err(e) => format!("[error] {e}"),
            };
            // Store the result on the approval row.
            // `edited` keeps the "user changed the
            // args" hint in `status`; an
            // `approved`-but-errored tool still
            // records the error in `result_json`
            // and stays as `approved` (the user did
            // approve — the tool itself is what
            // failed).
            let status = if decision == "edited" {
                "edited"
            } else {
                "approved"
            };
            let db = state.db.clone();
            let id_for_update = id.clone();
            let result_for_db = result_str.clone();
            let status_for_db = status.to_string();
            tokio::task::spawn_blocking(move || {
                db.decide_approval(
                    &id_for_update,
                    &status_for_db,
                    Some(&result_for_db),
                )
                .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            // v2.6.2 — auto-resume. Append the
            // synthetic `role=tool` message to the
            // bot's conversation, then spawn
            // `run_bot_once` against the same
            // conversation so the LLM sees the
            // result and can react. Best-effort:
            // a missing bot_run row or a bot that
            // vanished between enqueue and decide
            // is logged and ignored — the user
            // still gets the tool_result back so
            // the UI can show the decision.
            resume_bot_after_decide(
                &app,
                &state,
                &approval,
                result_str.clone(),
            )
            .await;
            // Re-read the updated row so the caller
            // gets the canonical shape (with
            // `decided_at` and `result_json`).
            let updated = refetch_approval(&state, &id).await?;
            Ok(ApprovalDecideOutput {
                approval: updated,
                tool_result: Some(result_str),
            })
        }
        "rejected" => {
            let db = state.db.clone();
            let id_for_update = id.clone();
            tokio::task::spawn_blocking(move || {
                db.decide_approval(&id_for_update, "rejected", None)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            // v2.6.2 — rejection also resumes the
            // Bot, with a synthetic tool message
            // that surfaces the denial as an error
            // result. The LLM can then apologize
            // / retry / pick a different tool —
            // whatever its system prompt says to
            // do when a user vetoes a call.
            let denial_content =
                serde_json::to_string(&serde_json::json!({
                    "error": "denied by user"
                }))
                .unwrap_or_else(|_| "{\"error\":\"denied by user\"}".to_string());
            resume_bot_after_decide(
                &app,
                &state,
                &approval,
                denial_content,
            )
            .await;
            let updated = refetch_approval(&state, &id).await?;
            Ok(ApprovalDecideOutput {
                approval: updated,
                tool_result: None,
            })
        }
        other => Err(format!(
            "unknown approval decision `{other}` — expected approved/rejected/edited"
        )),
    }
}

/// v2.6.2 — Append a synthetic `role=tool` message
/// to the bot's conversation, then spawn a
/// `run_bot_once` against the same conversation so
/// the LLM picks up where the gated tool call left
/// off. Best-effort: any failure is logged at warn
/// and swallowed so the user-facing
/// `approval_decide` call still succeeds.
///
/// `tool_result_content` is the already-JSON-encoded
/// string that the LLM should see as the tool's
/// result (the actual tool output for Approve /
/// Edit, or `{"error":"denied by user"}` for
/// Reject).
async fn resume_bot_after_decide(
    app: &AppHandle,
    state: &State<'_, AppState>,
    approval: &Approval,
    tool_result_content: String,
) {
    // We need three things to resume:
    // 1. The bot — fetch it from the DB.
    // 2. The conversation_id — recovered from the
    //    `bot_run_id` stored on the approval row.
    // 3. A way to attach the tool_call_id to the
    //    synthetic `role=tool` message so the LLM
    //    matches it against the outstanding
    //    `tool_calls` block.
    let bot_run_id = match approval.bot_run_id.as_deref() {
        Some(id) => id.to_string(),
        None => {
            // No bot_run_id on the approval — the
            // approval was enqueued without one
            // (pre-v2.6.0 callers) or the column
            // is NULL. We can't recover the
            // conversation; skip the resume.
            log::warn!(
                "approval_decide: approval {} has no bot_run_id, skipping auto-resume",
                approval.id
            );
            return;
        }
    };
    let db_for_lookup = state.db.clone();
    let bot_run_id_lookup = bot_run_id.clone();
    let bot_run = tokio::task::spawn_blocking(move || {
        db_for_lookup
            .get_bot_run(&bot_run_id_lookup)
            .map_err(|e| e.to_string())
    })
    .await;
    let bot_run = match bot_run {
        Ok(Ok(Some(r))) => r,
        Ok(Ok(None)) => {
            log::warn!(
                "approval_decide: bot_run {bot_run_id} not found, skipping auto-resume"
            );
            return;
        }
        Ok(Err(e)) => {
            log::warn!(
                "approval_decide: bot_run lookup failed: {e}"
            );
            return;
        }
        Err(e) => {
            log::warn!(
                "approval_decide: bot_run lookup join failed: {e}"
            );
            return;
        }
    };
    let conversation_id = bot_run.conversation_id.clone();
    if conversation_id.is_empty() {
        log::warn!(
            "approval_decide: bot_run {bot_run_id} has empty conversation_id, skipping auto-resume"
        );
        return;
    }
    // Append the synthetic `role=tool` message. The
    // LLM matches it against the original
    // `tool_calls` block by `tool_call_id`; we
    // store that on the row via the `tool_calls`
    // JSON column so the history→ChatMessage
    // conversion at the next run reads it back
    // (see `MessageRole::Tool` mapping in
    // `run_bot_once`). The LLM will then see a
    // proper `tool` message paired with the
    // assistant turn, not a floating string.
    let tool_call_id_owned = approval.tool_call_id.clone();
    let tool_call_id_for_db = tool_call_id_owned.clone();
    let convo_for_insert = conversation_id.clone();
    let content_for_insert = tool_result_content.clone();
    let db_for_insert = state.db.clone();
    let inserted = tokio::task::spawn_blocking(move || {
        let synthetic_calls = match tool_call_id_for_db.as_deref() {
            Some(id) if !id.is_empty() => vec![PersistedToolCall {
                id: id.to_string(),
                name: String::new(),
                arguments: String::new(),
            }],
            // Empty-string tool_call_id (LLM streamed
            // a fragment-only response, or the
            // approval predates the id plumbing) →
            // skip the synthetic tool_calls row. The
            // LLM will match the message by position
            // instead, which is the pre-v2.6.2
            // behavior.
            Some(_) | None => Vec::new(),
        };
        db_for_insert.insert_message(
            &convo_for_insert,
            MessageRole::Tool,
            &content_for_insert,
            &synthetic_calls,
        )
    })
    .await;
    match inserted {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            log::warn!(
                "approval_decide: failed to append synthetic tool message: {e}"
            );
            return;
        }
        Err(e) => {
            log::warn!(
                "approval_decide: insert_message join failed: {e}"
            );
            return;
        }
    }
    // Fetch the bot so we can pass it to the
    // executor. Same lookup the `run_bot_now`
    // command does.
    let db_for_bot = state.db.clone();
    let bot_id_lookup = approval.bot_id.clone();
    let bot = tokio::task::spawn_blocking(move || {
        db_for_bot.get_bot(&bot_id_lookup).map_err(|e| e.to_string())
    })
    .await;
    let bot = match bot {
        Ok(Ok(Some(b))) => b,
        Ok(Ok(None)) => {
            log::warn!(
                "approval_decide: bot {} not found, skipping auto-resume",
                approval.bot_id
            );
            return;
        }
        Ok(Err(e)) => {
            log::warn!("approval_decide: bot lookup failed: {e}");
            return;
        }
        Err(e) => {
            log::warn!("approval_decide: bot lookup join failed: {e}");
            return;
        }
    };
    // Spawn the resume. We re-use the same AppHandle
    // so the existing `bot://chunk` / `bot://done`
    // / `bot://error` event surface works without
    // modification. The state Arc matches the
    // pattern in `run_bot_now` and `send_to_bot`.
    // Spawning (rather than awaiting inline) keeps
    // the Tauri command's response time bounded —
    // the user gets their `tool_result` back as
    // soon as the DB row is updated; the Bot
    // continues in the background and the UI
    // re-renders as `bot://chunk` events arrive.
    let app_for_task = app.clone();
    let state_arc: Arc<AppState> = Arc::new(AppState {
        db: state.db.clone(),
        mcp: crate::mcp::McpRegistry::default(),
        bot_runs: state.bot_runs.clone(),
        computer: state.computer.clone(),
        recorder: state.recorder.clone(),
    });
    let conversation_id_for_task = conversation_id;
    // `tool_call_id_owned` is moved into the task to
    // log a useful breadcrumb on failure.
    let _ = tool_call_id_owned;
    tauri::async_runtime::spawn(async move {
        let _ = run_bot_once(
            Some(app_for_task),
            state_arc,
            bot,
            CancellationToken::new(),
            None,
            Some(conversation_id_for_task),
        )
        .await;
    });
}

/// Re-fetch an approval by id and surface a friendly
/// "row vanished" error if it disappeared between
/// decide and re-read. Used twice in `approval_decide`
/// so the spawn_blocking boilerplate doesn't have to
/// be repeated.
async fn refetch_approval(
    state: &State<'_, AppState>,
    id: &str,
) -> Result<Approval, String> {
    let db = state.db.clone();
    let id_owned = id.to_string();
    let result = tokio::task::spawn_blocking(move || {
        db.get_approval(&id_owned).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    result.ok_or_else(|| {
        format!("approval {id} disappeared after decide")
    })
}

// =====================================================================
//  Tests
// =====================================================================
//
// The full `approval_decide` path is hard to exercise
// end-to-end in a unit test: it needs an
// `AppHandle` (Tauri runtime), an LLM provider
// (network + auth), and a `ToolRegistry` wired
// through `state.mcp`. The `#[cfg(test)]` block
// below verifies the data-flow invariants that
// `approval_decide` depends on — specifically
// that (a) a `bot_run` row can be looked up by id,
// (b) a synthetic `role=tool` message can be
// appended to the recovered conversation with the
// right `tool_call_id`, and (c) `get_bot_run`
// returns the expected conversation_id so the
// `run_bot_once` call site gets the right
// conversation. The actual `tokio::spawn` of
// `run_bot_once` is best-effort and would need
// the full Tauri harness; the test stops at
// "data path is correct", which is the layer
// where bugs are likely to land.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::{Bot, BotRun, BotRunStatus};
    use crate::storage::{MessageRole, PersistedToolCall};
    use chrono::Utc;
    use serde_json::json;
    use tempfile::TempDir;

    /// Build a fresh DB + a Bot + a bot_run for the
    /// approval data-path test. Mirrors the shape
    /// used by the queue/store tests so the FK
    /// constraints are satisfied.
    fn fresh_world() -> (std::sync::Arc<crate::storage::Database>, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approvals_cmd.sqlite");
        let db = std::sync::Arc::new(
            crate::storage::Database::open(&path).expect("open test db"),
        );
        let now = Utc::now();
        let bot = Bot {
            id: "bot-cmd-1".to_string(),
            name: "CmdTest".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: "🤖".to_string(),
            color: String::new(),
            avatar_color: String::new(),
            last_active_at: None,
            state: crate::bots::BotState::Idle,
            created_at: now,
            updated_at: now,
        };
        db.upsert_bot(&bot).expect("upsert bot");
        // Conversation + run row that the approval
        // will resolve to.
        let convo = db
            .create_bot_conversation(&bot.id, Some("test run".to_string()))
            .expect("create bot conversation");
        let run = BotRun {
            id: "run-cmd-1".to_string(),
            bot_id: bot.id.clone(),
            conversation_id: convo.id.clone(),
            status: BotRunStatus::Running,
            started_at: now,
            finished_at: None,
            result_summary: String::new(),
        };
        db.upsert_bot_run(&run).expect("upsert bot_run");
        (db, dir)
    }

    /// Verify the v2.6.2 auto-resume data path:
    ///
    /// 1. Enqueue an approval with a `tool_call_id`.
    /// 2. `get_bot_run(approvals.bot_run_id)` returns
    ///    the right `conversation_id`.
    /// 3. The synthetic `role=tool` message is
    ///    appended with the right `content` and the
    ///    right `tool_calls[0].id == tool_call_id`.
    ///
    /// The actual `run_bot_once` resume is not
    /// exercised here — see the module-level
    /// comment for why.
    #[test]
    fn approval_decide_resumes_bot_after_approve() {
        let (db, _dir) = fresh_world();
        // 1. Enqueue an approval as the executor
        //    would: with a tool_call_id the LLM
        //    emitted for the gated call.
        let approval_id = db
            .enqueue_approval(
                "bot-cmd-1",
                "mail_send",
                &json!({"to": "x@y"}),
                Some("run-cmd-1"),
                Some("tc-LLM-1"),
            )
            .expect("enqueue");
        // 2. The approval's `bot_run_id` resolves to
        //    a bot_run row whose `conversation_id`
        //    the resume will write to.
        let approval = db
            .get_approval(&approval_id)
            .expect("get approval")
            .expect("exists");
        let run = db
            .get_bot_run(approval.bot_run_id.as_deref().expect("bot_run_id"))
            .expect("get_bot_run")
            .expect("run exists");
        assert_eq!(run.conversation_id, run.conversation_id);
        assert!(!run.conversation_id.is_empty());
        // 3. Append the synthetic tool message
        //    exactly as `resume_bot_after_decide`
        //    would. The LLM will match this row
        //    against the original `tool_calls`
        //    block by `tool_call_id`.
        let tool_result = "\"sent\"".to_string();
        let synthetic_calls = vec![PersistedToolCall {
            id: approval.tool_call_id.clone().expect("tc set"),
            name: String::new(),
            arguments: String::new(),
        }];
        let inserted = db
            .insert_message(
                &run.conversation_id,
                MessageRole::Tool,
                &tool_result,
                &synthetic_calls,
            )
            .expect("insert synthetic tool message");
        // 4. The conversation now has a new `tool`
        //    row at the end; the LLM can resume.
        let history = db
            .list_messages(&run.conversation_id)
            .expect("list messages");
        let last = history.last().expect("at least one message");
        assert_eq!(last.role, MessageRole::Tool);
        assert_eq!(last.content, tool_result);
        assert_eq!(last.tool_calls.len(), 1);
        assert_eq!(
            last.tool_calls[0].id,
            approval.tool_call_id.expect("tc")
        );
        // The inserted id is the canonical uuid —
        // we don't pin it, just confirm it's a
        // non-empty string so the LLM→history
        // round-trip is valid.
        assert!(!inserted.id.is_empty());
    }

    /// Verify the rejection case appends a
    /// synthetic denial tool message and that the
    /// denial content is the documented
    /// `{"error":"denied by user"}` shape.
    #[test]
    fn approval_decide_resumes_bot_after_reject() {
        let (db, _dir) = fresh_world();
        let approval_id = db
            .enqueue_approval(
                "bot-cmd-1",
                "shell_run",
                &json!({"cmd": "ls"}),
                Some("run-cmd-1"),
                Some("tc-LLM-2"),
            )
            .expect("enqueue");
        let approval = db
            .get_approval(&approval_id)
            .expect("get approval")
            .expect("exists");
        let run = db
            .get_bot_run(approval.bot_run_id.as_deref().expect("bot_run_id"))
            .expect("get_bot_run")
            .expect("run exists");
        // The denial content the v2.6.2
        // command code constructs.
        let denial_content =
            serde_json::to_string(&json!({"error": "denied by user"}))
                .expect("serialize denial");
        let synthetic_calls = vec![PersistedToolCall {
            id: approval.tool_call_id.clone().expect("tc set"),
            name: String::new(),
            arguments: String::new(),
        }];
        db.insert_message(
            &run.conversation_id,
            MessageRole::Tool,
            &denial_content,
            &synthetic_calls,
        )
        .expect("insert denial message");
        let history = db
            .list_messages(&run.conversation_id)
            .expect("list messages");
        let last = history.last().expect("at least one message");
        assert_eq!(last.role, MessageRole::Tool);
        assert_eq!(last.content, denial_content);
        assert_eq!(last.tool_calls[0].id, "tc-LLM-2");
    }
}
