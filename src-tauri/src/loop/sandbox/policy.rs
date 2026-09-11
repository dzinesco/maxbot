//! Path-scoping policy for the sandbox.
//!
//! Every file-tool call funnels through [`validate_path`]. The
//! function is the single load-bearing gate that decides whether a
//! requested path is allowed:
//!
//! 1. **Substring deny-list.** If the requested path's string form
//!    contains `passphrase` (case insensitive), or if the path's
//!    filename is `maxbot.sqlite`, reject with
//!    [`SandboxError::PassphraseDbInaccessible`]. This is a
//!    defense-in-depth check; it fires regardless of where the
//!    sandbox root lives, so the agent can never reach the
//!    passphrase DB even if the sandbox is misconfigured.
//!
//! 2. **Absolute / relative resolution.** Relative paths are anchored
//!    to the sandbox root; absolute paths are taken as-is.
//!
//! 3. **Canonicalization.** Both the requested path and the sandbox
//!    root are canonicalized with [`std::fs::canonicalize`]. On
//!    macOS this resolves `/tmp` to `/private/tmp`, symlinks to
//!    their targets, and `..` segments. This is what blocks
//!    `..` traversal and symlink escape uniformly — by the time we
//!    look at the canonical path, the `..` and the symlink hop are
//!    gone, and a single `starts_with` check is authoritative.
//!
//! 4. **Prefix check.** The canonical path must start with the
//!    canonical sandbox root. Anything else is rejected. We use
//!    [`Path::starts_with`] (component-wise, not byte-wise), which
//!    correctly handles the trailing-separator case and avoids the
//!    `/sandbox-evil` vs `/sandbox` prefix collision that a naive
//!    `str::starts_with` would miss.
//!
//! 5. **Symlink escape vs. plain outside.** If the canonical path
//!    differs from the input, the rejection is reported as
//!    [`SandboxError::SymlinkEscape`] (the more informative
//!    variant). Otherwise it's [`SandboxError::PathOutsideSandbox`].
//!
//! ## Files that don't exist yet
//!
//! `write_file` may be called with a path that doesn't exist yet.
//! In that case, `canonicalize` fails on the leaf. We canonicalize
//! the deepest existing ancestor (parent chain) and re-append the
//! missing tail; the prefix check still applies.

use std::path::{Path, PathBuf};

use super::error::SandboxError;

/// Resolve `input` to a canonical absolute path and verify it lives
/// inside `sandbox_root`. Returns the canonical path on success.
pub fn validate_path(input: &Path, sandbox_root: &Path) -> Result<PathBuf, SandboxError> {
    // (1) Substring deny-list. Run before any IO so a forbidden
    //     path never touches the disk.
    let lowered = input.to_string_lossy().to_lowercase();
    if lowered.contains("passphrase") {
        return Err(SandboxError::PassphraseDbInaccessible {
            requested: input.to_path_buf(),
        });
    }
    if input.file_name().map(|n| n == "maxbot.sqlite").unwrap_or(false) {
        return Err(SandboxError::PassphraseDbInaccessible {
            requested: input.to_path_buf(),
        });
    }

    // (2) Absolute or anchored-to-root.
    let abs: PathBuf = if input.is_absolute() {
        input.to_path_buf()
    } else {
        sandbox_root.join(input)
    };

    // (3) Canonicalize. Handle the "file does not exist" case by
    //     walking up the parent chain until we find something that
    //     does exist, canonicalize that, then re-append the missing
    //     tail.
    let canonical = canonicalize_with_missing_tail(&abs)?;

    let canonical_root = std::fs::canonicalize(sandbox_root)?;

    // (4) Prefix check.
    if !canonical.starts_with(&canonical_root) {
        // (5) Distinguish symlink escape from explicit outside.
        if canonical != abs {
            return Err(SandboxError::SymlinkEscape {
                requested: abs,
                resolved: canonical,
            });
        }
        return Err(SandboxError::PathOutsideSandbox {
            requested: abs,
            resolved: canonical,
            sandbox_root: canonical_root,
        });
    }

    Ok(canonical)
}

/// Canonicalize `path`, or — if it doesn't exist — canonicalize the
/// deepest existing ancestor and re-append the missing tail.
fn canonicalize_with_missing_tail(path: &Path) -> Result<PathBuf, SandboxError> {
    match std::fs::canonicalize(path) {
        Ok(p) => Ok(p),
        Err(_) => {
            // Walk up looking for an existing ancestor.
            let mut existing: PathBuf = path.to_path_buf();
            let mut missing: Vec<PathBuf> = Vec::new();
            loop {
                if existing.exists() {
                    break;
                }
                let parent = match existing.parent() {
                    Some(p) => p.to_path_buf(),
                    None => break,
                };
                let name = match existing.file_name() {
                    Some(n) => n.to_os_string(),
                    None => break,
                };
                missing.push(PathBuf::from(name));
                existing = parent;
            }
            let mut canon = std::fs::canonicalize(&existing)?;
            // Re-append the missing tail in reverse order.
            for part in missing.iter().rev() {
                canon.push(part);
            }
            Ok(canon)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    /// Each policy test gets its own isolated fixture directory
    /// under `$TMPDIR/maxbot-policy-test-<pid>-<nanos>/...`. We do
    /// not delete-and-recreate a shared dir because parallel test
    /// threads can race the same path.
    fn fixture() -> (PathBuf, PathBuf) {
        let tag = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        );
        let base = std::env::temp_dir().join(format!("maxbot-policy-test-{tag}"));
        let root = base.join("loop/sandbox");
        static INIT: Once = Once::new();
        // Ensure the dir exists (cheap; idempotent) and `inside.txt`
        // is written once per fixture lifetime.
        INIT.call_once(|| {
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("inside.txt"), b"inside").unwrap();
        });
        // Also create the directory lazily for tests that may run
        // before INIT, and ensure the file is present.
        let _ = std::fs::create_dir_all(&root);
        if !root.join("inside.txt").exists() {
            std::fs::write(root.join("inside.txt"), b"inside").unwrap();
        }
        (base, root)
    }

    #[test]
    fn inside_sandbox_accepted() {
        let (base, root) = fixture();
        let file = base.join("loop/sandbox/inside.txt");
        let p = validate_path(&file, &root).unwrap();
        // Compare canonical-to-canonical because macOS
        // resolves `/tmp` and `/var/folders/...` to their
        // `/private/...` form.
        let canon_root = std::fs::canonicalize(&root).unwrap();
        assert!(p.starts_with(&canon_root));
    }

    #[test]
    fn outside_sandbox_rejected() {
        let (base, root) = fixture();
        let outside = base.join("loop/TASK.md");
        std::fs::create_dir_all(base.join("loop")).unwrap();
        std::fs::write(&outside, "x").unwrap();
        let err = validate_path(&outside, &root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path outside sandbox") || msg.contains("symlink escape"),
            "expected path-outside rejection, got: {msg}"
        );
    }

    #[test]
    fn passphrase_substring_rejected() {
        let (_base, root) = fixture();
        let bad = root.join("passphrase_store.txt");
        let err = validate_path(&bad, &root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("passphrase DB inaccessible"),
            "expected passphrase DB rejection, got: {msg}"
        );
    }

    #[test]
    fn maxbot_sqlite_filename_rejected() {
        let (_base, root) = fixture();
        let bad = root.join("maxbot.sqlite");
        let err = validate_path(&bad, &root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("passphrase DB inaccessible"),
            "expected passphrase DB rejection, got: {msg}"
        );
    }

    #[test]
    fn traversal_blocked() {
        let (base, root) = fixture();
        let bad = base.join("loop/sandbox/../../etc/passwd");
        let err = validate_path(&bad, &root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path outside sandbox") || msg.contains("symlink escape"),
            "expected path-outside rejection, got: {msg}"
        );
    }

    #[test]
    fn symlink_escape_blocked() {
        let (base, root) = fixture();
        let link = root.join("escape");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        let err = validate_path(&link, &root).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path outside sandbox") || msg.contains("symlink escape"),
            "expected symlink-escape rejection, got: {msg}"
        );
    }
}
