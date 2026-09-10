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

Bots are first-class citizens — they live in the sidebar as a **Bot
roster** (the primary surface), each gets a per-bot folder on disk,
and they can message each other through a shared inbox. In v2.0, each
Bot can also have its own **Linux computer** (a QEMU/KVM VM on a
server you control) — see [Per-Bot computers](#per-bot-computers) below.

The Bot roster replaces the v1.0 conversation list as the sidebar's
primary list. Conversations are scoped per-Bot.

### Create a bot

1. In the sidebar, click <kbd>+ New Bot</kbd>.
2. Fill in:
   - **Name** — what shows in the sidebar. Specialist-Bot templates
     below suggest good single-responsibility names.
   - **Description** — one-line summary, surfaces in tooltips
   - **System prompt** — the bot's persona + instructions. The
     specialist-Bot templates below fill this in for you.
   - **Default model** — overrides the global default if you want a
     different model for this bot
   - **Allowed tools** — pick from the tool list; the bot can only
     invoke the ones you check
   - **Icon + color** — visual identifier in the sidebar
3. (Optional) Check **Provision a computer** to give this Bot its
   own Linux VM (see [Per-Bot computers](#per-bot-computers)). Set
   the disk size (default 10 GB) and RAM (default 2048 MB).
4. Save. The bot appears in the roster immediately.

### Specialist-Bot templates

The Bot editor has four one-click specialist templates. Click a chip
to fill in the name placeholder + system prompt:

- **Figma Specialist** (placeholder: "Figma Bro") — uses the Figma
  desktop app and plugins to design screens, components, and
  prototypes. Hands work back when a design is ready for review.
- **Code Reviewer** (placeholder: "Devbot") — reads diffs, runs
  tests, and surfaces issues. Hands work back when the review is
  ready.
- **Researcher** (placeholder: "Detective") — uses the browser,
  web search, and file tools to gather and synthesize information.
  Hands work back when the research is ready.
- **Inbox Triage** (placeholder: "Mailroom") — uses mail.app,
  calendar.app, and reminders.app to sort, schedule, and follow up
  on messages. Hands work back when the queue is processed.

You can edit any field after picking a template before saving.

### Per-Bot computers

Each Bot can have its own Linux VM on a server you control. The
server runs QEMU/KVM + libvirt; MaxBot drives it over SSH. The VM has
an XFCE desktop and an SSH-accessible shell. The Bot drives its own
computer via tool calls (`shell_run`, `file_read`, `file_write`);
the human operator can see what the Bot is doing at any time.

#### One-time server setup

See `docs/server-setup.md`. Run that once on the Linux box you want
to use, then come back to MaxBot's **Settings → Computer** tab and
fill in:

- **Server host** — the IP or hostname (e.g. `192.168.0.49`)
- **Server SSH user** — your non-root user on the server
- **Server SSH key id** — leave blank for now (uses your Mac's
  default `~/.ssh/id_ed25519`)
- **VNC local port range** — the range MaxBot uses for `ssh -L`
  tunnels (default `5900–5999`)
- **Default per-Bot RAM** (default 2048 MB) and **disk** (default 10 GB)
- **Computer passphrase** — used to encrypt per-Bot SSH keypairs.
  Set once, used for every new Bot.

Click **Test connection** to confirm MaxBot can talk to the server.

#### Provisioning a Bot's computer

When you check **Provision a computer** in the Bot editor and save,
MaxBot:

1. Generates a new Ed25519 keypair for the Bot (encrypted at rest)
2. SSHes to the server and runs `virt-install` with a cloud-init
   seed ISO that injects XFCE, LightDM, qemu-guest-agent, and your
   per-Bot SSH public key
3. Polls the libvirt DHCP lease to find the VM's IP
4. Marks the Bot's `computers.state = running` (or `error` on failure)

Provisioning takes 30–60 seconds for the VM to boot and get an IP.
The first-boot `apt-get install` of LightDM + XFCE takes 3–5 minutes
in the background — the VM is usable (SSH-able, qemu-guest-agent
responding) long before the desktop is fully ready.

#### Three access levels (Status / Preview / Takeover)

Once a Bot has a running computer, three ways to see what it's doing:

- **Status** — a small chip in the title bar that turns purple
  while the VM is active. Always on.
- **Preview** — a pinned side panel (~30% width) showing a fresh
  JPEG of the QEMU framebuffer, polled by the host every 300ms
  (`virsh screenshot` on the Linux server, piped back to the Mac
  over the existing SSH connection). The Bot can keep driving
  while you watch. **No in-VM VNC server runs in v3.7.2+** — the
  Preview is a single JPEG that refreshes, not a live RFB stream.
- **Takeover** — full mouse + keyboard control via TigerVNC
  (v3.7.5+; macOS Screen Sharing mis-handles `VNC_AUTH_NONE` and
  prompts for a password, so we ship TigerVNC instead). MaxBot
  opens an `ssh -N -L <port>:127.0.0.1:<qemu-vnc> <user>@<host>`
  tunnel and hands the renderer `vnc://127.0.0.1:<port>`. The
  panel mounts in `takeover` mode with two footer buttons:
  - **Hand back** — the 2FA happy path. Decides the gating
    approval as `approved` and the Bot's run resumes on the next
    turn. (See [`docs/2fa-walkthrough.md`](2fa-walkthrough.md)
    for the full flow.)
  - **Stop now** (v3.7.5+) — the abort path. Decides the gating
    approval as `rejected` and (best-effort) calls `stop_bot_run`
    to halt the executor. The run row ends up `Failed`.

The loopback URL never contains the server's IP/hostname — both
Preview and Takeover bind 127.0.0.1 only, so a process on the Mac
that can reach localhost can't accidentally reach the VM's
framebuffer.

For the canonical 2FA walkthrough (Gmail → 2FA prompt →
takeover → solve → hand back → Bot continues), see
[`docs/2fa-walkthrough.md`](2fa-walkthrough.md).

#### Tool routing

When a Bot has a computer, these tools route through SSH to the VM
instead of running locally on the Mac:

- `shell_run` — runs the command in the Bot's VM via SSH exec
- `file_read` / `file_write` — SFTP into the Bot's VM
- (Other tools — `ego_browser`, `apple_script_*`, etc. — still run
  on the Mac. v2.0 only routes the Unix-y tools; future slices will
  expand this.)

If a Bot does **not** have a provisioned computer, all tools run
locally on the Mac (the v1.0 behavior).

### Edit / delete

Click a bot in the roster to reopen the editor. Delete is a
destructive action that wipes the bot's folder on disk **and
destroys its VM** (if any).

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

### First morning you should see a run (v3.3.0)

The Re-record + Last-run loop is the canary surface for the
"do it once → correct it → save → schedule" workflow. To
verify it end-to-end on a fresh install:

1. **Open the Skills tab** in the side panel. You'll see a
   single seeded skill called <code>maxbot-daily-checkin</code>
   — a placeholder <code>web_fetch</code> step that ships
   with the app so the new Re-record and Last-run
   affordances have something to point at out of the box.
2. **Click Run** on that row. A run row lands in
   <kbd>skill_runs</kbd> (durable summary) and a trace row
   in <kbd>skill_run_traces</kbd> (per-step output). The
   Last-run expand on the skill row will show: timestamp,
   duration in seconds, the per-step tool call, success
   status, and the trigger input.
3. **Click Re-record** to open the recorder preloaded with
   the skill's existing JSON. Change the URL, the tool, or
   the args — whatever makes the skill useful to you. Click
   <kbd>Update</kbd> to save in place. The skill's id and
   any bot schedule pointing at it are preserved.
4. **Open the Routines tab** and add a routine bound to
   that skill (e.g. cron <code>0 8 * * *</code>). The
   routine row gets a <kbd>View run</kbd> link that
   navigates back to the Skills panel.
5. **Wait until 8am the next morning.** The schedule fires,
   the run lands in <kbd>Activity</kbd> and
   <kbd>skill_runs</kbd>, and the trace row in
   <kbd>skill_run_traces</kbd> shows the bot's
   tool-call history for that run.

#### Real-world follow-up: a TOC canary skill (Phase 8)

The seeded <code>maxbot-daily-checkin</code> skill is a
placeholder — the real canary surface is a
<code>check-active-roster-for-missing-schedule</code>
skill that calls the AxisCare API to look for shifts
without a Caregiver assigned. That skill needs the
<code>axis-api</code> MCP server wired into MaxBot's MCP
registry, which is Phase 8 (not Phase 4). When Phase 8
ships, the existing Skills panel + Re-record + Last-run
flow is the surface the new MCP-backed skill will plug
into — no UI changes needed.

Tyler's <code>axis-api</code> skill at
<code>~/.minimax/skills/axis-api</code> is the canary
source for the real skill's HTTP shape (the
<code>/clients/:id/schedule</code> endpoint, the
<code>Authorization: Bearer &lt;token&gt;</code> header,
the JSON response shape). Phase 8 will read that skill's
docs and translate them into MaxBot's
<code>(tool, args)</code> step schema.

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

## What MaxBot's per-Bot VM does and does NOT protect against (v3.5.0)

A per-Bot Linux VM gives every Bot a private computer to drive
— its own filesystem, its own shell history, its own cookies
and logins. That is a real isolation boundary **between the Bot
and the rest of your Mac**, and between a Bot and the rest of
the host. It is **not** a security boundary between two Bots in
the same group.

**What the per-Bot VM does protect against:**

- One Bot's runaway shell command or compromised browser
  session can NOT touch the host filesystem, your Mac's
  `~/Library`, or another Bot's VM.
- A Bot can't read your `~/.ssh`, your `~/.aws`, your macOS
  keychain, or any other Mac-local secret (the Bot's tools
  run on its own VM, not on your Mac).
- A Bot's `apt install` / `systemctl` / `cron` only affect
  its own VM.

**What the per-Bot VM does NOT protect against:**

- **Two Bots in a group can read each other's stuff.** Both
  Bots can reach the server-side paths that hold their
  per-Bot data (`~/bots/<bot_id>/` on the host) and the
  shared folder (`~/bots/_shared/`, see
  [Shared folder](#shared-folder-v350) below). If a
  Detective and a Mailroom are in the same group, Mailroom
  can `shared_read` every handoff Detective left in
  `shared/`, and Detective can `shared_read` every draft
  Mailroom wrote.
- **The shared folder is intentionally not isolated.** It's
  the whole point — Bots in a group need to leave artifacts
  for each other. If you put credentials in `shared/`,
  every Bot in the group can read them.
- **The host is shared by all Bots on the same server.** The
  libvirt host (`crispy` in this guide) is a real Linux box;
  any Bot that can run shell on the host through a quirk
  sees the whole host filesystem. Today, no Bot tool
  reaches the host shell directly — all Bot-to-host access
  is mediated by the `maxbotd` daemon. The path-safety
  guard in `shared_*` is the second line of defense; the
  daemon is the first.
- **Per-Bot VM is per-Bot domain on the host, not per-Bot
  user.** The libvirt domain is the isolation unit, NOT a
  Linux user account. A per-Bot Linux user account would be
  a real security boundary (separate UIDs, separate file
  permissions, separate `sudo`). MaxBot doesn't ship that
  today. If a Bot needs true credential isolation, the
  workaround is to run it on a separate server entirely
  (see the [Grok Bot reference](grok-bot-reference.md) for
  why MaxBot hasn't migrated to a shared-VM model and what
  the per-Bot-VM model does and doesn't give you).

**The rule of thumb:** per-Bot VM = "my Bot can't trash my
Mac." It is NOT "my Bot can't see what my other Bot is
doing." If a Bot needs to handle a credential that should
not be readable by any other Bot in the group, that Bot
belongs on its own server (or you manually sandbox it
with a separate Linux user — out of scope for MaxBot
today).

## Shared folder (v3.5.0)

A single host-side directory at `~/bots/_shared/` on the
server is the cross-Bot handoff surface. The `maxbotd`
daemon owns the path; the Mac app's `shared_read`,
`shared_write`, and `shared_list` tools are the only
sanctioned way for a Bot to leave or pick up artifacts for
another Bot in the same group without going through the
per-Bot VM. Three rules:

- **Read is `Auto` in the Grok Bot defaults preset.** A
  Bot can pick up a handoff note without asking.
- **Write is `Ask` in the Grok Bot defaults preset.**
  Every `shared_write` is gated on a single human
  confirmation.
- **The path-safety guard is load-bearing.** The tool
  refuses absolute paths, `..` segments, and symlinks that
  resolve outside `~/bots/_shared/`. Without that guard,
  `shared_write` would be a host-filesystem write primitive
  for any Bot's LLM. See
  `src-tauri/src/tools/shared_fs.rs` for the implementation
  and tests.

This folder is the closest thing MaxBot ships to Grok
Bot's "shared VM" model. See
[grok-bot-reference.md](grok-bot-reference.md) for the
mapping.

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
