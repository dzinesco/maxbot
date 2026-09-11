// v2.0 Slice E — per-Bot chat scoping.
//
// Before this slice, ChatView was a thin wrapper over
// `<MessageBubble>`s — the App.tsx `<main>` element owned
// the conversation header, the conversation list, and the
// per-conversation composer. After this slice, the
// conversation list moves into the main column (so the
// user can see which conversation is open and pick
// another without leaving the chat), and `ChatView`
// renders the per-Bot header + the message stream.
//
// The component is still the canonical "render an
// array of messages" surface — the App.tsx side
// still owns the data fetching and the message array
// (`messages: Message[]`), and the composer lives in
// the parent so the empty-state composer (`<Composer
// disabled>`) is reusable.

import { useEffect, useMemo, useRef } from "react";
import type { Bot, Computer, Conversation, Message } from "../lib/api";
import {
  MessageBubble,
  TTSToolbar,
  mostRecentAssistantMessage,
} from "./MessageBubble";
import { BotAvatar } from "./BotAvatar";

interface ChatViewProps {
  messages: Message[];
  streamingId: string | null;
  onRegenerate?: () => void;
  /**
   * v2.0 Slice E: per-Bot chat scoping. The currently
   * selected Bot is rendered in the header (avatar + name +
   * state). When `null` (no Bot selected), the header is
   * hidden and the empty state should be shown by the
   * parent instead.
   */
  activeBot?: Bot | null;
  /**
   * v2.0 Slice E: per-Bot chat scoping. The list of
   * conversations scoped to the active Bot, shown as a
   * small horizontal pill row above the messages. The
   * parent is the canonical owner of conversation
   * creation (so a "new conversation" button next to the
   * list can be wired in one place).
   */
  scopedConversations?: Conversation[];
  /** Currently-active conversation id (highlighted in the
   *  scoped list). */
  activeConversationId?: string | null;
  /** Callback when the user picks a different conversation
   *  from the scoped list. */
  onSelectConversation?: (conversationId: string) => void;
  /** Callback when the user clicks the "New conversation"
   *  button in the scoped list. */
  onNewConversation?: () => void;
  /** v3.7.13 — UX-7. The active Bot's Computer
   *  Use VM, if one exists. The chat header
   *  surfaces a "PC" pill when the VM is
   *  provisioning / stopped / errored, and the
   *  pill click target opens the Computer panel.
   *  `null` means "no VM yet" — no pill. */
  activeComputer?: Computer | null;
  /** v3.7.13 — UX-7. Fires when the user
   *  clicks the "PC" pill. The parent opens
   *  the Computer panel for `activeBot.id`. */
  onOpenComputer?: (botId: string) => void;
}

export function ChatView({
  messages,
  streamingId,
  onRegenerate,
  activeBot,
  scopedConversations,
  activeConversationId,
  onSelectConversation,
  onNewConversation,
  activeComputer,
  onOpenComputer,
}: ChatViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to the bottom on new content. Cheap and effective for the
  // current "follow the latest token" UX.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [messages]);

  // Find the last message: if it's an assistant message that's not
  // currently streaming, surface a "Regenerate" button below it.
  const last = messages[messages.length - 1];
  const showRegenerate =
    !!onRegenerate &&
    !!last &&
    last.role === "assistant" &&
    streamingId !== last.id &&
    streamingId === null;

  // Pre-compute the avatar's "what is it doing right now" string for
  // the header. Slice E wires this from the active run's status; the
  // header reads it via the standard `title` tooltip on the avatar.
  const lastRunForHeader = useMemo(
    () => undefined as { status: import("../lib/api").BotRunStatus } | undefined,
    [],
  );

  return (
    <div className="chat-view" data-testid="chat-view">
      {activeBot && (
        <div className="chat-view__header" data-testid="chat-view-header">
          <BotAvatar
            bot={activeBot}
            lastRun={lastRunForHeader}
            size={28}
          />
          <div className="chat-view__header-meta">
            <div className="chat-view__header-name">
              {activeBot.name}
            </div>
            <div
              className="chat-view__header-state"
              data-state={activeBot.state ?? "idle"}
            >
              {stateLabel(activeBot.state)}
            </div>
          </div>
          {/*
            v3.7.13 — UX-7. Computer pill. The pill
            is hidden when the VM is running (the
            user can already see the Bot is up —
            surfacing the chip would be visual
            noise). When the VM is provisioning /
            stopped / errored, the pill shows the
            state and a single click target that
            opens the Computer panel. The click
            target is the same one the sidebar
            uses; ChatView delegates to the
            parent's `onOpenComputer(botId)`.
          */}
          {activeComputer &&
            activeComputer.state !== "running" &&
            onOpenComputer && (
              <button
                type="button"
                className="chat-view__pc-pill"
                data-testid="chat-view-pc-pill"
                data-state={activeComputer.state}
                onClick={() => onOpenComputer(activeBot.id)}
                title={`Open the Computer panel (${activeComputer.state})`}
              >
                <span className="chat-view__pc-pill-dot" aria-hidden />
                <span>
                  PC · {stateLabelForComputer(activeComputer.state)}
                </span>
              </button>
            )}
          {scopedConversations && scopedConversations.length > 0 && (
            <ConversationPills
              conversations={scopedConversations}
              activeConversationId={activeConversationId ?? null}
              onSelect={onSelectConversation}
              onNew={onNewConversation}
            />
          )}
        </div>
      )}
      <div className="messages" ref={scrollRef}>
        <TTSToolbar
          target={mostRecentAssistantMessage(messages)}
        />
        {messages.map((m) => (
          <MessageBubble
            key={m.id}
            message={m}
            streaming={streamingId === m.id}
            onRetry={onRegenerate}
            botId={activeBot?.id ?? null}
          />
        ))}
        {showRegenerate && onRegenerate && (
          <div className="regenerate-bar">
            <button
              className="ghost small"
              onClick={onRegenerate}
              title="Re-run the last user message and replace this response"
            >
              ↻ Regenerate
            </button>
          </div>
        )}
      </div>
    </div>
  );
}

// ---- Conversation pill row ----

/**
 * A horizontal row of pills, one per conversation in the
 * scoped list. The active conversation is highlighted; a
 * trailing `+` button calls `onNew`. The row scrolls
 * horizontally on overflow so a Bot with many
 * conversations doesn't push the chat off-screen.
 */
function ConversationPills({
  conversations,
  activeConversationId,
  onSelect,
  onNew,
}: {
  conversations: Conversation[];
  activeConversationId: string | null;
  onSelect?: (id: string) => void;
  onNew?: () => void;
}) {
  return (
    <div
      className="chat-view__pills"
      role="tablist"
      aria-label="Conversations"
    >
      {conversations.map((c) => {
        const isActive = c.id === activeConversationId;
        return (
          <button
            key={c.id}
            type="button"
            role="tab"
            aria-selected={isActive}
            className={[
              "chat-view__pill",
              isActive ? "chat-view__pill--active" : "",
            ]
              .filter(Boolean)
              .join(" ")}
            onClick={() => onSelect?.(c.id)}
            title={c.title || "Untitled conversation"}
          >
            {c.title || "Untitled"}
          </button>
        );
      })}
      {onNew && (
        <button
          type="button"
          className="chat-view__pill chat-view__pill--new"
          onClick={onNew}
          title="Start a new conversation with this Bot"
          aria-label="New conversation"
        >
          +
        </button>
      )}
    </div>
  );
}

// ---- Helpers ----

function stateLabel(state: Bot["state"]): string {
  switch (state) {
    case "thinking":
      return "Thinking…";
    case "working":
      return "Working…";
    case "waiting":
      return "Waiting for you";
    case "blocked":
      return "Blocked — last run failed";
    case "done":
      return "Done";
    case "idle":
    default:
      return "Idle";
  }
}

// v3.7.13 — UX-7. Computer-state verb for the
// chat-view PC pill. The pill surfaces
// non-running states (provisioning / stopped /
// error); the "running" case is hidden entirely
// so the pill doesn't add visual noise when
// everything is fine.
function stateLabelForComputer(state: string): string {
  switch (state) {
    case "provisioning":
      return "provisioning";
    case "stopped":
      return "stopped";
    case "error":
      return "errored";
    case "running":
      return "running";
    default:
      return state;
  }
}
