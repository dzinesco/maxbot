//! Tauri command surface. Every IPC entry point lives here so the Rust/Tauri
//! boundary is easy to audit.

pub mod approvals;
pub mod bots;
pub mod chat;
pub mod computer;
pub mod conversations;
pub mod daemon;
pub mod groups;
pub mod memory;
pub mod meta;
pub mod oauth;
pub mod settings;
pub mod skills;
pub mod tcc;
pub mod tts;
pub mod voice;
