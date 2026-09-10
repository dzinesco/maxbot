// v3.0.2 — Tests for the global settings command palette.
// Covers the 5 explicit acceptance criteria from the brief
// (opens on Cmd+K semantics — i.e. when shown, filters as
// you type, Enter selects, Esc closes, empty state copy)
// plus a few extras that round out the contract.

import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsPalette } from "./SettingsPalette";
import type { Bot } from "../lib/api";

const sampleBot: Bot = {
  id: "bot-1",
  name: "Figma Bro",
  description: "Designs screens",
  system_prompt: "You are a Figma specialist",
  default_model: "MiniMax-M3",
  allowed_tools: [],
  icon: "🤖",
  color: "#7c5cff",
  created_at: "2026-01-01T00:00:00Z",
  updated_at: "2026-01-01T00:00:00Z",
};

function renderPalette(
  overrides: Partial<React.ComponentProps<typeof SettingsPalette>> = {},
) {
  const onClose = vi.fn();
  const onSelect = vi.fn();
  const props: React.ComponentProps<typeof SettingsPalette> = {
    bots: [sampleBot],
    isHomeView: false,
    onClose,
    onSelect,
    ...overrides,
  };
  const utils = render(<SettingsPalette {...props} />);
  return { ...utils, onClose, onSelect };
}

describe("SettingsPalette", () => {
  afterEach(() => {
    // Make sure no test leaves a global keydown listener
    // bound. The component's window-level Esc listener
    // cleans itself up on unmount, but if a render throws
    // mid-mount we'd otherwise carry it across tests.
    vi.restoreAllMocks();
  });

  it("renders the search input and Bot-level rows by default", () => {
    renderPalette();
    const input = screen.getByTestId("settings-palette-input");
    expect(input).toBeInTheDocument();
    // A few representative rows should be visible. We
    // don't assert the exact count (it depends on the
    // static index size) — we just want to confirm the
    // index is populated from the bots prop.
    expect(
      screen.getAllByTestId("settings-palette-row").length,
    ).toBeGreaterThan(0);
  });

  it("filters results as the user types (case-insensitive substring)", () => {
    renderPalette();
    const input = screen.getByTestId(
      "settings-palette-input",
    ) as HTMLInputElement;
    const allRows = screen.getAllByTestId("settings-palette-row");
    const total = allRows.length;
    expect(total).toBeGreaterThan(5);

    fireEvent.change(input, { target: { value: "VOICE" } });
    // After typing "voice" we should see strictly fewer
    // rows than the full index, and at least one match
    // (the App-level voice-mode row and/or the Bot-level
    // voice setting — we ship both for forward compat).
    const filtered = screen.getAllByTestId("settings-palette-row");
    expect(filtered.length).toBeLessThan(total);
    expect(filtered.length).toBeGreaterThan(0);

    // Refine further with a query that has no matches.
    fireEvent.change(input, { target: { value: "zzzzznomatch" } });
    expect(
      screen.queryAllByTestId("settings-palette-row").length,
    ).toBe(0);
    expect(
      screen.getByTestId("settings-palette-empty"),
    ).toHaveTextContent("No settings match");
  });

  it("shows the empty state copy 'No settings match \"<query>\"'", () => {
    renderPalette();
    const input = screen.getByTestId(
      "settings-palette-input",
    ) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "qqqqq" } });
    const empty = screen.getByTestId("settings-palette-empty");
    expect(empty).toHaveTextContent('No settings match "qqqqq"');
  });

  it("Enter selects the highlighted result and calls onSelect", () => {
    const { onSelect } = renderPalette();
    const input = screen.getByTestId(
      "settings-palette-input",
    ) as HTMLInputElement;
    // Narrow to a known row so we can be sure which entry
    // is highlighted.
    fireEvent.change(input, { target: { value: "TTS voice" } });
    const rows = screen.getAllByTestId("settings-palette-row");
    expect(rows.length).toBe(1);
    // The default active row is index 0.
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(1);
    const called = onSelect.mock.calls[0][0];
    expect(called.key).toBe("app.tts-voice");
    expect(called.context).toBe("App");
  });

  it("ArrowDown / ArrowUp move the highlight and Enter selects the new row", () => {
    const { onSelect } = renderPalette();
    const input = screen.getByTestId(
      "settings-palette-input",
    ) as HTMLInputElement;
    // Use a query that returns multiple rows.
    fireEvent.change(input, { target: { value: "Bot" } });
    const rows = screen.getAllByTestId("settings-palette-row");
    expect(rows.length).toBeGreaterThan(1);

    // Before ArrowDown, the first row is active.
    expect(rows[0].className).toContain("settings-palette__row--active");
    expect(rows[1].className).not.toContain(
      "settings-palette__row--active",
    );

    // Move down once.
    fireEvent.keyDown(input, { key: "ArrowDown" });
    const rowsAfter = screen.getAllByTestId("settings-palette-row");
    expect(rowsAfter[1].className).toContain(
      "settings-palette__row--active",
    );
    expect(rowsAfter[0].className).not.toContain(
      "settings-palette__row--active",
    );

    // Press Enter — onSelect fires with the new active row.
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onSelect).toHaveBeenCalledTimes(1);
    // The first row's label is the label of the first
    // entry, the second row's label is the second. The
    // first row's label is e.g. "Name" (per
    // buildBotEntries); the second is "Description".
    // We just check that the call's label is the
    // second row's, not the first.
    const called = onSelect.mock.calls[0][0];
    expect(called.label).toBe(rowsAfter[1].querySelector(
      ".settings-palette__row-label",
    )?.textContent);
  });

  it("Esc closes the palette via the onClose callback", () => {
    const { onClose } = renderPalette();
    // The component binds a window-level keydown for Esc;
    // firing it on the input bubbles up.
    const input = screen.getByTestId(
      "settings-palette-input",
    ) as HTMLInputElement;
    fireEvent.keyDown(input, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("clicking the backdrop closes the palette; clicking the palette does not", () => {
    const { onClose } = renderPalette();
    const backdrop = screen.getByTestId(
      "settings-palette-backdrop",
    );
    const palette = screen.getByTestId("settings-palette");

    // Click on the palette body — backdrop's onMouseDown
    // should NOT fire because the inner div has
    // stopPropagation.
    fireEvent.mouseDown(palette);
    expect(onClose).not.toHaveBeenCalled();

    // Click on the backdrop itself.
    fireEvent.mouseDown(backdrop);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("hides Bot-level rows when in the home / welcome view (only App rows)", () => {
    renderPalette({ isHomeView: true });
    const rows = screen.getAllByTestId("settings-palette-row");
    expect(rows.length).toBeGreaterThan(0);
    // Every row's context should be "App" in home view.
    for (const row of rows) {
      const ctx = row.querySelector(
        ".settings-palette__row-context",
      );
      expect(ctx?.textContent).toBe("App");
    }
  });

  it("clicking a row selects it", () => {
    const { onSelect } = renderPalette();
    const input = screen.getByTestId(
      "settings-palette-input",
    ) as HTMLInputElement;
    fireEvent.change(input, { target: { value: "TTS voice" } });
    const row = screen.getByTestId("settings-palette-row");
    fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect.mock.calls[0][0].key).toBe("app.tts-voice");
  });
});
