// v2.7.0 — Voice (bidirectional) component tests.
//
// The button uses `navigator.mediaDevices.getUserMedia` and the
// `MediaRecorder` API — both of which jsdom doesn't ship. We
// polyfill the minimum surface the component touches so we can
// verify the user gesture → getUserMedia → MediaRecorder.start
// → mouseup → MediaRecorder.stop → dataavailable → transcribe_audio
// flow without an actual browser.
//
// The mock recorder doesn't actually produce audio — we synthesize
// a one-byte Blob on `stop()` so the component's "empty buffer"
// guard doesn't fire.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { VoiceButton } from "./VoiceButton";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

// In-memory MediaRecorder fake. Each `start()` resets
// `state` to "recording"; `stop()` flips to "inactive"
// and fires a `dataavailable` event with a one-byte Blob
// before calling `onstop`. The component's onstop handler
// is responsible for chaining the rest of the pipeline.
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
    // Synthesize a non-empty chunk so the component's
    // "recording was empty" guard doesn't fire.
    if (this.ondataavailable) {
      this.ondataavailable({ data: new Blob([new Uint8Array([1])], { type: this.mimeType }) });
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

let getUserMediaMock: ReturnType<typeof vi.fn>;
let lastRecorder: MockMediaRecorder | null = null;

beforeEach(() => {
  invokeMock.mockReset();
  lastRecorder = null;
  getUserMediaMock = vi.fn().mockResolvedValue(new MockMediaStream());
  // jsdom doesn't expose navigator.mediaDevices by default;
  // patch it onto the global navigator for the duration of
  // the test. Each test installs a fresh mock so calls don't
  // leak between tests.
  Object.defineProperty(global.navigator, "mediaDevices", {
    configurable: true,
    value: {
      getUserMedia: getUserMediaMock,
    },
  });
  // Stub MediaRecorder on the global scope. The component
  // references it as a bare identifier (`new MediaRecorder(...)`)
  // so it must live on `globalThis`.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  (globalThis as any).MediaRecorder = class extends MockMediaRecorder {
    constructor(stream: MockMediaStream, options?: { mimeType?: string }) {
      super(stream, options);
      lastRecorder = this;
    }
  };
});

afterEach(() => {
  cleanup();
  // Tear down the navigator patch so unrelated tests aren't
  // affected. (jsdom restores the original navigator between
  // tests on its own, but we set `configurable: true` so this
  // is safe.)
});

describe("VoiceButton", () => {
  it("calls getUserMedia on mousedown", async () => {
    render(<VoiceButton onTranscript={vi.fn()} />);
    const button = screen.getByTestId("voice-button");
    fireEvent.mouseDown(button);
    await waitFor(() => {
      expect(getUserMediaMock).toHaveBeenCalledWith({ audio: true });
    });
  });

  it("starts the MediaRecorder and flips the visual state to recording", async () => {
    render(<VoiceButton onTranscript={vi.fn()} />);
    const button = screen.getByTestId("voice-button");
    fireEvent.mouseDown(button);
    await waitFor(() => {
      expect(lastRecorder).not.toBeNull();
    });
    expect(lastRecorder!.state).toBe("recording");
    expect(button.getAttribute("data-recording")).toBe("true");
  });

  it("calls transcribe_audio with the recorded blob on mouseup", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "transcribe_audio") {
        return Promise.resolve("hello from whisper");
      }
      return Promise.resolve(null);
    });
    const onTranscript = vi.fn();
    render(<VoiceButton onTranscript={onTranscript} />);
    const button = screen.getByTestId("voice-button");
    fireEvent.mouseDown(button);
    await waitFor(() => expect(lastRecorder).not.toBeNull());
    fireEvent.mouseUp(button);
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith(
        "transcribe_audio",
        expect.objectContaining({
          mimeType: expect.stringMatching(/^audio\//),
        }),
      );
    });
    await waitFor(() => {
      expect(onTranscript).toHaveBeenCalledWith("hello from whisper");
    });
  });

  it("surfaces STT errors via onError instead of swallowing them", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "transcribe_audio") {
        return Promise.reject(new Error("network down"));
      }
      return Promise.resolve(null);
    });
    const onError = vi.fn();
    const onTranscript = vi.fn();
    render(
      <VoiceButton onTranscript={onTranscript} onError={onError} />,
    );
    const button = screen.getByTestId("voice-button");
    fireEvent.mouseDown(button);
    await waitFor(() => expect(lastRecorder).not.toBeNull());
    fireEvent.mouseUp(button);
    await waitFor(() => {
      expect(onError).toHaveBeenCalled();
    });
    const msg = onError.mock.calls[0][0] as string;
    expect(msg).toMatch(/STT failed/);
    expect(msg).toMatch(/network down/);
    expect(onTranscript).not.toHaveBeenCalled();
  });

  it("surfaces microphone permission denial as an error", async () => {
    getUserMediaMock.mockRejectedValueOnce(
      new Error("Permission denied"),
    );
    const onError = vi.fn();
    render(
      <VoiceButton onTranscript={vi.fn()} onError={onError} />,
    );
    const button = screen.getByTestId("voice-button");
    fireEvent.mouseDown(button);
    await waitFor(() => {
      expect(onError).toHaveBeenCalledWith("Permission denied");
    });
  });
});
