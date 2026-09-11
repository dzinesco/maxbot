import { describe, expect, it, vi } from "vitest";
import { act, render, screen } from "@testing-library/react";

// v2.4.0 — the GroupChatView subscribes to the
// `bot://chunk` / `bot://done` / `bot://error` event
// stream on mount. happy-dom doesn't run inside the
// Tauri webview, so the `listen` call would otherwise
// throw on `transformCallback` (a `__TAURI_INTERNALS__`
// dependency). We mock the three `on*` entry points
// we use so the effect resolves cleanly and the
// DOM-level assertions can run.
const events = vi.hoisted(() => ({ chunk: null as null | ((event: any) => void), unlisten: vi.fn() }));
vi.mock("../lib/tauri", () => ({
  onBotChunk: (handler: (event: any) => void) => { events.chunk = handler; return Promise.resolve(events.unlisten); },
  onBotDone: () => Promise.resolve(() => {}),
  onBotError: () => Promise.resolve(() => {}),
}));

import { GroupChatView } from "./GroupChatView";
import type { Bot, GroupMessage } from "../lib/api";

const bots: Bot[] = [
  {
    id: "b1",
    name: "Researcher",
    description: "",
    system_prompt: "",
    default_model: "MiniMax-M3",
    allowed_tools: [],
    icon: "🔍",
    color: "#7c5cff",
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
    avatar_color: "",
    last_active_at: null,
    state: "idle",
  },
  {
    id: "b2",
    name: "Writer",
    description: "",
    system_prompt: "",
    default_model: "MiniMax-M3",
    allowed_tools: [],
    icon: "✍️",
    color: "#ff7c5c",
    created_at: "2026-01-01T00:00:00.000Z",
    updated_at: "2026-01-01T00:00:00.000Z",
    avatar_color: "",
    last_active_at: null,
    state: "idle",
  },
];

const group = {
  id: "g1",
  name: "Research pod",
  owner_bot_id: "b1",
  created_at: "2026-01-01T00:00:00.000Z",
  updated_at: "2026-01-01T00:00:00.000Z",
};

describe("GroupChatView", () => {
  it("renders a handoff card for role='handoff' rows", () => {
    const messages: GroupMessage[] = [
      {
        id: "m1",
        group_id: "g1",
        bot_id: "b1",
        role: "user",
        content: "find the Q3 numbers",
        mentions: ["b1"],
        handoff_to: null,
        created_at: "2026-01-01T00:00:00.000Z",
      },
      {
        id: "m2",
        group_id: "g1",
        bot_id: "b1",
        role: "handoff",
        content: "here are the Q3 numbers — please draft the email",
        mentions: [],
        handoff_to: "b2",
        created_at: "2026-01-01T00:00:01.000Z",
      },
    ];
    render(
      <GroupChatView
        group={group}
        messages={messages}
        bots={bots}
        activeRunByBot={{}}
        activeBotRunIds={[]}
      />,
    );
    const card = screen.getByTestId("group-handoff-card");
    expect(card).toBeInTheDocument();
    expect(card.textContent).toMatch(/Writer/);
    expect(card.textContent).toMatch(/Q3 numbers/);
  });

  it("renders the participant rail with member names", () => {
    const messages: GroupMessage[] = [];
    render(
      <GroupChatView
        group={group}
        messages={messages}
        bots={bots}
        activeRunByBot={{}}
        activeBotRunIds={[]}
      />,
    );
    const rail = screen.getByTestId("group-chat-rail");
    expect(rail.textContent).toMatch(/Researcher/);
    expect(rail.textContent).toMatch(/Writer/);
  });

  it("renders an empty state when no messages", () => {
    render(
      <GroupChatView
        group={group}
        messages={[]}
        bots={bots}
        activeRunByBot={{}}
        activeBotRunIds={[]}
      />,
    );
    // The empty state copy includes a code-styled
    // @BotName hint.
    expect(screen.getByText(/@BotName/)).toBeInTheDocument();
  });
});

it("streams real chunk payloads and releases finished response buffers", async () => {
  const view = render(<GroupChatView group={group} messages={[]} bots={bots} activeRunByBot={{ b1: "pending" }} />);
  await act(async () => {});
  act(() => events.chunk?.({ bot_id: "b1", bot_run_id: "r1", conversation_id: "c1", chunk: { kind: "text", delta: "Live answer" } }));
  expect(screen.getByTestId("group-live-response")).toHaveTextContent("Live answer");
  view.rerender(<GroupChatView group={group} messages={[]} bots={bots} activeRunByBot={{}} />);
  expect(screen.queryByTestId("group-live-response")).toBeNull();
  view.rerender(<GroupChatView group={group} messages={[]} bots={bots} activeRunByBot={{ b1: "pending" }} />);
  expect(screen.getByTestId("group-live-response")).not.toHaveTextContent("Live answer");
  view.unmount();
  expect(events.unlisten).toHaveBeenCalled();
});
