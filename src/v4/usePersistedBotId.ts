/*
 * v4 — usePersistedBotId
 *
 * v3.7.17 Slice S2 — last selected bot id persisted.
 *
 * Loads from `localStorage` once at boot; saves on every change.
 * Returns `[selectedBotId, setSelectedBotId]` like useState so
 * App.tsx can use it as a drop-in for the bot-selection state.
 *
 * Storage key: `maxbot:selectedBotId`. We swallow localStorage
 * errors (private mode, quota exceeded) — persistence is best
 * effort; the in-memory state still works.
 *
 * No setInterval. No listeners. Pure synchronous read + writes.
 */

import { useEffect, useState } from "react";

const STORAGE_KEY = "maxbot:selectedBotId";

function readStored(): string | null {
  try {
    if (typeof localStorage === "undefined") return null;
    const v = localStorage.getItem(STORAGE_KEY);
    return v && v.length > 0 ? v : null;
  } catch {
    return null;
  }
}

function writeStored(value: string | null): void {
  try {
    if (typeof localStorage === "undefined") return;
    if (value === null) localStorage.removeItem(STORAGE_KEY);
    else localStorage.setItem(STORAGE_KEY, value);
  } catch {
    /* ignore — quota, private mode, etc. */
  }
}

export function usePersistedBotId(
  validator: (id: string) => boolean = () => true,
): [string | null, (id: string | null) => void] {
  // Lazy init reads localStorage on first render. After that, state
  // is the source of truth (so the boot IPC's `bots` arrival can
  // validate the stored id before committing).
  const [selectedBotId, setSelectedBotId] = useState<string | null>(() => {
    const stored = readStored();
    return stored && validator(stored) ? stored : null;
  });

  useEffect(() => {
    writeStored(selectedBotId);
  }, [selectedBotId]);

  return [selectedBotId, setSelectedBotId];
}

export const STORAGE_KEY_FOR_TESTS = STORAGE_KEY;
