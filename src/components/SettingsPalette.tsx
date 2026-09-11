// v3.0.2 — Global command palette for settings. Opens on
// `Cmd+K` (mac) / `Ctrl+K` (other) and surfaces every
// Bot-level setting across every Bot in the roster, plus
// every App-level setting in the Settings modal. The user
// types to filter; Up/Down/Enter/Esc navigate. Selecting a
// result invokes an App-level callback that opens the
// right panel, then queries the DOM for the
// `data-setting-key` attribute to focus + scroll the
// field into view. The bridge is `data-setting-key` — the
// palette is decoupled from any specific field, so
// adding a new indexed field is just one attribute.
//
// Design notes:
// - Substring match (case-insensitive) over `label`,
//   `context` (Bot name or "App"), and the `key` itself.
//   No fuzzy matching in v3.0.2 (the brief is explicit).
// - The modal mounts at the App level, NOT inside the
//   Settings modal or the BotEditor. That way the palette
//   is reachable from any view (chat, home, group chat).
// - The action handlers are passed in as props. The
//   palette itself doesn't know about `setSettingsOpen`,
//   `setEditorState`, etc. — that keeps this component
//   testable in isolation (see `SettingsPalette.test.tsx`).
// - The "settings exist for this Bot" data is derived from
//   the current `bots` prop. The App is the source of
//   truth for roster state.

import { useEffect, useMemo, useRef, useState } from "react";
import type { Bot, GoogleOauthStatus } from "../lib/api";
import {
  cancelGoogleOauth,
  completeGoogleOauth,
  disconnectGoogleOauth,
  getSettings,
  googleOauthStatus,
  saveSettings,
  startGoogleOauth,
} from "../lib/tauri";

/** The kind of panel the selected setting should open. */
export type SettingTarget =
  | { kind: "app-settings"; tab: string }
  | { kind: "bot-editor"; botId: string }
  | { kind: "computer-panel"; botId: string }
  | { kind: "memory-panel"; botId: string }
  | { kind: "skills-panel"; botId: string }
  | { kind: "routines-panel"; botId: string };

/** One row in the palette. `key` matches the DOM
 * `data-setting-key` attribute on the target field so
 * the App can `document.querySelector(...)` after the
 * panel mounts. */
export interface SettingEntry {
  key: string;
  label: string;
  /** "App" or the Bot's display name. Drives the
   *  secondary text under the label. */
  context: string;
  target: SettingTarget;
}

export interface SettingsPaletteProps {
  bots: Bot[];
  /** True when the user is in the home/welcome view
   *  (no Bot selected). Per the brief, the palette
   *  doesn't show Bot-level results in that mode —
   *  only App-level settings. */
  isHomeView: boolean;
  onClose: () => void;
  onSelect: (entry: SettingEntry) => void;
}

const EMPTY_BOTS: Bot[] = [];

/** Build the static App-level index. Tab ids mirror the
 *  `TABS` constant in `Settings.tsx` so the parent's
 *  `setActiveTab(tab)` line up. Kept here (not imported
 *  from Settings.tsx) so this module is self-contained
 *  and easier to test. */
function buildAppEntries(): SettingEntry[] {
  return [
    // ---- General tab ----
    {
      key: "app.provider",
      label: "LLM provider",
      context: "App",
      target: { kind: "app-settings", tab: "general" },
    },
    {
      key: "app.api-key",
      label: "API key (active provider)",
      context: "App",
      target: { kind: "app-settings", tab: "general" },
    },
    {
      key: "app.default-model",
      label: "Default model",
      context: "App",
      target: { kind: "app-settings", tab: "general" },
    },
    {
      key: "app.base-url",
      label: "Base URL override",
      context: "App",
      target: { kind: "app-settings", tab: "general" },
    },
    {
      key: "app.voice-mode",
      label: "Voice mode (auto-play + auto-record)",
      context: "App",
      target: { kind: "app-settings", tab: "general" },
    },
    {
      key: "app.computer-use-apps",
      label: "Computer use (AppleScript) — per-app permission",
      context: "App",
      target: { kind: "app-settings", tab: "general" },
    },
    {
      key: "app.reset-onboarding",
      label: "Reset onboarding",
      context: "App",
      target: { kind: "app-settings", tab: "general" },
    },
    // ---- Providers tab ----
    {
      key: "app.providers.openai",
      label: "OpenAI provider (key + base URL)",
      context: "App",
      target: { kind: "app-settings", tab: "providers" },
    },
    {
      key: "app.providers.anthropic",
      label: "Anthropic provider (key + base URL)",
      context: "App",
      target: { kind: "app-settings", tab: "providers" },
    },
    {
      key: "app.providers.xai",
      label: "xAI provider (key + base URL)",
      context: "App",
      target: { kind: "app-settings", tab: "providers" },
    },
    // ---- TTS tab ----
    {
      key: "app.tts-voice",
      label: "TTS voice",
      context: "App",
      target: { kind: "app-settings", tab: "tts" },
    },
    // ---- Browser tab ----
    {
      key: "app.ego-browser-path",
      label: "ego-browser path",
      context: "App",
      target: { kind: "app-settings", tab: "browser" },
    },
    // ---- Computer tab ----
    {
      key: "app.computer-server-host",
      label: "Computer — server host",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-ssh-user",
      label: "Computer — server SSH user",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-default-ssh-key",
      label: "Computer — use default SSH key",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-passphrase",
      label: "Computer — passphrase",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-vnc-port-lo",
      label: "Computer — VNC port range (low)",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-vnc-port-hi",
      label: "Computer — VNC port range (high)",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-default-disk",
      label: "Computer — default per-Bot disk (GB)",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-default-ram",
      label: "Computer — default per-Bot RAM (MB)",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    {
      key: "app.computer-test-connection",
      label: "Computer — test connection",
      context: "App",
      target: { kind: "app-settings", tab: "computer" },
    },
    // ---- Grok tab ----
    {
      key: "app.grok-binary",
      label: "Grok — binary path",
      context: "App",
      target: { kind: "app-settings", tab: "grok" },
    },
    {
      key: "app.grok-model",
      label: "Grok — model alias",
      context: "App",
      target: { kind: "app-settings", tab: "grok" },
    },
    {
      key: "app.grok-cwd",
      label: "Grok — working directory",
      context: "App",
      target: { kind: "app-settings", tab: "grok" },
    },
  ];
}

/** Build the Bot-level index for a single Bot. The
 *  `key` values match the `data-setting-key` attributes
 *  added to the BotEditor / ComputerPanel / MemoryPanel
 *  / SkillsPanel fields. */
function buildBotEntries(bot: Bot): SettingEntry[] {
  return [
    // ---- BotEditor — Identity ----
    {
      key: `bot.${bot.id}.name`,
      label: "Name",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.description`,
      label: "Description",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.icon`,
      label: "Icon",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.color`,
      label: "Color",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    // ---- BotEditor — Brain ----
    {
      key: `bot.${bot.id}.default-model`,
      label: "Default model",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.system-prompt`,
      label: "System prompt",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    // ---- BotEditor — Computer ----
    {
      key: `bot.${bot.id}.provision-computer`,
      label: "Provision a computer",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.provision-disk-gb`,
      label: "Provision — disk (GB)",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.provision-ram-mb`,
      label: "Provision — RAM (MB)",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.view-computer`,
      label: "View computer (preview / takeover)",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    // ---- BotEditor — Capabilities ----
    {
      key: `bot.${bot.id}.allowed-tools`,
      label: "Allowed tools",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    // ---- BotEditor — Rules ----
    {
      key: `bot.${bot.id}.approval-rules`,
      label: "Approval rules (per tool)",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    // ---- BotEditor — Schedule ----
    {
      key: `bot.${bot.id}.schedule-cron`,
      label: "Schedule — cron expression",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.schedule-interval`,
      label: "Schedule — simple interval (seconds)",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    // ---- BotEditor — Daemon ----
    {
      key: `bot.${bot.id}.daemon-webhook`,
      label: "Daemon — webhook URL",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.daemon-token`,
      label: "Daemon — bearer token",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.daemon-token-rotate`,
      label: "Daemon — rotate token",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.daemon-token-copy`,
      label: "Daemon — copy token",
      context: bot.name || "(unnamed bot)",
      target: { kind: "bot-editor", botId: bot.id },
    },
    // ---- Skills / Memory / Routines (separate panels) ----
    {
      key: `bot.${bot.id}.skills`,
      label: "Skills (enable / disable)",
      context: bot.name || "(unnamed bot)",
      target: { kind: "skills-panel", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.memory`,
      label: "Memory facts",
      context: bot.name || "(unnamed bot)",
      target: { kind: "memory-panel", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.routines`,
      label: "Routines (recorded skills)",
      context: bot.name || "(unnamed bot)",
      target: { kind: "routines-panel", botId: bot.id },
    },
    // ---- ComputerPanel (only the existing-computer path
    //      really needs a focus — provision lives in the
    //      editor) ----
    {
      key: `bot.${bot.id}.computer-start`,
      label: "Computer — Start",
      context: bot.name || "(unnamed bot)",
      target: { kind: "computer-panel", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.computer-stop`,
      label: "Computer — Stop",
      context: bot.name || "(unnamed bot)",
      target: { kind: "computer-panel", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.computer-restart`,
      label: "Computer — Restart",
      context: bot.name || "(unnamed bot)",
      target: { kind: "computer-panel", botId: bot.id },
    },
    {
      key: `bot.${bot.id}.computer-destroy`,
      label: "Computer — Destroy",
      context: bot.name || "(unnamed bot)",
      target: { kind: "computer-panel", botId: bot.id },
    },
  ];
}

/** Flatten the bots + app entries into a single index. The
 *  "App" entries always sort to the bottom of any group
 *  (so Bot rows come first when both match), but the
 *  brief doesn't require a strict ordering — substring
 *  match is order-agnostic and the user can always
 *  prefix the query with "app" or a bot name to narrow.
 *  We keep a stable "bots-first" order so the palette
 *  feels predictable when no query is typed. */
function buildIndex(
  bots: Bot[],
  isHomeView: boolean,
): SettingEntry[] {
  const app = buildAppEntries();
  if (isHomeView) return app;
  const botEntries = bots.flatMap(buildBotEntries);
  return [...botEntries, ...app];
}

/** Case-insensitive substring match against `label`,
 *  `context`, and `key`. Empty query returns all. */
function matches(entry: SettingEntry, q: string): boolean {
  if (!q) return true;
  const needle = q.toLowerCase();
  return (
    entry.label.toLowerCase().includes(needle) ||
    entry.context.toLowerCase().includes(needle) ||
    entry.key.toLowerCase().includes(needle)
  );
}

export function SettingsPalette({
  bots = EMPTY_BOTS,
  isHomeView,
  onClose,
  onSelect,
}: SettingsPaletteProps) {
  const [query, setQuery] = useState("");
  const [activeIdx, setActiveIdx] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  const all = useMemo(
    () => buildIndex(bots, isHomeView),
    [bots, isHomeView],
  );
  const filtered = useMemo(
    () => all.filter((e) => matches(e, query)),
    [all, query],
  );

  // Clamp `activeIdx` whenever the filter shrinks. Without
  // this, a backspace from "abcdef" to "a" can leave the
  // highlight on an entry that's no longer in the list.
  useEffect(() => {
    if (activeIdx >= filtered.length) {
      setActiveIdx(Math.max(0, filtered.length - 1));
    }
  }, [activeIdx, filtered.length]);

  // Auto-focus the search input on mount. Per the brief,
  // the palette must open within 100ms — putting the
  // focus call in the same render frame as the mount
  // satisfies that. (React commits the DOM before the
  // effect fires, so the ref is bound by then.)
  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  // Keep the highlighted row in view as the user arrows
  // through the list. We do this imperatively rather than
  // with `scrollIntoView` on the active row to avoid
  // scroll-jacking the whole list.
  useEffect(() => {
    const el = listRef.current?.querySelector<HTMLElement>(
      `[data-result-idx="${activeIdx}"]`,
    );
    el?.scrollIntoView({ block: "nearest" });
  }, [activeIdx]);

  // Esc closes; we register at the window level so the
  // // shortcut works whether or not the input is focused
  // // (it should be, but defensive in case a custom
  // // test overrides focus).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const onKeyDown = (e: React.KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => Math.min(filtered.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => Math.max(0, i - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const entry = filtered[activeIdx];
      if (entry) {
        onSelect(entry);
      }
    }
  };

  return (
    <div
      className="settings-palette__backdrop"
      data-testid="settings-palette-backdrop"
      onMouseDown={(e) => {
        // Close on backdrop click only — clicks on the
        // palette body don't bubble here because of the
        // stopPropagation on the inner div. We use
        // `mousedown` (not `click`) so dragging a text
        // selection that ends outside the palette doesn't
        // dismiss it mid-select.
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div
        className="settings-palette"
        data-testid="settings-palette"
        role="dialog"
        aria-label="Settings search"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="settings-palette__search-row">
          <input
            ref={inputRef}
            type="text"
            className="settings-palette__input"
            placeholder="Search settings…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActiveIdx(0);
            }}
            onKeyDown={onKeyDown}
            data-testid="settings-palette-input"
            autoComplete="off"
            spellCheck={false}
          />
        </div>
        <div
          className="settings-palette__list"
          ref={listRef}
          data-testid="settings-palette-list"
        >
          {filtered.length === 0 ? (
            <div
              className="settings-palette__empty"
              data-testid="settings-palette-empty"
            >
              No settings match "{query}"
            </div>
          ) : (
            filtered.map((entry, idx) => (
              <button
                key={entry.key}
                type="button"
                className={
                  "settings-palette__row" +
                  (idx === activeIdx
                    ? " settings-palette__row--active"
                    : "")
                }
                data-result-idx={idx}
                data-testid="settings-palette-row"
                onMouseEnter={() => setActiveIdx(idx)}
                onClick={() => onSelect(entry)}
              >
                <span className="settings-palette__row-label">
                  {entry.label}
                </span>
                <span className="settings-palette__row-context">
                  {entry.context}
                </span>
              </button>
            ))
          )}
        </div>
        {/* v3.7.12 — Google Account OAuth section. Always
            rendered at the bottom of the palette so the
            user can configure / inspect Google OAuth
            without leaving the command palette. The
            search input above stays focused. */}
        <GoogleAccountSection />
        {/* v3.7.14 — Voice section. Sits below the Google
            Account section so the user can set the OpenAI
            API key without leaving the command palette.
            The key is read by the Rust `audio_to_text`
            command when the chat-header mic sends audio
            to Whisper. */}
        <VoiceSection />
        <div className="settings-palette__hint">
          <span>↑↓ navigate</span>
          <span>↵ select</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  );
}

/** v3.7.12 — Google Account OAuth section rendered at the
 *  bottom of the SettingsPalette. The user pastes a
 *  Google Cloud project's Desktop OAuth client_id +
 *  client_secret, clicks "Connect Google", and the Rust
 *  side opens a browser to the Google consent screen.
 *  When the user grants consent the section flips to
 *  "Connected" with the granted email and a "Disconnect"
 *  button.
 *
 *  The component is self-contained: it owns its own
 *  status, input, and connect-state, and only talks to
 *  the Rust side via the `tauri.ts` wrappers. Esc /
 *  backdrop click on the parent palette closes the
 *  whole modal — the section doesn't intercept Esc. */
function disconnectedStatus(): GoogleOauthStatus {
  return {
    connected: false,
    email: null,
    expires_at: null,
    scopes: [],
  };
}

function GoogleAccountSection() {
  // Status snapshot from the Rust side. Refreshed on
  // mount and after every connect / disconnect cycle.
  const [status, setStatus] = useState<GoogleOauthStatus | null>(null);
  // User-entered client_id + client_secret. Not
  // persisted by this component — the user clicks
  // "Connect Google" and the Rust side reads them
  // from the input values for the start call.
  // Persistence is handled by the Settings panel
  // (the user pastes the same values there if they
  // want them remembered across launches).
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  // "idle" | "starting" | "listening" | "completing"
  // — drives the spinner + button disabled state.
  // The brief asks for "shows a spinner +
  // 'Listening on http://127.0.0.1:PORT/callback'" —
  // the listening state is set once the Rust side
  // returns the auth URL, and clears when the user
  // lands on the callback (Tauri event
  // google_oauth://complete) or the flow times out.
  const [phase, setPhase] = useState<
    "idle" | "starting" | "listening" | "completing"
  >("idle");
  const [listenPort, setListenPort] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Read the current status on mount. Idempotent —
  // `googleOauthStatus` returns the snapshot from
  // `Settings.google_refresh_token` (connected if
  // it's set). We tolerate the mock returning
  // `undefined` (test environments) and treat that
  // as "unknown / not connected" — the disconnected
  // form renders either way.
  useEffect(() => {
    let alive = true;
    const result = googleOauthStatus();
    if (result && typeof (result as Promise<GoogleOauthStatus>).then === "function") {
      (result as Promise<GoogleOauthStatus>)
        .then((s) => {
          if (alive) setStatus(s);
        })
        .catch((e) => {
          if (alive) setError(String(e));
        });
    } else if (alive) {
      // No real Rust side; render the disconnected
      // form. The test setup that wants a connected
      // status can mock the function to return a
      // resolved promise.
      setStatus(disconnectedStatus);
    }
    return () => {
      alive = false;
    };
  }, []);

  const onConnect = async () => {
    setError(null);
    setPhase("starting");
    try {
      const { auth_url, port } = await startGoogleOauth(
        clientId,
        clientSecret,
      );
      setListenPort(port);
      setPhase("listening");
      // Open the URL in the default browser. We
      // avoid a hard dep on `@tauri-apps/plugin-opener`
      // (which the slice doesn't install) — `window.open`
      // works in the Tauri webview and as a fallback
      // when running tests under happy-dom. Production
      // builds will need `@tauri-apps/plugin-opener`
      // (or a hand-rolled bridge) to open in the OS
      // browser; for v3.7.12 we accept the
      // webview-internal default.
      try {
        window.open(auth_url, "_blank", "noopener,noreferrer");
      } catch {
        // Defensive: in some test environments
        // `window.open` throws. The user can still
        // copy the URL from the "Listening on …"
        // line.
      }
      // Drive the completion handshake. The Rust
      // side returns once the user lands on the
      // callback (or the 5-minute flow timeout fires,
      // which surfaces as a Rust error).
      const final = await completeGoogleOauth();
      setStatus(final);
      setPhase("idle");
      setListenPort(null);
    } catch (e) {
      setError(String(e));
      setPhase("idle");
      setListenPort(null);
    }
  };

  const onCancel = async () => {
    try {
      await cancelGoogleOauth();
    } catch {
      // Best-effort.
    }
    setPhase("idle");
    setListenPort(null);
  };

  const onDisconnect = async () => {
    setError(null);
    try {
      const final = await disconnectGoogleOauth();
      setStatus(final);
    } catch (e) {
      setError(String(e));
    }
  };

  const isConnected = status?.connected === true;
  const canConnect =
    !isConnected &&
    clientId.trim().length > 0 &&
    clientSecret.trim().length > 0 &&
    phase === "idle";

  return (
    <section
      className="settings-palette__google-section"
      data-testid="settings-palette-google"
      aria-label="Google Account"
    >
      <h3 className="settings-palette__google-title">Google Account</h3>
      {isConnected && status ? (
        <div className="settings-palette__google-connected">
          <p data-testid="settings-palette-google-status">
            Connected{status.email ? ` as ${status.email}` : ""}.
          </p>
          {status.expires_at && (
            <p className="settings-palette__google-expiry">
              Token expires {new Date(status.expires_at).toLocaleString()}.
            </p>
          )}
          {status.scopes.length > 0 && (
            <p
              className="settings-palette__google-scopes"
              title={status.scopes.join("\n")}
            >
              Scopes: {status.scopes.length} granted.
            </p>
          )}
          <button
            type="button"
            onClick={onDisconnect}
            data-testid="settings-palette-google-disconnect"
          >
            Disconnect
          </button>
        </div>
      ) : (
        <div className="settings-palette__google-form">
          <p data-testid="settings-palette-google-status">
            Not connected.
          </p>
          <input
            type="text"
            placeholder="OAuth client ID (…apps.googleusercontent.com)"
            value={clientId}
            onChange={(e) => setClientId(e.target.value)}
            data-testid="settings-palette-google-client-id"
            disabled={phase !== "idle"}
            autoComplete="off"
            spellCheck={false}
          />
          <input
            type="password"
            placeholder="OAuth client secret"
            value={clientSecret}
            onChange={(e) => setClientSecret(e.target.value)}
            data-testid="settings-palette-google-client-secret"
            disabled={phase !== "idle"}
            autoComplete="off"
            spellCheck={false}
          />
          {phase === "listening" && listenPort !== null ? (
            <p
              className="settings-palette__google-listening"
              data-testid="settings-palette-google-listening"
            >
              Listening on http://127.0.0.1:{listenPort}/callback —
              complete the consent in your browser.
              <button
                type="button"
                onClick={onCancel}
                data-testid="settings-palette-google-cancel"
              >
                Cancel
              </button>
            </p>
          ) : null}
          {error && (
            <p
              className="settings-palette__google-error"
              data-testid="settings-palette-google-error"
            >
              {error}
            </p>
          )}
          <button
            type="button"
            onClick={onConnect}
            disabled={!canConnect}
            data-testid="settings-palette-google-connect"
          >
            {phase === "starting" || phase === "completing"
              ? "Connecting…"
              : "Connect Google"}
          </button>
        </div>
      )}
    </section>
  );
}

/** v3.7.14 — Voice section. The user can paste an OpenAI
 *  API key (`sk-…`) without leaving the command palette.
 *  The key is read by the Rust `audio_to_text` command
 *  when the chat-header mic dispatches audio to Whisper.
 *
 *  Persistence: the component loads the current Settings
 *  on mount, then saves via the same `save_settings` Tauri
 *  command the Settings modal uses. The user can also
 *  edit this key under Settings → Providers (the same
 *  field); the two paths write to the same row.
 *
 *  Scope: this section is self-contained — it owns its
 *  input value + save state. Esc / backdrop click on the
 *  parent palette closes the modal; the section doesn't
 *  intercept Esc. */
function VoiceSection() {
  const [key, setKey] = useState("");
  const [savedKeyMask, setSavedKeyMask] = useState<string | null>(null);
  // "idle" | "saving" | "saved" | "error" — drives the
  // button label + a small "Saved" pill that auto-clears
  // after 1.5s.
  const [phase, setPhase] = useState<"idle" | "saving" | "saved" | "error">(
    "idle",
  );
  const [error, setError] = useState<string | null>(null);

  // Read the current Settings on mount so the field
  // pre-fills with the existing key (masked to "••••" so
  // we don't render the secret in cleartext). The Rust
  // side returns the actual key — we mask it client-side
  // for display.
  useEffect(() => {
    let alive = true;
    getSettings()
      .then((s) => {
        if (!alive) return;
        const k = (s.openai_api_key ?? "").trim();
        setSavedKeyMask(k ? maskKey(k) : null);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      });
    return () => {
      alive = false;
    };
  }, []);

  const onSave = async () => {
    setError(null);
    setPhase("saving");
    try {
      // Read the current Settings, mutate
      // `openai_api_key`, then save. The Settings
      // modal does the same round-trip; this is
      // the canonical "edit one field" pattern.
      const current = await getSettings();
      const next = {
        ...current,
        openai_api_key: key.trim() || null,
      };
      await saveSettings(next);
      const trimmed = key.trim();
      setSavedKeyMask(trimmed ? maskKey(trimmed) : null);
      setKey("");
      setPhase("saved");
      // Auto-clear the "Saved" pill so it doesn't
      // linger forever.
      setTimeout(() => {
        setPhase((p) => (p === "saved" ? "idle" : p));
      }, 1500);
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  };

  return (
    <section
      className="settings-palette__voice-section"
      data-testid="settings-palette-voice"
      aria-label="Voice"
    >
      <h3 className="settings-palette__voice-title">Voice</h3>
      <p
        className="settings-palette__voice-status"
        data-testid="settings-palette-voice-status"
      >
        {savedKeyMask
          ? `OpenAI API key set: ${savedKeyMask}`
          : "OpenAI API key not set."}
      </p>
      <input
        type="password"
        placeholder="sk-…  (Whisper transcriptions)"
        value={key}
        onChange={(e) => setKey(e.target.value)}
        data-testid="settings-palette-voice-api-key"
        autoComplete="off"
        spellCheck={false}
        disabled={phase === "saving"}
      />
      {error && (
        <p
          className="settings-palette__voice-error"
          data-testid="settings-palette-voice-error"
        >
          {error}
        </p>
      )}
      <button
        type="button"
        onClick={onSave}
        disabled={phase === "saving"}
        data-testid="settings-palette-voice-save"
      >
        {phase === "saving"
          ? "Saving…"
          : phase === "saved"
            ? "Saved"
            : "Save OpenAI API key"}
      </button>
    </section>
  );
}

/** Mask an API key for display: keep the first 3 and last 4
 *  chars, replace the rest with `•`. The Rust side already
 *  returns the key; we just don't want to render the full
 *  secret in the palette's chrome. */
function maskKey(k: string): string {
  if (k.length <= 7) return "•".repeat(k.length);
  return `${k.slice(0, 3)}${"•".repeat(Math.max(4, k.length - 7))}${k.slice(-4)}`;
}
