// Component tests for the v2.0 Slice D BotEditor additions:
//
//   1. Specialist template chip click fills the name + system prompt.
//   2. "Provision a computer" checkbox reveals disk + RAM inputs.
//   3. Save flow calls `computerProvision` when the checkbox is
//      checked (with the disk + RAM values from the inputs).
//
// The BotEditor imports `../lib/tauri` for `revealBotFolder` and the
// new `computerProvision` / `computerGet` calls. We mock the tauri
// module to:
//   - return a controlled `Bot` from `upsertBot` (via the parent
//     `onSave` handler we wire in each test).
//   - record the `computerProvision` call so the test can assert
//     on it.
//   - return `null` from `computerGet` so the "no computer" state
//     is the default (we don't care about the existing-computer
//     UX in these three tests).
//
// The BotEditor also uses `confirm()` / `alert()` for some flows
// (e.g. delete); we don't exercise those here.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

// Mock the tauri module. We provide mock fns the tests can assert on.
vi.mock("../lib/tauri", () => ({
  computerGet: vi.fn(() => Promise.resolve(null)),
  computerProvision: vi.fn(() => Promise.resolve()),
  revealBotFolder: vi.fn(() => Promise.resolve("/tmp/bot-folder")),
}));

import { BotEditor } from "./BotEditor";
import { computerProvision } from "../lib/tauri";
import type { Bot, BotSchedule, ToolSummary } from "../lib/api";

const noTools: ToolSummary[] = [];

const blankBot = (overrides: Partial<Bot> = {}): Bot => {
  const now = new Date().toISOString();
  return {
    id: "",
    name: "",
    description: "",
    system_prompt: "",
    default_model: "MiniMax-M3",
    allowed_tools: [],
    icon: "🤖",
    color: "",
    created_at: now,
    updated_at: now,
    ...overrides,
  };
};

const noSchedule: BotSchedule = {
  bot_id: "",
  interval_seconds: 0,
  cron_expression: "",
  last_run_at: null,
  last_conversation_id: null,
};

/** Minimal props the editor needs. The `onSave` is a vitest mock
 * that returns the bot it was handed — the editor awaits this
 * after the user clicks Save, and the saved id is what it
 * forwards to `computerProvision`. */
function renderEditor(
  onSave: (bot: Bot, schedule: BotSchedule) => Promise<Bot> | Bot,
  initial: Bot = blankBot(),
) {
  return render(
    <BotEditor
      initial={initial}
      schedule={noSchedule}
      availableTools={noTools}
      isNew={true}
      onClose={() => {}}
      onSave={onSave}
    />,
  );
}

beforeEach(() => {
  vi.mocked(computerProvision).mockReset();
  vi.mocked(computerProvision).mockResolvedValue(undefined);
});

describe("BotEditor — specialist templates", () => {
  it("clicking a chip fills the name and system prompt", () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "new-id" }));
    renderEditor(onSave);

    // Click the Figma Specialist chip. The chip's data-testid
    // follows the pattern `specialist-chip-<key>`.
    const chip = screen.getByTestId("specialist-chip-figma");
    fireEvent.click(chip);

    // The Name input now has the template's placeholder name.
    const nameInput = screen.getByPlaceholderText(
      "e.g. Figma Bro, Devbot, Spec",
    ) as HTMLInputElement;
    expect(nameInput.value).toBe("Figma Bro");

    // The System Prompt textarea now has the template body.
    const promptArea = screen.getByPlaceholderText(
      /You are a careful, concise analyst/,
    ) as HTMLTextAreaElement;
    expect(promptArea.value).toContain("specialist in Figma");
    expect(promptArea.value).toContain("Hand work back to the human");
  });
});

describe("BotEditor — computer section", () => {
  it("'Provision a computer' checkbox reveals disk + RAM inputs", () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "x" }));
    renderEditor(onSave);

    // The disk and RAM inputs start hidden — they only render
    // when the checkbox is checked.
    expect(screen.queryByTestId("provision-disk-gb")).toBeNull();
    expect(screen.queryByTestId("provision-ram-mb")).toBeNull();

    // Check the box.
    const checkbox = screen.getByTestId(
      "provision-computer-checkbox",
    ) as HTMLInputElement;
    expect(checkbox.checked).toBe(false);
    fireEvent.click(checkbox);

    // The disk + RAM inputs are now visible with the default
    // values (10 GB / 2048 MB).
    const disk = screen.getByTestId("provision-disk-gb") as HTMLInputElement;
    const ram = screen.getByTestId("provision-ram-mb") as HTMLInputElement;
    expect(disk.value).toBe("10");
    expect(ram.value).toBe("2048");
  });
});

describe("BotEditor — save flow", () => {
  it("calls computerProvision with disk+RAM when checkbox is checked", async () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "saved-1" }));
    renderEditor(onSave);

    // Fill in a name (required by the save flow) and check the
    // provision box, then edit the disk + RAM values so we can
    // assert they propagate.
    const nameInput = screen.getByPlaceholderText(
      "e.g. Figma Bro, Devbot, Spec",
    ) as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: "Testbot" } });

    fireEvent.click(screen.getByTestId("provision-computer-checkbox"));

    const disk = screen.getByTestId("provision-disk-gb") as HTMLInputElement;
    const ram = screen.getByTestId("provision-ram-mb") as HTMLInputElement;
    fireEvent.change(disk, { target: { value: "25" } });
    fireEvent.change(ram, { target: { value: "4096" } });

    // Click Save. The BotEditor awaits onSave, then awaits
    // computerProvision when the box is checked.
    fireEvent.click(screen.getByTestId("bot-editor-save"));

    await waitFor(() => {
      expect(computerProvision).toHaveBeenCalledTimes(1);
    });
    // The Rust side takes the saved bot id and the disk/ram
    // values from the editor's local state.
    expect(computerProvision).toHaveBeenCalledWith("saved-1", {
      disk_gb: 25,
      ram_mb: 4096,
    });
  });

  it("does NOT call computerProvision when the checkbox is unchecked", async () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "saved-2" }));
    renderEditor(onSave);

    // Fill in a name only.
    const nameInput = screen.getByPlaceholderText(
      "e.g. Figma Bro, Devbot, Spec",
    ) as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: "Plain" } });

    fireEvent.click(screen.getByTestId("bot-editor-save"));

    // Wait for onSave to have resolved; computerProvision should
    // never fire when the box is unchecked.
    await waitFor(() => {
      expect(onSave).toHaveBeenCalledTimes(1);
    });
    expect(computerProvision).not.toHaveBeenCalled();
  });
});
