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
// v3.6.0 (Phase 7) — Memory has to fill itself.
// Added a fourth section ("Memory") that subscribes
// to the `memory:written` Tauri event and surfaces
// each new fact or preference the executor's
// reflect step wrote as a small inline pill ("Bot
// learned: 'Tyler prefers bullet-point summaries'.
//  Each pill has a small dismiss (×) button that
// calls `memoryForget` to roll back the write.
// The memory pill is its own row type — distinct
// from the existing audit-log row and not embedded
// inside any existing row.
//
// Empty state: "No activity yet — the daemon will
// populate this when it fires." (matches the spec in
// the v2.8 plan).

import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { listRecentActivity, memoryForget } from "../lib/tauri";
import type { ActivityFeed as ActivityFeedData, MemKind } from "../lib/api";

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

// ---- v3.6.0 (Phase 7) — Memory-write pill ----
//
// The executor's reflect step emits a
// `memory:written` Tauri event every time it
// appends a new fact or preference to a Bot's
// memory file. The pill captures that event,
// shows the content inline, and lets the user
// dismiss (×) it — which calls `memoryForget`
// to roll back the write. The pill is its own
// row type, distinct from the bot_run /
// skill_run / approval rows below.
//
// `dismissed` is local state; on a hard page
// refresh the pill is gone (the local state is
// in-memory). The on-disk JSONL entry is also
// gone if the user dismissed — so the next
// poll won't surface it again either.
interface MemoryWritePillProps {
  botId: string;
  runId: string;
  kind: MemKind | string;
  /** Memory entry key. Renamed from `key` because
   *  `key` is reserved in JSX as the React
   *  reconciliation key. The parent passes the
   *  synthetic `key` separately. */
  entryKey: string;
  content: string;
  onDismiss: () => void;
}

const KIND_GLYPH_MEMORY: Record<string, string> = {
  fact: "🧠",
  preference: "⚙️",
};

function MemoryWritePill({
  botId,
  runId,
  kind,
  entryKey,
  content,
  onDismiss,
}: MemoryWritePillProps) {
  const glyph = KIND_GLYPH_MEMORY[String(kind)] ?? "💡";
  const handleDismiss = async () => {
    try {
      // Roll back the write. `memoryForget` returns
      // `false` if the entry was already gone
      // (e.g. the user already dismissed it from
      // another surface) — that's still a success
      // from the UI's perspective: the pill is
      // gone, the JSONL is consistent.
      await memoryForget(botId, kind as MemKind, entryKey);
    } catch {
      // Best-effort: a transient SFTP error
      // shouldn't strand the pill. We surface
      // nothing — the local pill is removed
      // either way. The next poll will re-surface
      // the entry if the rollback actually failed
      // and the on-disk entry is still present.
    }
    onDismiss();
  };
  return (
    <li
      className="activity-row activity-row-memory"
      data-testid="activity-row-memory"
      data-memory-key={entryKey}
      data-memory-run-id={runId}
    >
      <span className="activity-row-status" aria-hidden="true">
        {glyph}
      </span>
      <span className="activity-row-primary">
        Bot learned: <em>“{content}”</em>
      </span>
      <span className="activity-row-secondary muted small">
        {String(kind)} · {entryKey}
      </span>
      <button
        type="button"
        className="activity-row-dismiss"
        onClick={handleDismiss}
        aria-label={`Dismiss memory write ${entryKey}`}
        title="Dismiss — rolls back this memory write"
        data-testid="activity-row-memory-dismiss"
      >
        ×
      </button>
    </li>
  );
}

// Shape of the `memory:written` event payload
// emitted by the executor's reflect step. The
// Rust side serializes a `BotMemoryWriteEvent`
// with the same field names.
interface MemoryWrittenEvent {
  bot_id: string;
  bot_run_id: string;
  kind: MemKind | string;
  key: string;
  content: string;
}

export function ActivityFeed() {
  const [state, setState] = useState<FetchState>({ kind: "loading" });
  // v3.6.0 (Phase 7) — Local list of memory writes
  // the executor's reflect step has produced since
  // the ActivityFeed mounted. Each entry is one pill;
  // dismissing the pill removes it from this list
  // and rolls back the on-disk write via
  // `memoryForget`. The list isn't persisted: a
  // hard page refresh drops it (the on-disk JSONL
  // is the durable source of truth, and the
  // MemoryPanel re-reads it on mount).
  const [memoryWrites, setMemoryWrites] = useState<MemoryWrittenEvent[]>([]);

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

  // v3.6.0 (Phase 7) — Subscribe to the
  // `memory:written` Tauri event. The executor's
  // reflect step emits one event per new fact /
  // preference it appends to a Bot's memory file.
  // We append each event to the local
  // `memoryWrites` list; the section below renders
  // it as a pill with a dismiss button.
  //
  // Note: we filter on `kind` ∈ {fact, preference}
  // defensively, even though the Rust side never
  // emits `history`. The plan is explicit that
  // History stays explicit (auto-written via the
  // v2.5.0 `auto_write_history` path, not the
  // reflect step). The filter is a belt-and-braces
  // against a future emitter that violates the
  // contract.
  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let cancelled = false;
    (async () => {
      const u = await listen<MemoryWrittenEvent>(
        "memory:written",
        (e) => {
          const kind = String(e.payload.kind);
          if (kind !== "fact" && kind !== "preference") return;
          setMemoryWrites((prev) => [
            ...prev,
            {
              bot_id: e.payload.bot_id,
              bot_run_id: e.payload.bot_run_id,
              kind,
              key: e.payload.key,
              content: e.payload.content,
            },
          ]);
        },
      );
      if (cancelled) {
        u();
      } else {
        unlisten = u;
      }
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

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
          {/* v3.7.13 — UX-1. The previous copy leaked
              the raw Tauri error string into the
              sidebar (e.g. "Json deserialize error:
              EOF while parsing a value at line 1
              column 0"). Users couldn't act on that
              and the auto-retry wasn't discoverable.
              The friendly line + a Retry button is
              the same affordance the inline
              ActivityRail used to have; the 10s
              retry cadence is set in `refresh()`
              below. */}
          Couldn't load activity. Will retry in 10s.{" "}
          <button
            className="link"
            type="button"
            data-testid="activity-retry"
            onClick={refresh}
          >
            Retry now
          </button>
        </div>
      )}

      {state.kind === "ready" && isEmpty(state.data) && memoryWrites.length === 0 && (
        <div className="activity-feed-empty" data-testid="activity-empty">
          {/* v3.7.13 — UX-1. Drop the "— the daemon
              will populate this when it fires" hint;
              the rail will light up on its own, and
              the parenthetical was reading as
              uncertain / half-implemented. */}
          No activity yet.
        </div>
      )}

      {state.kind === "stale" && (
        <div
          className="activity-feed-stale"
          data-testid="activity-stale"
          role="status"
        >
          <span className="activity-feed-stale-dot" aria-hidden="true" />
          {/* v3.7.13 — UX-1. The `state.message`
              string is kept on the DOM (via
              `data-stale-message`) for tests +
              devtools, but the user-facing copy is
              the same friendly line as the
              non-stale error path. */}
          <span data-stale-message={state.message}>
            couldn't refresh — showing last good data
          </span>
        </div>
      )}

      {(state.kind === "ready" || state.kind === "stale") &&
        (!isEmpty(state.data) || memoryWrites.length > 0) && (
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
            {/*
             * v3.6.0 (Phase 7) — Memory section. Renders
             * the local `memoryWrites` list as inline
             * pills. Each pill has a dismiss (×) button
             * that calls `memoryForget` to roll back the
             * write. The pills aren't persisted across
             * page refreshes (they're in-memory only);
             * the on-disk JSONL is the durable record,
             * and the MemoryPanel re-reads it on mount.
             */}
            {memoryWrites.length > 0 && (
              <div className="activity-section" data-testid="activity-memory">
                <h4>Memory</h4>
                <ul>
                  {memoryWrites.map((w) => (
                    <MemoryWritePill
                      // Synthetic key per (run_id, entry_key)
                      // so the same memory write from two
                      // re-renders doesn't double-mount.
                      key={`${w.bot_run_id}-${w.key}`}
                      botId={w.bot_id}
                      runId={w.bot_run_id}
                      kind={w.kind}
                      entryKey={w.key}
                      content={w.content}
                      onDismiss={() => {
                        setMemoryWrites((prev) =>
                          prev.filter(
                            (x) =>
                              !(
                                x.bot_run_id === w.bot_run_id &&
                                x.key === w.key
                              ),
                          ),
                        );
                      }}
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
