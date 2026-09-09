import { useEffect, useState } from "react";
import type { Bot } from "../lib/api";

interface SendToBotModalProps {
  bots: Bot[];
  onClose: () => void;
  onSend: (toBotId: string, body: string, triggerRun: boolean) => Promise<void>;
}

/**
 * Modal for the user to send a message to one of their bots. The
 * message is enqueued in the bot's inbox and (by default) the bot is
 * triggered to run immediately so the user sees the response. The
 * "trigger run" checkbox lets the user enqueue without kicking the
 * bot off — useful when they want to schedule a batch of requests.
 */
export function SendToBotModal({ bots, onClose, onSend }: SendToBotModalProps) {
  const [selectedBotId, setSelectedBotId] = useState(bots[0]?.id ?? "");
  const [body, setBody] = useState("");
  const [triggerRun, setTriggerRun] = useState(true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  if (bots.length === 0) {
    return (
      <div className="modal-backdrop" onClick={onClose}>
        <div
          className="modal"
          onClick={(e) => e.stopPropagation()}
          role="dialog"
          aria-modal="true"
        >
          <div className="modal-body">
            <div className="muted">
              You don't have any bots yet. Create one from the Bots panel in
              the sidebar.
            </div>
          </div>
          <div className="modal-footer">
            <div className="spacer" />
            <button onClick={onClose}>Close</button>
          </div>
        </div>
      </div>
    );
  }

  async function handleSend() {
    if (!body.trim() || !selectedBotId) return;
    setBusy(true);
    try {
      await onSend(selectedBotId, body.trim(), triggerRun);
      onClose();
    } catch (e) {
      alert(`Send failed: ${e}`);
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal send-to-bot-modal"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label="Send to bot"
      >
        <header className="modal-header">
          <h2>Send to bot</h2>
          <button
            className="ghost small"
            onClick={onClose}
            aria-label="Close"
          >
            ✕
          </button>
        </header>
        <div className="modal-body">
          <div className="form-row">
            <label>Bot</label>
            <select
              value={selectedBotId}
              onChange={(e) => setSelectedBotId(e.target.value)}
            >
              {bots.map((b) => (
                <option key={b.id} value={b.id}>
                  {b.icon || "🤖"} {b.name}
                  {b.description ? ` — ${b.description}` : ""}
                </option>
              ))}
            </select>
          </div>
          <div className="form-row">
            <label>Message</label>
            <textarea
              autoFocus
              value={body}
              onChange={(e) => setBody(e.target.value)}
              rows={6}
              placeholder="What do you want the bot to do? Be specific — this is the kickoff prompt the bot will see."
            />
          </div>
          <label className="checkbox">
            <input
              type="checkbox"
              checked={triggerRun}
              onChange={(e) => setTriggerRun(e.target.checked)}
            />
            Run the bot now so I see the response
          </label>
        </div>
        <footer className="modal-footer">
          <div className="muted small">
            The message is enqueued in the bot's inbox regardless.
          </div>
          <div className="spacer" />
          <button onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button
            className="primary"
            onClick={handleSend}
            disabled={busy || !body.trim() || !selectedBotId}
          >
            {busy ? "Sending…" : triggerRun ? "Send & run" : "Enqueue"}
          </button>
        </footer>
      </div>
    </div>
  );
}
