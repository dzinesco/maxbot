import { useEffect, useRef, useState } from "react";
import { parseMentions, type MentionMember } from "../lib/mentions";
import { VoiceButton } from "./VoiceButton";

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
  /**
   * v2.4.0 — when set to `"group"`, the Composer
   * parses `@BotName` mentions on send and routes to
   * the matching group members. The placeholder
   * changes and the "Send to bot →" hint is hidden
   * (groups are a separate top-level view). When
   * undefined / `"chat"`, the Composer behaves as
   * before.
   */
  mode?: "chat" | "group";
  /**
   * v2.4.0 — Bot id → Bot lookup table used to
   * resolve mention names. Required when
   * `mode === "group"`; ignored otherwise.
   */
  groupMembers?: MentionMember[];
  /**
   * v2.4.0 — fired when the user sends in group mode.
   * The parent is responsible for the round-trip:
   * `groupSend` first, then a `groupRunTurn` per
   * mentioned Bot. The Composer just hands off the
   * parsed mentions.
   */
  onGroupSend?: (body: string, mentionedBotIds: string[]) => void;
  /**
   * v2.7.0 — error sink for the VoiceButton (e.g.
   * permission denied, STT failure). When omitted,
   * errors are silently dropped. The parent usually
   * wires this to the same toast surface used for
   * chat errors.
   */
  onVoiceError?: (message: string) => void;
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
  mode = "chat",
  groupMembers,
  onGroupSend,
  onVoiceError,
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

  const isGroup = mode === "group";
  const mentions = isGroup && groupMembers
    ? parseMentions(value, groupMembers)
    : [];
  const hasMention = mentions.length > 0;

  const submit = () => {
    const trimmed = value.trim();
    if (!trimmed || streaming) return;
    if (isGroup) {
      // In group mode, only fire when the user mentioned
      // a member. Sending a broadcast would either run
      // every Bot (expensive) or none (silent). v2.4
      // requires explicit routing, so we just no-op
      // when no mentions are present and let the hint
      // below the textarea explain why.
      if (!hasMention) return;
      onGroupSend?.(trimmed, mentions);
      setValue("");
      return;
    }
    onSend(trimmed);
    setValue("");
  };

  // v2.7.0 — Voice (bidirectional). When the
  // VoiceButton returns a transcript, drop it into the
  // textarea so the user can review / edit before
  // sending. We intentionally don't auto-send: voice
  // transcripts are noisy enough that the user almost
  // always wants a second look. The button itself is
  // hidden in group mode (the mention-routing logic
  // below the textarea is too important to compete
  // with for screen real estate).
  const handleVoiceTranscript = (text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    setValue((prev) => (prev.trim() ? `${prev} ${trimmed}` : trimmed));
  };

  return (
    <div className="composer-wrap">
      <div className="composer">
        <textarea
          ref={ref}
          placeholder={
            isGroup
              ? "Message the group… mention a Bot with @BotName"
              : "Message MaxBot…"
          }
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              submit();
            }
          }}
          rows={1}
          data-testid={isGroup ? "group-composer-textarea" : "composer-textarea"}
        />
        <div className="row">
          <div className="hints">
            {isGroup ? (
              hasMention ? (
                <span data-testid="group-composer-hint">
                  Will route to {mentions.length} Bot
                  {mentions.length === 1 ? "" : "s"}
                </span>
              ) : (
                <span data-testid="group-composer-hint">
                  Mention a Bot with @BotName to send to one of the group
                  members.
                </span>
              )
            ) : (
              <>
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
              </>
            )}
          </div>
          <div className="composer-actions">
            {!isGroup && (
              <VoiceButton
                onTranscript={handleVoiceTranscript}
                onError={onVoiceError}
                disabled={streaming}
              />
            )}
            {!isGroup && lastAssistantText && (
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
            {streaming && !isGroup ? (
              <button className="send stop" onClick={onStop}>
                Stop
              </button>
            ) : (
              <button
                className="send"
                onClick={submit}
                disabled={streaming || !value.trim() || (isGroup && !hasMention)}
                data-testid="group-composer-send"
              >
                {streaming ? "Working…" : "Send"}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
