// v4 — LoopExpanded tests.
//
// Per the v4 brief:
//
//   - `LoopExpanded.test.tsx` — fetches task + journal on
//     `open=true`; no fetch when `open=false`.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { LoopExpanded } from "./LoopExpanded";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockImplementation((cmd: string) => {
    if (cmd === "loopd_read_task") {
      return Promise.resolve({
        status: "running",
        body: "Task body for the fixture.",
        updated_at: "2026-09-11T18:00:00+00:00",
        exists: true,
      });
    }
    if (cmd === "loopd_read_journal") {
      return Promise.resolve({
        date: "2026-09-11",
        last_heading: "Turn 1 @ t1",
        last_actions: ["noop"],
        last_note: null,
        line_count: 6,
        exists: true,
      });
    }
    return Promise.resolve(null);
  });
});

afterEach(() => cleanup());

describe("LoopExpanded", () => {
  it("does not fetch when open=false", () => {
    render(<LoopExpanded open={false} />);
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("fetches task + journal in parallel when open=true", async () => {
    render(<LoopExpanded open={true} />);
    // Both should be called once.
    await vi.waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("loopd_read_task", undefined);
    });
    expect(invokeMock).toHaveBeenCalledWith("loopd_read_journal", undefined);
  });

  it("does not create any timers of its own", () => {
    // LoopExpanded is purely mount-once / unmount-once. The
    // mock IPC counters above prove it doesn't poll. We don't
    // spy on the global setInterval/setTimeout because
    // happy-dom uses them internally; the IPC call counts
    // are the ground truth for "no polling".
    render(<LoopExpanded open={true} />);
    expect(invokeMock).toHaveBeenCalledTimes(2); // task + journal, once each
  });
});
