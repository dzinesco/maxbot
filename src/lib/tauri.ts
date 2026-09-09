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

// ---- MCP ----

export async function listMcpServers(): Promise<McpServerInfo[]> {
  return invoke<McpServerInfo[]>("list_mcp_servers");
}
