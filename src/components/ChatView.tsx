import { useEffect, useRef } from "react";
import type { Message } from "../lib/api";
import { MessageBubble } from "./MessageBubble";

interface ChatViewProps {
  messages: Message[];
  streamingId: string | null;
}

export function ChatView({ messages, streamingId }: ChatViewProps) {
  const scrollRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to the bottom on new content. Cheap and effective for the
  // current "follow the latest token" UX.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [messages]);

  return (
    <div className="messages" ref={scrollRef}>
      {messages.map((m) => (
        <MessageBubble
          key={m.id}
          message={m}
          streaming={streamingId === m.id}
        />
      ))}
    </div>
  );
}
