# Third-Party Notices

MaxBot is primarily MIT-licensed (`LICENSE` at the repo root).
The following components are derivative works of code licensed
under different terms; the licenses below apply to those
components only.

---

## `agent_runtime::tier` — verbatim copy

**Source:** <https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/tier.rs>

**File in this tree:** `src-tauri/src/agent_runtime/tier.rs`

**Upstream license:** Apache License, Version 2.0
(<http://www.apache.org/licenses/LICENSE-2.0>)

**Modifications:** None — verbatim copy, attribution block
prepended.

---

## `agent_runtime::waterfall` — verbatim copy

**Source:** <https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/waterfall.rs>

**File in this tree:** `src-tauri/src/agent_runtime/waterfall.rs`

**Upstream license:** Apache License, Version 2.0
(<http://www.apache.org/licenses/LICENSE-2.0>)

**Modifications:** None — verbatim copy, attribution block
prepended.

---

## `agent_runtime::entry` — inspired-by, not copied

**Source (reference only):**
<https://github.com/xai-org/grok-build/tree/main/crates/codegen/xai-grok-shell/src/leader>

**File in this tree:** `src-tauri/src/agent_runtime/entry.rs`

**License:** MIT (matches the rest of MaxBot).

**Notes:** This file re-implements the *shape* of `xai-grok-shell`'s
leader module (`EntryMode`, `EntryConfig`, `AgentRuntime`,
`bring_up`) in MaxBot-flavoured types. It is **not** a derivative
work of any specific upstream source file — the closest upstream
file (`leader/in_process.rs`, 3.5KB) pulls in xAI-private crates
that aren't available in this tree.
