// v2.0 Slice E — the 6-state presence system for the Bot
// roster sidebar. Each Bot gets a 32×32 avatar that animates
// according to its current state, with priority logic that
// factors in `bot.state` (column on `bots`), the most-recent
// `bot_run.status` (if the bot is running), and
// `computer.state` (if a computer is provisioned). See
// `deriveAvatarState` below for the full priority table.
//
// The state derivation is a pure function so it's testable
// without rendering — the `BotAvatar.test.tsx` suite asserts
// the priority order (blocked > waiting > working > thinking
// > done > idle) and the per-state visual markers.
//
// Why CSS-only animations: the previous design (lottie /
// framer-motion / a heavy SVG keyframe set) is overkill for
// six states with subtle motion. CSS keyframes + small SVG
// icons + unicode glyphs render in 1ms and don't pull in a
// 200kb dependency. The animations are scoped under
// `.bot-avatar--*` so they don't leak.

import { useMemo } from "react";
import type { Bot, BotRunStatus, BotState, Computer, ComputerState } from "../lib/api";

// ---- State derivation (pure) ----

export interface AvatarInputs {
  /** The persisted presence hint on the Bot row. */
  botState: BotState | undefined;
  /** The most-recent bot run's status, if any. */
  lastRunStatus: BotRunStatus | undefined;
  /**
   * The Bot's computer row state, if a computer is
   * provisioned. The Rust side stores this as a free-form
   * string (so future enum additions don't immediately
   * break the renderer), so we accept the broader
   * `string | ComputerState` and ignore unknown values.
   */
  computerState: ComputerState | string | undefined;
}

/**
 * Compute the effective avatar state from the three inputs.
 * Pure function — given the same inputs, returns the same
 * state. Test surface in `BotAvatar.test.tsx`.
 *
 * Priority (from the plan):
 *   1. failed run                   → blocked
 *   2. computer provisioning        → working
 *   3. computer stopped with run    → waiting
 *   4. computer running             → working
 *   5. persisted state              → as-is
 *   6. default                      → idle
 *
 * The renderer uses this single function so the visual state
 * is deterministic — the test asserts the exact same table.
 */
export function deriveAvatarState(inputs: AvatarInputs): BotState {
  const { botState, lastRunStatus, computerState } = inputs;

  // 1. A failed run always wins — even if the executor hasn't
  //    yet written `blocked` to the bots row, the renderer
  //    should flash red so the user sees the failure
  //    immediately.
  if (lastRunStatus === "failed") {
    return "blocked";
  }

  // 2. Computer lifecycle beats the persisted state — a Bot
  //    whose VM is still spinning up is "working" even if the
  //    last run was `done` 30 seconds ago. The user cares
  //    about the live state, not the last completed state.
  //    Unknown computer states ("destroyed", future additions)
  //    are treated as "no live signal" and fall through.
  if (computerState === "provisioning") {
    return "working";
  }
  if (computerState === "error") {
    return "blocked";
  }
  if (computerState === "stopped" && lastRunStatus === "running") {
    return "waiting";
  }
  if (computerState === "running") {
    return "working";
  }

  // 3. Otherwise honor the persisted state. Unknown values
  //    (`undefined`, future enum additions) fall back to
  //    `idle` so the UI never renders a half-broken state.
  if (botState && isKnownState(botState)) {
    return botState;
  }
  return "idle";
}

const KNOWN_STATES: ReadonlySet<BotState> = new Set<BotState>([
  "idle",
  "thinking",
  "working",
  "waiting",
  "blocked",
  "done",
]);

export function isKnownState(value: string): value is BotState {
  return KNOWN_STATES.has(value as BotState);
}

// ---- Visual description (for the hover tooltip) ----

const STATE_DESCRIPTIONS: Record<BotState, string> = {
  idle: "Idle — waiting for the next message",
  thinking: "Thinking — composing a response",
  working: "Working — running a tool or computer step",
  waiting: "Waiting — needs your input to continue",
  blocked: "Blocked — the last run failed",
  done: "Done — last run completed",
};

/**
 * Human-readable description of the state. Used for the
 * hover tooltip (via the standard `title` attribute) and as
 * the test-time assertion target.
 */
export function stateDescription(
  state: BotState,
  lastAction?: string,
): string {
  if (lastAction && state !== "idle") {
    return `${STATE_DESCRIPTIONS[state]} (${lastAction})`;
  }
  return STATE_DESCRIPTIONS[state];
}

// ---- Component ----

export interface BotAvatarProps {
  bot: Bot;
  /** Most recent bot run for this Bot. Undefined if never run. */
  lastRun?: { status: BotRunStatus };
  /** The Bot's computer row, if a computer is provisioned. */
  computer?: Computer | null;
  /** The last action the Bot is doing, for the tooltip. */
  lastAction?: string;
  /** Pixel size; default 32 to match the design. */
  size?: number;
  /** Optional className passthrough for the wrapping element. */
  className?: string;
  /** Force a particular state — used by tests + the
   *  "Create new Bot" preview. */
  stateOverride?: BotState;
}

/**
 * The 6-state avatar. Renders a 32×32 visual that animates
 * according to the derived state, with a hover tooltip that
 * describes what the Bot is doing.
 */
export function BotAvatar({
  bot,
  lastRun,
  computer,
  lastAction,
  size = 32,
  className,
  stateOverride,
}: BotAvatarProps) {
  // Memoize the derivation — the inputs only change when the
  // parent re-fetches the bot, the last run, or the
  // computer. Avoids re-running the priority logic on every
  // parent re-render.
  const state = useMemo<BotState>(() => {
    if (stateOverride) return stateOverride;
    return deriveAvatarState({
      botState: bot.state,
      lastRunStatus: lastRun?.status,
      computerState: computer?.state,
    });
  }, [
    stateOverride,
    bot.state,
    lastRun?.status,
    computer?.state,
  ]);

  const tooltip = stateDescription(state, lastAction);
  const initials = deriveInitials(bot.name, bot.icon);

  return (
    <div
      className={[
        "bot-avatar",
        `bot-avatar--${state}`,
        className,
      ]
        .filter(Boolean)
        .join(" ")}
      style={{
        width: size,
        height: size,
        // v2.0 Slice E: the avatar gradient mixes `color`
        // (primary) + `avatar_color` (optional secondary).
        // When only `color` is set we fall back to a flat
        // fill; when both are set we render a 135° linear
        // gradient so each Bot has its own visual fingerprint
        // in the roster.
        background: avatarBackground(bot),
        color: initialsColor(bot),
        fontSize: Math.max(10, Math.floor(size * 0.42)),
      }}
      role="img"
      aria-label={`${bot.name || "Bot"} — ${state}`}
      title={tooltip}
      data-bot-state={state}
    >
      <span className="bot-avatar__initials" aria-hidden="true">
        {initials}
      </span>
      <StateOverlay state={state} size={size} />
    </div>
  );
}

// ---- Helpers ----

/**
 * Build the avatar's background CSS. The `color` field is
 * the primary; `avatar_color` is the optional secondary that
 * produces a 135° gradient. Both default to the theme's
 * accent when empty, so a brand-new Bot still looks
 * intentional.
 */
function avatarBackground(bot: Bot): string {
  const primary = bot.color || "#7c5cff";
  const secondary = bot.avatar_color;
  if (secondary && secondary !== primary) {
    return `linear-gradient(135deg, ${primary} 0%, ${secondary} 100%)`;
  }
  return primary;
}

/**
 * Choose a readable text color for the initials — dark on
 * light backgrounds, light on dark ones. The check is
 * intentionally crude (luminance threshold) so we don't
 * pull in a color library; the goal is "readable" not
 * "WCAG-perfect".
 */
function initialsColor(bot: Bot): string {
  const primary = bot.color || "#7c5cff";
  // Strip leading "#" and parse to RGB.
  const hex = primary.replace("#", "");
  if (hex.length !== 6) return "#ffffff";
  const r = parseInt(hex.slice(0, 2), 16);
  const g = parseInt(hex.slice(2, 4), 16);
  const b = parseInt(hex.slice(4, 6), 16);
  // Relative luminance, simplified.
  const lum = (0.299 * r + 0.587 * g + 0.114 * b) / 255;
  return lum > 0.6 ? "#1a1a1f" : "#ffffff";
}

/**
 * Pick the initials to render inside the avatar circle.
 * Prefers an explicit `bot.icon` (a single emoji) when
 * it's a single grapheme; otherwise falls back to the
 * first letter of each whitespace-delimited word in the
 * name.
 */
function deriveInitials(name: string, icon: string): string {
  if (icon && [...icon].length === 1) {
    return icon;
  }
  if (icon) {
    // Multi-codepoint icon (e.g. "🤖" is one codepoint
    // but a flag is two). Take the first grapheme.
    return [...icon][0] || "?";
  }
  const parts = name.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return "?";
  if (parts.length === 1) {
    return parts[0].slice(0, 2).toUpperCase();
  }
  return (parts[0][0] + parts[1][0]).toUpperCase();
}

/**
 * The per-state visual overlay. Sits on top of the avatar
 * base (the colored circle with initials) and renders the
 * state-specific marker — pulsing dots, a checkmark flash,
 * a red ring, an hourglass, etc. The base CSS in
 * `styles.css` handles all animations.
 */
function StateOverlay({
  state,
  size,
}: {
  state: BotState;
  size: number;
}) {
  // The overlay is a corner badge — sized to about a third
  // of the avatar so it reads at a glance without overpowering
  // the initials. Each state has its own class for the CSS to
  // target.
  const badgeSize = Math.max(10, Math.floor(size * 0.34));

  switch (state) {
    case "idle":
      // No badge — idle is the resting state. The CSS gives
      // the base circle a slow opacity breath via
      // `.bot-avatar--idle`.
      return null;

    case "thinking":
      return (
        <span
          className="bot-avatar__badge bot-avatar__badge--thinking"
          style={{ width: badgeSize, height: badgeSize }}
          aria-hidden="true"
        >
          <span className="bot-avatar__dot" />
          <span className="bot-avatar__dot" />
          <span className="bot-avatar__dot" />
        </span>
      );

    case "working":
      return (
        <span
          className="bot-avatar__badge bot-avatar__badge--working"
          style={{ width: badgeSize, height: badgeSize }}
          aria-hidden="true"
        >
          <span className="bot-avatar__spinner" />
        </span>
      );

    case "waiting":
      return (
        <span
          className="bot-avatar__badge bot-avatar__badge--waiting"
          style={{
            width: badgeSize,
            height: badgeSize,
            fontSize: Math.max(8, Math.floor(badgeSize * 0.7)),
            lineHeight: 1,
          }}
          aria-hidden="true"
        >
          ⏳
        </span>
      );

    case "blocked":
      return (
        <span
          className="bot-avatar__badge bot-avatar__badge--blocked"
          style={{
            width: badgeSize,
            height: badgeSize,
            fontSize: Math.max(8, Math.floor(badgeSize * 0.75)),
            lineHeight: 1,
          }}
          aria-hidden="true"
        >
          !
        </span>
      );

    case "done":
      // The 300ms done flash is a CSS animation on the
      // wrapper. We render a small checkmark badge so the
      // user gets a clear "yes, finished" cue before the
      // avatar transitions back to idle.
      return (
        <span
          className="bot-avatar__badge bot-avatar__badge--done"
          style={{
            width: badgeSize,
            height: badgeSize,
            fontSize: Math.max(8, Math.floor(badgeSize * 0.7)),
            lineHeight: 1,
          }}
          aria-hidden="true"
        >
          ✓
        </span>
      );
  }
}
