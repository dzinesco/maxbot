import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { Composer } from "./components/Composer";
import { Settings } from "./components/Settings";
import {
  createConversation,
  deleteConversation,
  getMessages,
  getSettings,
  listConversations,
  onChunk,
  onDone,
  onError,
  renameConversation,
  saveSettings,
  sendMessage,
  stopMessage,
} from "./lib/tauri";
import type {
  ChunkEvent,
  Conversation,
  DoneEvent,
  ErrorEvent,
  Message,
  Settings as SettingsT,
} from "./lib/api";
import { DEFAULT_SETTINGS } from "./lib/api";

interface PendingTurn {
  /** id of the assistant message we're streaming into */
  assistantId: string;
  /** the text we've accumulated (mirror of DB content) */
  text: string;
  /** tool calls accumulated from tool_call_delta chunks */
  toolCalls: Map<string, { id: string; name: string; arguments: string }>;
}

export default function App() {
  const [conversations, setConversations] = useState<Conversation[]>([]);
  const [activeId, setActiveId] = useState<string | null>(null);
  const [messages, setMessages] = useState<Message[]>([]);
  const [settings, setSettings] = useState<SettingsT>(DEFAULT_SETTINGS);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [streamingId, setStreamingId] = useState<string | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  /** Map of assistantId -> accumulated streaming state. Cleared on done. */
  const pendingRef = useRef<PendingTurn | null>(null);
  const requestSeq = useRef(0);

  // --- bootstrap ---
  useEffect(() => {
    (async () => {
      try {
        const [s, c] = await Promise.all([getSettings(), listConversations()]);
        setSettings(s);
        setConversations(c);
        if (c.length > 0) {
          setActiveId(c[0].id);
        } else {
          // No conversations yet — create a fresh "New chat" so the user
          // can start typing immediately.
          const created = await createConversation(undefined);
          setConversations([created]);
          setActiveId(created.id);
        }
      } catch (e) {
        setBootError(String(e));
      }
    })();
  }, []);

  // --- event subscriptions (live for the whole app) ---
  useEffect(() => {
    const unlistens: Array<() => void> = [];
    let cancelled = false;
    (async () => {
      const u1 = await onChunk((event: ChunkEvent) => {
        const pending = pendingRef.current;
        if (!pending || pending.assistantId !== event.assistant_message_id) {
          return;
        }
        if (event.chunk.kind === "text") {
          pending.text += event.chunk.delta;
          // Update the local message state so the UI re-renders with the
          // new text. Cheap because we set the whole message in place.
          setMessages((prev) =>
            prev.map((m) =>
              m.id === pending.assistantId
                ? { ...m, content: pending.text }
                : m,
            ),
          );
        } else if (event.chunk.kind === "tool_call_delta") {
          const tc = event.chunk;
          const existing = pending.toolCalls.get(tc.id) ?? {
            id: tc.id,
            name: "",
            arguments: "",
          };
          if (tc.name) existing.name = tc.name;
          if (tc.arguments_delta) existing.arguments += tc.arguments_delta;
          pending.toolCalls.set(tc.id, existing);
        }
      });
      const u2 = await onDone((_event: DoneEvent) => {
        const pending = pendingRef.current;
        if (pending) {
          setMessages((prev) =>
            prev.map((m) =>
              m.id === pending.assistantId
                ? {
                    ...m,
                    tool_calls: Array.from(pending.toolCalls.values()),
                  }
                : m,
            ),
          );
        }
        pendingRef.current = null;
        setStreamingId(null);
      });
      const u3 = await onError((event: ErrorEvent) => {
        const pending = pendingRef.current;
        const message = event.message;
        if (pending) {
          setMessages((prev) =>
            prev.map((m) =>
              m.id === pending.assistantId
                ? { ...m, content: pending.text + `\n\n[error] ${message}` }
                : m,
            ),
          );
        }
        pendingRef.current = null;
        setStreamingId(null);
      });
      if (cancelled) {
        u1();
        u2();
        u3();
      } else {
        unlistens.push(u1, u2, u3);
      }
    })();
    return () => {
      cancelled = true;
      for (const u of unlistens) u();
    };
  }, []);

  // --- load messages when active conversation changes ---
  useEffect(() => {
    if (!activeId) {
      setMessages([]);
      return;
    }
    (async () => {
      try {
        const m = await getMessages(activeId);
        setMessages(m);
      } catch (e) {
        console.error("load messages failed:", e);
      }
    })();
  }, [activeId]);

  // --- handlers ---
  const refreshConversations = useCallback(async () => {
    try {
      const c = await listConversations();
      setConversations(c);
    } catch (e) {
      console.error("list conversations failed:", e);
    }
  }, []);

  const handleNewConversation = useCallback(async () => {
    const created = await createConversation(undefined);
    await refreshConversations();
    setActiveId(created.id);
  }, [refreshConversations]);

  const handleDeleteConversation = useCallback(
    async (id: string) => {
      await deleteConversation(id);
      const remaining = conversations.filter((c) => c.id !== id);
      setConversations(remaining);
      if (activeId === id) {
        if (remaining.length > 0) {
          setActiveId(remaining[0].id);
        } else {
          const created = await createConversation(undefined);
          setConversations([created]);
          setActiveId(created.id);
        }
      }
    },
    [activeId, conversations],
  );

  const handleRenameConversation = useCallback(
    async (id: string, title: string) => {
      await renameConversation(id, title);
      await refreshConversations();
    },
    [refreshConversations],
  );

  const handleSend = useCallback(
    async (content: string) => {
      if (!activeId) return;
      if (!settings.minimax_api_key) {
        setSettingsOpen(true);
        return;
      }
      const requestId = `req-${++requestSeq.current}`;
      try {
        const result = await sendMessage(activeId, content, requestId);
        // Optimistically add the user + placeholder assistant message so the
        // UI shows the turn before any chunks arrive. The Rust side has
        // already persisted them, so a follow-up reloadMessages would
        // re-fetch the same data.
        const now = new Date().toISOString();
        const userMsg: Message = {
          id: result.user_message_id,
          conversation_id: activeId,
          role: "user",
          content,
          tool_calls: [],
          created_at: now,
        };
        const assistantMsg: Message = {
          id: result.assistant_message_id,
          conversation_id: activeId,
          role: "assistant",
          content: "",
          tool_calls: [],
          created_at: now,
        };
        setMessages((prev) => [...prev, userMsg, assistantMsg]);
        pendingRef.current = {
          assistantId: result.assistant_message_id,
          text: "",
          toolCalls: new Map(),
        };
        setStreamingId(result.assistant_message_id);
        await refreshConversations();
      } catch (e) {
        setBootError(`send failed: ${String(e)}`);
      }
    },
    [activeId, settings.minimax_api_key, refreshConversations],
  );

  const handleStop = useCallback(async () => {
    if (!streamingId) return;
    try {
      await stopMessage(streamingId);
    } catch (e) {
      console.error("stop failed:", e);
    }
  }, [streamingId]);

  const handleSaveSettings = useCallback(async (next: SettingsT) => {
    await saveSettings(next);
    setSettings(next);
  }, []);

  const status = useMemo<"online" | "missing" | "checking">(() => {
    if (bootError) return "missing";
    if (!settings) return "checking";
    return settings.minimax_api_key ? "online" : "missing";
  }, [bootError, settings]);

  return (
    <div className="app">
      <Sidebar
        conversations={conversations}
        activeId={activeId}
        onSelect={setActiveId}
        onNew={handleNewConversation}
        onDelete={handleDeleteConversation}
        onRename={handleRenameConversation}
        onOpenSettings={() => setSettingsOpen(true)}
        status={status}
      />
      <main className="main">
        {messages.length === 0 ? (
          <div className="main-empty">
            <h1>MaxBot</h1>
            <p>
              A MiniMax-first chat client. Type a message below to start.
              {status === "missing"
                ? " Set your MiniMax API key in Settings to begin."
                : ""}
            </p>
            {bootError && (
              <p style={{ color: "var(--danger)" }}>{bootError}</p>
            )}
          </div>
        ) : (
          <ChatView messages={messages} streamingId={streamingId} />
        )}
        <Composer
          onSend={handleSend}
          onStop={handleStop}
          streaming={streamingId !== null}
        />
      </main>
      {settingsOpen && (
        <Settings
          initial={settings}
          onClose={() => setSettingsOpen(false)}
          onSave={handleSaveSettings}
        />
      )}
    </div>
  );
}
