// v4 — usePersistedBotId tests.
//
// Per the S2 brief: "Tests ... persist selected bot."
//
// Hook contract:
//   - First render reads `maxbot:selectedBotId` from localStorage.
//   - If the validator returns true for the stored id, the hook
//     returns it as the initial state.
//   - If the validator returns false (or the stored id is missing),
//     the hook returns null.
//   - Every state change writes the new value to localStorage
//     (or removes it on null).

import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import {
  STORAGE_KEY_FOR_TESTS,
  usePersistedBotId,
} from "./usePersistedBotId";

const KEY = STORAGE_KEY_FOR_TESTS;

beforeEach(() => {
  localStorage.clear();
});

afterEach(() => {
  cleanup();
  localStorage.clear();
});

describe("usePersistedBotId", () => {
  it("returns null on first render when localStorage is empty", () => {
    const { result } = renderHook(() => usePersistedBotId());
    expect(result.current[0]).toBeNull();
  });

  it("returns the stored id when localStorage has one and the validator passes", () => {
    localStorage.setItem(KEY, "bot-1");
    const validator = (id: string) => id === "bot-1";
    const { result } = renderHook(() => usePersistedBotId(validator));
    expect(result.current[0]).toBe("bot-1");
  });

  it("returns null when the stored id fails the validator", () => {
    localStorage.setItem(KEY, "bot-stale");
    const validator = (id: string) => id === "bot-1";
    const { result } = renderHook(() => usePersistedBotId(validator));
    expect(result.current[0]).toBeNull();
  });

  it("writes the new id to localStorage on change", () => {
    const { result } = renderHook(() => usePersistedBotId());
    expect(localStorage.getItem(KEY)).toBeNull();
    act(() => {
      result.current[1]("bot-2");
    });
    expect(localStorage.getItem(KEY)).toBe("bot-2");
  });

  it("removes the key from localStorage when set to null", () => {
    localStorage.setItem(KEY, "bot-1");
    const { result } = renderHook(() => usePersistedBotId());
    expect(result.current[0]).toBe("bot-1");
    act(() => {
      result.current[1](null);
    });
    expect(localStorage.getItem(KEY)).toBeNull();
  });

  it("survives a remount (the next render reads the latest persisted value)", () => {
    const first = renderHook(() => usePersistedBotId());
    act(() => {
      first.result.current[1]("bot-3");
    });
    first.unmount();
    const second = renderHook(() => usePersistedBotId());
    expect(second.result.current[0]).toBe("bot-3");
  });
});
