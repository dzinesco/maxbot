import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { Composer } from "./components/Composer";
import { Settings } from "./components/Settings";
import { BotsPanel } from "./components/BotsPanel";
import { BotEditor } from "./components/BotEditor";
import { BotInbox } from "./components/BotInbox";
import { SendToBotModal } from "./components/SendToBotModal";
import {
  createConversation,
  deleteBot,
  deleteConversation,
  getBot,
  getBotSchedule,
  getMessages,
  searchMessages,
  getSettings,
  listAllSchedules,
  listAvailableTools,
  listBots,
  listBotRuns,
  listActiveBotRuns,
  listConversations,
  listInbox,
  markInboxRead,
  onBotChunk,
  onBotDone,
  onBotError,
  onChunk,
  onDone,
  onError,
  renameConversation,
  regenerateLast,
  runBotNow,
  saveSettings,
  sendMessage,
  sendToBot,
  stopBotRun,
  stopMessage,
  upsertBot,
  upsertBotSchedule,
} from "./lib/tauri";
import {
  blankBot,
  DEFAULT_SETTINGS,
  type Bot,
  type BotChunkEvent,
  type BotDoneEvent,
  type BotErrorEvent,
  type BotMessage,
  type BotRun,
  type BotSchedule,
  type ChunkEvent,
  type Conversation,
  type DoneEvent,
  type ErrorEvent,
  type Message,
  type Settings as SettingsT,
  type ToolSummary,
} from "./lib/api";

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
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<Message[]>([]);
  const [settings, setSettings] = useState<SettingsT>(DEFAULT_SETTINGS);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [streamingId, setStreamingId] = useState<string | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [bots, setBots] = useState<Bot[]>([]);
  const [botSchedules, setBotSchedules] = useState<Record<string, BotSchedule>>(
    {},
  );
  const [botLastRuns, setBotLastRuns] = useState<Record<string, BotRun>>({});
  const [unreadCounts, setUnreadCounts] = useState<Record<string, number>>({});
  const [availableTools, setAvailableTools] = useState<ToolSummary[]>([]);
  const [editorState, setEditorState] = useState<
    | { mode: "closed" }
    | { mode: "new" }
    | { mode: "edit"; botId: string }
    | { mode: "inbox"; botId: string }
  >({ mode: "closed" });
  const [editorDraft, setEditorDraft] = useState<{
    bot: Bot;
    schedule: BotSchedule | null;
  } | null>(null);
  const [editorAvailableTools, setEditorAvailableTools] = useState<
    ToolSummary[]
  >([]);
  const [botsCollapsed, setBotsCollapsed] = useState(false);
  const [runningBotId, setRunningBotId] = useState<string | null>(null);
  /** Map of active bot run id → bot id. Populated by runBotNow /
   * sendToBot (which return the run id) and pruned by the bot-done
   * event listener. Polled from the DB every 2s as a fallback in
   * case an event is missed (e.g. the app was relaunched mid-run). */
  const [activeRunByBot, setActiveRunByBot] = useState<Record<string, string>>(
    {},
  );
  /** Map of assistantId -> accumulated streaming state. Cleared on done. */
  const pendingRef = useRef<PendingTurn | null>(null);
  /** Same as `pendingRef` but for bot runs (assistantMessageId keyed). */
  const botPendingRef = useRef<PendingTurn | null>(null);
  /** Last finished bot run per bot — surfaces in the chat view as a banner. */
  const [lastBotFinish, setLastBotFinish] = useState<{
    botId: string;
    runId: string;
    conversationId: string;
    summary: string;
  } | null>(null);
  const requestSeq = useRef(0);

  // --- search (debounced) ---
  // When searchQuery is non-empty, kick off a `search_messages` call
  // 250ms after the last keystroke. We store the raw matching
  // messages and let the Sidebar render a snippet per matching
  // conversation. Empty query clears the result set.
  useEffect(() => {
    const q = searchQuery.trim();
    if (!q) {
      setSearchResults([]);
      return;
    }
    const handle = setTimeout(async () => {
      try {
        const results = await searchMessages(q, 100);
        setSearchResults(results);
      } catch (e) {
        console.error("search failed:", e);
        setSearchResults([]);
      }
    }, 250);
    return () => clearTimeout(handle);
  }, [searchQuery]);

  // --- bootstrap ---
  useEffect(() => {
    (async () => {
      try {
        const [s, c, b, sched, tools] = await Promise.all([
          getSettings(),
          listConversations(),
          listBots(),
          listAllSchedules(),
          listAvailableTools(),
        ]);
        setSettings(s);
        setConversations(c);
        setBots(b);
        const schedMap: Record<string, BotSchedule> = {};
        for (const sc of sched) schedMap[sc.bot_id] = sc;
        setBotSchedules(schedMap);
        setAvailableTools(tools);
        if (c.length > 0) {
          setActiveId(c[0].id);
        } else {
          // No conversations yet — create a fresh "New chat" so the user
          // can start typing immediately.
          const created = await createConversation(undefined, undefined);
          setConversations([created]);
          setActiveId(created.id);
        }
        // Last-run snapshot per bot, plus unread counts.
        const runsEntries = await Promise.all(
          b.map(async (bot) => {
            const runs = await listBotRuns(bot.id, 1);
            return [bot.id, runs[0]] as const;
          }),
        );
        const runMap: Record<string, BotRun> = {};
        for (const [id, r] of runsEntries) if (r) runMap[id] = r;
        setBotLastRuns(runMap);
        const inboxEntries = await Promise.all(
          b.map(async (bot) => {
            const msgs = await listInbox(bot.id, false);
            return [bot.id, msgs.length] as const;
          }),
        );
        const unread: Record<string, number> = {};
        for (const [id, n] of inboxEntries) if (n > 0) unread[id] = n;
        setUnreadCounts(unread);
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
      // Bot events: route into the active conversation's messages so the
      // user sees the bot streaming into its own thread in the chat view.
      const u4 = await onBotChunk((event: BotChunkEvent) => {
        // Only render the bot's stream if the user is currently looking
        // at the bot's conversation; otherwise we'd be writing into the
        // wrong thread.
        if (event.conversation_id !== activeIdRef.current) return;
        let p = botPendingRef.current;
        if (!p || p.assistantId !== "bot-active") {
          p = {
            assistantId: "bot-active",
            text: "",
            toolCalls: new Map(),
          };
          botPendingRef.current = p;
        }
        if (event.chunk.kind === "text") {
          p.text += event.chunk.delta;
          setMessages((prev) =>
            upsertBotStreamMessage(prev, p.text, p.toolCalls),
          );
        } else if (event.chunk.kind === "tool_call_delta") {
          const tc = event.chunk;
          const existing = p.toolCalls.get(tc.id) ?? {
            id: tc.id,
            name: "",
            arguments: "",
          };
          if (tc.name) existing.name = tc.name;
          if (tc.arguments_delta) existing.arguments += tc.arguments_delta;
          p.toolCalls.set(tc.id, existing);
        }
      });
      const u5 = await onBotDone((event: BotDoneEvent) => {
        const pending = botPendingRef.current;
        if (pending) {
          setMessages((prev) =>
            upsertBotStreamMessage(
              prev,
              pending.text,
              pending.toolCalls,
              true,
            ),
          );
        }
        botPendingRef.current = null;
        setRunningBotId(null);
        setActiveRunByBot((prev) => {
          if (!(event.bot_id in prev)) return prev;
          const next = { ...prev };
          delete next[event.bot_id];
          return next;
        });
        setLastBotFinish({
          botId: event.bot_id,
          runId: event.bot_run_id,
          conversationId: event.conversation_id,
          summary: event.result_summary,
        });
        // Refresh last-run map for the affected bot.
        listBotRuns(event.bot_id, 1)
          .then((runs) => {
            const r = runs[0];
            if (r) setBotLastRuns((prev) => ({ ...prev, [event.bot_id]: r }));
          })
          .catch(() => {});
        // Refresh conversations in case a new bot-conversation was created.
        listConversations()
          .then(setConversations)
          .catch(() => {});
      });
      const u6 = await onBotError((event: BotErrorEvent) => {
        setLastBotFinish({
          botId: event.bot_id,
          runId: event.bot_run_id,
          conversationId: event.conversation_id,
          summary: `[error] ${event.message}`,
        });
        botPendingRef.current = null;
        setRunningBotId(null);
        setActiveRunByBot((prev) => {
          if (!(event.bot_id in prev)) return prev;
          const next = { ...prev };
          delete next[event.bot_id];
          return next;
        });
      });
      if (cancelled) {
        u1();
        u2();
        u3();
        u4();
        u5();
        u6();
      } else {
        unlistens.push(u1, u2, u3, u4, u5, u6);
      }
    })();
    return () => {
      cancelled = true;
      for (const u of unlistens) u();
    };
  }, []);

  // Keep a ref to the active conversation id so the bot chunk handler
  // (registered once) can see the latest value without re-subscribing.
  const activeIdRef = useRef<string | null>(null);
  useEffect(() => {
    activeIdRef.current = activeId;
  }, [activeId]);

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
        // Clear any stale "bot streaming" state when switching threads.
        botPendingRef.current = null;
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
    const created = await createConversation(undefined, undefined);
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
          const created = await createConversation(undefined, undefined);
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

  const handleRegenerate = useCallback(async () => {
    if (!activeId || streamingId) return;
    try {
      const out = await regenerateLast(activeId);
      // Same plumbing as handleSend: optimistic-insert the new
      // assistant placeholder, then let the existing chunk/done/
      // error event handlers update it in place.
      setMessages((prev) => {
        // Drop any leftover messages (the backend deleted the
        // previous response) and append the fresh placeholder.
        const filtered = prev.filter(
          (m) => !m.tool_calls || true, // keep tool messages, drop later assistant
        );
        // Simpler: just keep up through the last user message,
        // then add the new assistant placeholder.
        let lastUserIdx = -1;
        for (let i = filtered.length - 1; i >= 0; i--) {
          if (filtered[i].role === "user") {
            lastUserIdx = i;
            break;
          }
        }
        const kept =
          lastUserIdx >= 0 ? filtered.slice(0, lastUserIdx + 1) : filtered;
        return [
          ...kept,
          {
            id: out.assistant_message_id,
            conversation_id: activeId,
            role: "assistant",
            content: "",
            tool_calls: [],
            created_at: new Date().toISOString(),
          },
        ];
      });
      pendingRef.current = {
        assistantId: out.assistant_message_id,
        text: "",
        toolCalls: new Map(),
      };
      setStreamingId(out.assistant_message_id);
    } catch (e) {
      console.error("regenerate failed:", e);
      setBootError(`regenerate failed: ${String(e)}`);
    }
  }, [activeId, streamingId]);

  const handleSaveSettings = useCallback(async (next: SettingsT) => {
    await saveSettings(next);
    setSettings(next);
  }, []);

  // --- bot handlers ---

  const refreshBots = useCallback(async () => {
    const [b, sched] = await Promise.all([listBots(), listAllSchedules()]);
    setBots(b);
    const schedMap: Record<string, BotSchedule> = {};
    for (const sc of sched) schedMap[sc.bot_id] = sc;
    setBotSchedules(schedMap);
    const runsEntries = await Promise.all(
      b.map(async (bot) => {
        const runs = await listBotRuns(bot.id, 1);
        return [bot.id, runs[0]] as const;
      }),
    );
    const runMap: Record<string, BotRun> = {};
    for (const [id, r] of runsEntries) if (r) runMap[id] = r;
    setBotLastRuns(runMap);
    const inboxEntries = await Promise.all(
      b.map(async (bot) => {
        const msgs = await listInbox(bot.id, false);
        return [bot.id, msgs.length] as const;
      }),
    );
    const unread: Record<string, number> = {};
    for (const [id, n] of inboxEntries) if (n > 0) unread[id] = n;
    setUnreadCounts(unread);
  }, []);

  const handleNewBot = useCallback(() => {
    setEditorDraft({ bot: blankBot(), schedule: null });
    setEditorAvailableTools(availableTools);
    setEditorState({ mode: "new" });
  }, [availableTools]);

  const handleEditBot = useCallback(
    async (id: string) => {
      const [bot, schedule] = await Promise.all([
        getBot(id),
        getBotSchedule(id),
      ]);
      if (!bot) {
        alert("Bot no longer exists.");
        return;
      }
      setEditorDraft({ bot, schedule });
      setEditorAvailableTools(availableTools);
      setEditorState({ mode: "edit", botId: id });
    },
    [availableTools],
  );

  const handleDeleteBot = useCallback(
    async (id: string) => {
      await deleteBot(id);
      await refreshBots();
    },
    [refreshBots],
  );

  const handleRunBot = useCallback(
    async (id: string) => {
      setRunningBotId(id);
      setLastBotFinish(null);
      try {
        const out = await runBotNow(id);
        // Track the active run so the BotsPanel can show a Stop
        // button. The run is removed from this map when the bot-done
        // or bot-error event fires.
        setActiveRunByBot((prev) => ({ ...prev, [id]: out.run_id }));
        // Switch the active view to the bot's conversation so the user
        // sees the streaming output. Refresh conversations first so
        // any newly-created bot thread is in the sidebar.
        await refreshConversations();
        if (out.conversation_id) {
          setActiveId(out.conversation_id);
        }
      } catch (e) {
        alert(`Run failed: ${String(e)}`);
        setRunningBotId(null);
      }
    },
    [refreshConversations],
  );

  const handleStopBot = useCallback(async (botId: string) => {
    const runId = activeRunByBot[botId];
    if (!runId) return;
    try {
      await stopBotRun(runId);
      // The bot-done or bot-error event will fire and clear the
      // activeRunByBot entry. As a UX nicety, optimistically clear
      // the "running" state immediately so the button hides right
      // away.
      setRunningBotId((cur) => (cur === botId ? null : cur));
    } catch (e) {
      console.error("stop_bot_run failed:", e);
    }
  }, [activeRunByBot]);

  const handleOpenInbox = useCallback(
    async (id: string) => {
      // Mark the bot's inbox as read so the badge clears, then open
      // the inbox modal. We could open the modal first and then mark
      // read, but doing it up front means the user sees the right
      // count immediately on next reload.
      await markInboxRead(id);
      setUnreadCounts((prev) => {
        const next = { ...prev };
        delete next[id];
        return next;
      });
      setEditorState({ mode: "inbox", botId: id });
    },
    [],
  );

  const handleSaveBot = useCallback(
    async (bot: Bot, schedule: BotSchedule) => {
      const saved = await upsertBot(bot);
      await upsertBotSchedule({ ...schedule, bot_id: saved.id });
      setEditorState({ mode: "closed" });
      setEditorDraft(null);
      await refreshBots();
    },
    [refreshBots],
  );

  // --- send-to-bot (user-to-bot inbox message) ---

  const [sendToBotOpen, setSendToBotOpen] = useState(false);

  const handleSendToBot = useCallback(
    async (toBotId: string, body: string, triggerRun: boolean) => {
      await sendToBot(toBotId, body, triggerRun);
      if (triggerRun) {
        // Switch the active view to the bot's last conversation so
        // the user sees the bot's response stream. The bot's
        // scheduler / executor uses schedule.last_conversation_id as
        // the persistent thread.
        const sched = botSchedules[toBotId];
        if (sched?.last_conversation_id) {
          await refreshConversations();
          setActiveId(sched.last_conversation_id);
        } else {
          // No prior conversation — bot will create one when it
          // runs. Refresh the conversation list so it appears.
          setTimeout(() => {
            refreshConversations();
          }, 500);
        }
        setLastBotFinish(null);
        setRunningBotId(toBotId);
      }
      // Refresh unread counts in case the user sent to a bot with
      // existing unread mail (the bot's view of inbox is unchanged,
      // but for the badge math: the message we just enqueued is
      // unread from the bot's perspective).
      const inbox = await listInbox(toBotId, false);
      setUnreadCounts((prev) => ({ ...prev, [toBotId]: inbox.length }));
    },
    [botSchedules, refreshConversations],
  );

  // --- derived ---

  const activeConversation = useMemo(
    () => conversations.find((c) => c.id === activeId) ?? null,
    [conversations, activeId],
  );
  const activeBot = useMemo(() => {
    if (!activeConversation?.bot_id) return null;
    return bots.find((b) => b.id === activeConversation.bot_id) ?? null;
  }, [activeConversation, bots]);

  // When search is active, group the matching messages by
  // conversation_id and pick the most recent snippet per
  // conversation. The Sidebar renders one row per conversation with
  // the snippet inline.
  const searchView = useMemo(() => {
    const q = searchQuery.trim();
    if (!q) return null;
    const byConv = new Map<
      string,
      { count: number; firstSnippet: string; mostRecentAt: string }
    >();
    for (const m of searchResults) {
      const existing = byConv.get(m.conversation_id);
      // Truncate the matching message to a short snippet, anchoring
      // around the first occurrence of the query.
      const lower = m.content.toLowerCase();
      const idx = lower.indexOf(q.toLowerCase());
      const snippet = makeSnippet(m.content, idx, q.length, 100);
      if (!existing) {
        byConv.set(m.conversation_id, {
          count: 1,
          firstSnippet: snippet,
          mostRecentAt: m.created_at,
        });
      } else {
        existing.count += 1;
        if (m.created_at > existing.mostRecentAt) {
          existing.mostRecentAt = m.created_at;
          existing.firstSnippet = snippet;
        }
      }
    }
    return byConv;
  }, [searchQuery, searchResults]);

  // The list of conversations to render in the sidebar. When
  // searching, this is the subset that has at least one match,
  // ordered by most recent match.
  const visibleConversations = useMemo(() => {
    if (!searchView) return conversations;
    return conversations
      .filter((c) => searchView.has(c.id))
      .sort(
        (a, b) =>
          searchView.get(b.id)!.mostRecentAt.localeCompare(
            searchView.get(a.id)!.mostRecentAt,
          ),
      );
  }, [conversations, searchView]);
  const inboxMessagesForModal = useMemo<BotMessage[]>(() => [], []);

  // Editor draft drives the modal: when the user clicks New/Edit, we
  // prefill the draft. The save handler is in `handleSaveBot` above.
  const editorModal = (() => {
    if (editorState.mode === "closed" || !editorDraft) return null;
    if (editorState.mode === "inbox") {
      const bot = bots.find((b) => b.id === editorState.botId);
      if (!bot) return null;
      return (
        <BotInboxView
          botId={editorState.botId}
          onClose={() => setEditorState({ mode: "closed" })}
        />
      );
    }
    return (
      <BotEditor
        initial={editorDraft.bot}
        schedule={editorDraft.schedule}
        availableTools={editorAvailableTools}
        isNew={editorState.mode === "new"}
        onClose={() => {
          setEditorState({ mode: "closed" });
          setEditorDraft(null);
        }}
        onSave={handleSaveBot}
        onDelete={
          editorState.mode === "edit"
            ? () => {
                handleDeleteBot(editorState.botId);
                setEditorState({ mode: "closed" });
                setEditorDraft(null);
              }
            : undefined
        }
      />
    );
  })();

  const status = useMemo<"online" | "missing" | "checking">(() => {
    if (bootError) return "missing";
    if (!settings) return "checking";
    return settings.minimax_api_key ? "online" : "missing";
  }, [bootError, settings]);

  return (
    <div className="app">
      <Sidebar
        conversations={visibleConversations}
        activeId={activeId}
        bots={bots}
        onSelect={setActiveId}
        onNew={handleNewConversation}
        onDelete={handleDeleteConversation}
        onRename={handleRenameConversation}
        onOpenSettings={() => setSettingsOpen(true)}
        searchQuery={searchQuery}
        onSearchChange={setSearchQuery}
        searchView={
          searchView
            ? Object.fromEntries(
                Array.from(searchView.entries()).map(([id, v]) => [id, v]),
              )
            : null
        }
        status={status}
        botPanel={
          <BotsPanel
            bots={bots}
            schedules={botSchedules}
            runs={botLastRuns}
            unreadCounts={unreadCounts}
            activeRuns={activeRunByBot}
            onStopBot={handleStopBot}
            onNewBot={handleNewBot}
            onEditBot={handleEditBot}
            onDeleteBot={handleDeleteBot}
            onRunBot={handleRunBot}
            onOpenInbox={handleOpenInbox}
            runningBotId={runningBotId}
            collapsed={botsCollapsed}
            onToggleCollapsed={() => setBotsCollapsed((v) => !v)}
          />
        }
      />
      <main className="main">
        {activeBot && (
          <div className="bot-banner">
            <span className="bot-icon">{activeBot.icon || "🤖"}</span>
            <span>
              Viewing <strong>{activeBot.name}</strong>'s thread
            </span>
            {runningBotId === activeBot.id && (
              <span className="bot-running-dot" title="Running now…" />
            )}
            <button
              className="ghost small"
              onClick={() => handleEditBot(activeBot.id)}
            >
              Edit bot
            </button>
          </div>
        )}
        {lastBotFinish && lastBotFinish.conversationId === activeId && (
          <div
            className={`bot-finish-banner${
              lastBotFinish.summary.startsWith("[error]") ? " error" : ""
            }`}
          >
            <span>
              Bot run finished:{" "}
              {lastBotFinish.summary.length > 200
                ? lastBotFinish.summary.slice(0, 200) + "…"
                : lastBotFinish.summary}
            </span>
            <button
              className="ghost small"
              onClick={() => setLastBotFinish(null)}
            >
              Dismiss
            </button>
          </div>
        )}
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
          <ChatView
            messages={messages}
            streamingId={streamingId}
            onRegenerate={handleRegenerate}
          />
        )}
        <Composer
          onSend={handleSend}
          onStop={handleStop}
          onOpenSendToBot={() => setSendToBotOpen(true)}
          hasBots={bots.length > 0}
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
      {editorModal}
      {sendToBotOpen && (
        <SendToBotModal
          bots={bots}
          onClose={() => setSendToBotOpen(false)}
          onSend={handleSendToBot}
        />
      )}
    </div>
  );
}

/**
 * Helper used by the bot chunk handler: keeps a single "bot-streaming"
 * message in the message list. While the bot is streaming, we upsert
 * one ephemeral message with id `__bot_stream__`; on done/error we
 * finalize it (clear the placeholder id, leave content in place).
 */
function upsertBotStreamMessage(
  prev: Message[],
  text: string,
  toolCalls: Map<string, { id: string; name: string; arguments: string }>,
  finalize: boolean = false,
): Message[] {
  const placeholderId = "__bot_stream__";
  const now = new Date().toISOString();
  const tcs = Array.from(toolCalls.values());
  if (finalize) {
    // Drop the placeholder; the persistent assistant messages from the
    // server-side run are already in the DB and will appear on next
    // reload. We just leave the streaming text in the ephemeral row
    // and then remove it.
    return prev.filter((m) => m.id !== placeholderId);
  }
  const idx = prev.findIndex((m) => m.id === placeholderId);
  if (idx === -1) {
    return [
      ...prev,
      {
        id: placeholderId,
        conversation_id: "",
        role: "assistant",
        content: text,
        tool_calls: tcs,
        created_at: now,
      },
    ];
  }
  const next = prev.slice();
  next[idx] = { ...next[idx], content: text, tool_calls: tcs };
  return next;
}

/**
 * Build a snippet around a query match. `idx` is the (case-folded)
 * index of the query in the content; if idx is -1 the snippet
 * starts at the top of the message. The snippet is at most `maxLen`
 * characters and includes a leading "…" when truncated on the left.
 */
function makeSnippet(
  content: string,
  idx: number,
  queryLen: number,
  maxLen: number,
): string {
  if (content.length <= maxLen) return content;
  // Whitespace-collapse newlines so snippets read cleanly in the
  // sidebar.
  const collapsed = content.replace(/\s+/g, " ");
  if (idx < 0) {
    return collapsed.slice(0, maxLen) + "…";
  }
  // Center the snippet around the match.
  const half = Math.floor((maxLen - queryLen) / 2);
  const start = Math.max(0, idx - half);
  const end = Math.min(collapsed.length, start + maxLen);
  const actualStart = Math.max(0, end - maxLen);
  const prefix = actualStart > 0 ? "…" : "";
  const suffix = end < collapsed.length ? "…" : "";
  return prefix + collapsed.slice(actualStart, end) + suffix;
}

/** Modal wrapper that fetches and renders the bot's full inbox. */
function BotInboxView({
  botId,
  onClose,
}: {
  botId: string;
  onClose: () => void;
}) {
  const [bot, setBot] = useState<Bot | null>(null);
  const [msgs, setMsgs] = useState<BotMessage[]>([]);
  useEffect(() => {
    (async () => {
      const [b, m] = await Promise.all([getBot(botId), listInbox(botId, true)]);
      setBot(b);
      setMsgs(m);
    })();
  }, [botId]);
  if (!bot) return null;
  return <BotInbox bot={bot} messages={msgs} onClose={onClose} />;
}
