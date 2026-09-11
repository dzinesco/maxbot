//! Slice 1 — keep-alive loop module root.
//!
//! Sub-slices delivered as separate PRs onto the
//! `release/3.7.17-bundle-id-rename` branch:
//!
//! - `io`       — Sub-slice B: file I/O trait + `FileLoopIO` impl
//!                (canonical owner of the contract types and lock).
//! - `daemon`   — Sub-slice A: supervisor process, heartbeat, turn loop.
//!                Consumes `LoopIO` from `io` (no duplicate state).
//! - `sandbox`  — Sub-slice C: tool surface scoped to
//!                `<data_dir>/loop/sandbox/`. Fails closed on the
//!                passphrase DB.
//!
//! The supervisor (`daemon`) consumes the `LoopIO` trait from `io`; the
//! `sandbox` is independent and binds the passphrase DB to a path the
//! I/O trait refuses to read (defense in depth — see `io::assert_safe`).

pub mod daemon;
pub mod io;
pub mod sandbox;
