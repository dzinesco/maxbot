// v2.2.0 — RecordSkillDialog.
//
// Two-step flow:
//   1. The user picks a Bot and types a kickoff prompt
//      ("What do you want it to do?").
//   2. We call `skill_record_start(bot_id)` which spawns
//      a Bot run in the background. The dialog shows
//      "Recording…" and a Stop button. The user can
//      also watch the Bot's response stream in the chat
//      panel; this dialog just tracks the recording
//      session.
//   3. On Stop, we call `skill_record_stop(recording_id)`
//      to drain the captured tool calls into a candidate
//      Skill. The dialog shows the candidate's name /
//      description / steps in editable fields; Save
//      calls `skill_create`.

import { useCallback, useEffect, useState } from "react";
import { createSkill, skillRecordStart, skillRecordStop } from "../lib/tauri";
import type { Bot, Skill, SkillStep } from "../lib/api";

interface RecordSkillDialogProps {
  bots: Bot[];
  defaultBotId: string | null;
  onClose: () => void;
  onSaved: (skill: Skill) => void;
}

type Phase =
  | { kind: "configure" }
  | { kind: "recording"; recordingId: string; botId: string }
  | { kind: "editing"; recordingId: string; candidate: Skill };

export function RecordSkillDialog(props: RecordSkillDialogProps) {
  const { bots, defaultBotId, onClose, onSaved } = props;
  const [phase, setPhase] = useState<Phase>({ kind: "configure" });
  const [botId, setBotId] = useState(
    defaultBotId ?? (bots[0] ? bots[0].id : ""),
  );
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [stepsJson, setStepsJson] = useState<string>(
    JSON.stringify(
      [{ tool: "example_tool", args: {} }],
      null,
      2,
    ),
  );
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
  }, [phase, stepsJson, name, description, onSaved]);

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-stacked skills-dialog"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-stacked-header">
          <h2>
            {phase.kind === "configure" && "Record a Skill"}
            {phase.kind === "recording" && "Recording…"}
            {phase.kind === "editing" && "Save Skill"}
          </h2>
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
              >
                {saving ? "Saving…" : "Save"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
