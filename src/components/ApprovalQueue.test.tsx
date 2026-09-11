// Component tests for the v2.6.0 ApprovalQueue panel.
//
// We exercise two paths:
//   1. The queue renders pending approvals with
//      Approve / Reject / Edit buttons.
//   2. Clicking Reject calls `approval_decide` with
//      `"rejected"`.
//
// We mock the tauri module so the test never hits
// the Tauri runtime.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";

vi.mock("../lib/tauri", () => ({
  approvalList: vi.fn(),
  approvalDecide: vi.fn(),
  // v3.4.0 (Phase 5) — Takeover predicate. The
  // tests below use real `mail_send` tool names
  // (not `__takeover__`), so the predicate
  // returns false in every fixture; we still need
  // to export it because the queue component
  // imports it.
  isTakeoverApproval: (a: Approval) =>
    a.tool_name === "__takeover__",
}));

import { ApprovalQueue } from "./ApprovalQueue";
import {
  approvalDecide,
  approvalList,
  isTakeoverApproval,
} from "../lib/tauri";
import type { Approval, Bot } from "../lib/api";

// Touch the imported `isTakeoverApproval` so the
// test file's `noUnusedImports` / `verbatimModuleSyntax`
// doesn't trip on it; the queue component is what
// actually exercises it.
void isTakeoverApproval;

const blankBot = (id: string, name: string): Bot => {
  const now = new Date().toISOString();
  return {
    id,
    name,
    description: "",
    system_prompt: "",
    default_model: "MiniMax-M3",
    allowed_tools: [],
    icon: "🤖",
    color: "",
    created_at: now,
    updated_at: now,
  };
};

const sampleApproval = (id: string, overrides: Partial<Approval> = {}): Approval => ({
  id,
  bot_id: "bot-1",
  tool_name: "mail_send",
  status: "pending",
  payload: { to: "user@example.com", subject: "hi" },
  result: null,
  bot_run_id: "run-1",
  tool_call_id: "tc-1",
  created_at: "2026-09-09T16:00:00Z",
  decided_at: null,
  ...overrides,
});

beforeEach(() => {
  vi.mocked(approvalList).mockReset();
  vi.mocked(approvalDecide).mockReset();
  vi.mocked(approvalDecide).mockResolvedValue({
    approval: sampleApproval("ignored"),
    // v3.7.13 — Mock the new structured result
    // shape. The old `tool_result: "ok"` string is
    // now `tool_result: { kind: "approved", ... }`.
    // Tests that need a different shape (e.g.
    // `denied`) override per-test.
    tool_result: {
      kind: "approved",
      payload: { ok: true },
      summary: "ok",
    },
    tool_result_string: "ok",
  });
});

describe("ApprovalQueue", () => {
  it("renders the pending approvals with quick-action Approve/Reject buttons", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([
      sampleApproval("a-1"),
      sampleApproval("a-2", { tool_name: "file_write" }),
    ]);
    render(<ApprovalQueue bots={[blankBot("bot-1", "TestBot")]} />);
    // Both rows render. We look for the Approve buttons
    // by data-testid so we don't depend on the
    // row's `pending` label, which can drift.
    await waitFor(() => {
      expect(screen.getByTestId("approval-approve-a-1")).toBeTruthy();
      expect(screen.getByTestId("approval-reject-a-1")).toBeTruthy();
      // v3.7.13 — UX-4. The per-row "Edit & send"
      // button is gone; the queue is a navigation
      // list, the Edit & approve flow happens in
      // the parent's ApprovalSheet.
      expect(screen.queryByTestId("approval-edit-a-1")).toBeNull();
      expect(screen.getByTestId("approval-approve-a-2")).toBeTruthy();
    });
  });

  it("clicking Reject calls approval_decide with 'rejected'", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([sampleApproval("a-9")]);
    render(<ApprovalQueue bots={[blankBot("bot-1", "TestBot")]} />);
    await waitFor(() => {
      expect(screen.getByTestId("approval-reject-a-9")).toBeTruthy();
    });
    fireEvent.click(screen.getByTestId("approval-reject-a-9"));
    await waitFor(() => {
      expect(approvalDecide).toHaveBeenCalledWith(
        "a-9",
        "rejected",
        undefined,
      );
    });
  });

  it("clicking Approve calls approval_decide with 'approved'", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([sampleApproval("a-7")]);
    render(<ApprovalQueue bots={[blankBot("bot-1", "TestBot")]} />);
    await waitFor(() => {
      expect(screen.getByTestId("approval-approve-a-7")).toBeTruthy();
    });
    fireEvent.click(screen.getByTestId("approval-approve-a-7"));
    await waitFor(() => {
      expect(approvalDecide).toHaveBeenCalledWith(
        "a-7",
        "approved",
        undefined,
      );
    });
  });

  // v3.7.13 — UX-4. Clicking a non-takeover row
  // body opens the parent's ApprovalSheet via
  // the `onSelect` callback. The per-row
  // Approve / Reject buttons bypass the sheet
  // (so the user can quick-decide without
  // waiting for the sheet to mount).
  it("clicking a row body calls onSelect with the approval and Bot", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([sampleApproval("a-3")]);
    const onSelect = vi.fn();
    render(
      <ApprovalQueue
        bots={[blankBot("bot-1", "TestBot")]}
        onSelect={onSelect}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("approval-row-a-3")).toBeTruthy();
    });
    fireEvent.click(screen.getByTestId("approval-row-a-3"));
    expect(onSelect).toHaveBeenCalledWith(
      expect.objectContaining({ id: "a-3", tool_name: "mail_send" }),
      expect.objectContaining({ id: "bot-1", name: "TestBot" }),
    );
    // The quick-action Approve was NOT fired
    // (we only clicked the row body).
    expect(approvalDecide).not.toHaveBeenCalled();
  });

  // v3.7.13 — UX-4. Clicking a per-row Approve
  // button does NOT also open the sheet (the
  // click event would otherwise bubble up to
  // the row and trigger onSelect). The
  // stopPropagation on the row-actions
  // container is the fix.
  it("clicking a per-row Approve does NOT also fire onSelect", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([sampleApproval("a-3b")]);
    const onSelect = vi.fn();
    render(
      <ApprovalQueue
        bots={[blankBot("bot-1", "TestBot")]}
        onSelect={onSelect}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("approval-approve-a-3b")).toBeTruthy();
    });
    fireEvent.click(screen.getByTestId("approval-approve-a-3b"));
    await waitFor(() => {
      expect(approvalDecide).toHaveBeenCalledWith("a-3b", "approved", undefined);
    });
    expect(onSelect).not.toHaveBeenCalled();
  });

  // v3.4.0 (Phase 5) — Takeover approvals surface
  // different action buttons (Take over + Skip)
  // and a dedicated reason line. We assert the
  // parent gets the takeover request via the
  // `onTakeoverRequested` callback, and that the
  // Skip button routes through the regular
  // `approval_decide('rejected')` path.
  it("renders Take over / Skip buttons for a Takeover approval", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([
      sampleApproval("a-tak", {
        tool_name: "__takeover__",
        reason: "2FA prompt visible",
        payload: { needs_human: "2FA prompt visible" },
      }),
    ]);
    const onTakeover = vi.fn();
    render(
      <ApprovalQueue
        bots={[blankBot("bot-1", "TestBot")]}
        onTakeoverRequested={onTakeover}
      />,
    );
    await waitFor(() => {
      expect(screen.getByTestId("approval-takeover-a-tak")).toBeTruthy();
    });
    // The row's reason line is the LLM's
    // self-reported `needs_human` string.
    expect(screen.getByTestId("approval-reason").textContent).toContain(
      "2FA prompt visible",
    );
    // No regular Approve / Reject buttons
    // on a Takeover row.
    expect(screen.queryByTestId("approval-approve-a-tak")).toBeNull();
    // Clicking "Take over" fires the parent's
    // callback with the bot_id + approval_id, NOT
    // a direct approval_decide call.
    fireEvent.click(screen.getByTestId("approval-takeover-a-tak"));
    expect(onTakeover).toHaveBeenCalledWith("bot-1", "a-tak");
    expect(approvalDecide).not.toHaveBeenCalled();
  });

  it("renders the audit-log reason line for a regular approval", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([
      sampleApproval("a-rr", {
        reason: "sending email to user@example.com — schedule change",
      }),
    ]);
    render(<ApprovalQueue bots={[blankBot("bot-1", "TestBot")]} />);
    await waitFor(() => {
      expect(screen.getByTestId("approval-approve-a-rr")).toBeTruthy();
    });
    expect(screen.getByTestId("approval-reason").textContent).toContain(
      "sending email to user@example.com",
    );
  });

  it("falls back to a generic reason copy when reason is null", async () => {
    vi.mocked(approvalList).mockResolvedValueOnce([
      sampleApproval("a-leg", { reason: null }),
    ]);
    render(<ApprovalQueue bots={[blankBot("bot-1", "TestBot")]} />);
    await waitFor(() => {
      expect(screen.getByTestId("approval-approve-a-leg")).toBeTruthy();
    });
    expect(screen.getByTestId("approval-reason").textContent).toContain(
      "approval",
    );
  });
});
