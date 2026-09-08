import { useEffect, useMemo, useState } from "react";
import type { Bot, BotSchedule, ToolSummary } from "../lib/api";

interface BotEditorProps {
  /** The bot being edited, or a fresh blank bot for create. */
  initial: Bot;
  schedule: BotSchedule | null;
  availableTools: ToolSummary[];
  /** True if this is a new bot (Save will issue an upsert with a fresh id). */
  isNew: boolean;
  onClose: () => void;
  onSave: (bot: Bot, schedule: BotSchedule) => void;
  onDelete?: () => void;
}

const ICON_PRESETS = ["🤖", "📋", "📧", "📅", "🔍", "🛠️", "🎨", "💼", "🐕", "🐱", "🚀", "🧠"];
const COLOR_PRESETS = [
  "",
  "#7c5cff",
  "#ff7c5c",
  "#5cff7c",
  "#5cd6ff",
  "#ff5cd6",
  "#ffd35c",
  "#5cffd3",
];

const SCHEDULE_PRESETS: { label: string; seconds: number }[] = [
  { label: "Off", seconds: 0 },
  { label: "Every minute", seconds: 60 },
  { label: "Every 5 min", seconds: 300 },
  { label: "Every 15 min", seconds: 900 },
  { label: "Every 30 min", seconds: 1800 },
  { label: "Every hour", seconds: 3600 },
  { label: "Every 6 hours", seconds: 21600 },
  { label: "Daily", seconds: 86400 },
];

/**
 * Modal for creating or editing a bot. The form is single-page with
 * sections: identity (name/icon/color/description), brain (default
 * model + system prompt), capabilities (allowed tools), and schedule.
 * Save is a single upsert; cancel discards.
 */
export function BotEditor({
  initial,
  schedule,
  availableTools,
  isNew,
  onClose,
  onSave,
  onDelete,
}: BotEditorProps) {
  const [bot, setBot] = useState<Bot>(initial);
  const [intervalSeconds, setIntervalSeconds] = useState<number>(
    schedule?.interval_seconds ?? 0,
  );
  const [showTools, setShowTools] = useState(false);

  // Esc closes
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const toolByName = useMemo(() => {
    const m = new Map<string, ToolSummary>();
    for (const t of availableTools) m.set(t.name, t);
    return m;
  }, [availableTools]);

  function toggleTool(name: string) {
    setBot((b) => {
      const has = b.allowed_tools.includes(name);
      return {
        ...b,
        allowed_tools: has
          ? b.allowed_tools.filter((t) => t !== name)
          : [...b.allowed_tools, name],
      };
    });
  }

  function handleSave() {
    const trimmedName = bot.name.trim();
    if (!trimmedName) {
      alert("Bot name is required.");
      return;
    }
    const finalBot: Bot = { ...bot, name: trimmedName };
    const finalSchedule: BotSchedule = {
      bot_id: finalBot.id,
      interval_seconds: intervalSeconds,
      last_run_at: schedule?.last_run_at ?? null,
      last_conversation_id: schedule?.last_conversation_id ?? null,
    };
    onSave(finalBot, finalSchedule);
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal bot-editor"
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={isNew ? "New bot" : "Edit bot"}
      >
        <header className="modal-header">
          <h2>{isNew ? "New bot" : "Edit bot"}</h2>
          <button className="ghost small" onClick={onClose} aria-label="Close">
            ✕
          </button>
        </header>

        <div className="modal-body">
          {/* Identity */}
          <section className="form-section">
            <h3>Identity</h3>
            <div className="form-row">
              <label>Name</label>
              <input
                value={bot.name}
                onChange={(e) => setBot({ ...bot, name: e.target.value })}
                placeholder="e.g. Elon, Clipboard, Spec"
              />
            </div>
            <div className="form-row">
              <label>Description</label>
              <input
                value={bot.description}
                onChange={(e) =>
                  setBot({ ...bot, description: e.target.value })
                }
                placeholder="One-line purpose"
              />
            </div>
            <div className="form-row inline">
              <div>
                <label>Icon</label>
                <div className="preset-row">
                  {ICON_PRESETS.map((i) => (
                    <button
                      key={i}
                      type="button"
                      className={`icon-preset${bot.icon === i ? " active" : ""}`}
                      onClick={() => setBot({ ...bot, icon: i })}
                    >
                      {i}
                    </button>
                  ))}
                  <input
                    type="text"
                    value={bot.icon}
                    onChange={(e) => setBot({ ...bot, icon: e.target.value })}
                    style={{ width: 48, marginLeft: 4 }}
                    maxLength={2}
                  />
                </div>
              </div>
              <div>
                <label>Color</label>
                <div className="preset-row">
                  {COLOR_PRESETS.map((c) => (
                    <button
                      key={c || "default"}
                      type="button"
                      className={`color-preset${bot.color === c ? " active" : ""}`}
                      style={c ? { background: c } : {}}
                      onClick={() => setBot({ ...bot, color: c })}
                      title={c || "default"}
                    />
                  ))}
                </div>
              </div>
            </div>
          </section>

          {/* Brain */}
          <section className="form-section">
            <h3>Brain</h3>
            <div className="form-row">
              <label>Default model</label>
              <input
                value={bot.default_model}
                onChange={(e) =>
                  setBot({ ...bot, default_model: e.target.value })
                }
                placeholder="MiniMax-M3"
              />
            </div>
            <div className="form-row">
              <label>
                System prompt
                <span className="hint"> — what the bot "is" and how it should act</span>
              </label>
              <textarea
                value={bot.system_prompt}
                onChange={(e) =>
                  setBot({ ...bot, system_prompt: e.target.value })
                }
                rows={8}
                placeholder={
                  "You are a careful, concise analyst. When asked to act, summarize what you did and why. Use the available tools when they help."
                }
              />
            </div>
          </section>

          {/* Capabilities */}
          <section className="form-section">
            <h3>
              Capabilities
              <button
                type="button"
                className="ghost small"
                onClick={() => setShowTools((v) => !v)}
                style={{ marginLeft: 8 }}
              >
                {showTools ? "Hide" : `Edit (${bot.allowed_tools.length})`}
              </button>
            </h3>
            {bot.allowed_tools.length === 0 ? (
              <div className="muted small">
                No tools enabled. The bot will only be able to chat.
              </div>
            ) : (
              <ul className="enabled-tools">
                {bot.allowed_tools.map((t) => {
                  const meta = toolByName.get(t);
                  return (
                    <li key={t}>
                      <code>{t}</code>
                      {meta?.requires_consent && (
                        <span className="badge small" title="Requires user consent">
                          consent
                        </span>
                      )}
                    </li>
                  );
                })}
              </ul>
            )}
            {showTools && (
              <ul className="tool-picker">
                {availableTools.map((t) => {
                  const checked = bot.allowed_tools.includes(t.name);
                  return (
                    <li key={t.name}>
                      <label>
                        <input
                          type="checkbox"
                          checked={checked}
                          onChange={() => toggleTool(t.name)}
                        />
                        <code>{t.name}</code>
                        {t.requires_consent && (
                          <span className="badge small">consent</span>
                        )}
                        <span className="muted small"> — {t.description}</span>
                      </label>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>

          {/* Schedule */}
          <section className="form-section">
            <h3>Schedule</h3>
            <div className="form-row">
              <label>Run interval</label>
              <div className="preset-row">
                {SCHEDULE_PRESETS.map((p) => (
                  <button
                    key={p.label}
                    type="button"
                    className={`interval-preset${
                      intervalSeconds === p.seconds ? " active" : ""
                    }`}
                    onClick={() => setIntervalSeconds(p.seconds)}
                  >
                    {p.label}
                  </button>
                ))}
                <input
                  type="number"
                  min={0}
                  value={intervalSeconds}
                  onChange={(e) =>
                    setIntervalSeconds(parseInt(e.target.value || "0", 10))
                  }
                  style={{ width: 80, marginLeft: 4 }}
                />
                <span className="muted small">seconds</span>
              </div>
            </div>
            {schedule?.last_run_at && (
              <div className="muted small">
                Last run: {formatRelative(schedule.last_run_at)}
              </div>
            )}
          </section>
        </div>

        <footer className="modal-footer">
          {!isNew && onDelete && (
            <button
              className="danger"
              onClick={() => {
                if (
                  confirm(
                    `Delete "${bot.name}"? This also removes its schedule and run history.`,
                  )
                ) {
                  onDelete();
                }
              }}
            >
              Delete bot
            </button>
          )}
          <div className="spacer" />
          <button onClick={onClose}>Cancel</button>
          <button className="primary" onClick={handleSave}>
            {isNew ? "Create bot" : "Save"}
          </button>
        </footer>
      </div>
    </div>
  );
}

function formatRelative(iso: string): string {
  const t = new Date(iso).getTime();
  const now = Date.now();
  const dt = Math.max(0, now - t);
  const s = Math.floor(dt / 1000);
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86400)}d ago`;
}
