// v2.0 Slice E — tests for the BotRoster sidebar component.
//
// The roster is the new primary surface in the sidebar. The
// tests assert the four behaviors the plan calls out:
//
//   1. Empty state: when `bots` is empty, render the
//      "Create your first Bot" CTA and call
//      `onCreateBot` when it's clicked.
//   2. Row click: clicking a row calls `onSelectBot` with
//      the right Bot id.
//   3. Search: typing in the search box filters the list
//      case-insensitively (substring match on the Bot's
//      name).
//   4. "+ New Bot" button: clicking it calls
//      `onCreateBot`.
//
// v3.7.6: also test the per-row destroy (×) button:
//   5. The destroy button calls `onDestroyBot` with the
//      right Bot id and does NOT trigger `onSelectBot`.
//   6. When `onDestroyBot` is omitted, the destroy button
//      is not rendered (defensive — old call sites stay
//      clean).
//
// We use `@testing-library/react` for rendering and
// `fireEvent` for user gestures. The `data-testid` attrs
// on each interactive element make the assertions
// independent of styling churn.

import { describe, expect, it, vi } from "vitest";
import { render, screen, fireEvent, within } from "@testing-library/react";
import { BotRoster } from "./BotRoster";
import type { Bot } from "../lib/api";

const blankBot = (overrides: Partial<Bot> = {}): Bot => {
  const now = new Date().toISOString();
  return {
    id: "id-1",
    name: "Bot 1",
    description: "",
    system_prompt: "",
    default_model: "MiniMax-M3",
    allowed_tools: [],
    icon: "🤖",
    color: "",
    created_at: now,
    updated_at: now,
    ...overrides,
  };
};

describe("BotRoster — empty state", () => {
  it("shows the 'Create your first Bot' CTA when bots is empty", () => {
    render(
      <BotRoster
        bots={[]}
        selectedBotId={null}
        onSelectBot={() => {}}
        onCreateBot={() => {}}
      />,
    );
    const empty = screen.getByTestId("bot-roster-empty");
    expect(empty).toBeTruthy();
    // The CTA button lives inside the empty state.
    const cta = within(empty).getByTestId("bot-roster-empty-cta");
    expect(cta.textContent).toContain("Create your first Bot");
  });

  it("calls onCreateBot when the empty-state CTA is clicked", () => {
    const onCreateBot = vi.fn();
    render(
      <BotRoster
        bots={[]}
        selectedBotId={null}
        onSelectBot={() => {}}
        onCreateBot={onCreateBot}
      />,
    );
    fireEvent.click(screen.getByTestId("bot-roster-empty-cta"));
    expect(onCreateBot).toHaveBeenCalledTimes(1);
  });
});

describe("BotRoster — row click", () => {
  it("calls onSelectBot with the row's Bot id when the row is clicked", () => {
    const onSelectBot = vi.fn();
    const bots = [blankBot({ id: "alpha", name: "Alpha" })];
    render(
      <BotRoster
        bots={bots}
        selectedBotId={null}
        onSelectBot={onSelectBot}
        onCreateBot={() => {}}
      />,
    );
    const item = screen.getByTestId("bot-roster-item");
    fireEvent.click(within(item).getByRole("button"));
    expect(onSelectBot).toHaveBeenCalledWith("alpha");
  });
});

describe("BotRoster — search filter", () => {
  it("filters by name (case-insensitive substring match)", () => {
    const bots = [
      blankBot({ id: "alpha", name: "Alpha Bot" }),
      blankBot({ id: "beta", name: "Beta Bot" }),
      blankBot({ id: "gamma", name: "Gamma Assistant" }),
    ];
    render(
      <BotRoster
        bots={bots}
        selectedBotId={null}
        onSelectBot={() => {}}
        onCreateBot={() => {}}
      />,
    );
    // All three rows visible before searching.
    expect(screen.getAllByTestId("bot-roster-item").length).toBe(3);
    // Type "beta" — only the matching row remains.
    const search = screen.getByTestId("bot-roster-search");
    fireEvent.change(search, { target: { value: "beta" } });
    const items = screen.getAllByTestId("bot-roster-item");
    expect(items.length).toBe(1);
    expect(items[0].getAttribute("data-bot-id")).toBe("beta");
  });

  it("search is case-insensitive", () => {
    const bots = [blankBot({ id: "alpha", name: "Alpha Bot" })];
    render(
      <BotRoster
        bots={bots}
        selectedBotId={null}
        onSelectBot={() => {}}
        onCreateBot={() => {}}
      />,
    );
    fireEvent.change(screen.getByTestId("bot-roster-search"), {
      target: { value: "ALPHA" },
    });
    expect(screen.getAllByTestId("bot-roster-item").length).toBe(1);
  });
});

describe("BotRoster — '+ New Bot' button", () => {
  it("calls onCreateBot when the header '+ New Bot' button is clicked", () => {
    const onCreateBot = vi.fn();
    const bots = [blankBot({ id: "alpha", name: "Alpha" })];
    render(
      <BotRoster
        bots={bots}
        selectedBotId={null}
        onSelectBot={() => {}}
        onCreateBot={onCreateBot}
      />,
    );
    fireEvent.click(screen.getByTestId("bot-roster-new-btn"));
    expect(onCreateBot).toHaveBeenCalledTimes(1);
  });
});

describe("BotRoster — selected row", () => {
  it("applies the 'selected' modifier class to the matching row", () => {
    const bots = [
      blankBot({ id: "alpha", name: "Alpha" }),
      blankBot({ id: "beta", name: "Beta" }),
    ];
    const { container } = render(
      <BotRoster
        bots={bots}
        selectedBotId="beta"
        onSelectBot={() => {}}
        onCreateBot={() => {}}
      />,
    );
    const items = container.querySelectorAll(".bot-roster__item");
    expect(items.length).toBe(2);
    // The selected row has the modifier class; the other
    // doesn't.
    const selected = container.querySelector(
      ".bot-roster__item--selected",
    );
    expect(selected).toBeTruthy();
    expect(selected!.getAttribute("data-bot-id")).toBe("beta");
  });
});

describe("BotRoster — destroy button (v3.7.6)", () => {
  it("renders a destroy (×) button per row when onDestroyBot is provided", () => {
    const bots = [
      blankBot({ id: "alpha", name: "Alpha" }),
      blankBot({ id: "beta", name: "Beta" }),
    ];
    render(
      <BotRoster
        bots={bots}
        selectedBotId={null}
        onSelectBot={() => {}}
        onCreateBot={() => {}}
        onDestroyBot={() => {}}
      />,
    );
    const destroys = screen.getAllByTestId("bot-roster-destroy");
    expect(destroys.length).toBe(2);
  });

  it("calls onDestroyBot with the row's Bot id when the × is clicked", () => {
    const onDestroyBot = vi.fn();
    const onSelectBot = vi.fn();
    const bots = [blankBot({ id: "alpha", name: "Alpha" })];
    render(
      <BotRoster
        bots={bots}
        selectedBotId={null}
        onSelectBot={onSelectBot}
        onCreateBot={() => {}}
        onDestroyBot={onDestroyBot}
      />,
    );
    fireEvent.click(screen.getByTestId("bot-roster-destroy"));
    expect(onDestroyBot).toHaveBeenCalledWith("alpha");
    // Click must NOT also trigger the row's onSelectBot —
    // e.stopPropagation() in the handler is what guarantees
    // this.
    expect(onSelectBot).not.toHaveBeenCalled();
  });

  it("does not render a destroy button when onDestroyBot is omitted", () => {
    const bots = [blankBot({ id: "alpha", name: "Alpha" })];
    render(
      <BotRoster
        bots={bots}
        selectedBotId={null}
        onSelectBot={() => {}}
        onCreateBot={() => {}}
      />,
    );
    // queryBy returns null when not present, unlike getBy
    // which throws.
    expect(screen.queryByTestId("bot-roster-destroy")).toBeNull();
  });
});
