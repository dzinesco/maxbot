# OVERNIGHT_LOG — 2026-09-10 → 2026-09-11

Working branch: `overnight/2026-09-10` (off `main` @ `509ae66` = v3.7.15).

## Inventory (read in 60s)

### Versions
- `package.json`: 3.7.15
- `src-tauri/Cargo.toml`: 3.7.15
- `src-tauri/tauri.conf.json`: 3.7.15
- Working tree: clean on `main`, up to date with `origin/main`

### Test baseline (all green)
- `npm test`: 22 files, **194 passed**, 0 failed (vitest 5.0.0, 2.4s)
- `cargo test --lib`: **356 passed**, 0 failed, 3 ignored
- `cargo test --bin maxbotd`: **10 passed**, 0 failed, 1 ignored (the 1 ignored is the live `daemon_webhook_round_trip_30s` — out of scope, no live server)
- No failing tests. **Priority A (fix failures) is N/A.**

### CHANGELOG coverage
- Documented: v3.7.0, v3.7.1, v3.7.2, v3.7.3, v3.7.4, v3.7.5
- **Undocumented shipped versions (per git log, between documented sections):**
  v3.7.6 (Destroy in roster), v3.7.7 (Stop now + 2FA docs), v3.7.8 (backup_memory),
  v3.7.9 (click-through takeover), v3.7.10 (Chromium --user-data-dir), v3.7.11
  (composite reliability), v3.7.12 (Google OAuth), v3.7.13 (UX hardening),
  v3.7.14 (VoiceMode), v3.7.15 (STT provider fix)
- The user prompt's "backup_memory was deferred" is **incorrect** — `backup_memory`
  is fully shipped: Rust command in `src-tauri/src/commands/memory.rs:169`,
  tauri.ts wrapper in `src/lib/tauri.ts:865`, MemoryPanel UI in
  `src/components/MemoryPanel.tsx:158`, tests in
  `src/components/MemoryPanel.test.tsx:159-260`. Moving it off Priority B.

### TODO / FIXME / "future work" inventory
- `src-tauri/src/computer/provision.rs:307` — **multi-VM VNC port allocation
  (future work; v3.7.5 is a single-VM slice)** — this is Priority C. Real
  leftover from v3.7.5.
- `src-tauri/src/skills/executor.rs:90` — input binding is a future slice
  (out of scope tonight, not a hard TODO).
- `src-tauri/src/computer/input.rs:62,81` — VNC port display-number
  allocation notes. Related to Priority C.
- `src-tauri/src/approvals/defaults.rs:62` — "future work will start
  gating." Not actionable tonight.
- `src-tauri/src/bin/maxbotd.rs:438,484` — future slices for stricter
  token / filesystem hardening. Not actionable tonight.
- All other matches were false positives (e.g. `Connector::OAuth::…future`).

### Doc alignment state (Priority D)
- `docs/user-guide.md` Computer section: last swept in v3.7.4 (Tauri 2 + noVNC
  references removed) but still mentions "v3.7.7's first-bot-20-minutes.md is
  the canonical flow" — needs the v3.7.9 click-through takeover mention
  removed (the click-through was rolled BACK in v3.7.9 step 8 — now the
  Hand back / Stop now flow uses SSH tunnels again per the v3.7.9
  commit `ddc1fba`).
- `docs/first-bot-20-minutes.md`: canonical 20-minute flow. Need to
  read and align with v3.7.10 (Chromium --user-data-dir) +
  v3.7.9 (TigerVNC) language.
- `README.md` "Three access levels": line 13 says "Take over with
  Screen Sharing" — that's correct for v3.7.9+, but the sentence
  should reference `v3.7.2+ flow` which it does. Probably fine.

### Data
- `~/Library/Application Support/com.maxbot.app/` — not touched.
- Live SQLite / live VMs / live libvirt — not touched (smoke tests
  are gated behind `#[ignore]` and stay ignored).

## Ranked task list

1. **Priority C: VNC multi-VM port allocation.** v3.7.5 hard-codes port
   5900 fallback (`provision.rs:316`). For VM #2, port 5900 is
   already in use, so QEMU's `to=5999` auto-pick returns 5901 (or
   the first free). But the Mac side still tries to SSH-tunnel to
   5900. Slice:
   - Mac side picks the next free display number from
     `Settings.computer_vnc_local_port_range` and the existing
     `computers.vnc_port` rows. Pass that display number to the
     script as a new positional.
   - `provision-vm.sh` substitutes `127.0.0.1:<N>,password=off,to=5999`
     instead of `:0`.
   - Drop the 5900 fallback in `provision.rs` — `virsh vncdisplay`
     still won't know about the qemu:commandline VNC, but we KNOW
     the port (we picked it). Use that.
   - Add tests + CHANGELOG entry.
2. **Doc alignment (Priority D).** Light sweep of user-guide.md
   Computer section + first-bot-20-minutes.md to match v3.7.9
   (TigerVNC) + v3.7.10 (Chromium persistent) + v3.7.13 (UX).
   No mass rewrite.
3. **CHANGELOG backfill (housekeeping).** Add a `## Unreleased` block
   that lists v3.7.6 through v3.7.15 with one-line summaries +
   pointers to the git history. Don't fabricate details — refer
   to `git log` for canonical per-commit summaries.
4. **Small UI/UX holes (Priority E).** Discoverability only —
   no dead buttons, no new features.

## Assumptions / reversibility
- Multi-VM port allocation defaults to display 0 (port 5900) when
  no other VM is provisioned. This is what v3.7.5 already does.
  Existing VMs (single-VM era) keep their port; new VMs get the
  next free. Reversible: a single destroy + re-provision.
- For the qemu:commandline `-vnc` line, we keep `to=5999` so QEMU
  can still auto-pick if a specific port is busy. The Mac side
  then learns the actual port by parsing QEMU's monitor output
  OR by relying on the user-picked display number. Going with the
  latter (cleaner; matches the data we already pass in).
- CHANGELOG backfill summaries are sourced from `git log --oneline`
  + the per-version commit messages that already exist on the
  branch. Not invented.

## Hard rules in effect
- Stay in repo. No new product.
- No live SSH / live libvirt / live VMs / real API keys.
- No push to origin. Local commits on `overnight/2026-09-10` only.
- File-disjoint slices where possible.
- Tests green for everything I touch.
- BLOCKER.md only if the same issue hits twice.

## Progress
- 22:23 — Inventory done. All tests green. No priority-A work needed.
- (next) — Start Slice 1 (multi-VM VNC port allocation).
