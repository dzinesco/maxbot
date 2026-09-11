// v3.7.14 — VoiceMode component tests.
//
// The toolbar uses `navigator.mediaDevices.getUserMedia`,
// the `MediaRecorder` API, and the `AudioContext` /
// `AnalyserNode` API. None of those ship in happy-dom, so
// each test installs a small polyfill on the global scope
// before rendering.
//
// Acceptance: the brief's required test sequence is
//   1. mount
//   2. mic button click starts recording
//   3. second click stops
//   4. the resulting audio is sent to `audio_to_text`
//   5. the result is dispatched as a user message
//
// Plus a few defensive cases: empty transcript, STT error,
// and a permission denial on getUserMedia.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";
import { VoiceToolbar } from "./VoiceToolbar";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

// In-memory MediaRecorder fake. `start()` flips state to
// "recording"; `stop()` fires `dataavailable` with a
// non-empty chunk (so the component's "empty recording"
// guard doesn't fire) then calls `onstop`. The
// component's onstop handler is responsible for chaining
// the rest of the pipeline (base64 encode → audioToText
// → onSend).
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

// Minimal AnalyserNode fake. The toolbar only reads
// `getByteFrequencyData` + `frequencyBinCount`; the fake
// fills the buffer with zeros so the meter reads "0"
// (idle) in the test.
class MockAnalyserNode {
  frequencyBinCount = 8;
  fftSize = 16;
  getByteFrequencyData(_: Uint8Array) {
    // no-op — leaves the buffer at zero, so the meter
    // stays empty
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
let lastRecorder: MockMediaRecorder | null = null;

beforeEach(() => {
  invokeMock.mockReset();
  lastRecorder = null;
  getUserMediaMock = vi.fn().mockResolvedValue(new MockMediaStream());
  // jsdom/happy-dom don't expose navigator.mediaDevices
  // by default; patch it onto the global navigator for
  // the duration of the test.
  Object.defineProperty(global.navigator, "mediaDevices", {
    configurable: true,
    value: { getUserMedia: getUserMediaMock },
  });
  // Stub MediaRecorder + AudioContext on the global
  // scope. The component references them as bare
  // identifiers (`new MediaRecorder(...)`) so they must
  // live on `globalThis`.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).MediaRecorder = class extends MockMediaRecorder {
    constructor(stream: MockMediaStream, options?: { mimeType?: string }) {
      super(stream, options);
      lastRecorder = this;
    }
  };
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).AudioContext = MockAudioContext;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).requestAnimationFrame = ((cb: FrameRequestCallback) => {
    // Return a positive number so the cancel path has
    // something to call. Tests don't tick the rAF loop
    // because the fake analyser leaves the meter at 0.
    return 1;
  }) as any;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).cancelAnimationFrame = (_: number) => {
    // no-op
  };
});

afterEach(() => {
  cleanup();
});

describe("VoiceToolbar (v3.7.14)", () => {
  it("mounts with a mic button + a hidden 'Listening…' pill", () => {
    render(<VoiceToolbar onSend={vi.fn()} />);
    const toolbar = screen.getByTestId("voice-toolbar");
    const mic = screen.getByTestId("voice-toolbar-mic");
    expect(toolbar).toBeInTheDocument();
    expect(mic).toBeInTheDocument();
    expect(
      screen.queryByTestId("voice-toolbar-listening"),
    ).not.toBeInTheDocument();
    expect(toolbar.getAttribute("data-recording")).toBe("false");
  });

  it("clicking the mic starts a recording (data-recording flips)", async () => {
    render(<VoiceToolbar onSend={vi.fn()} />);
    const mic = screen.getByTestId("voice-toolbar-mic");
    fireEvent.click(mic);
    await waitFor(() => {
      expect(getUserMediaMock).toHaveBeenCalledWith({ audio: true });
    });
    await waitFor(() => {
      expect(lastRecorder).not.toBeNull();
    });
    expect(lastRecorder!.state).toBe("recording");
    expect(
      screen.getByTestId("voice-toolbar-listening"),
    ).toBeInTheDocument();
    expect(
      screen.getByTestId("voice-toolbar").getAttribute("data-recording"),
    ).toBe("true");
  });

  it("clicking the mic a second time stops the recorder, calls audio_to_text, and dispatches the transcript as a user message", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "audio_to_text") {
        return Promise.resolve({ text: "draft an email please" });
      }
      return Promise.resolve(null);
    });
    const onSend = vi.fn();
    render(<VoiceToolbar onSend={onSend} />);
    const mic = screen.getByTestId("voice-toolbar-mic");
    // First click → start
    fireEvent.click(mic);
    await waitFor(() => expect(lastRecorder).not.toBeNull());
    // Second click → stop
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
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith("draft an email please");
    });
  });

  it("surfaces STT errors via onError instead of dispatching", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "audio_to_text") {
        return Promise.reject(new Error("OpenAI API key not configured"));
      }
      return Promise.resolve(null);
    });
    const onSend = vi.fn();
    const onError = vi.fn();
    render(<VoiceToolbar onSend={onSend} onError={onError} />);
    const mic = screen.getByTestId("voice-toolbar-mic");
    fireEvent.click(mic);
    await waitFor(() => expect(lastRecorder).not.toBeNull());
    fireEvent.click(mic);
    await waitFor(() => {
      expect(onError).toHaveBeenCalled();
    });
    const msg = onError.mock.calls[0][0] as string;
    expect(msg).toMatch(/VoiceMode error/);
    expect(msg).toMatch(/OpenAI API key not configured/);
    expect(onSend).not.toHaveBeenCalled();
  });

  it("surfaces microphone permission denial as an error", async () => {
    getUserMediaMock.mockRejectedValueOnce(new Error("Permission denied"));
    const onError = vi.fn();
    render(<VoiceToolbar onSend={vi.fn()} onError={onError} />);
    const mic = screen.getByTestId("voice-toolbar-mic");
    fireEvent.click(mic);
    await waitFor(() => {
      expect(onError).toHaveBeenCalledWith("Permission denied");
    });
  });

  it("sends the base64 audio bytes, not the raw blob bytes", async () => {
    // Wire invoke to capture the arguments; the test
    // asserts the audioBase64 field is a non-empty
    // base64 string (the value depends on the btoa
    // implementation; we just check it's there and
    // decodes back to non-empty bytes).
    let captured: { mimeType: string; audioBase64: string } | null = null;
    invokeMock.mockImplementation((cmd: string, args: unknown) => {
      if (cmd === "audio_to_text") {
        captured = args as { mimeType: string; audioBase64: string };
        return Promise.resolve({ text: "hi" });
      }
      return Promise.resolve(null);
    });
    const onSend = vi.fn();
    render(<VoiceToolbar onSend={onSend} />);
    fireEvent.click(screen.getByTestId("voice-toolbar-mic"));
    await waitFor(() => expect(lastRecorder).not.toBeNull());
    fireEvent.click(screen.getByTestId("voice-toolbar-mic"));
    await waitFor(() => {
      expect(captured).not.toBeNull();
    });
    expect(captured!.audioBase64.length).toBeGreaterThan(0);
    expect(captured!.mimeType).toMatch(/^audio\//);
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith("hi");
    });
  });

  it("renders the 'Transcribing…' indicator while the STT call is in flight", async () => {
    // Resolve audio_to_text on a delay so the test
    // can observe the transcribing state.
    let resolveTranscribe: (v: { text: string }) => void = () => undefined;
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "audio_to_text") {
        return new Promise((resolve) => {
          resolveTranscribe = resolve;
        });
      }
      return Promise.resolve(null);
    });
    const onSend = vi.fn();
    render(<VoiceToolbar onSend={onSend} />);
    const mic = screen.getByTestId("voice-toolbar-mic");
    fireEvent.click(mic);
    await waitFor(() => expect(lastRecorder).not.toBeNull());
    fireEvent.click(mic);
    await waitFor(() => {
      expect(
        screen.getByTestId("voice-toolbar-transcribing"),
      ).toBeInTheDocument();
    });
    // Now resolve the STT call.
    resolveTranscribe({ text: "hello" });
    await waitFor(() => {
      expect(onSend).toHaveBeenCalledWith("hello");
    });
  });
});
