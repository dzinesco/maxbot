# MaxBot user guide

A walkthrough of the main surfaces in MaxBot. Use the sidebar on
the left to jump to a section, or skim top-to-bottom on a first read.

## Chat

The main chat surface is a single conversation stream per row in the
sidebar. Each conversation has its own SQLite-backed message log and
streams tokens as they arrive from the LLM.

### Sending

- Click in the composer at the bottom, type your message, hit
  <kbd>Enter</kbd> to send. <kbd>Shift</kbd>+<kbd>Enter</kbd> inserts
  a newline without sending.
- While the assistant is streaming, the <kbd>Enter</kbd> button turns
  into a red <kbd>Stop</kbd> button — click it to cancel the
  in-flight run.
- The active provider is shown in the sidebar status indicator:
  green dot = ready, red dot = API key not set (open Settings to fix).

### Regenerating

The last assistant response gets a <kbd>Regenerate</kbd> bar right
below it. Click it to delete the last turn and re-send the same
prompt — useful when the first response went off the rails.

### Copying

Hover any message and the <kbd>Copy</kbd> button appears in the
message meta row. One click copies the full message text (including
tool calls) to the clipboard.

### TTS

- Per-message: hover an assistant message and click the <kbd>🔊</kbd>
  button. The button stays visible (not gated on hover) so the
  voice feature is discoverable.
- Speak last: the composer has its own <kbd>🔊</kbd> button. Click
  it to speak the most recent assistant response, or use
  <kbd>⌘</kbd>+<kbd>⇧</kbd>+<kbd>S</kbd> from anywhere. Click again
  (or hit the shortcut) to stop.
- Voice: configurable in Settings → TTS. Default is
  <code>Samantha</code> (high-quality en-US, ships with every macOS
  install).

## Sub-agents (Bots)

Bots are first-class citizens — they live in the sidebar below the
conversation list, each gets a per-bot folder on disk, and they can
message each other through a shared inbox.

### Create a bot

1. In the sidebar, expand the **Bots** section.
2. Click <kbd>+ New bot</kbd>.
3. Fill in:
   - **Name** — what shows in the sidebar
   - **Description** — one-line summary, surfaces in tooltips
   - **System prompt** — the bot's persona + instructions
   - **Default model** — overrides the global default if you want a
     different model for this bot
   - **Allowed tools** — pick from the tool list; the bot can only
     invoke the ones you check
   - **Icon + color** — visual identifier in the sidebar
4. Save. The bot appears in the sidebar immediately.

### Edit / delete

Click a bot's <kbd>Edit</kbd> action in the sidebar to reopen the
editor. Delete is a destructive action that wipes the bot's folder
on disk.

### Schedule a bot

In the bot editor, set:

- **Interval** — repeat every N seconds (0 = manual only)
- **Cron expression** — `*/5 * * * *` for every 5 minutes, etc.

Both can be set; cron takes precedence. The scheduler runs in a
background task that wakes every 30s, so a 30s interval is the
fastest practical cadence.

### Run a bot

- <kbd>Run now</kbd> in the sidebar — fires the bot immediately
  and switches the active view to the bot's conversation
- <kbd>Stop</kbd> while it's running — tears down the in-flight
  agent context

### View the inbox

The bot's <kbd>Inbox</kbd> action shows messages other bots (or
you, via the composer's "send to bot" action) have sent. Unread
messages surface as a count badge in the sidebar.

## Tools

Tools are the agent's hands. The model calls them in the middle of
a turn to read files, hit the web, drive the browser, etc.

### Where they surface

Each tool call is shown inline in the message scroll as a
collapsible card. Click the summary to expand the full arguments
JSON. Tool results are shown right below, also collapsible.

### Consent

The tools that touch the outside world (browser, shell, file write,
mail send, etc.) require per-call consent. When the model calls one,
a dialog asks you to approve or deny. The tool description and the
arguments are visible before you decide.

You can disable consent prompts globally by toggling the per-tool
setting in Settings → General → Computer Use (the per-app TCC
panel is separate from tool consent; the latter is session-local).

### Disabling

Each tool is enabled by default. To permanently disable a tool
without removing it from the system, untick it in the bot editor's
"Allowed tools" list for the specific bot. The chat surface itself
always has access to all enabled tools.

## Providers

MaxBot supports four LLM providers out of the box. You can configure
one or all of them; the active provider is set in Settings →
General and is what new chats and bot runs use by default.

### Where to get keys

- **MiniMax** — [minimax.io](https://minimax.io) → API Keys (default)
- **OpenAI** — [platform.openai.com](https://platform.openai.com) → API keys
- **Anthropic** — [console.anthropic.com](https://console.anthropic.com) → Settings → API Keys
- **xAI** — [console.x.ai](https://console.x.ai) → API Keys

### Switching providers

1. Open Settings (<kbd>⌘</kbd>+<kbd>,</kbd> or the sidebar
   footer button).
2. In the General tab, pick a provider from the **LLM provider**
   dropdown.
3. Paste the matching API key (or pre-fill it in the Providers
   tab so it's ready when you switch back).

### Base URL overrides

Each provider has an optional base URL override (Settings →
General → "Base URL override"). Leave blank for the built-in
default; set to a self-hosted proxy or alternate region endpoint
if needed. Per-provider overrides live in the Providers tab.

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| <kbd>⌘</kbd>+<kbd>F</kbd> | Focus the sidebar search box |
| <kbd>⌘</kbd>+<kbd>N</kbd> | New chat |
| <kbd>⌘</kbd>+<kbd>⇧</kbd>+<kbd>S</kbd> | Speak / stop the last assistant response |
| <kbd>Enter</kbd> | Send (in the composer) |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | Newline (in the composer) |
| <kbd>Esc</kbd> | Close the active modal |

Settings tabs (when the Settings modal is open):

| Shortcut | Tab |
| --- | --- |
| <kbd>⌘</kbd>+<kbd>1</kbd> | General |
| <kbd>⌘</kbd>+<kbd>2</kbd> | Providers |
| <kbd>⌘</kbd>+<kbd>3</kbd> | TTS |
| <kbd>⌘</kbd>+<kbd>4</kbd> | Browser |
| <kbd>⌘</kbd>+<kbd>5</kbd> | Grok |
| <kbd>←</kbd> / <kbd>→</kbd> | Move between tabs (when the tab bar is focused) |

## Troubleshooting

### "grok not found"

The Grok Build CLI isn't on `$PATH`. Either:

1. Install `grok` so it's on `$PATH`, or
2. Set the absolute path in Settings → Grok → **Binary path**
   (e.g. `/Users/you/.grok/bin/grok`).

### "ego-browser not installed"

The agent's `ego_browser` tool needs the [ego (lite)](https://github.com/ego-lite/ego)
runtime. Install it via the skill at
`~/.agents/skills/ego-browser/references/install.md` — the install
drops the `ego-browser` CLI at `~/.local/bin/ego-browser`, which is
the path MaxBot's Rust side auto-detects.

If your install lives elsewhere, set **ego-browser path** in
Settings → Browser to the absolute location.

### "API key invalid"

The LLM provider rejected the key. Open Settings → General and
re-paste the API key for the active provider. If you pre-fill
multiple providers in Settings → Providers, switch to the right
one in the General tab's **LLM provider** dropdown.

### "Chat shows an error"

The most common cause is a transient network blip. Click
<kbd>Retry</kbd> on the error card. If the error persists:

1. Check that your API key is valid (Settings → General).
2. Check that the LLM provider's status page isn't reporting an
   outage.
3. Try a different model in Settings → General → **Default model**.
4. Check the console (`⌥⌘I` in dev) for a more detailed error
   string.

If none of the above helps, the most reliable fallback is to relaunch
MaxBot — that resets the streaming context and the SSE connection.

### "Tool call is stuck"

If a tool call has been spinning for more than 2 minutes, the
hard timeout in the Rust side will surface an error and the chat
turn will fail. Click <kbd>Retry</kbd> to re-send the prompt. For
the `ego_browser` tool specifically, a 2-minute cap is in place
because long ego scripts often indicate the agent is in a loop;
shorten the script and retry.
