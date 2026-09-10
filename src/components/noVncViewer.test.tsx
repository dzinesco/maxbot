// Component tests for the v3.0.7 noVncViewer.
//
// The RFB class is mocked because happy-dom doesn't ship a
// working `WebSocket` constructor that satisfies noVNC's
// expectations. Each test registers a fresh mock RFB
// constructor and inspects the instance the viewer created.
//
// Coverage:
//   1. NoVncViewer constructs RFB with the given wsUrl
//      (assert via mock RFB instance).
//   2. When `connect` fires but no `desktopname` / `resize`
//      event fires within the timeout, `onError` is called
//      with a "no framebuffer" message.
//   3. When `credentialsrequired` fires and a `credentials`
//      prop is passed, `sendCredentials` is called with the
//      password.
//   4. When `wsUrl` changes, the old RFB is disconnected
//      and a new one is constructed with the new URL.
//
// (v3.0.7) — new test file. The existing ComputerPanel.test.tsx
// covers the panel-level integration; this file covers the
// viewer's RFB lifecycle directly.

import { describe, expect, it, vi, beforeEach, afterEach } from "vitest";
import { act, render } from "@testing-library/react";
import { createRef } from "react";

type Listener = (ev: Event | CustomEvent) => void;

interface MockRfbInstance {
  wsUrl: string;
  listeners: Record<string, Listener[]>;
  scaleViewport: boolean;
  viewOnly: boolean;
  showCursor: boolean;
  background: string;
  disconnected: boolean;
  sendCredentials: ReturnType<typeof vi.fn>;
  addEventListener: (type: string, listener: Listener) => void;
  removeEventListener: (type: string, listener: Listener) => void;
  fire: (type: string, ev?: Event | CustomEvent) => void;
  disconnect: ReturnType<typeof vi.fn>;
}

// `vi.hoisted` runs the factory before the imports and
// before `vi.mock` (which is hoisted to the top). We use
// it to capture state that the mock factory needs to read
// (the latest constructed instance) and that the test
// bodies need to assert against. Plain top-level `let` is
// unreliable here because `vi.mock` factory is hoisted
// and would capture the variables at undefined-initialization
// time.
//
// The RFB constructor must look like a real class to the
// `new` operator — `vi.fn().mockImplementation(buildMockRfb)`
// alone warns that "vi.fn() did not use 'function' or 'class'
// in its implementation" and the `new` call doesn't trigger
// the implementation. We side-step that by writing the mock
// as a real constructor function and using a `Proxy` only to
// keep `RfbMock.mock.calls` populated for assertions.
const { RfbMock, getLastInstance, clearInstances } = vi.hoisted(() => {
  const instances: MockRfbInstance[] = [];
  // The constructor body. We capture `target` / `url` / `options`
  // here and return the assembled instance. JS lets us return
  // any object from a constructor; the `new` operator uses it
  // as the result.
  const RfbMockCtor = function RfbMockCtor(
    this: unknown,
    target: HTMLElement,
    url: string,
    options?: {
      shared?: boolean;
      credentials?: { password?: string };
    },
  ): MockRfbInstance {
    const opts = options ?? {};
    const listeners: Record<string, Listener[]> = {};
    const inst: MockRfbInstance = {
      wsUrl: url,
      listeners,
      scaleViewport: true,
      viewOnly: false,
      showCursor: false,
      background: "",
      disconnected: false,
      sendCredentials: vi.fn(
        ({ password }: { password?: string }) => {
          // Mirror what the real RFB does — stash for
          // inspection.
          (inst as unknown as { lastPassword?: string }).lastPassword =
            password;
        },
      ),
      addEventListener(type, listener) {
        (listeners[type] ||= []).push(listener);
      },
      removeEventListener(type, listener) {
        const arr = listeners[type];
        if (!arr) return;
        const idx = arr.indexOf(listener);
        if (idx >= 0) arr.splice(idx, 1);
      },
      fire(type, ev) {
        const arr = listeners[type] || [];
        for (const l of [...arr]) {
          try {
            l(ev ?? new Event(type));
          } catch {
            // ignore
          }
        }
      },
      disconnect: vi.fn(() => {
        inst.disconnected = true;
      }),
    };
    if (opts.credentials?.password) {
      (inst as unknown as { initialPassword?: string }).initialPassword =
        opts.credentials.password;
    }
    instances.push(inst);
    // The real RFB mutates the target's DOM (injects a div +
    // canvas). happy-dom allows that; just append a placeholder
    // so the cleanup's `while (el.firstChild)` loop has work.
    const placeholder = document.createElement("div");
    placeholder.className = "novnc-canvas-mock";
    target.appendChild(placeholder);
    // Suppress unused-this — the `this` parameter exists so
    // the function looks like a method/constructor to callers
    // and vitest's spy machinery, but the real RFB doesn't
    // read `this` either.
    void this;
    return inst;
  } as unknown as {
    new (
      target: HTMLElement,
      url: string,
      options?: {
        shared?: boolean;
        credentials?: { password?: string };
      },
    ): MockRfbInstance;
    mock: { calls: unknown[][] };
  };
  // The mock is constructed by a real constructor function
  // (`RfbMockCtor` above) so the `new` operator works.
  // The wrapped spy here is what test code holds a
  // reference to — `vi.fn()` gives us `mock.calls` and
  // `mockClear()` for assertions. The proxy's `construct`
  // trap routes `new RfbMock(...)` through the spy so the
  // spy sees the call.
  const spy = vi.fn(
    (
      target: HTMLElement,
      url: string,
      options?: {
        shared?: boolean;
        credentials?: { password?: string };
      },
    ) => {
      return new RfbMockCtor(target, url, options);
    },
  );
  const ProxiedRfbMock = new Proxy(spy, {
    construct(_target, args) {
      return Reflect.apply(
        spy,
        undefined,
        args as [HTMLElement, string, ...unknown[]],
      );
    },
  });
  // The `as unknown as` chain widens the proxied function
  // to expose the spy's `mock` / `mockClear` / `mockReset`
  // methods. The Proxy itself is what the runtime
  // uses for `new RFB(...)` (the `construct` trap), but
  // TypeScript only sees the constructor signature, so
  // we splice the mock methods on here.
  const RfbMockFinal = ProxiedRfbMock as unknown as typeof spy & {
    new (
      target: HTMLElement,
      url: string,
      options?: {
        shared?: boolean;
        credentials?: { password?: string };
      },
    ): MockRfbInstance;
  };
  return {
    RfbMock: RfbMockFinal,
    getLastInstance: () => instances[instances.length - 1] ?? null,
    clearInstances: () => {
      instances.length = 0;
    },
  };
});

vi.mock("@novnc/novnc", () => ({
  default: RfbMock,
}));

// Import after the mock so the component picks it up.
import { NoVncViewer } from "./noVncViewer";

beforeEach(() => {
  RfbMock.mockClear();
  clearInstances();
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

function renderViewer(extraProps: Record<string, unknown> = {}) {
  const ref = createRef<{
    sendKey: (k: number, c: string, d: boolean) => void;
    sendMouse: (x: number, y: number, b: number) => void;
    isConnected: () => boolean;
  }>();
  const onConnect = vi.fn();
  const onDisconnect = vi.fn();
  const onError = vi.fn();
  const result = render(
    <NoVncViewer
      ref={ref}
      wsUrl="ws://localhost:5900/"
      onConnect={onConnect}
      onDisconnect={onDisconnect}
      onError={onError}
      {...extraProps}
    />,
  );
  return { ref, onConnect, onDisconnect, onError, ...result };
}

describe("noVncViewer — RFB construction", () => {
  it("constructs an RFB with the given wsUrl on mount", () => {
    renderViewer({ wsUrl: "ws://localhost:6000/" });
    expect(RfbMock).toHaveBeenCalledTimes(1);
    expect(RfbMock.mock.calls[0][1]).toBe("ws://localhost:6000/");
    // (v3.0.7) The mock instance's `wsUrl` field
    // records what the constructor saw. (The real RFB
    // doesn't expose this — we record it ourselves for
    // assertion.)
    expect(getLastInstance()?.wsUrl).toBe("ws://localhost:6000/");
  });

  it("passes shared: true to the RFB constructor", () => {
    renderViewer();
    const options = RfbMock.mock.calls[0][2] as { shared?: boolean };
    expect(options?.shared).toBe(true);
  });

  it("passes credentials through to the RFB constructor when provided", () => {
    renderViewer({
      credentials: { password: "hunter2" },
    });
    const options = RfbMock.mock.calls[0][2] as {
      credentials?: { password?: string };
    };
    expect(options?.credentials?.password).toBe("hunter2");
  });

  it("omits credentials when the prop is not provided", () => {
    renderViewer();
    const options = RfbMock.mock.calls[0][2] as {
      credentials?: { password?: string };
    };
    // The default is `undefined` — the noVNC constructor
    // accepts this and the local Tauri proxy doesn't
    // trigger a credentials handshake.
    expect(options?.credentials).toBeUndefined();
  });
});

describe("noVncViewer — framebuffer reception", () => {
  it("calls onConnect only after a framebuffer event (desktopname) is fired", () => {
    const { onConnect } = renderViewer();
    expect(RfbMock).toHaveBeenCalledTimes(1);
    const rfb = getLastInstance()!;
    // `connect` is NOT enough to call onConnect in v3.0.7.
    act(() => {
      rfb.fire("connect");
    });
    expect(onConnect).not.toHaveBeenCalled();
    // The first framebuffer event (desktopname) flips
    // `firstFrameReceived` and fires onConnect.
    act(() => {
      rfb.fire("desktopname");
    });
    expect(onConnect).toHaveBeenCalledTimes(1);
  });

  it("treats resize as a first-frame signal", () => {
    const { onConnect } = renderViewer();
    const rfb = getLastInstance()!;
    act(() => {
      rfb.fire("connect");
    });
    act(() => {
      rfb.fire("resize");
    });
    expect(onConnect).toHaveBeenCalledTimes(1);
  });

  it("fires onConnect only once even if multiple framebuffer events arrive", () => {
    const { onConnect } = renderViewer();
    const rfb = getLastInstance()!;
    act(() => {
      rfb.fire("connect");
      rfb.fire("desktopname");
      rfb.fire("resize");
    });
    expect(onConnect).toHaveBeenCalledTimes(1);
  });

  it("clears the no-frame timeout when a framebuffer event arrives", () => {
    const { onError } = renderViewer();
    const rfb = getLastInstance()!;
    act(() => {
      rfb.fire("connect");
    });
    // Advance just under the 6s threshold.
    act(() => {
      vi.advanceTimersByTime(5500);
    });
    expect(onError).not.toHaveBeenCalled();
    // Now paint a frame — the timer should be cancelled.
    act(() => {
      rfb.fire("desktopname");
    });
    // Advance past the original threshold.
    act(() => {
      vi.advanceTimersByTime(2000);
    });
    expect(onError).not.toHaveBeenCalled();
  });
});

describe("noVncViewer — no-frame timeout", () => {
  it("calls onError with a no-framebuffer message if no frame arrives in time", () => {
    const { onError } = renderViewer();
    const rfb = getLastInstance()!;
    act(() => {
      rfb.fire("connect");
    });
    // No desktopname / resize / etc. — the 6s no-frame
    // timer should fire.
    act(() => {
      vi.advanceTimersByTime(6000);
    });
    expect(onError).toHaveBeenCalledTimes(1);
    const msg = onError.mock.calls[0][0] as string;
    expect(msg).toMatch(/no framebuffer received/i);
  });

  it("does NOT call onError if a frame arrives before the timeout", () => {
    const { onError } = renderViewer();
    const rfb = getLastInstance()!;
    act(() => {
      rfb.fire("connect");
    });
    act(() => {
      vi.advanceTimersByTime(3000);
      rfb.fire("desktopname");
      vi.advanceTimersByTime(5000);
    });
    expect(onError).not.toHaveBeenCalled();
  });
});

describe("noVncViewer — credentials flow", () => {
  it("responds to credentialsrequired with sendCredentials(password) when credentials prop is set", () => {
    renderViewer({ credentials: { password: "secret123" } });
    const rfb = getLastInstance()!;
    act(() => {
      rfb.fire("credentialsrequired");
    });
    expect(rfb.sendCredentials).toHaveBeenCalledTimes(1);
    expect(rfb.sendCredentials).toHaveBeenCalledWith({
      password: "secret123",
    });
  });

  it("calls onError when credentialsrequired fires without a credentials prop", () => {
    const { onError } = renderViewer();
    const rfb = getLastInstance()!;
    act(() => {
      rfb.fire("credentialsrequired");
    });
    expect(onError).toHaveBeenCalledTimes(1);
    const msg = onError.mock.calls[0][0] as string;
    expect(msg).toMatch(/credentials/i);
    // sendCredentials must NOT be called when the
    // caller didn't provide a password.
    expect(rfb.sendCredentials).not.toHaveBeenCalled();
  });
});

describe("noVncViewer — wsUrl change", () => {
  it("disconnects the old RFB and constructs a new one with the new URL", () => {
    const { rerender } = renderViewer({ wsUrl: "ws://localhost:5900/" });
    expect(RfbMock).toHaveBeenCalledTimes(1);
    const first = getLastInstance()!;
    // Re-render with a fresh URL (the Tauri side typically
    // hands us a new one after a VM restart).
    rerender(
      <NoVncViewer
        wsUrl="ws://localhost:5901/"
        onConnect={vi.fn()}
        onDisconnect={vi.fn()}
        onError={vi.fn()}
      />,
    );
    // The old RFB is disconnected.
    expect(first.disconnect).toHaveBeenCalled();
    // A new RFB was constructed with the new URL.
    expect(RfbMock).toHaveBeenCalledTimes(2);
    expect(RfbMock.mock.calls[1][1]).toBe("ws://localhost:5901/");
    expect(getLastInstance()?.wsUrl).toBe("ws://localhost:5901/");
  });

  it("clears the container children on every reconnect", () => {
    const { container, rerender } = renderViewer({ wsUrl: "ws://localhost:5900/" });
    // The mock RFB appends a placeholder into the target.
    // After a wsUrl change, the new effect cleanup should
    // have cleared it before the new RFB was constructed.
    const viewer = container.querySelector(".novnc-viewer")!;
    expect(viewer.children.length).toBeGreaterThan(0);
    rerender(
      <NoVncViewer
        wsUrl="ws://localhost:5901/"
        onConnect={vi.fn()}
        onDisconnect={vi.fn()}
        onError={vi.fn()}
      />,
    );
    // The cleanup's `while (el.firstChild) el.removeChild(...)`
    // ran, then the new RFB's mock injected a fresh
    // placeholder. The viewer should have exactly one
    // child — the new RFB's canvas, not stale children
    // from the previous RFB.
    expect(viewer.children.length).toBe(1);
    expect(viewer.children[0].className).toBe("novnc-canvas-mock");
  });
});

describe("noVncViewer — ref API (preserved from v3.0.6)", () => {
  it("forwards sendKey, sendMouse, isConnected on the ref", () => {
    const { ref } = renderViewer();
    // The methods are present and callable even before
    // `connect` fires.
    expect(typeof ref.current?.sendKey).toBe("function");
    expect(typeof ref.current?.sendMouse).toBe("function");
    expect(typeof ref.current?.isConnected).toBe("function");
    // isConnected starts false.
    expect(ref.current?.isConnected()).toBe(false);
  });
});
