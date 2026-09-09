import { useEffect, useRef, useState } from "react";
import type { Message, PersistedToolCall } from "../lib/api";
import { ttsSpeak, ttsStop } from "../lib/tauri";

interface MessageBubbleProps {
  message: Message;
  streaming?: boolean;
}

/** Very small markdown subset: paragraphs, fenced code blocks, inline code,
 * and links. Enough to make MiniMax responses readable without pulling in a
 * 200KB markdown library. Anything else renders as plain text.
 */
function renderMarkdown(input: string): React.ReactNode {
  const nodes: React.ReactNode[] = [];
  const lines = input.split("\n");
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
    <details className="tool-calls">
      <summary>Tool calls ({calls.length})</summary>
      {calls.map((tc) => (
        <pre key={tc.id}>
          {tc.name}({tc.arguments || "{}"})
        </pre>
      ))}
    </details>
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
  return (
    <div
      className={`message ${message.role}${streaming ? " streaming" : ""}`}
    >
      <div className="avatar">
        {isUser ? "You" : isTool ? "T" : "M"}
      </div>
      <div className="body">
        <div className="meta">
          {isUser ? "You" : isTool ? "Tool" : "MaxBot"}
          {!streaming && message.content && (
            <>
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
            </>
          )}
        </div>
        <div className="content">{renderMarkdown(message.content)}</div>
        <ToolCalls calls={message.tool_calls} />
      </div>
    </div>
  );
}
