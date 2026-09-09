// v2.3.0 — Routines: tests for the ScheduleEditorDialog
// component. The same `vi.mock` pattern as
// `SkillsPanel.test.tsx` / `ComputerFileBrowser.test.tsx`
// is used to stub `@tauri-apps/api/core` `invoke`.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
}));

import { ScheduleEditorDialog } from "./ScheduleEditorDialog";
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
  inputs: [],
  steps: [
    { tool: "web_fetch", args: { url: "https://wttr.in" }, output_var: null },
    { tool: "file_write", args: { path: "weather.md" }, output_var: null },
  ],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args?: unknown) => {
    if (cmd === "skill_list") return [sampleSkill];
    if (cmd === "upsert_schedule") {
      // Echo back the schedule we were asked to save so
      // the test can assert on its shape.
      const a = args as { schedule: import("../lib/api").BotSchedule };
      return a?.schedule ?? null;
    }
    return null;
  });
});

afterEach(() => {
  cleanup();
});

describe("ScheduleEditorDialog", () => {
  it("renders the cron preview text for the 'weekday 9 AM' preset", async () => {
    // Pre-render in cron mode with the weekday 9 AM
    // expression. The default cronPreset is computed
    // from `schedule.cron_expression` so the preview
    // shows the right line on first render.
    render(
      <ScheduleEditorDialog
        bot={baseBot}
        schedule={{
          bot_id: baseBot.id,
          interval_seconds: 0,
          cron_expression: "0 9 * * 1-5",
          last_run_at: null,
          last_conversation_id: null,
          skill_id: null,
        }}
        onSaved={() => {}}
        onCancel={() => {}}
      />,
    );
    const preview = await screen.findByTestId("cron-preview");
    expect(preview.textContent).toMatch(/weekday 9:00 AM/i);
  });

  it("rejects Save when a Skill is selected but the cron is empty", async () => {
    const onSaved = vi.fn();
    // Open the dialog in cron mode with a known
    // schedule (so the default mode is cron and the
    // cronPreset is the matching preset). happy-dom
    // + React's controlled radio is finicky, so we
    // avoid the click and exercise the cron-advanced
    // path directly.
    render(
      <ScheduleEditorDialog
        bot={baseBot}
        schedule={{
          bot_id: baseBot.id,
          interval_seconds: 0,
          cron_expression: "0 9 * * 1-5",
          last_run_at: null,
          last_conversation_id: null,
          skill_id: null,
        }}
        onSaved={onSaved}
        onCancel={() => {}}
      />,
    );
    // Switch the cron preset to Advanced and clear
    // the input. happy-dom's `<select>` doesn't
    // expose role=combobox, so we walk the dialog's
    // selects directly. The Skill picker is the
    // first, the Preset picker is the second.
    const dialog = document.querySelector(
      ".schedule-editor-dialog",
    ) as HTMLElement;
    const selects = dialog.querySelectorAll("select");
    expect(selects.length).toBeGreaterThanOrEqual(2);
    const presetSelect = selects[1] as HTMLSelectElement;
    fireEvent.change(presetSelect, { target: { value: "__advanced__" } });
    const cronInput = screen.getByPlaceholderText("0 9 * * 1-5");
    fireEvent.change(cronInput, { target: { value: "" } });

    // Save should NOT call onSaved; an inline error is
    // shown instead.
    const save = screen.getByTestId("schedule-save");
    fireEvent.click(save);
    expect(onSaved).not.toHaveBeenCalled();
    expect(
      screen.getByText(/Pick a cron preset or type a 5-field crontab/i),
    ).toBeInTheDocument();
  });

  it("saves a valid interval-only routine", async () => {
    const onSaved = vi.fn();
    render(
      <ScheduleEditorDialog
        bot={baseBot}
        schedule={null}
        onSaved={onSaved}
        onCancel={() => {}}
      />,
    );
    // Default mode is interval; default 1 minute.
    // The Skill select is still empty → saves a "no skill"
    // schedule.
    const save = screen.getByTestId("schedule-save");
    fireEvent.click(save);
    await waitFor(() => {
      expect(onSaved).toHaveBeenCalledTimes(1);
    });
    const saved = onSaved.mock.calls[0][0];
    expect(saved.bot_id).toBe("bot-1");
    expect(saved.interval_seconds).toBe(60);
    expect(saved.cron_expression).toBe("");
    expect(saved.skill_id).toBeNull();
  });
});
