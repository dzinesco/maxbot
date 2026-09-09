import { useEffect, useMemo, useRef, useState } from "react";
import type { Message, PersistedToolCall } from "../lib/api";
import { ttsSpeak, ttsStop } from "../lib/tauri";

interface MessageBubbleProps {
  message: Message;
  streaming?: boolean;
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

function ToolCalls({ calls }: { calls: PersistedToolCall[] }) {
  if (calls.length === 0) return null;
  return (
    <div className="tool-calls">
      <div className="tool-calls-header">
        {calls.length} tool call{calls.length === 1 ? "" : "s"}
      </div>
      {calls.map((tc) => (
        <ToolCallCard key={tc.id} call={tc} />
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
    <details className="tool-call-card">
      <summary>
        <span className="tool-call-icon" aria-hidden>
          ⚡
        </span>
        <span className="tool-call-name">{call.name}</span>
        {preview && <span className="tool-call-preview">{preview}</span>}
      </summary>
      <pre className="tool-call-args">
        {parsed.ok ? JSON.stringify(parsed.value, null, 2) : parsed.value}
      </pre>
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

export function MessageBubble({ message, streaming }: MessageBubbleProps) {
  const isUser = message.role === "user";
  const isTool = message.role === "tool";
  const isAssistant = !isUser && !isTool;
  const [copied, setCopied] = useState(false);
  const [speaking, setSpeaking] = useState(false);
  // Tracks whether THIS bubble is the one currently being spoken. We
  // rely on a local ref + state so a click on a different bubble's
  // speaker button doesn't tear down our speech in flight.
  const speechMarker = useRef<string | null>(null);
  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(message.content);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      // Clipboard API can fail in iframes / non-secure contexts;
      // fall back to a hidden textarea + execCommand.
      const ta = document.createElement("textarea");
      ta.value = message.content;
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
  };
  const handleToggleSpeech = async () => {
    if (speaking) {
      try {
        await ttsStop();
      } catch {
        // best-effort — the speech will eventually end on its own
      }
      setSpeaking(false);
      speechMarker.current = null;
      return;
    }
    if (!message.content.trim()) return;
    setSpeaking(true);
    // Estimate how long the speech will take. `say` averages ~150
    // words/minute; we don't know the exact duration, but a rough
    // chars/12 estimate keeps the button in "Stop" state long enough
    // for typical messages. The user can always click Stop to
    // override.
    const chars = message.content.length;
    const approxSeconds = Math.max(3, Math.ceil(chars / 12));
    speechMarker.current = message.id;
    setTimeout(() => {
      if (speechMarker.current === message.id) {
        setSpeaking(false);
        speechMarker.current = null;
      }
    }, approxSeconds * 1000);
    try {
      await ttsSpeak(message.content);
    } catch (e) {
      // If the Rust side errors, fall out of the speaking state so
      // the button doesn't get stuck.
      setSpeaking(false);
      speechMarker.current = null;
      // Surface a non-blocking hint in the console; the model can
      // call tts_speak too if the user wants the error in a tool
      // result.
      // eslint-disable-next-line no-console
      console.warn("tts_speak failed:", e);
    }
  };
  // If the message is replaced (e.g. after a Regenerate), make sure
  // we don't leave the button in "speaking" state.
  useEffect(() => {
    return () => {
      if (speechMarker.current === message.id) {
        // Best-effort: stop any speech that was triggered by this
        // bubble. We don't await — the bubble is going away.
        ttsStop().catch(() => {});
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [message.id]);
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
              {isAssistant && (
                <button
                  className={`copy-btn tts-btn${speaking ? " speaking" : ""}`}
                  onClick={handleToggleSpeech}
                  title={
                    speaking
                      ? "Stop speaking (or press ⌘⇧S)"
                      : "Speak this message aloud (⌘⇧S)"
                  }
                >
                  {speaking ? "⏹ Stop" : "🔊 Speak"}
                </button>
              )}
              <button
                className="copy-btn"
                onClick={handleCopy}
                title="Copy message text"
              >
                {copied ? "Copied" : "Copy"}
              </button>
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
        <ToolCalls calls={message.tool_calls} />
      </div>
    </div>
  );
}
