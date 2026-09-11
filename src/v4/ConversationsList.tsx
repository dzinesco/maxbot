/*
 * v4 — ConversationsList (S2.6)
 *
 * v3.7.17 Slice S2.6 — display-only.
 *
 * - Conversations are now fetched + held by App.tsx (lifted
 *   state). The list is passed in via the `conversations`
 *   prop. App also handles the "select bot = auto-pick
 *   latest, or create new" path.
 * - The "+ New chat" button stays — it's the manual override
 *   when the user wants a fresh thread on the same bot.
 * - Thread titles come from the Rust title field, which App
 *   updates to the first user message via renameConversation
 *   after a send. No more "New chat" label after a send.
 *
 * S2.5 carried the list fetch in this component. The fetch
 * is gone — the brief ("Selecting a bot auto-selects latest
 * thread; if none, create one") forced the lifting.
 *
 * Hard rules:
 *   - No IPC. Pure render.
 *   - No setInterval.
 */

import "./styles/conversations.css";

export interface ConversationsListProps {
  /** The bot whose conversations to list. null = no bot selected. */
  botId: string | null;
  /** Conversations for the selected bot, sorted by updated_at desc
   *  (App.tsx sorts before passing). `null` = loading. */
  conversations: Conversation[] | null;
  /** The currently-active conversation id (highlighted in the list). */
  selectedConvId: string | null;
  /** Called when the user clicks a conversation row. */
  onSelectConv: (convId: string) => void;
  /** Called when the user clicks "+ New chat". App.tsx creates
   *  + selects. */
  onNewChat: () => void;
  /** True while a new chat is being created (disables button). */
  newChatBusy: boolean;
}

// Local type re-export so we don't import the whole `Conversation`
// interface from lib/api just for the prop. Mirrors src/lib/api.ts.
interface Conversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  bot_id: string | null;
}

function relativeTime(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const diff = Date.now() - t;
  if (diff < 60_000) return "just now";
  if (diff < 60 * 60_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 24 * 60 * 60_000) return `${Math.floor(diff / (60 * 60_000))}h ago`;
  return `${Math.floor(diff / (24 * 60 * 60_000))}d ago`;
}

function titleFor(c: Conversation): string {
  return (c.title && c.title.trim()) || "";
}

export function ConversationsList({
  botId,
  conversations,
  selectedConvId,
  onSelectConv,
  onNewChat,
  newChatBusy,
}: ConversationsListProps) {
  if (!botId) return null;

  const loading = conversations === null;
  const list = conversations ?? [];

  return (
    <div className="v4-conv-list" aria-label="Conversations">
      {loading && (
        <div className="v4-conv-list-loading">Loading…</div>
      )}

      {!loading && list.length === 0 && (
        <div className="v4-conv-list-empty">No threads yet.</div>
      )}

      {!loading && list.length > 0 && (
        <ul className="v4-conv-list-rows" role="list">
          {list.map((c) => {
            const selected = c.id === selectedConvId;
            const title = titleFor(c) || "New chat";
            return (
              <li key={c.id}>
                <button
                  type="button"
                  className={`v4-conv-list-row ${selected ? "is-selected" : ""}`}
                  onClick={() => onSelectConv(c.id)}
                  aria-current={selected ? "true" : undefined}
                  title={c.id}
                >
                  <span className="v4-conv-list-row-title">{title}</span>
                  <span className="v4-conv-list-row-when">
                    {relativeTime(c.updated_at)}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}

      {/* "+ New chat" — manual override for a fresh thread on the
          same bot. The auto-create path in App.tsx already gives
          the user a new thread on bot select when the bot has
          none; this button is for "I want a second one". */}
      <button
        type="button"
        className="v4-conv-list-row v4-conv-list-new"
        onClick={onNewChat}
        disabled={newChatBusy}
        title="Start a new conversation with this bot"
      >
        <span className="v4-conv-list-new-label">
          {newChatBusy ? "…" : "+ New chat"}
        </span>
      </button>
    </div>
  );
}
