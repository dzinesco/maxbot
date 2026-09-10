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

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { ActivityFeed } from "./ActivityFeed";

vi.mock("../lib/tauri", () => ({
  listRecentActivity: vi.fn(),
}));

import { listRecentActivity } from "../lib/tauri";
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
    // The exact spec copy is pinned: a future rewording
    // would need an explicit review.
    expect(
      screen.getByText(/No activity yet — the daemon will populate this when it fires\./),
    ).toBeTruthy();
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
});
