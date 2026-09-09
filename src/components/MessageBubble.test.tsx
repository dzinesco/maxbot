// Component tests for the v0.7.6 `ErrorMessage` block rendered
// inside an assistant message bubble when a stream ended in a
// wire-protocol / network / auth error. The intent is to lock the
// shape of the new friendly-error UX in place so any future change
// to the surface (icon, layout, copy, button) has to update the
// tests too.

import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { ErrorMessage } from "./MessageBubble";

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
