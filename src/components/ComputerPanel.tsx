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
import type { Computer } from "../lib/api";
import {
  computerConsoleUrl,
  computerDestroy,
  computerGet,
  computerStart,
  computerStop,
  onComputerStateChanged,
} from "../lib/tauri";
import { NoVncViewer } from "./noVncViewer";

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
  const [actionPending, setActionPending] = useState(false);
  const [uptime, setUptime] = useState<number | null>(null);
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
          } catch (e) {
            // The viewer is unmounted; we just don't render
            // it. The next poll retries.
            setConsoleUrl(null);
            console.warn("computer_console_url failed:", e);
          }
        } else {
          setConsoleUrl(null);
        }
      } else {
        setConsoleUrl(null);
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
        disabled={actionPending}
      />
      <div className="computer-panel__body">
        {consoleUrl ? (
          <NoVncViewer
            wsUrl={consoleUrl}
            viewOnly={viewOnly}
            scaleViewport
            onError={(e) => setErrorMsg(e)}
          />
        ) : (
          <div className="computer-panel__body--loading">
            <div className="computer-panel__spinner" />
            <div>Opening console…</div>
          </div>
        )}
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
      >
        Start
      </button>
      <button
        className="ghost small"
        onClick={onStop}
        disabled={stateDisabled || state !== "running"}
        title="Shut down the VM (clean)"
      >
        Stop
      </button>
      <button
        className="ghost small"
        onClick={onRestart}
        disabled={stateDisabled || state !== "running"}
        title="Stop + start the VM"
      >
        Restart
      </button>
      <button
        className="danger small"
        onClick={onDestroy}
        disabled={stateDisabled || !computer}
        title="Destroy the VM and remove its disk"
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
