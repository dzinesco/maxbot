// Hand-rolled type declarations for `@novnc/novnc` (the package
// ships no `.d.ts`). Only the surface the renderer actually
// uses is typed; everything else falls back to a permissive
// `any` via the `[key: string]: any` index signature.
//
// Reference: https://github.com/novnc/noVNC/blob/master/docs/API.md
// and `core/rfb.js` in the package.

declare module "@novnc/novnc" {
  export interface RFBOptions {
    /** Optional VNC credentials, used by the RFB when the
     * server demands authentication. The renderer's local
     * proxy doesn't trigger this, but we keep the type
     * accurate for direct-from-VNC-server usage. */
    credentials?: { password?: string; username?: string };
    /** Shared mode (allow other VNC viewers to attach). */
    shared?: boolean;
    /** Repeater ID for VNC repeater setups. */
    repeaterID?: string;
    /** Sub-protocols the WebSocket should advertise. */
    wsProtocols?: string[];
  }

  export interface RFBDisconnectDetail {
    clean: boolean;
  }

  /**
   * The default export of `@novnc/novnc/core/rfb.js`. The RFB
   * class extends an internal `EventTargetMixin`, so consumers
   * can `addEventListener("connect", ...)`, etc.
   */
  export default class RFB {
    constructor(
      target: HTMLElement,
      urlOrChannel: string,
      options?: RFBOptions,
    );

    /** Disable / enable local input. The VNC server still
     * receives the user's mouse + keyboard when the canvas
     * has focus; `viewOnly = true` makes the viewer passive. */
    viewOnly: boolean;
    /** Scale the remote desktop to fit the canvas. */
    scaleViewport: boolean;
    /** Clip the viewport to the canvas's CSS box. */
    clipViewport: boolean;
    /** Show the remote server's local cursor (vs. the
     * noVNC default local-only cursor). */
    showCursor: boolean;
    /** Whether clicking the canvas captures focus. */
    focusOnClick: boolean;
    /** Background CSS color for the empty area around the
     * remote desktop. */
    background: string;

    /** Send a key event. `keysym` is the X11 keysym integer;
     * `code` is the DOM `KeyboardEvent.code`; `down` is
     * true for press, false for release. */
    sendKey(keysym: number, code: string, down: boolean): void;

    /** Send a mouse event. `x` / `y` are framebuffer
     * coordinates; `buttonMask` is the RFB bitmask
     * (1 = left, 2 = middle, 4 = right, 8.. for extra
     * buttons per RFB §5.4.3). */
    sendMouse(x: number, y: number, buttonMask: number): void;

    /** Send the Ctrl-Alt-Del key chord (useful when the
     * remote OS is at a login screen). */
    sendCtrlAltDel(): void;

    /** Cleanly close the WebSocket. */
    disconnect(): void;

    /** Approve a TLS fingerprint challenge (RSA-AES
     * authentication). */
    approveServer(): void;

    /** Update credentials mid-handshake. */
    sendCredentials(creds: { password?: string; username?: string }): void;

    /** Machine power operations (XVP). */
    machineShutdown(): void;
    machineReboot(): void;
    machineReset(): void;

    // EventTarget-shaped — the underlying mixin uses
    // `addEventListener` with custom event names: "connect",
    // "disconnect", "securityfailure", "clipboard",
    // "bell", "desktopname", "resize", "focus", "blur",
    // "credentialsrequired", "serververification", etc.
    addEventListener(
      type: string,
      listener: (ev: Event | CustomEvent) => void,
    ): void;
    removeEventListener(
      type: string,
      listener: (ev: Event | CustomEvent) => void,
    ): void;

    // Permissive catch-all for the rest of the RFB API
    // (the noVNC library has more getters/methods than we
    // need to type here).
    [key: string]: any;
  }
}
