/*
 * v4 — App (S2)
 *
 * v3.7.17 Slice S2 — session, not a demo.
 *
 * State machine:
 *   - `bots`: full bot list (from boot `listBots`).
 *   - `settings`: app settings (from boot `getSettings`).
 *   - `selectedBotId`: persisted via `usePersistedBotId`. Restored
 *     on next launch so the user lands on the same bot.
 *   - `selectedConvId`: thread within the selected bot. Reset to
 *     null when the user picks a different bot.
 *
 * Per the S2 brief:
 *   - New chat button creates a conversation for the selected
 *     bot and switches to it.
 *   - Conversation list for the selected bot only, fetched on
 *     bot select (not at boot). Lives in `ConversationsList`.
 *   - Clicking a thread loads that conversation; last selected
 *     bot id persisted.
 *   - Boot remains exactly listBots + getSettings.
 *   - No setInterval; no history poll while streaming.
 *
 * Per the v4 hard rules:
 *   - First paint = 2 IPC calls.
 *   - No setInterval anywhere.
 *   - No Tauri listen anywhere in this file (chat listeners live
 *     in ChatPane; computer state listeners live in ComputerRoute).
 *
 * Layout:
 *   sidebar: brand → Roster → (if a bot) ConversationsList → LoopChip + LoopExpanded
 *   main:    ChatPane (when bot + conv) | ComputerRoute (when bot + view=computer) | empty state
 */

import { useCallback, useEffect, useState } from "react";
import {
  createConversation,
  getSettings,
  listBots,
  loopdStart,
  loopdStop,
} from "../lib/tauri";
import type { Bot, Settings } from "../lib/api";

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

export default function App() {
  const [bots, setBots] = useState<Bot[] | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  // Persisted bot selection. The hook reads localStorage on mount
  // and writes on every change. We seed with `null` until the
  // bot list arrives (so we can validate the stored id).
  const [persistedBotId, setPersistedBotId] = usePersistedBotId();
  const [selectedConvId, setSelectedConvId] = useState<string | null>(null);
  const [newChatBusy, setNewChatBusy] = useState(false);
  const [view, setView] = useState<View>("roster");
  const [loopExpanded, setLoopExpanded] = useState(false);
  const [loopBusy, setLoopBusy] = useState<"start" | "stop" | null>(null);

  // Boot — exactly two IPC calls. No listeners, no intervals.
  useEffect(() => {
    let cancelled = false;
    Promise.all([listBots(), getSettings()])
      .then(([bs, ss]) => {
        if (cancelled) return;
        setBots(bs);
        setSettings(ss);
        // Validate the persisted bot id now that we have the list.
        // If the stored bot was deleted, drop the persisted value
        // and clear selectedBotId. If valid, restore it.
        if (persistedBotId && bs.find((b) => b.id === persistedBotId)) {
          // already set; nothing to do.
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

  const handleSelectBot = useCallback((botId: string) => {
    setPersistedBotId(botId);
    setSelectedConvId(null);
    setView("chat");
  }, [setPersistedBotId]);

  const handleSelectConv = useCallback((convId: string) => {
    setSelectedConvId(convId);
    setView("chat");
  }, []);

  const handleNewChat = useCallback(async () => {
    if (!persistedBotId) return;
    setNewChatBusy(true);
    try {
      const conv = await createConversation(undefined, persistedBotId);
      setSelectedConvId(conv.id);
      setView("chat");
    } catch (e) {
      console.error("createConversation failed:", e);
    } finally {
      setNewChatBusy(false);
    }
  }, [persistedBotId]);

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

  const handleLoopStart = useCallback(async () => {
    setLoopBusy("start");
    try {
      await loopdStart();
    } catch (e) {
      console.error("loopdStart failed:", e);
    } finally {
      setLoopBusy(null);
    }
  }, []);

  const handleLoopStop = useCallback(async () => {
    setLoopBusy("stop");
    try {
      await loopdStop();
    } catch (e) {
      console.error("loopdStop failed:", e);
    } finally {
      setLoopBusy(null);
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

        {persistedBotId && bots && (
          <ConversationsList
            botId={persistedBotId}
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
            onStart={handleLoopStart}
            onStop={handleLoopStop}
            busy={loopBusy !== null}
          />
          <LoopExpanded open={loopExpanded} />
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
              <ChatPane bot={selectedBot} conversationId={selectedConvId} />
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
