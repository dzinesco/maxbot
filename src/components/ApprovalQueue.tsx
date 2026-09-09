// v2.6.0 — ApprovalQueue panel.
//
// Top-level view that lists every pending approval.
// Each row shows the Bot, the tool, the JSON payload
// (truncated to 4 lines with a "show more" toggle),
// and three buttons: Approve, Reject, Edit & send.
//
// Approve / Reject / Edit dispatch `approval_decide`.
// On success the row animates out and the queue
// re-fetches.
//
// The "Edit & send" button opens a modal with a JSON
// textarea pre-filled with the original payload.
// The textarea has a red border when the JSON is
// invalid; the Approve button inside the modal is
// disabled until the parse succeeds.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  approvalDecide,
  approvalList,
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
}

export function ApprovalQueue(props: ApprovalQueueProps) {
  const { bots, botId } = props;
  const [approvals, setApprovals] = useState<Approval[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // The id currently being decided on — used so the
  // row's buttons can show a spinner and we don't
  // double-fire on a slow network.
  const [busy, setBusy] = useState<string | null>(null);
  // The id currently being edited — when set, the
  // Edit modal is open.
  const [editing, setEditing] = useState<Approval | null>(null);

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
        setEditing(null);
      }
    },
    [refresh],
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
          return (
            <li
              key={a.id}
              className="approval-queue__row"
              data-testid={`approval-row-${a.id}`}
            >
              <div className="approval-queue__row-head">
                <span className="approval-queue__bot">
                  {bot?.icon || "🤖"}{" "}
                  <strong>{bot?.name || a.bot_id}</strong>
                </span>
                <code className="approval-queue__tool">{a.tool_name}</code>
                <span className="muted small">
                  {new Date(a.created_at).toLocaleString()}
                </span>
              </div>
              <PayloadPreview payload={a.payload} />
              <div className="approval-queue__row-actions">
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
                <button
                  className="ghost small"
                  disabled={busy === a.id}
                  onClick={() => setEditing(a)}
                  data-testid={`approval-edit-${a.id}`}
                >
                  Edit & send
                </button>
                {busy === a.id && (
                  <span className="muted small">deciding…</span>
                )}
              </div>
            </li>
          );
        })}
      </ul>
      {editing && (
        <EditModal
          approval={editing}
          onCancel={() => setEditing(null)}
          onSubmit={(newArgs) => handleDecide(editing.id, "approved", newArgs)}
        />
      )}
    </div>
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

// ---- edit modal ----

interface EditModalProps {
  approval: Approval;
  onCancel: () => void;
  onSubmit: (newArgs: unknown) => void;
}

function EditModal({ approval, onCancel, onSubmit }: EditModalProps) {
  const [text, setText] = useState(() => {
    try {
      return JSON.stringify(approval.payload, null, 2);
    } catch {
      return "{}";
    }
  });
  const [submitting, setSubmitting] = useState(false);

  // Live-validate. `null` means "valid"; otherwise the
  // parse error message renders below the textarea.
  const parseError = useMemo(() => {
    if (text.trim() === "") return "payload is empty";
    try {
      JSON.parse(text);
      return null;
    } catch (e) {
      return String(e);
    }
  }, [text]);

  const handleSubmit = useCallback(async () => {
    if (parseError) return;
    setSubmitting(true);
    try {
      onSubmit(JSON.parse(text));
    } finally {
      setSubmitting(false);
    }
  }, [parseError, text, onSubmit]);

  return (
    <div
      className="modal-backdrop"
      onClick={onCancel}
      data-testid="approval-edit-modal"
    >
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="Edit approval payload"
      >
        <header className="modal-header">
          <h2>Edit & send: {approval.tool_name}</h2>
          <button className="ghost small" onClick={onCancel} aria-label="Close">
            ✕
          </button>
        </header>
        <div className="modal-body">
          <p className="muted small">
            Rewrite the tool's arguments as JSON, then
            click Approve. The Bot will see the edited
            result on its next turn.
          </p>
          <textarea
            className={`approval-queue__edit-area${
              parseError ? " approval-queue__edit-area--invalid" : ""
            }`}
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={14}
            spellCheck={false}
            data-testid="approval-edit-textarea"
          />
          {parseError && (
            <p className="error small" role="alert">
              {parseError}
            </p>
          )}
        </div>
        <footer className="modal-footer">
          <button className="ghost small" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="primary small"
            disabled={parseError !== null || submitting}
            onClick={handleSubmit}
            data-testid="approval-edit-submit"
          >
            Approve with edited args
          </button>
        </footer>
      </div>
    </div>
  );
}
