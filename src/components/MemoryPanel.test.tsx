// v2.5.0 — MemoryPanel tests.
//
// We mock `@tauri-apps/api/core` so `invoke` returns canned
// data. The mock pattern matches the existing
// `SkillsPanel.test.tsx` / `ComputerFileBrowser.test.tsx`
// setup: vi.mock the module, then assert the panel makes the
// expected calls and renders the expected text.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import { MemoryPanel } from "./MemoryPanel";
import type { MemEntry } from "../lib/api";

const sampleFact: MemEntry = {
  kind: "fact",
  key: "user_name",
  content: "Tyler",
  created_at: "2026-01-01T00:00:00Z",
};
const samplePref: MemEntry = {
  kind: "preference",
  key: "timezone",
  content: "America/Denver",
  created_at: "2026-01-02T00:00:00Z",
};
const sampleHistory: MemEntry = {
  kind: "history",
  key: "",
  content: "user: hi  |  bot: hello!",
  created_at: "2026-01-03T00:00:00Z",
};

beforeEach(() => {
  invokeMock.mockReset();
  // Default: each list call returns its sample row. The
  // panel calls memory_list three times in parallel on
  // mount — once per kind.
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "memory_list") return [sampleFact];
    return [];
  });
});

afterEach(() => {
  cleanup();
});

describe("MemoryPanel", () => {
  it("renders facts, preferences, and history in three sections", async () => {
    // Per-kind mock: each list call returns the matching
    // sample. The Promise.all in the panel sends all three
    // in one tick.
    invokeMock.mockImplementation(async (cmd: string, args?: { kind?: string }) => {
      if (cmd === "memory_list") {
        if (args?.kind === "fact") return [sampleFact];
        if (args?.kind === "preference") return [samplePref];
        if (args?.kind === "history") return [sampleHistory];
      }
      return [];
    });
    render(<MemoryPanel botId="bot-1" />);
    await waitFor(() => {
      expect(screen.getByTestId("memory-col-fact")).toBeInTheDocument();
      expect(screen.getByTestId("memory-col-preference")).toBeInTheDocument();
      expect(screen.getByTestId("memory-col-history")).toBeInTheDocument();
    });
    // Each sample's content is rendered in the appropriate column.
    expect(screen.getByText("Tyler")).toBeInTheDocument();
    expect(screen.getByText("America/Denver")).toBeInTheDocument();
    // The history sample's content uses a `|` separator that
    // testing-library's default text matcher treats as a
    // literal-vs-regex ambiguity; regex matchers dodge the
    // pipe entirely.
    expect(screen.getByText(/user: hi/)).toBeInTheDocument();
    expect(screen.getByText(/bot: hello/)).toBeInTheDocument();
  });

  it("calls memory_remember with the right args when Add fact is submitted", async () => {
    const user = userEvent.setup();
    let rememberArgs: Record<string, unknown> | undefined;
    invokeMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "memory_remember") {
        rememberArgs = args;
        return {
          kind: args?.kind,
          key: args?.key,
          content: args?.content,
          created_at: "2026-01-04T00:00:00Z",
        };
      }
      if (cmd === "memory_list") return [];
      return null;
    });

    render(<MemoryPanel botId="bot-1" />);
    await waitFor(() => {
      expect(screen.getByTestId("memory-form-fact")).toBeInTheDocument();
    });

    // Fill the fact form.
    const keyInput = screen.getByLabelText("fact key");
    const contentInput = screen.getByLabelText("fact content");
    await user.type(keyInput, "user_name");
    await user.type(contentInput, "Tyler");

    // Submit.
    const submit = screen.getByTestId("memory-add-fact");
    await user.click(submit);

    await waitFor(() => {
      expect(rememberArgs).toBeTruthy();
      expect(rememberArgs?.botId).toBe("bot-1");
      expect(rememberArgs?.kind).toBe("fact");
      expect(rememberArgs?.key).toBe("user_name");
      expect(rememberArgs?.content).toBe("Tyler");
    });
  });

  it("renders an empty state when no entries are present", async () => {
    invokeMock.mockImplementation(async () => []);
    render(<MemoryPanel botId="bot-1" />);
    await waitFor(() => {
      // Each column shows a "no X yet" line.
      expect(screen.getAllByText(/no (facts|preferences|history) yet/i).length).toBeGreaterThanOrEqual(1);
    });
  });
});
