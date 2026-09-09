//! v2.6.0 — The dispatch hook.
//!
//! `enqueue_if_ask_rule` is the single chokepoint the
//! executor calls before invoking a tool. It returns:
//!
//! * `None` — the rule is `auto`; the caller proceeds
//!   to run the tool normally.
//! * `Some(approval_id)` — the rule is `ask`; the
//!   approval is queued and the caller should return
//!   a `"approval pending (id=...)"` message to the
//!   LLM. The actual tool runs later, when the user
//!   clicks Approve / Edit & send in the UI.
//! * `Err(msg)` — the rule is `deny`; the caller
//!   surfaces the error to the LLM.
//!
//! The Bot's reasoning loop keeps running after a
//! `Some` — the LLM might want to do other work
//! (read files, build context) while the user
//! deliberates. v2.6.1 will auto-resume the Bot
//! when an approval is decided.

use serde_json::Value;

use crate::AppState;

use super::Rule;

/// Decision returned by `enqueue_if_ask_rule`.
///
/// Flattened to `Result<Option<String>, String>` so
/// the executor's match arms stay readable. The
/// three states map directly onto the three Rule
/// values:
///
/// * `Ok(None)`        → `Rule::Auto`
/// * `Ok(Some(id))`    → `Rule::Ask`
/// * `Err(msg)`        → `Rule::Deny`
///
/// Using `Result` here (not a 3-variant enum) is
/// deliberate: the executor's existing tool-call
/// site already speaks `Result<ToolResult, E>`,
/// so we don't introduce a new shape just for
/// approval gating.
pub async fn enqueue_if_ask_rule(
    state: &AppState,
    bot_id: &str,
    tool_name: &str,
    payload: &Value,
    bot_run_id: Option<&str>,
) -> Result<Option<String>, String> {
    // Look up the rule. `get_approval_rule` defaults
    // to `Auto` when no row exists, which is the
    // "open by default" semantic for tools not in
    // the seed list.
    let rule = state
        .db
        .get_approval_rule(bot_id, tool_name)
        .map_err(|e| format!("approval rule lookup failed: {e}"))?;
    match rule {
        Rule::Auto => Ok(None),
        Rule::Ask => {
            let id = state
                .db
                .enqueue_approval(bot_id, tool_name, payload, bot_run_id)
                .map_err(|e| format!("enqueue approval failed: {e}"))?;
            Ok(Some(id))
        }
        Rule::Deny => Err(format!(
            "tool `{tool_name}` is denied for this Bot (approval rule: deny)"
        )),
    }
}

/// Friendly string the executor returns to the LLM
/// when an approval is queued. Keeping it in one
/// place so the wire format stays consistent across
/// the executor's two tool-dispatch sites (the
/// per-iteration loop and `handle_message_bot`).
pub fn pending_message(approval_id: &str) -> String {
    format!(
        "approval pending (id={approval_id}) — the user must approve this tool call before it runs"
    )
}

// =====================================================================
//  Tests
// =====================================================================
//
// We exercise the three core decision branches
// (auto → None, ask → Some, deny → Err) and the
// store fallback semantics through the queue
// entrypoint. The store-level tests in `store.rs`
// cover the round-trip details; here we just
// verify the rule lookup gates the right thing.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bots::registry::BotRunRegistry;
    use crate::computer::ComputerManager;
    use crate::mcp::McpRegistry;
    use crate::skills::recorder::RecorderState;
    use crate::storage::Database;
    use serde_json::json;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// Build a minimal `AppState` for the queue test.
    /// We use a tmpfile SQLite and default
    /// `ComputerManager` / `McpRegistry` so the
    /// constructor doesn't panic. The other fields
    /// (`bot_runs`, `recorder`) only matter to the
    /// LLM path, not the queue.
    fn fresh_state() -> (Arc<AppState>, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("queue.sqlite");
        let db = Database::open(&path).expect("open test db");
        // Seed one Bot. The queue test uses its id
        // directly, so the FKs on `approval_rules`
        // and `approvals` are satisfied.
        let now = chrono::Utc::now().to_rfc3339();
        let bot_id = "bot-queue-1";
        db.conn
            .lock()
            .expect("db lock poisoned")
            .execute(
                "INSERT INTO bots (id, name, allowed_tools, created_at, updated_at)
                 VALUES (?, 'QueueTest', '[]', ?, ?)",
                rusqlite::params![bot_id, now, now],
            )
            .expect("insert bot");
        // The ComputerManager::new only reads Settings;
        // an empty settings blob is fine.
        let settings = crate::Settings::default();
        let computer = Arc::new(ComputerManager::new(&settings));
        let state = Arc::new(AppState {
            db: Arc::new(db),
            mcp: McpRegistry::default(),
            bot_runs: Arc::new(BotRunRegistry::new()),
            computer,
            recorder: Arc::new(RecorderState::new()),
        });
        (state, dir)
    }

    #[tokio::test]
    async fn enqueue_if_ask_rule_returns_none_for_auto() {
        let (state, _dir) = fresh_state();
        // No rule row → defaults to Auto.
        let result = enqueue_if_ask_rule(
            &state,
            "bot-queue-1",
            "any_tool",
            &json!({}),
            None,
        )
        .await
        .expect("auto rule returns Ok");
        assert!(result.is_none(), "auto rule must return Ok(None)");
    }

    #[tokio::test]
    async fn enqueue_if_ask_rule_returns_some_for_ask_rule() {
        let (state, _dir) = fresh_state();
        state
            .db
            .set_approval_rule("bot-queue-1", "mail_send", Rule::Ask)
            .expect("set ask");
        let result = enqueue_if_ask_rule(
            &state,
            "bot-queue-1",
            "mail_send",
            &json!({ "to": "x@y" }),
            Some("run-99"),
        )
        .await
        .expect("ask rule returns Ok");
        let id = result.expect("ask rule returns Ok(Some(id))");
        // The id we got back should resolve to a
        // pending approval row with the payload
        // we handed in.
        let approval = state
            .db
            .get_approval(&id)
            .expect("get_approval")
            .expect("approval row exists");
        assert_eq!(approval.status, "pending");
        assert_eq!(approval.tool_name, "mail_send");
        assert_eq!(approval.bot_run_id.as_deref(), Some("run-99"));
    }

    #[tokio::test]
    async fn enqueue_if_ask_rule_returns_err_for_deny() {
        let (state, _dir) = fresh_state();
        state
            .db
            .set_approval_rule("bot-queue-1", "shell_run", Rule::Deny)
            .expect("set deny");
        let result = enqueue_if_ask_rule(
            &state,
            "bot-queue-1",
            "shell_run",
            &json!({ "cmd": "rm -rf /" }),
            None,
        )
        .await;
        assert!(
            result.is_err(),
            "deny rule must return Err, got {result:?}"
        );
        let err = result.unwrap_err();
        // The error must mention the tool by name
        // so the LLM can see which one was blocked.
        assert!(err.contains("shell_run"), "error msg: {err}");
    }
}
