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
  bot_id: string | null;
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

// ---- Bot types ----

export interface Bot {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  default_model: string;
  allowed_tools: string[];
  icon: string;
  color: string;
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

// ---- MCP ----

export interface McpServerInfo {
  name: string;
  tool_count: number;
  tool_names: string[];
}
