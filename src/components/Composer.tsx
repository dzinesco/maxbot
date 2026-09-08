import { useEffect, useRef, useState } from "react";

interface ComposerProps {
  onSend: (content: string) => void;
  onStop: () => void;
  streaming: boolean;
}

export function Composer({ onSend, onStop, streaming }: ComposerProps) {
  const [value, setValue] = useState("");
  const ref = useRef<HTMLTextAreaElement>(null);

  // Auto-grow the textarea up to a reasonable cap.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(220, el.scrollHeight)}px`;
  }, [value]);

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed || streaming) return;
    onSend(trimmed);
    setValue("");
  };

  return (
    <div className="composer-wrap">
      <div className="composer">
        <textarea
          ref={ref}
          placeholder="Message MaxBot…"
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          rows={1}
        />
        <div className="row">
          <div className="hints">
            <span>Enter to send · Shift+Enter for newline</span>
          </div>
          {streaming ? (
            <button className="send stop" onClick={onStop}>
              Stop
            </button>
          ) : (
            <button
              className="send"
              onClick={submit}
              disabled={!value.trim()}
            >
              Send
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
