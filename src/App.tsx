import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { Composer } from "./components/Composer";
import { Settings } from "./components/Settings";
// v2.0 Slice E: the `BotsPanel` component is now a thin
// re-export of `BotRoster` (kept for backward compat). The
// sidebar mounts the roster directly, so this import is
// no longer needed. `BotsPanel` itself is still importable
// from older entry points — it just renders the same
// roster underneath.
import { BotEditor } from "./components/BotEditor";
import { BotInbox } from "./components/BotInbox";
import { ComputerPanel } from "./components/ComputerPanel";
import { SendToBotModal } from "./components/SendToBotModal";
import { Welcome } from "./components/Welcome";
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
  metaGet,
  metaSet,
  migrateMessageErrorShape,
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
  ttsSpeak,
  ttsStop,
  upsertBot,
  upsertBotSchedule,
} from "./lib/tauri";
import {
  blankBot,
  DEFAULT_SETTINGS,
  isConfigured,
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
  /**
   * First-run gate. Read from the `meta` table on bootstrap. `null`
   * means the bootstrap hasn't finished yet (we're still in the
   * loading state); `true` means show the chat; `false` means show
   * the welcome. The user can flip back to `false` from Settings
   * via the "Reset onboarding" entry.
   */
  const [isOnboarded, setIsOnboarded] = useState<boolean | null>(null);
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
  const [ttsSpeaking, setTtsSpeaking] = useState(false);
  // v2.0 Slice E: the Bot currently focused in the sidebar
  // roster. Drives (a) the highlight in the roster, (b) the
  // conversation list shown in ChatView, and (c) the
  // "Viewing <Bot>'s thread" banner. `null` = no Bot
  // selected (e.g. brand-new install with no Bots).
  const [selectedBotId, setSelectedBotId] = useState<string | null>(null);
  // v2.0 Slice D: when the user clicks "View computer" in the
  // BotEditor, we open the ComputerPanel in preview mode for
  // the bot being edited. The BotEditor closes itself; the
  // panel mounts on top.
  const [computerPanelBotId, setComputerPanelBotId] = useState<string | null>(
    null,
  );
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
        const [s, c, b, sched, tools, onboarded] = await Promise.all([
          getSettings(),
          listConversations(),
          listBots(),
          listAllSchedules(),
          listAvailableTools(),
          metaGet("is_onboarded"),
        ]);
        setSettings(s);
        setConversations(c);
        setBots(b);
        const schedMap: Record<string, BotSchedule> = {};
        for (const sc of sched) schedMap[sc.bot_id] = sc;
        setBotSchedules(schedMap);
        setAvailableTools(tools);
        setIsOnboarded(onboarded === "1");
        // v2.0 Slice E: per-Bot chat scoping. On boot, pick a
        // sensible starting Bot: (a) if the user has any
        // Bots, auto-select the first one and open its
        // most-recent conversation (or create one); (b) if
        // they have no Bots, leave the chat area in the
        // "Select or create a Bot" empty state.
        if (onboarded === "1") {
          if (b.length > 0) {
            // Auto-select the first bot by name. The roster
            // sorts alphabetically (Rust `ORDER BY name`), so
            // this is deterministic across launches.
            const firstBot = b[0];
            setSelectedBotId(firstBot.id);
            const matching = c
              .filter((conv) => conv.bot_id === firstBot.id)
              .sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1));
            if (matching[0]) {
              setActiveId(matching[0].id);
            } else {
              // No prior conversation for this Bot — create
              // a fresh one so the chat area isn't stuck on
              // the "no conversations" empty state.
              const created = await createConversation(
                undefined,
                firstBot.id,
              );
              setConversations([...c, created]);
              setActiveId(created.id);
            }
          } else if (c.length > 0) {
            // Legacy path: no Bots yet (e.g. a v1.0 install
            // upgrading to v2.0) but there are old
            // conversations. Open the first one so the
            // user doesn't see an empty screen.
            setActiveId(c[0].id);
          }
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
          // v0.7.6: the error no longer gets appended to the visible
          // content. The Rust side already persisted `error_message`
          // on the message row, but we set it on the in-memory state
          // too so the UI re-renders the ErrorMessage block
          // immediately without a DB reload.
          setMessages((prev) =>
            prev.map((m) =>
              m.id === pending.assistantId
                ? { ...m, error_message: message }
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
        // v0.7.6 migration: messages that pre-date the friendly
        // error UX still carry the old "[error] …" suffix appended
        // to their `content`. On first load after upgrade, split
        // those out: the prefix becomes `content`, the trailing
        // error text becomes `error_message`. Idempotent — once a
        // message has the new shape, this leaves it alone.
        const normalized = m.map(splitLegacyErrorSuffix);
        setMessages(normalized);
        // Persist the split shape for any messages that needed
        // migration, so the next reload doesn't re-run the
        // regex on every page load. Fire-and-forget; a failure
        // here just means the migration runs again next time.
        for (let i = 0; i < m.length; i++) {
          if (normalized[i] !== m[i]) {
            const { content, error_message } = normalized[i];
            migrateMessageErrorShape(
              m[i].id,
              content,
              error_message ?? "",
            ).catch((e) => {
              console.warn("migrate_message_error_shape failed:", e);
            });
          }
        }
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

  // v2.0 Slice E: per-Bot chat scoping. Selecting a Bot in
  // the roster either opens the most-recent conversation for
  // that Bot, or creates a new one if there is none. The
  // chat list shown in ChatView is filtered to this Bot.
  const handleSelectBot = useCallback(
    async (botId: string) => {
      setSelectedBotId(botId);
      const matching = conversations
        .filter((c) => c.bot_id === botId)
        .sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1));
      if (matching[0]) {
        setActiveId(matching[0].id);
        return;
      }
      // No prior conversation — create one for this Bot and
      // jump to it. The renderer's empty state would otherwise
      // show "no conversations for this Bot" forever.
      const created = await createConversation(undefined, botId);
      await refreshConversations();
      setActiveId(created.id);
    },
    [conversations, refreshConversations],
  );

  // v2.0 Slice E: per-Bot "new conversation" handler. Used
  // by both the header's "+ New chat" button and the in-chat
  // CTA. Pass through the currently-selected Bot; the parent
  // is the canonical owner of "which Bot is selected" so we
  // don't accept a `botId` parameter from the chat area (we
  // read it from `selectedBotId` instead).
  const handleNewBotConversation = useCallback(async () => {
    if (!selectedBotId) {
      // Defensive: ChatView shouldn't show the button when
      // no Bot is selected, but if a stale state sneaks
      // through, fall back to the global handler.
      await handleNewConversation();
      return;
    }
    const created = await createConversation(undefined, selectedBotId);
    await refreshConversations();
    setActiveId(created.id);
  }, [selectedBotId, handleNewConversation, refreshConversations]);

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
      if (!isConfigured(settings)) {
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
          error_message: null,
        };
        const assistantMsg: Message = {
          id: result.assistant_message_id,
          conversation_id: activeId,
          role: "assistant",
          content: "",
          tool_calls: [],
          created_at: now,
          error_message: null,
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

  // ---- Voice (TTS) ----
  // The most recent assistant message (skipping the in-flight streaming
  // one when its content is still empty). Used by the composer's 🔊
  // button and the ⌘⇧S keyboard shortcut.
  const lastAssistantText = useMemo(() => {
    for (let i = messages.length - 1; i >= 0; i--) {
      const m = messages[i];
      if (m.role === "assistant" && m.content.trim().length > 0) {
        return m.content;
      }
    }
    return null;
  }, [messages]);

  const handleToggleSpeakLast = useCallback(async () => {
    if (ttsSpeaking) {
      try {
        await ttsStop();
      } catch (e) {
        console.error("tts stop failed:", e);
      }
      setTtsSpeaking(false);
      return;
    }
    if (!lastAssistantText) return;
    setTtsSpeaking(true);
    const chars = lastAssistantText.length;
    const approxMs = Math.max(3000, Math.ceil(chars / 12) * 1000);
    setTimeout(() => setTtsSpeaking(false), approxMs);
    try {
      await ttsSpeak(lastAssistantText);
    } catch (e) {
      console.error("tts speak failed:", e);
      setTtsSpeaking(false);
    }
  }, [ttsSpeaking, lastAssistantText]);

  // ⌘⇧S → speak the last assistant response (or stop if already
  // v2.0 Slice E: the sidebar's "+ New chat" header button
  // dispatches a `maxbot:new-conversation` custom event with
  // the currently-selected Bot id. We listen for it here so
  // the chat area (which is the canonical owner of
  // conversation creation) handles the call in one place.
  useEffect(() => {
    const onNew = (e: Event) => {
      const detail = (e as CustomEvent<{ botId: string }>).detail;
      if (detail && detail.botId) {
        // The sidebar already set `selectedBotId`; just call
        // the per-Bot handler. (If the user has no Bots
        // selected the button is a no-op.)
        handleNewBotConversation();
      } else {
        handleNewConversation();
      }
    };
    const onSelectFirst = () => {
      // The sidebar couldn't create a chat because no Bot
      // is selected. Surface a soft prompt — a tiny toast
      // banner that auto-dismisses. We don't have a global
      // toast system, so a console hint + a temporary
      // banner element is the path of least resistance.
      // (The BotsPanel empty state already shows the
      // "Create your first Bot" CTA, so the user can
      // self-recover.)
      if (bots.length === 0) {
        // No Bots at all — the chat area already shows
        // the empty state, so just bail.
        return;
      }
      // Bots exist but none selected — the most common
      // cause is a stale state. Auto-select the first
      // Bot and create a conversation for it.
      if (bots[0]) {
        setSelectedBotId(bots[0].id);
      }
    };
    window.addEventListener("maxbot:new-conversation", onNew);
    window.addEventListener("maxbot:select-bot-first", onSelectFirst);
    return () => {
      window.removeEventListener("maxbot:new-conversation", onNew);
      window.removeEventListener(
        "maxbot:select-bot-first",
        onSelectFirst,
      );
    };
  }, [bots, handleNewBotConversation, handleNewConversation]);

  // speaking). Bound at the document level so it works from anywhere
  // in the chat surface, not just inside the composer.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (
        (e.metaKey || e.ctrlKey) &&
        e.shiftKey &&
        (e.key === "S" || e.key === "s")
      ) {
        e.preventDefault();
        handleToggleSpeakLast();
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [handleToggleSpeakLast]);

  // ⌘N → start a new chat. Bound at the document level so it works
  // from anywhere in the app. We suppress the default so the browser
  // doesn't open a new window. While the Settings or Bot editor
  // modals are open we let the input handling inside the modal win
  // — the existing modal escape / cancel paths cover that surface.
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (
        (e.metaKey || e.ctrlKey) &&
        !e.shiftKey &&
        !e.altKey &&
        (e.key === "N" || e.key === "n")
      ) {
        // Don't fire if the user is typing into a text field inside
        // an open modal — let the input own the keystroke.
        const target = e.target as HTMLElement | null;
        if (target && (target.tagName === "INPUT" || target.tagName === "TEXTAREA")) {
          return;
        }
        e.preventDefault();
        // v2.0 Slice E: per-Bot "new conversation" — uses
        // the currently-selected Bot. If no Bot is selected
        // the global handler falls back to a fresh Bot-less
        // conversation (which the chat area can still
        // open).
        if (selectedBotId) {
          handleNewBotConversation();
        } else {
          handleNewConversation();
        }
      }
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [handleNewBotConversation, handleNewConversation, selectedBotId]);

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
            error_message: null,
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
      // The editor stays open after this returns: the BotEditor
      // uses the returned `saved` Bot to call `computerProvision`
      // (when the user opted in) and then closes itself. We
      // deliberately don't `setEditorState({ mode: "closed" })`
      // here — closing mid-save would unmount the modal before
      // the provision call has a chance to surface its error.
      await refreshBots();
      return saved;
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
  // v2.0 Slice E: per-Bot chat scoping. The chat area's
  // pill row shows the conversations tied to the
  // currently-selected Bot, sorted most-recent first. A
  // Bot with zero conversations renders an empty pill
  // row (the parent shows the "no conversations" empty
  // state instead).
  const scopedConversations = useMemo(() => {
    if (!selectedBotId) return [];
    return conversations
      .filter((c) => c.bot_id === selectedBotId)
      .sort((a, b) => (a.updated_at < b.updated_at ? 1 : -1));
  }, [conversations, selectedBotId]);
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
        onViewComputer={(botId) => setComputerPanelBotId(botId)}
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
    return isConfigured(settings) ? "online" : "missing";
  }, [bootError, settings]);

  // Path shown on the welcome surface when the user picks "I have a
  // .env file". Mirrors the env_loader's first candidate, so a
  // copy-paste of the shown path is the most likely to actually
  // take effect on the next launch.
  const envHintPath = useMemo(() => {
    const home =
      (typeof process !== "undefined" && process.env?.HOME) || "~";
    return `${home}/.maxbot.env`;
  }, []);

  const handleOnboardingComplete = useCallback(async () => {
    // Set the meta flag (idempotent — the dialog also writes it, but
    // the "Skip for now" path doesn't go through the dialog). Then
    // flip local state and seed a default conversation so the user
    // lands on a fresh chat.
    try {
      await metaSet("is_onboarded", "1");
    } catch (e) {
      console.error("metaSet is_onboarded failed:", e);
    }
    setIsOnboarded(true);
    if (conversations.length === 0) {
      try {
        const created = await createConversation(undefined, undefined);
        setConversations([created]);
        setActiveId(created.id);
      } catch (e) {
        console.error("createConversation post-onboard failed:", e);
      }
    } else if (!activeId) {
      setActiveId(conversations[0].id);
    }
  }, [conversations, activeId]);

  // Loading state: don't flash the welcome before bootstrap completes.
  // A blank screen with the brand color is fine here — the bootstrap
  // typically resolves in <100ms. If the bootstrap throws, surface
  // the error inline so the user is not stuck on a blank black
  // window with no way to know what went wrong.
  if (isOnboarded === null) {
    return (
      <div className="app app-boot" style={{ padding: 24, color: "#fafafa" }}>
        {bootError ? (
          <div data-testid="boot-error" style={{ maxWidth: 720 }}>
            <h1 style={{ fontSize: 16, color: "#f87171", marginTop: 0 }}>
              MaxBot couldn't finish starting
            </h1>
            <p style={{ color: "#a1a1aa", fontSize: 13 }}>
              The bootstrap threw before we could decide whether to show
              onboarding or the chat. Restart MaxBot (Cmd+Q, reopen) — if
              it keeps happening, paste the error below.
            </p>
            <pre
              style={{
                background: "#0e0e10",
                padding: 12,
                borderRadius: 6,
                overflow: "auto",
                whiteSpace: "pre-wrap",
                margin: 0,
                fontSize: 12,
                color: "#fafafa",
                fontFamily:
                  "ui-monospace, SFMono-Regular, Menlo, monospace",
              }}
            >
              {bootError}
            </pre>
          </div>
        ) : null}
      </div>
    );
  }

  // First-run gate. The Welcome is full-bleed (no sidebar) — it's
  // a focused, single-decision surface. The sidebar is reachable
  // only after the user dismisses.
  if (!isOnboarded) {
    return (
      <div className="app">
        <Welcome
          initialSettings={settings}
          onComplete={handleOnboardingComplete}
          envHintPath={envHintPath}
        />
      </div>
    );
  }

  return (
    <div className="app">
      <Sidebar
        // v2.0 Slice E: the sidebar's primary object is the
        // Bot roster. The conversation list and the old
        // message-search UI have been removed from the
        // sidebar (the conversation list now lives in the
        // chat area, scoped to the selected Bot). Search
        // across messages is still available — see
        // `ChatView` for the in-chat search box.
        selectedBotId={selectedBotId}
        activeConversationId={activeId}
        bots={bots}
        lastRunsByBot={botLastRuns}
        onSelectBot={handleSelectBot}
        onCreateBot={handleNewBot}
        onOpenComputer={(botId) => setComputerPanelBotId(botId)}
        status={status}
        onOpenSettings={() => setSettingsOpen(true)}
      />
      <main className="main">
        {bots.length === 0 ? (
          // v2.0 Slice E: brand-new user with no Bots.
          // The chat area shows a single, focused "Create
          // your first Bot" prompt instead of an empty
          // thread. The composer is hidden — there is no
          // Bot to send a message to yet.
          <div className="main-empty main-empty--no-bots">
            <div className="main-empty-mark" aria-hidden="true">
              M
            </div>
            <h1>Welcome to MaxBot</h1>
            <p className="main-empty-tagline">
              Bots are persistent assistants with their own tools, computer,
              and memory. Create your first one to start chatting.
            </p>
            <button
              className="primary main-empty-cta"
              onClick={handleNewBot}
              data-testid="main-empty-cta"
            >
              Create your first Bot
            </button>
            {bootError && (
              <p style={{ color: "var(--danger)" }}>{bootError}</p>
            )}
          </div>
        ) : (
          <>
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
                <div className="main-empty-mark">M</div>
                <h1>MaxBot</h1>
                <p className="main-empty-tagline">
                  A multi-provider AI desktop client with sub-agents, browser
                  automation, and the Grok Build CLI in your toolbelt.
                  {status === "missing"
                    ? " Set your LLM provider API key in Settings to begin."
                    : " Type a message below to start."}
                </p>
                <div className="main-empty-hints">
                  <span className="main-empty-hint">
                    <kbd>⌘</kbd>+<kbd>⇧</kbd>+<kbd>S</kbd> reads the last
                    response aloud
                  </span>
                  <span className="main-empty-hint">
                    <kbd>⌘</kbd>+<kbd>F</kbd> searches across conversations
                  </span>
                </div>
                {bootError && (
                  <p style={{ color: "var(--danger)" }}>{bootError}</p>
                )}
              </div>
            ) : (
              <ChatView
                messages={messages}
                streamingId={streamingId}
                onRegenerate={handleRegenerate}
                activeBot={activeBot}
                scopedConversations={scopedConversations}
                activeConversationId={activeId}
                onSelectConversation={setActiveId}
                onNewConversation={handleNewBotConversation}
              />
            )}
            <Composer
              onSend={handleSend}
              onStop={handleStop}
              onOpenSendToBot={() => setSendToBotOpen(true)}
              hasBots={bots.length > 0}
              streaming={streamingId !== null}
              lastAssistantText={lastAssistantText}
              ttsSpeaking={ttsSpeaking}
              onToggleSpeakLast={handleToggleSpeakLast}
            />
          </>
        )}
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
      {computerPanelBotId && (
        <div
          className="modal-overlay"
          onClick={() => setComputerPanelBotId(null)}
        >
          <div
            className="modal-stacked modal-stacked-wide"
            onClick={(e) => e.stopPropagation()}
          >
            <ComputerPanel
              botId={computerPanelBotId}
              mode="preview"
              onClose={() => setComputerPanelBotId(null)}
            />
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * v0.7.6 one-time migration: assistant messages written by v0.7.5
 * (and earlier) carry the error as a "\n\n[error] …" suffix on
 * `content`. The new error UX wants `content` to be the streamed
 * text and `error_message` to be the friendly description. This
 * function splits the suffix out when it sees the legacy shape and
 * leaves clean messages alone.
 *
 * The match is intentionally narrow: only a single trailing
 * `\n\n[error] ...` block. Anything more elaborate is left as-is
 * so we never accidentally mangle valid content (e.g. a literal
 * `[error]` in a code block the user typed in).
 */
export function splitLegacyErrorSuffix(message: Message): Message {
  // Already migrated — error_message is set, content is the clean
  // streamed text. Don't touch it.
  if (message.error_message) return message;
  const marker = "\n\n[error] ";
  const idx = message.content.lastIndexOf(marker);
  if (idx < 0) return message;
  const prefix = message.content.slice(0, idx);
  const raw = message.content.slice(idx + marker.length).trim();
  // Map the raw v0.7.5 string to a friendly message the same way
  // the Rust side does for fresh errors. The matching here is
  // best-effort — we keep the raw text as a fallback so we never
  // silently drop information the user might have wanted to see.
  const friendly = legacyRawErrorToFriendly(raw);
  return {
    ...message,
    content: prefix,
    error_message: friendly ?? raw,
  };
}

/** Best-effort mapping of the v0.7.5 raw error text to a friendly
 * phrase. `null` means "no specific match" — the caller falls back
 * to the raw text. Kept in lockstep with `StreamError::friendly_message`
 * in `src-tauri/src/llm/stream.rs`. */
function legacyRawErrorToFriendly(raw: string): string | null {
  const lower = raw.toLowerCase();
  if (lower.includes("missing api key")) return "API key not set — open Settings";
  if (lower.startsWith("http 401") || lower.startsWith("http 403"))
    return "Authentication failed — check your API key";
  if (lower.startsWith("http 5")) return "The provider is having trouble — try again";
  if (lower.startsWith("http 4")) return "Authentication failed — check your API key";
  if (lower.startsWith("network") || lower.includes("connection"))
    return "Connection lost — check your network";
  if (lower.startsWith("protocol"))
    return "The model returned an unexpected response";
  return null;
}

/**
 * Helper used by the bot chunk handler: keeps a single "bot-streaming"
 * message in the message list. While the bot is streaming, we upsert

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
        error_message: null,
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
