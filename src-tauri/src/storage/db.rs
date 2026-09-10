//! SQLite schema and CRUD operations for MaxBot's local state.
//!
//! Three tables: `conversations`, `messages`, and `settings`. The
//! `messages.tool_calls` column stores serialized tool-call fragments as
//! JSON so that we can re-hydrate a full assistant turn on reload.

use std::path::Path;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    System,
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    fn as_str(self) -> &'static str {
        match self {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "user" => Some(Self::User),
            "assistant" => Some(Self::Assistant),
            "tool" => Some(Self::Tool),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: MessageRole,
    pub content: String,
    /// Tool calls that accompanied this message (assistant role only).
    #[serde(default)]
    pub tool_calls: Vec<PersistedToolCall>,
    pub created_at: DateTime<Utc>,
    /// Friendly error description when a streamed assistant turn
    /// ended in a stream error. `None` for normal messages. The
    /// raw `content` is whatever streamed successfully before the
    /// error fired. Persisted to the `error_message` column on the
    /// `messages` table so the error state survives a reload.
    /// The React side renders this as an `ErrorMessage` block
    /// inside the assistant bubble (with a Retry button) instead
    /// of as a raw text suffix on `content`.
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// If this conversation was started by (or for) a bot, the bot's id.
    /// Null for human-initiated chats. Used by the bot executor to keep
    /// a single conversation per recurring bot.
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// Which LLM provider to use by default for new chats and bot runs.
    /// One of: "minimax" (default), "openai", "anthropic", "xai". Lowercase
    /// string so adding a new provider doesn't require a DB migration —
    /// unknown values fall back to MiniMax.
    #[serde(default)]
    pub provider_kind: String,

    // ---- Per-provider API keys ----
    /// MiniMax API key. Stored unencrypted at rest in the local SQLite DB
    /// (which lives in the per-user Application Support directory with the
    /// default macOS file protection class). For higher security, swap
    /// these for the system Keychain — TODO once the Settings UI surfaces
    /// the tradeoff.
    #[serde(default)]
    pub minimax_api_key: Option<String>,
    /// OpenAI API key (`sk-…`). None = this provider isn't configured.
    #[serde(default)]
    pub openai_api_key: Option<String>,
    /// Anthropic API key (`sk-ant-…`). None = not configured.
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    /// xAI API key (`xai-…`). None = not configured.
    #[serde(default)]
    pub xai_api_key: Option<String>,

    // ---- Per-provider base URL overrides ----
    /// Default model id for the active provider (e.g. "MiniMax-M3",
    /// "gpt-4o", "claude-3-5-sonnet-latest", "grok-2-latest"). Empty
    /// string falls back to the active provider's `default_model()`.
    #[serde(default)]
    pub default_model: String,
    /// Base URL override for MiniMax. Empty string uses the international
    /// endpoint at api.minimax.io. Old settings blobs that used
    /// `base_url` deserialize into this field via the `alias`.
    #[serde(default, alias = "base_url")]
    pub minimax_base_url: String,

    // ---- TTS ----
    /// Voice name passed to `say -v` when the model invokes `tts_speak`
    /// without an explicit voice. Empty string falls back to the
    /// built-in default (Samantha, ships with every macOS install).
    #[serde(default)]
    pub tts_voice: String,

    // ---- Grok Build CLI (v0.7.0) ----
    /// Path to the `grok` binary the `grok_prompt` tool spawns.
    /// May be a bare name on PATH or an absolute path. Default
    /// `grok` — Tyler's install lives at `~/.grok/bin/grok`, so
    /// either add it to PATH or set the absolute path here.
    #[serde(default)]
    pub grok_build_binary: String,
    /// Model alias passed to `grok --model <alias>`. The alias
    /// resolves to an API model via `~/.grok/config.toml`
    /// `[model.<alias>]`. Default `minimax` (the alias the
    /// official install uses for the MiniMax provider).
    #[serde(default)]
    pub grok_build_model: String,
    /// Working directory for the `grok agent stdio` subprocess.
    /// Empty (default) = `<app_data_dir>/grok/` is created and
    /// used. Set to an absolute path to point the agent at a
    /// specific project.
    #[serde(default)]
    pub grok_cwd: String,
    /// Persisted ACP session id. Auto-managed: written on first
    /// bring-up, read on the next app launch to resume the same
    /// conversation. Not exposed in the Settings UI.
    #[serde(default)]
    pub grok_session_id: Option<String>,
    /// Base URL override for OpenAI. Empty = `https://api.openai.com/v1`.
    #[serde(default)]
    pub openai_base_url: String,
    /// Base URL override for Anthropic. Empty = `https://api.anthropic.com`
    /// (the path `/v1/messages` is appended automatically).
    #[serde(default)]
    pub anthropic_base_url: String,
    /// Base URL override for xAI. Empty = `https://api.x.ai/v1`.
    #[serde(default)]
    pub xai_base_url: String,

    // ---- v2.0: per-Bot Computer (Slice B) ----
    /// Hostname or IP of the Linux server that hosts per-Bot
    /// libvirt VMs. Empty = the Computer feature is disabled
    /// and all `computer_*` Tauri commands return
    /// `ServerNotConfigured`. The Settings → Computer tab
    /// surfaces this field; Slice A's runbook walks the user
    /// through picking the host.
    #[serde(default)]
    pub computer_server_host: String,
    /// SSH user on the Linux server. Default `tyler`. Only
    /// used when `computer_server_host` is set.
    #[serde(default)]
    pub computer_server_ssh_user: String,
    /// Optional path to a specific SSH key for the server
    /// connection. Empty = rely on the OS keychain /
    /// ssh-agent. Most Tyler-style installs have the Mac key
    /// at `~/.ssh/id_ed25519` already and leave this empty.
    #[serde(default)]
    pub computer_server_ssh_key_id: String,
    /// Local TCP port range the VNC proxy binds to, in
    /// `"lo-hi"` form. Default `5900-5999`. The plan calls
    /// for 100 ports of headroom; the renderer gets one port
    /// per active VNC console.
    #[serde(default)]
    pub computer_vnc_local_port_range: String,
    /// User-set passphrase. Used to derive the Argon2id key
    /// that encrypts per-Bot SSH keypairs. If empty, the
    /// user hasn't completed Slice A's "Set up your Linux
    /// server" flow yet and per-Bot provisioning fails with
    /// `PassphraseMissing`.
    #[serde(default)]
    pub computer_passphrase: String,
    /// When true, MaxBot's SSH path leaves `identity_file`
    /// empty so ssh falls back to the user's default key
    /// (`~/.ssh/id_ed25519` / `~/.ssh/id_rsa` / ssh-agent).
    /// When false (legacy default), per-Bot SSH keys are
    /// decrypted on every call using `computer_passphrase`.
    /// Introduced in v2.3.5 to drop the passphrase gate —
    /// MaxBot is a personal tool, the per-Bot key model was
    /// over-engineered. Default true so new installs skip
    /// the passphrase field entirely; the per-Bot key path
    /// still works for legacy users who haven't switched.
    #[serde(default = "default_computer_use_default_ssh_key")]
    pub computer_use_default_ssh_key: bool,
    /// Default disk size (GiB) for a newly-provisioned Bot
    /// VM. The Bot editor lets the user override this. Plan
    /// default is 10 GB; we use 10 here for parity.
    #[serde(default = "default_computer_disk")]
    pub computer_default_disk_gb: u32,
    /// Default RAM (MiB) for a newly-provisioned Bot VM.
    /// Plan default is 2048 MiB; the Slice A runbook
    /// recommends 3072 MiB to silence virt-install's
    /// warning, but we keep 2048 here because the user
    /// config is the public default.
    #[serde(default = "default_computer_ram")]
    pub computer_default_ram_mb: u32,

    // ---- v2.7.0 — Voice (bidirectional) ----
    /// When true, the chat UI auto-plays each assistant
    /// reply via `tts_speak` and then auto-arms the
    /// Composer mic for a follow-up voice turn. The
    /// Composer still has a hold-to-record VoiceButton
    /// even when this is off — the flag only governs the
    /// auto-play + auto-record loop. Default false so a
    /// fresh install doesn't talk at the user.
    #[serde(default)]
    pub voice_mode_enabled: bool,

    // ---- v3.7.0 (Phase 8) — Connector credentials ----
    /// Google OAuth access token used by the Gmail and
    /// Google Calendar connectors (`connectors::gmail`
    /// and `connectors::calendar`). One Google account,
    /// one token; the user pastes it here once and
    /// both connectors pick it up. Stored plaintext in
    /// SQLite (same threat model as the LLM API keys
    /// above; the per-user Application Support directory
    /// carries the default macOS file protection
    /// class). None = both connectors return a
    /// "set google_access_token in Settings" error when
    /// called.
    #[serde(default)]
    pub google_access_token: Option<String>,
    /// GitHub Personal Access Token (classic or
    /// fine-grained) used by the `connectors::github`
    /// tools. None = the GitHub connector returns a
    /// "set github_pat in Settings" error when called.
    /// The user generates the token from
    /// <https://github.com/settings/tokens> and pastes
    /// it here; the connector sends it as
    /// `Authorization: token <PAT>`. Per the brief,
    /// OAuth Apps are out of scope for v3.7.0.
    #[serde(default)]
    pub github_pat: Option<String>,
}

fn default_computer_disk() -> u32 {
    10
}
fn default_computer_ram() -> u32 {
    2048
}
/// v2.3.5: when true, MaxBot's SSH path leaves
/// `identity_file` empty so ssh falls back to the user's
/// default key. New installs default to on so the
/// passphrase field can stay empty.
fn default_computer_use_default_ssh_key() -> bool {
    true
}

/// v2.4.0 — a multi-Bot group chat. A group has 2-6
/// member Bots (enforced in `create_group`); messages
/// are routed to a specific member via `@BotName` mentions
/// parsed in the Composer. `owner_bot_id` is the creator;
/// the renderer uses it as the default sender for
/// unset `bot_id` columns (currently unused, but the
/// column makes ownership obvious in queries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChat {
    pub id: String,
    pub name: String,
    pub owner_bot_id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A `GroupChat` plus the list of its member Bot ids.
/// Returned by `list_groups` and `get_group` so the
/// renderer doesn't have to do a follow-up
/// `list_group_members` per row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupChatWithMembers {
    pub chat: GroupChat,
    pub member_bot_ids: Vec<String>,
}

/// v2.4.0 — one row of the `group_messages` transcript.
/// `role` is one of `"user"`, `"assistant"`, or
/// `"handoff"`. `mentions_json` is a JSON array of Bot
/// ids (stored as TEXT so we can re-hydrate it on read).
/// `handoff_to` is the resolved Bot id for `role =
/// "handoff"` rows; null otherwise.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMessage {
    pub id: String,
    pub group_id: String,
    /// Bot id of the speaker. `None` for user-sent
    /// messages (the user isn't a Bot).
    pub bot_id: Option<String>,
    /// One of `"user"`, `"assistant"`, `"handoff"`.
    /// Stored as a free-form string so a future role
    /// addition (e.g. `"system"`) doesn't require a
    /// migration; the executor only writes the three
    /// values above.
    pub role: String,
    pub content: String,
    /// Bot ids that were @-mentioned in the user
    /// message that produced this transcript row. For
    /// assistant / handoff rows this is the union of
    /// the originating user message's mentions plus
    /// any explicit mentions inside the reply.
    #[serde(default)]
    pub mentions: Vec<String>,
    /// Set on `role = "handoff"` rows to the resolved
    /// target Bot id. `None` for everything else.
    #[serde(default)]
    pub handoff_to: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// One row in the `ssh_keys` table. Returned by
/// `Database::get_ssh_key` so the ComputerManager can
/// decrypt the private half on demand.
#[derive(Debug, Clone)]
pub struct SshKeyRow {
    pub id: String,
    pub public_key: String,
    pub private_key_encrypted: Vec<u8>,
    pub created_at: DateTime<Utc>,
}

pub struct Database {
    // `pub(crate)` so the new-module `impl Database`
    // blocks (e.g. `crate::approvals::store`) can do
    // their own queries without round-tripping
    // through a `with_conn` helper. Locking discipline
    // is the same as in `db.rs` — short-lived, no
    // await points while held.
    pub(crate) conn: Mutex<Connection>,
}

impl Database {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA synchronous = NORMAL;",
        )?;
        let db = Self { conn: Mutex::new(conn) };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS conversations (
                id          TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL,
                bot_id      TEXT
             );
             CREATE TABLE IF NOT EXISTS messages (
                id              TEXT PRIMARY KEY,
                conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
                role            TEXT NOT NULL,
                content         TEXT NOT NULL,
                tool_calls_json TEXT NOT NULL DEFAULT '[]',
                created_at      TEXT NOT NULL,
                error_message   TEXT
             );
             CREATE INDEX IF NOT EXISTS messages_by_conversation
                 ON messages(conversation_id, created_at);
             CREATE TABLE IF NOT EXISTS settings (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS bots (
                id              TEXT PRIMARY KEY,
                name            TEXT NOT NULL,
                description     TEXT NOT NULL DEFAULT '',
                system_prompt   TEXT NOT NULL DEFAULT '',
                default_model   TEXT NOT NULL DEFAULT 'MiniMax-M3',
                allowed_tools   TEXT NOT NULL DEFAULT '[]',
                icon            TEXT NOT NULL DEFAULT '',
                color           TEXT NOT NULL DEFAULT '',
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS bot_schedules (
                bot_id              TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
                interval_seconds    INTEGER NOT NULL DEFAULT 0,
                cron_expression     TEXT NOT NULL DEFAULT '',
                last_run_at         TEXT,
                last_conversation_id TEXT
             );
             CREATE TABLE IF NOT EXISTS bot_runs (
                id              TEXT PRIMARY KEY,
                bot_id          TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                conversation_id TEXT NOT NULL,
                status          TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                finished_at     TEXT,
                result_summary  TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS bot_runs_by_bot
                 ON bot_runs(bot_id, started_at DESC);
             CREATE TABLE IF NOT EXISTS bot_messages (
                id              TEXT PRIMARY KEY,
                from_bot_id     TEXT NOT NULL,
                to_bot_id       TEXT NOT NULL,
                body            TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                read            INTEGER NOT NULL DEFAULT 0,
                conversation_id TEXT
             );
             CREATE INDEX IF NOT EXISTS bot_messages_inbox
                 ON bot_messages(to_bot_id, read, created_at);
             CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
             );
             -- v2.0 Slice B: per-Bot Computer. One row per
             -- Bot that has a provisioned VM. The schema is
             -- 1:1 with `bots` so a bot is either
             -- provisioned (row exists) or not (no row).
             CREATE TABLE IF NOT EXISTS computers (
                bot_id          TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
                vm_name         TEXT NOT NULL DEFAULT '',
                vm_ip           TEXT,
                vnc_port        INTEGER,
                ssh_key_id      TEXT,
                state           TEXT NOT NULL DEFAULT 'provisioning',
                last_seen_at    TEXT,
                created_at      TEXT NOT NULL
             );
             -- v2.0 Slice B: per-Bot SSH keypair storage.
             -- The private key is encrypted with a key
             -- derived from `Settings.computer_passphrase`
             -- via Argon2id. The public key is stored in
             -- plaintext (it's not a secret) and is what
             -- cloud-init embeds in `ssh_authorized_keys`.
             CREATE TABLE IF NOT EXISTS ssh_keys (
                id                    TEXT PRIMARY KEY,
                public_key            TEXT NOT NULL,
                private_key_encrypted BLOB NOT NULL,
                created_at            TEXT NOT NULL
             );
             -- v2.2.0 — Skills: reusable multi-step procedures.
             -- A Skill is a saved, named list of (tool, args) steps the
             -- user can invoke on demand. The shape is intentionally
             -- flat JSON so the file is diffable and grep-friendly.
             -- `inputs_json` is a JSON array of Param{name,kind,default,choices};
             -- `steps_json` is a JSON array of Step{tool,args,output_var}.
             CREATE TABLE IF NOT EXISTS skills (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                inputs_json TEXT NOT NULL DEFAULT '[]',
                steps_json  TEXT NOT NULL DEFAULT '[]',
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
             );
             -- v2.2.0 — Skill runs: every invocation of a Skill persists
             -- a row here for the renderer's run-history view. Steps are
             -- kept in-memory in the AppState registry; this table
             -- stores the durable summary (status, started/finished,
             -- result summary, inputs).
             CREATE TABLE IF NOT EXISTS skill_runs (
                id              TEXT PRIMARY KEY,
                skill_id        TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
                bot_id          TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                inputs_json     TEXT NOT NULL DEFAULT '{}',
                status          TEXT NOT NULL,
                started_at      TEXT NOT NULL,
                finished_at     TEXT,
                result_summary  TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS skill_runs_by_skill
                 ON skill_runs(skill_id, started_at DESC);
             -- v3.3.0 — Skill run traces: per-step output capture
             -- for the Last run view. The existing `skill_runs`
             -- table stays the durable summary (status, started/finished,
             -- result summary, inputs); this new table is the
             -- per-step trace — the LLM's tool calls / results / next
             -- prompt, one row per run, with the full per-step array
             -- JSON-encoded into `per_step_output`. Best-effort
             -- capture: a trace-write failure does NOT fail the run.
             -- The `run_id` column is the linkage back to
             -- `skill_runs.id` so a single run has both a summary
             -- row and a trace row.
             CREATE TABLE IF NOT EXISTS skill_run_traces (
                id              TEXT PRIMARY KEY,
                run_id          TEXT NOT NULL,
                skill_id        TEXT NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
                started_at      TEXT NOT NULL,
                duration_ms     INTEGER NOT NULL DEFAULT 0,
                per_step_output TEXT NOT NULL DEFAULT '[]',
                success         INTEGER NOT NULL DEFAULT 1,
                trigger_input   TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS skill_run_traces_by_skill
                 ON skill_run_traces(skill_id, started_at DESC);
             -- v2.4.0 — Multi-Bot groups: 2-6 Bots can
             -- collaborate in a single conversation. The
             -- `group_chats` row is the conversation; the
             -- `group_members` rows enumerate the Bot
             -- participants; `group_messages` is the
             -- transcript. `conversations.kind` is NOT
             -- migrated — group chats live in their own
             -- tables and `mainView` distinguishes them
             -- on the renderer side. The `handoff_to`
             -- column on `group_messages` is set for
             -- `role='handoff'` rows and points at the
             -- Bot that should pick the message up next
             -- turn.
             CREATE TABLE IF NOT EXISTS group_chats (
                id          TEXT PRIMARY KEY,
                name        TEXT NOT NULL,
                owner_bot_id TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                created_at  TEXT NOT NULL,
                updated_at  TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS group_members (
                group_id    TEXT NOT NULL REFERENCES group_chats(id) ON DELETE CASCADE,
                bot_id      TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                PRIMARY KEY (group_id, bot_id)
             );
             CREATE TABLE IF NOT EXISTS group_messages (
                id            TEXT PRIMARY KEY,
                group_id      TEXT NOT NULL REFERENCES group_chats(id) ON DELETE CASCADE,
                bot_id        TEXT REFERENCES bots(id) ON DELETE SET NULL,
                role          TEXT NOT NULL,
                content       TEXT NOT NULL,
                mentions_json TEXT NOT NULL DEFAULT '[]',
                handoff_to    TEXT,
                created_at    TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS group_messages_by_group
                 ON group_messages(group_id, created_at);
             CREATE INDEX IF NOT EXISTS group_members_by_bot
                 ON group_members(bot_id);
             -- v2.6.0 — Approval flows. Per-Bot per-tool
             -- rule (auto/ask/deny) and a queue of pending
             -- approvals that the user can Approve / Reject /
             -- Edit & send. See `crate::approvals`.
             CREATE TABLE IF NOT EXISTS approval_rules (
                 bot_id    TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                 tool_name TEXT NOT NULL,
                 rule      TEXT NOT NULL,
                 PRIMARY KEY (bot_id, tool_name)
             );
             CREATE TABLE IF NOT EXISTS approvals (
                 id           TEXT PRIMARY KEY,
                 bot_id       TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
                 tool_name    TEXT NOT NULL,
                 status       TEXT NOT NULL,
                 payload_json TEXT NOT NULL,
                 result_json  TEXT,
                 bot_run_id   TEXT,
                 created_at   TEXT NOT NULL,
                 decided_at   TEXT
             );
             CREATE INDEX IF NOT EXISTS approvals_by_bot_status
                 ON approvals(bot_id, status, created_at DESC);
             -- v3.4.0 (Phase 5) — Per-Bot Takeover state.
             -- One row per Bot in a non-`running` takeover
             -- state. The Bot executor upserts this when a
             -- tool return carries `needs_human`; the user
             -- clears it (via the Hand back / Reject UI) by
             -- setting state back to `running` (which the
             -- storage layer implements as `DELETE`). The
             -- row persists across app close/reopen so a
             -- daemon-driven run that paused the Bot
             -- surfaces the pending approval in the queue
             -- on next launch.
             CREATE TABLE IF NOT EXISTS bot_takeover_state (
                 bot_id          TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
                 state           TEXT NOT NULL,
                 approval_id     TEXT NOT NULL,
                 reason          TEXT NOT NULL DEFAULT '',
                 triggering_tool TEXT NOT NULL DEFAULT '',
                 created_at      TEXT NOT NULL,
                 updated_at      TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS bot_takeover_state_by_state
                 ON bot_takeover_state(state, updated_at DESC);
             -- v2.8.0 — Always-on Daemon (24/7). One row per
             -- Bot that has a daemon webhook enabled. The
             -- `maxbotd` binary consults this table on every
             -- inbound `POST /hooks/<bot_id>` to verify the
             -- bearer token in the `Authorization` header. The
             -- same table is read by the Tauri app's
             -- BotEditor to show the current token / Rotate /
             -- Copy controls. Tokens are 32 random bytes
             -- hex-encoded (64 chars); we don't hash them
             -- because the read path needs the plaintext
             -- (the daemon verifies the inbound header
             -- against it directly). Treat the SQLite file
             -- as the secret boundary.
             CREATE TABLE IF NOT EXISTS daemon_tokens (
                 bot_id      TEXT PRIMARY KEY REFERENCES bots(id) ON DELETE CASCADE,
                 token       TEXT NOT NULL,
                 created_at  TEXT NOT NULL,
                 last_used   TEXT
             );",
        )?;
        // Idempotent column additions for older databases. SQLite
        // doesn't have IF NOT EXISTS for columns, so we probe
        // pragma_table_info to decide whether to ALTER.
        add_column_if_missing(
            &conn,
            "bot_schedules",
            "cron_expression",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        // v2.6.2 — wire the LLM's tool_call_id into the
        // `approvals` row so the auto-resume path can
        // append a synthetic `role=tool` message with
        // the right `tool_call_id` to match the model
        // expectation. NULL is fine for pre-v2.6.2
        // rows: the resume still appends a tool message,
        // it just matches by position. Backfill from
        // `bot_runs` would be brittle (the LLM's
        // streaming tool_call id is not persisted
        // anywhere we can recover it), so we leave
        // older rows as NULL.
        add_column_if_missing(
            &conn,
            "approvals",
            "tool_call_id",
            "TEXT",
        )?;
        // v3.4.0 (Phase 5) — "Why this asked" reason on
        // each approval row. Populated when the approval
        // is enqueued (not when decided), so a pending
        // row in the queue already carries the reason.
        // NULL is fine for pre-v3.4.0 rows; the
        // ActivityFeed / ApprovalQueue render the absence
        // as a generic "approval required" copy rather
        // than failing.
        add_column_if_missing(
            &conn,
            "approvals",
            "reason",
            "TEXT",
        )?;
        // v0.7.6: error_message on `messages` — friendly description
        // of a stream error, surfaced by the chat command when the
        // assistant turn ends in a wire-protocol / network / auth
        // failure. The raw `content` keeps whatever streamed before
        // the error; `error_message` is rendered as a separate UI
        // block by the React side (with a Retry button) instead of
        // being appended to the visible content.
        add_column_if_missing(
            &conn,
            "messages",
            "error_message",
            "TEXT",
        )?;
        // v2.0 Slice E: per-Bot presence columns. The sidebar's
        // `BotRoster` shows a 6-state avatar per Bot, derived from
        // `state` (set by the bot executor) plus the most-recent
        // `bot_run.status` and `computers.state` (the renderer
        // factors those in client-side). `avatar_color` is an
        // optional second color for the avatar gradient. `state`
        // defaults to `idle` for older rows that pre-date the
        // column.
        add_column_if_missing(&conn, "bots", "avatar_color", "TEXT")?;
        add_column_if_missing(&conn, "bots", "last_active_at", "TEXT")?;
        add_column_if_missing(
            &conn,
            "bots",
            "state",
            "TEXT NOT NULL DEFAULT 'idle'",
        )?;
        // v3.2.0 — Computer Use target. The Bot editor lets the
        // user pick between "vm" (default, the per-Bot Linux VM
        // via `vm_computer_use`), "mac" (legacy `ego_browser`),
        // and "mac-with-approval" (Mac path wrapped in an
        // approval gate, Phase 5). Existing rows backfill to
        // "vm" so v3.1.0 Bots seamlessly move to the in-VM
        // path on first open of the Bot editor in v3.2.0+.
        add_column_if_missing(
            &conn,
            "bots",
            "computer_use",
            "TEXT NOT NULL DEFAULT 'vm'",
        )?;
        // v3.7.0 (Phase 8) — per-Bot connector enable
        // toggles. Comma-separated list of connector
        // ids (`"gmail"`, `"calendar"`, `"github"`);
        // empty = no connectors enabled. The tool
        // registry reads this column at run time and
        // hides the matching `gmail_*` / `calendar_*` /
        // `github_*` tools when the corresponding id
        // is absent. Nullable (no NOT NULL DEFAULT) so
        // existing rows backfill to NULL → empty
        // string on the read path (via
        // `Option<String>::unwrap_or_default()`).
        add_column_if_missing(&conn, "bots", "connectors_enabled", "TEXT")?;
        // v2.2.0 — forward-compat for v2.3 schedules. v2.2 doesn't
        // wire the schedules UI to Skills, but adding the column now
        // means a future migration doesn't have to ALTER an
        // already-populated `bot_schedules` table. No FOREIGN KEY
        // here on purpose: we don't want a v2.2 install that
        // somehow loses the Skills table (e.g. test fixture) to
        // fail loading schedules.
        add_column_if_missing(
            &conn,
            "bot_schedules",
            "skill_id",
            "TEXT",
        )?;
        // v2.3.5 — drop the per-Bot passphrase gate by using
        // the user's default SSH key. Existing DBs get upgraded
        // to the "on" state via the column DEFAULT. The legacy
        // `computer_passphrase` field stays around for users
        // who haven't switched yet, but the SSH hot path now
        // skips the per-Bot key fetch when this flag is on.
        add_column_if_missing(
            &conn,
            "settings",
            "computer_use_default_ssh_key",
            "INTEGER NOT NULL DEFAULT 1",
        )?;
        // v2.6.0 — Seed default approval rules for every
        // existing Bot. Tools that send data out
        // (`mail_send`, `message_bot`, `file_write`,
        // `shell_run`) default to `ask`; every other tool
        // defaults to `auto`. The `INSERT OR IGNORE` is
        // idempotent — a Bot that already has a row in
        // `approval_rules` (e.g. set by the user before
        // the upgrade) keeps its rule. Note: `message_bot`
        // is special-cased in the executor (it routes via
        // the DB rather than going through the tool
        // registry) but we still seed a rule for it so the
        // user can flip it to `deny` from the editor.
        conn.execute_batch(
            "INSERT OR IGNORE INTO approval_rules (bot_id, tool_name, rule)
             SELECT id, 'mail_send', 'ask' FROM bots;
             INSERT OR IGNORE INTO approval_rules (bot_id, tool_name, rule)
             SELECT id, 'message_bot', 'ask' FROM bots;
             INSERT OR IGNORE INTO approval_rules (bot_id, tool_name, rule)
             SELECT id, 'file_write', 'ask' FROM bots;
             INSERT OR IGNORE INTO approval_rules (bot_id, tool_name, rule)
             SELECT id, 'shell_run', 'ask' FROM bots;",
        )?;
        // v3.1.0 — `triggered_by` on `bot_runs`. New column
        // for the Phase 2 maxbotd verification flow: tells the
        // ActivityFeed whether a run came from the in-app
        // "Run now" button (`"app"`), the always-on scheduler
        // (`"daemon"`), or a webhook POST (`"webhook"`).
        // Defaults to `"app"` for both new rows and existing
        // legacy rows (existing rows get the column's DEFAULT
        // via SQLite's add-column-with-default behavior). The
        // executor + daemon read this back as a String and
        // parse it into `BotRunTriggeredBy`; unknown values
        // fall back to `App` so a future enum addition can't
        // crash a row read.
        add_column_if_missing(
            &conn,
            "bot_runs",
            "triggered_by",
            "TEXT NOT NULL DEFAULT 'app'",
        )?;
        Ok(())
    }

    // ----- conversations -----

    pub fn list_conversations(&self) -> rusqlite::Result<Vec<Conversation>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, title, created_at, updated_at, bot_id
             FROM conversations
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Conversation {
                id: row.get(0)?,
                title: row.get(1)?,
                created_at: parse_dt(row.get::<_, String>(2)?),
                updated_at: parse_dt(row.get::<_, String>(3)?),
                bot_id: row.get(4)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn create_conversation(
        &self,
        title: Option<String>,
        bot_id: Option<&str>,
    ) -> rusqlite::Result<Conversation> {
        let now = Utc::now();
        let convo = Conversation {
            id: Uuid::new_v4().to_string(),
            title: title.unwrap_or_else(|| "New chat".to_string()),
            created_at: now,
            updated_at: now,
            bot_id: bot_id.map(str::to_string),
        };
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at, bot_id) VALUES (?, ?, ?, ?, ?)",
            params![
                convo.id,
                convo.title,
                convo.created_at.to_rfc3339(),
                convo.updated_at.to_rfc3339(),
                convo.bot_id,
            ],
        )?;
        Ok(convo)
    }

    /// Create a conversation that belongs to a specific bot. The bot
    /// executor uses this for scheduled runs so the recurring bot has
    /// a single persistent log.
    pub fn create_bot_conversation(
        &self,
        bot_id: &str,
        title: Option<String>,
    ) -> rusqlite::Result<Conversation> {
        let now = Utc::now();
        let convo = Conversation {
            id: Uuid::new_v4().to_string(),
            title: title.unwrap_or_else(|| format!("Bot: {}", bot_id)),
            created_at: now,
            updated_at: now,
            bot_id: Some(bot_id.to_string()),
        };
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO conversations (id, title, created_at, updated_at, bot_id) VALUES (?, ?, ?, ?, ?)",
            params![
                convo.id,
                convo.title,
                convo.created_at.to_rfc3339(),
                convo.updated_at.to_rfc3339(),
                convo.bot_id,
            ],
        )?;
        Ok(convo)
    }

    pub fn delete_conversation(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute("DELETE FROM conversations WHERE id = ?", params![id])?;
        Ok(())
    }

    pub fn rename_conversation(&self, id: &str, title: &str) -> rusqlite::Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE conversations SET title = ?, updated_at = ? WHERE id = ?",
            params![title, now, id],
        )?;
        Ok(())
    }

    pub fn touch_conversation(&self, id: &str) -> rusqlite::Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![now, id],
        )?;
        Ok(())
    }

    // ----- messages -----

    pub fn list_messages(&self, conversation_id: &str) -> rusqlite::Result<Vec<Message>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, tool_calls_json, created_at, error_message
             FROM messages
             WHERE conversation_id = ?
             ORDER BY created_at ASC",
        )?;
        let rows = stmt.query_map(params![conversation_id], |row| {
            let role_str: String = row.get(2)?;
            let tool_calls_json: String = row.get(4)?;
            let tool_calls: Vec<PersistedToolCall> =
                serde_json::from_str(&tool_calls_json).unwrap_or_default();
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: MessageRole::parse(&role_str).unwrap_or(MessageRole::User),
                content: row.get(3)?,
                tool_calls,
                created_at: parse_dt(row.get::<_, String>(5)?),
                error_message: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Full-text-ish search across all messages. Uses SQL `LIKE` (case
    /// insensitive) so it works without a separate FTS5 virtual table.
    /// Returns one row per matching message, newest first. The caller
    /// is expected to group by `conversation_id` and pick a snippet.
    /// Empty query returns no rows.
    pub fn search_messages(
        &self,
        query: &str,
        limit: u32,
    ) -> rusqlite::Result<Vec<Message>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(Vec::new());
        }
        // Wrap in % for substring match. SQL injection: we use a
        // parameterized query, so the user's query is treated as a
        // literal value. We escape LIKE wildcards in the user input
        // so a search for "100%" doesn't match everything.
        let escaped = q
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("%{}%", escaped);
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, tool_calls_json, created_at, error_message
             FROM messages
             WHERE content LIKE ? ESCAPE '\\'
             ORDER BY created_at DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
            let role_str: String = row.get(2)?;
            let tool_calls_json: String = row.get(4)?;
            let tool_calls: Vec<PersistedToolCall> =
                serde_json::from_str(&tool_calls_json).unwrap_or_default();
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: MessageRole::parse(&role_str).unwrap_or(MessageRole::User),
                content: row.get(3)?,
                tool_calls,
                created_at: parse_dt(row.get::<_, String>(5)?),
                error_message: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn insert_message(
        &self,
        conversation_id: &str,
        role: MessageRole,
        content: &str,
        tool_calls: &[PersistedToolCall],
    ) -> rusqlite::Result<Message> {
        let now = Utc::now();
        let message = Message {
            id: Uuid::new_v4().to_string(),
            conversation_id: conversation_id.to_string(),
            role,
            content: content.to_string(),
            tool_calls: tool_calls.to_vec(),
            created_at: now,
            error_message: None,
        };
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO messages (id, conversation_id, role, content, tool_calls_json, created_at, error_message)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                message.id,
                message.conversation_id,
                message.role.as_str(),
                message.content,
                serde_json::to_string(&message.tool_calls).unwrap_or_else(|_| "[]".to_string()),
                message.created_at.to_rfc3339(),
                message.error_message,
            ],
        )?;
        // Bump the conversation's updated_at so the sidebar re-orders.
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![now.to_rfc3339(), conversation_id],
        )?;
        Ok(message)
    }

    /// Delete all messages in `conversation_id` strictly after
    /// `after_message_id`. Used by "Regenerate" to wipe the previous
    /// assistant response (and any tool messages between the user
    /// message and the response) before re-running the chat loop.
    /// Returns the number of rows deleted.
    pub fn delete_messages_after(
        &self,
        conversation_id: &str,
        after_message_id: &str,
    ) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().expect("db lock poisoned");
        // Look up the created_at of the anchor message, then delete
        // every other message in the same conversation with a
        // strictly later timestamp.
        let anchor_ts: Option<String> = conn
            .query_row(
                "SELECT created_at FROM messages WHERE id = ?",
                params![after_message_id],
                |row| row.get(0),
            )
            .ok();
        let Some(anchor_ts) = anchor_ts else {
            return Ok(0);
        };
        let n = conn.execute(
            "DELETE FROM messages
             WHERE conversation_id = ?
               AND id != ?
               AND created_at > ?",
            params![conversation_id, after_message_id, anchor_ts],
        )?;
        Ok(n)
    }

    /// Find the most recent user message in a conversation. Returns
    /// None if the conversation has no user messages.
    pub fn last_user_message(
        &self,
        conversation_id: &str,
    ) -> rusqlite::Result<Option<Message>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, conversation_id, role, content, tool_calls_json, created_at, error_message
             FROM messages
             WHERE conversation_id = ? AND role = 'user'
             ORDER BY created_at DESC
             LIMIT 1",
        )?;
        let mut rows = stmt.query_map(params![conversation_id], |row| {
            let role_str: String = row.get(2)?;
            let tool_calls_json: String = row.get(4)?;
            let tool_calls: Vec<PersistedToolCall> =
                serde_json::from_str(&tool_calls_json).unwrap_or_default();
            Ok(Message {
                id: row.get(0)?,
                conversation_id: row.get(1)?,
                role: MessageRole::parse(&role_str).unwrap_or(MessageRole::User),
                content: row.get(3)?,
                tool_calls,
                created_at: parse_dt(row.get::<_, String>(5)?),
                error_message: row.get(6)?,
            })
        })?;
        if let Some(row) = rows.next() {
            Ok(Some(row?))
        } else {
            Ok(None)
        }
    }

    /// Append to an existing assistant message in place. Used for streaming
    /// turns: the empty assistant message is inserted on send, and each
    /// token appends to its `content` until the stream finishes.
    pub fn append_message_content(
        &self,
        message_id: &str,
        delta: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE messages SET content = content || ? WHERE id = ?",
            params![delta, message_id],
        )?;
        Ok(())
    }

    /// Persist a friendly error description on a streamed assistant
    /// turn. Called by the chat command when a stream ends in a
    /// `StreamError` — the `content` column already has whatever
    /// streamed before the error, and `error_message` is rendered
    /// by the React side as a separate UI block (with a Retry
    /// button) instead of being appended to the visible content.
    pub fn set_message_error_message(
        &self,
        message_id: &str,
        error_message: Option<&str>,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE messages SET error_message = ? WHERE id = ?",
            params![error_message, message_id],
        )?;
        Ok(())
    }

    /// One-shot migration: write the post-split shape of a legacy
    /// "[error] …" message. Sets `content` to the streamed prefix
    /// and `error_message` to the friendly description in a single
    /// statement so the next reload sees the clean shape (no need
    /// to re-split on every page load). Used by the App.tsx
    /// `splitLegacyErrorSuffix` migration on first load after
    /// upgrade.
    pub fn migrate_message_to_error_shape(
        &self,
        message_id: &str,
        content: &str,
        error_message: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE messages SET content = ?, error_message = ? WHERE id = ?",
            params![content, error_message, message_id],
        )?;
        Ok(())
    }

    // ----- bots -----

    pub fn list_bots(&self) -> rusqlite::Result<Vec<crate::bots::Bot>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, description, system_prompt, default_model, allowed_tools, icon, color, avatar_color, last_active_at, state, computer_use, connectors_enabled, created_at, updated_at
             FROM bots ORDER BY name COLLATE NOCASE ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let allowed_tools_json: String = row.get(5)?;
            let allowed_tools: Vec<String> =
                serde_json::from_str(&allowed_tools_json).unwrap_or_default();
            let last_active_str: Option<String> = row.get(9)?;
            let state_str: String = row.get(10)?;
            // v2.0.1 fix: `avatar_color` is nullable (no DEFAULT in
            // the migration) so v1.0 rows carry NULL. Reading it as
            // `String` raises `Invalid column type Null at index: 8`.
            // Pull as `Option<String>` and default to "" so the
            // presence gradient can fall back to `color`.
            let avatar_color: Option<String> = row.get(8)?;
            // v3.2.0 — `computer_use` defaults to "vm" on the
            // read path (via parse_computer_use) so any future
            // value (e.g. "hybrid") and any pre-v3.2.0 row
            // (which carries "vm" from the column DEFAULT)
            // land in a known-good state.
            let computer_use_str: String = row.get(11)?;
            // v3.7.0 (Phase 8) — `connectors_enabled` is
            // nullable (no DEFAULT) so pre-v3.7.0 rows carry
            // NULL. Pull as `Option<String>` and default to ""
            // so the registry's `parse_enabled` sees an empty
            // list (= no connectors enabled). The registry's
            // `connectors_filtered` then leaves the connector
            // tools in the per-Bot tool list, which matches
            // the pre-v3.7.0 behavior (connector tools
            // effectively disabled by default).
            let connectors_enabled: Option<String> = row.get(12)?;
            Ok(crate::bots::Bot {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                system_prompt: row.get(3)?,
                default_model: row.get(4)?,
                allowed_tools,
                icon: row.get(6)?,
                color: row.get(7)?,
                avatar_color: avatar_color.unwrap_or_default(),
                last_active_at: last_active_str.map(parse_dt),
                state: crate::bots::BotState::parse(&state_str),
                computer_use: crate::bots::parse_computer_use(&computer_use_str).to_string(),
                connectors_enabled: connectors_enabled.unwrap_or_default(),
                created_at: parse_dt(row.get::<_, String>(13)?),
                updated_at: parse_dt(row.get::<_, String>(14)?),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_bot(&self, id: &str) -> rusqlite::Result<Option<crate::bots::Bot>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, description, system_prompt, default_model, allowed_tools, icon, color, avatar_color, last_active_at, state, computer_use, connectors_enabled, created_at, updated_at
             FROM bots WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        let allowed_tools_json: String = row.get(5)?;
        let allowed_tools: Vec<String> =
            serde_json::from_str(&allowed_tools_json).unwrap_or_default();
        let last_active_str: Option<String> = row.get(9)?;
        let state_str: String = row.get(10)?;
        // v2.0.1 fix: same as `list_bots` — `avatar_color` is
        // nullable so v1.0 rows carry NULL. Default to "".
        let avatar_color: Option<String> = row.get(8)?;
        // v3.2.0 — see list_bots for the parse_computer_use rationale.
        let computer_use_str: String = row.get(11)?;
        // v3.7.0 (Phase 8) — see list_bots for the
        // connectors_enabled rationale.
        let connectors_enabled: Option<String> = row.get(12)?;
        Ok(Some(crate::bots::Bot {
            id: row.get(0)?,
            name: row.get(1)?,
            description: row.get(2)?,
            system_prompt: row.get(3)?,
            default_model: row.get(4)?,
            allowed_tools,
            icon: row.get(6)?,
            color: row.get(7)?,
            avatar_color: avatar_color.unwrap_or_default(),
            last_active_at: last_active_str.map(parse_dt),
            state: crate::bots::BotState::parse(&state_str),
            computer_use: crate::bots::parse_computer_use(&computer_use_str).to_string(),
            connectors_enabled: connectors_enabled.unwrap_or_default(),
            created_at: parse_dt(row.get::<_, String>(13)?),
            updated_at: parse_dt(row.get::<_, String>(14)?),
        }))
    }

    pub fn upsert_bot(&self, bot: &crate::bots::Bot) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let allowed_tools_json =
            serde_json::to_string(&bot.allowed_tools).unwrap_or_else(|_| "[]".to_string());
        let last_active = bot.last_active_at.as_ref().map(|d| d.to_rfc3339());
        // v3.2.0 — normalize the stored value via
        // parse_computer_use so a typo in the editor can't
        // persist a value the registry won't recognize.
        // The enum has exactly three valid values today; an
        // empty / unknown string lands in "vm" (the default).
        let computer_use = crate::bots::parse_computer_use(&bot.computer_use).to_string();
        // v3.7.0 (Phase 8) — trim and skip empties so a
        // stray `"  ,  , gmail"` from the editor lands
        // as `"gmail"` instead of an obviously-bad
        // string. Unknown ids are filtered out at the
        // registry's read path (`parse_enabled`), so a
        // typo can't disable a connector.
        let connectors_enabled: String = bot
            .connectors_enabled
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        conn.execute(
            "INSERT INTO bots (id, name, description, system_prompt, default_model, allowed_tools, icon, color, avatar_color, last_active_at, state, computer_use, connectors_enabled, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                system_prompt = excluded.system_prompt,
                default_model = excluded.default_model,
                allowed_tools = excluded.allowed_tools,
                icon = excluded.icon,
                color = excluded.color,
                avatar_color = excluded.avatar_color,
                last_active_at = excluded.last_active_at,
                state = excluded.state,
                computer_use = excluded.computer_use,
                connectors_enabled = excluded.connectors_enabled,
                updated_at = excluded.updated_at",
            params![
                bot.id,
                bot.name,
                bot.description,
                bot.system_prompt,
                bot.default_model,
                allowed_tools_json,
                bot.icon,
                bot.color,
                bot.avatar_color,
                last_active,
                bot.state.as_str(),
                computer_use,
                connectors_enabled,
                bot.created_at.to_rfc3339(),
                bot.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn delete_bot(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute("DELETE FROM bots WHERE id = ?", params![id])?;
        Ok(())
    }

    /// v2.0 Slice E: light-touch update of just the `state` column
    /// for a Bot. The bot executor calls this at run start
    /// (`working`/`thinking`), at run end (`done`), and when the
    /// Bot detects a user-input requirement (`blocked`).
    /// Returns 0 if the bot id doesn't exist (caller can ignore
    /// the count — the renderer never blocks on a stale state
    /// write).
    pub fn set_bot_state(
        &self,
        id: &str,
        state: crate::bots::BotState,
    ) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let updated = conn.execute(
            "UPDATE bots SET state = ? WHERE id = ?",
            params![state.as_str(), id],
        )?;
        Ok(updated)
    }

    /// v2.0 Slice E: bump `last_active_at` to "now" for the
    /// given bot. Called from the bot executor after a successful
    /// run, and from the chat command when the user sends a
    /// message to a bot. Used by the sidebar roster to show
    /// "2m ago" timestamps.
    pub fn touch_bot_last_active(&self, id: &str) -> rusqlite::Result<usize> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let now = chrono::Utc::now().to_rfc3339();
        let updated = conn.execute(
            "UPDATE bots SET last_active_at = ? WHERE id = ?",
            params![now, id],
        )?;
        Ok(updated)
    }

    pub fn get_schedule(&self, bot_id: &str) -> rusqlite::Result<Option<crate::bots::BotSchedule>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, interval_seconds, cron_expression, last_run_at, last_conversation_id, skill_id
             FROM bot_schedules WHERE bot_id = ?",
        )?;
        let mut rows = stmt.query(params![bot_id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        let last_run_str: Option<String> = row.get(3)?;
        Ok(Some(crate::bots::BotSchedule {
            bot_id: row.get(0)?,
            interval_seconds: row.get::<_, i64>(1)? as u32,
            cron_expression: row.get(2)?,
            last_run_at: last_run_str.map(parse_dt),
            last_conversation_id: row.get(4)?,
            skill_id: row.get(5)?,
        }))
    }

    pub fn upsert_schedule(&self, schedule: &crate::bots::BotSchedule) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let last_run_at = schedule
            .last_run_at
            .as_ref()
            .map(|d| d.to_rfc3339());
        conn.execute(
            "INSERT INTO bot_schedules (bot_id, interval_seconds, cron_expression, last_run_at, last_conversation_id, skill_id)
             VALUES (?, ?, ?, ?, ?, ?)
             ON CONFLICT(bot_id) DO UPDATE SET
                interval_seconds = excluded.interval_seconds,
                cron_expression = excluded.cron_expression,
                last_run_at = excluded.last_run_at,
                last_conversation_id = excluded.last_conversation_id,
                skill_id = excluded.skill_id",
            params![
                schedule.bot_id,
                schedule.interval_seconds as i64,
                schedule.cron_expression,
                last_run_at,
                schedule.last_conversation_id,
                schedule.skill_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_due_schedules(&self, now: DateTime<Utc>) -> rusqlite::Result<Vec<crate::bots::BotSchedule>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, interval_seconds, cron_expression, last_run_at, last_conversation_id, skill_id
             FROM bot_schedules
             WHERE (interval_seconds > 0 OR cron_expression != '')
               AND (last_run_at IS NULL OR
                    (julianday(?) - julianday(last_run_at)) * 86400.0 >= interval_seconds)",
        )?;
        let rows = stmt.query_map(params![now.to_rfc3339()], |row| {
            let last_run_str: Option<String> = row.get(3)?;
            Ok(crate::bots::BotSchedule {
                bot_id: row.get(0)?,
                interval_seconds: row.get::<_, i64>(1)? as u32,
                cron_expression: row.get(2)?,
                last_run_at: last_run_str.map(parse_dt),
                last_conversation_id: row.get(4)?,
                skill_id: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn list_all_schedules(&self) -> rusqlite::Result<Vec<crate::bots::BotSchedule>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, interval_seconds, cron_expression, last_run_at, last_conversation_id, skill_id
             FROM bot_schedules",
        )?;
        let rows = stmt.query_map([], |row| {
            let last_run_str: Option<String> = row.get(3)?;
            Ok(crate::bots::BotSchedule {
                bot_id: row.get(0)?,
                interval_seconds: row.get::<_, i64>(1)? as u32,
                cron_expression: row.get(2)?,
                last_run_at: last_run_str.map(parse_dt),
                last_conversation_id: row.get(4)?,
                skill_id: row.get(5)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn upsert_bot_run(&self, run: &crate::bots::BotRun) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let finished_at = run.finished_at.as_ref().map(|d| d.to_rfc3339());
        let status_str = match run.status {
            crate::bots::BotRunStatus::Running => "running",
            crate::bots::BotRunStatus::Succeeded => "succeeded",
            crate::bots::BotRunStatus::Failed => "failed",
            crate::bots::BotRunStatus::Cancelled => "cancelled",
        };
        conn.execute(
            "INSERT INTO bot_runs (id, bot_id, conversation_id, status, started_at, finished_at, result_summary, triggered_by)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                status = excluded.status,
                finished_at = excluded.finished_at,
                result_summary = excluded.result_summary,
                triggered_by = excluded.triggered_by",
            params![
                run.id,
                run.bot_id,
                run.conversation_id,
                status_str,
                run.started_at.to_rfc3339(),
                finished_at,
                run.result_summary,
                run.triggered_by,
            ],
        )?;
        Ok(())
    }

    /// v2.6.2 — Fetch a single bot_run by id. Used by
    /// the approval auto-resume path to recover the
    /// `conversation_id` (which we don't store on the
    /// approval row) so the synthetic tool message
    /// lands in the right conversation. Returns
    /// `Ok(None)` if the run id is unknown — callers
    /// treat that as a soft error (the original run
    /// may have been GC'd; we just skip the resume).
    pub fn get_bot_run(&self, run_id: &str) -> rusqlite::Result<Option<crate::bots::BotRun>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, bot_id, conversation_id, status, started_at, finished_at, result_summary, triggered_by
             FROM bot_runs WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        let status_str: String = row.get(3)?;
        let finished_str: Option<String> = row.get(5)?;
        let status = match status_str.as_str() {
            "running" => crate::bots::BotRunStatus::Running,
            "failed" => crate::bots::BotRunStatus::Failed,
            "cancelled" => crate::bots::BotRunStatus::Cancelled,
            _ => crate::bots::BotRunStatus::Succeeded,
        };
        Ok(Some(crate::bots::BotRun {
            id: row.get(0)?,
            bot_id: row.get(1)?,
            conversation_id: row.get(2)?,
            status,
            started_at: parse_dt(row.get::<_, String>(4)?),
            finished_at: finished_str.map(parse_dt),
            result_summary: row.get(6)?,
            triggered_by: row.get(7)?,
        }))
    }

    pub fn list_bot_runs(&self, bot_id: &str, limit: u32) -> rusqlite::Result<Vec<crate::bots::BotRun>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, bot_id, conversation_id, status, started_at, finished_at, result_summary, triggered_by
             FROM bot_runs WHERE bot_id = ?
             ORDER BY started_at DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![bot_id, limit as i64], |row| {
            let status_str: String = row.get(3)?;
            let finished_str: Option<String> = row.get(5)?;
            let status = match status_str.as_str() {
                "running" => crate::bots::BotRunStatus::Running,
                "failed" => crate::bots::BotRunStatus::Failed,
                "cancelled" => crate::bots::BotRunStatus::Cancelled,
                _ => crate::bots::BotRunStatus::Succeeded,
            };
            Ok(crate::bots::BotRun {
                id: row.get(0)?,
                bot_id: row.get(1)?,
                conversation_id: row.get(2)?,
                status,
                started_at: parse_dt(row.get::<_, String>(4)?),
                finished_at: finished_str.map(parse_dt),
                result_summary: row.get(6)?,
                triggered_by: row.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// v2.8.0 — ActivityFeed: most-recent Bot runs across
    /// **all** Bots, used by the Sidebar's ActivityFeed
    /// component. No `bot_id` filter. The query is
    /// intentionally cheap (small `LIMIT`); for the
    /// per-Bot history view the UI still uses
    /// `list_bot_runs`.
    pub fn list_recent_bot_runs(&self, limit: u32) -> rusqlite::Result<Vec<crate::bots::BotRun>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, bot_id, conversation_id, status, started_at, finished_at, result_summary, triggered_by
             FROM bot_runs
             ORDER BY started_at DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let status_str: String = row.get(3)?;
            let finished_str: Option<String> = row.get(5)?;
            let status = match status_str.as_str() {
                "running" => crate::bots::BotRunStatus::Running,
                "failed" => crate::bots::BotRunStatus::Failed,
                "cancelled" => crate::bots::BotRunStatus::Cancelled,
                _ => crate::bots::BotRunStatus::Succeeded,
            };
            Ok(crate::bots::BotRun {
                id: row.get(0)?,
                bot_id: row.get(1)?,
                conversation_id: row.get(2)?,
                status,
                started_at: parse_dt(row.get::<_, String>(4)?),
                finished_at: finished_str.map(parse_dt),
                result_summary: row.get(6)?,
                triggered_by: row.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Return the run ids of every bot run currently in the
    /// "running" state. Used by the UI to know which bots have a
    /// live run that a Stop button can fire against. The actual
    /// cancellation goes through the in-process `BotRunRegistry`,
    /// not the DB — the DB is the source of truth for display, the
    /// registry is the mechanism for cancellation.
    pub fn list_active_run_ids(&self) -> rusqlite::Result<Vec<String>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id FROM bot_runs
             WHERE status = 'running'
             ORDER BY started_at DESC
             LIMIT 50",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ----- inter-agent messages -----

    pub fn enqueue_bot_message(&self, msg: &crate::bots::BotMessage) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO bot_messages (id, from_bot_id, to_bot_id, body, created_at, read, conversation_id)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            params![
                msg.id,
                msg.from_bot_id,
                msg.to_bot_id,
                msg.body,
                msg.created_at.to_rfc3339(),
                msg.read as i64,
                msg.conversation_id,
            ],
        )?;
        Ok(())
    }

    pub fn list_inbox(&self, bot_id: &str, include_read: bool) -> rusqlite::Result<Vec<crate::bots::BotMessage>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let query = if include_read {
            "SELECT id, from_bot_id, to_bot_id, body, created_at, read, conversation_id
             FROM bot_messages WHERE to_bot_id = ? ORDER BY created_at ASC"
        } else {
            "SELECT id, from_bot_id, to_bot_id, body, created_at, read, conversation_id
             FROM bot_messages WHERE to_bot_id = ? AND read = 0 ORDER BY created_at ASC"
        };
        let mut stmt = conn.prepare(query)?;
        let rows = stmt.query_map(params![bot_id], |row| {
            Ok(crate::bots::BotMessage {
                id: row.get(0)?,
                from_bot_id: row.get(1)?,
                to_bot_id: row.get(2)?,
                body: row.get(3)?,
                created_at: parse_dt(row.get::<_, String>(4)?),
                read: row.get::<_, i64>(5)? != 0,
                conversation_id: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn mark_bot_messages_read(&self, bot_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE bot_messages SET read = 1 WHERE to_bot_id = ? AND read = 0",
            params![bot_id],
        )?;
        Ok(())
    }

    // ----- v2.0 Slice B: computers + ssh_keys -----
    //
    // Per-Bot libvirt VM state. The ComputerManager owns
    // these — Tauri commands dispatch into it, but the
    // table is read by the renderer for Status / Preview
    // mode badges.

    /// Upsert a computer row. Always writes — the caller
    /// (ComputerManager::provision) is the source of
    /// truth for whether the bot has a VM.
    pub fn upsert_computer(&self, c: &crate::computer::Computer) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let last_seen = c.last_seen_at.as_ref().map(|d| d.to_rfc3339());
        conn.execute(
            "INSERT INTO computers (bot_id, vm_name, vm_ip, vnc_port, ssh_key_id, state, last_seen_at, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(bot_id) DO UPDATE SET
                vm_name = excluded.vm_name,
                vm_ip = excluded.vm_ip,
                vnc_port = excluded.vnc_port,
                ssh_key_id = excluded.ssh_key_id,
                state = excluded.state,
                last_seen_at = excluded.last_seen_at",
            params![
                c.bot_id,
                c.vm_name,
                c.vm_ip,
                c.vnc_port.map(|p| p as i64),
                c.ssh_key_id,
                c.state,
                last_seen,
                c.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Update only the `state` column. Used by
    /// `computer_start / stop` after a successful virsh
    /// call. The other columns (ip, vnc_port, vm_name)
    /// don't change on a lifecycle action.
    pub fn set_computer_state(&self, bot_id: &str, state: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE computers SET state = ? WHERE bot_id = ?",
            params![state, bot_id],
        )?;
        Ok(())
    }

    pub fn get_computer(
        &self,
        bot_id: &str,
    ) -> rusqlite::Result<Option<crate::computer::Computer>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, vm_name, vm_ip, vnc_port, ssh_key_id, state, last_seen_at, created_at
             FROM computers WHERE bot_id = ?",
        )?;
        let mut rows = stmt.query(params![bot_id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        let last_seen: Option<String> = row.get(6)?;
        let vnc_port: Option<i64> = row.get(3)?;
        Ok(Some(crate::computer::Computer {
            bot_id: row.get(0)?,
            vm_name: row.get(1)?,
            vm_ip: row.get(2)?,
            vnc_port: vnc_port.map(|p| p as u16),
            ssh_key_id: row.get(4)?,
            state: row.get(5)?,
            last_seen_at: last_seen.map(parse_dt),
            created_at: parse_dt(row.get::<_, String>(7)?),
        }))
    }

    pub fn list_computers(&self) -> rusqlite::Result<Vec<crate::computer::Computer>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT bot_id, vm_name, vm_ip, vnc_port, ssh_key_id, state, last_seen_at, created_at
             FROM computers",
        )?;
        let rows = stmt.query_map([], |row| {
            let last_seen: Option<String> = row.get(6)?;
            let vnc_port: Option<i64> = row.get(3)?;
            Ok(crate::computer::Computer {
                bot_id: row.get(0)?,
                vm_name: row.get(1)?,
                vm_ip: row.get(2)?,
                vnc_port: vnc_port.map(|p| p as u16),
                ssh_key_id: row.get(4)?,
                state: row.get(5)?,
                last_seen_at: last_seen.map(parse_dt),
                created_at: parse_dt(row.get::<_, String>(7)?),
            })
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete_computer(&self, bot_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute("DELETE FROM computers WHERE bot_id = ?", params![bot_id])?;
        Ok(())
    }

    /// Upsert an SSH key row. The `private_key_encrypted`
    /// is a chacha20poly1305 blob; stored as BLOB.
    pub fn upsert_ssh_key(
        &self,
        id: &str,
        public_key: &str,
        private_key_encrypted: &[u8],
    ) -> rusqlite::Result<()> {
        let now = Utc::now().to_rfc3339();
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO ssh_keys (id, public_key, private_key_encrypted, created_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                public_key = excluded.public_key,
                private_key_encrypted = excluded.private_key_encrypted",
            params![id, public_key, private_key_encrypted, now],
        )?;
        Ok(())
    }

    pub fn get_ssh_key(&self, id: &str) -> rusqlite::Result<Option<SshKeyRow>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, public_key, private_key_encrypted, created_at
             FROM ssh_keys WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        Ok(Some(SshKeyRow {
            id: row.get(0)?,
            public_key: row.get(1)?,
            private_key_encrypted: row.get(2)?,
            created_at: parse_dt(row.get::<_, String>(3)?),
        }))
    }

    pub fn delete_ssh_key(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute("DELETE FROM ssh_keys WHERE id = ?", params![id])?;
        Ok(())
    }

    // ----- conversations (continued) -----

    /// Update a conversation's bot ownership. Used by the bot executor
    /// when it creates a fresh conversation for a scheduled run.
    pub fn set_conversation_bot(&self, conversation_id: &str, bot_id: Option<&str>) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE conversations SET bot_id = ? WHERE id = ?",
            params![bot_id, conversation_id],
        )?;
        Ok(())
    }

    // ----- v2.8.0 daemon_tokens -----

    /// Fetch the per-Bot bearer token used by the
    /// `maxbotd` HTTP server. Returns `None` if the user
    /// hasn't generated a token yet (the BotEditor
    /// surfaces this as "not set"). Used by both the
    /// Tauri app (to show the token / Copy / Rotate) and
    /// by `maxbotd` (to verify inbound `Authorization`
    /// headers).
    pub fn get_daemon_token(&self, bot_id: &str) -> rusqlite::Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn
            .prepare("SELECT token FROM daemon_tokens WHERE bot_id = ?")?;
        let raw: Option<String> = stmt
            .query_row(params![bot_id], |row| row.get(0))
            .optional()?;
        Ok(raw)
    }

    /// Persist (or replace) a per-Bot bearer token.
    /// `created_at` is stamped on first insert and
    /// preserved on update; `last_used` is updated
    /// separately by `touch_daemon_token`. The token
    /// value is the full hex string; no hashing.
    pub fn set_daemon_token(&self, bot_id: &str, token: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO daemon_tokens (bot_id, token, created_at)
             VALUES (?, ?, ?)
             ON CONFLICT(bot_id) DO UPDATE SET token = excluded.token",
            params![bot_id, token, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Stamp the current `last_used` for a token. Called
    /// by `maxbotd` after a successful inbound webhook
    /// so the UI can show "last used 3m ago".
    pub fn touch_daemon_token(&self, bot_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE daemon_tokens SET last_used = ? WHERE bot_id = ?",
            params![Utc::now().to_rfc3339(), bot_id],
        )?;
        Ok(())
    }

    /// Generate a fresh 32-byte random hex token, write
    /// it to the `daemon_tokens` table, and return it.
    /// Called by the BotEditor's "Generate" / "Rotate"
    /// button. The token never leaves the SQLite file in
    /// the clear unless the UI shows it (for the Copy
    /// action).
    pub fn rotate_daemon_token(&self, bot_id: &str) -> rusqlite::Result<String> {
        use rand::RngCore;
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        let token = bytes.iter().map(|b| format!("{:02x}", b)).collect::<String>();
        self.set_daemon_token(bot_id, &token)?;
        Ok(token)
    }

    /// Replace a message's tool_calls JSON and bump the conversation's
    /// updated_at. Called once at the end of a streaming turn, after the
    /// concatenated tool-call fragments are known.
    pub fn append_tool_calls(
        &self,
        message_id: &str,
        tool_calls: &[PersistedToolCall],
        conversation_id: &str,
    ) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE messages SET tool_calls_json = ? WHERE id = ?",
            params![
                serde_json::to_string(tool_calls).unwrap_or_else(|_| "[]".to_string()),
                message_id
            ],
        )?;
        conn.execute(
            "UPDATE conversations SET updated_at = ? WHERE id = ?",
            params![Utc::now().to_rfc3339(), conversation_id],
        )?;
        Ok(())
    }

    // ----- settings -----

    pub fn load_settings(&self) -> rusqlite::Result<Settings> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare("SELECT value FROM settings WHERE key = 'singleton'")?;
        let raw: Option<String> = stmt.query_row([], |row| row.get(0)).optional()?;
        match raw {
            Some(s) => serde_json::from_str(&s).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
                )
            }),
            None => Ok(Settings::default()),
        }
    }

    pub fn save_settings(&self, settings: &Settings) -> rusqlite::Result<()> {
        let raw = serde_json::to_string(settings).map_err(|e| {
            rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )))
        })?;
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO settings (key, value) VALUES ('singleton', ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![raw],
        )?;
        Ok(())
    }

    // ----- meta -----
    //
    // Small key/value table for first-run / onboarding state, schema
    // version, and other non-settings flags. Distinct from `settings`
    // because (a) it's a key/value shape rather than a JSON blob and
    // (b) it has no migration story for legacy users — we add columns
    // to `settings` when we need to add fields, but `meta` is fine
    // for flat strings.

    /// Look up a meta value by key. Returns None if the key is missing.
    pub fn meta_get(&self, key: &str) -> rusqlite::Result<Option<String>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?")?;
        let mut rows = stmt.query(params![key])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
    }

    /// Insert or overwrite a meta value. Always writes the new value —
    /// if the key already existed, the old value is replaced.
    pub fn meta_set(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO meta (key, value) VALUES (?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// List every (key, value) pair in the meta table. Used for the
    /// "Reset onboarding" debug flow and any future developer
    /// introspection. Ordering is by key for stable snapshots.
    pub fn meta_list(&self) -> rusqlite::Result<Vec<(String, String)>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare("SELECT key, value FROM meta ORDER BY key ASC")?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    // ----- v2.4.0 group chats -----
    //
    // Storage for the multi-Bot "group chat" view. The
    // schema is intentionally separate from the
    // `conversations` table: a group is a multi-Bot
    // collaboration, not a 1:1 chat with a single Bot.
    // The renderer distinguishes via the `mainView`
    // state (extended with `"group"` in v2.4.0).

    /// Every group, most-recently-updated first. Each
    /// row's `member_bot_ids` field is filled in by a
    /// second pass so the renderer can render the
    /// participant list without a follow-up call per
    /// group.
    pub fn list_groups(&self) -> rusqlite::Result<Vec<GroupChatWithMembers>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, owner_bot_id, created_at, updated_at
             FROM group_chats ORDER BY updated_at DESC",
        )?;
        let groups: Vec<GroupChat> = stmt
            .query_map([], |row| {
                Ok(GroupChat {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    owner_bot_id: row.get(2)?,
                    created_at: parse_dt(row.get::<_, String>(3)?),
                    updated_at: parse_dt(row.get::<_, String>(4)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // Hydrate member lists. We do this in a second
        // pass (rather than a single JOIN) so an empty
        // membership doesn't truncate the group from the
        // response — a UI that wants the raw group list
        // can call `list_groups` and get every row
        // regardless of member count.
        let mut stmt_m = conn.prepare(
            "SELECT bot_id FROM group_members WHERE group_id = ? ORDER BY bot_id",
        )?;
        let mut out = Vec::with_capacity(groups.len());
        for g in groups {
            let members: Vec<String> = stmt_m
                .query_map(params![g.id], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            out.push(GroupChatWithMembers {
                chat: g,
                member_bot_ids: members,
            });
        }
        Ok(out)
    }

    /// Look up a single group (with members) by id.
    pub fn get_group(
        &self,
        id: &str,
    ) -> rusqlite::Result<Option<GroupChatWithMembers>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let chat = match conn
            .query_row(
                "SELECT id, name, owner_bot_id, created_at, updated_at
                 FROM group_chats WHERE id = ?",
                params![id],
                |row| {
                    Ok(GroupChat {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        owner_bot_id: row.get(2)?,
                        created_at: parse_dt(row.get::<_, String>(3)?),
                        updated_at: parse_dt(row.get::<_, String>(4)?),
                    })
                },
            )
            .optional()?
        {
            Some(c) => c,
            None => return Ok(None),
        };
        let mut stmt = conn.prepare(
            "SELECT bot_id FROM group_members WHERE group_id = ? ORDER BY bot_id",
        )?;
        let members: Vec<String> = stmt
            .query_map(params![id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(Some(GroupChatWithMembers {
            chat,
            member_bot_ids: members,
        }))
    }

    /// Atomically create a group + insert the owner +
    /// the additional members. The whole operation
    /// happens in a single transaction so we never end
    /// up with a group_chats row and a missing
    /// group_members row (which would render the group
    /// empty in the sidebar). Returns the persisted
    /// group + the full member list.
    pub fn create_group(
        &self,
        name: &str,
        owner_bot_id: &str,
        member_bot_ids: &[String],
    ) -> rusqlite::Result<GroupChatWithMembers> {
        let now = Utc::now();
        let mut conn = self.conn.lock().expect("db lock poisoned");
        let tx = conn.transaction()?;
        let group_id = Uuid::new_v4().to_string();
        tx.execute(
            "INSERT INTO group_chats (id, name, owner_bot_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
            params![
                group_id,
                name,
                owner_bot_id,
                now.to_rfc3339(),
                now.to_rfc3339()
            ],
        )?;
        // Insert the owner as a member too, then any
        // additional members. De-dupe the combined
        // list so a caller that accidentally re-passes
        // the owner doesn't trip the PRIMARY KEY.
        let mut seen = std::collections::HashSet::new();
        let mut all = Vec::with_capacity(member_bot_ids.len() + 1);
        all.push(owner_bot_id.to_string());
        for m in member_bot_ids {
            if seen.insert(m.clone()) {
                all.push(m.clone());
            }
        }
        // The owner is unconditionally included; if
        // the caller also passed it, drop the
        // duplicate after the set check.
        all.dedup();
        for m in &all {
            tx.execute(
                "INSERT INTO group_members (group_id, bot_id) VALUES (?, ?)",
                params![group_id, m],
            )?;
        }
        tx.commit()?;
        Ok(GroupChatWithMembers {
            chat: GroupChat {
                id: group_id,
                name: name.to_string(),
                owner_bot_id: owner_bot_id.to_string(),
                created_at: now,
                updated_at: now,
            },
            member_bot_ids: all,
        })
    }

    /// Add a Bot to a group. Idempotent — re-adding a
    /// member is a no-op so the renderer's "select all"
    /// path doesn't have to pre-check.
    pub fn add_group_member(&self, group_id: &str, bot_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        // Bump the group's updated_at so the sidebar
        // re-orders to the top.
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT OR IGNORE INTO group_members (group_id, bot_id) VALUES (?, ?)",
            params![group_id, bot_id],
        )?;
        conn.execute(
            "UPDATE group_chats SET updated_at = ? WHERE id = ?",
            params![now, group_id],
        )?;
        Ok(())
    }

    /// Remove a Bot from a group. Removing the owner is
    /// a no-op (the owner's row stays; groups are
    /// immutable w.r.t. ownership for v2.4).
    pub fn remove_group_member(&self, group_id: &str, bot_id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let now = Utc::now().to_rfc3339();
        // Refuse to remove the owner — the renderer
        // doesn't surface this option, and silently
        // leaving an owner-less group would break
        // queries.
        let owner: Option<String> = conn
            .query_row(
                "SELECT owner_bot_id FROM group_chats WHERE id = ?",
                params![group_id],
                |row| row.get(0),
            )
            .optional()?;
        if owner.as_deref() == Some(bot_id) {
            return Ok(());
        }
        conn.execute(
            "DELETE FROM group_members WHERE group_id = ? AND bot_id = ?",
            params![group_id, bot_id],
        )?;
        conn.execute(
            "UPDATE group_chats SET updated_at = ? WHERE id = ?",
            params![now, group_id],
        )?;
        Ok(())
    }

    /// Append a message to a group's transcript. The
    /// `mentions_json` column is set from
    /// `serde_json::to_string(&mentions)`; the executor
    /// passes the user-typed mention list so the
    /// renderer can render a "→ @Writer" badge on the
    /// row.
    pub fn append_group_message(
        &self,
        group_id: &str,
        bot_id: Option<&str>,
        role: &str,
        content: &str,
        mentions: &[String],
        handoff_to: Option<&str>,
    ) -> rusqlite::Result<GroupMessage> {
        let now = Utc::now();
        let id = Uuid::new_v4().to_string();
        let mentions_json =
            serde_json::to_string(mentions).unwrap_or_else(|_| "[]".to_string());
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO group_messages
                (id, group_id, bot_id, role, content, mentions_json, handoff_to, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                id,
                group_id,
                bot_id,
                role,
                content,
                mentions_json,
                handoff_to,
                now.to_rfc3339(),
            ],
        )?;
        // Bump the group's updated_at so the sidebar
        // re-orders.
        conn.execute(
            "UPDATE group_chats SET updated_at = ? WHERE id = ?",
            params![now.to_rfc3339(), group_id],
        )?;
        Ok(GroupMessage {
            id,
            group_id: group_id.to_string(),
            bot_id: bot_id.map(str::to_string),
            role: role.to_string(),
            content: content.to_string(),
            mentions: mentions.to_vec(),
            handoff_to: handoff_to.map(str::to_string),
            created_at: now,
        })
    }

    /// List the most-recent `limit` messages for a
    /// group, oldest-first (so the renderer can append
    /// directly to the transcript). `limit = 0` means
    /// "all" — used by the history-on-open path.
    pub fn list_group_messages(
        &self,
        group_id: &str,
        limit: u32,
    ) -> rusqlite::Result<Vec<GroupMessage>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let limit_clause = if limit == 0 {
            String::new()
        } else {
            format!(" LIMIT {}", limit)
        };
        let sql = format!(
            "SELECT id, group_id, bot_id, role, content, mentions_json, handoff_to, created_at
             FROM group_messages
             WHERE group_id = ?
             ORDER BY created_at ASC{}",
            limit_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![group_id], |row| {
            let mentions_json: String = row.get(5)?;
            let mentions: Vec<String> =
                serde_json::from_str(&mentions_json).unwrap_or_default();
            Ok(GroupMessage {
                id: row.get(0)?,
                group_id: row.get(1)?,
                bot_id: row.get(2)?,
                role: row.get(3)?,
                content: row.get(4)?,
                mentions,
                handoff_to: row.get(6)?,
                created_at: parse_dt(row.get::<_, String>(7)?),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

fn parse_dt(value: String) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(&value)
        .map(|d| d.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

/// Cheap wrapper that lets the FromStr impl for chrono errors flow through
/// where needed. Unused at the moment, but kept here so future call sites
/// don't repeat the `Box::new(io::Error::...)` pattern.
#[allow(dead_code)]
fn io_err<E: ToString>(e: E) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
}

/// SQLite has no `ALTER TABLE ... ADD COLUMN IF NOT EXISTS`, so we
/// probe `pragma_table_info` and add the column if it's missing. Used
/// for backwards-compatible migrations on tables that may already
/// exist from an older schema version.
fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    column_def: &str,
) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({})", table))?;
    let mut rows = stmt.query([])?;
    let mut has_column = false;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            has_column = true;
            break;
        }
    }
    if !has_column {
        conn.execute(
            &format!(
                "ALTER TABLE {} ADD COLUMN {} {}",
                table, column, column_def
            ),
            [],
        )?;
    }
    Ok(())
}

// ----- v2.2.0 Skills -----
//
// The DB layer for Skills is defined here (alongside the
// other `impl Database` blocks) so it can access the
// private `conn` field. The pure-Rust shapes (`Skill`,
// `Step`, `Param`, `SkillRun`) live in
// `crate::skills`; the JSON-blob columns (`inputs_json`,
// `steps_json`) round-trip them.

impl Database {
    /// Insert or replace a Skill row. `inputs` and `steps`
    /// are JSON-serialized into the table; everything else
    /// is a scalar column. ON CONFLICT(id) refreshes the
    /// editable fields — used by both `skill_create` (new
    /// id) and the future "rename / edit" path.
    pub fn upsert_skill(&self, skill: &crate::skills::Skill) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let inputs_json =
            serde_json::to_string(&skill.inputs).unwrap_or_else(|_| "[]".to_string());
        let steps_json =
            serde_json::to_string(&skill.steps).unwrap_or_else(|_| "[]".to_string());
        conn.execute(
            "INSERT INTO skills (id, name, description, inputs_json, steps_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                inputs_json = excluded.inputs_json,
                steps_json = excluded.steps_json,
                updated_at = excluded.updated_at",
            params![
                skill.id,
                skill.name,
                skill.description,
                inputs_json,
                steps_json,
                skill.created_at.to_rfc3339(),
                skill.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn get_skill(&self, id: &str) -> rusqlite::Result<Option<crate::skills::Skill>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, description, inputs_json, steps_json, created_at, updated_at
             FROM skills WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        Ok(Some(parse_skill_row(&row)?))
    }

    /// All skills, most-recently-updated first. The renderer
    /// uses this to populate the Skills tab. No pagination
    /// — the typical install has tens, not thousands.
    pub fn list_skills(&self) -> rusqlite::Result<Vec<crate::skills::Skill>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, name, description, inputs_json, steps_json, created_at, updated_at
             FROM skills
             ORDER BY updated_at DESC",
        )?;
        let rows = stmt.query_map([], parse_skill_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete_skill(&self, id: &str) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        // CASCADE on `skill_runs.skill_id` removes the
        // runs automatically.
        conn.execute("DELETE FROM skills WHERE id = ?", params![id])?;
        Ok(())
    }

    /// v3.3.0 — Re-record: overwrite the editable fields of an
    /// existing Skill in place. Preserves `id` and `created_at`;
    /// bumps `updated_at`. The `name`, `description`,
    /// `inputs_json`, and `steps_json` columns are replaced with
    /// the caller's new values. Any `bot_schedules.skill_id`
    /// pointing at this row is untouched (the schedule lives
    /// on the bot, not the skill). Returns the updated row, or
    /// `None` if no Skill with that id exists. This is the
    /// storage half of the `skill_update` Tauri command — the
    /// UI's "Re-record" button calls into that command with
    /// the skill's existing id and the user's edited JSON.
    pub fn update_skill_in_place(
        &self,
        id: &str,
        name: &str,
        description: &str,
        inputs_json: &str,
        steps_json: &str,
    ) -> rusqlite::Result<Option<crate::skills::Skill>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        // Look up the existing row first so we can preserve
        // `created_at` and return the merged shape. The
        // upsert path is `ON CONFLICT(id) DO UPDATE`, which
        // would clobber `created_at` if we passed the new
        // shape's `created_at` here — so we read it instead
        // and write it back.
        let existing_created_at: Option<String> = conn
            .query_row(
                "SELECT created_at FROM skills WHERE id = ?",
                params![id],
                |row| row.get(0),
            )
            .ok();
        let Some(created_at) = existing_created_at else {
            return Ok(None);
        };
        let now = Utc::now();
        let updated = conn.execute(
            "UPDATE skills SET
                name = ?,
                description = ?,
                inputs_json = ?,
                steps_json = ?,
                updated_at = ?
             WHERE id = ?",
            params![name, description, inputs_json, steps_json, now.to_rfc3339(), id],
        )?;
        if updated == 0 {
            return Ok(None);
        }
        // Build the merged shape and return it so the
        // renderer can refresh its row without a follow-up
        // get_skill round-trip.
        Ok(Some(crate::skills::Skill {
            id: id.to_string(),
            name: name.to_string(),
            description: description.to_string(),
            inputs: serde_json::from_str(inputs_json).unwrap_or_default(),
            steps: serde_json::from_str(steps_json).unwrap_or_default(),
            created_at: parse_dt(created_at),
            updated_at: now,
        }))
    }

    /// Insert a fresh `skill_runs` row. The caller fills
    /// in id, status, started_at; finished_at and
    /// result_summary are blanked and updated later via
    /// `update_skill_run`.
    pub fn insert_skill_run(&self, run: &crate::skills::SkillRun) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let inputs_json =
            serde_json::to_string(&run.inputs).unwrap_or_else(|_| "{}".to_string());
        conn.execute(
            "INSERT INTO skill_runs (id, skill_id, bot_id, inputs_json, status, started_at, finished_at, result_summary)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                run.id,
                run.skill_id,
                run.bot_id,
                inputs_json,
                run.status.as_str(),
                run.started_at.to_rfc3339(),
                run.finished_at.as_ref().map(|d| d.to_rfc3339()),
                run.result_summary,
            ],
        )?;
        Ok(())
    }

    /// Patch a run with the latest status, finished_at,
    /// and result_summary. The `inputs_json` is set on
    /// insert and never updated — the user's inputs are
    /// an immutable record of how the run started.
    pub fn update_skill_run(&self, run: &crate::skills::SkillRun) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "UPDATE skill_runs SET
                status = ?,
                finished_at = ?,
                result_summary = ?
             WHERE id = ?",
            params![
                run.status.as_str(),
                run.finished_at.as_ref().map(|d| d.to_rfc3339()),
                run.result_summary,
                run.id,
            ],
        )?;
        Ok(())
    }

    /// Most-recent runs for a Skill, used by the
    /// run-history panel under each Skill. `limit` is
    /// `None` for "give me all of them" (the typical
    /// case — few runs per Skill).
    pub fn list_skill_runs(
        &self,
        skill_id: &str,
        limit: Option<u32>,
    ) -> rusqlite::Result<Vec<crate::skills::SkillRun>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let query = if let Some(n) = limit {
            format!(
                "SELECT id, skill_id, bot_id, inputs_json, status, started_at, finished_at, result_summary
                 FROM skill_runs WHERE skill_id = ?
                 ORDER BY started_at DESC LIMIT {}",
                n as i64
            )
        } else {
            "SELECT id, skill_id, bot_id, inputs_json, status, started_at, finished_at, result_summary
             FROM skill_runs WHERE skill_id = ?
             ORDER BY started_at DESC"
                .to_string()
        };
        let mut stmt = conn.prepare(&query)?;
        let rows = stmt.query_map(params![skill_id], parse_skill_run_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// v2.8.0 — ActivityFeed: most-recent Skill runs across
    /// **all** Skills, used by the Sidebar's ActivityFeed
    /// component. No `skill_id` filter. The `LIMIT` is
    /// small (5 from the renderer). The result is sorted
    /// by `started_at DESC` so the newest run is first.
    pub fn list_recent_skill_runs(
        &self,
        limit: u32,
    ) -> rusqlite::Result<Vec<crate::skills::SkillRun>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, skill_id, bot_id, inputs_json, status, started_at, finished_at, result_summary
             FROM skill_runs
             ORDER BY started_at DESC LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit as i64], parse_skill_run_row)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Single-run lookup. Used by the run-progress card
    /// on the Skills panel: the renderer keeps a `run_id`
    /// it got from `skill_run` and polls this every
    /// second while the run is in progress.
    pub fn get_skill_run(&self, run_id: &str) -> rusqlite::Result<Option<crate::skills::SkillRun>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, skill_id, bot_id, inputs_json, status, started_at, finished_at, result_summary
             FROM skill_runs WHERE id = ?",
        )?;
        let mut rows = stmt.query(params![run_id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        Ok(Some(parse_skill_run_row(&row)?))
    }

    // ----- v3.3.0 — Skill run traces (per-step output) -----

    /// Insert a per-step trace row for a run. Best-effort:
    /// the executor's call site logs failures but does NOT
    /// propagate them. The `per_step_output` is a JSON-encoded
    /// array of `{role, content, tool_name, tool_args,
    /// tool_result, ts}` — see `crate::skills::StepTrace` for
    /// the shape. `trigger_input` is the user message or
    /// scheduled payload that started the run; for empty /
    /// scheduled runs the executor passes an empty string.
    pub fn insert_skill_run_trace(&self, trace: &crate::skills::SkillRunTrace) -> rusqlite::Result<()> {
        let conn = self.conn.lock().expect("db lock poisoned");
        conn.execute(
            "INSERT INTO skill_run_traces
                (id, run_id, skill_id, started_at, duration_ms, per_step_output, success, trigger_input)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                trace.id,
                trace.run_id,
                trace.skill_id,
                trace.started_at.to_rfc3339(),
                trace.duration_ms as i64,
                trace.per_step_output_json(),
                if trace.success { 1 } else { 0 },
                trace.trigger_input,
            ],
        )?;
        Ok(())
    }

    /// Most-recent trace row for a Skill. Powers the
    /// Skills panel's "Last run" expandable view. Returns
    /// `None` if the Skill has never been run.
    pub fn latest_skill_run_trace(
        &self,
        skill_id: &str,
    ) -> rusqlite::Result<Option<crate::skills::SkillRunTrace>> {
        let conn = self.conn.lock().expect("db lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, run_id, skill_id, started_at, duration_ms, per_step_output, success, trigger_input
             FROM skill_run_traces WHERE skill_id = ?
             ORDER BY started_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query(params![skill_id])?;
        let row = match rows.next()? {
            Some(r) => r,
            None => return Ok(None),
        };
        Ok(Some(parse_skill_run_trace_row(&row)?))
    }
}

fn parse_skill_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::skills::Skill> {
    let inputs_json: String = row.get(3)?;
    let steps_json: String = row.get(4)?;
    let inputs: Vec<crate::skills::Param> = serde_json::from_str(&inputs_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
        )
    })?;
    let steps: Vec<crate::skills::Step> = serde_json::from_str(&steps_json).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
        )
    })?;
    Ok(crate::skills::Skill {
        id: row.get(0)?,
        name: row.get(1)?,
        description: row.get(2)?,
        inputs,
        steps,
        created_at: parse_dt(row.get::<_, String>(5)?),
        updated_at: parse_dt(row.get::<_, String>(6)?),
    })
}

fn parse_skill_run_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::skills::SkillRun> {
    let inputs_json: String = row.get(3)?;
    let status_str: String = row.get(4)?;
    let finished_str: Option<String> = row.get(6)?;
    let inputs: serde_json::Value = serde_json::from_str(&inputs_json)
        .unwrap_or(serde_json::Value::Object(Default::default()));
    Ok(crate::skills::SkillRun {
        id: row.get(0)?,
        skill_id: row.get(1)?,
        bot_id: row.get(2)?,
        inputs,
        status: match status_str.as_str() {
            "running" => crate::skills::SkillRunStatus::Running,
            "failed" => crate::skills::SkillRunStatus::Failed,
            "cancelled" => crate::skills::SkillRunStatus::Cancelled,
            _ => crate::skills::SkillRunStatus::Succeeded,
        },
        started_at: parse_dt(row.get::<_, String>(5)?),
        finished_at: finished_str.map(parse_dt),
        result_summary: row.get(7)?,
        // Per-step progress is in-memory only; the DB row
        // does not persist it.
        steps: Vec::new(),
    })
}

fn parse_skill_run_trace_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<crate::skills::SkillRunTrace> {
    let per_step_json: String = row.get(5)?;
    let success_int: i64 = row.get(6)?;
    let per_step: Vec<crate::skills::StepTrace> = serde_json::from_str(&per_step_json)
        .unwrap_or_default();
    Ok(crate::skills::SkillRunTrace {
        id: row.get(0)?,
        run_id: row.get(1)?,
        skill_id: row.get(2)?,
        started_at: parse_dt(row.get::<_, String>(3)?),
        duration_ms: row.get::<_, i64>(4)? as u64,
        per_step,
        success: success_int != 0,
        trigger_input: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a fresh in-memory database for each test so they don't
    /// share state. SQLite's `:memory:` is per-connection, so we
    /// open a new Database against a tempdir-backed path — same
    /// shape as the production code, but no shared file.
    fn fresh_db() -> Database {
        let dir = std::env::temp_dir().join(format!("maxbot-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.sqlite");
        Database::open(&path).expect("open test db")
    }

    #[test]
    fn meta_get_set_round_trip() {
        let db = fresh_db();
        assert_eq!(db.meta_get("is_onboarded").unwrap(), None);
        db.meta_set("is_onboarded", "1").unwrap();
        assert_eq!(db.meta_get("is_onboarded").unwrap().as_deref(), Some("1"));
        // Distinct keys are independent — writing one doesn't touch
        // the other.
        db.meta_set("schema_version", "7").unwrap();
        assert_eq!(db.meta_get("is_onboarded").unwrap().as_deref(), Some("1"));
        assert_eq!(db.meta_get("schema_version").unwrap().as_deref(), Some("7"));
    }

    #[test]
    fn meta_set_overwrites_previous_value() {
        let db = fresh_db();
        db.meta_set("is_onboarded", "0").unwrap();
        assert_eq!(db.meta_get("is_onboarded").unwrap().as_deref(), Some("0"));
        // Calling meta_set again on the same key replaces the value
        // — important for the "reset onboarding" path, which writes
        // the empty string to clear the flag.
        db.meta_set("is_onboarded", "1").unwrap();
        assert_eq!(db.meta_get("is_onboarded").unwrap().as_deref(), Some("1"));
        db.meta_set("is_onboarded", "").unwrap();
        assert_eq!(db.meta_get("is_onboarded").unwrap().as_deref(), Some(""));
        // meta_list also reflects the latest value.
        let all = db.meta_list().unwrap();
        let entry = all.iter().find(|(k, _)| k == "is_onboarded").unwrap();
        assert_eq!(entry.1, "");
    }

    // ----- v0.7.6 Message JSON shape backwards-compat -----
    // The wire shape of `Message` gained an `error_message` field.
    // Old v0.7.5 JSON blobs (no field) must still deserialize, with
    // `error_message: None`. If they didn't, every pre-v0.7.6
    // conversation would 500 on load.

    #[test]
    fn message_without_error_message_deserializes_to_none() {
        let raw = r#"{
            "id": "msg-old",
            "conversation_id": "conv-1",
            "role": "assistant",
            "content": "hello there",
            "tool_calls": [],
            "created_at": "2026-01-01T00:00:00Z"
        }"#;
        let msg: Message = serde_json::from_str(raw)
            .expect("old message JSON should still deserialize");
        assert_eq!(msg.id, "msg-old");
        assert_eq!(msg.content, "hello there");
        assert!(
            msg.error_message.is_none(),
            "old blob must default to error_message: None, got {:?}",
            msg.error_message
        );
    }

    #[test]
    fn message_with_error_message_round_trips() {
        let raw = r#"{
            "id": "msg-new",
            "conversation_id": "conv-1",
            "role": "assistant",
            "content": "partial response",
            "tool_calls": [],
            "created_at": "2026-01-01T00:00:00Z",
            "error_message": "Connection lost — check your network"
        }"#;
        let msg: Message = serde_json::from_str(raw).expect("new message JSON should parse");
        assert_eq!(
            msg.error_message.as_deref(),
            Some("Connection lost — check your network")
        );
        // Re-serialize and re-parse: ensures the new field is
        // actually emitted, not silently dropped by serde.
        let again: Message =
            serde_json::from_str(&serde_json::to_string(&msg).unwrap()).unwrap();
        assert_eq!(again.error_message, msg.error_message);
        assert_eq!(again.content, msg.content);
    }

    #[test]
    fn message_with_explicit_null_error_message_stays_null() {
        let raw = r#"{
            "id": "msg-null",
            "conversation_id": "conv-1",
            "role": "assistant",
            "content": "ok",
            "tool_calls": [],
            "created_at": "2026-01-01T00:00:00Z",
            "error_message": null
        }"#;
        let msg: Message = serde_json::from_str(raw)
            .expect("explicit null should deserialize");
        assert!(msg.error_message.is_none());
    }

    // ----- Settings JSON shape backwards-compat (grok_session_id) -----
    // `Settings` gained an optional `grok_session_id` field for the
    // Grok Build CLI session-resume path. Old settings JSON blobs
    // (no `grok_session_id` key) must still deserialize, with
    // `grok_session_id: None`. If they didn't, every pre-v0.7.6
    // user would 500 on launch and lose their saved API keys.
    //
    // The smallest possible "old blob" is the empty object — the
    // Settings struct is built with `#[serde(default)]` on every
    // field, so `{}` must parse into a `Settings` whose every
    // field is the type's default (empty strings, None options).

    #[test]
    fn empty_settings_blob_deserializes() {
        let s: Settings = serde_json::from_str("{}")
            .expect("empty settings blob must deserialize (every field is #[serde(default)])");
        assert_eq!(s.provider_kind, "");
        assert!(s.minimax_api_key.is_none());
        assert!(s.openai_api_key.is_none());
        assert!(s.anthropic_api_key.is_none());
        assert!(s.xai_api_key.is_none());
        assert_eq!(s.default_model, "");
        assert_eq!(s.minimax_base_url, "");
        assert_eq!(s.tts_voice, "");
        assert_eq!(s.grok_build_binary, "");
        assert_eq!(s.grok_build_model, "");
        assert_eq!(s.grok_cwd, "");
        assert!(s.grok_session_id.is_none(), "new field must default to None");
        assert_eq!(s.openai_base_url, "");
        assert_eq!(s.anthropic_base_url, "");
        assert_eq!(s.xai_base_url, "");
        // v2.3.5: the default-key flag must default to on so
        // existing DBs that haven't seen the field get the
        // new behavior (no passphrase gate).
        assert!(s.computer_use_default_ssh_key, "default-key flag must default to true");
    }

    #[test]
    fn old_settings_blob_round_trips_through_database() {
        // Write a pre-v0.7.6 settings blob (no `grok_session_id`)
        // directly into the SQLite settings row, then read it back
        // through `load_settings`. Every field must default
        // correctly; the new `grok_session_id` field must be
        // `None`.
        let db = fresh_db();
        let old_blob = r#"{
            "provider_kind": "openai",
            "openai_api_key": "sk-old",
            "default_model": "gpt-4o-mini"
        }"#;
        // Persist the blob the same way `save_settings` would,
        // but without going through `Settings` first (since the
        // old blob doesn't have the new field, that round-trip
        // is exactly what we're testing).
        {
            let conn = db.conn.lock().expect("db lock poisoned");
            conn.execute(
                "INSERT INTO settings (key, value) VALUES ('singleton', ?)",
                params![old_blob],
            )
            .expect("write old settings blob");
        }
        let s = db.load_settings().expect("load old settings blob");
        assert_eq!(s.provider_kind, "openai");
        assert_eq!(s.openai_api_key.as_deref(), Some("sk-old"));
        assert_eq!(s.default_model, "gpt-4o-mini");
        // Every other field must default — most importantly the
        // new `grok_session_id` field, which the old blob doesn't
        // carry.
        assert!(s.grok_session_id.is_none());
        assert!(s.anthropic_api_key.is_none());
        assert!(s.xai_api_key.is_none());
        assert!(s.minimax_api_key.is_none());
        assert_eq!(s.minimax_base_url, "");
        assert_eq!(s.grok_build_binary, "");
        assert_eq!(s.grok_build_model, "");
        assert_eq!(s.grok_cwd, "");
    }

    // ----- v2.0 Slice E: Bot presence columns -----
    //
    // The `bots` table gained `avatar_color`, `last_active_at`,
    // and `state` columns. Existing rows pre-dating the
    // migration must still load with sensible defaults:
    //   - avatar_color: empty string
    //   - last_active_at: None
    //   - state: Idle (the SQL DEFAULT 'idle')

    fn seed_bot(db: &Database, id: &str, name: &str) {
        let now = chrono::Utc::now();
        let bot = crate::bots::Bot {
            id: id.to_string(),
            name: name.to_string(),
            description: "".to_string(),
            system_prompt: "".to_string(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: "🤖".to_string(),
            color: "".to_string(),
            avatar_color: "".to_string(),
            last_active_at: None,
            state: crate::bots::BotState::Idle,
            // v3.2.0 — `computer_use` defaults to "vm" for
            // new Bots. These tests don't exercise the
            // field; the value is just here so the struct
            // literal compiles.
            computer_use: "vm".to_string(),
            // v3.7.0 (Phase 8) — `connectors_enabled`
            // is the new per-Bot field for the
            // connector enable toggles. Empty string =
            // no connectors enabled. None of the
            // existing test fixtures exercise the
            // connector surface; the empty value is
            // the safe default that matches a Bot
            // row written by pre-v3.7.0 code.
            connectors_enabled: String::new(),
            created_at: now,
            updated_at: now,
        };
        db.upsert_bot(&bot).expect("upsert bot");
    }

    #[test]
    fn bot_set_state_updates_the_persisted_column() {
        let db = fresh_db();
        seed_bot(&db, "b1", "Alpha");
        // Default state is Idle.
        let b = db.get_bot("b1").unwrap().expect("bot exists");
        assert_eq!(b.state, crate::bots::BotState::Idle);
        // Promote to Working.
        let updated =
            db.set_bot_state("b1", crate::bots::BotState::Working)
                .expect("set state");
        assert_eq!(updated, 1, "one row should be affected");
        let b = db.get_bot("b1").unwrap().expect("bot exists");
        assert_eq!(b.state, crate::bots::BotState::Working);
        // Promote to Blocked (the most common post-run state).
        db.set_bot_state("b1", crate::bots::BotState::Blocked)
            .expect("set state");
        let b = db.get_bot("b1").unwrap().expect("bot exists");
        assert_eq!(b.state, crate::bots::BotState::Blocked);
    }

    #[test]
    fn bot_set_state_returns_zero_for_unknown_id() {
        let db = fresh_db();
        // The renderer should never block on a stale state
        // write; the count of 0 is a useful signal that the
        // Bot no longer exists.
        let updated =
            db.set_bot_state("does-not-exist", crate::bots::BotState::Idle)
                .expect("set state on missing id");
        assert_eq!(updated, 0);
    }

    #[test]
    fn bot_touch_last_active_records_a_timestamp() {
        let db = fresh_db();
        seed_bot(&db, "b1", "Alpha");
        // Brand-new bot — no last_active_at.
        let b = db.get_bot("b1").unwrap().expect("bot exists");
        assert!(b.last_active_at.is_none());
        // Touch it.
        let updated = db.touch_bot_last_active("b1").expect("touch");
        assert_eq!(updated, 1);
        let b = db.get_bot("b1").unwrap().expect("bot exists");
        let ts = b
            .last_active_at
            .expect("last_active_at should be set after touch");
        let now = chrono::Utc::now();
        // The timestamp is within a second of "now" — the
        // touch writes the current UTC time.
        let delta = (now - ts).num_seconds().abs();
        assert!(
            delta <= 1,
            "last_active_at should be within 1s of now, got {delta}s"
        );
    }

    #[test]
    fn bot_round_trip_preserves_avatar_color_and_state() {
        let db = fresh_db();
        let now = chrono::Utc::now();
        let bot = crate::bots::Bot {
            id: "b1".to_string(),
            name: "Alpha".to_string(),
            description: "test".to_string(),
            system_prompt: "".to_string(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: "🤖".to_string(),
            color: "#7c5cff".to_string(),
            avatar_color: "#ff5c7c".to_string(),
            last_active_at: Some(now),
            state: crate::bots::BotState::Thinking,
            // v3.2.0 — added the `computer_use` field.
            // This test pins avatar_color + state +
            // last_active_at round-trip; the value here is
            // just to make the struct literal compile.
            computer_use: "vm".to_string(),
            // v3.7.0 (Phase 8) — added the
            // `connectors_enabled` field. The test
            // doesn't exercise the connector
            // surface; the value is here only so
            // the struct literal compiles.
            connectors_enabled: String::new(),
            created_at: now,
            updated_at: now,
        };
        db.upsert_bot(&bot).expect("upsert bot");
        let loaded = db.get_bot("b1").unwrap().expect("bot exists");
        assert_eq!(loaded.avatar_color, "#ff5c7c");
        assert_eq!(loaded.state, crate::bots::BotState::Thinking);
        assert!(loaded.last_active_at.is_some());
    }

    // v2.0.1 regression: a v1.0 row upgraded to v2.0 has NULL
    // `avatar_color` and `last_active_at` (no DEFAULT in the
    // migration). Reading those as non-Option used to fail with
    // "Invalid column type Null at index: 8, name: avatar_color",
    // which left the React app stuck on its loading state —
    // visually a blank black window. The list/get paths now read
    // the nullable columns as `Option<String>` and default to "" /
    // None so v1.0 rows load cleanly.
    #[test]
    fn v1_bot_with_null_avatar_color_loads_with_empty_string() {
        let db = fresh_db();
        // Simulate a v1.0 install by creating the bot through the
        // upsert path BEFORE the v2.0 columns existed. The easiest
        // way is to insert a row directly with only the v1.0
        // columns populated, then call list_bots / get_bot.
        let conn = db.conn.lock().expect("db lock poisoned");
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO bots (id, name, description, system_prompt,
                default_model, allowed_tools, icon, color,
                created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "v1-bot",
                "Legacy",
                "from v1.0",
                "",
                "MiniMax-M3",
                "[]",
                "",
                "",
                &now,
                &now,
            ],
        )
        .expect("insert v1.0-style row");
        drop(conn);

        // Both reads must succeed and yield the empty defaults.
        let loaded = db.get_bot("v1-bot").unwrap().expect("bot exists");
        assert_eq!(loaded.avatar_color, "");
        assert!(loaded.last_active_at.is_none());
        assert_eq!(loaded.state, crate::bots::BotState::Idle);

        let listed = db.list_bots().expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].avatar_color, "");
        assert!(listed[0].last_active_at.is_none());
    }

    // ---- v2.8.0 — daemon_tokens ----

    /// Helper: insert a minimal Bot row so the
    /// `daemon_tokens.bot_id` foreign key is satisfied.
    /// The token tests only care about the token
    /// column, not the bot's full shape.
    fn insert_minimal_bot(db: &Database, id: &str) {
        let now = chrono::Utc::now();
        db.upsert_bot(&crate::bots::Bot {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            system_prompt: String::new(),
            default_model: "MiniMax-M3".to_string(),
            allowed_tools: vec![],
            icon: String::new(),
            color: String::new(),
            avatar_color: String::new(),
            created_at: now,
            updated_at: now,
            state: crate::bots::BotState::Idle,
            last_active_at: None,
            // v3.2.0 — `computer_use` defaults to "vm" for
            // new Bots. The token tests don't exercise
            // the field.
            computer_use: "vm".to_string(),
            // v3.7.0 (Phase 8) — empty
            // `connectors_enabled` is the safe
            // default; the token tests don't
            // exercise the connector surface.
            connectors_enabled: String::new(),
        })
        .expect("upsert bot");
    }

    /// `set_daemon_token` followed by `get_daemon_token`
    /// returns the same value. Confirms the
    /// `daemon_tokens` table is wired up correctly in
    /// `migrate()`.
    #[test]
    fn daemon_token_set_then_get_round_trip() {
        let db = fresh_db();
        insert_minimal_bot(&db, "bot-a");
        insert_minimal_bot(&db, "bot-b");
        // First read: nothing stored yet.
        assert!(db.get_daemon_token("bot-a").unwrap().is_none());
        db.set_daemon_token("bot-a", "tok-1").unwrap();
        assert_eq!(
            db.get_daemon_token("bot-a").unwrap().as_deref(),
            Some("tok-1")
        );
        // A different bot's token is unaffected.
        assert!(db.get_daemon_token("bot-b").unwrap().is_none());
    }

    /// `rotate_daemon_token` produces a 32-byte
    /// hex-encoded string (64 chars) and overwrites
    /// any previous value. Two consecutive rotations
    /// give two distinct tokens.
    #[test]
    fn daemon_token_rotate_produces_64_hex_chars_and_overwrites() {
        let db = fresh_db();
        insert_minimal_bot(&db, "bot-x");
        let first = db.rotate_daemon_token("bot-x").unwrap();
        let second = db.rotate_daemon_token("bot-x").unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(second.len(), 64);
        assert_ne!(first, second);
        // The stored value is the second rotation, not
        // the first.
        assert_eq!(
            db.get_daemon_token("bot-x").unwrap().as_deref(),
            Some(second.as_str())
        );
    }

    /// `touch_daemon_token` updates `last_used` but
    /// leaves the token bytes intact. Confirms the
    /// three-column tuple (bot_id, token, last_used)
    /// is well-formed.
    #[test]
    fn daemon_token_touch_preserves_token_value() {
        let db = fresh_db();
        insert_minimal_bot(&db, "bot-t");
        db.set_daemon_token("bot-t", "preserved-token").unwrap();
        // A no-op insert of a fresh row that already
        // exists; the on-conflict path is exercised.
        db.set_daemon_token("bot-t", "preserved-token").unwrap();
        assert_eq!(
            db.get_daemon_token("bot-t").unwrap().as_deref(),
            Some("preserved-token")
        );
    }

    /// `list_recent_bot_runs` returns the most-recent
    /// runs across **all** bots (no `bot_id` filter).
    /// The `LIMIT` is respected.
    #[test]
    fn list_recent_bot_runs_returns_most_recent_across_bots() {
        let db = fresh_db();
        // Insert two bots.
        let now = chrono::Utc::now();
        for id in ["bot-a", "bot-b"] {
            db.upsert_bot(&crate::bots::Bot {
                id: id.to_string(),
                name: id.to_string(),
                description: String::new(),
                system_prompt: String::new(),
                default_model: "MiniMax-M3".to_string(),
                allowed_tools: vec![],
                icon: String::new(),
                color: String::new(),
                avatar_color: String::new(),
                created_at: now,
                updated_at: now,
                state: crate::bots::BotState::Idle,
                last_active_at: None,
                // v3.2.0 — `computer_use` defaults to "vm".
                computer_use: "vm".to_string(),
                // v3.7.0 (Phase 8) — empty
                // `connectors_enabled`; this test
                // doesn't exercise the connector
                // surface.
                connectors_enabled: String::new(),
            })
            .expect("upsert");
        }
        // Three runs across both bots, ordered by
        // started_at. bot-b-2 is the newest.
        let runs = vec![
            ("run-1", "bot-a", "succeeded", "2026-09-08T10:00:00Z"),
            ("run-2", "bot-b", "succeeded", "2026-09-08T11:00:00Z"),
            ("run-3", "bot-a", "failed", "2026-09-08T12:00:00Z"),
        ];
        for (id, bot_id, status, started_at) in &runs {
            db.upsert_bot_run(&crate::bots::BotRun {
                id: id.to_string(),
                bot_id: bot_id.to_string(),
                conversation_id: "conv".to_string(),
                status: if *status == "succeeded" {
                    crate::bots::BotRunStatus::Succeeded
                } else {
                    crate::bots::BotRunStatus::Failed
                },
                started_at: parse_dt(started_at.to_string()),
                finished_at: None,
                result_summary: String::new(),
                triggered_by: "app".to_string(),
            })
            .expect("upsert run");
        }
        // LIMIT 2 → the two most recent (run-3, run-2).
        let recent = db.list_recent_bot_runs(2).expect("list");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].id, "run-3");
        assert_eq!(recent[1].id, "run-2");
        // No bot_id filter — both rows can come from
        // different bots.
        assert_ne!(recent[0].bot_id, recent[1].bot_id);
    }
}
