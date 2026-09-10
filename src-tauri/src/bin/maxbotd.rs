//! `maxbotd` — the always-on daemon binary.
//!
//! v2.8.0 ships the first always-on daemon for MaxBot.
//! This binary runs on the user's Linux server (`crispy`)
//! alongside the existing `maxbot` Tauri app on the Mac.
//! Both share the same SQLite file (the `--db` arg) and
//! the same `maxbot_lib` source tree.
//!
//! Responsibilities:
//!
//! 1. **Scheduler poll.** Every 30s, walk `bot_schedules`
//!    for anything due, then call the same
//!    `run_bot_once` that the in-app scheduler uses.
//!    The Tauri app's AppHandle isn't available in this
//!    process — `run_bot_once` accepts `Option<AppHandle>`
//!    for exactly this case (v2.8.0) and skips the
//!    `bot://chunk` / `bot://done` / `bot://error` event
//!    emits. The persisted `bot_runs` row is the source
//!    of truth; the Mac app sees it via the ActivityFeed
//!    on next open / poll.
//!
//! 2. **Webhook server.** `POST /hooks/<bot_id>` on
//!    `0.0.0.0:8443` by default. The `Authorization:
//!    Bearer <token>` header is verified against the
//!    per-Bot token in the `daemon_tokens` SQLite table.
//!    On a 202, the daemon spawns a tokio task that
//!    loads the Bot, appends the webhook body as a
//!    synthetic user message, and runs the Bot via
//!    `run_bot_once` with `None` AppHandle.
//!
//! Out of scope for v2.8.0:
//!
//! - **TLS.** v2.8 listens on plain HTTP. The bearer
//!   token is the only auth. Reverse-proxy with TLS at
//!   the edge (Caddy / nginx) or wait for v3.0.
//! - **Mac notifications.** The daemon is a separate
//!   process; the outbound IPC back to the MaxBot app
//!   needs more design. Tracked as v2.8.1.
//! - **Per-VM `bots/<id>/daemon.json` token storage.**
//!   v2.8 reads tokens from the SQLite `daemon_tokens`
//!   table. The VM-side JSON is a v3.0 polish for
//!   shareable, copyable per-Bot credentials.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{Path, Query, State as AxumState},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use maxbot_lib::bots::executor::run_bot_once;
use maxbot_lib::bots::Bot;
use maxbot_lib::mcp::McpRegistry;
use maxbot_lib::storage::Database;
use maxbot_lib::AppState;

const DEFAULT_BIND: &str = "0.0.0.0:8443";
const DEFAULT_DB: &str = "~/.maxbot/maxbot.db";
const SCHEDULER_TICK_SECONDS: u64 = 30;

/// Shared state for the HTTP server: a single
/// `Arc<Database>` is enough — the executor needs an
/// `Arc<AppState>`, which we build per-request from the
/// same DB + a default `McpRegistry`. (The daemon
/// doesn't run any MCP tools for v2.8 — the per-Bot
/// tool registry is allowed_tools-filtered and the
/// webhook payload is a single user message.)
struct DaemonState {
    db: Arc<Database>,
}

#[derive(Serialize)]
struct AcceptedResponse {
    bot_run_id: String,
    status: &'static str,
}

/// CLI args. We hand-parse instead of pulling in `clap`
/// to keep the dep tree minimal. Flags: `--db <path>`
/// and `--bind <addr>`. Anything else prints help and
/// exits 2.
struct Args {
    db: PathBuf,
    bind: SocketAddr,
}

fn parse_args() -> Result<Args, String> {
    let mut db: Option<PathBuf> = None;
    let mut bind: Option<SocketAddr> = None;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--db" => {
                let v = it.next().ok_or("--db needs a value")?;
                db = Some(expand_tilde(&v));
            }
            "--bind" => {
                let v = it.next().ok_or("--bind needs a value")?;
                bind = Some(v.parse().map_err(|e| format!("--bind: {e}"))?);
            }
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                return Err(format!("unknown arg: {other}"));
            }
        }
    }
    let db = db.unwrap_or_else(|| expand_tilde(DEFAULT_DB));
    let bind = bind.unwrap_or_else(|| {
        DEFAULT_BIND
            .parse()
            .expect("DEFAULT_BIND must parse")
    });
    Ok(Args { db, bind })
}

fn print_help() {
    eprintln!(
        "maxbotd — MaxBot always-on daemon\n\
         \n\
         USAGE:\n  \
             maxbotd [--db <path>] [--bind <addr>]\n\
         \n\
         FLAGS:\n  \
             --db    <path>   SQLite file path (default: {DEFAULT_DB})\n  \
             --bind  <addr>   HTTP listen address (default: {DEFAULT_BIND})\n  \
             -h, --help       Show this help\n\
         \n\
         ROUTES:\n  \
             GET  /health                              Liveness probe (no auth)\n  \
             POST /hooks/<bot_id>                      Auth: Authorization: Bearer <token>\n  \
             GET  /bots/<bot_id>/recent_runs?limit=N   Auth: Authorization: Bearer <token>"
    );
}

/// `~/...` → absolute path. We don't pull in the `dirs`
/// crate for this — `std::env::home_dir` is deprecated
/// but still functional on macOS / Linux, and a daemon
/// only needs the path at startup.
fn expand_tilde(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(p)
}

#[tokio::main]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("maxbotd: {e}");
            print_help();
            std::process::exit(2);
        }
    };

    let db = match Database::open(&args.db) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!(
                "maxbotd: could not open SQLite at {}: {e}",
                args.db.display()
            );
            std::process::exit(1);
        }
    };
    log::info!("maxbotd: db = {}", args.db.display());

    let state = Arc::new(DaemonState { db: db.clone() });

    // Background scheduler: same 30s cadence as the
    // Tauri app's `bots::scheduler`. Reuses the same
    // `run_bot_once` (Option<AppHandle>::None variant).
    spawn_scheduler(db.clone());

    // HTTP server: webhook (POST) + liveness (GET /health,
    // no auth) + per-Bot recent-runs (GET, bearer auth).
    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/hooks/:bot_id", post(handle_webhook))
        .route("/bots/:bot_id/recent_runs", get(handle_recent_runs))
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(args.bind).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("maxbotd: bind {} failed: {e}", args.bind);
            std::process::exit(1);
        }
    };
    log::info!("maxbotd: listening on http://{}", args.bind);

    if let Err(e) = axum::serve(listener, app).await {
        log::error!("maxbotd: server crashed: {e}");
        std::process::exit(1);
    }
}

/// v3.1.0 — Liveness probe. No auth, no DB hit.
/// Returns `{ ok: true, version: <CARGO_PKG_VERSION> }` so
/// a quick `curl` (or a systemd `Type=notify` watcher, or
/// the Mac app's setup doc) can confirm the daemon is up
/// and the build matches expectations. Mirrors the shape
/// the docs use in `docs/maxbotd-setup.md`.
async fn handle_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

/// v3.1.0 — Per-Bot recent-runs. Bearer-authenticated like
/// `/hooks/<bot_id>`. Returns the most-recent `limit` rows
/// for the given Bot (default 20, capped at 200 by
/// `Database::list_bot_runs`). Used by the Mac app's
/// BotEditor "Test webhook" verification flow and the
/// external `curl` examples in `docs/maxbotd-setup.md`.
async fn handle_recent_runs(
    Path(bot_id): Path<String>,
    AxumState(state): AxumState<Arc<DaemonState>>,
    headers: HeaderMap,
    Query(params): Query<RecentRunsParams>,
) -> impl IntoResponse {
    let token = match extract_bearer(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "missing or malformed Authorization header" })),
            )
                .into_response();
        }
    };
    let db = state.db.clone();
    let bot_id_for_auth = bot_id.clone();
    let stored = match tokio::task::spawn_blocking(move || {
        db.get_daemon_token(&bot_id_for_auth)
    })
    .await
    {
        Ok(Ok(Some(t))) if t == token => t,
        Ok(Ok(Some(_))) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid token" })),
            )
                .into_response();
        }
        Ok(Ok(None)) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "no daemon token configured for this bot" })),
            )
                .into_response();
        }
        Ok(Err(e)) => {
            log::error!("maxbotd: db error during token lookup: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "db error" })),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("maxbotd: token lookup task panicked: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
                .into_response();
        }
    };
    drop(stored);
    let limit = params.limit.unwrap_or(20).clamp(1, 200);
    let db = state.db.clone();
    let bot_id_for_list = bot_id.clone();
    let runs = match tokio::task::spawn_blocking(move || {
        db.list_bot_runs(&bot_id_for_list, limit)
    })
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            log::error!("maxbotd: list_bot_runs failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "db error" })),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("maxbotd: list_bot_runs task panicked: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
                .into_response();
        }
    };
    (StatusCode::OK, Json(runs)).into_response()
}

/// v3.1.0 — Query string for `handle_recent_runs`. `limit`
/// is optional; default 20. Clamped server-side.
#[derive(serde::Deserialize)]
struct RecentRunsParams {
    limit: Option<u32>,
}

/// Webhook handler. Returns 202 Accepted with
/// `{ bot_run_id, status: "accepted" }` on success.
/// The actual Bot run happens asynchronously in a
/// spawned tokio task — the HTTP response is the
/// receipt, not the run output.
async fn handle_webhook(
    Path(bot_id): Path<String>,
    AxumState(state): AxumState<Arc<DaemonState>>,
    headers: HeaderMap,
    body: String,
) -> impl IntoResponse {
    // 1. Pull the bearer token from the Authorization
    //    header. Reject anything missing / malformed.
    let token = match extract_bearer(&headers) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "missing or malformed Authorization header" })),
            )
                .into_response();
        }
    };

    // 2. Verify the token against the daemon_tokens row.
    //    DB lookup is sync; run on the blocking pool.
    let db = state.db.clone();
    let bot_id_for_auth = bot_id.clone();
    let stored = match tokio::task::spawn_blocking(move || {
        db.get_daemon_token(&bot_id_for_auth)
    })
    .await
    {
        Ok(Ok(Some(t))) if t == token => t,
        Ok(Ok(Some(_))) => {
            // Some(token) but didn't match the
            // header — invalid bearer.
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "invalid token" })),
            )
                .into_response();
        }
        Ok(Ok(None)) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "no daemon token configured for this bot" })),
            )
                .into_response();
        }
        Ok(Err(e)) => {
            log::error!("maxbotd: db error during token lookup: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "db error" })),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("maxbotd: token lookup task panicked: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
                .into_response();
        }
    };
    drop(stored);

    // 3. Stamp the token's last_used timestamp. Best-
    //    effort; a DB error here is logged but doesn't
    //    fail the request.
    let db = state.db.clone();
    let bot_id_for_touch = bot_id.clone();
    let _ = tokio::task::spawn_blocking(move || {
        if let Err(e) = db.touch_daemon_token(&bot_id_for_touch) {
            log::warn!("maxbotd: touch_daemon_token failed: {e}");
        }
    })
    .await;

    // 4. Spawn the run asynchronously. We hand the
    //    caller the synthetic bot_run_id from a fresh
    //    BotRun row; the executor's run will overwrite
    //    the status to "running" → "succeeded" / "failed"
    //    as it goes. Inserting the row up front lets the
    //    ActivityFeed show "running" while the LLM is
    //    mid-stream.
    let db = state.db.clone();
    let bot_id_for_lookup = bot_id.clone();
    let bot = match tokio::task::spawn_blocking(move || {
        db.get_bot(&bot_id_for_lookup)
    })
    .await
    {
        Ok(Ok(Some(b))) => b,
        Ok(Ok(None)) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no bot with that id" })),
            )
                .into_response();
        }
        Ok(Err(e)) => {
            log::error!("maxbotd: db error during bot lookup: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "db error" })),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("maxbotd: bot lookup task panicked: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
                .into_response();
        }
    };

    // 5. Persist the synthetic user message + a
    //    `running` bot_runs row. The executor's
    //    `run_bot_once` will rewrite the run row's
    //    status when it finishes.
    let run_id = uuid::Uuid::new_v4().to_string();
    let conversation_id = match ensure_conversation(&state.db, &bot, &body).await {
        Ok(id) => id,
        Err(e) => {
            log::error!("maxbotd: ensure_conversation failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "could not create conversation" })),
            )
                .into_response();
        }
    };

    let db = state.db.clone();
    let bot_id_for_run = bot.id.clone();
    let run_id_for_row = run_id.clone();
    let conversation_id_for_row = conversation_id.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let run = maxbot_lib::bots::BotRun {
            id: run_id_for_row,
            bot_id: bot_id_for_run,
            conversation_id: conversation_id_for_row,
            status: maxbot_lib::bots::BotRunStatus::Running,
            started_at: Utc::now(),
            finished_at: None,
            result_summary: String::new(),
            triggered_by: "webhook".to_string(),
        };
        if let Err(e) = db.upsert_bot_run(&run) {
            log::warn!("maxbotd: upsert_bot_run failed: {e}");
        }
    })
    .await;

    // 6. Hand the run to the executor. The state
    //    here is intentionally minimal: the daemon
    //    doesn't run any MCP servers (the per-Bot
    //    allowed_tools list is enforced by the
    //    executor regardless), and the Computer
    //    manager / recorder are empty defaults.
    let app_state = Arc::new(AppState {
        db: state.db.clone(),
        mcp: McpRegistry::default(),
        bot_runs: Arc::new(maxbot_lib::bots::registry::BotRunRegistry::new()),
        computer: Arc::new(maxbot_lib::computer::ComputerManager::new(
            &maxbot_lib::storage::Settings::default(),
        )),
        recorder: Arc::new(maxbot_lib::skills::recorder::RecorderState::new()),
    });
    let bot_for_task = bot.clone();
    let cancel = CancellationToken::new();
    tokio::spawn(async move {
        log::info!("maxbotd: firing webhook run for bot {}", bot_for_task.id);
        let _ = run_bot_once(
            None,
            app_state,
            bot_for_task,
            cancel,
            None,
            Some(conversation_id),
            Some("webhook"),
        )
        .await;
    });

    (
        StatusCode::ACCEPTED,
        Json(AcceptedResponse {
            bot_run_id: run_id,
            status: "accepted",
        }),
    )
        .into_response()
}

/// Create (or reuse) a fresh conversation for a
/// webhook-fired run, then append the synthetic user
/// message (the webhook body) to it. Reuses the
/// per-Bot conversation for recurring bots when
/// possible — same convention as the in-app
/// `run_bot_once`.
async fn ensure_conversation(
    db: &Arc<Database>,
    bot: &Bot,
    body: &str,
) -> Result<String, String> {
    let db = db.clone();
    let bot_id = bot.id.clone();
    let bot_name = bot.name.clone();
    let body = body.to_string();
    let result: Result<String, String> = tokio::task::spawn_blocking(move || {
        // Reuse the bot's last conversation if it has
        // one. Otherwise create fresh.
        let convo = if let Ok(Some(sched)) = db.get_schedule(&bot_id) {
            if let Some(existing) = sched.last_conversation_id {
                if db
                    .list_conversations()
                    .map(|list| list.iter().any(|c| c.id == existing))
                    .unwrap_or(false)
                {
                    db.touch_conversation(&existing).ok();
                    existing
                } else {
                    create_bot_convo(&db, &bot_id, &bot_name)?
                }
            } else {
                create_bot_convo(&db, &bot_id, &bot_name)?
            }
        } else {
            create_bot_convo(&db, &bot_id, &bot_name)?
        };

        // Append the synthetic user message.
        let msg_id = uuid::Uuid::new_v4().to_string();
        db.insert_message(
            &convo,
            maxbot_lib::storage::MessageRole::User,
            &body,
            &[],
        )
        .map_err(|e| format!("insert_message: {e}"))?;
        // Silence the unused warning for msg_id — the
        // message id is generated by `insert_message`
        // internally. Kept here as a future-proofing
        // breadcrumb for callers that want to capture it.
        let _ = msg_id;
        Ok(convo)
    })
    .await
    .map_err(|e| format!("ensure_conversation task: {e}"))?;
    result
}

fn create_bot_convo(
    db: &Database,
    bot_id: &str,
    bot_name: &str,
) -> Result<String, String> {
    let title = format!("{bot_name} — run");
    let convo = db
        .create_bot_conversation(bot_id, Some(title.clone()))
        .or_else(|_| db.create_conversation(Some(title), Some(bot_id)))
        .map_err(|e| format!("create_conversation: {e}"))?;
    Ok(convo.id)
}

/// Extract the bearer token from an
/// `Authorization: Bearer <token>` header. Returns
/// `None` if the header is missing or doesn't follow
/// the expected shape. Case-insensitive on the
/// scheme; trimming is the caller's problem (we
/// compare bytes-for-bytes with the stored token).
fn extract_bearer(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let mut parts = raw.splitn(2, ' ');
    let scheme = parts.next()?.trim();
    let token = parts.next()?.trim();
    if !scheme.eq_ignore_ascii_case("bearer") || token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// Spawn the 30s scheduler poll. Mirrors the Tauri
/// app's `bots::scheduler::spawn` but uses
/// `None` for the AppHandle. The actual due-check
/// logic calls `Database::list_due_schedules` and
/// then the existing `run_bot_once` (Option::None
/// variant) per schedule.
fn spawn_scheduler(db: Arc<Database>) {
    tokio::spawn(async move {
        let tick = Duration::from_secs(SCHEDULER_TICK_SECONDS);
        loop {
            tokio::time::sleep(tick).await;
            if let Err(e) = scheduler_tick(&db).await {
                log::warn!("maxbotd: scheduler tick failed: {e}");
            }
        }
    });
}

async fn scheduler_tick(db: &Arc<Database>) -> Result<(), String> {
    // Clone once so we can move into the lookup
    // closure and still have an `Arc<Database>` for
    // the for-loop iterations.
    let db_for_list: Arc<Database> = Arc::clone(db);
    let due: Vec<maxbot_lib::bots::BotSchedule> =
        tokio::task::spawn_blocking(move || db_for_list.list_due_schedules(Utc::now()))
            .await
            .map_err(|e| format!("list_due_schedules task: {e}"))?
            .map_err(|e| format!("list_due_schedules: {e}"))?;
    if due.is_empty() {
        return Ok(());
    }
    log::info!("maxbotd: scheduler tick — {} schedule(s) due", due.len());
    for sched in due {
        let db_for_get: Arc<Database> = Arc::clone(db);
        let bot_id = sched.bot_id.clone();
        let bot = match tokio::task::spawn_blocking(move || db_for_get.get_bot(&bot_id))
            .await
            .map_err(|e| format!("get_bot task: {e}"))?
        {
            Ok(Some(b)) => b,
            _ => continue,
        };
        // Minimal AppState — same as the webhook
        // handler. The executor enforces the per-Bot
        // allowed_tools filter regardless of the
        // registry contents.
        let app_state = Arc::new(AppState {
            db: Arc::clone(db),
            mcp: McpRegistry::default(),
            bot_runs: Arc::new(maxbot_lib::bots::registry::BotRunRegistry::new()),
            computer: Arc::new(maxbot_lib::computer::ComputerManager::new(
                &maxbot_lib::storage::Settings::default(),
            )),
            recorder: Arc::new(maxbot_lib::skills::recorder::RecorderState::new()),
        });
        let cancel = CancellationToken::new();
        tokio::spawn(async move {
            log::info!("maxbotd: firing scheduled bot {}", bot.id);
            let _ = run_bot_once(
                None,
                app_state,
                bot,
                cancel,
                None,
                None,
                Some("daemon"),
            )
            .await;
        });
    }
    Ok(())
}

// ----- tests -----

#[cfg(test)]
mod tests {
    use super::*;

    /// `daemon_token_round_trip` — set a token, then
    /// fetch it back. Confirms the SQLite path
    /// end-to-end on a temp file.
    #[test]
    fn daemon_token_round_trip() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("test.db");
        let db = Database::open(&path).expect("open");
        db.set_daemon_token("bot-a", "tok-1").expect("set");
        let got = db.get_daemon_token("bot-a").expect("get");
        assert_eq!(got.as_deref(), Some("tok-1"));
        // A different bot's token is unaffected.
        let other = db.get_daemon_token("bot-b").expect("get other");
        assert!(other.is_none());
    }

    /// `rotate_daemon_token_invalidates_old` — calling
    /// rotate twice gives two different tokens; the
    /// second one is what's stored, not the first.
    #[test]
    fn rotate_daemon_token_invalidates_old() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("test.db");
        let db = Database::open(&path).expect("open");
        let first = db.rotate_daemon_token("bot-x").expect("rotate 1");
        let second = db.rotate_daemon_token("bot-x").expect("rotate 2");
        assert_ne!(first, second, "rotate must produce a new token");
        assert_eq!(first.len(), 64, "32 bytes hex-encoded = 64 chars");
        assert_eq!(second.len(), 64);
        let stored = db.get_daemon_token("bot-x").expect("get");
        assert_eq!(stored.as_deref(), Some(second.as_str()));
    }

    /// `webhook_auth_rejects_missing_token` — start the
    /// HTTP server on a random port, hit
    /// `POST /hooks/<id>` with no Authorization header,
    /// assert 401. Uses a fresh DB so no token is set.
    #[tokio::test]
    async fn webhook_auth_rejects_missing_token() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("test.db");
        let db = Arc::new(Database::open(&path).expect("open"));
        // Seed a bot so the 401 is "missing token", not
        // "no bot".
        let bot = Bot {
            id: "bot-401".to_string(),
            name: "test".to_string(),
            description: String::new(),
            system_prompt: String::new(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: String::new(),
            color: String::new(),
            avatar_color: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            state: maxbot_lib::bots::BotState::Idle,
            last_active_at: None,
        };
        db.upsert_bot(&bot).expect("upsert bot");
        // Configure a token so the path is "missing
        // header" (401) and not "no token configured".
        db.set_daemon_token("bot-401", "secret").expect("set");

        let state = Arc::new(DaemonState { db: db.clone() });
        let app = Router::new()
            .route("/hooks/:bot_id", post(handle_webhook))
            .with_state(state);
        // Bind to 127.0.0.1:0 so the OS picks a free
        // port — keeps the test parallel-safe.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        // No Authorization header → 401.
        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/hooks/bot-401"))
            .body("hello")
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Malformed Authorization header → 401.
        let resp = client
            .post(format!("http://{addr}/hooks/bot-401"))
            .header("Authorization", "Basic abc")
            .body("hello")
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        server.abort();
    }

    /// `extract_bearer` parses the standard shape and
    /// rejects everything else. Cheap; runs in
    /// milliseconds.
    #[test]
    fn extract_bearer_parses_standard_shape() {
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer abc.def.ghi".parse().unwrap());
        assert_eq!(extract_bearer(&h).as_deref(), Some("abc.def.ghi"));
        // Case-insensitive scheme.
        let mut h = HeaderMap::new();
        h.insert("authorization", "bearer token-1".parse().unwrap());
        assert_eq!(extract_bearer(&h).as_deref(), Some("token-1"));
        // Missing header.
        let h = HeaderMap::new();
        assert!(extract_bearer(&h).is_none());
        // Wrong scheme.
        let mut h = HeaderMap::new();
        h.insert("authorization", "Basic abc".parse().unwrap());
        assert!(extract_bearer(&h).is_none());
        // Empty token.
        let mut h = HeaderMap::new();
        h.insert("authorization", "Bearer ".parse().unwrap());
        assert!(extract_bearer(&h).is_none());
    }

    /// `daemon_webhook_round_trip_30s` — start the
    /// daemon on a random local port, hit
    /// `POST /hooks/<bot_id>` with the correct bearer
    /// token, assert the response is 202 Accepted
    /// with a `bot_run_id`. The Bot run itself is
    /// then either polled to completion or the 30s
    /// budget elapses (the LLM call is fake-keyed in
    /// this test; with a real API key the run
    /// completes in seconds, but we don't require
    /// that here). `#[ignore]`'d so it doesn't run in
    /// normal `cargo test --lib`.
    ///
    /// Manual run:
    ///   cargo test --bin maxbotd \
    ///       daemon_webhook_round_trip_30s -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn daemon_webhook_round_trip_30s() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("test.db");
        let db = Arc::new(Database::open(&path).expect("open"));
        let bot = Bot {
            id: "bot-rt".to_string(),
            name: "round-trip".to_string(),
            description: String::new(),
            system_prompt: "You are a tiny echo bot. Reply with exactly: pong".to_string(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: String::new(),
            color: String::new(),
            avatar_color: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            state: maxbot_lib::bots::BotState::Idle,
            last_active_at: None,
        };
        db.upsert_bot(&bot).expect("upsert bot");
        let token = db.rotate_daemon_token("bot-rt").expect("rotate");
        // Seed a Settings row with a fake API key. The
        // executor will get past the "no API key" guard
        // and then either succeed (real key) or fail
        // (fake key) on the LLM call. Either way, the
        // webhook + auth path is what we're proving.
        let mut settings = maxbot_lib::storage::Settings::default();
        settings.minimax_api_key = Some("test-key".to_string());
        db.save_settings(&settings).expect("settings");

        let state = Arc::new(DaemonState { db: db.clone() });
        let app = Router::new()
            .route("/hooks/:bot_id", post(handle_webhook))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/hooks/bot-rt"))
            .header("Authorization", format!("Bearer {token}"))
            .body("ping")
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), StatusCode::ACCEPTED);
        let body: serde_json::Value = resp.json().await.expect("json");
        let run_id = body
            .get("bot_run_id")
            .and_then(|v| v.as_str())
            .expect("bot_run_id")
            .to_string();
        assert!(!run_id.is_empty());

        // Poll for up to 30s. We don't assert the run
        // reached a terminal state — that depends on
        // the LLM provider, and a fake key in a test
        // environment will hang the LLM call. The
        // webhook + auth proof is the 202 + bot_run_id
        // assertion above. This loop just verifies the
        // bot_runs row was persisted (which is
        // independent of the LLM call).
        let mut persisted = false;
        for _ in 0..30 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let id = run_id.clone();
            let db = db.clone();
            let run: Option<maxbot_lib::bots::BotRun> =
                tokio::task::spawn_blocking(move || db.get_bot_run(&id))
                    .await
                    .ok()
                    .and_then(|r| r.ok().flatten());
            if run.is_some() {
                persisted = true;
                break;
            }
        }
        assert!(persisted, "bot_runs row was not persisted within 30s");

        server.abort();
    }
}

// Keep the unused-import warning quiet on platforms
// where the test module pulls in extra symbols but
// doesn't use them. The actual usage is in the
// handler.
#[allow(dead_code)]
fn _silence_unused_mutex_warning() -> Mutex<()> {
    Mutex::new(())
}
