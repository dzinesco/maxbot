// v3.7.17 Slice 2 — UI surface for `maxbot_loopd`.
//
// Mounted in the Sidebar above the footer. Polls
// `loopdStatus` / `loopdReadTask` / `loopdReadJournal` every
// 2 seconds and shows the supervisor's current state.
//
// The component is **read-only with respect to the loop's
// data files**. The only writes are lifecycle calls
// (`loopdStart` / `loopdStop`) — both of which spawn or kill
// `maxbot_loopd`. Every actual file write (STATE.json,
// TASK.md, journal, MEMORY.json) goes through the
// supervisor's `atomic_write` path; this component never
// touches those files directly. Per Tyler's Slice 2 brief.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  loopdReadJournal,
  loopdReadTask,
  loopdStart,
  loopdStatus,
  loopdStop,
} from "../lib/tauri";
import type { LoopdJournal, LoopdStatus, LoopdTask } from "../lib/api";

/** Poll interval for the three IPC calls. 2s matches the
 *  supervisor's `--heartbeat-secs` default, so a turn
 *  starting in the supervisor shows up in the UI within
 *  one poll cycle. Tighter than 2s would burn CPU for
 *  little gain; looser would feel laggy to the user. */
const POLL_INTERVAL_MS = 2000;

/** Status text shown next to the indicator dot. Kept
 *  short — the Sidebar is ~280px wide and the panel
 *  doesn't get its own row. */
function statusLabel(s: LoopdStatus | null): {
  text: string;
  tone: "alive" | "dead" | "idle" | "unknown";
} {
  if (!s) return { text: "Loop: …", tone: "unknown" };
  if (s.state === "alive") {
    return { text: `Loop: alive (pid ${s.pid ?? "?"})`, tone: "alive" };
  }
  if (s.state === "dead") {
    return { text: "Loop: dead (offer Start)", tone: "dead" };
  }
  return { text: "Loop: idle (no STATE.json)", tone: "idle" };
}

function truncateBody(body: string, max = 240): string {
  if (body.length <= max) return body;
  return body.slice(0, max).trimEnd() + "…";
}

export function LoopPanel() {
  const [status, setStatus] = useState<LoopdStatus | null>(null);
  const [task, setTask] = useState<LoopdTask | null>(null);
  const [journal, setJournal] = useState<LoopdJournal | null>(null);
  const [busy, setBusy] = useState<"start" | "stop" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Mounted ref prevents a stale poll from setting state after
  // unmount (the 2s poll can outlive the component if the user
  // quits while a poll is in flight).
  const mounted = useRef(true);

  const poll = useCallback(async () => {
    if (!mounted.current) return;
    try {
      const [s, t, j] = await Promise.all([
        loopdStatus(),
        loopdReadTask(),
        loopdReadJournal(),
      ]);
      if (!mounted.current) return;
      setStatus(s);
      setTask(t);
      setJournal(j);
      setError(null);
    } catch (e) {
      if (!mounted.current) return;
      // Surface the error once; subsequent polls clear it.
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    poll();
    const id = window.setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      mounted.current = false;
      window.clearInterval(id);
    };
  }, [poll]);

  const handleStart = useCallback(async () => {
    setBusy("start");
    setError(null);
    try {
      const s = await loopdStart();
      if (mounted.current) setStatus(s);
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setBusy(null);
    }
  }, []);

  const handleStop = useCallback(async () => {
    setBusy("stop");
    setError(null);
    try {
      const s = await loopdStop();
      if (mounted.current) setStatus(s);
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setBusy(null);
    }
  }, []);

  const label = statusLabel(status);
  const showStart = status?.state !== "alive";
  const showStop = status?.state === "alive";

  return (
    <section className="loop-panel" aria-label="Loop supervisor">
      <header className="loop-panel-header">
        <span
          className={`loop-panel-indicator loop-panel-indicator--${label.tone}`}
          data-state={label.tone}
          aria-hidden
        />
        <span className="loop-panel-status" title={label.text}>
          {label.text}
        </span>
        {showStart && (
          <button
            type="button"
            className="ghost small loop-panel-btn"
            onClick={handleStart}
            disabled={busy !== null}
            aria-label="Start maxbot_loopd"
            title="Start maxbot_loopd"
          >
            {busy === "start" ? "Starting…" : "Start"}
          </button>
        )}
        {showStop && (
          <button
            type="button"
            className="ghost small loop-panel-btn"
            onClick={handleStop}
            disabled={busy !== null}
            aria-label="Stop maxbot_loopd"
            title="Stop maxbot_loopd (SIGTERM, then SIGKILL)"
          >
            {busy === "stop" ? "Stopping…" : "Stop"}
          </button>
        )}
      </header>

      {/* TASK.md preview. `exists=false` means the supervisor
          has never written one — common on a fresh install or
          before the first turn. */}
      {task && task.exists && (
        <div className="loop-panel-task" title={task.updated_at}>
          <span className={`loop-panel-pill loop-panel-pill--${task.status || "unknown"}`}>
            {task.status || "unknown"}
          </span>
          <span className="loop-panel-task-body">
            {truncateBody(task.body)}
          </span>
        </div>
      )}
      {task && !task.exists && (
        <div className="loop-panel-task loop-panel-task--empty">
          TASK.md: not yet written
        </div>
      )}

      {/* Last journal heading + actions. The supervisor's
          journal is append-only (plain `O_APPEND`); reading it
          from the UI is fine because nothing on the renderer
          side touches the journal file. */}
      {journal && journal.exists && journal.last_heading && (
        <div className="loop-panel-journal">
          <div className="loop-panel-journal-heading">
            {journal.last_heading}
          </div>
          {journal.last_actions.length > 0 && (
            <ul className="loop-panel-journal-actions">
              {journal.last_actions.slice(0, 4).map((a, i) => (
                <li key={i}>{a}</li>
              ))}
            </ul>
          )}
          {journal.last_note && (
            <div className="loop-panel-journal-note">{journal.last_note}</div>
          )}
        </div>
      )}
      {journal && !journal.exists && (
        <div className="loop-panel-journal loop-panel-journal--empty">
          No journal entries yet
        </div>
      )}

      {error && (
        <div className="loop-panel-error" role="alert">
          {error}
        </div>
      )}
    </section>
  );
}
