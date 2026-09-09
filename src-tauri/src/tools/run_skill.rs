//! v2.2.0 — `RunSkillTool`.
//!
//! Allows a Bot (during its agent loop) to invoke a named
//! Skill. The LLM passes `{ skill_id, args }` and gets back
//! a short summary of the run. The full per-step trace lives
//! in `AppState::skill_runs` and is queryable by the UI.
//!
//! `requires_consent = false` because the Bot is autonomous
//! inside its agent loop (consent is already gated at the
//! `run_bot_once` level via the bot's `allowed_tools`
//! allowlist — the model can only call `run_skill` if the
//! bot's owner granted it).

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;
use tauri::AppHandle;
use tauri::Manager;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::skills::executor::run_skill;
use crate::skills::{SkillRun, SkillRunStatus};
use crate::tools::tool::{Tool, ToolContext, ToolError, ToolInvocation, ToolResult};
use crate::AppState;

pub struct RunSkillTool;

#[async_trait]
impl Tool for RunSkillTool {
    fn name(&self) -> &'static str {
        "run_skill"
    }

    fn description(&self) -> &str {
        "Run a saved Skill against the current Bot. \
         Returns a short summary of the run; the full per-step \
         trace is stored under the run id and is viewable in \
         the Skills panel. Use this when the user has a \
         reusable procedure you should execute end-to-end."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill_id": {
                    "type": "string",
                    "description": "The id of the Skill to run. \
                        The Bot can list available Skills via the \
                        standard UI; the run is initiated with the \
                        Skill's id from there."
                },
                "args": {
                    "type": "object",
                    "description": "Inputs to substitute into the \
                        Skill's steps. The keys must match the \
                        Skill's `inputs` schema. Optional: a Skill \
                        with no inputs can be called with an empty \
                        object."
                }
            },
            "required": ["skill_id"]
        })
    }

    fn requires_consent(&self) -> bool {
        // The Bot is autonomous; consent was already
        // granted when the owner added `run_skill` to the
        // bot's `allowed_tools` allowlist.
        false
    }

    async fn execute(
        &self,
        invocation: ToolInvocation,
        context: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let skill_id = invocation
            .arguments
            .get("skill_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ToolError::InvalidArguments("missing required field: skill_id".to_string())
            })?
            .to_string();
        let user_inputs = invocation
            .arguments
            .get("args")
            .cloned()
            .unwrap_or(Value::Object(Default::default()));
        // The bot_id is set by the Bot executor when it
        // dispatches a tool call. If it's missing, the
        // caller is something other than the agent loop;
        // bail.
        let bot_id = invocation.bot_id.clone().ok_or_else(|| {
            ToolError::InvalidArguments(
                "run_skill requires a bot context (no bot_id on invocation)".to_string(),
            )
        })?;

        let app: AppHandle = context.app.clone().ok_or_else(|| {
            ToolError::InvalidArguments("run_skill requires a Tauri AppHandle".to_string())
        })?;
        let state: tauri::State<Arc<AppState>> = app.state();
        let state = state.inner().clone();

        // Look up the Skill. A missing skill is a tool error,
        // not a SkillRun failure — the Bot should be able to
        // recover (e.g. the user removed the Skill mid-run).
        let skill = match state.db.get_skill(&skill_id) {
            Ok(Some(s)) => s,
            Ok(None) => {
                return Ok(ToolResult {
                    content: format!("no skill found with id {skill_id}"),
                    is_error: true,
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    content: format!("db error: {e}"),
                    is_error: true,
                });
            }
        };

        // Allocate a run row up front so the UI sees a
        // "running" entry the moment the tool fires.
        let run_id = Uuid::new_v4().to_string();
        let started = SkillRun {
            id: run_id.clone(),
            skill_id: skill.id.clone(),
            bot_id: bot_id.clone(),
            inputs: user_inputs.clone(),
            status: SkillRunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            result_summary: String::new(),
            steps: Vec::new(),
        };
        if let Err(e) = state.db.insert_skill_run(&started) {
            return Ok(ToolResult {
                content: format!("could not record skill run start: {e}"),
                is_error: true,
            });
        }

        let cancel = CancellationToken::new();
        let run = run_skill(
            app.clone(),
            state.clone(),
            run_id.clone(),
            skill,
            bot_id,
            user_inputs,
            cancel,
        )
        .await;

        // Persist the final state. The in-memory `steps`
        // are intentionally dropped — the DB row is the
        // durable record; per-step progress is only kept
        // for the in-memory view (we don't currently
        // surface mid-run progress from a tool call
        // through to the chat panel, but the schema is
        // ready when we do).
        let mut to_persist = run.clone();
        to_persist.steps.clear();
        if let Err(e) = state.db.update_skill_run(&to_persist) {
            return Ok(ToolResult {
                content: format!(
                    "skill run {} finished (status={}) but persistence failed: {e}",
                    to_persist.id,
                    to_persist.status.as_str()
                ),
                is_error: true,
            });
        }

        // Short, model-friendly summary. The full run is
        // queryable via `skill_run_status`; the model
        // doesn't need the per-step trace inline.
        let summary = format!(
            "skill run {run_id} finished: status={}, result={}",
            run.status.as_str(),
            run.result_summary
        );
        Ok(ToolResult {
            content: summary,
            is_error: matches!(run.status, SkillRunStatus::Failed | SkillRunStatus::Cancelled),
        })
    }
}
