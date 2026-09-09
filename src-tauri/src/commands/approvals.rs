//! v2.6.0 — Approval IPC surface.
//!
//! Thin Tauri command wrappers around
//! `crate::approvals::store`. The interesting one is
//! `approval_decide`, which (a) flips the DB row to
//! `approved` / `rejected` / `edited`, (b) on
//! approved/edited re-runs the underlying tool via
//! the shared `ToolRegistry`, and (c) writes the
//! tool's result string back to `result_json`.
//!
//! The Bot does NOT auto-resume after a decision —
//! that's v2.6.1. For v2.6 the user manually sends
//! a follow-up if they want the loop to continue.

use serde::Serialize;
use serde_json::Value;
use tauri::State;
use uuid::Uuid;

use crate::approvals::{Approval, ApprovalRule, Rule};
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
