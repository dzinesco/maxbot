// v3.7.14 — VoiceMode. The click-to-start / click-to-stop
// mic in the chat header. Sits next to the existing
// TTSToolbar (v3.7.13) which speaks the LLM response;
// VoiceToolbar captures the user's voice and dispatches
// the transcript as a regular user message.
//
// Why a separate component from the existing
// `VoiceButton` (v2.7.0, hold-to-record, in the Composer)?
// The two serve different moments in the conversation:
//
//   • `VoiceButton` is for "I'm typing a long message, I
//     just want to dictate a draft to review" — hold to
//     record, transcript drops into the textarea, user
//     edits and sends. Stays in the Composer.
//
//   • `VoiceToolbar` is for "I'm in the middle of a
//     voice-first back-and-forth with the Bot" — click
//     to start, click to stop, transcript auto-sends. The
//     UX-5 collapsed per-bubble chrome to a single header
//     toolbar; the mic logically belongs there next to
//     the Speak / Copy buttons.
//
// MediaRecorder lifecycle mirrors VoiceButton: getUserMedia
// → MediaRecorder → onstop → arrayBuffer → base64 →
// audioToText → onSend(text). The base64 step is unique
// to this toolbar because the new `audio_to_text` Tauri
// command takes a base64 string over the JSON IPC bus.
//
// The level meter reads `AnalyserNode.getByteFrequencyData`
// on a `requestAnimationFrame` loop. The meter is a thin
// horizontal bar that fills left-to-right; when there's no
// signal the bar drains to zero. The pulse-dot is a CSS
// animation, not a JS interval — cheaper and respects
// `prefers-reduced-motion`.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { audioToText } from "../lib/tauri";

interface VoiceToolbarProps {
  /** Auto-dispatch the recognized transcript as a user
   *  message. The parent's `onSend` already handles the
   *  full requestId / state pipeline; the toolbar just
   *  hands the text over. */
  onSend: (text: string) => void;
  /** Optional error sink. When unset, errors are
   *  console.warn'd. The parent usually wires this to the
   *  same toast surface used for chat errors. */
  onError?: (message: string) => void;
  /** Disabled state. The button renders a muted icon and
   *  ignores pointer events when true. Used while the
   *  LLM is streaming (we don't want to capture audio
   *  during an in-flight run). */
  disabled?: boolean;
}

/** Best-effort MIME-type negotiation. Same candidates as
 *  the existing `VoiceButton` — webm/opus first, ogg,
 *  mp4 fallback, no specific codec. Whisper accepts
 *  every one of these as a multipart `file` upload. */
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
      // some browsers throw on isTypeSupported for
      // unknown codecs; swallow and move on
    }
  }
  return undefined;
}

/** `Uint8Array` → standard-base64 string, chunked to
 *  avoid blowing the JS call-stack on large buffers.
 *  `btoa(String.fromCharCode(...bytes))` works for small
 *  blobs but throws "Maximum call stack size exceeded"
 *  on 100+ KB audio. The chunked version handles
 *  arbitrary lengths. */
function bytesToBase64(bytes: Uint8Array): string {
  if (typeof btoa === "function") {
    // happy-dom and modern browsers ship `btoa`; the
    // chunked loop is a 1:1 substitute for the spread
    // idiom and is safe up to 100+ MB.
    let binary = "";
    const chunk = 0x8000;
    for (let i = 0; i < bytes.length; i += chunk) {
      binary += String.fromCharCode(
        ...bytes.subarray(i, Math.min(i + chunk, bytes.length)),
      );
    }
    return btoa(binary);
  }
  // No `btoa` (rare). Fall back to per-byte hex — slower
  // but always works.
  let hex = "";
  for (let i = 0; i < bytes.length; i++) {
    hex += bytes[i].toString(16).padStart(2, "0");
  }
  return hex;
}

export function VoiceToolbar({
  onSend,
  onError,
  disabled = false,
}: VoiceToolbarProps) {
  const [recording, setRecording] = useState(false);
  const [transcribing, setTranscribing] = useState(false);
  // v3.7.16 smoothness: the level meter is a thin
  // bar whose width is the only thing that changes at
  // 60fps during recording. We used to drive it with
  // a React `useState` and a setLevel(pct) on every
  // RAF tick, which re-rendered the entire
  // VoiceToolbar at 60fps. Now we drive it with a
  // direct DOM mutation: the RAF tick writes to
  // `fillRef.current.style.width` and React never
  // re-renders. The `level` state was removed; the
  // `aria-valuenow` is also handled via a ref so the
  // accessibility tree stays in sync.
  const fillRef = useRef<HTMLDivElement | null>(null);
  const meterRef = useRef<HTMLDivElement | null>(null);

  // Mutable refs for the in-flight capture. Putting the
  // MediaStream / MediaRecorder / AudioContext into
  // React state would tear them down on re-render.
  const streamRef = useRef<MediaStream | null>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const mimeTypeRef = useRef<string | undefined>(undefined);
  // Level-meter plumbing. `rafRef` holds the
  // requestAnimationFrame handle so we can cancel it on
  // stop / unmount. `analyserRef` / `audioCtxRef` live
  // until the next start() to avoid re-creating them
  // mid-session.
  const rafRef = useRef<number | null>(null);
  const audioCtxRef = useRef<AudioContext | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);

  // Keep the latest onSend / onError in a ref so the
  // stop handler (registered once) can call them without
  // re-binding on every parent render.
  const onSendRef = useRef(onSend);
  const onErrorRef = useRef(onError);
  useEffect(() => {
    onSendRef.current = onSend;
    onErrorRef.current = onError;
  }, [onSend, onError]);

  // Stop the level-meter loop and tear down the
  // AudioContext. Safe to call multiple times.
  const stopMeter = useCallback(() => {
    if (rafRef.current !== null) {
      cancelAnimationFrame(rafRef.current);
      rafRef.current = null;
    }
    if (audioCtxRef.current) {
      try {
        audioCtxRef.current.close();
      } catch {
        // ignore — close() throws if already closed
      }
      audioCtxRef.current = null;
    }
    analyserRef.current = null;
    // v3.7.16: zero the meter via direct DOM
    // mutation, not React state. No re-render.
    if (fillRef.current) {
      fillRef.current.style.width = "0%";
    }
    if (meterRef.current) {
      meterRef.current.setAttribute("aria-valuenow", "0");
    }
  }, []);

  const start = useCallback(async () => {
    if (recording || transcribing || disabled) return;
    if (
      typeof navigator === "undefined" ||
      !navigator.mediaDevices?.getUserMedia
    ) {
      onErrorRef.current?.(
        "Microphone access isn't available in this context.",
      );
      return;
    }
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (e) {
      onErrorRef.current?.(
        e instanceof Error ? e.message : "Microphone permission denied",
      );
      return;
    }
    streamRef.current = stream;
    chunksRef.current = [];
    const mimeType = pickRecorderMime();
    mimeTypeRef.current = mimeType;
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
      // Tear down the stream + meter.
      const stream = streamRef.current;
      streamRef.current = null;
      recorderRef.current = null;
      stream?.getTracks().forEach((t) => t.stop());
      stopMeter();
      const blob = new Blob(chunksRef.current, {
        type: mimeTypeRef.current ?? "audio/webm",
      });
      chunksRef.current = [];
      if (blob.size === 0) {
        onErrorRef.current?.("Recording was empty — try again.");
        return;
      }
      setTranscribing(true);
      try {
        const buf = new Uint8Array(await blob.arrayBuffer());
        const base64 = bytesToBase64(buf);
        const result = await audioToText(base64, blob.type);
        const text = (result.text ?? "").trim();
        if (!text) {
          onErrorRef.current?.(
            "VoiceMode: empty transcript. Try again with a clearer sentence.",
          );
          return;
        }
        onSendRef.current?.(text);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        onErrorRef.current?.(`VoiceMode error: ${msg}`);
      } finally {
        setTranscribing(false);
      }
    };
    recorder.onerror = (ev) => {
      const err = (ev as unknown as { error?: Error }).error;
      onErrorRef.current?.(
        err?.message || "Microphone recording error",
      );
      try {
        recorder.stop();
      } catch {
        // ignore
      }
      const stream = streamRef.current;
      streamRef.current = null;
      recorderRef.current = null;
      stream?.getTracks().forEach((t) => t.stop());
      stopMeter();
      setRecording(false);
    };

    // Wire the level meter. We use the
    // `AudioContext` + `MediaStreamSource` +
    // `AnalyserNode` path instead of polling
    // `MediaRecorder` events because we want a
    // visual signal even when the user is talking
    // quietly.
    try {
      const AudioCtor: typeof AudioContext | undefined =
        // The TS lib doesn't ship the AudioContext
        // webkit-prefixed alias; happy-dom and
        // macOS WKWebView do.
        (window as unknown as { webkitAudioContext?: typeof AudioContext })
          .webkitAudioContext ?? window.AudioContext;
      if (AudioCtor) {
        const audioCtx = new AudioCtor();
        const source = audioCtx.createMediaStreamSource(stream);
        const analyser = audioCtx.createAnalyser();
        analyser.fftSize = 256;
        source.connect(analyser);
        audioCtxRef.current = audioCtx;
        analyserRef.current = analyser;
        const data = new Uint8Array(analyser.frequencyBinCount);
        const tick = () => {
          const a = analyserRef.current;
          if (!a) return;
          a.getByteFrequencyData(data);
          // RMS-ish over the spectrum. The 0-255 byte
          // values map to 0-1; the multiplier is a
          // perceptual scale that keeps speech in the
          // 20-80% range without saturating on loud
          // input.
          let sum = 0;
          for (let i = 0; i < data.length; i++) {
            const v = data[i] / 255;
            sum += v * v;
          }
          const rms = Math.sqrt(sum / data.length);
          const pct = Math.min(100, Math.round(rms * 180));
          // v3.7.16 smoothness: write the meter
          // width directly to the DOM instead of
          // driving a React state. The setLevel(pct)
          // path caused 60 re-renders per second of
          // the whole VoiceToolbar; the direct write
          // touches only the meter fill (a single
          // CSS property) and the aria attribute.
          if (fillRef.current) {
            fillRef.current.style.width = `${pct}%`;
          }
          if (meterRef.current) {
            meterRef.current.setAttribute(
              "aria-valuenow",
              String(pct),
            );
          }
          rafRef.current = requestAnimationFrame(tick);
        };
        rafRef.current = requestAnimationFrame(tick);
      }
    } catch (e) {
      // Level meter is best-effort. The mic still works
      // without it; we just don't draw a meter.
      // eslint-disable-next-line no-console
      console.warn("voice-toolbar: level meter init failed", e);
    }

    try {
      recorder.start();
      setRecording(true);
    } catch (e) {
      onErrorRef.current?.(
        e instanceof Error ? e.message : "Failed to start recording",
      );
      stream.getTracks().forEach((t) => t.stop());
      streamRef.current = null;
      recorderRef.current = null;
      stopMeter();
    }
  }, [recording, transcribing, disabled, stopMeter]);

  const stop = useCallback(() => {
    const recorder = recorderRef.current;
    if (!recorder) {
      setRecording(false);
      return;
    }
    setRecording(false);
    try {
      if (recorder.state !== "inactive") recorder.stop();
    } catch {
      // already stopped; safe to ignore
    }
  }, []);

  // If the component unmounts mid-recording (parent
  // navigation, modal close), make sure we don't leak
  // the mic or a half-built MediaRecorder.
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
      stopMeter();
    };
  }, [stopMeter]);

  const onClick = () => {
    if (disabled || transcribing) return;
    if (recording) stop();
    else start();
  };

  const buttonLabel = transcribing
    ? "Transcribing…"
    : recording
      ? "Stop recording"
      : "Start voice message";

  return (
    <div
      className="voice-toolbar"
      data-testid="voice-toolbar"
      data-recording={recording ? "true" : "false"}
      data-busy={transcribing ? "true" : "false"}
    >
      {recording && (
        <div
          className="voice-toolbar__recording"
          data-testid="voice-toolbar-listening"
        >
          <span className="voice-toolbar__pulse" aria-hidden />
          <span className="voice-toolbar__label">Listening…</span>
          <div
            ref={meterRef}
            className="voice-toolbar__meter"
            data-testid="voice-toolbar-meter"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={0}
          >
            <div
              ref={fillRef}
              className="voice-toolbar__meter-fill"
              style={{ width: "0%" }}
            />
          </div>
        </div>
      )}
      {transcribing && !recording && (
        <span
          className="voice-toolbar__transcribing"
          data-testid="voice-toolbar-transcribing"
        >
          Transcribing…
        </span>
      )}
      <button
        type="button"
        className={`voice-toolbar__btn${recording ? " recording" : ""}${
          transcribing ? " busy" : ""
        }`}
        onClick={onClick}
        disabled={disabled || transcribing}
        title={buttonLabel}
        aria-label={buttonLabel}
        data-testid="voice-toolbar-mic"
      >
        {transcribing ? "…" : recording ? "⏹" : "🎤"}
      </button>
    </div>
  );
}
