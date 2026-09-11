/*
 * v4 — Roster (S2.5 — Grok Bot look)
 *
 * Per the S2.5 brief:
 *   - 32px circle avatar (initials or emoji icon).
 *   - 14px name.
 *   - 12px muted status word — no rainbow presence dot.
 *   - Selected row: subtle lighter fill, NO left accent stripe.
 *
 * Per the v4 hard rules: "no activity feed, no loop
 * journal, no preview" in the Roster. Click → App.tsx sets
 * `selectedBotId` and `view = "chat"`; ChatPane takes over.
 *
 * No setInterval. Relative timestamps re-render only on
 * mount; the roster itself does no polling.
 */

import type { Bot, BotState } from "../lib/api";
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

/** Single muted status word for the roster row. One color, one tone. */
function statusWord(state: BotState | undefined): string {
  switch (state) {
    case "thinking":
    case "working":
      return "Working";
    case "blocked":
    case "waiting":
      return "Blocked";
    case "done":
      return "Done";
    default:
      return "Idle";
  }
}

/** Initials fallback: first letter of the first two words,
 *  uppercased. "Alpha" → "A". "Test Bot" → "TB". */
function initialsFor(name: string): string {
  const trimmed = (name || "").trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/\s+/).slice(0, 2);
  return parts.map((p) => p[0] || "").join("").toUpperCase() || "?";
}

/** True when the icon string looks like an emoji (one grapheme
 *  cluster, no ASCII letters). Lets us render emoji icons at a
 *  larger size without falling back to initials. */
function isEmojiIcon(icon: string | undefined): boolean {
  if (!icon) return false;
  if (/[a-zA-Z0-9]/.test(icon)) return false;
  // Trim to one grapheme-ish chunk: anything beyond the first
  // cluster gets ignored. Cheap heuristic — good enough for the
  // emoji set used by the Bot editor (single-glyph picker).
  return icon.length > 0;
}

interface AvatarProps {
  name: string;
  icon: string | undefined;
  color: string | undefined;
}

/** 32px circle avatar — emoji icon if it looks like one, else
 *  initials on a tinted background (bot.color or default). */
function Avatar({ name, icon, color }: AvatarProps) {
  if (icon && isEmojiIcon(icon)) {
    return (
      <span
        className="v4-roster-avatar v4-roster-avatar--emoji"
        aria-hidden
      >
        {icon}
      </span>
    );
  }
  const bg = color && /^#([0-9a-f]{3}|[0-9a-f]{6})$/i.test(color) ? color : undefined;
  const style = bg
    ? { background: bg, color: "var(--bg-0)" }
    : undefined;
  return (
    <span className="v4-roster-avatar" style={style} aria-hidden>
      {initialsFor(name)}
    </span>
  );
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
        const selected = bot.id === selectedBotId;
        const status = statusWord(bot.state);
        const when = relativeTime(bot.last_active_at);
        return (
          <li key={bot.id}>
            <button
              type="button"
              className={`v4-roster-row ${selected ? "is-selected" : ""}`}
              onClick={() => onSelectBot(bot.id)}
              aria-current={selected ? "true" : undefined}
              title={`${bot.name} · ${status}`}
            >
              <Avatar name={bot.name} icon={bot.icon} color={bot.color} />
              <span className="v4-roster-text">
                <span className="v4-roster-name">{bot.name}</span>
                <span className="v4-roster-when">
                  {status} · {when}
                </span>
              </span>
            </button>
          </li>
        );
      })}
    </ul>
  );
}
