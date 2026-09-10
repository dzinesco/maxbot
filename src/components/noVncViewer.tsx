// React wrapper around the `@novnc/novnc` RFB client.
//
// The Tauri side starts a per-Bot SSH-tunneled WebSocket↔RFB proxy
// on a local port (see `src-tauri/src/computer/vnc.rs`) and returns
// a `ws://localhost:<port>/` URL via `computer_console_url`. This
// component:
//
//   1. Constructs the noVNC `RFB` against a target <div> (noVNC
//      creates its own canvas inside).
//   2. Surfaces connect / disconnect / error via props (so the
//      panel can show a loading state).
//   3. Reconnects with exponential backoff if the WebSocket
//      drops (max 5 attempts before giving up).
//   4. Exposes `sendKey` / `sendMouse` on a forwarded ref so a
//      parent (Takeover mode) can drive input programmatically
//      (noVNC's own keyboard/mouse handlers do the normal
//      take-the-mouse path automatically).
//   5. (v3.0.7) Waits for the first framebuffer event before
//      dropping the "Connecting to VM…" overlay. The RFB
//      `connect` event fires on local WS open, which can
//      succeed even when the SSH-tunneled VNC stream behind it
//      is dead — leaving the canvas black with no overlay. The
//      new `firstFrameReceived` state is set true on the first
//      `desktopname`, `resize`, or canvas-pixel event. A 6s
//      no-frame timeout surfaces a red error overlay and
//      triggers a reconnect instead of leaving a silent black
//      canvas.
//
// Bundled via the `@novnc/novnc` package — no CDN. The package
// only ships `core/rfb.js` (+ dependencies under `core/` and
// `vendor/`); importing the default export gives us the `RFB`
// class directly.

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";

// `RFB` is a default export of the package's main entry. Vite
// resolves `@novnc/novnc` to `./core/rfb.js` (per the package's
// `exports` field) and we get the class itself.
import RFB from "@novnc/novnc";

export interface NoVncViewerHandle {
  /** Send a synthesized key event. Keysym is the X11 keysym
   * integer; code is the DOM `KeyboardEvent.code` string (e.g.
   * "KeyA"). `down` distinguishes press/release. */
  sendKey(keysym: number, code: string, down: boolean): void;
  /** Send a synthesized mouse event. Position is in the
   * framebuffer's coordinate space (not the canvas's CSS px);
   * buttonMask is a bitmask (1=Left, 2=Middle, 4=Right, 8.. for
   * additional buttons per RFB §5.4.3). */
  sendMouse(x: number, y: number, buttonMask: number): void;
  /** Whether the WebSocket is currently connected. */
  isConnected(): boolean;
}

export interface NoVncViewerProps {
  /** `ws://localhost:<port>/` URL returned by the
   * `computer_console_url` Tauri command. */
  wsUrl: string;
  /** Fired once the RFB handshake completes and the first
   * framebuffer has been received. */
  onConnect?: () => void;
  /** Fired on a clean disconnect (including our own unmount). */
  onDisconnect?: () => void;
  /** Fired on a non-recoverable error after exhausting the
   * reconnect budget. The string is the human-readable message. */
  onError?: (e: string) => void;
  /** Show the remote server's local cursor (vs the noVNC
   * default local-only cursor). Default false. */
  showRemoteCursor?: boolean;
  /** Disable local input (read-only viewer). Used in Preview
   * mode where the Bot is still driving. */
  viewOnly?: boolean;
  /** Scale the remote desktop to fit the canvas. Default true. */
  scaleViewport?: boolean;
  /** (v3.0.7) Optional VNC credentials. The viewer responds
   * to the RFB's `credentialsrequired` event by calling
   * `sendCredentials(password)`. If the prop is omitted the
   * default noVNC password dialog would appear (which we
   * don't want — the Tauri side handles auth in 99% of
   * cases, and the rare VNC-password VM is wired through
   * this prop). */
  credentials?: { password: string };
}

const MAX_RETRIES = 5;
const BASE_BACKOFF_MS = 500;
// (v3.0.7) Floor for the "Connecting to VM…" overlay so the
// user has time to read it even if the RFB handshake and
// the first framebuffer pixel both happen immediately.
const MIN_OVERLAY_MS = 3000;
// (v3.0.7) After `connect` fires, give the framebuffer this
// long to actually paint a pixel. If it doesn't, surface a
// "no framebuffer received" error and reconnect — the most
// common cause is a dead SSH tunnel behind a working local
// WebSocket, which previously left a silent black canvas.
const NO_FRAME_TIMEOUT_MS = 6000;

export const NoVncViewer = forwardRef<NoVncViewerHandle, NoVncViewerProps>(
  function NoVncViewer(
    {
      wsUrl,
      onConnect,
      onDisconnect,
      onError,
      showRemoteCursor = false,
      viewOnly = false,
      scaleViewport = true,
      credentials,
    },
    ref,
  ) {
    const containerRef = useRef<HTMLDivElement | null>(null);
    const rfbRef = useRef<InstanceType<typeof RFB> | null>(null);
    const [connected, setConnected] = useState(false);
    // (v3.0.7) The "have we actually seen the framebuffer
    // paint anything?" signal. Distinct from `connected`,
    // which is just "the local WebSocket is open". The
    // "Connecting to VM…" overlay stays up until both
    // `connected` AND `firstFrameReceived` are true (and the
    // MIN_OVERLAY_MS floor has elapsed). The no-frame timeout
    // fires if `firstFrameReceived` is still false
    // NO_FRAME_TIMEOUT_MS after `connect`.
    const [firstFrameReceived, setFirstFrameReceived] = useState(false);
    const [lastError, setLastError] = useState<string | null>(null);
    const [showOverlay, setShowOverlay] = useState(true);
    // Refs for the latest callbacks so we don't re-create the
    // RFB on every parent re-render.
    const onConnectRef = useRef(onConnect);
    const onDisconnectRef = useRef(onDisconnect);
    const onErrorRef = useRef(onError);
    onConnectRef.current = onConnect;
    onDisconnectRef.current = onDisconnect;
    onErrorRef.current = onError;
    // Reconnect bookkeeping: how many attempts have we made
    // for the current `wsUrl`?
    const attemptsRef = useRef(0);
    // Scheduled reconnect handle — kept so we can cancel on
    // unmount or before a fresh connect.
    const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
      null,
    );
    // Mounted flag — when false, the cleanup runs and we stop
    // scheduling reconnects.
    const mountedRef = useRef(true);
    // Manual cancellation flag — set when the parent switches
    // `wsUrl` or unmounts.
    const cancelledRef = useRef(false);
    // Minimum-show timer for the overlay. We keep the
    // "Connecting to VM…" message on screen for at least
    // MIN_OVERLAY_MS after every fresh mount, even if the
    // RFB fires `connect` and the framebuffer streams
    // immediately, so the user has a chance to read it.
    // The timer is reset on every mount / wsUrl change.
    const minShowTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
      null,
    );
    // (v3.0.7) No-frame timeout. Scheduled when `connect`
    // fires; cleared on the first framebuffer event or on
    // cleanup. If it fires, we surface an error and call
    // `scheduleReconnect()` — a dead SSH tunnel behind a
    // working local WebSocket is the most common cause.
    const noFrameTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
      null,
    );
    // (v3.0.7) ResizeObserver that re-applies
    // `rfb.scaleViewport = true` when the container's
    // intrinsic size changes. noVNC often paints 0×0 until
    // a resize event after the initial layout settles; the
    // observer also catches panel-resize during drag.
    const resizeObserverRef = useRef<ResizeObserver | null>(null);
    // (v3.0.7) Ref mirror of `firstFrameReceived` so the
    // `connect` closure (which runs once per RFB
    // construction) can read the current value without
    // having to re-create the RFB on every state update.
    const firstFrameReceivedRef = useRef(false);
    firstFrameReceivedRef.current = firstFrameReceived;

    // Imperative handle: forward sendKey / sendMouse to the
    // active RFB instance. v3.0.7 preserves the existing API.
    useImperativeHandle(
      ref,
      () => ({
        sendKey(keysym, code, down) {
          rfbRef.current?.sendKey(keysym, code, down);
        },
        sendMouse(x, y, buttonMask) {
          // The RFB class exposes `_sendMouse` (private) but
          // `sendMouse` is the public path used by the input
          // handler. The noVNC public API here mirrors what
          // its own internal mouse handler calls.
          const r = rfbRef.current as unknown as {
            _sendMouse?: (x: number, y: number, mask: number) => void;
            sendMouse?: (x: number, y: number, mask: number) => void;
          } | null;
          if (!r) return;
          if (typeof r.sendMouse === "function") {
            r.sendMouse(x, y, buttonMask);
          } else if (r._sendMouse) {
            r._sendMouse(x, y, buttonMask);
          }
        },
        isConnected() {
          return connected;
        },
      }),
      [connected],
    );

    // (v3.0.7) Single connect effect keyed on `wsUrl`. This
    // handles BOTH the initial mount (because the effect runs
    // once with the initial `wsUrl`) AND every reconnect
    // (because the parent re-renders with a fresh URL from
    // the Rust side). No "skip first run" branches, no split
    // mount + wsUrl effects — that's the path that races
    // under React 19 Strict Mode and leaves a stale RFB
    // around when a fresh URL arrives.
    useEffect(() => {
      mountedRef.current = true;
      cancelledRef.current = false;
      attemptsRef.current = 0;
      // Reset the per-mount signals: the overlay is up, no
      // framebuffer seen yet, no error rendered. The
      // min-show timer will allow the overlay to drop after
      // MIN_OVERLAY_MS, but no sooner.
      setShowOverlay(true);
      setFirstFrameReceived(false);
      setLastError(null);
      firstFrameReceivedRef.current = false;
      if (minShowTimerRef.current !== null) {
        clearTimeout(minShowTimerRef.current);
        minShowTimerRef.current = null;
      }
      if (noFrameTimerRef.current !== null) {
        clearTimeout(noFrameTimerRef.current);
        noFrameTimerRef.current = null;
      }
      minShowTimerRef.current = setTimeout(() => {
        minShowTimerRef.current = null;
        if (mountedRef.current) setShowOverlay(false);
      }, MIN_OVERLAY_MS);
      // (v3.0.7) Wire a ResizeObserver on the container so
      // a panel drag re-applies `scaleViewport` and forces
      // a fresh layout. The noVNC constructor captures the
      // container's initial size synchronously; if the
      // container is 0×0 at that moment (the common case
      // for a freshly-mounted panel inside a flex chain
      // that hasn't laid out yet), noVNC paints to a 0×0
      // canvas. Re-applying `scaleViewport` on every
      // resize event re-derives the canvas dimensions.
      const containerEl = containerRef.current;
      if (containerEl && typeof ResizeObserver !== "undefined") {
        const ro = new ResizeObserver(() => {
          const r = rfbRef.current;
          if (!r) return;
          r.scaleViewport = true;
        });
        ro.observe(containerEl);
        resizeObserverRef.current = ro;
      }
      connect();
      return () => {
        mountedRef.current = false;
        cancelledRef.current = true;
        if (reconnectTimerRef.current !== null) {
          clearTimeout(reconnectTimerRef.current);
          reconnectTimerRef.current = null;
        }
        if (minShowTimerRef.current !== null) {
          clearTimeout(minShowTimerRef.current);
          minShowTimerRef.current = null;
        }
        if (noFrameTimerRef.current !== null) {
          clearTimeout(noFrameTimerRef.current);
          noFrameTimerRef.current = null;
        }
        if (resizeObserverRef.current !== null) {
          resizeObserverRef.current.disconnect();
          resizeObserverRef.current = null;
        }
        const r = rfbRef.current;
        rfbRef.current = null;
        if (r) {
          try {
            r.disconnect();
          } catch {
            // noVNC's disconnect() can throw if the WebSocket
            // was never opened; safe to ignore on cleanup.
          }
        }
        // noVNC injects a `<div>` and a `<canvas>` into the
        // container on construction. Remove them so the next
        // mount doesn't see stale elements.
        const el = containerRef.current;
        if (el) {
          while (el.firstChild) el.removeChild(el.firstChild);
        }
      };
      // The connect function reads the latest `viewOnly`,
      // `scaleViewport`, `showRemoteCursor`, and `credentials`
      // via the props closure. We re-run the whole effect
      // when `wsUrl` changes (the only common case is a
      // fresh URL from the Tauri side after a VM restart),
      // and React's render before the effect picks up the
      // latest values for the other props too.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [wsUrl]);

    function connect() {
      if (cancelledRef.current || !mountedRef.current) return;
      const el = containerRef.current;
      if (!el) return;
      // Construct the RFB. The noVNC constructor kicks off
      // the WebSocket handshake synchronously — there is no
      // separate `connect()` call. We wrap it in try/catch
      // because the underlying `WebSocket(uri)` constructor
      // can throw in environments that don't ship a real
      // WebSocket implementation (jsdom in tests, certain
      // embedded webviews, etc.). Treat a constructor
      // failure the same as a disconnected WebSocket: report
      // it and let the reconnect path retry.
      let rfb: InstanceType<typeof RFB>;
      try {
        rfb = new RFB(el, wsUrl, {
          // (v3.0.7) Shared mode: allow other VNC viewers
          // to attach to the same session. The previous
          // comment said this was the intent but the
          // option wasn't actually passed — explicitly
          // pass it now so the second viewer (e.g. a
          // second preview tab) doesn't get an "exclusive
          // session" rejection.
          shared: true,
          // If the caller supplied a VNC password at
          // mount time, hand it to the constructor so
          // the RFB doesn't need to ask via
          // `credentialsrequired`. The fallback
          // `credentialsrequired` handler below still
          // works for the case where the server demands
          // a password mid-session.
          credentials: credentials
            ? { password: credentials.password }
            : undefined,
        });
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        const errMsg = `VNC connection failed: ${message}`;
        setLastError(errMsg);
        onErrorRef.current?.(errMsg);
        scheduleReconnect();
        return;
      }
      rfbRef.current = rfb;
      rfb.viewOnly = viewOnly;
      rfb.scaleViewport = scaleViewport;
      // (v3.0.7) Re-apply scaleViewport after the
      // container has been laid out. The constructor
      // captures the container's current size
      // synchronously, but at construction time the
      // surrounding flex chain often hasn't sized the
      // container yet — re-applying after a microtask
      // lets noVNC re-derive the canvas size against
      // the post-layout dimensions.
      queueMicrotask(() => {
        if (rfbRef.current === rfb) rfb.scaleViewport = true;
      });
      // The remote-cursor flag is set via the `showCursor`
      // setter; passing it in the options object is the
      // documented path.
      (rfb as unknown as { showCursor?: boolean }).showCursor =
        showRemoteCursor;
      // Background matches MaxBot's --bg-0 so empty space at
      // the edges of the remote screen doesn't pop visually.
      rfb.background = "rgb(14, 14, 16)";

      rfb.addEventListener("connect", () => {
        attemptsRef.current = 0;
        if (mountedRef.current) setConnected(true);
        // (v3.0.7) Schedule the no-frame timeout. If the
        // framebuffer doesn't paint within
        // NO_FRAME_TIMEOUT_MS, the user gets a real
        // error message instead of a silent black canvas
        // (the symptom of a working local WS but a dead
        // SSH tunnel behind it).
        if (noFrameTimerRef.current !== null) {
          clearTimeout(noFrameTimerRef.current);
          noFrameTimerRef.current = null;
        }
        noFrameTimerRef.current = setTimeout(() => {
          noFrameTimerRef.current = null;
          if (!mountedRef.current || cancelledRef.current) return;
          if (firstFrameReceivedRef.current) return;
          const errMsg =
            "VNC connected but no framebuffer received in " +
            Math.round(NO_FRAME_TIMEOUT_MS / 1000) +
            "s — check VM console / VNC server";
          setLastError(errMsg);
          onErrorRef.current?.(errMsg);
          // Treat as a failed connect: drop the
          // overlay so the red error message is
          // visible, and schedule a reconnect.
          setShowOverlay(false);
          scheduleReconnect();
        }, NO_FRAME_TIMEOUT_MS);
        // (v3.0.7) `connect` is NOT enough to call
        // `onConnect` anymore — that fires on
        // `firstFrameReceived`. Tell the parent the
        // socket is up via `setConnected` (it gates
        // the imperative ref's `isConnected()`), but
        // hold the onConnect callback until the
        // framebuffer is confirmed.
        setLastError(null);
      });
      rfb.addEventListener("disconnect", (e) => {
        if (mountedRef.current) setConnected(false);
        // (v3.0.7) Cancel any pending no-frame timer
        // when the connection drops.
        if (noFrameTimerRef.current !== null) {
          clearTimeout(noFrameTimerRef.current);
          noFrameTimerRef.current = null;
        }
        // Reset the first-frame signal so a
        // successful reconnect shows the overlay
        // again until the new framebuffer paints.
        setFirstFrameReceived(false);
        firstFrameReceivedRef.current = false;
        onDisconnectRef.current?.();
        // NoVNC's RFB fires `disconnect` even on our own
        // unmount. Check `cancelledRef` so we don't loop
        // trying to reconnect after we said goodbye.
        if (cancelledRef.current || !mountedRef.current) return;
        // Clean disconnects (the server politely closed the
        // socket) don't trigger a reconnect — the user is
        // expected to click "Hand back" or the Bot shuts
        // the VM down. Only reconnect on unclean drops.
        const detail = (e as CustomEvent<{ clean?: boolean }>).detail;
        if (detail?.clean) return;
        scheduleReconnect();
      });
      rfb.addEventListener("securityfailure", () => {
        const errMsg = "VNC security handshake failed";
        setLastError(errMsg);
        onErrorRef.current?.(errMsg);
        setShowOverlay(false);
      });
      // (v3.0.7) The RFB fires `credentialsrequired` when
      // the server demands a password mid-handshake. If
      // the caller passed a `credentials` prop, reply
      // immediately with the password. Without this
      // handler, noVNC shows its own password dialog —
      // not the right UX for a renderer inside Tauri.
      rfb.addEventListener("credentialsrequired", () => {
        if (credentials?.password) {
          rfb.sendCredentials({ password: credentials.password });
        } else {
          const errMsg =
            "VNC server requested credentials but no password was provided";
          setLastError(errMsg);
          onErrorRef.current?.(errMsg);
          setShowOverlay(false);
        }
      });
      // (v3.0.7) `desktopname` is the first event the RFB
      // fires after the server has actually responded with
      // the desktop name — strong evidence the
      // WS↔VNC chain is alive. The RFB also fires
      // `resize` once it knows the framebuffer
      // dimensions. Either of these counts as
      // "framebuffer received" for the purposes of
      // dropping the overlay.
      const markFirstFrame = () => {
        if (firstFrameReceivedRef.current) return;
        firstFrameReceivedRef.current = true;
        if (mountedRef.current) {
          setFirstFrameReceived(true);
          // Drop the overlay only after the min-show
          // floor has elapsed (the minShowTimerRef
          // handles the actual unsetting). If the
          // floor already elapsed while we were
          // waiting for the framebuffer, drop it now.
          if (minShowTimerRef.current === null) {
            setShowOverlay(false);
          }
        }
        if (noFrameTimerRef.current !== null) {
          clearTimeout(noFrameTimerRef.current);
          noFrameTimerRef.current = null;
        }
        // Now that the framebuffer is confirmed
        // streaming, fire the parent's onConnect.
        onConnectRef.current?.();
      };
      rfb.addEventListener("desktopname", markFirstFrame);
      rfb.addEventListener("resize", markFirstFrame);
      rfb.addEventListener("clipboard", () => {
        // no-op for now; the renderer's preview doesn't
        // surface clipboard sync.
      });
    }

    function scheduleReconnect() {
      if (!mountedRef.current || cancelledRef.current) return;
      if (attemptsRef.current >= MAX_RETRIES) {
        const errMsg = `Lost VNC connection — gave up after ${MAX_RETRIES} retries`;
        setLastError(errMsg);
        onErrorRef.current?.(errMsg);
        return;
      }
      const attempt = ++attemptsRef.current;
      // Exponential backoff with a 500ms base: 500, 1000,
      // 2000, 4000, 8000ms.
      const delay = BASE_BACKOFF_MS * 2 ** (attempt - 1);
      reconnectTimerRef.current = setTimeout(() => {
        reconnectTimerRef.current = null;
        if (cancelledRef.current || !mountedRef.current) return;
        // Drop the old RFB cleanly so the next
        // construction gets a fresh canvas.
        const r = rfbRef.current;
        rfbRef.current = null;
        if (r) {
          try {
            r.disconnect();
          } catch {
            // ignore
          }
        }
        // noVNC owns the DOM children of the container;
        // clear them before constructing a fresh RFB so
        // the new instance gets a clean canvas.
        const el = containerRef.current;
        if (el) {
          while (el.firstChild) el.removeChild(el.firstChild);
        }
        // (v3.0.7) Re-arm the per-mount signals so the
        // overlay is back up and the no-frame timer is
        // rescheduled.
        setShowOverlay(true);
        setFirstFrameReceived(false);
        setLastError(null);
        firstFrameReceivedRef.current = false;
        if (minShowTimerRef.current !== null) {
          clearTimeout(minShowTimerRef.current);
          minShowTimerRef.current = null;
        }
        minShowTimerRef.current = setTimeout(() => {
          minShowTimerRef.current = null;
          if (mountedRef.current) setShowOverlay(false);
        }, MIN_OVERLAY_MS);
        connect();
      }, delay);
    }

    // (v3.0.7) Derive the `data-show-overlay` signal from
    // BOTH the min-show timer AND the first-frame
    // signal. The overlay stays up until the floor
    // elapses AND the framebuffer paints. If a hard
    // error happens (`lastError` is set and no
    // framebuffer ever arrived), show the red error
    // overlay instead and drop the "Connecting to VM…"
    // message.
    const overlayActive =
      showOverlay || (!firstFrameReceived && !lastError);

    return (
      <div
        ref={containerRef}
        className="novnc-viewer"
        // The noVNC constructor appends its own canvas +
        // screen div into the container; this wrapper just
        // sets the layout box.
        style={{
          position: "relative",
          width: "100%",
          height: "100%",
          background: "var(--bg-0)",
          display: "flex",
        }}
        data-connected={connected ? "true" : "false"}
        data-show-overlay={overlayActive ? "true" : "false"}
      >
        {/* (v3.0.7) Centered red error overlay. Shown when
            `lastError` is set AND the framebuffer never
            arrived (the silent-black-canvas case). When
            the framebuffer arrives successfully the
            `lastError` is cleared and the overlay unmounts. */}
        {lastError && !firstFrameReceived ? (
          <div
            className="novnc-viewer__error"
            data-testid="novnc-viewer-error"
            role="alert"
          >
            <div className="novnc-viewer__error-title">
              Console error
            </div>
            <div className="novnc-viewer__error-detail">{lastError}</div>
          </div>
        ) : null}
      </div>
    );
  },
);
