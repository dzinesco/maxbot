import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Message, PersistedToolCall } from "../lib/api";
import { approvalDecide, memoryRemember, ttsSpeak, ttsStop } from "../lib/tauri";

interface MessageBubbleProps {
  message: Message;
  streaming?: boolean;
  /** Called when the user clicks the Retry button on an error
   * block. Wires the bubble to App.tsx's `handleRegenerate` so
   * the user can re-run the last turn from the failure point
   * itself, not just from the bottom bar. */
  onRetry?: () => void;
  /** v2.5.0 — when set, assistant bubbles get a "📌" button
   *  that saves the message to the Bot's memory as a fact.
   *  Chat runs leave this `null` (the user is chatting with the
   *  default MaxBot, not a specific Bot). */
  botId?: string | null;
  /** v3.7.13 — UX-3. The most-recent user message
   *  text, used to pre-fill the `to` field on a
   *  `mail_draft` / `gmail_send` form when the
   *  model left it empty. Required for the form
   *  pre-fill rule; the chat view passes it
   *  through from the message list. */
  lastUserMessage?: string | null;
}

/** Derive a snake_case memory key from the first 6
 * non-trivial words of a message. Falls back to a timestamp-
 * based key when the message is empty or only punctuation. */
function deriveMemoryKey(content: string): string {
  const words = content
    .toLowerCase()
    .replace(/[^a-z0-9\s]/g, " ")
    .split(/\s+/)
    .filter((w) => w.length > 1)
    .slice(0, 6);
  if (words.length === 0) {
    return `note_${Date.now()}`;
  }
  return words.join("_");
}

/** Strip `<think>...</think>` blocks (and any other reasoning-style
 * tags the model may emit) from the visible content. The raw
 * reasoning is preserved in the DB and the streaming layer — we
 * only hide it from the chat view. Multi-line, nested-safe.
 *
 * Handles two cases:
 *  1. Paired `<think>…</think>` — strip the whole run.
 *  2. Orphaned `<think>` with no closing tag (model stream cut off
 *     mid-reasoning, or model simply forgot to close it) — strip
 *     from the tag to end of input so the user doesn't see a raw
 *     opening tag with no payload.
 */
function stripReasoningTags(input: string): string {
  let out = input;
  let prev: string | null = null;
  while (out !== prev) {
    prev = out;
    out = out.replace(/<think>[\s\S]*?<\/think>/gi, "");
  }
  // Orphaned opening tag (no closing). Strip from `<think>` to end.
  out = out.replace(/<think>[\s\S]*$/gi, "");
  return out.trim();
}

/** Very small markdown subset: paragraphs, fenced code blocks, inline code,
 * and links. Enough to make MiniMax responses readable without pulling in a
 * 200KB markdown library. Anything else renders as plain text.
 */
function renderMarkdown(input: string): React.ReactNode {
  const cleaned = stripReasoningTags(input);
  const nodes: React.ReactNode[] = [];
  const lines = cleaned.split("\n");
  let i = 0;
  let key = 0;
  while (i < lines.length) {
    const line = lines[i];
    if (line.startsWith("```")) {
      const codeLines: string[] = [];
      i++;
      while (i < lines.length && !lines[i].startsWith("```")) {
        codeLines.push(lines[i]);
        i++;
      }
      if (i < lines.length) i++;
      nodes.push(
        <pre key={key++}>
          <code>{codeLines.join("\n")}</code>
        </pre>,
      );
      continue;
    }
    if (line.trim() === "") {
      i++;
      continue;
    }
    const paraLines: string[] = [line];
    i++;
    while (i < lines.length && lines[i].trim() !== "" && !lines[i].startsWith("```")) {
      paraLines.push(lines[i]);
      i++;
    }
    nodes.push(<p key={key++}>{renderInline(paraLines.join(" "))}</p>);
  }
  return nodes;
}

function renderInline(text: string): React.ReactNode {
  const parts: React.ReactNode[] = [];
  let i = 0;
  let key = 0;
  while (i < text.length) {
    if (text[i] === "`") {
      const end = text.indexOf("`", i + 1);
      if (end > i) {
        parts.push(<code key={key++}>{text.slice(i + 1, end)}</code>);
        i = end + 1;
        continue;
      }
    }
    if (text.startsWith("**", i)) {
      const end = text.indexOf("**", i + 2);
      if (end > i) {
        parts.push(<strong key={key++}>{text.slice(i + 2, end)}</strong>);
        i = end + 2;
        continue;
      }
    }
    const next = text.slice(i).search(/[`*]/);
    if (next === -1) {
      parts.push(text.slice(i));
      break;
    }
    parts.push(text.slice(i, i + next));
    i = i + next;
  }
  return parts;
}

function ToolCalls({
  calls,
  lastUserMessage,
}: {
  calls: PersistedToolCall[];
  lastUserMessage?: string | null;
}) {
  if (calls.length === 0) return null;
  // v3.7.13 — UX-3 + UX-5. Gated tool names get a
  // form instead of a JSON preview. The
  // chat-bubble context doesn't have an
  // approvalId, so the form renders read-only
  // (the user sees what the model wants, the
  // approval sheet is where they act on it).
  //
  // v3.7.13 — UX-5. The pluralized "N tool calls"
  // header is gone. For gated tool names, the
  // header is the same one-liner the approval
  // form uses ("Draft email" / "Send email" / "Add
  // to calendar") so the user sees a single
  // intent line. Unknown tools fall through to a
  // neutral "Tool call" header.
  const firstCall = calls[0];
  const headerText = FORM_TOOLS.has(firstCall.name)
    ? approveLabelFor(firstCall.name)
    : firstCall.name === "shell_run" || firstCall.name.startsWith("vm_")
    ? "Computer"
    : "Tool call";
  return (
    <div className="tool-calls">
      <div
        className="tool-calls-header"
        data-testid="tool-calls-header"
      >
        {headerText}
      </div>
      {calls.map((tc) => (
        <ToolCallFormOrCard
          key={tc.id}
          call={tc}
          lastUserMessage={lastUserMessage}
        />
      ))}
    </div>
  );
}

/** A single tool call, rendered as a card with a one-line preview
 * that expands to the full argument JSON. Designed so the chat
 * stays scannable when an agent makes several calls in a row. */
function ToolCallCard({ call }: { call: PersistedToolCall }) {
  const parsed = useMemo(() => {
    const raw = call.arguments || "{}";
    try {
      return { ok: true as const, value: JSON.parse(raw) as unknown };
    } catch {
      return { ok: false as const, value: raw };
    }
  }, [call.arguments]);

  // Categorize the tool by name. The category drives the
  // left-edge accent strip and the icon color so the user can
  // scan a multi-call agent turn and tell at a glance which
  // surface the agent is operating on (browser, code/CLI,
  // filesystem, or neutral). Adding a new tool: drop its name
  // in here. Unknown tools land in 'default'.
  const category = useMemo<ToolCategory>(() => {
    if (
      call.name === "ego_browser" ||
      call.name.startsWith("safari_") ||
      call.name.startsWith("chrome_")
    ) {
      return "browser";
    }
    if (
      call.name === "grok_prompt" ||
      call.name === "shell_run" ||
      call.name === "file_read" ||
      call.name === "file_write" ||
      call.name === "tts_speak" ||
      call.name === "tts_stop" ||
      call.name === "web_fetch" ||
      call.name === "web_search"
    ) {
      return "code";
    }
    if (
      call.name.startsWith("memory_") ||
      call.name.startsWith("scratchpad_") ||
      call.name.startsWith("outputs_") ||
      call.name === "app_open" ||
      call.name === "app_list"
    ) {
      return "fs";
    }
    return "default";
  }, [call.name]);

  const preview = useMemo(() => {
    const trim = (s: string, n: number) =>
      s.length > n ? s.slice(0, n) + "…" : s;
    if (!parsed.ok) return trim(String(parsed.value), 80);
    const v = parsed.value;
    if (v === null || typeof v !== "object" || Array.isArray(v)) {
      return trim(JSON.stringify(v), 80);
    }
    const entries = Object.entries(v as Record<string, unknown>).slice(0, 2);
    return entries
      .map(([k, val]) => {
        const s = typeof val === "string" ? val : JSON.stringify(val);
        return `${k}: ${trim(s, 40)}`;
      })
      .join("  ·  ");
  }, [parsed]);

  return (
    <details className="tool-call-card" data-category={category}>
      <summary>
        <span className="tool-call-icon" aria-hidden>
          {TOOL_ICONS[category]}
        </span>
        <span className="tool-call-name">{call.name}</span>
        {preview && <span className="tool-call-preview">{preview}</span>}
      </summary>
      <div className="tool-call-body">
        <pre className="tool-call-args">
          {parsed.ok ? JSON.stringify(parsed.value, null, 2) : parsed.value}
        </pre>
      </div>
    </details>
  );
}

type ToolCategory = "browser" | "code" | "fs" | "default";

const TOOL_ICONS: Record<ToolCategory, string> = {
  browser: "🌐",
  code: "⚡",
  fs: "📁",
  default: "⚙",
};

// ---- v3.7.13 — UX-3. Form for gated tool calls ----
//
// `mail_draft`, `calendar_event_create`, and
// `gmail_send` each show a small editable form
// instead of a JSON blob. The pre-fill rule: if
// the model left the destination/title field
// empty (or sent `""`), pull it from the most-
// recent user message. This is the same rule the
// inline approval form uses; pinning the same
// behavior in two places keeps the affordance
// consistent.
//
// The form is rendered in two modes:
//
//   1. Read-only preview (the inline message
//      bubble). The form fields are visible but
//      not editable, and the approve/cancel
//      buttons are hidden. The user can see what
//      the model is proposing without leaving the
//      chat.
//
//   2. Editable (the approval sheet, UX-4). The
//      form is editable, with approve/cancel
//      buttons wired to `approvalDecide`.
//
// Unknown tool names fall through to the
// existing `ToolCallCard` JSON preview.

/** Tool names that get a form instead of a JSON
 *  blob. Adding a new gated tool: drop it in
 *  here AND extend the schema in
 *  `formSchemaFor()` below. */
const FORM_TOOLS = new Set([
  "mail_draft",
  "calendar_event_create",
  "gmail_send",
]);

type FormFieldSchema = {
  key: string;
  label: string;
  type: "text" | "textarea" | "datetime-local";
  /** Optional: if the model sent an empty value
   *  for this key, pre-fill from the user's last
   *  message. */
  prefillFromUserMessage?: boolean;
  /** Lines (for textarea). */
  rows?: number;
};

function formSchemaFor(toolName: string): FormFieldSchema[] | null {
  if (toolName === "mail_draft" || toolName === "gmail_send") {
    return [
      {
        key: "to",
        label: "To",
        type: "text",
        prefillFromUserMessage: true,
      },
      { key: "subject", label: "Subject", type: "text" },
      { key: "body", label: "Body", type: "textarea", rows: 6 },
    ];
  }
  if (toolName === "calendar_event_create") {
    return [
      {
        key: "title",
        label: "Title",
        type: "text",
        prefillFromUserMessage: true,
      },
      { key: "starts", label: "Starts", type: "datetime-local" },
      { key: "ends", label: "Ends", type: "datetime-local" },
      {
        key: "description",
        label: "Description",
        type: "textarea",
        rows: 4,
      },
      { key: "attendees", label: "Attendees", type: "text" },
    ];
  }
  return null;
}

/** Approve-button label per tool. The wording is
 *  user-facing copy, not a verb; the
 *  `approvalDecide` call uses the canonical
 *  `"approved"` decision regardless. */
function approveLabelFor(toolName: string): string {
  if (toolName === "mail_draft") return "Create draft";
  if (toolName === "calendar_event_create") return "Add to calendar";
  if (toolName === "gmail_send") return "Send email";
  return "Approve";
}

export interface ToolCallFormProps {
  toolName: string;
  /** The original tool args the model emitted.
   *  Used as the form's initial value. */
  args: Record<string, unknown>;
  /** The most-recent user message text. Used to
   *  pre-fill fields the model left empty. */
  lastUserMessage?: string | null;
  /** Approval id. When set, the form is
   *  interactive and the buttons call
   *  `approvalDecide`. When null, the form is a
   *  read-only preview (the inline bubble case). */
  approvalId?: string | null;
  /** Called after the user clicks Approve. The
   *  parent is responsible for clearing the
   *  approval (e.g. by calling approvalDecide or
   *  by removing the row from the queue). */
  onApprove?: (editedArgs: Record<string, unknown>) => void;
  /** Called after the user clicks Cancel. */
  onCancel?: () => void;
}

export function ToolCallForm({
  toolName,
  args,
  lastUserMessage,
  approvalId,
  onApprove,
  onCancel,
}: ToolCallFormProps) {
  const schema = formSchemaFor(toolName);
  // Unknown tool — the caller should fall through
  // to the JSON preview instead. Render an empty
  // <div> as a safety net.
  if (!schema) {
    return <div data-testid="tool-call-form-unknown" />;
  }
  const interactive = !!approvalId;
  // Initial form state: each schema key gets the
  // model's value, falling back to the user's
  // last message for empty fields where
  // `prefillFromUserMessage` is set.
  const initial = useMemo(() => {
    const out: Record<string, string> = {};
    for (const field of schema) {
      const v = args[field.key];
      const isEmpty =
        v === undefined || v === null || (typeof v === "string" && v === "");
      if (isEmpty && field.prefillFromUserMessage && lastUserMessage) {
        out[field.key] = lastUserMessage;
      } else if (typeof v === "string") {
        out[field.key] = v;
      } else {
        out[field.key] = v == null ? "" : JSON.stringify(v);
      }
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [toolName, JSON.stringify(args), lastUserMessage]);

  const [values, setValues] = useState<Record<string, string>>(initial);
  // Re-sync when the initial state changes (e.g.
  // a new tool call lands while the form is
  // open). Without this, switching between
  // pending tool calls in the approval queue
  // would show stale data.
  useEffect(() => {
    setValues(initial);
  }, [initial]);

  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const handleApprove = useCallback(async () => {
    if (!approvalId || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await approvalDecide(approvalId, "approved", values);
      onApprove?.(values);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }, [approvalId, submitting, values, onApprove]);

  const handleCancel = useCallback(async () => {
    if (!approvalId || submitting) return;
    setSubmitting(true);
    setError(null);
    try {
      await approvalDecide(approvalId, "rejected", {
        reason: "user changed mind",
      });
      onCancel?.();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSubmitting(false);
    }
  }, [approvalId, submitting, onCancel]);

  return (
    <div
      className="tool-call-form"
      data-testid="tool-call-form"
      data-tool={toolName}
    >
      {schema.map((field) => {
        const id = `tcf-${toolName}-${field.key}`;
        if (field.type === "textarea") {
          return (
            <label key={field.key} className="tool-call-form__field">
              <span className="tool-call-form__label">{field.label}</span>
              <textarea
                id={id}
                data-testid={`tool-call-form-${field.key}`}
                value={values[field.key] ?? ""}
                onChange={(e) =>
                  setValues((v) => ({ ...v, [field.key]: e.target.value }))
                }
                rows={field.rows ?? 3}
                readOnly={!interactive}
                disabled={!interactive}
              />
            </label>
          );
        }
        return (
          <label key={field.key} className="tool-call-form__field">
            <span className="tool-call-form__label">{field.label}</span>
            <input
              id={id}
              data-testid={`tool-call-form-${field.key}`}
              type={field.type}
              value={values[field.key] ?? ""}
              onChange={(e) =>
                setValues((v) => ({ ...v, [field.key]: e.target.value }))
              }
              readOnly={!interactive}
              disabled={!interactive}
            />
          </label>
        );
      })}
      {interactive && (
        <div className="tool-call-form__actions">
          {error && (
            <span
              className="tool-call-form__error"
              data-testid="tool-call-form-error"
            >
              {error}
            </span>
          )}
          <button
            type="button"
            className="tool-call-form__cancel"
            data-testid="tool-call-form-cancel"
            onClick={handleCancel}
            disabled={submitting}
          >
            Cancel
          </button>
          <button
            type="button"
            className="tool-call-form__approve"
            data-testid="tool-call-form-approve"
            onClick={handleApprove}
            disabled={submitting}
          >
            {submitting ? "Working…" : approveLabelFor(toolName)}
          </button>
        </div>
      )}
    </div>
  );
}

/** v3.7.13 — UX-3. Render an inline form for the
 *  gated tool names; fall through to the JSON
 *  preview for anything else. The approvalId
 *  param is null in the chat-bubble context (we
 *  don't have one to wire to); the form renders
 *  read-only. The approval sheet passes a real
 *  approvalId and gets the interactive version. */
function ToolCallFormOrCard({
  call,
  lastUserMessage,
  approvalId,
}: {
  call: PersistedToolCall;
  lastUserMessage?: string | null;
  approvalId?: string | null;
}) {
  const schema = formSchemaFor(call.name);
  if (!schema) return <ToolCallCard call={call} />;
  let parsedArgs: Record<string, unknown> = {};
  try {
    const parsed = JSON.parse(call.arguments || "{}");
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      parsedArgs = parsed as Record<string, unknown>;
    }
  } catch {
    // malformed args — fall through with empty
  }
  return (
    <details
      className="tool-call-card"
      data-category="code"
      data-testid="tool-call-card-form"
      open
    >
      <summary>
        <span className="tool-call-icon" aria-hidden>
          ⚡
        </span>
        <span className="tool-call-name">{call.name}</span>
        <span className="tool-call-preview">
          {approveLabelFor(call.name)}
        </span>
      </summary>
      <div className="tool-call-body">
        <ToolCallForm
          toolName={call.name}
          args={parsedArgs}
          lastUserMessage={lastUserMessage}
          approvalId={approvalId}
        />
      </div>
    </details>
  );
}

/** Animated three-dot indicator, shown in place of a streaming
 * bubble's content while we're waiting for the first token. The
 * streaming caret (▍) only shows once content has arrived. */
function TypingDots() {
  return (
    <div className="typing-dots" aria-label="Assistant is typing">
      <span />
      <span />
      <span />
    </div>
  );
}

/**
 * Error block rendered INSIDE the assistant message bubble when a
 * stream ends in a wire-protocol / network / auth failure. The
 * friendly description comes from `StreamError::friendly_message`
 * on the Rust side; the optional `onRetry` callback fires the same
 * `handleRegenerate` flow used by the bottom-bar Regenerate button.
 *
 * Distinct visual treatment on purpose: danger color, subtle tinted
 * background, no message-body chrome (no `Copy` button, no avatar
 * re-emit). It signals "this turn didn't complete" without
 * contaminating the chat scroll with raw `[error] protocol: …`
 * strings.
 */
export function ErrorMessage({
  error,
  onRetry,
}: {
  error: string;
  onRetry?: () => void;
}) {
  return (
    <div className="error-message" role="alert">
      <span className="error-message-icon" aria-hidden>
        ⚠
      </span>
      <div className="error-message-body">
        <div className="error-message-title">Couldn't finish the response</div>
        <pre className="error-message-detail">{error}</pre>
      </div>
      {onRetry && (
        <button
          className="error-message-retry"
          onClick={onRetry}
          title="Re-run the last user message and replace this response"
        >
          ↻ Retry
        </button>
      )}
    </div>
  );
}

/** Collapsible wrapper for long tool result messages. Tool
 * results can be 20+ KB of JSON; showing that wall of text by
 * default buries the rest of the chat. Default-collapsed shows a
 * short preview + an "Expand" button. */
function CollapsibleToolResult({ content }: { content: string }) {
  const [open, setOpen] = useState(false);
  // Preview: first ~200 chars on the first non-empty line,
  // trimmed. If the content is JSON-ish, prefer to show a single
  // short string instead of dumping the raw object.
  const preview = useMemo(() => {
    const trimmed = content.trim();
    if (trimmed.length === 0) return "(empty result)";
    // Try to extract a one-line summary from common result shapes.
    try {
      const j = JSON.parse(trimmed) as unknown;
      if (typeof j === "string") return j.slice(0, 200);
      if (j && typeof j === "object") {
        const obj = j as Record<string, unknown>;
        for (const key of ["result", "reply", "text", "message", "output", "data"]) {
          if (typeof obj[key] === "string") {
            return String(obj[key]).slice(0, 200);
          }
        }
      }
    } catch {
      // not JSON; fall through
    }
    return trimmed.split("\n").find((l) => l.trim().length > 0)?.slice(0, 200) ?? trimmed.slice(0, 200);
  }, [content]);

  return (
    <div className="tool-result">
      {open ? (
        <>
          <pre className="tool-result-body">{content}</pre>
          <button
            className="link"
            onClick={() => setOpen(false)}
            title="Show less"
          >
            Show less
          </button>
        </>
      ) : (
        <button
          className="tool-result-preview"
          onClick={() => setOpen(true)}
          title="Expand full result"
        >
          <span className="tool-result-snippet">{preview}</span>
          <span className="tool-result-meta">
            {content.length.toLocaleString()} chars · click to expand
          </span>
        </button>
      )}
    </div>
  );
}

// ---- v3.7.13 — UX-5. TTS / Copy toolbar ----
//
// The pre-v3.7.13 message bubble had a per-bubble
// "🔊 Speak" + "Copy" pair in the meta row. v3.7.13
// moves both to a single toolbar above the message
// list, so the user has one place to act on the
// most-recent assistant turn (the only one worth
// speaking / copying in practice). The per-bubble
// pin (📌) stays — that's per-message memory
// affordance, not TTS.
//
// The toolbar handles the same `tts_speak` /
// `tts_stop` IPC the per-bubble buttons did, and
// keeps the same "Speaking…" / "Stop" toggle. The
// `mostRecentAssistantMessage` helper is exported
// so ChatView can compute the target text without
// duplicating the role check.
export function mostRecentAssistantMessage(
  messages: Message[],
): Message | null {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].role === "assistant") {
      return messages[i];
    }
  }
  return null;
}

interface TTSToolbarProps {
  /** The most-recent assistant message. When
   *  null, the toolbar renders disabled. */
  target: Message | null;
}

/** v3.7.13 — UX-5. Speak / Copy toolbar rendered
 *  above the message list. One Speak button that
 *  speaks the most-recent assistant message, one
 *  Copy button that copies it. The keyboard
 *  shortcut is bound at the ChatView level (space,
 *  when the user isn't focused in an input) — see
 *  ChatView.tsx. */
export function TTSToolbar({ target }: TTSToolbarProps) {
  const [speaking, setSpeaking] = useState(false);
  const [copied, setCopied] = useState(false);
  const speechMarker = useRef<string | null>(null);

  const handleToggleSpeech = useCallback(async () => {
    if (!target) return;
    if (speaking) {
      try {
        await ttsStop();
      } catch {
        // best-effort — the speech will end on its own
      }
      setSpeaking(false);
      speechMarker.current = null;
      return;
    }
    try {
      speechMarker.current = target.id;
      setSpeaking(true);
      await ttsSpeak(target.content);
      // The `speaking` state flips back via
      // the marker's cleanup so a stop click
      // mid-speech isn't a no-op. The
      // speakable-vs-empty check guards
      // against an empty assistant message
      // (e.g. while streaming).
      if (speechMarker.current === target.id) {
        setSpeaking(false);
        speechMarker.current = null;
      }
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("tts_speak failed:", e);
      setSpeaking(false);
      speechMarker.current = null;
    }
  }, [target, speaking]);

  const handleCopy = useCallback(async () => {
    if (!target) return;
    try {
      await navigator.clipboard.writeText(target.content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // Fallback for non-secure contexts
      // (clipboard API is gated on https or
      // localhost).
      const ta = document.createElement("textarea");
      ta.value = target.content;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
        setCopied(true);
        setTimeout(() => setCopied(false), 1200);
      } catch {
        // give up silently
      } finally {
        document.body.removeChild(ta);
      }
    }
  }, [target]);

  const disabled = !target || !target.content;

  return (
    <div
      className="tts-toolbar"
      data-testid="tts-toolbar"
      data-disabled={disabled ? "true" : "false"}
    >
      <button
        type="button"
        className={`tts-toolbar__btn${speaking ? " speaking" : ""}`}
        data-testid="tts-toolbar-speak"
        onClick={handleToggleSpeech}
        disabled={disabled}
        title={
          speaking
            ? "Stop speaking (or press space)"
            : "Speak the most-recent message (space)"
        }
      >
        {speaking ? "⏹ Stop" : "🔊 Speak"}
      </button>
      <button
        type="button"
        className="tts-toolbar__btn"
        data-testid="tts-toolbar-copy"
        onClick={handleCopy}
        disabled={disabled}
        title="Copy the most-recent message"
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

export const MessageBubble = memo(function MessageBubble({
  message,
  streaming,
  onRetry,
  botId,
  lastUserMessage,
}: MessageBubbleProps) {
  const isUser = message.role === "user";
  const isTool = message.role === "tool";
  const isAssistant = !isUser && !isTool;
  // v2.5.0 — the "📌 Remember this" button briefly shows
  // "Saved" after a successful click. Per-bubble state so a
  // click on one bubble's pin doesn't tear down another's.
  // v3.7.13 — UX-5. The per-bubble Speak / Copy
  // buttons are gone; those moved to the
  // TTSToolbar above the message list.
  const [pinned, setPinned] = useState(false);

  const handlePin = useCallback(async () => {
    if (!botId || !message.content.trim()) return;
    const key = deriveMemoryKey(message.content);
    try {
      await memoryRemember(botId, "fact", key, message.content);
      setPinned(true);
      setTimeout(() => setPinned(false), 1500);
    } catch (e) {
      // eslint-disable-next-line no-console
      console.warn("memory_remember failed:", e);
    }
  }, [botId, message.content]);

  // Friendly "10:42 AM" formatting for the meta line. Show on
  // every non-streaming message; cheap to render and useful when
  // scrolling back through a long conversation.
  const metaTime = useMemo(() => {
    const d = new Date(message.created_at);
    if (Number.isNaN(d.getTime())) return "";
    return d.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  }, [message.created_at]);

  // Threshold above which a tool result is collapsed by default.
  // ~500 chars is roughly a paragraph — short enough to inline
  // a one-liner like "search took 0.3s" or "file written to
  // /tmp/x", long enough that we shouldn't dump 20 KB of JSON
  // into the chat scroll.
  const TOOL_RESULT_COLLAPSE_THRESHOLD = 500;

  return (
    <div
      className={`message ${message.role}${streaming ? " streaming" : ""}`}
    >
      <div className="avatar">
        {isUser ? "You" : isTool ? "T" : "M"}
      </div>
      <div className="body">
        <div className="meta">
          <span className="meta-name">
            {isUser ? "You" : isTool ? "Tool" : "MaxBot"}
          </span>
          {metaTime && <span className="meta-time">{metaTime}</span>}
          {!streaming && message.content && (
            <span className="meta-actions">
              {isAssistant && botId && (
                <button
                  className="copy-btn pin-btn"
                  onClick={handlePin}
                  title="Save this message to the Bot's memory as a fact"
                  data-testid="message-pin-button"
                >
                  {pinned ? "Saved" : "📌"}
                </button>
              )}
            </span>
          )}
        </div>
        {streaming && !message.content ? (
          <TypingDots />
        ) : isTool && message.content.length > TOOL_RESULT_COLLAPSE_THRESHOLD ? (
          <CollapsibleToolResult content={message.content} />
        ) : (
          <div className="content">{renderMarkdown(message.content)}</div>
        )}
        {/* v0.7.6: stream errors are rendered as a dedicated
         * block instead of being appended to `content`. Only
         * shows on assistant messages (not user / tool). The
         * Retry button is wired up by ChatView → App. */}
        {!streaming && isAssistant && message.error_message && (
          <ErrorMessage error={message.error_message} onRetry={onRetry} />
        )}
        <ToolCalls
          calls={message.tool_calls}
          lastUserMessage={lastUserMessage}
        />
      </div>
    </div>
  );
});
