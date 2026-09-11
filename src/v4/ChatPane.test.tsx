// v4 — ChatPane tests (S1 + S2).
//
// S1 coverage:
//   - Send wires through sendMessage; chunks append; Done clears pending.
//   - Stop calls stopMessage with the assistant_message_id.
//   - No history polling while a stream is active.
//
// S2 changes:
//   - ChatPane now takes a `conversationId` prop. When null, the
//     pane shows an empty state. When set, it loads that
//     conversation's history. The S1 "auto-load latest /
//     auto-create when none" tests are replaced by the S2
//     contract.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { ChatPane } from "./ChatPane";
import type { Bot, Conversation, Message } from "../lib/api";

const invokeMock = vi.fn();
const listenMock = vi.fn();
const unlistenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: (event: string, handler: (e: { payload: unknown }) => void) => {
    listenMock(event, handler);
    return Promise.resolve(unlistenMock);
  },
}));

const handlers: Record<string, Array<(payload: unknown) => void>> = {};

const now = "2026-09-11T18:00:00+00:00";

const bot: Bot = {
  id: "bot-1",
  name: "Alpha",
  description: "",
  system_prompt: "",
  default_model: "minimax",
  allowed_tools: [],
  icon: "",
  color: "",
  state: "idle",
  last_active_at: now,
  created_at: now,
  updated_at: now,
};

const conv: Conversation = {
  id: "conv-1",
  title: "Alpha chat",
  created_at: now,
  updated_at: now,
  bot_id: "bot-1",
};

const userMsg: Message = {
  id: "msg-user-1",
  conversation_id: "conv-1",
  role: "user",
  content: "hi",
  created_at: now,
  tool_calls: [],
  error_message: null,
};

const assistantMsg: Message = {
  id: "msg-assistant-1",
  conversation_id: "conv-1",
  role: "assistant",
  content: "hello!",
  created_at: now,
  tool_calls: [],
  error_message: null,
};

function fire(event: string, payload: unknown) {
  for (const h of handlers[event] || []) h(payload);
}

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockReset();
  unlistenMock.mockReset();
  for (const k of Object.keys(handlers)) delete handlers[k];
  listenMock.mockImplementation(
    (event: string, handler: (e: { payload: unknown }) => void) => {
      if (!handlers[event]) handlers[event] = [];
      handlers[event].push((p) => handler({ payload: p }));
      return Promise.resolve(unlistenMock);
    },
  );
  // Default mock: listConversations, getMessages, listBots.
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "list_conversations") return Promise.resolve([conv]);
    if (cmd === "get_messages") return Promise.resolve([userMsg, assistantMsg]);
    if (cmd === "list_bots") return Promise.resolve([bot]);
    return Promise.resolve(null);
  });
});

afterEach(() => cleanup());

describe("ChatPane — S2 contract (prop-driven conversation)", () => {
  it("renders an empty state when conversationId is null", async () => {
    render(<ChatPane bot={bot} conversationId={null} />);
    // Does NOT call listConversations or getMessages on mount
    // (S2 contract: history is prop-driven, not auto-loaded).
    await waitFor(() => {
      // listConversations / get_messages should NOT be called.
      const calls = invokeMock.mock.calls.map((c) => c[0]);
      expect(calls).not.toContain("get_messages");
    });
    expect(screen.getByText(/pick a thread/i)).toBeTruthy();
  });

  it("loads messages for the explicit conversationId prop", async () => {
    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("get_messages", {
        conversationId: "conv-1",
      });
    });
    expect(screen.getByText("hi")).toBeTruthy();
    expect(screen.getByText("hello!")).toBeTruthy();
  });
});

describe("ChatPane — S1 send wiring", () => {
  it("Send wires through sendMessage; chunks append to the draft", async () => {
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "list_conversations") return Promise.resolve([conv]);
      if (cmd === "get_messages") return Promise.resolve([userMsg, assistantMsg]);
      if (cmd === "send_message") {
        return Promise.resolve({
          user_message_id: "msg-user-2",
          assistant_message_id: "msg-assistant-2",
          request_id: (args as { requestId: string })?.requestId,
        });
      }
      return Promise.resolve(null);
    });

    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => screen.getByText("hello!"));
    const textarea = screen.getByPlaceholderText(/Message Alpha/i);
    fireEvent.change(textarea, { target: { value: "tell me a story" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "send_message",
        expect.objectContaining({
          conversationId: "conv-1",
          content: "tell me a story",
        }),
      );
    });
    expect(screen.getByText(/writing/i)).toBeTruthy();
    const reqId = (
      invokeMock.mock.calls.find(
        (c) => c[0] === "send_message",
      ) as [string, { requestId: string }]
    )[1].requestId;
    fire("chat://chunk", {
      request_id: reqId,
      assistant_message_id: "msg-assistant-2",
      chunk: { kind: "text", delta: "Once " },
    });
    fire("chat://chunk", {
      request_id: reqId,
      assistant_message_id: "msg-assistant-2",
      chunk: { kind: "text", delta: "upon a time" },
    });
    await waitFor(() => {
      expect(screen.getByText(/Once upon a time/)).toBeTruthy();
    });
    fire("chat://done", {
      request_id: reqId,
      assistant_message_id: "msg-assistant-2",
      finish_reason: "stop",
    });
    await waitFor(() => {
      // Draft's pending cleared.
      expect(screen.queryByText(/writing/i) === null).toBe(true);
    });
  });

  it("Stop calls stopMessage with the assistant_message_id", async () => {
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "list_conversations") return Promise.resolve([conv]);
      if (cmd === "get_messages") return Promise.resolve([userMsg, assistantMsg]);
      if (cmd === "send_message") {
        return Promise.resolve({
          user_message_id: "msg-user-3",
          assistant_message_id: "msg-assistant-3",
          request_id: (args as { requestId: string })?.requestId,
        });
      }
      if (cmd === "stop_message") return Promise.resolve(undefined);
      return Promise.resolve(null);
    });

    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => screen.getByText("hello!"));
    const textarea = screen.getByPlaceholderText(/Message Alpha/i);
    fireEvent.change(textarea, { target: { value: "go" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    const stopBtn = await screen.findByText(/^Stop$/);
    fireEvent.click(stopBtn);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("stop_message", {
        assistantMessageId: "msg-assistant-3",
      });
    });
  });
});

describe("ChatPane — S1 no history polling while a stream is active", () => {
  it("does not call get_messages on the idle tick while a stream is pending", async () => {
    let sendResolve!: (v: unknown) => void;
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "list_conversations") return Promise.resolve([conv]);
      if (cmd === "get_messages") return Promise.resolve([userMsg, assistantMsg]);
      if (cmd === "send_message") {
        return new Promise((res) => {
          sendResolve = res;
        });
      }
      return Promise.resolve(null);
    });

    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => screen.getByText("hello!"));

    const beforeSend = invokeMock.mock.calls.filter(
      (c) => c[0] === "get_messages",
    ).length;
    expect(beforeSend).toBeGreaterThanOrEqual(1);

    const textarea = screen.getByPlaceholderText(/Message Alpha/i);
    fireEvent.change(textarea, { target: { value: "x" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("send_message", expect.anything()),
    );

    // While pending, no new get_messages calls from the idle tick.
    const duringStream = invokeMock.mock.calls.filter(
      (c) => c[0] === "get_messages",
    ).length;
    expect(duringStream).toBe(beforeSend);

    // Resolve sendMessage + fire Done — exactly one post-Done refresh.
    const reqId = (
      invokeMock.mock.calls.find(
        (c) => c[0] === "send_message",
      ) as [string, { requestId: string }]
    )[1].requestId;
    sendResolve({
      user_message_id: "msg-user-4",
      assistant_message_id: "msg-assistant-4",
      request_id: reqId,
    });
    fire("chat://done", {
      request_id: reqId,
      assistant_message_id: "msg-assistant-4",
      finish_reason: "stop",
    });
    const afterDone = invokeMock.mock.calls.filter(
      (c) => c[0] === "get_messages",
    ).length;
    expect(afterDone).toBe(duringStream + 1);
  });
});

describe("ChatPane — S3 tool calls (name + one-line result, no full JSON)", () => {
  it("renders tool name from a tool_call_delta under the draft, no JSON", async () => {
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "get_messages") return Promise.resolve([userMsg, assistantMsg]);
      if (cmd === "send_message") {
        return Promise.resolve({
          user_message_id: "msg-user-tc",
          assistant_message_id: "msg-assistant-tc",
          request_id: (args as { requestId: string })?.requestId,
        });
      }
      return Promise.resolve(null);
    });

    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => screen.getByText("hello!"));
    const textarea = screen.getByPlaceholderText(/Message Alpha/i);
    fireEvent.change(textarea, { target: { value: "go" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    await waitFor(() => screen.getByText(/writing/i));

    const reqId = (
      invokeMock.mock.calls.find(
        (c) => c[0] === "send_message",
      ) as [string, { requestId: string }]
    )[1].requestId;

    // Provider sends the name in the first tool_call_delta.
    fire("chat://chunk", {
      request_id: reqId,
      assistant_message_id: "msg-assistant-tc",
      chunk: {
        kind: "tool_call_delta",
        id: "tc-1",
        name: "web_search",
        arguments_delta: '{"q":"maxbot v4"}',
      },
    });

    // The tool name should appear under the draft. The JSON
    // arguments_delta must NOT appear anywhere in the rendered
    // output (the v4 rule: no tool JSON in React state).
    await waitFor(() => {
      expect(screen.getByText("web_search")).toBeTruthy();
    });
    expect(screen.queryByText(/maxbot v4/)).toBeNull();
    expect(screen.queryByText(/"q"/)).toBeNull();
  });

  it("renders tool calls under an assistant message loaded from history (name + truncated result)", async () => {
    // S3 history shape: assistant with tool_calls, followed by a
    // tool result message. Pairing is positional.
    const persistedToolCalls = [
      { id: "tc-h1", name: "shell_run", arguments: '{"cmd":"date"}' },
    ];
    const assistantWithTools: Message = {
      id: "msg-asst-tools",
      conversation_id: "conv-1",
      role: "assistant",
      content: "Let me check.",
      created_at: now,
      tool_calls: persistedToolCalls,
      error_message: null,
    };
    const longResult = "x".repeat(200);
    const toolResult: Message = {
      id: "msg-tool-result",
      conversation_id: "conv-1",
      role: "tool",
      content: longResult,
      created_at: now,
      tool_calls: [],
      error_message: null,
    };
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_messages")
        return Promise.resolve([
          userMsg,
          assistantWithTools,
          toolResult,
        ]);
      return Promise.resolve(null);
    });

    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => screen.getByText("Let me check."));
    expect(screen.getByText("shell_run")).toBeTruthy();
    // Truncated to 80 chars + ellipsis.
    const resultEls = screen.getAllByText(/^x{80}…$/);
    expect(resultEls.length).toBeGreaterThanOrEqual(1);
  });
});

describe("ChatPane — S3a think-strip", () => {
  it("removes <think>…</think> from a loaded assistant message", async () => {
    const assistantWithThink: Message = {
      id: "msg-asst-think",
      conversation_id: "conv-1",
      role: "assistant",
      content:
        "<think>I should answer the date question.</think>Today is a fine day.",
      created_at: now,
      tool_calls: [],
      error_message: null,
    };
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "get_messages") return Promise.resolve([userMsg, assistantWithThink]);
      return Promise.resolve(null);
    });
    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() =>
      expect(screen.getByText(/Today is a fine day\./)).toBeTruthy(),
    );
    // The think tag and its body must NOT appear in the DOM.
    expect(screen.queryByText(/I should answer/)).toBeNull();
    expect(screen.queryByText(/<think>/)).toBeNull();
  });

  it("strips think tags from the streaming draft text", async () => {
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "get_messages") return Promise.resolve([userMsg, assistantMsg]);
      if (cmd === "send_message") {
        return Promise.resolve({
          user_message_id: "msg-user-think",
          assistant_message_id: "msg-asst-think-live",
          request_id: (args as { requestId: string })?.requestId,
        });
      }
      return Promise.resolve(null);
    });
    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => screen.getByText("hello!"));
    const textarea = screen.getByPlaceholderText(/Message Alpha/i);
    fireEvent.change(textarea, { target: { value: "what day is it?" } });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    await waitFor(() => screen.getByText(/writing/i));

    const reqId = (
      invokeMock.mock.calls.find(
        (c) => c[0] === "send_message",
      ) as [string, { requestId: string }]
    )[1].requestId;
    // Stream a text chunk that contains a <think> block.
    fire("chat://chunk", {
      request_id: reqId,
      assistant_message_id: "msg-asst-think-live",
      chunk: {
        kind: "text",
        delta: "<think>let me check</think>Today is Wednesday.",
      },
    });
    await waitFor(() =>
      expect(screen.getByText(/Today is Wednesday\./)).toBeTruthy(),
    );
    expect(screen.queryByText(/let me check/)).toBeNull();
  });
});

describe("ChatPane — S5 send debounce", () => {
  it("a second send in the same tick does not fire send_message twice", async () => {
    // The S5 brief: when the user mashes Enter or clicks
    // Enter+Send in rapid succession, only ONE send_message
    // IPC lands. The React `busy` state is async; the
    // sendingRef flips synchronously, so the second
    // invocation sees the flag and bails.
    let sendCount = 0;
    let sendResolveFn!: (v: unknown) => void;
    invokeMock.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === "get_messages") return Promise.resolve([userMsg, assistantMsg]);
      if (cmd === "send_message") {
        sendCount++;
        return new Promise((res) => {
          sendResolveFn = res;
        });
      }
      return Promise.resolve(null);
    });

    render(<ChatPane bot={bot} conversationId="conv-1" />);
    await waitFor(() => screen.getByText("hello!"));
    const textarea = screen.getByPlaceholderText(/Message Alpha/i);
    fireEvent.change(textarea, { target: { value: "what day is it" } });

    // Three back-to-back Enter presses in the same tick.
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });
    fireEvent.keyDown(textarea, { key: "Enter", shiftKey: false });

    // Allow the first IPC to land.
    await waitFor(() => expect(sendCount).toBeGreaterThanOrEqual(1));
    // Resolve so the test can exit cleanly.
    const reqId = (
      invokeMock.mock.calls.find(
        (c) => c[0] === "send_message",
      ) as [string, { requestId: string }]
    )[1].requestId;
    sendResolveFn({
      user_message_id: "msg-u",
      assistant_message_id: "msg-a",
      request_id: reqId,
    });
    // Only one send_message despite three Enter presses.
    expect(sendCount).toBe(1);
  });
});
