//! Tauri command surface. Every IPC entry point lives here so the Rust/Tauri
//! boundary is easy to audit.

pub mod bots;
pub mod chat;
pub mod computer;
pub mod conversations;
pub mod meta;
pub mod settings;
pub mod tcc;
pub mod tts;
