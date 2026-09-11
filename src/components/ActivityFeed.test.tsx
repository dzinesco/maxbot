// v2.8.0 — Always-on Daemon: ActivityFeed component
// tests. We mock the tauri module so the test never
// hits the Tauri runtime.
//
// What we cover:
//   1. Renders the three section headers when all
//      three lists are populated.
//   2. Empty state when no activity rows come back
//      (the literal copy from the spec is asserted
//      against so a future rewording triggers a
//      review).
//   3. Loading state on first mount.
//
// v3.6.0 (Phase 7) — Memory has to fill itself.
// Added tests for the inline memory-write pill:
//   4. Renders the Memory section + a pill when a
//      `memory:written` Tauri event fires.
//   5. The dismiss button calls `memoryForget` and
//      removes the pill from the local list.
//   6. History writes are filtered out (the reflect
//      step never emits them, but the UI defends
//      against a misbehaving future emitter).

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor, act, fireEvent } from "@testing-library/react";
import { ActivityFeed } from "./ActivityFeed";

vi.mock("../lib/tauri", () => ({
  listRecentActivity: vi.fn(),
  memoryForget: vi.fn(),
}));

// Mock the Tauri event bus. The ActivityFeed
// subscribes to `memory:written` on mount. The
// mock lets each test capture the registered
// handler and dispatch a synthetic event when
// the test is ready.
const memoryWrittenListeners: Array<
  (e: { payload: unknown }) => void
> = [];

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (eventName: string, handler: (e: { payload: unknown }) => void) => {
    if (eventName === "memory:written") {
      memoryWrittenListeners.push(handler);
    }
    return () => {
      const idx = memoryWrittenListeners.indexOf(handler);
      if (idx >= 0) memoryWrittenListeners.splice(idx, 1);
    };
  }),
}));

import { listRecentActivity, memoryForget } from "../lib/tauri";
import type { ActivityFeed as ActivityFeedData, Approval, BotRun, SkillRun } from "../lib/api";

const now = new Date().toISOString();

const makeBotRun = (id: string, status: string): BotRun => ({
  id,
  bot_id: `bot-${id}`,
  conversation_id: `conv-${id}`,
  status: status as BotRun["status"],
  started_at: now,
  finished_at: now,
  result_summary: "",
});

const makeSkillRun = (id: string, status: string): SkillRun => ({
  id,
  skill_id: `skill-${id}`,
  bot_id: `bot-${id}`,
  inputs: {},
  steps: [],
  status: status as SkillRun["status"],
  started_at: now,
  finished_at: now,
  result_summary: "",
});

const makeApproval = (id: string, status: string): Approval => ({
  id,
  bot_id: `bot-${id}`,
  tool_name: `tool-${id}`,
  status: status as Approval["status"],
  payload: {},
  result: null,
  bot_run_id: null,
  tool_call_id: null,
  created_at: now,
  decided_at: null,
});

const fullData: ActivityFeedData = {
  bot_runs: [
    makeBotRun("run-1", "succeeded"),
    makeBotRun("run-2", "failed"),
  ],
  skill_runs: [makeSkillRun("s-1", "succeeded")],
  approvals: [makeApproval("a-1", "pending")],
};

describe("ActivityFeed", () => {
  beforeEach(() => {
    vi.mocked(listRecentActivity).mockReset();
    vi.mocked(memoryForget).mockReset().mockResolvedValue(true);
    memoryWrittenListeners.length = 0;
  });

  it("renders the three sections when all three lists are populated", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue(fullData);
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-bots")).toBeTruthy();
      expect(screen.getByTestId("activity-skills")).toBeTruthy();
      expect(screen.getByTestId("activity-approvals")).toBeTruthy();
    });
    // The Bots section header.
    expect(screen.getByText("Bots")).toBeTruthy();
    // The Skills section header.
    expect(screen.getByText("Skills")).toBeTruthy();
    // The Approvals section header.
    expect(screen.getByText("Approvals")).toBeTruthy();
    // At least one row is rendered (the run-1 bot run).
    const rows = screen.getAllByTestId("activity-row");
    expect(rows.length).toBeGreaterThanOrEqual(4);
  });

  it("shows the empty-state copy when no activity is returned", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [],
      skill_runs: [],
      approvals: [],
    });
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-empty")).toBeTruthy();
    });
    // v3.7.13 — UX-1. The copy tightened to
    // "No activity yet." The previous
    // "— the daemon will populate this when it
    // fires" parenthetical is gone: it read as
    // uncertain / half-implemented, and the
    // rail lights up on its own.
    expect(
      screen.getByText(/^No activity yet\.$/),
    ).toBeTruthy();
    // The old "— the daemon will populate this
    // when it fires" phrase must NOT appear
    // anywhere on the page; pinning the absence
    // is the cleanest way to keep the rewording
    // intentional.
    expect(
      screen.queryByText(/populate this when it fires/i),
    ).toBeNull();
  });

  // v3.7.13 — UX-1. The previous error path
  // rendered the raw `state.message` Tauri error
  // string in the sidebar (e.g. "Json deserialize
  // error: EOF while parsing a value at line 1
  // column 0"). Users couldn't act on that and
  // the auto-retry cadence wasn't discoverable.
  // The new copy is a friendly one-liner plus a
  // "Retry now" button that calls `refresh()`.
  it("shows a friendly error + Retry button on load failure", async () => {
    vi.mocked(listRecentActivity).mockRejectedValue(
      new Error("Json deserialize error: EOF while parsing"),
    );
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-error")).toBeTruthy();
    });
    // The friendly copy is the user-facing
    // string; the raw Tauri error is NOT in the
    // rendered DOM.
    expect(
      screen.getByText(/Couldn't load activity\. Will retry in 10s\./),
    ).toBeTruthy();
    expect(
      screen.queryByText(/Json deserialize error/i),
    ).toBeNull();
    // The "Retry now" button is present and
    // calls the loader again when clicked.
    const retry = screen.getByTestId("activity-retry") as HTMLButtonElement;
    expect(retry).toBeTruthy();
    const before = vi.mocked(listRecentActivity).mock.calls.length;
    fireEvent.click(retry);
    await waitFor(() => {
      expect(vi.mocked(listRecentActivity).mock.calls.length).toBeGreaterThan(
        before,
      );
    });
  });

  it("shows the loading placeholder on first mount", () => {
    // Never resolve so the component stays in
    // the loading state.
    vi.mocked(listRecentActivity).mockReturnValue(new Promise(() => {}));
    render(<ActivityFeed />);
    expect(screen.getByTestId("activity-loading")).toBeTruthy();
  });

  it("hides sections that are empty but renders the ones with data", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [makeBotRun("only-bot", "succeeded")],
      skill_runs: [],
      approvals: [],
    });
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-bots")).toBeTruthy();
    });
    expect(screen.queryByTestId("activity-skills")).toBeNull();
    expect(screen.queryByTestId("activity-approvals")).toBeNull();
    expect(screen.queryByTestId("activity-empty")).toBeNull();
  });

  // v3.4.0 (Phase 5) — "Why this asked" reason is
  // surfaced inline on each approval row. The
  // shape is "Why: <reason>". Legacy rows (no
  // reason) render no extra line so the activity
  // feed doesn't grow past 1.5x its v3.3.0 line
  // count.
  it("surfaces the audit-log reason inline on approval rows", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [],
      skill_runs: [],
      approvals: [
        {
          ...makeApproval("a-r1", "pending"),
          reason: "sending email to client@axis.com",
        },
      ],
    });
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-approvals")).toBeTruthy();
    });
    expect(screen.getByTestId("activity-row-reason").textContent).toContain(
      "sending email to client@axis.com",
    );
  });

  it("does not render a reason line for legacy rows with null reason", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [],
      skill_runs: [],
      approvals: [
        { ...makeApproval("a-r2", "pending"), reason: null },
      ],
    });
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-approvals")).toBeTruthy();
    });
    expect(screen.queryByTestId("activity-row-reason")).toBeNull();
  });

  // ---- v3.6.0 (Phase 7) — Memory-write pill ----
  //
  // These tests cover the new "Memory" section in
  // the ActivityFeed. The reflect step's
  // `memory:written` Tauri event is mocked via
  // the `listen` spy at the top of the file.

  it("renders the Memory section + a pill when a memory:written event fires", async () => {
    // Empty server data so the only thing the
    // user sees in the feed is the local pill.
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [],
      skill_runs: [],
      approvals: [],
    });
    render(<ActivityFeed />);
    // Wait for the initial poll to resolve so
    // the empty state is settled before we
    // dispatch the event.
    await waitFor(() => {
      expect(screen.getByTestId("activity-empty")).toBeTruthy();
    });
    // Now dispatch a synthetic event. The
    // `act` wrapper flushes the React state
    // update before we assert.
    await act(async () => {
      for (const handler of memoryWrittenListeners) {
        handler({
          payload: {
            bot_id: "bot-1",
            bot_run_id: "run-1",
            kind: "preference",
            key: "summary_format",
            content: "bullet points, never paragraphs",
          },
        });
      }
    });
    await waitFor(() => {
      expect(screen.getByTestId("activity-memory")).toBeTruthy();
    });
    // The pill body uses the exact spec copy:
    // "Bot learned: 'Tyler prefers bullet-point
    // summaries'". We assert the key parts
    // rather than the full string to keep
    // content wording flexible.
    const pill = screen.getByTestId("activity-row-memory");
    expect(pill.textContent).toContain("Bot learned:");
    expect(pill.textContent).toContain("bullet points, never paragraphs");
    expect(pill.textContent).toContain("preference");
    expect(pill.textContent).toContain("summary_format");
    // The dismiss button is present.
    expect(screen.getByTestId("activity-row-memory-dismiss")).toBeTruthy();
  });

  it("dismiss button calls memoryForget and removes the pill", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [],
      skill_runs: [],
      approvals: [],
    });
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-empty")).toBeTruthy();
    });
    await act(async () => {
      for (const handler of memoryWrittenListeners) {
        handler({
          payload: {
            bot_id: "bot-2",
            bot_run_id: "run-2",
            kind: "fact",
            key: "primary_city",
            content: "Denver, CO",
          },
        });
      }
    });
    await waitFor(() => {
      expect(screen.getByTestId("activity-row-memory")).toBeTruthy();
    });
    // Click dismiss.
    await act(async () => {
      screen.getByTestId("activity-row-memory-dismiss").click();
    });
    // memoryForget is called with the same
    // (botId, kind, key) the pill carried.
    await waitFor(() => {
      expect(memoryForget).toHaveBeenCalledWith(
        "bot-2",
        "fact",
        "primary_city",
      );
    });
    // The pill is gone.
    await waitFor(() => {
      expect(screen.queryByTestId("activity-row-memory")).toBeNull();
    });
  });

  it("filters out history writes (the reflect step never emits them, but the UI defends)", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [],
      skill_runs: [],
      approvals: [],
    });
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-empty")).toBeTruthy();
    });
    // Dispatch a history write — the UI must
    // not surface it as a pill.
    await act(async () => {
      for (const handler of memoryWrittenListeners) {
        handler({
          payload: {
            bot_id: "bot-3",
            bot_run_id: "run-3",
            kind: "history",
            key: "",
            content: "user: hi | bot: hello",
          },
        });
      }
    });
    // Give the React tree a tick to re-render.
    await new Promise((r) => setTimeout(r, 0));
    expect(screen.queryByTestId("activity-row-memory")).toBeNull();
    expect(screen.queryByTestId("activity-memory")).toBeNull();
  });

  it("does not show the empty state when a memory write is present even if the server feed is empty", async () => {
    vi.mocked(listRecentActivity).mockResolvedValue({
      bot_runs: [],
      skill_runs: [],
      approvals: [],
    });
    render(<ActivityFeed />);
    await waitFor(() => {
      expect(screen.getByTestId("activity-empty")).toBeTruthy();
    });
    await act(async () => {
      for (const handler of memoryWrittenListeners) {
        handler({
          payload: {
            bot_id: "bot-4",
            bot_run_id: "run-4",
            kind: "fact",
            key: "name",
            content: "Tyler",
          },
        });
      }
    });
    await waitFor(() => {
      expect(screen.getByTestId("activity-memory")).toBeTruthy();
    });
    // The "No activity yet" empty state is
    // suppressed while at least one memory
    // write is in the local list.
    expect(screen.queryByTestId("activity-empty")).toBeNull();
  });
});
