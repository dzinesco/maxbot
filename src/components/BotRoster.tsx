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
//   - a hover-revealed "×" destroy button (v3.7.6) that
//     asks for confirmation, then cascades:
//       1) `computer_destroy(botId)` (best-effort; no
//          error if no VM is provisioned)
//       2) `delete_bot(botId)` to drop the SQLite row
//     The cascade is the canonical "destroy a Bot" path
//     Tyler asked for after "i cant destroy bots, no
//     button" — the ComputerPanel's Destroy button is
//     hidden until you open the panel for that Bot, and
//     the Bot editor's "Delete" button doesn't clean up
//     the VM, so neither was a discoverable way to fully
//     remove a Bot.
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
  /** Callback when the row's destroy (×) button is clicked.
   *  v3.7.6: the parent is responsible for confirmation
   *  + the cascade (`computer_destroy` then `delete_bot`).
   *  Omit to hide the destroy button entirely. */
  onDestroyBot?: (botId: string) => void;
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
  onDestroyBot,
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
                    {/* v3.7.13 — UX-2. Replace the
                        "2m ago" timestamp with a
                        presence word derived from the
                        Bot's most-recent run. The
                        timestamp was confusing
                        because a Bot with a recent
                        "succeeded" run and a Bot
                        with a recent "errored" run
                        both said "just now" — the
                        user couldn't tell what the
                        Bot was actually doing.
                        `presence()` is the same
                        state vocabulary the avatar
                        uses; the row subtitle is
                        now a one-word verb that
                        matches the avatar's color
                        (Working/amber, Waiting/blue,
                        Queued/neutral, Idle/grey). */}
                    <div
                      className="bot-roster__last-active"
                      data-testid="bot-roster-presence"
                      data-presence={presence(lastRun)}
                    >
                      {presence(lastRun)}
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
                  {/* v3.7.6: per-row destroy (×) button. Hover-
                      revealed so it doesn't clutter the sidebar
                      but is one click away. The parent's
                      `onDestroyBot` is responsible for the
                      confirm + the cascade (computer_destroy
                      then delete_bot). */}
                  {onDestroyBot && (
                    <button
                      type="button"
                      className="bot-roster__destroy"
                      onClick={(e) => {
                        e.stopPropagation();
                        onDestroyBot(bot.id);
                      }}
                      title="Destroy this Bot (VM + chat history)"
                      aria-label={`Destroy ${bot.name || "Unnamed Bot"}`}
                      data-testid="bot-roster-destroy"
                    >
                      ×
                    </button>
                  )}
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

// ---- v3.7.13 — UX-2. Presence word ----

/**
 * Derive a one-word presence label from the Bot's
 * most-recent run status. Returns `"Idle"` when the
 * Bot has never run, which is the most accurate
 * state for a fresh install (vs. the previous
 * `"—"` placeholder, which read as
 * missing/broken).
 *
 * The four states match the `BotAvatar` 6-state
 * indicator: a running run → "Working" (amber);
 * an awaiting-approval run → "Waiting" (blue);
 * a queued run → "Queued" (neutral); anything
 * else (succeeded, failed, no run at all) →
 * "Idle" (grey). The mapping is intentionally
 * lossy: a "succeeded 3 minutes ago" run and a
 * "failed 3 minutes ago" run both read "Idle",
 * because the roster is a navigation list, not an
 * activity log — the Activity rail carries the
 * per-run detail.
 *
 * The status argument is typed as `string` (not
 * `BotRunStatus`) so the function can accept the
 * "awaiting_approval" / "queued" values the
 * BotAvatar's 6-state indicator already speaks,
 * even though today's Rust `BotRunStatus` enum
 * only has 4 variants. The string match falls
 * through to "Idle" for unknown values, so a
 * future enum addition doesn't break the
 * roster.
 */
export function presence(run: { status: string } | undefined): string {
  if (!run) return "Idle";
  switch (run.status) {
    case "running":
      return "Working";
    case "awaiting_approval":
      return "Waiting";
    case "queued":
      return "Queued";
    default:
      return "Idle";
  }
}

// ---- "2m ago" formatting ----

/**
 * Format an ISO timestamp as a compact relative time
 * ("just now", "5m ago", "2h ago", "3d ago"). Returns "—"
 * for null/empty inputs so a brand-new Bot doesn't render
 * a misleading "0s ago".
 *
 * v3.7.13 — UX-2. The roster's row subtitle no
 * longer uses this; it's still here for
 * backwards-compat callers (the BotEditor's
 * detail view, for example) and for the
 * `formatLastActive` test below, which pins the
 * shape against a future rewording.
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
