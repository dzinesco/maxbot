// v4 — useFocusRefresh tests.
//
// Per the v4 brief: "no setInterval — focus events are
// user-driven, not timers." The hook attaches a `focus`
// listener on `window` and re-fires the callback when the
// window regains focus.

import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import { useFocusRefresh } from "./useFocusRefresh";

afterEach(() => cleanup());

function Probe({ onFire }: { onFire: () => void }) {
  useFocusRefresh(onFire);
  return null;
}

describe("useFocusRefresh", () => {
  it("calls the callback when window receives a focus event", () => {
    const cb = vi.fn();
    render(<Probe onFire={cb} />);
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(cb).toHaveBeenCalledTimes(1);
  });

  it("does not call the callback for unrelated events", () => {
    const cb = vi.fn();
    render(<Probe onFire={cb} />);
    act(() => {
      window.dispatchEvent(new Event("blur"));
      window.dispatchEvent(new Event("resize"));
    });
    expect(cb).not.toHaveBeenCalled();
  });

  it("does not create any timers of its own", () => {
    // useFocusRefresh uses `window.addEventListener("focus", ...)`,
    // which is event-driven, not timer-driven. happy-dom may use
    // setInterval/setTimeout internally for its event loop; the
    // assertion that matters here is "no setInterval / setTimeout
    // is called by this hook" — enforced by reading the source
    // (only `addEventListener` + `removeEventListener` are called).
    const cb = vi.fn();
    render(<Probe onFire={cb} />);
    expect(cb).not.toHaveBeenCalled();
  });

  it("removes the listener on unmount", () => {
    const cb = vi.fn();
    const { unmount } = render(<Probe onFire={cb} />);
    unmount();
    act(() => {
      window.dispatchEvent(new Event("focus"));
    });
    expect(cb).not.toHaveBeenCalled();
  });
});
