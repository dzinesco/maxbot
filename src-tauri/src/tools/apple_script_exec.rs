//! Shared AppleScript executor. Every AppleScript-based tool (Mail,
//! Calendar, Safari, the `apple_script_run` escape hatch, etc.) routes
//! through `run_osa_script` so we get consistent timeout behavior,
//! error formatting, and TCC denial detection.
//!
//! On TCC denial, `osascript` exits non-zero with a stderr like:
//!   `execution error: Not authorized to send Apple events to Mail. (-1743)`
//! or older:
//!   `Not authorized to send Apple events to com.apple.mail. (-1743)`
//! We detect the `-1743` error code and the "Not authorized" /
//! "permission" string, then surface a friendly hint pointing the user
//! at System Settings → Privacy & Security → Automation.

use std::time::Duration;

use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct AppleScriptOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    /// True if the failure was a TCC permission denial. The caller can
    /// surface a "Grant access in System Settings → Privacy & Security
    /// → Automation" hint to the user instead of treating it as a
    /// generic AppleScript error.
    pub tcc_denied: bool,
}

impl AppleScriptOutput {
    pub fn succeeded(&self) -> bool {
        self.exit_code == Some(0)
    }
    pub fn tcc_hint(&self) -> Option<&'static str> {
        if self.tcc_denied {
            Some(
                "macOS denied the request to control that app. Open System \
                 Settings → Privacy & Security → Automation, find MaxBot, \
                 and turn on the app. Then click Request access again.",
            )
        } else {
            None
        }
    }
}

/// Run an AppleScript via `osascript -e`. Returns the captured output
/// plus a flag indicating whether the failure was a TCC denial.
pub async fn run_osa_script(
    script: &str,
    timeout_secs: u64,
) -> Result<AppleScriptOutput, String> {
    let mut cmd = Command::new("osascript");
    cmd.arg("-e").arg(script);
    let output = match tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("osascript spawn failed: {e}")),
        Err(_) => {
            return Err(format!(
                "osascript exceeded {timeout_secs}s timeout. If this was a \
                 long-running task, run it from a `do shell script` block \
                 inside the script and stream output manually."
            ));
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let exit_code = output.status.code();
    let tcc_denied = detect_tcc_denial(&stderr, exit_code);
    Ok(AppleScriptOutput {
        stdout,
        stderr,
        exit_code,
        tcc_denied,
    })
}

/// Detect TCC denial. We look for the well-known error code -1743
/// (`errAEEventNotHandled`) plus the "Not authorized" / "permission"
/// string. This is the same signature across Mail, Calendar, Reminders,
/// Notes, Safari, Chrome, and System Events.
fn detect_tcc_denial(stderr: &str, exit_code: Option<i32>) -> bool {
    if exit_code == Some(-1743) {
        return true;
    }
    let lower = stderr.to_lowercase();
    lower.contains("not authorized")
        || lower.contains("permission")
        || lower.contains("appleevent permission")
}

#[cfg(target_os = "macos")]
mod macos_probe {
    use super::run_osa_script;
    use std::time::Duration;

    /// Run a tiny probe AppleScript that touches a target app. If TCC is
    /// granted, this returns Ok(true) without UI; if not, the OS shows
    /// the first-run prompt and the script either completes (if the
    /// user granted) or fails (if the user denied). Either way we
    /// return a structured outcome.
    pub async fn probe_tcc(app_osa_name: &str) -> Result<bool, String> {
        let script = format!(
            r#"tell application "{}" to get name"#,
            app_osa_name
        );
        // First attempt — likely triggers the prompt if TCC is unset.
        let first = run_osa_script(&script, 5).await?;
        if first.succeeded() {
            return Ok(true);
        }
        // If the first call didn't show a prompt and just failed, give
        // it one more shot — sometimes the prompt races the script.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let second = run_osa_script(&script, 5).await?;
        Ok(second.succeeded())
    }
}

#[cfg(not(target_os = "macos"))]
mod macos_probe {
    pub async fn probe_tcc(_app_osa_name: &str) -> Result<bool, String> {
        Ok(false)
    }
}

pub use macos_probe::probe_tcc;

#[cfg(test)]
mod tests {
    use super::detect_tcc_denial;

    #[test]
    fn tcc_denied_via_exit_code() {
        assert!(detect_tcc_denial("anything", Some(-1743)));
    }

    #[test]
    fn tcc_denied_via_message() {
        assert!(detect_tcc_denial(
            "execution error: Not authorized to send Apple events to Mail. (-1743)",
            Some(1),
        ));
    }

    #[test]
    fn tcc_denied_via_permission_keyword() {
        assert!(detect_tcc_denial(
            "37:50: execution error: Mail got an error: AppleEvent permission required.",
            Some(1),
        ));
    }

    #[test]
    fn not_tcc_when_unrelated_error() {
        assert!(!detect_tcc_denial(
            "execution error: The variable someVar is not defined. (-2753)",
            Some(1),
        ));
    }
}
