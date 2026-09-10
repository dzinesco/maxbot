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
import type { Bot } from "../lib/api";

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
        <div className="settings-palette__hint">
          <span>↑↓ navigate</span>
          <span>↵ select</span>
          <span>esc close</span>
        </div>
      </div>
    </div>
  );
}
