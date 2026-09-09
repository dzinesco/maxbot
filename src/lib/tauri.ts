// Thin Tauri IPC wrapper. The shapes on the Rust side are auto-serialized to
// JSON, so all of these calls round-trip cleanly with the type definitions
// in lib/api.ts.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  Bot,
  BotChunkEvent,
  BotDoneEvent,
  BotErrorEvent,
  BotMessage,
  BotRun,
  BotRunOutput,
  BotSchedule,
  ChunkEvent,
  Computer,
  ComputerStateChangedEvent,
  ControllableApp,
  Conversation,
  DoneEvent,
  ErrorEvent,
  McpServerInfo,
  Message,
  SendMessageResponse,
  Settings,
  TccProbeResult,
  ToolSummary,
  TtsSpeakResponse,
} from "./api";

export async function listConversations(): Promise<Conversation[]> {
  return invoke<Conversation[]>("list_conversations");
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
