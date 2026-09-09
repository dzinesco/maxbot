// v2.0 Slice E — the Bot-roster sidebar.
//
// Before this slice, the sidebar showed a list of conversations
// with Bots as a secondary attribute. After this slice, the
// sidebar shows a list of **Bots** as the primary object, and
// each Bot has a scoped conversation list that lives in the
// main chat area (not the sidebar).
//
// Layout (top → bottom):
//   1. Header: brand mark + "New chat" button (creates a
//      conversation for the currently-selected Bot; if no
//      Bot is selected, it prompts the user to pick one).
//   2. BotRoster: the new primary list. Each row has the
//      6-state avatar, name, "2m ago" timestamp, and a
//      computer sub-chip.
//   3. Optional `botPanel` slot (kept for backward compat —
//      `App.tsx` may pass `null` here now).
//   4. Footer: status indicator + Settings button.
//
// The search box that used to live here (for searching
// across messages) is now scoped to the chat area — see
// `ChatView`. The Plan defers the "Past conversations"
// history toggle to v2.3, so we don't ship a hidden
// conversation list behind a toggle in this slice.

import { useEffect, useMemo, useState } from "react";
import { BotRoster, type BotRosterProps } from "./BotRoster";
import type {
  Bot,
  BotRun,
  Computer,
  ComputerState,
  Conversation,
} from "../lib/api";
import { computerGet } from "../lib/tauri";

export interface SidebarProps {
  /**
   * The currently-selected Bot id. `null` if no Bot is
   * selected. Drives the highlighted row in the roster
   * and the scope of the chat panel.
   */
  selectedBotId: string | null;
  /** The full Bot list. */
  bots: Bot[];
  /** Per-Bot last-run summaries (used for the avatar's
   *  state derivation). The full `BotRun` shape is
   *  accepted so the avatar can read the status directly
   *  without an extra cast at the call site. */
  lastRunsByBot?: Record<string, BotRun | undefined>;
  /** Per-Bot computer rows. */
  computersByBot?: Record<string, Computer | null>;
  /** Per-Bot "what is it doing right now" string for the
   *  hover tooltip. */
  lastActionsByBot?: Record<string, string | undefined>;

  /** Callback when a Bot row is clicked. The parent
   *  decides what conversation to show for the Bot (or
   *  creates a new one). */
  onSelectBot: (botId: string) => void;
  /** Callback when the user clicks `+ New Bot`. */
  onCreateBot: () => void;
  /** Callback when the user clicks the per-row computer
   *  chip. Mounts the ComputerPanel for that Bot. */
  onOpenComputer?: (botId: string) => void;

  /**
   * The currently-active conversation id. Shown only as a
   * visual hint (e.g. for the "Viewing <Bot>'s thread"
   * banner — the parent renders that banner, not the
   * sidebar).
   */
  activeConversationId?: string | null;

  /** The status indicator (LLM provider state). */
  status: "online" | "missing" | "checking";
  /** Open the Settings modal. */
  onOpenSettings: () => void;

  /**
   * Legacy slot — previously the sidebar mounted the
   * `BotsPanel` here. Slice E makes the roster the
   * sidebar itself, so this slot is unused but kept so
   * App.tsx still compiles without surgery.
   */
  botPanel?: React.ReactNode;
}

export function Sidebar({
  selectedBotId,
  bots,
  lastRunsByBot,
  computersByBot,
  lastActionsByBot,
  onSelectBot,
  onCreateBot,
  onOpenComputer,
  status,
  onOpenSettings,
  botPanel,
}: SidebarProps) {
  // Same defensive computer-row hydration as the
  // pre-Slice-E sidebar: if the parent hasn't supplied
  // a computer for a Bot, we round-trip `computerGet`
  // to find out if one is provisioned. This is a
  // one-shot effect (no polling) — the ComputerPanel
  // itself drives live updates once the user opens it.
  const [localComputers, setLocalComputers] = useState<
    Record<string, Computer | null>
  >({});
  useEffect(() => {
    const missing = bots
      .map((b) => b.id)
      .filter(
        (id) =>
          computersByBot === undefined || !(id in computersByBot),
      )
      .filter((id) => !(id in localComputers));
    if (missing.length === 0) return;
    let cancelled = false;
    Promise.all(
      missing.map((id) =>
        computerGet(id)
          .then((c) => ({ id, c }))
          .catch(() => ({ id, c: null as Computer | null })),
      ),
    ).then((results) => {
      if (cancelled) return;
      const next: Record<string, Computer | null> = {};
      for (const { id, c } of results) next[id] = c;
      setLocalComputers((prev) => ({ ...prev, ...next }));
    });
    return () => {
      cancelled = true;
    };
  }, [bots, computersByBot, localComputers]);

  // Merge: prop wins, local fills the gaps.
  const mergedComputers: Record<string, Computer | null> = useMemo(() => {
    const out: Record<string, Computer | null> = {};
    for (const b of bots) {
      if (computersByBot && b.id in computersByBot) {
        out[b.id] = computersByBot[b.id] ?? null;
      } else if (b.id in localComputers) {
        out[b.id] = localComputers[b.id];
      }
    }
    return out;
  }, [bots, computersByBot, localComputers]);

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <div className="brand">
          <span className="brand-mark" aria-hidden>
            M
          </span>
          <span className="brand-name">MaxBot</span>
        </div>
        <NewChatButton
          selectedBotId={selectedBotId}
          // The "+ New chat" header button is now a stub —
          // ChatView owns the per-Bot "new conversation"
          // CTA, so this button is hidden when no Bot is
          // selected and shows a helpful tooltip
          // otherwise. (The actual conversation-creation
          // call lives in the chat area to keep the Bot
          // scoping obvious.)
        />
      </div>

      <BotRoster
        bots={bots}
        selectedBotId={selectedBotId}
        lastRunsByBot={lastRunsByBot}
        computersByBot={mergedComputers}
        lastActionsByBot={lastActionsByBot}
        onSelectBot={onSelectBot}
        onCreateBot={onCreateBot}
        onOpenComputer={onOpenComputer}
      />

      {botPanel}

      <div className="sidebar-footer">
        <div
          className={`status ${status === "online" ? "online" : status === "missing" ? "missing" : ""}`}
          title={
            status === "online"
              ? "LLM provider is configured and ready"
              : status === "missing"
                ? "API key not set — open Settings"
                : "Checking provider status…"
          }
        >
          <span className="indicator" />
          <span>
            {status === "online"
              ? "Ready"
              : status === "missing"
                ? "API key not set"
                : "Checking…"}
          </span>
        </div>
        <button onClick={onOpenSettings} className="ghost small">
          Settings
        </button>
      </div>
    </aside>
  );
}

// ---- "+ New chat" header button ----

/**
 * The "New chat" button in the header. Per Slice E, the
 * canonical "new conversation" CTA is the per-Bot button
 * inside `ChatView` (it scopes the new conversation to the
 * selected Bot). The header button is kept for muscle
 * memory but no longer calls any Tauri command directly;
 * instead it shows a hint when no Bot is selected, and
 * otherwise it dispatches a `bot-roster-new-conversation`
 * event the chat view listens for.
 */
function NewChatButton({
  selectedBotId,
}: {
  selectedBotId: string | null;
}) {
  // The button's title is the only piece of state that
  // changes based on whether a Bot is selected.
  const title = selectedBotId
    ? "Start a new conversation with the selected Bot"
    : "Select a Bot first, then start a new conversation";

  return (
    <button
      className="primary"
      onClick={() => {
        if (!selectedBotId) {
          // No Bot selected — flash a hint via a custom
          // event so the chat area can show a "Select a
          // Bot first" message without the sidebar
          // needing to know the chat's internals.
          window.dispatchEvent(
            new CustomEvent("maxbot:select-bot-first"),
          );
          return;
        }
        // Tell the chat view to create a new conversation
        // for the selected Bot. The chat view is the
        // canonical owner of conversation creation, so it
        // makes the Tauri call and re-renders.
        window.dispatchEvent(
          new CustomEvent("maxbot:new-conversation", {
            detail: { botId: selectedBotId },
          }),
        );
      }}
      title={title}
      data-testid="sidebar-new-chat"
    >
      + New chat
    </button>
  );
}

// ---- Re-exports for back-compat ----

/** Re-export `BotRoster` and its props under the
 *  `BotRosterProps` name from the sidebar's module so old
 *  imports of the form `import type { BotRosterProps }
 *  from "./Sidebar"` keep working. */
export type { BotRosterProps };

// ---- Misc helpers (kept for backward-compat tests) ----

/** Map a Computer state to the roster chip's modifier
 *  class. Exported so tests can assert the visual mapping
 *  without rendering the full sidebar. */
export function computerStateClass(state: ComputerState | string | null | undefined): string {
  switch (state) {
    case "running":
      return "running";
    case "stopped":
      return "stopped";
    case "provisioning":
      return "provisioning";
    case "error":
      return "error";
    default:
      return "unknown";
  }
}

/** Helper for callers that want to find the most-recent
 *  conversation for a given Bot (e.g. the parent's
 *  `onSelectBot` handler). Exported so `App.tsx` doesn't
 *  reimplement the same logic. */
export function mostRecentConversationForBot(
  conversations: Conversation[],
  botId: string,
): Conversation | null {
  const matching = conversations
    .filter((c) => c.bot_id === botId)
    .sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1));
  return matching[0] ?? null;
}
