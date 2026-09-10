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

> v3.7.9 note: the "Take over" action is in-app — click
> the Computer panel's Drive button and use your trackpad
> + keyboard. The approval flow's Hand back / Stop now
> semantics are unchanged (Hand back = approval(approved),
> Stop now = stop_bot_run + approval(rejected)). The
> previous external-viewer-plus-SSH-tunnel path is gone;
> see [`docs/first-bot-20-minutes.md`](first-bot-20-minutes.md)
> for the new end-to-end flow.

## What you'll see in the UI

1. The approval lands in the Approvals queue with the
   row text "needs human: {triggering_tool} — 2FA prompt
   detected on {site}".
2. The row's primary button is **Take over** (not
   Approve). Approve / Reject still appear as the
   secondary row, but for a takeover the primary path is
   "Take over → click Drive → solve in panel → hand back".
3. Clicking **Take over** mounts the ComputerPanel in
   preview mode already driving (the per-Bot driving
   flag is set by the parent before mount, so the user
   lands in the panel able to interact with the VM
   without an extra click).
4. The panel's existing JPEG preview IS the interactive
   surface — move your trackpad, type, click on it; the
   events go to xdotool over the existing SSH connection.
   Login if the screen is locked, then navigate to the
   2FA prompt. No external viewer opens, no password
   prompt.
5. The 2FA prompt is on whatever device or app the service
   expects (your phone for TOTP, your hardware key for
   WebAuthn, etc.). Solve it the way you normally would.
6. Once the service accepts the second factor and the
   Bot's tool call can complete, click **Hand back** in
   the driving banner. That decides the approval as
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

- The in-panel click-through rides the existing
  `SshPool.vm_exec` pipe the rest of the Computer
  surface already uses — there is no separate SSH
  tunnel, no external VNC viewer, and no extra port
  to manage. The `xdotool` script is rendered on the
  Rust side and the input commands go to the VM's
  X11 session over SSH.
- The server (`crispy`, `192.168.0.49`) is reachable
  over SSH from the Mac's keychain agent. No extra
  setup is needed once Settings → `computer_server_host`
  and Settings → `computer_use_default_ssh_key` are set.
- The per-Bot driving flag on the Rust side is the
  only auth gate. While the flag is set, the Bot's
  `vm_computer_use` tool refuses — so even if a
  botched click happens on the Bot's side, no
  conflicting input can be injected.

## Edge cases

### "Drive" but no input is accepted

The per-Bot driving flag is the source of truth. If
clicking the JPEG doesn't drive the VM, the
`computer_input_open` Tauri command likely failed or
the screenshot poll hasn't picked up the new
framebuffer dimensions yet. Check the MaxBot log
(Settings → About) for the underlying error. Re-click
Drive; the in-banner Hand back button is the way to
release the flag.

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
current state. The driving flag is cleared. To shut
down the VM cleanly, use the **Stop** button in the
ComputerPanel's toolbar (not the approval row's Stop now).
That calls `computer_stop` (clean ACPI shutdown via
`virsh shutdown`).

### Takeover panel won't close

The takeover modal-overlay's click handler is a no-op
intentionally (so the user has to use the buttons
explicitly, otherwise the Bot would stay paused). If
both buttons are unresponsive, close the panel via
<kbd>⌘</kbd>+<kbd>W</kbd> — the approval row will
still be pending and you can decide it from the
Approvals queue
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
