// v3.7.13 — UX-4. Approval sheet.
//
// The pre-v3.7.13 approval flow put an inline
// Edit modal on the ApprovalQueue. The modal
// blocked the chat, so users had to context-
// switch out of the conversation to act on a
// pending approval. v3.7.13 replaces the modal
// with a sheet: a fixed panel that lives at the
// edge of the viewport and leaves the chat
// visible behind it.
//
// Layout:
//   - viewport width < 1024px  → bottom panel,
//     220px tall, slides up from the bottom
//   - viewport width ≥ 1024px  → right-side
//     drawer, 340px wide, full viewport height
//
// The sheet is mutually exclusive with
// `takeoverPanel` — App.tsx suppresses the sheet
// while a takeover is in flight. The sheet is
// driven by `approvalSheet` state in App.tsx;
// clicking a row in `ApprovalQueue` sets that
// state to `{ approval, bot }` and the sheet
// appears. Approve / Edit & approve / Deny fire
// `approvalDecide`; the sheet clears itself
// when the call resolves.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { Approval, Bot } from "../lib/api";
import { approvalDecide } from "../lib/tauri";
import { ToolCallForm } from "./MessageBubble";

export interface ApprovalSheetProps {
  /** The approval the user clicked on, plus the
   *  Bot that owns it. `null` means the sheet
   *  is closed. */
  sheet: { approval: Approval; bot: Bot } | null;
  /** Called when the sheet is dismissed without
   *  a decision (the × button, Escape, or
   *  outside-click). */
  onClose: () => void;
  /** Called after a successful decision
   *  (Approve / Edit & approve / Deny). Used
   *  to remove the row from the queue and
   *  clear the sheet state. */
  onDecided: (approvalId: string) => void;
}

/** Decide which CSS class the sheet should use
 *  based on the current viewport width. Called
 *  in the render so the layout is reactive to
 *  window resizes. */
function useResponsivePlacement() {
  const [isWide, setIsWide] = useState(() => {
    if (typeof window === "undefined") return false;
    return window.innerWidth >= 1024;
  });
  useEffect(() => {
    const onResize = () => setIsWide(window.innerWidth >= 1024);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  return isWide;
}

export function ApprovalSheet({
  sheet,
  onClose,
  onDecided,
}: ApprovalSheetProps) {
  const isWide = useResponsivePlacement();
  // Escape closes the sheet — the same
  // convention the inline Edit modal used.
  useEffect(() => {
    if (!sheet) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [sheet, onClose]);

  // Local busy + error state for the
  // approve / edit / deny actions. The
  // buttons disable while a decision is in
  // flight and re-enable after `approvalDecide`
  // resolves.
  const [busy, setBusy] = useState<null | "approve" | "edit" | "deny">(null);
  const [error, setError] = useState<string | null>(null);
  // The "edit" mode is a toggle inside the
  // sheet: clicking "Edit & approve" flips the
  // form into editable mode, the user tweaks,
  // then clicks "Approve" to commit. We keep
  // the form values here so the edit
  // survives a re-render.
  const [editing, setEditing] = useState(false);
  const [editedArgs, setEditedArgs] = useState<Record<string, unknown>>({});

  // Reset local state when the sheet is
  // re-opened for a different approval (e.g. the
  // user closes it and clicks another row).
  useEffect(() => {
    if (sheet) {
      setBusy(null);
      setError(null);
      setEditing(false);
      // The initial edited args are the
      // approval's original payload, so
      // "Edit & approve" without changes
      // behaves like a plain approve.
      setEditedArgs(
        (sheet.approval.payload as Record<string, unknown>) ?? {},
      );
    }
  }, [sheet?.approval.id]);

  const handleApprove = useCallback(
    async (
      decision: "approved" | "rejected" | "edited",
      // `args` is the live edited values for
      // the "edited" decision. We accept it as
      // an argument (instead of reading
      // `editedArgs` from state) because the
      // form's onApprove callback fires before
      // React's setState has flushed the new
      // edited args — using state would race
      // the click and commit the OLD values.
      args?: Record<string, unknown>,
    ) => {
      if (!sheet) return;
      setBusy(decision === "approved" ? "approve" : decision === "edited" ? "edit" : "deny");
      setError(null);
      try {
        if (decision === "approved") {
          await approvalDecide(sheet.approval.id, "approved", undefined);
        } else if (decision === "rejected") {
          await approvalDecide(sheet.approval.id, "rejected", {
            reason: "denied from sheet",
          });
        } else {
          await approvalDecide(
            sheet.approval.id,
            "edited",
            args ?? editedArgs,
          );
        }
        onDecided(sheet.approval.id);
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setBusy(null);
      }
    },
    [sheet, editedArgs, onDecided],
  );

  const formData = useMemo(() => {
    if (!sheet) return null;
    // The approval's `payload` is a Record of
    // arbitrary args. The form expects a
    // string-keyed object — same shape, so we
    // just pass it through.
    const payload = sheet.approval.payload as Record<string, unknown>;
    return {
      toolName: sheet.approval.tool_name,
      args: payload,
    };
  }, [sheet]);

  if (!sheet) return null;
  const placement = isWide ? "right" : "bottom";

  return (
    <div
      className={`approval-sheet approval-sheet--${placement}`}
      data-testid="approval-sheet"
      data-placement={placement}
      role="dialog"
      aria-label={`Approval for ${sheet.bot.name || "Bot"}`}
    >
      <header className="approval-sheet__header">
        <div className="approval-sheet__title">
          <span
            className="approval-sheet__bot-icon"
            aria-hidden
            data-testid="approval-sheet-bot-icon"
          >
            {sheet.bot.icon || "🤖"}
          </span>
          <div className="approval-sheet__title-text">
            <div
              className="approval-sheet__bot-name"
              data-testid="approval-sheet-bot-name"
            >
              {sheet.bot.name || "Unnamed Bot"}
            </div>
            <div
              className="approval-sheet__tool"
              data-testid="approval-sheet-tool-name"
            >
              {sheet.approval.tool_name}
            </div>
          </div>
        </div>
        <button
          type="button"
          className="approval-sheet__close"
          data-testid="approval-sheet-close"
          onClick={onClose}
          aria-label="Close approval sheet"
        >
          ×
        </button>
      </header>

      {formData && (
        <div className="approval-sheet__body">
          <ToolCallForm
            toolName={formData.toolName}
            args={formData.args}
            approvalId={editing ? sheet.approval.id : null}
            onApprove={(args) => {
              // Persist the edited values
              // (so the next "Edit & approve"
              // round opens with the user's
              // last edits, not the model's
              // original) and commit. The
              // form's local `values` is the
              // source of truth here — we
              // pass `args` directly to
              // `handleApprove` so the
              // commit doesn't race React's
              // setState flush.
              setEditedArgs(args);
              handleApprove("edited", args);
            }}
            onCancel={() => setEditing(false)}
          />
        </div>
      )}

      {error && (
        <div
          className="approval-sheet__error"
          data-testid="approval-sheet-error"
        >
          {error}
        </div>
      )}

      <footer className="approval-sheet__footer">
        {!editing && (
          <button
            type="button"
            className="approval-sheet__deny"
            data-testid="approval-sheet-deny"
            onClick={() => handleApprove("rejected")}
            disabled={busy !== null}
          >
            Deny
          </button>
        )}
        {editing ? (
          // In edit mode, the form's own
          // approve / cancel buttons do the
          // work; the sheet's footer just
          // offers a way to bail out of edit
          // mode without committing. The
          // form's "Approve" commits with
          // the edited values via its
          // `onApprove` callback (which
          // calls `setEditedArgs` +
          // `handleApprove("edited")`).
          <button
            type="button"
            className="approval-sheet__edit"
            data-testid="approval-sheet-edit-cancel"
            onClick={() => setEditing(false)}
            disabled={busy !== null}
          >
            Cancel edit
          </button>
        ) : (
          <>
            <button
              type="button"
              className="approval-sheet__edit"
              data-testid="approval-sheet-edit-toggle"
              onClick={() => setEditing(true)}
              disabled={busy !== null}
            >
              Edit & approve
            </button>
            <button
              type="button"
              className="approval-sheet__approve"
              data-testid="approval-sheet-approve"
              onClick={() => handleApprove("approved")}
              disabled={busy !== null}
            >
              {busy === "approve" ? "Approving…" : "Approve"}
            </button>
          </>
        )}
      </footer>
    </div>
  );
}
