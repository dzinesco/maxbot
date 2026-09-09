//! TCC (Transparency, Consent, Control) helpers for the AppleScript
//! Computer Use surface. The Settings UI uses these commands to:
//!
//! - List the apps MaxBot can control
//! - Trigger the first-run macOS prompt for a specific app
//! - Probe whether access is granted (by re-running a no-op AppleScript)
//! - Open System Settings → Privacy & Security → Automation
//!
//! We don't try to read TCC.db directly; the OS surfaces access state
//! implicitly via osascript exit codes, which is enough for the UI to
//! show "granted / not yet / denied".

use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_opener::OpenerExt;

use crate::tools::apple_script_exec::probe_tcc;

#[derive(Serialize, Clone)]
pub struct ControllableApp {
    /// Stable key used by the UI to identify the app in state.
    pub key: &'static str,
    /// Human-readable name for the UI.
    pub display_name: &'static str,
    /// The name as `tell application "..."` expects. For most Apple
    /// apps this matches the file name; for some it doesn't
    /// (e.g. "System Events", "Finder").
    pub osa_name: &'static str,
    /// The bundle id — used to show a deep link into System Settings
    /// once the user has granted.
    pub bundle_id: &'static str,
    /// One-line description of what MaxBot can do once access is granted.
    pub description: &'static str,
}

const APPS: &[ControllableApp] = &[
    ControllableApp {
        key: "system_events",
        display_name: "System Events",
        osa_name: "System Events",
        bundle_id: "com.apple.systemevents",
        description: "Required for: clipboard, frontmost app, system volume, dark mode, window list. The core enabler for most Computer Use tools.",
    },
    ControllableApp {
        key: "mail",
        display_name: "Mail",
        osa_name: "Mail",
        bundle_id: "com.apple.mail",
        description: "Required for: mail_inbox, mail_send, mail_draft, mail_search.",
    },
    ControllableApp {
        key: "calendar",
        display_name: "Calendar",
        osa_name: "Calendar",
        bundle_id: "com.apple.iCal",
        description: "Required for: calendar_today, calendar_week, calendar_create_event.",
    },
    ControllableApp {
        key: "reminders",
        display_name: "Reminders",
        osa_name: "Reminders",
        bundle_id: "com.apple.Reminders",
        description: "Required for: reminders_list, reminders_add, reminders_complete.",
    },
    ControllableApp {
        key: "notes",
        display_name: "Notes",
        osa_name: "Notes",
        bundle_id: "com.apple.Notes",
        description: "Required for: notes_search, notes_read, notes_create.",
    },
    ControllableApp {
        key: "safari",
        display_name: "Safari",
        osa_name: "Safari",
        bundle_id: "com.apple.Safari",
        description: "Required for: safari_open, safari_exec_js, safari_tabs, safari_current_url.",
    },
    ControllableApp {
        key: "chrome",
        display_name: "Google Chrome",
        osa_name: "Google Chrome",
        bundle_id: "com.google.Chrome",
        description: "Required for: chrome_open, chrome_exec_js, chrome_tabs, chrome_current_url. Will only work if Chrome is installed.",
    },
    ControllableApp {
        key: "finder",
        display_name: "Finder",
        osa_name: "Finder",
        bundle_id: "com.apple.finder",
        description: "Required for: file operations beyond file_read / file_write. Already works for our existing file tools.",
    },
];

#[tauri::command]
pub async fn list_controllable_apps() -> Result<Vec<ControllableApp>, String> {
    Ok(APPS.to_vec())
}

#[derive(Serialize, Clone)]
pub struct TccProbeResult {
    pub granted: bool,
    /// Raw message from the probe — either the app name (on success)
    /// or the osascript error (on failure). Useful for debugging.
    pub message: String,
}

/// Trigger the first-run macOS prompt for the given app and report
/// whether the call ultimately succeeded. The OS shows the prompt
/// when TCC is in the "not determined" state; if it's already granted
/// the call returns immediately; if it's denied the call fails and
/// the user has to fix it in System Settings.
#[tauri::command]
pub async fn request_tcc_for(key: String) -> Result<TccProbeResult, String> {
    let app = APPS
        .iter()
        .find(|a| a.key == key)
        .ok_or_else(|| format!("unknown app key: {key}"))?;
    let granted = probe_tcc(app.osa_name).await?;
    Ok(TccProbeResult {
        granted,
        message: if granted {
            format!("granted — {} responded", app.display_name)
        } else {
            format!(
                "{} did not respond. Open System Settings → Privacy & \
                 Security → Automation and turn on {} for MaxBot, then \
                 click Request access again.",
                app.display_name, app.display_name
            )
        },
    })
}

/// Open System Settings directly to the Automation pane so the user
/// can grant/deny at their own pace. Uses the `x-apple.systempreferences:`
/// URL scheme.
#[tauri::command]
pub async fn open_automation_settings(app: AppHandle) -> Result<(), String> {
    // The Privacy_Automation preference is at
    //   x-apple.systempreferences:com.apple.preference.security?Privacy_Automation
    // but on some macOS versions the URL is just the parent pane. We
    // try the Automation-deep-link first, fall back to the parent.
    let candidates = [
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Automation",
        "x-apple.systempreferences:com.apple.preference.security",
    ];
    for url in candidates {
        if app.opener().open_url(url, None::<&str>).is_ok() {
            return Ok(());
        }
    }
    Err("could not open System Settings".to_string())
}
