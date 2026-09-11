// v4 — ApprovalSheet tests (S3).
//
// Coverage:
//   - Renders nothing when botId is null (no fetch).
//   - On mount + botId, fetches approvalList and shows the first
//     pending approval.
//   - Approve calls approvalDecide(approved) and clears the sheet.
//   - Deny calls approvalDecide(rejected) and clears the sheet.
//   - Window focus re-fetches (no interval, no timer).
//
// Note: we test the focus refresh behavior by firing the real
// `focus` event on the window object (fireEvent.focus) — that's
// the same path `useFocusRefresh` listens on. We avoid spying on
// window.addEventListener directly because it leaks across tests
// under happy-dom (multiple components in earlier files also call
// addEventListener).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { ApprovalSheet } from "./ApprovalSheet";
import type { Approval } from "../lib/api";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => {
  cleanup();
});

const now = "2026-09-11T18:00:00+00:00";

function pendingApproval(over: Partial<Approval> = {}): Approval {
  return {
    id: "apr-1",
    bot_id: "bot-1",
    tool_name: "shell_run",
    status: "pending",
    payload: { cmd: "date" },
    result: null,
    bot_run_id: null,
    tool_call_id: "tc-1",
    created_at: now,
    decided_at: null,
    reason: "Need to run a shell command",
    is_takeover: false,
    ...over,
  };
}

describe("ApprovalSheet — S3 contract", () => {
  it("renders nothing when botId is null (no fetch)", async () => {
    invokeMock.mockImplementation(() =>
      Promise.reject(new Error("should not be called")),
    );
    render(<ApprovalSheet botId={null} />);
    // Give any (incorrectly) queued effect a tick to fire.
    await new Promise((r) => setTimeout(r, 10));
    expect(invokeMock).not.toHaveBeenCalled();
    expect(screen.queryByTestId("v4-approval-sheet")).toBeNull();
  });

  it("fetches approvalList on mount and shows the first pending approval", async () => {
    const apr = pendingApproval();
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "approval_list") return Promise.resolve([apr]);
      return Promise.resolve(null);
    });
    render(<ApprovalSheet botId="bot-1" />);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("approval_list", {
        botId: "bot-1",
      });
    });
    await waitFor(() =>
      expect(screen.getByTestId("v4-approval-sheet")).toBeTruthy(),
    );
    expect(screen.getByText("shell_run")).toBeTruthy();
    expect(screen.getByText(/Need to run a shell command/i)).toBeTruthy();
  });

  it("renders nothing when approvalList returns no pending rows", async () => {
    const decided = {
      ...pendingApproval(),
      id: "apr-old",
      status: "approved" as const,
    };
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "approval_list") return Promise.resolve([decided]);
      return Promise.resolve(null);
    });
    render(<ApprovalSheet botId="bot-1" />);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("approval_list", {
        botId: "bot-1",
      });
    });
    // Sheet should never appear when nothing is pending.
    await waitFor(() => {
      expect(screen.queryByTestId("v4-approval-sheet")).toBeNull();
    });
  });

  it("Approve calls approvalDecide(approved) and clears the sheet", async () => {
    const apr = pendingApproval();
    let decided = false;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "approval_list") {
        // After decide, the row is gone — return an empty list.
        return Promise.resolve(decided ? [] : [apr]);
      }
      if (cmd === "approval_decide") {
        decided = true;
        return Promise.resolve({
          approval: { ...apr, status: "approved" },
          tool_result: null,
        });
      }
      return Promise.resolve(null);
    });
    render(<ApprovalSheet botId="bot-1" />);
    const approveBtn = await screen.findByTestId("v4-approval-sheet-approve");
    fireEvent.click(approveBtn);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "approval_decide",
        expect.objectContaining({ id: "apr-1", decision: "approved" }),
      );
    });
    // After approve, the sheet clears and re-fetches — the new
    // list returns empty, so the sheet stays cleared.
    await waitFor(() => {
      expect(screen.queryByTestId("v4-approval-sheet")).toBeNull();
    });
    const listCalls = invokeMock.mock.calls.filter(
      (c) => c[0] === "approval_list",
    );
    expect(listCalls.length).toBeGreaterThanOrEqual(2);
  });

  it("Deny calls approvalDecide(rejected) and clears the sheet", async () => {
    const apr = pendingApproval();
    let decided = false;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "approval_list") {
        return Promise.resolve(decided ? [] : [apr]);
      }
      if (cmd === "approval_decide") {
        decided = true;
        return Promise.resolve({
          approval: { ...apr, status: "rejected" },
          tool_result: null,
        });
      }
      return Promise.resolve(null);
    });
    render(<ApprovalSheet botId="bot-1" />);
    const denyBtn = await screen.findByTestId("v4-approval-sheet-deny");
    fireEvent.click(denyBtn);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "approval_decide",
        expect.objectContaining({
          id: "apr-1",
          decision: "rejected",
        }),
      );
    });
    await waitFor(() => {
      expect(screen.queryByTestId("v4-approval-sheet")).toBeNull();
    });
  });

  it("window focus re-fetches approvalList (no interval)", async () => {
    const apr = pendingApproval();
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "approval_list") return Promise.resolve([apr]);
      return Promise.resolve(null);
    });
    render(<ApprovalSheet botId="bot-1" />);
    // Wait for the mount fetch.
    await waitFor(() =>
      expect(invokeMock).toHaveBeenCalledWith("approval_list", {
        botId: "bot-1",
      }),
    );
    const before = invokeMock.mock.calls.filter(
      (c) => c[0] === "approval_list",
    ).length;
    // Real focus event — same path useFocusRefresh listens on.
    fireEvent.focus(window);
    await waitFor(() => {
      const after = invokeMock.mock.calls.filter(
        (c) => c[0] === "approval_list",
      ).length;
      expect(after).toBe(before + 1);
    });
  });
});
