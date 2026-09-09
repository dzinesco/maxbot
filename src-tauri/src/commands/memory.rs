//! v2.5 — Persistent memory Tauri commands.
//!
//! These wrap [`crate::memory::store`] and are wired into
//! `lib.rs` via `invoke_handler!`. They share the same
//! non-fatal-read semantics as the store: a Bot without a VM
//! returns empty results, never an error.

use serde::Deserialize;
use tauri::State;

use crate::memory::store;
use crate::memory::{MemEntry, MemKind};
use crate::AppState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KindArg {
    Fact,
    Preference,
    History,
}

impl From<KindArg> for MemKind {
    fn from(k: KindArg) -> Self {
        match k {
            KindArg::Fact => Self::Fact,
            KindArg::Preference => Self::Preference,
            KindArg::History => Self::History,
        }
    }
}

/// Top-5 keyword matches across all 3 kinds. Read failures
/// (no VM, timeout, parse error) collapse to an empty Vec.
#[tauri::command]
pub async fn memory_search(
    state: State<'_, AppState>,
    bot_id: String,
    query: String,
) -> Result<Vec<MemEntry>, String> {
    let pool = state.computer.ssh_pool();
    let entries = store::search(&*pool, &bot_id, &query, 5).await;
    Ok(entries)
}

/// Append a new entry. `history` is not exposed to the renderer
/// here (the executor auto-writes history) but we accept it for
/// symmetry — the panel doesn't surface it, but a power user
/// hitting the IPC directly will work.
#[tauri::command]
pub async fn memory_remember(
    state: State<'_, AppState>,
    bot_id: String,
    kind: KindArg,
    key: String,
    content: String,
) -> Result<MemEntry, String> {
    let mem_kind: MemKind = kind.into();
    let entry = MemEntry::new(mem_kind, key, content);
    let pool = state.computer.ssh_pool();
    store::append(&*pool, &bot_id, mem_kind, entry.clone())
        .await
        .map_err(|e| e.to_string())?;
    Ok(entry)
}

/// Delete by key. Returns whether anything was actually removed
/// (false if the key wasn't found).
#[tauri::command]
pub async fn memory_forget(
    state: State<'_, AppState>,
    bot_id: String,
    kind: KindArg,
    key: String,
) -> Result<bool, String> {
    let mem_kind: MemKind = kind.into();
    let pool = state.computer.ssh_pool();
    store::delete(&*pool, &bot_id, mem_kind, &key)
        .await
        .map_err(|e| e.to_string())
}

/// List all entries of one kind, oldest first. Used by the
/// MemoryPanel's three-column read-only browser.
#[tauri::command]
pub async fn memory_list(
    state: State<'_, AppState>,
    bot_id: String,
    kind: KindArg,
) -> Result<Vec<MemEntry>, String> {
    let mem_kind: MemKind = kind.into();
    let pool = state.computer.ssh_pool();
    Ok(store::read_all(&*pool, &bot_id, mem_kind).await)
}
