import type { Message, PersistedToolCall } from "../lib/api";

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
        </div>
        <div className="content">{renderMarkdown(message.content)}</div>
        <ToolCalls calls={message.tool_calls} />
      </div>
    </div>
  );
}
