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
}

export interface Conversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
}

export interface Settings {
  minimax_api_key: string | null;
  default_model: string;
  base_url: string;
}

export const DEFAULT_SETTINGS: Settings = {
  minimax_api_key: null,
  default_model: "MiniMax-M3",
  base_url: "",
};

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
