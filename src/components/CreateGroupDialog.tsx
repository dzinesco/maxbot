// v2.4.0 — Create-group modal.
//
// Shown from the Sidebar's "Groups" section. Lets the
// user pick a name, an owner Bot, and 1-5 additional
// Bots (so the total is 2-6, matching the Rust
// `validate_group_size` rule). On submit, calls
// `groupCreate` and emits the new group id to the
// parent so it can select the new group.

import { useState } from "react";
import type { Bot } from "../lib/api";
import { groupCreate } from "../lib/tauri";

interface CreateGroupDialogProps {
  /** All known Bots. The owner is selected from this list. */
  bots: Bot[];
  /** Default owner (the currently-selected Bot in the
   *  sidebar). `null` for a brand-new user. */
  defaultOwnerBotId?: string | null;
  /** Called with the new group id on successful create. */
  onCreated: (groupId: string) => void;
  onClose: () => void;
}

const MIN_TOTAL = 2;
const MAX_TOTAL = 6;
const MIN_EXTRA = 1; // owner + at least 1 additional
const MAX_EXTRA = 5; // owner + up to 5 additional = 6

export function CreateGroupDialog({
  bots,
  defaultOwnerBotId,
  onCreated,
  onClose,
}: CreateGroupDialogProps) {
  const [name, setName] = useState("");
  const [ownerBotId, setOwnerBotId] = useState<string>(
    defaultOwnerBotId ?? bots[0]?.id ?? "",
  );
  const [memberBotIds, setMemberBotIds] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const totalSelected = memberBotIds.length + 1; // owner counts as 1
  const validName = name.trim().length > 0;
  const validTotal = totalSelected >= MIN_TOTAL && totalSelected <= MAX_TOTAL;
  const canSubmit = validName && !!ownerBotId && validTotal && !submitting;

  const toggleMember = (botId: string) => {
    if (botId === ownerBotId) return; // owner can't also be additional
    setMemberBotIds((prev) =>
      prev.includes(botId)
        ? prev.filter((x) => x !== botId)
        : prev.length < MAX_EXTRA
          ? [...prev, botId]
          : prev,
    );
  };

  const submit = async () => {
    if (!canSubmit) return;
    setSubmitting(true);
    setError(null);
    try {
      const result = await groupCreate(name.trim(), ownerBotId, memberBotIds);
      onCreated(result.chat.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  if (bots.length < 2) {
    return (
      <div className="modal-overlay" onClick={onClose}>
        <div
          className="modal-stacked"
          onClick={(e) => e.stopPropagation()}
          data-testid="create-group-dialog"
        >
          <h2>Create a group</h2>
          <p>You need at least 2 Bots to create a group.</p>
          <div className="modal-actions">
            <button className="ghost" onClick={onClose}>
              Close
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div
        className="modal-stacked"
        onClick={(e) => e.stopPropagation()}
        data-testid="create-group-dialog"
      >
        <h2>Create a group</h2>
        <label className="field">
          <span>Group name</span>
          <input
            type="text"
            value={name}
            placeholder="Research pod"
            onChange={(e) => setName(e.target.value)}
            data-testid="create-group-name"
          />
        </label>
        <label className="field">
          <span>Owner</span>
          <select
            value={ownerBotId}
            onChange={(e) => setOwnerBotId(e.target.value)}
            data-testid="create-group-owner"
          >
            {bots.map((b) => (
              <option key={b.id} value={b.id}>
                {b.name}
              </option>
            ))}
          </select>
        </label>
        <div className="field">
          <span>
            Additional Bots ({totalSelected}/{MAX_TOTAL} total)
          </span>
          <div className="create-group-bot-picker">
            {bots
              .filter((b) => b.id !== ownerBotId)
              .map((b) => {
                const checked = memberBotIds.includes(b.id);
                return (
                  <label
                    key={b.id}
                    className={`create-group-bot-row${
                      checked ? " create-group-bot-row--checked" : ""
                    }`}
                  >
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggleMember(b.id)}
                      data-testid={`create-group-bot-${b.id}`}
                    />
                    <span>{b.name}</span>
                  </label>
                );
              })}
          </div>
        </div>
        {error && (
          <p style={{ color: "var(--danger)" }} data-testid="create-group-error">
            {error}
          </p>
        )}
        <div className="modal-actions">
          <button className="ghost" onClick={onClose}>
            Cancel
          </button>
          <button
            className="primary"
            onClick={submit}
            disabled={!canSubmit}
            data-testid="create-group-submit"
          >
            {submitting ? "Creating…" : "Create group"}
          </button>
        </div>
      </div>
    </div>
  );
}
