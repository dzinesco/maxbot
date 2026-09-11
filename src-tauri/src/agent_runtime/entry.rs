//! Agent-runtime entry-point abstraction, modeled on
//! `xai-org/grok-build`'s `xai-grok-shell` crate's `leader/`
//! module.
//!
//! Source: <https://github.com/xai-org/grok-build/tree/main/crates/codegen/xai-grok-shell/src/leader>
//!
//! The upstream `leader` module exposes three run modes for the
//! agent runtime:
//!
//! * `leader`     — long-lived supervisor process; the parent
//!                   of any `grok` invocation.
//! * `stdio`      — JSON-RPC over stdio to a child `grok` agent.
//! * `headless`   — no TUI; just the agent loop driving itself.
//!
//! The simplest concrete impl upstream is
//! `leader/in_process.rs` (3.5KB): an in-process `MvpAgent` glued
//! to an `acp::AgentSideConnection` over a `tokio::io::simplex`
//! pair. We don't port that file directly because it pulls in
//! xAI-private crates (`xai_acp_lib`, `xai_grok_otel`,
//! `xai_grok_login`); instead this module captures the same shape
//! in MaxBot-flavoured types and leaves the wiring for follow-up
//! slices.
//!
//! # Slice status
//!
//! `bring_up` returns `Err("not wired yet")` for every mode. The
//! concrete `InProcess`, `Stdio`, and `Headless` impls land when
//! the in-process chat loop replaces the existing
//! `crate::commands::chat` and `crate::grok_build` modules.

use std::path::PathBuf;
use std::sync::Arc;

/// How to bring up the agent runtime. Mirrors the three run
/// modes exposed by `xai-grok-shell`'s `leader` module.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntryMode {
    /// Run the agent in-process, with the parent Tauri shell
    /// driving it directly. This is where the native chat loop
    /// (`crate::commands::chat`) eventually lives.
    InProcess,
    /// Speak ACP over stdio to a `grok` subprocess. Currently
    /// implemented by `crate::grok_build`; the future
    /// integration point is a thin adapter that wraps the
    /// existing `GrokSession` and exposes it as an
    /// `AgentRuntime`.
    Stdio,
    /// Run the agent without a UI. Used by the
    /// always-on daemon `maxbotd` and the keep-alive
    /// supervisor `maxbot_loopd`.
    Headless,
}

/// Per-mode startup parameters. Fields unused by the chosen mode
/// are silently ignored by the concrete impl.
#[derive(Debug, Clone, Default)]
pub struct EntryConfig {
    /// Working directory for the agent (defaults to the
    /// bot's `bots/<id>/` for `InProcess` and `Headless`; the
    /// `grok` subprocess's `cwd` for `Stdio`).
    pub cwd: Option<PathBuf>,
    /// Model alias (e.g. `minimax`, `grok-4-fast-reasoning`,
    /// `claude-sonnet-4-5`). Defaults to whatever the parent
    /// already chose.
    pub model: Option<String>,
    /// Resume an existing session by id. Only meaningful for
    /// `Stdio` mode (the existing `crate::grok_build`
    /// already persists `grok_session_id` for this).
    pub resume_session_id: Option<String>,
    /// API key for `InProcess` and `Headless` modes. `Stdio`
    /// mode picks it up from the subprocess's own auth path.
    pub api_key: Option<String>,
}

/// One chunk of a streamed agent response.
///
/// The taxonomy is wider than the current
/// `crate::grok_build::session::SessionEvent`
/// (`Chunk | ToolUse | Error`). It mirrors
/// `xai-grok-shell`'s typed ACP `session/update` notifications
/// so future UI code can pattern-match without string-typing.
#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    /// Token-level text chunk from the assistant message.
    MessageChunk(String),
    /// Agent invoked a tool — name + JSON args. The renderer's
    /// `ApprovalSheet` decides whether to ask for consent.
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    /// Tool returned. The renderer surfaces this as a tool
    /// result row.
    ToolResult {
        name: String,
        output: String,
    },
    /// Agent needs explicit user consent before a tool can run.
    /// Mirrors the approval gate that already exists in
    /// `crate::approvals`.
    PermissionRequired {
        name: String,
        reason: String,
    },
    /// Turn finished cleanly (`stopReason == "end_turn"`).
    TurnEnd,
    /// Something failed. `message` is human-readable and the
    /// chat renderer logs it at warn level.
    Error(String),
}

/// The runtime handle returned by `bring_up`. Cheap to clone
/// (`Arc` inside). One trait surface for all three modes so
/// call-sites don't need to know which mode they got back.
pub trait AgentRuntime: Send + Sync {
    /// Which run mode this runtime is bound to. Lets
    /// call-sites assert ("if this is `Stdio`, then the
    /// resume id is meaningful") without dynamic dispatch on
    /// a separate field.
    fn mode(&self) -> EntryMode;

    /// Submit a user prompt and stream the agent's response
    /// through `sink`. The future resolves when the turn
    /// ends (`RuntimeEvent::TurnEnd`) or fails
    /// (`RuntimeEvent::Error`).
    fn prompt<'a>(
        &'a self,
        text: &'a str,
        sink: &'a mut dyn FnMut(RuntimeEvent),
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>;
}

/// Bring up the runtime for the given mode.
///
/// # Status
///
/// All three modes are currently **not wired**. This function
/// returns `Err("agent_runtime::entry::bring_up: <mode> not wired
/// yet")` until the corresponding impl lands in a follow-up
/// slice:
///
/// * `InProcess` — replace `crate::commands::chat`'s ad-hoc
///   loop with an `MvpAgent`-style in-process impl.
/// * `Stdio` — wrap `crate::grok_build::session::GrokSession` in
///   a thin adapter that maps its `SessionEvent` stream to
///   `RuntimeEvent`.
/// * `Headless` — drive the `maxbotd` and `maxbot_loopd`
///   binaries through the same `AgentRuntime` trait so they
///   stop re-implementing the chat loop.
pub async fn bring_up(
    mode: EntryMode,
    cfg: EntryConfig,
) -> Result<Arc<dyn AgentRuntime>, String> {
    let _ = cfg;
    let msg = match mode {
        EntryMode::InProcess => {
            "agent_runtime::entry::bring_up: InProcess not wired yet \
             (will replace crate::commands::chat)"
        }
        EntryMode::Stdio => {
            "agent_runtime::entry::bring_up: Stdio not wired yet \
             (use crate::grok_build::session::GrokSession until the \
             adapter lands)"
        }
        EntryMode::Headless => {
            "agent_runtime::entry::bring_up: Headless not wired yet \
             (will replace maxbotd / maxbot_loopd ad-hoc loops)"
        }
    };
    Err(msg.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_mode_eq_and_hash() {
        assert_eq!(EntryMode::InProcess, EntryMode::InProcess);
        assert_ne!(EntryMode::InProcess, EntryMode::Stdio);
        assert_ne!(EntryMode::Stdio, EntryMode::Headless);

        // Hash + Eq together means the enum can be a HashMap key.
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(EntryMode::InProcess);
        set.insert(EntryMode::Stdio);
        set.insert(EntryMode::Headless);
        assert_eq!(set.len(), 3);
    }

    #[test]
    fn entry_config_default_is_all_none() {
        let c = EntryConfig::default();
        assert!(c.cwd.is_none());
        assert!(c.model.is_none());
        assert!(c.resume_session_id.is_none());
        assert!(c.api_key.is_none());
    }

    #[test]
    fn runtime_event_clone_carries_payload() {
        // Clone must preserve the inner payload so the
        // broadcast::Receiver can pass events across await
        // points without re-parsing.
        let ev = RuntimeEvent::ToolCall {
            name: "shell_run".to_string(),
            args: serde_json::json!({"command": "ls"}),
        };
        let ev2 = ev.clone();
        match ev2 {
            RuntimeEvent::ToolCall { name, args } => {
                assert_eq!(name, "shell_run");
                assert_eq!(args["command"], "ls");
            }
            _ => panic!("wrong variant after clone"),
        }
    }

    #[tokio::test]
    async fn bring_up_returns_not_wired_for_all_modes() {
        for mode in [EntryMode::InProcess, EntryMode::Stdio, EntryMode::Headless] {
            let r = bring_up(mode, EntryConfig::default()).await;
            assert!(r.is_err(), "mode {:?} must error", mode);
            let err = r.err().unwrap();
            assert!(
                err.contains("not wired"),
                "mode {:?}: expected 'not wired' in {:?}",
                mode,
                err
            );
        }
    }
}
