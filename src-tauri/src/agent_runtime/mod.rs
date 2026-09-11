//! `agent_runtime` — scaffold for MaxBot's agent runtime, modeled on
//! `xai-org/grok-build`'s `xai-grok-shell` crate (Apache-2.0).
//!
//! # Why this exists
//!
//! MaxBot currently has two chat surfaces running side-by-side:
//!
//! * The native chat agent loop in `crate::commands::chat` (the
//!   S3a work — date header, per-bot tool allowlist, title auto-
//!   rename, IANA-tz date format).
//! * The `grok agent stdio` JSON-RPC subprocess integration in
//!   `crate::grok_build` (`session.rs`, ~36KB).
//!
//! The subprocess dependency is what we want to grow out of. The
//! plan is to fold the native chat loop into a single in-process
//! runtime that matches `xai-grok-shell`'s shape:
//!
//! * One `Leader` entry-point with three run modes — `InProcess`
//!   (Tauri app), `Stdio` (subprocess, current `grok_build`),
//!   `Headless` (`maxbotd` / `maxbot_loopd`).
//! * A typed `RuntimeEvent` stream that matches `xai-grok-shell`'s
//!   ACP `session/update` taxonomy, instead of the current
//!   `Chunk | ToolUse | Error` triple in `grok_build::SessionEvent`.
//! * A small `tier.rs` classification helper, copied verbatim from
//!   `xai-grok-shell/src/tier.rs`, so future tier-gated tools
//!   have one place to classify the user.
//! * A `waterfall.rs` subagent-timing helper, copied verbatim from
//!   `xai-grok-shell/src/waterfall.rs`, for stage-level latency
//!   observability.
//!
//! # What this slice ships
//!
//! This is scaffolding only. The two `xai-grok-shell` files that
//! are self-contained (`tier.rs`, `waterfall.rs`) are copied
//! verbatim with attribution. The `entry.rs` module captures the
//! leader pattern as Rust types (`EntryMode`, `EntryConfig`,
//! `RuntimeEvent`, `AgentRuntime`, `bring_up`) but `bring_up`
//! returns `Err("not wired yet")` for all three modes. The actual
//! in-process agent, the stdio adapter, and the headless driver
//! land in follow-up PRs.
//!
//! # What this slice does NOT touch
//!
//! * `crate::commands::chat` (S3a work — still the primary chat loop).
//! * `crate::grok_build` (subprocess client — kept until the
//!   in-process runtime replaces it).
//! * `src/`, `src/v4/`, `src-tauri/src/bin/*` (UI + binaries).
//! * Any provider, tool, or storage code.
//!
//! # Source attribution
//!
//! Verbatim copies of `xai-grok-shell`'s `tier.rs` and
//! `waterfall.rs` are licensed by xAI under the Apache License,
//! Version 2.0. See [`THIRD_PARTY_NOTES`] at the bottom of each
//! file and the project-root `THIRD_PARTY_NOTICES.md` for the
//! full attribution block.
//!
//! [`THIRD_PARTY_NOTES`]: https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-shell/src/lib.rs
//!
//! Source: <https://github.com/xai-org/grok-build>

pub mod entry;
pub mod tier;
pub mod waterfall;

pub use entry::{bring_up, AgentRuntime, EntryConfig, EntryMode, RuntimeEvent};
