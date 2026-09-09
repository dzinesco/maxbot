import { useEffect, useState } from "react";
import {
  listControllableApps,
  openAutomationSettings,
  requestTccFor,
} from "../lib/tauri";
import type { ControllableApp } from "../lib/api";

type AppStatus = "unknown" | "granted" | "denied" | "pending";

/**
 * The "Computer Use" section in Settings. Shows the apps MaxBot can
 * control via AppleScript, lets the user pre-grant TCC permissions one
 * at a time, and surfaces the "Open System Settings" deep link as a
 * fallback. The status field is per-session: we re-probe on each
 * "Request access" click and surface the result.
 */
export function ComputerUseSettings() {
  const [apps, setApps] = useState<ControllableApp[]>([]);
  const [status, setStatus] = useState<Record<string, AppStatus>>({});
  const [messages, setMessages] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<Record<string, boolean>>({});
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    (async () => {
      try {
        const list = await listControllableApps();
        setApps(list);
        // Initial probe: walk the list and run a quick check. This may
        // surface the first-run prompt for each app — that prompt can
        // only handle one app at a time, so we don't auto-probe
        // everything; we wait for the user to click "Request access"
        // per app. We mark everything as "unknown" on load.
        const init: Record<string, AppStatus> = {};
        for (const a of list) init[a.key] = "unknown";
        setStatus(init);
      } finally {
        setLoading(false);
      }
    })();
  }, []);

  const handleRequest = async (app: ControllableApp) => {
    setBusy((b) => ({ ...b, [app.key]: true }));
    setStatus((s) => ({ ...s, [app.key]: "pending" }));
    setMessages((m) => ({ ...m, [app.key]: "" }));
    try {
      const result = await requestTccFor(app.key);
      setStatus((s) => ({
        ...s,
        [app.key]: result.granted ? "granted" : "denied",
      }));
      setMessages((m) => ({ ...m, [app.key]: result.message }));
    } catch (e) {
      setStatus((s) => ({ ...s, [app.key]: "denied" }));
      setMessages((m) => ({ ...m, [app.key]: String(e) }));
    } finally {
      setBusy((b) => ({ ...b, [app.key]: false }));
    }
  };

  const handleOpenSettings = async () => {
    try {
      await openAutomationSettings();
    } catch (e) {
      alert(`Could not open System Settings: ${e}`);
    }
  };

  if (loading) {
    return (
      <div className="field">
        <label>Computer Use</label>
        <div className="muted small">Loading…</div>
      </div>
    );
  }

  return (
    <div className="field">
      <label>Computer Use (AppleScript)</label>
      <div className="hint">
        MaxBot can drive macOS apps via AppleScript. macOS requires you
        to grant per-app permission the first time; click{" "}
        <em>Request access</em> below for each app you want to use. If
        a prompt doesn't appear, or you want to review all permissions
        at once, use <em>Open Automation Settings</em>.
      </div>
      <ul className="cu-app-list">
        {apps.map((app) => {
          const s = status[app.key] ?? "unknown";
          const msg = messages[app.key];
          return (
            <li key={app.key} className="cu-app-row">
              <div className="cu-app-main">
                <div className="cu-app-name">
                  {app.display_name}
                  <span className={`cu-status cu-status-${s}`}>
                    {s === "granted"
                      ? "granted"
                      : s === "denied"
                        ? "denied"
                        : s === "pending"
                          ? "checking…"
                          : "not yet"}
                  </span>
                </div>
                <div className="cu-app-desc small muted">{app.description}</div>
                {msg && s === "denied" && (
                  <div className="cu-app-msg small">{msg}</div>
                )}
              </div>
              <button
                className="ghost small"
                disabled={busy[app.key]}
                onClick={() => handleRequest(app)}
                title={`Run a probe AppleScript to trigger the first-run prompt for ${app.display_name}`}
              >
                {busy[app.key] ? "Probing…" : "Request access"}
              </button>
            </li>
          );
        })}
      </ul>
      <div className="actions" style={{ marginTop: 8, justifyContent: "flex-start" }}>
        <button onClick={handleOpenSettings}>Open Automation Settings</button>
      </div>
    </div>
  );
}
