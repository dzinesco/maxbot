// v4 — ComputerRoute tests (S6).
//
// Coverage:
//   1. Renders the "Waiting for first frame…" placeholder
//      before the first tick.
//   2. After a successful screenshot tick, an <img> with a
//      blob: URL appears.
//   3. Two sequential ticks → the prior blob URL is revoked
//      before the next URL is created (no blob leak).
//   4. Unmount revokes the final blob URL + calls the
//      unlisten fn.
//   5. onComputerStateChanged events for the same bot
//      update the displayed state.
//   6. onComputerStateChanged events for a different bot
//      are ignored.
//   7. Error path: computer_screenshot rejects →
//      "Retrying…" placeholder + the tick keeps going.
//   8. Close button calls onClose.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { ComputerRoute } from "./ComputerRoute";
import type { Computer, ComputerStateChangedEvent } from "../lib/api";

const invokeMock = vi.fn();
const listenMock = vi.fn();
const unlistenMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  // Mirror ChatPane's mock: the production wrapper passes a
  // lambda `(e) => userHandler(e.payload)`, so we call our
  // captured handler with `{ payload }` and it forwards
  // straight to the user.
  listen: (
    event: string,
    handler: (e: { payload: unknown }) => void,
  ) => {
    listenMock(event, handler);
    return Promise.resolve(unlistenMock);
  },
}));

// Track every URL.createObjectURL / revokeObjectURL call so
// tests can assert the blob lifecycle.
const createdUrls: string[] = [];
const revokedUrls: string[] = [];
const realCreate = URL.createObjectURL.bind(URL);
const realRevoke = URL.revokeObjectURL.bind(URL);
URL.createObjectURL = ((blob: Blob) => {
  const url = realCreate(blob);
  createdUrls.push(url);
  return url;
}) as typeof URL.createObjectURL;
URL.revokeObjectURL = ((url: string) => {
  revokedUrls.push(url);
  return realRevoke(url);
}) as typeof URL.revokeObjectURL;

const handlersByEvent: Record<string, Array<(e: { payload: unknown }) => void>> = {};

function setUpStateCapture() {
  for (const k of Object.keys(handlersByEvent)) {
    delete handlersByEvent[k];
  }
  listenMock.mockImplementation(
    (
      event: string,
      handler: (e: { payload: unknown }) => void,
    ) => {
      if (!handlersByEvent[event]) handlersByEvent[event] = [];
      handlersByEvent[event].push(handler);
      return Promise.resolve(unlistenMock);
    },
  );
}

function fireStateEvent(payload: ComputerStateChangedEvent) {
  for (const h of handlersByEvent["computer://state-changed"] ?? []) {
    h({ payload });
  }
}

const baseComputer: Computer = {
  bot_id: "bot-1",
  vm_name: "vm-alpha",
  vm_ip: "10.0.0.5",
  vnc_port: 5900,
  ssh_key_id: "key-1",
  state: "running",
  last_seen_at: "2026-09-11T18:00:00+00:00",
  created_at: "2026-09-11T18:00:00+00:00",
};

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockReset();
  unlistenMock.mockReset();
  createdUrls.length = 0;
  revokedUrls.length = 0;
  setUpStateCapture();
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "computer_get") return Promise.resolve(baseComputer);
    if (cmd === "computer_screenshot")
      // 4 bytes is enough to satisfy ComputerRoute's
      // bytesToBlob. happy-dom doesn't validate PNG.
      return Promise.resolve({ bytes: [0, 1, 2, 3], width: 4, height: 4 });
    return Promise.resolve(null);
  });
});

afterEach(() => {
  cleanup();
});

describe("ComputerRoute — S6 contract", () => {
  it("renders the waiting placeholder before the first tick", async () => {
    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    expect(screen.getByText(/Waiting for first frame/i)).toBeTruthy();
    expect(screen.queryByRole("img")).toBeNull();
  });

  it("shows an <img> with a blob: URL after a successful screenshot", async () => {
    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    await waitFor(
      () => {
        const img = screen.getByRole("img") as HTMLImageElement;
        expect(img.src.startsWith("blob:")).toBe(true);
      },
      { timeout: 3000 },
    );
    expect(createdUrls.length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByText(/Waiting for first frame/i)).toBeNull();
  });

  it("revokes the prior blob URL before creating the next one", async () => {
    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    // Wait for at least two ticks so we can observe the
    // revoke-before-create pattern.
    await waitFor(
      () => expect(
        invokeMock.mock.calls.filter(
          (c) => c[0] === "computer_screenshot",
        ).length,
      ).toBeGreaterThanOrEqual(2),
      { timeout: 5000 },
    );
    expect(createdUrls.length).toBeGreaterThanOrEqual(2);
    // The first created URL must be revoked by now.
    expect(revokedUrls).toContain(createdUrls[0]);
    // At most one live URL outstanding: the latest frame.
    const live = createdUrls.filter((u) => !revokedUrls.includes(u));
    expect(live.length).toBeLessThanOrEqual(1);
  });

  it("revokes the final blob URL on unmount and unsubscribes", async () => {
    const { unmount } = render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    await waitFor(
      () => expect(createdUrls.length).toBeGreaterThanOrEqual(1),
      { timeout: 3000 },
    );
    const lastUrl = createdUrls[createdUrls.length - 1];
    expect(revokedUrls).not.toContain(lastUrl);
    unmount();
    expect(revokedUrls).toContain(lastUrl);
    expect(unlistenMock).toHaveBeenCalled();
  });

  it("updates the displayed state when a state-change event arrives for this bot", async () => {
    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    // Wait for the state listener to register.
    await waitFor(() =>
      expect(handlersByEvent["computer://state-changed"]?.length).toBeGreaterThanOrEqual(1),
    );
    expect(screen.getByText("running")).toBeTruthy();
    fireStateEvent({
      bot_id: "bot-1",
      state: "stopped",
    });
    await waitFor(() => expect(screen.getByText("stopped")).toBeTruthy());
  });

  it("ignores state-change events for a different bot", async () => {
    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    await waitFor(() =>
      expect(handlersByEvent["computer://state-changed"]?.length).toBeGreaterThanOrEqual(1),
    );
    fireStateEvent({
      bot_id: "some-other-bot",
      state: "stopped",
    });
    await new Promise((r) => setTimeout(r, 50));
    expect(screen.getByText("running")).toBeTruthy();
    expect(screen.queryByText("stopped")).toBeNull();
  });

  it("shows Retrying… on screenshot error and keeps polling", async () => {
    let shotCount = 0;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "computer_get") return Promise.resolve(baseComputer);
      if (cmd === "computer_screenshot") {
        shotCount++;
        if (shotCount === 1) {
          return Promise.reject(new Error("vm not provisioned"));
        }
        // Subsequent ticks succeed — verifies the chain
        // continues despite the first failure.
        return Promise.resolve({
          bytes: [0, 1, 2, 3],
          width: 4,
          height: 4,
        });
      }
      return Promise.resolve(null);
    });

    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    // First tick rejects → error banner + Retrying placeholder.
    await waitFor(
      () => {
        expect(screen.getByText(/Screenshot failed/i)).toBeTruthy();
      },
      { timeout: 3000 },
    );
    expect(screen.getByText(/Retrying/i)).toBeTruthy();
    // Second tick (after backoff) succeeds → frame appears.
    await waitFor(
      () => {
        const img = screen.queryByRole("img") as HTMLImageElement | null;
        expect(img?.src.startsWith("blob:")).toBe(true);
      },
      { timeout: 5000 },
    );
    expect(shotCount).toBeGreaterThanOrEqual(2);
  });

  it("Close button calls onClose", async () => {
    const onClose = vi.fn();
    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={onClose}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: /close/i }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("auto-calls computer_show_desktop on mount (S7)", async () => {
    render(
      <ComputerRoute
        botId="bot-1"
        botName="Robot"
        onClose={() => {}}
      />,
    );
    // Wait for the call to land. computer_get resolves
    // first (the test mock), then computer_show_desktop
    // fires fire-and-forget.
    await waitFor(() => {
      const calls = invokeMock.mock.calls.filter(
        (c) => c[0] === "computer_show_desktop",
      );
      expect(calls.length).toBeGreaterThanOrEqual(1);
    });
    const call = invokeMock.mock.calls.find(
      (c) => c[0] === "computer_show_desktop",
    );
    expect(call?.[1]).toEqual({ botId: "bot-1" });
  });
});
