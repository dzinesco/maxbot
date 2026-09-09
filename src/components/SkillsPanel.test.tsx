// v2.2.0 — SkillsPanel tests.
//
// We mock `@tauri-apps/api/core` so `invoke` returns
// canned data. The mock pattern matches the
// `ComputerFileBrowser.test.tsx` setup (vi.mock
// @tauri-apps/api/core, then assert the panel makes the
// expected calls and renders the expected text).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// Mock the Tauri invoke surface. We need both the core
// `invoke` (used by `lib/tauri.ts`) and the Tauri event
// `listen` (which the SkillsPanel never uses, but
// other tests in the codebase do).
const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

// Import after the mocks so the panel uses the mocked
// `invoke`.
import { SkillsPanel } from "./SkillsPanel";
import type { Bot, Skill } from "../lib/api";

const baseBot: Bot = {
  id: "bot-1",
  name: "Helper",
  description: "",
  system_prompt: "",
  default_model: "MiniMax-M3",
  allowed_tools: [],
  icon: "🤖",
  color: "",
  avatar_color: "",
  last_active_at: null,
  state: "idle",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

const sampleSkill: Skill = {
  id: "skill-1",
  name: "fetch-sf-weather",
  description: "Look up SF weather via wttr.in",
  inputs: [
    { name: "city", kind: "string", default: "San Francisco", choices: null },
  ],
  steps: [
    { tool: "web_fetch", args: { url: "https://wttr.in" }, output_var: null },
  ],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

beforeEach(() => {
  invokeMock.mockReset();
  // Default: skill_list returns a single skill and the
  // history is empty.
  invokeMock.mockImplementation(async (cmd: string) => {
    if (cmd === "skill_list") return [sampleSkill];
    if (cmd === "skill_run_history") return [];
    return null;
  });
});

afterEach(() => {
  cleanup();
});

describe("SkillsPanel", () => {
  it("renders the empty state when there are no skills", async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "skill_list") return [];
      return [];
    });
    render(
      <SkillsPanel bots={[baseBot]} defaultBotId="bot-1" onOpenRecord={() => {}} />,
    );
    await waitFor(() => {
      expect(screen.getByText(/No Skills yet/i)).toBeInTheDocument();
    });
  });

  it("lists a skill and opens the run dialog with its input form", async () => {
    const user = userEvent.setup();
    render(
      <SkillsPanel bots={[baseBot]} defaultBotId="bot-1" onOpenRecord={() => {}} />,
    );
    // Wait for the panel to populate from skill_list.
    await waitFor(() => {
      expect(screen.getByText("fetch-sf-weather")).toBeInTheDocument();
    });
    expect(screen.getByText(/1 step/i)).toBeInTheDocument();

    // Click Run.
    const runButton = screen.getByRole("button", { name: /^Run$/ });
    await user.click(runButton);

    // The dialog should now show the skill's name and a
    // labeled input for `city` (pre-populated with the
    // default).
    await waitFor(() => {
      expect(screen.getByText(/Run: fetch-sf-weather/i)).toBeInTheDocument();
    });
    expect(screen.getByText("city")).toBeInTheDocument();
    // The default value is rendered in an <em> alongside
    // the field label.
    expect(screen.getByText(/default: San Francisco/i)).toBeInTheDocument();
  });

  it("invokes skill_run with the user-edited input values when Run is submitted", async () => {
    const user = userEvent.setup();
    // First call returns the skill; second call (the
    // actual run) returns a finished run summary.
    invokeMock.mockImplementation(async (cmd: string, args?: { skillId?: string; inputs?: Record<string, unknown> }) => {
      if (cmd === "skill_list") return [sampleSkill];
      if (cmd === "skill_run_history") return [];
      if (cmd === "skill_run") {
        return {
          id: "run-1",
          skill_id: args?.skillId ?? "skill-1",
          bot_id: "bot-1",
          inputs: args?.inputs ?? {},
          status: "succeeded",
          started_at: "2026-01-01T00:00:00Z",
          finished_at: "2026-01-01T00:00:01Z",
          result_summary: "1 step(s) succeeded",
          steps: [],
        };
      }
      return null;
    });

    render(
      <SkillsPanel bots={[baseBot]} defaultBotId="bot-1" onOpenRecord={() => {}} />,
    );
    await waitFor(() => {
      expect(screen.getByText("fetch-sf-weather")).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: /^Run$/ }));

    // Edit the city field.
    const cityInput = await screen.findByLabelText(/city/i);
    await user.clear(cityInput);
    await user.type(cityInput, "Oakland");

    // Submit.
    const submit = screen.getAllByRole("button", { name: /^Run$/ });
    // The first Run button is the panel's, the second is
    // the dialog's. Pick the last (dialog's submit).
    await user.click(submit[submit.length - 1]);

    await waitFor(() => {
      const calls = invokeMock.mock.calls;
      const runCall = calls.find((c) => c[0] === "skill_run");
      expect(runCall).toBeTruthy();
      // Arg shape: (cmd, { botId, skillId, inputs })
      const passedArgs = runCall?.[1] as { botId?: string; skillId?: string; inputs?: Record<string, unknown> };
      expect(passedArgs.skillId).toBe("skill-1");
      expect(passedArgs.botId).toBe("bot-1");
      expect(passedArgs.inputs?.city).toBe("Oakland");
    });
  });
});
