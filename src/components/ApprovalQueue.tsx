// v2.6.0 — ApprovalQueue panel.
//
// Top-level view that lists every pending approval.
// Each row shows the Bot, the tool, the JSON payload
// (truncated to 4 lines with a "show more" toggle),
// and quick-action Approve / Reject buttons.
//
// v3.7.13 — UX-4. The previous "Edit & send" inline
// modal is gone. The queue is now a navigation list:
// clicking a row's body opens the parent's
// `ApprovalSheet`, where the user can Approve, Edit
// & approve, or Deny with a proper form. The
// per-row Approve / Reject buttons stay as
// quick-action shortcuts for the common case.
//
// The "Edit & send" button was removed because the
// JSON-textarea modal forced the user to context-
// switch out of the chat to act on a pending
// approval. The sheet (a fixed panel) keeps the
// chat visible while the user decides.
//
// Takeover approvals keep their own row layout
// (Take over / Skip). The sheet doesn't apply
// to takeovers — those are handled by the parent's
// `onTakeoverRequested` callback opening the
// Computer panel.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  approvalDecide,
  approvalList,
  isTakeoverApproval,
} from "../lib/tauri";
import type { Approval, Bot } from "../lib/api";

interface ApprovalQueueProps {
  /** All Bots so we can show the Bot's name + icon
   *  next to each approval. Filter is local: a
   *  missing Bot just renders the id. */
  bots: Bot[];
  /** Optional Bot filter; when set, the queue shows
   *  only that Bot's pending approvals. */
  botId?: string;
  /** v3.4.0 (Phase 5) — fired when the user clicks
   *  "Take over" on a Takeover approval. The parent
   *  opens the Computer panel in `takeover` mode
   *  for `botId` and wires the "Hand back" button
   *  to `approvalDecide(approved)` against
   *  `approvalId`. The queue itself only surfaces
   *  the request — the parent owns the panel
   *  mount/lifecycle. */
  onTakeoverRequested?: (botId: string, approvalId: string) => void;
  /** v3.7.13 — UX-4. Fired when the user clicks a
   *  non-takeover row body. The parent (App.tsx)
   *  opens the `ApprovalSheet` with the
   *  corresponding approval. The row's per-row
   *  Approve / Reject buttons bypass this
   *  callback and decide inline. */
  onSelect?: (approval: Approval, bot: Bot | undefined) => void;
}

export function ApprovalQueue(props: ApprovalQueueProps) {
  const { bots, botId, onTakeoverRequested, onSelect } = props;
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // The id currently being decided on — used so the
  // row's buttons can show a spinner and we don't
  // double-fire on a slow network.
  const [busy, setBusy] = useState<string | null>(null);

  const botById = useMemo(() => {
    const m: Record<string, Bot> = {};
    for (const b of bots) m[b.id] = b;
    return m;
  }, [bots]);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await approvalList(botId);
      setApprovals(list);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [botId]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleDecide = useCallback(
    async (id: string, decision: "approved" | "rejected", editedArgs?: unknown) => {
      setBusy(id);
      try {
        await approvalDecide(id, decision, editedArgs);
        // Optimistically drop the row from the local
        // list; the refetch below will reconcile if
        // the Rust side put it back into a different
        // state.
        setApprovals((prev) => prev.filter((a) => a.id !== id));
        await refresh();
      } catch (e) {
        setError(`approval decision failed: ${e}`);
      } finally {
        setBusy(null);
      }
    },
    [refresh],
  );

  // v3.4.0 (Phase 5) — Take over. Delegates to
  // the parent's `onTakeoverRequested` callback,
  // which opens the Computer panel in `takeover`
  // mode. The user then drives the VM
  // interactively (solve 2FA, click a captcha,
  // etc.) and clicks "Hand back" in the Computer
  // panel to resume the Bot. The Hand back button
  // is wired by the parent to call
  // `approvalDecide(approved)` against this
  // approval id.
  const handleTakeOver = useCallback(
    (a: Approval) => {
      if (onTakeoverRequested) {
        onTakeoverRequested(a.bot_id, a.id);
      }
    },
    [onTakeoverRequested],
  );

  return (
    <div className="approval-queue" data-testid="approval-queue">
      <header className="approval-queue__header">
        <h2>Approvals</h2>
        <p className="muted small">
          {botId
            ? "Pending tool calls for this Bot."
            : "Pending tool calls across all Bots. Open the queue, decide each one, and the Bot will see the result on its next turn."}
        </p>
      </header>
      {loading && <div className="muted small">Loading…</div>}
      {error && (
        <div className="error small" role="alert">
          {error}
        </div>
      )}
      {!loading && approvals.length === 0 && (
        <div className="approval-queue__empty muted">
          No pending approvals. The queue clears as you Approve / Reject each one.
        </div>
      )}
      <ul className="approval-queue__list">
        {approvals.map((a) => {
          const bot = botById[a.bot_id];
          // v3.4.0 — a Takeover approval gets
          // different action buttons (no Approve
          // / Reject / Edit — those don't apply to a
          // "Bot is paused, please drive the VM"
          // request).
          const takeover = isTakeoverApproval(a);
          return (
            <li
              key={a.id}
              className={
                "approval-queue__row" +
                (takeover ? " approval-queue__row--takeover" : "")
              }
              data-testid={`approval-row-${a.id}`}
              // v3.7.13 — UX-4. Clicking the row
              // body opens the parent's
              // `ApprovalSheet`. Takeover rows
              // delegate to `handleTakeOver`
              // instead — they don't go through
              // the sheet. Buttons inside the row
              // call `stopPropagation` so a click
              // on Approve / Reject doesn't
              // also open the sheet.
              onClick={() => {
                if (takeover) return;
                onSelect?.(a, bot);
              }}
              role={takeover ? undefined : "button"}
              tabIndex={takeover ? -1 : 0}
              onKeyDown={(e) => {
                if (takeover) return;
                if (e.key === "Enter" || e.key === " ") {
                  e.preventDefault();
                  onSelect?.(a, bot);
                }
              }}
            >
              <div className="approval-queue__row-head">
                <span className="approval-queue__bot">
                  {bot?.icon || "🤖"}{" "}
                  <strong>{bot?.name || a.bot_id}</strong>
                </span>
                <code className="approval-queue__tool">
                  {takeover
                    ? "Take over (drive the VM)"
                    : a.tool_name}
                </code>
                <span className="muted small">
                  {new Date(a.created_at).toLocaleString()}
                </span>
              </div>
              {/* v3.4.0 — "Why this asked" reason
                surfaced inline. Falls back to a
                generic copy when the row predates
                the v3.4.0 reason column. */}
              <ReasonLine
                reason={a.reason ?? null}
                fallback={
                  takeover
                    ? "Bot needs human help driving its VM."
                    : "This tool requires your approval."
                }
              />
              <PayloadPreview payload={a.payload} />
              <div
                className="approval-queue__row-actions"
                // Stop the row's click from also
                // firing when the user clicks one
                // of the per-row quick-action
                // buttons.
                onClick={(e) => e.stopPropagation()}
              >
                {takeover ? (
                  <>
                    <button
                      className="primary small"
                      disabled={busy === a.id}
                      onClick={() => handleTakeOver(a)}
                      data-testid={`approval-takeover-${a.id}`}
                    >
                      Take over
                    </button>
                    <button
                      className="ghost small"
                      disabled={busy === a.id}
                      onClick={() => handleDecide(a.id, "rejected")}
                      data-testid={`approval-skip-${a.id}`}
                    >
                      Skip
                    </button>
                  </>
                ) : (
                  <>
                    <button
                      className="primary small"
                      disabled={busy === a.id}
                      onClick={() => handleDecide(a.id, "approved")}
                      data-testid={`approval-approve-${a.id}`}
                    >
                      Approve
                    </button>
                    <button
                      className="ghost small"
                      disabled={busy === a.id}
                      onClick={() => handleDecide(a.id, "rejected")}
                      data-testid={`approval-reject-${a.id}`}
                    >
                      Reject
                    </button>
                  </>
                )}
                {busy === a.id && (
                  <span className="muted small">deciding…</span>
                )}
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
}

// ---- v3.4.0 — Reason ("Why this asked") ----

/** Inline expansion of the approval row's reason.
 *  Surfaces the audit-log "Why this asked" line so
 *  the user can see the rationale without opening
 *  the Edit modal. Falls back to a generic copy
 *  when the row predates the v3.4.0 reason column
 *  (NULL reason). */
function ReasonLine({
  reason,
  fallback,
}: {
  reason: string | null;
  fallback: string;
}) {
  const text = reason && reason.trim().length > 0 ? reason : fallback;
  return (
    <p
      className="approval-queue__reason muted small"
      data-testid="approval-reason"
    >
      <span className="approval-queue__reason-label">Why this asked:</span>{" "}
      {text}
    </p>
  );
}

// ---- payload preview ----

/** Pretty-print a JSON payload and clip to 4 lines
 *  with a "show more" toggle. The full payload is
 *  always available in the Edit modal. */
function PayloadPreview({ payload }: { payload: unknown }) {
  const [expanded, setExpanded] = useState(false);
  const text = useMemo(() => {
    if (payload === null || payload === undefined) return "null";
    try {
      return JSON.stringify(payload, null, 2);
    } catch {
      return String(payload);
    }
  }, [payload]);
  const lines = text.split("\n");
  const clipped = lines.length > 4 && !expanded;
  const display = clipped ? lines.slice(0, 4).join("\n") + "\n…" : text;
  return (
    <pre className="approval-queue__payload">
      <code>{display}</code>
      {lines.length > 4 && (
        <button
          type="button"
          className="ghost small approval-queue__show-more"
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? "show less" : "show more"}
        </button>
      )}
    </pre>
  );
}

// ---- v3.7.13 — UX-4. Edit modal removed ----
//
// The pre-v3.7.13 `EditModal` component lived at
// the bottom of this file. It has been replaced
// by the parent's `ApprovalSheet` (see
// `src/components/ApprovalSheet.tsx`). The sheet
// is a fixed panel that opens when the user
// clicks a row body; the JSON-textarea modal
// forced a full-screen context switch, which
// was the whole point of the v3.7.13 UX
// hardening series.
