import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent, act } from "@testing-library/react";
import { ApprovalSheet } from "./ApprovalSheet";
import { approvalDecide } from "../lib/tauri";
import type { Approval, Bot } from "../lib/api";

vi.mock("../lib/tauri", () => ({
  approvalDecide: vi.fn().mockResolvedValue({
    approval: { id: "ap-1", status: "approved" },
    tool_result: { kind: "approved", payload: {}, summary: "ok" },
    tool_result_string: "ok",
  }),
}));

const makeBot = (overrides: Partial<Bot> = {}): Bot => ({
  id: "bot-1",
  name: "Robot",
  description: "",
  system_prompt: "",
  default_model: "MiniMax-M3",
  allowed_tools: [],
  icon: "🤖",
  color: "",
  avatar_color: "",
  last_active_at: null,
  state: "idle",
  ...overrides,
});

const makeApproval = (overrides: Partial<Approval> = {}): Approval => ({
  id: "ap-1",
  bot_id: "bot-1",
  tool_name: "mail_draft",
  payload: { to: "x@y.com", subject: "Hi", body: "Hello" },
  status: "pending",
  result: null,
  tool_call_id: null,
  created_at: "2026-01-01T00:00:00Z",
  decided_at: null,
  reason: null,
  ...overrides,
});

describe("ApprovalSheet", () => {
  beforeEach(() => {
    vi.mocked(approvalDecide).mockReset();
    vi.mocked(approvalDecide).mockResolvedValue({
      approval: { id: "ap-1", status: "approved" } as Approval,
      tool_result: { kind: "approved", payload: {}, summary: "ok" },
      tool_result_string: "ok",
    });
  });

  it("renders nothing when sheet is null", () => {
    const { container } = render(
      <ApprovalSheet sheet={null} onClose={() => {}} onDecided={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("shows the Bot name + tool name in the header", () => {
    render(
      <ApprovalSheet
        sheet={{ approval: makeApproval(), bot: makeBot({ name: "Robot" }) }}
        onClose={() => {}}
        onDecided={() => {}}
      />,
    );
    expect(screen.getByTestId("approval-sheet-bot-name").textContent).toBe(
      "Robot",
    );
    expect(screen.getByTestId("approval-sheet-tool-name").textContent).toBe(
      "mail_draft",
    );
  });

  it("renders the form fields for `mail_draft`", () => {
    render(
      <ApprovalSheet
        sheet={{
          approval: makeApproval({
            payload: { to: "x@y.com", subject: "Hi", body: "Hello" },
          }),
          bot: makeBot(),
        }}
        onClose={() => {}}
        onDecided={() => {}}
      />,
    );
    expect(screen.getByTestId("tool-call-form")).toBeInTheDocument();
    expect(screen.getByTestId("tool-call-form-to")).toBeInTheDocument();
    expect(screen.getByTestId("tool-call-form-subject")).toBeInTheDocument();
    expect(screen.getByTestId("tool-call-form-body")).toBeInTheDocument();
  });

  it("calls approvalDecide('approved') on Approve", async () => {
    const onDecided = vi.fn();
    render(
      <ApprovalSheet
        sheet={{ approval: makeApproval(), bot: makeBot() }}
        onClose={() => {}}
        onDecided={onDecided}
      />,
    );
    fireEvent.click(screen.getByTestId("approval-sheet-approve"));
    await waitFor(() => {
      expect(vi.mocked(approvalDecide)).toHaveBeenCalledWith(
        "ap-1",
        "approved",
        undefined,
      );
    });
    expect(onDecided).toHaveBeenCalledWith("ap-1");
  });

  it("calls approvalDecide('rejected') on Deny", async () => {
    const onDecided = vi.fn();
    render(
      <ApprovalSheet
        sheet={{ approval: makeApproval(), bot: makeBot() }}
        onClose={() => {}}
        onDecided={onDecided}
      />,
    );
    fireEvent.click(screen.getByTestId("approval-sheet-deny"));
    await waitFor(() => {
      expect(vi.mocked(approvalDecide)).toHaveBeenCalledWith(
        "ap-1",
        "rejected",
        { reason: "denied from sheet" },
      );
    });
    expect(onDecided).toHaveBeenCalledWith("ap-1");
  });

  it("toggles into edit mode on 'Edit & approve' and commits edited args", async () => {
    const onDecided = vi.fn();
    render(
      <ApprovalSheet
        sheet={{
          approval: makeApproval({
            payload: { to: "x@y.com", subject: "Hi", body: "Hello" },
          }),
          bot: makeBot(),
        }}
        onClose={() => {}}
        onDecided={onDecided}
      />,
    );
    // Click "Edit & approve" — the form
    // becomes editable (its own approve/cancel
    // buttons appear) and the sheet's toggle
    // is hidden in favor of a "Cancel edit"
    // exit.
    fireEvent.click(screen.getByTestId("approval-sheet-edit-toggle"));
    expect(screen.queryByTestId("approval-sheet-edit-toggle")).toBeNull();
    expect(screen.getByTestId("tool-call-form-approve")).toBeInTheDocument();
    // Edit the `to` field, then click the
    // form's Approve — that's the one that
    // commits the edited values.
    const toInput = screen.getByTestId(
      "tool-call-form-to",
    ) as HTMLInputElement;
    fireEvent.change(toInput, { target: { value: "edited@z.com" } });
    fireEvent.click(screen.getByTestId("tool-call-form-approve"));
    await waitFor(() => {
      expect(vi.mocked(approvalDecide)).toHaveBeenCalledWith(
        "ap-1",
        "edited",
        expect.objectContaining({ to: "edited@z.com" }),
      );
    });
  });

  it("calls onClose when the × button is clicked", () => {
    const onClose = vi.fn();
    render(
      <ApprovalSheet
        sheet={{ approval: makeApproval(), bot: makeBot() }}
        onClose={onClose}
        onDecided={() => {}}
      />,
    );
    fireEvent.click(screen.getByTestId("approval-sheet-close"));
    expect(onClose).toHaveBeenCalled();
  });

  it("calls onClose when Escape is pressed", () => {
    const onClose = vi.fn();
    render(
      <ApprovalSheet
        sheet={{ approval: makeApproval(), bot: makeBot() }}
        onClose={onClose}
        onDecided={() => {}}
      />,
    );
    act(() => {
      fireEvent.keyDown(window, { key: "Escape" });
    });
    expect(onClose).toHaveBeenCalled();
  });

  it("renders the form for `calendar_event_create`", () => {
    render(
      <ApprovalSheet
        sheet={{
          approval: makeApproval({
            tool_name: "calendar_event_create",
            payload: { title: "Standup", starts: "2026-01-01T10:00" },
          }),
          bot: makeBot(),
        }}
        onClose={() => {}}
        onDecided={() => {}}
      />,
    );
    const form = screen.getByTestId("tool-call-form");
    expect(form.dataset.tool).toBe("calendar_event_create");
    expect(screen.getByTestId("tool-call-form-title")).toBeInTheDocument();
    expect(screen.getByTestId("tool-call-form-starts")).toBeInTheDocument();
  });

  it("shows an error message when approvalDecide fails", async () => {
    vi.mocked(approvalDecide).mockRejectedValueOnce(new Error("boom"));
    render(
      <ApprovalSheet
        sheet={{ approval: makeApproval(), bot: makeBot() }}
        onClose={() => {}}
        onDecided={() => {}}
      />,
    );
    fireEvent.click(screen.getByTestId("approval-sheet-deny"));
    await waitFor(() => {
      expect(screen.getByTestId("approval-sheet-error").textContent).toBe(
        "boom",
      );
    });
  });
});
