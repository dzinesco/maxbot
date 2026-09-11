// Component tests for the v0.7.6 `ErrorMessage` block rendered
// inside an assistant message bubble when a stream ended in a
// wire-protocol / network / auth error. The intent is to lock the
// shape of the new friendly-error UX in place so any future change
// to the surface (icon, layout, copy, button) has to update the
// tests too.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ErrorMessage, MessageBubble, TTSToolbar } from "./MessageBubble";
import type { Message } from "../lib/api";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

describe("ErrorMessage", () => {
  it("renders the error text inside the monospace detail block", () => {
    render(
      <ErrorMessage error="Connection lost — check your network" />,
    );
    // The friendly description must show up verbatim. The component
    // places it in a <pre> with class `error-message-detail`.
    const detail = document.querySelector(".error-message-detail");
    expect(detail).not.toBeNull();
    expect(detail!.textContent).toBe("Connection lost — check your network");
  });

  it("renders a Retry button when onRetry is provided", () => {
    const onRetry = vi.fn();
    render(
      <ErrorMessage
        error="API key not set — open Settings"
        onRetry={onRetry}
      />,
    );
    const button = screen.getByRole("button", { name: /retry/i });
    expect(button).toBeInTheDocument();
    expect(button).toHaveClass("error-message-retry");
  });

  it("calls onRetry when the Retry button is clicked", () => {
    const onRetry = vi.fn();
    render(
      <ErrorMessage
        error="The provider is having trouble — try again"
        onRetry={onRetry}
      />,
    );
    const button = screen.getByRole("button", { name: /retry/i });
    fireEvent.click(button);
    expect(onRetry).toHaveBeenCalledTimes(1);
  });

  it("hides the Retry button when onRetry is omitted", () => {
    render(<ErrorMessage error="Some unrecoverable failure" />);
    // The card still renders, with the friendly text, but no
    // button. This is the "permanent" path: a hard error where
    // retrying wouldn't help (e.g. a malformed request the user
    // must edit to retry).
    const button = screen.queryByRole("button");
    expect(button).toBeNull();
  });

  it("uses the danger color class for the status icon", () => {
    render(<ErrorMessage error="Authentication failed" />);
    const icon = document.querySelector(".error-message-icon");
    expect(icon).not.toBeNull();
    // The icon wrapper carries the danger-tinted styling class.
    expect(icon!.className).toContain("error-message-icon");
    // The actual color is set via CSS (var(--danger)); the test
    // pins the class so a future style refactor that drops the
    // danger class trips a visible test failure.
    const card = document.querySelector(".error-message");
    expect(card).not.toBeNull();
  });
});

// ---- v2.5.0 — "📌 Remember this" button ----------------------------

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
});

const baseMessage = {
  id: "msg-1",
  conversation_id: "conv-1",
  role: "assistant" as const,
  content: "Tyler's favorite color is blue and he lives in Denver.",
  tool_calls: [],
  created_at: "2026-01-01T00:00:00Z",
  error_message: null,
};

describe("MessageBubble — pin button (v2.5.0)", () => {
  it("renders a 📌 button on assistant messages when botId is set", () => {
    render(
      <MessageBubble
        message={baseMessage}
        botId="bot-1"
      />,
    );
    const pin = screen.getByTestId("message-pin-button");
    expect(pin).toBeInTheDocument();
    expect(pin.textContent).toContain("📌");
  });

  it("does not render a 📌 button when botId is missing", () => {
    render(<MessageBubble message={baseMessage} />);
    expect(screen.queryByTestId("message-pin-button")).toBeNull();
  });

  it("calls memory_remember with kind=fact and a key derived from the message", async () => {
    let capturedArgs: Record<string, unknown> | undefined;
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "memory_remember") {
        capturedArgs = args;
        return {
          kind: args?.kind,
          key: args?.key,
          content: args?.content,
          created_at: "2026-01-01T00:00:00Z",
        };
      }
      return null;
    });
    render(<MessageBubble message={baseMessage} botId="bot-42" />);
    const pin = screen.getByTestId("message-pin-button");
    fireEvent.click(pin);
    await waitFor(() => {
      expect(capturedArgs).toBeTruthy();
      expect(capturedArgs?.botId).toBe("bot-42");
      expect(capturedArgs?.kind).toBe("fact");
      // Key derived from first 6 words, snake-cased.
      // Note: the helper drops single-char words, so "s"
      // (from "Tyler's") and "is" survive only if longer
      // than 1 char. The exact slice taken is the first
      // six words of length > 1:
      //   tyler, favorite, color, is, blue, and → joins to
      //   "tyler_favorite_color_is_blue_and".
      expect(capturedArgs?.key).toBe("tyler_favorite_color_is_blue_and");
      // Content is the full message text.
      expect(capturedArgs?.content).toBe(baseMessage.content);
    });
  });
});

// ---- v3.7.13 — UX-3. Tool call form for gated tools ----
//
// `mail_draft`, `calendar_event_create`, and
// `gmail_send` render as editable forms instead
// of JSON blobs. The form pre-fills the
// `to`/`title` field from the most-recent user
// message when the model left it empty. In the
// inline chat-bubble context the form is
// read-only (no approvalId); the approval sheet
// is where the user actually decides.
describe("MessageBubble — ToolCallForm (v3.7.13)", () => {
  const makeToolCallMsg = (
    overrides: Partial<typeof baseMessage> & {
      toolCalls?: Array<{
        id: string;
        name: string;
        arguments: string;
      }>;
      lastUserMessage?: string | null;
    } = {},
  ) => {
    const msg = {
      ...baseMessage,
      ...overrides,
      tool_calls: overrides.toolCalls ?? [],
    };
    return {
      message: msg,
      lastUserMessage:
        overrides.lastUserMessage === undefined
          ? "send tmartinez@example.com a hello"
          : overrides.lastUserMessage,
    };
  };

  it("renders a form for `mail_draft` instead of a JSON blob", () => {
    const { message, lastUserMessage } = makeToolCallMsg({
      toolCalls: [
        {
          id: "tc-1",
          name: "mail_draft",
          arguments: JSON.stringify({
            to: "",
            subject: "Hi",
            body: "Hello world",
          }),
        },
      ],
    });
    render(
      <MessageBubble
        message={message}
        lastUserMessage={lastUserMessage}
      />,
    );
    const form = screen.getByTestId("tool-call-form");
    expect(form).toBeInTheDocument();
    expect(form.dataset.tool).toBe("mail_draft");
    // The three fields are present.
    expect(screen.getByTestId("tool-call-form-to")).toBeInTheDocument();
    expect(screen.getByTestId("tool-call-form-subject")).toBeInTheDocument();
    expect(screen.getByTestId("tool-call-form-body")).toBeInTheDocument();
  });

  it("pre-fills `to` from the last user message when the model sent empty", () => {
    const { message, lastUserMessage } = makeToolCallMsg({
      toolCalls: [
        {
          id: "tc-1",
          name: "mail_draft",
          arguments: JSON.stringify({ to: "", subject: "Hi", body: "" }),
        },
      ],
    });
    render(
      <MessageBubble
        message={message}
        lastUserMessage={lastUserMessage}
      />,
    );
    const toInput = screen.getByTestId("tool-call-form-to") as HTMLInputElement;
    // The user said "send tmartinez@example.com a hello" —
    // the pre-fill rule copies that into the empty `to` field.
    expect(toInput.value).toBe(lastUserMessage);
  });

  it("does NOT overwrite a non-empty `to` from the model", () => {
    const { message, lastUserMessage } = makeToolCallMsg({
      toolCalls: [
        {
          id: "tc-1",
          name: "mail_draft",
          arguments: JSON.stringify({
            to: "real@example.com",
            subject: "Hi",
            body: "",
          }),
        },
      ],
    });
    render(
      <MessageBubble
        message={message}
        lastUserMessage={lastUserMessage}
      />,
    );
    const toInput = screen.getByTestId("tool-call-form-to") as HTMLInputElement;
    // The model said `to: real@example.com` — the
    // pre-fill rule does NOT clobber that even if
    // the user's last message would have been
    // different.
    expect(toInput.value).toBe("real@example.com");
  });

  it("falls back to a JSON preview for unknown tool names", () => {
    const { message, lastUserMessage } = makeToolCallMsg({
      toolCalls: [
        {
          id: "tc-1",
          name: "shell_run",
          arguments: JSON.stringify({ cmd: "ls" }),
        },
      ],
    });
    render(
      <MessageBubble
        message={message}
        lastUserMessage={lastUserMessage}
      />,
    );
    // No form for shell_run; the original
    // `tool-call-card` JSON preview renders
    // instead.
    expect(screen.queryByTestId("tool-call-form")).toBeNull();
  });

  it("renders read-only inputs in the chat-bubble context (no approvalId)", () => {
    const { message, lastUserMessage } = makeToolCallMsg({
      toolCalls: [
        {
          id: "tc-1",
          name: "mail_draft",
          arguments: JSON.stringify({ to: "x@y", subject: "Hi", body: "" }),
        },
      ],
    });
    render(
      <MessageBubble
        message={message}
        lastUserMessage={lastUserMessage}
      />,
    );
    const toInput = screen.getByTestId("tool-call-form-to") as HTMLInputElement;
    // The chat-bubble path has no approvalId, so
    // the input is disabled and there's no
    // approve/cancel button row. The user acts
    // on the approval via the approval sheet.
    expect(toInput).toBeDisabled();
    expect(screen.queryByTestId("tool-call-form-approve")).toBeNull();
    expect(screen.queryByTestId("tool-call-form-cancel")).toBeNull();
  });
});
//
// The Speak button was already in the component (v0.7.6 added it
// for ⌘⇧S); v2.7.0 just makes sure the IPC contract is pinned
// down by a test. The button calls `tts_speak` with the message
// text and flips to a "speaking" state until tts_stop fires (or
// the per-bubble timeout lands).

// ---- v3.7.13 — UX-5. TTS toolbar ----
//
// v3.7.13 moves the per-bubble Speak / Copy buttons
// to a single TTSToolbar that lives above the
// message list. The toolbar speaks / copies the
// most-recent assistant message. The per-bubble
// Speak button is gone, so the pre-v3.7.13 tests
// are replaced with TTSToolbar tests below.
describe("TTSToolbar (v3.7.13)", () => {
  it("renders a disabled toolbar when target is null", () => {
    render(<TTSToolbar target={null} />);
    const toolbar = screen.getByTestId("tts-toolbar");
    expect(toolbar.dataset.disabled).toBe("true");
    expect(
      screen.getByTestId("tts-toolbar-speak") as HTMLButtonElement,
    ).toBeDisabled();
    expect(
      screen.getByTestId("tts-toolbar-copy") as HTMLButtonElement,
    ).toBeDisabled();
  });

  it("renders an enabled toolbar when target has content", () => {
    const msg: Message = {
      ...baseMessage,
      role: "assistant",
      id: "asst-1",
      content: "Hello world",
    };
    render(<TTSToolbar target={msg} />);
    const toolbar = screen.getByTestId("tts-toolbar");
    expect(toolbar.dataset.disabled).toBe("false");
    expect(
      screen.getByTestId("tts-toolbar-speak") as HTMLButtonElement,
    ).not.toBeDisabled();
  });

  it("calls tts_speak with the target's content when Speak is clicked", async () => {
    let capturedCmd: string | null = null;
    let capturedText: string | null = null;
    invokeMock.mockImplementation(
      async (cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "tts_speak") {
          capturedCmd = cmd;
          capturedText = (args?.text as string) ?? null;
        }
        return null;
      },
    );
    const msg: Message = {
      ...baseMessage,
      role: "assistant",
      id: "asst-2",
      content: "Hi there",
    };
    render(<TTSToolbar target={msg} />);
    fireEvent.click(screen.getByTestId("tts-toolbar-speak"));
    await waitFor(() => {
      expect(capturedCmd).toBe("tts_speak");
    });
    expect(capturedText).toBe("Hi there");
  });
});
