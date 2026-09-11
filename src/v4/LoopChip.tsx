/*
 * v4 — LoopChip
 *
 * Status pill for `maxbot_loopd`. Always rendered. Fetches
 * `loopdStatus` ONCE on mount. No setInterval. No polling.
 *
 * Re-fetches status on:
 * - Window focus (via useFocusRefresh) — cheap, user-driven.
 * - Manual Start/Stop button click — callback bubbles up to
 *   App.tsx which calls loopdStart/loopdStop and re-renders.
 *
 * Click the chip to expand into LoopExpanded (full task body +
 * last journal heading). The expanded body is fetched on demand
 * inside LoopExpanded — this chip never holds the body.
 *
 * Per Tyler's v4 hard rules: "no setInterval unless surface
 * visible" — the chip IS always visible, but the data it shows
 * is "what's the daemon doing right now" which is itself a
 * low-cardinality state field, not a stream. One fetch on
 * focus is enough.
 */

import { useEffect, useState } from "react";
import { loopdStatus } from "../lib/tauri";
import type { LoopdStatus } from "../lib/api";
import { useFocusRefresh } from "./useFocusRefresh";
import "./styles/loop.css";

export interface LoopChipProps {
  /** Called when the user clicks the chip body. Toggles expand. */
  onToggleExpand: () => void;
  /** Whether the expand panel is currently open. Drives styling. */
  expanded: boolean;
  /** Called when the user clicks Start. App.tsx handles the IPC. */
  onStart: () => void;
  /** Called when the user clicks Stop. App.tsx handles the IPC. */
  onStop: () => void;
  /** True while a Start/Stop IPC is in flight. Disables buttons. */
  busy: boolean;
}

function statusLabel(
  s: LoopdStatus | null,
): { text: string; tone: "alive" | "dead" | "idle" | "unknown" } {
  if (!s) return { text: "Loop: …", tone: "unknown" };
  if (s.state === "alive") {
    return { text: `Loop alive · pid ${s.pid ?? "?"}`, tone: "alive" };
  }
  if (s.state === "dead") {
    return { text: "Loop dead · Start", tone: "dead" };
  }
  return { text: "Loop idle · Start", tone: "idle" };
}

export function LoopChip({
  onToggleExpand,
  expanded,
  onStart,
  onStop,
  busy,
}: LoopChipProps) {
  const [status, setStatus] = useState<LoopdStatus | null>(null);

  const refresh = () => {
    let cancelled = false;
    loopdStatus()
      .then((s) => {
        if (!cancelled) setStatus(s);
      })
      .catch(() => {
        if (!cancelled) setStatus(null);
      });
    return () => {
      cancelled = true;
    };
  };

  useEffect(() => {
    const cleanup = refresh();
    return cleanup;
  }, []);

  useFocusRefresh(refresh);

  const label = statusLabel(status);
  const showStart = status?.state !== "alive";
  const showStop = status?.state === "alive";

  return (
    <section className="v4-loop-chip" aria-label="Loop supervisor">
      <button
        type="button"
        className={`v4-loop-chip-toggle ${expanded ? "is-open" : ""}`}
        onClick={onToggleExpand}
        aria-expanded={expanded}
      >
        <span
          className={`v4-loop-chip-indicator v4-loop-chip-indicator--${label.tone}`}
          data-state={label.tone}
          aria-hidden
        />
        <span className="v4-loop-chip-text">{label.text}</span>
        <span className="v4-loop-chip-caret" aria-hidden>
          {expanded ? "▾" : "▸"}
        </span>
      </button>
      {(showStart || showStop) && (
        <div className="v4-loop-chip-actions">
          {showStart && (
            <button
              type="button"
              className="v4-loop-chip-action primary"
              onClick={onStart}
              disabled={busy}
              title="Start maxbot_loopd"
            >
              {busy ? "Starting…" : "Start"}
            </button>
          )}
          {showStop && (
            <button
              type="button"
              className="v4-loop-chip-action danger"
              onClick={onStop}
              disabled={busy}
              title="Stop maxbot_loopd (SIGTERM, then SIGKILL)"
            >
              {busy ? "Stopping…" : "Stop"}
            </button>
          )}
        </div>
      )}
    </section>
  );
}
