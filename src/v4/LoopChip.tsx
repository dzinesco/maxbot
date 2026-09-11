/*
 * v4 — LoopChip (S2.6 — small text chip in rail footer)
 *
 * Per the S2.6 brief:
 *   - Loop: small text chip in the rail footer.
 *   - Remove the big white Start button from the default rail.
 *
 * The chip is now a single text row — status word + caret.
 * The Start/Stop action moves out of v4 entirely; users who
 * need to manage `maxbot_loopd` fall back to the daily
 * /Applications/MaxBot.app (where the Loop panel is full-
 * featured). Re-adding the action in v4 is a follow-up slice.
 *
 * Per the v4 hard rules:
 *   - No setInterval — status is fetched once on mount +
 *     on window focus.
 *   - Listeners paired with unlistens in the same useEffect.
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
}

function statusLabel(
  s: LoopdStatus | null,
): { text: string; tone: "alive" | "dead" | "idle" | "unknown" } {
  if (!s) return { text: "Loop: …", tone: "unknown" };
  if (s.state === "alive") {
    return { text: `Loop alive · pid ${s.pid ?? "?"}`, tone: "alive" };
  }
  if (s.state === "dead") {
    return { text: "Loop dead", tone: "dead" };
  }
  return { text: "Loop idle", tone: "idle" };
}

export function LoopChip({ onToggleExpand, expanded }: LoopChipProps) {
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

  return (
    <section className="v4-loop-chip" aria-label="Loop supervisor">
      <button
        type="button"
        className={`v4-loop-chip-toggle ${expanded ? "is-open" : ""}`}
        onClick={onToggleExpand}
        aria-expanded={expanded}
        title="Loop supervisor — click to expand"
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
    </section>
  );
}
