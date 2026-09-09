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

beforeEach(() => {
  vi.mocked(computerGet).mockReset();
  vi.mocked(computerConsoleUrl).mockReset();
  vi.mocked(computerConsoleUrl).mockResolvedValue(
    "ws://localhost:5900/",
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
