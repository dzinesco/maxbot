import { useState } from "react";
import type { Bot, BotRun, BotSchedule } from "../lib/api";

interface BotsPanelProps {
  bots: Bot[];
  schedules: Record<string, BotSchedule | undefined>;
  runs: Record<string, BotRun | undefined>;
  unreadCounts: Record<string, number>;
  /** Map of bot id → currently active run id (cancellable). */
  activeRuns: Record<string, string>;
  onNewBot: () => void;
  onEditBot: (id: string) => void;
  onDeleteBot: (id: string) => void;
  onRunBot: (id: string) => void;
  onStopBot: (id: string) => void;
  onOpenInbox: (id: string) => void;
  /** Bot id currently being executed (shows a small spinner). */
  runningBotId: string | null;
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

/**
 * Sidebar panel listing all configured bots. Each row is a compact card
 * with the bot's icon, name, a one-line description, the schedule, and
 * the last run's status. The row exposes "Run", "Edit", and "Inbox"
 * actions; "Inbox" only shows when there are unread messages.
 */
export function BotsPanel({
  bots,
  schedules,
  runs,
  unreadCounts,
  activeRuns,
  onNewBot,
  onEditBot,
  onDeleteBot,
  onRunBot,
  onStopBot,
  onOpenInbox,
  runningBotId,
  collapsed,
  onToggleCollapsed,
}: BotsPanelProps) {
  return (
    <section className="bots-panel">
      <header
        className="bots-panel-header"
        onClick={onToggleCollapsed}
        title={collapsed ? "Click to expand" : "Click to collapse"}
      >
        <span className="caret">{collapsed ? "▶" : "▼"}</span>
        <span>Bots</span>
        <span className="bots-panel-count">{bots.length}</span>
        <button
          className="primary small"
          onClick={(e) => {
            e.stopPropagation();
            onNewBot();
          }}
          title="Create a new bot"
        >
          + New
        </button>
      </header>
      {!collapsed && (
        <div className="bots-list">
          {bots.length === 0 && (
            <div className="bots-empty">
              No bots yet. Click "+ New" to spin one up.
            </div>
          )}
          {bots.map((bot) => (
            <BotRow
              key={bot.id}
              bot={bot}
              schedule={schedules[bot.id]}
              lastRun={runs[bot.id]}
              unread={unreadCounts[bot.id] ?? 0}
              running={runningBotId === bot.id}
              hasActiveRun={bot.id in activeRuns}
              onEdit={() => onEditBot(bot.id)}
              onDelete={() => onDeleteBot(bot.id)}
              onRun={() => onRunBot(bot.id)}
              onStop={() => onStopBot(bot.id)}
              onOpenInbox={() => onOpenInbox(bot.id)}
            />
          ))}
        </div>
      )}
    </section>
  );
}

interface BotRowProps {
  bot: Bot;
  schedule?: BotSchedule;
  lastRun?: BotRun;
  unread: number;
  running: boolean;
  hasActiveRun: boolean;
  onEdit: () => void;
  onDelete: () => void;
  onRun: () => void;
  onStop: () => void;
  onOpenInbox: () => void;
}

function BotRow({
  bot,
  schedule,
  lastRun,
  unread,
  running,
  hasActiveRun,
  onEdit,
  onDelete,
  onRun,
  onStop,
  onOpenInbox,
}: BotRowProps) {
  const [menuOpen, setMenuOpen] = useState(false);
  const intervalLabel = formatSchedule(schedule);
  return (
    <div
      className={`bot-row${running ? " running" : ""}`}
      style={bot.color ? { borderLeftColor: bot.color } : undefined}
    >
      <div className="bot-row-main">
        <span className="bot-icon" aria-hidden>
          {bot.icon || "🤖"}
        </span>
        <div className="bot-meta">
          <div className="bot-name-line">
            <span className="bot-name">{bot.name || "(unnamed)"}</span>
            {unread > 0 && (
              <button
                className="bot-unread"
                onClick={onOpenInbox}
                title={`${unread} unread message(s)`}
              >
                {unread}
              </button>
            )}
            {running && (
              <span className="bot-running-dot" title="Running now…" />
            )}
          </div>
          <div className="bot-sub">
            {bot.description || (
              <span className="muted">No description</span>
            )}
          </div>
          <div className="bot-sub small">
            <span className="muted">{intervalLabel}</span>
            {lastRun && (
              <>
                <span className="muted">·</span>
                <span className={`bot-status bot-status-${lastRun.status}`}>
                  {lastRun.status}
                </span>
              </>
            )}
          </div>
        </div>
        <div className="bot-actions">
          {hasActiveRun ? (
            <button
              onClick={onStop}
              className="danger small"
              title="Stop the current run"
            >
              ◼ Stop
            </button>
          ) : (
            <button
              onClick={onRun}
              disabled={running}
              className="primary small"
              title="Run this bot now"
            >
              {running ? "Running…" : "Run"}
            </button>
          )}
          <button
            className="ghost small"
            onClick={() => setMenuOpen((v) => !v)}
            title="More"
            aria-haspopup="true"
            aria-expanded={menuOpen}
          >
            ⋯
          </button>
        </div>
      </div>
      {menuOpen && (
        <div className="bot-menu">
          <button
            onClick={() => {
              setMenuOpen(false);
              onEdit();
            }}
          >
            Edit
          </button>
          {unread > 0 && (
            <button
              onClick={() => {
                setMenuOpen(false);
                onOpenInbox();
              }}
            >
              Inbox ({unread})
            </button>
          )}
          <button
            className="danger"
            onClick={() => {
              setMenuOpen(false);
              onDelete();
            }}
          >
            Delete
          </button>
        </div>
      )}
    </div>
  );
}

function formatSchedule(schedule: BotSchedule | undefined): string {
  if (!schedule) return "no schedule";
  if (schedule.cron_expression && schedule.cron_expression.trim()) {
    return `cron: ${schedule.cron_expression}`;
  }
  const seconds = schedule.interval_seconds;
  if (!seconds || seconds <= 0) return "no schedule";
  if (seconds < 60) return `every ${seconds}s`;
  if (seconds < 3600) return `every ${Math.round(seconds / 60)}m`;
  if (seconds < 86400) return `every ${Math.round(seconds / 3600)}h`;
  return `every ${Math.round(seconds / 86400)}d`;
}
