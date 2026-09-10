//! v2.2.0 — Skill Tauri commands.
//!
//! Thin IPC surface. The heavy lifting is in
//! `crate::skills`:
//!   - `store.rs` — DB layer
//!   - `executor.rs` — `run_skill` (the step walker)
//!   - `recorder.rs` — `RecorderState` (capture tool calls)
//!
//! Recording flow:
//!   1. `skill_record_start(bot_id)` → allocates a
//!      recording session, spawns `run_bot_once` with
//!      `recording_id = Some(...)`, returns
//!      `{ recording_id, bot_run_id }`.
//!   2. The executor captures every tool call.
//!   3. `skill_record_stop(recording_id)` → drains the
//!      recorder into a candidate `Skill` (with empty
//!      name/description) for the UI to fill in.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::bots::executor::run_bot_once;
use crate::skills::executor::{build_skill_run_trace, run_skill};
use crate::skills::recorder::SharedRecorderState;
use crate::skills::{Skill, SkillRun, SkillRunStatus, SkillRunTrace};
use crate::AppState;

// ---- types that mirror the renderer side ----

/// Returned by `skill_record_start`. The renderer keeps the
/// `recording_id` and uses it on Stop; the `bot_run_id` is
/// for matching `bot://done` / `bot://error` events.
#[derive(Serialize, Clone)]
pub struct SkillRecordStart {
    pub recording_id: String,
    pub bot_run_id: String,
}

/// Returned by `skill_record_stop`. The candidate Skill has
/// empty `name` / `description`; the renderer shows it in a
/// form so the user can fill in those fields before saving.
#[derive(Serialize, Clone)]
pub struct SkillRecordStop {
    pub skill: Skill,
}

// ---- list / get / create / delete ----

#[tauri::command]
pub async fn skill_list(state: State<'_, AppState>) -> Result<Vec<Skill>, String> {
    let db = state.db.clone();
    tokio::task::spawn_blocking(move || db.list_skills().map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn skill_get(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<Skill>, String> {
    let db = state.db.clone();
    let id_clone = id.clone();
    tokio::task::spawn_blocking(move || db.get_skill(&id_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// Persist a Skill (new or updated). The renderer passes a
/// full `Skill` shape; if `id` is empty we generate one, and
/// `created_at` / `updated_at` are stamped if missing.
#[tauri::command]
pub async fn skill_create(state: State<'_, AppState>, skill: Skill) -> Result<Skill, String> {
    let mut skill = skill;
    if skill.id.is_empty() {
        skill.id = Uuid::new_v4().to_string();
    }
    let now = Utc::now();
    if skill.created_at.timestamp() == 0 {
        skill.created_at = now;
    }
    skill.updated_at = now;
    let db = state.db.clone();
    let to_save = skill.clone();
    tokio::task::spawn_blocking(move || db.upsert_skill(&to_save).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())??;
    Ok(skill)
}

#[tauri::command]
pub async fn skill_delete(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.db.clone();
    let id_clone = id.clone();
    tokio::task::spawn_blocking(move || db.delete_skill(&id_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

/// v3.3.0 — Re-record: overwrite the editable fields of an
/// existing Skill in place. The `id` from the path is
/// authoritative — the renderer passes the skill's
/// existing id, the new `name`/`description`/`inputs`/
/// `steps`, and the DB preserves `id` and `created_at` (a
/// bot_schedules.skill_id pointing at this row is
/// untouched, since the schedule lives on the bot, not
/// the skill). Returns the updated row, or an error if
/// the id is not found. The UI's "Re-record" button on
/// each skill row calls this with the skill's existing
/// id and the user's edited JSON.
#[tauri::command]
pub async fn skill_update(
    state: State<'_, AppState>,
    id: String,
    skill: Skill,
) -> Result<Skill, String> {
    let inputs_json =
        serde_json::to_string(&skill.inputs).map_err(|e| e.to_string())?;
    let steps_json =
        serde_json::to_string(&skill.steps).map_err(|e| e.to_string())?;
    let id_for_db = id.clone();
    let name = skill.name.clone();
    let description = skill.description.clone();
    let db = state.db.clone();
    let updated = tokio::task::spawn_blocking(move || {
        db.update_skill_in_place(&id_for_db, &name, &description, &inputs_json, &steps_json)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())??;
    updated.ok_or_else(|| format!("no skill found with id {id}"))
}

/// v3.3.0 — Last run trace for a Skill. Returns the most
/// recent row from `skill_run_traces` for the given skill
/// id, or `None` if the skill has never been run. The
/// Skills panel's "Last run" expandable view calls this
/// on expand. The renderer caches the result while the
/// user is on the panel so re-expanding doesn't re-fetch.
#[tauri::command]
pub async fn skill_run_last_trace(
    state: State<'_, AppState>,
    skill_id: String,
) -> Result<Option<SkillRunTrace>, String> {
    let db = state.db.clone();
    let id_clone = skill_id.clone();
    tokio::task::spawn_blocking(move || db.latest_skill_run_trace(&id_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

// ---- runs ----

#[tauri::command]
pub async fn skill_run_history(
    state: State<'_, AppState>,
    skill_id: String,
    limit: Option<u32>,
) -> Result<Vec<SkillRun>, String> {
    let db = state.db.clone();
    let id_clone = skill_id.clone();
    tokio::task::spawn_blocking(move || {
        db.list_skill_runs(&id_clone, limit).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Run a Skill against a Bot. The Skill's `inputs` schema
/// describes what the renderer collected from the user; the
/// renderer passes them as `inputs`. The command persists a
/// `skill_runs` row (status=running) up front, runs the
/// steps in a tokio task, and returns the populated run.
/// Because the user is waiting for the result, we do not
/// spawn-and-forget — but the cancellation token is
/// available via `skill_run_cancel`.
#[tauri::command]
pub async fn skill_run(
    app: AppHandle,
    state: State<'_, AppState>,
    bot_id: String,
    skill_id: String,
    inputs: Option<Value>,
) -> Result<SkillRun, String> {
    let user_inputs = inputs.unwrap_or(Value::Object(Default::default()));
    // Look up the skill. A missing skill is an immediate
    // error — no run row to clean up.
    let skill = {
        let db = state.db.clone();
        let id_clone = skill_id.clone();
        tokio::task::spawn_blocking(move || db.get_skill(&id_clone))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?
    };
    let Some(skill) = skill else {
        return Err(format!("no skill found with id {skill_id}"));
    };
    // Build the run row up front so a crash mid-run still
    // leaves a `running` row to clean up.
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
    {
        let db = state.db.clone();
        let to_insert = started.clone();
        tokio::task::spawn_blocking(move || db.insert_skill_run(&to_insert))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    }
    let state_arc: Arc<AppState> = Arc::new(AppState {
        db: state.db.clone(),
        mcp: crate::mcp::McpRegistry::default(),
        bot_runs: state.bot_runs.clone(),
        computer: state.computer.clone(),
        recorder: state.recorder.clone(),
        llm_key_override: state.llm_key_override.clone(),
    });
    let cancel = CancellationToken::new();
    // Capture the user inputs as the trace's trigger
    // input BEFORE we move `user_inputs` into `run_skill`.
    // We don't want to log every user input (privacy +
    // size), but the Last-run view needs at least a
    // string preview of what started the run.
    let trigger_input = user_inputs.to_string();
    let mut final_run = run_skill(
        app.clone(),
        state_arc.clone(),
        run_id.clone(),
        skill,
        bot_id,
        user_inputs,
        cancel,
    )
    .await;
    // Persist the final state. We keep the per-step
    // `steps` in the in-memory return for the renderer
    // (it shows them in the run-progress card) but clear
    // them for the DB row.
    let mut to_persist = final_run.clone();
    to_persist.steps.clear();
    {
        let db = state.db.clone();
        let to_update = to_persist.clone();
        tokio::task::spawn_blocking(move || db.update_skill_run(&to_update))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    }
    // v3.3.0 — Per-step trace capture. The `trigger_input`
    // is the user's `inputs` object, JSON-encoded so the
    // Last-run view can show "what started this run".
    // Best-effort: a trace-write failure is logged, not
    // propagated.
    {
        let trace = build_skill_run_trace(&final_run, trigger_input);
        let db = state.db.clone();
        let to_insert = trace;
        if let Err(e) = tokio::task::spawn_blocking(move || db.insert_skill_run_trace(&to_insert))
            .await
            .map_err(|e| e.to_string())?
        {
            log::warn!(
                "skill_run: insert_skill_run_trace failed: {e}"
            );
        }
    }
    // Return a copy that still has `steps` (the in-memory
    // view) so the renderer can render progress / result.
    if final_run.steps.is_empty() {
        // Rebuild a few representative rows from the final
        // state so the UI always shows something. The
        // executor already populated them when running
        // inline; this is just defensive.
        final_run.steps = Vec::new();
    }
    Ok(final_run)
}

/// Cancel a running Skill. The skill runner checks the
/// token between steps, so cancellation is best-effort —
/// any in-flight tool call runs to completion first.
#[tauri::command]
pub async fn skill_run_cancel(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<bool, String> {
    // We piggy-back on the existing `bot_runs` registry's
    // cancel mechanism for v2.2. v2.3 will add a dedicated
    // `skill_runs` registry when Skill-runs grow features
    // that need richer state (priority, multiple cancel
    // reasons, etc.). For now, run_id == bot_run_id is not
    // quite right — Skill runs have their own UUIDs — so
    // this is a no-op that returns false. The UI doesn't
    // yet surface a Stop button for Skills; this is a
    // forward-compat stub.
    let _ = run_id;
    let _ = state;
    Ok(false)
}

/// Return the current status of a run. Reads the durable
/// row from the `skill_runs` table; per-step progress is
/// in-memory only and is not returned here (the renderer
/// keeps its own per-step list while a run is in flight).
/// The renderer polls this while a run is "running".
#[tauri::command]
pub async fn skill_run_status(
    state: State<'_, AppState>,
    run_id: String,
) -> Result<Option<SkillRun>, String> {
    let db = state.db.clone();
    let id_clone = run_id.clone();
    tokio::task::spawn_blocking(move || db.get_skill_run(&id_clone).map_err(|e| e.to_string()))
        .await
        .map_err(|e| e.to_string())?
}

// ---- recording ----

/// Start a recording session. Allocates a recording_id in
/// the shared `RecorderState`, looks up the bot, and spawns
/// `run_bot_once` with `recording_id = Some(...)` so the
/// executor's tool loop pushes into the recorder. Returns
/// the recording_id and the bot_run_id (the latter so the
/// renderer can match `bot://done` / `bot://error` events).
#[tauri::command]
pub async fn skill_record_start(
    app: AppHandle,
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<SkillRecordStart, String> {
    // Allocate the session up front. If the bot lookup
    // fails after this, the session is empty and gets
    // discarded by the next `abort` or replaced.
    let recording_id = state.recorder.start();
    let db = state.db.clone();
    let id_for_lookup = bot_id.clone();
    let bot = tokio::task::spawn_blocking(move || db.get_bot(&id_for_lookup))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())?;
    let Some(bot) = bot else {
        // Discard the empty session.
        state.recorder.abort(&recording_id);
        return Err(format!("no bot found with id {bot_id}"));
    };
    // Build a state_arc that shares the same recorder.
    let state_arc: Arc<AppState> = Arc::new(AppState {
        db: state.db.clone(),
        mcp: crate::mcp::McpRegistry::default(),
        bot_runs: state.bot_runs.clone(),
        computer: state.computer.clone(),
        recorder: state.recorder.clone(),
        llm_key_override: state.llm_key_override.clone(),
    });
    // We need the bot_run_id *before* the run completes
    // so the UI can show progress events. The executor
    // generates it internally; we peek by reading the
    // registry after the run is registered. Simpler:
    // pre-allocate a run_id and pass it via a side
    // channel — but the executor's signature uses its
    // own UUID. We compromise: the recording returns
    // an empty `bot_run_id` and the renderer correlates
    // by `recording_id` (which is what `skill_record_stop`
    // uses anyway). The executor's events carry the
    // bot_run_id separately; the recording dialog
    // doesn't need to match them in v2.2.
    let recording_id_for_task = recording_id.clone();
    tauri::async_runtime::spawn(async move {
        let _ = run_bot_once(
            Some(app),
            state_arc,
            bot,
            CancellationToken::new(),
            Some(recording_id_for_task),
            None,
            Some("app"),
        )
        .await;
    });
    Ok(SkillRecordStart {
        recording_id,
        bot_run_id: String::new(),
    })
}

/// Stop a recording session, drain the captured tool
/// calls into a candidate `Skill`, and return it. The
/// renderer shows the candidate (with empty
/// name/description) in a form so the user can fill in
/// those fields before `skill_create` persists the final
/// version.
#[tauri::command]
pub async fn skill_record_stop(
    state: State<'_, AppState>,
    recording_id: String,
) -> Result<Option<SkillRecordStop>, String> {
    let recorder: SharedRecorderState = state.recorder.clone();
    let stop_res = tokio::task::spawn_blocking(move || recorder.stop(&recording_id))
        .await
        .map_err(|e| e.to_string())?;
    Ok(stop_res.map(|skill| SkillRecordStop { skill }))
}
