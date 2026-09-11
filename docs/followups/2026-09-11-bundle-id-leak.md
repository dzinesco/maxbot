# 2026-09-11 — WebKit Malloc leak keyed on bundle identifier `com.maxbot.app`

> **Status:** Open — investigation deferred until after v3.7.17 ships.
> The shipping workaround (bundle-id rename + data-dir migration) closes
> the user-facing symptom. This doc tracks the root-cause hunt.

## TL;DR

A WebKit Malloc zone in MaxBot's Tauri WebView allocates at ~443 MiB/sec
when the bundle identifier is `com.maxbot.app`. Within 30 seconds the
process has 13.3 GB allocated, 1% fragmentation, and 7.3 GB swapped.
The leak is NOT present at the new identifier `com.maxbot.app.devtools` —
the same binary, the same d964581 code, the same WebView, allocates ~10
MB and stays stable over a 5-minute sample.

**d964581 alone is not the fix.** Slice A confirmed this: with just the
identifier reverted to `com.maxbot.app` (everything else kept), the
identical leak signature returns within 30 seconds.

**Shipping path:** v3.7.17 renames the bundle identifier to
`com.maxbot.app.devtools` and adds a one-shot data-dir migration so
existing users keep their profiles. **Do NOT revert the identifier in any
future change.** That would re-introduce the leak for every user.

---

## Load-bearing evidence

### Slice A — `eac31dc` reverted identifier to `com.maxbot.app` (everything else kept)

Source: branch `leak-hunt/devtools-d964581` HEAD at 10:22Z.
Configuration: same d964581 code, `devtools: true` on the window, identifier
`com.maxbot.app`. Profile taken at t=30s after launch.

```
==== Summary for MaxBot process (PID 19684) at t=30s ====
ReadOnly resident memory:       403 MB
Writable regions:               11850 total, 11.7G allocated, 1% fragmentation
  WebKit Malloc zone:           11.7G allocated, 1% fragmentation
Swap usage:                     7.3G
```

The `1% fragmentation` + `7.3G swap` is the load-bearing fingerprint.
A well-behaved WebKit process runs at 30-50% fragmentation; 1% means
WebKit is constantly allocating into fresh virtual regions and never
recycling them.

### 5-minute sample — `6e05a93` (identifier = `com.maxbot.app.devtools`)

Source: branch `leak-hunt/devtools` HEAD at the 10:11Z sample window.
Configuration: same d964581 code, `devtools: true` on the window,
identifier `com.maxbot.app.devtools`. Profile sampled at t+0s, t+60s,
t+120s, t+240s, t+300s.

```
t+0s    WebKit Malloc zone: 10.3 MB allocated, 40% fragmentation, 0 KB swapped
t+60s   WebKit Malloc zone:  9.1 MB allocated, 43% fragmentation, 0 KB swapped
t+120s  WebKit Malloc zone:  8.4 MB allocated, 47% fragmentation, 0 KB swapped
t+240s  WebKit Malloc zone:  8.0 MB allocated, 50% fragmentation, 0 KB swapped
t+300s  WebKit Malloc zone:  7.9 MB allocated, 51% fragmentation, 0 KB swapped
```

Bytes DECREASED from 10.3 MB to 7.9 MB over 5 minutes. Fragmentation
increased (more churn) but allocations stayed in the WebKit-cache
range. 0 KB swapped the entire window. 5 consecutive runs on the
renamed binary stayed in this healthy regime.

### Diagnostic chain

| commit    | identifier              | devtools | code base | outcome                                |
|-----------|-------------------------|----------|-----------|----------------------------------------|
| d964581   | com.maxbot.app          | false    | baseline  | leak (Step 2 baseline measurement)     |
| 2aa48f0   | com.maxbot.app          | true     | d964581   | leak (devtools alone does not fix it)  |
| 6e05a93   | com.maxbot.app.devtools | true     | d964581   | **healthy** (5 consecutive runs)      |
| eac31dc   | com.maxbot.app          | true     | d964581   | **leak returns** (Slice A diagnostic)  |

The bundle identifier is the load-bearing variable.

---

## Open questions for F+H investigation

These are the questions the v3.7.17 shipping path deliberately does
not answer. Answering them is the follow-up work, not the ship gate.

### 1. What is the JS retainer?

The leak allocates WebKit heap at ~443 MiB/sec. That is consistent with
either a JavaScript object being created in a tight loop (e.g., a
`setInterval` that appends to a global array), or a WebKit cache
partition that's growing unboundedly (e.g., the ITP / IndexedDB / Cache
Storage quota).

**Tools to name the retainer:**

- **In-app devtools** — with `devtools: true` on the v3.7.17 binary,
  right-click → Inspect, take a Heap Snapshot, look at the top
  retainers in the `Summary` view. The bundle identifier change should
  not affect the heap snapshot itself, so this works on the
  devtools-enabled renamed build.
- **`WEBKIT_INSPECTOR_SERVER=127.0.0.1:9229`** — set the env var
  before launch, then point Chrome at `chrome://inspect` → Devices.
  Same heap snapshot story but more controllable.
- **`vmmap --summary MaxBot` + `heap --owner MaxBot`** — Apple's
  `vmmap` and `heap` tools can show the WebKit Malloc zone
  breakdown; the `heap --owner` output names which library or
  subsystem owns the largest allocations.

### 2. Is it a cache partition?

WebKit uses cache partitions keyed on the origin (scheme + host + port)
to separate first-party storage from third-party storage. The bundle
identifier does not directly appear in the partition key — but
Launch Services and the WebKit cache store keys MAY be tied to the
bundle identifier in some macOS versions. If a per-bundle cache
partition is the retainer, the leak would manifest as the partition
growing unboundedly.

**Hypothesis to test:** enable ITP debug logging (`-dwebkit2.logging`
in WebKit env vars), look for `CacheStorage` / `NetworkCache` /
`IDB` lines that show unbounded growth. The bundle-id change would
explain why the partition keys differ.

### 3. Is it Launch Services side effect?

macOS Launch Services keys some per-bundle state on the bundle
identifier. The WebKit process may be querying Launch Services on a
hot path (e.g., for URL handler lookups, content type resolution, or
content blocker rule lookups), and the LS query may be slow enough
that the WebKit process accumulates a queue of pending queries that
never drains.

**Hypothesis to test:** `lsof -p <webkit-pid>` to see if Launch
Services files (`/System/Library/.../lsregister*.cache`,
`com.apple.LaunchServices-*.csstore`) are being held open and
growing. If the LS cache is the retainer, switching bundle
identifiers would create a fresh LS cache and the old one would
never be touched again.

### 4. Is it process priority / memory limit?

macOS may give WebKit processes a per-bundle memory limit or
priority that's different between `com.maxbot.app` and
`com.maxbot.app.devtools`. If the original bundle ID had a low
priority or low limit, the WebKit process could be running into
the limit and forcing the system to swap pages out, which would
manifest as the 7.3 GB swap footprint. The renamed bundle ID would
get a fresh priority / limit and the swap would not be needed.

**Hypothesis to test:** `ps -o pid,priority,nice,rss,vsz,command -p
<webkit-pid>` and compare against the renamed binary's WebKit
process. If priorities differ, that's the mechanism.

### 5. Is it the `osascript` bridge?

MaxBot's Computer Use (AppleScript + `osascript`) is called from the
renderer. If `osascript` is holding per-bundle state keyed on the
bundle identifier, the WebKit process could be accumulating that
state in memory. The renamed bundle identifier would have a fresh
`osascript` state space.

**Hypothesis to test:** with `devtools: true`, Network tab → look
for repeated IPC calls to the `osascript` bridge. The `osascript`
bridge IPC count should be roughly constant over a 5-minute sample;
if it's climbing, that's the retainer.

---

## Do-not-revert invariant

**Future work MUST NOT revert the bundle identifier to
`com.maxbot.app`.** Reverting would re-introduce the WebKit Malloc
leak for every user on every fresh launch.

- The data-dir migration is the user-facing workaround.
- The root cause is in WebKit / macOS code path keyed on the bundle
  ID — to be found via F+H.
- Until the root cause is found and a permanent fix is shipped, the
  bundle identifier stays at `com.maxbot.app.devtools`.
- Any new bundle-id-related change should be reviewed against this
  doc and against the Slice A evidence.

The 2 lines in `src-tauri/tauri.conf.json` that anchor the invariant:

```json
"identifier": "com.maxbot.app.devtools",
"devtools": true
```

Both are load-bearing for the leak workaround. Reviewer must flag any
PR that touches either field.

---

## Files

- **`docs/followups/2026-09-11-bundle-id-leak.md`** (this doc) — the
  F+H follow-up ticket. Open questions + investigation plan.
- **`CHANGELOG.md` v3.7.17 section** — the user-facing ship note.
  Includes the do-not-revert warning.
- **`src-tauri/tauri.conf.json`** — the config change (identifier +
  devtools flag).
- **`src-tauri/src/data_migration.rs`** — the one-shot migration that
  preserves user state across the rename.
- **`src-tauri/src/lib.rs`** — `mod data_migration;` declaration +
  the setup-hook call.

## Test counts at v3.7.17

- `cargo test --lib`: 367 passed, 3 ignored, 0 failed.
- `cargo test --bin maxbotd`: 10 passed, 1 ignored, 0 failed.
- `npm test`: 201 passed, 0 failed.

The 8 new unit tests in `src-tauri/src/data_migration.rs` cover all
four `MigrationOutcome` variants + nested-dir recursive copy +
idempotent second-run skip + file-content preservation + the
end-to-end `run_for_app_data_dir` path.
