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
}

const MAX_RETRIES = 5;
const BASE_BACKOFF_MS = 500;

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
    },
    ref,
  ) {
    const containerRef = useRef<HTMLDivElement | null>(null);
    const rfbRef = useRef<InstanceType<typeof RFB> | null>(null);
    const [connected, setConnected] = useState(false);
    // Refs for the latest callbacks so we don't re-create the
    // RFB on every parent re-render.
    const onConnectRef = useRef(onConnect);
    const onDisconnectRef = useRef(onDisconnect);
    const onErrorRef = useRef(onError);
    onConnectRef.current = onConnect;
    onDisconnectRef.current = onDisconnect;
    onErrorRef.current = onError;
    // `wsUrl` lives in a ref for the same reason: when it
    // changes, we tear down + reconnect (handled by an effect).
    const urlRef = useRef(wsUrl);
    urlRef.current = wsUrl;
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

    // Imperative handle: forward sendKey / sendMouse to the
    // active RFB instance.
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

    useEffect(() => {
      mountedRef.current = true;
      cancelledRef.current = false;
      attemptsRef.current = 0;
      connect();
      return () => {
        mountedRef.current = false;
        cancelledRef.current = true;
        if (reconnectTimerRef.current !== null) {
          clearTimeout(reconnectTimerRef.current);
          reconnectTimerRef.current = null;
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
      // We intentionally only run on mount/unmount. The
      // `wsUrl` change handler below is a separate effect.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, []);

    // Reconnect on `wsUrl` change. Most common path: the
    // panel re-renders with a fresh console URL after the
    // Tauri side restarted the proxy.
    useEffect(() => {
      // Skip the first run (handled by the mount effect).
      const r = rfbRef.current;
      if (!r) return;
      cancelledRef.current = true;
      attemptsRef.current = 0;
      if (reconnectTimerRef.current !== null) {
        clearTimeout(reconnectTimerRef.current);
        reconnectTimerRef.current = null;
      }
      try {
        r.disconnect();
      } catch {
        // ignore
      }
      rfbRef.current = null;
      cancelledRef.current = false;
      connect();
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
        rfb = new RFB(el, urlRef.current, {
          // Reasonable defaults for a local WebSocket:
          // shared mode (other VNC viewers can attach),
          // no repeater, no credentials (the VNC password is
          // wired through x11vnc on the VM side and we don't
          // need it on the noVNC end since the Tauri proxy
          // runs on localhost).
        });
      } catch (e) {
        const message = e instanceof Error ? e.message : String(e);
        onErrorRef.current?.(`VNC connection failed: ${message}`);
        scheduleReconnect();
        return;
      }
      rfbRef.current = rfb;
      rfb.viewOnly = viewOnly;
      rfb.scaleViewport = scaleViewport;
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
        onConnectRef.current?.();
      });
      rfb.addEventListener("disconnect", (e) => {
        if (mountedRef.current) setConnected(false);
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
        onErrorRef.current?.("VNC security handshake failed");
      });
      rfb.addEventListener("clipboard", () => {
        // no-op for now; the renderer's preview doesn't
        // surface clipboard sync.
      });
    }

    function scheduleReconnect() {
      if (!mountedRef.current || cancelledRef.current) return;
      if (attemptsRef.current >= MAX_RETRIES) {
        onErrorRef.current?.(
          `Lost VNC connection — gave up after ${MAX_RETRIES} retries`,
        );
        return;
      }
      const attempt = ++attemptsRef.current;
      // Exponential backoff with a 500ms base: 500, 1000,
      // 2000, 4000, 8000ms.
      const delay = BASE_BACKOFF_MS * 2 ** (attempt - 1);
      reconnectTimerRef.current = setTimeout(() => {
        reconnectTimerRef.current = null;
        if (cancelledRef.current || !mountedRef.current) return;
        // noVNC owns the DOM children of the container;
        // clear them before constructing a fresh RFB so
        // the new instance gets a clean canvas.
        const el = containerRef.current;
        if (el) {
          while (el.firstChild) el.removeChild(el.firstChild);
        }
        connect();
      }, delay);
    }

    return (
      <div
        ref={containerRef}
        className="novnc-viewer"
        // The noVNC constructor appends its own canvas +
        // screen div into the container; this wrapper just
        // sets the layout box.
        style={{
          width: "100%",
          height: "100%",
          position: "relative",
          overflow: "hidden",
          background: "var(--bg-0)",
        }}
        data-connected={connected ? "true" : "false"}
      />
    );
  },
);
