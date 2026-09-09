// v2.3.0 — Routines: tests for the RoutinesPanel.
// Same mock pattern as the other vitest tests:
// `vi.mock("@tauri-apps/api/core")` and stub `invoke`.

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

import { RoutinesPanel } from "./RoutinesPanel";
import type { Bot, BotSchedule, Skill } from "../lib/api";

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

const noopSkill: Skill = {
  id: "skill-1",
  name: "noop-skill",
  description: "A no-op test skill",
  inputs: [],
  steps: [
    { tool: "noop_a", args: {}, output_var: null },
    { tool: "noop_b", args: {}, output_var: null },
    { tool: "noop_c", args: {}, output_var: null },
  ],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

beforeEach(() => {
  invokeMock.mockReset();
  // Default: no routines, one bot, one skill.
  invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "list_bots") return [baseBot];
    if (cmd === "list_all_schedules") return [];
    if (cmd === "skill_list") return [noopSkill];
    if (cmd === "upsert_schedule") {
      const a = args as { schedule: import("../lib/api").BotSchedule };
      return a?.schedule ?? null;
    }
    return null;
  });
  // happy-dom's `window.confirm` exists but always
  // returns false; patch it so the Delete flow
  // actually runs.
  window.confirm = () => true;
});

afterEach(() => {
  cleanup();
});

describe("RoutinesPanel", () => {
  it("shows the 'No routines yet' empty state when listAllSchedules returns []", async () => {
    render(<RoutinesPanel activeBotId={null} onOpenBot={() => {}} />);
    const empty = await screen.findByTestId("routines-empty");
    expect(empty.textContent).toMatch(/No routines yet/i);
  });

  it("lists a bot's existing routine with the correct skill name and step count", async () => {
    // Re-stub the list_all call to return one routine.
    const schedule: BotSchedule = {
      bot_id: "bot-1",
      interval_seconds: 0,
      cron_expression: "0 9 * * 1-5",
      last_run_at: null,
      last_conversation_id: null,
      skill_id: "skill-1",
    };
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_bots") return [baseBot];
      if (cmd === "list_all_schedules") return [schedule];
      if (cmd === "skill_list") return [noopSkill];
      return null;
    });

    render(<RoutinesPanel activeBotId="bot-1" onOpenBot={() => {}} />);
    const rows = await screen.findAllByTestId("routine-row");
    expect(rows.length).toBe(1);
    // Skill name + 3 steps are surfaced in the row.
    expect(rows[0].textContent).toMatch(/noop-skill/);
    expect(rows[0].textContent).toMatch(/3 steps/);
    // Schedule description for "0 9 * * 1-5".
    expect(rows[0].textContent).toMatch(/every weekday at 9 AM/);
    // The active bot gets the highlighted row class.
    expect(rows[0].className).toMatch(/routine-row--active/);
  });

  it("clicking Delete clears the routine", async () => {
    const user = userEvent.setup();
    const schedule: BotSchedule = {
      bot_id: "bot-1",
      interval_seconds: 60,
      cron_expression: "",
      last_run_at: null,
      last_conversation_id: null,
      skill_id: null,
    };
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "list_bots") return [baseBot];
      if (cmd === "list_all_schedules") return [schedule];
      if (cmd === "skill_list") return [noopSkill];
      if (cmd === "upsert_schedule") return null;
      return null;
    });

    render(<RoutinesPanel activeBotId={null} onOpenBot={() => {}} />);
    const row = await screen.findByTestId("routine-row");
    const deleteBtn = screen.getByTestId("routine-delete");
    await user.click(deleteBtn);

    await waitFor(() => {
      const upsertCall = invokeMock.mock.calls.find(
        (c) => c[0] === "upsert_schedule",
      );
      expect(upsertCall).toBeDefined();
    });
    // The upsert call should have written a "disabled" schedule.
    const upsertCall = invokeMock.mock.calls.find(
      (c) => c[0] === "upsert_schedule",
    );
    expect(upsertCall).toBeDefined();
    const args = upsertCall?.[1] as { schedule: BotSchedule };
    expect(args.schedule.interval_seconds).toBe(0);
    expect(args.schedule.cron_expression).toBe("");
    expect(args.schedule.skill_id).toBeNull();
  });
});
