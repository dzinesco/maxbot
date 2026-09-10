# Changelog

All notable changes to MaxBot are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/) and the project
adheres to [Semantic Versioning](https://semver.org/).

## v3.3.0 — 2026-09-09

### Added
- **Re-record button on each skill row.** Opens
  `RecordSkillDialog` preloaded with the skill's existing
  JSON so the user can edit and re-save in place. The save
  path uses the new `skill_update` Tauri command, which
  preserves the skill's `id`, `created_at`, and any
  `bot_schedules.skill_id` reference (the bot's schedule
  continues to fire on the same skill — only the
  replayed content changes).
- **Last run expandable view per skill row.** Each skill
  has a `▸ Last run` toggle. Expanding it calls the new
  `skill_run_last_trace` Tauri command, which returns the
  most recent row from the new `skill_run_traces` table.
  The view shows: timestamp, duration in seconds,
  success/failure status, the run's id, the trigger input
  (the user message or scheduled payload that started
  the run), and the per-step tool calls with their args
  and result text.
- **`skill_run_traces` table + `SkillRunTrace` / `StepTrace`
  types.** New durable table — one row per run, with the
  per-step output (tool name, args, result, timestamp,
  role) as a JSON-encoded array. Captured best-effort by
  the skill executor on every run (both manual and
  scheduled); a trace-write failure does NOT fail the run.
- **`skill_update` Tauri command.** `(id, skill: Skill) ->
  Skill`. Looks up the existing row, preserves `id` and
  `created_at`, overwrites `name` / `description` /
  `inputs` / `steps`, and bumps `updated_at`. Used by the
  Re-record flow.
- **`skill_run_last_trace` Tauri command.** `(skill_id) ->
  Option<SkillRunTrace>`. Returns the most recent trace
  for a skill, or `None` if the skill has never been run.
- **Seeded `maxbot-daily-checkin` demo skill.** A
  single-step `web_fetch` placeholder that ships on
  first launch so the new Re-record and Last-run
  affordances have something to point at out of the
  box. Idempotent: if any skill already exists, the
  seed is skipped. Users can re-record it to make it
  their own.
- **"View run" link on Routines.** Routines bound to a
  Skill (`schedule.skill_id != null`) gain a
  `View run` button. The link navigates the parent to
  the Skills panel; the Last-run expand lives there
  and is fetched on demand. Read-only on the Routines
  surface — no inline expand.

### Docs
- **"First morning you should see a run" section** in
  `docs/user-guide.md`. Walks through: record a skill,
  schedule for 8am, verify the run lands in Activity +
  the Last run view the next morning. Includes the
  Phase 8 follow-up note pointing at the
  `axis-api` skill at `~/.minimax/skills/axis-api` as
  the canary source for the real TOC skill.

## v3.2.0 — 2026-09-09

### Added
- **`vm_computer_use` tool.** A single LLM-callable tool with a
  `script` parameter — newline-separated helper calls
  (`screenshot()`, `click_at(x, y)`, `type(text)`, `key(name)`,
  `open_url(url)`). Each helper SSHes to the Bot's per-Bot Linux
  VM and runs the right `xdotool` / `chromium-browser` / `scrot`
  command. The post-action screenshot (base64 PNG) is the tool's
  result, mirroring `ego_browser`'s shape. Requires consent
  because the script can navigate, click, and fill forms in the
  Bot's own browser.
- **`vm_browser_open` skill primitive.** A thin wrapper around
  `vm_computer_use` for the "navigate to a URL and snapshot"
  case. The recorder auto-rewrites a recorded `vm_computer_use`
  step whose script is exactly one `open_url(...)` call into a
  `vm_browser_open(url)` step — cleaner skill specs, same end
  state on replay (the replay path calls the wrapper, which
  dispatches back to `vm_computer_use` with the same
  `open_url` script).
- **Per-Bot `computer_use` setting.** New Bots default to
  `"vm"`. The Bot editor exposes a `Computer Use` dropdown
  with three options: VM (default), Mac, Mac with approval.
  The setting is read by the tool registry when constructing
  the per-Bot tool list — `vm_computer_use` and
  `vm_browser_open` are in the list when `computer_use ==
  "vm"`; `ego_browser` is in the list when `computer_use ==
  "mac"`. The `mac-with-approval` value is reserved for the
  v3.4.0 Takeover work; the enum lives in the schema today so a
  future Bot doesn't need a migration.
- **`chromium-browser`, `xdotool`, `scrot` in cloud-init.** The
  new packages join the `packages:` block in
  `src-tauri/scripts/provision-vm.sh` (NOT `provision.rs` —
  the cloud-init YAML lives in the shell script, not the
  Rust file). cloud-init's `package_install` module handles
  them as a single apt transaction; the install is idempotent
  on a fresh image.
- **3+ new vitest cases** for the Bot editor's new `Computer
  Use` dropdown (default value, all three options, save flow
  propagates the choice). Plus Rust tests for
  `parse_computer_use` (3 known values + unknown-value
  fallback) and the recorder's `vm_browser_open` rewrite
  (single `open_url`, multi-call scripts, non-vm_computer_use
  steps, end-to-end `recorder.record()`).

### Changed
- `tools/registry::ToolRegistry::computer_use_filtered` is the
  new per-Bot tool filter; it composes on top of the
  allowlist (`allowed_tools`) and swaps the Computer Use
  tools in or out per `bot.computer_use`. Bots with
  `allowed_tools: []` see no Computer Use tools regardless
  of the `computer_use` value.
- The `Bot` struct gains a `computer_use: String` field with
  a `#[serde(default = "default_computer_use")]` fallback to
  `"vm"`. The `bots` SQLite table gains a `computer_use`
  column with `DEFAULT 'vm'`, added via
  `add_column_if_missing` so existing rows backfill cleanly.

### Notes
- **`bin/maxbotd.rs` had two test-fixture `Bot { ... }`
  constructors updated** to add the new
  `computer_use: "vm".to_string()` field. The brief asked
  for "no daemon changes," but adding a struct field
  requires every constructor to set the new field or
  compilation fails. The change is 2 lines, test-only
  (no daemon behavior), and required to make the slice
  compile. Documented as the only deviation.
- **`src-tauri/src/groups/mod.rs` had one test-fixture
  `Bot { ... }` constructor updated** for the same
  reason. Also test-only, 1 line.
- **The cloud-init `packages:` block is in
  `src-tauri/scripts/provision-vm.sh`, NOT
  `src-tauri/src/computer/provision.rs`** — the brief
  asked the implementer to find it. The new packages
  are added to the existing YAML block in
  `provision-vm.sh`.
- **Existing VMs do NOT pick up the new packages
  automatically.** cloud-init only runs once per VM (at
  first boot). Re-provision existing VMs to get the
  v3.2.0 toolchain. The `docs/server-setup.md` note
  explicitly says so.

## v3.1.0 — 2026-09-09

### Added
- **`docs/maxbotd-setup.md` — the install + verify runbook for the
  always-on daemon.** Covers: what `maxbotd` is and the two paths
  it serves (30 s scheduler poll + `POST /hooks/<bot_id>`
  webhook), the one-time install on `crispy` (build on the Mac,
  copy the binary, drop the systemd unit at
  `/etc/systemd/system/maxbotd.service`, create the unprivileged
  `maxbot` user, mount the shared SQLite file), a `tmux` /
  `screen` fallback for ad-hoc poking, the `curl /health`
  liveness probe, the "Connect a Bot" walkthrough
  (Generate token → curl the webhook → see the 202), the
  "Run a test" flow (Test-webhook button in the BotEditor),
  the headline **"close the Mac app, work continues"** verification
  flow (set a 1-minute routine, `Cmd+Q`, reopen, see the
  `via daemon` badge), and a troubleshooting table covering
  port collision, auth failure, missing `--db`, QGA not ready,
  and the Tauri-webview CORS error from the Test-webhook button.
- **`maxbotd` `GET /health` route** (liveness probe, no auth).
  Returns `{ "ok": true, "version": <CARGO_PKG_VERSION> }` so
  `curl` (or systemd, or the docs) can confirm the daemon is up
  and the binary matches expectations.
- **`maxbotd` `GET /bots/<id>/recent_runs?limit=N` route** (bearer
  auth). Returns the most-recent `bot_runs` rows for the given
  Bot, default 20, capped at 200. Used by the Test-webhook
  verification flow and the `curl` examples in
  `docs/maxbotd-setup.md`.
- **`Test webhook` button in the BotEditor Daemon tab.** Sits
  next to Generate / Rotate / Copy. POSTs a small
  `{ "text": "webhook test from MaxBot at <iso8601>" }` payload
  to the configured webhook URL with the current bearer token.
  Shows the response inline (green for 202, red for anything
  else, with the response body or the webview CORS error).
  Disabled until both the token and `computer_server_host` are
  configured.
- **`triggered_by` column on `bot_runs`.** New `TEXT NOT NULL
  DEFAULT 'app'` column with three values: `"app"` (in-app "Run
  now" / `send_to_bot` / approval auto-resume), `"daemon"`
  (`maxbotd` scheduler poll), `"webhook"` (`maxbotd` `POST
  /hooks/<bot_id>`). Added via `add_column_if_missing` in
  `Database::migrate()` so existing rows backfill to `"app"`
  with the column's `DEFAULT`. Read everywhere `BotRun` is
  read (get / list / list_recent).
- **`BotRunTriggeredBy` enum on the Rust side**, with
  `as_str()` + `parse()` helpers. `parse()` falls back to
  `App` for any unknown value so a future enum addition
  can't crash a row read.
- **ActivityFeed row badge** — each Bot row in the sidebar's
  Activity panel now shows a small pill: `via app` (subtle
  default), `via daemon` (Electric Blue), `via webhook`
  (cyan-leaning Blue). The two off-app variants are visually
  distinct from the in-app default so the user can tell
  "the Mac app fired this" apart from "the daemon fired
  this while the Mac was closed" at a glance.

### Changed
- **`run_bot_once(...)` gains a 7th parameter**: `triggered_by:
  Option<&'static str>`. Defaults to `"app"` for any caller
  that still passes `None` (preserves pre-v3.1.0 behavior for
  the 6 existing call sites: `run_with_timeout`, in-app
  scheduler, `run_bot_now`, `send_to_bot`, `skill_record_start`,
  `decide_approval` auto-resume). New daemon call sites pass
  `Some("daemon")` (scheduler tick) or `Some("webhook")` (POST
  handler). The 6-arg shape was a load-bearing contract for
  the daemon (which uses `None` for `AppHandle`); the 7th
  parameter is additive and doesn't break the type guard.

## v3.0.7 — 2026-09-09

### Fixed
- **noVNC streaming — viewer paints the VM framebuffer, no silent
  black canvas.** The RFB `connect` event fires on local WebSocket
  open, which can succeed even when the SSH-tunneled VNC stream
  behind it is dead — the canvas stayed black with no overlay and
  no error. The viewer now waits for the first framebuffer event
  (`desktopname` / `resize` / pixel change) before dropping the
  "Connecting to VM…" overlay, surfaces a 6s no-frame timeout as
  a red centered error, renders the canvas with `flex: 1;
  min-height: 0;` so it actually fills nested flex parents,
  passes `shared: true` to the RFB constructor, wires a
  `ResizeObserver` to re-scale on container size changes, and
  supports an optional VNC `credentials` prop. Consolidates the
  mount + wsUrl effects into a single `useEffect` keyed on
  `wsUrl` so React 19 Strict Mode can't race a stale mount with
  a fresh URL.

## v3.0.6 — 2026-09-09

### Docs
- **`docs/v3-grand-tour.md` — per-panel walkthrough with
  screenshot placeholders.** A 13-section tour that covers
  every surface in MaxBot v3: the sidebar (Bot roster +
  version pill + ActivityFeed), the Bot Editor's 8 sections
  (Identity, Brain, Workspace, Computer, Capabilities, Rules,
  Schedule, Daemon), the chat panel, the Computer panel
  (Status / Preview / Takeover + Files), the Skills panel, the
  Memory panel, the Approval Queue, the Routines panel, the
  Activity Feed, the Settings drawer and the <kbd>⌘</kbd>+<kbd>K</kbd>
  command palette, the "What's new in v3" overlay, and closing
  notes on the v3 polish theme. Five real screenshots landed
  (Overview, Chat, Computer Console in loading state, the
  Memory panel, and the <kbd>⌘</kbd>+<kbd>K</kbd> command
  palette open over the Memory panel) — the rest are
  placeholders pending a hand-capture pass by Tyler (the
  worker shell context does not have macOS Screen Recording
  permission, so `screencapture` fails with "cannot write
  file to intended destination"). The expected filenames are
  enumerated in `docs/v3-grand-tour/screenshots/.gitkeep`.
  No source code changes; the `/Applications/MaxBot.app`
  binary stays at v3.0.5.

## v3.0.5 — 2026-09-09

### Fixed
- **Console auto-recover waits for QEMU guest agent
  readiness.** The v3.0.4 e2e test
  `console_e2e_against_crispy` failed against a freshly
  provisioned VM with
  `Guest agent is not responding: QEMU guest agent is not connected`:
  when `console_url` is called immediately after
  `provision_vm` returns, the VM is `running` with a
  DHCP lease, but the QGA socket hasn't fully connected
  yet. v3.0.5 adds the fix in two parts:

  1. **Provision waits for QGA** —
     `src-tauri/src/computer/provision.rs` now calls a
     new `wait_for_qga_ready` helper in
     `src-tauri/src/computer/mod.rs` at the end of
     `provision_vm`, right after the VNC-port poll. The
     helper polls `virsh qemu-agent-command <name>
     '{"execute":"guest-ping"}'` every 3 seconds up to
     5 minutes. This is the actual fix for the race
     the v3.0.4 e2e caught: `provision_vm` only waited
     for `running` + DHCP, while cloud-init's
     `packages:` block (xfce4, x11vnc,
     qemu-guest-agent, openssh-server) + `runcmd:`
     `systemctl enable --now qemu-guest-agent` finish
     well after that. On a warm cloud-image cache the
     wait is typically 30-90s; on a cold cache
     (first provision) it can be several minutes.
     The 5-minute timeout matches the typical
     worst-case cloud-init packages+runcmd time on
     the Ubuntu 24.04 noble cloud image used by
     `provision-vm.sh`; if it ever trips, the VM's
     cloud-init is genuinely broken, and the new
     `ComputerError::QgaTimeout` surfaces that to the
     UI with the last stderr from virsh.

  2. **`qemu_agent_command` retries the brief
     socket-not-ready window** —
     `src-tauri/src/computer/libvirt.rs` adds a small
     retry loop (5 attempts, 500ms backoff, ~2s worst
     case) inside `qemu_agent_command` that retries
     only on the specific `Guest agent is not
     responding` / `QEMU guest agent is not connected`
     stderr pattern from virsh. All other QGA errors
     — command not found, permission denied, parse
     failures, ssh errors — are surfaced immediately;
     the retry does not paper over real failures.
     Because the retry lives at the
     `qemu_agent_command` layer, every QGA caller
     benefits (provision, console auto-recover, and
     any future ones), not just
     `install_default_key_via_qga`. New unit tests
     pin the classifier and the retry bounds (3-5
     attempts, 200-500ms backoff) so a future libvirt
     version changing the stderr wording is caught by
     the test suite rather than a flaky e2e run.

### Changed
- **Bumped to 3.0.5** in `package.json`,
  `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## v3.0.4 — 2026-09-09

### Added
- **Console end-to-end integration test against
  `crispy`.** A new `#[ignore]`-gated Rust integration
  test, `console_e2e_against_crispy`, in
  `src-tauri/src/computer/mod.rs` (sibling to
  `provision_e2e_against_crispy`). The test provisions a
  fresh per-Bot VM on Tyler's libvirt host
  (192.168.0.49), waits for `running` + DHCP, then calls
  `console_url(bot_id)` to exercise the v3.0.3 auto-
  recover path end-to-end: it asserts the returned
  WebSocket URL is a `ws://localhost:<port>/` and TCP-
  probes the port to prove the proxy is actually
  listening. This is the load-bearing CI gate that proves
  the v3.0.3 "Console just works on first click" fix
  holds against a real VM, not just in unit-test
  isolation. Marked `#[ignore]` so `cargo test --lib`
  stays fast; run on demand with:
  ```
  cargo test --lib console_e2e_against_crispy -- --ignored --nocapture
  ```
  No production code, no frontend, no daemon changes —
  pure test infrastructure.

### Changed
- **Bumped to 3.0.4** in `package.json`,
  `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## v3.0.3 — 2026-09-09

### Fixed
- **Console auto-installs the user's default SSH key on
  tunnel auth failure.** Clicking Console on a fresh VM
  no longer fails on the first try with
  `vnc: vnc tunnel auth failed: (no stderr output captured)`
  and asks the user to click "Use my default key" in the
  toolbar — `console_url` now detects the `TunnelAuthFailed`
  from `vnc::start`, runs the same QGA-based
  `install_default_key_via_qga` recovery that the
  toolbar button triggers, and retries the tunnel
  automatically. The user never sees the auth error;
  Console just works. The "Use my default key" button
  is preserved as a manual override for users with
  non-default keys or VMs that need a refresh. v3.0.3
  extracts the QGA install logic from
  `install_default_key` (steps 1–3, lines ~543–610 of
  `src-tauri/src/computer/mod.rs`) into a private
  `pub(crate) async fn install_default_key_via_qga`
  helper; the public Tauri command is now a thin
  wrapper around the helper, with identical signature
  and error behavior. The auto-recover logic is
  scoped to `TunnelAuthFailed` only; any other
  `VncError` is propagated unchanged. The auto-recover
  is invisible to the user — no UI changes, no toast,
  no log spam (the only log line is
  `vnc tunnel auth failed, attempting default key
  install for {bot_id}: {stderr}` at `info` level).
  If the QGA install also fails, the user gets a clear
  error that names both failure modes: `tunnel auth
  failed and default key install failed: ... —
  check that ~/.ssh/id_ed25519.pub (or id_rsa.pub /
  id_ecdsa.pub) exists`. If the retry tunnel still
  fails with `TunnelAuthFailed`, the error is:
  `tunnel auth failed even after default key install:
  ... — verify the VM has accepted the new key (try
  'ssh bot@<vm-ip>' from your shell)`.

### Changed
- **Bumped to 3.0.3** in `package.json`,
  `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## v3.0.2 — 2026-09-09

### Added
- **Global settings command palette (`Cmd+K` / `Ctrl+K`).** A
  keyboard-first way to find any setting without clicking
  through tabs. Press `Cmd+K` (mac) or `Ctrl+K` (other) to
  open; type to filter; `↑` / `↓` to navigate; `Enter` to
  select; `Esc` to close. Selecting a result opens the
  relevant panel (Settings, Bot editor, Computer, Skills,
  Memory, Routines) and focuses the field. The palette
  does not appear in the home / welcome view (per the
  brief's "only show results for the current context"
  rule) and shows only App-level rows when no Bot is
  selected.
- **`SettingsPalette` component + test.** A presentational
  modal in `src/components/SettingsPalette.tsx` that
  builds its index dynamically from the `bots` prop. The
  search is case-insensitive substring over the entry's
  `label`, `context` (Bot name or "App"), and the
  `data-setting-key` value. The vitest suite covers
  open-via-render, filter-as-you-type, Enter-select,
  ArrowDown-navigation, Esc-close, backdrop-click-close,
  palette-body-does-NOT-close, home-view-gates-Bot-rows,
  and click-to-select.
- **`data-setting-key` annotations.** Every Bot-level
  setting (Identity, Brain, Workspace, Computer, Capabilities,
  Rules, Schedule, Daemon, Skills, Memory, Routines) and
  every App-level setting (General, Providers, TTS, Browser,
  Computer, Grok tabs) now carries a `data-setting-key`
  attribute. The attribute is the bridge between the
  palette's `entry.key` and the DOM element to focus.
  No existing field behavior, label, or value was
  changed — the attribute is purely additive.
- **`Settings.initialTab` prop + `key`-driven remount.** The
  Settings modal accepts an `initialTab` so the palette
  can jump straight to the TTS / Browser / Computer / Grok
  tab. The App bumps the modal's `key` to remount it on
  tab change, so the new tab is honored without leaking
  state between visits.

### Changed
- **Bumped to 3.0.2** in `package.json`,
  `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## v3.0.1 — 2026-09-09

### Added
- **"What's new in v3" first-launch overlay.** On the
  first launch after v3.0.1, a modal summarizes the v3.0
  design polish in 5 bullets (palette, font, tactile
  feedback, skeleton shimmer, ActivityFeed stale-data
  UX) so the user doesn't miss what changed. Single
  "Got it" button dismisses; pressing `Esc` or clicking
  the backdrop also dismisses. The seen state is
  persisted in the `meta` table under the
  `seen_v3_intro` key, so the overlay never reappears
  on subsequent launches. The key is read in parallel
  with `is_onboarded` in the App.tsx bootstrap; the
  overlay only mounts when the user is onboarded AND
  the flag is unset (a mid-onboarding user is on the
  Welcome surface, not the chat, so the overlay would
  be confusing there).
- **`WhatsNewOverlay` component + test.** A
  presentational modal in `src/components/WhatsNewOverlay.tsx`
  with a dedicated `data-testid` for each of the 5
  bullets, the Got-it button, and the dismiss paths.
  The vitest suite covers: (a) all 5 bullets render
  in order, (b) clicking Got-it calls the dismiss
  handler, (c) pressing Esc calls the dismiss handler,
  (d) non-Escape keys do NOT dismiss, (e) backdrop
  click dismisses, (f) clicking inside the modal does
  NOT dismiss (stopPropagation).

### Changed
- **Bumped to 3.0.1** in `package.json`,
  `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## v3.0.0 — 2026-09-09

v3.0.0 is the integration release — no new features, just a
focused pass over the visual + interaction layer. The goal
was to make v2.x feel like a finished product, not a stack
of features.

### Changed
- **Brand palette: violet → Electric Blue.** The brand
  monogram (`.brand-mark`, `.main-empty-mark`, `.welcome-mark`),
  the assistant avatar in the chat, the assistant message
  wash, and `--accent` / `--accent-hover` / `--accent-soft`
  all moved off the violet/lila `#7c3aed` family onto
  `#2563eb → #1d4ed8 → #1e3a8a`. Single accent, saturation
  under 80%, reads as "trust" on both dark and light UI.
  Box-shadow tints were retuned to match.
- **Body font: Inter → system sans.** `--font-sans` now
  resolves to `Public Sans → Outfit → SF Pro Text →
  -apple-system → system-ui`. No webfont dep, no network
  fetch — the user already has these on macOS by default.
  Rationale documented inline in `:root` in `src/styles.css`.
- **Viewport units: 100vh → 100dvh.** The `.app` shell,
  `.app-boot` scaffold, and `.welcome` surface all use
  `100dvh` so the iOS-style URL-bar collapse doesn't
  shift the layout.
- **Tactile feedback on `:active` for every button.**
  A global `:where(button):active:not(:disabled) { transform:
  translateY(1px); }` rule with an 80ms ease-out settle.
  Specificity 0 so existing card-lift and scale rules still
  win on the elements that opt in to those effects.

### Added
- **Skeletal loaders (`.skeleton` / `.skeleton--2` /
  `.skeleton--3`).** A shimmer keyframe + 1/2/3-line
  placeholders for "loading chat history" / "loading
  bots" / "loading skills" surfaces. Honors
  `prefers-reduced-motion`.
- **Bot-avatar hover tooltip.** The `BotAvatar` already
  exposed a `data-bot-state` attribute and a `title=`
  fallback. v3.0.0 adds a styled hover pill that fades
  in below the avatar, tinted by state (green = done,
  red = blocked, blue = working/thinking). The browser
  `title=` is still there for a11y / keyboard users.
- **ActivityFeed got CSS.** v2.8.0 shipped the
  `ActivityFeed` component but never styled it (it was
  relying on parent styles leaking in). v3.0.0 adds a
  Cockpit-mode surface: sectioned lists, hairline row
  dividers, monospaced timestamps, status-color pills.
  See the `--v3.0.0 polish` block at the end of
  `src/styles.css`.
- **Non-blocking "couldn't refresh" pill.** When a
  poll fails AFTER the feed has data, the feed now
  shows the last good data plus a small inline
  "couldn't refresh — showing last good data" pill
  instead of replacing everything with an error. The
  first-ever failure (no data yet) still shows a
  full-feed error so the user knows something is wrong.

### Fixed
- **`100vh` on iOS-style URL-bar collapse** would shift
  the layout on every URL-bar hide/show. Replaced with
  `100dvh` on the three surfaces that need to fill the
  viewport.
- **ActivityFeed was unstyled in v2.8.0** (all those
  `activity-feed__*` class names had no rules in
  `styles.css`). The new `--v3.0.0 polish` block fixes
  the visual and brings it in line with the BotRoster
  row style.

### Notes
- The previous tests (90 vitest / 173 cargo) still pass.
  v3.0.0 is CSS + a single component-render branch; no
  test count change.
- The pre-existing uncommitted edit to
  `src-tauri/src/bin/maxbotd.rs` (a `Bot` struct field
  alignment) is included in the v3.0.0 commit since
  `git add -A` picks it up; it does not affect the test
  count.
- Added `default-run = "maxbot"` to `[package]` in
  `src-tauri/Cargo.toml`. Without it, Tauri 2's bundler
  errors with "failed to find main binary" because the
  workspace has two binaries (`maxbot` from `src/main.rs`
  and `maxbotd` from `src/bin/maxbotd.rs`) and Tauri
  can't infer which one is the main app. This is what
  caused the first v3.0.0 build to silently keep the
  pre-existing v2.6.3 `Info.plist` in
  `MaxBot.app/Contents/Info.plist`; the fix forces a
  fresh re-bundle so the installed app actually
  advertises `3.0.0` to macOS.

## v2.8.0 — 2026-09-09

### Added
- **Always-on Daemon (`maxbotd`).** A new second
  binary in the same source tree (`src-tauri/src/bin/
  maxbotd.rs`) that runs headless on the user's Linux
  server (`crispy`). It shares the same SQLite file
  and the same `maxbot_lib` code as the Tauri app —
  same `Database`, same `run_bot_once`, same
  scheduler tick logic. Two responsibilities:
  - **30s scheduler poll.** Walks `bot_schedules`
    every 30s, fires `run_bot_once` for any due
    schedule. Reuses the in-app scheduler verbatim
    (v2.8.0 refactored `run_bot_once` to accept
    `Option<AppHandle>`; the daemon passes `None` and
    skips the Tauri `bot://chunk` / `bot://done` /
    `bot://error` event emits).
  - **Webhook server.** `POST /hooks/<bot_id>` on
    `0.0.0.0:8443` by default. Auth: `Authorization:
    Bearer <token>` against a per-Bot token stored in
    the new `daemon_tokens` SQLite table. On 202, the
    daemon creates a fresh conversation, appends the
    webhook body as a synthetic user message, and
    fires `run_bot_once` asynchronously. Returns
    `{ "bot_run_id": "...", "status": "accepted" }`.
- **ActivityFeed.** New Sidebar section showing the
  last 5 Bot runs, last 5 Skill runs, and last 5
  Approvals across **all** Bots. Polls
  `list_recent_activity` every 10s. Empty state: "No
  activity yet — the daemon will populate this when
  it fires." The Mac app sees whatever the daemon
  wrote on next poll, so the feed is effectively
  real-time as long as the app is open.
- **BotEditor Daemon tab.** Shows the per-Bot bearer
  token (or "(not set)"), a Rotate button, a Copy
  button (uses `navigator.clipboard.writeText`), and
  the public webhook URL
  (`https://<computer_server_host>:8443/hooks/<bot_id>`).
  New `get_daemon_token` / `rotate_daemon_token`
  Tauri commands. Token rotation invalidates the old
  token immediately; inbound webhooks using the old
  token start returning 401.
- **Database additions.** New `daemon_tokens` table
  (bot_id PK, token, created_at, last_used). New
  `list_recent_bot_runs`, `list_recent_skill_runs`,
  and `list_recent_approvals` queries (each takes a
  small `LIMIT` and returns rows across all owners
  for the ActivityFeed).

### Fixed
- **Run bot from a separate process.** `run_bot_once`
  and `run_with_timeout` now accept
  `Option<AppHandle>`. Existing Tauri-app call sites
  pass `Some(app)` and behave exactly as before; the
  daemon passes `None` and the executor skips event
  emission. The persisted `bot_runs` row is the
  source of truth either way.

### Changed
- **Executor's `AppHandle` parameter is now optional.**
  The 4 `app.emit` calls (`bot://chunk` text delta,
  `bot://chunk` tool-call delta, `bot://done`,
  `bot://error`) and the `agents.md` filesystem read
  are all guarded on `app.is_some()`. Daemon runs
  skip these and rely on the SQLite row.
- **Added `axum = "0.7"`** to `src-tauri/Cargo.toml`
  for the daemon's HTTP server. No other new
  top-level deps.

### Caveats / Deferred
- **TLS is v3.0.** v2.8 listens on plain HTTP. The
  bearer token is the only auth. Reverse-proxy with
  TLS at the edge (Caddy / nginx) or wait for v3.0.
- **Mac notifications on daemon-triggered events are
  v2.8.1.** The daemon is a separate process; an
  outbound IPC back to the MaxBot app for native
  notifications needs more design. The ActivityFeed
  is the v2.8.0 substitute.
- **VM-side `bots/<id>/daemon.json` token storage is
  v3.0.** v2.8 reads tokens from the SQLite
  `daemon_tokens` table. The VM-side JSON is a
  v3.0 polish for shareable, copyable per-Bot
  credentials.
- **Daemon bot runs do not read `agents.md`.** The
  file lives on the Mac's app data dir, not the
  server. Daemon runs fall back to `bot.system_prompt`
  only. Syncing agents.md to the server is out of
  scope for v2.8.

## v2.6.2 — 2026-09-09

### Fixed
- **Approval auto-resume.** The v2.6.0 approval queue
  let the user click Approve / Reject / Edit & send,
  but the Bot did not auto-resume after a decision —
  the user had to send a follow-up message to keep
  the loop going (caveat #1 from the v2.6.0 report).
  `approval_decide` now appends a synthetic
  `role=tool` message to the bot's conversation
  with the tool's result (or a `{"error":"denied
  by user"}` payload on Reject) and re-triggers the
  Bot against the same conversation via
  `run_bot_once` with a new optional
  `existing_conversation_id` parameter. The
  existing `bot://chunk` / `bot://done` /
  `bot://error` event surface drives the UI — no
  React changes required. Approve and Reject now
  feel like a single continuous turn instead of
  two.

### Added
- **`approvals.tool_call_id` column.** The LLM's
  tool_call id is now persisted on the approval
  row at enqueue time, so the auto-resume path can
  thread it through the synthetic `role=tool`
  message and the LLM can match it back to the
  outstanding `tool_calls` block. Pre-v2.6.2 rows
  stay `NULL` and fall back to positional matching.
- **`Database::get_bot_run(id)`.** Single-row
  lookup helper used by the auto-resume path to
  recover the `conversation_id` from
  `approvals.bot_run_id`. Mirrors the existing
  `list_bot_runs` shape; the resume is best-effort
  if the run vanished (e.g. a DB re-init between
  enqueue and decide).
- **`run_bot_once` `existing_conversation_id`
  parameter.** When `Some`, the executor pins the
  run to that conversation (verifying it still
  exists) and skips the "Run tick" kickoff so the
  LLM picks up from the synthetic tool message
  instead of seeing a brand-new turn inserted on
  top. `None` preserves the pre-v2.6.2 behavior
  for `run_bot_now`, `send_to_bot`, and the
  scheduler.

### Changed
- **`enqueue_approval` and `enqueue_if_ask_rule`
  signatures** now take a `tool_call_id:
  Option<&str>`. `None` is acceptable for
  back-compat with callers that don't have the
  LLM id (the column allows NULL).
- **`Approval` struct** gains a
  `pub tool_call_id: Option<String>` field with
  `#[serde(default)]` so the existing React
  `Approval` type round-trips without a breaking
  change. The renderer surfaces the field in
  `src/lib/api.ts` as `tool_call_id: string |
  null`; the existing `ApprovalQueue` test mock
  was updated to set it.
- **`approval_decide` signature** now takes
  `app: AppHandle` as the first parameter (Tauri
  injects it automatically — the React call site
  is unchanged). The new `AppHandle` is used to
  re-emit the existing `bot://chunk` /
  `bot://done` / `bot://error` events from the
  resumed run.

## v2.4.0 — 2026-09-09

### Added
- **Multi-Bot groups.** A Group is a chat surface where
  2–6 Bots collaborate. Each Group has a name, an
  owner Bot, and a member set. Groups live in their
  own `group_chats` / `group_members` / `group_messages`
  tables — the existing direct-chat `conversations`
  table is untouched, so upgrading installs see no
  migration churn on their existing chats.
- **`@BotName` mention routing.** The Composer parses
  `@BotName` tokens (case-insensitive, whole-word) and
  dispatches the message to the mentioned Bots in the
  order they appear. Groups are 2–6 — no broadcast
  pattern, no implicit "send to all" mode. A user
  message with no mentions is accepted but shows a
  hint: "Mention a Bot with @BotName to send to one
  of the group members."
- **`<handoff to="BotName">` routing between Bots.** A
  Bot that wants another member to take the next turn
  emits `<handoff to="BotName">…</handoff>` in its
  final reply. The group executor strips the tag from
  the rendered text, persists a `role='handoff'` row
  in `group_messages` with the resolved target Bot id,
  and the front-end watches for those rows to kick off
  the next `run_group_turn` (with the handoff body as
  the new user message). Unknown handoff targets
  surface as `GroupError::UnknownHandoffTarget`.
- **Sidebar "Groups" section.** Lists every group the
  user owns above the Bot list. Clicking a group opens
  `GroupChatView` (a ChatView variant with a left-side
  participant rail and a distinct handoff card for
  `role='handoff'` rows).
- **`CreateGroupDialog`.** Pick a name, an owner Bot,
  and 1–5 additional members. Client-side validation
  mirrors the 2–6 hard cap the Rust side enforces; the
  Rust side rejects the same case with a clear error
  if a future caller skips validation.
- **New Tauri commands.** `group_list`, `group_create`,
  `group_add_member`, `group_remove_member`,
  `group_send`, `group_history`, `group_run_turn` —
  all in `src-tauri/src/commands/groups.rs`, wired
  through the existing `invoke_handler!` macro.
- **Group executor** (`src-tauri/src/groups/mod.rs`).
  Reuses the existing `BotExecutor` and the existing
  `bot://chunk` / `bot://done` / `bot://error` event
  surface — no new event channel, no forked
  executor. Front-end keys streaming off `bot_run_id`
  so multiple Bots in a group can stream concurrently
  without crosstalk.
- **`@BotName` mention parser** (`src/lib/mentions.ts`).
  Pure, shared between the Composer (to pick which
  Bots to run) and the Renderer UI (to highlight
  mentions in the rendered message bubbles).

### Fixed
- (none for v2.4.0)

### Changed
- **Bumped to 2.4.0** in `package.json`,
  `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json`.

## v2.3.6 — 2026-09-09

### Fixed
- **"Use my default key" button was hidden for
  upgraded installs.** v2.3.5's migration defaults
  `computer_use_default_ssh_key` to `true`, but the
  toolbar button was gated on the inverse
  (`!settings.computer_use_default_ssh_key`), so a
  user with a pre-v2.3.5 VM (per-Bot key in
  `authorized_keys`) could never see the button that
  bootstraps the default key. v2.3.6 shows the button
  whenever the VM is `running` — the QGA install is
  idempotent, so re-clicking is safe. After a
  successful install, the button stays visible (the
  user can re-run to confirm) but is effectively a
  no-op.
- **Provision still required a passphrase even when
  the default-key flag was on.** v2.3.5 hid the
  passphrase field in the Settings UI when the flag
  was true, but the provision flow in
  `ComputerManager::provision` still bailed with
  `PassphraseMissing` on an empty passphrase — so new
  installs could provision zero Bots. v2.3.6
  extracts the passphrase decision into
  `resolve_provision_passphrase(&Settings)`: when the
  default-key flag is on, the per-Bot key is still
  encrypted+stored (so the user can toggle the flag
  off without re-provisioning), but the passphrase
  is a deterministic placeholder instead of the
  user's (empty) value. The placeholder is never
  used to decrypt on the hot path.
- **Stale "Check Settings → Computer → passphrase"
  hint in the Console error banner.** v2.3.5 hides
  the passphrase field, so the v2.3.4 hint was
  pointing users at a control that no longer exists.
  v2.3.6 points users at the install-default-key
  button (always visible on a running VM) and the
  `ssh crispy` terminal smoke test.

## v2.3.5 — 2026-09-09

### Added
- **`computer_use_default_ssh_key` setting (default
  `true`).** When on, MaxBot's SSH path leaves
  `identity_file` empty so ssh falls back to the user's
  default key (`~/.ssh/id_ed25519` / `~/.ssh/id_rsa` /
  ssh-agent). The per-Bot key + passphrase model was
  overkill for a personal tool — if you forgot the
  passphrase, your only path forward was Destroy +
  re-provision. New installs skip the passphrase
  field entirely.
- **`computer_install_default_key` Tauri command.**
  Installs the user's default SSH public key into an
  existing VM's `authorized_keys` via the QEMU guest
  agent (no SSH required — solves the chicken-and-egg
  case for users with an empty passphrase). The QGA
  pipeline uses `grep -qxF … || echo …`, so the
  install is idempotent — re-running doesn't duplicate
  the key line.
- **"Use my default key" button on the ComputerPanel
  toolbar.** Renders only when the VM is running AND
  the per-Bot key path is still active. Clicking it
  calls `computer_install_default_key`, flips the
  setting via `saveSettings`, and shows a small inline
  confirmation. The button disappears on the next
  render.
- **QEMU guest agent command on the
  `LibvirtClient`.** A `qemu_agent_command(domain,
  json)` wrapper over `virsh qemu-agent-command` so
  future slices can run commands on the guest without
  SSH.

### Fixed
- **Existing-VM bootstrap path.** A user with a
  pre-v2.3.5 VM (per-Bot key in
  `/home/bot/.ssh/authorized_keys`) can now click
  "Use my default key" and the VM's auth path
  switches over without Destroy.

### Changed
- **`ServerConfig::from_settings` zeros out
  `identity_file` when the default-key flag is on.**
  The existing `spawn_tunnel` already handles the
  empty-string case (skips the `-i` arg), so the
  VNC tunnel falls back to the user's default key.
- **`ComputerManager::ensure_bot_unlocked` skips the
  per-Bot key fetch when the default-key flag is
  on.** The VM endpoint is still cached so `vm_sftp_*`
  and `vm_exec` can route to the right IP, but
  `ssh_keys` is never read on the hot path. Legacy
  users with the flag off keep the original
  behavior.
- **VM-side ssh/sftp (`vm_exec`, `vm_sftp_*`) accepts
  `Option<&Arc<Vec<u8>>>` for the per-Bot key.** When
  `None` (the default-key path), the call skips `-i`
  and lets ssh fall back to the user's default key —
  which the install-default-key command put in the
  VM's `authorized_keys`.
- **`ssh_keys` table and the
  `computer_unlock_keys` IPC path are still there.**
  Legacy users who haven't switched yet still rely on
  them; v2.3.5 just stops reading them on the hot
  path. The provisioning flow continues to write them
  for backwards compat.

## v2.3.0 — 2026-09-09

### Added
- **Routines.** A Routine is "run Skill X at time Y on
  bot Z." The new `Routines` tab in the side panel
  lists every bot's routine, lets the user bind a
  Skill to a schedule, and offers New / Edit / Delete
  affordances. The existing `bot_schedules.interval_seconds`
  / `cron_expression` fields are now wired to a real
  UI.
- **Skill-bound schedules.** The `BotSchedule` row
  gained a `skill_id` column (forward-compat added in
  v2.2). When set, the background scheduler dispatches
  to the Skill executor — `run_skill` runs the Skill's
  steps, creates a `skill_runs` row, and bumps
  `last_run_at` — instead of `run_bot_once`. A
  scheduler-fired run sees the same per-Bot filesystem,
  Computer VM, and tool allowlist as a chat-time
  invocation; the only difference is the LLM chat
  loop is bypassed in favor of the recorded
  procedure.
- **`ScheduleEditorDialog`.** A modal that takes a
  `Bot` + an optional existing `BotSchedule` and lets
  the user pick: a Skill (or "no skill" — fires the
  chat loop), a trigger (interval in minutes or a
  5-field cron with 4 presets + an Advanced text
  input), and a live one-line English preview of the
  cron's next fire. Validation rejects an empty cron
  or a 0-minute interval.
- **Per-schedule dispatch is a `pub fn tick(app)` on
  the scheduler.** Factored out of the inner 30s
  loop so the `#[ignore]`d `scheduler_e2e` test can
  drive the same code path the production
  `scheduler_loop` uses, with a test-supplied
  in-process `NoopTool` registry.

### Changed
- **`BotSchedule.skill_id` is read/written through
  the four schedule methods on `Database`.** The
  column was added in v2.2 as a forward-compat; v2.3
  is the first slice to actually use it. `get_schedule`,
  `upsert_schedule`, `list_due_schedules`, and
  `list_all_schedules` now read and persist the
  field. No new migration — the column was already
  in `bot_schedules` since v2.2.
- **Scheduler dispatch branches on `skill_id`.** The
  existing `run_bot_once` path is unchanged; a new
  `fire_skill_due` path is taken when the schedule
  has a Skill bound. Both run as separate
  `tauri::async_runtime::spawn` tasks so a slow
  Bot or Skill doesn't block the next tick.
- **`run_skill` was split.** The existing
  `run_skill` is now a thin wrapper that builds
  the default `ToolRegistry` and calls a new
  `run_skill_inner(app: Option<AppHandle>, state,
  ..., registry)` whose registry is passed in by the
  caller. The e2e test passes a custom registry
  with a `NoopTool` so the skill runs end-to-end
  without needing the libvirt VM or a real
  AppHandle.

### Fixed
- **No new top-level dependencies.** The cron
  preview is rendered inline for the 4 presets and
  a handful of common shapes; the `cron` Rust
  crate (already a dep for the scheduler) handles
  the production due-time check. `tempfile` was
  added as a `[dev-dependencies]` for the
  `#[ignore]`d e2e test only — production builds
  don't link it.

## v2.2.0 — 2026-09-09

### Added
- **Skills.** A Skill is a saved, named list of
  `(tool, args)` steps the user can invoke on demand
  against any Bot. The Skill file is plain JSON — diffable,
  grep-friendly, no DSL — and lives in a new `skills` SQLite
  table plus a `skill_runs` table for the durable
  run-history. Three ways to create a Skill: paste JSON,
  "Import" a `.skill.json` file, or "Record" a live Bot run
  and let the executor's tool-dispatch hook capture every
  tool call into a candidate Skill that the user
  name-and-saves.
- **`output_var` substitution.** A step may set
  `output_var: "name"` to bind its output, and a later
  step's `args` may reference `"{{name}}"` to receive it.
  Substitution is whole-string only (no partial-string
  patching) so a placeholder never accidentally rewrites a
  URL or a SQL fragment. Missing-binding references fail
  the run with a friendly error and the failing step is
  highlighted in the run card.
- **`run_skill` tool for the LLM.** A Bot's agent loop
  can now call `run_skill({ skill_id, args })` to invoke
  a Skill from inside a longer task. The tool uses the
  shared `ToolRegistry` (no parallel tool path), runs
  autonomously (no consent dialog — the bot's
  `allowed_tools` allowlist is the chat-time guardrail),
  and returns a short summary; the full per-step trace is
  in the run history.
- **Skills panel UI.** A top-level "Skills" tab in the
  sidebar shows the saved skills with "Run" / "Record" /
  "Import" / "Delete" affordances. The Run dialog
  builds its form from the Skill's `inputs` schema. The
  Record dialog drives the configure → recording →
  editing → save flow with the captured steps
  editable in a JSON textarea before persistence.
- **Run history.** Each skill shows its last 5 runs
  inline (status, timestamp, summary). A future v2.3
  schedules table can wire a Skill to a Bot's schedule
  via the new `bot_schedules.skill_id` column (added
  forward-compat in this slice).

### Changed
- **Bot executor signature.** `run_bot_once` now takes
  an optional `recording_id: Option<String>` so the
  Skill recorder can hook in. Passing `None` preserves
  the v2.1 behavior exactly; the Bot scheduler and the
  `bot_run` / `send_to_bot` command paths all pass
  `None` and are unchanged. The recorder hook fires
  after every `bot_registry.execute(...)` inside the
  agent loop, capturing the resolved args, the tool
  output, and an `is_error` flag.
- **`AppState` gained a `recorder: Arc<RecorderState>`
  field.** The recorder is a `Mutex<HashMap<recording_id,
  Vec<RecordedStep>>>`; `skill_record_start` allocates a
  session id, `skill_record_stop` drains it into a
  candidate `Skill`. The recorder hook is the only
  consumer of this state on the executor side.

### Fixed
- **Skill step reference resolves a missing
  `output_var` binding with a friendly error.** Before
  this slice, a `{{ name }}` placeholder that no earlier
  step bound would crash the Skill at the tool call;
  v2.2 surfaces "step references `{{ name }}` but no
  earlier step bound that variable" and halts the run
  with the failed step highlighted.

## v2.1.0 — 2026-09-09

### Added
- **Edit / Save / Cancel in the ComputerPanel Files tab.** A
  user can now edit any file on a per-Bot VM and save it back.
  Save calls the existing `computer_file_write` Tauri command
  (atomic temp+rename). The Save flow ships with a new
  vitest that round-trips through the file-write mock.

### Fixed
- **Cold-cache SSH delay in the per-Bot provision script.**
  On a cold libvirt cache, sshd refused port-22 connections
  for 2+ minutes after the IP lease appeared. Root cause:
  Ubuntu 24.10's cloud image ships `/etc/ssh/sshd_config`
  but no host keys, and `cc_ssh` (which would generate them)
  runs *after* the `bootcmd` phase. When our bootcmd did
  `systemctl enable --now ssh`, sshd's ExecStartPre
  (`sshd -t`) failed with "no hostkeys available" and
  because `ssh.service` is `Type=notify`, systemctl blocked
  waiting for READY=1. The fix in `provision-vm.sh` adds a
  `bootcmd:` block that generates host keys with
  `ssh-keygen -A`, pre-creates the bot user + drops the
  authorized_keys, and `systemctl enable --now ssh` —
  all before any package install. End-to-end: sshd binds
  port 22 within ~30s of the IP lease. The
  `provision_e2e_against_crispy` smoke test's SFTP wait
  is back to 60s; the 180s fallback is removed.

### Changed
- **Cleaned up 35 dead-code warnings.** Unused imports
  in `bots/`, `grok_build/`, `tools/`, `llm/`, and
  `computer/` were deleted. Three genuinely dead helpers
  (`build_user_data`, `build_meta_data` in `provision.rs`,
  `list_bots_with_schedule` in `bots/filesystem.rs`) were
  removed entirely. Eight unnecessary `mut` qualifiers
  were dropped. `cargo build --release` now shows 4
  pre-existing `#[allow(dead_code)]` items and 0 new
  warnings. The total Rust test count dropped from 128
  to 125 — the three removed tests covered the deleted
  helpers.

## v2.0.4 — 2026-09-09

### Added
- **ComputerPanel Files tab** (read-only SFTP browser). The
  per-Bot VM Files tab lists `/home/bot` (default) and
  navigates into subdirectories. Click a file to view
  its contents in a monospace viewer pane. The
  `computer_file_list` / `computer_file_read` Tauri
  commands have been wrapped in
  `src/lib/tauri.ts` and `src/lib/api.ts`; the underlying
  SFTP path was previously unwired on the React side.
  Write support (`computerFileWrite`) is also wrapped
  and ready for the next slice's "Save" affordance.

### Fixed
- **Per-Bot SFTP / SSH connected directly to the libvirt
  NAT IP**, which is unroutable from the Mac. Every
  per-Bot SFTP call failed with "Network is unreachable"
  the moment the VM got a DHCP lease. The fix routes
  every per-Bot SFTP/SSH through the MaxBot server
  using `ProxyJump`. `VmEndpoint` now carries
  `proxy_host` / `proxy_user` (populated from
  `Settings.computer_server_host` / `_ssh_user` at
  provision time). The Rust `run_sftp_batch`,
  `run_sftp_capture`, `run_sftp_write`, and
  `run_ssh_with_key` all now add
  `-o ProxyJump={user}@{host}` so the per-Bot key
  authenticates the destination and the OS keychain
  authenticates the proxy hop. v2.0.0–v2.0.3 had the
  same latent bug; nobody hit it because the
  FileBrowser didn't exist yet.

## v2.0.3 — 2026-09-09

### Fixed
- **ComputerPanel VNC console rendered silently blank** when the
  noVNC RFB handshake failed. There were two related problems:

  1. **VNC password mismatch.** The `provision-vm.sh` script
     started x11vnc with `-rfbauth /home/bot/.vnc/passwd`,
     requiring a VNC password. The Rust side generated the
     password locally, sent it to the script, and then
     discarded it — the noVNC client never received a
     `password` option and the RFB handshake always failed
     with "no supported security types" or "VNC security
     handshake failed." The viewer area then rendered as
     a blank `<div>` with no visible feedback.
  2. **noVNC errors were silently swallowed.** The
     ComputerPanel's `onError` from the viewer was written
     into the same `errorMsg` state that's only shown when
     `computer.state === "error"`. For a running VM with a
     failed viewer, the error was set but never displayed.
     The user saw an empty viewer area with no indication
     of what went wrong.

  - **Fix for #1:** Drop the VNC password requirement.
    `x11vnc` is started with `-nopw` instead of
    `-rfbauth`. The VNC port is only reachable from the
    server's loopback (libvirt binds to 127.0.0.1), and
    the Tauri side bridges to it over an SSH tunnel that
    already requires the user's SSH key. The second secret
    was redundant. The 5th positional argument of the
    script is preserved (and ignored) for API
    compatibility.
  - **Fix for #2:** Add a separate `viewerError` state in
    the ComputerPanel and render it as a positioned
    overlay on top of the viewer (`computer-panel__viewer-error`
    in `styles.css`). The overlay shows the noVNC error
    message and a hint to "Hand back" or Restart.
    Successful `onConnect` clears the error so the overlay
    disappears on a successful reconnect.

### Changed
- **`src-tauri/scripts/provision-vm.sh`:** x11vnc now runs
  with `-nopw`; `x11vnc -storepasswd` and the
  `~/.vnc/passwd` setup are removed. The 5th argument
  (VNC password) is preserved but ignored. The deployed
  copy at `/opt/maxbot/provision-vm.sh` on `crispy` is
  updated. New VMs provision correctly; existing VMs
  need to be destroyed and re-provisioned (the user's
  running `maxbot-bot-155ffaaa-…` is one of those).

## v2.0.2 — 2026-09-09

### Fixed
- **Every per-Bot VM provision failed with "Computer error"**
  because the Rust `provision` chain had two latent bugs in the
  same code path. Surfaced via the `provision_e2e_against_crispy`
  smoke test (`#[ignore]`, `cargo test --lib
  provision_e2e_against_crispy -- --ignored --nocapture`).

  1. **DHCP hostname mismatch in the IP poll.** The script
     `provision-vm.sh` writes `local-hostname:
     ${VM_NAME//_/-}` into cloud-init meta-data, and
     `VM_NAME` is `maxbot-bot-{sanitized_bot_id}`. The Rust
     side built its expected hostname as
     `maxbot-{sanitize_for_hostname(bot_id)}` — missing
     the `bot-` segment — and the `find` on the lease list
     never matched. After 90s the IP poll timed out, the
     ComputerPanel polled the failed row, and the user
     saw "Computer error: The VM is in an error state.
     Try Restart, or Destroy and re-provision." even
     though the VM was up, had a DHCP lease, and was
     reachable.
  2. **`virsh net-dhcp-leases` parser was off-by-one.**
     It used hard-coded indices (`parts[1]` for MAC,
     `parts[3]` for IP, `parts[4]` for hostname) and was
     only tested against an ISO-8601 `T`-joined timestamp.
     Real `virsh net-dhcp-leases` on Ubuntu 24.10+ emits
     the timestamp as TWO whitespace-separated tokens
     (`YYYY-MM-DD HH:MM:SS`), which shifts every column
     by one. The parser's hostname ended up being the
     IP address, and the IP poll's `find` never matched.
     New parser anchors on the `ipv4`/`ipv6` token and is
     robust to either timestamp shape.
  3. **Domain name from the script's stdout was the
     whole script's output.** `qemu-img create` writes
     its formatting progress to stdout, and the script's
     `echo "$VM_NAME"` is the LAST line. The Rust code
     did `out.stdout.trim()` and got a several-hundred-
     byte "domain name" that included qemu-img's
     progress plus "Starting install... Domain creation
     completed." plus the real VM_NAME. Every
     subsequent `virsh vncdisplay` / `virsh destroy` /
     `virsh undefine` failed with "failed to get domain
     '...'." Fix: take the last non-empty line of
     stdout.

### Added
- **`#[ignore]`d integration test
  `provision_e2e_against_crispy`** in
  `src-tauri/src/computer/mod.rs`. Provisions a real
  VM on the live `crispy` server (env override:
  `MAXBOT_TEST_SERVER_HOST`, `MAXBOT_TEST_SERVER_SSH_USER`,
  `MAXBOT_TEST_PASSPHRASE`), asserts the row is in
  `running` state with an IP + VNC port, then tears the
  domain + pool down. Exits in ~15s on a warm
  libvirt. Catch this class of bug before it ships.
- **Regression test `parse_dhcp_leases_handles_space_separated_timestamp`**
  in `src-tauri/src/computer/libvirt.rs`. Real
  Ubuntu 24.10 virsh output captured at 2026-09-09
  11:42 MDT, two leases, asserts the parsed
  IP/MAC/hostname for both. The old test only covered
  the ISO-8601 timestamp variant and let this bug
  ship in v2.0.0.

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
