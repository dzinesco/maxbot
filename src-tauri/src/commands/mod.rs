//! Tauri command surface. Every IPC entry point lives here so the Rust/Tauri
//! boundary is easy to audit.

pub mod approvals;
pub mod bots;
pub mod chat;
pub mod computer;
pub mod conversations;
pub mod daemon;
pub mod groups;
// v3.7.17 Slice 2 — UI surface for `maxbot_loopd`.
// Spawn / stop the keep-alive loop supervisor and read back
// TASK.md / journal for the Sidebar's Loop panel. This module
// does NOT touch `loop/io` — the supervisor owns the canonical
// types and the lock; this is a read-only IPC adapter plus the
// spawn/stop lifecycle.
pub mod loopd;
pub mod memory;
pub mod meta;
pub mod oauth;
pub mod settings;
pub mod skills;
pub mod tcc;
pub mod tts;
pub mod voice;
