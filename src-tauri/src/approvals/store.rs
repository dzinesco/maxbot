//! v2.6.0 — Approval storage layer.
//!
//! Thin DB wrappers around the `approval_rules` and
//! `approvals` tables. No business logic — that lives
//! in `queue.rs` (the dispatch hook) and
//! `commands/approvals.rs` (the IPC surface).

use chrono::{DateTime, Utc};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use uuid::Uuid;

use crate::storage::Database;

use super::{Approval, ApprovalRule, Rule};

// ----- approval_rules -----

impl Database {
    /// Look up a per-Bot per-tool rule. Returns
    /// `Rule::Auto` if no row exists — callers should
    /// treat "missing" the same as "auto" so a brand-new
    /// Bot / tool is open-by-default.
    pub fn get_approval_rule(
        &self,
        bot_id: &str,
        tool_name: &str,
    ) -> rusqlite::Result<Rule> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let rule: Option<String> = conn
            .query_row(
                "SELECT rule FROM approval_rules WHERE bot_id = ? AND tool_name = ?",
                params![bot_id, tool_name],
                |row| row.get(0),
            )
            .optional()?;
        Ok(rule
            .map(|s| Rule::parse(s.as_str()))
            .unwrap_or(Rule::Auto))
    }

    /// Upsert a rule. Idempotent — the same
    /// `(bot_id, tool_name)` always lands on the new rule.
    pub fn set_approval_rule(
        &self,
        bot_id: &str,
        tool_name: &str,
        rule: Rule,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO approval_rules (bot_id, tool_name, rule)
             VALUES (?, ?, ?)
             ON CONFLICT(bot_id, tool_name) DO UPDATE SET rule = excluded.rule",
            params![bot_id, tool_name, rule.as_str()],
        )?;
        Ok(())
    }

    /// All rules for a Bot (one row per tool the Bot
    /// has an opinion about; missing tools default to
    /// `auto`).
    pub fn list_approval_rules(
        &self,
        bot_id: &str,
    ) -> rusqlite::Result<Vec<ApprovalRule>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, tool_name, rule FROM approval_rules
             WHERE bot_id = ? ORDER BY tool_name ASC",
        )?;
        let rows = stmt.query_map(params![bot_id], |row| {
            let rule_str: String = row.get(2)?;
            Ok(ApprovalRule {
                bot_id: row.get(0)?,
                tool_name: row.get(1)?,
                rule: Rule::parse(&rule_str),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }
}

// ----- approvals -----

impl Database {
    /// Insert a new pending approval. Returns the
    /// freshly-minted `id`.
    pub fn enqueue_approval(
        &self,
        bot_id: &str,
        tool_name: &str,
        payload: &Value,
        bot_run_id: Option<&str>,
    ) -> rusqlite::Result<String> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let payload_json = serde_json::to_string(payload)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        conn.execute(
            "INSERT INTO approvals
                (id, bot_id, tool_name, status, payload_json, bot_run_id, created_at)
             VALUES (?, ?, ?, 'pending', ?, ?, ?)",
            params![
                id,
                bot_id,
                tool_name,
                payload_json,
                bot_run_id,
                now.to_rfc3339(),
            ],
        )?;
        Ok(id)
    }

    /// Fetch one approval by id. `Ok(None)` if the id
    /// doesn't exist.
    pub fn get_approval(&self, id: &str) -> rusqlite::Result<Option<Approval>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, bot_id, tool_name, status, payload_json,
                    result_json, bot_run_id, created_at, decided_at
             FROM approvals WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        Ok(Some(row_to_approval(row)?))
    }

    /// Pending approvals. When `bot_id` is `Some`,
    /// filtered to that Bot.
    pub fn list_pending_approvals(
        &self,
        bot_id: Option<&str>,
    ) -> rusqlite::Result<Vec<Approval>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let (sql, params_vec): (&str, Vec<rusqlite::types::Value>) = match bot_id {
            Some(b) => (
                "SELECT id, bot_id, tool_name, status, payload_json,
                        result_json, bot_run_id, created_at, decided_at
                 FROM approvals
                 WHERE status = 'pending' AND bot_id = ?1
                 ORDER BY created_at DESC",
                vec![rusqlite::types::Value::Text(b.to_string())],
            ),
            None => (
                "SELECT id, bot_id, tool_name, status, payload_json,
                        result_json, bot_run_id, created_at, decided_at
                 FROM approvals
                 WHERE status = 'pending'
                 ORDER BY created_at DESC",
                Vec::new(),
            ),
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(params_vec.iter()), |row| {
                row_to_approval(row)
            })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Count pending approvals across all Bots. Used
    /// by the Sidebar badge.
    pub fn count_pending_approvals(&self) -> rusqlite::Result<u32> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM approvals WHERE status = 'pending'",
            [],
            |row| row.get(0),
        )?;
        Ok(n as u32)
    }

    /// Mark an approval as decided. `result` is the
    /// tool's result string (or `None` for rejections).
    /// `status` is one of `"approved"`, `"rejected"`,
    /// `"edited"`.
    pub fn decide_approval(
        &self,
        id: &str,
        status: &str,
        result: Option<&str>,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "UPDATE approvals
             SET status = ?, result_json = ?, decided_at = ?
             WHERE id = ?",
            params![status, result, now, id],
        )?;
        Ok(())
    }
}

/// Shared row → Approval conversion. Used by both
/// `get_approval` and `list_pending_approvals` so the
/// SQL stays in one place.
fn row_to_approval(row: &rusqlite::Row) -> rusqlite::Result<Approval> {
    let payload_str: String = row.get(4)?;
    let result_str: Option<String> = row.get(5)?;
    let bot_run_id: Option<String> = row.get(6)?;
    let created_at: DateTime<Utc> = parse_dt_field(row.get(7)?)?;
    let decided_at: Option<DateTime<Utc>> = match row.get::<_, Option<String>>(8)? {
        Some(s) => Some(parse_dt_field(s)?),
        None => None,
    };
    let payload: Value = serde_json::from_str(&payload_str).unwrap_or(Value::Null);
    let result: Option<Value> = result_str
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok());
    Ok(Approval {
        id: row.get(0)?,
        bot_id: row.get(1)?,
        tool_name: row.get(2)?,
        status: row.get(3)?,
        payload,
        result,
        bot_run_id,
        created_at,
        decided_at,
    })
}

/// Parse an RFC3339 datetime string from SQLite. We
/// don't share `parse_dt` from `db.rs` because that's
/// a private helper there; this is a small duplicate
/// that handles the same two shapes the rest of the
/// schema uses.
fn parse_dt_field(s: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })
}

// =====================================================================
//  Tests
// =====================================================================
//
// We exercise the store from a real (tmpfile) SQLite
// database so the SQL is verified end-to-end. The
// queue.rs dispatch test below uses these same
// primitives, but having a dedicated suite here makes
// the test failures self-explanatory.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Database;
    use serde_json::json;
    use tempfile::TempDir;

    fn fresh_db() -> (Database, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("approvals.sqlite");
        let db = Database::open(&path).expect("open test db");
        // Insert a Bot so the FK on `approvals.bot_id` and
        // `approval_rules.bot_id` is satisfied.
        let now = chrono::Utc::now().to_rfc3339();
        let bot_id = "bot-test-1";
        db.conn
            .lock()
            .expect("db lock poisoned")
            .execute(
                "INSERT INTO bots (id, name, allowed_tools, created_at, updated_at)
                 VALUES (?, 'Test', '[]', ?, ?)",
                rusqlite::params![bot_id, now, now],
            )
            .expect("insert bot");
        (db, dir)
    }

    #[test]
    fn get_rule_falls_back_to_auto_when_no_row() {
        let (db, _dir) = fresh_db();
        // No row in `approval_rules` for this Bot+tool.
        let r = db
            .get_approval_rule("bot-test-1", "nonexistent_tool")
            .expect("get_approval_rule");
        assert_eq!(r, Rule::Auto);
    }

    #[test]
    fn set_and_get_rule_round_trip() {
        let (db, _dir) = fresh_db();
        db.set_approval_rule("bot-test-1", "shell_run", Rule::Ask)
            .expect("set_rule ask");
        db.set_approval_rule("bot-test-1", "mail_send", Rule::Deny)
            .expect("set_rule deny");
        assert_eq!(
            db.get_approval_rule("bot-test-1", "shell_run")
                .expect("get_rule ask"),
            Rule::Ask,
        );
        assert_eq!(
            db.get_approval_rule("bot-test-1", "mail_send")
                .expect("get_rule deny"),
            Rule::Deny,
        );
    }

    #[test]
    fn set_rule_upserts_existing_row() {
        let (db, _dir) = fresh_db();
        db.set_approval_rule("bot-test-1", "shell_run", Rule::Ask)
            .expect("first set");
        db.set_approval_rule("bot-test-1", "shell_run", Rule::Deny)
            .expect("second set (upsert)");
        // One row, latest rule wins.
        let rules = db
            .list_approval_rules("bot-test-1")
            .expect("list_approval_rules");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].rule, Rule::Deny);
    }

    #[test]
    fn enqueue_and_get_approval_round_trip() {
        let (db, _dir) = fresh_db();
        let payload = json!({ "to": "user@example.com", "subject": "hi" });
        let id = db
            .enqueue_approval("bot-test-1", "mail_send", &payload, Some("run-1"))
            .expect("enqueue");
        let a = db
            .get_approval(&id)
            .expect("get_approval")
            .expect("approval exists");
        assert_eq!(a.status, "pending");
        assert_eq!(a.bot_id, "bot-test-1");
        assert_eq!(a.tool_name, "mail_send");
        assert_eq!(a.bot_run_id.as_deref(), Some("run-1"));
        assert_eq!(a.payload, payload);
        assert!(a.decided_at.is_none());
    }

    #[test]
    fn decide_approval_writes_status_and_result() {
        let (db, _dir) = fresh_db();
        let id = db
            .enqueue_approval(
                "bot-test-1",
                "mail_send",
                &json!({ "to": "x" }),
                None,
            )
            .expect("enqueue");
        db.decide_approval(&id, "approved", Some("\"sent\""))
            .expect("decide");
        let a = db
            .get_approval(&id)
            .expect("get")
            .expect("exists");
        assert_eq!(a.status, "approved");
        assert!(a.decided_at.is_some());
        assert_eq!(a.result, Some(json!("sent")));
    }

    #[test]
    fn list_pending_filters_by_bot_and_status() {
        let (db, _dir) = fresh_db();
        // Two pending + one approved. Pending should
        // surface all pending; the approved one is gone.
        let _ = db
            .enqueue_approval("bot-test-1", "mail_send", &json!({}), None)
            .expect("enqueue 1");
        let second = db
            .enqueue_approval("bot-test-1", "file_write", &json!({}), None)
            .expect("enqueue 2");
        db.decide_approval(&second, "rejected", None)
            .expect("decide");
        let pending = db
            .list_pending_approvals(None)
            .expect("list pending");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].tool_name, "mail_send");
        // Filter by bot: same one row, since we only
        // have one Bot.
        let pending_b = db
            .list_pending_approvals(Some("bot-test-1"))
            .expect("list pending bot");
        assert_eq!(pending_b.len(), 1);
    }

    #[test]
    fn count_pending_approvals() {
        let (db, _dir) = fresh_db();
        let _ = db
            .enqueue_approval("bot-test-1", "mail_send", &json!({}), None)
            .expect("enqueue a");
        let _ = db
            .enqueue_approval("bot-test-1", "file_write", &json!({}), None)
            .expect("enqueue b");
        let decided = db
            .enqueue_approval("bot-test-1", "shell_run", &json!({}), None)
            .expect("enqueue c");
        db.decide_approval(&decided, "approved", Some("\"ok\""))
            .expect("decide c");
        let n = db
            .count_pending_approvals()
            .expect("count pending");
        assert_eq!(n, 2);
    }
}
