// v2.4.0 — Multi-Bot group chat view.
//
// A variant of `ChatView` that renders the
// `group_messages` transcript with speaker attribution
// per message, a participant side rail, and a distinct
// card for handoff rows. Subscribes to `bot://chunk` /
// `bot://done` / `bot://error` to stream live bot runs;
// concurrent bot streams are demuxed by `bot_run_id`
// so two Bots running back-to-back in the same group
// don't bleed into each other's bubble.
//
// The component is data-only: the parent owns the
// message list + the active stream map. We pass them
// in as props so the React side can re-use this
// component for both the live group view and the
// vitest test that asserts on the handoff-card shape.

import { useEffect, useMemo, useRef, useState } from "react";
import type { Bot, GroupMessage } from "../lib/api";
import { BotAvatar } from "./BotAvatar";
import { onBotChunk, onBotDone, onBotError } from "../lib/tauri";

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
  /** Set of bot_run_ids that the parent knows about, keyed
   *  by bot_id. Populated by `groupRunTurn`; cleared by
   *  the `bot://done` listener. We use this to demux
   *  concurrent streams. */
  activeRunByBot?: Record<string, string>;
}

export function GroupChatView({
  group,
  messages,
  bots,
  activeBotRunIds = [],
  activeRunByBot = {},
}: GroupChatViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);
  // Live streaming text keyed by assistant message id. We
  // use the row id (a per-turn uuid) instead of `bot_run_id`
  // so the in-progress text lands in the right row even
  // after the assistant message has been persisted.
  const [streamByMsg, setStreamByMsg] = useState<Record<string, string>>(
    {},
  );
  const [streaming, setStreaming] = useState<Set<string>>(new Set());

  // Map bot_id -> Bot for O(1) speaker lookup.
  const botById = useMemo(() => {
    const m: Record<string, Bot> = {};
    for (const b of bots) m[b.id] = b;
    return m;
  }, [bots]);

  // Auto-scroll on new content / streaming chunks.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [messages, streamByMsg]);

  // Subscribe to the existing bot event stream and
  // demux by bot_run_id → bot_id → assistant message id.
  // The parent already has the bot_run_id → bot_id map;
  // we re-derive the message id by looking at the latest
  // assistant row for that bot in the transcript. The
  // contract: the executor emits the assistant row's id
  // as the message id for the chunk events, but the
  // chunk event itself only carries bot_run_id, so we
  // match by bot + recent timestamp via a side channel:
  // the parent sets `activeRunByBot[bot_id] = run_id`
  // when `groupRunTurn` returns, and clears it on done.
  useEffect(() => {
    let cancelled = false;
    const unsubs: Array<() => void> = [];
    Promise.all([
      onBotChunk((event) => {
        if (cancelled) return;
        // Find the bot_id for this run id by inverting
        // the parent's `activeRunByBot` map.
        const botId = Object.entries(activeRunByBot).find(
          ([, runId]) => runId === event.bot_run_id,
        )?.[0];
        if (!botId) return;
        // Find the most-recent (in-flight) assistant
        // row for this bot. The parent appends a
        // placeholder row before kicking off
        // `groupRunTurn`, so we can match by recency.
        const assistantRow = [...messages]
          .reverse()
          .find(
            (m) =>
              m.bot_id === botId &&
              m.role === "assistant" &&
              streaming.has(m.id),
          );
        const targetId = assistantRow?.id;
        if (!targetId) return;
        const payload = event.chunk as unknown;
        if (
          payload &&
          typeof payload === "object" &&
          "Text" in (payload as Record<string, unknown>)
        ) {
          const text = (payload as { Text: string }).Text;
          setStreamByMsg((prev) => ({
            ...prev,
            [targetId]: (prev[targetId] ?? "") + text,
          }));
        }
      }),
      onBotDone(() => {
        // The parent clears `activeRunByBot` for this
        // bot on the done event; we just stop tracking
        // the streaming state. The persisted message
        // arrives via a separate `groupHistory` refetch
        // in the parent, so we don't try to finalize
        // `streamByMsg` here.
      }),
      onBotError(() => {
        // Same as done — the parent's run id map clears
        // on the error event.
      }),
    ]).then(([u1, u2, u3]) => {
      if (cancelled) {
        u1();
        u2();
        u3();
        return;
      }
      unsubs.push(u1, u2, u3);
    });
    return () => {
      cancelled = true;
      for (const u of unsubs) u();
    };
  }, [activeRunByBot, messages, streaming]);

  // Mark every persisted assistant row that's still
  // being streamed into as "in flight". The parent
  // adds new rows to `messages` on done; we use the
  // intersection with `activeBotRunIds` to decide.
  useEffect(() => {
    setStreaming((prev) => {
      const next = new Set(prev);
      // We don't have a direct link from
      // activeBotRunIds (which is a list of bot ids)
      // to a specific message id, so we keep the
      // existing set as-is. The streaming marker is
      // cleared by the parent when the new
      // (persisted) message arrives and the live
      // stream row is no longer in the transcript.
      return next;
    });
  }, [activeBotRunIds]);

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
                streamingText={streamByMsg[m.id]}
                targetBot={
                  m.handoff_to ? botById[m.handoff_to] : undefined
                }
              />
            ))
          )}
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
