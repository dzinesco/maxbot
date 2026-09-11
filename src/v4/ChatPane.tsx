/*
 * v4 — ChatPane (S1)
 *
 * v3.7.17 Slice S1 — chat is real.
 *
 * Per the S1 brief:
 *   - Selecting a bot loads its latest conversation messages.
 *   - Send streams via existing onChunk/onDone/onError.
 *   - Stop cancels the run.
 *   - No history polling while a stream is active.
 *   - Keep boot at listBots + getSettings (App.tsx owns that).
 *   - No setInterval.
 *
 * Conversation lifecycle:
 *   - On mount, listConversations(bot.id). If the bot has any,
 *     pick the most-recently-updated and load its history.
 *     Otherwise create a fresh conversation. Either way the
 *     pane shows the real history — no orphan empty threads.
 *   - User types in Composer → sendMessage(convId, text, reqId).
 *     The response gives us assistant_message_id (server-side);
 *     we store it on the draft so Stop can cancel by id.
 *   - Chunk events with matching request_id append to the draft.
 *   - Done clears the draft's pending flag and refreshes history.
 *   - Stop calls stopMessage(assistant_message_id) — the server
 *     emits a Done event with finish_reason="stop" or similar;
 *     our existing onDone path handles the cleanup.
 *
 * No-history-poll-while-streaming:
 *   - The 15s recursive setTimeout below only fires when
 *     `draft?.pending` is false (no active stream).
 *   - When a stream is active, history is refreshed by the
 *     Done handler — exactly once, when the stream completes.
 *   - When no stream is active and the pane is idle, the
 *     15s tick keeps the message list current with the
 *     daemon's persisted state.
 *
 * Per-surface hard rules (v4):
 *   - Mount only when botId is provided.
 *   - Listeners paired with unlistens in the same useEffect.
 *   - No setInterval — all timers are recursive setTimeout.
 *   - No screenshot bytes / tool JSON / journal text in
 *     React state beyond the visible slice.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  createConversation,
  getMessages,
  listBots,
  listConversations,
  onChunk,
  onDone,
  onError,
  sendMessage,
  stopMessage,
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
  /** Local id; not the server's message id (we render locally). */
  id: string;
  /** Request id we generated; only events with matching id append. */
  requestId: string;
  /** Server-side assistant message id (from sendMessage response).
   *  Used as the argument to stopMessage. */
  assistantMessageId: string;
  /** Accumulated text from text-delta chunks. */
  text: string;
  /** True while the request is in flight. */
  pending: boolean;
}

function generateRequestId(): string {
  if (
    typeof crypto !== "undefined" &&
    typeof crypto.randomUUID === "function"
  ) {
    return crypto.randomUUID();
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2, 10)}`;
}

function pickLatestConversation(
  list: Conversation[],
  botId: string,
): Conversation | null {
  // listConversations already filters by bot_id server-side,
  // but be defensive: also filter on the client in case the
  // server returns a wider set.
  const forBot = list.filter((c) => c.bot_id === botId);
  if (forBot.length === 0) return null;
  // Sort by updated_at desc; fall back to created_at.
  const ts = (c: Conversation): number =>
    Date.parse(c.updated_at || c.created_at || "") || 0;
  return forBot.slice().sort((a, b) => ts(b) - ts(a))[0];
}

export function ChatPane({ bot }: ChatPaneProps) {
  const [conversation, setConversation] = useState<Conversation | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [composer, setComposer] = useState("");
  const [draft, setDraft] = useState<AssistantDraft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Track the current conversation id in a ref so listeners
  // and the history-refresh tick ignore stale state when the
  // pane re-mounts for a different bot.
  const convRef = useRef<string | null>(null);
  convRef.current = conversation?.id ?? null;
  // Track active stream so the history tick knows to back off.
  const pendingRef = useRef<boolean>(false);
  pendingRef.current = draft?.pending ?? false;

  // Mount: load latest conversation (or create one) + history +
  // register listeners. Runs once per bot.
  useEffect(() => {
    let cancelled = false;
    let unlistens: Array<() => void> = [];

    (async () => {
      try {
        const list = await listConversations(bot.id);
        if (cancelled) return;
        const latest = pickLatestConversation(list, bot.id);
        const conv = latest ?? (await createConversation(undefined, bot.id));
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
          return { ...prev, pending: false };
        });
        // Refresh history after the assistant message lands.
        // This is the only history fetch while a stream is in
        // flight — the Done event is exactly when the assistant
        // message is persisted, so we read it once here.
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
      unlistens.forEach((fn) => {
        try {
          fn();
        } catch {
          /* ignore — listener may not be ready yet */
        }
      });
    };
  }, [bot.id]);

  // Idle history refresh — single recursive setTimeout scoped
  // to this ChatPane; dies on unmount. Backoff-aware: skips
  // while a stream is active (the Done handler covers that
  // case), and runs every 15s when the pane is idle.
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = () => {
      if (cancelled) return;
      // v3.7.17 Slice S1: no history polling while a stream is
      // active. Skip the tick entirely; the Done handler will
      // refresh once when the stream completes.
      if (pendingRef.current) {
        timer = setTimeout(tick, 5_000); // re-check soon
        return;
      }
      // Refresh bot list (for last_active_at).
      listBots().catch(() => {
        /* ignore — best-effort */
      });
      // Refresh history for the active conversation.
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
    // Optimistic draft — the assistant_message_id is filled in
    // by sendMessage's response below; we use a placeholder until
    // then so Stop is disabled until the server has accepted the
    // request.
    setDraft({
      id: `local-${requestId}`,
      requestId,
      assistantMessageId: "",
      text: "",
      pending: true,
    });
    try {
      // Append the user message optimistically.
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
      const resp = await sendMessage(conversation.id, text, requestId);
      // Server accepted the request — now we have the real
      // assistant_message_id. Stop can fire from this point on.
      setDraft((prev) =>
        prev && prev.requestId === requestId
          ? { ...prev, assistantMessageId: resp.assistant_message_id }
          : prev,
      );
    } catch (e) {
      setError(`Send failed: ${e}`);
      setDraft(null);
    } finally {
      setBusy(false);
    }
  }, [composer, conversation, busy]);

  const handleStop = useCallback(async () => {
    if (!draft || !draft.pending || !draft.assistantMessageId) return;
    try {
      await stopMessage(draft.assistantMessageId);
      // The server emits Done with finish_reason reflecting the
      // stop; our onDone handler clears the draft's pending flag
      // and refreshes history.
    } catch (e) {
      setError(`Stop failed: ${e}`);
    }
  }, [draft]);

  const handleComposerKey = useCallback(
    (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSend();
      }
    },
    [handleSend],
  );

  const showStop = !!draft && draft.pending && !!draft.assistantMessageId;

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
        {showStop && (
          <button
            type="button"
            className="v4-chat-pane-stop danger small"
            onClick={handleStop}
            title="Stop the running assistant response"
          >
            Stop
          </button>
        )}
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
