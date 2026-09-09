// v2.0 Slice E — tests for the 6-state presence system.
//
// The state-derivation logic is a pure function
// (`deriveAvatarState`) so we can assert the priority
// table directly without rendering the component. The
// rendered test (the pulsing red border) uses
// `@testing-library/react` to mount the component and
// inspect the DOM.

import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import {
  BotAvatar,
  deriveAvatarState,
  isKnownState,
  stateDescription,
} from "./BotAvatar";
import type { Bot } from "../lib/api";

const blankBot: Bot = {
  id: "b1",
  name: "Test Bot",
  description: "",
  system_prompt: "",
  default_model: "MiniMax-M3",
  allowed_tools: [],
  icon: "🤖",
  color: "#7c5cff",
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-01T00:00:00.000Z",
};

describe("BotAvatar — state derivation (pure)", () => {
  it("applies the documented priority: blocked > waiting > working > thinking > done > idle", () => {
    // failed run always wins, even if the persisted state says
    // "idle". This is the entire point of the priority table —
    // the user sees a failure immediately, before the
    // executor catches up.
    expect(
      deriveAvatarState({
        botState: "idle",
        lastRunStatus: "failed",
        computerState: undefined,
      }),
    ).toBe("blocked");
  });

  it("treats computer provisioning as 'working' regardless of persisted state", () => {
    expect(
      deriveAvatarState({
        botState: "done",
        lastRunStatus: "succeeded",
        computerState: "provisioning",
      }),
    ).toBe("working");
  });

  it("treats computer error as 'blocked'", () => {
    expect(
      deriveAvatarState({
        botState: "working",
        lastRunStatus: "running",
        computerState: "error",
      }),
    ).toBe("blocked");
  });

  it("treats a stopped computer with a running bot as 'waiting'", () => {
    expect(
      deriveAvatarState({
        botState: "working",
        lastRunStatus: "running",
        computerState: "stopped",
      }),
    ).toBe("waiting");
  });

  it("treats a running computer as 'working'", () => {
    expect(
      deriveAvatarState({
        botState: "idle",
        lastRunStatus: undefined,
        computerState: "running",
      }),
    ).toBe("working");
  });

  it("honors the persisted state when no live signals override it", () => {
    expect(
      deriveAvatarState({
        botState: "thinking",
        lastRunStatus: undefined,
        computerState: undefined,
      }),
    ).toBe("thinking");
  });

  it("falls back to 'idle' for unknown persisted values", () => {
    expect(
      deriveAvatarState({
        // The function accepts BotState | undefined, but
        // simulate a legacy / corrupted row by passing
        // undefined through a cast. The priority table
        // doesn't crash — it returns idle.
        botState: undefined,
        lastRunStatus: "succeeded",
        computerState: undefined,
      }),
    ).toBe("idle");
  });
});

describe("BotAvatar — rendered visuals", () => {
  it("renders a pulsing red border for the 'blocked' state", () => {
    const { container } = render(
      <BotAvatar bot={blankBot} stateOverride="blocked" />,
    );
    const avatar = container.querySelector(".bot-avatar")!;
    // The `blocked` state applies the modifier class
    // `.bot-avatar--blocked` — the CSS keyframe
    // `bot-avatar-blocked-ring` does the actual pulsing.
    expect(avatar.classList.contains("bot-avatar--blocked")).toBe(
      true,
    );
    expect(avatar.getAttribute("data-bot-state")).toBe("blocked");
  });

  it("renders the thinking badge (three dots) for the 'thinking' state", () => {
    const { container } = render(
      <BotAvatar bot={blankBot} stateOverride="thinking" />,
    );
    const badge = container.querySelector(
      ".bot-avatar__badge--thinking",
    );
    expect(badge).toBeTruthy();
    expect(
      badge!.querySelectorAll(".bot-avatar__dot").length,
    ).toBe(3);
  });

  it("renders a checkmark badge for the 'done' state", () => {
    const { container } = render(
      <BotAvatar bot={blankBot} stateOverride="done" />,
    );
    const badge = container.querySelector(".bot-avatar__badge--done");
    expect(badge).toBeTruthy();
    expect(badge!.textContent).toBe("✓");
  });

  it("renders no badge for the 'idle' state (resting)", () => {
    const { container } = render(
      <BotAvatar bot={blankBot} stateOverride="idle" />,
    );
    expect(
      container.querySelector(".bot-avatar__badge"),
    ).toBeNull();
  });
});

describe("BotAvatar — hover tooltip", () => {
  it("sets the standard 'title' attribute to the state description", () => {
    render(<BotAvatar bot={blankBot} stateOverride="working" />);
    const avatar = screen.getByRole("img");
    // The title attribute carries the tooltip text. We assert
    // the description is non-empty and starts with the
    // state-specific label.
    const title = avatar.getAttribute("title") ?? "";
    expect(title.length).toBeGreaterThan(0);
    expect(title.toLowerCase()).toContain("working");
  });

  it("appends the last action to the tooltip when provided", () => {
    render(
      <BotAvatar
        bot={blankBot}
        stateOverride="working"
        lastAction="Running shell: ls -la"
      />,
    );
    const avatar = screen.getByRole("img");
    const title = avatar.getAttribute("title") ?? "";
    expect(title).toContain("Running shell: ls -la");
  });
});

describe("BotAvatar — helpers", () => {
  it("isKnownState returns true for the six canonical values", () => {
    expect(isKnownState("idle")).toBe(true);
    expect(isKnownState("thinking")).toBe(true);
    expect(isKnownState("working")).toBe(true);
    expect(isKnownState("waiting")).toBe(true);
    expect(isKnownState("blocked")).toBe(true);
    expect(isKnownState("done")).toBe(true);
  });

  it("isKnownState returns false for unknown values", () => {
    expect(isKnownState("running")).toBe(false);
    expect(isKnownState("")).toBe(false);
  });

  it("stateDescription returns a non-empty human-readable string", () => {
    const text = stateDescription("idle");
    expect(text.length).toBeGreaterThan(0);
    expect(text.toLowerCase()).toContain("idle");
  });
});
