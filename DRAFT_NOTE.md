# DRAFT_NOTE — overnight/smooth-2026-09-11

> **This branch is a DRAFT. Do not merge to main.**

## What this branch actually does

Eight commits of renderer lifecycle cleanup:

- Screenshot poll blob URL lifecycle hardening (bot switch, error, hidden)
- 300ms → 500ms default poll cadence
- Bytes-unchanged short-circuit when the QEMU framebuffer is static
- `React.memo` for MessageBubble + BotRoster
- VoiceToolbar level meter via direct DOM mutation (bypass React)
- Optimistic save in the Voice / STT settings palette section
- Computer footer error surface + voice `onError` routing
- v3.8.0 version bump + CHANGELOG entry

## What this branch does NOT do

It does **not** fix the v3.7.15 / v3.8.0 WebKit renderer
memory leak. The leak is in the WebKit Malloc zone
(JSC heap) at ~122 MB/sec per `vmmap --summary` on
the WebContent subprocess. Closing blob URL / RAF /
poll lifecycle holes makes the *surface* JS garbage
smaller, but the underlying retainer is WebKit's
internal allocation pattern.

A v3.8.0 build of this branch was installed at
`/Applications/MaxBot.app` on 2026-09-11 morning and
hit **2.2 GB / 91% CPU after 55 seconds**. Identical
failure mode to v3.7.15.

## What to do instead

The next slice needs a **devtools build**, not a refactor:

1. Add `"devtools": true` to `tauri.conf.json`'s
   `app.windows[0]`, build, install to a side path
   (`MaxBot-devtools.app`), **never** over the daily
   app pin.
2. Launch, open Safari → Develop → [device] →
   MaxBot WebView, take heap snapshots at t=0s,
   t=15s, t=30s with Computer preview open and
   closed.
3. The largest retained-size delta is the leak
   source. "We revoked blob URLs" is not a fix
   until the snapshot names a retainer.

## Why this is on origin

The previous session committed the work locally but
never pushed. Future agents should see the branch and
its caveat, not rediscover it from a local checkout.

PR #1 (multi-VM VNC port allocation + doc backfill +
the broken `up —` toolbar title fix) is the only
shipping work from the overnight sessions. Merge that
first; this branch sits.
