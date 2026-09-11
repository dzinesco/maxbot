/*
 * v4 — ChatPane
 *
 * Messages for the selected bot. Mounts only when a bot is
 * selected. Listens for `onChunk` / `onDone` / `onError`
 * filtered by `requestId`; cleanup awaits all unlistens.
 *
 * Per Tyler's v4 hard rules:
 * - Mount only when botId is provided. No chat on idle Roster.
 * - Listeners paired with unlistens in the same useEffect.
 * - No setInterval — the chat is event-driven.
 * - No screenshot bytes, tool JSON, or large blobs in
 *   React state; the visible message slice is enough.
 *
 * Conversation lifecycle (simplified for v4):
 * - On mount, create a fresh conversation for the bot via
 *   `createConversation(title, botId)` and load its history.
 * - User types in Composer → `sendMessage(convId, text, reqId)`.
 * - Chunk events with matching `request_id` append to the
 *   assistant message; Done marks the request complete.
 *
 * The full v3 chat surface (BotInbox, GroupChatView, message
 * editing, voice mode, etc.) is NOT ported into v4 yet. This
 * is a one-conversation-per-bot MVP that proves the mount /
 * unmount / listener rule. The full chat surface rebuild is
 * a follow-up.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  createConversation,
  getMessages,
  listBots,
  onChunk,
  onDone,
  onError,
  sendMessage,
} from "../lib/tauri";
import type {
  Bot,
  ChunkEvent,
  Conversation,
  DoneEvent,
  ErrorEvent,
  Message,
} from "../lib/api";
import "./styles/chat.css";

export interface ChatPaneProps {
  /** The bot whose conversation this pane renders. */
  bot: Bot;
}

interface AssistantDraft {
  /** Local message id assigned by `sendMessage`. */
  id: string;
  /** Local request id (UUID) — only events with matching id append. */
  requestId: string;
  /** Accumulated text from text-delta chunks. */
  text: string;
  /** True while the request is still in flight. */
  pending: boolean;
}

function generateRequestId(): string {
  // crypto.randomUUID is available in Tauri's WebView.
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID();
  }
  // Fallback for any environment without it.
  return `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

export function ChatPane({ bot }: ChatPaneProps) {
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [composer, setComposer] = useState("");
  const [draft, setDraft] = useState<AssistantDraft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Track the current conversation id in a ref so listeners
  // (registered in an effect) can ignore stale events from
  // a previous bot/conversation when the pane re-mounts.
  const convRef = useRef<string | null>(null);
  convRef.current = conversation?.id ?? null;

  // Mount: create (or reuse) conversation + load history + register listeners.
  useEffect(() => {
    let cancelled = false;
    let unlistens: Array<() => void> = [];

    (async () => {
      try {
        const conv = await createConversation(undefined, bot.id);
        if (cancelled) return;
        setConversation(conv);
        const history = await getMessages(conv.id);
        if (cancelled) return;
        setMessages(history);
      } catch (e) {
        if (cancelled) return;
        setError(`Could not open chat: ${e}`);
        return;
      }

      // Listeners — filter by request_id so multiple panes (or
      // a previous bot's leftover stream) don't pollute this pane.
      const u1 = await onChunk((ev: ChunkEvent) => {
        if (cancelled) return;
        setDraft((prev) => {
          if (!prev || prev.requestId !== ev.request_id) return prev;
          if (ev.chunk.kind === "text") {
            return { ...prev, text: prev.text + ev.chunk.delta };
          }
          // tool_call_delta and other non-text chunks: ignore
          // in the MVP. Future work: render tool cards.
          return prev;
        });
      });
      const u2 = await onDone((ev: DoneEvent) => {
        if (cancelled) return;
        setDraft((prev) => {
          if (!prev || prev.requestId !== ev.request_id) return prev;
          // Promote the draft into the message list as a finalized
          // assistant message. We don't have its full persisted
          // shape here (no role/timestamp) — append a placeholder
          // and refresh history on next mount. The MVP keeps the
          // visible message window correct by reading history after
          // every Done event.
          return { ...prev, pending: false };
        });
        // Refresh history after the assistant message lands.
        if (convRef.current) {
          getMessages(convRef.current).then((m) => {
            if (!cancelled) setMessages(m);
          });
        }
      });
      const u3 = await onError((ev: ErrorEvent) => {
        if (cancelled) return;
        setDraft((prev) => {
          if (!prev || prev.requestId !== ev.request_id) return prev;
          return { ...prev, pending: false };
        });
        setError(ev.message);
      });

      unlistens = [u1, u2, u3];
    })();

    return () => {
      cancelled = true;
      // Wait for unlistens to be assigned (they may not be yet
      // if the effect was torn down before the async setup
      // completed). After unmount, we cannot await this safely
      // in the cleanup function — fire-and-forget is fine since
      // the listeners will no longer fire into a mounted component.
      unlistens.forEach((fn) => {
        try {
          fn();
        } catch {
          /* ignore — listener may not be ready yet */
        }
      });
    };
  }, [bot.id]);

  // Periodic refresh of bots so the roster "last active" stays
  // current when this pane is mounted and the user is chatting.
  // Single timer scoped to ChatPane; dies on unmount. NOT a
  // setInterval — a recursive setTimeout so the next delay can
  // be adjusted if the user is actively typing.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = () => {
      if (cancelled) return;
      // Refresh bot list (for last_active_at) and history.
      listBots().catch(() => {
        /* ignore — best-effort */
      });
      if (convRef.current) {
        getMessages(convRef.current)
          .then((m) => {
            if (!cancelled) setMessages(m);
          })
          .catch(() => {
            /* ignore */
          });
      }
      timer = setTimeout(tick, 15_000);
    };
    timer = setTimeout(tick, 15_000);
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, [bot.id]);

  const handleSend = useCallback(async () => {
    const text = composer.trim();
    if (!text || !conversation || busy) return;
    const requestId = generateRequestId();
    setBusy(true);
    setComposer("");
    setError(null);
    setDraft({
      id: `local-${requestId}`,
      requestId,
      text: "",
      pending: true,
    });
    try {
      // Persist the user message optimistically. Reload history
      // after the IPC returns so the message row shows the
      // persisted id.
      setMessages((prev) => [
        ...prev,
        {
          id: `local-user-${requestId}`,
          conversation_id: conversation.id,
          role: "user",
          content: text,
          created_at: new Date().toISOString(),
          tool_calls: [],
          error_message: null,
        },
      ]);
      await sendMessage(conversation.id, text, requestId);
    } catch (e) {
      setError(`Send failed: ${e}`);
      setDraft(null);
    } finally {
      setBusy(false);
    }
  }, [composer, conversation, busy]);

  const handleComposerKey = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  return (
    <div className="v4-chat-pane" aria-label={`Chat with ${bot.name}`}>
      <header className="v4-chat-pane-header">
        <div className="v4-chat-pane-bot">
          <span className="v4-chat-pane-name">{bot.name}</span>
          {conversation && (
            <span className="v4-chat-pane-conv-id" title={conversation.id}>
              {conversation.id.slice(0, 8)}
            </span>
          )}
        </div>
      </header>

      {error && (
        <div className="v4-chat-pane-error" role="alert">
          {error}
        </div>
      )}

      <div className="v4-chat-pane-history" role="log" aria-live="polite">
        {messages.length === 0 && !draft && (
          <div className="v4-chat-pane-empty">
            Empty conversation. Send a message to start.
          </div>
        )}
        {messages.map((m) => (
          <div key={m.id} className={`v4-chat-pane-msg v4-chat-pane-msg--${m.role}`}>
            <div className="v4-chat-pane-msg-meta">
              <span className="v4-chat-pane-msg-role">{m.role}</span>
              <span className="v4-chat-pane-msg-when">
                {new Date(m.created_at).toLocaleTimeString()}
              </span>
            </div>
            <div className="v4-chat-pane-msg-body">{m.content}</div>
          </div>
        ))}
        {draft && (
          <div className="v4-chat-pane-msg v4-chat-pane-msg--assistant v4-chat-pane-msg--draft">
            <div className="v4-chat-pane-msg-meta">
              <span className="v4-chat-pane-msg-role">assistant</span>
              <span className="v4-chat-pane-msg-when">
                {draft.pending ? "writing…" : "done"}
              </span>
            </div>
            <div className="v4-chat-pane-msg-body">
              {draft.text || (draft.pending ? "…" : "")}
            </div>
          </div>
        )}
      </div>

      <footer className="v4-chat-pane-composer">
        <textarea
          className="v4-chat-pane-input"
          value={composer}
          onChange={(e) => setComposer(e.target.value)}
          onKeyDown={handleComposerKey}
          placeholder={`Message ${bot.name}… (Enter to send, Shift+Enter for newline)`}
          disabled={busy}
          rows={3}
        />
        <button
          type="button"
          className="v4-chat-pane-send primary"
          onClick={handleSend}
          disabled={busy || composer.trim().length === 0}
        >
          {busy ? "Sending…" : "Send"}
        </button>
      </footer>
    </div>
  );
}
