/*
 * v4 — ChatPane (S2)
 *
 * v3.7.17 Slice S2 — session, not a demo.
 *
 * Per the S2 brief:
 *   - New chat button creates a conversation for the selected bot
 *     and switches to it (handled in App.tsx; this pane just
 *     renders whatever conversationId is passed in).
 *   - Conversation list for the selected bot only, fetched on
 *     bot select, not at boot (handled by ConversationsList).
 *   - Clicking a thread loads that conversation (App.tsx sets
 *     `selectedConvId`; this pane reacts).
 *
 * What changed from S1:
 *   - Pane now takes `conversationId` as a prop. No more
 *     auto-loading the latest conversation on mount — that was
 *     S1's "session, not a demo" placeholder.
 *   - When `conversationId` is null, the pane shows a
 *     "select a thread or start a new chat" empty state.
 *   - When `conversationId` changes, history is reloaded for
 *     that conversation; the active stream is unaffected
 *     (request_id filtering).
 *   - Listeners (onChunk/onDone/onError) are still mounted
 *     with the bot; switching conversations doesn't re-register
 *     listeners. The pending stream (if any) continues across
 *     thread switches — but S1 already prevents history polling
 *     while streaming, so the stale state is bounded.
 *
 * Hard rules (v4):
 *   - Mount only when botId is provided.
 *   - Listeners paired with unlistens in the same useEffect.
 *   - No setInterval — all timers are recursive setTimeout.
 *   - No history polling while a stream is active.
 *   - No screenshot bytes / tool JSON / journal text in
 *     React state beyond the visible slice.
 */

import { useCallback, useEffect, useRef, useState } from "react";
import {
  getMessages,
  listBots,
  onChunk,
  onDone,
  onError,
  sendMessage,
  stopMessage,
} from "../lib/tauri";
import type {
  Bot,
  ChunkEvent,
  DoneEvent,
  ErrorEvent,
  Message,
} from "../lib/api";
import "./styles/chat.css";

export interface ChatPaneProps {
  /** The bot whose conversation this pane renders. */
  bot: Bot;
  /**
   * The conversation id to render. When null, the pane shows an
   * empty state — the user must pick a thread or click "New chat"
   * in App.tsx (S2 contract).
   */
  conversationId: string | null;
}

interface AssistantDraft {
  id: string;
  requestId: string;
  assistantMessageId: string;
  text: string;
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

export function ChatPane({ bot, conversationId }: ChatPaneProps) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [composer, setComposer] = useState("");
  const [draft, setDraft] = useState<AssistantDraft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Track the current conversation id in a ref so listeners
  // and the history-refresh tick ignore stale state when the
  // conversation changes mid-stream.
  const convRef = useRef<string | null>(null);
  convRef.current = conversationId;
  // Track active stream so the history tick knows to back off.
  const pendingRef = useRef<boolean>(false);
  pendingRef.current = draft?.pending ?? false;

  // History load — keyed on conversationId. When the user
  // switches threads via App.tsx, this fires and reloads.
  useEffect(() => {
    if (!conversationId) {
      setMessages([]);
      return;
    }
    let cancelled = false;
    getMessages(conversationId)
      .then((m) => {
        if (!cancelled) setMessages(m);
      })
      .catch((e) => {
        if (!cancelled) setError(`Could not load chat: ${e}`);
      });
    return () => {
      cancelled = true;
    };
  }, [conversationId]);

  // Listeners — registered once per bot, not per conversation.
  // Filter by request_id so a previous bot's leftover stream
  // doesn't pollute the new bot's pane.
  useEffect(() => {
    let cancelled = false;
    let unlistens: Array<() => void> = [];

    (async () => {
      const u1 = await onChunk((ev: ChunkEvent) => {
        if (cancelled) return;
        setDraft((prev) => {
          if (!prev || prev.requestId !== ev.request_id) return prev;
          if (ev.chunk.kind === "text") {
            return { ...prev, text: prev.text + ev.chunk.delta };
          }
          return prev;
        });
      });
      const u2 = await onDone((ev: DoneEvent) => {
        if (cancelled) return;
        setDraft((prev) => {
          if (!prev || prev.requestId !== ev.request_id) return prev;
          return { ...prev, pending: false };
        });
        // Done is the single history refresh while a stream is
        // in flight (S1 rule). The conversation id at Done time
        // is `convRef.current` — i.e. whatever thread the user is
        // on when the stream ends.
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
          /* ignore */
        }
      });
    };
  }, [bot.id]);

  // Idle history refresh — recursive setTimeout scoped to
  // this ChatPane; dies on unmount. Backoff-aware: skips
  // while a stream is active (Done handler covers that).
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = () => {
      if (cancelled) return;
      if (!conversationId) {
        timer = setTimeout(tick, 15_000);
        return;
      }
      // v3.7.17 S1/S2: no history polling while a stream is
      // active. Re-check in 5s and resume the cadence after.
      if (pendingRef.current) {
        timer = setTimeout(tick, 5_000);
        return;
      }
      listBots().catch(() => {
        /* ignore */
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
  }, [bot.id, conversationId]);

  const handleSend = useCallback(async () => {
    const text = composer.trim();
    if (!text || !conversationId || busy) return;
    const requestId = generateRequestId();
    setBusy(true);
    setComposer("");
    setError(null);
    setDraft({
      id: `local-${requestId}`,
      requestId,
      assistantMessageId: "",
      text: "",
      pending: true,
    });
    try {
      setMessages((prev) => [
        ...prev,
        {
          id: `local-user-${requestId}`,
          conversation_id: conversationId,
          role: "user",
          content: text,
          created_at: new Date().toISOString(),
          tool_calls: [],
          error_message: null,
        },
      ]);
      const resp = await sendMessage(conversationId, text, requestId);
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
  }, [composer, conversationId, busy]);

  const handleStop = useCallback(async () => {
    if (!draft || !draft.pending || !draft.assistantMessageId) return;
    try {
      await stopMessage(draft.assistantMessageId);
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
  const showEmpty =
    !conversationId || (messages.length === 0 && !draft);

  return (
    <div className="v4-chat-pane" aria-label={`Chat with ${bot.name}`}>
      <header className="v4-chat-pane-header">
        <div className="v4-chat-pane-bot">
          <span className="v4-chat-pane-name">{bot.name}</span>
          {conversationId && (
            <span className="v4-chat-pane-conv-id" title={conversationId}>
              {conversationId.slice(0, 8)}
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
        {showEmpty && (
          <div className="v4-chat-pane-empty">
            {!conversationId
              ? "Pick a thread or start a new chat."
              : "Empty conversation. Send a message to start."}
          </div>
        )}
        {conversationId &&
          messages.map((m) => (
            <div
              key={m.id}
              className={`v4-chat-pane-msg v4-chat-pane-msg--${m.role}`}
            >
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
          placeholder={
            conversationId
              ? `Message ${bot.name}… (Enter to send, Shift+Enter for newline)`
              : "Pick a thread or start a new chat."
          }
          disabled={busy || !conversationId}
          rows={3}
        />
        <button
          type="button"
          className="v4-chat-pane-send primary"
          onClick={handleSend}
          disabled={busy || composer.trim().length === 0 || !conversationId}
        >
          {busy ? "Sending…" : "Send"}
        </button>
      </footer>
    </div>
  );
}
