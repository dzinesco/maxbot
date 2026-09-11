# First 20 minutes with a Bot (Computer flow)

A guided tour of what the v3.7.5+ Computer surface looks like for a
new user with the Mac app installed, the daemon running, and one Bot
in the roster. The goal is to show you the current flow end-to-end
without reading every release note — the v3-grand-tour screenshots
are still pre-v3.7.2 (see the [stale-screenshots note][v3gt-note]),
so this doc is the canonical "show me the new flow" reference.

If you only want the prose, read top-to-bottom. If you want the
mechanics, jump to the section you need.

[v3gt-note]: v3-grand-tour.md#computer-panel

## What you need first

- The MaxBot.app installed and launched at least once
- The `maxbotd` daemon running on your Linux server (one-time setup:
  [`docs/maxbotd-setup.md`](maxbotd-setup.md))
- A Bot in the roster with a provisioned computer (see the
  [Computer section][user-guide-computer] of `user-guide.md` for the
  server + per-Bot-VM plumbing)

[user-guide-computer]: user-guide.md#per-bot-computers

The 20-minute timeline below assumes that stack is in place. If you
hit a problem, jump to the [Troubleshooting](#troubleshooting) tail
section or the [Related docs](#related-docs) at the bottom.

## 0:00 — Sidebar opens to a Bot roster, not a conversation list

The sidebar's primary surface is the **Bot roster** (it replaced
the v1.0 conversation list in v2.0). Each row has the Bot's avatar,
name, and a small status chip. The avatar animates through six
presence states: `idle / thinking / working / waiting / blocked /
done`. Click a row to select; the chat surface on the right scopes
to that Bot.

The status chip is the lightest-weight "what's it doing right now"
signal. To actually *see* what the Bot is doing inside its VM, you
need the Computer panel.

## 2:00 — Open the Computer chip and watch the Preview

Click the **Computer** chip on Robot (VNC 5901) in the title bar.
The Computer panel mounts in **Preview** mode by default: a pinned
side panel (~30% width) showing a fresh JPEG of the QEMU
framebuffer. The host polls `virsh screenshot` every 300 ms and
pipes the JPEG back to the Mac over the existing SSH connection,
so the Preview is a near-live view of the desktop.

You can read what the Bot is doing without interrupting it. The
Bot keeps driving the VM; you watch read-only.

There is no in-webview RFB canvas anymore. v3.7.2 replaced the
v3.0.x noVNC viewer with the host-side screenshot poll because the
noVNC bridge was a high-maintenance WebSocket↔RFB shim that
couldn't keep up at 300 ms cadence.

## 5:00 — Hand the Bot a site that needs 2FA

Tell the Bot to sign in to a service that uses TOTP, SMS, push,
hardware key, or passkey as a second factor. Gmail, GitHub, a bank
site — anything Cloudflare-Access-gated. (For the canonical Gmail →
2FA → takeover → solve → hand back walk-through, see
[`docs/2fa-walkthrough.md`](2fa-walkthrough.md).)

The Bot drives its own VM with the `vm_computer_use` tool. When the
page renders the 2FA prompt, the tool sees the prompt as the page
content and emits `needs_human: true` on the return. That signal
routes through `approval_decide` (the v2.6.0 approval row
machinery) instead of running the next tool iteration. The Bot's
run is **parked**, not aborted.

A new approval row lands in the Approvals queue with a **Take over**
primary button. That button is the only correct entry point for
2FA in the current model — the run will not resume until you click
**Hand back** or **Stop now** in the Computer panel.

## 8:00 — Click Drive

Click **Drive** in the Computer panel's toolbar. The banner reads
"You are driving — bot input paused". Move your trackpad / type /
click on the JPEG — the events go to xdotool over the existing SSH
connection. The bot's `vm_computer_use` is paused while you're
driving. Click **Hand back** in the banner when done. No external
VNC viewer; no password prompt.

The in-panel click-through replaces the previous
SSH-tunnel-plus-external-viewer path. There is no separate
`takeover` mode — the preview panel handles both the read-only
view and the click-through drive. The per-Bot driving flag is
set on the Rust side when you click Drive; the Bot's
`vm_computer_use` tool refuses while the flag is true.

Solve the 2FA in the VM. When you're done, click **Hand back** in
the banner. The approval is decided as `approved`, the per-Bot
driving flag is cleared, and the Bot's run resumes on the next
turn with a synthetic tool success.

## 12:00 — Decide not to hand back? Click Stop now

Sometimes you don't want the run to resume. The Bot went off the
rails, the 2FA prompt is actually a security warning, you need to
re-plan the task. v3.7.5 added a **Stop now** action on the
approval row, kept unchanged in v3.7.9.

**Hand back** and **Stop now** are not two ways to do the same
thing. They are two decisions:

- **Hand back** = the in-banner button while you're driving.
  Cascades to `approval_decide(approved)` + clears the per-Bot
  driving flag + the Bot's run resumes on the next turn. This is
  the 2FA happy path. The Bot's tool call returns success
  synthetically; the model treats the 2FA as solved and
  continues.
- **Stop now** = the approval row's reject button (v3.7.5
  semantics, unchanged). Cascades to `stop_bot_run(runId)`
  (best-effort) + `approval_decide(rejected)` + close the panel.
  The run row ends up `Failed`. The Bot's executor is halted.

`stop_bot_run` is best-effort on purpose. For daemon-parked runs
(started while the app was closed), the runId is not in
`activeRunByBot` — the `approvalDecide` alone unblocks the row,
which is the right outcome. The user-facing contract is: after
**Stop now**, the row is `Failed` and no more tool calls will
fire. (For the full breakdown of what "halts the executor" means
and the edge cases, see
[`docs/2fa-walkthrough.md`](2fa-walkthrough.md#hand-back-vs-stop-now).)

## 15:00 — Destroy a Bot you no longer need

Hover its row in the roster. A small **×** appears at the right of
the row (v3.7.6). Click it. A confirm dialog asks "Destroy Bot +
VM?" — the dialog is the same one used by the Bot editor's Delete
button, so the two entry points converge on the same cascade:

1. `computer_destroy` — `virsh undefine` + remove the cloud-init
   seed ISO and per-Bot storage on the server
2. `delete_bot` — drop the SQLite row

Both run in one cascade, so you can't end up with an orphaned VM
or an orphaned DB row. The Bot editor's Delete button delegates
to the same handler (it was rewired in v3.7.7 to match).

## 18:00 — Back up the Bot's memory before destroying it

Per-Bot memory (Facts, Preferences, History) lives on the Bot's
VM. Destroying the VM in step 15:00 deletes the per-Bot memory
files along with the rest of the VM's filesystem. If you want to
preserve memory — either to read it later, to seed a new Bot, or
to share it with the rest of the group — back it up first.

Open the **Memory** tab and click **Back up to shared/** (v3.7.8).
MaxBot collects the Bot's three JSONL files (Facts, Preferences,
History), concatenates them into a single JSONL stream, and writes
it to the host's `~/bots/_shared/memory/<bot_id>/<timestamp>.jsonl`
over the existing SSH connection (one round-trip, base64-encoded so
the file write is binary-safe).

A toast shows the absolute host path. Other Bots in the same group
can read the file with the `shared_fs` Bot tool — see the
[Shared folder section][user-guide-shared] of `user-guide.md` for
the underlying `shared_read` / `shared_write` / `shared_list` Bot
tools and the path-safety guard. The `kind` field is on every line
of the JSONL, so a future restore step can split by kind.

The button is enabled for any selected Bot, even one with no memory
yet — empty memory writes an empty file with the timestamp marker
so you get a clear "0 entries" toast instead of an error.

[user-guide-shared]: user-guide.md#shared-folder-v350

## Troubleshooting

A few specific failure modes the 20-minute timeline can surface:

- **Drive doesn't take effect** — the in-panel click-through
  requires the per-Bot driving flag to be set on the Rust side.
  If clicking **Drive** doesn't show the banner, the
  `computer_input_open` Tauri command likely failed. Check the
  MaxBot log (Settings → About) for the underlying error.
- **Hand back doesn't resume the Bot** — the most common cause is
  closing the panel without clicking Hand back in the banner. Find
  the row in the Approvals queue and click Hand back or Stop now
  explicitly. There is no "close the panel and abandon" affordance
  by design.
- **The Preview is a black screen** — the VM is still booting. The
  first-boot `apt-get install` of LightDM + XFCE takes 3–5
  minutes; the VM is SSH-able long before the desktop is up. Check
  `virsh console <bot>` on the server.
- **VM left running after Stop now** — expected. Stop now halts
  the executor; it doesn't `virsh shutdown` the VM. Destroy the
  Bot (15:00) to clean it up.
- **Memory backup wrote to a "wrong" path** — the host's `$HOME`
  is the SSH user's home, not the Mac's home. The toast shows the
  absolute resolved path; if it's `/home/tyler/bots/_shared/...`
  and you expected `/Users/tyler/...`, that's correct.

Per-Bot VMs are isolated to a libvirt domain (a QEMU/KVM
process), not a Linux user account — two Bots in the same group
can read each other's `~/bots/<bot_id>/` host-side paths. The
[isolation section][user-guide-isolation] of `user-guide.md` spells
out what the per-Bot VM does and does not protect against. For the
2FA daemon internals, see [`docs/2fa-walkthrough.md`](2fa-walkthrough.md).

[user-guide-isolation]: user-guide.md#what-maxbots-per-bot-vm-does-and-does-not-protect-against-v350

## Related docs

- [`docs/user-guide.md`](user-guide.md) — the prose reference for
  every surface (Bot roster, Computer section, Tools, Providers,
  Shared folder, isolation guarantees)
- [`docs/2fa-walkthrough.md`](2fa-walkthrough.md) — canonical
  end-to-end Gmail → 2FA → takeover → solve → hand back walk-through,
  with the "Hand back vs Stop now" decision table and the daemon
  internals
- [`docs/maxbotd-setup.md`](maxbotd-setup.md) — one-time `maxbotd`
  daemon install on the Linux server (the daemon is the only path
  to the host filesystem that Bots can reach, so this is what makes
  the shared folder and per-Bot memory paths work)
- [`docs/server-setup.md`](server-setup.md) — the libvirt / QEMU
  / cloud-init provisioning on the Linux server (per-Bot VM
  creation, Ed25519 keypair generation, the v3.7.2 noVNC removal
- The **Google Account** section of the settings palette (open with
  <kbd>⌘</kbd>+<kbd>K</kbd>, scroll to the bottom) handles the
  one-time OAuth setup for the Gmail + Calendar connector tools.
  Paste a Google Cloud project's Desktop OAuth client_id +
  client_secret, click **Connect Google**, complete the consent in
  the browser, and the refresh token is stored. See
  [user-guide.md#google-account-v3712](user-guide.md#google-account-v3712)
  for the full flow + revocation instructions.
- The **VoiceMode (v3.7.15)** mic in the chat header lets you
  dictate a message hands-free: click the <kbd>🎤</kbd>, speak,
  click again to stop, and the transcript auto-sends. It uses the
  MiniMax API key (already set in the **Providers → MiniMax**
  section of the settings palette) and macOS microphone permission
  (the first click prompts). See
  [user-guide.md#voicemode-v3715](user-guide.md#voicemode-v3715)
  for the full flow.
  notes)
