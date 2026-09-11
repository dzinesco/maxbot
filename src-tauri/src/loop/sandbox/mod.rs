//! Sandbox bind — Sub-slice C of Slice 1 (keep-alive loop).
//!
//! This module is the agent's tool surface for filesystem and shell
//! access. It exists so the agent loop can call `read_file`,
//! `write_file`, `list_dir`, and `exec` and be guaranteed that no
//! path it touches escapes its root.
//!
//! ## What it does
//!
//!   * **Path scoping.** Every file tool funnels through
//!     [`policy::validate_path`], which canonicalizes the input and
//!     enforces a prefix match against the sandbox root. `..`
//!     traversal, symlink hops, and absolute paths from outside the
//!     sandbox are all rejected — by the time we look at the
//!     canonical path, those escapes have been resolved away.
//!
//!   * **Defense-in-depth deny-list.** Any path containing the
//!     substring `passphrase` (case insensitive) or naming
//!     `maxbot.sqlite` is rejected with
//!     [`SandboxError::PassphraseDbInaccessible`]. This fires
//!     regardless of where the sandbox root lives, so the agent
//!     can never reach the passphrase DB even if the sandbox is
//!     misconfigured.
//!
//!   * **Exec scoping.** `exec` runs the command in a `sandbox-exec`
//!     child with a Seatbelt profile that restricts file I/O to
//!     the sandbox root + the standard system library paths, and
//!     denies network. `cat /etc/passwd` and
//!     `cat ~/Library/.../maxbot.sqlite` both fail closed at the
//!     kernel layer.
//!
//! ## What it doesn't do
//!
//!   * It does not call the model. The supervisor (in `loop/daemon/`)
//!     calls the model API outside the sandbox; the sandbox is
//!     only the tool surface.
//!   * It does not implement the keep-alive loop itself. The
//!     supervisor is responsible for the turn cycle.
//!   * It does not implement async I/O. All calls are synchronous
//!     (`std::fs`, `std::process`). The supervisor can wrap with
//!     `tokio::task::spawn_blocking` if it needs an async API.
//!
//! ## Sandbox root
//!
//! The production root is `<data_dir>/loop/sandbox/`, where
//! `data_dir` is the Tauri `app_data_dir()` (typically
//! `~/Library/Application Support/com.maxbot.app.devtools/`). The
//! `Sandbox::new` constructor takes any path; the production wiring
//! is the supervisor's responsibility. This module does not call
//! `app_data_dir()` itself.

mod error;
mod exec;
mod policy;

use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use error::SandboxError;
pub use exec::ExecOutput;

use policy::validate_path;

/// A scoped filesystem + exec surface rooted at a single canonical
/// directory. Cheap to clone (the root is wrapped in `Arc`).
#[derive(Debug, Clone)]
pub struct Sandbox {
    root: Arc<PathBuf>,
}

/// One entry returned from [`Sandbox::list_dir`].
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub is_file: bool,
    pub is_dir: bool,
}

impl Sandbox {
    /// Construct a sandbox rooted at `root`. The root must already
    /// exist as a directory. The path is canonicalized eagerly so
    /// every later path check is component-wise against the same
    /// root.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, SandboxError> {
        let root = root.into();
        let meta = std::fs::metadata(&root)?;
        if !meta.is_dir() {
            return Err(SandboxError::InvalidRoot(root));
        }
        let canonical = std::fs::canonicalize(&root)?;
        Ok(Self {
            root: Arc::new(canonical),
        })
    }

    /// The canonical sandbox root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Read the bytes at `path`. The path must be inside the
    /// sandbox (see module docs). Missing files return
    /// [`SandboxError::Io`] wrapping `NotFound`.
    pub fn read_file(&self, path: impl AsRef<Path>) -> Result<Vec<u8>, SandboxError> {
        let resolved = validate_path(path.as_ref(), &self.root)?;
        Ok(std::fs::read(&resolved)?)
    }

    /// Write `content` to `path`. The path must be inside the
    /// sandbox. Missing parent directories are created.
    pub fn write_file(
        &self,
        path: impl AsRef<Path>,
        content: &[u8],
    ) -> Result<(), SandboxError> {
        let resolved = validate_path(path.as_ref(), &self.root)?;
        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&resolved, content)?;
        Ok(())
    }

    /// List the immediate entries of `path`. The path must be a
    /// directory inside the sandbox.
    pub fn list_dir(&self, path: impl AsRef<Path>) -> Result<Vec<DirEntry>, SandboxError> {
        let resolved = validate_path(path.as_ref(), &self.root)?;
        let read = std::fs::read_dir(&resolved)?;
        let mut out = Vec::new();
        for entry in read {
            let entry = entry?;
            let ft = entry.file_type()?;
            out.push(DirEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_file: ft.is_file(),
                is_dir: ft.is_dir(),
            });
        }
        Ok(out)
    }

    /// Run a shell command (`sh -c <cmd>`) inside a `sandbox-exec`
    /// child. The sandbox profile restricts file I/O to the
    /// sandbox root and denies network. See [`exec::run_sandboxed`].
    pub fn exec(&self, cmd: &str) -> Result<ExecOutput, SandboxError> {
        exec::run_sandboxed(cmd, &self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build the test fixture at `/tmp/slice1-sandbox-test/` and
    /// return a `Sandbox` rooted at `<fixture>/loop/sandbox/`.
    /// Per the brief, tests run against this fixture; the real
    /// `~/Library/Application Support/com.maxbot.app.devtools/`
    /// is NOT touched.
    ///
    /// We do NOT delete and recreate the fixture on every call —
    /// `cargo test` runs tests in parallel by default, and a
    /// racing `remove_dir_all` would intermittently fail
    /// `Sandbox::new` (canonicalize) on a sibling test. Instead
    /// we ensure the dir + `test.txt` exist once.
    fn fixture() -> Sandbox {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let base = PathBuf::from("/tmp/slice1-sandbox-test");
            let root = base.join("loop/sandbox");
            let _ = std::fs::remove_dir_all(&base);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("test.txt"), b"hello sandbox").unwrap();
        });
        let base = PathBuf::from("/tmp/slice1-sandbox-test");
        let root = base.join("loop/sandbox");
        Sandbox::new(&root).unwrap()
    }

    #[test]
    fn test_1_read_inside_sandbox_succeeds() {
        let sb = fixture();
        let file = std::path::PathBuf::from("/tmp/slice1-sandbox-test/loop/sandbox/test.txt");
        let content = sb.read_file(&file).unwrap();
        assert_eq!(content, b"hello sandbox");
    }

    #[test]
    fn test_2_read_outside_sandbox_fails() {
        let sb = fixture();
        std::fs::create_dir_all("/tmp/slice1-sandbox-test/loop").unwrap();
        std::fs::write("/tmp/slice1-sandbox-test/loop/TASK.md", "outside").unwrap();
        let bad = std::path::PathBuf::from("/tmp/slice1-sandbox-test/loop/TASK.md");
        let err = sb.read_file(&bad).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path outside sandbox") || msg.contains("symlink escape"),
            "expected path-outside rejection, got: {msg}"
        );
    }

    #[test]
    fn test_3_read_passphrase_db_fails() {
        let sb = fixture();
        let bad = std::path::PathBuf::from(
            "/Users/me/Library/Application Support/com.maxbot.app.devtools/maxbot.sqlite",
        );
        let err = sb.read_file(&bad).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("passphrase DB inaccessible"),
            "expected 'passphrase DB inaccessible' in error, got: {msg}"
        );
    }

    #[test]
    fn test_4_exec_outside_sandbox_fails() {
        let sb = fixture();
        let out = sb.exec("cat /etc/passwd").unwrap();
        assert!(
            !out.success(),
            "expected non-zero exit; status={} stdout={} stderr={}",
            out.status, out.stdout, out.stderr
        );
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
    fn test_5_exec_passphrase_db_fails() {
        let sb = fixture();
        let out = sb
            .exec(
                "cat \"/Users/me/Library/Application Support/com.maxbot.app.devtools/maxbot.sqlite\"",
            )
            .unwrap();
        assert!(
            !out.success(),
            "expected non-zero exit; status={} stdout={} stderr={}",
            out.status, out.stdout, out.stderr
        );
        let lc = out.stderr.to_lowercase();
        assert!(
            lc.contains("permission")
                || lc.contains("denied")
                || lc.contains("operation not permitted")
                || lc.contains("no such file"),
            "stderr should mention permission or IO error; got: {}",
            out.stderr
        );
    }

    #[test]
    fn test_6_path_traversal_blocked() {
        let sb = fixture();
        let bad = std::path::PathBuf::from(
            "/tmp/slice1-sandbox-test/loop/sandbox/../../etc/passwd",
        );
        let err = sb.read_file(&bad).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path outside sandbox") || msg.contains("symlink escape"),
            "expected path-outside rejection, got: {msg}"
        );
    }

    #[test]
    fn test_7_symlink_traversal_blocked() {
        let sb = fixture();
        let link = std::path::PathBuf::from("/tmp/slice1-sandbox-test/loop/sandbox/escape");
        std::os::unix::fs::symlink("/etc/passwd", &link).unwrap();
        let err = sb.read_file(&link).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path outside sandbox") || msg.contains("symlink escape"),
            "expected symlink-escape rejection, got: {msg}"
        );
    }

    #[test]
    fn bonus_write_inside_sandbox_succeeds() {
        let sb = fixture();
        let target = std::path::PathBuf::from("/tmp/slice1-sandbox-test/loop/sandbox/out.txt");
        sb.write_file(&target, b"written").unwrap();
        let read = std::fs::read_to_string(&target).unwrap();
        assert_eq!(read, "written");
    }

    #[test]
    fn bonus_list_dir_returns_entries() {
        let sb = fixture();
        let entries = sb
            .list_dir(std::path::PathBuf::from(
                "/tmp/slice1-sandbox-test/loop/sandbox",
            ))
            .unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"test.txt"), "got: {:?}", names);
    }

    #[test]
    fn bonus_write_outside_sandbox_rejected() {
        let sb = fixture();
        let bad = std::path::PathBuf::from("/tmp/slice1-sandbox-test/loop/evil.txt");
        let err = sb.write_file(&bad, b"x").unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("path outside sandbox") || msg.contains("symlink escape"),
            "expected path-outside rejection, got: {msg}"
        );
    }
}
