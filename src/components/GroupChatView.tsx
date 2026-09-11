// Group transcripts are persisted by the parent; live chunks are temporary
// and released when each command finishes or this view unmounts.

import { useEffect, useMemo, useRef, useState } from "react";
import type { Bot, GroupMessage } from "../lib/api";
import { BotAvatar } from "./BotAvatar";
import { onBotChunk } from "../lib/tauri";

interface GroupChatViewProps {
  /** The group metadata. The header reads `chat.name` and
   *  the rail reads `member_bot_ids`. */
  group: { id: string; name: string; owner_bot_id: string };
  /** The transcript, oldest-first. The component re-renders
   *  when this changes (e.g. a new assistant or handoff row
   *  lands). */
  messages: GroupMessage[];
  /** All known Bots. Used to render the participant list
   *  and to resolve speaker ids to display names + avatars. */
  bots: Bot[];
  /** Bot ids currently mid-turn. The component renders the
   *  streaming text into the matching assistant row and
   *  shows a `…` cursor. */
  activeBotRunIds?: string[];
  /** Bots whose group command is awaiting completion. */
  activeRunByBot?: Record<string, string>;
}

export function GroupChatView({
  group,
  messages,
  bots,
  activeBotRunIds: _activeBotRunIds = [],
  activeRunByBot = {},
}: GroupChatViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const [live, setLive] = useState<Record<string, { runId: string; text: string }>>({});
  const activeRef = useRef(activeRunByBot);
  activeRef.current = activeRunByBot;
  const botById = useMemo(() => Object.fromEntries(bots.map((bot) => [bot.id, bot])), [bots]);

  useEffect(() => {
    setLive((previous) => Object.fromEntries(Object.entries(previous).filter(([id]) => id in activeRunByBot)));
  }, [activeRunByBot]);

  useEffect(() => {
    let cancelled = false;
    let unsubscribe: (() => void) | undefined;
    setLive({});
    onBotChunk((event) => {
      if (cancelled || !(event.bot_id in activeRef.current) || event.chunk.kind !== "text") return;
      const delta = event.chunk.delta;
      setLive((previous) => {
        const old = previous[event.bot_id];
        return { ...previous, [event.bot_id]: {
          runId: event.bot_run_id,
          text: (old?.runId === event.bot_run_id ? old.text : "") + delta,
        } };
      });
    }).then((unlisten) => {
      if (cancelled) unlisten();
      else unsubscribe = unlisten;
    }).catch((error) => console.warn("Group stream connection failed:", error));
    return () => { cancelled = true; unsubscribe?.(); };
  }, [group.id]);

  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, live]);

  return (
    <div className="chat-view group-chat-view" data-testid="group-chat-view">
      <div className="chat-view__header" data-testid="group-chat-header">
        <div className="chat-view__header-meta">
          <div className="chat-view__header-name">{group.name}</div>
          <div className="chat-view__header-state" data-state="group">
            Group · {bots.filter((b) => group && b.id).length} Bots
          </div>
        </div>
      </div>
      <div className="group-chat-layout">
        <div className="group-chat-rail" data-testid="group-chat-rail">
          <div className="group-chat-rail-label">Participants</div>
          {bots
            .filter((b) => b.id)
            .map((b) => (
              <div key={b.id} className="group-chat-rail-bot">
                <BotAvatar bot={b} size={20} />
                <span>{b.name}</span>
              </div>
            ))}
        </div>
        <div className="messages group-chat-messages" ref={scrollRef}>
          {messages.length === 0 ? (
            <div className="main-empty group-chat-empty">
              <div className="main-empty-mark">M</div>
              <h1>Group: {group.name}</h1>
              <p className="main-empty-tagline">
                Mention a Bot with <code>@BotName</code> to send a
                message to one of the group members.
              </p>
            </div>
          ) : (
            messages.map((m) => (
              <GroupMessageRow
                key={m.id}
                message={m}
                bot={m.bot_id ? botById[m.bot_id] : undefined}
                targetBot={
                  m.handoff_to ? botById[m.handoff_to] : undefined
                }
              />
            ))
          )}
          {Object.keys(activeRunByBot).map((botId) => (
            <div className="message assistant" key={`live-${botId}`} data-testid="group-live-response">
              <div className="avatar">{botById[botId]?.name ?? botId}</div>
              <div className="body">{live[botId]?.text || "Thinking…"}</div>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

// ---- One row in the transcript ----

function GroupMessageRow({
  message,
  bot,
  streamingText,
  targetBot,
}: {
  message: GroupMessage;
  bot?: Bot;
  streamingText?: string;
  targetBot?: Bot;
}) {
  if (message.role === "handoff") {
    // The Rust side persists the resolved target Bot
    // id in `handoff_to`; we look up the matching
    // Bot so the card displays the name (e.g.
    // "Writer") instead of the id (e.g. "b2"). If
    // we can't resolve it (shouldn't happen, but
    // defensive), fall back to the id so the user
    // still sees something.
    const targetName = targetBot?.name ?? message.handoff_to ?? "?";
    return (
      <div className="group-handoff-card" data-testid="group-handoff-card">
        <div className="group-handoff-card-label">
          → <strong>@{targetName}</strong> handoff
        </div>
        <div className="group-handoff-card-body">
          {message.content.slice(0, 240)}
          {message.content.length > 240 ? "…" : ""}
        </div>
        {bot && (
          <div className="group-handoff-card-from">from {bot.name}</div>
        )}
      </div>
    );
  }
  if (message.role === "user") {
    return (
      <div className="message user" data-testid="group-message-user">
        <div className="avatar" aria-hidden>U</div>
        <div className="body">{message.content}</div>
      </div>
    );
  }
  // assistant
  const display = streamingText ?? message.content;
  return (
    <div className="message assistant" data-testid="group-message-assistant">
      {bot ? (
        <BotAvatar bot={bot} size={28} />
      ) : (
        <div className="avatar" aria-hidden>M</div>
      )}
      <div className="body">
        <div className="group-message-speaker">{bot?.name ?? "Assistant"}</div>
        <div className="group-message-body">{display}</div>
      </div>
    </div>
  );
}
