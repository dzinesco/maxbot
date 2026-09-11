/*
 * v4 — App
 *
 * Top-level state machine for the renderer shell.
 *
 * Boot (first paint):
 *   - Promise.all([listBots, getSettings])
 *   - No listeners. No intervals.
 *   - Two IPC calls total. That's it.
 *
 * Views:
 *   - "roster"  — no bot selected, no surface open. Always
 *                 rendered when no chat/computer/drawer.
 *   - "chat"    — ChatPane mounts for selectedBotId.
 *   - "computer"— ComputerRoute mounts for selectedBotId.
 *   - "approvals" / "settings" — drawer; mount when opened.
 *
 * Loop surface (always present):
 *   - LoopChip — status only, no interval.
 *   - LoopExpanded — on-demand full task + journal.
 *
 * Per Tyler's v4 hard rules:
 *   - First paint = 2 IPC calls.
 *   - No setInterval anywhere in this file.
 *   - No Tauri listen anywhere in this file (loopd IPC
 *     doesn't need listen; chat listeners live in ChatPane;
 *     computer state listeners live in ComputerRoute).
 *
 * Out of scope for v4: the original App.tsx's many surfaces
 * (BotInbox, GroupChatView, SendToBotModal, MemoryPanel,
 * SkillsPanel, RoutinesPanel, BotEditor, Welcome overlays)
 * are not ported. v4 is a stability shell; full surface
 * rebuild is a follow-up.
 */

import { useCallback, useEffect, useState } from "react";
import {
  getSettings,
  listBots,
  loopdStart,
  loopdStop,
} from "../lib/tauri";
import type { Bot, Settings } from "../lib/api";

import { Roster } from "./Roster";
import { ChatPane } from "./ChatPane";
import { ComputerRoute } from "./ComputerRoute";
import { LoopChip } from "./LoopChip";
import { LoopExpanded } from "./LoopExpanded";
import "./styles/app.css";

type View = "roster" | "chat" | "computer" | "approvals" | "settings";

export default function App() {
  const [bots, setBots] = useState<Bot[] | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [bootError, setBootError] = useState<string | null>(null);
  const [selectedBotId, setSelectedBotId] = useState<string | null>(null);
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
      })
      .catch((e) => {
        if (cancelled) return;
        setBootError(String(e));
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const handleSelectBot = useCallback((botId: string) => {
    setSelectedBotId(botId);
    setView("chat");
  }, []);

  const handleOpenComputer = useCallback(() => {
    if (!selectedBotId) return;
    setView("computer");
  }, [selectedBotId]);

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
      // Surface error in console; the chip's own re-fetch
      // path will show a fresh status.
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

  // Render — top-level layout: sidebar | main.
  // Sidebar: roster + loop chip + (optional) loop expanded.
  // Main:    chat pane or computer route, or empty state.

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
              // Reload the window to retry boot.
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
  const selectedBot = bots?.find((b) => b.id === selectedBotId) ?? null;

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
            selectedBotId={selectedBotId}
            onSelectBot={handleSelectBot}
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
            <div className="v4-app-toolbar">
              <button
                type="button"
                className="ghost small"
                onClick={handleOpenComputer}
                title="Open Computer Use for this bot"
              >
                Computer
              </button>
            </div>
            <ChatPane bot={selectedBot} />
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
