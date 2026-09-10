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
  // v2.6.0 — Approval rules: the editor preloads
  // the rules on mount and writes on radio change.
  // The default mock returns an empty list so the
  // editor falls through to `auto` for every tool.
  approvalRuleList: vi.fn(() => Promise.resolve([])),
  approvalRuleSet: vi.fn(() => Promise.resolve()),
  // v3.4.0 (Phase 5) — Grok Bot defaults. The
  // button calls `applyGrokBotDefaults` then
  // re-fetches the rules via `approvalRuleList`.
  // The default mock resolves; the test below
  // overrides to assert the call shape.
  applyGrokBotDefaults: vi.fn(() => Promise.resolve()),
  // v2.8.0 — Daemon: per-Bot bearer token + webhook
  // URL. Default mocks return `null` (no token yet)
  // and a placeholder settings blob.
  getDaemonToken: vi.fn(() => Promise.resolve(null)),
  rotateDaemonToken: vi.fn(() => Promise.resolve("rotated-token-abcdef")),
  getSettings: vi.fn(() =>
    Promise.resolve({
      provider_kind: "minimax",
      minimax_api_key: null,
      openai_api_key: null,
      anthropic_api_key: null,
      xai_api_key: null,
      default_model: "MiniMax-M3",
      minimax_base_url: "",
      openai_base_url: "",
      anthropic_base_url: "",
      xai_base_url: "",
      minimax_model: "",
      openai_model: "",
      anthropic_model: "",
      xai_model: "",
      computer_server_host: "crispy.local",
      computer_user: "tyler",
      computer_ssh_port: 22,
      computer_passphrase: "",
      libvirt_uri: "",
      tts_voice: "",
      grok_build_binary: "",
      grok_build_model: "",
      grok_cwd: "",
    }),
  ),
}));

import { BotEditor } from "./BotEditor";
import {
  applyGrokBotDefaults,
  approvalRuleSet,
  computerProvision,
  getDaemonToken,
  rotateDaemonToken,
} from "../lib/tauri";
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
  // v2.6.0 — reset the approval rule mocks.
  vi.mocked(approvalRuleSet).mockReset();
  vi.mocked(approvalRuleSet).mockResolvedValue(undefined);
  // v3.4.0 (Phase 5) — reset the Grok Bot defaults
  // mock. The default is a no-op resolver; the
  // test below sets a custom resolver to assert
  // the call shape.
  vi.mocked(applyGrokBotDefaults).mockReset();
  vi.mocked(applyGrokBotDefaults).mockResolvedValue(undefined);
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

// ---- v3.2.0 — Computer Use target dropdown ----
//
// The Bot editor exposes a `Computer Use` dropdown that
// controls which Computer Use tools the Bot sees at run
// time. "VM" is the v3.2.0 default; "Mac" and "Mac with
// approval" are opt-in fallbacks.

describe("BotEditor — Computer Use dropdown (v3.2.0)", () => {
  it("renders a select with VM as the default", () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "x" }));
    // No override — blankBot() defaults to "" (the
    // server-side default is "vm" via the column DEFAULT
    // and serde fallback). The editor's UI falls through
    // to "vm" when the field is empty.
    renderEditor(onSave);
    const select = screen.getByTestId(
      "computer-use-select",
    ) as HTMLSelectElement;
    expect(select.value).toBe("vm");
  });

  it("exposes all three options", () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "x" }));
    renderEditor(onSave);
    const select = screen.getByTestId(
      "computer-use-select",
    ) as HTMLSelectElement;
    const values = Array.from(select.options).map((o) => o.value).sort();
    expect(values).toEqual(["mac", "mac-with-approval", "vm"]);
  });

  it("switching to Mac updates the bot and persists on save", async () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "saved-cu" }));
    renderEditor(onSave);

    const select = screen.getByTestId(
      "computer-use-select",
    ) as HTMLSelectElement;
    fireEvent.change(select, { target: { value: "mac" } });
    expect(select.value).toBe("mac");

    // Fill in a name (required by the save flow) and save.
    const nameInput = screen.getByPlaceholderText(
      "e.g. Figma Bro, Devbot, Spec",
    ) as HTMLInputElement;
    fireEvent.change(nameInput, { target: { value: "MacBot" } });
    fireEvent.click(screen.getByTestId("bot-editor-save"));

    await waitFor(() => {
      expect(onSave).toHaveBeenCalledTimes(1);
    });
    const saved = onSave.mock.calls[0][0];
    expect(saved.computer_use).toBe("mac");
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

// ---- v2.6.0 — Approval Rules section ----
//
// The Rules section is a flat list (one row per
// known tool) with three radios: auto / ask / deny.
// Picking one calls `approval_rule_set` immediately.

describe("BotEditor — Rules section (v2.6.0)", () => {
  // We need at least one tool so the row renders.
  const tools: ToolSummary[] = [
    {
      name: "mail_send",
      description: "Send an email.",
      requires_consent: true,
    },
    {
      name: "file_write",
      description: "Write a file.",
      requires_consent: true,
    },
  ];

  it("shows 3 radio buttons per known tool", () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "new-id" }));
    render(
      <BotEditor
        initial={blankBot()}
        schedule={noSchedule}
        availableTools={tools}
        isNew={true}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    // One row per tool, three radios per row.
    expect(screen.getByTestId("rule-row-mail_send")).toBeTruthy();
    expect(screen.getByTestId("rule-row-file_write")).toBeTruthy();
    expect(
      screen.getByTestId("rule-radio-mail_send-auto"),
    ).toBeTruthy();
    expect(
      screen.getByTestId("rule-radio-mail_send-ask"),
    ).toBeTruthy();
    expect(
      screen.getByTestId("rule-radio-mail_send-deny"),
    ).toBeTruthy();
  });

  it("picking a radio calls approval_rule_set with the new rule", async () => {
    // Bot must have an id for the editor to persist
    // the rule (the same id flows through to the
    // `approval_rule_set` invoke).
    const onSave = vi.fn((bot: Bot) =>
      Promise.resolve({ ...bot, id: "bot-rules" }),
    );
    render(
      <BotEditor
        initial={blankBot({ id: "bot-rules" })}
        schedule={noSchedule}
        availableTools={tools}
        isNew={true}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    fireEvent.click(screen.getByTestId("rule-radio-mail_send-ask"));
    await waitFor(() => {
      expect(approvalRuleSet).toHaveBeenCalledWith(
        "bot-rules",
        "mail_send",
        "ask",
      );
    });
  });

  it("picking deny on file_write persists the deny rule", async () => {
    const onSave = vi.fn((bot: Bot) =>
      Promise.resolve({ ...bot, id: "bot-rules-2" }),
    );
    render(
      <BotEditor
        initial={blankBot({ id: "bot-rules-2" })}
        schedule={noSchedule}
        availableTools={tools}
        isNew={true}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    fireEvent.click(screen.getByTestId("rule-radio-file_write-deny"));
    await waitFor(() => {
      expect(approvalRuleSet).toHaveBeenCalledWith(
        "bot-rules-2",
        "file_write",
        "deny",
      );
    });
  });
});

// v2.8.0 — Always-on Daemon: BotEditor surfaces the
// per-Bot bearer token and the public webhook URL.
// The Daemon section is hidden for new bots (no id
// yet) and shown for existing bots.

describe("BotEditor — Daemon section (v2.8.0)", () => {
  beforeEach(() => {
    vi.mocked(getDaemonToken).mockReset();
    vi.mocked(rotateDaemonToken).mockReset();
    vi.mocked(getDaemonToken).mockResolvedValue(null);
    vi.mocked(rotateDaemonToken).mockResolvedValue("rotated-token-xyz");
  });

  it("hides the Daemon section for new bots (no id yet)", () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "new-id" }));
    renderEditor(onSave, blankBot({ id: "" }));
    expect(screen.queryByTestId("bot-editor-daemon")).toBeNull();
  });

  it("shows '(not set)' when the bot has no daemon token yet", async () => {
    vi.mocked(getDaemonToken).mockResolvedValue(null);
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "bot-d-1" }));
    render(
      <BotEditor
        initial={blankBot({ id: "bot-d-1" })}
        schedule={noSchedule}
        availableTools={noTools}
        isNew={false}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("bot-editor-daemon")).toBeTruthy();
    });
    const tokenInput = screen.getByTestId("daemon-token-value") as HTMLInputElement;
    expect(tokenInput.value).toBe("(not set)");
  });

  it("shows the current token when one is configured", async () => {
    vi.mocked(getDaemonToken).mockResolvedValue("tok-1234-abcdef");
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "bot-d-2" }));
    render(
      <BotEditor
        initial={blankBot({ id: "bot-d-2" })}
        schedule={noSchedule}
        availableTools={noTools}
        isNew={false}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    await waitFor(() => {
      const tokenInput = screen.getByTestId("daemon-token-value") as HTMLInputElement;
      expect(tokenInput.value).toBe("tok-1234-abcdef");
    });
  });

  it("clicking Rotate calls rotateDaemonToken and shows the new token", async () => {
    vi.mocked(getDaemonToken).mockResolvedValue("old-token-aaaa");
    vi.mocked(rotateDaemonToken).mockResolvedValue("fresh-token-bbbb");
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "bot-d-3" }));
    render(
      <BotEditor
        initial={blankBot({ id: "bot-d-3" })}
        schedule={noSchedule}
        availableTools={noTools}
        isNew={false}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    fireEvent.click(screen.getByTestId("daemon-token-rotate"));
    await waitFor(() => {
      expect(rotateDaemonToken).toHaveBeenCalledWith("bot-d-3");
    });
    await waitFor(() => {
      const tokenInput = screen.getByTestId("daemon-token-value") as HTMLInputElement;
      expect(tokenInput.value).toBe("fresh-token-bbbb");
    });
  });

  it("clicking Copy writes the current token to the clipboard", async () => {
    vi.mocked(getDaemonToken).mockResolvedValue("clipboard-tok-7777");
    // jsdom doesn't ship a real clipboard, so we
    // stub `navigator.clipboard.writeText` with a
    // spy and assert on it.
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText },
      configurable: true,
    });
    const onSave = vi.fn((bot: Bot) => Promise.resolve({ ...bot, id: "bot-d-4" }));
    render(
      <BotEditor
        initial={blankBot({ id: "bot-d-4" })}
        schedule={noSchedule}
        availableTools={noTools}
        isNew={false}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    await waitFor(() => {
      const tokenInput = screen.getByTestId("daemon-token-value") as HTMLInputElement;
      expect(tokenInput.value).toBe("clipboard-tok-7777");
    });
    fireEvent.click(screen.getByTestId("daemon-token-copy"));
    await waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("clipboard-tok-7777");
    });
  });

  // v3.4.0 (Phase 5) — "Grok Bot defaults" button.
  // Clicking it on an existing Bot calls
  // `apply_grok_bot_defaults` and re-fetches the
  // rule list. The button is disabled for new Bots
  // (no id yet) because the upsert path already
  // applies the preset on first save — but the
  // helper is still wired for re-clicks.
  it("Grok Bot defaults button calls apply_grok_bot_defaults for an existing Bot", async () => {
    const onSave = vi.fn((bot: Bot) => Promise.resolve(bot));
    render(
      <BotEditor
        initial={blankBot({ id: "bot-existing-1" })}
        schedule={noSchedule}
        availableTools={noTools}
        isNew={false}
        onClose={() => {}}
        onSave={onSave}
      />,
    );
    const btn = await screen.findByTestId("grok-bot-defaults-button");
    expect(btn).toBeTruthy();
    fireEvent.click(btn);
    await waitFor(() => {
      expect(applyGrokBotDefaults).toHaveBeenCalledWith("bot-existing-1");
    });
  });
});
