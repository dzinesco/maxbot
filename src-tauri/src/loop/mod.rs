//! Slice 1 — keep-alive loop module root.
//!
//! Sub-slices delivered as separate PRs onto the
//! `release/3.7.17-bundle-id-rename` branch:
//!
//! - `io`       — Sub-slice B: file I/O trait + `FileLoopIO` impl
//!                (this file).
//! - `daemon`   — Sub-slice C: supervisor (NOT in this PR).
//! - `sandbox`  — Sub-slice D: path bind for the passphrase DB
//!                (NOT in this PR).
//!
//! The supervisor (`daemon`) consumes the `LoopIO` trait from `io`; the
//! `sandbox` is independent and binds the passphrase DB to a path the
//! I/O trait refuses to read (defense in depth — see `io::assert_safe`).

pub mod io;
