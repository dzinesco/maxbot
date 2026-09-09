// v2.2.0 — RecordSkillDialog tests.
//
// Three flows:
//   1. The configure phase shows a Bot picker.
//   2. Clicking Start calls skill_record_start and
//      transitions to the recording phase.
//   3. Clicking Stop calls skill_record_stop and
//      transitions to the editing phase with the
//      candidate's steps prefilled in a textarea;
//      Save calls skill_create.

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

import { RecordSkillDialog } from "./RecordSkillDialog";
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

const candidateSkill: Skill = {
  id: "candidate-1",
  name: "",
  description: "",
  inputs: [],
  steps: [
    { tool: "web_fetch", args: { url: "https://example.com" }, output_var: null },
  ],
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => {
  cleanup();
});

describe("RecordSkillDialog", () => {
  it("renders the configure phase with a Bot picker", () => {
    render(
      <RecordSkillDialog
        bots={[baseBot]}
        defaultBotId="bot-1"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );
    expect(screen.getByText("Record a Skill")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Start$/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Cancel$/ })).toBeInTheDocument();
  });

  it("transitions through start → recording → editing", async () => {
    const user = userEvent.setup();
    // The dialog's start call returns a recording id;
    // the stop call returns a candidate Skill.
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === "skill_record_start") {
        return { recording_id: "rec-1", bot_run_id: "" };
      }
      if (cmd === "skill_record_stop") {
        return { skill: candidateSkill };
      }
      return null;
    });

    render(
      <RecordSkillDialog
        bots={[baseBot]}
        defaultBotId="bot-1"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );

    // Click Start.
    await user.click(screen.getByRole("button", { name: /^Start$/ }));

    // The dialog should switch to the recording phase.
    await waitFor(() => {
      expect(screen.getByText(/Recording…/)).toBeInTheDocument();
    });

    // Click Stop.
    await user.click(screen.getByRole("button", { name: /^Stop$/ }));

    // The dialog should switch to the editing phase, with
    // the steps prefilled in a textarea.
    await waitFor(() => {
      expect(screen.getByText("Save Skill")).toBeInTheDocument();
    });
    // The editing phase has three text inputs: name,
    // description, and the steps textarea. The steps
    // textarea is the only one with `tagName === TEXTAREA`.
    const all = screen.getAllByRole("textbox");
    const textarea = all.find(
      (el) => (el as HTMLElement).tagName === "TEXTAREA",
    ) as HTMLTextAreaElement | undefined;
    expect(textarea).toBeDefined();
    expect(textarea!.value).toContain("web_fetch");
    expect(textarea!.value).toContain("https://example.com");
  });

  it("calls skill_create with the edited name/description/steps on Save", async () => {
    const user = userEvent.setup();
    const createdRef: { current: Skill | null } = { current: null };
    invokeMock.mockImplementation(async (cmd: string, args?: { skill?: Skill }) => {
      if (cmd === "skill_record_start") {
        return { recording_id: "rec-1", bot_run_id: "" };
      }
      if (cmd === "skill_record_stop") {
        return { skill: candidateSkill };
      }
      if (cmd === "skill_create") {
        createdRef.current = args?.skill ?? null;
        return args?.skill ?? null;
      }
      return null;
    });

    render(
      <RecordSkillDialog
        bots={[baseBot]}
        defaultBotId="bot-1"
        onClose={() => {}}
        onSaved={() => {}}
      />,
    );

    await user.click(screen.getByRole("button", { name: /^Start$/ }));
    await waitFor(() => {
      expect(screen.getByText(/Recording…/)).toBeInTheDocument();
    });
    await user.click(screen.getByRole("button", { name: /^Stop$/ }));
    await waitFor(() => {
      expect(screen.getByText("Save Skill")).toBeInTheDocument();
    });

    // Edit the name. The Name field is the first text
    // input in the editing phase.
    const inputs = screen.getAllByRole("textbox");
    // inputs[0] = name (single-line)
    // inputs[1] = description (single-line)
    // inputs[2] = steps (textarea)
    await user.clear(inputs[0]);
    await user.type(inputs[0], "saved-skill");
    await user.clear(inputs[1]);
    await user.type(inputs[1], "my description");

    await user.click(screen.getByRole("button", { name: /^Save$/ }));

    await waitFor(() => {
      expect(createdRef.current).not.toBeNull();
    });
    const created = createdRef.current as Skill;
    expect(created.name).toBe("saved-skill");
    expect(created.description).toBe("my description");
    expect(created.steps.length).toBe(1);
    expect(created.steps[0].tool).toBe("web_fetch");
  });
});
