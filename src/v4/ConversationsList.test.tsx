// v4 — ConversationsList tests.
//
// Per the S2 brief:
//   - "Conversation list for the selected bot only, fetched on
//      bot select, not at boot."
//   - "Tests: new chat, switch thread, persist selected bot."
//
// Coverage:
//   1. Mounting with botId=null renders nothing.
//   2. Mounting with a botId calls listConversations(bot.id)
//      exactly once (NOT at boot, NOT per render).
//   3. Sorted by updated_at desc.
//   4. Clicking a row fires onSelectConv.
//   5. Clicking "New chat" fires onNewChat.
//   6. Switching botId re-fetches.
//   7. selectedConvId that isn't in the current list triggers
//      a refetch (the new-chat-after-select path).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { ConversationsList } from "./ConversationsList";
import type { Conversation } from "../lib/api";

const invokeMock = vi.fn();

vi.mock("@tauri-apps/api/core", () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

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

beforeEach(() => {
  invokeMock.mockReset();
});

afterEach(() => cleanup());

describe("ConversationsList — S2 contract", () => {
  it("renders nothing when botId is null", () => {
    const { container } = render(
      <ConversationsList
        botId={null}
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    expect(container.firstChild).toBeNull();
    expect(invokeMock).not.toHaveBeenCalled();
  });

  it("calls listConversations(bot.id) once when a bot is selected", async () => {
    invokeMock.mockResolvedValue([convA, convB, convC]);
    render(
      <ConversationsList
        botId="bot-1"
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith("list_conversations", {
        botId: "bot-1",
      });
    });
    expect(
      invokeMock.mock.calls.filter((c) => c[0] === "list_conversations"),
    ).toHaveLength(1);
  });

  it("renders rows sorted by updated_at desc", async () => {
    invokeMock.mockResolvedValue([convA, convB, convC]);
    const { container } = render(
      <ConversationsList
        botId="bot-1"
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => screen.getByText("Newest chat"));
    // Scope to the threads list (not the "+ New chat" button, which
    // lives in its own .v4-conv-list-new row).
    const rows = container.querySelectorAll(
      ".v4-conv-list-rows .v4-conv-list-row-title",
    );
    const titles = Array.from(rows).map((n) => n.textContent);
    expect(titles).toEqual(["Newest chat", "Current chat", "Old chat"]);
  });

  it("clicking a row fires onSelectConv with that conversation id", async () => {
    invokeMock.mockResolvedValue([convA, convB]);
    const onSelect = vi.fn();
    render(
      <ConversationsList
        botId="bot-1"
        selectedConvId={null}
        onSelectConv={onSelect}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => screen.getByText("Current chat"));
    fireEvent.click(screen.getByText("Current chat"));
    expect(onSelect).toHaveBeenCalledWith("conv-B");
  });

  it("clicking New chat fires onNewChat", async () => {
    invokeMock.mockResolvedValue([]);
    const onNew = vi.fn();
    render(
      <ConversationsList
        botId="bot-1"
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={onNew}
        newChatBusy={false}
      />,
    );
    await waitFor(() => screen.getByText(/no threads yet/i));
    // S2.5 — the "New chat" button lives in its own row under the
    // threads list. Class on the row is .v4-conv-list-new so it's
    // distinguishable from real conversation rows.
    const btn = screen.getByText(/\+ New chat/);
    fireEvent.click(btn);
    expect(onNew).toHaveBeenCalledTimes(1);
  });

  it("marks the selected conversation row with is-selected", async () => {
    invokeMock.mockResolvedValue([convA, convB]);
    render(
      <ConversationsList
        botId="bot-1"
        selectedConvId="conv-B"
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => screen.getByText("Current chat"));
    const selected = screen.getByText("Current chat").closest("button");
    expect(selected?.classList.contains("is-selected")).toBe(true);
    const other = screen.getByText("Old chat").closest("button");
    expect(other?.classList.contains("is-selected")).toBe(false);
  });

  it("refetches when botId changes (new bot selected)", async () => {
    invokeMock.mockImplementation((cmd: string) => {
      if (cmd === "list_conversations") {
        // Return different lists for different bots.
        return Promise.resolve([convA]);
      }
      return Promise.resolve(null);
    });
    const { rerender } = render(
      <ConversationsList
        botId="bot-1"
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => screen.getByText("Old chat"));
    expect(
      invokeMock.mock.calls.filter((c) => c[0] === "list_conversations"),
    ).toHaveLength(1);

    rerender(
      <ConversationsList
        botId="bot-2"
        selectedConvId={null}
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => {
      expect(
        invokeMock.mock.calls.filter((c) => c[0] === "list_conversations"),
      ).toHaveLength(2);
    });
    expect(invokeMock).toHaveBeenLastCalledWith("list_conversations", {
      botId: "bot-2",
    });
  });

  it("refetches when selectedConvId changes to an id not in the list", async () => {
    invokeMock.mockResolvedValue([convA, convB]);
    const { rerender } = render(
      <ConversationsList
        botId="bot-1"
        selectedConvId="conv-A"
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => screen.getByText("Current chat"));
    expect(
      invokeMock.mock.calls.filter((c) => c[0] === "list_conversations"),
    ).toHaveLength(1);

    // selectedConvId changes to conv-B (which IS in the list) — no refetch.
    rerender(
      <ConversationsList
        botId="bot-1"
        selectedConvId="conv-B"
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    // Yield once so any potential effect runs, then check the count.
    await new Promise((r) => setTimeout(r, 50));
    expect(
      invokeMock.mock.calls.filter((c) => c[0] === "list_conversations"),
    ).toHaveLength(1);

    // selectedConvId changes to a new id (not in the list) — refetch.
    rerender(
      <ConversationsList
        botId="bot-1"
        selectedConvId="conv-NEW"
        onSelectConv={() => {}}
        onNewChat={() => {}}
        newChatBusy={false}
      />,
    );
    await waitFor(() => {
      expect(
        invokeMock.mock.calls.filter((c) => c[0] === "list_conversations"),
      ).toHaveLength(2);
    });
  });
});
