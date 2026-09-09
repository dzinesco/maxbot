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
import pkg from "../../package.json";
import { BotRoster, type BotRosterProps } from "./BotRoster";
import type {
  Bot,
  BotRun,
  Computer,
  ComputerState,
  Conversation,
  GroupChat,
} from "../lib/api";
import { approvalPendingCount, computerGet, groupList } from "../lib/tauri";
import { ActivityFeed } from "./ActivityFeed";

// Version pulled from package.json at build time so the sidebar
// pill always matches the running app. The `data-version-channel`
// attribute on the pill drives the color (see .brand-version CSS);
// "stable" is the green pill we ship today. When v2.8 / v2.9 land
// as pre-release channels, the same slot can carry a yellow/red
// pill without a layout change.
const APP_VERSION: string = pkg.version;
const VERSION_CHANNEL: "stable" | "beta" | "alpha" = "stable";

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
   * v2.2.0 / v2.3.0 — the currently-active top-level view.
   * The sidebar renders a tab row with "Bots" / "Skills" /
   * "Routines"; this prop tells the row which one is
   * highlighted. Callbacks fire on click. The parent
   * owns the state because the main view area switches
   * based on it.
   *
   * v2.4.0: extended with `"group"` for multi-Bot
   * group chats. The Sidebar's "Groups" section
   * dispatches a `select-group` event rather than
   * switching the top-level tabs — the active group's
   * pane replaces the chat area, and the Bots tab
   * stays the canonical "select a Bot" entry point.
   *
   * v2.6.0: added `"approvals"`. The "Approvals"
   * section flips `mainView` to this value (the
   * Sidebar's tablist doesn't include an Approvals
   * tab — the badge in the Approvals section is the
   * entry point, not a tab).
   */
  mainView: "chat" | "skills" | "routines" | "memory" | "approvals";
  onSelectView: (
    view: "chat" | "skills" | "routines" | "memory" | "approvals",
  ) => void;

  /**
   * v2.4.0: open the "Create group" dialog. The
   * Sidebar's Groups section renders a `+` button
   * that fires this; App.tsx mounts the dialog.
   */
  onCreateGroup?: () => void;

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
  mainView,
  onSelectView,
  onCreateGroup,
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
          <span
            className="brand-version"
            data-version-channel={VERSION_CHANNEL}
            title={`MaxBot v${APP_VERSION} (${VERSION_CHANNEL})`}
          >
            v{APP_VERSION}
          </span>
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

      {/* v2.2.0 / v2.3.0 — top-level view switcher. Three
         tabs: Bots (the chat UI), Skills (the Skills
         panel), and Routines (the per-bot scheduler UI).
         The parent owns which view is active; this just
         dispatches the click. */}
      <div className="sidebar-tabs" role="tablist">
        <button
          className={`sidebar-tab ${mainView === "chat" ? "sidebar-tab--active" : ""}`}
          onClick={() => onSelectView("chat")}
          role="tab"
          aria-selected={mainView === "chat"}
        >
          Bots
        </button>
        <button
          className={`sidebar-tab ${mainView === "skills" ? "sidebar-tab--active" : ""}`}
          onClick={() => onSelectView("skills")}
          role="tab"
          aria-selected={mainView === "skills"}
        >
          Skills
        </button>
        <button
          className={`sidebar-tab ${mainView === "routines" ? "sidebar-tab--active" : ""}`}
          onClick={() => onSelectView("routines")}
          role="tab"
          aria-selected={mainView === "routines"}
        >
          Routines
        </button>
        <button
          className={`sidebar-tab ${mainView === "memory" ? "sidebar-tab--active" : ""}`}
          onClick={() => onSelectView("memory")}
          role="tab"
          aria-selected={mainView === "memory"}
        >
          Memory
        </button>
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

      {/* v2.4.0 — Groups section. Sits between the Bot
         roster and the footer so the user can scan
         both at a glance. The list is local-state
         fetched on mount (one-shot, no polling) —
         a fresh list comes back on every page
         load, and the parent re-fetches after
         `groupCreate` so the new group lands at
         the top. */}
      <GroupsSection onCreateGroup={onCreateGroup} />

      {/* v2.6.0 — Approvals. Sits between the Groups
         section and the footer. The badge is the
         entry point: clicking it dispatches a custom
         event that flips `mainView` to `"approvals"`,
         which renders the ApprovalQueue in the main
         area. We poll the count lightly (every 5s +
         on window focus) so a Bot run that enqueues
         a fresh approval lands the badge within a
         few seconds. */}
      <ApprovalsSection />

      {botPanel}

      {/* v2.8.0 — Always-on Daemon: read-only feed of
          recent Bot runs / Skill runs / Approvals. Sits
          between the Bot list and the Approvals badge
          so the user sees "what fired while I was
          away" without opening a separate view. */}
      <ActivityFeed />

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

// ---- v2.4.0 Groups section ----

/**
 * Small list of saved groups, with a `+` button that
 * opens the CreateGroupDialog (parent owns the modal
 * state). The list is local state — we fetch once on
 * mount and re-fetch when the page returns to focus so
 * a freshly-created group shows up at the top.
 */
function GroupsSection({
  onCreateGroup,
}: {
  onCreateGroup?: () => void;
}) {
  const [groups, setGroups] = useState<GroupChat[]>([]);
  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      groupList()
        .then((rows) => {
          if (!cancelled) setGroups(rows);
        })
        .catch(() => {
          /* swallow — sidebar list is non-critical */
        });
    };
    refresh();
    // Re-fetch when the user returns to the tab so a
    // background `groupCreate` from a script lands.
    const onFocus = () => refresh();
    window.addEventListener("focus", onFocus);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", onFocus);
    };
  }, []);
  return (
    <div className="sidebar-groups" data-testid="sidebar-groups">
      <div className="sidebar-groups-header">
        <span>Groups</span>
        {onCreateGroup && (
          <button
            className="ghost small"
            onClick={onCreateGroup}
            title="Create a group"
            data-testid="sidebar-create-group"
          >
            +
          </button>
        )}
      </div>
      {groups.length === 0 ? (
        <div className="sidebar-groups-empty">No groups yet</div>
      ) : (
        <ul className="sidebar-groups-list">
          {groups.map((g) => (
            <li
              key={g.chat.id}
              className="sidebar-groups-row"
              data-testid={`sidebar-group-${g.chat.id}`}
              onClick={() =>
                window.dispatchEvent(
                  new CustomEvent("maxbot:select-group", {
                    detail: { groupId: g.chat.id },
                  }),
                )
              }
            >
              <span className="sidebar-groups-name">{g.chat.name}</span>
              <span className="sidebar-groups-count">
                {g.member_bot_ids.length}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ---- v2.6.0 Approvals section ----

/**
 * Sidebar section that shows the count of pending
 * approvals and routes the user to the
 * `ApprovalQueue` view. The count is light-poll
 * (5s + on focus) so a fresh enqueue lands the
 * badge within a few seconds; the actual
 * `ApprovalQueue` component does its own refetch
 * on mount, so this section doesn't need to share
 * state with the queue.
 */
function ApprovalsSection() {
  const [count, setCount] = useState(0);
  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      approvalPendingCount()
        .then((n) => {
          if (!cancelled) setCount(n);
        })
        .catch(() => {
          /* non-critical */
        });
    };
    refresh();
    const onFocus = () => refresh();
    window.addEventListener("focus", onFocus);
    // Light polling: a Bot's run is the only thing
    // that bumps the count and we want the badge to
    // land within a few seconds.
    const interval = window.setInterval(refresh, 5_000);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", onFocus);
      window.clearInterval(interval);
    };
  }, []);
  return (
    <div
      className="sidebar-approvals"
      data-testid="sidebar-approvals"
      onClick={() =>
        window.dispatchEvent(new CustomEvent("maxbot:open-approvals"))
      }
    >
      <div className="sidebar-approvals-row">
        <span>Approvals</span>
        {count > 0 && (
          <span
            className="sidebar-approvals-badge"
            data-testid="sidebar-approvals-badge"
          >
            {count}
          </span>
        )}
      </div>
    </div>
  );
}
