// Component tests for the v2.0 Slice C `ComputerPanel`.
//
// The plan calls for three tests:
//   1. Renders the Status icon variant correctly with a running bot.
//   2. Renders the Status icon variant correctly with a stopped bot.
//   3. Calls `computerConsoleUrl` when entering Preview mode
//      (mock the Tauri call).
//
// `ComputerPanel` imports `../lib/tauri` for its Tauri calls.
// happy-dom doesn't ship a Tauri runtime, so `invoke` returns
// a Promise that rejects. We mock the tauri module to:
//   - return a controlled `Computer` from `computerGet`.
//   - record the `computerConsoleUrl` call.
//   - make `listen` return a no-op unlisten fn.
//
// `noVncViewer` is also lazy-imported by the panel, but the
// viewer only mounts once `consoleUrl` resolves. We stub it
// with a tiny placeholder so the preview test doesn't try to
// open a real WebSocket (happy-dom doesn't ship a working
// `WebSocket` constructor that satisfies noVNC's expectations).

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import type { Settings } from "../lib/api";

// Mock the tauri module so the panel sees a controlled
// `Computer` and we can spy on the console-URL call.
vi.mock("../lib/tauri", () => {
  return {
    computerGet: vi.fn(),
    computerConsoleUrl: vi.fn(),
    computerStart: vi.fn(),
    computerStop: vi.fn(),
    computerDestroy: vi.fn(),
    computerFileList: vi.fn(),
    computerFileRead: vi.fn(),
    computerFileWrite: vi.fn(),
    // v2.3.5: the panel reads Settings on mount to decide
    // whether to show the "Use my default key" button, and
    // installs the default key on click. Both need to be
    // in the mock or the existing tests will reject.
    computerInstallDefaultKey: vi.fn(),
    getSettings: vi.fn(),
    saveSettings: vi.fn(),
    onComputerStateChanged: vi.fn(() => Promise.resolve(() => {})),
  };
});

// Stub NoVncViewer so the preview test doesn't try to
// instantiate the RFB class. We just render a div with
// a data attribute the test can assert on.
vi.mock("./noVncViewer", () => ({
  NoVncViewer: ({ wsUrl }: { wsUrl: string }) => (
    <div data-testid="novnc-stub" data-ws-url={wsUrl} />
  ),
}));

// Lazy-import after the mock so the panel picks it up.
import { ComputerPanel } from "./ComputerPanel";
import {
  computerGet,
  computerConsoleUrl,
  computerInstallDefaultKey,
  getSettings,
  saveSettings,
} from "../lib/tauri";

import type { Computer } from "../lib/api";

const runningComputer: Computer = {
  bot_id: "bot-1",
  vm_name: "maxbot-bot-1",
  vm_ip: "192.168.122.50",
  vnc_port: 5900,
  ssh_key_id: "key-1",
  state: "running",
  last_seen_at: new Date().toISOString(),
  created_at: new Date().toISOString(),
};

const stoppedComputer: Computer = {
  bot_id: "bot-2",
  vm_name: "maxbot-bot-2",
  vm_ip: null,
  vnc_port: null,
  ssh_key_id: "key-2",
  state: "stopped",
  last_seen_at: null,
  created_at: new Date().toISOString(),
};

// v2.3.5: a default settings blob. Tests can override
// `computer_use_default_ssh_key` per-case.
const defaultSettings: Settings = {
  provider_kind: "minimax",
  minimax_api_key: null,
  openai_api_key: null,
  anthropic_api_key: null,
  xai_api_key: null,
  default_model: "",
  minimax_base_url: "",
  tts_voice: "",
  grok_build_binary: "",
  grok_build_model: "",
  grok_cwd: "",
  ego_browser_path: "",
  nodejs_path: "",
  openai_base_url: "",
  anthropic_base_url: "",
  xai_base_url: "",
  computer_server_host: "192.168.0.49",
  computer_server_ssh_user: "tyler",
  computer_server_ssh_key_id: "",
  computer_vnc_local_port_range: "5900-5999",
  computer_passphrase: "",
  // Default off in the test fixture so the "Use my
  // default key" button is visible — that's the
  // migration case for existing VMs.
  computer_use_default_ssh_key: false,
  computer_default_disk_gb: 10,
  computer_default_ram_mb: 2048,
  // v2.7.0 — voice mode flag. Tests that don't
  // exercise voice don't care; default to off.
  voice_mode_enabled: false,
  // v3.7.1 — maxbotd URL. Tests that don't
  // exercise shared_* routing don't care; default
  // to the local-only URL.
  maxbotd_url: "http://127.0.0.1:8443",
};

beforeEach(() => {
  vi.mocked(computerGet).mockReset();
  vi.mocked(computerConsoleUrl).mockReset();
  vi.mocked(computerConsoleUrl).mockResolvedValue(
    "ws://localhost:5900/",
  );
  // v2.3.5: provide a default Settings response. Tests
  // can override via `vi.mocked(getSettings).mockResolvedValueOnce(...)`.
  vi.mocked(getSettings).mockReset();
  vi.mocked(getSettings).mockResolvedValue(defaultSettings);
  vi.mocked(saveSettings).mockReset();
  vi.mocked(saveSettings).mockResolvedValue();
  vi.mocked(computerInstallDefaultKey).mockReset();
  vi.mocked(computerInstallDefaultKey).mockResolvedValue(
    '{"return":{"pid":1234}}',
  );
});

describe("ComputerPanel — status mode", () => {
  it("renders a running chip with the running color class", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    render(<ComputerPanel botId="bot-1" mode="status" />);
    const chip = await screen.findByTestId("computer-status-chip");
    // The chip's class list carries the `running` variant on
    // both the outer icon and the inner dot — a regression
    // that drops the class would slip a non-color state
    // through silently otherwise.
    await waitFor(() => {
      expect(chip.className).toContain("computer-panel__status-icon--running");
    });
    expect(chip.querySelector(".computer-panel__status-dot--running")).not.toBeNull();
  });

  it("renders a stopped chip with the stopped color class", async () => {
    vi.mocked(computerGet).mockResolvedValue(stoppedComputer);
    render(<ComputerPanel botId="bot-2" mode="status" />);
    const chip = await screen.findByTestId("computer-status-chip");
    await waitFor(() => {
      expect(chip.className).toContain("computer-panel__status-icon--stopped");
    });
    expect(chip.querySelector(".computer-panel__status-dot--stopped")).not.toBeNull();
    // The running class should NOT be present — the test
    // would still pass if both classes were applied, but
    // the assertion documents the expected single-state
    // behavior.
    expect(chip.className).not.toContain("computer-panel__status-icon--running");
  });
});

describe("ComputerPanel — preview mode", () => {
  it("calls computerConsoleUrl when entering preview mode", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // Wait for the panel to mount and the initial
    // `computerGet` to resolve. The console URL is fetched
    // as a side-effect of the first successful `computerGet`
    // (the panel calls it once it knows the VM is running
    // and has a vnc_port).
    await waitFor(() => {
      expect(computerConsoleUrl).toHaveBeenCalledWith("bot-1");
    });
    // The panel should also have rendered its toolbar /
    // preview chrome. The noVNC stub receives the URL
    // forwarded by the panel — that confirms the URL
    // propagated from Tauri → panel → viewer.
    const panel = await screen.findByTestId("computer-panel");
    expect(panel).toBeInTheDocument();
    expect(panel.className).toContain("computer-panel__preview");
    expect(panel.querySelector('[data-testid="novnc-stub"]')?.getAttribute("data-ws-url"))
      .toBe("ws://localhost:5900/");
  });
});

// v2.3.5: the "Use my default key" toolbar button. Renders
// when the per-Bot key path is still active (the migration
// case for existing VMs) and hidden once the user has
// switched. Clicking it must call `computer_install_default_key`
// then `settings_update` with the flag flipped.
describe("ComputerPanel — install-default-key button", () => {
  it("renders the button when default-key is OFF and the VM is running", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    vi.mocked(getSettings).mockResolvedValue({
      ...defaultSettings,
      computer_use_default_ssh_key: false,
    });
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // Wait for the settings effect to resolve and the
    // button to render.
    const button = await screen.findByTestId("computer-install-default-key");
    expect(button).toBeInTheDocument();
  });

  it("renders the button when default-key is already ON (v2.3.6 bootstrap path)", async () => {
    // v2.3.6: the button is now shown whenever the VM
    // is running, regardless of the setting value. The
    // QGA install is idempotent so re-clicking is safe.
    // This is the bootstrap path for a user with a
    // pre-v2.3.5 VM (per-Bot key in authorized_keys)
    // who upgrades: the migration flips the setting to
    // `true` but the VM still only has the per-Bot key.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    vi.mocked(getSettings).mockResolvedValue({
      ...defaultSettings,
      computer_use_default_ssh_key: true,
    });
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    const button = await screen.findByTestId("computer-install-default-key");
    expect(button).toBeInTheDocument();
  });

  it("clicking the button calls install then saveSettings with the flag flipped", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    vi.mocked(getSettings).mockResolvedValue({
      ...defaultSettings,
      computer_use_default_ssh_key: false,
    });
    const user = (await import("@testing-library/user-event")).default;
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    const button = await screen.findByTestId("computer-install-default-key");
    await user.click(button);
    // 1. The Tauri install command fires.
    await waitFor(() => {
      expect(computerInstallDefaultKey).toHaveBeenCalledWith("bot-1");
    });
    // 2. After the install resolves, the panel flips the
    //    setting via `saveSettings` and the new value
    //    must be `true` for `computer_use_default_ssh_key`.
    await waitFor(() => {
      expect(saveSettings).toHaveBeenCalled();
      const last = vi.mocked(saveSettings).mock.calls.at(-1)![0];
      expect(last.computer_use_default_ssh_key).toBe(true);
    });
    // 3. v2.3.6: the button STAYS visible after a
    //    successful install (the install is idempotent,
    //    and the user might want to re-run to confirm).
    //    The inline confirmation banner shows up
    //    instead.
    const confirm = await screen.findByTestId("computer-install-confirm");
    expect(confirm.textContent).toMatch(/Default key installed/i);
    // The button is still on the toolbar (the user can
    // re-click; the QGA pipeline is idempotent).
    expect(
      screen.queryByTestId("computer-install-default-key"),
    ).not.toBeNull();
  });
});
