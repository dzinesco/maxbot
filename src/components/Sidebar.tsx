import { useEffect, useRef, useState, type ReactNode } from "react";
import type { Bot, Conversation } from "../lib/api";

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
}: SidebarProps) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const searchRef = useRef<HTMLInputElement>(null);
  const botsById = new Map(bots.map((b) => [b.id, b]));
  const searching = searchView !== null;

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
        >
          <span className="indicator" />
          {status === "online"
            ? "MiniMax ready"
            : status === "missing"
              ? "API key not set"
              : "Checking…"}
        </div>
        <button onClick={onOpenSettings}>Settings</button>
      </div>
    </aside>
  );
}
