//! v3.7.17 — One-shot verification tool for the data dir migration.
//!
//! Usage:
//!   cargo run --example migrate_real_data -- <old_dir> <new_dir> [--expect-skip]
//!
//! Calls `data_migration::run(old_dir, new_dir)`, prints the outcome, and
//! exits 0 on success. Used to verify the migration against a duplicate of
//! Tyler's real data dir BEFORE cutting the production build. The
//! production code path calls `data_migration::run_for_app_data_dir` from
//! the setup hook — this example exercises the same pure `run` core with
//! caller-supplied paths so it can be pointed at synthetic test fixtures.
//!
//! NOT shipped in the production binary. Examples are compiled only when
//! invoked via `cargo run --example`.

use std::path::PathBuf;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!(
            "usage: cargo run --example migrate_real_data -- <old_dir> <new_dir> [--expect-skip]"
        );
        std::process::exit(2);
    }
    let old_dir = PathBuf::from(&args[1]);
    let new_dir = PathBuf::from(&args[2]);
    let expect_skip = args.iter().any(|a| a == "--expect-skip");

    println!(
        "migrate_real_data: old={} new={} expect_skip={}",
        old_dir.display(),
        new_dir.display(),
        expect_skip
    );

    let outcome = maxbot_lib::data_migration::run(&old_dir, &new_dir);
    println!("outcome: {outcome:?}");

    match outcome {
        maxbot_lib::data_migration::MigrationOutcome::NoOp => {
            println!("OK — no-op (old dir missing; fresh install)");
            std::process::exit(0);
        }
        maxbot_lib::data_migration::MigrationOutcome::Migrated { copied_entries } => {
            if expect_skip {
                eprintln!("FAIL — expected Skipped, got Migrated ({copied_entries} entries)");
                std::process::exit(1);
            }
            println!("OK — migrated {copied_entries} entries");
            std::process::exit(0);
        }
        maxbot_lib::data_migration::MigrationOutcome::Skipped => {
            if !expect_skip {
                eprintln!("FAIL — expected Migrated, got Skipped");
                std::process::exit(1);
            }
            println!("OK — skipped (new dir non-empty; idempotent)");
            std::process::exit(0);
        }
        maxbot_lib::data_migration::MigrationOutcome::Failed { error } => {
            eprintln!("FAIL — migration failed: {error}");
            std::process::exit(1);
        }
    }
}
