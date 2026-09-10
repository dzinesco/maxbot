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
}
