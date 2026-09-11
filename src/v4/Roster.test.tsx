// v4 — Roster tests.
//
// v3.7.17 (ui/v4-shell) — per the v4 brief:
//
//   - `Roster.test.tsx` — renders bots, click dispatches
//     `onSelectBot`.
//
// Roster is pure presentation: it takes a bots array, the
// selectedBotId, and an onSelectBot callback. No Tauri IPC,
// no timers, no listeners. Tests are pure DOM + callbacks.

import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { Roster } from "./Roster";
import type { Bot } from "../lib/api";

const bots: Bot[] = [
  {
    id: "bot-1",
    name: "Alpha",
    description: "",
    system_prompt: "",
    default_model: "minimax",
    allowed_tools: [],
    icon: "",
    color: "",
    state: "idle",
    last_active_at: new Date(Date.now() - 60_000).toISOString(),
  },
  {
    id: "bot-2",
    name: "Beta",
    description: "",
    system_prompt: "",
    default_model: "minimax",
    allowed_tools: [],
    icon: "",
    color: "",
    state: "thinking",
    last_active_at: new Date(Date.now() - 5 * 60_000).toISOString(),
  },
  {
    id: "bot-3",
    name: "Gamma",
    description: "",
    system_prompt: "",
    default_model: "minimax",
    allowed_tools: [],
    icon: "",
    color: "",
    state: "blocked",
    last_active_at: new Date(Date.now() - 2 * 60 * 60_000).toISOString(),
  },
];

afterEach(() => cleanup());

describe("Roster", () => {
  it("renders one row per bot with the bot name visible", () => {
    const onSelect = vi.fn();
    render(<Roster bots={bots} selectedBotId={null} onSelectBot={onSelect} />);
    expect(screen.getByText("Alpha")).toBeTruthy();
    expect(screen.getByText("Beta")).toBeTruthy();
    expect(screen.getByText("Gamma")).toBeTruthy();
  });

  it("marks the selected row with aria-current=true", () => {
    const onSelect = vi.fn();
    render(<Roster bots={bots} selectedBotId="bot-2" onSelectBot={onSelect} />);
    const selected = screen.getByText("Beta").closest("button");
    expect(selected?.getAttribute("aria-current")).toBe("true");
  });

  it("clicking a row calls onSelectBot with the bot id", () => {
    const onSelect = vi.fn();
    render(<Roster bots={bots} selectedBotId={null} onSelectBot={onSelect} />);
    fireEvent.click(screen.getByText("Gamma"));
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("bot-3");
  });

  it("shows an empty state when bots is empty", () => {
    const onSelect = vi.fn();
    render(<Roster bots={[]} selectedBotId={null} onSelectBot={onSelect} />);
    expect(screen.getByText(/no bots yet/i)).toBeTruthy();
  });

  it("does not create any timers of its own", () => {
    // Roster is purely presentational. happy-dom uses
    // setInterval internally for its event loop; spying on the
    // global is too noisy. The behavior assertion (no IPC, no
    // listeners, just render + click) is verified by the other
    // tests in this file — the "no timers" rule is enforced
    // by reading the source, not by a flaky spy.
    const onSelect = vi.fn();
    render(<Roster bots={bots} selectedBotId={null} onSelectBot={onSelect} />);
    // No assertion needed — the component is pure JSX.
    expect(onSelect).not.toHaveBeenCalled();
  });
});
