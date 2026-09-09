import { useEffect, useRef, useState } from "react";

interface ComposerProps {
  onSend: (content: string) => void;
  onStop: () => void;
  onOpenSendToBot: () => void;
  hasBots: boolean;
  streaming: boolean;
  /** Text of the most recent assistant message (for the 🔊 button). */
  lastAssistantText: string | null;
  /** True while the assistant-message TTS is in flight. */
  ttsSpeaking: boolean;
  /** Toggle the speak-last-response action. */
  onToggleSpeakLast: () => void;
}

export function Composer({
  onSend,
  onStop,
  onOpenSendToBot,
  hasBots,
  streaming,
  lastAssistantText,
  ttsSpeaking,
  onToggleSpeakLast,
}: ComposerProps) {
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
            {hasBots && (
              <>
                {" · "}
                <button
                  className="link"
                  onClick={onOpenSendToBot}
                  title="Send a message to one of your bots"
                >
                  Send to bot →
                </button>
              </>
            )}
            {" · "}
            <span className="hint-tip">
              🔊 press <kbd>⌘</kbd>+<kbd>⇧</kbd>+<kbd>S</kbd> to speak
              the last response
            </span>
          </div>
          <div className="composer-actions">
            {lastAssistantText && (
              <button
                className={`composer-speak${ttsSpeaking ? " speaking" : ""}`}
                onClick={onToggleSpeakLast}
                title={
                  ttsSpeaking
                    ? "Stop speaking (⌘⇧S)"
                    : "Speak the last response aloud (⌘⇧S)"
                }
              >
                {ttsSpeaking ? "⏹ Stop" : "🔊 Speak"}
              </button>
            )}
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
    </div>
  );
}
