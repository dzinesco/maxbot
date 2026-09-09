//! Tauri commands for the per-Bot Computer (v2.0 Slice B).
//!
//! Each command here is a thin wrapper around
//! `ComputerManager`. The thinness is deliberate: the
//! manager owns the state, the commands just unpack the
//! arguments, call the manager, and serialize the result
//! or error to the renderer. All errors are formatted as
//! strings via `ComputerError::to_string` so the renderer's
//! `try { await invoke(...) } catch (e) { ... }` shows a
//! useful message without needing a structured error type.
//!
//! The renderer's TS types should mirror the Rust shapes
//! in `computer::Computer` and the `ProvisionOptions`
//! below. Both have `#[derive(Serialize)]` so the JSON
//! wire format is the camelCase version of the snake_case
//! Rust field names (serde default for `ComputerState`,
//! which has `#[serde(rename_all = "snake_case")]`).

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use crate::computer::provision::ProvisionOptions;
use crate::AppState;

/// Event name emitted on every state transition so the
/// renderer's `listen("computer://state-changed", ...)` can
/// update the bot's status badge without polling.
pub const STATE_CHANGED_EVENT: &str = "computer://state-changed";

/// Mirror of the renderer's TS `ProvisionOpts` (disk + ram).
/// The renderer can also omit these — in which case
/// `Settings.computer_default_disk_gb / _ram_mb` are used.
#[derive(Debug, Deserialize, Default)]
pub struct ProvisionOpts {
    pub disk_gb: Option<u32>,
    pub ram_mb: Option<u32>,
}

impl ProvisionOpts {
    fn resolve(self, default_disk: u32, default_ram: u32) -> ProvisionOptions {
        ProvisionOptions {
            disk_gb: self.disk_gb.unwrap_or(default_disk),
            ram_mb: self.ram_mb.unwrap_or(default_ram),
        }
    }
}

#[tauri::command]
pub async fn computer_get(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<Option<crate::computer::Computer>, String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    mgr.get(&db, &bot_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn computer_provision(
    state: State<'_, AppState>,
    app: AppHandle,
    bot_id: String,
    opts: Option<ProvisionOpts>,
) -> Result<(), String> {
    // Pull defaults from Settings if the caller didn't
    // provide them. We do this in the command (not the
    // manager) so the manager can be called from tests
    // without going through Settings.
    let (default_disk, default_ram) = {
        let db = state.db.clone();
        let s = db
            .load_settings()
            .map_err(|e| e.to_string())?;
        (
            s.computer_default_disk_gb,
            s.computer_default_ram_mb,
        )
    };
    let opts = opts
        .unwrap_or_default()
        .resolve(default_disk, default_ram);
    let db = state.db.clone();
    let mgr = state.computer.clone();
    let bot_id_for_event = bot_id.clone();
    let app_for_event = app.clone();
    // The provision itself is async and can take 1-2
    // minutes. We run it on a tokio task so the Tauri
    // command returns once the manager is done (the
    // Tauri side awaits the future). The renderer can
    // listen on `computer://state-changed` for progress
    // (the manager writes `provisioning` to the DB
    // before starting the long step).
    tokio::spawn(async move {
        let result = mgr.provision(&db, &bot_id, opts).await;
        let payload = match &result {
            Ok(()) => serde_json::json!({
                "bot_id": bot_id_for_event,
                "state": "running",
                "error": null,
            }),
            Err(e) => serde_json::json!({
                "bot_id": bot_id_for_event,
                "state": "error",
                "error": e.to_string(),
            }),
        };
        if let Err(send_err) = app_for_event.emit(STATE_CHANGED_EVENT, payload) {
            log::warn!("computer: failed to emit state-changed event: {send_err}");
        }
    });
    Ok(())
}

#[tauri::command]
pub async fn computer_start(
    state: State<'_, AppState>,
    app: AppHandle,
    bot_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    let bot_id_for_event = bot_id.clone();
    let result = mgr.start(&db, &bot_id).await.map_err(|e| e.to_string());
    let payload = match &result {
        Ok(()) => serde_json::json!({
            "bot_id": bot_id_for_event,
            "state": "running",
        }),
        Err(e) => serde_json::json!({
            "bot_id": bot_id_for_event,
            "state": "error",
            "error": e,
        }),
    };
    let _ = app.emit(STATE_CHANGED_EVENT, payload);
    result
}

#[tauri::command]
pub async fn computer_stop(
    state: State<'_, AppState>,
    app: AppHandle,
    bot_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    let bot_id_for_event = bot_id.clone();
    let result = mgr.stop(&db, &bot_id).await.map_err(|e| e.to_string());
    let payload = match &result {
        Ok(()) => serde_json::json!({
            "bot_id": bot_id_for_event,
            "state": "stopped",
        }),
        Err(e) => serde_json::json!({
            "bot_id": bot_id_for_event,
            "state": "error",
            "error": e,
        }),
    };
    let _ = app.emit(STATE_CHANGED_EVENT, payload);
    result
}

#[tauri::command]
pub async fn computer_destroy(
    state: State<'_, AppState>,
    app: AppHandle,
    bot_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    let bot_id_for_event = bot_id.clone();
    let result = mgr.destroy(&db, &bot_id).await.map_err(|e| e.to_string());
    let payload = match &result {
        Ok(()) => serde_json::json!({
            "bot_id": bot_id_for_event,
            "state": "destroyed",
        }),
        Err(e) => serde_json::json!({
            "bot_id": bot_id_for_event,
            "state": "error",
            "error": e,
        }),
    };
    let _ = app.emit(STATE_CHANGED_EVENT, payload);
    result
}

#[tauri::command]
pub async fn computer_console_url(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<String, String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    mgr.console_url(&db, &bot_id)
        .await
        .map_err(|e| e.to_string())
}

/// Smoke test the libvirt connection. Returns the
/// number of domains currently defined on the server.
/// Used by the Settings → Computer tab's "Test
/// connection" button.
#[tauri::command]
pub async fn computer_test_connection(
    state: State<'_, AppState>,
) -> Result<usize, String> {
    let mgr = state.computer.clone();
    mgr.test_connection().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn computer_file_list(
    state: State<'_, AppState>,
    bot_id: String,
    path: String,
) -> Result<Vec<crate::computer::ssh::SftpEntry>, String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    mgr.file_list(&db, &bot_id, &path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn computer_file_read(
    state: State<'_, AppState>,
    bot_id: String,
    path: String,
) -> Result<String, String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    mgr.file_read(&db, &bot_id, &path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn computer_file_write(
    state: State<'_, AppState>,
    bot_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    mgr.file_write(&db, &bot_id, &path, &content)
        .await
        .map_err(|e| e.to_string())
}

/// v2.3.5: install the user's default SSH public key
/// (`~/.ssh/id_ed25519.pub` / `id_rsa.pub` / `id_ecdsa.pub`)
/// into the VM's `authorized_keys` via the QEMU guest
/// agent. The install is idempotent — clicking the
/// ComputerPanel button twice doesn't duplicate the key
/// line. Returns the QGA's JSON response on success.
#[tauri::command]
pub async fn computer_install_default_key(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<String, String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    mgr.install_default_key(&db, &bot_id)
        .await
        .map_err(|e| e.to_string())
}
