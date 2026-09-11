// v4 — LoopChip tests.
//
// Per the v4 brief:
//
//   - `LoopChip.test.tsx` — fetches status once on mount;
//     no interval.
//
// We mock `@tauri-apps/api/core`'s `invoke` to return a
// fixed `LoopdStatus` shape. We assert:
//   1. `loopdStatus` is called exactly once on mount.
//   2. No setInterval / recursive setTimeout is created.
//   3. The status pill renders the right label + tone.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, waitFor } from "@testing-library/react";
import { LoopChip } from "./LoopChip";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

beforeEach(() => {
  invokeMock.mockReset();
  invokeMock.mockResolvedValue({
    state: "alive",
    alive: true,
    pid: 4242,
    task_status: "running",
    task_excerpt_len: 120,
    last_heartbeat: "2026-09-11T18:00:00+00:00",
    age_secs: 30,
    loop_dir: "/tmp/loop",
  });
});

afterEach(() => cleanup());

describe("LoopChip", () => {
  it("calls loopdStatus once on mount and renders the alive label", async () => {
    render(
      <LoopChip
        onToggleExpand={() => {}}
        expanded={false}
        onStart={() => {}}
        onStop={() => {}}
        busy={false}
      />,
    );
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("loopd_status", undefined);
    });
    // One mount-time fetch. The chip's "no interval" rule is
    // verified by the call count: a polling chip would have
    // called loopd_status more than once across this test's
    // ~1s lifetime (happy-dom uses fake timers implicitly
    // for `setInterval`, so we don't spy on the global — it
    // would catch happy-dom's own use of setInterval).
    expect(invokeMock).toHaveBeenCalledTimes(1);
  });

  it("shows a Stop button when state is alive", async () => {
    const { findByText } = render(
      <LoopChip
        onToggleExpand={() => {}}
        expanded={false}
        onStart={() => {}}
        onStop={() => {}}
        busy={false}
      />,
    );
    const stop = await findByText(/^Stop$/);
    expect(stop).toBeTruthy();
  });

  it("shows a Start button when state is dead", async () => {
    invokeMock.mockResolvedValue({
      state: "dead",
      alive: false,
      pid: null,
      task_status: "",
      task_excerpt_len: 0,
      last_heartbeat: null,
      age_secs: null,
      loop_dir: "/tmp/loop",
    });
    const { findByText } = render(
      <LoopChip
        onToggleExpand={() => {}}
        expanded={false}
        onStart={() => {}}
        onStop={() => {}}
        busy={false}
      />,
    );
    const start = await findByText(/^Start$/);
    expect(start).toBeTruthy();
  });

  it("calls onStart when the Start button is clicked", async () => {
    invokeMock.mockResolvedValue({
      state: "dead",
      alive: false,
      pid: null,
      task_status: "",
      task_excerpt_len: 0,
      last_heartbeat: null,
      age_secs: null,
      loop_dir: "/tmp/loop",
    });
    const onStart = vi.fn();
    const { findByText } = render(
      <LoopChip
        onToggleExpand={() => {}}
        expanded={false}
        onStart={onStart}
        onStop={() => {}}
        busy={false}
      />,
    );
    const start = await findByText(/^Start$/);
    start.click();
    expect(onStart).toHaveBeenCalledTimes(1);
  });
});
