# Changelog

All notable changes to MaxBot are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/) and the project
adheres to [Semantic Versioning](https://semver.org/).

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
