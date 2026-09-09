// Thin Tauri IPC wrapper. The shapes on the Rust side are auto-serialized to
// JSON, so all of these calls round-trip cleanly with the type definitions
// in lib/api.ts.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ActivityFeed,
  Bot,
  BotChunkEvent,
  BotDoneEvent,
  BotErrorEvent,
  BotMessage,
  BotRun,
  BotRunOutput,
  BotSchedule,
  BotState,
  ChunkEvent,
  Computer,
  ComputerStateChangedEvent,
  ControllableApp,
  Conversation,
  DoneEvent,
  ErrorEvent,
  GroupChat,
  GroupMessage,
  McpServerInfo,
  MemEntry,
  MemKind,
  Message,
  SendMessageResponse,
  Settings,
  Skill,
  SkillRecordStart,
  SkillRecordStop,
  SkillRun,
  TccProbeResult,
  ToolSummary,
  TtsSpeakResponse,
} from "./api";

export async function listConversations(
  // v2.0 Slice E: per-Bot chat scoping. When set, the
  // renderer only shows conversations associated with the
  // given bot. The Rust side currently doesn't filter
  // server-side (it returns the full list and we filter on
  // the client), but the parameter is wired through so a
  // future server-side filter can drop in without touching
  // call sites. `null` / `undefined` = no filter (legacy
  // behavior).
  botId?: string | null,
): Promise<Conversation[]> {
  return invoke<Conversation[]>("list_conversations", { botId });
}

export async function createConversation(
  title?: string,
  botId?: string,
): Promise<Conversation> {
  return invoke<Conversation>("create_conversation", {
    title: title ?? null,
    botId: botId ?? null,
  });
}

export async function deleteConversation(id: string): Promise<void> {
  await invoke("delete_conversation", { id });
}

export async function renameConversation(id: string, title: string): Promise<void> {
  await invoke("rename_conversation", { id, title });
}

export async function getMessages(conversationId: string): Promise<Message[]> {
  return invoke<Message[]>("get_messages", { conversationId });
}

export async function regenerateLast(
  conversationId: string,
): Promise<SendMessageResponse> {
  return invoke<SendMessageResponse>("regenerate_last", { conversationId });
}

export async function searchMessages(
  query: string,
  limit?: number,
): Promise<Message[]> {
  return invoke<Message[]>("search_messages", {
    query,
    limit: limit ?? null,
  });
}

/**
 * v0.7.6 one-time migration: write the post-split shape of a
 * legacy "[error] …" assistant message. Persists `content` and
 * `error_message` so the next reload doesn't have to re-split
 * the legacy suffix. Called by `splitLegacyErrorSuffix` on the
 * first load after upgrade when it finds a legacy error
 * message in the DB.
 */
export async function migrateMessageErrorShape(
  messageId: string,
  content: string,
  errorMessage: string,
): Promise<void> {
  await invoke("migrate_message_error_shape", {
    messageId,
    content,
    errorMessage,
  });
}

export async function getSettings(): Promise<Settings> {
  return invoke<Settings>("get_settings");
}

export async function saveSettings(settings: Settings): Promise<void> {
  await invoke("save_settings", { settings });
}

export async function deleteApiKey(): Promise<void> {
  await invoke("delete_api_key");
}

export async function sendMessage(
  conversationId: string,
  content: string,
  requestId: string,
): Promise<SendMessageResponse> {
  return invoke<SendMessageResponse>("send_message", {
    conversationId,
    content,
    requestId,
  });
}

export async function stopMessage(assistantMessageId: string): Promise<void> {
  await invoke("stop_message", { assistantMessageId });
}

export async function onChunk(
  handler: (event: ChunkEvent) => void,
): Promise<UnlistenFn> {
  return listen<ChunkEvent>("chat://chunk", (e) => handler(e.payload));
}

export async function onDone(
  handler: (event: DoneEvent) => void,
): Promise<UnlistenFn> {
  return listen<DoneEvent>("chat://done", (e) => handler(e.payload));
}

export async function onError(
  handler: (event: ErrorEvent) => void,
): Promise<UnlistenFn> {
  return listen<ErrorEvent>("chat://error", (e) => handler(e.payload));
}

// ---- Bot commands ----

export async function listBots(): Promise<Bot[]> {
  return invoke<Bot[]>("list_bots");
}

export async function getBot(id: string): Promise<Bot | null> {
  return invoke<Bot | null>("get_bot", { id });
}

export async function upsertBot(bot: Bot): Promise<Bot> {
  return invoke<Bot>("upsert_bot", { bot });
}

export async function deleteBot(id: string): Promise<void> {
  await invoke("delete_bot", { id });
}

// v2.0 Slice E: per-Bot presence. The bot executor writes
// `working`/`thinking` at run start, `done` at run end, and
// `blocked` when the run failed / user input is required. The
// renderer can also call this defensively when it sees a
// `bot_run.status` of `failed` in the run summary event before
// the executor catches up. The Rust side rejects unknown
// values with a clear error so a typo doesn't silently write
// garbage.
export async function botSetState(
  botId: string,
  state: BotState,
): Promise<void> {
  await invoke("bot_set_state", { botId, state });
}

// v2.0 Slice E: fetch a Bot with the new presence fields
// populated. Functionally equivalent to `getBot` — kept as a
// separate command so the renderer's roster code can ask for
// the "full presence view" explicitly, and so future
// presence-only fields can be added here without touching
// the existing `getBot` surface.
export async function botGetWithState(
  botId: string,
): Promise<Bot | null> {
  return invoke<Bot | null>("bot_get_with_state", { botId });
}

export async function getBotSchedule(
  botId: string,
): Promise<BotSchedule | null> {
  return invoke<BotSchedule | null>("get_schedule", { botId });
}

export async function upsertBotSchedule(
  schedule: BotSchedule,
): Promise<BotSchedule> {
  return invoke<BotSchedule>("upsert_schedule", { schedule });
}

export async function listAllSchedules(): Promise<BotSchedule[]> {
  return invoke<BotSchedule[]>("list_all_schedules");
}

export async function listBotRuns(
  botId: string,
  limit?: number,
): Promise<BotRun[]> {
  return invoke<BotRun[]>("list_bot_runs", {
    botId,
    limit: limit ?? null,
  });
}

export async function listInbox(
  botId: string,
  includeRead?: boolean,
): Promise<BotMessage[]> {
  return invoke<BotMessage[]>("list_inbox", {
    botId,
    includeRead: includeRead ?? null,
  });
}

export async function markInboxRead(botId: string): Promise<void> {
  await invoke("mark_inbox_read", { botId });
}

export async function listAvailableTools(): Promise<ToolSummary[]> {
  return invoke<ToolSummary[]>("list_available_tools");
}

export async function runBotNow(botId: string): Promise<BotRunOutput> {
  return invoke<BotRunOutput>("run_bot_now", { botId });
}

export async function sendToBot(
  toBotId: string,
  body: string,
  triggerRun: boolean = true,
): Promise<BotMessage> {
  return invoke<BotMessage>("send_to_bot", {
    toBotId,
    body,
    triggerRun,
  });
}

export async function onBotChunk(
  handler: (event: BotChunkEvent) => void,
): Promise<UnlistenFn> {
  return listen<BotChunkEvent>("bot://chunk", (e) => handler(e.payload));
}

export async function onBotDone(
  handler: (event: BotDoneEvent) => void,
): Promise<UnlistenFn> {
  return listen<BotDoneEvent>("bot://done", (e) => handler(e.payload));
}

export async function onBotError(
  handler: (event: BotErrorEvent) => void,
): Promise<UnlistenFn> {
  return listen<BotErrorEvent>("bot://error", (e) => handler(e.payload));
}

// ---- Computer Use / TCC ----

export async function listControllableApps(): Promise<ControllableApp[]> {
  return invoke<ControllableApp[]>("list_controllable_apps");
}

export async function requestTccFor(key: string): Promise<TccProbeResult> {
  return invoke<TccProbeResult>("request_tcc_for", { key });
}

export async function openAutomationSettings(): Promise<void> {
  await invoke("open_automation_settings");
}

// ---- Per-Bot Computer (v2.0 Slice C) ----
//
// These wrappers mirror the Tauri commands registered in
// `src-tauri/src/lib.rs` and implemented in
// `src-tauri/src/commands/computer.rs`. The Rust side is the
// source of truth for shapes (camelCase serializations) — keep
// the `Computer` interface in `./api` in sync if you add fields.

/** Fetch the persisted `Computer` row for a Bot, or null if none. */
export async function computerGet(botId: string): Promise<Computer | null> {
  return invoke<Computer | null>("computer_get", { botId });
}

/** Kick off VM provisioning for a Bot. The Tauri command returns
 * immediately and runs the orchestrator in a background tokio
 * task; the renderer should listen on `computer://state-changed`
 * for the terminal `running` / `error` transition. */
export async function computerProvision(
  botId: string,
  opts: { disk_gb: number; ram_mb: number },
): Promise<void> {
  await invoke("computer_provision", { botId, opts });
}

export async function computerStart(botId: string): Promise<void> {
  await invoke("computer_start", { botId });
}

export async function computerStop(botId: string): Promise<void> {
  await invoke("computer_stop", { botId });
}

export async function computerDestroy(botId: string): Promise<void> {
  await invoke("computer_destroy", { botId });
}

/** Return the `ws://localhost:<port>/` URL noVNC should connect
 * to. The Tauri side spawns a per-Bot SSH-tunneled VNC proxy and
 * returns the local WebSocket URL — never the server-side address
 * (security-critical: the renderer must not see
 * `ws://192.168.0.49:...`). */
export async function computerConsoleUrl(botId: string): Promise<string> {
  return invoke<string>("computer_console_url", { botId });
}

/** Smoke-test the libvirt connection. Returns the number of
 * domains currently defined on the server. Used by the Settings
 * → Computer tab's "Test connection" button.
 *
 * NOTE: returns a number (the Rust side returns `usize`), not a
 * string — the plan called for a string message, but the Rust
 * implementation chose the simpler `count` shape. */
export async function computerTestConnection(): Promise<number> {
  return invoke<number>("computer_test_connection");
}

/** Subscribe to state-change events for any Bot. The handler
 * receives the full payload (bot id + new state + optional
 * error). The Tauri side emits on every start/stop/destroy/provision
 * transition. */
export async function onComputerStateChanged(
  handler: (event: ComputerStateChangedEvent) => void,
): Promise<UnlistenFn> {
  return listen<ComputerStateChangedEvent>(
    "computer://state-changed",
    (e) => handler(e.payload),
  );
}

// ---- MCP ----

export async function listMcpServers(): Promise<McpServerInfo[]> {
  return invoke<McpServerInfo[]>("list_mcp_servers");
}

export async function stopBotRun(runId: string): Promise<boolean> {
  return invoke<boolean>("stop_bot_run", { runId });
}

export async function listActiveBotRuns(): Promise<string[]> {
  return invoke<string[]>("list_active_bot_runs");
}

/**
 * Reveal the bot's folder in Finder. The folder is created on first
 * access, so the user always sees the directory — even for a brand-new
 * bot that's never run. Returns the absolute folder path.
 */
export async function revealBotFolder(botId: string): Promise<string> {
  return invoke<string>("reveal_bot_folder", { botId });
}

// ---- TTS ----

/**
 * Speak `text` aloud using the host's `say` command. When `voice` is
 * omitted, the user's stored `tts_voice` preference is used (falling
 * back to "Samantha" on a fresh install).
 */
export async function ttsSpeak(
  text: string,
  voice?: string,
  rate?: number,
): Promise<TtsSpeakResponse> {
  return invoke<TtsSpeakResponse>("tts_speak", {
    text,
    voice: voice ?? null,
    rate: rate ?? null,
  });
}

/** Stop any in-flight speech. Returns true if a process was killed. */
export async function ttsStop(): Promise<boolean> {
  return invoke<boolean>("tts_stop");
}

/**
 * v2.7.0 — Speech-to-Text. POSTs a recorded audio blob to
 * OpenAI Whisper via the Rust `transcribe_audio` command and
 * returns the transcript string. The MIME type typically comes
 * from `MediaRecorder.mimeType` and is usually `audio/webm`
 * (Opus) on modern Chromium.
 *
 * Errors are returned as plain strings; the caller (VoiceButton)
 * surfaces them in a toast.
 */
export async function transcribeAudio(
  audioBytes: Uint8Array,
  mimeType: string,
): Promise<string> {
  return invoke<string>("transcribe_audio", {
    audioBytes: Array.from(audioBytes),
    mimeType,
  });
}

// ---- Meta key/value (onboarding state, schema version) ----

/**
 * Read a meta value. Returns null if the key is missing — convenient
 * for the frontend's "first run?" check, where `is_onboarded` is
 * absent on a brand-new install.
 */
export async function metaGet(key: string): Promise<string | null> {
  return invoke<string | null>("meta_get", { key });
}

/** Insert or overwrite a meta value. */
export async function metaSet(key: string, value: string): Promise<void> {
  await invoke("meta_set", { key, value });
}

/** List every (key, value) pair in the meta table. For debugging. */
export async function metaList(): Promise<Array<[string, string]>> {
  return invoke<Array<[string, string]>>("meta_list");
}

// ----- v2.0 per-Bot computer (file browser) -----

/** One entry from an SFTP directory listing on a per-Bot VM. */
export interface SftpEntry {
  /** Bare filename (no path prefix). */
  name: string;
  /** True for directories, false for regular files. */
  is_dir: boolean;
  /** Size in bytes. 0 for directories. */
  size: number;
}

/** List a directory on a per-Bot VM via SFTP. */
export async function computerFileList(
  botId: string,
  path: string,
): Promise<SftpEntry[]> {
  return invoke<SftpEntry[]>("computer_file_list", { botId, path });
}

/** Read a text file from a per-Bot VM via SFTP. The Rust side
 * caps the read at 256 KiB; binary files will be returned as
 * lossy UTF-8 (sufficient for `/etc/hosts`, source files, logs). */
export async function computerFileRead(
  botId: string,
  path: string,
): Promise<string> {
  return invoke<string>("computer_file_read", { botId, path });
}

/** Write a text file on a per-Bot VM via SFTP. The Rust side
 * writes atomically (write to `path.tmp`, rename) and creates
 * parent directories if they don't exist. */
export async function computerFileWrite(
  botId: string,
  path: string,
  content: string,
): Promise<void> {
  await invoke("computer_file_write", { botId, path, content });
}

// v2.3.5: install the user's default SSH public key into
// the VM's `authorized_keys` via the QEMU guest agent.
// Idempotent — safe to call multiple times. The
// ComputerPanel shows this as a "Use my default key"
// button next to the Start/Stop row, only when the
// `computer_use_default_ssh_key` setting is off.
export async function computerInstallDefaultKey(
  botId: string,
): Promise<string> {
  return invoke<string>("computer_install_default_key", { botId });
}

// ---- v2.2.0 Skills ----
//
// IPC wrappers for the Rust commands in
// `src-tauri/src/commands/skills.rs`. The names match the
// `#[tauri::command]` functions 1:1. Argument names match
// the Rust parameter names (Tauri auto-serializes to JSON
// using serde — Rust side reads as `skill_id`, JS side
// passes `{ skill_id: "..." }`).

/** List all saved Skills, most-recently-updated first. */
export async function listSkills(): Promise<Skill[]> {
  return invoke<Skill[]>("skill_list");
}

/** Look up a single Skill by id. Returns null if not found. */
export async function getSkill(id: string): Promise<Skill | null> {
  return invoke<Skill | null>("skill_get", { id });
}

/** Persist a Skill (new or updated). The renderer passes a
 *  full `Skill` shape; if `id` is empty the Rust side
 *  generates one. */
export async function createSkill(skill: Skill): Promise<Skill> {
  return invoke<Skill>("skill_create", { skill });
}

/** Delete a Skill by id. CASCADE removes its `skill_runs`. */
export async function deleteSkill(id: string): Promise<void> {
  await invoke("skill_delete", { id });
}

/** Run a Skill against a Bot. Returns the populated run
 *  (with per-step `steps`); the run is also persisted in
 *  the `skill_runs` table for the run-history view. */
export async function runSkill(
  botId: string,
  skillId: string,
  inputs: Record<string, unknown> = {},
): Promise<SkillRun> {
  return invoke<SkillRun>("skill_run", { botId, skillId, inputs });
}

/** Most-recent runs for a Skill. Used by the run-history
 *  panel under each Skill in the UI. */
export async function skillRunHistory(
  skillId: string,
  limit?: number,
): Promise<SkillRun[]> {
  return invoke<SkillRun[]>("skill_run_history", { skillId, limit });
}

/** Get the current durable status of a run. v2.2 doesn't
 *  expose per-step progress here; the renderer keeps
 *  per-step state in-memory for the run it just kicked
 *  off. */
export async function skillRunStatus(runId: string): Promise<SkillRun | null> {
  return invoke<SkillRun | null>("skill_run_status", { runId });
}

/** Start a recording session. The Rust side allocates a
 *  `recording_id`, kicks off a Bot run with that id (so
 *  the executor pushes into the recorder), and returns
 *  both ids. */
export async function skillRecordStart(botId: string): Promise<SkillRecordStart> {
  return invoke<SkillRecordStart>("skill_record_start", { botId });
}

/** Stop a recording session. Drains the recorder's
 *  captured tool calls into a candidate Skill (with empty
 *  name/description) for the UI to fill in before
 *  `createSkill` persists the final version. */
export async function skillRecordStop(recordingId: string): Promise<SkillRecordStop | null> {
  return invoke<SkillRecordStop | null>("skill_record_stop", { recordingId });
}

// ---- v2.4.0 Multi-Bot groups ----
//
// IPC wrappers for `src-tauri/src/commands/groups.rs`.
// The plan: 2-6 Bots collaborate in a single
// conversation. Messages are routed via `@BotName`
// mentions; the Rust executor persists assistant
// replies and `<handoff>` cards into the group's
// transcript. Bot runs stream via the existing
// `bot://chunk` events.

/** List every group, most-recently-updated first. Each
 *  row already includes the flat `member_bot_ids` list
 *  (no follow-up call needed). */
export async function groupList(): Promise<GroupChat[]> {
  return invoke<GroupChat[]>("group_list");
}

/** Look up a single group by id, including its member
 *  list. Returns null if the group was deleted. */
export async function groupGet(groupId: string): Promise<GroupChat | null> {
  return invoke<GroupChat | null>("group_get", { groupId });
}

/** Create a new group. `memberBotIds` is the additional
 *  members beyond the owner (which is implicit in the
 *  `ownerBotId` arg). Total members must be 2-6. */
export async function groupCreate(
  name: string,
  ownerBotId: string,
  memberBotIds: string[],
): Promise<GroupChat> {
  return invoke<GroupChat>("group_create", {
    name,
    ownerBotId,
    memberBotIds,
  });
}

/** Add a Bot to a group. Idempotent. */
export async function groupAddMember(groupId: string, botId: string): Promise<void> {
  await invoke("group_add_member", { groupId, botId });
}

/** Remove a Bot from a group. The owner cannot be
 *  removed. */
export async function groupRemoveMember(groupId: string, botId: string): Promise<void> {
  await invoke("group_remove_member", { groupId, botId });
}

/** Append a user-sent message to a group's transcript.
 *  `mentions` is the list of Bot ids the Composer
 *  resolved from the user's `@BotName` references; the
 *  executor reads it via the persisted row to decide
 *  which Bots to run on the next `groupRunTurn`. */
export async function groupSend(
  groupId: string,
  body: string,
  mentions: string[] = [],
): Promise<GroupMessage> {
  return invoke<GroupMessage>("group_send", {
    groupId,
    body,
    mentions: mentions ?? null,
  });
}

/** Last `limit` messages for a group, oldest-first. A
 *  limit of 0 returns every message; the default is 50. */
export async function groupHistory(
  groupId: string,
  limit?: number,
): Promise<GroupMessage[]> {
  return invoke<GroupMessage[]>("group_history", {
    groupId,
    limit: limit ?? null,
  });
}

/** Run a single Bot in a group for one turn. Streams
 *  via `bot://chunk` events (filtered by
 *  `event.bot_run_id` for the demux). Returns the new
 *  assistant message id. If `handoffFromMessageId` is
 *  set, the executor folds that handoff card's content
 *  into the bot's inbox so the next Bot has the
 *  upstream Bot's notes. */
export async function groupRunTurn(
  groupId: string,
  botId: string,
  handoffFromMessageId?: string,
): Promise<string> {
  return invoke<string>("group_run_turn", {
    groupId,
    botId,
    handoffFromMessageId: handoffFromMessageId ?? null,
  });
}

// ---- v2.5.0 Memory ----
//
// IPC wrappers for `src-tauri/src/commands/memory.rs`. Argument
// names match the Rust parameter names — Tauri auto-serializes
// to JSON, JS side passes `{ botId, kind, key, content }`.

/** Top-5 keyword matches across all 3 kinds (fact, preference,
 *  history). Read failures collapse to an empty array. */
export async function memorySearch(
  botId: string,
  query: string,
): Promise<MemEntry[]> {
  return invoke<MemEntry[]>("memory_search", { botId, query });
}

/** Append a new entry. `kind` is "fact" or "preference"; the
 *  LLM tool and the MemoryPanel both call this with those
 *  two kinds. `history` is auto-written by the executor and
 *  not exposed to the renderer here. */
export async function memoryRemember(
  botId: string,
  kind: MemKind,
  key: string,
  content: string,
): Promise<MemEntry> {
  return invoke<MemEntry>("memory_remember", { botId, kind, key, content });
}

/** Delete every entry of `kind` matching `key`. Returns
 *  whether anything was deleted. */
export async function memoryForget(
  botId: string,
  kind: MemKind,
  key: string,
): Promise<boolean> {
  return invoke<boolean>("memory_forget", { botId, kind, key });
}

/** List all entries of one kind, oldest first. */
export async function memoryList(
  botId: string,
  kind: MemKind,
): Promise<MemEntry[]> {
  return invoke<MemEntry[]>("memory_list", { botId, kind });
}

// ---- v2.6.0 — Approval flows ----
//
// IPC wrappers for `src-tauri/src/commands/approvals.rs`.
// `approval_decide` accepts the new `edited_args`
// payload; the `decision` is a string (one of
// "approved" / "rejected" / "edited") so the wire
// stays a single flat invoke call.

import type {
  Approval,
  ApprovalDecideOutput,
  ApprovalRule,
  Rule,
} from "./api";

/** All pending approvals. If `botId` is set, filtered
 *  to that Bot. */
export async function approvalList(botId?: string): Promise<Approval[]> {
  return invoke<Approval[]>("approval_list", {
    botId: botId ?? null,
  });
}

/** Fetch one approval by id. */
export async function approvalGet(id: string): Promise<Approval | null> {
  return invoke<Approval | null>("approval_get", { id });
}

/** Total pending approvals across all Bots. Drives the
 *  Sidebar's "Approvals" badge. */
export async function approvalPendingCount(): Promise<number> {
  return invoke<number>("approval_pending_count");
}

/** All rules for one Bot (every tool the Bot has an
 *  opinion about). Missing tools default to `auto`
 *  in the queue. */
export async function approvalRuleList(botId: string): Promise<ApprovalRule[]> {
  return invoke<ApprovalRule[]>("approval_rule_list", { botId });
}

/** Upsert a per-Bot per-tool rule. The Rust side
 *  parses "auto" / "ask" / "deny" via `Rule::parse`
 *  and falls back to `auto` for anything else. */
export async function approvalRuleSet(
  botId: string,
  toolName: string,
  rule: Rule,
): Promise<void> {
  await invoke<void>("approval_rule_set", {
    botId,
    toolName,
    rule,
  });
}

/** Decide an approval. `decision` is
 *  `"approved" | "rejected" | "edited"`. `editedArgs`
 *  is required when `decision === "edited"`, ignored
 *  otherwise. On `"approved"` / `"edited"` the
 *  underlying tool runs and its result is written to
 *  `approvals.result_json` before this returns. The
 *  Bot does NOT auto-resume (v2.6.1). */
export async function approvalDecide(
  id: string,
  decision: "approved" | "rejected" | "edited",
  editedArgs?: unknown,
): Promise<ApprovalDecideOutput> {
  return invoke<ApprovalDecideOutput>("approval_decide", {
    id,
    decision,
    editedArgs: editedArgs ?? null,
  });
}

// ---- v2.8.0 — Always-on Daemon (24/7) ----

/** Fetch the per-Bot bearer token used by the
 *  `maxbotd` HTTP server. `null` means the user
 *  hasn't generated one yet — the BotEditor surfaces
 *  this as "(not set)" and a Generate button. */
export async function getDaemonToken(botId: string): Promise<string | null> {
  return invoke<string | null>("get_daemon_token", { botId });
}

/** Generate (or rotate) a fresh per-Bot bearer token,
 *  write it to `daemon_tokens`, and return the new
 *  value for the UI to display + put on the
 *  clipboard. The old token is invalidated
 *  immediately. */
export async function rotateDaemonToken(botId: string): Promise<string> {
  return invoke<string>("rotate_daemon_token", { botId });
}

/** Return the three-section activity bundle for the
 *  Sidebar's ActivityFeed. Cheap: three small
 *  `LIMIT 5` queries against the same SQLite file. */
export async function listRecentActivity(): Promise<ActivityFeed> {
  return invoke<ActivityFeed>("list_recent_activity");
}
