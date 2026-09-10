//! DB layer for Skills.
//!
//! The actual SQL lives in `crate::storage::db` (alongside
//! all the other `impl Database` blocks) so it can access
//! the private `conn` field. This file is intentionally
//! empty of logic — it's the test-only seam for the Skills
//! DB layer. The shapes (`Skill`, `Step`, `Param`,
//! `SkillRun`) live in `crate::skills` and are re-exported
//! from `crate::storage::db` for tests that need them.

#[cfg(test)]
mod tests {
    use crate::skills::{Param, Skill, SkillRun, SkillRunStatus, Step};
    use crate::storage::Database;
    use chrono::Utc;
    use uuid::Uuid;

    fn fresh_db() -> Database {
        let dir = std::env::temp_dir().join(format!("maxbot-skill-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite");
        Database::open(&path).expect("open test db")
    }

    #[test]
    fn skill_round_trip_preserves_steps_and_inputs() {
        let db = fresh_db();
        let mut skill = Skill::new_empty();
        skill.name = "fetch weather".to_string();
        skill.description = "Look up SF weather".to_string();
        skill.inputs = vec![Param {
            name: "city".to_string(),
            kind: "string".to_string(),
            default: Some("San Francisco".to_string()),
            choices: None,
        }];
        skill.steps = vec![Step {
            tool: "web_fetch".to_string(),
            args: serde_json::json!({"url": "https://wttr.in/SF"}),
            output_var: None,
        }];
        db.upsert_skill(&skill).expect("upsert");

        let loaded = db.get_skill(&skill.id).expect("get").expect("exists");
        assert_eq!(loaded.name, "fetch weather");
        assert_eq!(loaded.description, "Look up SF weather");
        assert_eq!(loaded.inputs.len(), 1);
        assert_eq!(loaded.inputs[0].name, "city");
        assert_eq!(
            loaded.inputs[0].default.as_deref(),
            Some("San Francisco")
        );
        assert_eq!(loaded.steps.len(), 1);
        assert_eq!(loaded.steps[0].tool, "web_fetch");
    }

    #[test]
    fn skill_upsert_overwrites_existing_row() {
        let db = fresh_db();
        let mut skill = Skill::new_empty();
        skill.name = "first".to_string();
        db.upsert_skill(&skill).expect("first insert");
        skill.name = "renamed".to_string();
        skill.updated_at = Utc::now();
        db.upsert_skill(&skill).expect("overwrite");
        let loaded = db.get_skill(&skill.id).expect("get").expect("exists");
        assert_eq!(loaded.name, "renamed");
    }

    #[test]
    fn list_skills_orders_by_updated_at_desc() {
        let db = fresh_db();
        let mut a = Skill::new_empty();
        a.name = "a".to_string();
        a.updated_at = Utc::now() - chrono::Duration::seconds(60);
        let mut b = Skill::new_empty();
        b.name = "b".to_string();
        b.updated_at = Utc::now();
        db.upsert_skill(&a).unwrap();
        db.upsert_skill(&b).unwrap();
        let list = db.list_skills().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "b");
        assert_eq!(list[1].name, "a");
    }

    #[test]
    fn delete_skill_cascades_to_runs() {
        let db = fresh_db();
        // The skill_runs.bot_id FK references bots.id, so
        // we need a real bot row for the run to point at.
        // Use the existing `upsert_bot` helper.
        let now = Utc::now();
        let bot = crate::bots::Bot {
            id: "b1".to_string(),
            name: "test".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            default_model: "gpt-4o".to_string(),
            allowed_tools: Vec::new(),
            icon: String::new(),
            color: String::new(),
            avatar_color: String::new(),
            last_active_at: None,
            state: crate::bots::BotState::Idle,
            // v3.2.0 — `computer_use` defaults to "vm" for
            // new Bots. Skill tests don't exercise the
            // field; the value is just here so the struct
            // literal compiles.
            computer_use: "vm".to_string(),
            created_at: now,
            updated_at: now,
        };
        db.upsert_bot(&bot).unwrap();
        let skill = Skill::new_empty();
        db.upsert_skill(&skill).unwrap();
        let run = SkillRun {
            id: "r1".to_string(),
            skill_id: skill.id.clone(),
            bot_id: "b1".to_string(),
            inputs: serde_json::json!({}),
            status: SkillRunStatus::Succeeded,
            started_at: Utc::now(),
            finished_at: Some(Utc::now()),
            result_summary: "ok".to_string(),
            steps: Vec::new(),
        };
        db.insert_skill_run(&run).unwrap();
        assert_eq!(db.list_skill_runs(&skill.id, None).unwrap().len(), 1);
        db.delete_skill(&skill.id).unwrap();
        assert!(db.get_skill(&skill.id).unwrap().is_none());
        assert_eq!(db.list_skill_runs(&skill.id, None).unwrap().len(), 0);
    }

    // v3.3.0 — Re-record: `update_skill_in_place` overwrites
    // the editable fields and preserves `id` + `created_at`.
    // The test seeds a skill, captures its `created_at`, calls
    // the updater with new name/description/steps, and
    // confirms the returned row has the new fields plus the
    // original `id` and `created_at`. A non-existent id
    // returns `Ok(None)`.
    #[test]
    fn update_skill_in_place_overwrites_fields_preserves_id_and_created_at() {
        let db = fresh_db();
        let mut skill = Skill::new_empty();
        skill.name = "old-name".to_string();
        skill.description = "old description".to_string();
        skill.steps = vec![Step {
            tool: "web_fetch".to_string(),
            args: serde_json::json!({"url": "https://old.example"}),
            output_var: None,
        }];
        let original_created_at = skill.created_at;
        let original_id = skill.id.clone();
        db.upsert_skill(&skill).expect("first insert");

        let new_inputs_json = "[]".to_string();
        let new_steps_json = serde_json::to_string(&vec![Step {
            tool: "shell_run".to_string(),
            args: serde_json::json!({"command": "echo hi"}),
            output_var: None,
        }])
        .unwrap();
        let updated = db
            .update_skill_in_place(
                &original_id,
                "new-name",
                "new description",
                &new_inputs_json,
                &new_steps_json,
            )
            .expect("update ok")
            .expect("skill exists");
        // id and created_at preserved.
        assert_eq!(updated.id, original_id);
        assert_eq!(updated.created_at, original_created_at);
        // editable fields replaced.
        assert_eq!(updated.name, "new-name");
        assert_eq!(updated.description, "new description");
        assert_eq!(updated.steps.len(), 1);
        assert_eq!(updated.steps[0].tool, "shell_run");
        // updated_at is bumped.
        assert!(updated.updated_at >= original_created_at);
    }

    #[test]
    fn update_skill_in_place_returns_none_for_missing_id() {
        let db = fresh_db();
        let res = db
            .update_skill_in_place("nope", "x", "y", "[]", "[]")
            .expect("no db error");
        assert!(res.is_none());
    }

    // v3.3.0 — Last run trace: `latest_skill_run_trace`
    // returns the most recent row for a skill, and
    // returns `None` if the skill has no traces. Insert
    // two traces with different timestamps, confirm the
    // newer one wins.
    #[test]
    fn latest_skill_run_trace_returns_most_recent_row() {
        let db = fresh_db();
        let skill = Skill::new_empty();
        db.upsert_skill(&skill).expect("upsert");
        let skill_id = skill.id.clone();
        let older = crate::skills::SkillRunTrace {
            id: "trace-older".to_string(),
            run_id: "run-older".to_string(),
            skill_id: skill_id.clone(),
            started_at: chrono::Utc::now() - chrono::Duration::seconds(60),
            duration_ms: 1234,
            per_step: vec![],
            success: true,
            trigger_input: "older trigger".to_string(),
        };
        let newer = crate::skills::SkillRunTrace {
            id: "trace-newer".to_string(),
            run_id: "run-newer".to_string(),
            skill_id: skill_id.clone(),
            started_at: chrono::Utc::now(),
            duration_ms: 5678,
            per_step: vec![],
            success: false,
            trigger_input: "newer trigger".to_string(),
        };
        db.insert_skill_run_trace(&older).expect("insert older");
        db.insert_skill_run_trace(&newer).expect("insert newer");
        let got = db
            .latest_skill_run_trace(&skill_id)
            .expect("query ok")
            .expect("trace exists");
        assert_eq!(got.id, "trace-newer");
        assert_eq!(got.run_id, "run-newer");
        assert_eq!(got.duration_ms, 5678);
        assert!(!got.success);
        assert_eq!(got.trigger_input, "newer trigger");
    }

    #[test]
    fn latest_skill_run_trace_returns_none_for_never_run_skill() {
        let db = fresh_db();
        let skill = Skill::new_empty();
        db.upsert_skill(&skill).expect("upsert");
        let got = db.latest_skill_run_trace(&skill.id).expect("query ok");
        assert!(got.is_none());
    }

    // v3.3.0 — Per-step output round-trip: insert a
    // trace with a non-empty `per_step` array, read it
    // back, confirm the per-step entries survived the
    // JSON round-trip with their `tool_name`, `args`,
    // `result`, and `ts` intact.
    #[test]
    fn per_step_output_round_trips_through_db() {
        let db = fresh_db();
        let skill = Skill::new_empty();
        db.upsert_skill(&skill).expect("upsert");
        let ts = chrono::Utc::now();
        let per_step = vec![
            crate::skills::StepTrace {
                role: "tool".to_string(),
                content: "first output".to_string(),
                tool_name: "web_fetch".to_string(),
                tool_args: serde_json::json!({"url": "https://a.example"}),
                tool_result: "first output".to_string(),
                ts,
            },
            crate::skills::StepTrace {
                role: "tool".to_string(),
                content: "second output".to_string(),
                tool_name: "web_fetch".to_string(),
                tool_args: serde_json::json!({"url": "https://b.example"}),
                tool_result: "second output".to_string(),
                ts,
            },
        ];
        let trace = crate::skills::SkillRunTrace {
            id: "trace-rt".to_string(),
            run_id: "run-rt".to_string(),
            skill_id: skill.id.clone(),
            started_at: ts,
            duration_ms: 999,
            per_step: per_step.clone(),
            success: true,
            trigger_input: "rt trigger".to_string(),
        };
        db.insert_skill_run_trace(&trace).expect("insert");
        let got = db
            .latest_skill_run_trace(&skill.id)
            .expect("query")
            .expect("exists");
        assert_eq!(got.per_step.len(), 2);
        assert_eq!(got.per_step[0].tool_name, "web_fetch");
        assert_eq!(got.per_step[0].tool_result, "first output");
        assert_eq!(
            got.per_step[0].tool_args["url"],
            "https://a.example"
        );
        assert_eq!(got.per_step[1].tool_result, "second output");
    }
}
