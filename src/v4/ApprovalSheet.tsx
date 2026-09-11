/*
 * v4 — ApprovalSheet (S3)
 *
 * Auto-surfaces the first pending approval for the bot on mount
 * and on window focus. No interval — only event-driven refresh.
 *
 * Per the S3 brief:
 *   - Sheet only when listApprovals returns a pending one for
 *     this bot. If `botId` is null or the list is empty / all
 *     decided, the sheet renders nothing.
 *   - Fetch on botId change AND on window focus (via
 *     `useFocusRefresh`). No setInterval anywhere.
 *   - Approve / Deny only. The editable form is parked — S3
 *     keeps the surface thin so the leak stays fixed.
 *
 * What this sheet does NOT do (yet):
 *   - No payload editing ("Edit & approve"). S3 ships read-only.
 *   - No multi-row queue. The brief is "sheet only when one
 *     pending approval exists for this bot" — show one, decide
 *     one, re-fetch.
 *   - No takeover-specific buttons. Takeover approvals surface
 *     with the same Approve / Deny — the legacy sheet's takeover
 *     swap is parked for a follow-up slice.
 *
 * Hard rules (v4):
 *   - No listeners mounted while botId is null.
 *   - Window focus listener paired with removal in cleanup.
 *   - No JSON in state beyond the small fields we render
 *     (`tool_name`, the truncated payload preview). The full
 *     payload stays on the server until decide time.
 */

import { useCallback, useEffect, useState } from "react";
import type { Approval } from "../lib/api";
import { approvalDecide, approvalList } from "../lib/tauri";
import { useFocusRefresh } from "./useFocusRefresh";
import "./styles/approvals.css";

export interface ApprovalSheetProps {
  /** The bot id to filter approvals by. When null, the sheet is
   *  fully hidden — no fetch, no render. */
  botId: string | null;
}

function previewPayload(payload: unknown): string {
  // Render only a short, safe summary of the tool args — never
  // the full JSON. Truncate aggressively; this is a glance,
  // not a debugger.
  if (payload === null || payload === undefined) return "(no args)";
  let flat: string;
  try {
    flat = typeof payload === "string" ? payload : JSON.stringify(payload);
  } catch {
    flat = String(payload);
  }
  flat = flat.replace(/\s+/g, " ").trim();
  if (flat.length <= 100) return flat;
  return flat.slice(0, 100) + "…";
}

export function ApprovalSheet({ botId }: ApprovalSheetProps) {
  const [pending, setPending] = useState<Approval | null>(null);
  const [busy, setBusy] = useState<null | "approve" | "deny">(null);
  const [error, setError] = useState<string | null>(null);

  // The fetch: keyed on botId. Cancelled on unmount / re-key.
  // No setInterval — the same callback fires on window:focus.
  const refresh = useCallback(() => {
    if (!botId) return;
    approvalList(botId)
      .then((all) => {
        const next = all.find((a) => a.status === "pending") ?? null;
        setPending(next);
      })
      .catch((e) => {
        setError(`Could not load approvals: ${e}`);
      });
  }, [botId]);

  // Re-fetch on botId change + on mount.
  useEffect(() => {
    if (!botId) {
      setPending(null);
      setError(null);
      return;
    }
    let cancelled = false;
    approvalList(botId)
      .then((all) => {
        if (cancelled) return;
        const next = all.find((a) => a.status === "pending") ?? null;
        setPending(next);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(`Could not load approvals: ${e}`);
      });
    return () => {
      cancelled = true;
    };
  }, [botId]);

  // Re-fetch on window focus — same callback, no timer.
  useFocusRefresh(refresh);

  const handleApprove = useCallback(async () => {
    if (!pending || busy) return;
    setBusy("approve");
    setError(null);
    try {
      await approvalDecide(pending.id, "approved");
      // Clear + re-fetch so the next pending (if any) shows up.
      setPending(null);
      refresh();
    } catch (e) {
      setError(`Approve failed: ${e}`);
    } finally {
      setBusy(null);
    }
  }, [pending, busy, refresh]);

  const handleDeny = useCallback(async () => {
    if (!pending || busy) return;
    setBusy("deny");
    setError(null);
    try {
      await approvalDecide(pending.id, "rejected", {
        reason: "denied from sheet",
      });
      setPending(null);
      refresh();
    } catch (e) {
      setError(`Deny failed: ${e}`);
    } finally {
      setBusy(null);
    }
  }, [pending, busy, refresh]);

  if (!botId || !pending) return null;
  // The server (commands/chat.rs → enqueue_if_ask_rule) is the
  // single source of truth on which tools get an approval row.
  // Per S3a-real: the bot's per-instance `allowed_tools` list
  // is the gate. The renderer just renders what the server
  // sends.

  return (
    <div
      className="v4-approval-sheet"
      role="dialog"
      aria-label={`Approval required for ${pending.tool_name}`}
      data-testid="v4-approval-sheet"
    >
      <div className="v4-approval-sheet-header">
        <span className="v4-approval-sheet-tool">
          {pending.tool_name}
        </span>
        <span className="v4-approval-sheet-when">
          {new Date(pending.created_at).toLocaleTimeString()}
        </span>
      </div>
      {pending.reason && (
        <div className="v4-approval-sheet-reason">
          {pending.reason}
        </div>
      )}
      <div className="v4-approval-sheet-args">
        {previewPayload(pending.payload)}
      </div>
      {error && (
        <div className="v4-approval-sheet-error" role="alert">
          {error}
        </div>
      )}
      <div className="v4-approval-sheet-actions">
        <button
          type="button"
          className="v4-approval-sheet-deny danger small"
          onClick={handleDeny}
          disabled={busy !== null}
          data-testid="v4-approval-sheet-deny"
        >
          {busy === "deny" ? "Denying…" : "Deny"}
        </button>
        <button
          type="button"
          className="v4-approval-sheet-approve primary small"
          onClick={handleApprove}
          disabled={busy !== null}
          data-testid="v4-approval-sheet-approve"
        >
          {busy === "approve" ? "Approving…" : "Approve"}
        </button>
      </div>
    </div>
  );
}
