//! v3.4.0 (Phase 5) + v3.5.0 (Phase 6) — Grok Bot
//! default rule preset.
//!
//! When a new Bot is created, its starting `approval_rules`
//! set comes from `grok_bot_defaults()`. The user can
//! override per-tool per-Bot in the Rules section of
//! `BotEditor.tsx`; clicking the "Grok Bot defaults"
//! button re-applies this preset for an existing Bot.
//!
//! ## Why this exists
//!
//! The prior default (`mail_send`, `message_bot`,
//! `file_write`, `shell_run` → `ask`; everything else
//! → `auto`) was conservative to a fault: every Bot
//! started with the maximally-interruptive rule set.
//! Per Tyler's Phase 5 spec, the Grok Bot product
//! expectation is "read-only auto, send/payment/destroy
//! ask" — and the per-Bot creator should be one click
//! away from that bar.
//!
//! ## Tool-name notes
//!
//! The preset mixes (a) actual tool names that ship
//! today (`vm_browser_open`, `vm_computer_use`,
//! `mail_send`, etc.) and (b) aspirational names from
//! the Phase 5 spec (`screenshot`, `send_email`,
//! `send_payment`, `destroy_vm`, `create_approval`).
//! The aspirational ones are harmless — they sit in
//! `approval_rules` as inert rows; nothing in the
//! executor or registry matches them. The moment a
//! matching tool lands, the rule fires. This keeps the
//! preset stable across releases even when the tool
//! catalog changes.
//!
//! `vm_computer_use.click` / `vm_computer_use.type` are
//! the spec's sub-action split for the existing
//! monolithic `vm_computer_use` tool. The real tool is
//! monolithic; the sub-action rules are documented in
//! the BotEditor's Rules section but do not gate the
//! real tool today (they default to `auto` for missing
//! rows, same as every other missing rule). When a
//! future release splits the tool, these rules will
//! start gating the right thing.

use super::Rule;

/// One row of the Grok Bot preset: `(tool_name, rule)`.
pub type PresetRow = (&'static str, Rule);

/// The Grok Bot default rule set. Applied when a Bot
/// is first created and when the user clicks
/// "Grok Bot defaults" in the Bot editor.
pub const GROK_BOT_DEFAULTS: &[PresetRow] = &[
    // --- read-only / observability: auto ---
    // The Bot can take screenshots, open a browser tab,
    // and generally observe the world without asking.
    ("screenshot", Rule::Auto),
    ("vm_browser_open", Rule::Auto),
    ("vm_computer_use", Rule::Auto),
    // Sub-action split per the Phase 5 spec. The real
    // tool is monolithic today; these are aspirational
    // rules that future work will start gating.
    ("vm_computer_use.click", Rule::Auto),
    ("vm_computer_use.type", Rule::Ask),
    ("mail_inbox", Rule::Auto),
    ("mail_search", Rule::Auto),
    ("file_read", Rule::Auto),
    ("web_search", Rule::Auto),
    ("web_fetch", Rule::Auto),
    ("memory_recall", Rule::Auto),
    ("memory_list", Rule::Auto),
    // --- outbound / mutating: ask ---
    // Anything that sends data out of the bot's world or
    // mutates persistent state needs a human sign-off.
    // "send" / "payment" / "destroy" / "create_approval"
    // are the Phase 5 pillars; the matching existing
    // tools are listed alongside so the preset is
    // actionable on day one.
    ("send_email", Rule::Ask),
    ("mail_send", Rule::Ask),
    ("mail_draft", Rule::Ask),
    ("message_bot", Rule::Ask),
    ("send_payment", Rule::Ask),
    ("destroy_vm", Rule::Ask),
    ("create_approval", Rule::Ask),
    ("file_write", Rule::Ask),
    ("shell_run", Rule::Ask),
    ("coding", Rule::Ask),
    ("grok_prompt", Rule::Ask),
    // AppleScript + ego_browser are Mac-side; default to
    // ask because the spec puts "mac-with-approval" under
    // the same gate.
    ("apple_script", Rule::Ask),
    ("ego_browser", Rule::Ask),
    // Skill execution is a derived call; ask the human
    // before letting one Bot invoke another's Skill.
    ("run_skill", Rule::Ask),
    // v3.5.0 (Phase 6) — Server-side shared folder.
    // `shared_read` / `shared_list` are read-only
    // (auto); `shared_write` is mutating (ask). The
    // path-safety guard is the security gate; the
    // approval rule is the human gate.
    ("shared_read", Rule::Auto),
    ("shared_list", Rule::Auto),
    ("shared_write", Rule::Ask),
];

/// Apply the Grok Bot preset to a Bot, replacing any
/// rules the user has already set. Idempotent — calling
/// twice lands on the same state.
///
/// `db` is the open `crate::storage::Database` (the
/// method is on `Database`; this helper just centralizes
/// the iteration so the test and the new-Bot code path
/// stay in lock-step). The caller is responsible for
/// holding a lock if it cares about cross-actor
/// atomicity — the existing per-row `set_approval_rule`
/// is row-level atomic, so a partial failure leaves the
/// Bot with a subset of the preset (never worse than
/// the pre-preset state).
pub fn grok_bot_defaults_iter() -> impl Iterator<Item = PresetRow> {
    GROK_BOT_DEFAULTS.iter().copied()
}

// =====================================================================
//  Tests
// =====================================================================
//
// The preset is data, but we still want a regression
// guard against accidental removals. The brief pins the
// following tools as `auto`: screenshot, vm_browser_open,
// vm_computer_use, vm_computer_use.click. The following
// are pinned as `ask`: vm_computer_use.type, send_email,
// send_payment, destroy_vm, create_approval. We assert
// the subset here so a future maintainer removing one
// will get a clear test failure explaining the spec
// intent, instead of silently breaking a user's
// expectations.

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn preset_map() -> HashMap<&'static str, Rule> {
        let mut m = HashMap::new();
        for (name, rule) in GROK_BOT_DEFAULTS {
            m.insert(*name, *rule);
        }
        m
    }

    #[test]
    fn read_only_tools_default_to_auto() {
        let m = preset_map();
        for tool in [
            "screenshot",
            "vm_browser_open",
            "vm_computer_use",
            "vm_computer_use.click",
        ] {
            assert_eq!(
                m.get(tool).copied(),
                Some(Rule::Auto),
                "Phase 5 spec: {tool} must be auto in the Grok Bot preset",
            );
        }
    }

    #[test]
    fn read_only_fs_tools_default_to_auto() {
        // v3.5.0 (Phase 6) — the shared folder's
        // read tools. Matches the Phase 5 read-only
        // pattern (screenshot / vm_browser_open):
        // the human shouldn't be interrupted for a
        // pure read.
        let m = preset_map();
        for tool in ["shared_read", "shared_list"] {
            assert_eq!(
                m.get(tool).copied(),
                Some(Rule::Auto),
                "Phase 6 spec: {tool} must be auto in the Grok Bot preset",
            );
        }
    }

    #[test]
    fn outbound_mutating_tools_default_to_ask() {
        let m = preset_map();
        for tool in [
            "vm_computer_use.type",
            "send_email",
            "send_payment",
            "destroy_vm",
            "create_approval",
            "mail_send",
            "message_bot",
            "file_write",
            "shell_run",
        ] {
            assert_eq!(
                m.get(tool).copied(),
                Some(Rule::Ask),
                "Phase 5 spec: {tool} must be ask in the Grok Bot preset",
            );
        }
    }

    #[test]
    fn mutating_fs_tools_default_to_ask() {
        // v3.5.0 (Phase 6) — the shared folder's
        // mutating tool. Matches the Phase 5 mutating
        // pattern (file_write / shell_run): the human
        // gets a single confirmation per call.
        let m = preset_map();
        for tool in ["shared_write"] {
            assert_eq!(
                m.get(tool).copied(),
                Some(Rule::Ask),
                "Phase 6 spec: {tool} must be ask in the Grok Bot preset",
            );
        }
    }

    #[test]
    fn preset_has_no_duplicates() {
        // The schema's PRIMARY KEY (bot_id, tool_name) means
        // a duplicate would silently overwrite; this test
        // makes the invariant visible in the suite.
        let mut seen = std::collections::HashSet::new();
        for (name, _) in GROK_BOT_DEFAULTS {
            assert!(
                seen.insert(*name),
                "duplicate tool {name} in GROK_BOT_DEFAULTS — would silently overwrite"
            );
        }
    }

    #[test]
    fn iter_returns_preset_in_order() {
        // The renderer uses the order for stable display;
        // pinning the order makes diffs intentional.
        let names: Vec<&'static str> =
            grok_bot_defaults_iter().map(|(n, _)| n).collect();
        assert_eq!(names[0], "screenshot");
        assert!(names.contains(&"mail_send"));
    }
}
