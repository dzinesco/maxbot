// v2.3.0 — Routines: the editor dialog for binding a
// Skill to a bot's schedule. Lets the user pick:
//
//  1. A Skill (or "no skill" — fires the bot's chat loop)
//  2. A trigger — either an interval (every N seconds) or
//     a 5-field cron expression, with a 4-preset menu
//     and an "Advanced" text input.
//
// The dialog is mounted by `RoutinesPanel` and by the
// Routines "New routine" affordance. The parent owns the
// saved BotSchedule; the dialog is purely an editor.

import { useEffect, useMemo, useState } from "react";
import { listSkills, upsertBotSchedule } from "../lib/tauri";
import type { Bot, BotSchedule, Skill } from "../lib/api";

interface ScheduleEditorDialogProps {
  /** The bot this routine is bound to. */
  bot: Bot;
  /** Existing schedule for this bot, or null for a new
   *  routine. */
  schedule: BotSchedule | null;
  /** Called after a successful save. */
  onSaved: (s: BotSchedule) => void;
  /** Called when the user dismisses without saving. */
  onCancel: () => void;
}

const PRESETS: { label: string; expr: string; description: string }[] = [
  {
    label: "Every weekday at 9 AM",
    expr: "0 9 * * 1-5",
    description: "Next fire: weekday 9:00 AM (local)",
  },
  {
    label: "Every hour, on the hour",
    expr: "0 * * * *",
    description: "Next fire: top of the hour (local)",
  },
  {
    label: "Every day at 8 AM",
    expr: "0 8 * * *",
    description: "Next fire: 8:00 AM (local)",
  },
  {
    label: "Every Monday at 9 AM",
    expr: "0 9 * * 1",
    description: "Next fire: Monday 9:00 AM (local)",
  },
];

const ADVANCED = "__advanced__";

function describeCron(expr: string): string {
  // Render a one-line plain-English preview of the
  // 5-field cron. We do the rendering inline for the 4
  // presets and a handful of common shapes; everything
  // else gets a "next fire: <expr> (local)" fallback so
  // the field is never empty when the cron is valid.
  const trimmed = expr.trim();
  if (trimmed === "") return "—";
  const preset = PRESETS.find((p) => p.expr === trimmed);
  if (preset) return preset.description;
  if (trimmed === ADVANCED) return "Type a 5-field crontab below";
  // Crude inline parser. We avoid the `cron-parser`
  // dependency — only the 4 presets and a couple of
  // common shapes are rendered, and the user always has
  // the raw `expr` to fall back on.
  const parts = trimmed.split(/\s+/);
  if (parts.length === 5) {
    const [m, h, dom, mon, dow] = parts;
    if (m === "0" && h === "*" && dom === "*" && mon === "*" && dow === "*") {
      return "Every hour, on the hour";
    }
    if (m === "*/15" && h === "*" && dom === "*" && mon === "*" && dow === "*") {
      return "Every 15 minutes";
    }
    if (m === "*/30" && h === "*" && dom === "*" && mon === "*" && dow === "*") {
      return "Every 30 minutes";
    }
    if (m === "0" && /^\d+$/.test(h) && dom === "*" && mon === "*" && dow === "*") {
      return `Every day at ${h}:00`;
    }
    if (m === "0" && /^\d+$/.test(h) && dom === "*" && mon === "*" && /^1-5$/.test(dow)) {
      return `Weekdays at ${h}:00`;
    }
  }
  return `Custom: ${trimmed}`;
}

export function ScheduleEditorDialog(props: ScheduleEditorDialogProps) {
  const { bot, schedule, onSaved, onCancel } = props;
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loadingSkills, setLoadingSkills] = useState(true);
  const [skillId, setSkillId] = useState<string>(
    schedule?.skill_id ?? "",
  );
  const [mode, setMode] = useState<"interval" | "cron">(
    schedule?.cron_expression ? "cron" : "interval",
  );
  const [intervalMinutes, setIntervalMinutes] = useState<number>(
    Math.max(1, Math.round((schedule?.interval_seconds ?? 60) / 60)),
  );
  const [cronPreset, setCronPreset] = useState<string>(() => {
    const cur = schedule?.cron_expression ?? "";
    const match = PRESETS.find((p) => p.expr === cur);
    return match ? match.expr : cur ? ADVANCED : PRESETS[0].expr;
  });
  const [cronAdvanced, setCronAdvanced] = useState<string>(
    schedule?.cron_expression && !PRESETS.some((p) => p.expr === schedule?.cron_expression)
      ? schedule.cron_expression
      : "",
  );
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const list = await listSkills();
        if (!cancelled) setSkills(list);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoadingSkills(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const cronExpr = useMemo(() => {
    if (cronPreset === ADVANCED) return cronAdvanced.trim();
    return cronPreset;
  }, [cronPreset, cronAdvanced]);

  const cronDescription = useMemo(() => describeCron(cronExpr), [cronExpr]);

  async function handleSave() {
    setError(null);
    const interval_seconds = mode === "interval" ? intervalMinutes * 60 : 0;
    const cron_expression = mode === "cron" ? cronExpr : "";
    if (mode === "interval" && interval_seconds <= 0) {
      setError("Interval must be at least 1 minute");
      return;
    }
    if (mode === "cron" && !cron_expression) {
      setError("Pick a cron preset or type a 5-field crontab");
      return;
    }
    setSaving(true);
    try {
      const next: BotSchedule = {
        bot_id: bot.id,
        interval_seconds,
        cron_expression,
        last_run_at: schedule?.last_run_at ?? null,
        last_conversation_id: schedule?.last_conversation_id ?? null,
        skill_id: skillId || null,
      };
      const saved = await upsertBotSchedule(next);
      onSaved(saved);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div
      className="modal-overlay"
      role="dialog"
      aria-modal="true"
      aria-labelledby="schedule-editor-title"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="modal-stacked modal-stacked-wide schedule-editor-dialog">
        <div className="modal-stacked-header">
          <h2 id="schedule-editor-title">
            {schedule ? "Edit routine" : "New routine"} — {bot.name}
          </h2>
        </div>
        <div className="modal-stacked-body schedule-editor-dialog__body">
          {error && <div className="modal-stacked-error">{error}</div>}

          <label className="field-group">
            <span>Skill</span>
            <select
              aria-label="Skill to bind"
              value={skillId}
              onChange={(e) => setSkillId(e.target.value)}
              disabled={loadingSkills}
            >
              <option value="">
                (no skill — fire the bot's chat loop)
              </option>
              {skills.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.name} · {s.steps.length} step
                  {s.steps.length === 1 ? "" : "s"}
                </option>
              ))}
            </select>
            <span className="schedule-editor-dialog__hint">
              When a Skill is bound, the scheduler runs the
              Skill's steps on every fire instead of
              invoking the bot's chat loop.
            </span>
          </label>

          <label className="field-group">
            <span>Trigger</span>
            <div className="schedule-editor-dialog__trigger-row">
              <label className="schedule-editor-dialog__radio">
                <input
                  type="radio"
                  name="trigger-mode"
                  value="interval"
                  checked={mode === "interval"}
                  onChange={() => setMode("interval")}
                />
                Interval (every N minutes)
              </label>
              <label className="schedule-editor-dialog__radio">
                <input
                  type="radio"
                  name="trigger-mode"
                  value="cron"
                  checked={mode === "cron"}
                  onChange={() => setMode("cron")}
                />
                Cron (advanced)
              </label>
            </div>
          </label>

          {mode === "interval" ? (
            <label className="field-group">
              <span>Every</span>
              <input
                type="number"
                min={1}
                value={intervalMinutes}
                onChange={(e) =>
                  setIntervalMinutes(Math.max(1, Number(e.target.value) || 1))
                }
              />
              <span>minutes</span>
            </label>
          ) : (
            <>
              <label className="field-group">
                <span>Preset</span>
                <select
                  aria-label="Cron preset"
                  value={cronPreset}
                  onChange={(e) => setCronPreset(e.target.value)}
                >
                  {PRESETS.map((p) => (
                    <option key={p.expr} value={p.expr}>
                      {p.label}
                    </option>
                  ))}
                  <option value={ADVANCED}>Advanced…</option>
                </select>
              </label>
              {cronPreset === ADVANCED && (
                <label className="field-group">
                  <span>5-field crontab</span>
                  <input
                    type="text"
                    value={cronAdvanced}
                    onChange={(e) => setCronAdvanced(e.target.value)}
                    placeholder="0 9 * * 1-5"
                    spellCheck={false}
                  />
                </label>
              )}
              <div className="schedule-editor-dialog__preview" data-testid="cron-preview">
                {cronDescription}
              </div>
            </>
          )}
        </div>
        <div className="modal-stacked-footer">
          <button
            className="secondary-button"
            onClick={onCancel}
            disabled={saving}
          >
            Cancel
          </button>
          <button
            className="primary-button"
            onClick={handleSave}
            disabled={saving}
            data-testid="schedule-save"
          >
            {saving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
    </div>
  );
}
