//! v2.5 — Persistent per-Bot memory.
//!
//! A Bot's memory is content on the Bot's VM at
//! `bots/<id>/memory/{facts.jsonl, preferences.jsonl, history.jsonl}`.
//! Each line is a JSON object (see [`MemEntry`]). Reads and writes
//! go through the existing [`crate::computer::ssh::SshPool`] so we
//! don't add a new SSH subsystem.
//!
//! Public surface:
//! - [`MemEntry`] / [`MemKind`] — the on-disk and IPC shapes.
//! - [`store::read_all`] / [`append`] / [`delete`] / [`search`] —
//!   SFTP-backed, with a 2-second per-call timeout. Read failures
//!   return empty results so a Bot without a VM (or with a slow
//!   one) doesn't break the executor's system-prompt injection.

pub mod store;

use serde::{Deserialize, Serialize};

/// The three flavors of memory we persist. `History` is written
/// automatically by the executor at the end of a successful turn;
/// `Fact` and `Preference` are written by the LLM tool or by the
/// MemoryPanel UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemKind {
    Fact,
    Preference,
    History,
}

impl MemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Preference => "preference",
            Self::History => "history",
        }
    }

    /// Filename component (no extension).
    pub fn file_stem(self) -> &'static str {
        match self {
            Self::Fact => "facts",
            Self::Preference => "preferences",
            Self::History => "history",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fact" | "facts" => Some(Self::Fact),
            "preference" | "preferences" => Some(Self::Preference),
            "history" => Some(Self::History),
            _ => None,
        }
    }
}

/// One line of the per-Bot JSONL. For `History` entries `key` is
/// empty — the only content is `summary` and `at`. For `Fact` and
/// `Preference` the `key` is the lookup name (e.g. `user_name`) and
/// `content` is the value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemEntry {
    pub kind: MemKind,
    pub key: String,
    pub content: String,
    /// RFC3339 timestamp. For `History` we use the `at` field on
    /// the wire; the executor normalizes both on read.
    #[serde(default)]
    pub created_at: String,
}

impl MemEntry {
    pub fn new(kind: MemKind, key: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            kind,
            key: key.into(),
            content: content.into(),
            created_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}
