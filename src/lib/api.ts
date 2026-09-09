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
  /** Default disk size (GiB) for a newly-provisioned Bot VM. */
  computer_default_disk_gb: number;
  /** Default RAM (MiB) for a newly-provisioned Bot VM. */
  computer_default_ram_mb: number;
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
  computer_default_disk_gb: 10,
  computer_default_ram_mb: 2048,
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
}

export interface BotSchedule {
  bot_id: string;
  interval_seconds: number;
  cron_expression: string;
  last_run_at: string | null;
  last_conversation_id: string | null;
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
