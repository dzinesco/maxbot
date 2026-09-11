//! v3.7.17 — Bundle-identifier-rename data dir migration.
//!
//! macOS derives a Tauri app's per-user data dir from its
//! `CFBundleIdentifier`. v3.7.17 renames the identifier from
//! `com.maxbot.app` to `com.maxbot.app.devtools` to escape a WebKit
//! memory leak keyed on the original identifier (see
//! `docs/leak-hunt-2026-09-11.md`). Existing v3.7.16 users have their
//! state in `~/Library/Application Support/com.maxbot.app/`; on the
//! first v3.7.17 launch we copy that directory to the new location
//! so they keep their profiles.
//!
//! ## Behavior
//!
//! - If `old_dir` exists and `new_dir` is missing OR empty: copy `old_dir`
//!   contents recursively into `new_dir`. Old dir is left in place (Tyler
//!   wants it preserved for one release so a downgrade recovers cleanly).
//! - If `old_dir` does not exist: no-op. Fresh installs.
//! - If both exist and `new_dir` is non-empty: skip the copy and log a
//!   warning. Do NOT merge blindly — Tyler's directive: "don't merge
//!   blindly." A user who has already launched v3.7.17 once has their
//!   new dir; if they're returning to v3.7.16 then back, the old dir
//!   is the source of truth and the new dir is the destination, so a
//!   second copy on top of a populated new dir would silently corrupt
//!   the newer state.
//!
//! ## Idempotency
//!
//! Running the migration twice on the same (source, dest) is safe: the
//! second run sees `new_dir` non-empty and skips. We never delete the
//! old dir, so re-running is fully recoverable.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Outcome of a migration attempt. Logged to `log::info!` on the way out
/// so the user / dev can see what happened in the launch log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Old dir did not exist. Fresh install — nothing to do.
    NoOp,
    /// Old dir existed, new dir was missing/empty, copy succeeded.
    Migrated { copied_entries: usize },
    /// Both existed and new dir was non-empty. Skipped (Tyler's rule).
    Skipped,
    /// The copy was attempted but failed partway. The new dir may be
    /// partially populated; we leave the old dir untouched. Surfaced so
    /// the caller can decide whether to abort startup.
    Failed { error: String },
}

/// Migration entry point. Takes the resolved Tauri `app_data_dir` (the
/// NEW path, derived from `com.maxbot.app.devtools`) and computes the
/// OLD path by replacing `com.maxbot.app.devtools` with `com.maxbot.app`
/// in the final component. Falls back to the parent dir + literal old
/// name if the rewrite doesn't match (e.g. tests with synthetic paths).
pub fn run_for_app_data_dir(new_dir: &Path) -> MigrationOutcome {
    let old_dir = derive_old_dir(new_dir);
    run(&old_dir, new_dir)
}

/// Test-friendly entry point. The unit tests pass synthetic paths; the
/// production code path uses `run_for_app_data_dir`.
///
/// Returns one of the four `MigrationOutcome` variants. The caller is
/// responsible for logging (this function does not touch `log` itself so
/// it stays trivially testable).
pub fn run(old_dir: &Path, new_dir: &Path) -> MigrationOutcome {
    // Case 1: old dir missing — fresh install. No-op.
    if !old_dir.exists() {
        return MigrationOutcome::NoOp;
    }

    // Case 2: new dir already populated. Skip per Tyler's directive.
    //   "If both exist and the new dir is non-empty: DO NOT merge
    //    blindly. Skip the copy and log a warning."
    if new_dir_non_empty(new_dir) {
        return MigrationOutcome::Skipped;
    }

    // Case 3: old dir exists, new dir is missing or empty. Copy.
    match copy_dir_recursive(old_dir, new_dir) {
        Ok(n) => MigrationOutcome::Migrated { copied_entries: n },
        Err(e) => MigrationOutcome::Failed {
            error: format!("copy_dir_recursive({} -> {}): {e}", old_dir.display(), new_dir.display()),
        },
    }
}

/// Derive the OLD bundle-id dir from the NEW one by rewriting the final
/// path component. This keeps the function portable — works regardless
/// of whether the new dir is the macOS canonical path or a test path.
fn derive_old_dir(new_dir: &Path) -> PathBuf {
    let mut buf = new_dir.to_path_buf();
    buf.set_file_name("com.maxbot.app");
    buf
}

/// True if `new_dir` exists and contains at least one entry. An empty
/// (just-created) dir is treated as "needs copy". A missing dir is also
/// treated as "needs copy" — caller handles creation in `copy_dir_recursive`.
fn new_dir_non_empty(new_dir: &Path) -> bool {
    match fs::read_dir(new_dir) {
        Ok(mut it) => it.next().is_some(),
        Err(_) => false, // missing or unreadable counts as "needs copy"
    }
}

/// Recursive copy. Preserves file metadata where `fs::copy` does (mtime,
/// permissions); directories are created with `create_dir_all` so nested
/// paths work. Returns the count of entries copied (files + dirs).
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<usize> {
    fs::create_dir_all(dst)?;
    let mut count = 0usize;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_child = entry.path();
        let dst_child = dst.join(entry.file_name());
        let ft = entry.file_type()?;
        if ft.is_dir() {
            count += copy_dir_recursive(&src_child, &dst_child)?;
        } else if ft.is_symlink() {
            // Re-create the symlink (don't follow + copy the target —
            // the symlink is the more faithful preservation).
            let target = fs::read_link(&src_child)?;
            std::os::unix::fs::symlink(&target, &dst_child)?;
            count += 1;
        } else {
            fs::copy(&src_child, &dst_child)?;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    /// Build a unique scratch dir under `/tmp` so parallel test runs don't
    /// collide. Returns (root, old_dir, new_dir).
    fn three_tmp_dirs(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = PathBuf::from(format!("/tmp/maxbot-migration-test-{label}-{nanos}"));
        let old = root.join("com.maxbot.app");
        let new = root.join("com.maxbot.app.devtools");
        fs::create_dir_all(&old).unwrap();
        (root, old, new)
    }

    fn teardown(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn no_op_when_old_dir_missing() {
        let (root, old, new) = three_tmp_dirs("no-op");
        // No old dir created on purpose.
        let _ = fs::remove_dir_all(&old);
        let outcome = run(&old, &new);
        assert_eq!(outcome, MigrationOutcome::NoOp);
        assert!(!new.exists(), "new dir should not be created on no-op");
        teardown(&root);
    }

    #[test]
    fn migrates_when_new_dir_missing() {
        let (root, old, new) = three_tmp_dirs("migrate-missing");
        fs::write(old.join("maxbot.sqlite"), b"fake sqlite bytes").unwrap();
        fs::create_dir_all(old.join("nested")).unwrap();
        fs::write(old.join("nested/prefs.json"), b"{}").unwrap();
        let outcome = run(&old, &new);
        match outcome {
            MigrationOutcome::Migrated { copied_entries } => {
                // `copied_entries` counts files + symlinks (each recursive
                // call adds its own file/symlink hits). The top-level
                // `nested/` dir itself is created via `create_dir_all`
                // but not counted — directories aren't file content.
                // Here: sqlite (1) + nested/prefs.json (1) = 2.
                assert!(
                    copied_entries >= 2,
                    "expected at least 2 file entries (sqlite + prefs.json), got {copied_entries}"
                );
            }
            other => panic!("expected Migrated, got {other:?}"),
        }
        assert!(new.exists(), "new dir should be created");
        assert!(new.join("maxbot.sqlite").exists(), "sqlite should be copied");
        assert!(new.join("nested/prefs.json").exists(), "nested file should be copied");
        // Old dir preserved per Tyler's directive.
        assert!(old.exists(), "old dir must be left in place");
        teardown(&root);
    }

    #[test]
    fn migrates_when_new_dir_empty() {
        let (root, old, new) = three_tmp_dirs("migrate-empty");
        fs::create_dir_all(&new).unwrap(); // exists but empty
        fs::write(old.join("data.bin"), b"x").unwrap();
        let outcome = run(&old, &new);
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));
        assert!(new.join("data.bin").exists());
        teardown(&root);
    }

    #[test]
    fn skips_when_new_dir_non_empty() {
        let (root, old, new) = three_tmp_dirs("skip");
        fs::write(old.join("old-only.txt"), b"from old dir").unwrap();
        fs::create_dir_all(&new).unwrap();
        fs::write(new.join("existing.txt"), b"already there").unwrap();
        let outcome = run(&old, &new);
        assert_eq!(outcome, MigrationOutcome::Skipped);
        // Existing file preserved.
        assert_eq!(
            fs::read_to_string(new.join("existing.txt")).unwrap(),
            "already there"
        );
        // Old file NOT copied (per the skip rule).
        assert!(!new.join("old-only.txt").exists(), "must not copy on skip");
        // Old dir preserved.
        assert!(old.exists());
        teardown(&root);
    }

    #[test]
    fn idempotent_second_run_is_no_op() {
        // First run copies.
        let (root, old, new) = three_tmp_dirs("idempotent");
        fs::write(old.join("a.txt"), b"a").unwrap();
        let first = run(&old, &new);
        assert!(matches!(first, MigrationOutcome::Migrated { .. }));

        // Second run sees new dir non-empty and skips.
        let second = run(&old, &new);
        assert_eq!(second, MigrationOutcome::Skipped);

        // If a user manually deletes the new dir between runs, the
        // third run copies again — that's the recoverable-by-design
        // behavior Tyler asked for.
        fs::remove_dir_all(&new).unwrap();
        let third = run(&old, &new);
        assert!(matches!(third, MigrationOutcome::Migrated { .. }));
        teardown(&root);
    }

    #[test]
    fn preserves_nested_directories_recursively() {
        let (root, old, new) = three_tmp_dirs("nested");
        fs::create_dir_all(old.join("a/b/c")).unwrap();
        fs::write(old.join("a/b/c/deep.txt"), b"deep").unwrap();
        fs::write(old.join("a/top.txt"), b"top").unwrap();
        let outcome = run(&old, &new);
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));
        assert!(new.join("a/b/c/deep.txt").exists());
        assert!(new.join("a/top.txt").exists());
        teardown(&root);
    }

    #[test]
    fn copy_preserves_file_contents() {
        let (root, old, new) = three_tmp_dirs("contents");
        let payload = b"\x00\x01binary\xff payload\n";
        fs::write(old.join("blob.bin"), payload).unwrap();
        let outcome = run(&old, &new);
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));
        let copied = fs::read(new.join("blob.bin")).unwrap();
        assert_eq!(copied, payload);
        teardown(&root);
    }

    #[test]
    fn derive_old_dir_rewrites_bundle_id_component() {
        let new = PathBuf::from("/Users/me/Library/Application Support/com.maxbot.app.devtools");
        let old = derive_old_dir(&new);
        assert_eq!(
            old,
            PathBuf::from("/Users/me/Library/Application Support/com.maxbot.app")
        );
    }

    #[test]
    fn run_for_app_data_dir_end_to_end() {
        // Mimic the production call shape with a synthetic path.
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let parent = PathBuf::from(format!("/tmp/maxbot-migration-e2e-{nanos}"));
        let old = parent.join("com.maxbot.app");
        let new = parent.join("com.maxbot.app.devtools");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("seed.sqlite"), b"seed").unwrap();

        let outcome = run_for_app_data_dir(&new);
        assert!(matches!(outcome, MigrationOutcome::Migrated { .. }));
        assert!(new.join("seed.sqlite").exists());
        assert!(old.exists());

        let _ = fs::remove_dir_all(&parent);
    }
}
