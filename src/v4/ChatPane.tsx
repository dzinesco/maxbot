/*
 * v4 — ChatPane (S2.5 — Grok Bot look)
 *
 * S2.5 changes from S3:
 *   - Header: bot name + one status word + Stop. Drop the
 *     8-char conv-id hex (Grok doesn't show it).
 *   - Per-message meta (role + timestamp uppercase) DROPPED.
 *     No "ASSISTANT 12:34:56 PM" rows. Just the body.
 *   - User messages: right-aligned, muted (text-2).
 *   - Assistant messages: left-aligned, full width, 16px,
 *     1.5 line-height, near-white.
 *   - No bubbles. Plain text rows, role-coded only by
 *     alignment + color.
 *   - Composer: 44px pill, 12px radius, border #2a2a2c, sits
 *     16px off the bottom.
 *   - <think>…</think> blocks hidden in render (defensive
 *     even though .think has display:none in CSS).
 *
 * S3 tool-call behavior preserved: name + one-line result
 * under the owning assistant message, no JSON in state.
 *
 * Hard rules (v4):
 *   - Mount only when botId is provided.
 *   - Listeners paired with unlistens in the same useEffect.
 *   - No setInterval — all timers are recursive setTimeout.
 *   - No history polling while a stream is active.
 *   - No screenshot bytes / tool arguments JSON / journal text in
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
  BotState,
  ChunkEvent,
  DoneEvent,
  ErrorEvent,
  Message,
  PersistedToolCall,
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
  /**
   * Called when the FIRST user message is sent into a conversation
   * whose title is still the Rust default ("New chat") or empty.
   * App.tsx uses this to derive the thread title from the first
   * message + persist it via `renameConversation`. Fires only
   * once per conversation (subsequent sends keep the existing
   * title). Optional — when omitted, no title auto-update happens.
   */
  onFirstUserMessage?: (conversationId: string, content: string) => void;
}

interface AssistantDraft {
  id: string;
  requestId: string;
  assistantMessageId: string;
  text: string;
  pending: boolean;
}

/** A compact view of a tool call for rendering — name + one-line
 *  result, nothing else. Built from history (post-Done) and
 *  augmented by `liveToolCalls` during streaming. */
interface ToolCallRow {
  /** Persisted tool_call id from the wire. */
  id: string;
  /** Tool name. */
  name: string;
  /** One-line result summary. `null` while the call is still in
   *  flight (no result message has arrived yet). */
  result: string | null;
}

/**
 * Walk `messages` and build a positional map of every tool call
 * and its one-line result. The pairing rule: for each assistant
 * message at index `i` with `tool_calls` of length `n`, the next
 * `n` consecutive `role: "tool"` messages are its results, paired
 * positionally. The map key is the `PersistedToolCall.id`.
 *
 * Pure function. No IPC. Runs in render and after history loads.
 */
function summarizeToolCalls(messages: Message[]): Map<string, ToolCallRow> {
  const out = new Map<string, ToolCallRow>();
  for (let i = 0; i < messages.length; i++) {
    const m = messages[i];
    if (m.role !== "assistant" || m.tool_calls.length === 0) continue;
    const calls = m.tool_calls;
    let toolCursor = i + 1;
    for (let k = 0; k < calls.length; k++) {
      const call = calls[k];
      let resultText: string | null = null;
      // Walk forward to find the k-th role=tool message after
      // this assistant message.
      let found = 0;
      for (let j = toolCursor; j < messages.length; j++) {
        if (messages[j].role === "tool") {
          if (found === k) {
            resultText = messages[j].content;
            toolCursor = j + 1;
            break;
          }
          found++;
        }
      }
      // v4 S4 — Drop rows where the result is a
      // flailing-error from the pre-S3a model: an unknown
      // tool call or a schema-invalid call (missing field).
      // These were persisted as `[error] invalid arguments:
      // missing field: command` and `[error] unknown tool: X`
      // rows from before the bug fix. They were never useful
      // to the user — they only existed because the LLM was
      // looping on a bad call. The post-S3a model never emits
      // them; we don't render the historical ones either so
      // old threads don't look noisy.
      if (
        resultText !== null &&
        (resultText.startsWith("[error] invalid arguments") ||
          resultText.startsWith("[error] unknown tool"))
      ) {
        continue;
      }
      out.set(call.id, {
        id: call.id,
        name: call.name,
        result: resultText,
      });
    }
  }
  return out;
}

/** Truncate a one-line result summary for the chat row. Aggressive
 *  cap so a runaway JSON payload can't blow up render cost. */
function truncateResult(text: string | null, max = 80): string {
  if (text === null) return "running…";
  // Collapse newlines so the row stays single-line.
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  return flat.slice(0, max) + "…";
}

/**
 * Strip <think>…</think> reasoning blocks from assistant content.
 * S2.5 hides them in CSS (`.think { display: none }`), but we also
 * drop them in render so the DOM stays clean and the truncated
 * preview shows what the user actually said. Strips:
 *   - Complete blocks `<think>...</think>`
 *   - Orphan opening `<think>` (some providers forget the close)
 *   - Orphan closing `</think>` (some providers forget the open)
 * Whitespace around stripped regions is normalized so a dangling
 * `</think>` doesn't leave a leading newline in the rendered body.
 */
function stripThinkBlocks(text: string): string {
  if (!text) return text;
  // Drop complete blocks first (greedy across newlines).
  let out = text.replace(/<think>[\s\S]*?<\/think>/gi, "");
  // Drop orphan opening tags.
  out = out.replace(/<think>/gi, "");
  // Drop orphan closing tags.
  out = out.replace(/<\/think>/gi, "");
  // Collapse runs of blank lines left behind.
  out = out.replace(/\n{3,}/g, "\n\n");
  return out.trim();
}

/** Single status word for the chat header. */
function botStatusWord(
  state: BotState | undefined,
  isStreaming: boolean,
): string {
  if (isStreaming) return "Writing…";
  switch (state) {
    case "thinking":
    case "working":
      return "Working";
    case "blocked":
    case "waiting":
      return "Blocked";
    case "done":
      return "Done";
    default:
      return "Online";
  }
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

export function ChatPane({ bot, conversationId, onFirstUserMessage }: ChatPaneProps) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [composer, setComposer] = useState("");
  const [draft, setDraft] = useState<AssistantDraft | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // S3 — live tool-call tracking during streaming. Name only.
  // Cleared on conversation switch + on every new send.
  const [liveToolCalls, setLiveToolCalls] = useState<
    Map<string, { name: string }>
  >(new Map());
  // Track the current conversation id in a ref so listeners
  // and the history-refresh tick ignore stale state when the
  // conversation changes mid-stream.
  const convRef = useRef<string | null>(null);
  convRef.current = conversationId;
  // Track active stream so the history tick knows to back off.
  const pendingRef = useRef<boolean>(false);
  pendingRef.current = draft?.pending ?? false;
  // v4 S5 — Synchronous in-flight flag for handleSend.
  // React `busy` state is async (a second invocation in the
  // same tick can both read busy=false and both fire). A ref
  // updates synchronously, so back-to-back Enter presses or
  // Enter+Send clicks can't double-send.
  const sendingRef = useRef<boolean>(false);

  // History load — keyed on conversationId. When the user
  // switches threads via App.tsx, this fires and reloads.
  useEffect(() => {
    // Conversation switch — wipe the live tool map; it belongs to
    // the previous thread's stream.
    setLiveToolCalls(new Map());
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
        // Narrow on the destructured chunk so TypeScript keeps
        // the narrowed type through the setDraft / setLiveToolCalls
        // closures below. (Property-narrowing across a closure is
        // not always preserved; destructuring is.)
        const { chunk, request_id } = ev;
        if (chunk.kind === "text") {
          const delta = chunk.delta;
          setDraft((prev) => {
            if (!prev || prev.requestId !== request_id) return prev;
            return { ...prev, text: prev.text + delta };
          });
          return;
        }
        if (chunk.kind === "tool_call_delta") {
          // S3 — record the name only. We don't accumulate the
          // arguments_delta; the renderer never holds the tool's
          // argument JSON.
          const id = chunk.id;
          const incomingName = chunk.name ?? null;
          if (!id) return;
          setLiveToolCalls((prev) => {
            if (prev.has(id)) {
              // Already have this id — the name may have arrived
              // in a later chunk (some providers send the name
              // after the first arguments_delta). Merge but don't
              // overwrite a non-empty name with an empty one.
              if (!incomingName) return prev;
              const existing = prev.get(id);
              if (existing && existing.name) return prev;
              const next = new Map(prev);
              next.set(id, { name: incomingName });
              return next;
            }
            if (!incomingName) {
              // No name yet — track the id with a placeholder so
              // a later chunk can fill in the name.
              const next = new Map(prev);
              next.set(id, { name: "" });
              return next;
            }
            const next = new Map(prev);
            next.set(id, { name: incomingName });
            return next;
          });
          return;
        }
        // StreamChunk kind "done" — ignored on the chunk channel;
        // the dedicated DoneEvent handler is the source of truth.
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
    if (!text || !conversationId) return;
    // v4 S5 — Synchronous double-send guard. The ref flips
    // immediately so a rapid Enter+Enter or Enter+click lands
    // only one send_message IPC. The React `busy` state lags
    // by one render; this ref doesn't.
    if (sendingRef.current) return;
    sendingRef.current = true;
    const requestId = generateRequestId();
    setBusy(true);
    setComposer("");
    setError(null);
    // S3 — clear the live tool map for the new turn. The previous
    // turn's tools either landed in history (and render from
    // there) or didn't happen.
    setLiveToolCalls(new Map());
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
      // S2.6 — first user message in a thread drives the title.
      // Fires unconditionally on send; App.tsx decides whether
      // to actually rename (no-op when the title is already set).
      if (onFirstUserMessage) {
        onFirstUserMessage(conversationId, text);
      }
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
      sendingRef.current = false;
      setBusy(false);
    }
  }, [composer, conversationId, onFirstUserMessage]);

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
  const headerStatus = botStatusWord(bot.state, !!showStop);

  // S3 — tool row rendering. Build the persisted map once per
  // render; live entries that aren't in the map show as
  // `running…`. Empty placeholder names are dropped so the row
  // doesn't say ` · running…` before the name chunk arrives.
  const persistedTools = summarizeToolCalls(messages);
  const liveToolRows: ToolCallRow[] = [];
  for (const [id, entry] of liveToolCalls.entries()) {
    if (!entry.name) continue;
    const persisted = persistedTools.get(id);
    liveToolRows.push({
      id,
      name: entry.name,
      result: persisted?.result ?? null,
    });
  }

  return (
    <div className="v4-chat-pane" aria-label={`Chat with ${bot.name}`}>
      <header className="v4-chat-pane-header">
        <div className="v4-chat-pane-bot">
          <span className="v4-chat-pane-name">{bot.name}</span>
          <span className="v4-chat-pane-status" aria-live="polite">
            {headerStatus}
          </span>
        </div>
        {showStop && (
          <button
            type="button"
            className="v4-chat-pane-stop small"
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
              {/* S2.5 — strip <think> blocks in render. CSS also
                  hides .think as a defensive fallback. */}
              <div className="v4-chat-pane-msg-body">
                {m.role === "assistant"
                  ? stripThinkBlocks(m.content)
                  : m.content}
              </div>
              {m.role === "assistant" && m.tool_calls.length > 0 && (
                <ToolRows
                  calls={m.tool_calls}
                  persisted={persistedTools}
                />
              )}
            </div>
          ))}
        {draft && (
          <div className="v4-chat-pane-msg v4-chat-pane-msg--assistant v4-chat-pane-msg--draft">
            <div className="v4-chat-pane-msg-body">
              {/* S2.5 — strip <think> blocks from the live draft
                  text as well, so reasoning chunks the LLM emits
                  mid-stream never surface to the user. */}
              {stripThinkBlocks(draft.text)}
            </div>
            {liveToolRows.length > 0 && <ToolRows rows={liveToolRows} />}
          </div>
        )}
      </div>

      <footer className="v4-chat-pane-composer">
        {/* S2.6 — composer is one rounded field. The send button
            sits INSIDE the pill on the right edge; the input fills
            the rest. Enabled whenever a bot + thread are selected
            (the empty state on bot select is handled in App.tsx:
            when the bot has no threads, one is created). */}
        <div
          className={`v4-chat-pane-composer-field ${
            composer.trim().length === 0 ? "is-empty" : "has-text"
          }`}
        >
          <textarea
            className="v4-chat-pane-input"
            value={composer}
            onChange={(e) => setComposer(e.target.value)}
            onKeyDown={handleComposerKey}
            placeholder={
              conversationId
                ? `Message ${bot.name}…`
                : "Pick a thread or start a new chat."
            }
            disabled={!conversationId}
            rows={1}
          />
          <button
            type="button"
            className="v4-chat-pane-send"
            onClick={handleSend}
            disabled={
              busy || composer.trim().length === 0 || !conversationId
            }
            title="Send message"
            aria-label="Send message"
          >
            {/* Up-arrow icon — single glyph, matches the Grok
                composer convention. No text label. */}
            <svg
              width="16"
              height="16"
              viewBox="0 0 16 16"
              aria-hidden
              focusable="false"
            >
              <path
                d="M8 2 L8 12 M3 7 L8 2 L13 7"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                fill="none"
              />
            </svg>
          </button>
        </div>
      </footer>
    </div>
  );
}

/** Renders the compact tool-call rows under an assistant message.
 *  Two call shapes:
 *    - `<ToolRows calls={...} persisted={...} />` for messages
 *      loaded from history: walks the `PersistedToolCall[]`,
 *      looks up each id in `persisted` for the result.
 *    - `<ToolRows rows={...} />` for the in-flight draft: rows
 *      are already ToolCallRow-shaped (built in render). */
function ToolRows({
  calls,
  persisted,
  rows,
}: {
  calls?: PersistedToolCall[];
  persisted?: Map<string, ToolCallRow>;
  rows?: ToolCallRow[];
}) {
  const items: ToolCallRow[] = rows
    ? rows
    : (calls ?? []).map((c) => {
        const hit = persisted?.get(c.id);
        return {
          id: c.id,
          name: c.name,
          result: hit?.result ?? null,
        };
      });
  if (items.length === 0) return null;
  return (
    <ul className="v4-chat-pane-tools" aria-label="Tool calls">
      {items.map((it) => (
        <li key={it.id} className="v4-chat-pane-tool-row">
          <span className="v4-chat-pane-tool-name">{it.name}</span>
          <span className="v4-chat-pane-tool-sep">·</span>
          <span className="v4-chat-pane-tool-result">
            {truncateResult(it.result)}
          </span>
        </li>
      ))}
    </ul>
  );
}
