/*
 * v4 — ConversationsList (S2.5 — Grok Bot look)
 *
 * v3.7.17 Slice S2 — conversation list per selected bot.
 * S2.5 — drop the uppercase "Threads" label. The list sits
 *        under the bot name; the bot name above already
 *        identifies what this section is.
 *
 * - Fetches `listConversations(bot.id)` ONCE when the bot is
 *   selected (NOT at boot — boot stays at listBots + getSettings).
 * - Re-fetches if `bot.id` changes (user picked a different bot).
 * - Renders one row per conversation, sorted by `updated_at` desc.
 * - Clicking a row calls `onSelectConv(convId)`.
 * - No setInterval. The list is fetched once per bot.
 */

import { useEffect, useRef, useState } from "react";
import { listConversations } from "../lib/tauri";
import type { Conversation } from "../lib/api";
import "./styles/conversations.css";

export interface ConversationsListProps {
  /** The bot whose conversations to list. null = no bot selected. */
  botId: string | null;
  /** The currently-active conversation id (highlighted in the list). */
  selectedConvId: string | null;
  /** Called when the user clicks a conversation row. */
  onSelectConv: (convId: string) => void;
  /** Called when the user clicks "New chat". App.tsx creates + selects. */
  onNewChat: () => void;
  /** True while a new chat is being created (disables button). */
  newChatBusy: boolean;
}

function relativeTime(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "—";
  const diff = Date.now() - t;
  if (diff < 60_000) return "just now";
  if (diff < 60 * 60_000) return `${Math.floor(diff / 60_000)}m ago`;
  if (diff < 24 * 60 * 60_000) return `${Math.floor(diff / (60 * 60_000))}h ago`;
  return `${Math.floor(diff / (24 * 60 * 60_000))}d ago`;
}

function titleFor(c: Conversation): string {
  return (c.title && c.title.trim()) || "Untitled";
}

export function ConversationsList({
  botId,
  selectedConvId,
  onSelectConv,
  onNewChat,
  newChatBusy,
}: ConversationsListProps) {
  const [conversations, setConversations] = useState<Conversation[] | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  // Track which (botId, set of conversation ids) we've already
  // fetched. We refetch when:
  //   - botId changes, OR
  //   - selectedConvId changes to an id NOT in the loaded list
  //     (the new-chat path: App.tsx creates a conv + selects it
  //     before we have it in our list).
  // The ref lets the effect skip redundant fetches when the user
  // clicks a thread we already have.
  const lastFetchedRef = useRef<{
    botId: string | null;
    convIds: Set<string>;
  }>({ botId: null, convIds: new Set() });

  useEffect(() => {
    if (!botId) {
      setConversations(null);
      lastFetchedRef.current = { botId: null, convIds: new Set() };
      return;
    }
    const prev = lastFetchedRef.current;
    const needsFetch =
      prev.botId !== botId ||
      (selectedConvId !== null && !prev.convIds.has(selectedConvId));
    if (!needsFetch) return;

    let cancelled = false;
    setConversations(null); // clear stale list while loading
    listConversations(botId)
      .then((list) => {
        if (cancelled) return;
        const ts = (c: Conversation): number =>
          Date.parse(c.updated_at || c.created_at || "") || 0;
        const sorted = list.slice().sort((a, b) => ts(b) - ts(a));
        setConversations(sorted);
        lastFetchedRef.current = {
          botId,
          convIds: new Set(list.map((c) => c.id)),
        };
      })
      .catch((e) => {
        if (cancelled) return;
        setError(String(e));
        setConversations([]);
      });
    return () => {
      cancelled = true;
    };
  }, [botId, selectedConvId]);

  if (!botId) return null;

  // S2.5 — no "Threads" header. The "+ New chat" row always sits
  // below the threads (or in the empty state) so users can start
  // a fresh conversation even when no threads exist yet.
  return (
    <div className="v4-conv-list" aria-label="Conversations">
      {error && (
        <div className="v4-conv-list-error" role="alert">
          {error}
        </div>
      )}

      {conversations === null && !error && (
        <div className="v4-conv-list-loading">Loading…</div>
      )}

      {conversations && conversations.length === 0 && !error && (
        <div className="v4-conv-list-empty">No threads yet.</div>
      )}

      {conversations && conversations.length > 0 && (
        <ul className="v4-conv-list-rows" role="list">
          {conversations.map((c) => {
            const selected = c.id === selectedConvId;
            return (
              <li key={c.id}>
                <button
                  type="button"
                  className={`v4-conv-list-row ${selected ? "is-selected" : ""}`}
                  onClick={() => onSelectConv(c.id)}
                  aria-current={selected ? "true" : undefined}
                  title={c.id}
                >
                  <span className="v4-conv-list-row-title">{titleFor(c)}</span>
                  <span className="v4-conv-list-row-when">
                    {relativeTime(c.updated_at)}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      )}

      {/* "+ New chat" — always visible when a bot is selected.
          Below the threads (or below the empty-state message). */}
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
