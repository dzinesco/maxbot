// Thin Tauri IPC wrapper. The shapes on the Rust side are auto-serialized to
// JSON, so all of these calls round-trip cleanly with the type definitions
// in lib/api.ts.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ChunkEvent,
  Conversation,
  DoneEvent,
  ErrorEvent,
  Message,
  SendMessageResponse,
  Settings,
} from "./api";

export async function listConversations(): Promise<Conversation[]> {
  return invoke<Conversation[]>("list_conversations");
}

export async function createConversation(title?: string): Promise<Conversation> {
  return invoke<Conversation>("create_conversation", { title: title ?? null });
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
