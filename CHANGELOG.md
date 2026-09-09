# Changelog

All notable changes to MaxBot are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/) and the project
adheres to [Semantic Versioning](https://semver.org/).

## v2.0.1 — 2026-09-09

### Fixed
- **Bootstrap crash on v1.0 → v2.0 upgrade** that produced a blank
  black window. The `bots.avatar_color` and `bots.last_active_at`
  columns added in v2.0 Slice E are nullable (no `DEFAULT` in the
  migration), so any v1.0 row carried `NULL` for them. The Rust
  reader at `list_bots` and `get_bot` was calling
  `row.get::<_, String>(8)` on the nullable `avatar_color` column,
  which raised `Invalid column type Null at index: 8` and failed the
  whole bootstrap. The renderer was left on the `isOnboarded === null`
  loading branch (a brand-color div with no children) — visually
  indistinguishable from a dead window.
  - **Fix:** read the nullable columns as `Option<String>` /
    `Option<DateTime>` and default to `""` / `None`. Both
    `list_bots` and `get_bot` patched.
  - **Regression test:** `v1_bot_with_null_avatar_color_loads_with_empty_string`
    inserts a v1.0-shape row (only the original 10 columns populated)
    and asserts that both reads return the documented defaults.
  - **Defensive UX:** the React loading state now also surfaces the
    `bootError` text if any future bootstrap call throws, so the
    user is never stranded on a silent blank screen again.

## v2.0.0 — 2026-09-09

### Added — Per-Bot Linux computers
- **Each Bot can have its own QEMU/KVM VM** on a Linux server you
  control. Spawned via libvirt, cloud-init bootstrapped with XFCE +
  x11vnc + qemu-guest-agent, accessible via the local noVNC viewer
  in MaxBot's ComputerPanel (Status / Preview / Takeover modes).
- **`ComputerManager`** — Rust module that drives libvirt over SSH,
  generates per-Bot Ed25519 keypairs (encrypted with Argon2id +
  ChaCha20-Poly1305), and tunnels the VNC RFB stream via `ssh -L` +
  a local WebSocket↔RFB proxy. The Tauri side reuses the existing
  macOS-side `~/.ssh/id_ed25519` for the server connection; per-Bot
  keys live in a new `ssh_keys` table.
- **Three access levels** per Grok Bot's design (Status / Preview /
  Takeover): the title-bar chip turns purple while a Bot's VM is
  active; Preview opens a pinned side panel with the live noVNC
  viewer; Takeover goes full-window with mouse + keyboard.
- **Settings → Computer** tab: server host, SSH user, VNC port range,
  default per-Bot RAM / disk, computer passphrase, "Test connection"
  button. Gated on the host being set; an empty-state links to
  `docs/server-setup.md`.
- **`docs/server-setup.md`** — the one-time server runbook. Covers
  QEMU/KVM install, libvirt setup, cloud image caching, the
  `provision-vm.sh` script deployment, and an end-to-end smoke test
  via `virsh net-dhcp-leases default`.

### Added — Bot roster + 6-state presence
- **Sidebar refactored to a Bot roster** as the primary surface
  (conversations move behind per-Bot views). Each roster row shows
  the Bot's avatar, name, last-active timestamp, and a computer
  sub-icon (delegated to the ComputerPanel).
- **`BotAvatar` 6-state presence system**: `idle` / `thinking` /
  `working` / `waiting` / `blocked` / `done`. Priority order:
  `blocked > waiting > working > thinking > done > idle`. Each state
  has a distinct visual marker and animation; CSS-only (no Lottie).
- **Specialist-Bot templates**: one-click starters for Figma
  Specialist (Figma Bro), Code Reviewer (Devbot), Researcher
  (Detective), and Inbox Triage (Mailroom) — name + system prompt
  pre-filled, user can edit before saving.
- **`bots.state` column** on the SQLite `bots` table for the avatar
  state, plus `bot_set_state` Tauri command called by the bot
  executor at run start / run end / blocked.

### Added — Plumbing
- New SQLite tables: `computers` (per-Bot VM state, libvirt domain
  name, IP, VNC port, SSH key id, provisioning state) and
  `ssh_keys` (encrypted per-Bot keypairs).
- New Settings fields: `computer_server_host`, `computer_server_ssh_user`,
  `computer_server_ssh_key_id`, `computer_vnc_local_port_range`,
  `computer_passphrase`, `computer_default_disk_gb` (10),
  `computer_default_ram_mb` (2048).
- Tauri commands: `computer_get`, `computer_provision`,
  `computer_start`, `computer_stop`, `computer_destroy`,
  `computer_console_url`, `computer_test_connection`,
  `computer_file_list`, `computer_file_read`, `computer_file_write`,
  `bot_set_state`.
- `shell_run`, `file_read`, `file_write` tools now route through
  SSH into the Bot's VM when the Bot has a provisioned computer;
  local AppleScript path remains the fallback.

### Dependencies added
- `@novnc/novnc` (vendored) for the WebSocket RFB viewer.

### Tests
- 126 Rust tests pass (was 88 at v1.0.0). New: cloud-init user-data
  generation, Argon2id round-trip, keypair generation, tool routing
  (local vs SSH), VNC port allocator + wraparound, localhost-URL
  guard for `computer_console_url` (security test), SshKeyRow
  round-trip, Settings fields round-trip, shell_run tool routing
  logic, parse_sftp_ls parsing.
- 18 vitest tests pass (was 5). New: ComputerPanel status/preview
  rendering, specialist template chips, "Provision a computer"
  checkbox + disk/RAM inputs, BotAvatar state machine priority,
  BotAvatar blocked state visual, BotRoster filter by search,
  BotRoster row click → `onSelectBot`, BotRoster empty state, BotRoster
  new-Bot button.

## v1.0.0 — 2026-09-08
### Added
- First-run onboarding wizard (lands via the onboarding slice — see
  the Settings → General → Reset onboarding placeholder)
- Settings organized into 5 tabs (General / Providers / TTS / Browser
  / Grok) with sticky tab bar, per-tab independent scroll, `←/→`
  arrow-key nav when the tab bar is focused, and `⌘1`…`⌘5` jump
  shortcuts
- Multi-provider LLM client: MiniMax (default), OpenAI, Anthropic,
  xAI — per-provider API key + base URL override, one-line "find it
  here" hint for each
- Friendly error UI for chat failures (separate slice; see the
  ErrorMessage component)
- Sub-agent (bot) filesystem + memory — each bot has its own folder
  under `~/Library/Application Support/com.maxbot.app/bots/<id>/`
- ego-browser integration — replaces the older AppleScript browser
  path; runs JavaScript in ego (lite)'s embedded Node.js runtime
  with per-call consent
- Grok Build session integration — `grok agent stdio` JSON-RPC with
  persisted session id, configurable binary / model alias / cwd
- TTS via macOS `say` — voice picker in Settings, per-message 🔊
  button, `⌘⇧S` speak-last shortcut
- AppleScript Computer Use — browser, windows, mail, calendar,
  reminders, notes, system, Things 3, app launcher, with per-app
  TCC consent surfaced in Settings
- MCP client (stdio JSON-RPC) — load servers from
  `mcp_servers.json`, expose their tools to the model with the
  `mcp__<server>__<tool>` prefix
- Bot inbox for inter-agent messaging (`send_to_bot`) and a
  "send to bot" composer action
- Cron-expression bot schedules (replaces the pure-interval model)
- Regenerate button on the last assistant response
- Stop button for in-flight chat streams and bot runs
- Search across messages with a sidebar snippet view (`⌘F`)
- Copy button on each message
- `.env.example` documenting every env var the Rust env-loader
  reads; `.env` is seeded into SQLite on first launch but only when
  the corresponding field is empty
- User guide at `docs/user-guide.md`

### Changed
- Settings modal is now a 5-tab layout — the v0.7.x one-scroll
  surface is gone. Existing fields are preserved; new tabs surface
  the previously hidden Grok config + add Browser overrides
- Computer Use permissions panel lives in Settings → General instead
  of the middle of the form
- MCP server list lives in its own section on the Providers tab
  (accessible via the Tabs jump) — the v0.7.x placement below the
  TTS voice field is removed
- Provider pre-stash UI ("Configure other providers") is replaced
  by the dedicated Providers tab, so each provider gets a real
  description + placeholders + base URL field instead of a
  collapsed dropdown

### Fixed
- Empty SSE chunk no longer surfaces as a protocol error (the
  stream handler treats an empty `data:` line as a heartbeat and
  keeps the connection open)
- Stream stall recovery — a stalled run is detected after ~30s and
  the UI surfaces a "connection stalled — retry" affordance
- `<<think>>` reasoning tags are stripped from rendered message text
  before the model response hits the chat scroll
- Settings modal no longer scrolls on short windows (v0.6.7a's
  stacked header/body/footer pattern is preserved; the new tabs
  add a sticky tab bar without regressing the fix)

## v0.7.5 — 2026-09-08
- Motion + micro-interactions (hover lift on tool-call cards,
  animated streaming cursor, sidebar status pulse)

## v0.7.4 — 2026-09-08
- Density + rhythm pass + sidebar no-scroll (the conversation list
  fits without internal scrolling; `⌘F` search finds older threads)

## v0.7.3 — 2026-09-08
- Tool category color accents — the tool-call card's left-edge
  strip is colored by category (browser / code / fs / default) so a
  multi-call agent turn is scannable at a glance

## v0.7.2 — 2026-09-08
- Typography + monogram signature — Avenir Next for display
  surfaces, JetBrains Mono for code, and the "M" with the pulsing
  accent dot in the empty state

## v0.7.1 — 2026-09-08
- Polish pass — tool-call cards, typing dots, empty state

## v0.7.0 — 2026-09-08
- Grok Build CLI session integration (`grok agent stdio` JSON-RPC)
  with persisted ACP session id so a relaunch resumes the same
  conversation

## v0.6.9 — 2026-09-08
- ego-browser integration — replaces the AppleScript browser path
  with a real Chromium driven via the ego (lite) embedded Node.js
  runtime; per-call consent

## v0.6.8 — 2026-09-08
- Bots as first-class citizens — per-bot filesystem + memory
  (`bots/<id>/` under the app data dir), inter-agent inbox, bot
  editor in a modal, run-now from the sidebar

## v0.6.7a — 2026-09-08
- Fix Settings modal scroll on short windows — the stacked
  header/body/footer pattern (flex column + `min-height: 0` +
  `overflow-y: auto` on the body) so the form scrolls inside the
  modal even when the viewport is short

## v0.6.7 — 2026-09-08
- `app_open` / `app_list` tools (replaces the v0.6.6 Things 3
  integrations; the AppleScript Computer Use surface now opens
  arbitrary macOS apps by name or bundle id)

## v0.6.6 — 2026-09-08
- Things 3 tools — `today`, `inbox`, `upcoming`, `projects`,
  `add`, `complete` (later replaced by the v0.6.7 generic
  `app_open` / `app_list` pair)

## v0.6.5 — 2026-09-08
- Stream stall recovery + strip orphaned `<think>` tags from
  partial responses

## v0.6.4 — 2026-09-08
- Strip `<think>` reasoning tags from rendered message text
  before the response hits the chat scroll

## v0.6.3 — 2026-09-08
- Settings modal scrolls on short windows (initial pass; the
  v0.6.7a refactor is the final fix)

## v0.6.2 — 2026-09-08
- Voice discoverability — 🔊 in the composer + `⌘⇧S` + always-
  visible TTS button on each assistant message

## v0.6.1 — 2026-09-08
- Text-to-speech via macOS `say` — 🔊 button on assistant
  messages, configurable voice in Settings

## v0.6.0 — 2026-09-08
- Multi-provider LLM support — MiniMax (default) / OpenAI /
  Anthropic / xAI with a provider dropdown in Settings and a
  per-provider API key field

## v0.5.0 — 2026-09-08
- Stop button for in-flight bot runs — surfaces from the bot
  row in the sidebar and tears down the agent's streaming context

## v0.4.8 — 2026-09-08
- Regenerate button on the last assistant response

## v0.4.7 — 2026-09-08
- Search across messages (`⌘F`) + copy button on each message

## v0.4.6 — 2026-09-08
- MCP client (stdio) — model can call any local MCP server; tools
  surfaced with the `mcp__<server>__<tool>` prefix and per-call
  consent

## v0.4.5 — 2026-09-08
- Send-to-bot from chat composer — drop a message into a bot's
  inbox without leaving the main chat

## v0.4.4 — 2026-09-08
- Cron expressions for bot schedules (replaces the pure-interval
  model from v0.3)

## v0.4.3 — 2026-09-08
- Browsers + windows — closes the full AppleScript library
  (browsers, windows, mail, calendar, reminders, notes, system)

## v0.4.2 — 2026-09-08
- Calendar + reminders + notes — 9 tools via AppleScript

## v0.4.1 — 2026-09-08
- Mail — inbox, search, send, draft via AppleScript

## v0.4.0 — 2026-09-08
- AppleScript Computer Use foundation + clipboard + system +
  TCC UI (the per-app permission panel that lives in Settings →
  General today)

## v0.3.1 — 2026-09-08
- Load `MINIMAX_API_KEY` (+ base_url, default_model) from `.env`
  on first launch — the env loader that `src-tauri/src/lib.rs`
  uses to seed the SQLite settings row

## v0.3 — 2026-09-08
- Bots (sub-agents) — sidebar panel, editor, scheduler, run-now,
  inter-agent inbox

## v0.2 — 2026-09-08
- Tools slice: `web_fetch`, `web_search`, `file_read`, `file_write`,
  `shell_run`

## v0.1 — 2026-09-08
- Initial MaxBot — Tauri 2 + MiniMax + chat history (single
  provider, single conversation stream, local SQLite persistence)
