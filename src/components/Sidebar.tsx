import { useState } from "react";
import type { Conversation } from "../lib/api";

interface SidebarProps {
  conversations: Conversation[];
  activeId: string | null;
  onSelect: (id: string) => void;
  onNew: () => void;
  onDelete: (id: string) => void;
  onRename: (id: string, title: string) => void;
  onOpenSettings: () => void;
  status: "online" | "missing" | "checking";
}

export function Sidebar({
  conversations,
  activeId,
  onSelect,
  onNew,
  onDelete,
  onRename,
  onOpenSettings,
  status,
}: SidebarProps) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <div className="brand">
          <span className="brand-dot" />
          MaxBot
        </div>
        <button onClick={onNew} className="primary">
          + New chat
        </button>
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
            No conversations yet.
          </div>
        )}
        {conversations.map((c) => (
          <div
            key={c.id}
            className={`conversation-item${activeId === c.id ? " active" : ""}`}
            onClick={() => {
              if (editingId === c.id) return;
              onSelect(c.id);
            }}
            onDoubleClick={() => {
              setEditingId(c.id);
              setDraft(c.title);
            }}
            title="Double-click to rename"
          >
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
              <span className="title">{c.title}</span>
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
        ))}
      </div>
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
