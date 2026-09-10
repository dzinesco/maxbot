// ComputerPanel — the three-mode UI for the per-Bot VM
// (v2.0 Slice C).
//
// Modes (per the Grok Bot essay):
//   - status    : tiny chip (icon + dot + uptime). Used in the
//                 title bar (`App.tsx` chrome wires this; we
//                 just expose the standalone component).
//   - preview   : pinned side panel, ~30% width, noVNC viewer.
//                 Bot is still driving; viewer is read-only by
//                 default (the user can choose to take input).
//   - takeover  : full-window noVNC viewer with a "Hand back
//                 to Bot" button. Local input is on (the user
//                 can use the mouse + keyboard); noVNC is a
//                 passive viewer so the Bot can resume control
//                 regardless of whether the human has the mouse.
//
// Data flow:
//   1. `computerGet(botId)` is called on mount and every 5s in
//      Preview / Takeover mode.
//   2. We also subscribe to the `computer://state-changed`
//      event so a state transition (e.g. start/stop from the
//      toolbar) flips the panel without waiting for the next
//      poll.
//   3. The noVNC viewer is only mounted once
//      `computerConsoleUrl(botId)` resolves to a URL — that
//      also gives us the per-call port the Tauri side picked
//      from `Settings.computer_vnc_local_port_range`.
//   4. Toolbar buttons (Start / Stop / Restart / Destroy) call
//      the corresponding Tauri commands; they disable while
//      the VM is in `provisioning` (or `error`).
//
// Restart is a thin convenience: Stop + Start with a small
// delay between them so libvirt's `virsh start` after a clean
// shutdown doesn't race with the shutdown.

import { useCallback, useEffect, useRef, useState } from "react";
import type { Computer, Settings } from "../lib/api";
import {
  computerConsoleUrl,
  computerDestroy,
  computerGet,
  computerInstallDefaultKey,
  computerStart,
  computerStop,
  getSettings,
  onComputerStateChanged,
  saveSettings,
} from "../lib/tauri";
import { NoVncViewer } from "./noVncViewer";
import { ComputerFileBrowser } from "./ComputerFileBrowser";

export type ComputerMode = "status" | "preview" | "takeover";

export interface ComputerPanelProps {
  botId: string;
  mode: ComputerMode;
  /** Used in preview / takeover modes. Called when the user
   * clicks the close button (or "Hand back to Bot"). */
  onClose?: () => void;
  /** Override the default poll interval (ms). The plan calls
   * for 5s; tests pass a smaller value. */
  pollIntervalMs?: number;
}

const DEFAULT_POLL_MS = 5000;

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
      mode={mode}
      onClose={onClose}
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
  mode: "preview" | "takeover";
  onClose?: () => void;
  pollIntervalMs: number;
}

function FullComputerPanel({
  botId,
  mode,
  onClose,
  pollIntervalMs,
}: FullComputerPanelProps) {
  const [computer, setComputer] = useState<Computer | null>(null);
  const [loading, setLoading] = useState(true);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [consoleUrl, setConsoleUrl] = useState<string | null>(null);
  // `consoleUrlError` is set when the Tauri `computer_console_url`
  // command itself fails — typically because the SSH tunnel
  // child exited before the readiness window elapsed (auth
  // failed, no passphrase, etc.). We surface this in the
  // Console tab body so the user sees a real reason instead
  // of a black noVNC canvas.
  const [consoleUrlError, setConsoleUrlError] = useState<string | null>(null);
  // `viewerError` is the noVNC RFB/WebSocket error. Distinct
  // from `errorMsg`, which is reserved for the computer.state
  // === "error" path. The viewer's failure is the most
  // common failure mode in practice (VNC password, tunnel
  // drop, etc.) and previously rendered a silent blank
  // viewer with no visible feedback.
  const [viewerError, setViewerError] = useState<string | null>(null);
  const [actionPending, setActionPending] = useState(false);
  const [uptime, setUptime] = useState<number | null>(null);
  // Body tab: "console" (noVNC viewer) vs "files" (SFTP
  // browser). The Files tab only does real work when the
  // VM has an SSH endpoint and a known ssh_key row, but
  // the browser is mounted regardless and shows its own
  // error if the listing fails.
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
  // `lastSeenAt` is an ISO string; we tick once a second to
  // refresh the displayed uptime in status / preview modes.
  const lastSeenAt = computer?.last_seen_at ?? null;
  const uptimeTimer = useRef<ReturnType<typeof setInterval> | null>(null);

  // --- data loading ---
  // `loadComputer` fetches the persisted row. In preview /
  // takeover mode we also kick off the console URL on the
  // first successful load.
  const loadComputer = useCallback(async () => {
    try {
      const c = await computerGet(botId);
      setComputer(c);
      setErrorMsg(null);
      if (c) {
        // Fire-and-forget the console URL. We re-fetch on
        // every poll so the proxy stays fresh (the Tauri
        // side spawns a new proxy per `console_url` call;
        // the prior one dies when the WS closes).
        if (c.state === "running" && c.vnc_port !== null) {
          try {
            const url = await computerConsoleUrl(botId);
            setConsoleUrl(url);
            setConsoleUrlError(null);
          } catch (e) {
            // The Rust side waited for the SSH tunnel to
            // be ready (3s window) and refused to return a
            // URL because the tunnel child had already
            // exited. Surface the real reason — usually
            // the passphrase is missing, or the key was
            // rejected, or the host key didn't match.
            setConsoleUrl(null);
            setConsoleUrlError(String(e));
          }
        } else {
          setConsoleUrl(null);
          setConsoleUrlError(null);
        }
      } else {
        setConsoleUrl(null);
        setConsoleUrlError(null);
      }
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

  // --- render ---
  const wrapClass =
    mode === "takeover"
      ? "computer-panel computer-panel__takeover"
      : "computer-panel computer-panel__preview";
  const viewOnly = mode === "preview";

  if (loading) {
    return (
      <div className={wrapClass} data-mode={mode} data-testid="computer-panel">
        <ComputerToolbar
          mode={mode}
          computer={null}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
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
    return (
      <div className={wrapClass} data-mode={mode} data-testid="computer-panel">
        <ComputerToolbar
          mode={mode}
          computer={null}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
          disabled
        />
        <div className="computer-panel__body computer-panel__body--empty">
          <div className="computer-panel__empty-title">No computer</div>
          <div className="computer-panel__empty-detail">
            This Bot doesn't have a VM yet. Create one in the Bot editor
            ("Provision a computer") to give it a Linux desktop of its own.
          </div>
        </div>
      </div>
    );
  }

  if (computer.state === "provisioning") {
    return (
      <div className={wrapClass} data-mode={mode} data-testid="computer-panel">
        <ComputerToolbar
          mode={mode}
          computer={computer}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
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
      <div className={wrapClass} data-mode={mode} data-testid="computer-panel">
        <ComputerToolbar
          mode={mode}
          computer={computer}
          onClose={onClose}
          onStart={handleStart}
          onStop={handleStop}
          onRestart={handleRestart}
          onDestroy={handleDestroy}
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

  return (
    <div className={wrapClass} data-mode={mode} data-testid="computer-panel">
      <ComputerToolbar
        mode={mode}
        computer={computer}
        onClose={onClose}
        onStart={handleStart}
        onStop={handleStop}
        onRestart={handleRestart}
        onDestroy={handleDestroy}
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
            consoleUrl ? (
              <NoVncViewer
                wsUrl={consoleUrl}
                viewOnly={viewOnly}
                scaleViewport
                onError={(e) => setViewerError(e)}
                onConnect={() => setViewerError(null)}
              />
            ) : consoleUrlError ? (
              <div
                className="computer-panel__body--error"
                data-testid="console-url-error"
              >
                <div className="computer-panel__error-title">
                  Console unavailable
                </div>
                <div className="computer-panel__error-detail">
                  {consoleUrlError}
                </div>
                <div
                  className="computer-panel__error-detail"
                  style={{ opacity: 0.7, marginTop: 8 }}
                >
                  {/* v2.3.6: hint updated. v2.3.5 hides the
                    passphrase field when the default-key
                    flag is on, so the v2.3.4 hint
                    ("check the passphrase") is no longer
                    actionable for the common case. Point
                    users at the install-default-key button
                    (which is always visible on a running
                    VM) and the terminal smoke test. */}
                  Most common cause: the SSH tunnel could not
                  authenticate. Try the terminal smoke test
                  (<code>ssh crispy</code>) from your shell —
                  if that works, click "Use my default key"
                  in the toolbar to bootstrap the VM's
                  <code>authorized_keys</code> for MaxBot.
                </div>
              </div>
            ) : (
              <div className="computer-panel__body--loading">
                <div className="computer-panel__spinner" />
                <div>Opening console…</div>
              </div>
            )
          ) : (
            <ComputerFileBrowser botId={botId} />
          )}
        </div>
        {bodyTab === "console" && consoleUrl && viewerError ? (
          <div className="computer-panel__viewer-error" data-testid="viewer-error">
            <div className="computer-panel__error-title">Console error</div>
            <div className="computer-panel__error-detail">{viewerError}</div>
            <div className="computer-panel__error-detail" style={{ opacity: 0.7, marginTop: 8 }}>
              The WebSocket proxy may have failed to start, the SSH tunnel
              may have dropped, or the VNC server may have refused the
              connection. Try Hand back and re-open, or Restart the
              computer.
            </div>
          </div>
        ) : null}
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
      <ComputerFooter
        mode={mode}
        computer={computer}
        uptime={uptime}
        onHandBack={onClose}
      />
    </div>
  );
}

interface ComputerToolbarProps {
  mode: ComputerMode;
  computer: Computer | null;
  onClose?: () => void;
  onStart: () => void;
  onStop: () => void;
  onRestart: () => void;
  onDestroy: () => void;
  /** v2.3.5: show the "Use my default key" button when
   * the user is still on the per-Bot key path. Clicking
   * it installs the user's default public key into the
   * VM via QGA and flips the setting. */
  onInstallDefaultKey?: () => void;
  disabled?: boolean;
}

function ComputerToolbar({
  mode,
  computer,
  onClose,
  onStart,
  onStop,
  onRestart,
  onDestroy,
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
          title={mode === "takeover" ? "Hand back to Bot" : "Close preview"}
        >
          {mode === "takeover" ? "Hand back to Bot" : "Close"}
        </button>
      )}
    </div>
  );
}

interface ComputerFooterProps {
  mode: ComputerMode;
  computer: Computer;
  uptime: number | null;
  onHandBack?: () => void;
}

function ComputerFooter({
  mode,
  computer,
  uptime,
  onHandBack,
}: ComputerFooterProps) {
  return (
    <div className="computer-panel__footer">
      <span className="computer-panel__footer-cell">
        {computer.vm_name}
      </span>
      <span className="computer-panel__footer-cell muted">
        VNC :{computer.vnc_port ?? "—"}
      </span>
      <span className="computer-panel__footer-cell muted">
        up {formatUptime(uptime)}
      </span>
      <span className="computer-panel__footer-spacer" />
      {mode === "takeover" && (
        <span className="computer-panel__footer-cell muted">
          takeover active — keyboard + mouse captured
        </span>
      )}
      {onHandBack && mode !== "takeover" && (
        <button className="ghost small" onClick={onHandBack}>
          Close
        </button>
      )}
    </div>
  );
}

// `formatUptime` is exported so the test can reuse the same
// "1h 2m / 12m / 47s" formatting for assertions. Status
// mode's uptime is also keyed off the same function.
export { formatUptime };
