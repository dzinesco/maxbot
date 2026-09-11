// v3.0.2 — Tests for the global settings command palette.
// Covers the 5 explicit acceptance criteria from the brief
// (opens on Cmd+K semantics — i.e. when shown, filters as
// you type, Enter selects, Esc closes, empty state copy)
// plus a few extras that round out the contract.
//
// v3.7.12 — adds tests for the inline Google Account
// section: the Connect / Disconnect buttons, the input
// fields, and the status text. The section lives at the
// bottom of the palette modal regardless of the search
// query.

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SettingsPalette } from "./SettingsPalette";
import type { Bot, GoogleOauthStatus } from "../lib/api";

// Mock the tauri wrappers so the section's connect /
// disconnect handlers can be exercised without a
// real Rust side.
vi.mock("../lib/tauri", () => ({
  startGoogleOauth: vi.fn(),
  completeGoogleOauth: vi.fn(),
  cancelGoogleOauth: vi.fn(),
  disconnectGoogleOauth: vi.fn(),
  googleOauthStatus: vi.fn(),
  // v3.7.14 / v3.7.16 — Voice / STT section.
  // `getSettings` is called on
  // mount to pre-fill the masked key status; `saveSettings`
  // is called when the user clicks "Save OpenAI API key".
  // The existing palette tests don't exercise this path,
  // but the mock needs to return *something* so the
  // component doesn't blow up on mount.
  getSettings: vi.fn().mockResolvedValue({
    openai_api_key: null,
  }),
  saveSettings: vi.fn().mockResolvedValue(undefined),
}));

import {
  cancelGoogleOauth,
  completeGoogleOauth,
  disconnectGoogleOauth,
  googleOauthStatus,
  startGoogleOauth,
} from "../lib/tauri";

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

function disconnectedStatus(): GoogleOauthStatus {
  return {
    connected: false,
    email: null,
    expires_at: null,
    scopes: [],
  };
}

function connectedStatus(): GoogleOauthStatus {
  return {
    connected: true,
    email: "user@example.com",
    expires_at: new Date(Date.now() + 3600_000).toISOString(),
    scopes: [
      "openid",
      "email",
      "https://www.googleapis.com/auth/gmail.readonly",
      "https://www.googleapis.com/auth/calendar.events",
    ],
  };
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

  // ---- v3.7.12 — Google Account section ----
  //
  // The Google Account OAuth section is rendered at the
  // bottom of the palette modal regardless of the
  // search query. The tests below exercise the
  // "disconnected" path (input fields + Connect button)
  // and the "connected" path (status text + Disconnect
  // button).

  describe("Google Account section", () => {
    afterEach(() => {
      vi.mocked(googleOauthStatus).mockReset();
      vi.mocked(startGoogleOauth).mockReset();
      vi.mocked(completeGoogleOauth).mockReset();
      vi.mocked(cancelGoogleOauth).mockReset();
      vi.mocked(disconnectGoogleOauth).mockReset();
    });

    // Override renderPalette so the section's mount-
    // time `googleOauthStatus` call doesn't get
    // clobbered by the disconnected default. Each
    // test sets up its own mock with the right
    // status; renderPalette is called after the
    // mock setup so the test's mock wins.
    function renderWithStatus(status: GoogleOauthStatus) {
      vi.mocked(googleOauthStatus).mockReset();
      vi.mocked(googleOauthStatus).mockResolvedValue(status);
      return renderPalette();
    }

    it("renders the disconnected form with a disabled Connect button", async () => {
      vi.mocked(googleOauthStatus).mockResolvedValueOnce(
        disconnectedStatus(),
      );
      renderPalette();
      // Wait for the initial status read to settle —
      // the section flips to the "disconnected" form
      // once the status arrives.
      await waitFor(() => {
        expect(
          screen.getByTestId("settings-palette-google-status"),
        ).toHaveTextContent("Not connected");
      });
      const connectBtn = screen.getByTestId(
        "settings-palette-google-connect",
      ) as HTMLButtonElement;
      expect(connectBtn).toBeInTheDocument();
      // Empty client_id / client_secret → disabled.
      expect(connectBtn).toBeDisabled();
    });

    it("'Connect Google' is disabled when client_id is empty but client_secret is set", async () => {
      vi.mocked(googleOauthStatus).mockResolvedValueOnce(
        disconnectedStatus(),
      );
      renderPalette();
      await waitFor(() => {
        expect(
          screen.getByTestId("settings-palette-google-status"),
        ).toBeInTheDocument();
      });
      const idInput = screen.getByTestId(
        "settings-palette-google-client-id",
      ) as HTMLInputElement;
      const secretInput = screen.getByTestId(
        "settings-palette-google-client-secret",
      ) as HTMLInputElement;
      fireEvent.change(secretInput, { target: { value: "secret-1" } });
      // idInput is still empty → button stays disabled.
      expect(
        screen.getByTestId("settings-palette-google-connect"),
      ).toBeDisabled();
      fireEvent.change(idInput, {
        target: { value: "id.apps.googleusercontent.com" },
      });
      const connectBtn = screen.getByTestId(
        "settings-palette-google-connect",
      ) as HTMLButtonElement;
      expect(connectBtn).not.toBeDisabled();
    });

    it("clicking 'Connect Google' calls startGoogleOauth with the right args", async () => {
      vi.mocked(googleOauthStatus).mockResolvedValueOnce(
        disconnectedStatus(),
      );
      vi.mocked(startGoogleOauth).mockResolvedValueOnce({
        auth_url:
          "https://accounts.google.com/o/oauth2/v2/auth?response_type=code&client_id=test&state=abc",
        port: 8765,
      });
      // After the start resolves, `completeGoogleOauth`
      // is called and returns the connected status so
      // the section flips.
      vi.mocked(completeGoogleOauth).mockResolvedValueOnce(
        connectedStatus(),
      );
      renderPalette();
      await waitFor(() => {
        expect(
          screen.getByTestId("settings-palette-google-status"),
        ).toBeInTheDocument();
      });
      const idInput = screen.getByTestId(
        "settings-palette-google-client-id",
      );
      const secretInput = screen.getByTestId(
        "settings-palette-google-client-secret",
      );
      fireEvent.change(idInput, { target: { value: "test-client" } });
      fireEvent.change(secretInput, { target: { value: "test-secret" } });
      const connectBtn = screen.getByTestId(
        "settings-palette-google-connect",
      );
      fireEvent.click(connectBtn);
      await waitFor(() => {
        expect(startGoogleOauth).toHaveBeenCalledWith(
          "test-client",
          "test-secret",
        );
      });
    });

    it("the status text reflects 'Connected' when the status reports connected", async () => {
      renderWithStatus(connectedStatus());
      await waitFor(() => {
        expect(
          screen.getByTestId("settings-palette-google-status"),
        ).toHaveTextContent("Connected as user@example.com");
      });
      // The connected view exposes a Disconnect button
      // and hides the client_id / client_secret
      // inputs.
      expect(
        screen.getByTestId("settings-palette-google-disconnect"),
      ).toBeInTheDocument();
      expect(
        screen.queryByTestId("settings-palette-google-client-id"),
      ).not.toBeInTheDocument();
    });

    it("clicking 'Disconnect' clears the stored tokens and flips the status to 'Not connected'", async () => {
      vi.mocked(disconnectGoogleOauth).mockResolvedValueOnce(
        disconnectedStatus(),
      );
      renderWithStatus(connectedStatus());
      await waitFor(() => {
        expect(
          screen.getByTestId("settings-palette-google-disconnect"),
        ).toBeInTheDocument();
      });
      fireEvent.click(
        screen.getByTestId("settings-palette-google-disconnect"),
      );
      await waitFor(() => {
        expect(disconnectGoogleOauth).toHaveBeenCalledTimes(1);
      });
      await waitFor(() => {
        expect(
          screen.getByTestId("settings-palette-google-status"),
        ).toHaveTextContent("Not connected");
      });
      // The Disconnect button is gone, the Connect
      // form is back.
      expect(
        screen.queryByTestId("settings-palette-google-disconnect"),
      ).not.toBeInTheDocument();
      expect(
        screen.getByTestId("settings-palette-google-connect"),
      ).toBeInTheDocument();
    });
  });
});
