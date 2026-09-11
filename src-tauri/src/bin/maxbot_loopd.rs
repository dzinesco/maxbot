//! `maxbot_loopd` — the keep-alive loop supervisor binary.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use maxbot_lib::r#loop::daemon::{
    io::FileLoopIO, model::NoopModelCaller, run_supervisor,
    LockError, LoopError, SupervisorConfig,
};

struct Args {
    loop_dir: PathBuf,
    heartbeat_interval: Duration,
    log_level: String,
}

fn parse_args() -> Result<Args, String> {
    let mut loop_dir: Option<PathBuf> = None;
    let mut heartbeat_secs: u64 = 5;
    let mut log_level = "info".to_string();
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--loop-dir" => {
                let v = iter.next().ok_or("--loop-dir requires a value")?;
                loop_dir = Some(PathBuf::from(v));
            }
            "--heartbeat-secs" => {
                let v = iter.next().ok_or("--heartbeat-secs requires a value")?;
                heartbeat_secs = v.parse::<u64>().map_err(|e| format!("--heartbeat-secs: {e}"))?;
            }
            "--log-level" => {
                log_level = iter.next().ok_or("--log-level requires a value")?.to_string();
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    let loop_dir = loop_dir.ok_or("--loop-dir is required")?;
    Ok(Args {
        loop_dir,
        heartbeat_interval: Duration::from_secs(heartbeat_secs),
        log_level,
    })
}

fn print_help() {
    eprintln!(
        "maxbot_loopd — keep-alive loop supervisor\n\n\
         USAGE:\n  maxbot_loopd --loop-dir <path> [--heartbeat-secs N] [--log-level LEVEL]\n\n\
         EXIT CODES:\n  \
             0  success\n  \
             2  lock held by another live pid\n  \
             1  any other error"
    );
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("maxbot_loopd: {e}");
            return ExitCode::from(1);
        }
    };
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(&args.log_level),
    )
    .init();
    let io = Arc::new(FileLoopIO::new(&args.loop_dir));
    let model = Arc::new(NoopModelCaller);
    let cfg = SupervisorConfig { heartbeat_interval: args.heartbeat_interval };
    match run_supervisor(io, model, cfg) {
        Ok(outcome) => { log::info!("maxbot_loopd: outcome = {outcome:?}"); ExitCode::SUCCESS }
        Err(LoopError::Lock(LockError::Held(pid))) => {
            log::info!("maxbot_loopd: lock held by pid {pid}; exiting cleanly");
            ExitCode::from(2)
        }
        Err(e) => { log::error!("maxbot_loopd: error: {e}"); ExitCode::from(1) }
    }
}
