// v2.8.0 — Always-on Daemon (24/7). The Sidebar's
// "Activity" section: a read-only, three-section list
// of recent activity (last 5 Bot runs, last 5 Skill
// runs, last 5 Approvals). The data is pulled from
// the existing `bot_runs` / `skill_runs` / `approvals`
// SQLite tables by the `list_recent_activity` Tauri
// command. We poll every 10s; the daemon is on the
// server, so its `bot_runs` rows show up in the
// ActivityFeed next time the Mac app polls.
//
// Empty state: "No activity yet — the daemon will
// populate this when it fires." (matches the spec in
// the v2.8 plan).

import { useEffect, useState } from "react";
import { listRecentActivity } from "../lib/tauri";
import type { ActivityFeed as ActivityFeedData } from "../lib/api";

const POLL_INTERVAL_MS = 10_000;

type FetchState =
  | { kind: "loading" }
  | { kind: "ready"; data: ActivityFeedData }
  | { kind: "stale"; data: ActivityFeedData; message: string }
  | { kind: "error"; message: string };

function statusLabel(status: string): string {
  switch (status) {
    case "succeeded":
      return "✓";
    case "failed":
      return "✗";
    case "cancelled":
      return "⊘";
    case "running":
      return "…";
    case "pending":
      return "…";
    case "approved":
      return "✓";
    case "rejected":
      return "✗";
    case "edited":
      return "✎";
    default:
      return status;
  }
}

function shortId(id: string): string {
  return id.length > 8 ? id.slice(0, 8) : id;
}

function relativeTime(iso: string): string {
  const then = new Date(iso).getTime();
  if (!Number.isFinite(then)) return "";
  const delta = Date.now() - then;
  if (delta < 60_000) return "just now";
  if (delta < 3_600_000) return `${Math.floor(delta / 60_000)}m ago`;
  if (delta < 86_400_000) return `${Math.floor(delta / 3_600_000)}h ago`;
  return `${Math.floor(delta / 86_400_000)}d ago`;
}

interface RowProps {
  id: string;
  primary: string;
  secondary: string;
  status: string;
  when: string;
  /** v3.1.0 — entry point that fired the run. Renders a small
   *  pill so the user can tell "I closed the app and the
   *  daemon still ran this" apart from "I clicked Run now
   *  in the app." `undefined` (legacy row) → "via app" — the
   *  default per the v3.1.0 column migration. */
  triggeredBy?: "app" | "daemon" | "webhook";
  /** v3.4.0 (Phase 5) — "Why this asked" reason
   *  surfaced inline in the audit log. Renders as a
   *  small italic line below the row's primary +
   *  secondary. `undefined` (legacy row, or a row
   *  for which the executor didn't compute a
   *  reason) renders nothing — the row keeps its
   *  previous shape so the activity feed doesn't
   *  grow past 1.5x its v3.3.0 line count. */
  reason?: string | null;
}

function triggeredByLabel(value: "app" | "daemon" | "webhook" | undefined): string {
  switch (value) {
    case "daemon":
      return "via daemon";
    case "webhook":
      return "via webhook";
    case "app":
    default:
      return "via app";
  }
}

function Row({ id, primary, secondary, status, when, triggeredBy, reason }: RowProps) {
  return (
    <li className="activity-row" data-testid="activity-row" data-row-id={id}>
      <span className={`activity-row-status status-${status}`}>
        {statusLabel(status)}
      </span>
      <span className="activity-row-primary">{primary}</span>
      <span className="activity-row-secondary">{secondary}</span>
      <span
        className={`activity-row-triggered-by triggered-by-${triggeredBy ?? "app"}`}
        data-testid="activity-row-triggered-by"
        data-triggered-by={triggeredBy ?? "app"}
      >
        {triggeredByLabel(triggeredBy)}
      </span>
      <span className="activity-row-when">{when}</span>
      {reason && reason.trim().length > 0 ? (
        <span
          className="activity-row-reason muted small"
          data-testid="activity-row-reason"
        >
          Why: {reason}
        </span>
      ) : null}
    </li>
  );
}

function isEmpty(d: ActivityFeedData): boolean {
  return d.bot_runs.length === 0 && d.skill_runs.length === 0 && d.approvals.length === 0;
}

export function ActivityFeed() {
  const [state, setState] = useState<FetchState>({ kind: "loading" });

  const refresh = async () => {
    try {
      const data = await listRecentActivity();
      setState({ kind: "ready", data });
    } catch (e) {
      // v3.0.0 polish: keep showing the last good data when
      // a poll fails, and surface the error as a small inline
      // "couldn't refresh" pill instead of replacing the
      // whole feed with an error message. The data was
      // already read on the first successful poll, so the
      // user can keep reading while we try again in 10s.
      setState((prev) => {
        if (prev.kind === "ready") {
          return { kind: "stale", data: prev.data, message: String(e) };
        }
        return { kind: "error", message: String(e) };
      });
    }
  };

  useEffect(() => {
    refresh();
    const interval = window.setInterval(refresh, POLL_INTERVAL_MS);
    return () => window.clearInterval(interval);
  }, []);

  return (
    <section className="activity-feed" data-testid="activity-feed">
      <header className="activity-feed-header">
        <h3>Activity</h3>
        <button
          type="button"
          className="activity-feed-refresh"
          onClick={refresh}
          aria-label="Refresh activity"
          title="Refresh now"
        >
          ↻
        </button>
      </header>

      {state.kind === "loading" && (
        <div className="activity-feed-empty" data-testid="activity-loading">
          Loading…
        </div>
      )}

      {state.kind === "error" && (
        <div className="activity-feed-empty" data-testid="activity-error">
          {state.message}
        </div>
      )}

      {state.kind === "ready" && isEmpty(state.data) && (
        <div className="activity-feed-empty" data-testid="activity-empty">
          No activity yet — the daemon will populate this when it fires.
        </div>
      )}

      {state.kind === "stale" && (
        <div
          className="activity-feed-stale"
          data-testid="activity-stale"
          role="status"
        >
          <span className="activity-feed-stale-dot" aria-hidden="true" />
          <span>couldn't refresh — showing last good data</span>
        </div>
      )}

      {(state.kind === "ready" || state.kind === "stale") &&
        !isEmpty(state.data) && (
          <>
            {state.data.bot_runs.length > 0 && (
              <div className="activity-section" data-testid="activity-bots">
                <h4>Bots</h4>
                <ul>
                  {state.data.bot_runs.map((r) => (
                    <Row
                      key={r.id}
                      id={r.id}
                      primary={`#${shortId(r.id)}`}
                      secondary={r.bot_id}
                      status={r.status}
                      when={relativeTime(r.started_at)}
                      triggeredBy={r.triggered_by}
                    />
                  ))}
                </ul>
              </div>
            )}
            {state.data.skill_runs.length > 0 && (
              <div className="activity-section" data-testid="activity-skills">
                <h4>Skills</h4>
                <ul>
                  {state.data.skill_runs.map((r) => (
                    <Row
                      key={r.id}
                      id={r.id}
                      primary={`#${shortId(r.id)}`}
                      secondary={r.skill_id}
                      status={r.status}
                      when={relativeTime(r.started_at)}
                    />
                  ))}
                </ul>
              </div>
            )}
            {state.data.approvals.length > 0 && (
              <div className="activity-section" data-testid="activity-approvals">
                <h4>Approvals</h4>
                <ul>
                  {state.data.approvals.map((a) => (
                    <Row
                      key={a.id}
                      id={a.id}
                      primary={a.tool_name}
                      secondary={a.bot_id}
                      status={a.status}
                      when={relativeTime(a.created_at)}
                      // v3.4.0 — "Why this asked"
                      // reason inline. The Rust
                      // `list_recent_activity` query
                      // returns the full row including
                      // the new `reason` column. The
                      // `Row` renders nothing when
                      // `reason` is null (legacy
                      // row / no reason computed).
                      reason={a.reason ?? null}
                    />
                  ))}
                </ul>
              </div>
            )}
          </>
        )}
    </section>
  );
}
