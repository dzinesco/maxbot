/*
 * v4 — Roster
 *
 * The bot list. Always rendered. No preview, no journal,
 * no chat content — just the bot name, presence dot, and
 * "last active" relative time.
 *
 * Per Tyler's v4 hard rules: "no activity feed, no loop
 * journal, no preview" in the Roster. Click → App.tsx sets
 * `selectedBotId` and `view = "chat"`; ChatPane takes over.
 *
 * No setInterval. The presence dot uses the bot's `state`
 * field as last persisted by the daemon (the v1 schema
 * already updates this). Relative timestamps re-render
 * only on mount; if the user wants fresher data, they
 * focus the window (App.tsx handles refetch) — but the
 * roster itself does no polling.
 */

import type { Bot } from "../lib/api";
import "./styles/roster.css";

export interface RosterProps {
  bots: Bot[];
  selectedBotId: string | null;
  onSelectBot: (botId: string) => void;
}

function relativeTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "—";
  const diff = Date.now() - t;
  if (diff < 60_000) return "just now";
  if (diff < 60 * 60_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 24 * 60 * 60_000) return `${Math.floor(diff / (60 * 60_000))}h ago`;
  return `${Math.floor(diff / (24 * 60 * 60_000))}d ago`;
}

function presenceTone(state: Bot["state"] | undefined): string {
  switch (state) {
    case "thinking":
    case "working":
      return "busy";
    case "blocked":
    case "waiting":
      return "blocked";
    case "done":
      return "done";
    default:
      return "idle";
  }
}

export function Roster({ bots, selectedBotId, onSelectBot }: RosterProps) {
  if (bots.length === 0) {
    return (
      <div className="v4-roster v4-roster--empty">
        <div className="v4-roster-empty-text">
          No bots yet. The Welcome overlay will guide you through creating one.
        </div>
      </div>
    );
  }

  return (
    <ul className="v4-roster" role="list" aria-label="Bots">
      {bots.map((bot) => {
        const tone = presenceTone(bot.state);
        const selected = bot.id === selectedBotId;
        return (
          <li key={bot.id}>
            <button
              type="button"
              className={`v4-roster-row ${selected ? "is-selected" : ""}`}
              onClick={() => onSelectBot(bot.id)}
              aria-current={selected ? "true" : undefined}
            >
              <span
                className={`v4-roster-presence v4-roster-presence--${tone}`}
                data-state={tone}
                aria-hidden
              />
              <span className="v4-roster-name">{bot.name}</span>
              <span className="v4-roster-when">
                {relativeTime(bot.last_active_at)}
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
