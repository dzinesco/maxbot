// v2.3.0 — Routines: the Routines tab in the side panel.
// Lists routines (one per bot) grouped by bot, and offers
// New / Edit / Delete affordances. The "New routine"
// affordance first asks the user to pick a bot, then
// opens the ScheduleEditorDialog for that bot.

import { useCallback, useEffect, useMemo, useState } from "react";
import {
  listAllSchedules,
  listBots,
  listSkills,
  upsertBotSchedule,
} from "../lib/tauri";
import type { Bot, BotSchedule, Skill } from "../lib/api";
import { ScheduleEditorDialog } from "./ScheduleEditorDialog";

interface RoutinesPanelProps {
  /** Active bot id, used to highlight the row in the
   *  routines list. Optional. */
  activeBotId?: string | null;
  /** Called when the user clicks a routine's bot name —
   *  lets the parent App switch the main view to the
   *  chat panel. */
  onOpenBot?: (botId: string) => void;
}

function describeSchedule(s: BotSchedule): string {
  if (s.cron_expression) {
    if (s.cron_expression === "0 9 * * 1-5") return "every weekday at 9 AM";
    if (s.cron_expression === "0 * * * *") return "every hour, on the hour";
    if (s.cron_expression === "0 8 * * *") return "every day at 8 AM";
    if (s.cron_expression === "0 9 * * 1") return "every Monday at 9 AM";
    return `cron \`${s.cron_expression}\``;
  }
  if (s.interval_seconds > 0) {
    if (s.interval_seconds < 60) return `every ${s.interval_seconds}s`;
    const m = Math.round(s.interval_seconds / 60);
    if (m < 60) return `every ${m} min`;
    const h = Math.round(m / 60);
    return `every ${h}h`;
  }
  return "(disabled)";
}

export function RoutinesPanel(props: RoutinesPanelProps) {
  const { activeBotId, onOpenBot } = props;
  const [schedules, setSchedules] = useState<BotSchedule[]>([]);
  const [bots, setBots] = useState<Bot[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState<
    | { mode: "new" | "edit"; bot: Bot; schedule: BotSchedule | null }
    | null
  >(null);
  const [pickingBot, setPickingBot] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [b, s, sk] = await Promise.all([
        listBots(),
        listAllSchedules(),
        listSkills(),
      ]);
      setBots(b);
      setSchedules(s);
      setSkills(sk);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const skillById = useMemo(() => {
    const m = new Map<string, Skill>();
    for (const s of skills) m.set(s.id, s);
    return m;
  }, [skills]);

  const botById = useMemo(() => {
    const m = new Map<string, Bot>();
    for (const b of bots) m.set(b.id, b);
    return m;
  }, [bots]);

  // Group routines by bot. The "no routine" case is the
  // empty state — handled below.
  const routinesByBot = useMemo(() => {
    const groups = new Map<string, BotSchedule>();
    for (const s of schedules) groups.set(s.bot_id, s);
    return groups;
  }, [schedules]);

  async function handleDelete(schedule: BotSchedule) {
    if (!confirm("Delete this routine? The bot will stop firing on a schedule.")) {
      return;
    }
    try {
      await upsertBotSchedule({
        ...schedule,
        interval_seconds: 0,
        cron_expression: "",
        skill_id: null,
      });
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  if (loading) {
    return (
      <div className="routines-panel">
        <div className="routines-panel__loading">Loading routines…</div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="routines-panel">
        <div className="routines-panel__error">{error}</div>
      </div>
    );
  }

  if (schedules.length === 0) {
    return (
      <div className="routines-panel">
        <div className="routines-panel__header">
          <h3>Routines</h3>
          <div className="routines-panel__actions">
            <button
              className="primary-button"
              onClick={() => setPickingBot(true)}
              data-testid="new-routine"
            >
              + New routine
            </button>
          </div>
        </div>
        <div className="routines-panel__empty" data-testid="routines-empty">
          No routines yet — schedule a bot to run on a cadence, or bind a
          Skill to a schedule.
        </div>
        {pickingBot && (
          <BotPickerDialog
            bots={bots}
            onPick={(b) => {
              setPickingBot(false);
              setEditing({ mode: "new", bot: b, schedule: null });
            }}
            onCancel={() => setPickingBot(false)}
          />
        )}
        {editing && (
          <ScheduleEditorDialog
            bot={editing.bot}
            schedule={editing.schedule}
            onSaved={() => {
              setEditing(null);
              refresh();
            }}
            onCancel={() => setEditing(null)}
          />
        )}
      </div>
    );
  }

  return (
    <div className="routines-panel">
      <div className="routines-panel__header">
        <h3>Routines</h3>
        <div className="routines-panel__actions">
          <button
            className="primary-button"
            onClick={() => setPickingBot(true)}
            data-testid="new-routine"
          >
            + New routine
          </button>
        </div>
      </div>
      <div className="routines-panel__list" data-testid="routines-list">
        {schedules.map((s) => {
          const bot = botById.get(s.bot_id);
          if (!bot) return null;
          const skill = s.skill_id ? skillById.get(s.skill_id) : null;
          const isActive = s.bot_id === activeBotId;
          return (
            <div
              key={s.bot_id}
              className={
                "routine-row" + (isActive ? " routine-row--active" : "")
              }
              data-testid="routine-row"
            >
              <div className="routine-row__main">
                <button
                  className="routine-row__bot"
                  onClick={() => onOpenBot?.(s.bot_id)}
                >
                  {bot.name}
                </button>
                <span className="routine-row__action">
                  {skill
                    ? `Run ${skill.name} · ${skill.steps.length} step${
                        skill.steps.length === 1 ? "" : "s"
                      }`
                    : "Run bot chat loop"}
                </span>
                <span className="routine-row__when">
                  {describeSchedule(s)}
                </span>
              </div>
              <div className="routine-row__actions">
                <button
                  className="secondary-button"
                  onClick={() =>
                    setEditing({ mode: "edit", bot, schedule: s })
                  }
                  data-testid="routine-edit"
                >
                  Edit
                </button>
                <button
                  className="secondary-button routine-row__delete"
                  onClick={() => handleDelete(s)}
                  data-testid="routine-delete"
                >
                  Delete
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {pickingBot && (
        <BotPickerDialog
          bots={bots}
          onPick={(b) => {
            setPickingBot(false);
            setEditing({ mode: "new", bot: b, schedule: null });
          }}
          onCancel={() => setPickingBot(false)}
        />
      )}
      {editing && (
        <ScheduleEditorDialog
          bot={editing.bot}
          schedule={editing.schedule}
          onSaved={() => {
            setEditing(null);
            refresh();
          }}
          onCancel={() => setEditing(null)}
        />
      )}
    </div>
  );
}

interface BotPickerDialogProps {
  bots: Bot[];
  onPick: (b: Bot) => void;
  onCancel: () => void;
}

function BotPickerDialog(props: BotPickerDialogProps) {
  const { bots, onPick, onCancel } = props;
  const [pickedId, setPickedId] = useState<string>(bots[0]?.id ?? "");
  return (
    <div
      className="modal-overlay"
      role="dialog"
      aria-modal="true"
      onClick={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div className="modal-stacked">
        <div className="modal-stacked-header">
          <h2>Pick a bot for the new routine</h2>
        </div>
        <div className="modal-stacked-body">
          <label className="field-group">
            <span>Bot</span>
            <select
              value={pickedId}
              onChange={(e) => setPickedId(e.target.value)}
            >
              {bots.map((b) => (
                <option key={b.id} value={b.id}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div className="modal-stacked-footer">
          <button className="secondary-button" onClick={onCancel}>
            Cancel
          </button>
          <button
            className="primary-button"
            disabled={!pickedId}
            onClick={() => {
              const b = bots.find((x) => x.id === pickedId);
              if (b) onPick(b);
            }}
          >
            Continue
          </button>
        </div>
      </div>
    </div>
  );
}
