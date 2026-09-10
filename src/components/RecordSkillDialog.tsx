// v2.2.0 — RecordSkillDialog.
//
// Three flows:
//
// 1. **Record** (original flow): pick a Bot, run it, capture
//    the tool calls, save as a new Skill.
// 2. **Re-record** (v3.3.0): preload the dialog with an
//    existing Skill's JSON so the user can edit and re-save
//    in place. The save path calls `skill_update` (not
//    `skill_create`) so the Skill's id and the bot_schedule
//    pointing at it are preserved. The dialog title flips
//    to "Re-record: <name>" and the "Save" button reads
//    "Update".
// 3. **Import-style edit** (v3.3.0): preload a Skill from
//    a parent component via the `existingSkill` prop. Same
//    save path as Re-record.
//
// The phases are shared: `configure` (recording-only) →
// `editing` (save-or-update). The re-record path skips
// `configure` and lands directly in `editing`.

import { useCallback, useEffect, useState } from "react";
import {
  createSkill,
  skillRecordStart,
  skillRecordStop,
  updateSkill,
} from "../lib/tauri";
import type { Bot, Skill, SkillStep } from "../lib/api";

interface RecordSkillDialogProps {
  bots: Bot[];
  defaultBotId: string | null;
  onClose: () => void;
  onSaved: (skill: Skill) => void;
  /** v3.3.0 — when set, opens the dialog in
   *  "re-record" mode: the skill's existing JSON is
   *  pre-filled in the editor and the save path calls
   *  `skill_update` (preserves id + bot_schedule
   *  binding). The dialog title becomes "Re-record:
   *  <name>". */
  existingSkill?: Skill | null;
}

type Phase =
  | { kind: "configure" }
  | { kind: "recording"; recordingId: string; botId: string }
  | { kind: "editing"; recordingId: string | null; candidate: Skill };

export function RecordSkillDialog(props: RecordSkillDialogProps) {
  const { bots, defaultBotId, onClose, onSaved, existingSkill } = props;
  const isReRecord = Boolean(existingSkill);
  // In re-record mode, jump straight to `editing` with
  // the existing skill as the candidate. In record mode,
  // start at `configure`.
  const [phase, setPhase] = useState<Phase>(
    existingSkill
      ? {
          kind: "editing",
          recordingId: null,
          candidate: existingSkill,
        }
      : { kind: "configure" },
  );
  const [botId, setBotId] = useState(
    defaultBotId ?? (bots[0] ? bots[0].id : ""),
  );
  const [name, setName] = useState(
    existingSkill?.name ?? "",
  );
  const [description, setDescription] = useState(
    existingSkill?.description ?? "",
  );
  const initialStepsJson = existingSkill
    ? JSON.stringify(existingSkill.steps, null, 2)
    : JSON.stringify([{ tool: "example_tool", args: {} }], null, 2);
  const [stepsJson, setStepsJson] = useState<string>(initialStepsJson);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  // Reset the kickoff form when we move into recording —
  // the user has clicked Start and we don't want a stale
  // textarea hanging around in the dialog.
  useEffect(() => {
    if (phase.kind === "recording") {
      setError(null);
    }
  }, [phase]);

  const handleStart = useCallback(async () => {
    if (!botId) {
      setError("pick a bot to record");
      return;
    }
    setError(null);
    try {
      const start = await skillRecordStart(botId);
      setPhase({ kind: "recording", recordingId: start.recording_id, botId });
    } catch (e) {
      setError(String(e));
    }
  }, [botId]);

  const handleStop = useCallback(async () => {
    if (phase.kind !== "recording") return;
    setError(null);
    try {
      const stop = await skillRecordStop(phase.recordingId);
      if (!stop) {
        setError("recording session not found (already stopped?)");
        return;
      }
      // Seed the editor with empty name/description and
      // the JSON-prettified steps from the recording.
      const candidate: Skill = {
        ...stop.skill,
        name: name || "Recorded Skill",
        description: description || "",
      };
      setStepsJson(JSON.stringify(candidate.steps, null, 2));
      setPhase({
        kind: "editing",
        recordingId: phase.recordingId,
        candidate,
      });
    } catch (e) {
      setError(String(e));
    }
  }, [phase, name, description]);

  const handleAbort = useCallback(() => {
    // We just close — the recorder's session is dropped
    // when the Bot run finishes. (v2.2 has no explicit
    // "abort recording" Rust command; the recorded data
    // sits in the recorder's HashMap until the next
    // session replaces it or the process restarts.)
    onClose();
  }, [onClose]);

  const handleSave = useCallback(async () => {
    if (phase.kind !== "editing") return;
    setSaving(true);
    setError(null);
    try {
      const steps: SkillStep[] = JSON.parse(stepsJson);
      if (!Array.isArray(steps)) {
        throw new Error("`steps` must be a JSON array");
      }
      for (const s of steps) {
        if (!s.tool || typeof s.tool !== "string") {
          throw new Error("every step needs a string tool field");
        }
        if (!s.args || typeof s.args !== "object") {
          throw new Error(`step ${s.tool} needs an args object`);
        }
      }
      if (isReRecord) {
        // Re-record path: preserve id + created_at, overwrite
        // the editable fields. The DB layer's `update_skill_in_place`
        // bumps `updated_at` for us.
        const to_update: Skill = {
          ...phase.candidate,
          name: name.trim() || phase.candidate.name || "Recorded Skill",
          description: description.trim(),
          steps,
        };
        const updated = await updateSkill(to_update.id, to_update);
        onSaved(updated);
        return;
      }
      // Original record path: fresh Skill, id is generated
      // by the Rust side.
      const to_save: Skill = {
        ...phase.candidate,
        name: name.trim() || "Recorded Skill",
        description: description.trim(),
        steps,
      };
      const saved = await createSkill(to_save);
      onSaved(saved);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }, [phase, stepsJson, name, description, onSaved, isReRecord]);

  // The dialog title flips between "Record a Skill",
  // "Recording…", "Save Skill", and (v3.3.0) "Re-record: <name>".
  const dialogTitle = (() => {
    if (phase.kind === "configure") return "Record a Skill";
    if (phase.kind === "recording") return "Recording…";
    if (isReRecord) {
      const display = name || phase.candidate.name || "(untitled)";
      return `Re-record: ${display}`;
    }
    return "Save Skill";
  })();

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-stacked skills-dialog"
        onClick={(e) => e.stopPropagation()}
        data-testid="record-skill-dialog"
        data-mode={isReRecord ? "re-record" : "record"}
      >
        <div className="modal-stacked-header">
          <h2>{dialogTitle}</h2>
        </div>
        <div className="modal-stacked-body">
          {phase.kind === "configure" && (
            <>
              <p className="skills-dialog__description">
                Pick a Bot. After you click <b>Start</b>, the Bot
                will run in the background. Do the task in the
                chat panel — every tool the Bot calls is
                captured. When you're done, click <b>Stop</b> to
                save the captured calls as a Skill.
              </p>
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
              <label className="field-group">
                <span>Skill name (optional — you can edit before saving)</span>
                <input
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                  placeholder="e.g. fetch-sf-weather"
                />
              </label>
              <label className="field-group">
                <span>Description (optional)</span>
                <input
                  type="text"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                  placeholder="what this Skill does"
                />
              </label>
            </>
          )}

          {phase.kind === "recording" && (
            <>
              <p className="skills-dialog__description">
                <b>Recording.</b> Go to the chat panel and ask the
                Bot to do the task you want to capture. When it's
                done, click <b>Stop</b> below.
              </p>
              <p className="skills-dialog__hint">
                Tip: only the Bot's tool calls are captured —
                thinking / prose is not part of the Skill.
              </p>
            </>
          )}

          {phase.kind === "editing" && (
            <>
              {isReRecord && (
                <p className="skills-dialog__hint">
                  v3.3.0 — Re-record: the steps below are this
                  Skill's current JSON. Edit and click
                  <b> Update</b> to save in place (the Skill's
                  id, name, and any bot schedule binding are
                  preserved).
                </p>
              )}
              <label className="field-group">
                <span>Name</span>
                <input
                  type="text"
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                />
              </label>
              <label className="field-group">
                <span>Description</span>
                <input
                  type="text"
                  value={description}
                  onChange={(e) => setDescription(e.target.value)}
                />
              </label>
              <label className="field-group">
                <span>Steps (JSON)</span>
                <textarea
                  className="skills-dialog__textarea"
                  value={stepsJson}
                  onChange={(e) => setStepsJson(e.target.value)}
                  spellCheck={false}
                  rows={14}
                  data-testid="skills-dialog-steps"
                />
              </label>
            </>
          )}

          {error && (
            <div className="skills-panel__error" role="alert">
              {error}
            </div>
          )}
        </div>
        <div className="modal-stacked-footer">
          {phase.kind === "configure" && (
            <>
              <button className="ghost" onClick={onClose}>
                Cancel
              </button>
              <button
                className="primary"
                onClick={handleStart}
                disabled={!botId}
              >
                Start
              </button>
            </>
          )}
          {phase.kind === "recording" && (
            <>
              <button className="ghost" onClick={handleAbort}>
                Abort
              </button>
              <button className="primary" onClick={handleStop}>
                Stop
              </button>
            </>
          )}
          {phase.kind === "editing" && (
            <>
              <button
                className="ghost"
                onClick={onClose}
                disabled={saving}
              >
                Cancel
              </button>
              <button
                className="primary"
                onClick={handleSave}
                disabled={saving}
                data-testid="skills-dialog-save"
              >
                {saving ? "Saving…" : isReRecord ? "Update" : "Save"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
