//! The supervisor — the keep-alive loop's per-process brain.

use std::sync::Arc;

use chrono::Utc;

use super::io::LoopIO;
use super::model::{ModelCaller, ModelError};
use super::state::{JournalEntry, LockError, LoopError, State, TaskStatus};

#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub heartbeat_interval: std::time::Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: std::time::Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Idle,
    Turn { turn: u64, action_id: String, new_status: TaskStatus },
    TurnErrored { turn: u64, action_id: String, error: String },
}

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn fresh_state(my_pid: u32) -> State {
    State::new(my_pid, now_rfc3339(), uuid::Uuid::new_v4().to_string())
}

fn decide_is_idle(task: &super::state::Task) -> bool {
    match task.status {
        TaskStatus::Done => true,
        TaskStatus::Idle => task.body.trim().is_empty(),
        TaskStatus::Running | TaskStatus::Blocked => false,
    }
}

pub fn run_one_turn<I, M>(
    io: &I,
    model: &M,
    _cfg: &SupervisorConfig,
) -> Result<TurnOutcome, LoopError>
where I: LoopIO, M: ModelCaller,
{
    let task = io.read_task()?;
    if decide_is_idle(&task) {
        let mut state = io.read_state().unwrap_or_else(|_| fresh_state(std::process::id()));
        state.last_heartbeat = now_rfc3339();
        io.write_state(&state)?;
        return Ok(TurnOutcome::Idle);
    }
    let mut state = io.read_state().unwrap_or_else(|_| fresh_state(std::process::id()));
    state.turn = state.turn.saturating_add(1);
    let action_id = uuid::Uuid::new_v4().to_string();
    state.last_action_id = Some(action_id.clone());
    state.last_heartbeat = now_rfc3339();
    io.write_state(&state)?;
    io.update_task_status(TaskStatus::Running)?;
    let memory = io.read_memory().unwrap_or_default();
    match model.call(&task, &memory) {
        Ok(output) => {
            io.append_journal(&JournalEntry {
                turn: state.turn, at: now_rfc3339(),
                actions: output.actions.clone(), errors: Vec::new(), note: None,
            })?;
            if !output.facts.is_empty() { io.append_memory(&output.facts)?; }
            io.update_task_status(output.new_status)?;
            io.heartbeat()?;
            Ok(TurnOutcome::Turn { turn: state.turn, action_id, new_status: output.new_status })
        }
        Err(ModelError::Backend(err)) => {
            io.append_journal(&JournalEntry {
                turn: state.turn, at: now_rfc3339(),
                actions: Vec::new(), errors: vec![err.clone()],
                note: Some("model backend error".to_string()),
            })?;
            io.heartbeat()?;
            Ok(TurnOutcome::TurnErrored { turn: state.turn, action_id, error: err })
        }
    }
}

pub fn run_supervisor<I, M>(
    io: Arc<I>, model: Arc<M>, cfg: SupervisorConfig,
) -> Result<TurnOutcome, LoopError>
where I: LoopIO + 'static, M: ModelCaller + 'static,
{
    if let Err(LockError::Held(pid)) = io.acquire_lock() {
        return Err(LockError::Held(pid).into());
    }
    let result = run_one_turn(io.as_ref(), model.as_ref(), &cfg);
    let _ = io.release_lock();
    result
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use super::super::io::FileLoopIO;
    use super::super::model::NoopModelCaller;
    use super::super::state::{Task, TaskStatus};
    use super::*;

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!(
            "maxbot-sup-{}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0),
            n,
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn idle_task_exits_idle() {
        let dir = tempdir();
        let io = Arc::new(FileLoopIO::new(&dir));
        let model = Arc::new(NoopModelCaller);
        assert_eq!(
            run_supervisor(io.clone(), model.clone(), SupervisorConfig::default()).unwrap(),
            TurnOutcome::Idle
        );
    }

    #[test]
    fn done_task_exits_idle() {
        let dir = tempdir();
        let io = Arc::new(FileLoopIO::new(&dir));
        let model = Arc::new(NoopModelCaller);
        io.write_task(&Task { status: TaskStatus::Done, body: "done".into(), updated_at: String::new() }).unwrap();
        assert_eq!(
            run_supervisor(io.clone(), model.clone(), SupervisorConfig::default()).unwrap(),
            TurnOutcome::Idle
        );
    }

    #[test]
    fn running_task_runs_turn_bumps_state() {
        let dir = tempdir();
        let io = Arc::new(FileLoopIO::new(&dir));
        let model = Arc::new(NoopModelCaller);
        io.write_task(&Task { status: TaskStatus::Running, body: "x".into(), updated_at: String::new() }).unwrap();
        io.write_state(&State {
            run_id: "r1".into(), turn: 5, pid: io.my_pid,
            started_at: "2026-09-11T09:00:00Z".into(),
            last_heartbeat: "2026-09-11T09:00:00Z".into(),
            last_action_id: Some("uuid-1".into()),
        }).unwrap();
        let out = run_supervisor(io.clone(), model.clone(), SupervisorConfig::default()).unwrap();
        match out {
            TurnOutcome::Turn { turn, new_status, .. } => {
                assert_eq!(turn, 6);
                assert_eq!(new_status, TaskStatus::Done);
            }
            other => panic!("expected Turn, got {other:?}"),
        }
        let s = io.read_state().unwrap();
        assert_eq!(s.turn, 6);
        assert_ne!(s.last_action_id.as_deref(), Some("uuid-1"));
    }

    #[test]
    fn blocked_task_runs_turn_keeps_blocked() {
        let dir = tempdir();
        let io = Arc::new(FileLoopIO::new(&dir));
        let model = Arc::new(NoopModelCaller);
        io.write_task(&Task { status: TaskStatus::Blocked, body: "needs input".into(), updated_at: String::new() }).unwrap();
        let out = run_supervisor(io.clone(), model.clone(), SupervisorConfig::default()).unwrap();
        match out {
            TurnOutcome::Turn { new_status, .. } => assert_eq!(new_status, TaskStatus::Blocked),
            other => panic!("expected Turn, got {other:?}"),
        }
    }

    #[test]
    fn lock_held_by_foreign_pid() {
        let dir = tempdir();
        std::fs::write(dir.join("LOCK"), format!("pid={}\n", std::process::id())).unwrap();
        let mut io_c = FileLoopIO::new(&dir);
        io_c.my_pid = 9_999_997;
        assert!(matches!(io_c.acquire_lock(), Err(LockError::Held(_))));
    }

    #[test]
    fn turn_writes_journal() {
        let dir = tempdir();
        let io = Arc::new(FileLoopIO::new(&dir));
        let model = Arc::new(NoopModelCaller);
        io.write_task(&Task { status: TaskStatus::Running, body: "x".into(), updated_at: String::new() }).unwrap();
        run_supervisor(io.clone(), model.clone(), SupervisorConfig::default()).unwrap();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert!(dir.join("journal").join(format!("{today}.md")).exists());
    }

    #[test]
    fn lock_released_after_turn() {
        let dir = tempdir();
        let io = Arc::new(FileLoopIO::new(&dir));
        let model = Arc::new(NoopModelCaller);
        io.write_task(&Task { status: TaskStatus::Running, body: "x".into(), updated_at: String::new() }).unwrap();
        run_supervisor(io.clone(), model.clone(), SupervisorConfig::default()).unwrap();
        assert!(!dir.join("LOCK").exists());
    }
}
