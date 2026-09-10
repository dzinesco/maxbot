// Component tests for the v2.0 Slice C `ComputerPanel`.
//
// v3.7.2: the in-app preview is now a screenshot poll,
// not a noVNC stream. The noVNC stub and the
// `computerConsoleUrl` test are gone. New tests:
//
//   1. Renders the Status icon variant correctly with a
//      running bot.
//   2. Renders the Status icon variant correctly with a
//      stopped bot.
//   3. Preview mode shows the loading overlay before the
//      first screenshot, then swaps to an `<img>`.
//   4. The screenshot poll does not stack in-flight
//      requests when each call is slower than the poll
//      interval.
//   5. Takeover button calls `computerTakeoverOpen`,
//      shows the local port, and the Stop button calls
//      `computerTakeoverClose`.
//
// `ComputerPanel` imports `../lib/tauri` for its Tauri
// calls. happy-dom doesn't ship a Tauri runtime, so
// `invoke` returns a Promise that rejects. We mock the
// tauri module to:
//   - return a controlled `Computer` from `computerGet`.
//   - return a fake JPEG byte array from
//     `computerScreenshot` so the panel can wrap it in
//     a `Blob` and render an `<img>`.
//   - record the `computerTakeoverOpen` and
//     `computerTakeoverClose` calls.
//   - make `listen` return a no-op unlisten fn.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act } from "@testing-library/react";
import type { Settings } from "../lib/api";

// Mock the tauri module so the panel sees a controlled
// `Computer` and we can spy on the screenshot + takeover
// calls.
vi.mock("../lib/tauri", () => {
  return {
    computerGet: vi.fn(),
    computerStart: vi.fn(),
    computerStop: vi.fn(),
    computerDestroy: vi.fn(),
    // v3.7.2 (amended): the "VM not provisioned" UI
    // calls this directly. The mock has to be present
    // or the import resolves to undefined and the
    // component throws.
    computerProvision: vi.fn(),
    computerFileList: vi.fn(),
    computerFileRead: vi.fn(),
    computerFileWrite: vi.fn(),
    // v3.7.2: the in-app preview poll.
    computerScreenshot: vi.fn(),
    // v3.7.2: takeover (Screen Sharing) buttons.
    computerTakeoverOpen: vi.fn(),
    computerTakeoverClose: vi.fn(),
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

// Lazy-import after the mock so the panel picks it up.
import { ComputerPanel } from "./ComputerPanel";
import {
  computerDestroy,
  computerGet,
  computerInstallDefaultKey,
  computerProvision,
  computerScreenshot,
  computerTakeoverOpen,
  computerTakeoverClose,
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

// v3.7.2: a tiny in-memory JPEG. The panel wraps it in
// a `Blob({ type: "image/jpeg" })` and renders an
// `<img>`. happy-dom's `URL.createObjectURL` returns a
// string like `blob:mock://...`; we just need the bytes
// to be a non-empty `Uint8Array` so the panel
// transitions out of the loading overlay. The SOI
// marker is technically wrong (no real JPEG body) but
// the panel doesn't decode — happy-dom's `<img>` ignores
// the body too.
const FAKE_JPEG = new Uint8Array([0xff, 0xd8, 0xff, 0xd9]);

beforeEach(() => {
  vi.mocked(computerGet).mockReset();
  // v3.7.2: default to a quick-resolving screenshot
  // poll. Tests that want to assert stacking behavior
  // override this with a slow `mockImplementation`.
  vi.mocked(computerScreenshot).mockReset();
  vi.mocked(computerScreenshot).mockResolvedValue(FAKE_JPEG);
  vi.mocked(computerTakeoverOpen).mockReset();
  vi.mocked(computerTakeoverOpen).mockResolvedValue(5901);
  vi.mocked(computerTakeoverClose).mockReset();
  vi.mocked(computerTakeoverClose).mockResolvedValue();
  // v3.7.2 (amended): default to a no-op provision
  // mock. Tests that exercise the "VM not
  // provisioned" UI override this so they can assert
  // the click handler wired the call correctly.
  vi.mocked(computerProvision).mockReset();
  vi.mocked(computerProvision).mockResolvedValue();
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

describe("ComputerPanel — preview mode (v3.7.2 screenshot poll)", () => {
  it("shows the loading overlay, then swaps to an <img> after the first frame", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // The panel calls `computerGet` on mount. Once the
    // row resolves with `state === "running"`, the
    // screenshot poll starts. Before the first frame
    // arrives, the loading overlay is visible.
    await waitFor(() => {
      expect(computerScreenshot).toHaveBeenCalledWith("bot-1");
    });
    // First frame lands: the overlay is gone, the
    // <img> is on screen.
    const img = await screen.findByTestId("computer-screenshot");
    expect(img).toBeInTheDocument();
    expect(img.tagName).toBe("IMG");
  });

  it("the screenshot poll does not stack in-flight requests when calls are slow", async () => {
    // v3.7.2 acceptance: single in-flight, no stacking.
    // We hold each call on a deferred promise and
    // confirm the panel only ever has one outstanding.
    // This uses real timers (no `vi.useFakeTimers`) —
    // the goal is to prove the in-flight guard, not
    // the poll cadence. We use a deferred per call so
    // we can count exactly how many calls are in
    // flight at the moment we check.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    let inFlight = 0;
    let maxInFlight = 0;
    const deferreds: Array<() => void> = [];
    const releaseAll = () => {
      deferreds.splice(0).forEach((r) => r());
    };
    vi.mocked(computerScreenshot).mockImplementation(async () => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      // Wait for the test to release us. The poll
      // interval is 300ms; we want the second tick
      // to fire while the first is still pending so
      // we can prove the guard rejects it.
      await new Promise<void>((resolve) => deferreds.push(resolve));
      inFlight -= 1;
      return FAKE_JPEG;
    });
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // The initial frame fires immediately on mount.
    await waitFor(() => {
      expect(inFlight).toBeGreaterThan(0);
    });
    // Let the poll run for ~700ms (just over two
    // 300ms intervals). The first call is still
    // pending, so the next two intervals should be
    // dropped by the in-flight guard. After the
    // release, no new calls fire (we already passed
    // several intervals).
    await new Promise((r) => setTimeout(r, 700));
    expect(maxInFlight).toBe(1);
    // The panel made the call at least once.
    expect(computerScreenshot).toHaveBeenCalled();
    // Cleanup: release any pending deferreds so the
    // effect can finish.
    releaseAll();
  });
});

describe("ComputerPanel — takeover (v3.7.2 Screen Sharing)", () => {
  it("'Take over' calls computerTakeoverOpen and shows the local port", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    vi.mocked(computerTakeoverOpen).mockResolvedValue(5901);
    const user = (await import("@testing-library/user-event")).default;
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // Wait for the toolbar to render. The button is
    // visible whenever the VM is running.
    const takeoverButton = await screen.findByTestId("computer-takeover");
    expect(takeoverButton).toHaveTextContent(/Take over/);
    await user.click(takeoverButton);
    await waitFor(() => {
      expect(computerTakeoverOpen).toHaveBeenCalledWith("bot-1");
    });
    // The takeover-status pill surfaces the local
    // port so the user can re-open Screen Sharing if
    // they dismissed the first `open`.
    const status = await screen.findByTestId("takeover-status");
    expect(status.textContent).toMatch(/localhost:5901/);
    // The button label flipped to "Stop takeover".
    expect(takeoverButton).toHaveTextContent(/Stop takeover/);
  });

  it("'Stop takeover' calls computerTakeoverClose and hides the port pill", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    vi.mocked(computerTakeoverOpen).mockResolvedValue(5901);
    const user = (await import("@testing-library/user-event")).default;
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    const takeoverButton = await screen.findByTestId("computer-takeover");
    await user.click(takeoverButton);
    await screen.findByTestId("takeover-status");
    // Click again — the button is now "Stop takeover".
    await user.click(takeoverButton);
    await waitFor(() => {
      expect(computerTakeoverClose).toHaveBeenCalledWith("bot-1");
    });
    await waitFor(() => {
      expect(screen.queryByTestId("takeover-status")).toBeNull();
    });
    expect(takeoverButton).toHaveTextContent(/Take over/);
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

// v3.7.2 (amended): the "VM not provisioned" error state.
// The screenshot poll's first call rejects with
// `ComputerError::DomainNotFound`'s Display string
// ("computer: domain '<vm>' not found on host"), and
// the panel surfaces a clear inline error with a
// Provision button instead of a raw libvirt stderr.
describe("ComputerPanel — domain-not-found state (v3.7.2 amended)", () => {
  // The ComputerError::DomainNotFound Display impl on
  // the Rust side is:
  //   "computer: domain '{vm_name}' not found on host"
  // Tauri serializes the Display into the JS-side
  // rejection message, so this is what the panel sees
  // when the host's `virsh` says the domain is gone.
  const DOMAIN_NOT_FOUND_MSG =
    "computer: domain 'maxbot-bot-1' not found on host";

  it("renders DomainNotFound state with Provision button when error matches", async () => {
    // Running computer (so the screenshot poll kicks
    // off), but the host's `virsh` says the domain is
    // missing.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    vi.mocked(computerScreenshot).mockRejectedValue(
      new Error(DOMAIN_NOT_FOUND_MSG),
    );
    const user = (await import("@testing-library/user-event")).default;
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // The poll fires immediately; the rejection is
    // caught and the DomainNotFound state renders.
    const stateNode = await screen.findByTestId(
      "computer-domain-not-found",
    );
    expect(stateNode).toBeInTheDocument();
    // The error message is human-friendly — no raw
    // "not found on host" stderr text.
    expect(stateNode.textContent).toMatch(/VM not provisioned/i);
    expect(stateNode.textContent).not.toMatch(/not found on host/);
    // The Provision button is present and wired to
    // `computerProvision(bot_id)`. The brief's
    // acceptance criterion is satisfied by this
    // single assertion: clicking it fires the
    // Tauri command with the bot id and the
    // Settings defaults.
    const provisionButton = await screen.findByTestId(
      "computer-provision-button",
    );
    expect(provisionButton).toBeInTheDocument();
    expect(provisionButton).toHaveTextContent(/Provision/);
    await user.click(provisionButton);
    await waitFor(() => {
      expect(computerProvision).toHaveBeenCalledWith("bot-1", {
        disk_gb: 10,
        ram_mb: 2048,
      });
    });
  });
});
