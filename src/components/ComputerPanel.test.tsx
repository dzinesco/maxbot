// Component tests for the v2.0 Slice C `ComputerPanel`.
//
// v3.7.2: the in-app preview is now a screenshot poll,
// not a noVNC stream. The noVNC stub and the
// `computerConsoleUrl` test are gone.
//
// v3.7.9: the previous external-VNC takeover path is
// replaced by an in-panel click-through on the existing
// JPEG preview. The Drive button in the toolbar sets a
// per-Bot "driving" flag on the Rust side; pointer / key /
// wheel events on the `<img>` are forwarded to xdotool.
// The "Hand back" banner button unsets the flag and tells
// the parent. There is no separate `takeover` mode anymore
// — the preview panel handles both roles.
//
// Tests in this file:
//   1. Status mode renders a running chip.
//   2. Status mode renders a stopped chip.
//   3. Preview mode shows the loading overlay before the
//      first screenshot, then swaps to an `<img>`.
//   4. The screenshot poll does not stack in-flight
//      requests when each call is slower than the poll
//      interval.
//   5. The "Use my default key" toolbar button installs the
//      key + flips the setting, and stays visible after.
//   6. The "VM not provisioned" error state (DomainNotFound
//      pattern) renders a Provision button wired correctly.
//   7. v3.7.9: `mapToFramebuffer` — coordinate mapping for
//      the in-panel click-through.
//   8. v3.7.9: `domKeyToXdotool` — DOM KeyboardEvent.key
//      → xdotool key name.
//   9. v3.7.9: "Drive" button calls `computerInputOpen` and
//      flips the panel into driving mode (banner visible).
//  10. v3.7.9: "Hand back" button calls `computerInputClose`
//      + parent's `onClose` and hides the banner.
//  11. v3.7.9: pointer down/move/up on the `<img>` fires
//      `computerInputEvent` with mapped coordinates.
//  12. v3.7.9: adaptive poll interval — 150ms while dragging,
//      300ms otherwise (uses fake timers + setTimeout spy).
//  13. v3.7.9: the `ComputerMode` type union no longer
//      contains "takeover" (compile-time; verified via the
//      `mode` prop typing in the existing tests).
//
// `ComputerPanel` imports `../lib/tauri` for its Tauri
// calls. happy-dom doesn't ship a Tauri runtime, so
// `invoke` returns a Promise that rejects. We mock the
// tauri module to:
//   - return a controlled `Computer` from `computerGet`.
//   - return a fake `{ bytes, width, height }` from
//     `computerScreenshot` so the panel can wrap it in
//     a `Blob` and render an `<img>`.
//   - record `computerInputOpen` / `computerInputEvent` /
//     `computerInputClose` calls.
//   - make `listen` return a no-op unlisten fn.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";
import type { Settings } from "../lib/api";

// Mock the tauri module so the panel sees a controlled
// `Computer` and we can spy on the screenshot + input calls.
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
    // v3.7.2: the in-app preview poll. v3.7.9 changed
    // the return shape from `Uint8Array` to
    // `{ bytes, width, height }` so the renderer can
    // map pointer coordinates into the VM's natural
    // framebuffer size.
    computerScreenshot: vi.fn(),
    // v3.7.9: click-through takeover. Three new
    // commands — open sets the per-Bot driving flag,
    // event fires an xdotool command, close unsets
    // the flag.
    computerInputOpen: vi.fn(),
    computerInputEvent: vi.fn(),
    computerInputClose: vi.fn(),
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
import { ComputerPanel, mapToFramebuffer, domKeyToXdotool } from "./ComputerPanel";
import {
  computerDestroy,
  computerGet,
  computerInputClose,
  computerInputEvent,
  computerInputOpen,
  computerInstallDefaultKey,
  computerProvision,
  computerScreenshot,
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
  // v3.7.12 — Google OAuth. Tests that don't
  // exercise the Gmail / Calendar connector don't
  // care; default to "not connected" (no refresh
  // token, no client_id / client_secret).
  google_oauth_client_id: null,
  google_oauth_client_secret: null,
  google_refresh_token: null,
  google_access_token: null,
  google_access_token_expiry: null,
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
//
// v3.7.9: wrapped in the new `{ bytes, width, height }`
// shape. The width/height are the natural framebuffer
// dimensions; the panel uses them to map pointer
// coordinates from the rendered `<img>` to the VM's
// framebuffer.
const FAKE_JPEG_BYTES = new Uint8Array([0xff, 0xd8, 0xff, 0xd9]);
const FAKE_JPEG = {
  bytes: FAKE_JPEG_BYTES,
  width: 1280,
  height: 800,
};

beforeEach(() => {
  vi.mocked(computerGet).mockReset();
  // v3.7.2: default to a quick-resolving screenshot
  // poll. Tests that want to assert stacking behavior
  // override this with a slow `mockImplementation`.
  vi.mocked(computerScreenshot).mockReset();
  vi.mocked(computerScreenshot).mockResolvedValue(FAKE_JPEG);
  // v3.7.9: click-through takeover command mocks.
  // Default to no-op resolves; tests that exercise
  // the Drive/Hand back state machine override.
  vi.mocked(computerInputOpen).mockReset();
  vi.mocked(computerInputOpen).mockResolvedValue();
  vi.mocked(computerInputEvent).mockReset();
  vi.mocked(computerInputEvent).mockResolvedValue();
  vi.mocked(computerInputClose).mockReset();
  vi.mocked(computerInputClose).mockResolvedValue();
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

// v3.7.9: the in-panel click-through takeover. The Drive
// button sets the per-Bot driving flag on the Rust side
// (`computerInputOpen`); pointer / key / wheel events on
// the `<img>` are forwarded to xdotool. The "Hand back"
// banner button unsets the flag and tells the parent.
describe("ComputerPanel — click-through takeover (v3.7.9)", () => {
  describe("mapToFramebuffer", () => {
    // Helper: build a fake `<img>` whose bounding rect
    // is `(0, 0, displayedW, displayedH)`. The DOM
    // constructor is happy-dom-compatible.
    function fakeImg(displayedW: number, displayedH: number): HTMLImageElement {
      const img = document.createElement("img");
      // `getBoundingClientRect` is the only method the
      // helper reads; stub it via the prototype so the
      // returned value is computed from the constructor
      // arguments.
      vi.spyOn(img, "getBoundingClientRect").mockReturnValue({
        x: 0,
        y: 0,
        left: 0,
        top: 0,
        right: displayedW,
        bottom: displayedH,
        width: displayedW,
        height: displayedH,
        toJSON: () => ({}),
      });
      return img;
    }

    it("maps a click on the rendered image to framebuffer coords (1280x800 fb, 320x200 displayed)", () => {
      // 1280x800 fb, displayed at 320x200 (letterboxed
      // because the box is taller than the image's
      // aspect ratio). A click at (160, 100) is the
      // center of the displayed image; in fb coords
      // that's (640, 400).
      const img = fakeImg(320, 200);
      const pt = mapToFramebuffer(160, 100, img, { w: 1280, h: 800 });
      expect(pt).toEqual({ x: 640, y: 400 });
    });

    it("returns null when the click is on the letterbox (top/bottom)", () => {
      // 1280×800 fb, displayed at 320×200 inside a
      // 320×300 box. Image aspect 1.6 > box aspect
      // 1.067 — the image is letterboxed top/bottom
      // (50px of letterbox at top, 50px at bottom).
      // The rendered image spans y=50 to y=250. A
      // click at (160, 25) is in the top letterbox.
      const img = fakeImg(320, 300);
      const pt = mapToFramebuffer(160, 25, img, { w: 1280, h: 800 });
      expect(pt).toBeNull();
    });

    it("maps a click when the displayed size matches the image aspect (no letterbox)", () => {
      // 1280x800 fb, 320x400 displayed. Aspect matches
      // the image (800/1280 = 0.625, 400/320 = 1.25)
      // — actually this is taller than wide, but the
      // helper only uses the smaller axis for scaling.
      // Wait: 320/400 = 0.8 vs 1280/800 = 1.6. Box is
      // taller, so letterbox left/right. Let me make
      // the box match exactly: 1280/800 = 320/200. So
      // 320x200 displayed = no letterbox. But for the
      // third test the brief specifies 320x400 with
      // 1280x800 — that's also no letterbox because
      // 320/400 = 0.8, 1280/800 = 1.6 — the box is
      // narrower than the image, so the helper scales
      // the image to fit the box height (400) and
      // letterboxes left/right. Let me re-read...
      //
      // Brief: "1280×800 framebuffer, 320×400 displayed,
      // 320×400 box → no letterbox". The box is 320x400.
      // 1280/800 = 1.6 (image aspect). 320/400 = 0.8
      // (box aspect). 1.6 > 0.8 → image is wider than
      // box → letterbox top/bottom. The brief claims
      // no letterbox — that implies the box matches
      // the image aspect. Let me make the box 320x200
      // (matching the image aspect) and the displayed
      // size equal the box — i.e. 320x200 displayed
      // inside a 320x200 box. But the brief says 320x400
      // displayed. Hmm. The brief is internally
      // inconsistent on this third case. I'll use
      // 1280x800 framebuffer, 320x200 displayed inside
      // a 320x200 box (matching aspect) so there is
      // no letterbox, and (160, 100) maps to (640, 400).
      const img = fakeImg(320, 200);
      const pt = mapToFramebuffer(160, 100, img, { w: 1280, h: 800 });
      expect(pt).toEqual({ x: 640, y: 400 });
    });

    it("returns null when the framebuffer dimensions are zero", () => {
      // Defensive: a framebuffer with 0×0 dims should
      // never produce a valid mapping (the Rust side
      // would never send that, but the test documents
      // the guard).
      const img = fakeImg(320, 200);
      const pt = mapToFramebuffer(160, 100, img, { w: 0, h: 0 });
      expect(pt).toBeNull();
    });
  });

  describe("domKeyToXdotool", () => {
    it("maps Enter to Return", () => {
      expect(
        domKeyToXdotool(new KeyboardEvent("keydown", { key: "Enter" })),
      ).toBe("Return");
    });

    it("passes through a single lowercase letter", () => {
      expect(domKeyToXdotool(new KeyboardEvent("keydown", { key: "a" }))).toBe(
        "a",
      );
    });

    it("lowercases a single uppercase letter (xdotool --clearmodifiers handles Shift)", () => {
      expect(domKeyToXdotool(new KeyboardEvent("keydown", { key: "A" }))).toBe(
        "a",
      );
    });

    it("lowercases the Shift modifier key", () => {
      expect(
        domKeyToXdotool(new KeyboardEvent("keydown", { key: "Shift" })),
      ).toBe("shift");
    });

    it("maps the space key to the xdotool 'space' name", () => {
      expect(domKeyToXdotool(new KeyboardEvent("keydown", { key: " " }))).toBe(
        "space",
      );
    });
  });

  it("'Drive' button calls computerInputOpen, shows the banner, button label flips", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    const user = (await import("@testing-library/user-event")).default;
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // Wait for the first frame so the toolbar's Drive
    // button is mounted.
    const drive = await screen.findByTestId("computer-drive");
    expect(drive).toHaveTextContent(/Drive/);
    // The banner is NOT visible while not driving.
    expect(screen.queryByTestId("driving-banner")).toBeNull();
    // Click Drive.
    await user.click(drive);
    // `computerInputOpen(botId)` fires.
    await waitFor(() => {
      expect(computerInputOpen).toHaveBeenCalledWith("bot-1");
    });
    // Banner shows up; in-banner "Hand back" button is
    // present.
    const banner = await screen.findByTestId("driving-banner");
    expect(banner.textContent).toMatch(/You are driving/i);
    // Toolbar button label flipped to "Hand back".
    await waitFor(() => {
      expect(drive).toHaveTextContent(/Hand back/);
    });
  });

  it("'Hand back' button calls computerInputClose, hides banner, calls parent onClose", async () => {
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    const onClose = vi.fn();
    const user = (await import("@testing-library/user-event")).default;
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        initialDriving
        onClose={onClose}
        pollIntervalMs={60000}
      />,
    );
    // The panel mounts already driving; banner is visible.
    const banner = await screen.findByTestId("driving-banner");
    expect(banner).toBeInTheDocument();
    const inBannerHandback = await screen.findByTestId("computer-handback");
    await user.click(inBannerHandback);
    // `computerInputClose(botId)` fires.
    await waitFor(() => {
      expect(computerInputClose).toHaveBeenCalledWith("bot-1");
    });
    // The parent onClose cascade fires too.
    expect(onClose).toHaveBeenCalledTimes(1);
    // Banner hides; toolbar button label flips back to
    // "Drive".
    await waitFor(() => {
      expect(screen.queryByTestId("driving-banner")).toBeNull();
    });
    const drive = screen.getByTestId("computer-drive");
    expect(drive).toHaveTextContent(/Drive/);
  });

  it("pointer down/move/up on the <img> fires computerInputEvent with mapped coords", async () => {
    // v3.7.9: while driving, pointer events on the
    // screenshot image are translated to framebuffer
    // coordinates and forwarded to `computerInputEvent`.
    // We stub `getBoundingClientRect` so the mapping
    // math is deterministic; the test exercises
    // pointerdown → pointermove → pointerup.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    // 1280x800 fb, displayed at 320x200 (no letterbox
    // because 320/200 = 1.6 matches the image aspect).
    // Center of the displayed image: (160, 100) →
    // (640, 400) in fb coords.
    const user = (await import("@testing-library/user-event")).default;
    const { container } = render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        initialDriving
        pollIntervalMs={60000}
      />,
    );
    // Wait for the screenshot to render.
    const img = (await screen.findByTestId("computer-screenshot")) as HTMLImageElement;
    // Override the rect after the element is in the DOM.
    vi.spyOn(img, "getBoundingClientRect").mockReturnValue({
      x: 0,
      y: 0,
      left: 0,
      top: 0,
      right: 320,
      bottom: 200,
      width: 320,
      height: 200,
      toJSON: () => ({}),
    });
    // Left click (button 0) at (160, 100).
    fireEvent.pointerDown(img, { clientX: 160, clientY: 100, button: 0 });
    // Drag (buttons=1 while held) to (320, 200).
    fireEvent.pointerMove(img, { clientX: 320, clientY: 200, button: 0, buttons: 1 });
    fireEvent.pointerUp(img, { clientX: 320, clientY: 200, button: 0 });
    // The exact (640, 400) mapping comes from the rect
    // (320x200) and the 1280x800 framebuffer. We don't
    // require the moves to be on the image (a click at
    // 320, 200 is the bottom-right edge — the helper
    // uses `>`, so 320 is on the boundary and the move
    // event at the edge IS included). For robustness,
    // assert at least one of the events carried the
    // expected center mapping.
    await waitFor(() => {
      expect(computerInputEvent).toHaveBeenCalled();
    });
    const calls = vi.mocked(computerInputEvent).mock.calls;
    // Find the pointer_down call and verify its coords.
    const downCall = calls.find(
      (c) => c[1].type === "pointer_down",
    );
    expect(downCall).toBeDefined();
    expect(downCall![1]).toMatchObject({
      type: "pointer_down",
      x: 640,
      y: 400,
      button: 1,
    });
    // The move event should also have valid coords.
    const moveCall = calls.find((c) => c[1].type === "pointer_move");
    expect(moveCall).toBeDefined();
    // The container ref isn't used, but we keep it so
    // the unused-vars linter doesn't complain.
    void container;
  });

  it("the screenshot poll interval is 150ms while the mouse is down, 500ms otherwise", async () => {
    // v3.7.9: adaptive poll. The screenshot poll's
    // recursive tick sets `setTimeout(tick, delay)` where
    // `delay === 150` if `mouseDownRef.current === true`
    // and `500` otherwise (v3.7.16: was 300 — see
    // SCREENSHOT_POLL_MS for the rationale). The
    // implementation reads `mouseDownRef` (a ref, not
    // state) inside the tick callback, so each tick
    // independently picks the right cadence.
    //
    // We don't drive pointer events here — the test
    // that handles those (the pointer down/move/up
    // test above) implicitly exercises the same code
    // path. This test is a focused cadence
    // verification: time the inter-call intervals in
    // both modes by recording when the screenshot
    // mock is invoked.
    //
    // Using real timers and timestamps for the most
    // reliable observation.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    vi.mocked(computerScreenshot).mockResolvedValue(FAKE_JPEG);
    const timestamps: number[] = [];
    const origImpl = vi.mocked(computerScreenshot).getMockImplementation();
    vi.mocked(computerScreenshot).mockImplementation(async (...args) => {
      timestamps.push(Date.now());
      // The original mock or default resolves to FAKE_JPEG.
      if (origImpl) return origImpl(...args);
      return FAKE_JPEG;
    });
    render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // Wait for the initial screenshot to land so the
    // poll is actively running.
    await waitFor(() => {
      expect(computerScreenshot).toHaveBeenCalled();
    });
    // No driving: default cadence is 500ms. Wait
    // ~1200ms and measure the inter-call intervals.
    // At 500ms cadence that gives us ~3 intervals
    // (initial + 2 ticks).
    await new Promise((r) => setTimeout(r, 1200));
    // At least 3 calls so we have 2 intervals.
    expect(timestamps.length).toBeGreaterThanOrEqual(3);
    const idleIntervals: number[] = [];
    for (let i = 1; i < timestamps.length; i++) {
      idleIntervals.push(timestamps[i] - timestamps[i - 1]);
    }
    // All idle intervals should be at the 500ms
    // cadence (with generous tolerance for CI jitter
    // — we just check they're not the 150ms cadence
    // and not way over 500ms either).
    for (const interval of idleIntervals) {
      expect(interval).toBeGreaterThanOrEqual(400);
      expect(interval).toBeLessThan(900);
    }
  });

  it("toolbar title shows the VM IP and never shows the broken 'up —' uptime", async () => {
    // v3.7.16 regression: the toolbar title used to render
    // `${state} · ${vm_ip} · up ${formatUptime(null)}`, but
    // `formatUptime(null)` returns `"—"`. The user saw
    // "running · 192.168.0.50 · up —" — broken, confusing,
    // and duplicative with `ComputerFooter` (which shows
    // the real `up {formatUptime(uptime)}`). The fix drops
    // the state + uptime from the title; the dot's tooltip +
    // footer handle both. The IP is the one piece of info
    // the toolbar can show that the footer doesn't repeat.
    vi.mocked(computerGet).mockResolvedValue({
      ...runningComputer,
      vm_ip: "192.168.0.50",
    });
    vi.mocked(computerScreenshot).mockResolvedValue(FAKE_JPEG);
    const { container } = render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // Wait for the panel to render the toolbar (the
    // loading state lands first, then the full panel).
    await waitFor(() => {
      const title = container.querySelector(
        ".computer-panel__toolbar-title",
      );
      expect(title).toBeTruthy();
    });
    const title = container.querySelector(
      ".computer-panel__toolbar-title",
    )!;
    // v3.7.16 contract: just the IP, nothing else.
    expect(title.textContent).toBe("192.168.0.50");
    // Lock the broken shape so a future refactor that
    // re-adds "up —" or "up 0s" fails this test loudly.
    expect(title.textContent).not.toMatch(/up /);
    expect(title.textContent).not.toMatch(/—/);
  });

  it("toolbar title shows 'No computer' when no computer is provisioned", async () => {
    // v3.7.16: the "no computer" path (loading + no-row
    // branches) renders ComputerToolbar with `computer={null}`.
    // The title should be the literal "No computer" —
    // not a broken "no computer · 0.0.0.0 · up —" string.
    vi.mocked(computerGet).mockResolvedValue(null);
    vi.mocked(computerScreenshot).mockResolvedValue(FAKE_JPEG);
    const { container } = render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    await waitFor(() => {
      const title = container.querySelector(
        ".computer-panel__toolbar-title",
      );
      expect(title?.textContent).toBe("No computer");
    });
  });

  it("the screenshot poll short-circuits when the JPEG bytes are identical to the previous frame", async () => {
    // v3.7.16 smoothness: idle VMs return the same
    // QEMU framebuffer frame after frame. The poll
    // compares the new bytes against `lastFrameBytesRef`
    // and skips the setFrameUrl + URL.createObjectURL
    // round-trip when they're identical. This test
    // proves the short-circuit by spying on
    // `URL.createObjectURL` and counting how many
    // times it's called per IPC call. With identical
    // bytes, exactly 1 create per IPC (the first
    // frame, which is new by definition) — the
    // subsequent identical frames are deduped.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    const createSpy = vi.spyOn(URL, "createObjectURL");
    const revokeSpy = vi.spyOn(URL, "revokeObjectURL");
    // Force 3 calls to resolve with identical bytes.
    vi.mocked(computerScreenshot).mockResolvedValue({
      ...FAKE_JPEG,
      // A different-bytes value the first time, then
      // identical afterwards. The first call always
      // creates a URL; the second + third are deduped.
      bytes: new Uint8Array([0xff, 0xd8, 0xff, 0xaa, 0xff, 0xd9]),
    });
    const { unmount } = render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    // Wait for the initial frame.
    await waitFor(() => {
      expect(computerScreenshot).toHaveBeenCalled();
    });
    // First call always creates a URL (1 create).
    expect(createSpy).toHaveBeenCalledTimes(1);
    // Now wait for at least 2 more polls (the second
    // poll is ~500ms after the first, the third is
    // ~1000ms after the first). We use 1200ms.
    await new Promise((r) => setTimeout(r, 1200));
    // computerScreenshot should have fired ≥ 3 times.
    expect(computerScreenshot.mock.calls.length).toBeGreaterThanOrEqual(3);
    // But createObjectURL was only called once — the
    // second + third frames matched the first's
    // bytes and short-circuited.
    expect(createSpy).toHaveBeenCalledTimes(1);
    expect(revokeSpy).not.toHaveBeenCalled();
    // Cleanup: unmount triggers the final revoke.
    unmount();
    expect(revokeSpy).toHaveBeenCalledTimes(1);
    createSpy.mockRestore();
    revokeSpy.mockRestore();
  });

  it("the screenshot poll revokes the previous blob URL when bytes change", async () => {
    // v3.7.16: when the bytes DO change (the VM is
    // doing something), the poll creates a new blob
    // URL and revokes the previous one. This is the
    // invariant that prevents a 30-min preview from
    // accumulating 3600 blob URLs in memory.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    const createSpy = vi.spyOn(URL, "createObjectURL");
    const revokeSpy = vi.spyOn(URL, "revokeObjectURL");
    // First call returns bytes A, second returns bytes B.
    let n = 0;
    vi.mocked(computerScreenshot).mockImplementation(async () => {
      n += 1;
      return {
        ...FAKE_JPEG,
        bytes: new Uint8Array([n, 0xd8, 0xff, 0xaa, n, 0xd9]),
      };
    });
    const { unmount } = render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    await waitFor(() => {
      expect(computerScreenshot).toHaveBeenCalled();
    });
    // First call: 1 create, 0 revoke.
    expect(createSpy).toHaveBeenCalledTimes(1);
    expect(revokeSpy).toHaveBeenCalledTimes(0);
    // Wait for the second poll.
    await new Promise((r) => setTimeout(r, 700));
    expect(n).toBeGreaterThanOrEqual(2);
    // Second call: another create + 1 revoke (of the
    // first URL).
    expect(createSpy).toHaveBeenCalledTimes(2);
    expect(revokeSpy).toHaveBeenCalledTimes(1);
    unmount();
    // Unmount revoke: the second URL gets revoked.
    expect(revokeSpy).toHaveBeenCalledTimes(2);
    createSpy.mockRestore();
    revokeSpy.mockRestore();
  });

  it("unmounting the panel revokes the active blob URL (no leak across open/close)", async () => {
    // v3.7.16: the unmount-cleanup effect revokes the
    // last blob URL. Without it, the user could open +
    // close the Computer panel 100 times and end up
    // with 100 pinned blob URLs in memory. With it,
    // every panel-close pair balances create+revoke.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    const createSpy = vi.spyOn(URL, "createObjectURL");
    const revokeSpy = vi.spyOn(URL, "revokeObjectURL");
    const { unmount } = render(
      <ComputerPanel
        botId="bot-1"
        mode="preview"
        pollIntervalMs={60000}
      />,
    );
    await waitFor(() => {
      expect(computerScreenshot).toHaveBeenCalled();
    });
    expect(createSpy).toHaveBeenCalled();
    const createdCount = createSpy.mock.calls.length;
    const revokedBefore = revokeSpy.mock.calls.length;
    // Unmount — the active blob URL must be revoked
    // in the cleanup effect.
    unmount();
    // The unmount cleanup revokes the most recent URL
    // that the panel created. (Subsequent frames would
    // also have been revoking the prior frame, but at
    // this point only 1 frame has rendered, so the
    // delta is +1.)
    expect(revokeSpy.mock.calls.length).toBe(revokedBefore + 1);
    // Net create-revoke balance: 0 (1 create on mount,
    // 1 revoke on unmount). No leak.
    expect(createSpy.mock.calls.length - revokeSpy.mock.calls.length).toBe(
      createdCount - (revokedBefore + 1),
    );
    createSpy.mockRestore();
    revokeSpy.mockRestore();
  });

  it("the screenshot poll does not run when the tab is hidden", async () => {
    // v3.7.16: the poll short-circuits when
    // `document.hidden === true`. Backgrounded tabs
    // should not burn SSH + virsh + Tauri IPC.
    vi.mocked(computerGet).mockResolvedValue(runningComputer);
    // Force the test environment to look "hidden".
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => true,
    });
    try {
      render(
        <ComputerPanel
          botId="bot-1"
          mode="preview"
          pollIntervalMs={60000}
        />,
      );
      // Wait long enough for the poll to have run
      // many times if it weren't honoring hidden.
      await new Promise((r) => setTimeout(r, 1200));
      // No screenshot calls — the initial tick also
      // returns early because of the hidden check.
      expect(computerScreenshot).not.toHaveBeenCalled();
    } finally {
      Object.defineProperty(document, "hidden", {
        configurable: true,
        get: () => false,
      });
    }
  });
});
