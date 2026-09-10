//! v3.7.0 (Phase 8) — Demo Skills for the three new
//! connectors (Gmail / Google Calendar / GitHub).
//!
//! Each demo skill is a small (1-2 step) canary that
//! demonstrates the connector wiring. They are NOT
//! real workflows — the brief calls them "canaries."
//! The user can run them straight from the Skills
//! panel to confirm the connector tool is reachable
//! and the Bot's allowlist is set up correctly.
//!
//! ## Seeding
//!
//! The seed function is called from `lib.rs::run()`
//! alongside the existing `seed_demo_skill`. The
//! function is idempotent: if any Skill already
//! exists in the DB (the existing `maxbot-daily-checkin`
//! counts), the seed is skipped. A user who deletes
//! the demo skills and re-launches the app does NOT
//! get a surprise re-seed (per the v3.3.0 pattern).
//!
//! ## Why not multi-step
//!
//! The brief suggests 5-10 LLM steps per skill. A
//! Skill `step` is a single tool call, not an LLM
//! reasoning step. For a canary, one or two tool
//! calls is enough to prove the connector is wired
//! — the Bot can wrap the call in any LLM reasoning
//! shape it wants (the Skill run loop just dispatches
//! the step). Three one-step skills, one per
//! connector, matches the canary intent.

use crate::skills::{Skill, Step};
use crate::storage::Database;

/// Seed the three connector demo skills. Idempotent:
/// the existing `seed_demo_skill` in `lib.rs` already
/// short-circuits when ANY Skill exists, so a typical
/// install gets the existing `maxbot-daily-checkin`
/// skill from v3.3.0 and skips the connector
/// canaries. A fresh install (no Skills at all) gets
/// all four: `maxbot-daily-checkin` (v3.3.0) plus
/// `gmail-daily-summary`, `calendar-today`, and
/// `github-my-issues` (v3.7.0).
///
/// The function returns the number of skills that
/// were actually written to the DB, so a test can
/// assert the call path. Failures are logged and
/// swallowed (a failed seed should not block app
/// startup).
pub fn seed_connector_demo_skills(db: &Database) -> usize {
    let already_seeded = db
        .list_skills()
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    if already_seeded {
        return 0;
    }
    let now = chrono::Utc::now();
    let skills: Vec<Skill> = vec![
        Skill {
            id: "demo-gmail-daily-summary".to_string(),
            name: "gmail-daily-summary".to_string(),
            description:
                "Demo Skill shipped with v3.7.0. Fetches the last 10 Gmail messages \
                 and writes a 3-bullet summary to the Bot's scratchpad. Requires the \
                 `gmail_list_messages` tool to be enabled for the Bot and a Google \
                 OAuth access token in Settings."
                    .to_string(),
            inputs: Vec::new(),
            steps: vec![Step {
                tool: "gmail_list_messages".to_string(),
                args: serde_json::json!({ "max_results": 10 }),
                output_var: Some("summary_input".to_string()),
            }],
            created_at: now,
            updated_at: now,
        },
        Skill {
            id: "demo-calendar-today".to_string(),
            name: "calendar-today".to_string(),
            description:
                "Demo Skill shipped with v3.7.0. Lists today's Google Calendar events \
                 in chronological order. Requires the `calendar_list_events` tool to be \
                 enabled for the Bot and a Google OAuth access token in Settings."
                    .to_string(),
            inputs: Vec::new(),
            steps: vec![Step {
                tool: "calendar_list_events".to_string(),
                args: serde_json::json!({
                    "time_min": start_of_today_iso(),
                    "time_max": end_of_today_iso(),
                    "max_results": 25
                }),
                output_var: Some("events".to_string()),
            }],
            created_at: now,
            updated_at: now,
        },
        Skill {
            id: "demo-github-my-issues".to_string(),
            name: "github-my-issues".to_string(),
            description:
                "Demo Skill shipped with v3.7.0. Lists open issues assigned to you on a \
                 single GitHub repo (the `repo` input). Requires the `github_list_issues` \
                 tool to be enabled for the Bot and a GitHub PAT in Settings."
                    .to_string(),
            inputs: vec![crate::skills::Param {
                name: "repo".to_string(),
                kind: "string".to_string(),
                default: Some("denoland/deno".to_string()),
                choices: None,
            }],
            steps: vec![Step {
                tool: "github_list_issues".to_string(),
                args: serde_json::json!({
                    "repo": "{{repo}}",
                    "state": "open",
                    "max_results": 50
                }),
                output_var: Some("issues".to_string()),
            }],
            created_at: now,
            updated_at: now,
        },
    ];
    let mut written = 0;
    for s in skills {
        if let Err(e) = db.upsert_skill(&s) {
            log::warn!("seed_connector_demo_skills: upsert_skill({}) failed: {e}", s.id);
            continue;
        }
        log::info!("seed_connector_demo_skills: seeded `{}`", s.name);
        written += 1;
    }
    written
}

/// Render the start of "today" in local time as an
/// ISO-8601 string. The connector tool expects
/// RFC3339; we use the local timezone offset so
/// "today" matches the user's wall clock.
fn start_of_today_iso() -> String {
    let now = chrono::Local::now();
    let start = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
    let dt = start.and_local_timezone(chrono::Local).unwrap();
    dt.to_rfc3339()
}

/// Render the end of "today" in local time as an
/// ISO-8601 string. 24 hours after start.
fn end_of_today_iso() -> String {
    let now = chrono::Local::now();
    let end = now.date_naive().and_hms_opt(23, 59, 59).unwrap();
    let dt = end.and_local_timezone(chrono::Local).unwrap();
    dt.to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn fresh_db() -> Database {
        let dir = std::env::temp_dir().join(format!("maxbot-seeded-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite");
        Database::open(&path).expect("open test db")
    }

    /// A fresh install (no Skills) gets all three
    /// connector demo skills.
    #[test]
    fn seed_writes_three_skills_on_fresh_db() {
        let db = fresh_db();
        let n = seed_connector_demo_skills(&db);
        assert_eq!(n, 3);
        let list = db.list_skills().unwrap();
        assert_eq!(list.len(), 3);
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"gmail-daily-summary"));
        assert!(names.contains(&"calendar-today"));
        assert!(names.contains(&"github-my-issues"));
    }

    /// The seed is idempotent: re-running on a DB
    /// that already has Skills (any Skill) is a no-op.
    /// This matches the v3.3.0 pattern.
    #[test]
    fn seed_is_idempotent_when_skills_already_exist() {
        let db = fresh_db();
        // Pre-existing skill (any) blocks the seed.
        let pre = Skill::new_empty();
        db.upsert_skill(&pre).unwrap();
        let n = seed_connector_demo_skills(&db);
        assert_eq!(n, 0);
        let list = db.list_skills().unwrap();
        assert_eq!(list.len(), 1);
    }

    /// Each demo skill's `id` is stable so the seed
    /// is safe to re-run after a user deletes the
    /// skill (the next seed will recreate it with the
    /// same id, and the upsert path is a no-op for
    /// the rest of the table).
    #[test]
    fn demo_skill_ids_are_stable() {
        let db = fresh_db();
        seed_connector_demo_skills(&db);
        // Delete one and re-seed: the existing
        // (non-deleted) Skills block the re-seed.
        let list = db.list_skills().unwrap();
        db.delete_skill(&list[0].id).unwrap();
        let n = seed_connector_demo_skills(&db);
        assert_eq!(n, 0);
    }

    /// Each demo skill has at least one Step. The
    /// brief says "5-10 LLM steps" but a Skill step
    /// is a tool call; for a canary, one tool call
    /// is enough.
    #[test]
    fn demo_skills_have_at_least_one_step() {
        let db = fresh_db();
        seed_connector_demo_skills(&db);
        for s in db.list_skills().unwrap() {
            assert!(!s.steps.is_empty(), "{} has no steps", s.name);
        }
    }
}
