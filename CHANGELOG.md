# Changelog

All notable changes to MaxBot are documented in this file. The format
follows [Keep a Changelog](https://keepachangelog.com/) and the project
adheres to [Semantic Versioning](https://semver.org/).

## Unreleased — 3.7.17

### Added — `maxbot_loopd` sidecar bundle + Sidebar UI (Slices 1, 2, 3)

The keep-alive loop supervisor is now a bundled sidecar of
`MaxBot.app`. From the Sidebar's Loop panel, the user can
start/stop the supervisor, watch its state, and read the
current TASK.md + last journal entry. Closing MaxBot does
not kill the supervisor if a turn is in flight — the
supervisor is detached via `Stdio::null()` + `setsid()` and
its source-of-truth for "is anything running?" is
`STATE.json.pid` + `kill(pid, 0)`, not the child handle.

- **`src-tauri/src/loop/{io,daemon,sandbox}/`** — new
  supervisor module. Files-on-disk is the source of truth
  (no in-context accumulation); per-turn LLM with a single
  `NoopModelCaller` for now. Writes go through `atomic_write`
  (`.tmp` + `fsync` + `rename`), verified safe under SIGKILL
  in 26 live-binary rounds.
- **`src-tauri/src/bin/maxbot_loopd.rs`** — CLI binary
  (`--loop-dir`, `--heartbeat-secs`, `--log-level`).
- **`src-tauri/src/commands/loopd.rs`** — five Tauri
  commands (`loopd_status` / `loopd_start` / `loopd_stop` /
  `loopd_read_task` / `loopd_read_journal`). Read-only with
  respect to `loop/io` — parses `STATE.json` / `TASK.md` /
  journal standalone, never imports from `crate::r#loop::io`.
  Spawns the supervisor via `Command::spawn` with stdio
  detached and `setsid()` in `pre_exec`. Stops via SIGTERM
  → 3-second poll → SIGKILL.
- **`src/components/LoopPanel.tsx`** — Sidebar panel.
  Polls `loopdStatus` every 2s for the minimal shape (pid,
  state, task_status, task_excerpt_len, last_heartbeat,
  age_secs). Reads the full TASK.md body + last journal
  entry only on explicit Read (initial mount + manual
  Refresh). The Refresh button highlights when the body has
  changed since the last Read.
- **`src-tauri/tauri.conf.json`** — `bundle.externalBin`
  declares `binaries/maxbot_loopd` (resolved to
  `binaries/maxbot_loopd-<target-triple>` at bundle time).
  Tauri copies the sidecar to `Contents/MacOS/` of the
  packaged `.app` and codesigns it with the same ad-hoc /
  runtime flags as the host binary.
- **`src-tauri/binaries/.gitkeep`** — placeholder so the
  dir is tracked. The actual `maxbot_loopd-<triple>`
  binary is NOT checked in; the release pipeline builds it
  and stages it before `cargo tauri build`.

**Build command for the sidecar** (release pipeline):

```sh
cargo build --release --bin maxbot_loopd --target aarch64-apple-darwin
cp src-tauri/target/aarch64-apple-darwin/release/maxbot_loopd \
   src-tauri/binaries/maxbot_loopd-aarch64-apple-darwin
cargo tauri build
```

### Changed — Renderer `loopdStatus` IPC shape (Slice 3)

The Sidebar's 2-second poll pulls ONLY the minimal
`LoopdStatus` shape. Full TASK.md body + last journal
entry are loaded on explicit Read. The full STATE.json
counters (`turn`, `completed_turn`, `started_at`, `run_id`,
`last_action_id`) are NOT in the polling response anymore
— they were KB-sized reads on every tick that the renderer
didn't need.

### Bundle identifier

Bundle identifier is `com.maxbot.app` in this branch
(d964581 baseline). The v3.7.17 production identifier
rename (`com.maxbot.app` → `com.maxbot.app.devtools`) is
on `release/3.7.17-bundle-id-rename` and merges separately.
The `loopd` binary works with either identifier; the loop
dir is `<app_data_dir>/loop/`, which Tauri resolves
per-app.

### Out of scope (still parked)

- Plaintext `computer_passphrase` in SQLite settings row.
- Orphan VM `maxbot-bot-d42b8c1c-…`.
- `tauri.conf.json` trailing comma.
- WebKit memory leak "fixes" other than the v3.7.17
  identifier rename.
- Sandbox policy edits.

## v3.7.16 — 2026-09-10

### Changed — Multi-VM VNC port allocation (Hardening item #3)

The v3.7.5 VNC auth fix hard-coded the qemu:commandline
`-vnc 127.0.0.1:0,password=off,to=5999` and the Rust side
fell back to port 5900 whenever `virsh vncdisplay` failed
(`<graphics none>` means libvirt doesn't know about the
no-auth VNC, so `virsh vncdisplay` always failed).
That worked for VM #1 — QEMU's `to=5999` knob picked 5900
because it was the first free port. For VM #2, QEMU would
pick 5901 (first free after 5900) but the Mac side still
tried to SSH-tunnel to 5900, and the user got a black
screen. The "5900 fallback" comment in
`src-tauri/src/computer/provision.rs` explicitly noted
"Multiple VMs would need sequential port allocation
(future work; v3.7.5 is a single-VM slice per Tyler's
'lets use one vm' direction 2026-09-10)".

v3.7.16 makes the allocation explicit and Mac-side-driven:

- **`src-tauri/src/computer/mod.rs`** — `ComputerManager::provision`
  now reads the current `computers.vnc_port` values (via
  `db.list_computers()`), filters to `Option<u16>`, and
  hands the list to `next_free_vnc_display`. The chosen
  display is passed to `provision_vm` as a new
  `vnc_display: u8` parameter. If the 5900-5999 range
  is full (101 Bots provisioned), provisioning errors
  with a clear "VNC port range 5900-5999 is fully in use;
  destroy a Bot before provisioning another" message.
  v3.7.16 only reads the `vnc_port` column — the
  `state` field is irrelevant (a Destroyed Bot has its
  row removed, not set to a tombstone state, so the
  freed port automatically reappears as a candidate).

- **`src-tauri/src/computer/provision.rs`** —
  - `next_free_vnc_display(in_use, range_lo, range_hi)`
    is a new pure function. Returns the lowest display
    number in `[lo, hi]` not in `in_use - 5900`.
    Out-of-range entries in `in_use` are ignored. Returns
    `None` if the range is exhausted. Picks the lowest
    free slot — a Destroyed Bot's port gets reused
    immediately.
  - `vnc_port_from_display(display: u8) -> u16` is the
    `5900 + display` math, saturating to avoid
    overflowing u16 on a stray 255.
  - `poll_for_vnc_port` is replaced by
    `poll_for_vm_running` — same retry shape (10×500ms)
    but asks `virsh domstate` for the `running` state
    instead of `virsh vncdisplay`. The qemu:commandline
    VNC isn't visible to libvirt (no `<graphics>`
    element), so the old `vncdisplay` path was
    guaranteed to fail; the new path is a real "is the
    VM alive" check, and the port is the one we passed
    in. The 5900 fallback is gone.
  - The script call adds a 6th positional arg:
    `sudo -n /opt/maxbot/provision-vm.sh <name> <disk_gb>
    <ram_mb> <pubkey> '' <vnc_display>`. The empty 5th
    arg is the v3.7.5 preserved-and-ignored
    `<vnc_password>` slot — kept untouched for API
    stability. The new 6th arg carries the display
    number.
  - 9 new unit tests cover the free-display
    selection (empty list, skip in-use, lowest-free
    wins, full range returns None, range respected,
    invalid range returns None) and the
    display-to-port math (inverse, saturation).

- **`src-tauri/src/computer/libvirt.rs`** — new
  `LibvirtClient::domstate(pool, name)` wraps
  `virsh domstate <name>`. The result is parsed via the
  existing `DomainState::from_libvirt` (which already
  maps `running`, `shut off`, `crashed`, etc.). The
  renderer's ComputerPanel status path is unaffected
  (it still uses `list_domains` for the roster);
  `domstate` is strictly the "did the just-provisioned
  VM boot?" check.

- **`src-tauri/scripts/provision-vm.sh`** —
  - New 6th positional arg `VNC_DISPLAY="${6:-0}"`.
    Default `0` preserves the v3.7.5 behavior for
    hand-calls (debugging).
  - The qemu:commandline patch now substitutes
    `127.0.0.1:<VNC_DISPLAY>` instead of the hard-coded
    `127.0.0.1:0`. The Python heredoc gets
    `vnc_display` from `sys.argv[1]` and concatenates
    it into the `<qemu:arg value='...'/>` element.
  - The `to=5999` knob stays so QEMU can still
    auto-pick the next free port if the display number
    we pass is busy for any reason (it shouldn't be
    unless the host has other QEMU VMs outside MaxBot).
  - Header comment gains a v3.7.16 block explaining
    the new arg + the multi-VM rationale. The
    section-4 doc comment is updated to reference
    `<VNC_DISPLAY>` instead of `:0` and to note that
    the `poll_for_vnc_port` fallback is gone.

- **Wire shape unchanged.** `ProvisionResult.vnc_port`
  is still `u16`; the only thing that changed is where
  the value comes from (Mac-picked + Mac-confirmed
  vs. hard-coded 5900 fallback). Existing
  `db.list_computers` is the only DB call added.
- **Trust model unchanged.** Same SSH-gated tunnel,
  same libvirt loopback bind, same `password=off`
  qemu:commandline.
- **Migration.** Existing single-VM installs are
  unaffected: the script's `VNC_DISPLAY` defaults to
  `0`, and the Mac side picks `0` when no other rows
  exist. Re-provisioning an existing VM is not
  required (the port on its existing `computers` row
  stays the same).

## v3.7.6 — 2026-09-10

### Added — Per-row Destroy button in BotRoster (cascades `computer_destroy` + `delete_bot`)

- **`src/components/BotRoster.tsx`** — each row in the sidebar
  roster gets a small `×` button on hover. Clicking it fires a
  confirm dialog ("Destroy Bot + VM?") that cascades both
  `computer_destroy` (server-side `virsh undefine` +
  `remove-all-storage`) and `delete_bot` (the SQLite row)
  in one shot. The Bot editor's existing Delete button
  delegates to the same handler (v3.7.7 rewired it) so
  the two entry points converge on the same cascade.
- **No orphaned VMs.** The cascade runs in one transaction
  on the Rust side; if either step fails the other is
  rolled back to the pre-destroy state.
- The `×` is hover-only so the roster stays quiet when
  the user is just reading.

## v3.7.7 — 2026-09-10

### Added — Takeover **Stop now** button + canonical 2FA walkthrough

- **`src/components/ApprovalQueue.tsx`** — the approval row
  that surfaces a `needs_human: true` from the Bot's
  `vm_computer_use` tool now has two distinct primary
  actions: **Hand back** (the 2FA happy path — synthesizes
  a tool success so the Bot's run resumes) and
  **Stop now** (cascades to `stop_bot_run(runId)` +
  `approval_decide(rejected)`, ends the run row as
  `Failed`, halts the executor).
- **`docs/2fa-walkthrough.md`** — the canonical
  Gmail → 2FA → takeover → solve → hand back end-to-end
  walkthrough, with the "Hand back vs Stop now" decision
  table and the daemon-parked-run internals. The brief
  asked for a step-by-step the user can follow on a
  first attempt; the doc is that artifact.
- **`stop_bot_run` is best-effort on purpose** — for
  daemon-parked runs (started while the app was closed),
  the runId is not in `activeRunByBot`, so the
  `approvalDecide` alone unblocks the row. The
  user-facing contract: after **Stop now**, the row is
  `Failed` and no more tool calls will fire.

## v3.7.8 — 2026-09-10

### Added — `backup_memory(bot_id)` Tauri command + Memory panel button

- **`src-tauri/src/commands/memory.rs:169`** — new
  `backup_memory(bot_id)` Tauri command. Reads all
  three memory kinds (Facts, Preferences, History) from
  the Bot's VM, concatenates them into a single
  `~/bots/_shared/memory/<bot_id>/<timestamp>.jsonl`
  on the host (the same path the `shared_fs` Bot tool
  uses), and returns the absolute resolved path +
  entry count. The file write goes through the existing
  `SshPool` and is base64-encoded so it's binary-safe.
- **`src/components/MemoryPanel.tsx:158`** — new
  "Back up to shared/" button in the Memory panel. A
  `✓ Backed up N entries to /home/<user>/bots/_shared/...`
  toast confirms the write. Empty memory is a no-op
  (writes an empty file with a timestamp marker) so
  the user gets a clear "0 entries" toast instead of
  an error.
- **Restore is not part of v3.7.8** — a future slice
  will read the JSONL back into the per-Bot
  `{kind}.jsonl` files based on the `kind` field on
  every line. The schema is forward-compatible.
- The `kind` field is on every line of the JSONL
  (each kind's individual files don't carry the
  discriminator — it's implicit in the filename).

## v3.7.9 — 2026-09-10

### Changed — In-panel click-through **Drive** replaces SSH-tunnel Takeover

The v3.7.2 Takeover flow spawned an SSH-tunnel to the
Bot's VNC display and opened macOS Screen Sharing in a
new window. The v3.7.5 "TigerVNC instead of Screen
Sharing" pivot kept the same tunnel-plus-external-viewer
shape. v3.7.9 deletes both: the in-Preview click-through
**Drive** button forwards pointer + keyboard to xdotool
over the same SSH connection the rest of the Computer
surface already uses. No external viewer, no password
prompt, no tunnel.

- **`src/components/ComputerPanel.tsx`** — Drive is a
  single in-panel click. The banner reads "You are
  driving — bot input paused" while active. Click
  **Hand back** in the banner to clear the per-Bot
  driving flag; the Bot's `vm_computer_use` tool then
  resumes on the next turn. The preview JPEG and the
  Drive click targets are the same surface — there's
  no separate Takeover mode anymore.
- **`src-tauri/src/computer/mod.rs`** — the
  `ComputerManager` keeps a `driving: Mutex<HashMap<String,
  Arc<AtomicBool>>>` so the renderer can set / clear
  the per-Bot driving flag without round-tripping to
  the DB. The flag is consulted by `vm_computer_use`:
  when a Bot's flag is on, the tool refuses to run
  xdotool / scrot itself so the user's events and the
  agent's events don't fight for the same X11 session.
- **`src/lib/tauri.ts`** — the `takeover_*` wrappers
  (open / close / status) are replaced with
  `computer_input_open` / `computer_input_event` /
  `computer_input_close` + a `screenshot_size` query.
  The renderer translates panel clicks into the input
  event shape the Rust side expects.
- **No SSH tunnel, no external viewer, no
  password prompt.** The only auth is the SSH key
  the user already has to the server.
- **Existing VMs need Destroy + re-provision** to
  pick up the new `qemu:commandline` (the v3.7.5
  `auth=none` works correctly; v3.7.9 just drops
  the tunnel plumbing). For most users the only
  visible change is the panel UI: Preview + Drive
  instead of Preview + Take over (TigerVNC).

## v3.7.10 — 2026-09-10

### Changed — Persistent VM Chromium session (explicit `--user-data-dir`)

The Bot's Chromium session previously lost login state
between agent runs because `chromium-browser ... --user-data-dir
/tmp/...` pointed at a tmpfs that died with the run.
v3.7.10 makes the session persistent: the `provision-vm.sh`
script now creates `/home/bot/.config/chromium-default/`
on first boot, the `vm_computer_use` tool's `open_url`
helper pins `--user-data-dir=/home/bot/.config/chromium-default/`,
and the `Bot`'s library loads the same profile on every
subsequent run. Logins (GitHub, Google, banking sites)
survive across agent runs.

- **`src-tauri/scripts/provision-vm.sh`** — `bootcmd:`
  gains an `mkdir -p /home/bot/.config/chromium-default &&
  chown -R bot:bot ...` line so the dir exists before
  Chromium first runs.
- **`src-tauri/src/tools/vm_computer_use.rs`** —
  `build_open_url_cmd` now pins the user-data-dir
  (and pins `--no-first-run` so the welcome screen
  doesn't show on subsequent runs). The directory
  path is checked to start with `/home/bot/`; the
  `description_mentions_every_helper` test pins the
  helper list.
- **Existing VMs need Destroy + re-provision** to
  pick up the new `bootcmd`. New VMs get the
  persistent dir on first boot.

## v3.7.11 — 2026-09-10

### Changed — Composite reliability (OCR-augmented screenshots + `wait_for_stable`)

The `vm_computer_use` tool's screenshot helper used to
return a base64 PNG plus a JPEGs-as-text "page content"
guess from xdotool's `getactivewindow` output. For modern
JS-heavy sites the active-window title is often empty or
the cookie banner's, which made the agent loop on "the
page didn't change." v3.7.11 augments that with a
`tesseract` OCR pass on the PNG and a `wait_for_stable`
helper that polls the screenshot until three consecutive
frames hash equal (or a timeout fires).

- **`src-tauri/src/tools/vm_computer_use.rs`** —
  `take_screenshot` returns both the PNG and a best-effort
  OCR text (empty string when tesseract is missing on
  the server — the helper doesn't fail loudly because
  the PNG is still useful). `wait_for_stable` is a new
  helper that takes a per-Bot stable-hash of the last
  3 frames; agent scripts can wait for the page to
  actually settle instead of polling blind.
- **Better xdotool error context** — the script-side
  error message now includes the failed command's exit
  code + stderr tail so the agent can debug "I clicked
  the wrong coordinate" without SSHing in.
- **Tesseract is optional.** The helper gracefully
  degrades when it's not installed (the
  `take_screenshot_returns_empty_text_when_tesseract_missing`
  test pins the no-tesseract path).

## v3.7.12 — 2026-09-10

### Changed — Real Google OAuth 2.0 (replaces "paste a token" mock)

The v3.7.0 Gmail + Calendar connector tools accepted an
OAuth refresh token pasted into the Settings palette,
and stored it in the SQLite `connectors` table. v3.7.12
replaces that with the proper Desktop OAuth auth-code
flow: the user pastes a Google Cloud project's Desktop
OAuth `client_id` + `client_secret`, clicks **Connect
Google**, completes the consent in the browser, and the
refresh token is stored and rotated automatically.

- **`src-tauri/src/connectors/oauth.rs`** — new
  auth-code flow handler. The Mac side opens the
  browser, captures the redirect, and exchanges the
  auth code for a refresh token. Tokens are stored
  in the encrypted-at-rest `connectors` table; the
  `get_userinfo` follow-up runs at first use to
  confirm the refresh token is alive.
- **`src/components/Settings.tsx`** — new Google
  Account section with the `client_id` + `client_secret`
  inputs, the **Connect Google** button, and a
  **Disconnect** button that revokes + deletes the
  stored refresh token. The old "paste a token"
  textarea is gone.
- **Wire shape unchanged for the LLM.** The
  `gmail_send` / `calendar_event_create` tools
  resolve the same `googleapis.com` endpoints; the
  only thing that changed is how the refresh token
  gets in the SQLite row.

## v3.7.13 — 2026-09-10

### Changed — UX hardening (7 commits: UX-1 through UX-7)

- **UX-1 (`f0a9843`)** — `ActivityFeed` empty / error
  copy. The rail now explains "no recent activity" and
  "couldn't load — retry" instead of leaving the column
  blank.
- **UX-2 (`8e03b20`)** — `BotRoster` presence verb.
  The status chip now reads **Working / Waiting /
  Queued / Idle** instead of the v2.x "active" / "idle"
  binary, so the user can tell "the Bot is mid-tool"
  from "the Bot is mid-tool but blocked on an approval."
- **UX-3 (`3eb47cd`)** — Tool-call form for
  `mail_draft` / `calendar_event_create` / `gmail_send`.
  The approval row for these tools now shows a form
  with the parsed arguments, so the user can correct
  a wrong email address or recipient before approving
  instead of re-typing the whole tool call.
- **UX-4 (`d32a047`)** — Approval sheet (right
  drawer / bottom panel). Long approval rows now
  expand into a side drawer on wide screens and a
  bottom panel on narrow ones, so the full
  approval-decision context is visible without
  scrolling.
- **UX-5 (`29d745f`)** — Composer density. The
  TTSToolbar + tool-call one-liner collapse into a
  single dense row when the composer is short on
  width, and expand to a full toolbar on wide
  screens.
- **UX-6 (`141dc76`)** — Settings + BotEditor
  side-drawer panels. Both modals now slide in
  from the right edge instead of centering,
  matching the rest of the renderer's right-rail
  pattern.
- **UX-7 (`c54bc4f`)** — Chat header `PC` pill. A
  small chip in the chat header shows whether the
  selected Bot has a running computer (PC visible
  ↔ PC hidden), so the user knows whether
  Computer panel is going to show a VM or a
  "no computer" placeholder.
- **Wire shape (`fece74a`)** — `ApprovalToolResult`
  enum + `denied_tool_calls` guard. The
  `approval_decide` response now distinguishes
  `Approved` / `Denied` / `Error` (the previous
  boolean was ambiguous on transient failures), and
  the agent's tool loop refuses to re-issue a denied
  tool call within the same turn.

## v3.7.14 — 2026-09-10

### Added — VoiceMode (hands-free chat-header mic + MiniMax STT)

VoiceMode is the inbound half of the voice loop. A
click-to-start / click-to-stop mic in the chat header
captures audio, ships it to MiniMax's native STT
endpoint (`asr-1.0`), and dispatches the transcript as
a regular user message.

- **`src-tauri/src/commands/voice.rs`** — new
  `audio_to_text` Tauri command. The Mac side records
  via `MediaRecorder`, encodes to WAV in memory, and
  POSTs to `https://api.minimax.io/v1/audio/transcriptions`
  with the existing MiniMax API key. macOS microphone
  permission is requested on first click via the
  `NSMicrophoneUsageDescription` Info.plist key.
- **`src/components/VoiceToolbar.tsx`** — chat-header
  mic button + level meter + auto-send. The button
  toggles between `🎤` and `⏹`. The level meter
  fills left-to-right as the user talks so they can
  confirm the mic is picking up audio.
- **`src/lib/tauri.ts`** — `audioToText(wavBlob)` wraps
  the Rust command. The transcript is inserted into
  the composer + auto-sent (no manual send).
- **Settings → Providers → MiniMax** — the
  MiniMax API key is shared with the chat provider.
  A missing key surfaces a clear error the first
  time the user clicks the mic.
- The hold-to-record mic in the Composer is the v2.7.0
  dictation flow (transcript drops into the textarea
  for review). VoiceMode is the v3.7.14 hands-free
  flow (transcript auto-sends). Both coexist.

## v3.7.15 — 2026-09-10

### Changed — STT provider in VoiceMode (OpenAI Whisper → MiniMax `asr-1.0`)

The v3.7.14 VoiceMode routed the recorded audio to
OpenAI's `whisper-1` endpoint with the user's OpenAI
API key. That's a second API key the user has to keep
configured even if they never use OpenAI for chat. v3.7.15
switches the STT endpoint to MiniMax's native
`asr-1.0` model, which reuses the same MiniMax API key
the chat provider already needs.

- **`src-tauri/src/commands/voice.rs`** — the
  `audio_to_text` handler now POSTs to
  `https://api.minimax.io/v1/audio/transcriptions`
  (multipart/form-data with the WAV blob + the
  `asr-1.0` model) using the MiniMax bearer token.
  The OpenAI Whisper code path is gone (the
  `provider === "openai"` branch was the v3.7.14
  shape).
- **`src/lib/tauri.ts`** — the `audioToText` wrapper
  no longer takes a `provider` arg; the Rust side
  resolves the key from the current Settings.
- **User-visible change.** Settings → Providers →
  OpenAI is no longer required for VoiceMode. The
  first-time setup is just the MiniMax key. The
  existing v3.7.14 mic UX + level meter + auto-send
  are unchanged.

## v3.7.5 — 2026-09-10

### Changed — QEMU VNC `auth=none`; macOS Screen Sharing no longer prompts for a password (Hardening item #1)

The v3.7.2 takeover flow opens macOS Screen Sharing to the
Bot's VNC display over the existing `ssh -L` tunnel. The
script-side VNC password was already removed in v2.0.3 (the
Rust side still generated one, sent it to the script, and
the script discarded it — see the v2.0.3 header comment in
`provision-vm.sh`). QEMU 9.x on Ubuntu 25.10 still prompted
macOS for a password, though — `--graphics vnc,listen=...`
without an explicit auth let QEMU default to VNC's DES
challenge, and Screen Sharing dutifully asked for it. Tyler
had to hunt the password down to use Takeover, which broke
the "Take over for 2FA" path the v3.7.5 2FA walkthrough
relies on.

- **`src-tauri/scripts/provision-vm.sh`** — after
  `virt-install` defines the domain, a `virsh dumpxml
  | python3 | virsh define /dev/stdin` step injects
  a `<qemu:commandline>` block with
  `-vnc 127.0.0.1:0,password=off,to=5999`. The first
  attempt was to set `auth='none'` on the `<graphics>`
  element; rejected by libvirt 11.6.0 ("Unknown
  --graphics options: ['auth']"). The second attempt
  was to pass `auth=none` to QEMU's `-vnc` via
  `<qemu:commandline>`; rejected by QEMU 9.x on
  Ubuntu 25.10 ("Invalid parameter 'auth'"). The
  third attempt — `password=off` via
  `<qemu:commandline>` — was accepted and verified
  end-to-end: standalone QEMU bound to 127.0.0.1:0
  and the RFB handshake advertises VNC_AUTH_NONE
  only (security type 1, server returns 4 zero bytes
  after the client selects type 1). `to=5999` lets
  QEMU pick any free port in 5900-5999; the running
  test VM ended up on display 50 (port 5950) because
  5900-5908 were already in use. The patch uses
  Python (multi-line regex, can't be done with a
  single sed) to inject the qemu:commandline and
  add the `xmlns:qemu` namespace declaration. Header
  comment gains a v3.7.5 block explaining the three
  failed attempts and the working path. The 5th
  positional arg (`<vnc_password>`) is preserved-
  and-ignored for API compatibility; the v2.0.3
  comment is unchanged.
- **`src-tauri/src/computer/provision.rs`** — the per-Bot
  VNC password generation (8 base64url chars via
  `OsRng.fill`) and the corresponding command-line arg are
  gone. The 5-arg invocation becomes 4-arg. The `rand::Rng`
  import goes with it (no other use in this file). Doc
  comment at the top of the file explains the v3.7.5
  drop. The comment numbering inside `provision_vm` is
  renumbered 1-8 (was 1-9).
- **Trust model unchanged.** SSH-gated tunnel + libvirt
  loopback bind is still the only gate. macOS Screen
  Sharing now connects without prompting. Existing VMs
  (e.g. `maxbot-bot-ba979435-8…`) keep their old domain
  XML; Destroy + re-provision picks up the new `auth=none`.
  The v3.7.5 2FA walkthrough's first step is no longer
  "find a password."

(More v3.7.5 work — the 2FA walkthrough, `docs/2fa-walkthrough.md`,
and "Stop now" cancel-verification — lands in subsequent commits
on the v3.7.5 branch before the v3.7.5 tag.)

## v3.7.4 — 2026-09-10

### Changed — noVNC / x11vnc / tigervnc references swept (Hardening items #2 + #5)

- **`docs/user-guide.md` "Computer" section rewritten** to reflect
  the v3.7.2 display rework: Preview is now a 300ms host-side
  `virsh screenshot` poll, Takeover opens macOS Screen Sharing
  via an `ssh -L` tunnel. The v3.0.x noVNC canvas path is
  documented as gone. (TODO note left at the top of the section:
  v3.7.7's `docs/first-bot-20-minutes.md` is the canonical
  "show me the new flow" reference and will replace the
  paragraph with a link.)
- **`docs/server-setup.md` "What gets installed on each Bot VM"
  section updated** — the `packages:` block now lists
  `lightdm + xfce4 + ...` (per v3.7.2), not the historical
  `x11vnc + tigervnc-standalone-server`. A `v3.7.2 change`
  callout explains why the in-VM VNC server is no longer
  installed.
- **`docs/v3-grand-tour.md` "Computer Panel" section** carries
  a `v3.7.4` note explaining that the screenshots are pre-v3.7.2
  and that the canonical current flow is in v3.7.7's
  `docs/first-bot-20-minutes.md`. Text body updated; screenshot
  regen is a follow-up.
- **`docs/grok-bot-reference.md`** — the Slice C/D/E summary
  now reads "host-side screenshot preview + macOS Screen
  Sharing takeover" instead of "noVNC console".
- **`src-tauri/scripts/provision-vm.sh`** — historical
  `x11vnc` / `tigervnc` mentions cleaned out of the
  explanatory comments. The script does not install
  any of those packages; the v3.7.2 v3.7.2 comment
  block now uses generic "in-VM VNC server" / "in-VM
  display server" wording instead of naming the dropped
  tools. **No behavior change** — the `packages:` block
  was already `lightdm + xfce4 + ...` since v3.7.2.
- **`README.md`** — the "Three access levels" bullet drops
  "noVNC viewer" in favor of "fresh JPEG from the QEMU
  framebuffer" + macOS Screen Sharing.

### Fixed — Two pre-existing maxbotd FK test failures

- **`daemon_token_round_trip` and `rotate_daemon_token_invalidates_old`**
  were failing in `cargo test --bin maxbotd` because they
  built a `Bot { ... }` literal and called
  `set_daemon_token(bot.id, ...)` / `rotate_daemon_token(bot.id)`
  against a fresh DB with no `bots` row. The `daemon_tokens.bot_id`
  FK to `bots.id` is enforced; the inserts failed with
  `FOREIGN KEY constraint failed`. Both tests now call a
  small `insert_test_bot` helper (the same pattern used by
  `src-tauri/src/computer/mod.rs` since v3.0.3) to seed the
  `bots` row before the token operation. **No production
  behavior change** — the production code path always
  inserts a `bots` row first; only the test fixtures were
  wrong.

### Fixed — `pick_free_port_exhausted_returns_none` flakiness

The v3.7.2 port-picker test was parallel-runnable but
shared 127.0.0.1 state with the companion
`pick_free_port_returns_port_in_range` test, so two
parallel `cargo test` runs could race over a port.
Both tests now acquire a process-wide `PORT_LOCK`
static `Mutex<()>` (same pattern as the v3.0.3
`HOME_LOCK` in `src-tauri/src/computer/mod.rs`) before
touching the OS. The test count is unchanged
(`301/0/4` in `cargo test --lib`).

### Added — `#[ignore]`d crispy smoke tests for screenshot + takeover

Two new files under `src-tauri/tests/` provide
real-crispy verification of the v3.7.2 display path
without making them part of the default CI run:

- `src-tauri/tests/screenshot_smoke.rs` — captures one
  JPEG from a known-running Bot via the existing
  `screenshot::capture_jpeg` path, asserts the bytes
  start with `0xFF 0xD8` (JPEG magic) and are
  non-empty. Skips gracefully if crispy is unreachable.
- `src-tauri/tests/takeover_smoke.rs` — opens a takeover
  tunnel via the existing `vnc::open_takeover` path,
  asserts the local port is bound on 127.0.0.1, then
  closes and asserts the port is released. Skips
  gracefully if crispy is unreachable.

Both are `#[ignore]`d. Run them with
`cargo test --lib screenshot_smoke -- --ignored --nocapture`
and `cargo test --lib takeover_smoke -- --ignored --nocapture`
when real crispy is available.

## v3.7.3 — 2026-09-10

### Added — Always-on maxbotd audit (Hardening item #3)

The Mac app's `Settings.minimax_api_key` is now pushed to the
daemon on launch and on every Settings save, so a webhook or
scheduled run can reach the LLM while the Mac is closed. The
push is in-memory only — the key is never written to disk on
the daemon, and the Mac app's local Bot runs continue to
work even if the daemon is unreachable.

- **New `POST /settings` route on `maxbotd`.** Bearer-
  authenticated (same per-Bot token pattern as `/shared`,
  `/hooks/<id>`, `/bots/<id>/recent_runs`). Body shape:
  `{"minimax_api_key": "..."}`. The daemon stores the
  key in `Arc<RwLock<Option<String>>>` on the daemon state
  and shares the lock into every per-request `AppState` it
  passes to `run_bot_once`. An empty string clears the
  override; a missing field is a no-op.
- **`AppState.llm_key_override` field.** The executor
  reads the in-memory key first when constructing the
  LLM provider, falling back to the DB-loaded
  `Settings.minimax_api_key` when the override is `None`.
  The Mac app's own `AppState` initializes the lock to
  `None`, so the in-memory path is daemon-only.
- **`pushSettingsToDaemon` Tauri command.** Best-effort
  POST to the daemon's `/settings` route. Returns
  `Ok(())` even if the daemon is unreachable (the
  Mac app logs a `warn!` and continues). Picks a
  per-Bot token from the local `daemon_tokens` table
  for auth (the daemon's `/settings` route uses the
  per-Bot token pattern).
- **App.tsx wires the push on two paths.** The bootstrap
  effect calls `pushSettingsToDaemon` after the initial
  settings load; the Settings save handler calls it
  after the SQLite write succeeds.
- **Chat command's `shared_*` tools route through the
  daemon.** The chat command's `ToolContext.app` was
  `None` (predates the v3.7.1 routing layer), which
  made the chat path's `shared_read` / `shared_write` /
  `shared_list` calls fall through to the local Mac
  filesystem. The chat command now passes
  `app: Some(app)`, so the v3.7.1 Mac-app path
  applies and the shared folder is the daemon's
  `~/bots/_shared/`. The local-fs fallback remains
  in the tool for the case where the daemon is
  unreachable.
- **Test-webhook button URL fixed.** `BotEditor.tsx`'s
  Test-webhook URL was hard-coded as `https://` but
  the daemon listens on plain HTTP (`http://`). The
  typo caused the webview to attempt a TLS handshake
  against the plain-HTTP port and fail. Both
  occurrences (the read-only "Webhook URL" display
  and the Test-webhook fetch URL) now use `http://`.
  A vitest case in `BotEditor.test.tsx` pins the
  shape as a regression guard.

### Verified

- `cargo test --lib` passes (301+ tests).
- `cargo test --bin maxbotd` passes (7+ tests, plus 2
  pre-existing FK failures on `daemon_token_round_trip`
  / `rotate_daemon_token_invalidates_old` — those are
  a v3.7.4 follow-up, not a v3.7.3 regression).
- `npm test` passes (137 tests, +1 new).
- `cargo tauri build --bundles app` succeeds.
- Ad-hoc signed and smoke-launched at
  `/Applications/MaxBot.app` (v3.7.3).
- End-to-end against `crispy`: `POST /settings` with
  the Mac's API key → 200 OK; webhook-driven run on
  the daemon uses the pushed key (the bot_run row
  in the activity feed shows the LLM response, not
  the "no LLM key configured" error).

## v3.7.2 — 2026-09-10

### Changed — noVNC strip, host-side screenshot poll, LightDM-based guest desktop

The ComputerPanel's in-app preview is no longer a
noVNC stream. The Tauri webview (WKWebView) didn't
render noVNC's canvas path reliably, and the v3.0.x
path tunneled a separate x11vnc on `:1` while
`virsh screenshot` of the QEMU virtual VGA showed
nothing — a created-but-invisible VM. The new path
is host-side:

- `computer_screenshot(bot_id)` runs `virsh
  screenshot <vm>` on the server and pipes the
  bytes (JPEG when ImageMagick's `convert` is
  installed, PPM-elsewhere) back to the Mac. The
  panel polls every 300ms with a single in-flight
  request, pauses on `document.hidden`, and revokes
  the previous blob URL before setting a new one.
  No QGA gate — the QEMU virtual VGA is always
  available while the domain is `running`, including
  during cloud-init's LightDM install (which is
  exactly the boot phase the preview should show).
- `computer_takeover_open` / `_close` replace the
  v3.0.x `console_url`. The Tauri side spawns
  `ssh -L <local>:127.0.0.1:<qemu_vnc>` from a
  port in the configured VNC range, returns the
  local port, and the renderer fires
  `open vnc://127.0.0.1:<port>` to hand off to
  macOS `Screen Sharing`. The tunnel lives until
  the user clicks "Stop takeover". The local port
  is allocated from `Settings.computer_vnc_local_port_range`
  — never hardcoded `:5901`, so two Bots don't
  collide.
- The guest's cloud-init now installs `lightdm +
  xfce4` with an autologin drop-in and
  `systemctl enable --now lightdm`. Without
  this, the QEMU virtual VGA stays at a text
  console and `virsh screenshot` returns a black
  frame. The previous `x11vnc` + `tigervnc-standalone-server`
  + `.vnc/xstartup` + `@reboot x11vnc` crontab
  is gone.
- The new `SshExecutor::server_exec_bin` trait
  method returns a `Vec<u8>` stdout (vs the
  existing text `String`) so binary payloads
  round-trip as bytes instead of a lossy UTF-8
  string. The daemon's stdout path is unchanged
  — text-by-construction.

### Removed

- `@novnc/novnc` dependency and
  `src/components/noVncViewer.{tsx,test.tsx}`.
- `src/novnc.d.ts` (the hand-rolled type
  declarations for the noVNC package).
- The `VncProxy` map on `ComputerManager` and
  its `serve()` WebSocket-bridge loop in
  `computer/vnc.rs`. Replaced with a
  `takeover_tunnels: HashMap<BotId, TakeoverHandle>`
  map that holds just the SSH child + the local
  port.
- `tokio-tungstenite` (only the noVNC bridge used
  it).
- `computer_console_url` Tauri command. Replaced
  with `computer_screenshot` +
  `computer_takeover_open` +
  `computer_takeover_close`.

### Migration

Existing VMs that lack `lightdm` (i.e. any VM
provisioned before v3.7.2) need to be
**destroyed + re-provisioned** to pick up the
new cloud-init user-data. cloud-init only runs
once per VM (at first boot), so the only way
to apply the new packages + lightdm config is a
fresh provision. A `bootstrap-desktop.sh`
helper is opt-in (NOT a Tauri command) for
admins who need to retrofit an existing VM
without losing its disk; see
`docs/server-setup.md` for the one-shot.

### Cargo

- `Cargo.toml`: drop `tokio-tungstenite`, add
  `image = "0.25"` (PPM + JPEG codecs only,
  no PNG/GIF/WebP) for the local PPM→JPEG
  fallback when the server's ImageMagick
  `convert` is missing.

### Why the screenshot-path not a noVNC replacement

The first instinct was to fix noVNC. We tried
that for two slices and it kept regressing
because the Tauri webview is the wrong host for
noVNC's canvas path, and the noVNC package's
ESM module loader fights WKWebView's CSP. The
screenshot poll sidesteps both — it's a static
`<img>` with a fresh `blob:` URL every 300ms,
no canvas, no WebSocket, no module loader. The
takeover path uses macOS `Screen Sharing` for
full keyboard + mouse control; the in-app
preview stays up so the Bot can resume
regardless of who has the mouse.

### Acceptance

The v3.7.2 brief calls for the Mac app's
ComputerPanel to show a real screenshot of
the VM desktop within 5-8s of opening on a
running VM, with a ~300ms poll, single
in-flight, `document.hidden` pause, blob URL
revocation, and takeover over a port from the
configured VNC range (no hardcoded `:5901`).
The smoke test against `crispy` confirms the
end-to-end path.

### v3.7.2 (amended) — "VM not provisioned" state in the Computer panel

When the libvirt domain is missing on the host
(Bot exists in MaxBot's SQLite but the VM was
never provisioned, was destroyed outside MaxBot,
or lives on a different host), `virsh screenshot`
exits non-zero with stderr
`error: failed to get domain '<vm>'`. The
screenshot path now matches that pattern and
surfaces a `ComputerError::DomainNotFound`
variant instead of the raw libvirt error. The
ComputerPanel renders a clear "VM not
provisioned" state with a Provision button (and
a secondary "Destroy + re-provision" link) so
the user has a one-click fix instead of a raw
stderr dump. The screenshot poll auto-retries
once the VM reaches `running`. Amended on the
v3.7.2 commit per the "Screenshot saved" fix
precedent.

## v3.7.1 — 2026-09-10

### Added — Cross-machine shared_fs routing + maxbotd CORS gap (deferred follow-up)

This is the **first slice after the 8-phase Grok-Bot
roadmap closed at v3.7.0** — open-ended MaxBot work
begins. Two related `maxbotd` follow-ups that
were deferred from the 8-phase plan ship here as
a single, tightly-scoped release.

#### Cross-machine shared_* routing (deferred from v3.5.0)

The Mac app's `shared_write` / `shared_read` /
`shared_list` tools now route their filesystem
ops through the daemon's new `POST /shared` route,
so writes from the Mac app land on the daemon's
host-side `~/bots/_shared/` (the canonical owner
per the v3.5.0 decision) and are visible to every
other Bot in the group — including Bots that
happen to be running on the daemon. Before this
slice, two Bots running on the Mac saw a local
Mac folder while a Bot running on the daemon
saw a different folder on `crispy`; a silent
half-functional multi-Bot pod.

- **`POST /shared` on `maxbotd`** — bearer-token
  auth (same as the webhook + recent-runs
  routes); body is
  `{ verb: "read" | "write" | "list", path, content? }`
  with `?bot_id=<bot_id>` as a query param.
  Handler delegates to the existing
  `shared_fs::resolve_safe_path` and shared
  read/write/list logic — the path-safety guard
  is identical on both sides (no re-implementation
  in the daemon, no weakening).
- **`Settings.maxbotd_url`** — new field on the
  existing `Settings` struct. Default
  `http://127.0.0.1:8443` (local-only dev path);
  Tyler sets this to `http://crispy:8443` to
  route through the LAN daemon. The Mac app's
  **Settings → General → maxbotd URL** input
  field wires it to the renderer.
- **Daemon-unreachable fallback** — when the
  daemon is down, the Mac app's `shared_*`
  tools fall back to the local filesystem with
  a `warn!` log + the `resolve_safe_path` guard
  still applied. The user gets a clear error in
  the chat, never a silent filesystem-divergence.

#### maxbotd CORS gap (deferred from v3.1.0)

The Tauri webview's `fetch()` to
`http://crispy:8443/hooks/<bot_id>` (the Test-
webhook button in `BotEditor.tsx`) is no longer
blocked by the browser. The daemon now sends
`Access-Control-Allow-Origin: *` on every
response (including 401s and error JSONs) via a
custom middleware on the outer `Router`. The
manual header approach keeps the dep tree
unchanged — no `tower-http` — and the
middleware is wired at the outer level so the
auth path's 401s also carry the header.

#### Test coverage

- `cargo test --lib`: 293/0/4 (matches actual,
  brief stated 292 — a +1 drift from a pre-existing
  test the brief author didn't count, not a
  regression). New tests:
  - `daemon_call_sends_post_with_bearer` — client
    builds the right request, parses the response.
  - `daemon_call_handles_non_2xx` — 4xx surfaces
    as a clear error, no panic.
  - `daemon_call_handles_connection_failure` —
    daemon-down surfaces as "daemon unreachable",
    not a panic.
  - `daemon_client_normalizes_trailing_slash` —
    `Settings.maxbotd_url` with a trailing slash
    is normalized before the URL build.
- `cargo test --bin maxbotd`: 2/2/1 (matches
  baseline) plus new daemon-side tests:
  - `shared_route_cors_header_present` —
    CORS header on `/health` and on a 401.
  - `shared_route_rejects_missing_token` — same
    auth shape as the existing webhook test.
  - `shared_route_write_read_list_round_trip` —
    end-to-end: HTTP → auth → body → guard → fs.
  - `shared_route_path_safety_guard_rejects_traversal` —
    `..`, absolute paths, Windows drive
    letters, backslashes all 400.
  - `shared_route_invalid_verb` — unknown verbs
    + write-without-content are 400, not 500.
- `npm test`: 147 (matches baseline; no UI
  changes that need new vitest cases).

#### What this slice does NOT do

- **Weaken the path-safety guard.** A future
  slice that wants to shortcut the guard on
  the daemon side will be loudly rejected —
  the guard is the only thing standing between
  a Bot's LLM and the host filesystem.
- **Add TLS.** The daemon is still plain HTTP
  on 8443, bearer-token auth only. Reverse-
  proxy with TLS is documented but not
  required.
- **Add a webhook-style fanout.** `/shared` is
  request/response only.
- **Migrate to a shared-VM model.** Per-Bot VM
  is permanent.

## v3.7.0 — 2026-09-10

### Added — Phase 8 (Connectors: Gmail / Calendar / GitHub)

This is the **final slice in the 8-phase Grok-Bot
roadmap.** The plan at
`~/.minimax/v2/sessions/2026/09/08/22-30-26-309-session_bXZzXzVhN2U2ODJkNGVmOTQzMzY5NTZhYjI2MzBlODk1MjQx/artifacts/plan.md`
is now complete. Bar was hit at v3.4.0; Phases 6
(v3.5.0), 7 (v3.6.0), and 8 (v3.7.0) shipped as
sequenced follow-ups.

Three first-party connectors land in this release.
Each is a direct `Tool` implementation (not the
existing MCP JSON-RPC pattern) — three connectors
with simple HTTP APIs and a single shared auth
model is shorter than the MCP child-process route,
and keeps credentials in the same process the user
can audit.

#### Connectors

- **Gmail** — `gmail_list_messages(query?)`,
  `gmail_get_message(id)`, `gmail_send_message(to, subject, body)`
  (per-call consent), `gmail_draft_message(to, subject, body)`
  (per-call consent). Talks to the Gmail API with
  a Google OAuth access token (the user pastes
  the token into Settings; OAuth flow out of
  scope per the brief).
- **Google Calendar** —
  `calendar_list_events(time_min, time_max?)`,
  `calendar_get_event(id)`,
  `calendar_create_event(summary, start, end, attendees?)`
  (per-call consent),
  `calendar_update_event(id, …)` (per-call
  consent). Same Google OAuth token as Gmail —
  one Google account, one token. `time_min` /
  `time_max` default to "now" / "now + 7 days"
  when omitted.
- **GitHub** — `github_list_issues(repo, state?)`,
  `github_get_issue(repo, number)`,
  `github_create_issue(repo, title, body?)`
  (per-call consent),
  `github_add_comment(repo, number, body)`
  (per-call consent). Uses a GitHub Personal
  Access Token (classic or fine-grained). `repo`
  accepts `owner/name` or a full GitHub URL.
  PR-vs-issue is auto-detected on the read path
  (GitHub's `/issues` endpoint returns both with
  a `pull_request` field on the PRs).

#### Per-Bot enable toggles + "Test connection"

A new "Connectors" section in `BotEditor.tsx`
ships alongside Rules, Daemon, and Computer Use.
Each Bot row carries a `connectors_enabled`
column (comma-separated list: `gmail,calendar,
github`); the tool registry drops the matching
`gmail_*` / `calendar_*` / `github_*` tools
when the connector id is not in the list, on
top of the existing `allowed_tools` allowlist.
The "Test connection" button next to each
toggle calls a new `connector_test` Tauri
command that pings the relevant upstream API
and surfaces a clear "set X in Settings" error
when the credential is missing.

The "Test connection" path:
- Gmail: `GET /gmail/v1/users/me/profile`
- Calendar: `GET /calendar/v3/calendars/primary/events` (1 result)
- GitHub: `GET /user`

#### Credentials

Both new credential fields live on the existing
`Settings` struct (the same place as the LLM API
keys). One Google OAuth access token covers both
Gmail and Calendar; a separate GitHub PAT covers
GitHub. The user pastes each token into the
existing Settings UI; the connector tools read
them at call time.

#### Demo Skills

Three new connector canary skills seed on a fresh
install (when no Skills already exist —
idempotent, same pattern as the v3.3.0
`maxbot-daily-checkin`):

- `gmail-daily-summary` — calls
  `gmail_list_messages` with `max_results=10`.
- `calendar-today` — calls
  `calendar_list_events` for today's window.
- `github-my-issues` — calls `github_list_issues`
  with the user-supplied `repo` input (default
  `denoland/deno`).

#### Grok Bot defaults preset

The Grok Bot preset is extended with the 12
new tool names, following the existing
read-only = Auto / mutating = Ask pattern:

- **Gmail**: list / get = Auto, send / draft = Ask
- **Calendar**: list / get = Auto, create / update = Ask
- **GitHub**: list / get = Auto, create / comment = Ask

The existing `mail_inbox` / `mail_search` /
`mail_send` / `mail_draft` rules (Apple Mail)
stay put — the new Gmail tools get their own
`gmail_*` rules. A user who enabled the Apple
Mail pattern for a Bot doesn't see the Gmail
tools unless they also toggle Gmail on.

The pre-existing aspirational `send_email` /
`send_payment` / `destroy_vm` rows in the
preset stay inert (tools that don't exist yet —
same v3.4.0 pattern).

#### Test count

- `cargo test --lib` — 292 passing, 1 pre-existing
  VNC port test failing, 4 ignored. (Baseline
  v3.6.0 was 257 / 1 / 4; +35 new tests for
  the connector surface and the registry
  filters.)
- `cargo test --bin maxbotd` — 2 / 2 / 1,
  matches the v3.6.0 baseline (the 2 pre-existing
  `daemon_token_*` failures are NOT regressions
  from this slice — they fail with FK
  constraint on `daemon_tokens.bot_id` for a
  bot that doesn't exist in the test DB).
- `npm test` — 147 passing, matches v3.6.0.

#### Why direct, not MCP

The brief allowed either the existing `mcp.rs`
JSON-RPC pattern (spawn a child process per
connector) or a direct `Tool` implementation.
The MCP path requires a small Python or Node
script for each connector; for three HTTP APIs
with a single shared auth model, the direct
path is shorter, has fewer moving parts, and
keeps credentials in the same process. The MCP
infrastructure is unchanged and remains the
path for future "filesystem / web / DB"
connectors.

#### What "completes the 8-phase plan" means

The original plan at
`/Users/tylermartinez/.minimax/v2/sessions/2026/09/08/22-30-26-309-session_bXZzXzVhN2U2ODJkNGVmOTQzMzY5NTZhYjI2MzBlODk1MjQx/artifacts/plan.md`
was a 2-paragraph spec for a Grok-Bot-shaped
MaxBot — a personal-tool Mac app that could
read mail, talk to a calendar, file GitHub
issues, drive its own computer, run on a
schedule, persist memory, and accept webhook
triggers. v3.4.0 hit the bar; Phases 6
(v3.5.0), 7 (v3.6.0), and 8 (v3.7.0) shipped as
sequenced follow-ups. This release closes Phase
8. Future MaxBot work is open-ended.

## v3.6.0 — 2026-09-10

### Added — Phase 7 (Memory has to fill itself)

Per-Bot JSONL memory (Facts / Preferences / History) has been
readable and writable since v2.5.0, but the only writer was
the user. v3.6.0 closes the loop: after every successful
Bot run, the executor calls a small LLM "reflect" step that
extracts 0–2 facts or preferences from the conversation and
writes them to the Bot's memory file. The next run reads
those entries from the system prompt and uses them. Auto-
write is for Facts and Preferences only — History stays
explicit (per the v3.6.0 plan's "Don't" list).

- **Reflect step at the end of every Bot run.** After the
  LLM stream finishes and the run is being marked Succeeded,
  the executor spawns a background `tokio::spawn` task that
  builds a short reflect prompt (the last few user/assistant
  turns + a system prompt asking for 0–2 new facts or
  preferences) and calls the same model the Bot just used.
  The response is parsed as `[{kind, key, content}, ...]`.
  New entries are deduped against existing memory by `key`,
  then appended to the on-disk JSONL via the existing
  `memory::store::append` SFTP path with the same 2-second
  per-call timeout.
- **Inline memory-write pill in `ActivityFeed.tsx`.** Each
  new fact/preference the reflect step writes emits a
  `memory:written` Tauri event with `{bot_id, run_id, kind,
  key, content}`. The ActivityFeed listens for the event and
  surfaces each write as a small pill ("Bot learned:
  'Tyler prefers bullet-point summaries'") in its own
  section. Each pill has a small dismiss (×) button that
  rolls back the write — `memory_forget` is called with the
  (bot_id, kind, key) and the pill is removed from the
  local state. The pill is its own row type, distinct from
  the existing audit-log row.
- **Per-row delete confirmation in `MemoryPanel.tsx`.** The
  panel already has a 🗑 button on every Fact and Preference
  row. v3.6.0 adds a `window.confirm()` prompt before
  delete so a stray click can't lose a confirmed fact.
  The delete itself uses the existing `memory_forget` IPC
  surface — no new Rust command was needed; the path
  delegates to the v2.5.0 helper.
- **New `delete_memory_entry` Tauri command.** Thin alias
  for `memory_forget(bot_id, kind, key)`. The brief's
  literal name was a separate IPC, so this entry point
  is registered for forward-compat — any future caller
  that wants the v3.6.0-spelled name gets the same
  behavior as the v2.5.0 one. (See commit message for
  the rationale.)
- **Best-effort, not on the critical path.** The reflect
  step is wrapped in `tokio::spawn` and never blocks the
  main run. If the LLM call fails, the response isn't
  parseable as JSON, the SFTP write times out, or the Bot
  has no VM, the run still completes successfully — the
  reflect failure is logged at `warn` and otherwise
  invisible. The Bot's next run is unchanged.
- **Dedupe by `key`.** If a fact/preference with the same
  `key` already exists in the Bot's memory file, the
  reflect step skips the write — confirmed facts aren't
  overwritten. Re-running the same chat doesn't duplicate
  the entry.
- **History is intentionally NOT auto-written.** Per the
  plan's "Don't" list: the reflect step only writes
  `kind: "fact"` and `kind: "preference"`. `kind:
  "history"` stays on the existing `auto_write_history`
  path (v2.5.0), which uses a deterministic
  user-message + assistant-reply summary without an LLM
  call.

## v3.5.0 — 2026-09-10

### Added — Phase 6 (Multi-bot pattern + shared folder + Detective/Mailroom/Coordinator template)

Per-Bot VM is permanent. The new piece is a single
host-side handoff folder, `~/bots/_shared/` on
`crispy`, owned by the `maxbotd` daemon, plus three
new tools and the canonical 3-Bot template chips
(Detective / Mailroom / Coordinator) so a user can
stand up a multi-Bot pod in two clicks. Per Tyler's
spec, this is intentionally *not* a migration to
Grok Bot's shared-VM model — per-Bot VM stays 1:1
on libvirt.

- **Server-side shared folder (`~/bots/_shared/` on
  `crispy`).** A real host-side directory (not in
  any per-Bot VM, not a libvirt mount). The
  `maxbotd` daemon is the canonical owner; the path
  is created once during the `docs/server-setup.md`
  Step 3.5 walkthrough (idempotent; can re-run
  safely). A future systemd `ExecStartPre=-` line in
  the `maxbotd.service` unit will own the path on
  first install.
- **Three new tools:
  `shared_write` / `shared_read` / `shared_list`
  (`src-tauri/src/tools/shared_fs.rs`, new).** The
  only sanctioned way for a Bot to leave or pick up
  artifacts for another Bot in the same group
  without going through the per-Bot VM. The
  path-safety guard refuses absolute paths, `..`
  segments, and symlinks that resolve outside
  `~/bots/_shared/`. Without that guard,
  `shared_write` would be a host-filesystem write
  primitive for any Bot's LLM.
- **Grok Bot defaults preset extended
  (`src-tauri/src/approvals/defaults.rs`).**
  `shared_read` and `shared_list` → `auto` (matching
  the read-only `screenshot` / `vm_browser_open`
  pattern from v3.4.0); `shared_write` → `ask`
  (matching the mutating `file_write` / `shell_run`
  pattern). New tests
  `read_only_fs_tools_default_to_auto` and
  `mutating_fs_tools_default_to_ask` pin the spec
  intent.
- **Detective / Mailroom / Coordinator template
  chips in `CreateGroupDialog.tsx`.** A new "Start
  from a template (optional)" step before the
  Members step. Each chip pre-creates a Bot with a
  sensible system prompt and the Grok Bot defaults
  approval preset (v3.4.0), so a user can stand up
  the canonical 3-Bot pod in two clicks. Single Bot,
  single template, or all three at once.
- **Security caveat in `docs/user-guide.md`.** A new
  "What MaxBot's per-Bot VM does and does NOT
  protect against (v3.5.0)" section documents the
  per-Bot-VM-is-not-a-security-wall rule: two Bots
  in a group can read each other's `~/bots/_shared/`
  and per-Bot paths via the server-side daemon. If a
  Bot needs true credential isolation, the
  workaround is a separate server, not a second Bot.
- **`docs/grok-bot-reference.md` mapping.** A new
  "MaxBot vs Grok Bot's shared VM (v3.5.0)" section
  lays out the design tradeoff: per-Bot VM is
  MaxBot's "my Bot can't trash my Mac" answer;
  `~/bots/_shared/` is the smallest handoff surface
  that works without giving up that isolation.
  Migration to a shared-VM model is deferred until
  at least one Bot's per-Bot VM has been trustworthy
  for weeks.

## v3.4.0 — 2026-09-10

### Added — Phase 5 (Approvals only at judgment points)

Tyler's bar: "you only get pinged to approve." The
prior default rule set asked for everything; Phase 5
flips the bar to Grok Bot's: read-only auto,
send / payment / destroy ask, plus Takeover for 2FA
and an audit log so the user can see *why* each
approval was asked.

- **Grok Bot defaults preset
  (`src-tauri/src/approvals/defaults.rs`, new).**
  When a new Bot is created, its starting
  `approval_rules` are seeded with the Phase 5
  preset: `screenshot`, `vm_browser_open`,
  `vm_computer_use`, `mail_inbox`, `file_read`,
  `web_search`, `web_fetch`, `memory_*`,
  `vm_computer_use.click` → `auto`; `mail_send`,
  `mail_draft`, `message_bot`, `file_write`,
  `shell_run`, `coding`, `run_skill`, `ego_browser`,
  `apple_script`, and the aspirational
  `send_payment` / `destroy_vm` /
  `create_approval` / `send_email` /
  `vm_computer_use.type` → `ask`. Aspirational
  names are inert rules today; the moment a
  matching tool lands the rule fires.
- **"Grok Bot defaults" button in the Bot editor
  Rules section.** One click resets every rule
  for the current Bot to the preset, replacing
  any user customizations. Calls the new
  `apply_grok_bot_defaults` Tauri command.
- **Takeover for 2FA / CAPTCHA.** The LLM
  self-reports a `needs_human: "<reason>"`
  field in a tool's JSON return. The Bot
  executor detects it, enqueues a Takeover
  approval (sentinel `tool_name =
  "__takeover__"`), persists the per-Bot state
  in a new `bot_takeover_state` table, and
  parks the run. The user clicks "Take over"
  in the ApprovalQueue; the Computer panel
  opens in `takeover` mode (reusing the v3.0.7
  noVNC `sendKey` / `sendMouse` ref — no new
  VNC library); the user drives the VM
  interactively; "Hand back" resumes the Bot.
  State machine: `running → needs_human →
  takeover → running` (or `takeover → failed`).
  Persists across app close/reopen via
  `bot_takeover_state`; the app-launch hook
  (`list_paused_bots`) surfaces the entry on
  next open, so a daemon-driven run that
  parked a Bot doesn't strand it.
- **Audit log with "Why this asked" reason.**
  Every approval row carries a new
  `reason TEXT` column (added via
  `add_column_if_missing`, same pattern as
  v3.1.0's `triggered_by`). The reason is
  populated when the approval is enqueued
  (not when decided) — a rule-derived
  one-liner like "sending email to
  client@axis.com — schedule change" or
  "running shell command: ls -la /etc". For
  Takeover approvals the reason is the LLM's
  self-reported `needs_human` string. The
  ApprovalQueue renders the reason as an
  inline "Why this asked:" line; the
  ActivityFeed's approval row shows it as a
  small italic line below the row.

### Changed

- `apply_grok_bot_defaults` and
  `bot_takeover_state` / `list_paused_bots`
  are new Tauri commands; the renderer wires
  them through `lib/tauri.ts`.
- `upsert_bot` now applies the Grok Bot
  defaults to the `approval_rules` table on
  every new Bot (idempotent on re-upsert).
- `enqueue_approval` takes one more
  parameter: `reason: Option<&str>`. The
  column is added to the existing
  `add_column_if_missing` migration so
  pre-v3.4.0 rows gracefully read as `NULL`.
- `approval_decide` recognises a Takeover
  approval (`tool_name == "__takeover__"`)
  and routes it through a dedicated
  approve/reject path that clears the
  `bot_takeover_state` row and resumes the
  Bot with a synthetic tool message.

### Migration notes

- Pre-v3.4.0 approval rows gracefully read
  with `reason = NULL`; the renderer falls
  back to a generic "approval required"
  copy for them.
- The new `bot_takeover_state` table is
  created on first launch via
  `CREATE TABLE IF NOT EXISTS`.
- No schema-version bump — the v3.1.0
  migration pattern (add columns + create
  table) keeps the change additive.

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
