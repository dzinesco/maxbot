# `maxbotd` — install + verify

v3.1.0 · the always-on daemon for MaxBot Bots.

## What this is

`maxbotd` is a small Rust binary that ships in the MaxBot Tauri repo
(`src-tauri/src/bin/maxbotd.rs`, ~840 lines). It runs on your Linux
server — `crispy` at `192.168.0.49` — and keeps your Bots alive when
the Mac app is closed. It does two things, on a loop:

1. **Scheduler poll** — every 30 s, walk `bot_schedules` for anything
   due and call the same `run_bot_once(...)` the in-app scheduler
   uses, but with `AppHandle = None` (no Tauri events). The resulting
   `bot_runs` row is the source of truth; the Mac app shows it in
   Activity on next open / poll.
2. **Webhook receiver** — `POST /hooks/<bot_id>` on
   `0.0.0.0:8443`. Bearer-token auth per Bot, persisted in
   `daemon_tokens`. Returns `202 Accepted` with `{ bot_run_id }` and
   fires the Bot asynchronously with the webhook body as a synthetic
   user message.

Together these two paths prove the Phase 2 verification flow:
**"close the Mac app, scheduled routine still fires"** — and
"inbound webhook still works when the Mac app is closed."

## One-time install (on `crispy`)

The binary is built alongside the Tauri app on your Mac and copied
over. Two paths: `systemd` (recommended for "set and forget") or
`tmux` / `screen` (for ad-hoc poking).

### Build the binary on the Mac

```bash
cd /Users/tylermartinez/dev/maxbot/maxbot
cargo build --release --manifest-path=src-tauri/Cargo.toml --bin maxbotd
# Output: src-tauri/target/release/maxbotd
scp src-tauri/target/release/maxbotd crispy:/opt/maxbot/maxbotd
```

### `systemd` unit (recommended)

Place the binary at `/opt/maxbot/maxbotd` and the database at
`/var/lib/maxbot/maxbot.db` (the daemon uses whatever path you pass
to `--db`; we standardize on `/var/lib/maxbot/maxbot.db` to keep
`/opt` and `/var/lib` separated, the same pattern most distros
follow for `daemon` vs `data`).

```ini
# /etc/systemd/system/maxbotd.service
[Unit]
Description=MaxBot always-on daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/opt/maxbot/maxbotd --db /var/lib/maxbot/maxbot.db --bind 0.0.0.0:8443
Restart=on-failure
RestartSec=5
User=maxbot
Group=maxbot
# Hardening (defense in depth; maxbotd is a single-tenant daemon
# for Tyler's personal use, but the limit guards keep a runaway
# process from taking the box down).
LimitNOFILE=65536
MemoryMax=512M
TasksMax=512
# Logs go to journald; tail with `journalctl -u maxbotd -f`.
StandardOutput=journal
StandardError=journal
SyslogIdentifier=maxbotd

[Install]
WantedBy=multi-user.target
```

Then on `crispy`:

```bash
# Create the unprivileged user the daemon runs as.
sudo useradd --system --home /var/lib/maxbot --shell /usr/sbin/nologin maxbot

# Make the data dir; the daemon must be able to create / write
# the SQLite file there. Mounting the dir with `noatime` is
# optional but cuts down on unnecessary IO on a busy box.
sudo mkdir -p /var/lib/maxbot
sudo chown maxbot:maxbot /var/lib/maxbot
sudo chmod 750 /var/lib/maxbot

# Reuse the same SQLite file the Mac app uses. Copy it over
# from the Mac (the daemon only needs to read + append; the
# schema is owned by the Tauri app's `migrate()` call).
# Initial seed (one-time):
scp ~/Library/Application\ Support/com.maxbot.app/maxbot.db \
    crispy:/var/lib/maxbot/maxbot.db
sudo chown maxbot:maxbot /var/lib/maxbot/maxbot.db
sudo chmod 640 /var/lib/maxbot/maxbot.db

# Activate the unit.
sudo systemctl daemon-reload
sudo systemctl enable --now maxbotd
sudo systemctl status maxbotd
```

The daemon opens the SQLite file in WAL mode (the existing
`Database::open` in `src-tauri/src/storage/db.rs` sets
`journal_mode = WAL`), so the Mac app can keep writing to the
same file via SMB / NFS / `sshfs` while `maxbotd` is running. WAL
keeps readers and writers from blocking each other on the same
host; for cross-host, prefer `sshfs` (no locking issues) over SMB
(the SQLite lock file races on some SMB server implementations).

### `tmux` / `screen` fallback (ad-hoc)

If you don't want to install `systemd` (you're on a non-systemd
distro, or you just want to poke at it for 5 minutes), the binary
runs as a normal foreground process:

```bash
# In a tmux session:
ssh crispy
tmux new -s maxbotd
sudo -u maxbot /opt/maxbot/maxbotd \
    --db /var/lib/maxbot/maxbot.db \
    --bind 0.0.0.0:8443
# Ctrl-b d to detach; tmux attach -t maxbotd to come back.
```

`screen` works the same way (`screen -S maxbotd …`,
`Ctrl-a d` to detach). The daemon doesn't daemonize itself
(it stays in the foreground), so the `tmux` / `screen` /
`systemd` wrapper is what keeps it alive across SSH logouts.

## Verify the daemon is up

Liveness probe — no auth required, no DB hit:

```bash
curl -s http://crispy:8443/health
# → {"ok":true,"version":"3.1.0"}
```

The version field comes from `env!("CARGO_PKG_VERSION")` at
compile time, so a stale binary shows up immediately. If the
endpoint is unreachable, the daemon isn't running; check
`journalctl -u maxbotd -e --no-pager` (systemd) or reattach your
tmux session to see the boot log.

## Connect a Bot

1. Open the Mac app → **Bots** → click a Bot (or create one).
2. In the **Daemon** section, click **Generate**. The button
   rotates to a fresh 32-byte hex bearer token, writes it to
   `daemon_tokens` on the shared SQLite file, and copies it to
   the clipboard.
3. The **Webhook URL** field above the token shows the full URL
   the daemon will accept — e.g. `https://crispy:8443/hooks/<bot_id>`.
4. From any host on the same network, confirm the endpoint
   accepts a `POST`:

   ```bash
   TOKEN="<paste from clipboard>"
   curl -i -X POST \
       -H "Authorization: Bearer $TOKEN" \
       -H "Content-Type: application/json" \
       -d '{"text":"hello from the docs"}' \
       http://crispy:8443/hooks/<bot_id>
   # Expect: HTTP/1.1 202 Accepted
   #         {"bot_run_id":"...","status":"accepted"}
   ```

5. (Optional) confirm the run lands in the DB by hitting the
   new `GET /bots/<id>/recent_runs?limit=5` route with the same
   token:

   ```bash
   curl -s -H "Authorization: Bearer $TOKEN" \
       "http://crispy:8443/bots/<bot_id>/recent_runs?limit=5" \
       | jq '.[0]'
   # → { "id": "...", "bot_id": "...", "status": "succeeded",
   #      "started_at": "...", "triggered_by": "webhook", ... }
   ```

## Run a test from the Mac app

The **Daemon** section has a **Test webhook** button right next to
**Generate / Rotate / Copy**. Clicking it:

1. Reads the current token via `getDaemonToken(bot_id)`.
2. POSTs a small JSON body to the configured webhook URL with
   `Authorization: Bearer <token>`.
3. Shows the response in an inline status pill — `202 — Bot run
   started ({"bot_run_id":"..."})` on success, the error body
   (or a CORS / network error from the webview) on failure.

The run lands in the Mac app's **Activity** feed within ~5 s
with a blue `via webhook` badge. If it doesn't:

- **No status appears** — the Mac app's webview blocked the
  `fetch()` (the daemon is plain HTTP and sends no CORS headers).
  As of v3.7.1 the daemon sends
  `Access-Control-Allow-Origin: *` on every response, so the
  Test-webhook button works end-to-end from the renderer. If
  the button still fails after v3.7.1, open the DevTools
  network panel and check the actual response — a 4xx / 5xx
  is a real auth or route issue, not CORS.
- **`401 invalid token`** — click **Rotate** in the Mac app to
  generate a fresh token; the old one is invalidated immediately.

## Verify "close the app, work continues"

This is the headline Phase 2 flow. From the Mac app:

1. **Set up a 1-minute routine** — Bots → click a Bot → set
   "Every 1 minute" in the schedule editor → Save.
2. **Note the time** — the next run is at the next minute boundary
   (the scheduler snaps to wall-clock, not "now + 60 s").
3. **Quit the Mac app** — `Cmd+Q`, NOT just minimize. The Mac
   process exits; the daemon is on a different machine and is
   unaffected.
4. **Wait for the routine to fire** — the daemon's 30 s scheduler
   tick picks it up. The bot runs to completion on `crispy`. The
   `bot_runs` row is written.
5. **Reopen the Mac app** — open the **Bots** panel or the
   **Activity** sidebar. The run appears with a `via daemon` badge
   (the v3.1.0 column `triggered_by` is the proof — it's set to
   `"daemon"` only by the `maxbotd` scheduler path, never by the
   in-app scheduler).

If the run doesn't appear after ~90 s:

- Check the daemon logs: `journalctl -u maxbotd -e --no-pager`.
  You should see `maxbotd: scheduler tick — N schedule(s) due`
  followed by `maxbotd: firing scheduled bot <id>`.
- Confirm the SQLite path the daemon is using matches the path
  the Mac app is reading from. They MUST point at the same file
  (or at least the same shared mount). The `ExecStart=` line
  tells you what the daemon is using; the Mac app's settings
  show what it's writing to.
- If the schedule is configured but `interval_seconds = 0` and
  the cron expression is empty, the scheduler will skip it. The
  `bot_schedules` row's `interval_seconds` / `cron_expression`
  columns are the two trigger paths; the editor writes whichever
  one is set.

## Troubleshooting

| Symptom | Likely cause | Fix |
| --- | --- | --- |
| `bind 0.0.0.0:8443 failed: address in use` | Another process (often an old `maxbotd` from a prior tmux session) is on 8443. | `sudo ss -tlnp 'sport = :8443'` to find the PID, then kill it. Or pass `--bind 0.0.0.0:8444` to this one. |
| `curl /health` returns nothing | Daemon isn't running, or the firewall on `crispy` is blocking 8443. | `systemctl status maxbotd` (or `tmux attach -t maxbotd`). `sudo ufw status` if `crispy` is using UFW. |
| `401 invalid token` | The token was rotated (or the Bot was recreated) since the curl / Test-webhook call was issued. | Click **Rotate** in the Mac app → copy → retry. |
| `404 no bot with that id` | The `<bot_id>` in the URL doesn't match any row in `bots`. | The Mac app's BotEditor shows the full webhook URL with the right id; copy from there. |
| `500 db error` | The SQLite file at `--db <path>` is not writable by the `maxbotd` user, or it's a directory, or it doesn't exist and the parent dir isn't writable. | `ls -l /var/lib/maxbot/maxbot.db` and `ls -ld /var/lib/maxbot`. The `maxbot` user needs `rw` on the file and `rwx` on the dir. |
| Scheduler ticks fire but the bot errors with `set your MiniMax API key` | The shared SQLite file's `settings` row is on the Mac, not on `crispy`. The Mac's `Settings` panel writes the API key to its local DB; `crispy` has the schema but not the row. | Either: (a) seed `crispy`'s DB with the same settings row via the Mac app's "Export settings" (future), or (b) set the API key via a one-time `INSERT` on `crispy` directly, or (c) move the API-key storage to an env file the daemon reads at boot. Tracked as v3.1.1 polish. |
| QGA not ready (Bot VM hasn't booted yet) | A scheduled run fires before the per-Bot VM has finished provisioning. | The first run after a fresh Bot creation is allowed to fail; the next tick (≤30 s later) succeeds. If it keeps failing, check the VM state in **Computer** panel. |
| Test-webhook button shows `request failed: TypeError` | Tauri webview CORS — fixed in v3.7.1; the daemon now sends `Access-Control-Allow-Origin: *` on every response. If the button still fails after v3.7.1, check the browser network panel for a 4xx / 5xx and look at `journalctl -u maxbotd`. | No action needed; the CORS gap is closed as of v3.7.1. |

## Reference

- Source: `src-tauri/src/bin/maxbotd.rs` (v3.1.0, ~840 lines)
- Phase 2 plan: `~/.minimax/v2/sessions/2026/09/08/22-30-26-309-session_bXZzXzVhN2U2ODJkNGVmOTQzMzY5NTZhYjI2MzBlODk1MjQx/artifacts/plan.md`
- v3 grand tour: `docs/v3-grand-tour.md`

## v3.5.0 — Cross-Bot shared folder

The daemon is the canonical owner of
`~/bots/_shared/` on the host — the host-side
directory Bots in a group use to hand work to each
other via the Mac app's `shared_read` / `shared_write`
/ `shared_list` tools. The path-safety guard inside
`src-tauri/src/tools/shared_fs.rs` is the load-bearing
security piece; the daemon is the second line of
defense.

Create the directory once on `crispy` (idempotent):

```bash
ssh crispy 'mkdir -p ~/bots/_shared && chmod 0775 ~/bots/_shared'
```

For the full rationale + the per-Bot-VM-is-not-a-
security-wall rule, see
[`user-guide.md`](user-guide.md#what-maxbots-per-bot-vm-does-and-does-not-protect-against-v350)
and [`grok-bot-reference.md`](grok-bot-reference.md#maxbot-vs-grok-bots-shared-vm-v350).
The path itself is documented in

## v3.7.1 — Mac-app routes through the daemon

The Mac app's `shared_write` / `shared_read` /
`shared_list` tools now route through the daemon
(via the new `POST /shared` route), so a write from
the Mac app lands on the daemon's host-side
`~/bots/_shared/` and is visible to every other Bot
in the group. Without this, two Bots running on the
Mac saw a local Mac folder while a Bot running on
the daemon saw a different folder on `crispy` —
a silent half-functional multi-Bot pod.

### The `maxbotd_url` setting

The Mac app's **Settings → General → maxbotd URL**
field tells the Mac app where to send the
`POST /shared` calls. Default
`http://127.0.0.1:8443` (local-only — the daemon
runs on the Mac for dev). Set to
`http://crispy:8443` to route through the LAN
daemon. Field name in code: `Settings.maxbotd_url`
in `src-tauri/src/storage/db.rs`. The Mac app
trims a trailing slash and falls back to the
default if the field is empty.

### The `POST /shared` route

Auth: same `Authorization: Bearer <token>` as the
webhook + recent-runs routes. The token is verified
against the per-Bot `daemon_tokens.bot_id` row. The
Bot id is sent as `?bot_id=<bot_id>` so the daemon
can look up the right token row.

Body shape (JSON):

```json
{
  "verb":    "read" | "write" | "list",
  "path":    "relative/path/under/shared/",
  "content": "..."                 // required for "write"
}
```

Responses:

- `write` → `200 { "ok": true, "bytes_written": N, "path": "..." }`
- `read`  → `200 { "content": "...", "size": N }` (or `400` for
  directories; `413` for files over 1 MB)
- `list`  → `200 { "entries": [{ "name", "is_file", "size", "modified_at" }] }`

The path-safety guard (`resolve_safe_path` in
`src-tauri/src/tools/shared_fs.rs`) is the same
function the in-app tool uses — the daemon is the
second line of defense, and the guard refuses
absolute paths, `..` segments, Windows drive
letters, UNC shares, backslashes, and symlink
escapes.

### CORS

Every daemon response (including 401s and 4xx
errors) now carries
`Access-Control-Allow-Origin: *` via a
middleware on the outer `Router`. The Mac app's
Tauri webview no longer blocks the **Test
webhook** button in the Bot editor or the
`POST /shared` calls from the in-app
`shared_*` tools. The header is added manually
(no `tower-http` dep) — Tyler's LAN is the only
client surface, and a future slice can lock it
down if the daemon is exposed beyond the LAN.

### Manual smoke test

```bash
TOKEN="<paste from Bot editor → Daemon → Copy>"
# List the root
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d '{"verb":"list","path":""}' \
    "http://crispy:8443/shared?bot_id=<bot_id>" | jq
# → { "entries": [ ... ] }

# Write a file
curl -s -X POST -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    -d '{"verb":"write","path":"hello.txt","content":"hi from the docs"}' \
    "http://crispy:8443/shared?bot_id=<bot_id>"
# → { "ok": true, "bytes_written": 19, "path": "..." }

# Confirm it landed on disk
ssh tyler@crispy 'ls ~/bots/_shared/'
# → hello.txt
```

### Fallback behavior

When the daemon is unreachable (network down,
daemon not running, `maxbotd_url` misconfigured),
the Mac app's `shared_*` tools fall back to the
local Mac filesystem with a `warn!` log + the
`resolve_safe_path` guard still applied. The
Bot's daemon token isn't required for the
fallback — the user just gets the local-fs view
of `~/bots/_shared/`, which may diverge from
the daemon's view. The chat shows a clear error
in the read / list case; writes succeed
locally and surface a `warn!` in the log.

[`server-setup.md`](server-setup.md#step-35--create-the-shared-folder-v350).
