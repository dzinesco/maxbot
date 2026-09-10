// v2.4.0 — Create-group modal.
//
// v3.5.0 (Phase 6) — Added a "Template" step before
// the "Members" step. The chips are Detective /
// Mailroom / Coordinator: each one pre-creates a Bot
// with a sensible system prompt and the Grok Bot
// defaults approval preset (v3.4.0), so a user can
// stand up the canonical 3-Bot pod in two clicks.
//
// On submit, the dialog:
//   1. upserts every template-selected Bot (the
//      Grok Bot defaults land via the Rust `upsert_bot`
//      path, no extra wiring needed)
//   2. calls `groupCreate` with the picked owner + the
//      template Bots + any additional manually-picked
//      members
//
// Shown from the Sidebar's "Groups" section. Total
// members is 2-6, matching the Rust `validate_group_size`
// rule. On submit, calls `groupCreate` and emits the new
// group id to the parent so it can select the new group.

import { useMemo, useState } from "react";
import type { Bot } from "../lib/api";
import { groupCreate, upsertBot } from "../lib/tauri";

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

/** v3.5.0 (Phase 6) — Template chip shape. Each
 *  entry produces a Bot on submit. The system prompt
 *  is the only required content; the rest is filled
 *  in by `upsert_bot` (model defaults, color, icon,
 *  Grok Bot approval rules). */
interface TemplateChip {
  key: "detective" | "mailroom" | "coordinator";
  label: string;
  hint: string;
  defaultName: string;
  icon: string;
  color: string;
  systemPrompt: string;
}

const TEMPLATE_CHIPS: TemplateChip[] = [
  {
    key: "detective",
    label: "Detective",
    hint: "Searches the web and reads files. Hands a write-up to Mailroom.",
    defaultName: "Detective",
    icon: "🕵️",
    color: "#7c3aed",
    systemPrompt:
      "You are the Detective in this group. Your job is to research, search, and gather information. Use web_search, web_fetch, and file_read to assemble a write-up. Save your findings to the shared folder with shared_write so the next Bot (Mailroom) can pick them up. Hand work off by leaving a clear handoff note and a one-line summary in the transcript.",
  },
  {
    key: "mailroom",
    label: "Mailroom",
    hint: "Drafts replies, schedules follow-ups, and routes messages.",
    defaultName: "Mailroom",
    icon: "✉️",
    color: "#0891b2",
    systemPrompt:
      "You are the Mailroom in this group. Your job is to draft replies, schedule follow-ups, and route messages. Read the Detective's handoff from the shared folder with shared_read, then draft replies using the mail tools. Every outbound message must be confirmed with the human before send — your approval preset keeps mail_send on Ask.",
  },
  {
    key: "coordinator",
    label: "Coordinator",
    hint: "Watches the shared folder, hands work to the right Bot, and reports back to the human.",
    defaultName: "Coordinator",
    icon: "🧭",
    color: "#16a34a",
    systemPrompt:
      "You are the Coordinator in this group. Your job is to watch the shared folder (shared_list) and hand work to the right Bot. When a Detective write-up lands, route it to Mailroom. When Mailroom's draft is ready, summarize for the human. Stay out of the per-Bot VM; operate only through the shared folder and the human.",
  },
];

/** Pick a free Bot name by appending " 2", " 3", etc.
 *  if the default name is already taken. Used by the
 *  template step so clicking "Detective" twice in a
 *  row still produces a usable Bot. */
function uniqueName(base: string, taken: Set<string>): string {
  if (!taken.has(base)) return base;
  for (let i = 2; i < 100; i++) {
    const candidate = `${base} ${i}`;
    if (!taken.has(candidate)) return candidate;
  }
  // Pathological fallback.
  return `${base} ${Date.now()}`;
}

export function CreateGroupDialog({
  bots,
  defaultOwnerBotId,
  onCreated,
  onClose,
}: CreateGroupDialogProps) {
  // v3.5.0 — Template selection is the new "first
  // step." The user can pick zero, one, two, or all
  // three templates. Picked templates turn into
  // pre-created Bots on submit.
  const [pickedTemplates, setPickedTemplates] = useState<Set<string>>(
    new Set(),
  );
  const [name, setName] = useState("");
  const [ownerBotId, setOwnerBotId] = useState<string>(
    defaultOwnerBotId ?? bots[0]?.id ?? "",
  );
  const [memberBotIds, setMemberBotIds] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Bots that will be created from template chips.
  // Computed in render so the owner / member pickers
  // can pre-select them on the next step.
  const templateBots = useMemo<Bot[]>(() => {
    const taken = new Set(bots.map((b) => b.name));
    return TEMPLATE_CHIPS.filter((t) => pickedTemplates.has(t.key)).map(
      (t) => {
        const finalName = uniqueName(t.defaultName, taken);
        taken.add(finalName);
        return {
          // id is server-assigned; we ship an empty
          // string and let `upsert_bot` fill it in.
          id: "",
          name: finalName,
          description: t.hint,
          system_prompt: t.systemPrompt,
          // Empty model = backend default (xAI Grok 4
          // Fast, per the Mac app's onboarding).
          default_model: "",
          // Start with the full tool allowlist. The
          // Grok Bot defaults preset (v3.4.0) is
          // applied server-side on insert and gates
          // the actual tool use; the allowlist here
          // just determines which tools are visible to
          // the model in the chat request.
          allowed_tools: [
            "shared_read",
            "shared_list",
            "shared_write",
            "file_read",
            "file_write",
            "web_search",
            "web_fetch",
            "mail_inbox",
            "mail_search",
            "mail_draft",
          ],
          icon: t.icon,
          color: t.color,
          created_at: "",
          updated_at: "",
        };
      },
    );
  }, [pickedTemplates, bots]);

  const totalSelected = memberBotIds.length + 1; // owner counts as 1
  const validName = name.trim().length > 0;
  const validTotal = totalSelected >= MIN_TOTAL && totalSelected <= MAX_TOTAL;
  const canSubmit = validName && !!ownerBotId && validTotal && !submitting;

  const toggleTemplate = (key: string) => {
    setPickedTemplates((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  };

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
      // Upsert every template Bot first. The
      // server-side `upsert_bot` applies the Grok
      // Bot defaults preset (v3.4.0) on insert, so
      // these Bots come out of the gate with the
      // right approval rules — no extra wiring
      // needed. We collect the returned ids so the
      // group_create call knows which Bots to
      // assemble.
      const created: Bot[] = [];
      for (const draft of templateBots) {
        const persisted = await upsertBot(draft);
        created.push(persisted);
      }
      // Members = created template Bots + any
      // manually-picked existing Bots, deduped and
      // excluding the owner. Total must be 2-6
      // including the owner; the same validation
      // runs server-side.
      const memberIds: string[] = [];
      const seen = new Set<string>([ownerBotId]);
      for (const id of [
        ...created.map((b) => b.id),
        ...memberBotIds,
      ]) {
        if (!seen.has(id)) {
          seen.add(id);
          memberIds.push(id);
        }
      }
      const result = await groupCreate(
        name.trim(),
        ownerBotId,
        memberIds,
      );
      onCreated(result.chat.id);
    } catch (e) {
      setError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  if (bots.length < 2 && pickedTemplates.size === 0) {
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

        {/* v3.5.0 — Template step. Each chip
            pre-creates a Bot with the right system
            prompt and the Grok Bot defaults approval
            preset. The user can pick zero, one, two,
            or all three. */}
        <div className="field">
          <span>Start from a template (optional)</span>
          <div
            className="create-group-template-chips"
            data-testid="create-group-templates"
          >
            {TEMPLATE_CHIPS.map((t) => {
              const checked = pickedTemplates.has(t.key);
              return (
                <button
                  key={t.key}
                  type="button"
                  className={`create-group-template-chip${
                    checked ? " create-group-template-chip--checked" : ""
                  }`}
                  onClick={() => toggleTemplate(t.key)}
                  data-testid={`create-group-template-${t.key}`}
                  aria-pressed={checked}
                >
                  <span className="create-group-template-chip__icon">
                    {t.icon}
                  </span>
                  <span className="create-group-template-chip__label">
                    {t.label}
                  </span>
                  <span className="create-group-template-chip__hint">
                    {t.hint}
                  </span>
                </button>
              );
            })}
          </div>
          {templateBots.length > 0 && (
            <p
              className="create-group-template-summary"
              data-testid="create-group-template-summary"
            >
              Will create {templateBots.length} Bot
              {templateBots.length === 1 ? "" : "s"}:{" "}
              {templateBots.map((b) => b.name).join(", ")}
            </p>
          )}
        </div>

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
