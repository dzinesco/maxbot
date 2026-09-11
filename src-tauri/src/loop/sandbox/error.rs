//! Error type for sandbox operations.
//!
//! Every tool call (`read_file`, `write_file`, `list_dir`, `exec`)
//! returns `Result<T, SandboxError>`. The variants are designed to
//! give the agent (and the test suite) a precise reason for the
//! rejection so fail-closed tests can match on the message.
//!
//! The two load-bearing rejections are:
//!
//!   * [`SandboxError::PathOutsideSandbox`] — the requested path's
//!     canonical form lives outside the sandbox root. Covers `..`
//!     traversal, absolute paths from outside, and any other
//!     non-symlink escape.
//!
//!   * [`SandboxError::PassphraseDbInaccessible`] — the requested
//!     path either contains the substring `passphrase` (case
//!     insensitive) or names `maxbot.sqlite`. This is a
//!     defense-in-depth check that fires even if the sandbox root
//!     is misconfigured to include the data dir.
//!
//! [`SandboxError::SymlinkEscape`] is a more specific variant of
//! `PathOutsideSandbox` that names the resolved target so the
//! caller can tell a symlink hop from an explicit `..` traversal.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("path outside sandbox: requested {requested} resolves to {resolved} (sandbox root: {sandbox_root})")]
    PathOutsideSandbox {
        requested: PathBuf,
        resolved: PathBuf,
        sandbox_root: PathBuf,
    },

    #[error("passphrase DB inaccessible from sandbox: {requested}")]
    PassphraseDbInaccessible { requested: PathBuf },

    #[error("symlink escape: requested {requested} resolves to {resolved} which is outside the sandbox")]
    SymlinkEscape { requested: PathBuf, resolved: PathBuf },

    #[error("io error during sandbox operation: {0}")]
    Io(#[from] std::io::Error),

    #[error("failed to launch sandbox-exec: {0}")]
    SandboxExecLaunch(String),

    #[error("sandbox root does not exist or is not a directory: {0}")]
    InvalidRoot(PathBuf),
}
