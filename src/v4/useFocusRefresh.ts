/*
 * v4 — useFocusRefresh
 *
 * Hook: re-fires `callback` whenever the Tauri window regains
 * focus. No setInterval — purely event-driven.
 *
 * Use case: surfaces that fetch on demand (Loop expanded body,
 * chat history). When the user alt-tabs back to MaxBot, the
 * surface re-reads so the displayed state is current.
 *
 * Per Tyler's v4 hard rules: "no setInterval unless surface
 * visible" — focus events are user-driven, not timers.
 */

import { useEffect, useRef } from "react";

export function useFocusRefresh(callback: () => void): void {
  const cbRef = useRef(callback);
  cbRef.current = callback;

  useEffect(() => {
    const handler = () => {
      cbRef.current();
    };
    window.addEventListener("focus", handler);
    return () => {
      window.removeEventListener("focus", handler);
    };
  }, []);
}
