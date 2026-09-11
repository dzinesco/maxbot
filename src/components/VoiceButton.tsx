// v2.7.0 — Voice (bidirectional).
//
// Hold-to-talk mic button that ships audio to the STT
// endpoint via the Rust `transcribe_audio` command and
// feeds the transcript back into the parent (the Composer)
// so the user can review before sending. The STT
// provider is MiniMax `asr-1.0` per v3.7.15; the
// v2.7.0–v3.7.14 path was OpenAI Whisper.
//
// Why MediaRecorder in the webview instead of a Tauri mic
// plugin? The webview inherits the system TCC for microphone
// access — calling `getUserMedia({audio: true})` triggers
// the same permission prompt a regular browser tab would. A
// separate Tauri plugin would add a dep, a second permission
// surface, and a serialization round-trip for the audio
// buffer, all for the same outcome. The browser API is the
// simplest path that works.
//
// Visual:
//   idle      → 🎤
//   recording → ● REC (pulsing red dot via CSS)

import {
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { transcribeAudio } from "../lib/tauri";

interface VoiceButtonProps {
  /** Called with the recognized transcript. The parent is
   *  responsible for either filling its own input or
   *  auto-sending; the button never sends on the user's
   *  behalf. Keeps the button reusable outside the
   *  Composer (e.g. a future dictation toggle on the
   *  Bot editor). */
  onTranscript: (text: string) => void;
  /** Optional error sink. When unset, errors are silently
   *  dropped. The Composer wires this to a toast. */
  onError?: (message: string) => void;
  /** Disabled state. The button renders a muted icon and
   *  ignores pointer events when true. */
  disabled?: boolean;
}

/** Best-effort wrapper around `MediaRecorder`. Returns the
 * preferred MIME type for the user's runtime (webm/opus on
 * Chromium, ogg on Firefox). The wrapper falls back to the
 * default if the preference isn't supported. We don't
 * negotiate a specific codec beyond that — the MiniMax
 * `asr-1.0` endpoint (was OpenAI Whisper in v3.7.14 and
 * earlier) accepts both webm and ogg as multipart `file`
 * uploads. */
function pickRecorderMime(): string | undefined {
  if (typeof MediaRecorder === "undefined") return undefined;
  const candidates = [
    "audio/webm;codecs=opus",
    "audio/webm",
    "audio/ogg;codecs=opus",
    "audio/mp4",
  ];
  for (const c of candidates) {
    try {
      if (MediaRecorder.isTypeSupported(c)) return c;
    } catch {
      // some browsers throw on isTypeSupported for unknown
      // codecs; swallow and move on
    }
  }
  return undefined;
}

export function VoiceButton({
  onTranscript,
  onError,
  disabled = false,
}: VoiceButtonProps) {
  const [recording, setRecording] = useState(false);
  const [busy, setBusy] = useState(false);
  // Mutable refs for the in-flight capture. We intentionally
  // avoid putting the MediaStream / MediaRecorder into React
  // state — they're not serializable, and re-renders would
  // tear them down.
  const streamRef = useRef<MediaStream | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  // Keep the latest onTranscript / onError in a ref so the
  // stop handler (registered once) can call them without
  // re-binding on every parent render.
  const onTranscriptRef = useRef(onTranscript);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onTranscriptRef.current = onTranscript;
    onErrorRef.current = onError;
  }, [onTranscript, onError]);

  const start = useCallback(async () => {
    if (recording || busy || disabled) return;
    if (typeof navigator === "undefined" || !navigator.mediaDevices?.getUserMedia) {
      onErrorRef.current?.(
        "Microphone access isn't available in this context.",
      );
      return;
    }
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (e) {
      // Permission denial + secure-context failures land
      // here. Surface a useful message instead of bubbling
      // the raw DOMException.
      onErrorRef.current?.(
        e instanceof Error ? e.message : "Microphone permission denied",
      );
      return;
    }
    streamRef.current = stream;
    chunksRef.current = [];
    const mimeType = pickRecorderMime();
    const recorder = mimeType
      ? new MediaRecorder(stream, { mimeType })
      : new MediaRecorder(stream);
    recorderRef.current = recorder;
    recorder.ondataavailable = (ev) => {
      if (ev.data && ev.data.size > 0) {
        chunksRef.current.push(ev.data);
      }
    };
    recorder.onstop = async () => {
      // Tear down the stream — holding the mic open after
      // the user releases is a clear TCC smell.
      const stream = streamRef.current;
      streamRef.current = null;
      recorderRef.current = null;
      stream?.getTracks().forEach((t) => t.stop());
      const blob = new Blob(chunksRef.current, {
        type: mimeType ?? "audio/webm",
      });
      chunksRef.current = [];
      if (blob.size === 0) {
        onErrorRef.current?.("Recording was empty — try again.");
        return;
      }
      setBusy(true);
      try {
        const buf = new Uint8Array(await blob.arrayBuffer());
        const text = await transcribeAudio(buf, blob.type);
        onTranscriptRef.current?.(text);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        onErrorRef.current?.(`STT failed: ${msg}`);
      } finally {
        setBusy(false);
      }
    };
    recorder.onerror = (ev) => {
      // MediaRecorder fires its own `error` event on
      // hardware failures; surface the name so the user
      // can tell us what happened.
      const err = (ev as unknown as { error?: Error }).error;
      onErrorRef.current?.(
        err?.message || "Microphone recording error",
      );
      // Best-effort cleanup
      try { recorder.stop(); } catch { /* ignore */ }
      const stream = streamRef.current;
      streamRef.current = null;
      recorderRef.current = null;
      stream?.getTracks().forEach((t) => t.stop());
      setRecording(false);
    };
    try {
      recorder.start();
      setRecording(true);
    } catch (e) {
      onErrorRef.current?.(
        e instanceof Error ? e.message : "Failed to start recording",
      );
      // Clean up the stream we just acquired
      stream.getTracks().forEach((t) => t.stop());
      streamRef.current = null;
      recorderRef.current = null;
    }
  }, [recording, busy, disabled]);

  const stop = useCallback(() => {
    const recorder = recorderRef.current;
    if (!recorder) {
      // The user released before the click registered —
      // e.g. fast taps on touch — so the recorder may not
      // be set yet. The start() call guards against this
      // by checking `recording`, but the cleanup is also
      // safe: if there's no recorder, there's nothing to
      // stop.
      setRecording(false);
      return;
    }
    // Recorder will fire onstop → onTranscript. We clear
    // the `recording` flag now so the UI snaps back to
    // idle even though the STT call is still in flight.
    setRecording(false);
    try {
      if (recorder.state !== "inactive") recorder.stop();
    } catch {
      // already stopped; safe to ignore
    }
  }, []);

  // If the component unmounts mid-recording (parent
  // navigation, modal close), make sure we don't leak the
  // mic or a half-built MediaRecorder.
  useEffect(() => {
    return () => {
      const recorder = recorderRef.current;
      const stream = streamRef.current;
      recorderRef.current = null;
      streamRef.current = null;
      try {
        if (recorder && recorder.state !== "inactive") recorder.stop();
      } catch {
        // ignore
      }
      stream?.getTracks().forEach((t) => t.stop());
    };
  }, []);

  // Keyboard accessibility: holding Space while the
  // button is focused starts/stops recording. Mousedown /
  // mouseup on the button itself also works, of course.
  const onKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      if (!recording) start();
    }
  };
  const onKeyUp = (e: React.KeyboardEvent<HTMLButtonElement>) => {
    if (e.key === " " || e.key === "Enter") {
      e.preventDefault();
      if (recording) stop();
    }
  };

  return (
    <button
      type="button"
      className={`voice-btn${recording ? " recording" : ""}${busy ? " busy" : ""}`}
      disabled={disabled || busy}
      onMouseDown={(e) => {
        e.preventDefault();
        start();
      }}
      onMouseUp={(e) => {
        e.preventDefault();
        stop();
      }}
      onMouseLeave={() => {
        // Don't auto-stop on leave — the user might have
        // dragged off the button briefly. mousedown/mouseup
        // pairing on the same button is what governs the
        // hold gesture. (touchend still fires if they
        // release off-target.)
        if (recording) stop();
      }}
      onTouchStart={(e) => {
        e.preventDefault();
        start();
      }}
      onTouchEnd={(e) => {
        e.preventDefault();
        stop();
      }}
      onKeyDown={onKeyDown}
      onKeyUp={onKeyUp}
      title={
        recording
          ? "Release to transcribe"
          : busy
            ? "Transcribing…"
            : "Hold to record"
      }
      data-testid="voice-button"
      data-recording={recording ? "true" : "false"}
    >
      {recording ? "● REC" : busy ? "…" : "🎤"}
    </button>
  );
}
