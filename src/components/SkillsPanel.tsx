// v2.2.0 — SkillsPanel.
//
// Renders the Skills tab in the side panel. Lists saved
// Skills, exposes "Run" / "Record" / "Import" / "Delete"
// buttons, and shows the run history under each Skill.
//
// The "Run" button opens a small arg-form dialog built
// from the Skill's `inputs` schema. The "Record" button
// opens `RecordSkillDialog` (a separate component) which
// drives the recording flow.
//
// The panel does NOT own the dialog state for either
// flow — the parent (`App.tsx`) opens the dialogs and
// passes them in as props. This keeps the panel purely
// about listing and the dialogs purely about their
// respective flows.

import { useCallback, useEffect, useRef, useState } from "react";
import {
  createSkill,
  deleteSkill,
  listSkills,
  runSkill,
  skillRunHistory,
} from "../lib/tauri";
import type { Bot, Skill, SkillInput, SkillRun } from "../lib/api";

interface SkillsPanelProps {
  bots: Bot[];
  /** The currently-active bot, used as the default for "Run" / "Record". */
  defaultBotId: string | null;
  onOpenRecord: () => void;
}

export function SkillsPanel(props: SkillsPanelProps) {
  const { bots, defaultBotId, onOpenRecord } = props;
  const [skills, setSkills] = useState<Skill[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  // Per-Skill run history. Keyed by skill id.
  const [history, setHistory] = useState<Record<string, SkillRun[]>>({});
  // "Run" dialog state — which Skill is being run, plus
  // the current arg values from the form.
  const [running, setRunning] = useState<{
    skill: Skill;
    botId: string;
    argValues: Record<string, string>;
    inFlight: boolean;
    lastRun?: SkillRun;
  } | null>(null);
  // The import-file input is a hidden <input type="file">
  // we trigger programmatically (so the click handler can
  // sit in the toolbar).
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await listSkills();
      setSkills(list);
      // Refresh history in the background — best-effort,
      // don't fail the whole panel if a single lookup
      // fails.
      const h: Record<string, SkillRun[]> = {};
      await Promise.all(
        list.map(async (s) => {
          try {
            h[s.id] = await skillRunHistory(s.id, 5);
          } catch (e) {
            console.warn("skill_run_history failed for", s.id, e);
            h[s.id] = [];
          }
        }),
      );
      setHistory(h);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const handleRunClick = useCallback(
    (skill: Skill) => {
      const botId =
        defaultBotId ??
        (bots[0] ? bots[0].id : null);
      if (!botId) {
        setError("no bot available — create a Bot first");
        return;
      }
      // Seed arg values with the Skill's defaults.
      const seed: Record<string, string> = {};
      for (const input of skill.inputs) {
        if (input.default) seed[input.name] = input.default;
      }
      setRunning({
        skill,
        botId,
        argValues: seed,
        inFlight: false,
      });
    },
    [defaultBotId, bots],
  );

  const handleRunSubmit = useCallback(async () => {
    if (!running) return;
    setRunning({ ...running, inFlight: true });
    try {
      const result = await runSkill(
        running.botId,
        running.skill.id,
        running.argValues,
      );
      setRunning({ ...running, inFlight: false, lastRun: result });
      // Refresh the per-skill history so the panel shows
      // the new run.
      const h = await skillRunHistory(running.skill.id, 5);
      setHistory((prev) => ({ ...prev, [running.skill.id]: h }));
    } catch (e) {
      setRunning({
        ...running,
        inFlight: false,
        lastRun: {
          id: "",
          skill_id: running.skill.id,
          bot_id: running.botId,
          inputs: running.argValues,
          status: "failed",
          started_at: new Date().toISOString(),
          finished_at: new Date().toISOString(),
          result_summary: String(e),
          steps: [],
        },
      });
    }
  }, [running]);

  const handleDelete = useCallback(
    async (skill: Skill) => {
      // Cheap confirm — destructive and irreversible (cascades
      // to skill_runs). The dialog API isn't worth the
      // import for a single yes/no.
      if (!window.confirm(`Delete Skill "${skill.name}"? This will also remove its run history.`)) {
        return;
      }
      try {
        await deleteSkill(skill.id);
        await refresh();
      } catch (e) {
        setError(String(e));
      }
    },
    [refresh],
  );

  const handleImport = useCallback(async () => {
    fileInputRef.current?.click();
  }, []);

  const handleFileChosen = useCallback(
    async (e: React.ChangeEvent<HTMLInputElement>) => {
      const file = e.target.files?.[0];
      e.target.value = ""; // allow re-importing the same file
      if (!file) return;
      try {
        const text = await file.text();
        const parsed = JSON.parse(text);
        if (!parsed.name) {
          setError("Imported file has no `name` field.");
          return;
        }
        // Create a fresh Skill and let the Rust side assign
        // an id and timestamps.
        const skill: Skill = {
          id: "",
          name: parsed.name,
          description: parsed.description ?? "",
          inputs: parsed.inputs ?? [],
          steps: parsed.steps ?? [],
          created_at: new Date().toISOString(),
          updated_at: new Date().toISOString(),
        };
        await createSkill(skill);
        await refresh();
      } catch (e) {
        setError(`Import failed: ${e}`);
      }
    },
    [refresh],
  );

  return (
    <div className="skills-panel" data-setting-key="panel.skills">
      <div className="skills-panel__header">
        <h3>Skills</h3>
        <div className="skills-panel__actions">
          <button
            className="ghost"
            onClick={handleImport}
            title="Import a .skill.json file"
          >
            Import
          </button>
          <button
            className="ghost"
            onClick={onOpenRecord}
            disabled={bots.length === 0}
            title={
              bots.length === 0
                ? "Create a Bot first to record a Skill"
                : "Record a Skill from a live Bot run"
            }
          >
            Record
          </button>
        </div>
        <input
          ref={fileInputRef}
          type="file"
          accept="application/json,.skill.json,.json"
          style={{ display: "none" }}
          onChange={handleFileChosen}
        />
      </div>

      {error && (
        <div className="skills-panel__error" role="alert">
          {error}
        </div>
      )}

      {loading && skills.length === 0 && (
        <div className="skills-panel__empty">Loading…</div>
      )}

      {!loading && skills.length === 0 && (
        <div className="skills-panel__empty">
          No Skills yet. Click <b>Record</b> to capture a procedure from
          a live Bot run, or <b>Import</b> a <code>.skill.json</code> file.
        </div>
      )}

      <ul className="skills-panel__list">
        {skills.map((skill) => (
          <li key={skill.id} className="skills-panel__item">
            <div className="skills-panel__item-header">
              <div className="skills-panel__item-title">
                <span className="skills-panel__name">{skill.name || "(untitled)"}</span>
                <span className="skills-panel__count">
                  {skill.steps.length} step{skill.steps.length === 1 ? "" : "s"}
                </span>
              </div>
              <div className="skills-panel__item-actions">
                <button
                  className="primary"
                  onClick={() => handleRunClick(skill)}
                  disabled={bots.length === 0}
                >
                  Run
                </button>
                <button className="ghost" onClick={() => handleDelete(skill)}>
                  Delete
                </button>
              </div>
            </div>
            {skill.description && (
              <div className="skills-panel__description">{skill.description}</div>
            )}
            <div className="skills-panel__steps">
              {skill.steps.slice(0, 4).map((s, i) => (
                <span key={i} className="skills-panel__chip">
                  {s.tool}
                </span>
              ))}
              {skill.steps.length > 4 && (
                <span className="skills-panel__chip skills-panel__chip--more">
                  +{skill.steps.length - 4} more
                </span>
              )}
            </div>
            {(history[skill.id] ?? []).length > 0 && (
              <ul className="skills-panel__history">
                {(history[skill.id] ?? []).map((r) => (
                  <li key={r.id} className={`skills-panel__history-item skills-panel__history-item--${r.status}`}>
                    <span className="skills-panel__history-status">{r.status}</span>
                    <span className="skills-panel__history-time">
                      {new Date(r.started_at).toLocaleString()}
                    </span>
                    {r.result_summary && (
                      <span className="skills-panel__history-summary">{r.result_summary}</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </li>
        ))}
      </ul>

      {running && (
        <RunSkillDialog
          skill={running.skill}
          bots={bots}
          defaultBotId={running.botId}
          argValues={running.argValues}
          setArgValues={(name, value) =>
            setRunning({
              ...running,
              argValues: { ...running.argValues, [name]: value },
            })
          }
          inFlight={running.inFlight}
          lastRun={running.lastRun}
          onClose={() => setRunning(null)}
          onSubmit={handleRunSubmit}
        />
      )}
    </div>
  );
}

interface RunSkillDialogProps {
  skill: Skill;
  bots: Bot[];
  defaultBotId: string;
  argValues: Record<string, string>;
  setArgValues: (name: string, value: string) => void;
  inFlight: boolean;
  lastRun?: SkillRun;
  onClose: () => void;
  onSubmit: () => void;
}

function RunSkillDialog(props: RunSkillDialogProps) {
  const {
    skill,
    bots,
    defaultBotId,
    argValues,
    setArgValues,
    inFlight,
    lastRun,
    onClose,
    onSubmit,
  } = props;
  const [botId, setBotId] = useState(defaultBotId);

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-stacked skills-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-stacked-header">
          <h2>Run: {skill.name || "(untitled)"}</h2>
        </div>
        <div className="modal-stacked-body">
          {skill.description && (
            <p className="skills-dialog__description">{skill.description}</p>
          )}
          <label className="field-group">
            <span>Bot</span>
            <select
              value={botId}
              onChange={(e) => setBotId(e.target.value)}
            >
              {bots.map((b) => (
                <option key={b.id} value={b.id}>
                  {b.name}
                </option>
              ))}
            </select>
          </label>
          {skill.inputs.length === 0 ? (
            <p className="skills-dialog__no-inputs">
              This Skill has no inputs. It will run against {bots.find((b) => b.id === botId)?.name}.
            </p>
          ) : (
            skill.inputs.map((input: SkillInput) => (
              <label key={input.name} className="field-group">
                <span>{input.name}{input.default ? <em> (default: {input.default})</em> : null}</span>
                {input.kind === "choice" && input.choices ? (
                  <select
                    value={argValues[input.name] ?? input.default ?? ""}
                    onChange={(e) => setArgValues(input.name, e.target.value)}
                  >
                    {input.choices.map((c) => (
                      <option key={c} value={c}>
                        {c}
                      </option>
                    ))}
                  </select>
                ) : (
                  <input
                    type={input.kind === "number" ? "number" : "text"}
                    value={argValues[input.name] ?? ""}
                    onChange={(e) => setArgValues(input.name, e.target.value)}
                  />
                )}
              </label>
            ))
          )}
          {lastRun && (
            <div className={`skills-dialog__result skills-dialog__result--${lastRun.status}`}>
              <b>Status:</b> {lastRun.status}
              <br />
              <b>Result:</b> {lastRun.result_summary || "(no summary)"}
            </div>
          )}
        </div>
        <div className="modal-stacked-footer">
          <button className="ghost" onClick={onClose} disabled={inFlight}>
            Close
          </button>
          <button className="primary" onClick={onSubmit} disabled={inFlight}>
            {inFlight ? "Running…" : "Run"}
          </button>
        </div>
      </div>
    </div>
  );
}
