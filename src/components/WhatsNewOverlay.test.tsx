// v3.0.1 — tests for the "What's new in v3" first-launch overlay.
//
// The overlay is a presentational modal: it owns the 5 bullets,
// the Esc handler, and the Got-it button. The tests assert the
// three acceptance criteria that touch the component itself:
//   (a) all 5 bullets are rendered, verbatim, in order
//   (b) clicking the "Got it" button calls the dismiss handler
//   (c) pressing Esc calls the dismiss handler
//
// The persistence side (setting `seen_v3_intro` in the `meta`
// table, gating on it, surviving a restart) is the parent's
// job and is exercised in App.tsx — the overlay itself only
// guarantees the dismiss callback fires.

import { describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import {
  WhatsNewOverlay,
  WHATS_NEW_V3_BULLETS,
} from "./WhatsNewOverlay";

describe("WhatsNewOverlay — content", () => {
  it("renders the 5 polish bullets in order", () => {
    render(<WhatsNewOverlay onDismiss={() => {}} />);
    const bullets = screen.getAllByTestId("whats-new-bullet");
    expect(bullets.length).toBe(WHATS_NEW_V3_BULLETS.length);
    expect(bullets.length).toBe(5);
    // Same order as the source array — the user reads
    // top-to-bottom, so a shuffled list would jar.
    bullets.forEach((node, idx) => {
      expect(node.textContent).toBe(WHATS_NEW_V3_BULLETS[idx]);
    });
  });

  it("renders the overlay title and a single Got-it button", () => {
    render(<WhatsNewOverlay onDismiss={() => {}} />);
    expect(
      screen.getByRole("heading", { name: /what's new in v3/i }),
    ).toBeTruthy();
    // Exactly one dismiss affordance (the Got-it button).
    // The backdrop's onClick is the no-button dismiss path;
    // it's tested separately below.
    const buttons = screen.getAllByRole("button");
    expect(buttons.length).toBe(1);
    expect(buttons[0].textContent).toBe("Got it");
  });
});

describe("WhatsNewOverlay — dismiss paths", () => {
  it("clicking 'Got it' calls the dismiss handler", () => {
    const onDismiss = vi.fn();
    render(<WhatsNewOverlay onDismiss={onDismiss} />);
    const button = screen.getByTestId("whats-new-got-it");
    fireEvent.click(button);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("pressing Escape calls the dismiss handler", () => {
    const onDismiss = vi.fn();
    render(<WhatsNewOverlay onDismiss={onDismiss} />);
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("pressing a non-Escape key does NOT dismiss", () => {
    const onDismiss = vi.fn();
    render(<WhatsNewOverlay onDismiss={onDismiss} />);
    fireEvent.keyDown(window, { key: "Enter" });
    fireEvent.keyDown(window, { key: " " });
    fireEvent.keyDown(window, { key: "a" });
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("clicking the backdrop calls the dismiss handler", () => {
    const onDismiss = vi.fn();
    render(<WhatsNewOverlay onDismiss={onDismiss} />);
    // The overlay root is the backdrop; clicking it
    // dismisses. Clicking the inner modal does NOT
    // dismiss — that path is exercised implicitly
    // via the modal's stopPropagation handler.
    const overlay = screen.getByTestId("whats-new-overlay");
    fireEvent.click(overlay);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("clicking inside the modal does NOT dismiss (stopPropagation)", () => {
    const onDismiss = vi.fn();
    render(<WhatsNewOverlay onDismiss={onDismiss} />);
    const modal = screen.getByTestId("whats-new-modal");
    fireEvent.click(modal);
    expect(onDismiss).not.toHaveBeenCalled();
  });
});
