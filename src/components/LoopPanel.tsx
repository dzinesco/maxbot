// v3.7.17 Slice 3 — UI surface for `maxbot_loopd`.
//
// Mounted in the Sidebar above the footer.
//
// Two IPC surfaces, two cadences:
//
// 1. POLLING (every 2s): `loopdStatus` returns ONLY the
//    minimal shape (pid, state, task_status, task_excerpt_len,
//    last_heartbeat, age_secs). Cheap — drives the status pill
//    + task-status badge + "task changed since last Read" hint.
//
// 2. EXPLICIT READ (mount + manual Refresh button):
//    `loopdReadTask` + `loopdReadJournal` return the full TASK.md
//    body and the last journal heading/actions/note. These are
//    KB-sized markdown that doesn't change every 2s.
//
// Per Tyler's Slice 3 brief: "LoopPanel 2s poll returns ONLY
// { pid, state, task_status, task_excerpt_len }. Full TASK.md /
// journal only on explicit Read, not every tick."
//
// The component is **read-only with respect to the loop's data
// files**. The only writes are lifecycle calls (`loopdStart` /
// `loopdStop`) — both of which spawn or kill `maxbot_loopd`.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  loopdReadJournal,
  loopdReadTask,
  loopdStart,
  loopdStatus,
  loopdStop,
} from "../lib/tauri";
import type { LoopdJournal, LoopdStatus, LoopdTask } from "../lib/api";

/** Poll interval for the minimal `loopdStatus` IPC call. 2s
 *  matches the supervisor's `--heartbeat-secs` default, so a
 *  turn starting in the supervisor shows up in the UI within
 *  one poll cycle. Tighter would burn CPU for little gain;
 *  looser would feel laggy. */
const POLL_INTERVAL_MS = 2000;

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
  // Minimal polling state — updated every 2s.
  const [status, setStatus] = useState<LoopdStatus | null>(null);
  // Full content state — updated on explicit Read (mount + manual Refresh).
  const [task, setTask] = useState<LoopdTask | null>(null);
  const [journal, setJournal] = useState<LoopdJournal | null>(null);
  const [busy, setBusy] = useState<"start" | "stop" | "refresh" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // taskLen at last explicit Read. The poll's task_excerpt_len is
  // compared against this to surface "task changed — refresh to see".
  const [lastReadTaskLen, setLastReadTaskLen] = useState<number | null>(null);
  const mounted = useRef(true);

  const readFull = useCallback(async () => {
    if (!mounted.current) return;
    setBusy("refresh");
    setError(null);
    try {
      const [t, j] = await Promise.all([loopdReadTask(), loopdReadJournal()]);
      if (!mounted.current) return;
      setTask(t);
      setJournal(j);
      setLastReadTaskLen(t.exists ? t.body.length : 0);
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setBusy(null);
    }
  }, []);

  const poll = useCallback(async () => {
    if (!mounted.current) return;
    try {
      const s = await loopdStatus();
      if (!mounted.current) return;
      setStatus(s);
      setError(null);
    } catch (e) {
      if (mounted.current) setError(String(e));
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    // First paint: poll the minimal state AND read the full content.
    // After that, poll keeps going; full content only updates on
    // explicit Read (Refresh button).
    poll();
    readFull();
    const id = window.setInterval(poll, POLL_INTERVAL_MS);
    return () => {
      mounted.current = false;
      window.clearInterval(id);
    };
  }, [poll, readFull]);

  const handleStart = useCallback(async () => {
    setBusy("start");
    setError(null);
    try {
      const s = await loopdStart();
      if (mounted.current) {
        setStatus(s);
        // Refresh full content too — daemon just started and may
        // have written TASK.md / journal immediately.
        await readFull();
      }
    } catch (e) {
      if (mounted.current) setError(String(e));
    } finally {
      if (mounted.current) setBusy(null);
    }
  }, [readFull]);

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
  // "Task changed since last Read" hint. Cheap comparison — the
  // poll's task_excerpt_len (byte count) vs the readFull's
  // task.body.length (char count). They diverge by ±1 for non-ASCII
  // bodies, but the user just wants to know "did something change".
  const taskChanged =
    task?.exists === true &&
    status !== null &&
    lastReadTaskLen !== null &&
    status.task_excerpt_len !== lastReadTaskLen;

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

      {/* TASK.md preview. `exists=false` means the supervisor has
          never written one. The header row has the status pill +
          a "Refresh" affordance so the user can pull the latest
          full body on demand. */}
      {task && (
        <div className="loop-panel-task">
          <span className={`loop-panel-pill loop-panel-pill--${task.status || (status?.task_status ?? "unknown")}`}>
            {task.status || status?.task_status || "unknown"}
          </span>
          <span className="loop-panel-task-body">
            {task.exists ? truncateBody(task.body) : "(no TASK.md yet)"}
          </span>
          <button
            type="button"
            className="ghost small loop-panel-btn loop-panel-btn--refresh"
            onClick={readFull}
            disabled={busy !== null}
            aria-label="Refresh TASK.md and journal"
            title={
              taskChanged
                ? "Task changed since last Read — click to load new body"
                : "Refresh TASK.md and journal"
            }
            data-task-changed={taskChanged ? "true" : "false"}
          >
            {busy === "refresh" ? "…" : taskChanged ? "Refresh*" : "Refresh"}
          </button>
        </div>
      )}

      {/* Last journal heading + actions. The poll doesn't pull
          this — the readFull does. So this section renders the
          last-explicitly-read content. */}
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
