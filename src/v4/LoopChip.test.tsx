// v4 — LoopChip tests (S2.6).
//
// Per the v4 brief:
//
//   - `LoopChip.test.tsx` — fetches status once on mount;
//     no interval.
//
// S2.6 — the chip is now a small text-only row in the rail
// footer. No big Start/Stop buttons. We assert:
//   1. `loopdStatus` is called exactly once on mount.
//   2. The status text renders correctly per state.
//   3. The Start/Stop buttons are NOT rendered in the default
//      rail (S2.6 brief: remove them).
//   4. Clicking the toggle fires onToggleExpand.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { LoopChip } from "./LoopChip";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => cleanup());

describe("LoopChip — S2.6 small text chip", () => {
  it("calls loopdStatus once on mount and renders the alive label", async () => {
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
    render(
      <LoopChip onToggleExpand={() => {}} expanded={false} />,
    );
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("loopd_status", undefined);
    });
    expect(invokeMock).toHaveBeenCalledTimes(1);
    // Status text shows "alive" + pid.
    await waitFor(() => {
      expect(document.body.textContent).toMatch(/alive/);
    });
  });

  it("renders 'Loop dead' label when state is dead", async () => {
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
    render(<LoopChip onToggleExpand={() => {}} expanded={false} />);
    await waitFor(() => {
      expect(document.body.textContent).toMatch(/dead/);
    });
  });

  it("renders 'Loop idle' label when state is idle", async () => {
    invokeMock.mockResolvedValue({
      state: "idle",
      alive: false,
      pid: null,
      task_status: "",
      task_excerpt_len: 0,
      last_heartbeat: null,
      age_secs: null,
      loop_dir: "/tmp/loop",
    });
    render(<LoopChip onToggleExpand={() => {}} expanded={false} />);
    await waitFor(() => {
      expect(document.body.textContent).toMatch(/idle/);
    });
  });

  it("does NOT render a Start or Stop button in the default rail", async () => {
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
    const { container } = render(
      <LoopChip onToggleExpand={() => {}} expanded={false} />,
    );
    await waitFor(() => {
      expect(document.body.textContent).toMatch(/alive/);
    });
    // No action buttons.
    expect(
      container.querySelector(".v4-loop-chip-action"),
    ).toBeNull();
    // The toggle is still there.
    expect(
      container.querySelector(".v4-loop-chip-toggle"),
    ).toBeTruthy();
  });

  it("clicking the toggle fires onToggleExpand", async () => {
    invokeMock.mockResolvedValue({
      state: "idle",
      alive: false,
      pid: null,
      task_status: "",
      task_excerpt_len: 0,
      last_heartbeat: null,
      age_secs: null,
      loop_dir: "/tmp/loop",
    });
    const onToggle = vi.fn();
    const { container } = render(
      <LoopChip onToggleExpand={onToggle} expanded={false} />,
    );
    await waitFor(() => {
      expect(container.querySelector(".v4-loop-chip-toggle")).toBeTruthy();
    });
    const toggle = container.querySelector(
      ".v4-loop-chip-toggle",
    ) as HTMLButtonElement;
    fireEvent.click(toggle);
    expect(onToggle).toHaveBeenCalledTimes(1);
  });
});
