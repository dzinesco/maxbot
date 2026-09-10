// ComputerPanel — the two-mode UI for the per-Bot VM
// (v2.0 Slice C, v3.7.2 re-render, v3.7.9 click-through
// takeover).
//
// Modes:
//   - status    : tiny chip (icon + dot + uptime). Used in the
//                 title bar (`App.tsx` chrome wires this; we
//                 just expose the standalone component).
//   - preview   : pinned side panel, ~30% width, view-only
//                 screenshot poll. The Bot is still driving.
//                 "Drive" button in the toolbar hands the
//                 VM to the user via in-panel click-through
//                 (v3.7.9). The same screenshot poll stays
//                 up so the Bot can resume when the user
//                 hands back.
//
// v3.7.9 click-through takeover:
//   - A single in-panel click-through on the existing
//     JPEG preview replaces the previous external-VNC
//     takeover path. No external viewer app, no SSH
//     tunnel — pointer and key events are sent to the
//     VM's X11 session via the Rust-side `xdotool`
//     script renderer.
//   - The `initialDriving` prop (default false) lets the
//     approval-queue mount the panel already driving. The
//     `onClose` callback in that case cascades the
//     approval-decide + input-close (see App.tsx).
//
// v3.7.2 changes from v3.0.x:
//   - The noVNC↔RFB WebSocket bridge is gone. The Tauri
//     webview (WKWebView) didn't render noVNC's canvas
//     path reliably, and the QEMU virtual framebuffer
//     is what `virsh screenshot` captures anyway.
//   - The preview is now a JPEG poll at ~300ms via
//     `computerScreenshot(botId)`. The Tauri side runs
//     `virsh screenshot <vm> /tmp/...ppm` (or PPM→JPEG
//     via ImageMagick) and pipes the bytes back.
//
// Data flow:
//   1. `computerGet(botId)` is called on mount and every
//      `pollIntervalMs` (default 5s) in Preview.
//   2. We also subscribe to `computer://state-changed`
//      so a state transition flips the panel without
//      waiting for the next poll.
//   3. `computerScreenshot(botId)` is polled at 300ms
//      whenever `computer.state === "running"`. The
//      poll pauses on `document.hidden` and never stacks
//      more than one in-flight request. When the mouse
//      is down (drag) the poll accelerates to 150ms so
//      the user sees continuous visual feedback.
//   4. Toolbar buttons (Start / Stop / Restart / Destroy)
//      call the corresponding Tauri commands; they disable
//      while the VM is in `provisioning` (or `error`).
//   5. The "Drive" toolbar button calls
//      `computerInputOpen(botId)` which sets a per-Bot
//      flag on the Rust side. The Bot's `vm_computer_use`
//      tool refuses while the flag is true. Pointer /
//      key / wheel events on the `<img>` go to
//      `computerInputEvent(botId, event)`. "Hand back"
//      calls `computerInputClose(botId)`.
//
// Restart is a thin convenience: Stop + Start with a small
// delay between them so libvirt's `virsh start` after a clean
// shutdown doesn't race with the shutdown.

import { useCallback, useEffect, useRef, useState } from "react";
import type { Computer, Settings } from "../lib/api";
import {
  computerDestroy,
  computerGet,
  computerInputClose,
  computerInputEvent,
  computerInputOpen,
  computerInstallDefaultKey,
  computerProvision,
  computerScreenshot,
  computerStart,
  computerStop,
  getSettings,
  onComputerStateChanged,
  saveSettings,
} from "../lib/tauri";
import type { InputEvent } from "../lib/tauri";
import { ComputerFileBrowser } from "./ComputerFileBrowser";

export type ComputerMode = "status" | "preview";

export interface ComputerPanelProps {
  botId: string;
  mode: ComputerMode;
  /** Used in preview mode. Called when the user
   * clicks the close button. The panel also calls
   * `onClose` when the user clicks "Hand back" — the
   * parent owns the cascade (in the approval-queue
   * case that's `approvalDecide(approved)` +
   * `computerInputClose`). */
  onClose?: () => void;
  /** v3.7.9: when true, the panel mounts in driving
   * mode (the Rust driving flag is implicitly set by
   * the parent via `computerInputOpen` BEFORE mount;
   * the panel does not re-call it on mount). The
   * approval-queue uses this so the user lands in
   * the panel already able to interact with the VM
   * without a click. */
  initialDriving?: boolean;
  /** Override the default poll interval (ms) for
   * `computerGet`. The preview screenshot poll has its
   * own 300ms cadence. The plan calls for 5s; tests pass
   * a smaller value. */
  pollIntervalMs?: number;
}

const DEFAULT_POLL_MS = 5000;
const SCREENSHOT_POLL_MS = 300;
/** v3.7.9: when the user is dragging, drop the poll
 * interval to ~150ms so the visual feedback is smooth. */
const SCREENSHOT_POLL_DRAGGING_MS = 150;

interface FbSize {
  w: number;
  h: number;
}
type FbPoint = { x: number; y: number } | null;

/** v3.7.9: map a DOM PointerEvent's clientX/Y to
 *  framebuffer coordinates for the VM's QEMU virtual
 *  display. The `<img>` is letterboxed (object-fit:
 *  contain) so a click on the letterbox is "outside"
 *  the rendered image — return null in that case so
 *  the caller can drop the event. */
export function mapToFramebuffer(
  clientX: number,
  clientY: number,
  img: HTMLImageElement,
  fb: FbSize,
): FbPoint {
  if (!fb.w || !fb.h) return null;
  const rect = img.getBoundingClientRect();
  if (!rect.width || !rect.height) return null;
  const imgAspect = fb.w / fb.h;
  const boxAspect = rect.width / rect.height;
  let renderW: number;
  let renderH: number;
  let offsetX: number;
  let offsetY: number;
  if (imgAspect > boxAspect) {
    // Letterboxed top/bottom.
    renderW = rect.width;
    renderH = renderW / imgAspect;
    offsetX = 0;
    offsetY = (rect.height - renderH) / 2;
  } else {
    // Letterboxed left/right.
    renderH = rect.height;
    renderW = renderH * imgAspect;
    offsetX = (rect.width - renderW) / 2;
    offsetY = 0;
  }
  if (
    clientX < offsetX ||
    clientX > offsetX + renderW ||
    clientY < offsetY ||
    clientY > offsetY + renderH
  ) {
    return null;
  }
  const ratioX = (clientX - offsetX) / renderW;
  const ratioY = (clientY - offsetY) / renderH;
  return {
    x: Math.round(ratioX * fb.w),
    y: Math.round(ratioY * fb.h),
  };
}

/** v3.7.9: DOM `KeyboardEvent.key` → xdotool key
 *  name. Most printable characters pass through
 *  unchanged; named keys (Enter, Escape, Arrow*, …)
 *  are mapped via the table. Modifier keys are
 *  passed through lowercase — xdotool sees them as
 *  modifiers on the next non-modifier event when
 *  `--clearmodifiers` is set on the consuming side. */
const DOM_KEY_TO_XDOTOOL: Record<string, string> = {
  Enter: "Return",
  Escape: "Escape",
  Backspace: "BackSpace",
  Tab: "Tab",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
  Delete: "Delete",
  Home: "Home",
  End: "End",
  PageUp: "Page_Up",
  PageDown: "Page_Down",
  " ": "space",
};

export function domKeyToXdotool(e: KeyboardEvent): string {
  // Named keys: lookup the table first. The
  // table also covers the single-character " "
  // (space) which must map to "space" — without
  // the table check first, the single-character
  // branch below would pass it through as " "
  // and xdotool would reject it.
  if (DOM_KEY_TO_XDOTOOL[e.key]) {
    return DOM_KEY_TO_XDOTOOL[e.key];
  }
  // Modifier-only keys: skip (let xdotool see them
  // as modifiers on the next non-modifier event).
  if (
    e.key === "Shift" ||
    e.key === "Control" ||
    e.key === "Alt" ||
    e.key === "Meta"
  ) {
    return e.key.toLowerCase();
  }
  // Single character keys: pass through. For
  // letters, xdotool expects lowercase ("xdotool
  // keydown -- a") unless a Shift modifier is held.
  // We rely on xdotool's --clearmodifiers semantics
  // to handle Shift automatically. For symbols, the
  // key is what it is.
  if (e.key.length === 1) {
    return e.key.toLowerCase();
  }
  // Fallback: try the raw key as-is.
  return e.key;
}

/** Format seconds → "1h 2m" / "12m" / "47s". Used in status
 * mode for the uptime chip. */
function formatUptime(seconds: number | null): string {
  if (seconds === null || seconds < 0) return "—";
  if (seconds < 60) return `${Math.floor(seconds)}s`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m`;
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  return m === 0 ? `${h}h` : `${h}h ${m}m`;
}

function stateClass(state: string | null | undefined): string {
  switch (state) {
    case "running":
      return "running";
    case "stopped":
      return "stopped";
    case "provisioning":
      return "provisioning";
    case "error":
      return "error";
    default:
      return "unknown";
  }
}

export function ComputerPanel({
  botId,
  mode,
  onClose,
  initialDriving,
  pollIntervalMs = DEFAULT_POLL_MS,
}: ComputerPanelProps) {
  // Status mode is a self-contained chip — short-circuit before
  // the full panel logic so the sidebar / title bar can mount
  // many of these without a Tauri round-trip per render.
  if (mode === "status") {
    return <ComputerStatusChip botId={botId} />;
  }

  return (
    <FullComputerPanel
      botId={botId}
      onClose={onClose}
      initialDriving={initialDriving ?? false}
      pollIntervalMs={pollIntervalMs}
    />
  );
}

/** Tiny status icon for the title bar. Loads `computerGet`
 * once on mount (no polling) — title-bar presence is a hint,
 * not a live readout; the panel itself drives live updates. */
function ComputerStatusChip({ botId }: { botId: string }) {
  const [computer, setComputer] = useState<Computer | null>(null);
  useEffect(() => {
    let cancelled = false;
    computerGet(botId)
      .then((c) => {
        if (!cancelled) setComputer(c);
      })
      .catch(() => {
        // best-effort: if the lookup fails (Tauri not
        // available, or the bot has no computer row) we
        // just render the "no computer" chip.
        if (!cancelled) setComputer(null);
      });
    return () => {
      cancelled = true;
    };
  }, [botId]);

  const state = computer?.state ?? null;
  const cls = stateClass(state);
  const title = computer
    ? `${computer.state} · ${computer.vm_ip ?? "no ip"}`
    : "no computer";

  return (
    <span
      className={`computer-panel__status-icon computer-panel__status-icon--${cls}`}
      title={title}
      data-testid="computer-status-chip"
    >
      <span className={`computer-panel__status-dot computer-panel__status-dot--${cls}`} />
      <span className="computer-panel__status-label">PC</span>
    </span>
  );
}

interface FullComputerPanelProps {
  botId: string;
  onClose?: () => void;
  initialDriving: boolean;
  pollIntervalMs: number;
}

function FullComputerPanel({
  botId,
  onClose,
  initialDriving,
  pollIntervalMs,
}: FullComputerPanelProps) {
  const [computer, setComputer] = useState<Computer | null>(null);
  const [loading, setLoading] = useState(true);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  // v3.7.2: in-app preview is a screenshot poll, not a
  // noVNC stream. `frameUrl` is a `blob:` URL pointing at
  // the most recent JPEG; `firstFrame` is set after the
  // first successful pull so the panel can swap from the
  // loading overlay to the `<img>`.
  const [frameUrl, setFrameUrl] = useState<string | null>(null);
  const [firstFrame, setFirstFrame] = useState(false);
  // `viewerError` is set when the screenshot poll fails.
  // Distinct from `errorMsg`, which is reserved for the
  // computer.state === "error" path. The poll failure
  // is the most common failure mode in practice (passphrase
  // missing, host unreachable, VM in wrong state) and
  // previously rendered a silent blank panel.
  const [viewerError, setViewerError] = useState<string | null>(null);
  // v3.7.2 (amended): `domainNotFound` is set when the
  // screenshot poll fails with `ComputerError::DomainNotFound`
  // — the libvirt domain is missing on the host. The
  // message comes through as a stringified Rust error
  // (Tauri serializes the Display output, not a typed
  // payload), so we detect by substring. The vm name is
  // carried alongside so the "VM not provisioned" UI can
  // quote it back to the user.
  //
  // When this is set, the panel renders a clear "VM not
  // provisioned" state with a Provision button (calls
  // `computerProvision`) and a secondary "Destroy + re-
  // provision" link. The auto-retry is implicit: the
  // screenshot poll re-runs every 300ms, so as soon as
  // the VM transitions to `running` (which the
  // `computer://state-changed` listener picks up via
  // `loadComputer`), the panel swaps back to the live
  // preview.
  const [domainNotFound, setDomainNotFound] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [uptime, setUptime] = useState<number | null>(null);
  // Body tab: "console" (screenshot preview) vs "files"
  // (SFTP browser). The Files tab only does real work
  // when the VM has an SSH endpoint and a known ssh_key
  // row, but the browser is mounted regardless and shows
  // its own error if the listing fails.
  const [bodyTab, setBodyTab] = useState<"console" | "files">("console");
  // v2.3.5: settings snapshot. Used to decide whether to
  // show the "Use my default key" button in the toolbar.
  // The flip happens after a successful install so the
  // button disappears the next render.
  const [settings, setSettings] = useState<Settings | null>(null);
  // v2.3.5: short-lived confirmation banner after a
  // successful install — "Default key installed — restart
  // MaxBot to apply the new auth path".
  const [installConfirm, setInstallConfirm] = useState<string | null>(null);
  // v3.7.9: click-through driving mode. `driving` is
  // the React state; `drivingRef` mirrors it so the
  // unmount cleanup (which can't read state) can
  // decide whether to fire `computerInputClose` to
  // release the per-Bot driving flag.
  const [driving, setDriving] = useState<boolean>(initialDriving);
  const [drivingError, setDrivingError] = useState<string | null>(null);
  const drivingRef = useRef(driving);
  useEffect(() => {
    drivingRef.current = driving;
  }, [driving]);
  // `mouseDownRef` is true between pointerdown and
  // pointerup. The screenshot poll reads it to
  // accelerate from 300ms → 150ms while the user
  // is dragging, so the visual feedback is smooth.
  const mouseDownRef = useRef(false);
  // v3.7.9: the natural framebuffer dimensions. Set
  // by the screenshot poll (the IPC payload carries
  // `width`/`height` in v3.7.9+). The `<img>` events
  // map `clientX/Y` to framebuffer coordinates using
  // this ref.
  const fbSizeRef = useRef<FbSize>({ w: 0, h: 0 });
  const imgRef = useRef<HTMLImageElement>(null);
  // `lastSeenAt` is an ISO string; we tick once a second to
  // refresh the displayed uptime in status / preview modes.
  const lastSeenAt = computer?.last_seen_at ?? null;
  const uptimeTimer = useRef<ReturnType<typeof setInterval> | null>(null);
  // v3.7.2: refs for the screenshot poll. `inFlight` is
  // the "don't stack requests" guard. `blobRef` is the
  // previous blob URL we need to revoke before swapping
  // in a new one (otherwise the previous JPEG stays
  // pinned in memory until the next unmount).
  const inFlight = useRef(false);
  const blobRef = useRef<string | null>(null);

  // --- data loading ---
  // `loadComputer` fetches the persisted row. The
  // screenshot poll is its own effect (below).
  const loadComputer = useCallback(async () => {
    try {
      const c = await computerGet(botId);
      setComputer(c);
      setErrorMsg(null);
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setLoading(false);
    }
  }, [botId]);

  // Initial load + polling. Polling is the fallback: the
  // `computer://state-changed` event handles the transitions
  // we care about (start/stop/destroy) immediately, but
  // periodic re-fetch is the belt-and-suspenders path.
  useEffect(() => {
    let cancelled = false;
    loadComputer();
    const interval = setInterval(() => {
      if (!cancelled) loadComputer();
    }, pollIntervalMs);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, [loadComputer, pollIntervalMs]);

  // v2.3.5: load the settings snapshot once so the toolbar
  // can decide whether to show the "Use my default key"
  // button. The button is hidden when the default-key
  // flag is already on.
  useEffect(() => {
    let cancelled = false;
    getSettings()
      .then((s) => {
        if (!cancelled) setSettings(s);
      })
      .catch(() => {
        // Tauri bridge unavailable (e.g. in tests) —
        // leave settings null and the button stays hidden.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Event subscription: react to the Rust side's
  // `computer://state-changed` emissions so the panel
  // doesn't have to wait for the next 5s poll after the
  // user clicks Start/Stop in the toolbar.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    onComputerStateChanged((event) => {
      if (cancelled) return;
      if (event.bot_id !== botId) return;
      // Re-fetch the row to pick up ip / vnc_port / etc.
      loadComputer();
    })
      .then((u) => {
        if (cancelled) u();
        else unlisten = u;
      })
      .catch(() => {
        // no event channel in tests — silent no-op
      });
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [botId, loadComputer]);

  // Uptime ticker. Updates once a second when we have a
  // `last_seen_at`. The Rust side sets `last_seen_at` on
  // every successful VM interaction, so a "running" VM
  // with a recent timestamp is a confident active marker.
  useEffect(() => {
    if (uptimeTimer.current !== null) {
      clearInterval(uptimeTimer.current);
      uptimeTimer.current = null;
    }
    if (!lastSeenAt) {
      setUptime(null);
      return;
    }
    const tick = () => {
      const t = Date.parse(lastSeenAt);
      if (Number.isNaN(t)) {
        setUptime(null);
        return;
      }
      setUptime(Math.max(0, (Date.now() - t) / 1000));
    };
    tick();
    uptimeTimer.current = setInterval(tick, 1000);
    return () => {
      if (uptimeTimer.current !== null) {
        clearInterval(uptimeTimer.current);
        uptimeTimer.current = null;
      }
    };
  }, [lastSeenAt]);

  // v3.7.2 / v3.7.9: screenshot poll. Pulls a fresh JPEG
  // from the host's QEMU framebuffer at 300ms (or 150ms
  // while the user is dragging) whenever the VM is in
  // `running` state. Single in-flight request (no
  // stacking). Pauses on `document.hidden` so
  // backgrounded tabs don't burn SSH + virsh. The
  // previous blob URL is revoked before the next one
  // is set; otherwise the prior frame stays pinned
  // until unmount.
  //
  // v3.7.9: the poll is a `setTimeout`-based recursive
  // loop (not `setInterval`) so the next delay can
  // change on the fly based on `mouseDownRef.current`.
  // The poll also reads `width`/`height` off the new
  // `ComputerScreenshotOutput` payload into
  // `fbSizeRef` for the input-event coordinate mapping.
  //
  // The poll is keyed on `computer?.state` — flipping
  // to `stopped` / `provisioning` / `error` cancels
  // the timer (cleanup in the return) and clears any
  // in-flight ref. When state flips back to `running`,
  // the effect re-runs and the poll comes back
  // automatically.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;
    const pullFrame = async () => {
      if (cancelled) return;
      // Don't stack. The Tauri side returns ~10-30KB
      // of JPEG and the SSH round-trip is ~100-200ms
      // on the same LAN; if a request takes longer than
      // 300ms (e.g. cloud-init is busy), we'd rather
      // wait for it than pile on.
      if (inFlight.current) return;
      // Pause when the tab is backgrounded. The browser
      // may throttle timers anyway, but `document.hidden`
      // is the explicit signal.
      if (typeof document !== "undefined" && document.hidden) return;
      inFlight.current = true;
      try {
        // v3.7.9: the IPC contract changed — the
        // payload now carries the natural framebuffer
        // dimensions alongside the JPEG bytes. The
        // `Computer` type from `computerGet` does NOT
        // carry `framebufferWidth`/`Height`, so the
        // renderer reads them off the screenshot
        // payload here.
        const result = await computerScreenshot(botId);
        if (cancelled) return;
        if (result.width > 0 && result.height > 0) {
          fbSizeRef.current = { w: result.width, h: result.height };
        }
        // Wrap the bytes in a `Blob` for the
        // `URL.createObjectURL(blob)` call below. The
        // `bytes` field is always a `Uint8Array` from
        // the Rust side now (the old `number[]` path
        // is gone). The `BlobPart` cast keeps the
        // TypeScript 5.x DOM lib happy about
        // `Uint8Array<ArrayBuffer>` vs
        // `Uint8Array<ArrayBufferLike>`.
        const u8 =
          result.bytes instanceof Uint8Array
            ? result.bytes
            : new Uint8Array(result.bytes);
        const blob = new Blob([u8 as BlobPart], { type: "image/jpeg" });
        const url = URL.createObjectURL(blob);
        // Revoke the previous blob URL so it doesn't
        // leak. The image is rendered synchronously from
        // the new URL by the time React re-paints.
        if (blobRef.current) {
          URL.revokeObjectURL(blobRef.current);
        }
        blobRef.current = url;
        setFrameUrl(url);
        setFirstFrame(true);
        setViewerError(null);
        setDomainNotFound(null);
      } catch (e) {
        if (cancelled) return;
        const msg = String(e);
        // v3.7.2 (amended): detect the
        // `ComputerError::DomainNotFound` string the
        // Rust side emits. The Display impl is
        // "computer: domain '{vm_name}' not found on
        // host" — we extract the vm name in parens for
        // the "VM not provisioned" UI. The match is
        // case-insensitive because Tauri's `invoke`
        // rejection can be lower-cased on some
        // platforms.
        if (/domain\s+'([^']+)'\s+not found on host/i.test(msg)) {
          const m = msg.match(/'([^']+)'/);
          setDomainNotFound(m?.[1] ?? null);
          setViewerError(null);
        } else {
          setViewerError(msg);
          setDomainNotFound(null);
        }
      } finally {
        inFlight.current = false;
      }
    };
    const tick = () => {
      if (cancelled) return;
      void pullFrame();
      // v3.7.9: adaptive poll interval — drop to
      // 150ms while the user is dragging so visual
      // feedback is smooth, otherwise the default
      // 300ms.
      const delay = mouseDownRef.current
        ? SCREENSHOT_POLL_DRAGGING_MS
        : SCREENSHOT_POLL_MS;
      timer = setTimeout(tick, delay);
    };
    if (computer?.state === "running") {
      // Kick a frame immediately, then poll.
      void tick();
    }
    const onVis = () => {
      // When the tab becomes visible again, force a
      // frame so the user doesn't have to wait 300ms
      // for the first paint after returning.
      if (
        !document.hidden &&
        computer?.state === "running" &&
        !inFlight.current
      ) {
        void pullFrame();
      }
    };
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", onVis);
    }
    return () => {
      cancelled = true;
      if (timer !== null) {
        clearTimeout(timer);
      }
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", onVis);
      }
    };
  }, [botId, computer?.state]);

  // v3.7.2: revoke the most recent blob URL on unmount.
  // The timer-cleanup already clears the poll; this is
  // the symmetric cleanup for the blob ref so the last
  // frame doesn't pin its bytes after the panel closes.
  useEffect(() => {
    return () => {
      if (blobRef.current) {
        URL.revokeObjectURL(blobRef.current);
        blobRef.current = null;
      }
    };
  }, []);

  // v3.7.9: unmount cleanup for click-through driving.
  // If the panel is unmounted while `drivingRef.current
  // === true` (e.g. user closed the panel mid-driving,
  // or the parent unmounted without an explicit Hand
  // back), fire `computerInputClose` so the per-Bot
  // driving flag is released. The Rust side is
  // idempotent so a no-op double-close is safe.
  useEffect(() => {
    return () => {
      if (drivingRef.current) {
        void computerInputClose(botId);
      }
    };
  }, [botId]);

  // v3.7.9: Drive / Hand back handlers. The Drive
  // button calls `computerInputOpen` which sets the
  // per-Bot driving flag on the Rust side. The Bot's
  // `vm_computer_use` tool refuses while it's true.
  // Hand back calls `computerInputClose` and tells
  // the parent via `onClose?.()` so the parent can
  // run its cascade (e.g. `approvalDecide(approved)`).
  const handleDrive = useCallback(async () => {
    setDrivingError(null);
    try {
      await computerInputOpen(botId);
      setDriving(true);
    } catch (e) {
      setDrivingError(String(e));
    }
  }, [botId]);

  const handleHandBack = useCallback(async () => {
    try {
      await computerInputClose(botId);
    } finally {
      setDriving(false);
    }
    // Tell the parent the user is done driving.
    onClose?.();
  }, [botId, onClose]);

  // --- toolbar handlers ---
  const handleStart = useCallback(async () => {
    setActionPending(true);
    try {
      await computerStart(botId);
      await loadComputer();
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setActionPending(false);
    }
  }, [botId, loadComputer]);

  const handleStop = useCallback(async () => {
    setActionPending(true);
    try {
      await computerStop(botId);
      await loadComputer();
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setActionPending(false);
    }
  }, [botId, loadComputer]);

  const handleRestart = useCallback(async () => {
    setActionPending(true);
    try {
      await computerStop(botId);
      // Brief wait so libvirt's `virsh start` after
      // `shutdown` doesn't race the still-pending state.
      await new Promise((r) => setTimeout(r, 1000));
      await computerStart(botId);
      await loadComputer();
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setActionPending(false);
    }
  }, [botId, loadComputer]);

  const handleDestroy = useCallback(async () => {
    const confirmed = window.confirm(
      "Destroy this Bot's computer? The VM and its disk will be permanently removed.",
    );
    if (!confirmed) return;
    setActionPending(true);
    try {
      await computerDestroy(botId);
      await loadComputer();
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setActionPending(false);
    }
  }, [botId, loadComputer]);

  // v3.7.2 (amended): handle the "VM not provisioned" error
  // state. The user clicks Provision and the Tauri
  // `computer_provision` command runs the same
  // orchestrator the editor's "Provision a computer"
  // button kicks off. Settings carry the disk/RAM
  // defaults so we don't need to expose them here.
  //
  // On success, the `computer://state-changed` listener
  // re-fetches the row, the panel flips to
  // `provisioning` (existing branch), and the screenshot
  // poll re-engages once the VM reaches `running`. We
  // clear `domainNotFound` immediately so the error
  // banner disappears.
  const handleProvision = useCallback(async () => {
    setActionPending(true);
    setErrorMsg(null);
    try {
      const current = settings ?? (await getSettings());
      await computerProvision(botId, {
        disk_gb: current.computer_default_disk_gb,
        ram_mb: current.computer_default_ram_mb,
      });
      setDomainNotFound(null);
      await loadComputer();
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setActionPending(false);
    }
  }, [botId, loadComputer, settings]);

  // v3.7.2 (amended): "Destroy + re-provision" — for the
  // case where the local SQLite has a stale row pointing
  // at a domain that doesn't exist on the host (e.g. the
  // user destroyed the VM outside MaxBot, or moved the
  // host). Destroy drops the row, provision re-creates
  // the VM. The destroy is best-effort: if the local row
  // already says "no computer" we skip straight to
  // provision.
  const handleDestroyAndReprovision = useCallback(async () => {
    setActionPending(true);
    setErrorMsg(null);
    try {
      // The Tauri command errors if the row is already
      // gone; we ignore that case so the user doesn't
      // see a confusing "no computer to destroy" error.
      try {
        await computerDestroy(botId);
      } catch {
        // ignore — the row may not exist, that's fine
      }
      const current = settings ?? (await getSettings());
      await computerProvision(botId, {
        disk_gb: current.computer_default_disk_gb,
        ram_mb: current.computer_default_ram_mb,
      });
      setDomainNotFound(null);
      await loadComputer();
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setActionPending(false);
    }
  }, [botId, loadComputer, settings]);

  // v2.3.5: install the user's default SSH public key into
  // the VM via the QEMU guest agent, then flip the
  // `computer_use_default_ssh_key` flag so the next
  // Console click uses the default-key path. The Rust
  // command is idempotent (the QGA pipeline uses
  // `grep -qxF … || echo …`), so re-running is safe.
  const handleInstallDefaultKey = useCallback(async () => {
    setActionPending(true);
    setInstallConfirm(null);
    try {
      await computerInstallDefaultKey(botId);
      // Flip the setting so the next render hides the
      // button. We only send the new value plus the
      // minimum required fields; the Rust side's
      // `Settings` struct has `#[serde(default)]` on
      // every field, so a partial object round-trips
      // correctly.
      const current = settings ?? (await getSettings());
      const next: Settings = {
        ...current,
        computer_use_default_ssh_key: true,
      };
      await saveSettings(next);
      setSettings(next);
      setInstallConfirm(
        "Default key installed — restart MaxBot to apply the new auth path",
      );
    } catch (e) {
      setErrorMsg(String(e));
    } finally {
      setActionPending(false);
    }
  }, [botId, settings]);

  // v3.7.9: input event dispatchers. All four use
  // `mapToFramebuffer` to translate `clientX/Y` →
  // framebuffer coordinates. Buttons use the
  // `+ 1` convention so a left click (DOM button
  // 0) maps to xdotool button 1.
  const dispatchPointerEvent = useCallback(
    (
      e: React.PointerEvent<HTMLImageElement>,
      type: "pointer_down" | "pointer_up",
    ) => {
      if (!imgRef.current) return;
      const pt = mapToFramebuffer(
        e.clientX,
        e.clientY,
        imgRef.current,
        fbSizeRef.current,
      );
      if (!pt) return;
      const event: InputEvent = {
        type,
        x: pt.x,
        y: pt.y,
        button: e.button + 1,
      };
      void computerInputEvent(botId, event);
    },
    [botId],
  );

  const handlePointerMove = useCallback(
    (e: React.PointerEvent<HTMLImageElement>) => {
      if (!imgRef.current) return;
      // Only fire on drag (a button held) or while
      // the mouse is down by the panel's tracking
      // ref. Otherwise we'd flood the wire with one
      // event per pixel of idle mouse movement.
      if (e.buttons === 0 && !mouseDownRef.current) return;
      const pt = mapToFramebuffer(
        e.clientX,
        e.clientY,
        imgRef.current,
        fbSizeRef.current,
      );
      if (!pt) return;
      const event: InputEvent = { type: "pointer_move", x: pt.x, y: pt.y };
      void computerInputEvent(botId, event);
    },
    [botId],
  );

  const handleWheel = useCallback(
    (e: React.WheelEvent<HTMLImageElement>) => {
      if (!imgRef.current) return;
      e.preventDefault();
      const pt = mapToFramebuffer(
        e.clientX,
        e.clientY,
        imgRef.current,
        fbSizeRef.current,
      );
      if (!pt) return;
      const event: InputEvent = {
        type: "wheel",
        x: pt.x,
        y: pt.y,
        deltaY: e.deltaY,
      };
      void computerInputEvent(botId, event);
    },
    [botId],
  );

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLImageElement>) => {
      // Tab moves focus out of the image by default
      // — explicitly prevent it so the user can
      // keep driving the VM.
      if (e.key === "Tab") {
        e.preventDefault();
        return;
      }
      e.preventDefault();
      const event: InputEvent = {
        type: "key_down",
        name: domKeyToXdotool(e.nativeEvent),
      };
      void computerInputEvent(botId, event);
    },
    [botId],
  );

  const handleKeyUp = useCallback(
    (e: React.KeyboardEvent<HTMLImageElement>) => {
      e.preventDefault();
      const event: InputEvent = {
        type: "key_up",
        name: domKeyToXdotool(e.nativeEvent),
      };
      void computerInputEvent(botId, event);
    },
    [botId],
  );

  // --- render ---
  const wrapClass = "computer-panel computer-panel__preview";

  if (loading) {
    return (
      <div className={wrapClass} data-mode="preview" data-testid="computer-panel">
        <ComputerToolbar
          computer={null}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
          driving={driving}
          onDriveToggle={() => void handleDrive()}
          disabled
        />
        <div className="computer-panel__body computer-panel__body--loading">
          <div className="computer-panel__spinner" />
          <div>Loading computer…</div>
        </div>
      </div>
    );
  }

  if (!computer) {
    // v3.7.5: previously this branch told the user "go to
    // the Bot editor" — but the Bot editor doesn't have a
    // Provision button either, so users destroyed a VM and
    // then had to make a new Bot to get a working VM back.
    // Real fix: put a Provision button right here. Same
    // code path as the "VM not provisioned" branch
    // (`handleProvision` -> `computerProvision`), just
    // from a different starting state. Settings carry the
    // disk/RAM defaults so the user doesn't have to
    // re-enter them.
    return (
      <div className={wrapClass} data-mode="preview" data-testid="computer-panel">
        <ComputerToolbar
          computer={null}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
          driving={driving}
          onDriveToggle={() => void handleDrive()}
          disabled
        />
        <div className="computer-panel__body computer-panel__body--empty">
          <div className="computer-panel__empty-title">No computer</div>
          <div className="computer-panel__empty-detail">
            {domainNotFound ? (
              <>
                The libvirt domain
                {domainNotFound ? ` "${domainNotFound}"` : ""} is missing
                on the host. Click Provision to create a fresh VM, or
                Destroy + re-provision to start over.
              </>
            ) : (
              <>
                This Bot doesn't have a VM right now — either it was
                never provisioned, or you just clicked Destroy. Click
                Provision to spin up a new VM and keep this Bot's
                identity, conversations, and memory.
              </>
            )}
          </div>
          <div
            className="computer-panel__actions"
            style={{
              display: "flex",
              gap: 8,
              marginTop: 12,
            }}
          >
            <button
              type="button"
              className="computer-panel__primary"
              onClick={handleProvision}
              disabled={actionPending}
              data-testid="computer-empty-provision"
            >
              {actionPending ? "Provisioning…" : "Provision"}
            </button>
            <button
              type="button"
              className="computer-panel__secondary"
              onClick={handleDestroyAndReprovision}
              disabled={actionPending}
              data-testid="computer-empty-destroy-reprovision"
            >
              {actionPending ? "Working…" : "Destroy + re-provision"}
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (computer.state === "provisioning") {
    return (
      <div className={wrapClass} data-mode="preview" data-testid="computer-panel">
        <ComputerToolbar
          computer={computer}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
          driving={driving}
          onDriveToggle={() => void handleDrive()}
          disabled
        />
        <div className="computer-panel__body computer-panel__body--loading">
          <div className="computer-panel__spinner" />
          <div>Provisioning VM…</div>
          <div className="computer-panel__sub">
            Spinning up the libvirt domain, generating SSH keys, and waiting
            for cloud-init. This usually takes 1–2 minutes.
          </div>
        </div>
      </div>
    );
  }

  if (computer.state === "error") {
    return (
      <div className={wrapClass} data-mode="preview" data-testid="computer-panel">
        <ComputerToolbar
          computer={computer}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
          driving={driving}
          onDriveToggle={() => void handleDrive()}
          disabled={actionPending}
        />
        <div className="computer-panel__body computer-panel__body--error">
          <div className="computer-panel__error-title">
            Computer error
          </div>
          <div className="computer-panel__error-detail">
            {errorMsg ?? "The VM is in an error state. Try Restart, or Destroy and re-provision."}
          </div>
        </div>
      </div>
    );
  }

  // v3.7.2 (amended): the libvirt domain is missing on
  // the host. The Bot exists in MaxBot's SQLite, the
  // row says `state === "running"` (because the local
  // row is the source of truth for the UI), but the
  // host's `virsh screenshot` says the domain is
  // gone. Show a clear "VM not provisioned" state with
  // a Provision button (calls `computerProvision`) and
  // a secondary "Destroy + re-provision" link.
  //
  // The screenshot poll re-engages on the next render
  // after the VM transitions to `running` via the
  // `computer://state-changed` event, so the user
  // doesn't need to click anything to see the desktop
  // frame once provision completes.
  if (domainNotFound) {
    return (
      <div
        className={wrapClass}
        data-mode="preview"
        data-testid="computer-panel"
        data-domain-not-found="true"
      >
        <ComputerToolbar
          computer={computer}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
          driving={driving}
          onDriveToggle={() => void handleDrive()}
          disabled={actionPending}
        />
        <div
          className="computer-panel__body computer-panel__body--error"
          data-testid="computer-domain-not-found"
        >
          <div className="computer-panel__error-title">
            VM not provisioned
          </div>
          <div className="computer-panel__error-detail" style={{ opacity: 1 }}>
            The libvirt domain
            {domainNotFound ? ` "${domainNotFound}"` : ""} is missing on the
            host. This can happen if the VM was never provisioned, was
            destroyed outside of MaxBot, or lives on a different host. Click
            Provision to create the VM, or Destroy + re-provision to start
            fresh.
          </div>
          {errorMsg ? (
            <div
              className="computer-panel__error-detail"
              data-testid="computer-domain-not-found-error"
              style={{ marginTop: 8 }}
            >
              {errorMsg}
            </div>
          ) : null}
          <div
            className="computer-panel__actions"
            style={{
              display: "flex",
              gap: 8,
              marginTop: 12,
              alignItems: "center",
              justifyContent: "center",
              flexWrap: "wrap",
            }}
          >
            <button
              type="button"
              className="primary small"
              onClick={handleProvision}
              disabled={actionPending}
              data-testid="computer-provision-button"
            >
              {actionPending ? "Provisioning…" : "Provision"}
            </button>
            <button
              type="button"
              className="ghost small"
              onClick={handleDestroyAndReprovision}
              disabled={actionPending}
              data-testid="computer-destroy-reprovision-button"
            >
              {actionPending ? "Working…" : "Destroy + re-provision"}
            </button>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className={wrapClass} data-mode="preview" data-testid="computer-panel">
      <ComputerToolbar
        computer={computer}
        onClose={onClose}
        onStart={handleStart}
        onStop={handleStop}
        onRestart={handleRestart}
        onDestroy={handleDestroy}
        // v3.7.9: click-through takeover. The
        // single button flips between "Drive" and
        // "Hand back". The Driving action is
        // fire-and-forget; the panel's
        // `handleDrive` / `handleHandBack`
        // callbacks do the work.
        driving={driving}
        onDriveToggle={() =>
          driving ? void handleHandBack() : void handleDrive()
        }
        // v2.3.5: only show the "Use my default key"
        // button when the user is still on the per-Bot
        // key path. After a successful install the
        // setting flips and the next render hides it.
        onInstallDefaultKey={
          // v2.3.6: show the "Use my default key" button
          // whenever the VM is running, regardless of the
          // current setting. The QGA install is idempotent
          // (the `grep -qxF … || echo …` pipeline in
          // `install_default_key` makes it safe to re-run),
          // and this is the only UI path for the
          // chicken-and-egg case: a user with a v2.3.4 VM
          // (per-Bot key in authorized_keys) who upgrades to
          // v2.3.5 sees the setting flip to `true` via the
          // migration, but the per-Bot key is still the
          // only one in the VM. They need this button to
          // bootstrap. After a successful install, the
          // button stays visible (the user can re-run to
          // confirm) but is no-op.
          computer.state === "running"
            ? handleInstallDefaultKey
            : undefined
        }
        disabled={actionPending}
      />
      {/* v3.7.9: driving banner. Above the body
          so it sits between the toolbar and the
          screenshot. Sits only when `driving` is
          true; the in-banner "Hand back" button
          cascades to `handleHandBack` (which calls
          `onClose?.()` so the parent can decide). */}
      {driving && (
        <div
          className="computer-panel__driving-banner"
          data-testid="driving-banner"
        >
          <span>You are driving — bot input paused</span>
          <button
            type="button"
            className="primary small"
            onClick={() => void handleHandBack()}
            data-testid="computer-handback"
          >
            Hand back
          </button>
        </div>
      )}
      {drivingError && (
        <div
          className="computer-panel__error-detail"
          data-testid="driving-error"
        >
          {drivingError}
        </div>
      )}
      <div className="computer-panel__body">
        <div className="computer-panel__tabs" role="tablist">
          <button
            type="button"
            role="tab"
            aria-selected={bodyTab === "console"}
            className={
              "computer-panel__tab" +
              (bodyTab === "console" ? " computer-panel__tab--active" : "")
            }
            onClick={() => setBodyTab("console")}
            data-testid="computer-tab-console"
          >
            Console
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={bodyTab === "files"}
            className={
              "computer-panel__tab" +
              (bodyTab === "files" ? " computer-panel__tab--active" : "")
            }
            onClick={() => setBodyTab("files")}
            data-testid="computer-tab-files"
          >
            Files
          </button>
        </div>
        <div className="computer-panel__body-pane">
          {bodyTab === "console" ? (
            // v3.7.2: in-app preview is a screenshot
            // poll, not a noVNC stream. We render a
            // single `<img>` and let the poll effect
            // (above) update its `src`. Before the
            // first frame arrives, show a spinner so
            // the user knows we're still working.
            //
            // v3.7.9: the `<img>` is now the input
            // surface for click-through driving. The
            // handlers fire only when `driving` is
            // true and translate clientX/Y to
            // framebuffer coordinates.
            !firstFrame ? (
              <div className="computer-panel__body--loading">
                <div className="computer-panel__spinner" />
                <div>Connecting to VM…</div>
                {viewerError && (
                  <div
                    className="computer-panel__error-detail"
                    data-testid="viewer-error"
                  >
                    {viewerError}
                  </div>
                )}
              </div>
            ) : (
              <img
                ref={imgRef}
                src={frameUrl ?? undefined}
                alt="VM display"
                className="computer-panel__screenshot"
                draggable={false}
                data-testid="computer-screenshot"
                tabIndex={driving ? 0 : -1}
                style={{
                  objectFit: "contain",
                  cursor: driving ? "none" : "default",
                  userSelect: "none",
                }}
                onPointerDown={(e) => {
                  if (!driving || !imgRef.current) return;
                  imgRef.current.focus();
                  e.preventDefault();
                  mouseDownRef.current = true;
                  dispatchPointerEvent(e, "pointer_down");
                }}
                onPointerMove={handlePointerMove}
                onPointerUp={(e) => {
                  if (!driving || !imgRef.current) return;
                  mouseDownRef.current = false;
                  dispatchPointerEvent(e, "pointer_up");
                }}
                onPointerLeave={() => {
                  mouseDownRef.current = false;
                }}
                onWheel={handleWheel}
                onKeyDown={handleKeyDown}
                onKeyUp={handleKeyUp}
              />
            )
          ) : (
            <ComputerFileBrowser botId={botId} />
          )}
        </div>
        {/* v2.3.5: small inline confirmation after a
            successful "Use my default key" install. Sits
            above the footer so it doesn't fight the
            existing error banner for vertical space. */}
        {installConfirm ? (
          <div
            className="computer-panel__install-confirm"
            data-testid="computer-install-confirm"
            style={{
              padding: "6px 10px",
              background: "var(--accent-bg, #1e3a5f)",
              color: "var(--accent-fg, #cfe5ff)",
              fontSize: 12,
            }}
          >
            {installConfirm}
          </div>
        ) : null}
      </div>
      <ComputerFooter computer={computer} uptime={uptime} />
    </div>
  );
}

interface ComputerToolbarProps {
  computer: Computer | null;
  onClose?: () => void;
  onStart: () => void;
  onStop: () => void;
  onRestart: () => void;
  onDestroy: () => void;
  /** v3.7.9: single Drive / Hand back toggle.
   * The parent wires this to the right action
   * based on `driving`. */
  driving: boolean;
  onDriveToggle: () => void;
  /** v2.3.5: show the "Use my default key" button when
   * the user is still on the per-Bot key path. Clicking
   * it installs the user's default public key into the
   * VM via QGA and flips the setting. */
  onInstallDefaultKey?: () => void;
  disabled?: boolean;
}

function ComputerToolbar({
  computer,
  onClose,
  onStart,
  onStop,
  onRestart,
  onDestroy,
  driving,
  onDriveToggle,
  onInstallDefaultKey,
  disabled,
}: ComputerToolbarProps) {
  const state = computer?.state ?? null;
  // The Rust side emits "destroyed" as a transient terminal
  // state before the row is deleted. If we see it on a
  // panel that's still mounted (race with destroy), every
  // button is disabled.
  const stateDisabled = disabled || state === "provisioning";
  const stateClassName = `computer-panel__status-dot computer-panel__status-dot--${stateClass(state)}`;

  return (
    <div className="computer-panel__toolbar">
      <span className={stateClassName} title={state ?? "unknown"} />
      <span className="computer-panel__toolbar-title">
        {computer
          ? `${state ?? "unknown"} · ${computer.vm_ip ?? "no ip"} · up ${formatUptime(null)}`
          : "No computer"}
      </span>
      <span className="computer-panel__toolbar-spacer" />
      <button
        className="ghost small"
        onClick={onStart}
        disabled={stateDisabled || state === "running"}
        title="Start the VM"
        data-setting-key={`bot.${computer?.bot_id ?? ""}.computer-start`}
      >
        Start
      </button>
      <button
        className="ghost small"
        onClick={onStop}
        disabled={stateDisabled || state !== "running"}
        title="Shut down the VM (clean)"
        data-setting-key={`bot.${computer?.bot_id ?? ""}.computer-stop`}
      >
        Stop
      </button>
      <button
        className="ghost small"
        onClick={onRestart}
        disabled={stateDisabled || state !== "running"}
        title="Stop + start the VM"
        data-setting-key={`bot.${computer?.bot_id ?? ""}.computer-restart`}
      >
        Restart
      </button>
      {/* v2.3.5: only show when the VM is running (so the
        QGA is up) AND the user hasn't switched to the
        default-key path yet. After the install completes,
        the parent flips the setting and the button
        disappears on the next render. */}
      {onInstallDefaultKey && state === "running" && (
        <button
          className="ghost small"
          onClick={onInstallDefaultKey}
          disabled={stateDisabled}
          title="Install ~/.ssh/id_ed25519.pub into the VM's authorized_keys (QEMU guest agent — no SSH required)"
          data-testid="computer-install-default-key"
        >
          Use my default key
        </button>
      )}
      {/* v3.7.9: click-through takeover. The
          single button flips between "Drive" and
          "Hand back" based on the `driving` flag.
          The Rust side sets a per-Bot flag on
          `computerInputOpen`; the Bot's
          `vm_computer_use` tool refuses while
          it's true. The in-panel pointer / key
          events on the `<img>` are translated
          to xdotool commands and forwarded to
          the VM's X11 session. */}
      <button
        className={driving ? "primary small" : "ghost small"}
        onClick={onDriveToggle}
        disabled={stateDisabled || state !== "running"}
        title={
          driving
            ? "Stop driving, hand the VM back to the Bot"
            : "Drive the VM with your trackpad + keyboard"
        }
        data-testid="computer-drive"
      >
        {driving ? "Hand back" : "Drive"}
      </button>
      <button
        className="danger small"
        onClick={onDestroy}
        disabled={stateDisabled || !computer}
        title="Destroy the VM and remove its disk"
        data-setting-key={`bot.${computer?.bot_id ?? ""}.computer-destroy`}
      >
        Destroy
      </button>
      {onClose && (
        <button
          className="ghost small"
          onClick={onClose}
          title="Close preview"
        >
          Close
        </button>
      )}
    </div>
  );
}

interface ComputerFooterProps {
  computer: Computer;
  uptime: number | null;
}

function ComputerFooter({ computer, uptime }: ComputerFooterProps) {
  // v3.7.9: the footer is just the readout row now.
  // The "Hand back" / "Stop now" buttons that lived
  // here in v3.7.5/v3.7.7 are gone — driving mode
  // has its own in-body banner with a Hand back
  // button, and the abort path is the Approval
  // row's "Skip" / "Reject" in App.tsx.
  return (
    <div className="computer-panel__footer">
      <span className="computer-panel__footer-cell">{computer.vm_name}</span>
      <span className="computer-panel__footer-cell muted">
        VNC :{computer.vnc_port ?? "—"}
      </span>
      <span className="computer-panel__footer-cell muted">
        up {formatUptime(uptime)}
      </span>
    </div>
  );
}

// `formatUptime` is exported so the test can reuse the same
// "1h 2m / 12m / 47s" formatting for assertions. Status
// mode's uptime is also keyed off the same function.
export { formatUptime };
