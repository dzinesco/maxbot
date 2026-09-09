import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Bot, Computer, Conversation } from "../lib/api";
import { computerGet } from "../lib/tauri";

interface SidebarProps {
  conversations: Conversation[];
  activeId: string | null;
  bots: Bot[];
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onOpenSettings: () => void;
  status: "online" | "missing" | "checking";
  /** Slot for the BotsPanel component (rendered between conversation
   *  list and the footer). */
  botPanel?: ReactNode;
  searchQuery: string;
  onSearchChange: (q: string) => void;
  /** When set, the sidebar shows a one-line snippet per matching
   *  conversation. Map keys are conversation IDs. */
  searchView: Record<
    string,
    { count: number; firstSnippet: string; mostRecentAt: string }
  > | null;
  /** Optional pre-fetched map of bot id → computer. Pass this in to
   * skip the per-conversation `computerGet` round-trip; the sidebar
   * will still merge in the most recent row when given both a prop
   * and an async lookup. Slice E (Agent D) will switch to passing
   * the prop once the roster is the data source of truth. */
  computersByBot?: Record<string, Computer>;
}

export function Sidebar({
  conversations,
  activeId,
  bots,
  onSelect,
  onNew,
  onDelete,
  onRename,
  onOpenSettings,
  status,
  botPanel,
  searchQuery,
  onSearchChange,
  searchView,
  computersByBot,
}: SidebarProps) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  // Locally-merged `bot id → computer` map. We seed it with the
  // `computersByBot` prop (Slice E will pass this in) and fall
  // back to a per-bot `computerGet` round-trip for any bot
  // missing from the prop. This keeps Slice C independent of
  // the Slice E refactor (the sidebar still works without any
  // parent-supplied data, just slower on the first render).
  const [localComputers, setLocalComputers] = useState<
    Record<string, Computer | null>
  >({});
  const searchRef = useRef<HTMLInputElement>(null);
  const botsById = new Map(bots.map((b) => [b.id, b]));
  const searching = searchView !== null;

  // Fetch the computer row for every Bot that the parent
  // hasn't already told us about. One-shot, no polling — the
  // ComputerPanel itself drives live updates.
  useEffect(() => {
    const missing = bots
      .map((b) => b.id)
      .filter(
        (id) =>
          computersByBot === undefined ||
          !(id in computersByBot),
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

  // The merged view: prop wins over local. Conversations
  // without a Bot don't need a computer chip.
  const mergedComputers: Record<string, Computer | null> = {};
  for (const b of bots) {
    if (computersByBot && b.id in computersByBot) {
      mergedComputers[b.id] = computersByBot[b.id] ?? null;
    } else if (b.id in localComputers) {
      mergedComputers[b.id] = localComputers[b.id];
    }
  }

  // Cmd/Ctrl+F focuses the search box, just like every other chat
  // app. Escape clears it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "f") {
        e.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
      } else if (e.key === "Escape" && document.activeElement === searchRef.current) {
        onSearchChange("");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onSearchChange]);

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <div className="brand">
          <span className="brand-mark" aria-hidden>
            M
          </span>
          <span className="brand-name">MaxBot</span>
        </div>
        <button onClick={onNew} className="primary">
          + New chat
        </button>
      </div>
      <div className="sidebar-search">
        <input
          ref={searchRef}
          type="text"
          value={searchQuery}
          onChange={(e) => onSearchChange(e.target.value)}
          placeholder="Search messages…  ⌘F"
        />
        {searching && (
          <button
            className="ghost small"
            onClick={() => onSearchChange("")}
            title="Clear search (Esc)"
          >
            ✕
          </button>
        )}
      </div>
      <div className="conversation-list">
        {conversations.length === 0 && (
          <div
            style={{
              color: "var(--text-3)",
              fontSize: 12,
              padding: "12px 10px",
            }}
          >
            {searching
              ? `No matches for "${searchQuery}".`
              : "No conversations yet."}
          </div>
        )}
        {conversations.map((c) => {
          const bot = c.bot_id ? botsById.get(c.bot_id) : undefined;
          const view = searchView?.[c.id];
          return (
            <div
              key={c.id}
              className={`conversation-item${activeId === c.id ? " active" : ""}`}
              onClick={() => {
                if (editingId === c.id) return;
                onSelect(c.id);
              }}
              onDoubleClick={() => {
                if (searching) return;
                setEditingId(c.id);
                setDraft(c.title);
              }}
              title="Double-click to rename"
            >
              {bot && (
                <span
                  className="conv-bot-dot"
                  style={bot.color ? { background: bot.color } : undefined}
                  title={bot.name}
                >
                  {bot.icon || "🤖"}
                </span>
              )}
              {bot && mergedComputers[bot.id] && (
                <ComputerChip computer={mergedComputers[bot.id]!} />
              )}
              {editingId === c.id ? (
                <input
                  autoFocus
                  value={draft}
                  onChange={(e) => setDraft(e.target.value)}
                  onBlur={() => {
                    const trimmed = draft.trim();
                    if (trimmed && trimmed !== c.title) onRename(c.id, trimmed);
                    setEditingId(null);
                  }}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") {
                      (e.target as HTMLInputElement).blur();
                    } else if (e.key === "Escape") {
                      setEditingId(null);
                    }
                  }}
                  style={{ flex: 1, fontSize: 13, padding: "2px 6px" }}
                  onClick={(e) => e.stopPropagation()}
                />
              ) : (
                <div className="conversation-text">
                  <span className="title">{c.title}</span>
                  {view && (
                    <>
                      <span className="conv-snippet">
                        {view.firstSnippet}
                      </span>
                      <span className="conv-match-count">
                        {view.count} match{view.count === 1 ? "" : "es"}
                      </span>
                    </>
                  )}
                </div>
              )}
              <span
                className="actions"
                onClick={(e) => e.stopPropagation()}
              >
                <button
                  className="danger"
                  onClick={() => {
                    if (
                      confirm(
                        `Delete "${c.title}"? This cannot be undone.`,
                      )
                    ) {
                      onDelete(c.id);
                    }
                  }}
                >
                  Delete
                </button>
              </span>
            </div>
          );
        })}
      </div>
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

/** Small chip rendered next to a Bot's icon in the conversation
 * list, indicating the Bot has a provisioned computer. The
 * color mirrors the `computer-panel__status-dot` palette so the
 * visual language is consistent between the sidebar chip and
 * the full panel. */
function ComputerChip({ computer }: { computer: Computer | null }) {
  if (!computer) return null;
  const state = computer.state;
  const label = (() => {
    switch (state) {
      case "running":
        return "PC · running";
      case "stopped":
        return "PC · stopped";
      case "provisioning":
        return "PC · provisioning";
      case "error":
        return "PC · error";
      default:
        return "PC";
    }
  })();
  return (
    <span
      className={`sidebar__computer-chip sidebar__computer-chip--${stateClassChip(state)}`}
      title={`Bot computer: ${label}`}
      data-state={state}
    >
      <span className="sidebar__computer-chip-dot" />
      <span className="sidebar__computer-chip-label">{label}</span>
    </span>
  );
}

function stateClassChip(state: string | null | undefined): string {
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
