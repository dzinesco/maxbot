import { useEffect, useRef } from "react";
import type { Message } from "../lib/api";
import { MessageBubble } from "./MessageBubble";

interface ChatViewProps {
  messages: Message[];
  streamingId: string | null;
  onRegenerate?: () => void;
}

export function ChatView({
  messages,
  streamingId,
  onRegenerate,
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

  return (
    <div className="messages" ref={scrollRef}>
      {messages.map((m) => (
        <MessageBubble
          key={m.id}
          message={m}
          streaming={streamingId === m.id}
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
  );
}
