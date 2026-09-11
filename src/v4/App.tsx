/*
 * v4 — App (S2.6)
 *
 * v3.7.17 Slice S2.6 — fix the S2.5 screenshot review:
 *   - Selecting a bot auto-selects the latest thread; if
 *     none, creates a new one so the composer is live.
 *   - First user message becomes the thread title. The
 *     "New chat" label is replaced as soon as the user
 *     sends. We update both the local list state (so the
 *     thread row re-renders immediately) and the persisted
 *     DB title via `renameConversation`.
 *   - The Loop chip is now a small text row only — the
 *     big Start/Stop buttons are removed from the default
 *     rail. LoopChip itself handles the slim rendering.
 *
 * State machine:
 *   - `bots`: full bot list (from boot `listBots`).
 *   - `settings`: app settings (from boot `getSettings`).
 *   - `selectedBotId`: persisted via `usePersistedBotId`.
 *   - `conversations`: LIFTED from ConversationsList. Held
 *     here so a bot select can fetch + auto-select or
 *     create, and so a first-user-message in ChatPane can
 *     update the local list immediately.
 *   - `selectedConvId`: thread within the selected bot.
 *
 * v4 hard rules:
 *   - Boot = exactly listBots + getSettings. No new IPC at
 *     boot. No setInterval.
 *   - No Tauri listeners here (chat listeners live in
 *     ChatPane; computer state listeners live in
 *     ComputerRoute).
 *
 * Layout:
 *   sidebar: brand → Roster → ConversationsList → LoopChip + LoopExpanded
 *   main:    ChatPane (when bot + conv) | ComputerRoute (when bot + view=computer) | empty state
 */

import { useCallback, useEffect, useState } from "react";
import {
  createConversation,
  getSettings,
  listBots,
  listConversations,
  loopdStart,
  loopdStop,
  renameConversation,
} from "../lib/tauri";
import type { Bot, Conversation, Settings } from "../lib/api";

import { Roster } from "./Roster";
import { ChatPane } from "./ChatPane";
import { ComputerRoute } from "./ComputerRoute";
import { ConversationsList } from "./ConversationsList";
import { LoopChip } from "./LoopChip";
import { LoopExpanded } from "./LoopExpanded";
import { ApprovalSheet } from "./ApprovalSheet";
import { usePersistedBotId } from "./usePersistedBotId";
import "./styles/app.css";

type View = "roster" | "chat" | "computer" | "approvals" | "settings";

const DEFAULT_NEW_CHAT_TITLE = "New chat";

/** Truncate the first user message into a thread title. Word-
 *  boundary aware, max 60 chars. */
function truncateTitle(text: string, max = 60): string {
  const flat = text.replace(/\s+/g, " ").trim();
  if (flat.length <= max) return flat;
  // Find the last whitespace before `max` so we don't slice a word.
  const cut = flat.slice(0, max);
  const ws = cut.lastIndexOf(" ");
  if (ws > max * 0.6) return cut.slice(0, ws) + "…";
  return cut + "…";
}

/** Sort conversations by updated_at desc. The "latest" thread
 *  for a bot is the first item in this list. */
function sortByUpdated(list: Conversation[]): Conversation[] {
  return list.slice().sort((a, b) => {
    const at = Date.parse(a.updated_at || a.created_at || "") || 0;
    const bt = Date.parse(b.updated_at || b.created_at || "") || 0;
    return bt - at;
  });
}

export default function App() {
  const [bots, setBots] = useState<Bot[] | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [persistedBotId, setPersistedBotId] = usePersistedBotId();
  const [conversations, setConversations] = useState<Conversation[] | null>(
    null,
  );
  const [selectedConvId, setSelectedConvId] = useState<string | null>(null);
  const [selectingBot, setSelectingBot] = useState(false);
  const [newChatBusy, setNewChatBusy] = useState(false);
  const [view, setView] = useState<View>("roster");
  const [loopExpanded, setLoopExpanded] = useState(false);

  // Boot — exactly two IPC calls. No listeners, no intervals.
  useEffect(() => {
    let cancelled = false;
    Promise.all([listBots(), getSettings()])
      .then(([bs, ss]) => {
        if (cancelled) return;
        setBots(bs);
        setSettings(ss);
        // Validate the persisted bot id now that we have the list.
        if (persistedBotId && bs.find((b) => b.id === persistedBotId)) {
          // stored id is valid; restore it after the effect below
          // finishes by triggering a bot select.
        } else if (persistedBotId) {
          // stored id isn't in the bot list anymore; clear it.
          setPersistedBotId(null);
        }
      })
      .catch((e) => {
        if (cancelled) return;
        setBootError(String(e));
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []); // boot once; persistedBotId is read at mount via the hook.

  // Restore persisted bot on boot — after the list arrives,
  // run the same auto-select path the user-driven bot click
  // uses. Single source of truth for "selecting a bot = fetch
  // latest conv, or create one if none".
  useEffect(() => {
    if (bots === null) return;
    if (!persistedBotId) return;
    if (selectingBot) return;
    if (selectedConvId !== null) return;
    if (conversations !== null) return; // already loaded for this bot
    void handleSelectBot(persistedBotId);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [bots, persistedBotId]);

  /**
   * Bot select: fetch conversations for the bot, pick the
   * latest (sorted by updated_at desc), or create a new one
   * if the bot has none. Single source of truth — used by the
   * Roster click and by the persisted-bot restore on boot.
   */
  const handleSelectBot = useCallback(
    async (botId: string) => {
      if (selectingBot) return;
      setSelectingBot(true);
      setPersistedBotId(botId);
      setView("chat");
      try {
        const list = await listConversations(botId);
        const sorted = sortByUpdated(list);
        if (sorted.length > 0) {
          setConversations(sorted);
          setSelectedConvId(sorted[0].id);
          return;
        }
        // No threads yet — create one so the composer is live
        // immediately (S2.6 brief: "if none, create one so
        // composer is live").
        const conv = await createConversation(undefined, botId);
        setConversations([conv]);
        setSelectedConvId(conv.id);
      } catch (e) {
        console.error("handleSelectBot failed:", e);
        setConversations([]);
        setSelectedConvId(null);
      } finally {
        setSelectingBot(false);
      }
    },
    [selectingBot, setPersistedBotId],
  );

  const handleSelectConv = useCallback((convId: string) => {
    setSelectedConvId(convId);
    setView("chat");
  }, []);

  const handleNewChat = useCallback(async () => {
    if (!persistedBotId) return;
    setNewChatBusy(true);
    try {
      const conv = await createConversation(undefined, persistedBotId);
      setConversations((prev) => sortByUpdated([...(prev ?? []), conv]));
      setSelectedConvId(conv.id);
      setView("chat");
    } catch (e) {
      console.error("createConversation failed:", e);
    } finally {
      setNewChatBusy(false);
    }
  }, [persistedBotId]);

  /**
   * First-user-message callback from ChatPane. Updates the
   * local conversation title (so the thread row re-renders
   * immediately) AND persists it via `renameConversation` so
   * the title survives an app restart. Fire-and-forget — we
   * don't block the send flow on the rename IPC.
   *
   * Only renames if the current title is the Rust default
   * ("New chat") or empty. After the first rename, subsequent
   * sends into the same thread keep the existing title.
   */
  const handleFirstUserMessage = useCallback(
    (convId: string, content: string) => {
      const nextTitle = truncateTitle(content);
      let shouldRename = false;
      setConversations((prev) => {
        if (!prev) return prev;
        const idx = prev.findIndex((c) => c.id === convId);
        if (idx < 0) return prev;
        const cur = prev[idx];
        const curTitle = (cur.title || "").trim();
        if (curTitle && curTitle !== DEFAULT_NEW_CHAT_TITLE) {
          // Already has a meaningful title — leave it alone.
          return prev;
        }
        shouldRename = true;
        const next = prev.slice();
        next[idx] = { ...cur, title: nextTitle };
        return next;
      });
      if (shouldRename) {
        renameConversation(convId, nextTitle).catch((e) => {
          console.error("renameConversation failed:", e);
        });
      }
    },
    [],
  );

  const handleOpenComputer = useCallback(() => {
    if (!persistedBotId) return;
    setView("computer");
  }, [persistedBotId]);

  const handleCloseComputer = useCallback(() => {
    setView("chat");
  }, []);

  const handleToggleLoopExpand = useCallback(() => {
    setLoopExpanded((v) => !v);
  }, []);

  // LoopChip in S2.6 doesn't expose Start/Stop from the rail —
  // the action is parked (see LoopChip.tsx). The handlers below
  // stay defined so a future slice can wire them into the
  // expanded panel without touching App again.
  const handleLoopStart = useCallback(async () => {
    try {
      await loopdStart();
    } catch (e) {
      console.error("loopdStart failed:", e);
    }
  }, []);

  const handleLoopStop = useCallback(async () => {
    try {
      await loopdStop();
    } catch (e) {
      console.error("loopdStop failed:", e);
    }
  }, []);

  if (bootError) {
    return (
      <div className="v4-app v4-app--boot-error">
        <div className="v4-app-boot-error-panel">
          <h1>MaxBot failed to start</h1>
          <pre className="v4-app-boot-error-msg">{bootError}</pre>
          <button
            type="button"
            className="primary"
            onClick={() => {
              window.location.reload();
            }}
          >
            Retry
          </button>
        </div>
      </div>
    );
  }

  const isLoading = bots === null || settings === null;
  const selectedBot = bots?.find((b) => b.id === persistedBotId) ?? null;

  return (
    <div className="v4-app">
      <aside className="v4-app-sidebar">
        <div className="v4-app-sidebar-header">
          <span className="v4-app-brand">MaxBot</span>
          {isLoading && <span className="v4-app-loading">…</span>}
        </div>

        {bots && (
          <Roster
            bots={bots}
            selectedBotId={persistedBotId}
            onSelectBot={handleSelectBot}
          />
        )}

        {persistedBotId && (
          <ConversationsList
            botId={persistedBotId}
            conversations={conversations}
            selectedConvId={selectedConvId}
            onSelectConv={handleSelectConv}
            onNewChat={handleNewChat}
            newChatBusy={newChatBusy}
          />
        )}

        <div className="v4-app-sidebar-bottom">
          <LoopChip
            onToggleExpand={handleToggleLoopExpand}
            expanded={loopExpanded}
          />
          <LoopExpanded
            open={loopExpanded}
            onStart={handleLoopStart}
            onStop={handleLoopStop}
          />
        </div>
      </aside>

      <main className="v4-app-main">
        {view === "chat" && selectedBot && (
          <>
            {/* S2.5 — Computer toolbar dropped from idle layout.
                ChatPane owns its own header (bot name + status + Stop).
                The Computer route is parked; users wanting Computer
                Use can fall back to the daily /Applications/MaxBot.app
                or wait for a follow-up slice that re-adds a
                Computer button in the ChatPane header. */}
            <div className="v4-app-chat-area">
              <ChatPane
                bot={selectedBot}
                conversationId={selectedConvId}
                onFirstUserMessage={handleFirstUserMessage}
                onOpenComputer={handleOpenComputer}
              />
              {/* v4 S3 — ApprovalSheet renders the first pending
                  approval for the selected bot. Fetches on mount
                  + window focus. No interval. */}
              <ApprovalSheet botId={selectedBot.id} />
            </div>
          </>
        )}
        {view === "computer" && selectedBot && (
          <ComputerRoute
            botId={selectedBot.id}
            botName={selectedBot.name}
            onClose={handleCloseComputer}
          />
        )}
        {view === "roster" && (
          <div className="v4-app-empty">
            <div className="v4-app-empty-text">
              Pick a bot from the roster to start chatting.
            </div>
          </div>
        )}
        {view === "approvals" && (
          <div className="v4-app-empty">
            <div className="v4-app-empty-text">
              Approvals drawer — not ported in v4 yet.
            </div>
          </div>
        )}
        {view === "settings" && (
          <div className="v4-app-empty">
            <div className="v4-app-empty-text">
              Settings — not ported in v4 yet.
            </div>
          </div>
        )}
      </main>
    </div>
  );
}
