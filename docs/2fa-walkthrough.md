# 2FA walkthrough

The canonical "human in the loop" path for MaxBot. When a Bot
hits a 2FA prompt inside its VM, the Bot's run is parked,
you take over the screen, solve the prompt, and hand the
VM back. This doc walks the path end-to-end.

If you only want a one-paragraph summary, see the
"Takeover" section of `user-guide.md`. If you want the
mechanics (what the daemon does, how the approval row
moves), read this doc.

## When this happens

A Bot hits 2FA any time it tries to sign in to a service
that uses TOTP, SMS, push, hardware key, or passkey as a
second factor. The most common cases in the current
template pack:

- Gmail / Google Workspace (TOTP, push, passkey)
- GitHub (TOTP, passkey)
- Bank / brokerage sites
- Anything behind Cloudflare Access
- Apple ID, Microsoft account

The Bot's `vm_computer_use` tool sees the 2FA prompt as
the page content and emits `needs_human: true` on the
tool return. That signal routes through
`approval_decide` (the existing approval row machinery
from v2.6.0) instead of running the next tool iteration.

## What you'll see in the UI

1. The approval lands in the Approvals queue with the
   row text "needs human: {triggering_tool} — 2FA prompt
   detected on {site}".
2. The row's primary button is **Take over** (not
   Approve). Approve / Reject still appear as the
   secondary row, but for a takeover the primary path is
   "Take over → solve in screen sharing → hand back".
3. Clicking **Take over** mounts the ComputerPanel in
   `takeover` mode (the same panel that powers the
   preview, but with extra footer buttons).
4. TigerVNC opens automatically (v3.7.5: switched from
   macOS Screen Sharing because Screen Sharing mis-handles
   `VNC_AUTH_NONE` and prompts for a password). You'll
   see the VM's desktop — login if the screen is locked,
   then navigate to the 2FA prompt.
5. The 2FA prompt is on whatever device or app the service
   expects (your phone for TOTP, your hardware key for
   WebAuthn, etc.). Solve it the way you normally would.
6. Once the service accepts the second factor and the
   Bot's tool call can complete, click **Hand back** in
   the ComputerPanel footer. That decides the approval as
   `approved` (synthetic tool result) and the Bot's run
   resumes on the next turn.

If you decide not to hand the Bot back — e.g. the
account is a one-time throwaway, or you realized the Bot
shouldn't be doing this — click **Stop now** instead.
That decides the approval as `rejected` and (best-effort)
calls `stop_bot_run` to halt the executor. The run row
ends up `Failed`.

## Two buttons, one panel

- **Hand back** — `approval_decide(approved)`. The
  gating tool's synthetic result is success. The Bot's
  agent loop continues, the next LLM turn gets the tool
  result, and the run keeps going. This is the 2FA happy
  path.
- **Stop now** — `stop_bot_run` (best-effort) +
  `approval_decide(rejected)`. The gating tool's
  synthetic result is `{"error":"denied by user"}`. The
  Bot's run ends. Use this when the 2FA was a mistake,
  the Bot went off-policy, or you want to take a
  different action.

Both buttons close the takeover panel.

## What "halts the executor" means

The Mac app's run-loop token (`CancellationToken` keyed
by the user message id, see `src-tauri/src/commands/chat.rs`)
gets cancelled. The agent loop checks the token between
every tool call and at every LLM stream chunk, so
cancellation is best-effort within a single tool call but
prompt at every iteration boundary. The run row in the
Activity feed shows `Failed` with reason "user stopped"
shortly after.

If the run started daemon-side (e.g. a scheduled run
that parked while the app was closed), there's no
`CancellationToken` in the Mac app to flip — the
approval row is decided as `rejected`, which is the
unblock for daemon-side runs in any case.

## Network & auth notes

- The takeover tunnel is `ssh -N -L <local>:<qemu-vnc>
  <user>@<host>`, loopback only. The Mac's TigerVNC
  connects to `vnc://127.0.0.1:<local>`. The server's IP
  is never in the loopback URL.
- The server (`crispy`, `192.168.0.49`) is reachable
  over SSH from the Mac's keychain agent. No extra
  setup is needed once Settings → `computer_server_host`
  and Settings → `computer_use_default_ssh_key` are set.
- TigerVNC connects without prompting for a password
  because the per-Bot VM's VNC is bound to
  `127.0.0.1` with no auth (qemu:commandline
  `password=off`). The SSH tunnel is the only auth gate.
- If you see TigerVNC ask for a password, the SSH
  tunnel didn't establish. Check the Mac's
  `~/Library/Logs/MaxBot/` for the
  `computer_takeover_open` log line.

## Edge cases

### "Take over" but no screen opens

The Mac's `open` command couldn't find TigerVNC, or the
SSH tunnel died. Open TigerVNC manually and connect to
`vnc://127.0.0.1:<port>` — the local port is shown in
the ComputerPanel's footer (VNC :PORT). The
`computer_vnc_local_port` setting controls the range;
the default is 5900-5999.

### "Hand back" doesn't resume the run

The approval row was decided, but the next LLM turn
didn't happen. Check:

1. The Bot's run row in Activity — if it's `Failed`,
   the agent loop errored, not the approval. Read the
   error column.
2. The Bot's `approval_rules` for the triggering tool —
   if a rule is `deny`, the approval won't fire
   again. The "denied by user" was terminal.
3. The `stop_message` token in `StreamRegistry` — if a
   `Stop` was clicked during the takeover, the run is
   dead until you `Run now` again.

### "Stop now" leaves the VM running

The executor is halted, but the VM itself is left in its
current state. The takeover tunnel is closed. To shut
down the VM cleanly, use the **Stop** button in the
ComputerPanel's toolbar (not the footer's Stop now).
That calls `computer_stop` (clean ACPI shutdown via
`virsh shutdown`).

### Takeover panel won't close

The takeover modal-overlay's click handler is a no-op
intentionally (so the user has to use the buttons
explicitly, otherwise the Bot would stay paused). If
both buttons are unresponsive, force-quit the
ComputerPanel with <kbd>⌘</kbd>+<kbd>W</kbd> on the
TigerVNC window — the approval row will still be
pending and you can decide it from the Approvals queue
in the next app launch.

## Related docs

- `user-guide.md` — Computer section, one-paragraph
  summary of takeover
- `user-guide.md` — Approvals section, the queue that
  surfaces 2FA
- `server-setup.md` — what gets installed on each Bot
  VM (LightDM + XFCE for the takeover screen)
- `maxbotd-setup.md` — the daemon that drives runs and
  the `Restart=on-failure` invariant
