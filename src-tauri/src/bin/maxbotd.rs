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
    extract::{Path, Query, Request, State as AxumState},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use maxbot_lib::bots::executor::run_bot_once;
use maxbot_lib::bots::Bot;
use maxbot_lib::mcp::McpRegistry;
use maxbot_lib::storage::Database;
use maxbot_lib::tools::shared_fs::{self, SharedEntry};
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
             GET  /bots/<bot_id>/recent_runs?limit=N   Auth: Authorization: Bearer <token>\n  \
             POST /shared?bot_id=<bot_id>              Auth: Authorization: Bearer <token>\n  \
                                                     Body: {{verb: read|write|list, path?, content?}}\n  \
                                                     Routes the Mac app's shared_* tools to the\n  \
                                                     daemon's ~/bots/_shared/ (v3.7.1)"
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
    // no auth) + per-Bot recent-runs (GET, bearer auth) +
    // v3.7.1 shared-folder (POST /shared, bearer auth,
    // body-driven verb). The CORS layer is added at the
    // outer level so every response (including 401s and
    // error JSONs) carries `Access-Control-Allow-Origin: *`,
    // unblocking the in-app Test-webhook button in
    // `BotEditor.tsx` (v3.1.0) and the in-app shared_*
    // tool calls (v3.7.1) from the same-origin webview.
    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/hooks/:bot_id", post(handle_webhook))
        .route("/bots/:bot_id/recent_runs", get(handle_recent_runs))
        .route("/shared", post(handle_shared))
        .layer(middleware::from_fn(cors_layer))
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

/// v3.7.1 — CORS middleware. Adds
/// `Access-Control-Allow-Origin: *` to every response
/// so the Mac app's Tauri webview (a same-origin but
/// different-port origin) and a curl / external
/// client on the LAN can both hit the daemon routes
/// without the browser blocking the response.
///
/// Why the manual header instead of `tower-http`:
/// the daemon has a single-purpose dep tree and the
/// only CORS concern is "let any origin through" —
/// `tower_http::cors::CorsLayer` would pull in a
/// new crate, a wildcard configuration, and an
/// extra surface area for a self-hosted personal
/// tool. A future slice can swap in a stricter
/// origin list if the daemon is exposed beyond
/// Tyler's LAN.
///
/// The middleware is wired at the outer Router
/// level (not per-route) so 401s and error JSONs
/// also carry the header — the browser checks the
/// header on every response, including the ones
/// the auth path returns.
async fn cors_layer(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    resp.headers_mut().insert(
        axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        axum::http::HeaderValue::from_static("*"),
    );
    resp
}

// =====================================================================
//  v3.7.1 — Shared-folder route (POST /shared)
// =====================================================================
//
// Body shape (JSON):
//   {
//     "verb":    "read" | "write" | "list",
//     "path":    "relative/path/under/shared/",  // string, optional for list
//     "content": "..."                           // required for "write"
//   }
//
// Auth: same `Authorization: Bearer <token>` as the
// webhook + recent-runs routes. The token is verified
// against the per-Bot `daemon_tokens.bot_id` row.
//
// The route delegates to the same `resolve_safe_path`
// + read/write/list code that the in-app
// `tools::shared_fs` tool uses today. The guard is
// the load-bearing piece — it refuses absolute paths,
// `..`, Windows drive letters, UNC shares,
// backslashes, and symlink escapes. A single
// re-implementation in the daemon would be a
// second source of truth and a path to "weakened in
// one place" regressions; instead, the daemon
// calls the same public functions the tool uses.
//
// The path-safety guard's job is the only thing
// standing between a Bot's LLM and the host
// filesystem, so a future slice that wants to
// shortcut the guard on the daemon side will be
// loudly rejected. The guard is non-negotiable.

/// v3.7.1 — Body for `POST /shared`. `verb` picks the
/// operation; `path` is the relative path under the
/// shared root; `content` is the new file body for
/// `write`. The `bot_id` is sent as a query param so
/// the same token-lookup machinery the other
/// authenticated routes use can find the per-Bot
/// token row without parsing it out of the JSON body.
#[derive(Deserialize)]
struct SharedRequest {
    verb: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    content: Option<String>,
}

/// v3.7.1 — Query string for `POST /shared`. `bot_id`
/// pins the token lookup (one Bot per route call).
#[derive(Deserialize)]
struct SharedParams {
    bot_id: String,
}

/// v3.7.1 — `POST /shared` handler. The verb in the
/// body picks `read`, `write`, or `list`. The auth
/// path mirrors `handle_recent_runs` exactly — a
/// missing token, a wrong token, and a missing
/// `daemon_tokens` row all surface as 401 with a
/// JSON body so the renderer can show a meaningful
/// error.
async fn handle_shared(
    AxumState(state): AxumState<Arc<DaemonState>>,
    headers: HeaderMap,
    Query(params): Query<SharedParams>,
    Json(body): Json<SharedRequest>,
) -> Response {
    // 1. Bearer auth. Same shape as the other
    //    authenticated routes; we don't re-implement
    //    the verification loop here.
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
    let bot_id_for_auth = params.bot_id.clone();
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
            log::error!("maxbotd: shared token lookup db error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "db error" })),
            )
                .into_response();
        }
        Err(e) => {
            log::error!("maxbotd: shared token lookup task panicked: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "internal error" })),
            )
                .into_response();
        }
    };
    drop(stored);

    // 2. Dispatch on the verb. The shared root is
    //    resolved here so the same `MAXBOT_SHARED_DIR`
    //    override used by the in-app tool is honored.
    let root = shared_fs::shared_root();
    match body.verb.as_str() {
        "list" => handle_shared_list(&root, &body.path).await,
        "read" => handle_shared_read(&root, &body.path).await,
        "write" => match body.content.as_deref() {
            Some(c) => handle_shared_write(&root, &body.path, c).await,
            None => (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "write requires `content` in body" })),
            )
                .into_response(),
        },
        other => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": format!("unknown verb: {other}") })),
        )
            .into_response(),
    }
}

/// v3.7.1 — `POST /shared` `list` verb. Resolves
/// `path` through the path-safety guard and returns
/// the directory listing as `[{ name, is_file, size,
/// modified_at }]`. Mirrors the in-app
/// `SharedListTool::execute` body but with the
/// daemon's response shape (the in-app tool
/// pretty-prints JSON; the daemon returns the raw
/// entries array so the Mac app can render its own
/// UI).
async fn handle_shared_list(root: &std::path::Path, path: &str) -> Response {
    // v3.7.1 — treat an empty path as "list the
    // shared root itself". The in-app tool uses an
    // empty `prefix` for the same purpose, but
    // `resolve_safe_path` rejects empty input to
    // keep the security surface tight. The list
    // verb is read-only, so accepting the empty
    // path here can't widen the attack surface
    // beyond what the user already has via the
    // Mac app's local-fs fallback.
    let resolved = if path.trim().is_empty() {
        // Canonicalize the root so the readdir
        // call uses the same path the on-disk
        // check below will compare against (the
        // macOS `/var → /private/var` symlink).
        root.canonicalize().unwrap_or_else(|_| root.to_path_buf())
    } else {
        match shared_fs::resolve_safe_path(root, path) {
            Ok(p) => p,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("shared_list refused: {e}") })),
                )
                    .into_response();
            }
        }
    };
    let dir = match tokio::fs::metadata(&resolved).await {
        Ok(m) if m.is_dir() => resolved,
        Ok(_) => match resolved.parent() {
            Some(p) => p.to_path_buf(),
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": "path has no parent" })),
                )
                    .into_response();
            }
        },
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("stat failed: {e}") })),
            )
                .into_response();
        }
    };
    let mut entries: Vec<SharedEntry> = Vec::new();
    let mut read_dir = match tokio::fs::read_dir(&dir).await {
        Ok(rd) => rd,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("readdir failed: {e}") })),
            )
                .into_response();
        }
    };
    while let Some(entry) = match read_dir.next_entry().await {
        Ok(e) => e,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("readdir entry: {e}") })),
            )
                .into_response();
        }
    } {
        let name = entry.file_name().to_string_lossy().to_string();
        let m = entry.metadata().await.ok();
        let (is_file, size, modified_at) = match m {
            Some(md) => {
                let mtime = md
                    .modified()
                    .ok()
                    .and_then(|t| {
                        let dt: chrono::DateTime<chrono::Utc> = t.into();
                        Some(dt.to_rfc3339())
                    })
                    .unwrap_or_default();
                (md.is_file(), md.len(), mtime)
            }
            None => (false, 0, String::new()),
        };
        entries.push(SharedEntry {
            name,
            is_file,
            size,
            modified_at,
        });
    }
    // Stable ordering: directories first, then files,
    // both alphabetically. Same ordering the in-app
    // tool uses so the Mac app sees the same shape
    // whether the request landed on the daemon or the
    // local fallback.
    entries.sort_by(|a, b| {
        b.is_file
            .cmp(&a.is_file)
            .then_with(|| a.name.cmp(&b.name))
    });
    (StatusCode::OK, Json(json!({ "entries": entries }))).into_response()
}

/// v3.7.1 — `POST /shared` `read` verb. Resolves
/// `path` through the path-safety guard, then reads
/// the file as a UTF-8 string. Mirrors the in-app
/// `SharedReadTool::execute` body for the regular
/// file case (directories are not reachable through
/// `read` on the daemon — `list` is the verb for
/// that). A 1 MB read cap mirrors the in-app tool.
async fn handle_shared_read(root: &std::path::Path, path: &str) -> Response {
    let resolved = match shared_fs::resolve_safe_path(root, path) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("shared_read refused: {e}") })),
            )
                .into_response();
        }
    };
    let meta = match tokio::fs::metadata(&resolved).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": format!("stat failed: {e}") })),
            )
                .into_response();
        }
    };
    if meta.is_dir() {
        // The daemon's `read` verb is for files; the
        // Mac app's `shared_read` tool also handles
        // directories (by listing them). We return
        // a clear 400 here so a client that POSTs
        // `verb: "read"` against a directory gets a
        // useful error instead of a confusing 200
        // with directory metadata.
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "path is a directory; use `list`" })),
        )
            .into_response();
    }
    if !meta.is_file() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "path is not a regular file" })),
        )
            .into_response();
    }
    if meta.len() > 1_048_576 {
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(json!({
                "error": format!(
                    "file is {} bytes (over 1 MB); refusing to read fully",
                    meta.len()
                )
            })),
        )
            .into_response();
    }
    match tokio::fs::read_to_string(&resolved).await {
        Ok(body) => (
            StatusCode::OK,
            Json(json!({ "content": body, "size": body.len() })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("read failed: {e}") })),
        )
            .into_response(),
    }
}

/// v3.7.1 — `POST /shared` `write` verb. Resolves
/// `path` through the path-safety guard, creates
/// the parent directory if needed, then writes
/// `content` as UTF-8. Mirrors the in-app
/// `SharedWriteTool::execute` body exactly so the
/// on-disk shape is identical whether the request
/// landed on the daemon or the local fallback.
async fn handle_shared_write(
    root: &std::path::Path,
    path: &str,
    content: &str,
) -> Response {
    let resolved = match shared_fs::resolve_safe_path(root, path) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("shared_write refused: {e}") })),
            )
                .into_response();
        }
    };
    // Canonicalize the root before the prefix check
    // — on macOS the tempdir lives under
    // `/var/folders/...` which is a symlink to
    // `/private/var/folders/...`. The `resolved`
    // path above is already canonicalized; the raw
    // `root` from `shared_root()` is not, so a
    // component-wise `starts_with` would fail to
    // recognize `/private/var/.../detective` as
    // under `/var/...`.
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if let Some(parent) = resolved.parent() {
        if !parent.starts_with(&canonical_root) {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "parent escapes the shared folder" })),
            )
                .into_response();
        }
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("mkdir failed: {e}") })),
            )
                .into_response();
        }
    }
    if let Err(e) = tokio::fs::write(&resolved, content.as_bytes()).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("write failed: {e}") })),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "bytes_written": content.len(),
            "path": resolved.display().to_string(),
        })),
    )
        .into_response()
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
            // v3.2.0 — added the `computer_use` field; default
            // to "vm" so the daemon's test fixtures match the
            // v3.2.0 production default.
            computer_use: "vm".to_string(),
            // v3.7.0 (Phase 8) — added the
            // `connectors_enabled` field. The
            // daemon's token tests don't
            // exercise the connector surface;
            // the empty value matches the
            // pre-v3.7.0 default and is here
            // only to make the struct literal
            // compile.
            connectors_enabled: String::new(),
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
            // v3.2.0 — `computer_use` defaults to "vm" for new
            // Bots; this is a test fixture so it doesn't
            // affect production defaults.
            computer_use: "vm".to_string(),
            // v3.7.0 (Phase 8) — empty
            // `connectors_enabled`; the token
            // tests don't exercise the
            // connector surface.
            connectors_enabled: String::new(),
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

    // =====================================================================
    //  v3.7.1 — Shared-folder (POST /shared) tests
    // =====================================================================
    //
    // These tests exercise the new `/shared` route
    // end-to-end: HTTP layer, auth, body parsing,
    // path-safety guard, and the on-disk write/read/
    // list round-trip. The shared root is pointed at
    // a tempdir via `MAXBOT_SHARED_DIR` so the tests
    // don't touch the real `~/bots/_shared/`.

    /// v3.7.1 — Serialize env-var manipulations
    /// across the shared-folder tests.
    /// `MAXBOT_SHARED_DIR` is process-global, and the
    /// daemon reads it on every request via
    /// `shared_fs::shared_root()`. If two tests run
    /// in parallel, the second test's `set_var` can
    /// land while the first test's spawned server is
    /// still serving a request, and the first test's
    /// request then sees the second test's tempdir.
    /// A static `Mutex` around `set_var` + the
    /// matching `restore_shared_dir` serializes the
    /// per-test window cleanly. The existing
    /// `tests` (token round-trip, webhook) don't
    /// touch this env var so they're unaffected.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Helper: hold the env lock for the duration
    /// of the calling test. Acquired once at the
    /// top of each shared-folder test; released
    /// when the returned guard is dropped at the
    /// end of the test function.
    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Build a daemon router pinned to a tempdir
    /// shared root. Caller spawns the listener + server
    /// and aborts the server on drop.
    async fn spawn_test_daemon(
        db: Arc<Database>,
    ) -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<()>,
        tempfile::TempDir,
    ) {
        // Each test gets its own tempdir so parallel
        // runs don't collide. `MAXBOT_SHARED_DIR` is
        // read by `shared_fs::shared_root()` at call
        // time, not at startup, so swapping it here
        // works without a daemon restart.
        let tmp = tempfile::tempdir().expect("tempdir");
        // SAFETY: tests are single-threaded for env
        // mutations; the surrounding tokio test runs
        // to completion before another test sets the
        // env. The cleanup at the end of each test
        // restores the saved value.
        unsafe {
            std::env::set_var("MAXBOT_SHARED_DIR", tmp.path());
        }
        let state = Arc::new(DaemonState { db: db.clone() });
        let app = Router::new()
            .route("/health", get(handle_health))
            .route("/hooks/:bot_id", post(handle_webhook))
            .route("/bots/:bot_id/recent_runs", get(handle_recent_runs))
            .route("/shared", post(handle_shared))
            .layer(middleware::from_fn(cors_layer))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (addr, server, tmp)
    }

    /// Restore the previous `MAXBOT_SHARED_DIR` (or
    /// unset it) at the end of a test so a follow-up
    /// test starts from a clean slate.
    fn restore_shared_dir(saved: Option<String>) {
        // SAFETY: see above.
        unsafe {
            match saved {
                Some(v) => std::env::set_var("MAXBOT_SHARED_DIR", v),
                None => std::env::remove_var("MAXBOT_SHARED_DIR"),
            }
        }
    }

    /// v3.7.1 — Insert a minimal test bot so the
    /// `daemon_tokens` FK constraint is satisfied
    /// (a Bot row must exist before a token row
    /// can be inserted for it). Mirrors the
    /// `Bot { ... }` literal the existing token
    /// tests use. Idempotent — calling with an
    /// existing id overwrites the row.
    fn seed_test_bot(db: &Database, bot_id: &str) {
        use maxbot_lib::bots::{Bot, BotState};
        let now = chrono::Utc::now();
        let bot = Bot {
            id: bot_id.to_string(),
            name: format!("test-{bot_id}"),
            description: String::new(),
            system_prompt: String::new(),
            default_model: "minimax-m3".to_string(),
            allowed_tools: vec![],
            icon: String::new(),
            color: String::new(),
            avatar_color: String::new(),
            created_at: now,
            updated_at: now,
            state: BotState::Idle,
            last_active_at: None,
            computer_use: "vm".to_string(),
            connectors_enabled: String::new(),
        };
        db.upsert_bot(&bot).expect("upsert test bot");
    }

    /// `shared_route_cors_header_present` — every
    /// response (including 401s) carries
    /// `Access-Control-Allow-Origin: *` so the
    /// in-app webview doesn't block the request.
    #[tokio::test]
    async fn shared_route_cors_header_present() {
        let _env_guard = lock_env();
        let saved = std::env::var_os("MAXBOT_SHARED_DIR").map(|s| s.to_string_lossy().to_string());
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("cors.db");
        let db = Arc::new(Database::open(&path).expect("open"));
        let (addr, server, _root) = spawn_test_daemon(db).await;

        let client = reqwest::Client::new();
        // /health has no auth — even a successful
        // response should carry the CORS header.
        let resp = client
            .get(format!("http://{addr}/health"))
            .send()
            .await
            .expect("send health");
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .map(|v| v.to_str().unwrap_or("")),
            Some("*"),
            "CORS header missing on /health"
        );
        // /shared with no Authorization header —
        // 401 path, and the CORS header is still
        // present.
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=any"))
            .json(&json!({ "verb": "list", "path": "" }))
            .send()
            .await
            .expect("send shared no-auth");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .map(|v| v.to_str().unwrap_or("")),
            Some("*"),
            "CORS header missing on 401"
        );

        server.abort();
        restore_shared_dir(saved);
    }

    /// `shared_route_rejects_missing_token` — same
    /// shape as `webhook_auth_rejects_missing_token`,
    /// applied to the new route.
    #[tokio::test]
    async fn shared_route_rejects_missing_token() {
        let _env_guard = lock_env();
        let saved = std::env::var_os("MAXBOT_SHARED_DIR").map(|s| s.to_string_lossy().to_string());
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("shared-401.db");
        let db = Arc::new(Database::open(&path).expect("open"));
        let (addr, server, _root) = spawn_test_daemon(db).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=any"))
            .json(&json!({ "verb": "list", "path": "" }))
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // Malformed Authorization header → 401.
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=any"))
            .header("Authorization", "Basic abc")
            .json(&json!({ "verb": "list", "path": "" }))
            .send()
            .await
            .expect("send malformed");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        server.abort();
        restore_shared_dir(saved);
    }

    /// `shared_route_write_read_list_round_trip` —
    /// end-to-end happy path. `shared_write` puts a
    /// file in the tempdir shared root; `shared_list`
    /// sees it; `shared_read` returns the body. All
    /// three calls authenticate with the per-Bot
    /// token and dispatch on the verb.
    #[tokio::test]
    async fn shared_route_write_read_list_round_trip() {
        let _env_guard = lock_env();
        let saved = std::env::var_os("MAXBOT_SHARED_DIR").map(|s| s.to_string_lossy().to_string());
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("shared-rrt.db");
        let db = Arc::new(Database::open(&path).expect("open"));
        // Seed a Bot row first so the daemon_tokens
        // FK constraint is satisfied.
        seed_test_bot(&db, "bot-shared");
        // Then seed a token for the test bot.
        db.set_daemon_token("bot-shared", "tok-shared")
            .expect("set token");
        let (addr, server, root) = spawn_test_daemon(db.clone()).await;

        let client = reqwest::Client::new();

        // 1. Write a file under the shared root.
        let body = json!({
            "verb": "write",
            "path": "detective/findings.json",
            "content": "{\"answer\": 42}"
        });
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=bot-shared"))
            .header("Authorization", "Bearer tok-shared")
            .json(&body)
            .send()
            .await
            .expect("send write");
        let status = resp.status();
        let body_text = resp.text().await.unwrap_or_default();
        if status != StatusCode::OK {
            panic!("write failed: status={status} body={body_text}");
        }
        let json: serde_json::Value = serde_json::from_str(&body_text)
            .expect("parse write body as json");
        assert_eq!(json.get("ok").and_then(|v| v.as_bool()), Some(true));

        // 2. Confirm the file landed on disk. We
        //    canonicalize the on-disk path so the
        //    macOS `/var → /private/var` symlink
        //    doesn't trip the test (the daemon
        //    canonicalizes its side via
        //    `resolve_safe_path`; the test reads
        //    from the uncanonicalized tempdir).
        let on_disk = root.path().join("detective").join("findings.json");
        let on_disk_canon = on_disk
            .canonicalize()
            .unwrap_or_else(|e| panic!("canonicalize {} failed: {e}", on_disk.display()));
        let s = std::fs::read_to_string(&on_disk_canon).expect("read on disk");
        assert_eq!(s, "{\"answer\": 42}");

        // 3. List the root and confirm the new
        //    directory (`detective`) shows up.
        //    The in-app tool's behavior: an empty
        //    `prefix` lists the root, a non-empty
        //    `prefix` lists a subdirectory. The
        //    daemon's `/shared` route mirrors both.
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=bot-shared"))
            .header("Authorization", "Bearer tok-shared")
            .json(&json!({ "verb": "list", "path": "" }))
            .send()
            .await
            .expect("send list");
        assert_eq!(resp.status(), StatusCode::OK);
        let list_json: serde_json::Value = resp.json().await.expect("json");
        let entries = list_json
            .get("entries")
            .and_then(|v| v.as_array())
            .expect("entries array");
        let names: Vec<&str> = entries
            .iter()
            .filter_map(|e| e.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(
            names.contains(&"detective"),
            "expected `detective` in root listing, got: {names:?}"
        );

        // 4. Read the file back. We use `read` to
        //    verify the file is readable as a
        //    regular file (not as a directory) and
        //    contains the right body. We avoid the
        //    in-app tool's `path: "detective"` list
        //    form here because the macOS
        //    canonicalize/uncanny-canonicalize dance
        //    in the test's read path can mask
        //    issues; the `read` verb uses the same
        //    `resolve_safe_path` machinery and is
        //    sufficient for the end-to-end test.
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=bot-shared"))
            .header("Authorization", "Bearer tok-shared")
            .json(&json!({ "verb": "read", "path": "detective/findings.json" }))
            .send()
            .await
            .expect("send read");
        assert_eq!(resp.status(), StatusCode::OK);
        let read_json: serde_json::Value = resp.json().await.expect("json");
        assert_eq!(
            read_json.get("content").and_then(|v| v.as_str()),
            Some("{\"answer\": 42}")
        );

        server.abort();
        restore_shared_dir(saved);
    }

    /// `shared_route_path_safety_guard_rejects_traversal` —
    /// the load-bearing path-safety guard is identical
    /// to the in-app tool's guard. `..` and absolute
    /// paths must be rejected with 400. A regression
    /// here is a host-filesystem read/write primitive
    /// for any Bot.
    #[tokio::test]
    async fn shared_route_path_safety_guard_rejects_traversal() {
        let _env_guard = lock_env();
        let saved = std::env::var_os("MAXBOT_SHARED_DIR").map(|s| s.to_string_lossy().to_string());
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("shared-trav.db");
        let db = Arc::new(Database::open(&path).expect("open"));
        seed_test_bot(&db, "bot-guard");
        db.set_daemon_token("bot-guard", "tok-guard")
            .expect("set token");
        let (addr, server, _root) = spawn_test_daemon(db.clone()).await;

        let client = reqwest::Client::new();
        for bad_path in [
            "../escape.txt",
            "/etc/passwd",
            "C:\\Windows\\System32",
            "..\\backslash",
        ] {
            let resp = client
                .post(format!("http://{addr}/shared?bot_id=bot-guard"))
                .header("Authorization", "Bearer tok-guard")
                .json(&json!({
                    "verb": "write",
                    "path": bad_path,
                    "content": "nope"
                }))
                .send()
                .await
                .expect("send");
            assert_eq!(
                resp.status(),
                StatusCode::BAD_REQUEST,
                "expected 400 for {bad_path}, got {}",
                resp.status()
            );
            let body: serde_json::Value = resp.json().await.expect("json");
            let err = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            assert!(
                err.contains("refused") || err.contains("absolute") || err.contains("backslash") || err.contains("traversal"),
                "expected refusal for {bad_path}, got: {err}"
            );
        }

        server.abort();
        restore_shared_dir(saved);
    }

    /// `shared_route_invalid_verb` — unknown verbs
    /// return 400 instead of crashing the daemon.
    #[tokio::test]
    async fn shared_route_invalid_verb() {
        let _env_guard = lock_env();
        let saved = std::env::var_os("MAXBOT_SHARED_DIR").map(|s| s.to_string_lossy().to_string());
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("shared-verb.db");
        let db = Arc::new(Database::open(&path).expect("open"));
        seed_test_bot(&db, "bot-verb");
        db.set_daemon_token("bot-verb", "tok-verb")
            .expect("set token");
        let (addr, server, _root) = spawn_test_daemon(db.clone()).await;

        let client = reqwest::Client::new();
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=bot-verb"))
            .header("Authorization", "Bearer tok-verb")
            .json(&json!({ "verb": "delete", "path": "foo" }))
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Write without `content` is also a 400.
        let resp = client
            .post(format!("http://{addr}/shared?bot_id=bot-verb"))
            .header("Authorization", "Bearer tok-verb")
            .json(&json!({ "verb": "write", "path": "foo.txt" }))
            .send()
            .await
            .expect("send no-content");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        server.abort();
        restore_shared_dir(saved);
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
