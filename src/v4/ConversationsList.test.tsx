// v4 — ConversationsList tests (S2.6 — display-only).
//
// S2.6 — ConversationsList no longer fetches. App.tsx holds the
// conversation list (lifted state) and passes it down via the
// `conversations` prop. Coverage:
//
//   1. Mounting with botId=null renders nothing.
//   2. Renders the conversations passed via prop (no IPC).
//   3. Sorted by updated_at desc — verified by the prop order
//      (App.tsx sorts before passing).
//   4. Clicking a row fires onSelectConv.
//   5. Clicking "+ New chat" fires onNewChat.
//   6. Selected conversation row has is-selected class.
//   7. Empty list shows the "No threads yet." message.
//   8. Loading state (conversations=null) shows the Loading row.

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ConversationsList } from "./ConversationsList";
import type { Conversation } from "../lib/api";

const now = "2026-09-11T18:00:00+00:00";
const older = "2026-09-10T18:00:00+00:00";
const newest = "2026-09-11T19:00:00+00:00";

const convA: Conversation = {
  id: "conv-A",
  title: "Old chat",
  created_at: older,
  updated_at: older,
  bot_id: "bot-1",
};
const convB: Conversation = {
  id: "conv-B",
  title: "Current chat",
  created_at: now,
  updated_at: now,
  bot_id: "bot-1",
};
const convC: Conversation = {
  id: "conv-C",
  title: "Newest chat",
  created_at: now,
  updated_at: newest,
  bot_id: "bot-1",
};

afterEach(() => cleanup());

describe("ConversationsList — S2.6 display-only contract", () => {
  it("renders nothing when botId is null", () => {
    const { container } = render(
      <ConversationsList
        botId={null}
        conversations={null}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    expect(container.firstChild).toBeNull();
  });

  it("renders one row per conversation passed via prop (no IPC)", () => {
    render(
      <ConversationsList
        botId="bot-1"
        conversations={[convA, convB, convC]}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    expect(screen.getByText("Old chat")).toBeTruthy();
    expect(screen.getByText("Current chat")).toBeTruthy();
    expect(screen.getByText("Newest chat")).toBeTruthy();
  });

  it("renders rows in the order passed via prop (App sorts first)", () => {
    const { container } = render(
      <ConversationsList
        botId="bot-1"
        conversations={[convC, convB, convA]}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    const titles = container.querySelectorAll(
      ".v4-conv-list-rows .v4-conv-list-row-title",
    );
    expect(Array.from(titles).map((n) => n.textContent)).toEqual([
      "Newest chat",
      "Current chat",
      "Old chat",
    ]);
  });

  it("clicking a row fires onSelectConv with that conversation id", () => {
    const onSelect = vi.fn();
    render(
      <ConversationsList
        botId="bot-1"
        conversations={[convA, convB]}
        selectedConvId={null}
        onSelectConv={onSelect}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    fireEvent.click(screen.getByText("Current chat"));
    expect(onSelect).toHaveBeenCalledWith("conv-B");
  });

  it("clicking + New chat fires onNewChat", () => {
    const onNew = vi.fn();
    render(
      <ConversationsList
        botId="bot-1"
        conversations={[]}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={onNew}
        newChatBusy={false}
      />,
    );
    const btn = screen.getByText(/\+ New chat/);
    fireEvent.click(btn);
    expect(onNew).toHaveBeenCalledTimes(1);
  });

  it("marks the selected conversation row with is-selected", () => {
    render(
      <ConversationsList
        botId="bot-1"
        conversations={[convA, convB]}
        selectedConvId="conv-B"
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    const selected = screen.getByText("Current chat").closest("button");
    expect(selected?.classList.contains("is-selected")).toBe(true);
    const other = screen.getByText("Old chat").closest("button");
    expect(other?.classList.contains("is-selected")).toBe(false);
  });

  it("shows 'No threads yet.' when conversations is empty", () => {
    render(
      <ConversationsList
        botId="bot-1"
        conversations={[]}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    expect(screen.getByText(/no threads yet/i)).toBeTruthy();
  });

  it("shows the Loading row when conversations is null", () => {
    render(
      <ConversationsList
        botId="bot-1"
        conversations={null}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    expect(screen.getByText(/loading/i)).toBeTruthy();
  });

  it("falls back to 'New chat' label when the title is empty", () => {
    const emptyTitle: Conversation = {
      ...convB,
      id: "conv-empty",
      title: "",
    };
    render(
      <ConversationsList
        botId="bot-1"
        conversations={[emptyTitle]}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    expect(screen.getByText("New chat")).toBeTruthy();
  });
});
