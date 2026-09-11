//! Sub-slice C — sandbox bind (this PR).
//!
//! The keep-alive loop's three sub-slices:
//!   * Sub-slice A — supervisor (`loop::daemon`)
//!   * Sub-slice B — file I/O trait surface (`loop::io`)
//!   * Sub-slice C — sandbox bind (`loop::sandbox`)
//!
//! Each sub-slice declares its own module here. When all three
//! PRs land on `release/3.7.17-bundle-id-rename`, this file will
//! contain `pub mod daemon;`, `pub mod io;`, and `pub mod sandbox;`.
//!
//! For this PR (sub-slice C) we declare only `sandbox`. The other
//! sub-slices add their own `pub mod` lines when they merge.

pub mod sandbox;
