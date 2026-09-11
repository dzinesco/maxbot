// v3.7.14 — ChatView tests.
//
// The brief's required test sequence is:
//
//   1. VoiceToolbar mounts (the test renders ChatView and
//      asserts the toolbar's testid is in the document).
//   2. Click the mic → recording starts.
//   3. Click the mic again → recording stops.
//   4. The audio is sent to `audio_to_text`.
//   5. The result is dispatched as a user message via the
//      parent's `onSend` callback.
//
// We render ChatView with a minimal message set and the
// `onSend` callback captured into a `vi.fn()`. The
// `audio_to_text` mock returns a canned transcript;
// after the second click we assert the callback fired
// with that string.
//
// happy-dom doesn't ship MediaRecorder / AudioContext
// / getUserMedia, so the VoiceToolbar's internal polyfill
// (installed via `vi.mock` at module-load time) takes
// over. The mock recorder fires `dataavailable` + `onstop`
// on `stop()` so the toolbar chains the STT call without
// any user action beyond the two clicks.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { ChatView } from "./ChatView";
import type { Bot, Message } from "../lib/api";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

// In-memory MediaRecorder fake. Mirrors the pattern from
// `VoiceButton.test.tsx` and `VoiceToolbar.test.tsx`.
class MockMediaRecorder {
  static isTypeSupported(_mime: string) {
    return true;
  }
  state: "inactive" | "recording" = "inactive";
  ondataavailable: ((ev: { data: Blob }) => void) | null = null;
  onstop: (() => void) | null = null;
  onerror: ((ev: unknown) => void) | null = null;
  mimeType: string;
  constructor(
    public stream: MockMediaStream,
    options?: { mimeType?: string },
  ) {
    this.mimeType = options?.mimeType ?? "audio/webm";
  }
  start() {
    this.state = "recording";
  }
  stop() {
    if (this.state === "inactive") return;
    this.state = "inactive";
    if (this.ondataavailable) {
      this.ondataavailable({
        data: new Blob([new Uint8Array([1, 2, 3, 4])], {
          type: this.mimeType,
        }),
      });
    }
    if (this.onstop) this.onstop();
  }
}

class MockMediaStreamTrack {
  stop = vi.fn();
  readyState = "live" as const;
}

class MockMediaStream {
  getTracks() {
    return [new MockMediaStreamTrack()];
  }
}

class MockAnalyserNode {
  frequencyBinCount = 8;
  fftSize = 16;
  getByteFrequencyData(_: Uint8Array) {
    // leave the buffer at zero so the meter reads
    // "idle" in the test
  }
}

class MockMediaStreamSource {
  connect(_: unknown) {
    // no-op
  }
}

class MockAudioContext {
  state = "running" as const;
  createMediaStreamSource(_: MockMediaStream) {
    return new MockMediaStreamSource();
  }
  createAnalyser() {
    return new MockAnalyserNode();
  }
  close() {
    // no-op
  }
}

let getUserMediaMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeMock.mockReset();
  getUserMediaMock = vi.fn().mockResolvedValue(new MockMediaStream());
  Object.defineProperty(global.navigator, "mediaDevices", {
    configurable: true,
    value: { getUserMedia: getUserMediaMock },
  });
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).MediaRecorder = MockMediaRecorder;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).AudioContext = MockAudioContext;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).requestAnimationFrame = (() => 1) as any;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).cancelAnimationFrame = (() => undefined) as any;
});

afterEach(() => {
  cleanup();
});

const sampleBot: Bot = {
  id: "bot-1",
  name: "Robot",
  description: "",
  system_prompt: "",
  default_model: "",
  allowed_tools: [],
  icon: "🤖",
  color: "#7c5cff",
  state: "idle",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

const sampleMessages: Message[] = [
  {
    id: "u1",
    conversation_id: "c1",
    role: "user",
    content: "hello",
    tool_calls: [],
    created_at: "2026-01-01T00:00:00Z",
    error_message: null,
  },
  {
    id: "a1",
    conversation_id: "c1",
    role: "assistant",
    content: "Hi, how can I help?",
    tool_calls: [],
    created_at: "2026-01-01T00:00:01Z",
    error_message: null,
  },
];

describe("ChatView — v3.7.14 VoiceMode mount", () => {
  it("renders the VoiceToolbar alongside the TTSToolbar in the messages header", () => {
    const onSend = vi.fn();
    render(
      <ChatView
        messages={sampleMessages}
        streamingId={null}
        activeBot={sampleBot}
        onSend={onSend}
      />,
    );
    // The TTSToolbar (v3.7.13) is still present.
    expect(screen.getByTestId("tts-toolbar")).toBeInTheDocument();
    // The VoiceToolbar (v3.7.14) is mounted next to it.
    expect(screen.getByTestId("voice-toolbar")).toBeInTheDocument();
    expect(screen.getByTestId("voice-toolbar-mic")).toBeInTheDocument();
    // The header wrapper exists.
    expect(screen.getByTestId("messages-header")).toBeInTheDocument();
  });

  it("does NOT mount the VoiceToolbar when onSend is missing (back-compat)", () => {
    render(
      <ChatView
        messages={sampleMessages}
        streamingId={null}
        activeBot={sampleBot}
      />,
    );
    // The TTSToolbar still renders.
    expect(screen.getByTestId("tts-toolbar")).toBeInTheDocument();
    // The VoiceToolbar is hidden.
    expect(screen.queryByTestId("voice-toolbar")).not.toBeInTheDocument();
  });
});

describe("ChatView — v3.7.14 VoiceMode end-to-end click flow", () => {
  it(
    "click mic → recording → click mic → audio_to_text → onSend fires with transcript",
    async () => {
      invokeMock.mockImplementation((cmd: string) => {
        if (cmd === "audio_to_text") {
          return Promise.resolve({ text: "draft an email please" });
        }
        return Promise.resolve(null);
      });
      const onSend = vi.fn();
      render(
        <ChatView
          messages={sampleMessages}
          streamingId={null}
          activeBot={sampleBot}
          onSend={onSend}
        />,
      );
      const mic = screen.getByTestId("voice-toolbar-mic");
      // First click → recording starts.
      fireEvent.click(mic);
      await waitFor(() => {
        expect(getUserMediaMock).toHaveBeenCalledWith({ audio: true });
      });
      // The "Listening…" pill appears.
      await waitFor(() => {
        expect(
          screen.getByTestId("voice-toolbar-listening"),
        ).toBeInTheDocument();
      });
      // Second click → recording stops → STT call fires.
      fireEvent.click(mic);
      await waitFor(() => {
        expect(invokeMock).toHaveBeenCalledWith(
          "audio_to_text",
          expect.objectContaining({
            mimeType: expect.stringMatching(/^audio\//),
            audioBase64: expect.any(String),
          }),
        );
      });
      // The transcript is dispatched as a user message.
      await waitFor(() => {
        expect(onSend).toHaveBeenCalledWith("draft an email please");
      });
    },
  );
});
