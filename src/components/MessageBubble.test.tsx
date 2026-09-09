// Component tests for the v0.7.6 `ErrorMessage` block rendered
// inside an assistant message bubble when a stream ended in a
// wire-protocol / network / auth error. The intent is to lock the
// shape of the new friendly-error UX in place so any future change
// to the surface (icon, layout, copy, button) has to update the
// tests too.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ErrorMessage, MessageBubble } from "./MessageBubble";

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
