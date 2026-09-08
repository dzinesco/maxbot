import { useEffect } from "react";
import type { Bot, BotMessage } from "../lib/api";

interface BotInboxProps {
  bot: Bot;
  messages: BotMessage[];
  onClose: () => void;
}

/**
 * Read-only inbox view for a bot. Lists messages from other bots in
 * reverse chronological order; the parent calls `markInboxRead` after
 * open so the unread badge clears. The modal is dismissable with Esc or
 * the close button.
 */
export function BotInbox({ bot, messages, onClose }: BotInboxProps) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal bot-inbox"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={`${bot.name} inbox`}
      >
        <header className="modal-header">
          <h2>
            <span className="bot-icon">{bot.icon || "🤖"}</span> {bot.name} ·
            inbox
          </h2>
          <button className="ghost small" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>
        <div className="modal-body">
          {messages.length === 0 ? (
            <div className="muted">No messages.</div>
          ) : (
            <ul className="inbox-list">
              {[...messages]
                .sort((a, b) => b.created_at.localeCompare(a.created_at))
                .map((m) => (
                  <li
                    key={m.id}
                    className={`inbox-item${m.read ? "" : " unread"}`}
                  >
                    <div className="inbox-meta">
                      <span className="muted small">From</span>{" "}
                      <code>{m.from_bot_id}</code>
                      <span className="muted small">
                        {" "}
                        · {new Date(m.created_at).toLocaleString()}
                      </span>
                    </div>
                    <div className="inbox-body">{m.body}</div>
                  </li>
                ))}
            </ul>
          )}
        </div>
        <footer className="modal-footer">
          <div className="spacer" />
          <button onClick={onClose}>Close</button>
        </footer>
      </div>
    </div>
  );
}
