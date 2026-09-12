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

use serde::{Deserialize, Serialize};
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
            "error": e.to_string(),
        }),
    };
    let _ = app.emit(STATE_CHANGED_EVENT, payload);
    result
}

/// v4 S7 — start the X11 desktop inside the VM via
/// `xinit /usr/bin/xfce4-session -- :0`. The VM has
/// `xfce4` installed by cloud-init but no display manager
/// boots it on its own; the QEMU virtual framebuffer stays
/// in VGA text mode and the Computer preview shows an
/// empty black frame with a text-mode caret. We kick off
/// xinit via QGA `guest-exec` so the framebuffer shows a
/// graphical XFCE4 desktop within ~5–10 s of the next
/// screenshot poll.
///
/// Fire-and-forget. The renderer's auto-call catches and
/// ignores errors (e.g. xinit on a busy display exits
/// non-zero — that's the idempotency contract).
#[tauri::command]
pub async fn computer_show_desktop(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<(), String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    mgr.show_desktop(&db, &bot_id).await.map(|_| ()).map_err(|e| e.to_string())
}

/// v3.7.2: capture a single JPEG frame from the VM's
/// QEMU virtual framebuffer via `virsh screenshot` on
/// the host. The renderer polls this command at ~300ms
/// to paint the in-app preview. Returns the raw JPEG
/// bytes — Tauri serializes them as `Vec<u8>`, the
/// renderer wraps them in a `Blob` and sets `<img
/// src="blob:...">`.
///
/// The command is intentionally NOT gated on QGA. The
/// QEMU virtual VGA framebuffer is always available
/// while the domain is `running`, including during
/// cloud-init's LightDM install. Gating on QGA would
/// hide the exact boot phase the preview is meant to
/// show.
/// v3.7.9: structured screenshot response — the
/// JPEG bytes + the framebuffer's natural width /
/// height so the renderer can map pointer events
/// (in `clientX` / `clientY` pixels) to the
/// framebuffer coordinates xdotool expects. The
/// `camelCase` rename maps to `width` / `height` on
/// the TS side (Tauri's default JSON convention).
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ComputerScreenshotOutput {
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[tauri::command]
pub async fn computer_screenshot(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<ComputerScreenshotOutput, String> {
    let db = state.db.clone();
    let mgr = state.computer.clone();
    let (bytes, width, height) = mgr
        .screenshot_with_size(&db, &bot_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(ComputerScreenshotOutput {
        bytes,
        width,
        height,
    })
}

#[tauri::command]
pub async fn computer_input_open(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<(), String> {
    state
        .computer
        .input_open(&bot_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn computer_input_event(
    state: State<'_, AppState>,
    bot_id: String,
    event: crate::computer::input::InputEvent,
) -> Result<(), String> {
    state
        .computer
        .input_event(&bot_id, &event)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn computer_input_close(
    state: State<'_, AppState>,
    bot_id: String,
) -> Result<(), String> {
    state
        .computer
        .input_close(&bot_id)
        .await
        .map_err(|e| e.to_string())
}

// `computer_takeover_open` and `computer_takeover_close`
// were removed in v3.7.9. The v3.7.2/v3.7.5 takeover path
// (an `ssh -L` tunnel to the VM's VNC port, then a
// TigerVNC viewer on the local end) is replaced by the
// in-panel click-through takeover: the user drives the
// VM directly through the JPEG preview via the
// `computer_input_*` commands above, no VNC viewer
// involved. The renderer no longer references the
// old commands — the corresponding entries in
// `tauri::generate_handler!` were also removed.

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
