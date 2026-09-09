// v2.0 Slice E — the Bot roster sidebar. Replaces the
// conversation list as the primary object in the sidebar.
//
// Each row shows:
//   - the 6-state `BotAvatar` for the Bot
//   - the Bot's name (truncated to fit the sidebar width)
//   - "2m ago" / "just now" / "—" last-active timestamp
//   - a small computer sub-icon (the same
//     `sidebar__computer-chip` from Slice C), clicking it
//     opens `ComputerPanel` for that Bot
//
// A search box at the top filters the list by name
// (case-insensitive substring). A `+ New Bot` button at the
// top opens `BotEditor`. If the user has no Bots, the list
// shows an empty state with a "Create your first Bot"
// primary CTA.

import { useMemo, useState } from "react";
import type {
  Bot,
  BotRun,
  Computer,
  ComputerState,
} from "../lib/api";
import { BotAvatar } from "./BotAvatar";

// ---- Types ----

export interface BotRosterProps {
  bots: Bot[];
  /** Currently-selected Bot id, if any. */
  selectedBotId: string | null;
  /** Optional map of Bot id → most-recent run (for state
   *  derivation; the avatar factors in the run status). */
  lastRunsByBot?: Record<string, BotRun | undefined>;
  /** Optional map of Bot id → computer row (for the
   *  sidebar chip and the avatar's `computerState` input). */
  computersByBot?: Record<string, Computer | null | undefined>;
  /** Optional list of active `bot_run.status === "running"`
   *  summaries, surfaced as the tooltip's "last action". */
  lastActionsByBot?: Record<string, string | undefined>;
  /** Callback when a row is clicked. */
  onSelectBot: (botId: string) => void;
  /** Callback when the `+ New Bot` button is clicked or
   *  the empty-state CTA is clicked. */
  onCreateBot: () => void;
  /** Callback when a row's computer chip is clicked. */
  onOpenComputer?: (botId: string) => void;
}

// ---- Component ----

export function BotRoster({
  bots,
  selectedBotId,
  lastRunsByBot,
  computersByBot,
  lastActionsByBot,
  onSelectBot,
  onCreateBot,
  onOpenComputer,
}: BotRosterProps) {
  const [query, setQuery] = useState("");

  // Filter is case-insensitive substring on the Bot's name
  // (the icon field is excluded from search so a user can't
  // find a Bot by its emoji alone). Empty query → show all.
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return bots;
    return bots.filter((b) => (b.name || "").toLowerCase().includes(q));
  }, [bots, query]);

  // Empty state: no Bots at all (not just no search results).
  // A user with zero Bots gets the friendlier
  // "Create your first Bot" CTA; a user with Bots but an
  // unmatched query gets a softer "no matches" hint.
  const hasNoBots = bots.length === 0;
  const hasNoMatches = !hasNoBots && filtered.length === 0;

  return (
    <div className="bot-roster" data-testid="bot-roster">
      <div className="bot-roster__header">
        <input
          type="text"
          className="bot-roster__search"
          placeholder="Search bots…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          aria-label="Search bots"
          data-testid="bot-roster-search"
        />
        <button
          type="button"
          className="bot-roster__new-btn"
          onClick={onCreateBot}
          aria-label="Create new bot"
          data-testid="bot-roster-new-btn"
        >
          <span aria-hidden="true">+</span>
          <span className="bot-roster__new-btn-label">New Bot</span>
        </button>
      </div>

      {hasNoBots ? (
        <EmptyState onCreateBot={onCreateBot} />
      ) : hasNoMatches ? (
        <NoMatchesState query={query} onClear={() => setQuery("")} />
      ) : (
        <ul className="bot-roster__list" role="listbox" aria-label="Bots">
          {filtered.map((bot) => {
            const isSelected = bot.id === selectedBotId;
            const lastRun = lastRunsByBot?.[bot.id];
            const computer = computersByBot?.[bot.id];
            const lastAction = lastActionsByBot?.[bot.id];
            return (
              <li
                key={bot.id}
                className={[
                  "bot-roster__item",
                  isSelected ? "bot-roster__item--selected" : "",
                ]
                  .filter(Boolean)
                  .join(" ")}
                role="option"
                aria-selected={isSelected}
                data-bot-id={bot.id}
                data-testid="bot-roster-item"
              >
                <button
                  type="button"
                  className="bot-roster__row"
                  onClick={() => onSelectBot(bot.id)}
                >
                  <BotAvatar
                    bot={bot}
                    lastRun={lastRun}
                    computer={computer}
                    lastAction={lastAction}
                  />
                  <div className="bot-roster__meta">
                    <div className="bot-roster__name" title={bot.name}>
                      {bot.name || "Unnamed Bot"}
                    </div>
                    <div className="bot-roster__last-active">
                      {formatLastActive(bot.last_active_at)}
                    </div>
                  </div>
                  <ComputerChip
                    computer={computer}
                    onClick={
                      onOpenComputer
                        ? (e) => {
                            e.stopPropagation();
                            onOpenComputer(bot.id);
                          }
                        : undefined
                    }
                  />
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

// ---- Empty / no-matches states ----

function EmptyState({ onCreateBot }: { onCreateBot: () => void }) {
  return (
    <div
      className="bot-roster__empty-state"
      data-testid="bot-roster-empty"
    >
      <div className="bot-roster__empty-mark" aria-hidden="true">
        {/* A simple face silhouette — no Lottie, just inline
            SVG so the empty state has a fingerprint without
            a dependency. */}
        <svg viewBox="0 0 64 64" width="64" height="64">
          <circle
            cx="32"
            cy="32"
            r="28"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            opacity="0.5"
          />
          <circle cx="24" cy="28" r="3" fill="currentColor" />
          <circle cx="40" cy="28" r="3" fill="currentColor" />
          <path
            d="M22 42 Q32 48 42 42"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
          />
        </svg>
      </div>
      <h3 className="bot-roster__empty-title">No Bots yet</h3>
      <p className="bot-roster__empty-body">
        Bots are persistent assistants with their own tools, computer,
        and memory. Create your first one to get started.
      </p>
      <button
        type="button"
        className="bot-roster__empty-cta"
        onClick={onCreateBot}
        data-testid="bot-roster-empty-cta"
      >
        Create your first Bot
      </button>
    </div>
  );
}

function NoMatchesState({
  query,
  onClear,
}: {
  query: string;
  onClear: () => void;
}) {
  return (
    <div className="bot-roster__no-matches">
      <p>No bots match “{query}”.</p>
      <button
        type="button"
        className="bot-roster__no-matches-clear"
        onClick={onClear}
      >
        Clear search
      </button>
    </div>
  );
}

// ---- Computer chip (small sub-icon on the right of each row) ----

function ComputerChip({
  computer,
  onClick,
}: {
  computer: Computer | null | undefined;
  onClick?: (e: React.MouseEvent) => void;
}) {
  // A bot with no computer row → render a faded "no computer"
  // dot (signals "this Bot has no VM yet"). A bot with a
  // computer row → render the existing sidebar chip style
  // (running/provisioning/stopped/error). The chip is a
  // button so clicking it opens ComputerPanel without
  // selecting the Bot.
  if (!computer) {
    return (
      <span
        className="bot-roster__computer-chip bot-roster__computer-chip--none"
        title="No computer provisioned"
        aria-label="No computer provisioned"
        data-testid="bot-roster-computer-none"
        onClick={onClick}
        role={onClick ? "button" : undefined}
      >
        <span className="bot-roster__computer-chip-dot" />
      </span>
    );
  }

  const stateClass = `bot-roster__computer-chip--${computer.state}`;
  const stateLabel = computerStateLabel(computer.state);

  return (
    <span
      className={`bot-roster__computer-chip ${stateClass}`}
      title={stateLabel}
      aria-label={stateLabel}
      data-testid="bot-roster-computer"
      data-computer-state={computer.state}
      onClick={onClick}
      role={onClick ? "button" : undefined}
    >
      <span className="bot-roster__computer-chip-dot" />
    </span>
  );
}

function computerStateLabel(state: ComputerState | string): string {
  switch (state) {
    case "running":
      return "Computer: running";
    case "stopped":
      return "Computer: stopped";
    case "provisioning":
      return "Computer: provisioning";
    case "error":
      return "Computer: error";
    default:
      return `Computer: ${state}`;
  }
}

// ---- "2m ago" formatting ----

/**
 * Format an ISO timestamp as a compact relative time
 * ("just now", "5m ago", "2h ago", "3d ago"). Returns "—"
 * for null/empty inputs so a brand-new Bot doesn't render
 * a misleading "0s ago".
 */
function formatLastActive(iso: string | null | undefined): string {
  if (!iso) return "—";
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "—";
  const now = Date.now();
  const diffMs = now - then;
  if (diffMs < 0) return "just now";
  if (diffMs < 60_000) return "just now";
  const minutes = Math.floor(diffMs / 60_000);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 7) return `${days}d ago`;
  // Older than a week — fall back to a short date so the
  // roster stays compact.
  const date = new Date(iso);
  return date.toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}
