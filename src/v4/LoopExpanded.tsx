/*
 * v4 — LoopExpanded
 *
 * Full task body + last journal heading for `maxbot_loopd`.
 * Mounts only when the LoopChip is expanded. Fetches on
 * mount + on Refresh click. No setInterval.
 *
 * Per Tyler's v4 hard rules: "no setInterval unless surface
 * visible" — this surface is only visible when expanded, and
 * the user can refresh on demand.
 */

import { useEffect, useState } from "react";
import { loopdReadJournal, loopdReadTask } from "../lib/tauri";
import type { LoopdJournal, LoopdTask } from "../lib/api";

function truncateBody(body: string, max = 240): string {
  if (body.length <= max) return body;
  return body.slice(0, max).trimEnd() + "…";
}

export interface LoopExpandedProps {
  /** True while this panel should be mounted + visible. */
  open: boolean;
  /** Start the loop. Wired by App.tsx (parked in S2.6 — chip is
   *  text-only in the rail, action lives here in the expanded
   *  panel for a future slice that re-adds it). */
  onStart?: () => void | Promise<void>;
  /** Stop the loop. Same parking as `onStart`. */
  onStop?: () => void | Promise<void>;
}

export function LoopExpanded({ open, onStart, onStop }: LoopExpandedProps) {
  const [task, setTask] = useState<LoopdTask | null>(null);
  const [journal, setJournal] = useState<LoopdJournal | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Track whether we have a refresh pending, so the button can
  // disable while the IPC is in flight without flickering the body.
  const [refreshedOnce, setRefreshedOnce] = useState(false);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setBusy(true);
    setError(null);
    Promise.all([loopdReadTask(), loopdReadJournal()])
      .then(([t, j]) => {
        if (cancelled) return;
        setTask(t);
        setJournal(j);
        setRefreshedOnce(true);
      })
      .catch((e) => {
        if (cancelled) return;
        setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  if (!open) return null;

  return (
    <div className="v4-loop-expanded" role="region" aria-label="Loop task + journal">
      <div className="v4-loop-expanded-header">
        <span className="v4-loop-expanded-title">TASK.md + last journal</span>
        <button
          type="button"
          className="v4-loop-expanded-refresh ghost small"
          onClick={() => {
            // Inline refresh — re-trigger the same effect by forcing
            // a remount via a key bump on the section. Simpler: call
            // the IPCs directly here.
            let cancelled = false;
            setBusy(true);
            setError(null);
            Promise.all([loopdReadTask(), loopdReadJournal()])
              .then(([t, j]) => {
                if (cancelled) return;
                setTask(t);
                setJournal(j);
                setRefreshedOnce(true);
              })
              .catch((e) => {
                if (cancelled) return;
                setError(String(e));
              })
              .finally(() => {
                if (!cancelled) setBusy(false);
              });
            return () => {
              cancelled = true;
            };
          }}
          disabled={busy}
        >
          {busy ? "…" : "Refresh"}
        </button>
      </div>

      {error && (
        <div className="v4-loop-expanded-error" role="alert">
          {error}
        </div>
      )}

      {!refreshedOnce && !error && (
        <div className="v4-loop-expanded-empty">Loading…</div>
      )}

      {task && (
        <div className="v4-loop-expanded-task">
          <span className={`v4-loop-expanded-pill v4-loop-expanded-pill--${task.status || "unknown"}`}>
            {task.status || "unknown"}
          </span>
          <span className="v4-loop-expanded-task-body">
            {task.exists ? truncateBody(task.body) : "(no TASK.md yet)"}
          </span>
        </div>
      )}

      {journal && journal.exists && journal.last_heading && (
        <div className="v4-loop-expanded-journal">
          <div className="v4-loop-expanded-journal-heading">
            {journal.last_heading}
          </div>
          {journal.last_actions.length > 0 && (
            <ul className="v4-loop-expanded-journal-actions">
              {journal.last_actions.slice(0, 4).map((a, i) => (
                <li key={i}>{a}</li>
              ))}
            </ul>
          )}
          {journal.last_note && (
            <div className="v4-loop-expanded-journal-note">{journal.last_note}</div>
          )}
        </div>
      )}
      {journal && !journal.exists && (
        <div className="v4-loop-expanded-journal v4-loop-expanded-journal--empty">
          No journal entries yet
        </div>
      )}
    </div>
  );
}
