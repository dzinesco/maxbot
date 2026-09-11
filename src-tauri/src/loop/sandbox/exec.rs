//! `exec` tool — run a shell command inside `sandbox-exec`.
//!
//! The shell command itself is passed to `sh -c`. macOS's
//! `sandbox-exec` runs the child under a Seatbelt sandbox profile
//! built by [`build_profile`].
//!
//! ## Profile strategy
//!
//! On macOS 27 (Darwin 27) we found that `(deny default)` combined
//! with `(allow process-exec)` causes `sandbox-exec` to SIGABRT
//! during profile validation — this is a known issue with
//! restrictive Seatbelt profiles on that kernel. The profile
//! therefore uses the **deny-then-allow** strategy: start with
//! `(allow default)` (open everything) and deny the locations
//! the agent must not reach. The denies are evaluated AFTER the
//! default allow, so a deny rule on `/private/etc` blocks
//! `/etc/passwd` (which the kernel resolves to
//! `/private/etc/passwd`).
//!
//! The profile denies:
//!
//!   * `file-read` on `/private/etc` — blocks `/etc/passwd` and
//!     friends.
//!   * `file-read` and `file-write` on
//!     `/Users/<user>/Library/Application Support/com.maxbot.app.devtools`
//!     — defense in depth for the passphrase DB.
//!   * `file-write` on `/Users`, `/etc`, `/var`, `/private/tmp`
//!     (with an explicit allow for the sandbox subpath on top of
//!     the `/private/tmp` deny).
//!   * Network — `(deny network*)` blocks both inbound and
//!     outbound. The supervisor calls the model API outside the
//!     sandbox; `exec` is for the agent's shell-call surface only.
//!
//! Both manifest as a non-zero exit code with stderr mentioning
//! `permission denied` / `Operation not permitted`.

use std::path::Path;
use std::process::Command;

use super::error::SandboxError;

/// Output of a sandboxed command.
#[derive(Debug, Clone)]
pub struct ExecOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl ExecOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }
}

/// Run `cmd` in a `sandbox-exec` child. Returns the captured
/// stdout/stderr and exit code.
///
/// The child runs with `cwd = sandbox_root`, so callers can pass
/// relative paths like `cat hello.txt` and have them resolve
/// against the sandbox. The kernel resolves `sandbox_root` to its
/// canonical form internally, so the relative-path lookups line
/// up with the `(subpath ...)` rules in the profile.
pub fn run_sandboxed(cmd: &str, sandbox_root: &Path) -> Result<ExecOutput, SandboxError> {
    let profile = build_profile(sandbox_root);

    let output = Command::new("/usr/bin/sandbox-exec")
        .arg("-p")
        .arg(&profile)
        .arg("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(sandbox_root)
        .output()
        .map_err(|e| SandboxError::SandboxExecLaunch(e.to_string()))?;

    Ok(ExecOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Build the Seatbelt profile string.
///
/// `sandbox_root` is canonicalized first so the deny/allow rules
/// match against the same form the kernel uses when the child
/// opens a file. On macOS, `/tmp` is a symlink to `/private/tmp`;
/// the child sees both as the same inode but Seatbelt evaluates
/// `(subpath ...)` against the canonical path, so we have to
/// embed the canonical form.
///
/// ## Rule ordering
///
/// Seatbelt processes rules in order and the LAST matching rule
/// wins. We start with `(allow default)` (allow everything) and
/// then layer on broad denies for sensitive locations. The
/// sandbox allow for `file-write*` MUST come AFTER the broad
/// `/private/var` deny, otherwise the broad deny matches the
/// sandbox subpath (it's a subpath of `/private/var`) and
/// shadows the allow.
pub fn build_profile(sandbox_root: &Path) -> String {
    let canon_root = std::fs::canonicalize(sandbox_root)
        .unwrap_or_else(|_| sandbox_root.to_path_buf());
    let root = canon_root.to_string_lossy();
    let home_db = "/Users/me/Library/Application Support/com.maxbot.app.devtools";
    format!(
        r#"(version 1)
(allow default)
(deny file-read* (subpath "/private/etc"))
(deny file-read* (subpath "{home_db}"))
(deny file-write* (subpath "{home_db}"))
(deny file-write* (subpath "/private/tmp"))
(deny file-write* (subpath "/Users"))
(deny file-write* (subpath "/etc"))
(deny file-write* (subpath "/var"))
(deny file-write* (subpath "/private/var"))
(allow file-write* (subpath "{root}"))
(deny network*)
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture_root() -> PathBuf {
        // Per-test isolated fixture under $TMPDIR so parallel
        // `cargo test` threads don't race on a shared dir.
        let tag = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        );
        let base = std::env::temp_dir().join(format!("maxbot-exec-test-{tag}"));
        let root = base.join("loop/sandbox");
        let _ = std::fs::create_dir_all(&root);
        if !root.join("hello.txt").exists() {
            std::fs::write(root.join("hello.txt"), b"hi from sandbox").unwrap();
        }
        root
    }

    #[test]
    fn profile_smoke_test() {
        let root = fixture_root();
        let profile = build_profile(&root);
        let canon = root.canonicalize().unwrap();
        assert!(
            profile.contains(&canon.to_string_lossy().as_ref()),
            "profile missing canonical root {}; profile was:\n{profile}",
            canon.display()
        );
        assert!(profile.contains("(deny network*)"));
    }

    #[test]
    fn cat_inside_sandbox_succeeds() {
        let root = fixture_root();
        let out = run_sandboxed("cat hello.txt", &root).unwrap();
        assert!(
            out.success(),
            "expected success; status={}, stderr={}",
            out.status,
            out.stderr
        );
        assert!(out.stdout.contains("hi from sandbox"));
    }

    #[test]
    fn cat_etc_passwd_fails() {
        let root = fixture_root();
        let out = run_sandboxed("cat /etc/passwd", &root).unwrap();
        assert!(!out.success(), "expected non-zero exit; got success");
        let lc = out.stderr.to_lowercase();
        assert!(
            lc.contains("permission")
                || lc.contains("denied")
                || lc.contains("operation not permitted"),
            "stderr should mention permission denial; got: {}",
            out.stderr
        );
    }

    #[test]
    fn cat_maxbot_sqlite_fails() {
        let root = fixture_root();
        let out = run_sandboxed(
            "cat \"/Users/me/Library/Application Support/com.maxbot.app.devtools/maxbot.sqlite\"",
            &root,
        )
        .unwrap();
        assert!(!out.success(), "expected non-zero exit; got success");
        let lc = out.stderr.to_lowercase();
        assert!(
            lc.contains("permission")
                || lc.contains("denied")
                || lc.contains("operation not permitted")
                || lc.contains("no such file"),
            "stderr should mention permission/IO error; got: {}",
            out.stderr
        );
    }

    #[test]
    fn write_inside_sandbox_succeeds() {
        let root = fixture_root();
        eprintln!("root={:?} canon={:?}", root, std::fs::canonicalize(&root));
        let profile = build_profile(&root);
        eprintln!("profile:\n{profile}");
        let out = run_sandboxed("sh -c 'echo new > new.txt'", &root).unwrap();
        assert!(
            out.success(),
            "expected success; status={}, stderr={}",
            out.status,
            out.stderr
        );
        let written = std::fs::read_to_string(root.join("new.txt")).unwrap();
        assert_eq!(written.trim(), "new");
    }

    #[test]
    fn write_outside_sandbox_fails() {
        let root = fixture_root();
        let target = std::env::temp_dir().join(format!(
            "maxbot-exec-outside-{}-{}.txt",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        let _ = std::fs::remove_file(&target);
        let cmd = format!("sh -c 'echo bad > \"{}\"'", target.display());
        let out = run_sandboxed(&cmd, &root).unwrap();
        assert!(
            !out.success(),
            "expected write-outside to fail; got status={} stderr={}",
            out.status,
            out.stderr
        );
        assert!(
            !target.exists(),
            "target should not have been created; got: {}",
            target.display()
        );
    }
}
