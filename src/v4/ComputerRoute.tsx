/*
 * v4 — ComputerRoute
 *
 * Per-Bot Computer Use panel. Mounts only when the user
 * opens Computer from a bot's menu. Closes = unmount +
 * revoke blobs + cancel timers + unlisten.
 *
 * Per Tyler's v4 hard rules:
 * - Mount only when `open` is true.
 * - All timers (recursive setTimeout tick + heartbeat) live
 *   inside this component and die on unmount.
 * - URL.createObjectURL is paired with revokeObjectURL on
 *   every blob replace AND on unmount.
 * - No setInterval — the screenshot poll is a recursive
 *   setTimeout so the next delay can back off on errors.
 *
 * Screenshot fetch:
 * - Calls computerScreenshot(botId) on the tick.
 * - Receives { bytes: number[]-or-Uint8Array, width, height }.
 * - Wraps the bytes in a Blob (PNG mime), creates a URL,
 *   revokes the previous URL before replacing.
 *
 * State change events:
 * - Subscribes to onComputerStateChanged; updates `computer`.
 * - Cleanup awaits unlisten.
 *
 * Input driving is intentionally NOT ported into v4 yet —
 * computerInputOpen / computerInputEvent / computerInputClose
 * would need their own UI surface (click map, keyboard
 * forwarder). v4's ComputerRoute is read-only + screenshot
 * polling. The full drive-mode rebuild is a follow-up.
 */

import { useEffect, useRef, useState } from "react";
import {
  computerGet,
  computerScreenshot,
  onComputerStateChanged,
} from "../lib/tauri";
import type { Computer, ComputerStateChangedEvent } from "../lib/api";
import "./styles/computer.css";

// What the Rust side returns. Mirrors ComputerScreenshotOutput
// in `src-tauri/src/commands/computer.rs` — duplicated here
// instead of importing from src/lib/api.ts because the v4
// boundary says "no shared types from the old tree."
interface ComputerScreenshotOutput {
  bytes: number[] | Uint8Array;
  width: number;
  height: number;
}

function bytesToBlob(bytes: number[] | Uint8Array): Blob {
  // The Rust side returns either a plain number[] (JSON) or a
  // Uint8Array (binary). Normalize to a fresh ArrayBuffer-backed
  // Uint8Array, then construct the Blob. TypeScript's BlobPart
  // type wants ArrayBuffer-backed views specifically; the copy
  // ensures we're not pointing at a SharedArrayBuffer.
  let u8: Uint8Array;
  if (bytes instanceof Uint8Array) {
    const copy = new Uint8Array(bytes.byteLength);
    copy.set(bytes);
    u8 = copy;
  } else {
    u8 = new Uint8Array(bytes as number[]);
  }
  // The cast: `u8.buffer` is an ArrayBuffer (not SharedArrayBuffer)
  // because `new Uint8Array(length)` creates ArrayBuffer-backed
  // storage. TypeScript can't always infer this through the generic
  // overloads; the runtime is correct.
  return new Blob([u8.buffer as ArrayBuffer], { type: "image/png" });
}

export interface ComputerRouteProps {
  /** Bot whose computer to display. */
  botId: string;
  /** Bot name for the header. */
  botName: string;
  /** Called when the user clicks Close. App.tsx sets view back. */
  onClose: () => void;
}

export function ComputerRoute({ botId, botName, onClose }: ComputerRouteProps) {
  const [computer, setComputer] = useState<Computer | null>(null);
  const [frameUrl, setFrameUrl] = useState<string | null>(null);
  const [frameSize, setFrameSize] = useState<{ w: number; h: number } | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);
  const [lastTick, setLastTick] = useState<number | null>(null);

  const blobRef = useRef<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let unlistenState: (() => void) | null = null;

    // Initial computer metadata.
    (async () => {
      try {
        const c = await computerGet(botId);
        if (cancelled) return;
        setComputer(c);
      } catch (e) {
        if (cancelled) return;
        setError(`Could not load computer: ${e}`);
      }
    })();

    // State change listener.
    (async () => {
      try {
        const u = await onComputerStateChanged((ev: ComputerStateChangedEvent) => {
          if (cancelled) return;
          if (ev.bot_id !== botId) return;
          setComputer((prev) =>
            prev
              ? { ...prev, state: ev.state, last_seen_at: new Date().toISOString() }
              : prev,
          );
        });
        if (cancelled) {
          u();
        } else {
          unlistenState = u;
        }
      } catch {
        /* ignore — best-effort */
      }
    })();

    // Screenshot tick — recursive setTimeout with simple
    // backoff on errors. The interval is "as fast as the
    // previous tick returned" — for a healthy VM that's
    // ~250ms; on errors we slow to 2s to avoid hammering.
    const TICK_OK_MS = 750;
    const TICK_ERR_MS = 2_000;

    const tick = async () => {
      if (cancelled) return;
      try {
        const out = (await computerScreenshot(botId)) as ComputerScreenshotOutput;
        if (cancelled) return;
        const blob = bytesToBlob(out.bytes);
        const url = URL.createObjectURL(blob);
        if (blobRef.current) URL.revokeObjectURL(blobRef.current);
        blobRef.current = url;
        setFrameUrl(url);
        setFrameSize({ w: out.width, h: out.height });
        setLastTick(Date.now());
        setError(null);
        timer = setTimeout(tick, TICK_OK_MS);
      } catch (e) {
        if (cancelled) return;
        setError(`Screenshot failed: ${e}`);
        timer = setTimeout(tick, TICK_ERR_MS);
      }
    };

    // Kick off the first tick after a short delay so the
    // initial computerGet has a chance to complete.
    timer = setTimeout(tick, 100);

    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      if (unlistenState) {
        try {
          unlistenState();
        } catch {
          /* ignore */
        }
      }
      if (blobRef.current) {
        URL.revokeObjectURL(blobRef.current);
        blobRef.current = null;
      }
    };
  }, [botId]);

  return (
    <div className="v4-computer-route" aria-label={`Computer for ${botName}`}>
      <header className="v4-computer-route-header">
        <div className="v4-computer-route-title">
          <span className="v4-computer-route-name">{botName}</span>
          <span className="v4-computer-route-state">
            {computer?.state ?? "unknown"}
          </span>
          {frameSize && (
            <span className="v4-computer-route-size">
              {frameSize.w}×{frameSize.h}
            </span>
          )}
          {lastTick && (
            <span className="v4-computer-route-tick">
              tick {Math.round((Date.now() - lastTick) / 100) / 10}s ago
            </span>
          )}
        </div>
        <button
          type="button"
          className="v4-computer-route-close ghost small"
          onClick={onClose}
        >
          Close
        </button>
      </header>

      {error && (
        <div className="v4-computer-route-error" role="alert">
          {error}
        </div>
      )}

      <div className="v4-computer-route-frame-wrap">
        {frameUrl ? (
          <img
            className="v4-computer-route-frame"
            src={frameUrl}
            alt={`${botName} desktop`}
            draggable={false}
          />
        ) : (
          <div className="v4-computer-route-placeholder">
            {error ? "Retrying…" : "Waiting for first frame…"}
          </div>
        )}
      </div>

      <footer className="v4-computer-route-footer">
        <span className="v4-computer-route-vm">{computer?.vm_name ?? "—"}</span>
        <span className="v4-computer-route-ip">{computer?.vm_ip ?? "—"}</span>
      </footer>
    </div>
  );
}
