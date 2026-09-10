// Type definitions matching the Rust commands in src-tauri/src/commands/.

export type Role = "system" | "user" | "assistant" | "tool";

export interface PersistedToolCall {
  id: string;
  name: string;
  arguments: string;
}

export interface Message {
  id: string;
  conversation_id: string;
  role: Role;
  content: string;
  tool_calls: PersistedToolCall[];
  created_at: string;
  /**
   * Friendly description of a stream error that ended this assistant
   * turn. When set, the UI renders the `ErrorMessage` block (with
   * Retry) instead of the [error] … text suffix. Null for normal
   * messages, including pre-v0.7.6 messages that survived the
   * one-time migration normalization.
   */
  error_message: string | null;
}

export interface Conversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  bot_id: string | null;
}

export type ProviderKind = "minimax" | "openai" | "anthropic" | "xai";

export interface Settings {
  /** Which LLM provider to use by default. */
  provider_kind: ProviderKind | string;
  /** MiniMax API key. Null = not configured. */
  minimax_api_key: string | null;
  /** OpenAI API key (`sk-…`). Null = not configured. */
  openai_api_key: string | null;
  /** Anthropic API key (`sk-ant-…`). Null = not configured. */
  anthropic_api_key: string | null;
  /** xAI API key (`xai-…`). Null = not configured. */
  xai_api_key: string | null;
  /** Default model id for the active provider. Empty = provider default. */
  default_model: string;
  /** Base URL override for MiniMax. Empty = international endpoint. */
  minimax_base_url: string;
  /** Base URL override for OpenAI. Empty = api.openai.com/v1. */
  openai_base_url: string;
  /** Base URL override for Anthropic. Empty = api.anthropic.com. */
  anthropic_base_url: string;
  /** Base URL override for xAI. Empty = api.x.ai/v1. */
  xai_base_url: string;
  /** Voice name for the `say` command. Empty = Samantha default. */
  tts_voice: string;
  /** Path to the `grok` binary the `grok_prompt` tool spawns. May be a
   * bare name on PATH or an absolute path. Empty = `grok` (auto-detect
   * on $PATH, falling back to `~/.grok/bin/grok`). */
  grok_build_binary: string;
  /** Model alias passed to `grok --model <alias>`. The alias resolves
   * to an API model via `~/.grok/config.toml`. Empty = `minimax`. */
  grok_build_model: string;
  /** Working directory for the `grok agent stdio` subprocess. Empty =
   * the per-app data dir's `grok/` subfolder. Set to an absolute path
   * to point the agent at a specific project. */
  grok_cwd: string;
  /** Optional absolute path to the `ego-browser` CLI. Empty = auto-
   * detect `$HOME/.local/bin/ego-browser`, then `ego-browser` on
   * $PATH. The Rust side currently auto-resolves; this field is
   * surfaced in the Browser tab for a future override. */
  ego_browser_path: string;
  /** Optional override for the `node` binary that the ego (lite) Node
   * subcommand uses. Empty = whatever `ego-browser nodejs` resolves
   * internally. The Rust side currently doesn't read this; surfaced
   * in the Browser tab for a future override. */
  nodejs_path: string;

  // ---- v2.0 Slice D: per-Bot Computer (server) ----
  //
  // Field names mirror the Rust struct in
  // `src-tauri/src/storage/db.rs` exactly. The Rust side reads
  // `computer_vnc_local_port_range` as a single `"lo-hi"` string and
  // parses it (`parse_port_range`); the UI is free to render two
  // separate numeric inputs and serialize them back on save.
  // `computer_default_disk_gb` and `computer_default_ram_mb` are
  // `u32` on the Rust side; the JSON wire shape is just a number.
  /** Hostname or IP of the Linux server hosting per-Bot libvirt
   * VMs. Empty = the Computer feature is disabled and the
   * Settings → Computer tab shows the runbook empty state. */
  computer_server_host: string;
  /** SSH user on the Linux server. Default `tyler`. */
  computer_server_ssh_user: string;
  /** Optional SSH key id (references the `ssh_keys` table). Empty
   * = rely on the OS keychain / ssh-agent. */
  computer_server_ssh_key_id: string;
  /** Local TCP port range the VNC proxy binds to, in `"lo-hi"`
   * form (e.g. `"5900-5999"`). Matches the Rust struct's
   * `String` shape. */
  computer_vnc_local_port_range: string;
  /** User-set passphrase. Used to derive the Argon2id key that
   * encrypts per-Bot SSH keypairs. */
  computer_passphrase: string;
  /**
   * v2.3.5: when true, MaxBot's SSH path leaves `identity_file`
   * empty so ssh falls back to the user's default key
   * (`~/.ssh/id_ed25519` / `~/.ssh/id_rsa` / ssh-agent).
   * Default true — the per-Bot passphrase model was overkill
   * for a personal tool. Legacy users with `false` still get
   * the per-Bot key + passphrase path.
   */
  computer_use_default_ssh_key: boolean;
  /** Default disk size (GiB) for a newly-provisioned Bot VM. */
  computer_default_disk_gb: number;
  /** Default RAM (MiB) for a newly-provisioned Bot VM. */
  computer_default_ram_mb: number;

  // ---- v2.7.0 — Voice (bidirectional) ----
  //
  // When true, the chat UI auto-plays each assistant reply via
  // `tts_speak` and then auto-arms the Composer mic for a
  // follow-up voice turn. The Composer still has a hold-to-record
  // VoiceButton even when this is off — the flag only governs
  // the auto-play + auto-record loop. Default false so a fresh
  // install doesn't talk at the user.
  voice_mode_enabled: boolean;

  // ---- v3.7.1 — maxbotd URL ----
  //
  // Base URL of the always-on `maxbotd` daemon. The Mac app's
  // `shared_write` / `shared_read` / `shared_list` tools
  // route their filesystem ops through `POST /shared` on
  // this URL so the writes land on the daemon's host (the
  // canonical owner of `~/bots/_shared/`) rather than the
  // Mac's local filesystem. Default `http://127.0.0.1:8443`
  // (local-only dev path). Tyler sets this to
  // `http://crispy:8443` to route through the LAN daemon.
  maxbotd_url: string;
}

export interface ProviderPreset {
  /** Stable key, also the Settings.provider_kind value. */
  kind: ProviderKind;
  /** Human-friendly name for the dropdown. */
  display_name: string;
  /** Default model id when the user hasn't picked one. */
  default_model: string;
  /** Built-in default base URL. */
  default_base_url: string;
  /** Hint shown next to the API key field. */
  key_hint: string;
  /** Placeholder text in the API key input. */
  key_placeholder: string;
  /** Short note explaining the provider. */
  description: string;
}

export const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    kind: "minimax",
    display_name: "MiniMax (MiniMax-M3)",
    default_model: "MiniMax-M3",
    default_base_url: "https://api.minimax.io/v1",
    key_hint: "Find it at minimax.io → API Keys.",
    key_placeholder: "eyJ…",
    description: "MiniMax — 1M context, tool use, vision.",
  },
  {
    kind: "openai",
    display_name: "OpenAI (gpt-4o, o1, …)",
    default_model: "gpt-4o",
    default_base_url: "https://api.openai.com/v1",
    key_hint: "platform.openai.com → API keys.",
    key_placeholder: "sk-…",
    description: "OpenAI — gpt-4o, gpt-4o-mini, o1, o1-mini.",
  },
  {
    kind: "anthropic",
    display_name: "Anthropic (Claude 3.5 / 3.7)",
    default_model: "claude-3-5-sonnet-latest",
    default_base_url: "https://api.anthropic.com",
    key_hint: "console.anthropic.com → Settings → API Keys.",
    key_placeholder: "sk-ant-…",
    description: "Anthropic Claude — separate messages API.",
  },
  {
    kind: "xai",
    display_name: "xAI (Grok 2 / Grok 2 Vision)",
    default_model: "grok-2-latest",
    default_base_url: "https://api.x.ai/v1",
    key_hint: "console.x.ai → API Keys.",
    key_placeholder: "xai-…",
    description: "xAI Grok — OpenAI-compatible, fast.",
  },
];

export function presetFor(kind: string): ProviderPreset {
  return (
    PROVIDER_PRESETS.find((p) => p.kind === kind) ?? PROVIDER_PRESETS[0]
  );
}

/** Returns the API key field for a given provider kind. */
export function apiKeyFor(settings: Settings, kind: ProviderKind | string): string | null {
  switch (kind) {
    case "openai":
      return settings.openai_api_key;
    case "anthropic":
      return settings.anthropic_api_key;
    case "xai":
      return settings.xai_api_key;
    case "minimax":
    default:
      return settings.minimax_api_key;
  }
}

/** Returns the base URL override field for a given provider kind. */
export function baseUrlFor(settings: Settings, kind: ProviderKind | string): string {
  switch (kind) {
    case "openai":
      return settings.openai_base_url;
    case "anthropic":
      return settings.anthropic_base_url;
    case "xai":
      return settings.xai_base_url;
    case "minimax":
    default:
      return settings.minimax_base_url;
  }
}

/** True if the active provider has a non-empty API key. */
export function isConfigured(settings: Settings): boolean {
  const k = apiKeyFor(settings, settings.provider_kind);
  return typeof k === "string" && k.length > 0;
}

export const DEFAULT_SETTINGS: Settings = {
  provider_kind: "minimax",
  minimax_api_key: null,
  openai_api_key: null,
  anthropic_api_key: null,
  xai_api_key: null,
  default_model: "MiniMax-M3",
  minimax_base_url: "",
  openai_base_url: "",
  anthropic_base_url: "",
  xai_base_url: "",
  tts_voice: "",
  grok_build_binary: "",
  grok_build_model: "",
  grok_cwd: "",
  ego_browser_path: "",
  nodejs_path: "",
  // v2.0 Slice D: per-Bot Computer defaults. Match the Rust
  // side's `default_computer_disk` / `default_computer_ram`
  // helpers in `db.rs`. The `computer_vnc_local_port_range` is
  // stored as a single `"lo-hi"` string; the UI parses + reserializes
  // it for the two numeric inputs.
  computer_server_host: "",
  computer_server_ssh_user: "tyler",
  computer_server_ssh_key_id: "",
  computer_vnc_local_port_range: "5900-5999",
  computer_passphrase: "",
  // v2.3.5: default-key path is on so new installs skip
  // the passphrase field. Legacy installs round-trip with
  // whatever they had — the field's `#[serde(default)]`
  // makes "missing == true" the safe upgrade.
  computer_use_default_ssh_key: true,
  computer_default_disk_gb: 10,
  computer_default_ram_mb: 2048,
  // v2.7.0 — voice mode off by default. The user opts in
  // via the new "Voice mode" toggle in Settings → General.
  voice_mode_enabled: false,

  // ---- v3.7.1 — maxbotd URL ----
  //
  // Base URL of the always-on `maxbotd` daemon. Default
  // is the local-only `http://127.0.0.1:8443`; set this
  // to `http://crispy:8443` in Settings → General to route
  // shared_* tools through the LAN daemon. See
  // `docs/maxbotd-setup.md` for the full setup flow.
  maxbotd_url: "http://127.0.0.1:8443",
};

export interface TtsSpeakResponse {
  chars: number;
  voice: string;
  truncated_chars: number;
}

export type StreamChunk =
  | { kind: "text"; delta: string }
  | {
      kind: "tool_call_delta";
      id: string;
      name: string | null;
      arguments_delta: string | null;
    }
  | { kind: "done"; finish_reason: string };

export interface SendMessageResponse {
  user_message_id: string;
  assistant_message_id: string;
  request_id: string;
}

export interface ChunkEvent {
  request_id: string;
  assistant_message_id: string;
  chunk: StreamChunk;
}

export interface DoneEvent {
  request_id: string;
  assistant_message_id: string;
  finish_reason: string;
}

export interface ErrorEvent {
  request_id: string;
  assistant_message_id: string;
  message: string;
}

// ---- Bot types ----

/**
 * v2.0 Slice E: the six-state presence system. The renderer
 * derives the effective state at render time from `bot.state`
 * (this column) + the most-recent `bot_run.status` + the
 * `computers.state` row — `deriveAvatarState()` in `BotAvatar`
 * holds the priority logic. The values match the Rust side's
 * `BotState` enum; the renderer falls back to `"idle"` for any
 * unknown value so a future addition doesn't crash the UI.
 */
export type BotState =
  | "idle"
  | "thinking"
  | "working"
  | "waiting"
  | "blocked"
  | "done";

export const BOT_STATES: BotState[] = [
  "idle",
  "thinking",
  "working",
  "waiting",
  "blocked",
  "done",
];

export interface Bot {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  default_model: string;
  allowed_tools: string[];
  icon: string;
  color: string;
  // v2.0 Slice E: per-Bot presence + roster metadata.
  // `avatar_color` is an optional second color slot for the
  // avatar gradient. `last_active_at` powers the "2m ago"
  // timestamps in the sidebar roster. `state` is the
  // persisted presence hint; the renderer still factors in
  // `bot_run.status` and `computers.state` to land the final
  // visual.
  avatar_color?: string;
  last_active_at?: string | null;
  state?: BotState;
  // v3.2.0 — Computer Use target. The Bot editor exposes
  // a dropdown that maps to this field; the value picks
  // which Computer Use tools the Bot sees at run time.
  // "vm" (default) → the per-Bot Linux VM is the
  // Computer Use target (vm_computer_use +
  // vm_browser_open). "mac" → the legacy
  // ego_browser / AppleScript path. "mac-with-approval"
  // → reserved for the v3.4.0 Takeover work; the enum
  // value is in the schema today so a future Bot
  // doesn't need a migration. Optional in the TS type
  // so a v3.1.0 client deserializing a v3.2.0 row
  // doesn't crash — `BotEditor` falls back to "vm"
  // when the field is missing.
  computer_use?: "vm" | "mac" | "mac-with-approval" | string;
  // v3.7.0 (Phase 8) — comma-separated list of
  // enabled connector ids. The renderer turns the
  // list into checkboxes in the BotEditor's
  // Connectors section. Empty = no connectors
  // enabled. Valid ids today: "gmail", "calendar",
  // "github". Stored as a free-form string on the
  // Rust side so adding a new connector doesn't
  // require a schema migration.
  connectors_enabled?: string;
  created_at: string;
  updated_at: string;
}

export type BotRunStatus = "running" | "succeeded" | "failed" | "cancelled";

export interface BotRun {
  id: string;
  bot_id: string;
  conversation_id: string;
  status: BotRunStatus;
  started_at: string;
  finished_at: string | null;
  result_summary: string;
  /** v3.1.0 — which entry point fired this run. `"app"` for
   *  in-app "Run now" / `send_to_bot`, `"daemon"` for the
   *  always-on scheduler, `"webhook"` for a webhook POST.
   *  Defaults to `"app"` for legacy rows. */
  triggered_by?: "app" | "daemon" | "webhook";
}

export interface BotSchedule {
  bot_id: string;
  interval_seconds: number;
  cron_expression: string;
  last_run_at: string | null;
  last_conversation_id: string | null;
  /** v2.3.0 — Routines: when set, the scheduler dispatches
   *  to `run_skill` instead of `run_bot_once` on every fire.
   *  Null = the schedule runs the bot's chat loop (v2.2
   *  behavior). */
  skill_id?: string | null;
}

export interface BotMessage {
  id: string;
  from_bot_id: string;
  to_bot_id: string;
  body: string;
  created_at: string;
  read: boolean;
  conversation_id: string | null;
}

export interface ToolSummary {
  name: string;
  description: string;
  requires_consent: boolean;
}

// ---- v2.6.0 — Approval flows ----
//
// The renderer mirrors the Rust shapes in
// `src-tauri/src/approvals/mod.rs` (Rule enum + the
// ApprovalRule / Approval rows). `Rule` is a string
// union (matches serde's `rename_all = "lowercase"`)
// so the renderer can `as`-cast safely from
// `unknown` invoke results without an extra import.

export type Rule = "auto" | "ask" | "deny";

export interface ApprovalRule {
  bot_id: string;
  tool_name: string;
  rule: Rule;
}

export type ApprovalStatus =
  | "pending"
  | "approved"
  | "rejected"
  | "edited";

export interface Approval {
  id: string;
  bot_id: string;
  tool_name: string;
  status: ApprovalStatus;
  payload: unknown;
  result: unknown | null;
  bot_run_id: string | null;
  /**
   * v2.6.2 — The LLM-issued tool_call id for the
   * gated call. Used by the auto-resume path to
   * match the synthetic `role=tool` message back
   * to the model's outstanding `tool_calls`
   * block. `null` for pre-v2.6.2 rows and for
   * tools that pre-date the id plumbing.
   */
  tool_call_id: string | null;
  created_at: string;
  decided_at: string | null;
  /**
   * v3.4.0 (Phase 5) — "Why this asked" reason.
   * Short, human-readable explanation of why this
   * approval was queued. Populated when the
   * approval is enqueued (not when decided), so a
   * pending row in the queue already carries the
   * reason. Rule-derived by default
   * (e.g. "sending email to client@axis.com —
   * schedule change"); for Takeover approvals the
   * reason is the LLM's self-reported `needs_human`
   * string. `null` for rows enqueued before v3.4.0
   * — the ActivityFeed / ApprovalQueue render the
   * absence as a generic "approval required" copy
   * rather than failing.
   */
  reason?: string | null;
  /**
   * v3.4.0 (Phase 5) — Takeover marker. `true` when
   * this row is a Takeover request rather than a
   * real tool approval. Takeover requests are
   * surfaced in the ApprovalQueue with "Take over"
   * and "Hand back" buttons; on decide the
   * `bot_takeover_state` row is cleared and the
   * Bot's run is resumed.
   *
   * We compute this on the client from
   * `tool_name === "__takeover__"` — a sentinel
   * rather than a separate column to keep the
   * schema minimal. The renderer uses it to swap
   * the action buttons.
   */
  is_takeover?: boolean;
}

/**
 * v3.4.0 (Phase 5) — Per-Bot Takeover state. The
 * `bot_takeover_state` IPC surfaces this so the
 * renderer can show a "Bot is paused waiting for
 * human" badge on the sidebar / BotAvatar even when
 * the app opens after a daemon-driven run that
 * parked the Bot.
 */
export interface BotTakeoverState {
  bot_id: string;
  /** `"running" | "needs_human" | "takeover"` */
  state: "running" | "needs_human" | "takeover" | string;
  approval_id: string;
  reason: string;
  triggering_tool: string;
  created_at: string;
  updated_at: string;
}

/** Returned by `approval_decide` — the canonical
 *  Approval row plus the tool's result string (when
 *  approved / edited). The renderer uses
 *  `tool_result` for the toast and `approval.result`
 *  for the row's "decided" badge. */
export interface ApprovalDecideOutput {
  approval: Approval;
  tool_result: string | null;
}

// v2.8.0 — Always-on Daemon (24/7). The
// `list_recent_activity` Tauri command returns a
// bundle of the three ActivityFeed sections, each
// capped at 5 rows. The fields are camelCase
// already (serde's default), so the wire shape
// matches the JS shape directly.
export interface ActivityFeed {
  bot_runs: BotRun[];
  skill_runs: SkillRun[];
  approvals: Approval[];
}

export interface BotChunkEvent {
  bot_id: string;
  bot_run_id: string;
  conversation_id: string;
  chunk: StreamChunk;
}

export interface BotDoneEvent {
  bot_id: string;
  bot_run_id: string;
  conversation_id: string;
  result_summary: string;
}

export interface BotErrorEvent {
  bot_id: string;
  bot_run_id: string;
  conversation_id: string;
  message: string;
}

export interface BotRunOutput {
  run_id: string;
  conversation_id: string;
  status: BotRunStatus;
  result_summary: string;
}

// ---- v2.5.0 Memory ----
//
// One line of a per-Bot JSONL memory file. Lives on the Bot's
// VM and is exposed to the LLM as tools. `kind` is the JSONL
// line kind — "fact" / "preference" / "history". The Rust side
// uses an enum (MemKind) with the same lowercase names; a
// future addition will be visible here as a free-form string
// so the renderer doesn't break.

export type MemKind = "fact" | "preference" | "history";

export interface MemEntry {
  kind: MemKind | string;
  key: string;
  content: string;
  created_at: string;
}

// ---- v2.4.0 Multi-Bot groups ----

/**
 * A group of 2-6 Bots that collaborate in a single
 * conversation. `member_bot_ids` is the flat list of Bot
 * ids; the renderer resolves them to display names via
 * the existing `Bot[]` list. `owner_bot_id` is the
 * creator; the owner cannot be removed via the UI.
 */
export interface GroupChat {
  chat: {
    id: string;
    name: string;
    owner_bot_id: string;
    created_at: string;
    updated_at: string;
  };
  member_bot_ids: string[];
}

export type GroupRole = "user" | "assistant" | "handoff";

export interface GroupMessage {
  id: string;
  group_id: string;
  /** Speaker Bot id; `null` for user-sent messages. */
  bot_id: string | null;
  role: GroupRole;
  content: string;
  /** Bot ids that were @-mentioned. Renderer uses this
   * to render the "→ @Writer" badge on a row. */
  mentions: string[];
  /** For `role='handoff'` rows, the resolved target Bot
   * id. `null` otherwise. */
  handoff_to: string | null;
  created_at: string;
}

// Helper: build a fresh empty bot (used by the editor's "New bot" path).
export function blankBot(): Bot {
  const now = new Date().toISOString();
  return {
    id: "",
    name: "",
    description: "",
    system_prompt: "",
    default_model: "MiniMax-M3",
    allowed_tools: [],
    icon: "🤖",
    color: "",
    // v2.0 Slice E: roster metadata starts empty / idle.
    avatar_color: "",
    last_active_at: null,
    state: "idle",
    // v3.2.0 — Computer Use target. New Bots default to
    // "vm" so the in-VM path is the out-of-the-box
    // behavior. The Bot editor's dropdown also falls
    // back to "vm" when this field is empty, so an
    // older client that pre-dates the field still
    // lands in a known-good state.
    computer_use: "vm",
    created_at: now,
    updated_at: now,
  };
}

// ---- Computer Use / TCC types ----

export interface ControllableApp {
  key: string;
  display_name: string;
  osa_name: string;
  bundle_id: string;
  description: string;
}

export interface TccProbeResult {
  granted: boolean;
  message: string;
}

// ---- Per-Bot Computer (v2.0 Slice C) ----
//
// Mirrors the Rust struct in `src-tauri/src/computer/mod.rs`.
// `state` is a string (mirroring the `ComputerState` enum) — kept as
// a free-form string here so a future enum addition on the Rust side
// doesn't immediately break the renderer.
export type ComputerState =
  | "provisioning"
  | "running"
  | "stopped"
  | "error";

export interface Computer {
  bot_id: string;
  vm_name: string;
  vm_ip: string | null;
  vnc_port: number | null;
  ssh_key_id: string;
  state: ComputerState | string;
  last_seen_at: string | null;
  created_at: string;
}

/** One entry in a per-Bot VM directory listing. */
export interface SftpEntry {
  name: string;
  is_dir: boolean;
  size: number;
}

/** Payload emitted on the `computer://state-changed` Tauri event. */
export interface ComputerStateChangedEvent {
  bot_id: string;
  /** Best-effort state string from the Rust side. The terminal
   * "destroyed" state is not a `ComputerState` enum variant (the
   * row is gone) but the Rust side still emits it for the
   * renderer's bookkeeping — so the type is a free-form string. */
  state: string;
  error?: string | null;
}

// ---- MCP ----

export interface McpServerInfo {
  name: string;
  tool_count: number;
  tool_names: string[];
}

// ---- v2.2.0 Skills ----
//
// A Skill is a saved, named list of (tool, args) steps the user
// can invoke against a Bot. Shapes mirror the Rust types in
// `src-tauri/src/skills/mod.rs` and the IPC commands in
// `src-tauri/src/commands/skills.rs`. Timestamps are RFC3339
// strings (the Rust side uses `chrono::DateTime<Utc>`).

/** One input parameter the Skill collects from the user. */
export interface SkillInput {
  name: string;
  /** "string" | "number" | "path" | "choice" */
  kind: string;
  default?: string | null;
  choices?: string[] | null;
}

/** One tool call inside a Skill. */
export interface SkillStep {
  tool: string;
  args: Record<string, unknown>;
  /** If set, the tool's output is bound to this name and
   *  later steps' `args` may reference it as `"{{name}}"`. */
  output_var?: string | null;
}

export interface Skill {
  id: string;
  name: string;
  description: string;
  inputs: SkillInput[];
  steps: SkillStep[];
  created_at: string;
  updated_at: string;
}

export type SkillRunStatus = "running" | "succeeded" | "failed" | "cancelled";

/** Per-step progress inside a live Skill run. Only present
 *  on the in-memory `SkillRun` returned by `skill_run`; the
 *  DB-backed run returned by `skill_run_status` has `steps =
 *  []` (per-step detail is in-memory only). */
export interface SkillRunStep {
  tool: string;
  args: Record<string, unknown>;
  status: "running" | "succeeded" | "failed" | "skipped";
  output?: string | null;
  started_at: string;
  finished_at?: string | null;
  output_var?: string | null;
}

export interface SkillRun {
  id: string;
  skill_id: string;
  bot_id: string;
  inputs: Record<string, unknown>;
  status: SkillRunStatus;
  started_at: string;
  finished_at?: string | null;
  result_summary: string;
  steps: SkillRunStep[];
}

/** v3.3.0 — Per-step entry inside a `SkillRunTrace`.
 *  Mirrors the Rust `StepTrace` shape. `role` is one of
 *  `"tool"`, `"substitute"`, `"error"`, `"skipped"`. */
export interface SkillStepTrace {
  role: string;
  content: string;
  tool_name: string;
  tool_args: Record<string, unknown>;
  tool_result: string;
  ts: string;
}

/** v3.3.0 — Per-row trace of a Skill run. One row per run,
 *  with the per-step output as an array. The Skills
 *  panel's "Last run" expandable view calls
 *  `skillRunLastTrace` to populate this. */
export interface SkillRunTrace {
  id: string;
  run_id: string;
  skill_id: string;
  started_at: string;
  duration_ms: number;
  per_step: SkillStepTrace[];
  success: boolean;
  /** The user message or scheduled payload that started
   *  the run. Empty string for runs with no payload. */
  trigger_input: string;
}

/** Returned by `skill_record_start`. */
export interface SkillRecordStart {
  recording_id: string;
  /** Currently always empty — recording dialogs don't
   *  correlate to `bot://done` events in v2.2. The
   *  `recording_id` is the canonical handle. */
  bot_run_id: string;
}

/** Returned by `skill_record_stop`. */
export interface SkillRecordStop {
  skill: Skill;
}
