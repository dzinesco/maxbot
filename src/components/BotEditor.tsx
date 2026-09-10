import { useEffect, useMemo, useState } from "react";
import type { Bot, BotSchedule, Rule, ToolSummary } from "../lib/api";
import {
  applyGrokBotDefaults,
  approvalRuleList,
  approvalRuleSet,
  computerGet,
  computerProvision,
  getDaemonToken,
  getSettings,
  revealBotFolder,
  rotateDaemonToken,
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
  // ---- v2.6.0 — Approval rules ----
  // Per-Bot per-tool rule (auto / ask / deny). Loaded
  // once on mount from `approval_rule_list`. The
  // editor shows one row per known tool; missing
  // tools default to `auto` until the user sets
  // something else.
  const [rules, setRules] = useState<
    Record<string, Rule>
  >({});
  // v2.6.0 — pull the existing rules once we have
  // an `id`. For brand-new Bots (no id yet) we leave
  // the map empty; the Rust side seeds defaults on
  // first save, but the editor's preview shows
  // `auto` everywhere until that happens.
  useEffect(() => {
    const targetId = bot.id || initial.id;
    if (!targetId) return;
    let cancelled = false;
    approvalRuleList(targetId)
      .then((rows) => {
        if (cancelled) return;
        const map: Record<string, Rule> = {};
        for (const r of rows) map[r.tool_name] = r.rule;
        setRules(map);
      })
      .catch(() => {
        /* non-fatal — render auto by default */
      });
    return () => {
      cancelled = true;
    };
  }, [bot.id, initial.id]);

  // v2.8.0 — Daemon: load the per-Bot bearer token
  // (or `null` for "(not set)") and the Settings
  // (for the webhook URL's host). Both are async; we
  // only fetch once per Bot (the token doesn't change
  // unless the user clicks Rotate).
  const [daemonToken, setDaemonToken] = useState<string | null>(null);
  const [daemonServerHost, setDaemonServerHost] = useState<string>("");
  // v3.1.0 — Test-webhook status. `idle` means the button
  // is fresh; `running` disables it; `ok` / `error` show a
  // small inline message next to the button. Kept in
  // component state (not a toast) so the user can see the
  // result without dismissing anything.
  const [testStatus, setTestStatus] = useState<
    | { kind: "idle" }
    | { kind: "running" }
    | { kind: "ok"; message: string }
    | { kind: "error"; message: string }
  >({ kind: "idle" });
  useEffect(() => {
    let cancelled = false;
    if (isNew) {
      // New bots have no token yet.
      setDaemonToken(null);
      return;
    }
    getDaemonToken(bot.id)
      .then((tok) => {
        if (!cancelled) setDaemonToken(tok);
      })
      .catch(() => {
        if (!cancelled) setDaemonToken(null);
      });
    getSettings()
      .then((s) => {
        if (!cancelled) setDaemonServerHost(s.computer_server_host || "");
      })
      .catch(() => {
        /* non-fatal — the URL just shows "<host>:8443/..." */
      });
    return () => {
      cancelled = true;
    };
  }, [bot.id, isNew]);

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
                data-setting-key={`bot.${bot.id || initial.id}.name`}
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
                data-setting-key={`bot.${bot.id || initial.id}.description`}
              />
            </div>
            <div className="form-row inline">
              <div>
                <label>Icon</label>
                <div className="preset-row" data-setting-key={`bot.${bot.id || initial.id}.icon`}>
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
                <div className="preset-row" data-setting-key={`bot.${bot.id || initial.id}.color`}>
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
                data-setting-key={`bot.${bot.id || initial.id}.default-model`}
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
                data-setting-key={`bot.${bot.id || initial.id}.system-prompt`}
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
                  data-setting-key={`bot.${bot.id || initial.id}.view-computer`}
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
                data-setting-key={`bot.${bot.id || initial.id}.provision-computer`}
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
                    data-setting-key={`bot.${bot.id || initial.id}.provision-disk-gb`}
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
                    data-setting-key={`bot.${bot.id || initial.id}.provision-ram-mb`}
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
            {/* v3.2.0 — Computer Use target. The Bot editor
                lets the user pick between the in-VM path
                (default, the Bot's own Linux VM drives
                Chromium + xdotool) and the Mac /
                AppleScript path. "Mac with approval" is
                reserved for the v3.4.0 Takeover work; the
                enum value lives in the schema today so
                future Bots don't need a migration. The
                setting is per-Bot — different Bots can
                drive different surfaces. New Bots default
                to "vm" via `blankBot()`. */}
            <div className="form-row">
              <label>
                Computer Use
                <span className="hint">
                  {" "}— which surface the Bot's Computer
                  Use tools drive. The VM is the default in
                  v3.2.0; the Mac path is an opt-in
                  fallback.
                </span>
              </label>
              <select
                value={bot.computer_use || "vm"}
                onChange={(e) =>
                  setBot({ ...bot, computer_use: e.target.value })
                }
                data-testid="computer-use-select"
                data-setting-key={`bot.${bot.id || initial.id}.computer-use`}
              >
                <option value="vm">VM (default)</option>
                <option value="mac">Mac</option>
                <option value="mac-with-approval">Mac with approval</option>
              </select>
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
                data-setting-key={`bot.${bot.id || initial.id}.allowed-tools`}
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

          {/* v2.6.0 — Approval rules. One row per
            known tool. The user picks auto / ask /
            deny; the change is fire-and-forget
            persisted so the user can keep editing
            without waiting. The Rust side seeds
            defaults on first install, so for an
            existing Bot the map starts populated;
            for a new Bot every row is `auto` until
            the user picks something. */}
          <section className="form-section" data-setting-key={`bot.${bot.id || initial.id}.approval-rules`}>
            <h3>
              Rules
              <span className="hint">
                {" "}— auto runs, ask queues, deny blocks
              </span>
            </h3>
            {/* v3.4.0 (Phase 5) — Grok Bot defaults
              preset. One click resets every rule for
              this Bot to the Phase 5 preset
              (read-only auto, send/payment/destroy
              ask). Useful when a user has
              over-customized and wants to start over.
              The `apply_grok_bot_defaults` Tauri
              command replaces the rows in
              `approval_rules`; we then refetch and
              update local state. The button is
              disabled for new Bots (id is empty)
              because the upsert path already applies
              the preset on first save. */}
            <div className="rule-preset-row">
              <button
                type="button"
                className="interval-preset"
                data-testid="grok-bot-defaults-button"
                disabled={!bot.id && !initial.id}
                title="Reset every rule to the Grok Bot preset (read-only auto, send/payment/destroy ask)."
                onClick={() => {
                  const targetId = bot.id || initial.id;
                  if (!targetId) return;
                  applyGrokBotDefaults(targetId)
                    .then(() => approvalRuleList(targetId))
                    .then((rows) => {
                      const m: Record<string, Rule> = {};
                      for (const r of rows) m[r.tool_name] = r.rule;
                      setRules(m);
                    })
                    .catch((e) =>
                      console.warn("apply_grok_bot_defaults failed:", e),
                    );
                }}
              >
                Grok Bot defaults
              </button>
              <span className="hint">
                Resets every rule to read-only auto, send/payment/destroy ask.
              </span>
            </div>
            {/* v3.0.2 — settings-palette bridge. The Rules
              section is one logical "setting" — there's no
              single field to focus, but the user expects
              this label to be reachable. The
              data-setting-key on the section lets the
              palette scroll it into view. */}
            <ul className="rule-list">
              {availableTools.map((t) => {
                const current = rules[t.name] ?? "auto";
                return (
                  <li
                    key={t.name}
                    className="rule-list__row"
                    data-testid={`rule-row-${t.name}`}
                  >
                    <code className="rule-list__tool">{t.name}</code>
                    <div className="rule-list__radios">
                      {(["auto", "ask", "deny"] as Rule[]).map((r) => (
                        <label
                          key={r}
                          className="rule-list__radio"
                          data-testid={`rule-radio-${t.name}-${r}`}
                        >
                          <input
                            type="radio"
                            name={`rule-${t.name}`}
                            value={r}
                            checked={current === r}
                            onChange={() => {
                              setRules((prev) => ({
                                ...prev,
                                [t.name]: r,
                              }));
                              const targetId = bot.id || initial.id;
                              if (targetId) {
                                approvalRuleSet(targetId, t.name, r).catch(
                                  (e) =>
                                    console.warn(
                                      "approval_rule_set failed:",
                                      e,
                                    ),
                                );
                              }
                            }}
                          />
                          <span>{r}</span>
                        </label>
                      ))}
                    </div>
                  </li>
                );
              })}
            </ul>
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
                  data-setting-key={`bot.${bot.id || initial.id}.schedule-cron`}
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
                  data-setting-key={`bot.${bot.id || initial.id}.schedule-interval`}
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

          {/* v2.8.0 — Always-on Daemon. The BotEditor
              surfaces the per-Bot bearer token and the
              public webhook URL. New bots have no
              token until the user clicks "Generate".
              Plain HTTP in v2.8 (TLS is v3.0); the
              bearer token is the only auth. */}
          {!isNew && (
            <section className="form-section" data-testid="bot-editor-daemon">
              <h3>Daemon</h3>
              <div className="form-row">
                <label>Webhook URL</label>
                <input
                  type="text"
                  readOnly
                  data-testid="daemon-webhook-url"
                  data-setting-key={`bot.${bot.id || initial.id}.daemon-webhook`}
                  value={
                    daemonServerHost
                      ? `https://${daemonServerHost}:8443/hooks/${bot.id}`
                      : `<set computer_server_host in Settings>:8443/hooks/${bot.id}`
                  }
                  style={{ fontFamily: "var(--font-mono)", width: "100%" }}
                  onFocus={(e) => e.currentTarget.select()}
                />
                <div className="muted small">
                  POST a JSON body to this URL with header{" "}
                  <code>Authorization: Bearer &lt;token&gt;</code>. v2.8
                  is plain HTTP; TLS is v3.0.
                </div>
              </div>
              <div className="form-row">
                <label>Bearer token</label>
                <div
                  className="daemon-token-row"
                  data-testid="daemon-token-row"
                >
                  <input
                    type="text"
                    readOnly
                    data-testid="daemon-token-value"
                    data-setting-key={`bot.${bot.id || initial.id}.daemon-token`}
                    value={daemonToken ?? "(not set)"}
                    style={{ fontFamily: "var(--font-mono)", flex: 1 }}
                    onFocus={(e) => e.currentTarget.select()}
                  />
                  <button
                    type="button"
                    data-testid="daemon-token-rotate"
                    data-setting-key={`bot.${bot.id || initial.id}.daemon-token-rotate`}
                    onClick={async () => {
                      const tok = await rotateDaemonToken(bot.id);
                      setDaemonToken(tok);
                    }}
                  >
                    {daemonToken ? "Rotate" : "Generate"}
                  </button>
                  <button
                    type="button"
                    data-testid="daemon-token-copy"
                    data-setting-key={`bot.${bot.id || initial.id}.daemon-token-copy`}
                    disabled={!daemonToken}
                    onClick={() => {
                      if (daemonToken) {
                        navigator.clipboard.writeText(daemonToken);
                      }
                    }}
                  >
                    Copy
                  </button>
                </div>
                <div className="muted small">
                  Rotate invalidates the old token immediately. Anything
                  still using the old token will start receiving 401s.
                </div>
              </div>
              {/* v3.1.0 — Test webhook. POSTs a tiny synthetic
                  payload to the configured webhook URL with the
                  current bearer token. The daemon returns 202
                  with a `bot_run_id`; the run lands in Activity
                  within a few seconds. The button is disabled
                  until both the token and the server host are
                  configured. CORS is the renderer's problem
                  (the daemon is plain HTTP, no ACAO header) —
                  the most common failure is a CORS error from
                  the webview, which the toast surfaces verbatim. */}
              <div className="form-row">
                <label>Test webhook</label>
                <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
                  <button
                    type="button"
                    data-testid="daemon-test-webhook"
                    data-setting-key={`bot.${bot.id || initial.id}.daemon-test-webhook`}
                    disabled={!daemonToken || !daemonServerHost || testStatus.kind === "running"}
                    onClick={async () => {
                      setTestStatus({ kind: "running" });
                      try {
                        const tok = await getDaemonToken(bot.id);
                        if (!tok) {
                          setTestStatus({ kind: "error", message: "no token — click Generate first" });
                          return;
                        }
                        const url = `https://${daemonServerHost}:8443/hooks/${bot.id}`;
                        const body = JSON.stringify({
                          text: `webhook test from MaxBot at ${new Date().toISOString()}`,
                        });
                        const resp = await fetch(url, {
                          method: "POST",
                          headers: {
                            "Authorization": `Bearer ${tok}`,
                            "Content-Type": "application/json",
                          },
                          body,
                        });
                        const text = await resp.text();
                        if (resp.status === 202) {
                          setTestStatus({ kind: "ok", message: `${resp.status} — Bot run started (${text.slice(0, 80)})` });
                        } else {
                          setTestStatus({ kind: "error", message: `${resp.status} — ${text.slice(0, 200)}` });
                        }
                      } catch (e) {
                        setTestStatus({ kind: "error", message: `request failed: ${String(e)}` });
                      }
                    }}
                  >
                    {testStatus.kind === "running" ? "Sending…" : "Test webhook"}
                  </button>
                  {testStatus.kind === "ok" && (
                    <span
                      className="muted small"
                      data-testid="daemon-test-webhook-status"
                      data-status="ok"
                      style={{ color: "rgb(134, 239, 172)" }}
                    >
                      {testStatus.message}
                    </span>
                  )}
                  {testStatus.kind === "error" && (
                    <span
                      className="muted small"
                      data-testid="daemon-test-webhook-status"
                      data-status="error"
                      style={{ color: "rgb(252, 165, 165)" }}
                    >
                      {testStatus.message}
                    </span>
                  )}
                </div>
                <div className="muted small">
                  Sends a synthetic message to{" "}
                  <code>POST /hooks/{bot.id}</code>. Expect a 202.
                  The run shows up in Activity within ~5s with a
                  "via webhook" badge.
                </div>
              </div>
            </section>
          )}
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
