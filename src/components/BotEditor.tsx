import { useEffect, useMemo, useState } from "react";
import type { Bot, BotSchedule, ToolSummary } from "../lib/api";
import {
  computerGet,
  computerProvision,
  revealBotFolder,
} from "../lib/tauri";

interface BotEditorProps {
  /** The bot being edited, or a fresh blank bot for create. */
  initial: Bot;
  schedule: BotSchedule | null;
  availableTools: ToolSummary[];
  /** True if this is a new bot (Save will issue an upsert with a fresh id). */
  isNew: boolean;
  onClose: () => void;
  /**
   * Save the bot. Returns the persisted Bot (the parent calls
   * `upsert_bot` and gets back the row with its `id` assigned).
   * The BotEditor needs the returned id to optionally issue
   * `computer_provision` right after, since the Bot row must
   * exist before the per-Bot computer table can reference it.
   */
  onSave: (bot: Bot, schedule: BotSchedule) => Promise<Bot> | Bot;
  onDelete?: () => void;
  /** Open the ComputerPanel in preview mode for the bot being
   * edited. Only invoked when the bot has a computer row.
   * Slice E is expected to wire this through `App.tsx`. */
  onViewComputer?: (botId: string) => void;
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

/**
 * Specialist-Bot templates (v2.0 Slice D). One-click starters
 * inspired by the Grok Bot product pages — Bots are named
 * specialists with a single responsibility ("Figma Bro", "Devbot",
 * "Detective", "Mailroom"). Clicking a chip fills the Name and
 * System Prompt in the editor; the user can still edit before
 * saving. The `name` field is the placeholder shown in the Name
 * input when empty.
 */
interface SpecialistTemplate {
  key: string;
  name: string;
  /** Visible chip label (shorter than the system prompt's first line). */
  label: string;
  /** Tooltip / accessible name. */
  description: string;
  /** The placeholder shown in the Name input. */
  namePlaceholder: string;
  /** The system-prompt body that overwrites `bot.system_prompt`. */
  systemPrompt: string;
}

const SPECIALIST_TEMPLATES: SpecialistTemplate[] = [
  {
    key: "figma",
    name: "Figma Bro",
    label: "Figma Specialist",
    description: "Designs screens, components, and prototypes in Figma.",
    namePlaceholder: "Figma Bro",
    systemPrompt:
      "You are a specialist in Figma. Use the Figma desktop app and plugins to design screens, components, and prototypes. Hand work back to the human when a design is ready for review.",
  },
  {
    key: "code-review",
    name: "Devbot",
    label: "Code Reviewer",
    description: "Reviews diffs, runs tests, and surfaces issues.",
    namePlaceholder: "Devbot",
    systemPrompt:
      "You are a specialist in code review. Use git, ripgrep, and the editor to read diffs, run tests, and surface issues. Hand work back to the human when the review is ready.",
  },
  {
    key: "researcher",
    name: "Detective",
    label: "Researcher",
    description: "Browses the web, gathers, and synthesizes information.",
    namePlaceholder: "Detective",
    systemPrompt:
      "You are a specialist in research. Use the browser, web search, and your file tools to gather and synthesize information. Hand work back to the human when the research is ready.",
  },
  {
    key: "inbox",
    name: "Mailroom",
    label: "Inbox Triage",
    description: "Sorts, schedules, and follows up on messages.",
    namePlaceholder: "Mailroom",
    systemPrompt:
      "You are a specialist in inbox triage. Use mail.app, calendar.app, and reminders.app to sort, schedule, and follow up on messages. Hand work back to the human when the queue is processed.",
  },
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
 * Cron presets expressed in the standard 5-field crontab format
 * (minute hour day-of-month month day-of-week). Evaluated in the
 * user's local timezone by the scheduler.
 */
const CRON_PRESETS: { label: string; expr: string; hint: string }[] = [
  { label: "Off (cron)", expr: "", hint: "no cron schedule" },
  { label: "Every minute", expr: "* * * * *", hint: "* * * * *" },
  {
    label: "Weekdays at 9 AM",
    expr: "0 9 * * 1-5",
    hint: "Mon–Fri 09:00",
  },
  {
    label: "Weekdays at 8:54 AM",
    expr: "54 8 * * 1-5",
    hint: "Mon–Fri 08:54",
  },
  {
    label: "Daily at midnight",
    expr: "0 0 * * *",
    hint: "00:00 every day",
  },
  {
    label: "Daily at 6 AM",
    expr: "0 6 * * *",
    hint: "06:00 every day",
  },
  {
    label: "Hourly on the hour",
    expr: "0 * * * *",
    hint: "00:00 of every hour",
  },
  {
    label: "Mondays only",
    expr: "0 9 * * 1",
    hint: "Mon 09:00",
  },
];

/**
 * Modal for creating or editing a bot. The form is single-page with
 * sections: identity (name/icon/color/description + specialist
 * template chips), brain (default model + system prompt),
 * capabilities (allowed tools), computer (provision VM + disk/RAM),
 * workspace (per-bot filesystem), and schedule.
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
  onViewComputer,
}: BotEditorProps) {
  const [bot, setBot] = useState<Bot>(initial);
  const [intervalSeconds, setIntervalSeconds] = useState<number>(
    schedule?.interval_seconds ?? 0,
  );
  const [cronExpression, setCronExpression] = useState<string>(
    schedule?.cron_expression ?? "",
  );
  const [showTools, setShowTools] = useState(false);
  // ---- Computer section (v2.0 Slice D) ----
  // The provision intent is local to the editor: a fresh blank
  // bot has the checkbox off, the disk defaults to 10 and the
  // RAM to 2048 (the Rust defaults). On an edit we read the
  // existing computer row to surface its state in the panel.
  const [provisionComputer, setProvisionComputer] = useState(false);
  const [provisionDiskGb, setProvisionDiskGb] = useState<number>(10);
  const [provisionRamMb, setProvisionRamMb] = useState<number>(2048);
  const [existingComputer, setExistingComputer] = useState<
    { state: string; vm_ip: string | null; vnc_port: number | null } | null
  >(null);
  const [provisionError, setProvisionError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // Esc closes
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  // On mount, look up the existing computer row (only meaningful
  // when editing a Bot that may already have one). Best-effort:
  // the Rust side may not be running, in which case the panel
  // just shows the "no computer" state.
  useEffect(() => {
    if (isNew || !initial.id) return;
    let cancelled = false;
    computerGet(initial.id)
      .then((c) => {
        if (cancelled || !c) return;
        setExistingComputer({
          state: c.state as string,
          vm_ip: c.vm_ip,
          vnc_port: c.vnc_port,
        });
      })
      .catch(() => {
        // silent — ComputerPanel already handles Tauri errors
      });
    return () => {
      cancelled = true;
    };
  }, [initial.id, isNew]);

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

  /** Apply a specialist template: set the Name (only if empty or
   * already a template name) and the System Prompt. We overwrite
   * the prompt unconditionally so the user can re-apply a
   * template; if they want to keep their custom prompt they just
   * don't click the chip. */
  function applyTemplate(t: SpecialistTemplate) {
    setBot((b) => ({
      ...b,
      // Replace the name wholesale — the templates are designed
      // around the placeholder names (Figma Bro, Devbot, etc.).
      name: t.name,
      system_prompt: t.systemPrompt,
    }));
  }

  async function handleSave() {
    const trimmedName = bot.name.trim();
    if (!trimmedName) {
      alert("Bot name is required.");
      return;
    }
    setProvisionError(null);
    setSaving(true);
    const finalBot: Bot = { ...bot, name: trimmedName };
    const finalSchedule: BotSchedule = {
      bot_id: finalBot.id,
      interval_seconds: intervalSeconds,
      cron_expression: cronExpression.trim(),
      last_run_at: schedule?.last_run_at ?? null,
      last_conversation_id: schedule?.last_conversation_id ?? null,
    };
    try {
      // Persist the Bot first. The Rust side assigns a fresh id
      // for new bots; we need that id to optionally provision
      // the computer right after. The parent's `onSave` returns
      // the saved row (App.tsx awaits `upsert_bot`).
      const saved = await onSave(finalBot, finalSchedule);
      if (provisionComputer && saved?.id) {
        try {
          await computerProvision(saved.id, {
            disk_gb: provisionDiskGb,
            ram_mb: provisionRamMb,
          });
        } catch (e) {
          // The Bot is already saved; the Rust side writes an
          // `error` row in the `computers` table. Surface that
          // here so the user knows the VM didn't come up, but
          // don't close the modal — they may want to retry.
          setProvisionError(
            `Bot saved, but provisioning failed: ${e}. The VM row is in the "error" state — you can retry from the Computer panel or destroy and re-provision.`,
          );
          // Don't `onClose()` — let the user read the error.
          return;
        }
      }
      onClose();
    } catch (e) {
      // The Bot upsert itself failed (network, etc.) — surface
      // the error inline so the user can retry without losing
      // their input.
      setProvisionError(`Could not save bot: ${e}`);
    } finally {
      setSaving(false);
    }
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
                placeholder="e.g. Figma Bro, Devbot, Spec"
              />
            </div>
            <div className="form-row">
              <label>
                Specialist templates
                <span className="hint">
                  {" "}— one click fills the name + system prompt
                </span>
              </label>
              <div
                className="bot-editor__specialist-chips"
                role="group"
                aria-label="Specialist bot templates"
              >
                {SPECIALIST_TEMPLATES.map((t) => (
                  <button
                    key={t.key}
                    type="button"
                    className="bot-editor__specialist-chip"
                    onClick={() => applyTemplate(t)}
                    title={t.description}
                    data-testid={`specialist-chip-${t.key}`}
                  >
                    {t.label}
                  </button>
                ))}
              </div>
              <div className="hint">
                Bots with a single responsibility (per the Grok Bot
                product pages) are easier to hand off and easier to
                improve over time. Pick a template, then edit the
                prompt to fit your workflow.
              </div>
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

          {/* Workspace (per-bot filesystem) */}
          <section className="form-section">
            <h3>Workspace</h3>
            <div className="form-row">
              <label>
                Bot folder
                <span className="hint"> — the bot's own working directory</span>
              </label>
              <div className="bot-folder-row">
                <code className="bot-folder-path">
                  ~/Library/Application Support/com.maxbot.app/bots/{bot.id}/
                </code>
                <button
                  type="button"
                  className="ghost small"
                  onClick={async () => {
                    try {
                      await revealBotFolder(bot.id);
                    } catch (e) {
                      alert(`Could not reveal folder: ${e}`);
                    }
                  }}
                >
                  Reveal in Finder
                </button>
              </div>
              <div className="muted small hint">
                Contains <code>agents.md</code> (long-term memory, auto-seeded
                from the system prompt above and re-read on every run),
                <code> scratchpad.md</code> (working notes), and
                <code> outputs/</code> (artifacts the bot produces). The bot
                can read and write to these via the{" "}
                <code>memory_*</code>, <code>scratchpad_*</code>, and{" "}
                <code>outputs_*</code> tools.
              </div>
            </div>
          </section>

          {/* Computer (v2.0 Slice D) */}
          <section className="form-section bot-editor__computer-section">
            <h3>
              Computer
              {existingComputer && (
                <span
                  className={`computer-panel__status-dot computer-panel__status-dot--${existingComputer.state}`}
                  style={{ marginLeft: 8 }}
                  title={`VM state: ${existingComputer.state}`}
                />
              )}
              {existingComputer && onViewComputer && (
                <button
                  type="button"
                  className="ghost small"
                  style={{ marginLeft: 8 }}
                  onClick={() => onViewComputer(bot.id || initial.id)}
                >
                  View computer
                </button>
              )}
            </h3>
            {existingComputer && (
              <div className="muted small">
                This bot has a computer:{" "}
                <strong>{existingComputer.state}</strong>
                {existingComputer.vm_ip ? ` · ${existingComputer.vm_ip}` : ""}
                {existingComputer.vnc_port !== null
                  ? ` · VNC :${existingComputer.vnc_port}`
                  : ""}
                . Re-provisioning from here is not supported — destroy
                and create a new computer from the Computer panel.
              </div>
            )}
            <label className="bot-editor__provision-toggle">
              <input
                type="checkbox"
                checked={provisionComputer}
                onChange={(e) => setProvisionComputer(e.target.checked)}
                data-testid="provision-computer-checkbox"
              />
              <span>
                Provision a computer for this Bot
                <span className="hint">
                  {" "}— spin up a per-Bot Linux VM (libvirt on the
                  Settings → Computer server). The Bot drives the
                  VM via SSH and you can preview / take over from
                  the Computer panel.
                </span>
              </span>
            </label>
            {provisionComputer && (
              <div className="form-row inline bot-editor__computer-fields">
                <div>
                  <label>Disk size (GB, default 10)</label>
                  <input
                    type="number"
                    min={1}
                    value={provisionDiskGb}
                    onChange={(e) =>
                      setProvisionDiskGb(
                        Math.max(1, parseInt(e.target.value || "10", 10)),
                      )
                    }
                    data-testid="provision-disk-gb"
                  />
                </div>
                <div>
                  <label>RAM (MB, default 2048)</label>
                  <input
                    type="number"
                    min={256}
                    step={256}
                    value={provisionRamMb}
                    onChange={(e) =>
                      setProvisionRamMb(
                        Math.max(256, parseInt(e.target.value || "2048", 10)),
                      )
                    }
                    data-testid="provision-ram-mb"
                  />
                </div>
              </div>
            )}
            {provisionError && (
              <div
                className="bot-editor__provision-error"
                role="alert"
                data-testid="provision-error"
              >
                {provisionError}
              </div>
            )}
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
              <label>
                Cron expression{" "}
                <span className="hint">
                  — 5-field crontab in local time, e.g. "0 9 * * 1-5" for
                  "Weekdays at 9:00 AM". Wins over the simple interval
                  when set.
                </span>
              </label>
              <div className="preset-row">
                {CRON_PRESETS.map((p) => (
                  <button
                    key={p.label}
                    type="button"
                    className={`interval-preset${
                      cronExpression === p.expr ? " active" : ""
                    }`}
                    onClick={() => setCronExpression(p.expr)}
                    title={p.hint}
                  >
                    {p.label}
                  </button>
                ))}
                <input
                  type="text"
                  value={cronExpression}
                  onChange={(e) => setCronExpression(e.target.value)}
                  placeholder="0 9 * * 1-5"
                  style={{ width: 140, marginLeft: 4, fontFamily: "var(--font-mono)" }}
                />
              </div>
              {cronExpression && (
                <div className="muted small">
                  Preview: <code>{describeCron(cronExpression)}</code>
                </div>
              )}
            </div>
            <div className="form-row">
              <label>
                Simple interval{" "}
                <span className="hint">— fallback when cron is empty</span>
              </label>
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
          <button onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button
            className="primary"
            onClick={handleSave}
            disabled={saving}
            data-testid="bot-editor-save"
          >
            {saving
              ? provisionComputer
                ? "Creating…"
                : "Saving…"
              : isNew
                ? "Create bot"
                : "Save"}
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

/// Tiny human-readable description of a 5-field cron expression. Not
/// a full English converter — just enough to give the user a hint that
/// their expression is well-formed.
function describeCron(expr: string): string {
  const parts = expr.trim().split(/\s+/);
  if (parts.length !== 5) {
    return `expected 5 fields, got ${parts.length}`;
  }
  const [m, h, dom, mon, dow] = parts;
  const time = `${h.padStart(2, "0") || "*"}:${m.padStart(2, "0") || "*"}`;
  if (dow === "1-5" || dow === "MON-FRI") return `weekdays at ${time}`;
  if (dow === "0,6" || dow === "SAT,SUN") return `weekends at ${time}`;
  if (dow === "1" || dow === "MON") return `Mondays at ${time}`;
  if (dow === "*" && dom === "*") return `daily at ${time}`;
  if (dow === "*") return `${dom} of every month at ${time}`;
  return expr;
}
